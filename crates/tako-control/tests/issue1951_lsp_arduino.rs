//! #1951 の e2e: `.ino`（Arduino のスケッチ）の言語サーバ = arduino-language-server（ALS）の行
//!
//! 1. **既定の解決で ALS の行が選ばれ、起動引数に伴走と空の設定ファイルが正しいパスで並ぶ**:
//!    偽サーバ（`tako-lsp-fake`）を ALS の名前で置き、同じディレクトリに偽の arduino-cli / clangd を置く。
//!    既定の解決（`default_launch` = 差し替えの env → 伴走は同じディレクトリ）を子プロセスで走らせ
//!    （env を変えるので子に分ける = 並行する他のテストの env を汚さない）、偽サーバが記録した起動引数
//!    （`TAKO_LSP_FAKE_ARGV_LOG`）を読む。`-cli-config` のファイルは `<data_dir>/lsp-config/…` に 0 バイト
//! 2. **基板が無ければ起こさず、状態に理由が出る**: `sketch.yaml` の無いスケッチは `unconfigured` で
//!    偽サーバを 1 度も起こさない。`sketch.yaml` を書いてシェル統合の合図（`shell_activity::note`）を
//!    入れると、確かめ直して起きる（tako のターミナルで `arduino-cli board attach` を打った形）
//! 3. **ALS なし・伴走（clangd）なしでも壊れず、理由が読める**（注入した解決）
//! 4. **前提が未設定の器への問い合わせは待たない**（ホバー・定義ジャンプが上限まで固まらない）
//!
//! 実機（Arduino IDE の同梱物で誤りの波線・ホバー・補完が出る）は `scripts/test-lsp-arduino-1951.sh`
//! （隔離 GUI・隔離 HOME）。設計は `.agent/plans/2026-10-language-coverage.md` §5.3。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, GotoError, GotoRequest, HoverError, HoverRequest, Launch, LspConfig, LspManager,
};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::{stop_and_settle, wait_received};

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");
const ID: &str = "arduino-language-server";

const BLINK: &str = "void setup() {\n  pinMode(LED_BUILTIN, OUTPUT);\n}\n\nvoid loop() {\n}\n";

fn spec() -> &'static ServerSpec {
    servers::find_in(servers::SERVERS, ID).expect("ALS の行が検出表にある")
}

/// 1 件ぶんの置き場（スコープを抜けると消える = #1312）
struct Scratch {
    dir: ScratchDir,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        Self {
            dir: ScratchDir::new(&format!("tako-1951-{tag}")),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    /// 根に `.git` を置いた repo の中のスケッチ（ルートが repo の根へ上がらないことも見る）
    fn sketch(&self, name: &str, board: bool) -> PathBuf {
        let _ = self.file("repo/.git/HEAD", "ref: refs/heads/main\n");
        let ino = self.file(&format!("repo/sketches/{name}/{name}.ino"), BLINK);
        if board {
            self.file(
                &format!("repo/sketches/{name}/sketch.yaml"),
                "default_fqbn: arduino:avr:uno\n",
            );
        }
        ino
    }
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
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

fn status_of(manager: &LspManager) -> Value {
    manager.status(Some(ID))["servers"][0].clone()
}

fn config(launcher: tako_control::lsp::manager::Launcher) -> LspConfig {
    LspConfig {
        table: servers::SERVERS,
        launcher,
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

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

// --- 1 / 2. 既定の解決（子プロセス）---------------------------------------------

const CHILD_ENV: &str = "TAKO_1951_CHILD";

/// 子プロセス側（`CHILD_ENV` があるときだけ動く）: 既定の解決の manager で、基板のあるスケッチ →
/// 基板の無いスケッチ → 基板を書いて合図、を順に開き、各段の状態を `CHILD_ENV` のファイルへ書く
#[test]
fn 子プロセス_既定の解決で_ino_を開いて状態を書く() {
    let Ok(out) = std::env::var(CHILD_ENV) else {
        return;
    };
    let base = PathBuf::from(std::env::var("TAKO_1951_BASE").unwrap());
    let manager = LspManager::new(config(Arc::new(tako_control::lsp::manager::default_launch)));
    let mut result = serde_json::Map::new();
    // ① 基板あり → 起きる
    let with_board = base.join("repo/sketches/Blink/Blink.ino");
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &with_board, BLINK, 1);
    let mut status = Value::Null;
    wait_for(
        "基板ありのスケッチで稼働",
        Duration::from_secs(30),
        || {
            status = status_of(&manager);
            status["state"] == json!("running")
        },
    );
    result.insert("with_board".into(), status.clone());
    // ② 基板なし → 起こさない（理由と選ぶコマンド）
    let no_board = base.join("repo/sketches/NoBoard/NoBoard.ino");
    let mut link2 = DocLink::default();
    manager.sync(&mut link2, true, &no_board, BLINK, 1);
    let find = |manager: &LspManager, root: &Path| {
        manager.status(Some(ID))["servers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|s| s["root"] == json!(root.display().to_string()))
            .unwrap_or(Value::Null)
    };
    let no_board_root = no_board.parent().unwrap().to_path_buf();
    wait_for(
        "基板なしのスケッチが前提の未設定へ",
        Duration::from_secs(30),
        || {
            status = find(&manager, &no_board_root);
            status["state"] == json!("unconfigured")
        },
    );
    result.insert("no_board".into(), status.clone());
    result.insert(
        "document_server".into(),
        json!(manager.document_server(&no_board).map(|s| {
            json!({
                "id": s.id,
                "state": s.state.slug(),
                "reason": s.reason,
                "next_step": s.next_step,
            })
        })),
    );
    // ③ 基板を書いてシェル統合の合図 → 確かめ直して起きる
    std::fs::write(
        no_board_root.join("sketch.yaml"),
        "default_fqbn: arduino:avr:uno\n",
    )
    .unwrap();
    tako_core::shell_activity::note();
    wait_for(
        "合図で確かめ直して稼働",
        Duration::from_secs(30),
        || {
            status = find(&manager, &no_board_root);
            status["state"] == json!("running")
        },
    );
    result.insert("after_attach".into(), status.clone());
    // 器は 2 つ（Blink と NoBoard）。両方の偽サーバが shutdown を受けた = それより前の通知はログに載った
    stop_and_settle(&manager, &base.join("received.jsonl"));
    wait_received(&base.join("received.jsonl"), "shutdown", 2);
    drop((link, link2));
    std::fs::write(out, Value::Object(result).to_string()).unwrap();
}

/// 受け入れ条件: 偽サーバの e2e（ALS の行が選ばれ、起動引数に `-cli` / `-clangd` / `-cli-config` が
/// 正しいパスで並ぶ。基板が無いと起こさず、状態に理由が出る）
#[test]
fn 既定の解決で伴走と空の設定ファイルを並べて起き_基板が無ければ起こさない() {
    let scratch = Scratch::new("launch");
    let base = scratch.dir.path().to_path_buf();
    // 偽の Arduino IDE の同梱（ALS = 偽サーバの複製・伴走 2 つ）
    let ide = scratch.path("ide");
    std::fs::create_dir_all(&ide).unwrap();
    let als = ide.join(exe(ID));
    std::fs::copy(FAKE, &als).unwrap();
    for companion in ["arduino-cli", "clangd"] {
        std::fs::copy(FAKE, ide.join(exe(companion))).unwrap();
    }
    scratch.sketch("Blink", true);
    scratch.sketch("NoBoard", false);
    let data = scratch.path("data");
    let home = scratch.path("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = scratch.path("result.json");
    let argv_log = scratch.path("argv.jsonl");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "子プロセス_既定の解決で_ino_を開いて状態を書く",
            "--nocapture",
        ])
        .env(CHILD_ENV, &out)
        .env("TAKO_1951_BASE", &base)
        .env(servers::override_env_name(ID), &als)
        .env("TAKO_DATA_DIR", &data)
        .env("TAKO_LSP_FAKE_ARGV_LOG", &argv_log)
        .env("TAKO_LSP_FAKE_LOG", base.join("received.jsonl"))
        .env("TAKO_LSP_FAKE_SCENARIO", "normal")
        .env("HOME", &home)
        .stdin(std::process::Stdio::null());
    // unix は `$SHELL -l -c 'exec …'` で起こす。利用者の rc を読まない素のシェルにする
    if cfg!(unix) {
        child.env("SHELL", "/bin/sh");
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "子プロセスが落ちた（{}）: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();

    // ① ALS の行が起き、ルートはスケッチのフォルダ（repo の根へ上がらない）
    let blink_root = base.join("repo/sketches/Blink");
    let with_board = &result["with_board"];
    assert_eq!(with_board["id"], json!(ID), "{with_board:#}");
    assert_eq!(
        with_board["root"],
        json!(blink_root.display().to_string()),
        "{with_board:#}"
    );
    assert_eq!(with_board["source"], json!("override"), "{with_board:#}");
    // 起動引数: `-cli <同じ場所の arduino-cli> -clangd <同じ場所の clangd> -cli-config <data_dir の空ファイル>`
    let config_file = data.join("lsp-config").join(ID).join("arduino-cli.yaml");
    let argv = lines(&argv_log);
    assert_eq!(
        argv.len(),
        2,
        "起動は基板ありの 2 回だけ（基板なしでは起こさない）: {argv:?}"
    );
    for args in &argv {
        let args: Vec<&str> = args
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|a| a.as_str().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "-cli",
                ide.join(exe("arduino-cli")).to_str().unwrap(),
                "-clangd",
                ide.join(exe("clangd")).to_str().unwrap(),
                "-cli-config",
                config_file.to_str().unwrap(),
            ],
            "起動引数"
        );
    }
    let meta = std::fs::metadata(&config_file).expect("-cli-config のファイルを用意した");
    assert_eq!(meta.len(), 0, "中身は要らない（0 バイト）");

    // ② 基板なし: 起こさず、理由と選ぶコマンド（`tako lsp status` と画面が同じ文）
    let no_board = &result["no_board"];
    assert_eq!(no_board["pid"], Value::Null, "{no_board:#}");
    assert_eq!(no_board["spawn_count"], json!(0), "{no_board:#}");
    let reason = no_board["reason"].as_str().unwrap_or_default();
    let next_step = no_board["next_step"].as_str().unwrap_or_default();
    let command = no_board["setup_command"].as_str().unwrap_or_default();
    assert!(reason.contains("sketch.yaml"), "{reason}");
    assert!(
        command.contains(" board attach -b ") && command.contains("NoBoard"),
        "{command}"
    );
    // 伴走が ALS と同じ場所にあるので、案内のコマンドもそのフルパス（PATH から打てない形を避ける）
    assert!(
        command.contains(&exe("arduino-cli")) && command.contains("ide"),
        "{command}"
    );
    assert!(next_step.contains(command), "{next_step}");
    let shown = &result["document_server"];
    assert_eq!(shown["state"], json!("unconfigured"), "{shown:#}");
    assert_eq!(shown["reason"], json!(reason));
    assert_eq!(shown["next_step"], json!(next_step));

    // ③ 基板を書いて合図 → 起きた
    let after = &result["after_attach"];
    assert_eq!(after["state"], json!("running"), "{after:#}");
    assert_eq!(after["reason"], Value::Null, "{after:#}");

    // 偽サーバに届いた didOpen: languageId は cpp・rootUri はスケッチのフォルダ
    let received = lines(&base.join("received.jsonl"));
    let opened: Vec<&Value> = received
        .iter()
        .filter(|m| m["method"] == "textDocument/didOpen")
        .collect();
    assert_eq!(opened.len(), 2, "{opened:?}");
    for m in &opened {
        assert_eq!(m["params"]["textDocument"]["languageId"], json!("cpp"));
    }
    let roots: Vec<&Value> = received
        .iter()
        .filter(|m| m["method"] == "initialize")
        .map(|m| &m["params"]["rootUri"])
        .collect();
    assert!(
        roots.contains(&&json!(tako_core::file_uri::from_path(&blink_root))),
        "{roots:?}"
    );
}

// --- 3. ALS なし・伴走なし（注入した解決）---------------------------------------------

/// 受け入れ条件: ALS なし・clangd なしでも壊れず、今の表示（色だけ）に落ちて理由が読める
#[test]
fn als_なしと伴走なしは未導入へ落ちて理由と導入コマンドが残る() {
    let scratch = Scratch::new("missing");
    let ino = scratch.sketch("Blink", true);
    let spec = spec();
    let command = spec.install.command();
    for (case, launch) in [
        (
            "als",
            Launch::NotFound {
                program: spec.program.to_string(),
                override_env: None,
            },
        ),
        (
            "clangd",
            Launch::CompanionMissing {
                program_path: "/opt/ide/arduino-language-server".to_string(),
                companion: "clangd".to_string(),
            },
        ),
    ] {
        let given = launch.clone();
        let manager = LspManager::new(config(Arc::new(move |_: &ServerSpec| given.clone())));
        let mut link = DocLink::default();
        manager.sync(&mut link, true, &ino, BLINK, 1);
        let mut status = Value::Null;
        wait_for(
            &format!("{case}: 未導入へ落ちる"),
            Duration::from_secs(20),
            || {
                status = status_of(&manager);
                status["state"] == json!("not_installed")
            },
        );
        let reason = status["reason"].as_str().unwrap_or_default();
        let next_step = status["next_step"].as_str().unwrap_or_default();
        assert_eq!(status["install_command"], json!(command), "{status:#}");
        assert!(next_step.contains(command), "{case}: {next_step}");
        match case {
            // 同梱も探したことを名指す（PATH だけを探したと読ませない）
            "als" => assert!(reason.contains("Arduino IDE"), "{reason}"),
            _ => {
                assert!(
                    reason.contains("clangd")
                        && reason.contains("/opt/ide/arduino-language-server"),
                    "{reason}"
                );
                assert_eq!(status["missing_companion"], json!("clangd"), "{status:#}");
            }
        }
        // 画面の帯も同じ文（`document_server`）
        let shown = manager.document_server(&ino).expect("状態が読める");
        assert_eq!(shown.state.slug(), "not_installed");
        assert_eq!(shown.reason.as_deref(), Some(reason));
        // `tako lsp servers` の行も未導入
        let row = manager.servers()["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(ID))
            .cloned()
            .unwrap();
        assert_eq!(row["installed"], json!(false), "{row:#}");
        manager.shutdown_all(Duration::from_secs(5));
        drop(link);
    }
}

// --- 4. 前提が未設定の器への問い合わせは待たない -------------------------------------

/// 境界: 基板が無いスケッチでホバー・定義ジャンプを頼んでも、上限（ここでは 10 秒）まで待たずに
/// 理由（`unconfigured`）で返る。サーバは起こさない
#[test]
fn 前提が未設定なら問い合わせは待たずに理由を返す() {
    let scratch = Scratch::new("query");
    let ino = scratch.sketch("NoBoard", false);
    let spawned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&spawned);
    let manager = LspManager::new(config(Arc::new(move |_: &ServerSpec| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Launch::Found {
            plan: ChildCmd {
                program: FAKE.to_string(),
                args: vec!["--scenario".into(), "normal".into()],
            },
            program_path: FAKE.to_string(),
        }
    })));
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &ino, BLINK, 1);
    wait_for("前提の未設定", Duration::from_secs(20), || {
        status_of(&manager)["state"] == json!("unconfigured")
    });
    let started = Instant::now();
    let hover = manager.hover(&HoverRequest {
        path: ino.clone(),
        line: 1,
        column: 3,
        timeout: Duration::from_secs(10),
        document: Some(BLINK.to_string()),
        superseding: false,
        open: true,
        ticket: None,
    });
    match hover {
        Err(HoverError::Query(GotoError::Unconfigured {
            server,
            reason,
            next_step,
        })) => {
            assert_eq!(server, ID);
            assert!(reason.contains("sketch.yaml"), "{reason}");
            assert!(next_step.contains(" board attach -b "), "{next_step}");
        }
        other => panic!("ホバーは前提の未設定で返る: {other:?}"),
    }
    let goto = manager.goto(&GotoRequest {
        kind: GotoKind::Definition,
        path: ino.clone(),
        line: 1,
        column: 3,
        timeout: Duration::from_secs(10),
        document: Some(BLINK.to_string()),
    });
    match goto {
        Err(error @ GotoError::Unconfigured { .. }) => {
            assert_eq!(error.status(), "unconfigured");
            assert_eq!(error.to_json(GotoKind::Definition)["server"], json!(ID));
        }
        other => panic!("定義ジャンプは前提の未設定で返る: {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "上限まで待たない: {:?}",
        started.elapsed()
    );
    // 解決はしたが（どの ALS かは状態に出る）、起こしてはいない
    let status = status_of(&manager);
    assert_eq!(status["spawn_count"], json!(0), "{status:#}");
    assert_eq!(status["program_path"], json!(FAKE), "{status:#}");
    assert!(spawned.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    manager.shutdown_all(Duration::from_secs(5));
    drop(link);
}
