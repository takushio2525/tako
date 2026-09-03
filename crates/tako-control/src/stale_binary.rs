//! stale_binary — 長生きセッションの claude バイナリ鮮度検知（Issue #498）
//!
//! ペイン生成時に解決した claude の実バイナリパスを記録し、稼働中プロセスは
//! 境界 B5（[`tako_core::platform::procinfo::image_path`]）で取得、
//! `~/.local/bin/claude` の現在の解決先と突き合わせる。
//! 差異があれば stale と判定し、UI バナー + CLI/MCP で通知・張り直しを提供する。
//!
//! ## 「差異」の形が OS で違う（#936）
//!
//! - macOS: ランチャは **symlink** で、`proc_pidpath` は実体（`versions/<版>`）を
//!   返す。自己更新は symlink の張り替えなので、**解決先のパスが変わる**
//! - Windows: ランチャは `…\.local\bin\claude.exe` の**実体のコピー**
//!   （実測 2026-09-04: symlink でもハードリンクでもなく、`versions\<版>` と
//!   バイト一致する別ファイル）。自己更新は
//!   **旧 exe を `claude.exe.old.<ts>` へ改名 → 新 exe を同じ名前で設置**なので、
//!   古いプロセスの実行ファイルパスが `…\claude.exe.old.<ts>` へ変わる
//!   （`QueryFullProcessImageNameW` が改名を反映する = 実測）。**比較の形は
//!   両 OS で同じ**（実行中のパス ≠ 現在のパス）まま成立する
//!
//! どちらも比較する 2 つのパスを**同じ正規化**で作らないと成り立たない。
//! 現在側は [`tako_core::platform::path::canonicalize`]（verbatim prefix を
//! 剥がす。#970）を通し、実行中側は境界が Win32 形式で返す

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// ペインごとの stale 判定情報
#[derive(Debug, Clone)]
pub struct PaneClaudeInfo {
    /// ペイン生成時に解決した claude バイナリの実パス（symlink 解決済み）
    pub spawned_binary: PathBuf,
    /// ペイン内 claude プロセスの PID（判定時に取得）
    pub pid: Option<u32>,
    /// バナーをユーザーが閉じたか（閉じても次回検知で再提示される）
    pub dismissed: bool,
}

/// stale 判定の結果
#[derive(Debug, Clone)]
pub struct StaleStatus {
    /// stale か否か
    pub stale: bool,
    /// 起動時のバイナリパス
    pub spawned_binary: String,
    /// 現在の `claude` symlink が指す実パス
    pub current_binary: String,
    /// 起動時バイナリから抽出したバージョン（取得できなければ空）
    pub spawned_version: String,
    /// 現在バイナリから抽出したバージョン（取得できなければ空）
    pub current_version: String,
    /// ペイン内 claude の PID
    pub pid: Option<u32>,
}

/// グローバルな stale 判定キャッシュ（5 秒 TTL）
static CACHE: Mutex<Option<CachedResult>> = Mutex::new(None);

struct CachedResult {
    current_binary: PathBuf,
    current_version: String,
    at: std::time::Instant,
}

const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// `~/.local/bin/claude`（または PATH 上の claude）の symlink を解決し実パスを返す
pub fn resolve_current_claude_binary() -> Option<PathBuf> {
    if let Some(cached) = CACHE.lock().ok()?.as_ref() {
        if cached.at.elapsed() < CACHE_TTL {
            return Some(cached.current_binary.clone());
        }
    }

    let path = resolve_claude_symlink()?;
    let version = extract_version_from_path(&path);

    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(CachedResult {
            current_binary: path.clone(),
            current_version: version,
            at: std::time::Instant::now(),
        });
    }
    Some(path)
}

/// `claude` コマンドの symlink 先を実パスまで解決する。
///
/// **境界（B26）を通す**: Windows の素の `canonicalize` は verbatim 形式
/// （`\\?\C:\…`）を返すが、突き合わせ相手の
/// [`tako_core::platform::procinfo::image_path`] は Win32 形式なので、
/// 剥がさないと**どのペインも常に stale** になる（#970 / #936）
fn resolve_claude_symlink() -> Option<PathBuf> {
    // canonicalize で symlink チェーンを完全解決
    tako_core::platform::path::canonicalize(&launcher_path()?).ok()
}

/// PATH 上の `claude` ランチャ（symlink は解決しない）を探す。
///
/// #772: 旧実装は `which claude` のサブプロセスだった。指紋取り（後述の
/// `current_binary_fingerprint`）は 2 秒ごとに走るので、ここでプロセスを起こすと
/// それだけで恒常的なコストになる。PATH の走査は stat だけで済むので自前で行い、
/// 取りこぼし（PATH の解釈差・`PATHEXT`・PATH 未伝播）に備えて境界 B16 を保険に残す
pub fn launcher_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join(CLAUDE_BIN);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    which_claude()
}

#[cfg(windows)]
const CLAUDE_BIN: &str = "claude.exe";
#[cfg(not(windows))]
const CLAUDE_BIN: &str = "claude";

/// 実行可能な通常ファイルか（symlink は追う = `which` と同じ判定）。
///
/// #936: 旧実装は非 unix で**無条件 `true`** を返していた（実行ビットという概念が
/// 無いので判定を書けなかった）。判定材料が OS で変わるので境界 B16 へ寄せた
/// （Windows は拡張子が `PATHEXT` に在るか）
fn is_executable_file(path: &Path) -> bool {
    tako_core::platform::exe::is_executable_file(path)
}

/// 上の PATH 走査で見つからなかったときの保険。境界 B16
/// （[`tako_core::platform::exe::find`]）へ委ねる。
///
/// **`which` を起こしてはいけない**（#898）: Windows に `which` は無いので旧実装は
/// 必ず `None` を返し、`claude` が PATH に伝播していない環境（インストーラが PATH を
/// 書いても実行中プロセスには届かない）で**検知が丸ごと無効**になっていた。
/// 境界は Windows では `PATHEXT`（`claude.cmd` の npm シムも拾う）と
/// ユーザー導入先を**サブプロセスなしで**走査する。
///
/// 上の走査を残しているのは #772 のため（指紋取りは定期実行なので、
/// 見つかる限り stat だけで済ませたい）。ここへ落ちるのは走査が空振りしたときだけ
fn which_claude() -> Option<PathBuf> {
    tako_core::platform::exe::find("claude").map(PathBuf::from)
}

/// バイナリパスからバージョンを推定する。
/// Claude CLI は `~/.local/share/claude/versions/<version>`（新）または
/// `~/.claude/local/claude-cli-<version>/claude`（旧）の構造。
///
/// パスに版が無ければ **実行ファイルの版リソース**（Windows の exe だけが持つ）→
/// `claude --version` の順に落ちる。Windows のランチャは実体のコピーで
/// パスに版が入らないので、版リソースが無いと 253MB の実行ファイルを
/// 定期走査のたびに起こすことになる（#936）
pub fn extract_version_from_path(path: &Path) -> String {
    if let Some(version) = version_from_segments(&path_segments(path)) {
        return version;
    }
    // Windows の exe は版をリソースとして持つ（`claude.exe` = `FileVersion 2.1.247.0`）。
    // あちらのランチャは symlink ではなく実体のコピーなのでパスから版が読めず、
    // これが無いと必ず `claude --version` の起動へ落ちる（#936）
    if let Some(version) = tako_core::platform::exe::file_version(path) {
        return version;
    }
    // フォールバック: claude --version（重いので最終手段）
    extract_version_via_cli(path)
}

/// パスを「区切りで割った成分」へ落とす。
///
/// **`/` と `\` の両方を区切りとして扱う**（`Path::components` は使わない）:
/// Windows の実パスは `…\versions\2.1.220` なので、components だと macOS からは
/// 1 成分に見えて **Windows 形の入力を macOS 上で検査できない**（#515 / #913 と
/// 同じ「プラットフォームで分岐せず macOS から Windows 形を通す」方針）。
/// unix のファイル名に `\` を含められる余地は残るが、影響は版の表示だけ
fn path_segments(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 成分列から版を読む（純粋関数）。読めなければ `None`
fn version_from_segments(segments: &[String]) -> Option<String> {
    // 新形式: .../versions/2.1.220（`versions` の**次の成分**が版。
    // Windows は `versions\2.1.220` が実行ファイル自身 = 実測）
    for (index, segment) in segments.iter().enumerate() {
        if segment != "versions" {
            continue;
        }
        // `versions` が末尾なら版は書かれていない（`?` で抜けると旧形式の判定まで飛ぶ）
        let Some(next) = segments.get(index + 1) else {
            continue;
        };
        if next.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Some(next.clone());
        }
    }
    // 旧形式: .../claude-cli-2.1.220/claude（版はディレクトリ名なので
    // **末尾の成分では無い**ことまで見る = `claude-cli-wrapper` を版と読まない）
    for (index, segment) in segments.iter().enumerate() {
        let Some(version) = segment.strip_prefix("claude-cli-") else {
            continue;
        };
        if !version.is_empty() && index + 1 < segments.len() {
            return Some(version.to_string());
        }
    }
    None
}

fn extract_version_via_cli(binary: &Path) -> String {
    // #586: GUI プロセスから到達するのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(
        std::process::Command::new(binary).arg("--version"),
    )
    .output()
    .ok()
    .filter(|o| o.status.success())
    .and_then(|o| {
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        // "claude v2.1.220" 形式
        text.split_whitespace()
            .find(|w| w.starts_with('v') || w.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(|w| w.trim_start_matches('v').to_string())
    })
    .unwrap_or_default()
}

/// 稼働中プロセスのバイナリパスを取得する。
///
/// #936: 旧実装は macOS の `proc_pidpath` と Linux の `/proc/<pid>/exe` だけで、
/// **Windows は常に `None`** = 実行中の claude を特定できないので警告が出なかった。
/// 判定材料が OS で変わるので境界 B5（[`tako_core::platform::procinfo::image_path`]）へ寄せた
pub fn pidpath(pid: u32) -> Option<PathBuf> {
    tako_core::platform::procinfo::image_path(pid)
}

/// ペインの stale 状態を判定する
pub fn check_stale(info: &PaneClaudeInfo) -> StaleStatus {
    let current = resolve_current_claude_binary();
    let current_binary = current.clone().unwrap_or_default();
    let current_version = CACHE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|c| c.current_version.clone()))
        .unwrap_or_default();
    let spawned_version = extract_version_from_path(&info.spawned_binary);

    let stale = current
        .as_ref()
        .is_some_and(|cur| cur != &info.spawned_binary);

    StaleStatus {
        stale,
        spawned_binary: info.spawned_binary.to_string_lossy().to_string(),
        current_binary: current_binary.to_string_lossy().to_string(),
        spawned_version,
        current_version,
        pid: info.pid,
    }
}

/// バックエンドセッション内で claude バイナリを実行しているプロセスの PID を探す。
/// tmux pane_pid → 子プロセスチェーンを辿り、`pidpath` で「claude」を含む
/// パスを実行しているプロセスを返す。`claude agents --json` に依存しないため
/// 隔離環境でも動く
pub fn find_claude_pid_for_backend(backend_session: &str) -> Option<u32> {
    let snapshot = crate::agents::ProcessSnapshot::capture();
    find_claude_pid(&snapshot, backend_session)
}

/// 与えられた pid 集合の中から、実行ファイルが `needle` を含むものを探す（#1067）。
///
/// [`find_claude_pid`] の一般形（`needle = "claude"` が従来の挙動）。
/// **判定材料は 2 通り**: まず実行ファイルのパス（`ProcessSnapshot` の argv は
/// `platform::procinfo` 経路では空になるので Windows はこちらだけ）、
/// それが取れない環境ではコマンド行を見る
pub fn find_agent_pid_among(
    snapshot: &crate::agents::ProcessSnapshot,
    pids: &[u32],
    needle: &str,
) -> Option<u32> {
    pids.iter().copied().find(|&pid| {
        pidpath(pid).is_some_and(|path| path.to_string_lossy().contains(needle))
            || snapshot.argv(pid).is_some_and(|argv| {
                // コマンド行は第 1 語（実行ファイル）だけを見る（引数の中の語で誤爆しない）
                argv.split_whitespace()
                    .next()
                    .is_some_and(|prog| prog.contains(needle))
            })
    })
}

/// 共有スナップショットから backend session 配下の claude PID を探す。
fn find_claude_pid(
    snapshot: &crate::agents::ProcessSnapshot,
    backend_session: &str,
) -> Option<u32> {
    snapshot
        .descendant_pids(backend_session)
        .into_iter()
        .find(|&pid| pidpath(pid).is_some_and(|path| path.to_string_lossy().contains("claude")))
}

// ===== 定期走査（#772: メインスレッド専有 400ms/tick の根治） =====
//
// 旧実装は 2 秒ごとの `periodic_prep` の中で、master / worker ペインごとに
// tmux と ps を起動していた（UI スレッド専有 392〜466ms を実測）。
// 新実装は
//   1. 呼び出し側（UI スレッド）は対象ペインの一覧を作るだけ（メモリ操作のみ）
//   2. background で **stat だけの指紋**を取り、前回と変わっていなければ何もしない
//   3. 変化したとき（claude の入れ替え・対象ペインの増減・起動直後）と、
//      取りこぼし回収のための低頻度（60 秒）だけ tmux + ps を 1 回ずつ実行する
// という 3 段構えにしてある。

/// 走査対象のペイン 1 つぶん
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleScanTarget {
    /// 呼び出し側のペイン識別子（そのまま結果へ返す）
    pub key: u64,
    /// ペインのバックエンド tmux セッション名
    pub backend_session: String,
}

/// 走査結果のペイン 1 つぶん
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleScanResult {
    /// 対応する `StaleScanTarget::key`
    pub key: u64,
    /// ペイン内で動いている claude の実パス（見つからなければ None）
    pub running_binary: Option<PathBuf>,
}

/// claude バイナリの変化を検出するための指紋。サブプロセスを起こさず
/// PATH 走査 + `canonicalize` + `metadata` の stat だけで作る
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryFingerprint {
    /// symlink を解決した実バイナリのパス（版が変われば通常はここが変わる）
    pub binary: PathBuf,
    /// 実バイナリの更新時刻（同じパスのまま中身が差し替わる場合の保険）
    pub mtime: Option<std::time::SystemTime>,
    /// 実バイナリのサイズ（同上）
    pub len: u64,
}

/// 現在の claude バイナリの指紋を取る。サブプロセスを起こさない（stat のみ）
pub fn current_binary_fingerprint() -> Option<BinaryFingerprint> {
    fingerprint_of(&launcher_path()?)
}

/// 指定したランチャの指紋を取る（テストで偽 claude を差し込むための入口）
pub fn fingerprint_of(launcher: &Path) -> Option<BinaryFingerprint> {
    // 境界（B26）を通す = `pidpath` 側と同じ正規化。#970 / #936
    let binary = tako_core::platform::path::canonicalize(launcher).ok()?;
    let meta = std::fs::metadata(&binary).ok();
    Some(BinaryFingerprint {
        mtime: meta.as_ref().and_then(|m| m.modified().ok()),
        len: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        binary,
    })
}

/// 前回の走査結果（呼び出し側が保持し、`poll` のたびに受け渡す）
#[derive(Debug, Clone, Default)]
pub struct ScanState {
    /// 最後に重い走査を回したときのバイナリ指紋
    pub fingerprint: Option<BinaryFingerprint>,
    /// 最後に重い走査を回したときの対象ペイン集合
    pub targets: Vec<StaleScanTarget>,
    /// 最後に重い走査を回した時刻（None = まだ 1 度も走らせていない）
    pub scanned_at: Option<std::time::Instant>,
}

/// 取りこぼし回収のための低頻度な再走査間隔。
/// 「バナーを閉じた」「ペイン内で claude を手動で入れ直した」等、指紋にも
/// 対象集合にも表れない変化をこの間隔で拾う（旧実装の 2 秒 → 60 秒 = 1/30）
pub const RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// 重い走査（tmux + ps）を回すべきかの判定。純関数なので単体テストで固定できる
pub fn should_rescan(
    prev: &ScanState,
    fingerprint: Option<&BinaryFingerprint>,
    targets: &[StaleScanTarget],
    now: std::time::Instant,
) -> bool {
    // 起動直後の初回（tako 再起動でつないだ既存セッションが stale なことがある）
    let Some(scanned_at) = prev.scanned_at else {
        return true;
    };
    // claude が入れ替わった
    if prev.fingerprint.as_ref() != fingerprint {
        return true;
    }
    // 対象ペインが増減した
    if prev.targets != targets {
        return true;
    }
    // 低頻度の取りこぼし回収
    now.duration_since(scanned_at) >= RESCAN_INTERVAL
}

/// `poll` の結果
#[derive(Debug, Clone)]
pub enum ScanOutcome {
    /// 指紋・対象とも変化なしで再走査の時期でもない = tmux も ps も起動していない
    Skipped,
    /// 重い走査を回した
    Scanned {
        /// 呼び出し側が次回に渡し直す状態
        state: ScanState,
        /// 現在の claude 実バイナリ（解決できなければ None）
        current_binary: Option<PathBuf>,
        /// 現在の claude のバージョン表記
        current_version: String,
        /// ペインごとの実行中バイナリ
        results: Vec<StaleScanResult>,
    },
}

/// stale 検知の定期走査本体。**background executor から呼ぶこと**
/// （変化時は tmux / ps を起動する）
pub fn poll(targets: Vec<StaleScanTarget>, prev: &ScanState) -> ScanOutcome {
    poll_with_snapshot(targets, prev, None)
}

/// sleep guard と同じ tick の `ProcessSnapshot` を共有して走査する。
/// snapshot が無く、かつ stale 側の再走査が必要なら内部で 1 回だけ採取する。
pub fn poll_with_snapshot(
    targets: Vec<StaleScanTarget>,
    prev: &ScanState,
    snapshot: Option<&crate::agents::ProcessSnapshot>,
) -> ScanOutcome {
    poll_with_launcher_and_snapshot(targets, prev, launcher_path(), snapshot)
}

/// ランチャを明示して走査する（セルフテストが偽 claude を差し込むための入口。
/// 環境変数を書き換えずに済ませたい = background で PATH を読んでいる最中に
/// `set_var` する競合を作らない）
pub fn poll_with_launcher(
    targets: Vec<StaleScanTarget>,
    prev: &ScanState,
    launcher: Option<PathBuf>,
) -> ScanOutcome {
    poll_with_launcher_and_snapshot(targets, prev, launcher, None)
}

fn poll_with_launcher_and_snapshot(
    targets: Vec<StaleScanTarget>,
    prev: &ScanState,
    launcher: Option<PathBuf>,
    snapshot: Option<&crate::agents::ProcessSnapshot>,
) -> ScanOutcome {
    let fingerprint = launcher.as_deref().and_then(fingerprint_of);
    let now = std::time::Instant::now();
    if !should_rescan(prev, fingerprint.as_ref(), &targets, now) {
        return ScanOutcome::Skipped;
    }

    let current_binary = fingerprint.as_ref().map(|f| f.binary.clone());
    let current_version = current_binary
        .as_deref()
        .map(extract_version_from_path)
        .unwrap_or_default();

    // 対象が空でも状態は進める（次の tick で無駄に走らせない）
    let results = if targets.is_empty() || current_binary.is_none() {
        Vec::new()
    } else {
        let captured;
        let snapshot = match snapshot {
            Some(snapshot) => snapshot,
            None => {
                captured = crate::agents::ProcessSnapshot::capture();
                &captured
            }
        };
        targets
            .iter()
            .map(|t| StaleScanResult {
                key: t.key,
                running_binary: find_claude_pid(snapshot, &t.backend_session).and_then(pidpath),
            })
            .collect()
    };

    ScanOutcome::Scanned {
        state: ScanState {
            fingerprint,
            targets,
            scanned_at: Some(now),
        },
        current_binary,
        current_version,
        results,
    }
}

/// StaleStatus を JSON に変換
pub fn status_to_json(status: &StaleStatus) -> serde_json::Value {
    serde_json::json!({
        "stale": status.stale,
        "spawned_binary": status.spawned_binary,
        "current_binary": status.current_binary,
        "spawned_version": status.spawned_version,
        "current_version": status.current_version,
        "pid": status.pid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version_from_path() {
        // 旧形式: claude-cli-<ver>/claude
        let path = PathBuf::from("/Users/user/.claude/local/claude-cli-2.1.220/claude");
        assert_eq!(extract_version_from_path(&path), "2.1.220");

        // 新形式: versions/<ver>（symlink 先そのもの）
        let path_new = PathBuf::from("/Users/user/.local/share/claude/versions/2.1.220");
        assert_eq!(extract_version_from_path(&path_new), "2.1.220");

        // パスに版が無く、実体も無いので CLI フォールバックも空を返す
        let unknown = std::env::temp_dir().join("tako-936-no-such-claude");
        assert_eq!(extract_version_from_path(&unknown), "");
    }

    /// **#936: Windows 形のパスを macOS から検査する**。`Path::components` で
    /// 割ると `C:\…\versions\2.1.220` は macOS 上で 1 成分に見えるため、
    /// 区切りは `/` と `\` の両方を見る実装になっている
    #[test]
    fn windows形のパスからも版を読む() {
        // 新形式（実測: Windows は `versions\<版>` が実行ファイル自身）
        let new_form = [
            r"C:\Users\winuser\.local\share\claude\versions\2.1.247",
            r"C:\Users\winuser\.local\share\claude\versions\2.1.247\claude.exe",
        ];
        for path in new_form {
            assert_eq!(
                version_from_segments(&path_segments(Path::new(path))).as_deref(),
                Some("2.1.247"),
                "{path}"
            );
        }
        // 旧形式
        assert_eq!(
            version_from_segments(&path_segments(Path::new(
                r"C:\Users\winuser\.claude\local\claude-cli-2.1.220\claude.exe"
            )))
            .as_deref(),
            Some("2.1.220")
        );
        // Windows のランチャは実体のコピーなので版が書かれていない
        // （版リソース → CLI の順で落ちる）
        assert_eq!(
            version_from_segments(&path_segments(Path::new(
                r"C:\Users\winuser\.local\bin\claude.exe"
            ))),
            None
        );
        // 自己更新で改名された旧 exe も同じ（実測の名前の形）
        assert_eq!(
            version_from_segments(&path_segments(Path::new(
                r"C:\Users\winuser\.local\bin\claude.exe.old.1787816114562"
            ))),
            None
        );
    }

    #[test]
    fn 版と紛らわしい成分を版と読まない() {
        // `versions` が末尾 = 版が書かれていない
        assert_eq!(
            version_from_segments(&path_segments(Path::new("/opt/claude/versions"))),
            None
        );
        // 数字始まりでない成分は版ではない（`current` 等の別名）
        assert_eq!(
            version_from_segments(&path_segments(Path::new("/opt/claude/versions/current"))),
            None
        );
        // `foo-versions` は `versions` ではない（旧実装は `/versions/` の
        // 部分一致だったので、`/x/versions/…` 以外は同じ結果になる）
        assert_eq!(
            version_from_segments(&path_segments(Path::new("/opt/old-versions/2.1.220"))),
            None
        );
        // `claude-cli-` が末尾成分なら版ディレクトリではない
        assert_eq!(
            version_from_segments(&path_segments(Path::new("/opt/bin/claude-cli-wrapper"))),
            None
        );
    }

    #[test]
    fn test_check_stale_same_binary() {
        let info = PaneClaudeInfo {
            spawned_binary: PathBuf::from("/fake/path/claude-cli-2.1.220/claude"),
            pid: Some(12345),
            dismissed: false,
        };
        // resolve_current_claude_binary が Some を返さない環境では stale=false
        let status = check_stale(&info);
        // CI などでは claude が無いため stale=false になる
        if status.current_binary.is_empty() {
            assert!(!status.stale);
        }
    }

    #[test]
    fn test_check_stale_different_binary() {
        // 手動で異なるバイナリを設定
        let info = PaneClaudeInfo {
            spawned_binary: PathBuf::from("/fake/old/claude-cli-2.1.218/claude"),
            pid: None,
            dismissed: false,
        };
        let status = check_stale(&info);
        // current が解決できる環境では stale=true になるはず
        if !status.current_binary.is_empty() {
            assert!(status.stale);
        }
    }

    #[test]
    fn test_pidpath_self() {
        let pid = std::process::id();
        let path = pidpath(pid);
        // 自プロセスのパスは取得できるはず
        assert!(path.is_some(), "自プロセスの pidpath が None");
    }

    #[test]
    fn test_pidpath_nonexistent() {
        let path = pidpath(99999999);
        assert!(path.is_none());
    }

    // ===== #772: 走査頻度の削減 =====

    fn fp(path: &str, len: u64) -> BinaryFingerprint {
        BinaryFingerprint {
            binary: PathBuf::from(path),
            mtime: None,
            len,
        }
    }

    fn target(key: u64, session: &str) -> StaleScanTarget {
        StaleScanTarget {
            key,
            backend_session: session.to_string(),
        }
    }

    #[test]
    fn 初回は必ず走査する() {
        let prev = ScanState::default();
        assert!(should_rescan(
            &prev,
            Some(&fp("/a/claude", 1)),
            &[target(1, "tako-s1")],
            std::time::Instant::now()
        ));
    }

    #[test]
    fn 指紋も対象も変わらなければ走査しない() {
        let now = std::time::Instant::now();
        let targets = vec![target(1, "tako-s1")];
        let prev = ScanState {
            fingerprint: Some(fp("/a/claude", 1)),
            targets: targets.clone(),
            scanned_at: Some(now),
        };
        assert!(!should_rescan(
            &prev,
            Some(&fp("/a/claude", 1)),
            &targets,
            now
        ));
    }

    #[test]
    fn バイナリのパスが変わったら走査する() {
        let now = std::time::Instant::now();
        let targets = vec![target(1, "tako-s1")];
        let prev = ScanState {
            fingerprint: Some(fp("/versions/2.1.220", 1)),
            targets: targets.clone(),
            scanned_at: Some(now),
        };
        assert!(should_rescan(
            &prev,
            Some(&fp("/versions/2.1.221", 1)),
            &targets,
            now
        ));
    }

    #[test]
    fn 同じパスでも中身が差し替わったら走査する() {
        let now = std::time::Instant::now();
        let targets = vec![target(1, "tako-s1")];
        let prev = ScanState {
            fingerprint: Some(fp("/a/claude", 100)),
            targets: targets.clone(),
            scanned_at: Some(now),
        };
        assert!(should_rescan(
            &prev,
            Some(&fp("/a/claude", 200)),
            &targets,
            now
        ));
    }

    #[test]
    fn 対象ペインが増減したら走査する() {
        let now = std::time::Instant::now();
        let prev = ScanState {
            fingerprint: Some(fp("/a/claude", 1)),
            targets: vec![target(1, "tako-s1")],
            scanned_at: Some(now),
        };
        let grown = vec![target(1, "tako-s1"), target(2, "tako-s2")];
        assert!(should_rescan(&prev, Some(&fp("/a/claude", 1)), &grown, now));
    }

    #[test]
    fn 変化がなくても既定間隔を過ぎたら走査する() {
        // Instant の減算は起動直後に overflow しうるので、基準時刻から先へ進める
        let scanned_at = std::time::Instant::now();
        let targets = vec![target(1, "tako-s1")];
        let prev = ScanState {
            fingerprint: Some(fp("/a/claude", 1)),
            targets: targets.clone(),
            scanned_at: Some(scanned_at),
        };
        assert!(should_rescan(
            &prev,
            Some(&fp("/a/claude", 1)),
            &targets,
            scanned_at + RESCAN_INTERVAL
        ));
        // 間隔の手前では走らせない
        assert!(!should_rescan(
            &prev,
            Some(&fp("/a/claude", 1)),
            &targets,
            scanned_at + RESCAN_INTERVAL / 2
        ));
    }

    #[test]
    fn claudeが消えたら走査する() {
        let now = std::time::Instant::now();
        let targets = vec![target(1, "tako-s1")];
        let prev = ScanState {
            fingerprint: Some(fp("/a/claude", 1)),
            targets: targets.clone(),
            scanned_at: Some(now),
        };
        assert!(should_rescan(&prev, None, &targets, now));
    }

    #[test]
    fn ランチャ探索は実行可能な通常ファイルだけを拾う() {
        let root = std::env::temp_dir().join(format!("tako-stale-772-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("テスト用ディレクトリ");
        // 実行できないファイル（拾わない）。**名前は両 OS で成立する形にする**:
        // unix は実行ビットが無いから、Windows は拡張子が `PATHEXT` に無いから
        // 落ちる（`claude.exe.old.<ts>` は claude の自己更新が残す実際の名前 = 実測）。
        // `CLAUDE_BIN` そのままだと Windows では `.exe` = 実行できる判定になる（#936）
        let plain = root.join("bin-plain");
        std::fs::create_dir_all(&plain).unwrap();
        let stale = plain.join(format!("{CLAUDE_BIN}.old.1787816114562"));
        std::fs::write(&stale, b"#!/bin/sh\n").unwrap();
        assert!(!is_executable_file(&stale));
        // 実行ビットあり（拾う）
        let exec = root.join("bin-exec");
        std::fs::create_dir_all(&exec).unwrap();
        let bin = exec.join(CLAUDE_BIN);
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(is_executable_file(&bin));
        // ディレクトリは拾わない
        assert!(!is_executable_file(&exec));
        // 存在しないものは拾わない
        assert!(!is_executable_file(&root.join("nope")));

        // 後片付け（一時ディレクトリ配下であることを確かめてから消す）
        assert!(
            root.starts_with(std::env::temp_dir())
                && root
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("tako-stale-772-")),
            "テスト用ディレクトリ以外を消そうとした: {}",
            root.display()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 指紋はサブプロセス無しで取れる() {
        // claude が居ない環境では None、居れば実バイナリと一致する
        match current_binary_fingerprint() {
            Some(f) => {
                assert!(f.binary.is_absolute(), "指紋のパスが絶対パスでない");
                assert_eq!(Some(f.binary.clone()), resolve_claude_symlink());
            }
            None => assert!(launcher_path().is_none() || resolve_claude_symlink().is_none()),
        }
    }
}
