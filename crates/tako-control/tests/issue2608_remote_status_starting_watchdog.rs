//! daemon を起こしている途中の status が stop を案内しないことの番犬（#2608）
//!
//! # なぜ要るか
//!
//! 起動の 1 実装 `spawn_daemon` が起こす新しい daemon は、待ち受けを開いて port ファイルを書いて
//! から tailscale の serve を張り終えて PID ファイルを書く（#2466 の実測で 8 秒）。その間に別の
//! プロセスが `tako remote status` を読むと「PID ファイルが無いのに記録された待ち受けが応答」=
//! #2455 の「PID ファイルだけが消えた daemon」と同じ形に見え、`tako remote stop` で止めてと
//! 案内していた。11/01 の本番で install 後の GUI の再起動直後（自動復帰 #1485 が旧版を入れ替える
//! #2518 の途中）に観察し、段 1 で隔離 GUI に 8.2 秒の区間として再現した。案内どおり止めると
//! 立ち上がったばかりの新版を止める。
//!
//! # 何を縛るか
//!
//! 1. `spawn_daemon` は旧版の停止より前から抜けるまで「起こしている途中」の印を置く
//!    （`StartingGuard` を名前つきで持つ。`let _ =` はその場で消える）
//! 2. `with_pidless_listener` は印を先に見て、効いていれば `pidless_listener` と stop の案内を出さない
//! 3. daemon の痕跡の掃除（`cleanup_state_files`）は印を消さない（入れ替えで畳む旧版が消してしまう）
//! 4. CLI の `tako remote status` は印の 1 行を stderr にも出す
//! 5. 実際の `daemon_status` が、印の有無・書き手の生死・期限・A/B で答えを変える（実経路）
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

struct Sources {
    remote: String,
    cli: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        remote: read(root, REMOTE),
        cli: read(root, CLI),
    }
}

/// 違反を `ファイル:行 — 理由` で返す
fn all(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |file: &str, line: usize, why: &str| out.push(format!("{file}:{line} — {why}"));

    // 1. spawn_daemon は旧版の停止より前に印を名前つきで持つ
    match fn_window(&s.remote, "pub fn spawn_daemon()") {
        None => push(REMOTE, 0, "`spawn_daemon` が無い（走査の空振り）"),
        Some((at, w)) => {
            let begin = code_line(at, &w, "StartingGuard::begin(");
            let stop = code_line(at, &w, "stop_stale_generation(&status)");
            match begin {
                None => push(
                    REMOTE,
                    at,
                    "`spawn_daemon` が「起こしている途中」の印（`StartingGuard::begin`）を置いていない。\
                     新しい daemon が PID ファイルを書くまでの間、status が stop を案内する（#2608）",
                ),
                Some(line) => {
                    if code_line(at, &w, "let _ = StartingGuard::begin(").is_some() {
                        push(
                            REMOTE,
                            line,
                            "`let _ = StartingGuard::begin(..)` はその場で印を消す。名前つきで \
                             `spawn_daemon` を抜けるまで持つ（#2608）",
                        );
                    }
                    if stop.is_some_and(|s| s < line) {
                        push(
                            REMOTE,
                            line,
                            "印を旧版の停止（`stop_stale_generation`）より後に置いている。\
                             旧版を止めてから新版が PID ファイルを書くまでが入れ替えの途中（#2608）",
                        );
                    }
                }
            }
            if stop.is_none() {
                push(
                    REMOTE,
                    at,
                    "`spawn_daemon` が旧版を止めていない（走査の空振り）",
                );
            }
        }
    }

    // 2. PID ファイルの無い待ち受けの案内より先に印を見る
    match fn_window(&s.remote, "fn with_pidless_listener(") {
        None => push(REMOTE, 0, "`with_pidless_listener` が無い（走査の空振り）"),
        Some((at, w)) => {
            let check = code_line(at, &w, "start_in_progress()");
            let warn = code_line(at, &w, "status[\"pidless_listener\"]");
            match (check, warn) {
                (_, None) => push(REMOTE, at, "`pidless_listener` を載せていない（走査の空振り）"),
                (None, Some(line)) => push(
                    REMOTE,
                    line,
                    "起こしている途中の印を見ずに `pidless_listener` と `tako remote stop` の案内を出している。\
                     起動中の新しい daemon の待ち受けを止めさせる（#2608）",
                ),
                (Some(c), Some(line)) if c > line => push(
                    REMOTE,
                    line,
                    "印を見る前に `pidless_listener` を載せている（#2608）",
                ),
                _ => {}
            }
        }
    }

    // 3. 痕跡の掃除は印を消さない
    match fn_window(&s.remote, "fn cleanup_state_files()") {
        None => push(REMOTE, 0, "`cleanup_state_files` が無い（走査の空振り）"),
        Some((at, w)) => {
            if let Some(line) = code_line(at, &w, "starting_path()") {
                push(
                    REMOTE,
                    line,
                    "`cleanup_state_files` が「起こしている途中」の印を消している。入れ替えで畳む旧版の \
                     daemon が掃除を走らせるので、印が新版の起動の途中で消える（#2608）",
                );
            }
        }
    }

    // 4. CLI の人向けの行
    match fn_window(&s.cli, "fn remote_status()") {
        None => push(CLI, 0, "`remote_status` が無い（走査の空振り）"),
        Some((at, w)) => {
            if code_line(at, &w, "status[\"starting\"][\"detail\"]").is_none() {
                push(
                    CLI,
                    at,
                    "`tako remote status` が起こしている途中の 1 行（`starting.detail`）を stderr に出していない（#2608）",
                );
            }
        }
    }
    out
}

#[test]
fn 起こしている途中の印を配線している() {
    let found = all(&sources(&workspace_root()));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = sources(&root);
    for (src, file, needle) in [
        (&s.remote, REMOTE, "pub fn spawn_daemon()"),
        (&s.remote, REMOTE, "fn with_pidless_listener("),
        (&s.remote, REMOTE, "fn cleanup_state_files()"),
        (&s.cli, CLI, "fn remote_status()"),
    ] {
        let (_, w) = fn_window(src, needle).unwrap_or_else(|| panic!("{file}: `{needle}` が無い"));
        assert!(
            w.lines().count() >= 5,
            "{file}: `{needle}` の窓が短すぎる（走査の空振り）"
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
    let base = sources(&root);
    let named = |s: &Sources, file: &str, what: &str| {
        let found = all(s);
        assert!(
            found.iter().any(|o| o.starts_with(&format!("{file}:"))),
            "注入「{what}」を {file} で名指しできていない: {found:?}"
        );
    };

    // 注入 1: 印を置かない（#2608 前の spawn_daemon）
    let s = Sources {
        remote: inject(
            &base.remote,
            "let _starting = StartingGuard::begin(renewing);",
            "",
            "印を置かない",
        ),
        cli: base.cli.clone(),
    };
    named(&s, REMOTE, "印を置かない");

    // 注入 2: 名前の無い束縛（その場で消える）
    let s = Sources {
        remote: inject(
            &base.remote,
            "let _starting = StartingGuard::begin(renewing);",
            "let _ = StartingGuard::begin(renewing);",
            "その場で消える束縛",
        ),
        cli: base.cli.clone(),
    };
    named(&s, REMOTE, "その場で消える束縛");

    // 注入 3: 印を見ずに #2455 の案内を出す（#2608 前の with_pidless_listener）
    let s = Sources {
        remote: inject(
            &base.remote,
            "if let Some(marker) = start_in_progress() {\n        let described",
            "if let Some(marker) = None::<crate::remote_starting::StartingMarker> {\n        let described",
            "印を見ない",
        ),
        cli: base.cli.clone(),
    };
    named(&s, REMOTE, "印を見ない");

    // 注入 4: 痕跡の掃除が印まで消す
    let s = Sources {
        remote: inject(
            &base.remote,
            "    let _ = std::fs::remove_file(pid_path());\n    let _ = std::fs::remove_file(token_path());",
            "    let _ = std::fs::remove_file(pid_path());\n    let _ = std::fs::remove_file(starting_path());\n    let _ = std::fs::remove_file(token_path());",
            "掃除が印を消す",
        ),
        cli: base.cli.clone(),
    };
    named(&s, REMOTE, "掃除が印を消す");

    // 注入 5: CLI が 1 行を出さない
    let s = Sources {
        remote: base.remote.clone(),
        cli: inject(
            &base.cli,
            "if let Some(detail) = status[\"starting\"][\"detail\"].as_str() {",
            "if let Some(detail) = None::<&str> {",
            "CLI が出さない",
        ),
    };
    named(&s, CLI, "CLI が出さない");
}

/// 実経路: 隔離した state dir に「PID ファイルの無い待ち受け」（port ファイル + 応答する
/// TCP の待ち受け）を作り、本物の `daemon_status` を読む。
///
/// - 印が無い = 従来どおり `pidless_listener` と `tako remote stop` の案内（陽性対照。#2455）
/// - `spawn_daemon` と同じ `StartingGuard` を置いている間 = `starting`（replacing）で stop を案内しない
/// - A/B（`TAKO_2608_LEGACY=1`）= 印があっても #2608 前の答え
/// - 印を置いたプロセスが抜けた（guard の Drop）/ 書き手が死んでいる / 期限を過ぎた = 従来の答えへ戻る
/// - 後から別の起動が書き直した印を、先の guard の Drop が消さない
/// - daemon が動いている（PID ファイルの先が生きている）間も `starting` を返す
///
/// tailscale は存在しないパスを渡す（`TAKO_TAILSCALE_BIN` は指定のパスだけを試す = 本物を叩かない）
#[cfg(unix)]
#[test]
fn 起こしている途中のstatusはstopを案内しない() {
    // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(起こしている途中のstatusはstopを案内しない) {
        return;
    }
    use serde_json::Value;
    use tako_control::remote::{
        daemon_status, starting_path, with_origin, RequestOrigin, StartingGuard,
    };
    use tako_control::remote_starting::{StartingMarker, LEGACY_ENV, STATE_REPLACING, TTL_SECS};

    let root = workspace_root();
    let remote_src = read(&root, REMOTE);
    let pidless_at = format!(
        "{REMOTE}:{}",
        fn_window(&remote_src, "fn with_pidless_listener(")
            .expect("with_pidless_listener")
            .0
    );

    let scratch = tako_core::test_residue::ScratchDir::new("tako-2608-status");
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
    std::env::remove_var("TAKO_REMOTE_ENDPOINT");

    // PID ファイルの無い待ち受け（起動中の新しい daemon と、#2455 の残り物の両方がこの形）
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("待ち受けの bind");
    let port = listener.local_addr().unwrap().port();
    std::fs::write(state.join("tako-remote.port"), port.to_string()).unwrap();
    let endpoint = format!("127.0.0.1:{port}");

    let stop_guided = |s: &Value| {
        s["warnings"].as_array().is_some_and(|w| {
            w.iter()
                .any(|l| l.as_str().unwrap_or("").contains("tako remote stop"))
        })
    };

    // 陽性対照: 印が無い = 従来どおり
    let plain = daemon_status();
    assert_eq!(plain["running"], false, "{plain}");
    assert_eq!(plain["pidless_listener"], endpoint.as_str(), "{plain}");
    assert!(
        stop_guided(&plain),
        "陽性対照で stop を案内していない: {plain}"
    );
    assert!(plain.get("starting").is_none(), "{plain}");

    // 入れ替えの途中（GUI 起動時の自動復帰が旧版 PID 4242 を入れ替えている）
    let during = {
        let _guard = with_origin(RequestOrigin::Autostart, || {
            StartingGuard::begin(Some(4242))
        });
        assert!(starting_path().is_file(), "印が置かれていない");
        let during = daemon_status();
        std::env::set_var(LEGACY_ENV, "1");
        let legacy = daemon_status();
        std::env::remove_var(LEGACY_ENV);
        assert!(
            legacy.get("pidless_listener").is_some() && stop_guided(&legacy),
            "A/B（{LEGACY_ENV}=1）が #2608 前の答えへ戻っていない: {legacy}"
        );
        during
    };
    assert_eq!(during["running"], false, "{during}");
    assert!(
        during.get("pidless_listener").is_none(),
        "{pidless_at}: 起こしている途中の新しい daemon の待ち受けを `pidless_listener` と答えている（#2608）: {during}"
    );
    assert!(
        !stop_guided(&during),
        "{pidless_at}: 起こしている途中なのに `tako remote stop` を案内している（#2608）: {during}"
    );
    assert_eq!(during["starting"]["state"], STATE_REPLACING, "{during}");
    assert_eq!(during["starting"]["previous_pid"], 4242, "{during}");
    assert_eq!(during["starting"]["via"], "autostart", "{during}");
    assert_eq!(
        during["starting"]["listener"],
        endpoint.as_str(),
        "{during}"
    );
    assert!(
        during["warnings"][0]
            .as_str()
            .is_some_and(|l| l.contains("入れ替えている途中")),
        "{during}"
    );

    // guard が抜けたら印は消え、従来の答えへ戻る
    assert!(
        !starting_path().exists(),
        "guard の Drop が印を消していない"
    );
    let after = daemon_status();
    assert!(
        after.get("pidless_listener").is_some() && stop_guided(&after),
        "{after}"
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let write_marker = |m: &StartingMarker| {
        std::fs::write(starting_path(), serde_json::to_string(m).unwrap()).unwrap();
    };

    // 書き手が死んでいる（GUI が途中で落ちた）印は効かない
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
    let crashed = daemon_status();
    assert!(
        crashed.get("starting").is_none() && stop_guided(&crashed),
        "書き手が死んだ印が「起動中」のまま残っている: {crashed}"
    );

    // 期限を過ぎた印（書き手は生きているが固まった）は効かない
    write_marker(&StartingMarker {
        at: now - TTL_SECS - 5,
        pid: std::process::id(),
        via: "cli".into(),
        previous_pid: None,
    });
    let expired = daemon_status();
    assert!(
        expired.get("starting").is_none() && stop_guided(&expired),
        "期限を過ぎた印が「起動中」のまま残っている: {expired}"
    );
    std::fs::remove_file(starting_path()).unwrap();

    // 後から別の起動が書き直した印を、先の guard の Drop は消さない
    {
        let _first = StartingGuard::begin(None);
        write_marker(&StartingMarker {
            at: now,
            pid: std::process::id(),
            via: "cli".into(),
            previous_pid: None,
        });
    }
    assert!(
        starting_path().is_file(),
        "後から書き直された別の起動の印を消している"
    );
    std::fs::remove_file(starting_path()).unwrap();

    // daemon が動いている間（入れ替える前の旧版 / PID ファイルを書いた直後の新版）も言う
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "read line", "tako", "remote", "serve"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("使い捨ての sh");
    let exe = scratch.path().join("tako");
    std::fs::write(&exe, b"tako").unwrap();
    std::fs::write(
        state.join("tako-remote.pid"),
        format!("{}\n{}\n{now}\n", child.id(), exe.display()),
    )
    .unwrap();
    let running = {
        let _guard = StartingGuard::begin(Some(child.id()));
        daemon_status()
    };
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(running["running"], true, "{running}");
    assert_eq!(running["starting"]["state"], STATE_REPLACING, "{running}");
    assert!(!stop_guided(&running), "{running}");
    drop(listener);
}
