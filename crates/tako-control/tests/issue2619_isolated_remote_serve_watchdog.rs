//! 隔離中（`TAKO_ISOLATED`）の remote start が本物の tailscale の serve を張らないことの番犬（#2619）
//!
//! # なぜ要るか
//!
//! tailscale の serve の設定はこの機械のノード単位の状態で、隔離の砂場の外にある。砂場の CLI から
//! `tako remote start` を打つと、daemon は既定の tailscale へ serve を張っていた（11/01 の観察と段 1 の
//! 再現: 既定のノード = GUI 版の Tailscale に `serve_set`）。砂場の向くノードが本番の公開と同じなら、
//! 起動時の張り替えの判定が本番の待ち受けを「前回のポートの残骸」と読んで張り替える = 本番の公開を奪う。
//! 門は `remote_isolated_serve::check`（判定は純関数 `allows` の 1 実装）。
//!
//! # 何を縛るか
//!
//! 1. 起動の 1 実装 `spawn_daemon` は、頼んだ記録の後・自分の印（`StartingGuard::begin`）と子の起動
//!    （`Command::new`）より前に門を通り、断りは型（`RemoteRefusal::IsolatedServe`）の表示で返す
//! 2. serve を張る関数 `establish_tailscale_serve` は、tailscale に話しかける前に門を通る
//!    （daemon の中の最後の守り = `tako remote serve` を直に起こした場合）
//! 3. UDS の `remote_setup::configure_serve` も serve を張る前に門を通る
//! 4. **serve を書く呼び出し**（`serve_start_target(_on)` / `serve_stop(_on)` / 生の `&["serve", …]`）は
//!    既知の場所（上の 3 つ・稼働中の張り直し `assert_target`・tailscale.rs の包み）にしか無い。
//!    新しい書き手は file:line で名指す（門を通すか、ここへ理由つきで足すかを決める）
//! 5. 実際の `check` が env（隔離・検証モード・偽 tailscale・opt-in・A/B）で答えを変える（実経路。
//!    **tailscale は起こさない** = 判定だけを縛る）
//!
//! 後始末の `serve_stop_if_ours(_on)`（自分の待ち受けと完全一致のときだけ外す）は 4 の書き手に数えない。
//!
//! # serve の外の、機械全体へ触る経路（#2624）
//!
//! 6. stop の後始末 `cleanup_serve_leftover` は、本物の tailscale を引く（`find_tailscale`）・外す
//!    （`serve_stop_if_ours_on`）前に門（`Op::ServeCleanup`）を通り、見送りを audit.log へ残す
//! 7. 対話の setup の導入 `install_tailscale` は、導入器（`setup_deps::offer_and_install`）の前に
//!    門（`check_install`）を通り、断りは型（`RemoteRefusal::IsolatedInstall`）を表示言語の文にして返す
//! 8. 対話の setup の `run_interactive` は、QR を既定のビューアで開く（`open_default`）前に
//!    門（`Op::OpenViewer`）を通る
//! 9. 後始末の書き手 `serve_stop_if_ours(_on)` の呼び手と、remote_setup.rs の導入器・ビューアの呼び手は
//!    既知の場所（上の 3 つ・門を通った daemon の中の `run_daemon`・tailscale.rs の包み）にしか無い
//! 10. 実際の門（`allowed` / `check_install`）が env で答えを変える（操作ごとの通す理由。実経路）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので [`走査が空振りしていない`] で窓を固定し、
//! [`修正前の形へ戻すと名指しする`] で**修正前を再現した注入**が file:line で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const SETUP: &str = "crates/tako-control/src/remote_setup.rs";
const TAILSCALE: &str = "crates/tako-control/src/tailscale.rs";
/// 門の呼び出し（この形で書く）
const GATE: &str = "crate::remote_isolated_serve::check()";
/// #2624 の門（後始末・導入・ビューア。この形で書く）
const GATE_CLEANUP: &str =
    "crate::remote_isolated_serve::allowed(crate::remote_isolated_serve::Op::ServeCleanup)";
const GATE_INSTALL: &str = "crate::remote_isolated_serve::check_install()";
const GATE_VIEWER: &str =
    "crate::remote_isolated_serve::allowed(crate::remote_isolated_serve::Op::OpenViewer)";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    let raw =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
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

/// 窓の中で `needle` を含む最初の**コードの**行の 1-based 行番号（無ければ None）
fn code_line(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|i| at + i)
}

/// 門が `before` のどれよりも前にあるか（`before` の行は窓の中で最初に現れたもの）
fn gate_first(
    file: &str,
    src: &str,
    func: &str,
    before: &[&str],
    why: &str,
    out: &mut Vec<String>,
) -> Option<usize> {
    gate_first_with(file, src, func, GATE, before, why, "#2619", out)
}

/// [`gate_first`] の門の形と Issue を選べる版（#2624 の門も同じ規則で縛る）
#[allow(clippy::too_many_arguments)]
fn gate_first_with(
    file: &str,
    src: &str,
    func: &str,
    gate_call: &str,
    before: &[&str],
    why: &str,
    issue: &str,
    out: &mut Vec<String>,
) -> Option<usize> {
    let Some((at, w)) = fn_window(src, func) else {
        out.push(format!("{file}:0 — `{func}` が無い（走査の空振り）"));
        return None;
    };
    let Some(gate) = code_line(at, &w, gate_call) else {
        out.push(format!(
            "{file}:{at} — `{func}` が隔離の門（`{gate_call}`）を通らずに機械全体へ向かう。{why}（{issue}）"
        ));
        return None;
    };
    for b in before {
        match code_line(at, &w, b) {
            Some(line) if line < gate => out.push(format!(
                "{file}:{line} — `{func}` が隔離の門より前に `{b}` へ進む。{why}（{issue}）"
            )),
            Some(_) => {}
            None => out.push(format!(
                "{file}:{at} — `{func}` に `{b}` が見つからない（走査の空振り）"
            )),
        }
    }
    Some(gate)
}

/// 6〜8 の違反（#2624: serve の外の、機械全体へ触る経路）
fn machine_op_violations(remote: &str, setup: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 6. stop の後始末: 本物の tailscale を引く・外す前
    if let Some(gate) = gate_first_with(
        REMOTE,
        remote,
        "fn cleanup_serve_leftover()",
        GATE_CLEANUP,
        &[
            "crate::tailscale::find_tailscale()",
            "crate::tailscale::serve_stop_if_ours_on(",
        ],
        "隔離の砂場の stop が機械全体の serve を読み・外す",
        "#2624",
        &mut out,
    ) {
        let (at, w) = fn_window(remote, "fn cleanup_serve_leftover()").expect("上で見つけた");
        match code_line(at, &w, "\"isolated_serve_cleanup_skipped\"") {
            Some(l) if l > gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門で見送った後始末を audit.log（`isolated_serve_cleanup_skipped`）へ残していない（#2624）"
            )),
        }
    }

    // 7. 対話の setup の導入: 導入器の前・断りは型を表示言語の文にする
    if let Some(gate) = gate_first_with(
        SETUP,
        setup,
        "fn install_tailscale(",
        GATE_INSTALL,
        &["setup_deps::offer_and_install("],
        "隔離の砂場の setup が機械全体へ Tailscale を導入する",
        "#2624",
        &mut out,
    ) {
        let (at, w) = fn_window(setup, "fn install_tailscale(").expect("上で見つけた");
        let refusal_typed = w
            .lines()
            .skip(gate - at)
            .take(2)
            .any(|l| l.contains(".text_in("));
        if !refusal_typed {
            out.push(format!(
                "{SETUP}:{gate} — 導入の断りが型（`RemoteRefusal::IsolatedInstall`）の表示言語の文になっていない。\
                 英語表示に日本語が混ざる（#2365 / #2624）"
            ));
        }
    }

    // 8. 対話の setup の QR: 既定のビューアで開く前
    gate_first_with(
        SETUP,
        setup,
        "pub fn run_interactive(",
        GATE_VIEWER,
        &["open_default("],
        "隔離の砂場の setup が利用者の画面へビューアの窓を出す",
        "#2624",
        &mut out,
    );
    out
}

/// 後始末の書き手 `serve_stop_if_ours(_on)` を呼んでよい関数（ファイル, 関数名）。
/// 門を通った daemon の中（`run_daemon` = `establish_tailscale_serve` の後）・門を通る後始末・tailscale.rs の包み
const KNOWN_CLEANUP_CALLERS: &[(&str, &str)] = &[
    (REMOTE, "run_daemon"),
    (REMOTE, "cleanup_serve_leftover"),
    (TAILSCALE, "serve_stop_if_ours"),
];

/// remote_setup.rs の中で導入器・ビューアを呼んでよい関数（門を通る 2 つだけ）
const KNOWN_SETUP_MACHINE_CALLERS: &[(&str, &str)] = &[
    ("offer_and_install(", "install_tailscale"),
    ("open_default(", "run_interactive"),
];

/// 行が `needle`（`名前(`）を呼んでいるか（宣言・長い名前の一部は数えない）
fn calls(line: &str, needle: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    code.match_indices(needle).any(|(i, _)| {
        !code[..i].trim_end().ends_with("fn")
            && code[..i]
                .chars()
                .last()
                .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'))
    })
}

/// 9. 既知の場所の外の後始末の書き手・remote_setup.rs の導入器 / ビューアの呼び手
fn machine_caller_violations(files: &[(&str, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (file, src) in files {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let cleanup =
                calls(line, "serve_stop_if_ours(") || calls(line, "serve_stop_if_ours_on(");
            let setup_hit = if *file == SETUP {
                KNOWN_SETUP_MACHINE_CALLERS
                    .iter()
                    .find(|(needle, _)| calls(line, needle))
            } else {
                None
            };
            if !cleanup && setup_hit.is_none() {
                continue;
            }
            let func = enclosing_fn(&lines, i).unwrap_or_default();
            if cleanup
                && !KNOWN_CLEANUP_CALLERS
                    .iter()
                    .any(|(f, n)| f == file && *n == func)
            {
                out.push(format!(
                    "{file}:{} — `{func}` が後始末の書き手（`serve_stop_if_ours`）を呼ぶ。隔離の砂場から機械全体の\
                     serve を読み・外しうるので、隔離の門（`{GATE_CLEANUP}`）を通すか、番犬の既知の呼び手へ理由つきで足す（#2624）",
                    i + 1
                ));
            }
            if let Some((needle, known)) = setup_hit {
                if func != *known {
                    out.push(format!(
                        "{file}:{} — `{func}` が `{needle}` を呼ぶ。隔離の砂場から機械全体へ導入する / 利用者の画面へ\
                         窓を出すので、`{known}` の門を通す形へ寄せる（#2624）",
                        i + 1
                    ));
                }
            }
        }
    }
    out
}

/// 1〜3 の違反
fn gate_violations(remote: &str, setup: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 1. spawn_daemon: 頼んだ記録の後・自分の印と子の起動の前
    if let Some(gate) = gate_first(
        REMOTE,
        remote,
        "pub fn spawn_daemon()",
        &[
            "StartingGuard::begin(",
            "Command::new(",
            "start_blocks_request()",
        ],
        "隔離の砂場から機械全体の serve を張る daemon を起こす",
        &mut out,
    ) {
        let (at, w) = fn_window(remote, "pub fn spawn_daemon()").expect("上で見つけた");
        match code_line(at, &w, "audit_daemon_request(\"daemon_start_requested\"") {
            Some(l) if l < gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門が頼んだ記録（`daemon_start_requested`）より前にある。\
                 断った起動が audit.log に残らない（#2533 / #2619）"
            )),
        }
        match code_line(at, &w, "audit_daemon_request(\"isolated_serve_refused\"") {
            Some(l) if l > gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門で断ったことを audit.log（`isolated_serve_refused`）へ残していない（#2619）"
            )),
        }
        match code_line(at, &w, "return Err(refusal.to_string())") {
            Some(l) if l > gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門の断りが型（`RemoteRefusal::IsolatedServe`）の表示で返っていない。\
                 英語表示に日本語が混ざる（#2365 / #2619）"
            )),
        }
    }

    // 2. establish_tailscale_serve: tailscale に話しかける前
    gate_first(
        REMOTE,
        remote,
        "fn establish_tailscale_serve(",
        &[
            "crate::tailscale::setup_status()",
            "crate::tailscale::serve_state_on(",
            "crate::tailscale::serve_start_target_on(",
        ],
        "daemon を直に起こした隔離の砂場が既定の tailscale へ serve を張る",
        &mut out,
    );

    // 3. configure_serve（UDS の setup）
    gate_first(
        SETUP,
        setup,
        "pub fn configure_serve(",
        &["tailscale::serve_state(", "tailscale::serve_start_target("],
        "隔離の砂場の setup が既定の tailscale へ serve を張る",
        &mut out,
    );
    out
}

/// serve を書く呼び出しが書かれてよい関数（ファイル, 関数名）。
/// 門を通る 3 つ + 起動時の門を通った daemon の中でだけ回る張り直し + tailscale.rs の包み
const KNOWN_WRITERS: &[(&str, &str)] = &[
    (REMOTE, "establish_tailscale_serve"),
    (REMOTE, "assert_target"),
    (SETUP, "configure_serve"),
    (TAILSCALE, "serve_start_target"),
    (TAILSCALE, "serve_start_target_on"),
    (TAILSCALE, "serve_stop"),
    (TAILSCALE, "serve_stop_on"),
    (TAILSCALE, "serve_stop_if_ours_on"),
];

/// 行が serve を書く呼び出しか（後始末の `serve_stop_if_ours` は数えない）
fn is_writer_call(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    let calls = [
        "serve_start_target(",
        "serve_start_target_on(",
        "serve_stop(",
        "serve_stop_on(",
    ];
    let named = calls.iter().any(|c| {
        code.match_indices(c).any(|(i, _)| {
            // 宣言（`fn serve_stop(`）は呼び出しではない
            !code[..i].trim_end().ends_with("fn")
                && code[..i]
                    .chars()
                    .last()
                    .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'))
        })
    });
    let raw = code.contains("&[\"serve\",") && !code.contains("&[\"serve\", \"status\"");
    named || raw
}

/// 行を囲む関数の名前（字下げが浅い直近の `fn 名前(`）
fn enclosing_fn(lines: &[&str], idx: usize) -> Option<String> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let here = indent(lines[idx]);
    lines[..idx].iter().rev().find_map(|l| {
        if indent(l) >= here {
            return None;
        }
        let t = l.trim_start();
        let pos = t.find("fn ")?;
        let head = &t[..pos];
        if !head.is_empty()
            && !head
                .split_whitespace()
                .all(|w| matches!(w, "pub" | "pub(crate)" | "async" | "const" | "unsafe"))
        {
            return None;
        }
        let rest = &t[pos + 3..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

/// 4. 既知の場所の外の書き手
fn writer_violations(files: &[(&str, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (file, src) in files {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") || !is_writer_call(line) {
                continue;
            }
            let func = enclosing_fn(&lines, i).unwrap_or_default();
            if !KNOWN_WRITERS.iter().any(|(f, n)| f == file && *n == func) {
                out.push(format!(
                    "{file}:{} — `{func}` が tailscale の serve を書く。隔離の砂場から機械全体の serve を\
                     書き換えうるので、隔離の門（`{GATE}`）を通すか、通さない理由を番犬の既知の書き手へ足す（#2619）",
                    i + 1
                ));
            }
        }
    }
    out
}

/// `crates/*/src` の本番コード（rs）を全部読む（テスト専用のファイルは外し、テスト領域は空白へ潰す。
/// 小さいファイルもあるので下限の確かめ（`production`）は通さない = 行番号は原文のまま）
fn production_sources(root: &Path) -> Vec<(String, String)> {
    let files = production_range::product_sources();
    let test_only = production_range::test_only_files(&files);
    files
        .into_iter()
        .filter(|rel| !test_only.contains(rel))
        .map(|rel| {
            let raw = std::fs::read_to_string(root.join(&rel))
                .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
            let text = production_range::scan(&raw).text;
            (rel, text)
        })
        .collect()
}

fn all_violations(root: &Path) -> Vec<String> {
    let remote = read(root, REMOTE);
    let setup = read(root, SETUP);
    let mut out = gate_violations(&remote, &setup);
    out.extend(machine_op_violations(&remote, &setup));
    let files = production_sources(root);
    let borrowed: Vec<(&str, String)> =
        files.iter().map(|(f, s)| (f.as_str(), s.clone())).collect();
    out.extend(writer_violations(&borrowed));
    out.extend(machine_caller_violations(&borrowed));
    out
}

#[test]
fn 隔離中のremote_startはserveを張る前に門を通る() {
    let found = all_violations(&workspace_root());
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let remote = read(&root, REMOTE);
    let setup = read(&root, SETUP);
    for (src, file, needle) in [
        (&remote, REMOTE, "pub fn spawn_daemon()"),
        (&remote, REMOTE, "fn establish_tailscale_serve("),
        (&setup, SETUP, "pub fn configure_serve("),
        (&remote, REMOTE, "fn cleanup_serve_leftover()"),
        (&setup, SETUP, "fn install_tailscale("),
        (&setup, SETUP, "pub fn run_interactive("),
    ] {
        let (_, w) = fn_window(src, needle).unwrap_or_else(|| panic!("{file}: `{needle}` が無い"));
        assert!(
            w.lines().count() >= 5,
            "{file}: `{needle}` の窓が短すぎる（走査の空振り）"
        );
    }
    // 書き手の走査は既知の書き手を全部見つけている（見つけられないなら判定が空振り）
    let files = production_sources(&root);
    assert!(
        files.len() >= 100,
        "本番コードの走査が少なすぎる: {}",
        files.len()
    );
    for (file, func) in KNOWN_WRITERS {
        let src = &files
            .iter()
            .find(|(f, _)| f == file)
            .unwrap_or_else(|| panic!("{file} が走査に無い"))
            .1;
        let lines: Vec<&str> = src.lines().collect();
        let hit = lines.iter().enumerate().any(|(i, l)| {
            !l.trim_start().starts_with("//")
                && is_writer_call(l)
                && enclosing_fn(&lines, i).as_deref() == Some(*func)
        });
        assert!(
            hit,
            "{file}: 既知の書き手 `{func}` に serve を書く呼び出しが見つからない（走査の空振りか、書き手が消えた = 表を直す）"
        );
    }
    // #2624: 後始末の書き手・導入器・ビューアの既知の呼び手も、それぞれ実際に呼んでいる
    let calls_in = |file: &str, func: &str, needles: &[&str]| {
        let src = &files
            .iter()
            .find(|(f, _)| f == file)
            .unwrap_or_else(|| panic!("{file} が走査に無い"))
            .1;
        let lines: Vec<&str> = src.lines().collect();
        lines.iter().enumerate().any(|(i, l)| {
            !l.trim_start().starts_with("//")
                && needles.iter().any(|n| calls(l, n))
                && enclosing_fn(&lines, i).as_deref() == Some(func)
        })
    };
    for (file, func) in KNOWN_CLEANUP_CALLERS {
        assert!(
            calls_in(file, func, &["serve_stop_if_ours(", "serve_stop_if_ours_on("]),
            "{file}: 既知の呼び手 `{func}` に後始末の書き手の呼び出しが見つからない（走査の空振り = 表を直す）"
        );
    }
    for (needle, func) in KNOWN_SETUP_MACHINE_CALLERS {
        assert!(
            calls_in(SETUP, func, &[needle]),
            "{SETUP}: 既知の呼び手 `{func}` に `{needle}` が見つからない（走査の空振り = 表を直す）"
        );
    }
}

/// 注入を当てる（対象が無ければ注入そのものが空振りしている）
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    let out = base.replacen(from, to, 1);
    assert!(out != base, "注入「{what}」の対象が見つからない: {from:?}");
    out
}

#[test]
fn 修正前の形へ戻すと名指しする() {
    let root = workspace_root();
    let remote = read(&root, REMOTE);
    let setup = read(&root, SETUP);
    let named = |found: Vec<String>, file: &str, what: &str| {
        assert!(
            found.iter().any(
                |o| o.starts_with(&format!("{file}:")) && !o.starts_with(&format!("{file}:0 "))
            ),
            "注入「{what}」を {file} の行で名指しできていない: {found:?}"
        );
    };
    let gate_in_spawn =
        "    if let Err(refusal) = crate::remote_isolated_serve::check() {\n        audit_daemon_request(\"isolated_serve_refused\"";

    // 注入 1: spawn_daemon が門を通らない（#2619 前）
    named(
        gate_violations(
            &inject(
                &remote,
                gate_in_spawn,
                "    if let Err(refusal) = Ok::<(), crate::RemoteRefusal>(()) {\n        audit_daemon_request(\"isolated_serve_refused\"",
                "spawn_daemon が門を通らない",
            ),
            &setup,
        ),
        REMOTE,
        "spawn_daemon が門を通らない",
    );

    // 注入 2: 門を自分の印の後へずらす
    {
        let moved = inject(
            &remote,
            "    let _starting = StartingGuard::begin(renewing);\n",
            "",
            "印を外す",
        );
        let moved = inject(
            &moved,
            gate_in_spawn,
            &format!("    let _starting = StartingGuard::begin(None);\n{gate_in_spawn}"),
            "門を印の後へ",
        );
        named(gate_violations(&moved, &setup), REMOTE, "門を印の後へ");
    }

    // 注入 3: 断りを日本語の固定文で返す（#2365 前の形）
    named(
        gate_violations(
            &inject(
                &remote,
                "return Err(refusal.to_string())",
                "return Err(format!(\"隔離中なので serve を張りません: {refusal:?}\"))",
                "日本語の固定文",
            ),
            &setup,
        ),
        REMOTE,
        "日本語の固定文",
    );

    // 注入 4: establish_tailscale_serve が門を通らない（daemon を直に起こす経路）
    named(
        gate_violations(
            &inject(
                &remote,
                "    crate::remote_isolated_serve::check().map_err(|r| io::Error::other(r.to_string()))?;\n",
                "",
                "establish が門を通らない",
            ),
            &setup,
        ),
        REMOTE,
        "establish が門を通らない",
    );

    // 注入 5: configure_serve が門を通らない
    named(
        gate_violations(
            &remote,
            &inject(
                &setup,
                "    crate::remote_isolated_serve::check().map_err(|r| r.to_string())?;\n",
                "",
                "configure_serve が門を通らない",
            ),
        ),
        SETUP,
        "configure_serve が門を通らない",
    );

    // 注入 6: 門の外に新しい書き手（daemon_status の中で serve を張り直す）
    let injected = inject(
        &remote,
        "pub fn spawn_daemon() -> Result<Value, String> {\n",
        "pub fn spawn_daemon() -> Result<Value, String> {\n    let _ = crate::tailscale::serve_start_target_on(\"tailscale\", None, \"http://127.0.0.1:1\");\n",
        "門の外の書き手",
    );
    named(
        writer_violations(&[(REMOTE, injected)]),
        REMOTE,
        "門の外の書き手（名前つき）",
    );
    // 注入 7: 生の引数で書く
    let injected = inject(
        &setup,
        "pub fn configure_serve(",
        "fn sneaky(cli: &str) {\n    let _ = std::process::Command::new(cli).args(&[\"serve\", \"--bg\", \"--https=443\", \"http://127.0.0.1:1\"]);\n}\n\npub fn configure_serve(",
        "生の引数の書き手",
    );
    named(
        writer_violations(&[(SETUP, injected)]),
        SETUP,
        "生の引数の書き手",
    );
}

/// #2624 の経路を修正前の形へ戻す注入を file:line で名指しする
#[test]
fn 機械全体へ触る経路を修正前へ戻すと名指しする() {
    let root = workspace_root();
    let remote = read(&root, REMOTE);
    let setup = read(&root, SETUP);
    let named = |found: Vec<String>, file: &str, what: &str| {
        assert!(
            found.iter().any(
                |o| o.starts_with(&format!("{file}:")) && !o.starts_with(&format!("{file}:0 "))
            ),
            "注入「{what}」を {file} の行で名指しできていない: {found:?}"
        );
    };
    // 修正後の形はどれも名指さない（注入の効きを読む基準）
    assert!(machine_op_violations(&remote, &setup).is_empty());

    // 注入 1: stop の後始末が門を通らない（#2624 前）
    named(
        machine_op_violations(
            &inject(
                &remote,
                &format!("if !{GATE_CLEANUP} {{"),
                "if false {",
                "後始末の門を外す",
            ),
            &setup,
        ),
        REMOTE,
        "後始末の門を外す",
    );
    // 注入 2: 門より前に本物の tailscale を引く
    named(
        machine_op_violations(
            &inject(
                &remote,
                "fn cleanup_serve_leftover() {\n",
                "fn cleanup_serve_leftover() {\n    let _ = crate::tailscale::find_tailscale();\n",
                "門より前に引く",
            ),
            &setup,
        ),
        REMOTE,
        "門より前に引く",
    );
    // 注入 3: 見送りを audit.log へ残さない
    named(
        machine_op_violations(
            &inject(
                &remote,
                "\"isolated_serve_cleanup_skipped\"",
                "\"skipped\"",
                "見送りの記録を外す",
            ),
            &setup,
        ),
        REMOTE,
        "見送りの記録を外す",
    );
    // 注入 4: 導入が門を通らない（#2624 前 = 本物の brew が走る）
    named(
        machine_op_violations(
            &remote,
            &inject(
                &setup,
                GATE_INSTALL,
                "Ok::<(), tako_control::RemoteRefusal>(())",
                "導入の門を外す",
            ),
        ),
        SETUP,
        "導入の門を外す",
    );
    // 注入 5: 導入の断りを日本語の固定文で返す（#2365 前の形）
    named(
        machine_op_violations(
            &remote,
            &inject(
                &setup,
                ".map_err(|r| r.text_in(crate::settings::peek_lang_setting().resolve()))?;",
                ".map_err(|_| \"隔離中なので導入しません\".to_string())?;",
                "導入の断りを固定文に",
            ),
        ),
        SETUP,
        "導入の断りを固定文に",
    );
    // 注入 6: QR のビューアが門を通らない（#2624 前 = 利用者の画面に窓が出る）
    named(
        machine_op_violations(
            &remote,
            &inject(&setup, GATE_VIEWER, "true", "ビューアの門を外す"),
        ),
        SETUP,
        "ビューアの門を外す",
    );
    // 注入 7: 既知の場所の外に後始末の書き手
    named(
        machine_caller_violations(&[(
            REMOTE,
            inject(
                &remote,
                "pub fn spawn_daemon() -> Result<Value, String> {\n",
                "pub fn spawn_daemon() -> Result<Value, String> {\n    let _ = crate::tailscale::serve_stop_if_ours_on(\"tailscale\", None, None);\n",
                "門の外の後始末",
            ),
        )]),
        REMOTE,
        "門の外の後始末",
    );
    // 注入 8: remote_setup.rs の別の関数が導入器 / ビューアを呼ぶ
    for (what, call) in [
        (
            "門の外のビューア",
            "    let _ = crate::platform::os_integration::open_default(std::path::Path::new(\"/x\"));\n",
        ),
        (
            "門の外の導入器",
            "    let _ = setup_deps::offer_and_install(todo!(), todo!(), todo!());\n",
        ),
    ] {
        named(
            machine_caller_violations(&[(
                SETUP,
                inject(
                    &setup,
                    "pub fn configure_serve(cli: &str) -> Result<ServeStep, String> {\n",
                    &format!("pub fn configure_serve(cli: &str) -> Result<ServeStep, String> {{\n{call}"),
                    what,
                ),
            )]),
            SETUP,
            what,
        );
    }
}

/// 実経路（#2624）: 実際の `allowed` / `check_install` が env で操作ごとに答えを変える
/// （**tailscale も brew も起こさない**。判定だけを縛る）
#[test]
fn 実際の機械全体の門はenvで答えを変える() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(実際の機械全体の門はenvで答えを変える) {
        return;
    }
    use tako_control::remote_isolated_serve::{
        allowed, check_install, Op, LEGACY_2624_ENV, LEGACY_ENV, OPT_IN_ENV,
    };
    use tako_core::i18n::{set_lang, Lang};
    const NAMES: &[&str] = &[
        "TAKO_ISOLATED",
        "TAKO_REMOTE_TEST_MODE",
        "TAKO_TAILSCALE_BIN",
        OPT_IN_ENV,
        LEGACY_ENV,
        LEGACY_2624_ENV,
    ];
    let set = |vars: &[(&str, &str)]| {
        for n in NAMES {
            std::env::remove_var(n);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
    };
    let scratch = tako_core::test_residue::ScratchDir::new("issue2624-gate");
    let fake = scratch.path().join("fake-tailscale");
    let fake = fake.to_str().expect("utf-8");
    let iso = ("TAKO_ISOLATED", "1");
    // (env, [後始末, 導入, ビューア])
    type Env<'a> = Vec<(&'a str, &'a str)>;
    let table: Vec<(Env, [bool; 3])> = vec![
        (vec![], [true, true, true]),
        (vec![("TAKO_ISOLATED", "0")], [true, true, true]),
        (vec![iso], [false, false, false]),
        (
            vec![iso, ("TAKO_REMOTE_TEST_MODE", "1")],
            [false, false, false],
        ),
        (
            vec![iso, ("TAKO_TAILSCALE_BIN", fake)],
            [true, false, false],
        ),
        (
            vec![iso, ("TAKO_TAILSCALE_BIN", "tailscale")],
            [false, false, false],
        ),
        (vec![iso, (OPT_IN_ENV, "1")], [true, true, true]),
        (vec![iso, (LEGACY_ENV, "1")], [true, false, false]),
        (vec![iso, (LEGACY_2624_ENV, "1")], [true, true, true]),
        (vec![iso, (LEGACY_2624_ENV, "0")], [false, false, false]),
    ];
    for (vars, want) in &table {
        set(vars);
        let got = [
            allowed(Op::ServeCleanup),
            check_install().is_ok(),
            allowed(Op::OpenViewer),
        ];
        assert_eq!(&got, want, "{vars:?}");
    }
    // 導入の断りは日英（英語に日本語が混ざらない・opt-in を名指す）
    set(&[iso]);
    set_lang(Lang::En);
    let en = check_install().expect_err("断る").to_string();
    assert!(
        en.contains("TAKO_REMOTE_ISOLATED_SERVE=1")
            && !en.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)
                || ('\u{4e00}'..='\u{9fff}').contains(&c)),
        "{en}"
    );
    set_lang(Lang::Ja);
    let ja = check_install().expect_err("断る").to_string();
    assert!(
        ja.contains("隔離中") && ja.contains("TAKO_REMOTE_ISOLATED_SERVE=1"),
        "{ja}"
    );
    set(&[]);
}

/// 実経路: 実際の `check` が env で答えを変える（**tailscale は起こさない**。判定だけを縛る）。
///
/// - 隔離でない（本番）は常に通す
/// - 隔離中: 理由が無ければ日英の断り（英語に日本語が混ざらない）。検証モード・偽 tailscale・opt-in・A/B で通す
/// - 素の名前・本物の候補を名指す `TAKO_TAILSCALE_BIN` は偽物と見なさない
#[test]
fn 実際の門はenvで答えを変える() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(実際の門はenvで答えを変える) {
        return;
    }
    use tako_control::remote_isolated_serve::{check, LEGACY_ENV, OPT_IN_ENV};
    use tako_core::i18n::{set_lang, Lang};
    const NAMES: &[&str] = &[
        "TAKO_ISOLATED",
        "TAKO_REMOTE_TEST_MODE",
        "TAKO_TAILSCALE_BIN",
        OPT_IN_ENV,
        LEGACY_ENV,
    ];
    let with = |vars: &[(&str, &str)]| {
        for n in NAMES {
            std::env::remove_var(n);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        check()
    };
    let scratch = tako_core::test_residue::ScratchDir::new("issue2619-gate");
    let fake = scratch.path().join("fake-tailscale");
    let fake = fake.to_str().expect("utf-8");

    // 本番（隔離でない）は常に通す
    assert!(with(&[]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "0")]).is_ok());
    assert!(with(&[("TAKO_TAILSCALE_BIN", "tailscale")]).is_ok());

    // 隔離中で理由が無い → 断り（日英）
    for v in ["1", "true", "on"] {
        assert!(with(&[("TAKO_ISOLATED", v)]).is_err(), "TAKO_ISOLATED={v}");
    }
    set_lang(Lang::En);
    let en = with(&[("TAKO_ISOLATED", "1")])
        .expect_err("断る")
        .to_string();
    assert!(
        en.contains("TAKO_REMOTE_ISOLATED_SERVE=1")
            && !en.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)),
        "{en}"
    );
    set_lang(Lang::Ja);
    let ja = with(&[("TAKO_ISOLATED", "1")])
        .expect_err("断る")
        .to_string();
    assert!(ja.contains("隔離中"), "{ja}");

    // 隔離中でも通す
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_REMOTE_TEST_MODE", "1")]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", fake)]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "1"), (OPT_IN_ENV, "1")]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "1"), (LEGACY_ENV, "1")]).is_ok());

    // 偽物と見なさない差し替え・偽の値
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", "tailscale")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", "")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_REMOTE_TEST_MODE", "0")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), (OPT_IN_ENV, "0")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), (LEGACY_ENV, "0")]).is_err());
    #[cfg(target_os = "macos")]
    for real in ["/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale"] {
        if Path::new(real).exists() {
            assert!(
                with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", real)]).is_err(),
                "本物の候補 {real} を偽物と読んだ"
            );
        }
    }
    for n in NAMES {
        std::env::remove_var(n);
    }
}
