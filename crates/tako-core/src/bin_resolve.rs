//! 外部バイナリ（git / tmux）の場所の解決（`git::git_bin()` / `tmux::tmux_bin()` の共通基盤）
//!
//! 解決の順は 環境変数（`TAKO_GIT_BIN` / `TAKO_TMUX_BIN`）→ PATH の名前（`git --version` が通るか）
//! → 既知の場所 → ログインシェルの `command -v` → 素の名前。ログインシェルを聞くのは、Dock 起動の
//! .app は PATH が最小構成（`/usr/bin:/bin:…`）で Homebrew の tmux が見えないため
//! （2026-06-12 の実機リグレッション）。結果は呼び手の `OnceLock` がプロセス内で 1 回だけ持つ。
//!
//! ## 上限（Issue #2151）
//!
//! 修正前は `--version` とログインシェルの `command -v` を**上限なしに**待った。rc が遅い・固まる
//! ログインシェルでは最初の git / tmux の操作が止まり（UI スレッドから呼ばれれば GUI ごと）、
//! しかも `OnceLock` の初期化の最中は、同じ値を引く**他のスレッドも全部**その完了を待たされた。
//!
//! - 解決の全体に締め切りを 1 本置く（[`DEFAULT_RESOLVE_TIMEOUT`]）。env [`RESOLVE_TIMEOUT_ENV`] は
//!   値を変えるだけで、0 / 不正 / 空は既定へ落ちる（**外せない**）。`--version` とログインシェルは
//!   締め切りまでの残りを `probe` の待ちの 1 実装へ渡す。初期化そのものが締め切りで終わるので、
//!   `OnceLock` を待つ他のスレッドもこれより長くは待たない
//! - 打ち切った段は飛ばして次へ進む。ログインシェルを打ち切ったら素の名前（= PATH の git）へ落ちる
//! - 打ち切ったら理由を 1 行 persist.log へ残す。tako-core は persist.log の書き手を持たないので、
//!   上の層（tako-app / tako CLI の main）が [`set_notice_sink`] で受け口を渡す。渡される前の行は
//!   数行だけ溜めておき、受け口が来たときに流す
//!
//! ## 上限の根拠（実測 2026-10-16・macOS・zsh）
//!
//! `zsh -l -c 'command -v git'` / `… tmux` が 0.14〜0.16 秒（3 回ずつ）、`git --version` が 0.01 秒。
//! 対話の rc まで読む `-l -i` でも 0.34〜0.35 秒（#1730 の実測）。5 秒はその 15 倍弱で、同じ種類の
//! 問い合わせ（Code Runner の実行環境の検出 = `probe::DEFAULT_RUNTIME_PROBE_TIMEOUT`）と揃える。
//! 打ち切りで失うのは「既知の場所にも PATH にも無く、ログインシェルだけが知っている git / tmux」で、
//! そのときは `TAKO_GIT_BIN` / `TAKO_TMUX_BIN` で場所を渡せる（persist.log の行がそれを案内する）
//!
//! ## A/B と注入
//!
//! - `TAKO_2151_LEGACY=1` で修正前の解決（上限なし・知らせなし）へ戻る。ゲートは
//!   [`crate::probe::issue2151_legacy`] の 1 か所
//! - `TAKO_2151_INJECT=skip-known-paths:<名前>[,<名前>…]` で、その名前の既知の場所の段を飛ばす
//!   （検証専用）。macOS には `/usr/bin/git` が必ずあるので、これが無いとログインシェルの段へ届かない

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::probe::TimeoutNotice;

/// 解決の全体の既定の上限。根拠はモジュール冒頭の実測
pub const DEFAULT_RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// 上限の上書き（秒）。0 / 不正 / 空は既定へ落ちる（**上限は外せない**）
pub const RESOLVE_TIMEOUT_ENV: &str = "TAKO_BIN_RESOLVE_TIMEOUT_SECS";

/// 既知の場所の段を飛ばす注入（検証専用）
pub const INJECT_ENV: &str = "TAKO_2151_INJECT";

/// 渡される前に溜めておく知らせの上限（1 プロセスで解決するのは git と tmux の 2 つだけ）
const PENDING_MAX: usize = 8;

/// 解決の上限（env で上書き可。外すことはできない）
pub fn resolve_timeout() -> Duration {
    crate::probe::parse_timeout_secs(
        std::env::var(RESOLVE_TIMEOUT_ENV).ok().as_deref(),
        DEFAULT_RESOLVE_TIMEOUT,
    )
}

/// 打ち切りの知らせの受け口を渡す（tako-app / tako CLI の main から 1 回）。
///
/// 受け口は persist.log へ 1 行書く関数（`diag::persist_log` 相当）。最初に渡されたものだけを使い、
/// それまでに溜まっていた行はここで流す
pub fn set_notice_sink(sink: fn(&str)) {
    NOTICES.install(sink);
}

/// どの段で決まったか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// 環境変数（`TAKO_GIT_BIN` 等）
    Env,
    /// PATH の名前（`--version` が通った）
    Path,
    /// 既知の場所
    KnownPath,
    /// ログインシェルの `command -v`（unix だけ。Windows はログインシェルを尋ねない）
    #[cfg_attr(not(unix), allow(dead_code))]
    LoginShell,
    /// どこでも決まらなかった（素の名前 = 起動時に PATH から探す）
    BareName,
}

/// 解決の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub path: String,
    pub source: Source,
    /// 締め切りで打ち切った段（無ければ空）
    pub timeouts: Vec<TimeoutNotice>,
}

impl Resolved {
    /// persist.log へ残す 1 行。打ち切った段が無ければ `None`（**正常な解決は書かない**）。
    ///
    /// `budget` は解決の全体の締め切り（段ごとの残りではなく、利用者が env で変える値を出す）。
    /// ログインシェルが答えた場所はホーム配下のことがあるので載せない（#927）。
    /// 名札は `probe::label` が組む（実行ファイル名と引数だけ = パスを含めない）
    pub(crate) fn log_line(&self, name: &str, env_var: &str, budget: Duration) -> Option<String> {
        if self.timeouts.is_empty() {
            return None;
        }
        let waited = self
            .timeouts
            .iter()
            .map(|t| format!("`{}`", t.label))
            .collect::<Vec<_>>()
            .join("・");
        let used = match self.source {
            Source::Env => format!("{env_var} の場所"),
            Source::Path => format!("PATH の `{name}`"),
            Source::KnownPath => format!("既知の場所の `{}`", self.path),
            Source::LoginShell => "ログインシェルが答えた場所".to_string(),
            Source::BareName => format!("素の名前の `{name}`（起動のたびに PATH から探す）"),
        };
        Some(format!(
            "{name} の場所の解決（#2151）: {waited} が締め切り（解決の全体で {} 秒）までに戻らないので\
             打ち切り、{used} を使う。場所は {env_var} で渡せる",
            budget.as_secs_f64()
        ))
    }
}

/// `git_bin()` / `tmux_bin()` の解決（呼び手の `OnceLock` の初期化から 1 回だけ呼ばれる）
pub(crate) fn resolve_bin(
    env_var: &str,
    name: &str,
    version_flag: &str,
    candidates: &[&str],
) -> String {
    // 注入は「既知の場所が無い機」を作るもの = 新旧どちらの腕にも効かせる（A/B を同じ条件で並べる）
    let skip_known = inject_skips_known_paths(std::env::var(INJECT_ENV).ok().as_deref(), name);
    let known: &[&str] = if skip_known { &[] } else { candidates };
    if crate::probe::issue2151_legacy() {
        return resolve_bin_legacy(env_var, name, version_flag, known);
    }
    let shell = login_shell();
    let budget = resolve_timeout();
    let resolved = resolve_within(env_var, name, version_flag, known, shell.as_deref(), budget);
    if let Some(line) = resolved.log_line(name, env_var, budget) {
        NOTICES.emit(&line);
    }
    resolved.path
}

/// 場所を尋ねるログインシェル（Windows には `$SHELL` も `command -v` も無いので尋ねない）
fn login_shell() -> Option<String> {
    #[cfg(unix)]
    {
        Some(crate::platform::exe::user_shell())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// 解決の本体（ログインシェルと締め切りを差し替えてテストで固定するために分けてある）。
///
/// `budget` は全体の締め切りで、子プロセスを待つ段（`--version` とログインシェル）には
/// その残りだけを渡す。`Duration::MAX` でも溢れない（残りは `saturating_sub` で数える）
pub(crate) fn resolve_within(
    env_var: &str,
    name: &str,
    version_flag: &str,
    candidates: &[&str],
    shell: Option<&str>,
    budget: Duration,
) -> Resolved {
    if let Some(bin) = std::env::var_os(env_var) {
        if !bin.is_empty() {
            return Resolved {
                path: bin.to_string_lossy().into_owned(),
                source: Source::Env,
                timeouts: Vec::new(),
            };
        }
    }
    let start = Instant::now();
    let left = || budget.saturating_sub(start.elapsed());
    let mut timeouts = Vec::new();
    // #586: 存在確認も GUI プロセスから走る（コンソールウィンドウの抑止は `output_with_timeout` が持つ）
    match crate::probe::output_with_timeout(name, &[version_flag], left()) {
        crate::probe::Outcome::Done { status, .. } if status.success() => {
            return Resolved {
                path: name.into(),
                source: Source::Path,
                timeouts,
            };
        }
        outcome => timeouts.extend(outcome.timeout_notice()),
    }
    for candidate in candidates {
        if std::path::Path::new(candidate).is_file() {
            return Resolved {
                path: (*candidate).into(),
                source: Source::KnownPath,
                timeouts,
            };
        }
    }
    #[cfg(unix)]
    {
        // `--version` が締め切りを使い切ったら尋ねない（尋ねても即座に打ち切るだけ）
        let rest = left();
        if let Some(shell) = shell.filter(|_| !rest.is_zero()) {
            let found = crate::platform::exe::login_shell_find(shell, name, rest);
            timeouts.extend(found.timeout);
            if let Some(path) = found.path {
                return Resolved {
                    path,
                    source: Source::LoginShell,
                    timeouts,
                };
            }
        }
    }
    #[cfg(not(unix))]
    let _ = shell;
    Resolved {
        path: name.into(),
        source: Source::BareName,
        timeouts,
    }
}

/// `TAKO_2151_INJECT=skip-known-paths:<名前>[,<名前>…]` がこの名前を指しているか（純関数）
fn inject_skips_known_paths(raw: Option<&str>, name: &str) -> bool {
    raw.and_then(|v| v.trim().strip_prefix("skip-known-paths:"))
        .is_some_and(|names| names.split(',').any(|n| n.trim() == name))
}

/// #2151 前の解決（**上限なし**。A/B の再現アーム = `TAKO_2151_LEGACY=1` のときだけ通る）
fn resolve_bin_legacy(
    env_var: &str,
    name: &str,
    version_flag: &str,
    candidates: &[&str],
) -> String {
    if let Some(bin) = std::env::var_os(env_var) {
        if !bin.is_empty() {
            return bin.to_string_lossy().into_owned();
        }
    }
    // #586: バイナリ存在確認も GUI プロセスから走る（コンソールウィンドウを出させない）
    if crate::platform::process::no_console_window(&mut std::process::Command::new(name))
        .arg(version_flag)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return name.into();
    }
    for candidate in candidates {
        if std::path::Path::new(candidate).is_file() {
            return (*candidate).into();
        }
    }
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        if let Ok(output) = std::process::Command::new(shell)
            .args(["-l", "-c", &format!("command -v {name}")])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && std::path::Path::new(&path).is_file() {
                    return path;
                }
            }
        }
    }
    name.into()
}

/// 打ち切りの知らせの受け口（受け口が来る前の行は [`PENDING_MAX`] 行まで溜める）
struct NoticeSink {
    state: Mutex<SinkState>,
}

struct SinkState {
    sink: Option<fn(&str)>,
    pending: Vec<String>,
}

impl NoticeSink {
    const fn new() -> Self {
        Self {
            state: Mutex::new(SinkState {
                sink: None,
                pending: Vec::new(),
            }),
        }
    }

    /// 受け口を渡す（最初の 1 つだけ）。溜まっていた行を流す。
    /// 流すのも書くのも同じ錠の中で行う（受け口の設置と知らせが同時に来ても行を落とさない）
    fn install(&self, sink: fn(&str)) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.sink.is_some() {
            return;
        }
        state.sink = Some(sink);
        for line in std::mem::take(&mut state.pending) {
            sink(&line);
        }
    }

    fn emit(&self, line: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match state.sink {
            Some(sink) => sink(line),
            None if state.pending.len() < PENDING_MAX => state.pending.push(line.to_string()),
            None => {}
        }
    }
}

static NOTICES: NoticeSink = NoticeSink::new();

#[cfg(test)]
mod tests {
    use super::*;

    /// 偽の実行ファイル（引数を無視して本文を実行する。ログインシェル・`git` の代役）
    #[cfg(unix)]
    fn fake_bin(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// 戻らないログインシェル。`exec` で置き換える = 打ち切りの kill が眠る本人に当たる（孫を残さない）。
    /// 自分の pid を `pid` へ書いてから眠る（打ち切った子が残っていないかを見るため）
    #[cfg(unix)]
    fn hang_shell(dir: &std::path::Path) -> String {
        let pid = dir.join("pid");
        fake_bin(
            dir,
            "hang-shell",
            &format!("echo $$ > '{}'\nexec sleep 600", pid.display()),
        )
    }

    /// 打ち切った子が居なくなるまで待つ（刈り取りは別スレッドなので少し遅れる）
    #[cfg(unix)]
    fn assert_reaped(dir: &std::path::Path) {
        let pid: u32 = std::fs::read_to_string(dir.join("pid"))
            .expect("偽のシェルが起動して pid を書いた")
            .trim()
            .parse()
            .expect("pid");
        let until = Instant::now() + Duration::from_secs(10);
        // 生死は境界の 1 実装で見る（#1557。ゾンビは「居る」なので刈り取りまで待つことになる）
        while crate::platform::process::pid_alive(pid) {
            assert!(Instant::now() < until, "打ち切った子 {pid} が残っている");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 受け入れ条件 1: 戻らないログインシェルは締め切りで打ち切り、素の名前（PATH）へ落ちて理由を残す。
    /// 状態で見る（実時間は assert しない）。上限が無ければここで戻らない
    #[cfg(unix)]
    #[test]
    fn 戻らないログインシェルは締め切りで打ち切って素の名前へ落ちる() {
        let dir = crate::test_residue::ScratchDir::new("bin-resolve-hang");
        let shell = hang_shell(dir.path());
        let got = resolve_within(
            "TAKO_2151_TEST_UNSET",
            "tako-2151-absent",
            "--version",
            &[],
            Some(&shell),
            Duration::from_secs(1),
        );
        assert_eq!(got.path, "tako-2151-absent");
        assert_eq!(got.source, Source::BareName);
        assert_eq!(got.timeouts.len(), 1, "{got:?}");
        assert_eq!(
            got.timeouts[0].label,
            "hang-shell -l -c command -v tako-2151-absent"
        );
        let line = got
            .log_line(
                "tako-2151-absent",
                "TAKO_2151_TEST_UNSET",
                Duration::from_secs(1),
            )
            .expect("打ち切ったら 1 行残す");
        assert!(
            line.contains("#2151") && line.contains("TAKO_2151_TEST_UNSET"),
            "{line}"
        );
        assert!(line.contains("素の名前の `tako-2151-absent`"), "{line}");
        assert!(line.contains("解決の全体で 1 秒"), "{line}");
        // 名札はパスを含めない（#927）
        let scratch = dir.path().display().to_string();
        assert!(!line.contains(&scratch), "{line}");
        assert_reaped(dir.path());
    }

    /// 受け入れ条件 3: `OnceLock` の初期化の最中に別スレッドが引いても、初期化が締め切りで
    /// 終わるので一緒に戻る（値は初期化した側のもの）。上限が無ければ両方戻らず、外側の締め切りで落ちる
    #[cfg(unix)]
    #[test]
    fn 初期化の最中に別スレッドが引いても締め切りで一緒に戻る() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc, OnceLock};

        let dir = crate::test_residue::ScratchDir::new("bin-resolve-oncelock");
        let shell = hang_shell(dir.path());
        let cell: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let started = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<(&'static str, bool, String)>();

        let (a_cell, a_started, a_tx) = (Arc::clone(&cell), Arc::clone(&started), tx.clone());
        std::thread::spawn(move || {
            let value = a_cell.get_or_init(|| {
                a_started.store(true, Ordering::SeqCst);
                resolve_within(
                    "TAKO_2151_TEST_UNSET",
                    "tako-2151-absent",
                    "--version",
                    &[],
                    Some(&shell),
                    Duration::from_secs(2),
                )
                .path
            });
            let _ = a_tx.send(("A", true, value.clone()));
        });
        let until = Instant::now() + Duration::from_secs(10);
        while !started.load(Ordering::SeqCst) {
            assert!(Instant::now() < until, "初期化が始まらない");
            std::thread::sleep(Duration::from_millis(5));
        }
        let b_cell = Arc::clone(&cell);
        std::thread::spawn(move || {
            // 引いた時点でまだ初期化の最中だった（= 初期化の完了を待つ側だった）か
            let was_pending = b_cell.get().is_none();
            let value = b_cell.get_or_init(|| "B が初期化した".to_string());
            let _ = tx.send(("B", was_pending, value.clone()));
        });
        let mut got = Vec::new();
        for _ in 0..2 {
            got.push(
                rx.recv_timeout(Duration::from_secs(30))
                    .expect("git を引いたスレッドが締め切りを過ぎても戻らない"),
            );
        }
        got.sort();
        assert_eq!(
            got,
            vec![
                ("A", true, "tako-2151-absent".to_string()),
                ("B", true, "tako-2151-absent".to_string()),
            ],
            "B は A の初期化の最中に引き、A の値で戻る"
        );
        assert_reaped(dir.path());
    }

    /// `--version` が戻らない名前は打ち切って既知の場所へ進む（PATH の git が固まるラッパーのとき）
    #[cfg(unix)]
    #[test]
    fn versionが戻らない名前は打ち切って既知の場所へ進む() {
        let dir = crate::test_residue::ScratchDir::new("bin-resolve-version");
        let hang = fake_bin(dir.path(), "hang-git", "exec sleep 600");
        let known = fake_bin(dir.path(), "known-git", "exit 0");
        let got = resolve_within(
            "TAKO_2151_TEST_UNSET",
            &hang,
            "--version",
            &[known.as_str()],
            None,
            Duration::from_secs(1),
        );
        assert_eq!(got.source, Source::KnownPath, "{got:?}");
        assert_eq!(got.path, known);
        assert_eq!(got.timeouts.len(), 1, "{got:?}");
        assert_eq!(got.timeouts[0].label, "hang-git --version");
        let line = got
            .log_line("git", "TAKO_GIT_BIN", Duration::from_millis(1500))
            .expect("1 行残す");
        assert!(line.contains("`hang-git --version`"), "{line}");
        assert!(line.contains(&format!("既知の場所の `{known}`")), "{line}");
        assert!(line.contains("解決の全体で 1.5 秒"), "{line}");
    }

    /// 戻るログインシェルの答えは従来どおり採る（打ち切りが無ければ何も書かない）
    #[cfg(unix)]
    #[test]
    fn ログインシェルが答えた場所を採る() {
        let dir = crate::test_residue::ScratchDir::new("bin-resolve-answer");
        let answer = fake_bin(dir.path(), "answer-git", "exit 0");
        let shell = fake_bin(dir.path(), "answer-shell", &format!("echo '{answer}'"));
        let got = resolve_within(
            "TAKO_2151_TEST_UNSET",
            "tako-2151-absent",
            "--version",
            &[],
            Some(&shell),
            DEFAULT_RESOLVE_TIMEOUT,
        );
        assert_eq!(
            got,
            Resolved {
                path: answer,
                source: Source::LoginShell,
                timeouts: Vec::new(),
            }
        );
        assert_eq!(
            got.log_line("git", "TAKO_GIT_BIN", DEFAULT_RESOLVE_TIMEOUT),
            None
        );
    }

    /// 受け入れ条件 2: 通常の環境（このテストのプロセスの PATH と `$SHELL`）では修正前と同じ場所になる
    #[test]
    fn 通常の環境では修正前と同じ場所になる() {
        let shell = login_shell();
        for (env_var, name, flag, candidates) in [
            (
                "TAKO_GIT_BIN",
                "git",
                "--version",
                &[
                    "/opt/homebrew/bin/git",
                    "/usr/local/bin/git",
                    "/usr/bin/git",
                ][..],
            ),
            (
                "TAKO_TMUX_BIN",
                "tmux",
                "-V",
                &[
                    "/opt/homebrew/bin/tmux",
                    "/usr/local/bin/tmux",
                    "/opt/local/bin/tmux",
                ][..],
            ),
        ] {
            let new = resolve_within(
                env_var,
                name,
                flag,
                candidates,
                shell.as_deref(),
                DEFAULT_RESOLVE_TIMEOUT,
            );
            assert!(new.timeouts.is_empty(), "{name}: {new:?}");
            assert_eq!(
                new.path,
                resolve_bin_legacy(env_var, name, flag, candidates),
                "{name}"
            );
        }
    }

    #[test]
    fn 注入は名指しした名前の既知の場所だけを飛ばす() {
        let raw = Some("skip-known-paths:git");
        assert!(inject_skips_known_paths(raw, "git"));
        assert!(!inject_skips_known_paths(raw, "tmux"));
        assert!(inject_skips_known_paths(
            Some(" skip-known-paths:tmux, git "),
            "git"
        ));
        for other in [None, Some(""), Some("git"), Some("skip-known-paths:")] {
            assert!(!inject_skips_known_paths(other, "git"), "{other:?}");
        }
    }

    /// 受け口が来る前の行は溜めて流し、溜めるのは上限まで。来た後は直に渡す
    #[test]
    fn 知らせは受け口が来るまで溜めて流す() {
        static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
        fn record(line: &str) {
            SEEN.lock().unwrap().push(line.to_string());
        }
        fn other(_: &str) {
            panic!("2 つ目の受け口は使わない");
        }
        let sink = NoticeSink::new();
        for i in 0..PENDING_MAX + 3 {
            sink.emit(&format!("行 {i}"));
        }
        assert!(SEEN.lock().unwrap().is_empty(), "受け口が来る前は書かない");
        sink.install(record);
        sink.install(other);
        sink.emit("後の行");
        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(seen.len(), PENDING_MAX + 1, "{seen:?}");
        assert_eq!(seen.first().map(String::as_str), Some("行 0"));
        assert_eq!(seen.last().map(String::as_str), Some("後の行"));
    }

    #[test]
    fn 打ち切りが無ければ何も書かない() {
        let resolved = Resolved {
            path: "git".into(),
            source: Source::Path,
            timeouts: Vec::new(),
        };
        assert_eq!(
            resolved.log_line("git", "TAKO_GIT_BIN", DEFAULT_RESOLVE_TIMEOUT),
            None
        );
    }
}
