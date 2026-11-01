//! daemon を起こしている途中に打った start / stop を断ることの番犬（#2613）
//!
//! # なぜ要るか
//!
//! 起動の 1 実装 `spawn_daemon` は、抜けるまで「起こしている途中」の印（#2608。
//! `<state_dir>/tako-remote.starting`）を置く。その印が効いている間に人や master が打った
//! `tako remote start`（CLI / MCP / 起動ボタン）は印を見ていなかった。段 1 で隔離 GUI に再現した:
//!
//! - 自動復帰（#1485）が旧版を入れ替えている（#2518）途中の start は、旧版を自分も「入れ替える相手」と
//!   読んで止めに行き子を起こす = daemon が 2 本立ち、PID ファイルに載らない 1 本が残った
//! - 新版が serve を張っている途中の start は、断る前に印を上書きし、抜けるときに消す = 残りの区間の
//!   status が #2455 の stop の案内へ戻った（#2608 の症状が戻る）
//! - 同じ区間の stop は「停止しました」と返しても進行中の起動が新版を起こし直すか、PID ファイルも
//!   管理トークンも無いので止められずに `kill` / 記録の `rm` を案内した
//!
//! # 何を縛るか
//!
//! 1. `spawn_daemon` は**状態を読むより前**に効いている印を見て、型の断り（`RemoteRefusal::DaemonStarting`）で返す
//! 2. `daemon_stop` / `daemon_force_stop` は頼んだ記録（#2533）の後・止める前に同じ印を見て断る
//! 3. 入れ替えの旧版の停止（`stop_stale_generation`）は明示の stop の入口を通らない（自分の印で自分を断らない）
//! 4. 判定は `active_start_marker`（`remote_starting::is_active`）の 1 実装を status（#2608）と共有し、
//!    A/B は status の `TAKO_2608_LEGACY` と分ける（`TAKO_2613_LEGACY`）
//! 5. CLI の `tako remote start` / `stop` は断りを組む前に表示言語を決める（静的既定の英語で凍らせない）
//! 6. 実際の `spawn_daemon` / `daemon_stop` が、印の有無・言語・A/B で答えを変える（実経路）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので [`走査が空振りしていない`] で窓を固定し、
//! [`修正前の形へ戻すと名指しする`] で**修正前を再現した注入**が file:line で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

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

fn read_remote(root: &Path) -> String {
    read(root, REMOTE)
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

/// 違反を `ファイル:行 — 理由` で返す
fn all(remote: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: &str| out.push(format!("{REMOTE}:{line} — {why}"));

    // 1. spawn_daemon は状態を読むより前に印を見て、型の断りで返す
    match fn_window(remote, "pub fn spawn_daemon()") {
        None => push(0, "`spawn_daemon` が無い（走査の空振り）"),
        Some((at, w)) => {
            let check = code_line(at, &w, "start_blocks_request()");
            let status = code_line(at, &w, "daemon_status()");
            let begin = code_line(at, &w, "StartingGuard::begin(");
            match check {
                None => push(
                    at,
                    "`spawn_daemon` が「起こしている途中」の印（`start_blocks_request`）を見ずに起こしに行く。\
                     入れ替えの途中の start が旧版を止めて daemon を 2 本にする / 印を上書きして消す（#2613）",
                ),
                Some(line) => {
                    if status.is_some_and(|s| s < line) {
                        push(
                            line,
                            "印を状態の読み取り（`daemon_status()`）より後で見ている。状態で決める断り\
                             （serve バイナリの食い違いで stop を勧める）が先に返る（#2613）",
                        );
                    }
                    if begin.is_some_and(|b| b < line) {
                        push(
                            line,
                            "印を自分の印（`StartingGuard::begin`）を置いた後で見ている。自分の印で自分を断るか、\
                             先の起動の印を上書きしてから見る（#2613）",
                        );
                    }
                    match code_line(at, &w, "RemoteRefusal::DaemonStarting {") {
                        Some(r) if r > line => {}
                        _ => push(
                            line,
                            "印で断るときの文が型の断り（`RemoteRefusal::DaemonStarting`）を通っていない。\
                             英語表示に日本語が混ざる（#2365 / #2613）",
                        ),
                    }
                    if code_line(at, &w, "audit_refused_while_starting(\"daemon_start_refused\"")
                        .is_none()
                    {
                        push(
                            line,
                            "印で断ったことを audit.log（`daemon_start_refused`）へ残していない（#2533 / #2613）",
                        );
                    }
                }
            }
            if status.is_none() || begin.is_none() {
                push(
                    at,
                    "`spawn_daemon` の状態の読み取り / 印の設置が見つからない（走査の空振り）",
                );
            }
        }
    }

    // 2. 明示の stop の入口は、頼んだ記録の後・止める前に印を見て断る
    for (needle, force) in [
        ("pub fn daemon_stop()", "false"),
        ("pub fn daemon_force_stop()", "true"),
    ] {
        match fn_window(remote, needle) {
            None => push(0, &format!("`{needle}` が無い（走査の空振り）")),
            Some((at, w)) => {
                let audit = code_line(at, &w, "audit_daemon_request(");
                let refuse = code_line(at, &w, "refuse_stop_while_starting()?");
                let stop = code_line(at, &w, &format!("daemon_stop_impl({force})"));
                match (audit, refuse, stop) {
                    (_, None, _) => push(
                        at,
                        &format!(
                            "`{needle}` が「起こしている途中」の印を見ずに止めに行く。止めても進行中の起動が \
                             新版を起こし直す / 止められずに `kill`・記録の `rm` を案内する（#2613）"
                        ),
                    ),
                    (_, _, None) => push(at, &format!("`{needle}` が止めていない（走査の空振り）")),
                    (Some(a), Some(r), Some(s)) if a < r && r < s => {}
                    (_, Some(r), _) => push(
                        r,
                        &format!(
                            "`{needle}` の印の確認の位置が違う。頼んだ記録（#2533）の後・止める前に置く（#2613）"
                        ),
                    ),
                }
            }
        }
    }
    match fn_window(remote, "fn refuse_stop_while_starting()") {
        None => push(0, "`refuse_stop_while_starting` が無い（走査の空振り）"),
        Some((at, w)) => {
            if code_line(at, &w, "start_blocks_request()").is_none() {
                push(
                    at,
                    "stop の断りが判定の 1 実装（`start_blocks_request`）を通っていない（#2613）",
                );
            }
            if code_line(at, &w, "RemoteRefusal::DaemonStartingStop {").is_none() {
                push(
                    at,
                    "stop の断りが型の断り（`RemoteRefusal::DaemonStartingStop`）を通っていない（#2613）",
                );
            }
        }
    }

    // 3. 入れ替えの旧版の停止は明示の stop の入口を通らない（自分の印で自分を断らない）
    match fn_window(remote, "fn stop_stale_generation(") {
        None => push(0, "`stop_stale_generation` が無い（走査の空振り）"),
        Some((at, w)) => {
            for bad in [
                "refuse_stop_while_starting(",
                "daemon_stop()",
                "daemon_force_stop()",
            ] {
                if let Some(line) = code_line(at, &w, bad) {
                    push(
                        line,
                        &format!(
                            "入れ替えの旧版の停止が明示の stop の入口（`{bad}`）を通っている。\
                             自分が置いた印で自分の入れ替えを断る（#2613）"
                        ),
                    );
                }
            }
            if code_line(at, &w, "daemon_stop_impl(").is_none() {
                push(
                    at,
                    "`stop_stale_generation` が旧版を止めていない（走査の空振り）",
                );
            }
        }
    }

    // 4. 判定は 1 実装・A/B は status と分ける
    match fn_window(remote, "fn start_blocks_request()") {
        None => push(0, "`start_blocks_request` が無い（走査の空振り）"),
        Some((at, w)) => {
            if code_line(at, &w, "remote_starting::start_legacy_mode()").is_none() {
                push(
                    at,
                    "明示の start / stop の A/B（`TAKO_2613_LEGACY`）を見ていない（#2613）",
                );
            }
            if let Some(line) = code_line(at, &w, "remote_starting::legacy_mode()") {
                push(
                    line,
                    "明示の start / stop の断りが status の A/B（`TAKO_2608_LEGACY`）で外れる。\
                     片方だけを戻して比べられない（#2613）",
                );
            }
            if code_line(at, &w, "active_start_marker()").is_none() {
                push(
                    at,
                    "判定の 1 実装（`active_start_marker`）を通っていない（#2613）",
                );
            }
        }
    }
    match fn_window(remote, "fn active_start_marker()") {
        None => push(0, "`active_start_marker` が無い（走査の空振り）"),
        Some((at, w)) => {
            if code_line(at, &w, "remote_starting::is_active(").is_none() {
                push(
                    at,
                    "印が効いているかを `remote_starting::is_active` の 1 実装で決めていない（#2608 / #2613）",
                );
            }
            if let Some(line) = code_line(at, &w, "legacy_mode()") {
                push(
                    line,
                    "判定の 1 実装が A/B を持っている。A/B は読む側（status / 明示の start・stop）が持つ（#2613）",
                );
            }
        }
    }
    match fn_window(remote, "fn start_in_progress()") {
        None => push(0, "`start_in_progress` が無い（走査の空振り）"),
        Some((at, w)) => {
            if code_line(at, &w, "active_start_marker()").is_none() {
                push(
                    at,
                    "status の「起こしている途中」が明示の start / stop と別の判定を持っている（#2613）",
                );
            }
        }
    }
    out
}

/// CLI の `tako remote start` / `stop` は断りを組む前に表示言語を決める（違反を `ファイル:行 — 理由` で返す）。
/// CLI の表示言語は静的既定が英語で、呼ぶ側が設定から決めないと日本語表示の人へ英語の断りが出る
fn cli_violations(cli: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (needle, call) in [
        ("fn remote_start()", "remote::spawn_daemon"),
        ("fn remote_stop(", "remote::daemon_"),
    ] {
        match fn_window(cli, needle) {
            None => out.push(format!("{CLI}:0 — `{needle}` が無い（走査の空振り）")),
            Some((at, w)) => match (code_line(at, &w, "init_local_lang()"), code_line(at, &w, call)) {
                (_, None) => out.push(format!(
                    "{CLI}:{at} — `{needle}` が `{call}` を呼んでいない（走査の空振り）"
                )),
                (Some(lang), Some(req)) if lang < req => {}
                (_, Some(req)) => out.push(format!(
                    "{CLI}:{req} — `{needle}` が表示言語を決める（`init_local_lang()`）前に断りを組む。\
                     静的既定の英語で、日本語表示の人へ英語の断りが出る（#2365 / #2613）"
                )),
            },
        }
    }
    out
}

#[test]
fn cliのstartとstopは断りの前に表示言語を決める() {
    let found = cli_violations(&read(&workspace_root(), CLI));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 起こしている途中のstartとstopを断る配線() {
    let found = all(&read_remote(&workspace_root()));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let remote = read_remote(&workspace_root());
    for needle in [
        "pub fn spawn_daemon()",
        "pub fn daemon_stop()",
        "pub fn daemon_force_stop()",
        "fn refuse_stop_while_starting()",
        "fn stop_stale_generation(",
        "fn start_blocks_request()",
        "fn active_start_marker()",
        "fn start_in_progress()",
    ] {
        let (_, w) =
            fn_window(&remote, needle).unwrap_or_else(|| panic!("{REMOTE}: `{needle}` が無い"));
        assert!(
            w.lines().count() >= 3,
            "{REMOTE}: `{needle}` の窓が短すぎる（走査の空振り）"
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
    let base = read_remote(&workspace_root());
    let named =
        |src: &str, what: &str| {
            let found = all(src);
            assert!(
                found.iter().any(|o| o.starts_with(&format!("{REMOTE}:"))
                    && !o.starts_with(&format!("{REMOTE}:0 "))),
                "注入「{what}」を {REMOTE} の行で名指しできていない: {found:?}"
            );
        };

    // 注入 1: 印を見ない（#2613 前の spawn_daemon）
    named(
        &inject(
            &base,
            "if let Some(marker) = start_blocks_request() {\n        audit_refused_while_starting(\"daemon_start_refused\"",
            "if let Some(marker) = None::<crate::remote_starting::StartingMarker> {\n        audit_refused_while_starting(\"daemon_start_refused\"",
            "start が印を見ない",
        ),
        "start が印を見ない",
    );

    // 注入 2: 状態を読んでから印を見る
    named(
        &inject(
            &base,
            "    // #2613: 別の起動が daemon を起こしている途中",
            "    let _early = daemon_status();\n    // #2613: 別の起動が daemon を起こしている途中",
            "状態の後で印を見る",
        ),
        "状態の後で印を見る",
    );

    // 注入 3: 断りの文を日本語の固定文で返す（#2365 前の形）
    named(
        &inject(
            &base,
            "crate::RemoteRefusal::DaemonStarting { marker, now }.to_string()",
            "format!(\"起動中です（PID {}・{now}）\", marker.pid)",
            "日本語の固定文",
        ),
        "日本語の固定文",
    );

    // 注入 4: stop が印を見ない（#2613 前の daemon_stop）
    named(
        &inject(
            &base,
            "    refuse_stop_while_starting()?;\n    forget_if_stopped(daemon_stop_impl(false))",
            "    forget_if_stopped(daemon_stop_impl(false))",
            "stop が印を見ない",
        ),
        "stop が印を見ない",
    );

    // 注入 5: 明示の start / stop の A/B を status の A/B へ混ぜる
    named(
        &inject(
            &base,
            "if crate::remote_starting::start_legacy_mode() {",
            "if crate::remote_starting::legacy_mode() {",
            "A/B を混ぜる",
        ),
        "A/B を混ぜる",
    );

    // 注入 6: 入れ替えの旧版の停止が明示の stop の入口を通る（自分の印で自分を断る）
    named(
        &inject(
            &base,
            "let stopped = daemon_stop_impl(false);",
            "let stopped = daemon_stop();",
            "入れ替えが自分を断る",
        ),
        "入れ替えが自分を断る",
    );

    // 注入 7: CLI の start が表示言語を決めずに呼ぶ
    let cli = read(&workspace_root(), CLI);
    let found = cli_violations(&inject(
        &cli,
        "    // 起動の途中の断り（`RemoteRefusal::DaemonStarting`。#2613）は表示言語で文にするので、呼ぶ前に決める\n    init_local_lang();\n",
        "",
        "CLI が表示言語を決めない",
    ));
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(&format!("{CLI}:")) && !o.starts_with(&format!("{CLI}:0 "))),
        "注入「CLI が表示言語を決めない」を {CLI} の行で名指しできていない: {found:?}"
    );
}

/// 実経路: 隔離した state dir に「起こしている途中」の印を置き、本物の `spawn_daemon` /
/// `daemon_stop` / `daemon_force_stop` を呼ぶ。
///
/// - 印が効いている間の start は日英の断り（英語に日本語が混ざらない・stop を案内しない）で、
///   印を書き換えず、子を起こさない（印を見て返るので、起こす経路まで進まない）
/// - status の A/B（`TAKO_2608_LEGACY=1`）では外れない
/// - 印が効いている間の stop / `--force` は断り、「起動していた」（`desired`）を消さない
/// - 断ったことは audit.log に残る（`daemon_start_refused` / `daemon_stop_refused`）
/// - 陽性対照: 印が無い・書き手が死んでいる・A/B（`TAKO_2613_LEGACY=1`）の stop は断らずに止めに行く
///   （daemon が居ないので「起動していない」で返る）
///
/// 起こす経路を通る陽性対照（印が無ければ start が起こす）は実 daemon が要るので
/// `scripts/test-remote-start-while-starting-2613.sh` が受け持つ。
/// tailscale は存在しないパスを渡す（`TAKO_TAILSCALE_BIN` は指定のパスだけを試す = 本物を叩かない）
#[cfg(unix)]
#[test]
fn 起こしている途中のstartとstopは断る() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(起こしている途中のstartとstopは断る) {
        return;
    }
    use tako_control::remote::{
        daemon_force_stop, daemon_stop, read_desired, set_desired, spawn_daemon, starting_path,
        with_origin, RequestOrigin, StartingGuard,
    };
    use tako_control::remote_starting::{StartingMarker, LEGACY_ENV, START_LEGACY_ENV};
    use tako_core::i18n::{set_lang, Lang};

    let root = workspace_root();
    let remote_src = read_remote(&root);
    let spawn_at = format!(
        "{REMOTE}:{}",
        fn_window(&remote_src, "pub fn spawn_daemon()")
            .expect("spawn_daemon")
            .0
    );
    let stop_at = format!(
        "{REMOTE}:{}",
        fn_window(&remote_src, "fn refuse_stop_while_starting()")
            .expect("refuse_stop_while_starting")
            .0
    );

    let scratch = tako_core::test_residue::ScratchDir::new("tako-2613-start");
    let state = scratch.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(scratch.path().join("data")).unwrap();
    std::env::set_var("TAKO_REMOTE_STATE_DIR", &state);
    std::env::set_var("TAKO_DATA_DIR", scratch.path().join("data"));
    std::env::set_var("TAKO_ISOLATED", "1");
    std::env::set_var(
        "TAKO_TAILSCALE_BIN",
        scratch.path().join("no-such-tailscale"),
    );
    std::env::remove_var(LEGACY_ENV);
    std::env::remove_var(START_LEGACY_ENV);
    std::env::remove_var("TAKO_REMOTE_ENDPOINT");
    set_desired();
    assert!(read_desired().is_some(), "「起動していた」を置けていない");
    // 断りの文を取り出す（断らずに進んだら、何をしたかを添えて落とす）
    let refused = |r: Result<serde_json::Value, String>, what: String| -> String {
        match r {
            Ok(v) => panic!("{what}: {v}"),
            Err(e) => e,
        }
    };

    {
        // GUI 起動時の自動復帰が旧版 PID 4242 を入れ替えている途中
        let _guard = with_origin(RequestOrigin::Autostart, || {
            StartingGuard::begin(Some(4242))
        });
        let marker_before = std::fs::read(starting_path()).expect("印が置かれている");

        set_lang(Lang::Ja);
        let ja = refused(
            with_origin(RequestOrigin::Cli, spawn_daemon),
            format!("{spawn_at}: 印が効いているのに start が断らない（#2613）"),
        );
        assert!(
            ja.contains("入れ替えている途中") && ja.contains("2 本目の daemon を起こさない"),
            "{spawn_at}: {ja}"
        );
        assert!(ja.contains("GUI 起動時の自動復帰"), "{ja}");
        assert!(
            !ja.contains("tako remote stop"),
            "{spawn_at}: stop を案内している: {ja}"
        );

        set_lang(Lang::En);
        let en = refused(
            with_origin(RequestOrigin::Dispatch, spawn_daemon),
            format!("{spawn_at}: 英語表示で断らない"),
        );
        assert!(en.is_ascii(), "{spawn_at}: 英語表示に日本語が混ざる: {en}");
        assert!(en.contains("is being replaced"), "{en}");

        // status の A/B では外れない
        std::env::set_var(LEGACY_ENV, "1");
        let legacy_status = spawn_daemon();
        std::env::remove_var(LEGACY_ENV);
        assert!(
            legacy_status.is_err_and(|e| e.contains("is being replaced")),
            "{spawn_at}: status の A/B（{LEGACY_ENV}=1）で start の断りが外れた"
        );

        // stop / --force も断り、「起動していた」を消さない
        let stop_en = refused(
            daemon_stop(),
            format!("{stop_at}: 印が効いているのに stop が断らない"),
        );
        assert!(
            stop_en.is_ascii() && stop_en.contains("It was not stopped"),
            "{stop_at}: {stop_en}"
        );
        set_lang(Lang::Ja);
        let stop_ja = refused(
            daemon_force_stop(),
            format!("{stop_at}: 印が効いているのに --force が断らない"),
        );
        assert!(stop_ja.contains("止めていません"), "{stop_at}: {stop_ja}");
        assert!(
            read_desired().is_some(),
            "{stop_at}: 断った stop が「起動していた」（desired）を消した"
        );

        assert_eq!(
            std::fs::read(starting_path()).ok().as_deref(),
            Some(marker_before.as_slice()),
            "{spawn_at}: 断った start / stop が印を書き換えた"
        );
    }
    assert!(
        !starting_path().exists(),
        "guard の Drop が印を消していない"
    );

    // audit.log に断った記録が残る（起こした側の経路つき）
    let audit = std::fs::read_to_string(state.join("audit.log")).unwrap_or_default();
    let refused_lines: Vec<serde_json::Value> = audit
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| {
            v["event"]
                .as_str()
                .is_some_and(|e| e == "daemon_start_refused" || e == "daemon_stop_refused")
        })
        .collect();
    assert_eq!(
        refused_lines
            .iter()
            .filter(|v| v["event"] == "daemon_start_refused")
            .count(),
        3,
        "{audit}"
    );
    assert_eq!(
        refused_lines
            .iter()
            .filter(|v| v["event"] == "daemon_stop_refused")
            .count(),
        2,
        "{audit}"
    );
    assert!(
        refused_lines.iter().all(|v| v["starter_via"] == "autostart"
            && v["previous_pid"] == 4242
            && v["state"] == "replacing"),
        "{audit}"
    );

    // 陽性対照: 印が無ければ stop は止めに行く（daemon が居ないので「起動していない」）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let not_running = |what: &str| {
        let e = refused(
            daemon_stop(),
            format!("{what}: daemon が居ないのに止まったと言った"),
        );
        assert!(
            e.contains("起動していない"),
            "{stop_at}: {what}の stop を断っている: {e}"
        );
        set_desired();
    };
    not_running("印が無いとき");

    let write_marker = |m: &StartingMarker| {
        std::fs::write(starting_path(), serde_json::to_string(m).unwrap()).unwrap();
    };
    // 書き手が死んでいる印は効かない
    let mut dead = std::process::Command::new("/bin/sh")
        .args(["-c", ":"])
        .spawn()
        .expect("すぐ終わる sh");
    let dead_pid = dead.id();
    dead.wait().unwrap();
    write_marker(&StartingMarker {
        at: now,
        pid: dead_pid,
        via: "autostart".into(),
        previous_pid: Some(4242),
    });
    not_running("書き手が死んだ印があるとき");

    // A/B（TAKO_2613_LEGACY=1）は効いている印があっても止めに行く = #2613 前
    write_marker(&StartingMarker {
        at: now,
        pid: std::process::id(),
        via: "cli".into(),
        previous_pid: None,
    });
    std::env::set_var(START_LEGACY_ENV, "1");
    not_running("A/B のとき");
    std::env::remove_var(START_LEGACY_ENV);
    let _ = std::fs::remove_file(starting_path());
}
