//! 常駐の remote daemon が install をまたいで旧版のまま残らないことの番犬（#2518）
//!
//! # なぜ要るか
//!
//! remote daemon（`tako remote serve`）は GUI とは別プロセスで、install（.app の差し替え）でも
//! GUI の再起動でも入れ替わらない。#2513 の段 1 で実測: 10/28 に起動した daemon が install 3 回と
//! GUI 再起動をまたいで旧版のまま約 2 日動き、消えた旧版の名前で exec 拒否を 37,445 件積んだ。
//!
//! 真因は `spawn_daemon` が稼働中の daemon の実行ファイルを**パスの文字列**で比べていたこと。
//! install の contents-swap はパスを変えずに中身（inode）だけを替えるので、パスが一致した
//! 時点で「既に起動中」と返り、GUI 起動時の自動復帰（#1485）も「already-running」で見送っていた。
//!
//! # 何を縛るか
//!
//! 1. daemon は起動時に自分の実行ファイルの識別を PID ファイルへ残す（`self_stamp_line`）
//! 2. 判定は `remote_generation::judge` の 1 実装で、**ファイルの識別**を比べる（パスの文字列ではない）
//! 3. `daemon_status` が判定を `serve_generation` に載せる（読む側は再計算しない）
//! 4. `spawn_daemon` はパスが一致しても、旧版なら「既に起動中」で返さず入れ替える
//! 5. 入れ替えの停止は「起動していた」（`desired`）を消さない（`forget_if_stopped` を通らない）
//! 6. GUI 起動時の自動復帰（`probe` → `autostart_decision`）が旧版を「既に起動している」に数えない
//! 7. GUI は入れ替えたことを `last_autostart` へ残す（`started_from`）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので [`走査が空振りしていない`] で窓を固定し、
//! [`パスの文字列だけで比べる形へ戻すと名指しする`] で**修正前を再現した注入**が file:line で
//! 名指しされることを確かめる。判定そのものは [`実ファイルの差し替えを旧版と判定する`] が
//! 実ファイル（同じパスへの rename = contents-swap と同じ形）で縛る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const GENERATION: &str = "crates/tako-control/src/remote_generation.rs";
const AUTOSTART: &str = "crates/tako-control/src/remote_autostart.rs";
const PANEL: &str = "crates/tako-app/src/remote_panel.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &'static str) -> String {
    let raw =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言行から N 行」では隣の関数を巻き込む）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行の 1-based 行番号（窓の先頭が `at`）
fn line_of(at: usize, window: &str, needle: &str) -> usize {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|i| at + i)
        .unwrap_or(at)
}

struct Sources {
    remote: String,
    generation: String,
    autostart: String,
    panel: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        remote: read(root, REMOTE),
        generation: read(root, GENERATION),
        autostart: read(root, AUTOSTART),
        panel: read(root, PANEL),
    }
}

/// `remote.rs` の検査（記録・判定の配線・入れ替え）
fn scan_remote(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: REMOTE,
            line,
            why,
        })
    };

    // 1. daemon は起動時に自分の実行ファイルの識別を残す
    match fn_window(src, "let pid_info = format!(") {
        None => push(
            0,
            "PID ファイルの組み立て（`let pid_info = format!(`）が見つからない（走査の空振り）"
                .into(),
        ),
        Some((at, _)) => {
            // 組み立ての式は数行なので、書き出し（`write_secret_file(&pid_path()`）までを見る
            let lines: Vec<&str> = src.lines().collect();
            let body: Vec<&str> = lines
                .iter()
                .skip(at - 1)
                .take_while(|l| !l.contains("write_secret_file(&pid_path()"))
                .copied()
                .collect();
            if !code_only(&body.join("\n")).contains("self_stamp_line(") {
                push(
                    at,
                    "PID ファイルに実行ファイルの識別（`remote_generation::self_stamp_line`）を残していない。\
                     残さないと、同じパスのまま差し替わった旧版をパスの文字列でしか見分けられない（#2518）"
                        .into(),
                );
            }
        }
    }

    // 2. 判定はファイルの識別を比べる 1 実装を通る
    match fn_window(src, "fn generation_of(") {
        None => push(
            0,
            "`generation_of` が無い（daemon の世代の判定が消えている。#2518）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("remote_generation::judge(")
                || !code.contains("remote_generation::observe(")
            {
                push(
                    at,
                    "世代の判定が `remote_generation::observe` + `judge`（ファイルの識別の比較）を通っていない（#2518）"
                        .into(),
                );
            }
            if code.contains("serve_binary()") {
                push(
                    line_of(at, &window, "serve_binary()"),
                    "世代の判定がパスの文字列（`serve_binary()` の解決先）と比べている。\
                     contents-swap はパスを変えないので旧版を見分けられない（#2518）"
                        .into(),
                );
            }
        }
    }

    // 3. status が判定を載せる（読む側は再計算しない）
    match fn_window(src, "pub fn daemon_status()") {
        None => push(0, "`daemon_status` が見つからない（走査の空振り）".into()),
        Some((at, window)) => {
            if !code_only(&window).contains("generation_fields(") {
                push(
                    at,
                    "`daemon_status` が `serve_generation`（`generation_fields`）を載せていない。\
                     旧版のまま動いていることを status も自動復帰も読めない（#2518）"
                        .into(),
                );
            }
        }
    }

    // 4. spawn_daemon はパスが一致しても旧版なら入れ替える
    match fn_window(src, "pub fn spawn_daemon()") {
        None => push(0, "`spawn_daemon` が見つからない（走査の空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("renewable_in_status(") || !code.contains("stop_stale_generation(") {
                push(
                    line_of(at, &window, "\"リモートサーバーは既に起動中\""),
                    "`spawn_daemon` が稼働中の daemon をパスの文字列（`running_exe != expected`）だけで比べ、\
                     一致したら「既に起動中」で返している。install の contents-swap はパスを変えないので、\
                     旧版が install も GUI の再起動もまたいで残る（#2518 の真因）"
                        .into(),
                );
            }
        }
    }

    // 5. 入れ替えの停止は「起動していた」を消さない
    match fn_window(src, "fn stop_stale_generation(") {
        None => push(
            0,
            "`stop_stale_generation` が無い（旧版を止める入れ替えの前半が消えている。#2518）"
                .into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("daemon_stop_impl(") {
                push(
                    at,
                    "入れ替えの停止が `daemon_stop_impl`（正体を確かめてから畳ませる 1 実装）を通っていない".into(),
                );
            }
            for forbidden in [
                "forget_if_stopped(",
                "daemon_stop()",
                "daemon_force_stop()",
                "clear_desired()",
            ] {
                if code.contains(forbidden) {
                    push(
                        line_of(at, &window, forbidden),
                        format!(
                            "入れ替えの停止が `{forbidden}` を通っている。「起動していた」（desired）が消え、\
                             次の GUI 起動で daemon が戻らなくなる（#2518 / #1485）"
                        ),
                    );
                }
            }
            if !code.contains("daemon_status()") {
                push(
                    at,
                    "止まったかを現況（`daemon_status`）で確かめていない。止まらないまま起こすと \
                     2 本目が待ち受けの取り合いで落ちる"
                        .into(),
                );
            }
        }
    }
    out
}

/// `remote_autostart.rs` の検査（GUI 起動時の判断）
fn scan_autostart(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: AUTOSTART,
            line,
            why,
        })
    };
    match fn_window(src, "pub fn probe()") {
        None => push(0, "`probe` が見つからない（走査の空振り）".into()),
        Some((at, window)) => {
            if !code_only(&window).contains("renewable_in_status(") {
                push(
                    line_of(at, &window, "daemon_stale:"),
                    "`probe` が旧版の判定（`remote_generation::renewable_in_status`）を読んでいない。\
                     GUI 起動時に旧版の daemon を「既に起動している」で見送る（#2518）"
                        .into(),
                );
            }
        }
    }
    match fn_window(src, "pub fn autostart_decision(") {
        None => push(
            0,
            "`autostart_decision` が見つからない（走査の空振り）".into(),
        ),
        Some((at, window)) => {
            if !code_only(&window).contains("!input.daemon_stale") {
                push(
                    line_of(at, &window, "AutostartSkip::AlreadyRunning"),
                    "旧版のまま動いている daemon を「既に起動している」に数えている（#2518）"
                        .into(),
                );
            }
        }
    }
    out
}

/// `remote_panel.rs` の検査（GUI が入れ替えを記録する）
fn scan_panel(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn spawn_remote_autostart(") {
        None => out.push(Offender {
            file: PANEL,
            line: 0,
            why: "`spawn_remote_autostart` が見つからない（走査の空振り）".into(),
        }),
        Some((at, window)) => {
            if !code_only(&window).contains("LastAutostart::started_from(") {
                out.push(Offender {
                    file: PANEL,
                    line: line_of(at, &window, "LastAutostart::started("),
                    why: "自動復帰の成功を `started_from`（`spawn_daemon` の応答を読む）で残していない。\
                          旧版を入れ替えてスマホが一瞬切れた理由が status に残らない（#2518）"
                        .into(),
                });
            }
        }
    }
    out
}

/// `remote_generation.rs` の検査（判定がパスを受け取らない）
fn scan_generation(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "pub fn judge(") {
        None => out.push(Offender {
            file: GENERATION,
            line: 0,
            why: "`judge` が無い（世代の判定の 1 実装が消えている。#2518）".into(),
        }),
        Some((at, window)) => {
            let signature: String = window.lines().take_while(|l| !l.contains('{')).collect();
            let head = window.lines().next().unwrap_or("");
            if signature.contains("&str") || signature.contains("Path") || head.contains("&str") {
                out.push(Offender {
                    file: GENERATION,
                    line: at,
                    why: "`judge` がパスを受け取っている。判定の材料はファイルの識別だけにする\
                          （パスが同じことは同じ世代の根拠にならない。#2518）"
                        .into(),
                });
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    scan_remote(&s.remote)
        .into_iter()
        .chain(scan_autostart(&s.autostart))
        .chain(scan_panel(&s.panel))
        .chain(scan_generation(&s.generation))
        .map(|o| o.report())
        .collect()
}

#[test]
fn 旧版のdaemonを見分けて入れ替える配線が揃っている() {
    let found = all(&sources(&workspace_root()));
    assert!(
        found.is_empty(),
        "#2518 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for (src, file, needle) in [
        (&s.remote, REMOTE, "let pid_info = format!("),
        (&s.remote, REMOTE, "fn generation_of("),
        (&s.remote, REMOTE, "pub fn daemon_status()"),
        (&s.remote, REMOTE, "pub fn spawn_daemon()"),
        (&s.remote, REMOTE, "fn stop_stale_generation("),
        (&s.autostart, AUTOSTART, "pub fn probe()"),
        (&s.autostart, AUTOSTART, "pub fn autostart_decision("),
        (&s.panel, PANEL, "fn spawn_remote_autostart("),
        (&s.generation, GENERATION, "pub fn judge("),
    ] {
        let (_, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{file} に `{needle}` が無い（番犬が空振りする）"));
        assert!(
            window.lines().count() >= 3,
            "{file} の `{needle}` の窓が短すぎる（{} 行）",
            window.lines().count()
        );
    }
}

/// 注入を当てて、名指しされた行を返す
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    let out = base.replacen(from, to, 1);
    assert!(out != base, "注入「{what}」の対象が見つからない: {from:?}");
    out
}

/// `src` の中で `needle` を含む最初のコードの行（1-based）
fn first_line(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("`{needle}` が見つからない"))
}

#[test]
fn パスの文字列だけで比べる形へ戻すと名指しする() {
    let root = workspace_root();
    let base = sources(&root);
    assert!(all(&base).is_empty(), "注入の前は緑であること");

    // 注入 1: #2518 前の spawn_daemon（パスが一致したら「既に起動中」で返す）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "        if !crate::remote_generation::renewable_in_status(&status) {\n            return Err(\"リモートサーバーは既に起動中\".to_string());\n        }\n        renewed_from = Some(stop_stale_generation(&status)?);\n",
        "        return Err(\"リモートサーバーは既に起動中\".to_string());\n",
        "パスの文字列だけで比べる spawn_daemon",
    );
    let (at, _) = fn_window(&s.remote, "pub fn spawn_daemon()").unwrap();
    let want = format!(
        "{REMOTE}:{}",
        at - 1
            + s.remote
                .lines()
                .skip(at - 1)
                .position(|l| l.contains("\"リモートサーバーは既に起動中\""))
                .unwrap()
            + 1
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "パスの文字列だけで比べる形を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 2: 判定がパスの文字列と比べる（識別を見ない）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "    let now = crate::remote_generation::observe(std::path::Path::new(exe));\n",
        "    let now = crate::remote_generation::observe(std::path::Path::new(exe));\n    let _same_path = exe == serve_binary();\n",
        "パスの文字列と比べる generation_of",
    );
    let want = format!(
        "{REMOTE}:{}",
        first_line(&s.remote, "let _same_path = exe == serve_binary();")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "判定のパス比較を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 3: PID ファイルに識別を残さない（#2518 前の 3 行形式）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "        crate::remote_generation::self_stamp_line(&exe),\n",
        "",
        "識別を残さない PID ファイル",
    );
    let want = format!(
        "{REMOTE}:{}",
        first_line(&s.remote, "let pid_info = format!(")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "識別を残さない形を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 4: 入れ替えの停止が desired を消す経路（daemon_stop）を通る
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "    let stopped = daemon_stop_impl(false);\n",
        "    let stopped = daemon_stop();\n",
        "desired を消す停止",
    );
    let want = format!(
        "{REMOTE}:{}",
        first_line(&s.remote, "    let stopped = daemon_stop();")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "desired を消す停止を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 5: status が世代を載せない
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "    if let Some((field, warning)) = generation_fields(&pid_info) {",
        "    if let Some((field, warning)) = None::<(Value, Option<String>)> {",
        "世代を載せない status",
    );
    let found = all(&s);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(&format!("{REMOTE}:")) && o.contains("generation_fields")),
        "世代を載せない status を名指しできていない: {found:#?}"
    );

    // 注入 6: GUI 起動時の判断が旧版を読まない（#2518 前の probe）
    let mut s = sources(&root);
    s.autostart = inject(
        &base.autostart,
        "        daemon_stale: crate::remote_generation::renewable_in_status(&status),\n",
        "        daemon_stale: false,\n",
        "旧版を読まない probe",
    );
    let want = format!(
        "{AUTOSTART}:{}",
        first_line(&s.autostart, "        daemon_stale: false,")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "旧版を読まない probe を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 7: 旧版も「既に起動している」に数える（#2518 前の autostart_decision）
    let mut s = sources(&root);
    s.autostart = inject(
        &base.autostart,
        "    if input.daemon_running && !input.daemon_stale {",
        "    if input.daemon_running {",
        "旧版も既に起動中に数える判断",
    );
    let found = all(&s);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(&format!("{AUTOSTART}:")) && o.contains("既に起動している")),
        "旧版を既に起動中に数える判断を名指しできていない: {found:#?}"
    );
}

/// GUI 起動時の判断（`remote_autostart::probe`）を **GUI を立てずに本物のまま**呼ぶ。
///
/// 材料は隔離した state ディレクトリ・`tako remote serve` に見える使い捨てのプロセス（PID ファイルの
/// 先）・その実行ファイルに見立てた実ファイル。実行ファイルを同じパスへの rename で差し替える前は
/// `already-running`、後は立て直し（`Ok`）、A/B（`TAKO_2518_LEGACY=1`）は `already-running` に戻る。
/// tailscale は存在しないパスを渡す（`TAKO_TAILSCALE_BIN` は指定のパスだけを試す = 本物を叩かない）
#[cfg(target_os = "macos")]
#[test]
fn gui起動時の判断は差し替わった旧版のdaemonを立て直し対象にする() {
    // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(gui起動時の判断は差し替わった旧版のdaemonを立て直し対象にする)
    {
        return;
    }
    use tako_control::remote_autostart::{probe, AutostartSkip};
    let root = workspace_root();
    let probe_at = format!(
        "{AUTOSTART}:{}",
        first_line(&read(&root, AUTOSTART), "pub fn probe()")
    );

    let dir = std::env::temp_dir().join(format!("tako-2518-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let exe = dir.join("tako");
    std::fs::write(&exe, b"tako generation A").unwrap();
    std::env::set_var("TAKO_REMOTE_STATE_DIR", &state);
    std::env::set_var("TAKO_DATA_DIR", dir.join("data"));
    std::env::set_var("TAKO_TAILSCALE_BIN", dir.join("no-such-tailscale"));
    for k in [
        tako_control::remote_generation::LEGACY_ENV,
        tako_control::remote_autostart::LEGACY_ENV,
    ] {
        std::env::remove_var(k);
    }

    // `tako remote serve` に見える使い捨てのプロセス（`read` は組み込み = 子を作らない）
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args(["-c", "read line", "tako", "remote", "serve"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // 起動の形は remote.rs の使い捨てのプロセスと揃える（#628）
    let mut child = tako_core::platform::process::no_console_window(&mut cmd)
        .spawn()
        .expect("使い捨ての sh");
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        state.join("tako-remote.pid"),
        format!(
            "{}\n{}\n{started}\n{}\n",
            child.id(),
            exe.display(),
            tako_control::remote_generation::self_stamp_line(&exe)
        ),
    )
    .unwrap();
    tako_control::remote::set_desired();

    let before = probe();
    // install の contents-swap と同じ形: 隣で作った新しいファイルを同じパスへ置く
    let staged = dir.join("staged-tako");
    std::fs::write(&staged, b"tako generation A").unwrap();
    std::fs::rename(&staged, &exe).unwrap();
    let after = probe();
    let status = tako_control::remote::daemon_status();
    std::env::set_var(tako_control::remote_generation::LEGACY_ENV, "1");
    let legacy = probe();
    std::env::remove_var(tako_control::remote_generation::LEGACY_ENV);

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        before,
        Err(AutostartSkip::AlreadyRunning),
        "{probe_at}: 現行の daemon を立て直そうとしている"
    );
    assert_eq!(
        after,
        Ok(()),
        "{probe_at}: 同じパスのまま差し替わった旧版の daemon を「既に起動している」で見送っている\
         （#2518 の症状）。status: {status}"
    );
    assert_eq!(status["serve_generation"]["state"], "replaced", "{status}");
    assert_eq!(
        legacy,
        Err(AutostartSkip::AlreadyRunning),
        "{probe_at}: A/B（{}=1）が #2518 前の判断へ戻っていない",
        tako_control::remote_generation::LEGACY_ENV
    );
}

/// 判定の本体を実ファイルで縛る: 同じパスへ別のファイルを rename で置く（install の
/// contents-swap と同じ形）と旧版、読み直すだけなら現行。パスの文字列は一度も変わらない
#[test]
fn 実ファイルの差し替えを旧版と判定する() {
    use tako_control::remote_generation::{judge, observe, renewable_in_status, status_field};
    let root = workspace_root();
    let judge_at = format!(
        "{GENERATION}:{}",
        first_line(&read(&root, GENERATION), "pub fn judge(")
    );

    let dir = std::env::temp_dir().join(format!("tako-2518-watchdog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir
        .join("tako.app")
        .join("Contents")
        .join("MacOS")
        .join("tako");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"tako generation A").unwrap();

    let started = observe(&exe);
    let recorded = started.stamp.expect("在るファイルは識別を持つ");
    let same = judge(Some(&recorded), Some(0), &observe(&exe));

    // 中身も長さも同じ新しいファイルを隣で作り、同じパスへ置く（パスの文字列は不変）
    let staged = dir.join("staged-tako");
    std::fs::write(&staged, b"tako generation A").unwrap();
    std::fs::rename(&staged, &exe).unwrap();
    let swapped = judge(Some(&recorded), Some(0), &observe(&exe));
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        same.tag(),
        "current",
        "{judge_at}: 読み直しただけで旧版と言っている"
    );
    if cfg!(unix) {
        assert_eq!(
            swapped.tag(),
            "replaced",
            "{judge_at}: 同じパスのまま差し替わった実行ファイルを旧版と言えていない\
             （パスの文字列で比べる形へ戻っている疑い。#2518）"
        );
        let status = serde_json::json!({
            "running": true,
            "serve_generation": status_field(swapped, &exe.display().to_string(), 1),
        });
        assert!(
            renewable_in_status(&status),
            "{judge_at}: 差し替わった旧版を立て直し対象と読めていない: {status}"
        );
    }
}
