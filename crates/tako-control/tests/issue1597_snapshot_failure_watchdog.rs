//! **#1597 の番犬**: 「在籍の列挙に失敗した」を「そのプロセスが居ない」と読まない。
//!
//! ## なぜ止めるのか
//!
//! Windows の生死判定は Toolhelp の在籍（`procinfo::snapshot`）で決まる。列挙が
//! 失敗した回は**空の `Vec`** になり、そのまま読むと**全 pid が「載っていない」=
//! 不在**に見える。`test_residue::OwnerProbe` はそれを `Owner::Dead` と読み、
//! `sweep_in` が**並行して走っている別 worker の test dir まで消しに行く**
//! （`test_residue.rs` 自身が「唯一の安全弁」と呼ぶ #625 の事故クラス）。
//!
//! 材料が無いときに倒れる先は 1 つしかない。**「消せない・撃てない」側**:
//!
//! - `procinfo::snapshot_checked` が `None` を返す（空の在籍表も失敗として畳む。
//!   呼び出したプロセス自身が必ず載るので「成功したが 0 件」は在り得ない）
//! - `platform::process::pid_alive` の Windows 腕は**「居る」**と答える
//! - `test_residue::OwnerProbe` は `Owner::Unknown` = 見送る
//!
//! ## 何を固定するか
//!
//! 規則そのもの（`None` / 空 → 「居る」）は `platform::process` と `test_residue` の
//! 単体テストが持ち、実プロセスでの一巡は `tako-core` の
//! `tests/test_data_residue.rs`（`TAKO_1597_ROSTER` の A/B）が測る。
//! ここが止めるのは**構造**で、1 行で戻せてしまう形が 5 通りある:
//!
//! 1. [`失敗を畳まない口が境界に在る`] — `snapshot()` が lossy 版でなくなる
//! 2. [`windowsの在籍列挙は空を失敗として畳む`] — Toolhelp の 0 件が素通りする
//! 3. [`pid_aliveのwindows腕は失敗を居る側へ倒す`] — lossy な `snapshot()` へ戻る
//! 4. [`owner_probeは列挙の失敗を不在と読まない`] — `alive` が `bool` へ戻る
//! 5. [`rosterは空の在籍表を失敗として畳む`] — 空を `Listed` として受け入れる
//!
//! 生死判定の**寄せ先**（境界 1 実装であること）は #1557 の番犬が見ている。
//! こちらは「その境界が材料の無い回に何と答えるか」だけを見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const PROCINFO: &str = "crates/tako-core/src/platform/procinfo.rs";
const PROCESS: &str = "crates/tako-core/src/platform/process.rs";
const RESIDUE: &str = "crates/tako-core/src/test_residue.rs";

/// 失敗と不在を分ける口。綴りはこの 1 か所に持つ
const CHECKED: &str = "snapshot_checked";
/// 失敗を空へ畳む lossy 版。**生死の判断に使ってはいけない**
const LOSSY: &str = "snapshot()";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」（#1557 の番犬と同じ作法）。
///
/// 禁じている綴り（lossy な `snapshot()`）は**製品側の doc コメントに
/// 「こう書いてはいけない」として実在する**ので、コメントと文字列は潰してから見る。
/// バイト長と行番号は保たれるので `file:line` の名指しがそのまま使える
fn code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    let production = production_range::production(&src, rel);
    production_range::code_view_of(&production)
}

/// シグネチャに一致する関数の `(行番号, 本文)` を**全件**返す。
///
/// 同じ綴りの関数が cfg 違いで複数ある（`pub fn pid_alive` は境界と 2 つの
/// `mod imp` に、`snapshot_checked` は Windows と非 Windows に居る）。
/// **最初の 1 件だけ見ると、直したい腕と別の腕を検査して緑になる**
fn fn_bodies(source: &str, signature: &str) -> Vec<(usize, String)> {
    let sig_line = signature.trim_start_matches('\n');
    let indent = " ".repeat(sig_line.len() - sig_line.trim_start().len());
    let close = format!("\n{indent}}}");
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = source[cursor..].find(signature) {
        let start = cursor + at;
        cursor = start + signature.len();
        let line = source[..start].lines().count() + 1;
        let end = source[start..]
            .find(&close)
            .map(|i| start + i)
            .unwrap_or(source.len());
        out.push((line, source[start..end].to_string()));
    }
    out
}

/// `needle` を含む腕を 1 つだけ選ぶ（cfg 違いの同名関数から目当ての実装を取る）
fn arm(bodies: &[(usize, String)], needle: &str, rel: &str, what: &str) -> (usize, String) {
    let mut hit = bodies.iter().filter(|(_, body)| body.contains(needle));
    let found = hit.next().unwrap_or_else(|| {
        panic!(
            "{rel} の {what} が見つからない（`{needle}` を含む腕が無い。\n\
             改名・作り替えをしたら番犬も直すこと。候補 {} 件: {:?}）",
            bodies.len(),
            bodies.iter().map(|(l, _)| *l).collect::<Vec<_>>()
        )
    });
    (found.0, found.1.clone())
}

// --------------------------------------------------------------- 検査本体
// 注入テストからも呼ぶので、読むソースは引数で受ける

fn check_boundary_has_checked(source: &str, rel: &str) {
    let bodies = fn_bodies(source, "\npub fn snapshot() -> Vec<ProcEntry> {");
    let (line, body) = bodies
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("{rel} に `pub fn snapshot()` が無い（改名したら番犬も直す）"));
    assert!(
        body.contains(CHECKED),
        "{rel}:{line} `snapshot()` が `{CHECKED}` の lossy 版になっていない（#1597）。\n\
         失敗を `None` で返す口が境界に無いと、呼び出し側は「取れなかった」と\n\
         「1 件も居ない」を区別できない。本文:\n{body}"
    );
}

fn check_windows_snapshot_folds_empty(source: &str, rel: &str) {
    let bodies = fn_bodies(
        source,
        "\n    pub(super) fn snapshot_checked() -> Option<Vec<ProcEntry>> {",
    );
    let (line, body) = arm(
        &bodies,
        "CreateToolhelp32Snapshot",
        rel,
        "Windows の在籍列挙",
    );
    assert!(
        body.contains("is_empty()"),
        "{rel}:{line} Windows の `{CHECKED}` が 0 件を失敗として畳んでいない（#1597）。\n\
         呼び出したプロセス自身が必ず載るので「成功したが 0 件」の在籍表は在り得ない。\n\
         そのまま返すと全 pid が不在に見える。本文:\n{body}"
    );
}

fn check_pid_alive_windows_arm(source: &str, rel: &str) {
    let bodies = fn_bodies(source, "\n    pub fn pid_alive(pid: u32) -> bool {");
    let (line, body) = arm(&bodies, "procinfo", rel, "pid_alive の Windows 腕");
    assert!(
        body.contains(CHECKED),
        "{rel}:{line} `pid_alive` の Windows 腕が `{CHECKED}` を通っていない（#1597）。\n\
         lossy な `snapshot()` で読むと Toolhelp が失敗した回に全 pid が「居ない」= \n\
         残骸に見え、掃除と停止が生きているプロセスを巻き込む。本文:\n{body}"
    );
    assert!(
        !body.contains(LOSSY),
        "{rel}:{line} `pid_alive` の Windows 腕に lossy な `{LOSSY}` が戻っている（#1597）。\n\
         本文:\n{body}"
    );
    assert!(
        body.contains("alive_in_snapshot"),
        "{rel}:{line} `pid_alive` の Windows 腕が読み方を自前に持ち直している（#1597）。\n\
         「材料が無い回は居る側へ倒す」は `alive_in_snapshot` の 1 実装に置く\n\
         （cfg で割れていないので macOS からも検証できる）。本文:\n{body}"
    );
}

fn check_owner_probe_keeps_unknown(source: &str, rel: &str) {
    let bodies = fn_bodies(source, "\n    fn alive(&self, pid: u32) -> Option<bool> {");
    if bodies.is_empty() {
        // 3 値が潰れている。**どの行か**まで名指す（見つからないとだけ言うと、
        // 番犬が壊れたのか製品が退行したのかを読み手が切り分けられない）
        let line = source
            .find("\n    fn alive(")
            .map(|at| source[..at].lines().count() + 1)
            .unwrap_or(0);
        panic!(
            "{rel}:{line} `OwnerProbe::alive` が `Option<bool>` を返していない（#1597）。\n\
             `bool` へ戻すと「答えられない」を `false`（= 不在）へ潰すことになり、\n\
             在籍の列挙に失敗した回に生きている置き場を消す"
        );
    }
    let bodies = fn_bodies(source, "\n    pub fn owner(&self, pid: u32) -> Owner {");
    let (oline, obody) = bodies
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("{rel} に `OwnerProbe::owner` が無い（改名したら番犬も直す）"));
    let flat = obody.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("None => Owner::Unknown"),
        "{rel}:{oline} `OwnerProbe::owner` が「生死を答えられない」を \
         `Owner::Unknown` へ倒していない（#1597）。\n\
         材料が無い回を `Dead` と読むと `sweep_in` が生きている置き場を消す。本文:\n{obody}"
    );
}

fn check_roster_folds_empty(source: &str, rel: &str) {
    let bodies = fn_bodies(
        source,
        "\n    fn from_snapshot(procs: Option<Vec<crate::platform::procinfo::ProcEntry>>) -> Self {",
    );
    let (line, body) = bodies.first().cloned().unwrap_or_else(|| {
        panic!("{rel} に `Roster::from_snapshot` が無い（改名したら番犬も直す）")
    });
    assert!(
        body.contains("is_empty()") && body.contains("Roster::Unavailable"),
        "{rel}:{line} `Roster::from_snapshot` が空の在籍表を失敗として畳んでいない（#1597）。\n\
         `Listed(空集合)` は「全員不在」と同義で、そのまま #1597 の事故になる。本文:\n{body}"
    );
    let (cline, cbody) = fn_bodies(source, "\nfn current_roster() -> Roster {")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("{rel} に `current_roster` が無い（改名したら番犬も直す）"));
    assert!(
        cbody.contains(CHECKED) && !cbody.contains(LOSSY),
        "{rel}:{cline} `current_roster` が lossy な `{LOSSY}` で材料を採っている（#1597）。\n\
         失敗が空の在籍表になり、全 pid が不在に見える。本文:\n{cbody}"
    );
}

// --------------------------------------------------------------- テスト

#[test]
fn 失敗を畳まない口が境界に在る() {
    check_boundary_has_checked(&code(PROCINFO), PROCINFO);
}

#[test]
fn windowsの在籍列挙は空を失敗として畳む() {
    check_windows_snapshot_folds_empty(&code(PROCINFO), PROCINFO);
}

#[test]
fn pid_aliveのwindows腕は失敗を居る側へ倒す() {
    check_pid_alive_windows_arm(&code(PROCESS), PROCESS);
}

#[test]
fn owner_probeは列挙の失敗を不在と読まない() {
    check_owner_probe_keeps_unknown(&code(RESIDUE), RESIDUE);
}

#[test]
fn rosterは空の在籍表を失敗として畳む() {
    check_roster_folds_empty(&code(RESIDUE), RESIDUE);
}

/// 規則そのものを**呼んで**確かめる（Windows の腕の純粋部分は cfg で割れていない
/// ので、この機からそのまま回せる）。構造の検査が全部緑でも、ここが逆向きなら落ちる
#[test]
fn 材料が無い回は居る側へ倒れる() {
    use tako_core::platform::process::alive_in_snapshot;
    use tako_core::platform::procinfo::ProcEntry;

    let procs = [ProcEntry {
        pid: 7,
        ppid: 1,
        name: "p7.exe".to_string(),
    }];
    assert!(alive_in_snapshot(Some(&procs), 7), "載っている pid は居る");
    assert!(
        !alive_in_snapshot(Some(&procs), 8),
        "列挙できた回は「載っていない = 居ない」と言える"
    );
    assert!(
        alive_in_snapshot(None, 8),
        "列挙に失敗した回に「居ない」と答えると全 pid が残骸に見える（#1597）"
    );
    assert!(
        alive_in_snapshot(Some(&[]), 8),
        "空の在籍表は列挙の失敗（自分自身が必ず載るので 0 件は在り得ない）"
    );
    assert_eq!(
        tako_core::platform::procinfo::snapshot_supported(),
        cfg!(windows),
        "列挙の手段を持つ OS の宣言が実装とズレている"
    );
}

/// 番犬に検出力があること: 直前の 5 本が**壊れた形を実際に落とす**。
///
/// 製品ソースを書き換える代わりに、壊れた形の断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 壊れた形を実際に落とす() {
    // 1. 境界が lossy 版しか持たない（#1597 以前）
    let legacy_boundary = "
pub fn snapshot() -> Vec<ProcEntry> {
    imp::snapshot()
}
";
    assert!(
        std::panic::catch_unwind(|| check_boundary_has_checked(legacy_boundary, "注入")).is_err(),
        "失敗を畳まない口が無い形を見逃した"
    );

    // 2. Toolhelp の 0 件をそのまま返す
    let legacy_win_snapshot = "
    pub(super) fn snapshot_checked() -> Option<Vec<ProcEntry>> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() {
                return None;
            }
            CloseHandle(snap);
        }
        Some(out)
    }
";
    assert!(
        std::panic::catch_unwind(|| check_windows_snapshot_folds_empty(
            legacy_win_snapshot,
            "注入"
        ))
        .is_err(),
        "0 件を成功として返す形を見逃した"
    );

    // 3. `pid_alive` の Windows 腕が lossy な列挙で読む（#1597 以前の実物）
    let legacy_pid_alive = "
    pub fn pid_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        super::super::procinfo::snapshot()
            .iter()
            .any(|p| p.pid == pid)
    }
";
    assert!(
        std::panic::catch_unwind(|| check_pid_alive_windows_arm(legacy_pid_alive, "注入")).is_err(),
        "lossy な列挙で生死を読む形を見逃した"
    );
    // 非 Windows の腕（procinfo を見ない）だけでは腕が見つからず、黙って緑にならない
    let unix_only = "
    pub fn pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }
";
    assert!(
        std::panic::catch_unwind(|| check_pid_alive_windows_arm(unix_only, "注入")).is_err(),
        "Windows の腕が消えた形を見逃した"
    );

    // 4. `alive` が `bool` へ戻る / `owner` が材料の無い回を `Dead` と読む
    let legacy_probe = "
    pub fn owner(&self, pid: u32) -> Owner {
        if !self.alive(pid) {
            return Owner::Dead;
        }
        Owner::Alive { started: None }
    }

    fn alive(&self, pid: u32) -> bool {
        self.live.contains(&pid)
    }
";
    assert!(
        std::panic::catch_unwind(|| check_owner_probe_keeps_unknown(legacy_probe, "注入")).is_err(),
        "3 値を bool へ潰した形を見逃した"
    );

    // 5. 空の在籍表を `Listed` として受け入れる / lossy な列挙で材料を採る
    let legacy_roster = "
    fn from_snapshot(procs: Option<Vec<crate::platform::procinfo::ProcEntry>>) -> Self {
        match procs {
            Some(procs) => Roster::Listed(procs.iter().map(|p| p.pid).collect()),
            None => Roster::Unavailable,
        }
    }

fn current_roster() -> Roster {
    Roster::Listed(
        crate::platform::procinfo::snapshot()
            .iter()
            .map(|p| p.pid)
            .collect(),
    )
}
";
    assert!(
        std::panic::catch_unwind(|| check_roster_folds_empty(legacy_roster, "注入")).is_err(),
        "空の在籍表を受け入れる形を見逃した"
    );
}
