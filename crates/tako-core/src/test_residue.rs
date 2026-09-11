//! 検証プロセスが一時ディレクトリへ残す使い捨て dir の後始末（Issue #1296）
//!
//! ## 何が起きていたか
//!
//! #944 の隔離は「テストプロセスは本番の data dir ではなく
//! `<TMPDIR>/tako-test-data-<pid>` を使う」ところまでで、**消す仕掛けが無かった**。
//! macOS の `TMPDIR`（`/var/folders/…/T/`）は再起動しても消えないので、
//! `cargo test` を回すたびに 1 プロセス 1 dir が積もる
//! （実測 2026-09-11: `tako-test-data-*` が 2,188 件・`du` で 115 MB。TMPDIR 全体では 572 MB で、
//! Issue 本文の 549 MB は TMPDIR 全体の数字。`tako-agent-config-*` も同じ形で 784 件残っていた）。
//!
//! ## 直し方（2 段構え）
//!
//! 1. **終了時に自分の dir を消す**（[`arm_self_cleanup`]）。作った瞬間に武装するので
//!    「作る経路が消す経路を持つ」形になる。`libc::atexit` は `main` からの復帰でも
//!    `std::process::exit`（libtest が失敗時に呼ぶ）でも走る
//! 2. **起動時に残骸を掃く**（[`sweep_stale_on_start`]）。1 は SIGKILL / abort では
//!    走らないので、次のテストプロセスが pid の生死で判定して回収する
//!
//! ## 消してよいものの規則（**ここが唯一の安全弁**）
//!
//! 名前の一致だけで消すと、**並行して走っている別 worker の `cargo test`** の
//! data dir を消す（#625 と同じ事故クラス）。判定は [`judge`] の 1 実装に集約し、
//! 消すのは次のどちらかだけ:
//!
//! - 自分の pid のもの（終了時）
//! - pid が**生きていない**もの（起動時 / 手動）
//!
//! pid は再利用されるので「生きている」だけでは所有者を決められない。
//! **dir の作成時刻より後に始まったプロセス**はその dir を作れないので
//! `ReusedPid` と分類し、**消さずに見送る**（判定の材料が無い側へ倒す）。
//! 生死が読めないとき（権限・取得手段が無い）も `Unknown` = 触らない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 掃除の対象にする置き場の種別。名前は必ず `<prefix><pid>` の形
pub struct Kind {
    /// ディレクトリ名の接頭辞
    pub prefix: &'static str,
    /// 何を置く場所か（CLI の説明に出す）
    pub note: &'static str,
    /// テストプロセスの起動時に**自動で**掃く対象か。
    ///
    /// `tako-agent-config-` を自動に含めないのは、これを作るのが
    /// テストバイナリだけでなく**製品バイナリの検証起動**（`TAKO_ISOLATED` /
    /// `TAKO_SELF_TEST`）でもあるため（#1253）。製品の挙動を変えずに
    /// 直せる範囲を #1296 の射程とし、手動の掃除口からは消せるようにしてある
    pub auto: bool,
}

/// 種別の正本。CLI（`tako test-residue`）と自動掃除の両方がここを引く
pub const KINDS: &[Kind] = &[
    Kind {
        prefix: "tako-test-data-",
        note: "cargo test の data dir（#944 の隔離先）",
        auto: true,
    },
    Kind {
        prefix: "tako-agent-config-",
        note: "検証プロセスの外部エージェント設定・シェル履歴（#1253 の隔離先）",
        auto: false,
    },
];

/// 起動時の自動掃除で 1 プロセスが消す上限（残りは次のプロセスが引き継ぐ）。
/// 2,000 件の残骸を 1 プロセスに背負わせるとテストの起動が目に見えて遅くなる
pub const AUTO_SWEEP_BUDGET: usize = 512;

/// 起動時の自動掃除に使う時間の上限。`remove_dir_all` が遅い機（ネットワーク
/// ボリューム・ウイルス対策）でテストの起動を止めないための脱出口
pub const AUTO_SWEEP_DEADLINE: Duration = Duration::from_secs(5);

/// 「dir より後に始まったプロセス = pid 再利用」と言い切るための余裕。
///
/// プロセスの起動時刻は秒＋マイクロ秒（macOS の `proc_bsdinfo`）、dir の作成時刻は
/// ファイルシステムの粒度で、両者の丸めが逆向きに出ることがある。
/// **誤って「再利用」と分類しても消しはしない**（どちらも見送る）ので、
/// ここは分類の見た目の精度にだけ効く
pub const REUSE_SLACK: Duration = Duration::from_secs(2);

/// A/B 用の逃げ道（`TAKO_1296_LEGACY=1`）。#1296 の後始末を切って**旧挙動を再現**する
/// （作るだけで消さない）。番犬がこれを立てて「残骸が積もること」を実測する
pub fn legacy_1296() -> bool {
    matches!(
        std::env::var("TAKO_1296_LEGACY").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// 残骸の所有者（`<prefix><pid>` の pid が指すプロセス）の状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// 生きていない = この dir は残骸
    Dead,
    /// 生きている（起動時刻が取れたら `started`）
    Alive { started: Option<SystemTime> },
    /// 生死を判定できない（pid として不正・取得手段が無い）= 触らない
    Unknown,
}

/// 1 件の残骸をどう扱うか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 自分のプロセスのもの（終了時に自分で消す）
    Own,
    /// 所有プロセスが居ない = 消してよい
    Stale,
    /// 所有プロセスが生きている = 使用中
    InUse,
    /// pid は生きているが、その**プロセスは dir より後に始まっている**
    /// = pid の再利用。所有者が分からないので見送る
    ReusedPid,
    /// 生死が読めない = 見送る
    Unknown,
}

impl Verdict {
    /// 機械可読なコード（JSON / ログ用）
    pub fn code(self) -> &'static str {
        match self {
            Verdict::Own => "own",
            Verdict::Stale => "stale",
            Verdict::InUse => "in_use",
            Verdict::ReusedPid => "reused_pid",
            Verdict::Unknown => "unknown",
        }
    }

    /// 人が読む理由
    pub fn detail(self) -> &'static str {
        match self {
            Verdict::Own => "このプロセス自身の置き場（終了時に自分で消す）",
            Verdict::Stale => "所有プロセスが居ない = 消せる",
            Verdict::InUse => "所有プロセスが生きている = 使用中",
            Verdict::ReusedPid => "pid は生きているが dir より後に始まった = pid 再利用（見送る）",
            Verdict::Unknown => "所有プロセスの生死を判定できない（見送る）",
        }
    }

    /// 消してよいか。**`Stale` だけ**が真
    pub fn removable(self) -> bool {
        matches!(self, Verdict::Stale)
    }
}

/// 判定の純粋ロジック（`created` は dir の作成時刻）。
///
/// 実際の削除は [`sweep_in`] が**消す直前にもう一度**この判定を取り直す
/// （列挙してから消すまでの間に相手が生き返る / 別プロセスが同じ名前で作り直す
/// 可能性があるため）
pub fn judge(pid: u32, self_pid: u32, created: Option<SystemTime>, owner: Owner) -> Verdict {
    if pid == self_pid {
        return Verdict::Own;
    }
    match owner {
        Owner::Dead => Verdict::Stale,
        Owner::Unknown => Verdict::Unknown,
        Owner::Alive { started } => match (started, created) {
            (Some(started), Some(created)) if started > created + REUSE_SLACK => Verdict::ReusedPid,
            _ => Verdict::InUse,
        },
    }
}

/// 残骸 1 件
#[derive(Debug, Clone)]
pub struct Residue {
    pub path: PathBuf,
    /// どの種別か（[`KINDS`] の `prefix`）
    pub prefix: String,
    pub pid: u32,
    pub created: Option<SystemTime>,
    /// 配下の合計バイト数（`measure = false` の走査では 0）
    pub bytes: u64,
    /// 配下のファイル数（`measure = false` の走査では 0）
    pub files: usize,
    pub verdict: Verdict,
}

/// プロセスの生死と起動時刻を引く器。
///
/// Windows は在籍の列挙（Toolhelp）が 1 回で全 pid を返すので、**走査の前に
/// 1 度だけ**スナップショットを取る（2,000 件の残骸に対して pid ごとに
/// 列挙し直すと O(n²) になる）
pub struct OwnerProbe {
    #[cfg(windows)]
    live: std::collections::HashSet<u32>,
}

impl Default for OwnerProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnerProbe {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            live: crate::platform::procinfo::snapshot()
                .iter()
                .map(|p| p.pid)
                .collect(),
        }
    }

    /// この pid のプロセスの状態。**「生きていない」と言い切れるときだけ [`Owner::Dead`]**
    pub fn owner(&self, pid: u32) -> Owner {
        if pid == 0 || pid > i32::MAX as u32 {
            // pid として使われない値 = 名前が壊れている。触らない
            return Owner::Unknown;
        }
        #[cfg(windows)]
        let alive = self.live.contains(&pid);
        #[cfg(not(windows))]
        let alive = crate::platform::process::pid_alive(pid);
        if !alive {
            return Owner::Dead;
        }
        Owner::Alive {
            // 秒精度でよい（比べる相手は dir の作成時刻で、余裕は REUSE_SLACK が持つ）
            started: crate::platform::procinfo::start_time_unix(pid)
                .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }
}

/// 一時ディレクトリ配下の残骸を列挙する。
///
/// `measure` が真なら配下のバイト数とファイル数も数える（CLI の dry-run 用。
/// 自動掃除では数えない = 2,000 件の walk を毎回のテスト起動で払わない）
pub fn scan_in(
    dir: &Path,
    prefixes: &[&str],
    self_pid: u32,
    probe: &OwnerProbe,
    measure: bool,
) -> Vec<Residue> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(prefix) = prefixes.iter().find(|p| name.starts_with(**p)) else {
            continue;
        };
        let Ok(pid) = name[prefix.len()..].parse::<u32>() else {
            continue; // `tako-test-data-<pid>` 以外の形（人が作った控え等）は触らない
        };
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => meta,
            // symlink / 通常ファイルは対象外（`remove_dir_all` の対象を dir に限る）
            _ => continue,
        };
        let created = meta.created().ok();
        let (bytes, files) = if measure { measure_dir(&path) } else { (0, 0) };
        let verdict = judge(pid, self_pid, created, probe.owner(pid));
        out.push(Residue {
            path,
            prefix: (*prefix).to_string(),
            pid,
            created,
            bytes,
            files,
            verdict,
        });
    }
    out.sort_by_key(|r| r.pid);
    out
}

/// 配下の合計バイト数とファイル数（symlink は辿らない）
fn measure_dir(dir: &Path) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut files = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() && !meta.is_symlink() {
                stack.push(entry.path());
            } else {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// 掃除の結果
#[derive(Debug, Clone, Default)]
pub struct SweepOutcome {
    /// 列挙した件数
    pub scanned: usize,
    /// 消した件数（`apply = false` なら常に 0）
    pub removed: usize,
    /// 消した合計バイト数（`measure` した走査でのみ意味を持つ）
    pub removed_bytes: u64,
    /// 消そうとして失敗した件数
    pub failed: usize,
    /// 見送った件数（判定コード別）
    pub skipped: BTreeMap<String, usize>,
    /// 消せなかったときの最初の理由（権限が無い等）。**黙って 0 件にしない**
    pub last_error: Option<String>,
    /// 実際に消したか（`--apply`）
    pub applied: bool,
    /// 予算（件数 / 時間）で打ち切ったか
    pub truncated: bool,
}

impl SweepOutcome {
    /// persist.log へ 1 行で残す形
    pub fn log_line(&self) -> String {
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|(code, n)| format!("{code}={n}"))
            .collect();
        format!(
            "test-residue: scanned={} removed={} bytes={} failed={} applied={} truncated={} skipped[{}]{}",
            self.scanned,
            self.removed,
            self.removed_bytes,
            self.failed,
            self.applied,
            self.truncated,
            skipped.join(" "),
            self.last_error
                .as_deref()
                .map(|e| format!(" error={e}"))
                .unwrap_or_default()
        )
    }
}

/// 列挙 → 判定 → （`apply` なら）削除。
///
/// **消す直前に判定を取り直す**のが要点（列挙は「開始時のスナップショット」なので、
/// その後に pid が再利用されて同じ名前の dir が作り直されると、生きている
/// プロセスの置き場を消しかねない）。取り直すのは 2 つ:
///
/// - 所有 pid がまだ死んでいるか
/// - dir の作成時刻が列挙時と同じか（別プロセスが作り直していたら変わる）
pub fn sweep_in(
    dir: &Path,
    prefixes: &[&str],
    self_pid: u32,
    apply: bool,
    budget: usize,
    deadline: Option<std::time::Instant>,
    measure: bool,
) -> (Vec<Residue>, SweepOutcome) {
    let probe = OwnerProbe::new();
    let residues = scan_in(dir, prefixes, self_pid, &probe, measure);
    let mut out = SweepOutcome {
        scanned: residues.len(),
        applied: apply,
        ..Default::default()
    };
    for res in &residues {
        if !res.verdict.removable() {
            *out.skipped
                .entry(res.verdict.code().to_string())
                .or_insert(0) += 1;
            continue;
        }
        if !apply {
            continue;
        }
        if out.removed + out.failed >= budget
            || deadline.is_some_and(|d| std::time::Instant::now() >= d)
        {
            out.truncated = true;
            break;
        }
        match remove_if_still_stale(res, self_pid, &|pid| OwnerProbe::new().owner(pid)) {
            Ok(true) => {
                out.removed += 1;
                out.removed_bytes += res.bytes;
            }
            // 消す直前の再判定で「消してよくない」に変わった = 見送り
            Ok(false) => *out.skipped.entry("recheck".to_string()).or_insert(0) += 1,
            Err(e) => {
                out.failed += 1;
                out.last_error.get_or_insert_with(|| e.to_string());
            }
        }
    }
    (residues, out)
}

/// 消す直前の再判定つき削除。消したら `Ok(true)`、見送ったら `Ok(false)`。
///
/// `probe` を引数で受けるのは、**生死を「いま」取り直す**ことが要点だから
/// （列挙時のスナップショットのまま消すと、その後に pid が再利用されて
/// 作り直された置き場を巻き込む）。テストからは決め打ちの状態を注入する
fn remove_if_still_stale(
    res: &Residue,
    self_pid: u32,
    probe: &dyn Fn(u32) -> Owner,
) -> std::io::Result<bool> {
    if judge(res.pid, self_pid, res.created, probe(res.pid)) != Verdict::Stale {
        return Ok(false);
    }
    // 作成時刻が列挙時と違う = 別プロセスが同じ名前で作り直した
    let created = std::fs::symlink_metadata(&res.path)
        .ok()
        .filter(|m| m.is_dir())
        .and_then(|m| m.created().ok());
    if created != res.created {
        return Ok(false);
    }
    match std::fs::remove_dir_all(&res.path) {
        Ok(()) => Ok(true),
        // 既に他のプロセスが消していた = 目的は達成されている
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

/// 手動の掃除口（`tako test-residue` / MCP `tako_test_residue`）の 1 実装。
///
/// 一時ディレクトリ全体を [`KINDS`] 全種別で走査し、サイズも数える
/// （**自動掃除とは違って全件・件数の上限なし**。ユーザーが明示的に叩く経路なので、
/// 2,000 件の walk を払ってでも「何が何バイトあるか」を出すのが役目）
pub fn sweep_temp(apply: bool) -> (PathBuf, Vec<Residue>, SweepOutcome) {
    let temp = std::env::temp_dir();
    let prefixes: Vec<&str> = KINDS.iter().map(|k| k.prefix).collect();
    let (items, outcome) = sweep_in(
        &temp,
        &prefixes,
        std::process::id(),
        apply,
        usize::MAX,
        None,
        true,
    );
    (temp, items, outcome)
}

/// 掃除の結果を JSON にする（CLI の `--json` と MCP の応答が**同じ 1 実装**）
pub fn report_json(temp: &Path, items: &[Residue], outcome: &SweepOutcome) -> serde_json::Value {
    let kinds: Vec<serde_json::Value> = KINDS
        .iter()
        .map(|kind| {
            let mine: Vec<&Residue> = items.iter().filter(|r| r.prefix == kind.prefix).collect();
            serde_json::json!({
                "prefix": kind.prefix,
                "note": kind.note,
                "auto_sweep": kind.auto,
                "total": mine.len(),
                "bytes": mine.iter().map(|r| r.bytes).sum::<u64>(),
                "removable": mine.iter().filter(|r| r.verdict.removable()).count(),
            })
        })
        .collect();
    let entries: Vec<serde_json::Value> = items
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.path.file_name().map(|n| n.to_string_lossy().to_string()),
                "pid": r.pid,
                "verdict": r.verdict.code(),
                "detail": r.verdict.detail(),
                "bytes": r.bytes,
                "files": r.files,
            })
        })
        .collect();
    serde_json::json!({
        "temp_dir": temp.to_string_lossy(),
        "applied": outcome.applied,
        "summary": {
            "total": outcome.scanned,
            "bytes": items.iter().map(|r| r.bytes).sum::<u64>(),
            "removable": items.iter().filter(|r| r.verdict.removable()).count(),
            "removable_bytes": items
                .iter()
                .filter(|r| r.verdict.removable())
                .map(|r| r.bytes)
                .sum::<u64>(),
            "removed": outcome.removed,
            "removed_bytes": outcome.removed_bytes,
            "failed": outcome.failed,
            "last_error": outcome.last_error,
            "truncated": outcome.truncated,
            "skipped": outcome.skipped,
        },
        "kinds": kinds,
        "entries": entries,
    })
}

/// このプロセスが作った使い捨て dir を**終了時に消す**（#1296 の案 1）。
///
/// `libc::atexit` に登録するので、`main` からの復帰でも
/// `std::process::exit`（libtest が失敗時に呼ぶ）でも走る。
/// SIGKILL / abort では走らないので [`sweep_stale_on_start`] と併用する
pub fn arm_self_cleanup(dir: &Path) {
    if legacy_1296() {
        return; // A/B: 修正前は作るだけで消さなかった
    }
    let mut own = own_dirs().lock().unwrap_or_else(|e| e.into_inner());
    if own.iter().any(|p| p == dir) {
        return;
    }
    own.push(dir.to_path_buf());
    drop(own);
    static ARMED: std::sync::Once = std::sync::Once::new();
    ARMED.call_once(|| {
        // SAFETY: 引数は `extern "C" fn()` そのもので、ハンドラは
        // 静的なパス一覧しか触らない（プロセス終了時に 1 度だけ呼ばれる）
        unsafe {
            libc::atexit(remove_own_dirs);
        }
    });
}

fn own_dirs() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    static OWN: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
    OWN.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// `atexit` から呼ばれる後始末。**失敗しても何も言わない**
/// （プロセスは既に終了処理に入っていて、診断の出口が無い）
extern "C" fn remove_own_dirs() {
    let own = own_dirs().lock().unwrap_or_else(|e| e.into_inner());
    for dir in own.iter() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// テストプロセスの起動時に 1 度だけ走る残骸の掃除（#1296 の案 2）。
///
/// 対象は [`KINDS`] の `auto` が真なものだけ。製品の経路からは呼ばれない
/// （呼び出し元は [`crate::paths::data_dir`] のテスト分岐だけ）が、
/// 誤配線に備えてここでも [`crate::paths::is_test_process`] を確かめる
pub fn sweep_stale_on_start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if legacy_1296() || !crate::paths::is_test_process() {
            return;
        }
        let prefixes: Vec<&str> = KINDS.iter().filter(|k| k.auto).map(|k| k.prefix).collect();
        let deadline = std::time::Instant::now() + AUTO_SWEEP_DEADLINE;
        let (_, outcome) = sweep_in(
            &std::env::temp_dir(),
            &prefixes,
            std::process::id(),
            true,
            AUTO_SWEEP_BUDGET,
            Some(deadline),
            false,
        );
        // 診断ログには出さない（テストプロセスの data dir は使い捨てで、
        // ここで persist.log を作ると「テストは本番へ書かない」検査のノイズになる）
        let _ = outcome;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELF_PID: u32 = 4242;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn 自分のpidの置き場は掃除の対象にしない() {
        let v = judge(SELF_PID, SELF_PID, Some(t(100)), Owner::Dead);
        assert_eq!(v, Verdict::Own);
        assert!(!v.removable());
    }

    #[test]
    fn pidが居なければ消せる() {
        let v = judge(7, SELF_PID, Some(t(100)), Owner::Dead);
        assert_eq!(v, Verdict::Stale);
        assert!(v.removable());
    }

    #[test]
    fn 生きているプロセスの置き場は消さない() {
        // dir（100 秒）より前に始まったプロセス（90 秒）= 作った本人でありうる
        let v = judge(
            7,
            SELF_PID,
            Some(t(100)),
            Owner::Alive {
                started: Some(t(90)),
            },
        );
        assert_eq!(v, Verdict::InUse);
        assert!(!v.removable());
    }

    /// #1296 の要点: **pid が生きていても所有者とは限らない**。
    /// dir より後に始まったプロセスはその dir を作れないので pid 再利用と分かるが、
    /// 所有者が分からない以上**消さずに見送る**
    #[test]
    fn pid再利用は起動時刻で見分けて見送る() {
        let v = judge(
            7,
            SELF_PID,
            Some(t(100)),
            Owner::Alive {
                started: Some(t(500)),
            },
        );
        assert_eq!(v, Verdict::ReusedPid, "{}", v.detail());
        assert!(!v.removable(), "再利用と分かっても消してはいけない");
    }

    #[test]
    fn 起動時刻が取れなければ使用中として見送る() {
        let v = judge(7, SELF_PID, Some(t(100)), Owner::Alive { started: None });
        assert_eq!(v, Verdict::InUse);
        let v = judge(
            7,
            SELF_PID,
            None,
            Owner::Alive {
                started: Some(t(500)),
            },
        );
        assert_eq!(
            v,
            Verdict::InUse,
            "dir の作成時刻が読めなければ比較できない"
        );
    }

    #[test]
    fn 丸めの差では再利用と言わない() {
        // 1 秒だけ後 = 粒度の差の範囲。REUSE_SLACK を超えないと再利用と言わない
        let v = judge(
            7,
            SELF_PID,
            Some(t(100)),
            Owner::Alive {
                started: Some(t(101)),
            },
        );
        assert_eq!(v, Verdict::InUse);
    }

    #[test]
    fn 生死が読めなければ触らない() {
        assert_eq!(
            judge(7, SELF_PID, Some(t(100)), Owner::Unknown),
            Verdict::Unknown
        );
        let probe = OwnerProbe::new();
        assert_eq!(probe.owner(0), Owner::Unknown, "0 はプロセスグループの指定");
        assert_eq!(probe.owner(u32::MAX), Owner::Unknown, "pid の範囲外");
    }

    /// 使い捨ての置き場を 1 つ作って `Residue` を組み立てる
    fn planted(tag: &str, pid: u32) -> (PathBuf, Residue) {
        let dir = std::env::temp_dir().join(format!(
            "tako-1296-unit-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join(format!("tako-test-data-{pid}"));
        std::fs::create_dir_all(&path).expect("置き場を作れる");
        std::fs::write(path.join("persist.log"), b"x").expect("中身を書ける");
        let created = std::fs::symlink_metadata(&path)
            .ok()
            .and_then(|m| m.created().ok());
        let residue = Residue {
            path: path.clone(),
            prefix: "tako-test-data-".to_string(),
            pid,
            created,
            bytes: 1,
            files: 1,
            verdict: Verdict::Stale,
        };
        (dir, residue)
    }

    /// 列挙してから消すまでの間に**別プロセスが同じ名前で作り直した**ら消さない。
    /// 作成時刻が列挙時と変わることで見分ける
    #[test]
    fn 消す直前に作り直された置き場は消さない() {
        let (root, residue) = planted("recreate", 7);
        let stale = |_: u32| Owner::Dead;

        // 作成時刻が食い違う = 別物とみなして見送る
        let mut impostor = residue.clone();
        impostor.created = Some(SystemTime::UNIX_EPOCH);
        assert!(!remove_if_still_stale(&impostor, SELF_PID, &stale).expect("io"));
        assert!(residue.path.exists(), "見送ったのに消えている");

        // 列挙時のままなら消す
        assert!(remove_if_still_stale(&residue, SELF_PID, &stale).expect("io"));
        assert!(!residue.path.exists(), "消えていない");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 消す直前に pid が生き返っていたら消さない（再判定が効いていること）
    #[test]
    fn 消す直前に所有者が生きていたら消さない() {
        let (root, residue) = planted("revived", 8);
        let alive = |_: u32| Owner::Alive { started: None };
        assert!(!remove_if_still_stale(&residue, SELF_PID, &alive).expect("io"));
        assert!(residue.path.exists(), "生きている所有者の置き場を消した");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 残骸が 0 件でも落ちない（`--apply` でも何も起きない）
    #[test]
    fn 残骸が無ければ何も起きない() {
        let root = std::env::temp_dir().join(format!(
            "tako-1296-unit-empty-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).expect("作れる");
        let (items, outcome) = sweep_in(
            &root,
            &["tako-test-data-"],
            SELF_PID,
            true,
            AUTO_SWEEP_BUDGET,
            None,
            true,
        );
        assert!(items.is_empty());
        assert_eq!(outcome.removed, 0);
        assert_eq!(outcome.failed, 0);
        assert!(!outcome.truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 読めないディレクトリを渡しても落ちない（走査は空を返す）
    #[test]
    fn 走査できない置き場では空を返す() {
        let missing = std::env::temp_dir().join("tako-1296-unit-missing-このパスは存在しない");
        let (items, outcome) = sweep_in(
            &missing,
            &["tako-test-data-"],
            SELF_PID,
            true,
            AUTO_SWEEP_BUDGET,
            None,
            true,
        );
        assert!(items.is_empty());
        assert_eq!(outcome.scanned, 0);
    }

    #[test]
    fn 自分のプロセスは生きていると読める() {
        let probe = OwnerProbe::new();
        assert!(
            matches!(probe.owner(std::process::id()), Owner::Alive { .. }),
            "自分自身が Dead に見える = 生死判定が壊れている"
        );
    }
}
