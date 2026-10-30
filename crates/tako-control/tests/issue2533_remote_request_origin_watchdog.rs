//! remote daemon の停止・起動を**誰が頼んだか**が audit.log に残ることの番犬（#2533）
//!
//! # なぜ要るか
//!
//! 10/30 17:37、install も GUI の再起動もしていない時刻に本番の remote daemon が止められて
//! 起こし直され、`desired` も書き直されていた。audit.log の `daemon_shutdown_requested` には
//! **畳んだ daemon 自身の pid と実行ファイルしか**無く、会話記録・順番待ちの記録・perf.log を
//! 突き合わせても要求元を名指せなかった（段 1 の結論は Issue / PR に書いた）。
//!
//! # 何を縛るか
//!
//! 1. 頼む側の入口（`daemon_stop` / `daemon_force_stop` / 入れ替えの `stop_stale_generation` /
//!    `spawn_daemon`）が、**動く前に** `audit_daemon_request` で自分のことを 1 行残す
//! 2. 停止の要求（`post_admin_shutdown`）が経路を名乗る
//! 3. daemon の受け口が、接続元を **OS から引いて**（`admin_peer_pid`）`requester` に載せる
//!    （名乗りの pid を信じない・経路は既知の値だけ写す）
//! 4. GUI と CLI の呼び手はすべて `with_origin` で経路を名乗る
//!    （dispatch の `--force` の腕だけは `dispatch.rs` の中で直に呼ぶので名乗らない =
//!    `unspecified` として残り、実行ファイルが tako-app であることで GUI と分かる）
//! 5. 頼む側の記録にトークンを載せない
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓を固定し、[`記録を外す形へ戻すと名指しする`] で
//! **修正前を再現した注入**が `file:line` で名指しされることを確かめる。記録の中身は
//! `remote.rs` の単体テスト（実際の HTTP 経路で接続元の pid を引く）が縛る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const APP: &str = "crates/tako-app/src/main.rs";
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

/// 窓の中で `needle` を含む最初の**コードの**行の位置（0-based。無ければ None）
fn code_pos(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

struct Sources {
    remote: String,
    cli: String,
    app: String,
    panel: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        remote: read(root, REMOTE),
        cli: read(root, CLI),
        app: read(root, APP),
        panel: read(root, PANEL),
    }
}

/// 入口が「動く前に」頼んだ側の記録を残しているか
fn require_record_before(
    src: &str,
    out: &mut Vec<Offender>,
    signature: &str,
    record: &str,
    action: &str,
) {
    let Some((at, window)) = fn_window(src, signature) else {
        out.push(Offender {
            file: REMOTE,
            line: 0,
            why: format!("`{signature}` が見つからない（走査の空振り）"),
        });
        return;
    };
    match (code_pos(&window, record), code_pos(&window, action)) {
        (Some(r), Some(a)) if r < a => {}
        (Some(_), Some(a)) => out.push(Offender {
            file: REMOTE,
            line: at + a,
            why: format!(
                "`{signature}` が `{action}` より後で要求元を記録している。\
                 動いた後（失敗・入れ替え後）では「誰が頼んだか」の行が欠ける（#2533）"
            ),
        }),
        (None, _) => out.push(Offender {
            file: REMOTE,
            line: at,
            why: format!(
                "`{signature}` が要求元を記録していない（`{record}` が無い）。\
                 audit.log に畳んだ daemon 自身の pid しか残らず、誰が頼んだか追えない（#2533）"
            ),
        }),
        (Some(_), None) => out.push(Offender {
            file: REMOTE,
            line: at,
            why: format!("`{signature}` に `{action}` が無い（走査の空振り）"),
        }),
    }
}

/// `remote.rs` の検査（頼む側の記録・名乗り・daemon 側の接続元）
fn scan_remote(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();

    // 1. 頼む側の入口が、動く前に自分のことを残す
    for (signature, record, action) in [
        (
            "pub fn daemon_stop()",
            "audit_daemon_request(\"daemon_stop_requested\", \"stop\"",
            "daemon_stop_impl(",
        ),
        (
            "pub fn daemon_force_stop()",
            "audit_daemon_request(\"daemon_stop_requested\", \"force_stop\"",
            "daemon_stop_impl(",
        ),
        (
            "fn stop_stale_generation(",
            "audit_daemon_request(\"daemon_stop_requested\", \"renew\"",
            "daemon_stop_impl(",
        ),
        (
            "pub fn spawn_daemon()",
            "audit_daemon_request(\"daemon_start_requested\"",
            "daemon_status()",
        ),
    ] {
        require_record_before(src, &mut out, signature, record, action);
    }

    // 2. 停止の要求が経路を名乗る
    match fn_window(src, "fn post_admin_shutdown(") {
        None => out.push(Offender {
            file: REMOTE,
            line: 0,
            why: "`post_admin_shutdown` が見つからない（走査の空振り）".into(),
        }),
        Some((at, window)) => {
            if code_pos(&window, "X-Tako-Requester-Via:").is_none()
                || code_pos(&window, "current_origin_str()").is_none()
            {
                out.push(Offender {
                    file: REMOTE,
                    line: at,
                    why: "停止の要求が経路（`X-Tako-Requester-Via`）を名乗っていない。\
                          daemon 側の記録で GUI のボタンか CLI かを分けられない（#2533）"
                        .into(),
                });
            }
        }
    }

    // 3. daemon の受け口が接続元を OS から引いて残す
    let lines: Vec<&str> = src.lines().collect();
    match lines
        .iter()
        .position(|l| l.contains("(tiny_http::Method::Post, ADMIN_SHUTDOWN_PATH) =>"))
    {
        None => out.push(Offender {
            file: REMOTE,
            line: 0,
            why: "停止の受け口（`ADMIN_SHUTDOWN_PATH` の腕）が見つからない（走査の空振り）".into(),
        }),
        Some(start) => {
            // 腕の終わり = 応答（`"shutting_down": true`）を返す行
            let end = lines[start..]
                .iter()
                .position(|l| l.contains("\"shutting_down\": true"))
                .map(|i| start + i)
                .unwrap_or(start);
            let arm = lines[start..=end].join("\n");
            let audit = code_pos(&arm, "\"daemon_shutdown_requested\"");
            let requester = code_pos(&arm, "shutdown_requester(");
            match (audit, requester) {
                (Some(a), Some(r)) if r < a && code_pos(&arm, "\"requester\"").is_some() => {}
                (Some(a), _) => out.push(Offender {
                    file: REMOTE,
                    line: start + 1 + a,
                    why: "`daemon_shutdown_requested` に要求元（`requester`）が載っていない。\
                          この行の pid / exe は畳む daemon 自身なので、誰が頼んだかが残らない（#2533）"
                        .into(),
                }),
                (None, _) => out.push(Offender {
                    file: REMOTE,
                    line: start + 1,
                    why: "停止の受け口が `daemon_shutdown_requested` を残していない".into(),
                }),
            }
        }
    }
    match fn_window(src, "fn shutdown_requester(") {
        None => out.push(Offender {
            file: REMOTE,
            line: 0,
            why: "`shutdown_requester` が見つからない（走査の空振り）".into(),
        }),
        Some((at, window)) => {
            if code_pos(&window, "admin_peer_pid(").is_none() {
                out.push(Offender {
                    file: REMOTE,
                    line: at,
                    why: "要求元の pid を OS から引いていない（`admin_peer_pid`）。\
                          名乗りの pid は管理トークンを持つ者なら偽れる（#2533）"
                        .into(),
                });
            }
            if code_pos(&window, "origin_label").is_none() {
                out.push(Offender {
                    file: REMOTE,
                    line: at,
                    why:
                        "名乗りの経路を既知の値に絞っていない（任意の文字列を監査ログへ流し込める）"
                            .into(),
                });
            }
        }
    }

    // 5. 頼む側の記録にトークン・コマンドラインを載せない
    match fn_window(src, "fn audit_daemon_request(") {
        None => out.push(Offender {
            file: REMOTE,
            line: 0,
            why: "`audit_daemon_request` が見つからない（走査の空振り）".into(),
        }),
        Some((at, window)) => {
            for forbidden in ["token", "TOKEN", "command_line", "args()"] {
                if let Some(rel) = code_pos(&window, forbidden) {
                    out.push(Offender {
                        file: REMOTE,
                        line: at + rel,
                        why: format!(
                            "要求元の記録に `{forbidden}` を載せている。audit.log には識別だけを残す（FR-6.8）"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// GUI と CLI の呼び手が経路を名乗っているか（呼んだ行か、その直前 2 行に `with_origin(` がある）
fn scan_callers(src: &str, file: &'static str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let calls = ["spawn_daemon", "daemon_stop", "daemon_force_stop"]
            .iter()
            .any(|f| code.contains(&format!("remote::{f}")));
        if !calls {
            continue;
        }
        let named = lines[i.saturating_sub(2)..=i]
            .iter()
            .any(|l| l.contains("with_origin("));
        if !named {
            out.push(Offender {
                file,
                line: i + 1,
                why: "remote daemon の起動・停止を経路を名乗らずに呼んでいる（`with_origin` の外）。\
                      audit.log の `via` が `unspecified` になり、ボタンか自動復帰か CLI かを分けられない（#2533）"
                    .into(),
            });
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut found = scan_remote(&s.remote);
    found.extend(scan_callers(&s.cli, CLI));
    found.extend(scan_callers(&s.app, APP));
    found.extend(scan_callers(&s.panel, PANEL));
    found.iter().map(Offender::report).collect()
}

#[test]
fn 停止と起動の要求元が記録される配線が揃っている() {
    let found = all(&sources(&workspace_root()));
    assert!(
        found.is_empty(),
        "#2533 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for (src, file, needle) in [
        (&s.remote, REMOTE, "pub fn daemon_stop()"),
        (&s.remote, REMOTE, "pub fn daemon_force_stop()"),
        (&s.remote, REMOTE, "fn stop_stale_generation("),
        (&s.remote, REMOTE, "pub fn spawn_daemon()"),
        (&s.remote, REMOTE, "fn post_admin_shutdown("),
        (&s.remote, REMOTE, "fn shutdown_requester("),
        (&s.remote, REMOTE, "fn audit_daemon_request("),
    ] {
        let (_, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{file} に `{needle}` が無い（番犬が空振りする）"));
        assert!(
            window.lines().count() >= 3,
            "{file} の `{needle}` の窓が短すぎる（{} 行）",
            window.lines().count()
        );
    }
    // 呼び手の走査が実際に呼び出しを見ている（0 件なら全部が無意味に緑になる）
    for (src, file, at_least) in [(&s.cli, CLI, 3), (&s.app, APP, 2), (&s.panel, PANEL, 3)] {
        let n = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| {
                ["spawn_daemon", "daemon_stop", "daemon_force_stop"]
                    .iter()
                    .any(|f| l.contains(&format!("remote::{f}")))
            })
            .count();
        assert!(
            n >= at_least,
            "{file} の起動・停止の呼び出しが {n} 件しか見えない（期待 {at_least} 件以上 = 走査の空振り）"
        );
    }
}

/// 注入を当てる
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
fn 記録を外す形へ戻すと名指しする() {
    let root = workspace_root();
    let base = sources(&root);
    assert!(all(&base).is_empty(), "注入の前は緑であること");

    // 注入 1: #2533 前の spawn_daemon（頼んだ側の記録が無い）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "    audit_daemon_request(\"daemon_start_requested\", \"start\", None);\n",
        "",
        "記録の無い spawn_daemon",
    );
    let want = format!(
        "{REMOTE}:{}",
        first_line(&s.remote, "pub fn spawn_daemon()")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "記録の無い spawn_daemon を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 2: #2533 前の受け口（daemon 自身の pid しか残さない）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "                    json!({ \"requester\": requester }),\n",
        "                    json!({}),\n",
        "要求元を載せない受け口",
    );
    let want = format!(
        "{REMOTE}:{}",
        first_line(&s.remote, "\"daemon_shutdown_requested\",")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "要求元を載せない受け口を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 3: 入れ替えの停止を記録してから止める順を崩す（止めた後に記録）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "    audit_daemon_request(\"daemon_stop_requested\", \"renew\", Some(pid));\n    let stopped = daemon_stop_impl(false);\n",
        "    let stopped = daemon_stop_impl(false);\n    audit_daemon_request(\"daemon_stop_requested\", \"renew\", Some(pid));\n",
        "止めた後に記録する入れ替え",
    );
    let (at, window) = fn_window(&s.remote, "fn stop_stale_generation(").unwrap();
    let want = format!(
        "{REMOTE}:{}",
        at + code_pos(&window, "daemon_stop_impl(").unwrap()
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "止めた後に記録する形を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 4: リモートカードの停止ボタンが経路を名乗らない
    let mut s = sources(&root);
    s.panel = inject(
        &base.panel,
        "                    tako_control::remote::with_origin(\n                        tako_control::remote::RequestOrigin::Panel,\n                        tako_control::remote::daemon_stop,\n                    )\n",
        "                    tako_control::remote::daemon_stop()\n",
        "名乗らない停止ボタン",
    );
    let want = format!(
        "{PANEL}:{}",
        first_line(&s.panel, "tako_control::remote::daemon_stop()")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "名乗らない停止ボタンを名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 5: 名乗りの pid を信じる（OS から引かない）
    let mut s = sources(&root);
    s.remote = inject(
        &base.remote,
        "    let pid = crate::remote_role::admin_peer_pid(endpoint, peer);\n",
        "    let pid: Option<u32> = via.and_then(|v| v.parse().ok());\n",
        "名乗りの pid を信じる受け口",
    );
    let want = format!(
        "{REMOTE}:{}",
        first_line(&s.remote, "fn shutdown_requester(")
    );
    let found = all(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&want)),
        "名乗りの pid を信じる形を名指しできていない（期待 {want}）: {found:#?}"
    );
}
