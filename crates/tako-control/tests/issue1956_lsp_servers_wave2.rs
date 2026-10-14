//! #1956 の e2e: 言語サーバの検出表の第 2 波（texlab / tombi / sourcekit-lsp / dart /
//! vscode-css-language-server / vscode-html-language-server）
//!
//! 1. **足した行の拡張子を編集モードで開くと、その行のサーバが表の languageId で開かれる**:
//!    偽サーバ（`tako-lsp-fake`）の実プロセス。起動の方法（launcher）が受けた行の ID と、偽サーバに
//!    届いた didOpen の `languageId` を、表（`tako_core::lsp::servers::SERVERS`）の値と突き合わせる
//! 2. **未導入でも壊れない**: どこにも無いとき、取れる行（texlab / tombi）は配布元へ届かない注入
//!    （ローカルの配布元が 503 を返す）で取得に失敗し、理由（`fetch_error` = http）と行の導入コマンドを
//!    残して未導入へ落ちる。取れない行は導入コマンドの案内へ落ちる。置き場に途中の物を残さない
//! 3. **PATH の版が古くて確かめ（`probe_args`）に落ちる texlab は信じない**（`Launch::Broken`）:
//!    既定の解決（`default_launch`）を子プロセスで走らせ、`$SHELL` を「PATH の texlab を返す偽の
//!    ログインシェル」にする（env を変えるので子に分ける = 並行する他のテストの env を汚さない）
//!
//! 実のサーバでの確認（波線・ホバー・常駐・起動時間・実の配布元からの取得）は
//! `scripts/test-lsp-servers-1956.sh`（隔離 GUI）。候補の比較と採った順は
//! `.agent/plans/2026-10-language-coverage.md` §8。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::fetch::FetchConfig;
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::{stop_and_settle, wait_running};

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 第 2 波で足した行（表の順）
const WAVE2: &[&str] = &[
    "texlab",
    "tombi",
    "sourcekit-lsp",
    "dart",
    "vscode-css-language-server",
    "vscode-html-language-server",
];

/// 1 件ぶんの置き場（スコープを抜けると消える = #1312）
struct Scratch {
    dir: ScratchDir,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        Self {
            dir: ScratchDir::new(&format!("tako-1956-{tag}")),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn log(&self) -> PathBuf {
        self.path("received.jsonl")
    }

    /// `rel` に本文を置いて絶対パスを返す
    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

fn wave2() -> Vec<&'static ServerSpec> {
    WAVE2
        .iter()
        .map(|id| {
            servers::find_in(servers::SERVERS, id).unwrap_or_else(|| panic!("{id} が検出表に無い"))
        })
        .collect()
}

/// 偽サーバで起こす設定。起動の方法が受けた行の ID を `asked` に積む
fn config(scratch: &Scratch, asked: Arc<Mutex<Vec<String>>>) -> LspConfig {
    let args: Vec<String> = ["--scenario", "normal", "--log"]
        .iter()
        .map(|s| s.to_string())
        .chain([scratch.log().display().to_string()])
        .collect();
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |spec: &ServerSpec| {
            asked.lock().unwrap().push(spec.id.to_string());
            Launch::Found {
                plan: ChildCmd {
                    program: FAKE.to_string(),
                    args: args.clone(),
                },
                program_path: FAKE.to_string(),
            }
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 0,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn received(scratch: &Scratch) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[track_caller]
fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {limit:?} 以内に起きなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 受け入れ条件: 足した行ごとに、偽サーバの e2e で正しい行・languageId が選ばれる
#[test]
fn 第2波の行は拡張子ごとに表の行と_language_id_で開かれる() {
    let mut checked = 0;
    for spec in wave2() {
        for kind in spec.documents {
            let scratch = Scratch::new(&format!("open-{}", kind.extension));
            let asked = Arc::new(Mutex::new(Vec::new()));
            let manager = LspManager::new(config(&scratch, Arc::clone(&asked)));
            // 大文字の拡張子でも同じ行（表の引き方は大文字小文字を問わない）
            let name = if checked % 2 == 0 {
                format!("sample.{}", kind.extension)
            } else {
                format!("SAMPLE.{}", kind.extension.to_ascii_uppercase())
            };
            let path = scratch.file(&name, "x\n");
            let mut link = DocLink::default();
            manager.sync(&mut link, true, &path, "x\n", 1);
            wait_running(&manager);
            let status = manager.status(None)["servers"][0].clone();
            assert_eq!(status["id"], json!(spec.id), "{name}: {status:#}");
            stop_and_settle(&manager, &scratch.log());
            assert_eq!(
                asked.lock().unwrap().as_slice(),
                [spec.id.to_string()],
                "{name}: 起こしたのは表のこの行だけ"
            );
            let messages = received(&scratch);
            let opened = messages
                .iter()
                .find(|m| m["method"] == "textDocument/didOpen")
                .unwrap_or_else(|| panic!("{name}: didOpen が届いていない"));
            assert_eq!(
                opened["params"]["textDocument"]["languageId"],
                json!(kind.language_id),
                "{name}"
            );
            assert_eq!(
                opened["params"]["textDocument"]["uri"],
                json!(tako_core::file_uri::from_path(&path)),
                "{name}"
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked, 9,
        "第 2 波の拡張子は 9 つ（tex / toml / swift / dart / css / scss / less / html / htm）"
    );
}

/// 配布元の代わり（届いた要求の path を数え、503 で答える = 配布元に届かない注入）
struct RefusingOrigin {
    base: String,
    paths: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl RefusingOrigin {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("配布元を立てる");
        let port = server.server_addr().to_ip().unwrap().port();
        let paths: Arc<Mutex<Vec<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (p, st) = (Arc::clone(&paths), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(50)) else {
                    continue;
                };
                p.lock().unwrap().push(request.url().to_string());
                let _ = request.respond(tiny_http::Response::empty(503));
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            paths,
            stop,
        }
    }
}

impl Drop for RefusingOrigin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// 受け入れ条件: 未導入でも壊れない（今の表示に落ちて理由が読める）+ 境界「取得が失敗する」
#[test]
fn 未導入なら取れる行は取得に失敗した理由へ_取れない行は導入コマンドへ落ちる() {
    let scratch = Scratch::new("missing");
    let origin = RefusingOrigin::start();
    let mut config = config(&scratch, Arc::default());
    config.launcher = Arc::new(|spec: &ServerSpec| Launch::NotFound {
        program: spec.program.to_string(),
        override_env: None,
    });
    let root = scratch.path("lsp-servers");
    let fetch = FetchConfig::with_root(root.clone(), Some(origin.base.clone()));
    let manager = LspManager::with_fetch(config, Some(fetch.clone()));
    let mut links = Vec::new();
    for spec in wave2() {
        let ext = spec.documents[0].extension;
        let path = scratch.file(&format!("{}/sample.{ext}", spec.id), "x\n");
        let mut link = DocLink::default();
        manager.sync(&mut link, true, &path, "x\n", 1);
        links.push(link);
        let can_fetch = fetch.can_fetch(spec);
        assert_eq!(
            can_fetch,
            spec.fetch
                .is_some_and(|f| f.available_for(fetch.os, fetch.arch)),
            "{}",
            spec.id
        );
        let mut status = Value::Null;
        wait_for(
            &format!("{} が未導入へ落ちる", spec.id),
            Duration::from_secs(30),
            || {
                status = manager.status(Some(spec.id))["servers"][0].clone();
                status["state"] == json!("not_installed")
                    && (!can_fetch || status["fetch_error"].is_string())
            },
        );
        let command = spec.install.command();
        assert_eq!(status["install_command"], json!(command), "{status:#}");
        let reason = status["reason"].as_str().unwrap_or_default();
        let next_step = status["next_step"].as_str().unwrap_or_default();
        assert!(!reason.is_empty(), "{}: 理由が読める", spec.id);
        assert!(
            next_step.contains(command),
            "{}: 次の一手に導入コマンド: {next_step}",
            spec.id
        );
        if can_fetch {
            // 503 = 配布元に届かない（http）。理由は取得物の名前を名指す
            assert_eq!(status["fetch_error"], json!("http"), "{status:#}");
            let label = spec.fetch.unwrap().label(spec.id);
            assert!(reason.contains(&label), "{reason} に {label}");
        } else {
            assert!(status["fetch_error"].is_null(), "{status:#}");
        }
    }
    // 取れる行は、表の asset の path（ミラーの形 `<host>/<path>`）を 1 回ずつ求めた
    let asked = origin.paths.lock().unwrap().clone();
    for spec in wave2().into_iter().filter(|s| fetch.can_fetch(s)) {
        let assets = spec.fetch.unwrap().assets_for(fetch.os, fetch.arch);
        assert_eq!(assets.len(), 1, "{}", spec.id);
        let want = assets[0].url.trim_start_matches("https:/").to_string();
        assert_eq!(
            asked.iter().filter(|p| **p == want).count(),
            1,
            "{} の取得物 {want} を 1 回求める: {asked:?}",
            spec.id
        );
    }
    // 置き場に版の段も途中の段も残さない（印の無い物を「入っている」と読まない）
    for spec in wave2() {
        let dir = root.join(spec.id);
        let left: Vec<_> = std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{}: 残った {left:?}", spec.id);
    }
    // `tako lsp servers` の行: 入っていない・取れるか・案内
    let rows = manager.servers()["servers"].as_array().cloned().unwrap();
    assert_eq!(rows.len(), servers::SERVERS.len());
    for spec in wave2() {
        let row = rows
            .iter()
            .find(|r| r["id"] == json!(spec.id))
            .unwrap_or_else(|| panic!("{} の行が無い", spec.id));
        assert_eq!(row["installed"], json!(false), "{row:#}");
        assert_eq!(
            row["fetch"]["available"],
            json!(fetch.can_fetch(spec)),
            "{row:#}"
        );
        assert_eq!(row["install_command"], json!(spec.install.command()));
    }
    manager.shutdown_all(Duration::from_secs(5));
    drop(links);
}

/// 既定の解決（`default_launch`）の子プロセス側。`CHILD_ENV` があるときだけ動く
const CHILD_ENV: &str = "TAKO_1956_LAUNCH_CHILD";
const CHILD_ID_ENV: &str = "TAKO_1956_LAUNCH_ID";

#[test]
fn 子プロセス_既定の解決を1回走らせて結果を書く() {
    let Ok(out) = std::env::var(CHILD_ENV) else {
        return;
    };
    let id = std::env::var(CHILD_ID_ENV).unwrap();
    let spec = servers::find_in(servers::SERVERS, &id).unwrap();
    let launch = tako_control::lsp::manager::default_launch(spec);
    std::fs::write(out, format!("{launch:?}")).unwrap();
}

/// 偽のログインシェル（`-l -c 'command -v <name>'` に `bin` の中のパスを答える）で、子プロセスに
/// 既定の解決を走らせた結果（`Launch` の Debug 表記）
#[cfg(unix)]
fn launch_in_child(scratch: &Scratch, id: &str, bin: &std::path::Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    let shell = scratch.path(&format!("shell-{id}"));
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nname=\"${{3#command -v }}\"\n[ -x \"{}/$name\" ] && echo \"{}/$name\"\nexit 0\n",
            bin.display(),
            bin.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = scratch.path(&format!("result-{id}.txt"));
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "子プロセス_既定の解決を1回走らせて結果を書く",
            "--nocapture",
        ])
        .env(CHILD_ENV, &out)
        .env(CHILD_ID_ENV, id)
        .env("SHELL", &shell)
        .env_remove(servers::override_env_name(id))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "子プロセスが落ちた（{}）: {}",
        child.status,
        String::from_utf8_lossy(&child.stderr)
    );
    std::fs::read_to_string(&out).unwrap()
}

/// 境界「PATH の版が古くて probe に失敗する」: 取れる行（texlab / tombi）は PATH の物を `--version` で
/// 確かめ、成功で終わらなければ `Broken`（= 見つからないと同じに扱い、tako が取る側へ回る）。
/// 確かめに通る物は `Found`。取れない行（vscode-css-language-server）は代わりが無いので確かめない
#[cfg(unix)]
#[test]
fn path_の古い版は確かめに落ちて信じない() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("probe");
    let stale = scratch.path("stale");
    let fresh = scratch.path("fresh");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::create_dir_all(&fresh).unwrap();
    // 古い版の形: `--version` を知らずに使い方の誤りで落ちる
    let stale_body =
        "#!/bin/sh\necho \"error: unexpected argument '--version' found\" >&2\nexit 2\n";
    let fresh_body = "#!/bin/sh\necho \"x 9.9.9\"\nexit 0\n";
    for (dir, body) in [(&stale, stale_body), (&fresh, fresh_body)] {
        for name in ["texlab", "tombi", "vscode-css-language-server"] {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    for id in ["texlab", "tombi"] {
        let got = launch_in_child(&scratch, id, &stale);
        assert!(
            got.starts_with("Broken") && got.contains("unexpected argument '--version'"),
            "{id}: PATH の古い版は確かめに落ちる: {got}"
        );
        let got = launch_in_child(&scratch, id, &fresh);
        assert!(
            got.starts_with("Found"),
            "{id}: 確かめに通る物は使う: {got}"
        );
    }
    let got = launch_in_child(&scratch, "vscode-css-language-server", &stale);
    assert!(
        got.starts_with("Found"),
        "取れない行は確かめない（代わりが無い）: {got}"
    );
}

/// 確かめに落ちた PATH の物の後も壊れない: `Broken` は未導入と同じに扱い、取れる行は取りに行く。
/// 取りにも行けなければ、確かめに落ちた理由ではなく取得の失敗と導入コマンドが残る
#[test]
fn 確かめに落ちた後に取得も失敗したら理由と導入コマンドが残る() {
    let scratch = Scratch::new("broken");
    // 閉じたポート = 接続できない（オフライン）
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut config = config(&scratch, Arc::default());
    config.launcher = Arc::new(|spec: &ServerSpec| Launch::Broken {
        program_path: format!("/opt/old/{}", spec.program),
        detail: "error: unexpected argument '--version' found".to_string(),
    });
    let fetch = FetchConfig::with_root(
        scratch.path("lsp-servers"),
        Some(format!("http://127.0.0.1:{port}")),
    );
    let manager = LspManager::with_fetch(config, Some(fetch));
    let spec = servers::find_in(servers::SERVERS, "texlab").unwrap();
    let path = scratch.file("doc/report.tex", "\\begin{document}\n");
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &path, "\\begin{document}\n", 1);
    let mut status = Value::Null;
    wait_for("未導入へ落ちる", Duration::from_secs(30), || {
        status = manager.status(Some(spec.id))["servers"][0].clone();
        status["state"] == json!("not_installed") && status["fetch_error"].is_string()
    });
    assert_eq!(status["fetch_error"], json!("network"), "{status:#}");
    assert_eq!(status["install_command"], json!(spec.install.command()));
    // `tako lsp servers` の行は PATH の物が壊れていたことも名指す（どれを直せばよいかが読める）
    let row = manager.servers()["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(spec.id))
        .cloned()
        .unwrap();
    assert_eq!(row["broken"]["path"], json!("/opt/old/texlab"), "{row:#}");
    manager.shutdown_all(Duration::from_secs(5));
}
