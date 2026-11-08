//! remote daemon が serve を張った後のどの出口で断っても、自分が張った serve を外すことの番犬（#2793）
//!
//! # なぜ要るか
//!
//! `run_daemon` は tailscale の serve を張った**後**にも断る出口を持つ（管理トークンの生成・デバイスレジストリの
//! 破損・PID / トークンの書き出し・シグナル監視 / 自己疎通スレッドの起動）。#2793 より前は明示で外す出口
//! （URL の書き出しの失敗・自己疎通の 502）だけが serve を外し、残りの `?` の出口は serve の設定
//! （HTTPS:443 → `http://127.0.0.1:<ポート>`）を、誰も待ち受けていないループバックのポートへ向けたまま返っていた。
//! そのポートを別のローカルのプロセスが取れば tailnet の HTTPS がそこへ届く。
//! いまは #2781 のガード `OwnedDaemonFiles` が「張った serve」を持ち、Drop で外す（出口ごとの後片付けではない
//! ので出口を増やしても漏れない）。**崩れ方は出口の側ではなくガードの持たせ方と外し方にある**ので、ここはそれを
//! 縛る。外す・外さないの振る舞いは `remote::tests::issue2793_*`、実 daemon の断りの後に serve の設定が残らない
//! ことは `crates/tako-cli/tests/issue2793_remote_refused_daemon_releases_serve.rs` が見る。
//!
//! # 何を縛るか
//!
//! 1. `establish_tailscale_serve` はガードへ張った serve を持たせ（`owned.hold_serve(`）、それは serve を書く
//!    呼び出し（`serve_start_target(_on)` / `serve_stop(_on)`）より**前**にある（張った後に持たせると、間で
//!    断った出口が外さない）
//! 2. `hold_serve` を呼ぶのは `establish_tailscale_serve` だけ（`TAKO_REMOTE_TEST_MODE=1` は serve を張らない
//!    ので、`run_daemon` の test_mode の枝で持たせると本物の設定を読み・外しうる）
//! 3. `run_daemon` は serve を直に外さない（`serve_stop_if_ours(_on)` / `serve_stop(_on)`）。外すのはガードの
//!    `owned.release_serve(` の 1 実装（持ち主の確かめと audit.log の記録をそこに集める）。
//!    `establish_tailscale_serve` にはガード（`&mut owned`）を渡す
//! 4. ガードの Drop は `self.release_serve(` を呼び、A/B（`self.serve_legacy` = `TAKO_2793_LEGACY=1`）を見て、
//!    ファイルの A/B の早期 return（`if self.legacy`）より**前**に置く（後ろだと `TAKO_2781_LEGACY=1` が serve
//!    まで外さなくする）
//! 5. `release_serve` は自分の到達先を向いているときだけ外し（`serve_stop_if_ours_on(`。`serve_stop(_on)` は
//!    利用者の設定・別の daemon の設定まで外す）、持っている serve を取り出して 1 回だけ外す（`.take()`）
//! 6. `release_serve` と Drop は stdout / stderr へ書かない（`spawn_daemon` が pipe を捨てた後は EPIPE で panic
//!    し、巻き戻しの最中の Drop の panic は abort になる）
//! 7. ガードの `bound()` は A/B の env（`OWNED_SERVE_LEGACY_ENV`）を読む
//!
//! Drop を飛ばす終わり方（`process::exit(` / `mem::forget(`）は #2781 の番犬が縛る。
//! [`修正前の形と崩れ方を名指しする`] で、修正前の形と崩れ方の注入が file:line で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const RUN_DAEMON: &str = "pub fn run_daemon() -> io::Result<()> {";
const ESTABLISH: &str = "fn establish_tailscale_serve(";
const DROP_IMPL: &str = "impl Drop for OwnedDaemonFiles {";
const RELEASE_FN: &str = "fn release_serve(";
const BOUND_FN: &str = "fn bound() -> Self {";
const HOLD: &str = "owned.hold_serve(";
const RELEASE: &str = "owned.release_serve(";
/// serve を書く呼び出し（張る・外す）
const WRITERS: [&str; 4] = [
    "serve_start_target(",
    "serve_start_target_on(",
    "serve_stop(",
    "serve_stop_on(",
];
/// serve を直に外す呼び出し（`run_daemon` で書かない）
const DIRECT_RELEASES: [&str; 4] = [
    "serve_stop_if_ours(",
    "serve_stop_if_ours_on(",
    "serve_stop(",
    "serve_stop_on(",
];
const PRINTS: [&str; 4] = ["println!(", "eprintln!(", "print!(", "eprint!("];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let raw = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, Vec<String>)> {
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
    Some((
        start + 1,
        lines[start..=end].iter().map(|l| l.to_string()).collect(),
    ))
}

/// 行のコード部分（`//` より前）
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

/// 行が `needle`（`名前(`）を呼んでいるか（宣言・長い名前の一部は数えない）
fn calls(line: &str, needle: &str) -> bool {
    let c = code(line);
    c.match_indices(needle).any(|(i, _)| {
        !c[..i].trim_end().ends_with("fn")
            && c[..i]
                .chars()
                .last()
                .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'))
    })
}

/// 窓の中で `needle` を含む最初のコードの行（窓の中の添字）
fn find(window: &[String], needle: &str) -> Option<usize> {
    window.iter().position(|l| code(l).contains(needle))
}

/// 行を囲む関数の名前（字下げが浅い直近の `fn` 宣言）
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
        let name: String = t[pos + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

/// 1. 張る前にガードへ持たせる
fn scan_establish(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = fn_window(src, ESTABLISH) else {
        out.push(format!("{REMOTE}:0 — `{ESTABLISH}` が無い（走査の空振り）"));
        return;
    };
    let writes: Vec<usize> = w
        .iter()
        .enumerate()
        .filter(|(_, l)| WRITERS.iter().any(|n| calls(l, n)))
        .map(|(i, _)| i)
        .collect();
    if writes.is_empty() {
        out.push(format!(
            "{REMOTE}:{at} — `establish_tailscale_serve` に serve を張る呼び出しが見つからない（走査の空振り）"
        ));
    }
    match find(&w, HOLD) {
        None => out.push(format!(
            "{REMOTE}:{at} — `establish_tailscale_serve` がガードへ張った serve を持たせていない（`{HOLD}`）。\
             serve を張った後の出口（デバイスレジストリの破損ほか）で断ると、serve の設定が誰も待ち受けていない\
             ループバックのポートを向いたまま残る（#2793）"
        )),
        Some(h) => {
            for i in writes.into_iter().filter(|&i| i < h) {
                out.push(format!(
                    "{REMOTE}:{} — `establish_tailscale_serve` が serve を書く呼び出しをガードへ持たせる（`{HOLD}`・\
                     {}行目）より前に置いている。ここで張って断ると外されない（#2793）: {}",
                    at + i,
                    at + h,
                    w[i].trim()
                ));
            }
        }
    }
}

/// 2. `hold_serve` を呼ぶのは `establish_tailscale_serve` だけ
fn scan_hold_callers(src: &str, out: &mut Vec<String>) {
    let lines: Vec<&str> = src.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if !calls(l, "hold_serve(") {
            continue;
        }
        let func = enclosing_fn(&lines, i).unwrap_or_default();
        if func != "establish_tailscale_serve" {
            out.push(format!(
                "{REMOTE}:{} — `{func}` がガードへ serve を持たせている（`hold_serve(`）。持たせるのは serve を張る\
                 `establish_tailscale_serve` だけ（`TAKO_REMOTE_TEST_MODE=1` は serve を張らないので、本物の設定を\
                 読み・外しうる。#2793）",
                i + 1
            ));
        }
    }
}

/// 3. `run_daemon` は serve を直に外さず、ガードを渡して張らせる
fn scan_daemon(src: &str, out: &mut Vec<String>) {
    let Some((at, w)) = fn_window(src, RUN_DAEMON) else {
        out.push(format!(
            "{REMOTE}:0 — `{RUN_DAEMON}` が無い（走査の空振り）"
        ));
        return;
    };
    for (i, l) in w.iter().enumerate() {
        if let Some(direct) = DIRECT_RELEASES.iter().find(|n| calls(l, n)) {
            out.push(format!(
                "{REMOTE}:{} — `run_daemon` が serve を直に外している（`{direct}`）。外すのはガードの `{RELEASE}` の\
                 1 実装（持ち主の確かめ・audit.log の記録・外した後の Drop を空にする。#2793）",
                at + i
            ));
        }
    }
    match w.iter().position(|l| calls(l, "establish_tailscale_serve(")) {
        None => out.push(format!(
            "{REMOTE}:{at} — `run_daemon` に `establish_tailscale_serve(` が見つからない（走査の空振り）"
        )),
        Some(i) if !code(&w[i]).contains("&mut owned") => out.push(format!(
            "{REMOTE}:{} — `run_daemon` が `establish_tailscale_serve` にガード（`&mut owned`）を渡していない。\
             張った serve をガードが持たない（#2793）",
            at + i
        )),
        Some(_) => {}
    }
    if find(&w, RELEASE).is_none() {
        out.push(format!(
            "{REMOTE}:{at} — `run_daemon` が正常な終了で `{RELEASE}` を呼んでいない（走査の空振りか、終了の後始末が\
             ガードの Drop へ落ちて記録の出口が「断った」になる。#2793）"
        ));
    }
}

/// 4〜7. ガードの Drop・`release_serve`・`bound()`
fn scan_guard(src: &str, out: &mut Vec<String>) {
    match fn_window(src, DROP_IMPL) {
        None => out.push(format!("{REMOTE}:0 — `{DROP_IMPL}` が無い（走査の空振り）")),
        Some((at, w)) => {
            match find(&w, "self.release_serve(") {
                None => out.push(format!(
                    "{REMOTE}:{at} — ガードの Drop が張った serve を外していない（`self.release_serve(`）。\
                     serve を張った後の出口で断ると serve の設定が残る（#2793）"
                )),
                Some(r) => {
                    if find(&w, "self.serve_legacy").is_none_or(|g| g > r) {
                        out.push(format!(
                            "{REMOTE}:{} — ガードの Drop が serve を外す前に A/B（`self.serve_legacy` = \
                             `TAKO_2793_LEGACY=1`）を見ていない（#2793）",
                            at + r
                        ));
                    }
                    if let Some(l) = find(&w, "if self.legacy").filter(|&l| l < r) {
                        out.push(format!(
                            "{REMOTE}:{} — ガードの Drop がファイルの A/B の早期 return（`if self.legacy`・{}行目）の\
                             後で serve を外している。`TAKO_2781_LEGACY=1` が serve まで外さなくする（#2793）",
                            at + r,
                            at + l
                        ));
                    }
                }
            }
            scan_prints(at, &w, "ガードの Drop", out);
        }
    }
    match fn_window(src, RELEASE_FN) {
        None => out.push(format!(
            "{REMOTE}:0 — `{RELEASE_FN}` が無い（走査の空振り）"
        )),
        Some((at, w)) => {
            if !w.iter().any(|l| calls(l, "serve_stop_if_ours_on(")) {
                out.push(format!(
                    "{REMOTE}:{at} — `release_serve` が自分の到達先を向いているかを確かめずに外している\
                     （`serve_stop_if_ours_on(` を通っていない）。利用者の設定・別の daemon の設定まで外す（#2793）"
                ));
            }
            for (i, l) in w.iter().enumerate() {
                if let Some(n) = ["serve_stop(", "serve_stop_on("]
                    .iter()
                    .find(|n| calls(l, n))
                {
                    out.push(format!(
                        "{REMOTE}:{} — `release_serve` が serve を確かめずに外す呼び出し（`{n}`）を使っている（#2793）",
                        at + i
                    ));
                }
            }
            if find(&w, "self.serve.take()").is_none() {
                out.push(format!(
                    "{REMOTE}:{at} — `release_serve` が持っている serve を取り出していない（`self.serve.take()`）。\
                     明示で外した後に Drop がもう一度外しにいく（#2793）"
                ));
            }
            scan_prints(at, &w, "`release_serve`", out);
        }
    }
    match fn_window(src, BOUND_FN) {
        Some((at, w)) if find(&w, "OWNED_SERVE_LEGACY_ENV").is_none() => out.push(format!(
            "{REMOTE}:{at} — ガードの `bound()` が A/B の env（`OWNED_SERVE_LEGACY_ENV`）を読んでいない（#2793）"
        )),
        Some(_) => {}
        None => out.push(format!("{REMOTE}:0 — ガードの `{BOUND_FN}` が無い（走査の空振り）")),
    }
}

/// 6. stdout / stderr へ書かない
fn scan_prints(at: usize, w: &[String], what: &str, out: &mut Vec<String>) {
    for (i, l) in w.iter().enumerate() {
        if let Some(p) = PRINTS.iter().find(|p| calls(l, p)) {
            out.push(format!(
                "{REMOTE}:{} — {what} が stdout / stderr へ書いている（`{p}`）。`spawn_daemon` が pipe を捨てた後は\
                 EPIPE で panic し、巻き戻しの最中の Drop の panic は abort になる（#2793）",
                at + i
            ));
        }
    }
}

fn scan_all(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    scan_establish(src, &mut out);
    scan_hold_callers(src, &mut out);
    scan_daemon(src, &mut out);
    scan_guard(src, &mut out);
    out
}

#[test]
fn daemonは張る前にガードへserveを持たせガードの1実装で外す() {
    let failures = scan_all(&read(REMOTE));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// 正しい形。注入はここからの置き換えで作る
const GOOD: &str = "\
pub fn run_daemon() -> io::Result<()> {
    let (server, endpoint) = local_endpoint::bind(&spec)?;
    let mut owned = OwnedDaemonFiles::bound();
    let (serve_handle, base_url) = if test_mode {
        (Arc::new(Mutex::new(handle)), url)
    } else {
        establish_tailscale_serve(&endpoint, &mut owned)?
    };
    let token = crate::generate_token()?;
    owned.release_serve(\"daemon_exit\");
    Ok(())
}
fn establish_tailscale_serve(
    endpoint: &local_endpoint::Endpoint,
    owned: &mut OwnedDaemonFiles,
) -> io::Result<(Arc<Mutex<ServeHandle>>, String)> {
    let state = crate::tailscale::serve_state_on(&cli, socket)?;
    owned.hold_serve(shared.clone(), our_target.clone());
    match state {
        NotConfigured => crate::tailscale::serve_start_target_on(&cli, socket, &our_target)?,
        _ => {}
    }
    Ok((shared, base_url))
}
impl OwnedDaemonFiles {
    fn bound() -> Self {
        owned.serve_legacy = std::env::var_os(OWNED_SERVE_LEGACY_ENV).is_some();
    }
    fn release_serve(&mut self, cause: &str) {
        let Some(held) = self.serve.take() else {
            return;
        };
        let _ = crate::tailscale::serve_stop_if_ours_on(&cli, socket, Some(&held.target));
    }
}
impl Drop for OwnedDaemonFiles {
    fn drop(&mut self) {
        if !self.serve_legacy {
            self.release_serve(cause);
        }
        if self.legacy {
            return;
        }
    }
}
";

fn replaced(base: &str, from: &str, to: &str) -> String {
    assert!(base.contains(from), "注入の元が無い: {from}");
    base.replacen(from, to, 1)
}

/// 注入した形の中で `needle` を含む最初の行の 1-based 行番号
fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("注入した形に {needle:?} が無い"))
        + 1
}

fn assert_names(src: &str, wants: &[String]) {
    let joined = scan_all(src).join("\n");
    for want in wants {
        assert!(
            joined.contains(want.as_str()),
            "名指せていない: {want}\n---\n{joined}"
        );
    }
}

#[test]
fn 修正前の形と崩れ方を名指しする() {
    // 正しい形は通る（以下の注入が空振りでないことの前提）
    assert!(scan_all(GOOD).is_empty(), "{:?}", scan_all(GOOD));

    // 修正前: ガードは serve を持たず、Drop は外さず、run_daemon の終わりで直に外す
    let before = replaced(
        GOOD,
        "    owned.hold_serve(shared.clone(), our_target.clone());\n",
        "",
    );
    let before = replaced(
        &before,
        "establish_tailscale_serve(&endpoint, &mut owned)?",
        "establish_tailscale_serve(&endpoint)?",
    );
    let before = replaced(
        &before,
        "        if !self.serve_legacy {\n            self.release_serve(cause);\n        }\n",
        "",
    );
    let before = replaced(
        &before,
        "    owned.release_serve(\"daemon_exit\");\n",
        "    let _ = crate::tailscale::serve_stop_if_ours_on(&cli, socket, Some(&target));\n",
    );
    assert_names(
        &before,
        &[
            format!(
                "{REMOTE}:{} — `establish_tailscale_serve` がガードへ張った serve を持たせていない",
                line_of(&before, ESTABLISH)
            ),
            format!(
                "{REMOTE}:{} — `run_daemon` が `establish_tailscale_serve` にガード（`&mut owned`）を渡していない",
                line_of(&before, "establish_tailscale_serve(&endpoint)?")
            ),
            format!(
                "{REMOTE}:{} — `run_daemon` が serve を直に外している（`serve_stop_if_ours_on(`）",
                line_of(&before, "let _ = crate::tailscale::serve_stop_if_ours_on(&cli, socket, Some(&target))")
            ),
            format!(
                "{REMOTE}:{} — ガードの Drop が張った serve を外していない",
                line_of(&before, DROP_IMPL)
            ),
        ],
    );

    // 張った後に持たせる（間で断ると外されない）
    let late = replaced(
        GOOD,
        "    owned.hold_serve(shared.clone(), our_target.clone());\n    match state {\n        NotConfigured => crate::tailscale::serve_start_target_on(&cli, socket, &our_target)?,\n        _ => {}\n    }\n",
        "    match state {\n        NotConfigured => crate::tailscale::serve_start_target_on(&cli, socket, &our_target)?,\n        _ => {}\n    }\n    owned.hold_serve(shared.clone(), our_target.clone());\n",
    );
    assert_names(
        &late,
        &[format!(
            "{REMOTE}:{} — `establish_tailscale_serve` が serve を書く呼び出しをガードへ持たせる",
            line_of(&late, "NotConfigured => ")
        )],
    );

    // test_mode の枝で持たせる
    let test_mode = replaced(
        GOOD,
        "        (Arc::new(Mutex::new(handle)), url)\n",
        "        owned.hold_serve(shared.clone(), target.clone());\n        (Arc::new(Mutex::new(handle)), url)\n",
    );
    assert_names(
        &test_mode,
        &[format!(
            "{REMOTE}:{} — `run_daemon` がガードへ serve を持たせている（`hold_serve(`）",
            line_of(
                &test_mode,
                "owned.hold_serve(shared.clone(), target.clone())"
            )
        )],
    );

    // Drop がファイルの A/B の後で外す・serve の A/B を見ない
    let after_legacy = replaced(
        GOOD,
        "        if !self.serve_legacy {\n            self.release_serve(cause);\n        }\n        if self.legacy {\n            return;\n        }\n",
        "        if self.legacy {\n            return;\n        }\n        self.release_serve(cause);\n",
    );
    assert_names(
        &after_legacy,
        &[
            format!(
                "{REMOTE}:{} — ガードの Drop がファイルの A/B の早期 return（`if self.legacy`・{}行目）の後で serve を外している",
                line_of(&after_legacy, "self.release_serve(cause)"),
                line_of(&after_legacy, "if self.legacy")
            ),
            format!(
                "{REMOTE}:{} — ガードの Drop が serve を外す前に A/B（`self.serve_legacy`",
                line_of(&after_legacy, "self.release_serve(cause)")
            ),
        ],
    );

    // release_serve が確かめずに外し、取り出さず、stderr へ書く。bound() が A/B の env を読まない
    let blind = replaced(
        GOOD,
        "        let _ = crate::tailscale::serve_stop_if_ours_on(&cli, socket, Some(&held.target));\n",
        "        let _ = crate::tailscale::serve_stop_on(&cli, socket);\n        eprintln!(\"serve を外した\");\n",
    );
    let blind = replaced(&blind, "self.serve.take()", "self.serve.clone()");
    let blind = replaced(&blind, "OWNED_SERVE_LEGACY_ENV", "OTHER_ENV");
    assert_names(
        &blind,
        &[
            format!(
                "{REMOTE}:{} — `release_serve` が自分の到達先を向いているかを確かめずに外している",
                line_of(&blind, RELEASE_FN)
            ),
            format!(
                "{REMOTE}:{} — `release_serve` が serve を確かめずに外す呼び出し（`serve_stop_on(`）",
                line_of(&blind, "serve_stop_on(&cli, socket)")
            ),
            format!(
                "{REMOTE}:{} — `release_serve` が持っている serve を取り出していない",
                line_of(&blind, RELEASE_FN)
            ),
            format!(
                "{REMOTE}:{} — `release_serve` が stdout / stderr へ書いている（`eprintln!(`）",
                line_of(&blind, "eprintln!(")
            ),
            format!(
                "{REMOTE}:{} — ガードの `bound()` が A/B の env（`OWNED_SERVE_LEGACY_ENV`）を読んでいない",
                line_of(&blind, BOUND_FN)
            ),
        ],
    );
}
