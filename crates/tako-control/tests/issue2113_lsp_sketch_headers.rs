//! #2113 の e2e: Arduino のスケッチ内の `.h` / `.cpp` を arduino-language-server（ALS）の器へ回す
//!
//! 1. **フォルダの形で ALS の行**（`<フォルダ名>.ino` のあるフォルダか `src/` の上）。同じスケッチの
//!    `.ino` と同じ器（ルート = スケッチのフォルダ）で、`src/` の下も同じ器
//! 2. **本体を隠れて先に開く**: 利用者が `.ino` を開いていなければ tako がディスクから読んで、仲間より
//!    先に `didOpen` する（起動時の送り直しでも本体が先）。本体の診断は利用者の文書に数えない。
//!    最後の仲間が閉じたら本体も閉じる
//! 3. **利用者が本体を開いて閉じても**、仲間が開いていれば器に残り、本文はディスクの中身へ戻る
//! 4. **基板が無いスケッチの `.h`** は起こさず前提の未設定（帯の理由）
//! 5. **スケッチでないフォルダの `.h`** は今どおり C / C++ の行（clangd）
//! 6. **開いた文書は開いたときの器で引く**: 開いた後に本体を消しても、ホバー・帯は同じ器を指す
//! 7. **A/B**: `TAKO_2113_LEGACY=1`（子プロセス）は拡張子だけで引く = スケッチの `.h` も clangd
//!
//! 実機（Arduino IDE の同梱物で嘘のエラーが消える・ホバー / 補完）は `scripts/test-lsp-sketch-headers-2113.sh`
//! （隔離 HOME）。設計は `.agent/plans/2026-10-language-coverage.md` §5.3。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, GotoError, HoverError, HoverRequest, Launch, LspConfig, LspManager,
};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::{log_delay_args, stop_and_settle};

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");
const ALS: &str = "arduino-language-server";
const CLANGD: &str = "clangd";

/// 本体（URI の並びでは仲間の `Alpha.h` より後ろ = 送る順を表の並びに任せると仲間が先になる）
const INO: &str = "#include \"Alpha.h\"\n\nvoid setup() {\n  pinMode(LED_BUILTIN, OUTPUT);\n}\n\nvoid loop() {\n  toggle(LED_BUILTIN);\n}\n";
const HEADER: &str = "#pragma once\n#include <Arduino.h>\n\ninline void toggle(uint8_t pin) {\n  digitalWrite(pin, !digitalRead(pin));\n}\n";
const UTIL: &str = "#pragma once\ninline int twice(int v) { return v * 2; }\n";

/// 1 件ぶんの置き場（スコープを抜けると消える = #1312）
struct Scratch {
    dir: ScratchDir,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        Self {
            dir: ScratchDir::new(&format!("tako-2113-{tag}")),
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

    /// 根に `.git` を置いた repo の中のスケッチ `name`（本体・仲間 `Alpha.h`・`src/util/util.h`）。
    /// 返すのはスケッチのフォルダ
    fn sketch(&self, name: &str, board: bool) -> PathBuf {
        let _ = self.file("repo/.git/HEAD", "ref: refs/heads/main\n");
        let dir = format!("repo/{name}");
        self.file(&format!("{dir}/{name}.ino"), INO);
        self.file(&format!("{dir}/Alpha.h"), HEADER);
        self.file(&format!("{dir}/src/util/util.h"), UTIL);
        if board {
            self.file(
                &format!("{dir}/sketch.yaml"),
                "default_fqbn: arduino:avr:uno\n",
            );
        }
        self.path(&dir)
    }

    fn log(&self) -> PathBuf {
        self.path("received.jsonl")
    }
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

/// どの行も偽サーバで起こす（`extra` は偽サーバへ足す引数）
fn manager_with(scratch: &Scratch, scenario: &str, idle_grace: Duration) -> LspManager {
    let mut args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
    ];
    args.extend(log_delay_args());
    LspManager::new(LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: FAKE.to_string(),
                args: args.clone(),
            },
            program_path: FAKE.to_string(),
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace,
        restart: RestartPolicy {
            max_restarts: 0,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    })
}

/// `root` をルートに持つ `id` の器の状態（無ければ `null`）
fn server_at(manager: &LspManager, id: &str, root: &Path) -> Value {
    manager.status(Some(id))["servers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s["root"] == json!(root.display().to_string()))
        .unwrap_or(Value::Null)
}

fn open(manager: &LspManager, path: &Path, text: &str) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, text, 1);
    assert!(
        matches!(link, DocLink::Open(_)),
        "{} が開かない: {link:?}",
        path.display()
    );
    link
}

fn uri(path: &Path) -> String {
    tako_core::file_uri::from_path(path)
}

/// 偽サーバに届いた `method` の URI の並び
fn uris_of(log: &Path, method: &str) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["method"] == json!(method))
        .map(|m| {
            m["params"]["textDocument"]["uri"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn messages(log: &Path, method: &str) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["method"] == json!(method))
        .collect()
}

// --- 1 / 2. 本体を隠れて先に開く --------------------------------------------------------

/// 受け入れ条件: 利用者が本体を開かずに `.h` だけを開いても、ALS の器へ本体 → 仲間の順に届く。
/// `src/` の下の `.h` も同じ器。起こし直しても本体が先。本体の診断は利用者の文書に数えない。
/// 最後の仲間が閉じたら本体も閉じる（仲間の後）
#[test]
fn 本体を開いていない_h_は本体を隠れて先に開き_最後の仲間で手放す() {
    let scratch = Scratch::new("hidden");
    let sketch = scratch.sketch("Zap", true);
    let ino = sketch.join("Zap.ino");
    let header = sketch.join("Alpha.h");
    let util = sketch.join("src/util/util.h");
    let manager = manager_with(&scratch, "normal", DEFAULT_IDLE_GRACE);
    for path in [&header, &util] {
        let got = manager.resolve(path).expect("受け持つ行");
        assert_eq!(
            (got.spec.id, got.language_id),
            (ALS, "cpp"),
            "{}",
            path.display()
        );
    }
    let header_link = open(&manager, &header, HEADER);
    wait_for("ALS の器が稼働", Duration::from_secs(30), || {
        server_at(&manager, ALS, &sketch)["state"] == json!("running")
    });
    // 偽サーバは didOpen ごとに診断を 1 件 publish する: 本体のぶんは利用者の文書に数えない
    wait_for("仲間と本体の診断", Duration::from_secs(30), || {
        manager.diagnostics_retained().1 >= 2
    });
    let status = server_at(&manager, ALS, &sketch);
    assert_eq!(status["documents"], json!(1), "{status:#}");
    assert_eq!(status["hidden_documents"], json!(1), "{status:#}");
    assert_eq!(
        status["diagnostics"],
        json!(1),
        "本体の診断を数えない: {status:#}"
    );
    assert_eq!(
        status["views"],
        json!(1),
        "本体に持ち手（ペイン）は無い: {status:#}"
    );
    assert_eq!(manager.diagnostics_count(&header), 1);
    assert!(manager.has_document(&ino), "本体は器に開いている");
    // `src/` の下の仲間も同じ器（ルート = スケッチのフォルダ。器は 1 つ）
    let util_link = open(&manager, &util, UTIL);
    wait_for(
        "src の下の仲間の診断",
        Duration::from_secs(30),
        || manager.diagnostics_count(&util) == 1,
    );
    let all = manager.status(Some(ALS))["servers"].clone();
    assert_eq!(all.as_array().map(Vec::len), Some(1), "器は 1 つ: {all:#}");
    let status = server_at(&manager, ALS, &sketch);
    assert_eq!(
        (&status["documents"], &status["hidden_documents"]),
        (&json!(2), &json!(1)),
        "{status:#}"
    );
    // 起こし直しても本体が先
    let generation = status["generation"].as_u64().unwrap();
    manager.restart(Some(ALS));
    wait_for("起こし直して稼働", Duration::from_secs(30), || {
        let s = server_at(&manager, ALS, &sketch);
        s["state"] == json!("running") && s["generation"].as_u64() > Some(generation)
    });
    // 仲間を 1 つ閉じても本体は残る（もう 1 つの仲間が開いている）
    drop(header_link);
    assert!(manager.has_document(&ino), "仲間が残っていれば本体も残る");
    // 最後の仲間を閉じたら本体も閉じる
    drop(util_link);
    assert!(!manager.has_document(&ino), "最後の仲間で本体を手放す");
    assert_eq!(manager.diagnostics_retained(), (0, 0), "本体の診断も捨てた");

    stop_and_settle(&manager, &scratch.log());
    let opened = uris_of(&scratch.log(), "textDocument/didOpen");
    assert_eq!(
        opened,
        [
            uri(&ino),
            uri(&header),
            uri(&util),
            // 起こし直し: 本体 → 仲間（仲間どうしは URI の並び）
            uri(&ino),
            uri(&header),
            uri(&util),
        ],
        "本体を仲間より先に送る"
    );
    for m in messages(&scratch.log(), "textDocument/didOpen") {
        assert_eq!(m["params"]["textDocument"]["languageId"], json!("cpp"));
    }
    let main_text = messages(&scratch.log(), "textDocument/didOpen")[0]["params"]["textDocument"]
        ["text"]
        .clone();
    assert_eq!(main_text, json!(INO), "本体はディスクの中身");
    assert_eq!(
        uris_of(&scratch.log(), "textDocument/didClose"),
        [uri(&header), uri(&util), uri(&ino)],
        "仲間を閉じてから本体を閉じる"
    );
    let roots: Vec<Value> = messages(&scratch.log(), "initialize")
        .iter()
        .map(|m| m["params"]["rootUri"].clone())
        .collect();
    assert_eq!(roots, [json!(uri(&sketch)), json!(uri(&sketch))]);
    // ユーザーのファイルは書き換えない
    assert_eq!(std::fs::read_to_string(&ino).unwrap(), INO);
}

// --- 3. 利用者が本体を開いて閉じる ---------------------------------------------------------

/// 境界: 利用者が本体を開いて編集し、保存せずに閉じても、仲間が開いていれば器に残り（`didClose` を
/// 送らない）、サーバの本文はディスクの中身へ戻る。最後の仲間を閉じたら本体も閉じる。
/// 隠れて開いていた本体を利用者が開いたら、利用者の本文へ揃える
#[test]
fn 利用者が本体を閉じても仲間が開いていれば器に残り本文はディスクへ戻る() {
    let scratch = Scratch::new("user-main");
    let sketch = scratch.sketch("Zap", true);
    let ino = sketch.join("Zap.ino");
    let header = sketch.join("Alpha.h");
    // 全文同期 = 届いた didChange の本文をそのまま比べられる
    let manager = manager_with(&scratch, "full-sync", DEFAULT_IDLE_GRACE);
    let mut ino_link = open(&manager, &ino, INO);
    let header_link = open(&manager, &header, HEADER);
    wait_for("ALS の器が稼働", Duration::from_secs(30), || {
        server_at(&manager, ALS, &sketch)["state"] == json!("running")
    });
    let edited = format!("{INO}// 保存しない編集\n");
    manager.sync(&mut ino_link, true, &ino, &edited, 2);
    let status = server_at(&manager, ALS, &sketch);
    assert_eq!(
        (&status["documents"], &status["hidden_documents"]),
        (&json!(2), &json!(0)),
        "利用者が開いている本体は隠れた本体に数えない: {status:#}"
    );
    drop(ino_link);
    assert!(manager.has_document(&ino), "仲間が開いているので器に残る");
    let status = server_at(&manager, ALS, &sketch);
    assert_eq!(status["hidden_documents"], json!(1), "{status:#}");
    // 隠れていた本体を利用者が開き直したら、利用者の本文へ揃える
    let reopened = format!("{INO}// 開き直した本文\n");
    let again = open(&manager, &ino, &reopened);
    drop(again);
    drop(header_link);
    assert!(!manager.has_document(&ino));

    stop_and_settle(&manager, &scratch.log());
    assert_eq!(
        uris_of(&scratch.log(), "textDocument/didOpen"),
        [uri(&ino), uri(&header)],
        "利用者が先に開いた本体は 1 回だけ開く"
    );
    let texts: Vec<Value> = messages(&scratch.log(), "textDocument/didChange")
        .iter()
        .filter(|m| m["params"]["textDocument"]["uri"] == json!(uri(&ino)))
        .map(|m| m["params"]["contentChanges"][0]["text"].clone())
        .collect();
    assert_eq!(
        texts,
        [json!(edited), json!(INO), json!(reopened), json!(INO)],
        "編集 → 閉じてディスクへ → 開き直した本文 → 閉じてディスクへ"
    );
    assert_eq!(
        uris_of(&scratch.log(), "textDocument/didClose"),
        [uri(&header), uri(&ino)],
        "本体の didClose は最後の仲間の後の 1 回だけ"
    );
    assert_eq!(
        std::fs::read_to_string(&ino).unwrap(),
        INO,
        "ファイルは書き換えない"
    );
}

// --- 4. 基板なし ---------------------------------------------------------------------

/// 受け入れ条件: 基板が無いスケッチの `.h` は今の表示（色だけ）= 起こさず前提の未設定で、帯の理由を
/// 返す。問い合わせは待たずに理由で返る（エラーで固まらない）
#[test]
fn 基板の無いスケッチの_h_は起こさず前提の未設定() {
    let scratch = Scratch::new("no-board");
    let sketch = scratch.sketch("NoBoard", false);
    let header = sketch.join("Alpha.h");
    let manager = manager_with(&scratch, "normal", DEFAULT_IDLE_GRACE);
    let link = open(&manager, &header, HEADER);
    wait_for("前提の未設定", Duration::from_secs(30), || {
        server_at(&manager, ALS, &sketch)["state"] == json!("unconfigured")
    });
    let status = server_at(&manager, ALS, &sketch);
    assert_eq!(status["spawn_count"], json!(0), "{status:#}");
    assert_eq!(status["hidden_documents"], json!(1), "{status:#}");
    let shown = manager.document_server(&header).expect("帯に出す状態");
    assert_eq!((shown.id, shown.state.slug()), (ALS, "unconfigured"));
    assert!(
        shown
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("sketch.yaml"),
        "{:?}",
        shown.reason
    );
    assert!(shown
        .next_step
        .as_deref()
        .unwrap_or_default()
        .contains(" board attach -b "));
    let hover = manager.hover(&HoverRequest {
        path: header.clone(),
        line: 4,
        column: 4,
        timeout: Duration::from_secs(10),
        document: None,
        superseding: false,
        open: true,
        ticket: None,
    });
    assert!(
        matches!(
            hover,
            Err(HoverError::Query(GotoError::Unconfigured {
                server: ALS,
                ..
            }))
        ),
        "{hover:?}"
    );
    assert_eq!(manager.diagnostics_count(&header), 0);
    manager.shutdown_all(Duration::from_secs(5));
    drop(link);
    assert_eq!(
        lsp_fake_e2e::received(&scratch.log(), "initialize"),
        0,
        "サーバは起こしていない"
    );
}

// --- 5. スケッチでないフォルダ -----------------------------------------------------------

/// 受け入れ条件: スケッチでないフォルダの `.h` / `.cpp` は今どおり C / C++ の行（本体も開かない）。
/// フォルダ名と大小だけ違う `.ino` があるフォルダも形に合わない（arduino-cli が組めない形）
#[test]
fn スケッチでないフォルダの_h_と_cpp_は今どおり_c_cpp_の行() {
    let scratch = Scratch::new("plain");
    let header = scratch.file("repo/lib/lib.h", "int f(void);\n");
    let source = scratch.file("repo/lib/lib.cpp", "int f() { return 1; }\n");
    scratch.file("repo/blinkc/Blinkc.ino", INO);
    let near = scratch.file("repo/blinkc/near.h", HEADER);
    let manager = manager_with(&scratch, "normal", DEFAULT_IDLE_GRACE);
    for (path, language_id) in [(&header, "c"), (&source, "cpp"), (&near, "c")] {
        let got = manager.resolve(path).expect("受け持つ行");
        assert_eq!(
            (got.spec.id, got.language_id),
            (CLANGD, language_id),
            "{}",
            path.display()
        );
    }
    let links: Vec<DocLink> = [
        (&header, "int f(void);\n"),
        (&source, "int f() { return 1; }\n"),
    ]
    .into_iter()
    .map(|(path, text)| open(&manager, path, text))
    .collect();
    wait_for("clangd の器が稼働", Duration::from_secs(30), || {
        manager.status(Some(CLANGD))["servers"][0]["state"] == json!("running")
    });
    assert_eq!(
        manager.status(Some(ALS))["servers"],
        json!([]),
        "ALS の器は無い"
    );
    let status = &manager.status(Some(CLANGD))["servers"][0];
    assert_eq!(status["hidden_documents"], json!(0), "{status:#}");
    stop_and_settle(&manager, &scratch.log());
    drop(links);
    // 2 つ目を開いたときに起動中なら起動の後に並びで送る = 順は起動の速さで変わる（集合で比べる）
    let mut got: Vec<(String, String)> = messages(&scratch.log(), "textDocument/didOpen")
        .iter()
        .map(|m| {
            let doc = &m["params"]["textDocument"];
            (
                doc["uri"].as_str().unwrap_or_default().to_string(),
                doc["languageId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    got.sort();
    let mut want = vec![
        (uri(&header), "c".to_string()),
        (uri(&source), "cpp".to_string()),
    ];
    want.sort();
    assert_eq!(got, want, "本体は開かない");
}

// --- 6. 開いた文書は開いたときの器で引く ---------------------------------------------------

/// 境界: 開いた後に本体を消しても（フォルダの形が崩れても）、開いている文書は開いたときの器で引く
/// （帯・ホバーが別の器を指さない）。閉じた後は表の答え（形が崩れた = C / C++ の行）
#[test]
fn 開いた後に本体が消えても開いたときの器で引く() {
    let scratch = Scratch::new("recorded");
    let sketch = scratch.sketch("Zap", true);
    let ino = sketch.join("Zap.ino");
    let header = sketch.join("Alpha.h");
    let manager = manager_with(&scratch, "normal", DEFAULT_IDLE_GRACE);
    let link = open(&manager, &header, HEADER);
    wait_for("ALS の器が稼働", Duration::from_secs(30), || {
        server_at(&manager, ALS, &sketch)["state"] == json!("running")
    });
    std::fs::remove_file(&ino).unwrap();
    assert_eq!(
        servers::resolve(&header).map(|r| r.spec.id),
        Some(CLANGD),
        "表の答えは形が崩れたので変わる"
    );
    assert_eq!(manager.resolve(&header).map(|r| r.spec.id), Some(ALS));
    assert_eq!(manager.document_server(&header).map(|s| s.id), Some(ALS));
    let answer = manager
        .hover(&HoverRequest {
            path: header.clone(),
            line: 4,
            column: 4,
            timeout: Duration::from_secs(10),
            document: None,
            superseding: false,
            open: true,
            ticket: None,
        })
        .expect("開いた器が答える");
    assert_eq!(answer.server, ALS);
    assert_eq!(
        manager.status(Some(CLANGD))["servers"],
        json!([]),
        "clangd を起こさない"
    );
    drop(link);
    assert_eq!(manager.resolve(&header).map(|r| r.spec.id), Some(CLANGD));
    manager.shutdown_all(Duration::from_secs(5));
}

// --- 7. A/B（子プロセス）--------------------------------------------------------------

const LEGACY_CHILD_ENV: &str = "TAKO_2113_CHILD";

/// 子プロセス側（`LEGACY_CHILD_ENV` があるときだけ動く）: 受け持つ行と器を JSON で書く
#[test]
fn 子プロセス_スケッチの_h_を開いて受け持つ器を書く() {
    let Ok(out) = std::env::var(LEGACY_CHILD_ENV) else {
        return;
    };
    let scratch = Scratch::new("child");
    let sketch = scratch.sketch("Zap", true);
    let header = sketch.join("Alpha.h");
    let manager = manager_with(&scratch, "normal", DEFAULT_IDLE_GRACE);
    let resolved = manager.resolve(&header).map(|r| r.spec.id);
    let link = open(&manager, &header, HEADER);
    wait_for("器が稼働", Duration::from_secs(30), || {
        manager.status(None)["servers"][0]["state"] == json!("running")
    });
    let servers: Vec<Value> = manager.status(None)["servers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| json!({ "id": s["id"], "root": s["root"], "hidden": s["hidden_documents"] }))
        .collect();
    stop_and_settle(&manager, &scratch.log());
    drop(link);
    let result = json!({
        "resolved": resolved,
        "servers": servers,
        "opened": uris_of(&scratch.log(), "textDocument/didOpen").len(),
    });
    std::fs::write(out, result.to_string()).unwrap();
}

/// A/B: 同じスケッチの `.h` が、旧（`TAKO_2113_LEGACY=1` = 拡張子だけ）では clangd の器で本体を開かず、
/// 新では ALS の器で本体と仲間の 2 つを開く
#[test]
fn 旧挙動ではスケッチの_h_も_clangd_で本体を開かない() {
    let run = |legacy: bool| -> Value {
        let scratch = Scratch::new(if legacy { "ab-old" } else { "ab-new" });
        let out = scratch.path("result.json");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "子プロセス_スケッチの_h_を開いて受け持つ器を書く",
                "--nocapture",
            ])
            .env(LEGACY_CHILD_ENV, &out)
            .env_remove("TAKO_2113_LEGACY")
            .stdin(std::process::Stdio::null());
        if legacy {
            child.env("TAKO_2113_LEGACY", "1");
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "子プロセスが落ちた（{}）: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap()
    };
    let old = run(true);
    assert_eq!(old["resolved"], json!(CLANGD), "{old:#}");
    assert_eq!(old["servers"][0]["id"], json!(CLANGD), "{old:#}");
    assert_eq!(old["opened"], json!(1), "本体を開かない: {old:#}");
    let new = run(false);
    assert_eq!(new["resolved"], json!(ALS), "{new:#}");
    assert_eq!(new["servers"][0]["id"], json!(ALS), "{new:#}");
    assert_eq!(new["servers"][0]["hidden"], json!(1), "{new:#}");
    assert_eq!(new["opened"], json!(2), "本体と仲間: {new:#}");
}

// --- 8. 実機（Arduino IDE の同梱物。GUI を除いた tako の実経路）-------------------------------

/// 実機の検査の入口: `TAKO_2113_REAL_HOME`（arduino:avr のコアを複製した隔離 HOME）と
/// `TAKO_2113_REAL_SKETCHES`（`scripts/test-lsp-sketch-headers-2113.sh` が用意する。Blink = `.ino` を先に・
/// Blink2 = `.h` だけ・Blink3 = `src/` の `.h` だけ・NoBoard = 基板なし・plain = スケッチでない）を
/// 渡したときだけ動く（無ければ何もしない = CI・IDE の無い機）。新（既定）と旧（`TAKO_2113_LEGACY=1`）を
/// 子プロセスで 1 回ずつ走らせ、同じ素材で比べる
#[test]
fn 実機_スケッチの_h_と_cpp_に嘘のエラーが出ずホバーと補完が出る() {
    let (Ok(home), Ok(sketches)) = (
        std::env::var("TAKO_2113_REAL_HOME"),
        std::env::var("TAKO_2113_REAL_SKETCHES"),
    ) else {
        return;
    };
    let run = |legacy: bool| -> Value {
        let scratch = Scratch::new(if legacy { "real-old" } else { "real-new" });
        let out = scratch.path("real.json");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "子プロセス_実機の同梱物でスケッチの_h_と_cpp_を問い合わせる",
                "--nocapture",
            ])
            .env(REAL_CHILD_ENV, &out)
            .env("TAKO_2113_REAL_SKETCHES", &sketches)
            .env("TAKO_DATA_DIR", scratch.path("data"))
            .env("HOME", &home)
            .env_remove("TAKO_2113_LEGACY")
            .env_remove(servers::override_env_name(ALS))
            .env_remove(servers::override_env_name(CLANGD))
            .stdin(std::process::Stdio::null());
        if legacy {
            child.env("TAKO_2113_LEGACY", "1");
        }
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
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap()
    };
    let new = run(false);
    println!(
        "TAKO_2113_REAL_NEW: {}",
        serde_json::to_string_pretty(&new).unwrap()
    );
    let old = run(true);
    println!(
        "TAKO_2113_REAL_OLD: {}",
        serde_json::to_string_pretty(&old).unwrap()
    );

    // 受け入れ条件 1: `.ino` を先に開いた場合と `.h` だけを開いた場合の両方で、嘘のエラー 0 件・ホバー / 補完
    for flow in ["ino_first", "h_only"] {
        let got = &new[flow];
        for doc in ["h", "cpp"] {
            assert_eq!(got[doc]["server"], json!(ALS), "{flow} {doc}: {got:#}");
            assert_eq!(got[doc]["published"], json!(true), "{flow} {doc}: {got:#}");
            assert_eq!(got[doc]["errors"], json!(0), "{flow} {doc}: {got:#}");
        }
        assert!(
            got["hover_h"]
                .as_str()
                .unwrap_or_default()
                .contains("digitalWrite"),
            "{flow}: {got:#}"
        );
        assert_eq!(
            got["completion_has_println"],
            json!(true),
            "{flow}: {got:#}"
        );
        assert_eq!(
            got["errors_after_queries"],
            json!({ "h": 0, "cpp": 0 }),
            "{flow}: 答えが出た後も 0 件: {got:#}"
        );
    }
    assert_eq!(new["h_only"]["hidden_documents"], json!(1), "{new:#}");
    assert_eq!(new["ino_first"]["hidden_documents"], json!(0), "{new:#}");
    let src = &new["src_only"];
    assert_eq!(
        (
            &src["util"]["server"],
            &src["util"]["errors"],
            &src["util"]["published"]
        ),
        (&json!(ALS), &json!(0), &json!(true)),
        "{src:#}"
    );
    // 受け入れ条件 2: 基板が無いスケッチの `.h` は起こさず前提の未設定
    let no_board = &new["no_board"];
    assert_eq!(no_board["state"], json!("unconfigured"), "{no_board:#}");
    assert_eq!(no_board["spawn_count"], json!(0), "{no_board:#}");
    assert_eq!(no_board["errors"], json!(0), "{no_board:#}");
    // 受け入れ条件 4: 旧（拡張子のまま clangd）は同じ `.h` / `.cpp` に嘘のエラー
    for doc in ["h", "cpp"] {
        let got = &old["h_only"][doc];
        assert_eq!(got["server"], json!(CLANGD), "{doc}: {old:#}");
        assert!(
            got["errors"].as_u64() > Some(0),
            "旧は嘘のエラー {doc}: {old:#}"
        );
        assert!(
            old["h_only"]["errors_after_queries"][doc].as_u64() > Some(0),
            "旧は問い合わせた後も嘘のエラー {doc}: {old:#}"
        );
    }
    // 受け入れ条件 3: スケッチでないフォルダは新旧とも clangd で、答えが同じ
    for doc in ["h", "cpp"] {
        assert_eq!(new["plain"][doc]["server"], json!(CLANGD), "{new:#}");
        assert_eq!(new["plain"][doc], old["plain"][doc], "新旧で同じ: {doc}");
    }
}

const REAL_CHILD_ENV: &str = "TAKO_2113_REAL_CHILD";

/// 実機の子プロセス側（`REAL_CHILD_ENV` があるときだけ動く）
#[test]
fn 子プロセス_実機の同梱物でスケッチの_h_と_cpp_を問い合わせる() {
    let Ok(out) = std::env::var(REAL_CHILD_ENV) else {
        return;
    };
    let sketches = PathBuf::from(std::env::var("TAKO_2113_REAL_SKETCHES").unwrap());
    let legacy = std::env::var_os("TAKO_2113_LEGACY").is_some();
    let mut result = serde_json::Map::new();
    let new_manager = || {
        let mut config = LspConfig {
            table: servers::SERVERS,
            launcher: Arc::new(tako_control::lsp::manager::default_launch),
            request_timeout: Duration::from_secs(60),
            shutdown_timeout: Duration::from_secs(5),
            idle_grace: DEFAULT_IDLE_GRACE,
            restart: RestartPolicy {
                max_restarts: 0,
                base_delay_ms: 50,
            },
            raw_log_dir: None,
        };
        config.request_timeout = Duration::from_secs(60);
        LspManager::new(config)
    };
    // ① `.ino` を先に開く（IDE の順）→ `.h` / `.cpp`
    {
        let manager = new_manager();
        let dir = sketches.join("Blink");
        let mut links = vec![real_open(&manager, &dir.join("Blink.ino"))];
        if !legacy {
            real_wait_running(&manager, ALS, &dir);
        }
        links.push(real_open(&manager, &dir.join("helper.h")));
        links.push(real_open(&manager, &dir.join("helper.cpp")));
        let mut flow = real_measure(&manager, &dir, &["Blink.ino", "helper.h", "helper.cpp"]);
        flow.insert("hidden_documents".into(), real_hidden(&manager, &dir));
        real_queries(&manager, &dir, &mut flow);
        result.insert("ino_first".into(), Value::Object(flow));
        manager.shutdown_all(Duration::from_secs(5));
        drop(links);
    }
    // ② `.h` だけ（利用者は本体を開かない。新では tako が隠れて開く）→ `.cpp`
    {
        let manager = new_manager();
        let dir = sketches.join("Blink2");
        let links = [
            real_open(&manager, &dir.join("helper.h")),
            real_open(&manager, &dir.join("helper.cpp")),
        ];
        let mut flow = real_measure(&manager, &dir, &["helper.h", "helper.cpp"]);
        flow.insert("hidden_documents".into(), real_hidden(&manager, &dir));
        real_queries(&manager, &dir, &mut flow);
        result.insert("h_only".into(), Value::Object(flow));
        manager.shutdown_all(Duration::from_secs(5));
        drop(links);
    }
    // ③ `src/` の下の `.h` だけ
    if !legacy {
        let manager = new_manager();
        let dir = sketches.join("Blink3");
        let link = real_open(&manager, &dir.join("src/util.h"));
        let mut flow = real_measure(&manager, &dir, &["src/util.h"]);
        let util = flow.remove("src/util.h").unwrap_or(Value::Null);
        flow.insert("util".into(), util);
        result.insert("src_only".into(), Value::Object(flow));
        manager.shutdown_all(Duration::from_secs(5));
        drop(link);
    }
    // ④ 基板なし
    if !legacy {
        let manager = new_manager();
        let dir = sketches.join("NoBoard");
        let header = dir.join("helper.h");
        let link = real_open(&manager, &header);
        let mut status = Value::Null;
        wait_for(
            "基板なしは前提の未設定",
            Duration::from_secs(60),
            || {
                status = server_at(&manager, ALS, &dir);
                status["state"] == json!("unconfigured")
            },
        );
        std::thread::sleep(Duration::from_secs(2));
        result.insert(
            "no_board".into(),
            json!({
                "state": status["state"],
                "spawn_count": server_at(&manager, ALS, &dir)["spawn_count"],
                "errors": manager.diagnostics_count(&header),
                "band": manager.document_server(&header).map(|s| s.state.slug()),
            }),
        );
        manager.shutdown_all(Duration::from_secs(5));
        drop(link);
    }
    // ⑤ スケッチでないフォルダ
    {
        let manager = new_manager();
        let dir = sketches.join("plain");
        let links = [
            real_open(&manager, &dir.join("plain.h")),
            real_open(&manager, &dir.join("plain.cpp")),
        ];
        let flow = real_measure(&manager, &dir, &["plain.h", "plain.cpp"]);
        result.insert("plain".into(), Value::Object(flow));
        manager.shutdown_all(Duration::from_secs(5));
        drop(links);
    }
    std::fs::write(out, Value::Object(result).to_string()).unwrap();
}

fn real_open(manager: &LspManager, path: &Path) -> DocLink {
    open(manager, path, &std::fs::read_to_string(path).unwrap())
}

fn real_wait_running(manager: &LspManager, id: &str, root: &Path) {
    wait_for("実のサーバが稼働", Duration::from_secs(90), || {
        server_at(manager, id, root)["state"] == json!("running")
    });
}

fn real_hidden(manager: &LspManager, root: &Path) -> Value {
    server_at(manager, ALS, root)["hidden_documents"].clone()
}

/// 開いた文書（`dir` からの相対）の診断が出揃うのを待って数える。出揃い = 開いた文書（と隠れた本体）が
/// すべて 1 度は publish され、その後 [`REAL_SETTLE`] のあいだ数が変わらない（ALS は clangd の診断を
/// 組み直しのたびに送り直す）。上限 120 秒で打ち切り、`published: false` と書く
fn real_measure(manager: &LspManager, dir: &Path, docs: &[&str]) -> serde_json::Map<String, Value> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(120);
    let paths: Vec<PathBuf> = docs.iter().map(|d| dir.join(d)).collect();
    // 開いている文書の数（隠れた本体を含む）= publish された URI の数が揃えば全部 1 度は届いた
    let expected =
        paths.len() + usize::try_from(real_hidden(manager, dir).as_u64().unwrap_or(0)).unwrap_or(0);
    let snapshot =
        || -> Vec<usize> { paths.iter().map(|p| manager.diagnostics_count(p)).collect() };
    let mut published_ms = None;
    let mut last = snapshot();
    let mut stable_since = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let (_, uris) = manager.diagnostics_retained();
        if published_ms.is_none() && uris >= expected {
            published_ms = Some(started.elapsed().as_millis());
            stable_since = Instant::now();
        }
        let now = snapshot();
        if now != last {
            last = now;
            stable_since = Instant::now();
        }
        if published_ms.is_some() && stable_since.elapsed() >= REAL_SETTLE {
            break;
        }
    }
    let mut flow = serde_json::Map::new();
    flow.insert("published_ms".into(), json!(published_ms));
    for (doc, path) in docs.iter().zip(&paths) {
        let key = match *doc {
            d if d.ends_with(".h") && !d.contains('/') => "h".to_string(),
            d if d.ends_with(".cpp") => "cpp".to_string(),
            d if d.ends_with(".ino") => "ino".to_string(),
            d => d.to_string(),
        };
        let uri = tako_core::file_uri::from_path(path);
        let items = manager
            .document_diagnostics(&uri)
            .map(|d| d.diagnostics.items.to_vec())
            .unwrap_or_default();
        let errors = items
            .iter()
            .filter(|d| d.severity == tako_core::lsp::diagnostic::Severity::Error)
            .count();
        flow.insert(
            key,
            json!({
                "server": manager.resolve(path).map(|r| r.spec.id),
                "published": published_ms.is_some(),
                "errors": errors,
                "diagnostics": items.len(),
                "messages": items
                    .iter()
                    .map(|d| format!("{}: {}", d.start.line + 1, d.message.lines().next().unwrap_or("")))
                    .collect::<Vec<_>>(),
            }),
        );
    }
    flow
}

/// 診断の数が変わらなければ出揃ったとみなす時間
const REAL_SETTLE: Duration = Duration::from_secs(6);

/// ホバー（`.h` の `digitalWrite`）と補完（`.cpp` の `Serial.` の後ろ）
fn real_queries(manager: &LspManager, dir: &Path, flow: &mut serde_json::Map<String, Value>) {
    let mut hover = String::new();
    for _ in 0..20 {
        hover = manager
            .hover(&HoverRequest {
                path: dir.join("helper.h"),
                line: 4,
                column: 4,
                timeout: Duration::from_secs(60),
                document: None,
                superseding: false,
                open: true,
                ticket: None,
            })
            .ok()
            .and_then(|a| a.content)
            .map(|h| h.value)
            .unwrap_or_default();
        if !hover.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    flow.insert(
        "hover_h".into(),
        json!(hover.lines().take(3).collect::<Vec<_>>().join(" / ")),
    );
    let completion = manager.completion(&tako_control::lsp::CompletionRequest {
        path: dir.join("helper.cpp"),
        line: 3,
        column: 9,
        timeout: Duration::from_secs(60),
        document: None,
        trigger: tako_core::lsp::completion::Trigger::Character('.'),
        superseding: false,
        ticket: None,
        resolve_top: 0,
    });
    let labels: Vec<String> = completion
        .as_ref()
        .map(|a| a.items.iter().map(|i| i.label.trim().to_string()).collect())
        .unwrap_or_default();
    flow.insert(
        "completion_head".into(),
        json!(labels.iter().take(6).collect::<Vec<_>>()),
    );
    flow.insert(
        "completion_has_println".into(),
        json!(labels.iter().any(|l| l.starts_with("println"))),
    );
    // 答えが出た = サーバが文書を読み終えた後にもう一度数える（0 件が「最初の空の publish を数えただけ」で
    // ないことの裏取り。ALS は clangd の診断を組み直しのたびに送り直す）
    std::thread::sleep(REAL_SETTLE);
    let errors_after = |name: &str| {
        let uri = tako_core::file_uri::from_path(&dir.join(name));
        manager.document_diagnostics(&uri).map_or(0, |d| {
            d.diagnostics
                .items
                .iter()
                .filter(|d| d.severity == tako_core::lsp::diagnostic::Severity::Error)
                .count()
        })
    };
    flow.insert(
        "errors_after_queries".into(),
        json!({ "h": errors_after("helper.h"), "cpp": errors_after("helper.cpp") }),
    );
}
