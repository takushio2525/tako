//! #1679 の e2e: 偽の言語サーバ（`tako-lsp-fake`）が返した `Diagnostic` 配列が、
//! `tako lsp diagnostics`（= MCP `tako_lsp`。どちらも `dispatch::lsp_diagnostics` の 1 実装）の
//! 応答で**固定値と一致する**ことを実プロセスで測る。
//!
//! 固定するもの: UTF-16 の桁 → UTF-8 バイト（日本語・絵文字）/ 重大度の絞り込みの境界 2 件
//! （`error` は省略もエラーとして含み警告を含まない・`warning` は警告を含み情報を含まない）/
//! 行末を超える範囲の丸め / UI へのキューの上限（溢れたら印）/ サーバを止めたら・文書を
//! 閉じたら保持 0（#830 の機序）。
//!
//! A/B（検出力の実証）: `TAKO_1007_LEGACY=1 cargo test -p tako-control --test
//! issue1679_lsp_diagnostics` で LSP を丸ごと止めると、ここが FAILED になる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::dispatch::{dispatch, DispatchError};
use tako_control::lsp::{
    manager::DEFAULT_IDLE_GRACE, DocLink, Launch, LspConfig, LspDocument, LspManager,
    DIAGNOSTICS_EVENT_CAPACITY,
};
use tako_control::protocol::{PreviewModeWire, Request};
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 日本語と絵文字（サロゲートペア）の行を含む。桁は下の配列が UTF-16 で指す
const SOURCE: &str = "fn main() {\n    let 日本 = \"😀\";\n    let x: i32 = \"s\";\n}\n";

/// 偽サーバが返す配列（LSP の座標 = 0 起点の行・UTF-16 の桁）
fn fixture() -> Value {
    json!([
        // 1 行目 `main`（ヒント）
        {"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}},
         "severity": 4, "message": "hint"},
        // 2 行目 `日本` = UTF-16 で 8..10 → UTF-8 で 8..14（警告）
        {"range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 10}},
         "severity": 2, "message": "unused variable", "source": "rustc", "code": "unused_variables"},
        // 2 行目 `"😀"` = UTF-16 で 13..17 → UTF-8 で 17..23（情報）
        {"range": {"start": {"line": 1, "character": 13}, "end": {"line": 1, "character": 17}},
         "severity": 3, "message": "info"},
        // 3 行目 `"s"`（エラー）
        {"range": {"start": {"line": 2, "character": 17}, "end": {"line": 2, "character": 20}},
         "severity": 1, "message": "mismatched types", "source": "rustc", "code": "E0308"},
        // 3 行目の行末を超える終点（`;` から桁 99）→ 行末へ丸める（エラー）
        {"range": {"start": {"line": 2, "character": 20}, "end": {"line": 2, "character": 99}},
         "severity": 1, "message": "past end"},
        // 重大度の省略 = エラー（数値のコード）
        {"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 1}},
         "message": "no severity", "code": 7},
    ])
}

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str, diagnostics: &Value) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1679-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(dir.join("src").join("main.rs"), SOURCE).unwrap();
        std::fs::write(dir.join("diagnostics.json"), diagnostics.to_string()).unwrap();
        Self(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("src").join("main.rs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(scratch: &Scratch) -> LspConfig {
    let args = vec![
        "--diagnostics".to_string(),
        scratch.0.join("diagnostics.json").display().to_string(),
    ];
    LspConfig {
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
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {limit:?} 以内に起きなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// GUI の代わり: ペイン 1 枚をプレビューペインとして見せ、その編集セッションの
/// [`DocLink`] を `lsp_documents` で返す（GUI の `EditState` と同じ役）
struct Host {
    workspace: Workspace,
    manager: LspManager,
    preview: PaneId,
    path: PathBuf,
    link: DocLink,
    editing: bool,
}

impl Host {
    fn new(manager: LspManager, path: &Path) -> Self {
        let workspace = Workspace::new("1", Pane::new(PaneOrigin::User));
        let preview = workspace.active_tab().tree().focused();
        Self {
            workspace,
            manager,
            preview,
            path: path.to_path_buf(),
            link: DocLink::default(),
            editing: false,
        }
    }

    /// 編集モードに入る（GUI の `sync_preview_lsp` と同じ呼び方）
    fn edit(&mut self, text: &str, version: u64) {
        self.editing = true;
        self.manager
            .sync(&mut self.link, true, &self.path, text, version);
    }

    fn uri(&self) -> String {
        match &self.link {
            DocLink::Open(lease) => lease.uri().to_string(),
            other => panic!("文書が開いていない: {other:?}"),
        }
    }

    fn diagnostics(&mut self, pane: Option<u64>, severity: Option<&str>) -> Value {
        self.try_diagnostics(pane, severity).expect("診断の一覧")
    }

    fn try_diagnostics(
        &mut self,
        pane: Option<u64>,
        severity: Option<&str>,
    ) -> Result<Value, DispatchError> {
        dispatch(
            self,
            Request::LspDiagnostics {
                pane,
                severity: severity.map(str::to_string),
            },
            PaneOrigin::Cli,
        )
    }
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
}

impl PreviewHost for Host {
    fn preview_state(&self, pane: PaneId) -> Option<(String, PreviewModeWire)> {
        (pane == self.preview).then(|| (self.path.display().to_string(), PreviewModeWire::Code))
    }
}

impl SystemHost for Host {
    fn lsp(&self) -> Option<&LspManager> {
        Some(&self.manager)
    }
    fn lsp_documents(&self) -> Vec<LspDocument> {
        if !self.editing {
            return Vec::new();
        }
        vec![LspDocument::new(
            self.preview.as_u64(),
            self.path.display().to_string(),
            self.editing,
            &self.link,
        )]
    }
}

impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}

fn opened(label: &str, diagnostics: &Value) -> (Scratch, Host) {
    let scratch = Scratch::new(label, diagnostics);
    let manager = LspManager::new(config(&scratch));
    let path = scratch.file();
    let mut host = Host::new(manager, &path);
    host.edit(SOURCE, 1);
    (scratch, host)
}

fn wait_published(host: &Host, count: usize) {
    let uri = host.uri();
    wait_until(
        "偽サーバの publish が表へ入る",
        Duration::from_secs(20),
        || {
            host.manager.document_diagnostics(&uri).is_some_and(|d| {
                d.diagnostics.version.is_some() && d.diagnostics.items.len() == count
            })
        },
    );
}

/// 受け入れ条件: 偽サーバが返した配列と `tako lsp diagnostics --severity error` の応答が
/// 固定値と一致する。**境界 2 件**: 省略された重大度はエラーとして入り、1 段軽い警告は入らない
#[test]
fn severity_error_の応答は固定値と一致する() {
    let (_scratch, mut host) = opened("error", &fixture());
    wait_published(&host, 6);
    let pane = host.preview.as_u64();
    let out = host.diagnostics(None, Some("error"));
    assert_eq!(out["enabled"], json!(true));
    assert_eq!(out["severity"], json!("error"));
    assert_eq!(out["total"], json!(3));
    let doc = &out["documents"][0];
    assert_eq!(doc["pane"], json!(pane));
    assert_eq!(doc["server"], json!(servers::SERVERS[0].id));
    assert_eq!(doc["version"], json!(1));
    // 数は絞る前（重大度ごと）
    assert_eq!(
        doc["counts"],
        json!({"error": 3, "warning": 1, "info": 1, "hint": 1})
    );
    // 位置は `tako edit replace-range` と同じ（行 1 始まり・桁 0 始まりの UTF-8 バイト）
    assert_eq!(
        doc["diagnostics"],
        json!([
            {"severity": "error",
             "range": {"start": {"line": 3, "column": 17}, "end": {"line": 3, "column": 20}},
             "message": "mismatched types", "source": "rustc", "code": "E0308"},
            {"severity": "error",
             "range": {"start": {"line": 3, "column": 20}, "end": {"line": 3, "column": 21}},
             "message": "past end"},
            {"severity": "error",
             "range": {"start": {"line": 4, "column": 0}, "end": {"line": 4, "column": 1}},
             "message": "no severity", "code": "7"},
        ])
    );
    // ペインを名指しても同じ中身（全文書の一覧と同じ 1 実装）
    let named = host.diagnostics(Some(pane), Some("error"));
    assert_eq!(named["documents"], out["documents"]);
}

/// 境界の 2 件目: `warning` は警告を含み（同じ重さ）、1 段軽い情報は含まない。
/// 日本語・絵文字の行の桁が UTF-8 バイトへ写っている
#[test]
fn severity_warning_は警告まで含み_utf16_の桁を写す() {
    let (_scratch, mut host) = opened("warning", &fixture());
    wait_published(&host, 6);
    let out = host.diagnostics(None, Some("warning"));
    let got: Vec<(String, Value, Value)> = out["documents"][0]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["severity"].as_str().unwrap().to_string(),
                d["range"]["start"].clone(),
                d["range"]["end"].clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            // `日本` = UTF-16 で 8..10 → UTF-8 で 8..14
            (
                "warning".into(),
                json!({"line": 2, "column": 8}),
                json!({"line": 2, "column": 14})
            ),
            (
                "error".into(),
                json!({"line": 3, "column": 17}),
                json!({"line": 3, "column": 20})
            ),
            (
                "error".into(),
                json!({"line": 3, "column": 20}),
                json!({"line": 3, "column": 21})
            ),
            (
                "error".into(),
                json!({"line": 4, "column": 0}),
                json!({"line": 4, "column": 1})
            ),
        ]
    );
    // 絞らなければ 6 件で、情報の `"😀"`（UTF-16 で 13..17）は UTF-8 で 17..23
    let all = host.diagnostics(None, None);
    assert_eq!(all["total"], json!(6));
    let info = all["documents"][0]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["severity"] == json!("info"))
        .cloned()
        .unwrap();
    assert_eq!(
        info["range"],
        json!({"start": {"line": 2, "column": 17}, "end": {"line": 2, "column": 23}})
    );
    // 返した範囲で本文を切ると、サーバが指した文字列そのものになる
    let line = SOURCE.lines().nth(1).unwrap();
    assert_eq!(&line[17..23], "\"😀\"");
    assert_eq!(&line[8..14], "日本");
}

/// エッジ: 診断 0 件の文書は空の配列と 0 の数を返す（「つながっていない」と区別できる）
#[test]
fn 診断0件の文書は空で返す() {
    let (_scratch, mut host) = opened("empty", &json!([]));
    wait_published(&host, 0);
    let out = host.diagnostics(None, None);
    assert_eq!(out["total"], json!(0));
    let doc = &out["documents"][0];
    assert_eq!(doc["diagnostics"], json!([]));
    assert_eq!(
        doc["counts"],
        json!({"error": 0, "warning": 0, "info": 0, "hint": 0})
    );
    assert!(
        doc.get("reason").is_none(),
        "つながっているので理由は付かない"
    );
}

/// 受け入れ条件（#830 の機序）: 診断を持つ文書を閉じたら、保持件数の口が 0 を返す。
/// サーバを止めたときも古い診断を残さない
#[test]
fn 閉じたら保持は0になり止めても残さない() {
    let (_scratch, mut host) = opened("close", &fixture());
    wait_published(&host, 6);
    assert_eq!(
        host.diagnostics(None, None)["retained"]["diagnostics"],
        json!(6)
    );

    // 止める → 文書は開いたままでも、そのサーバの診断は捨てる
    host.manager.stop(None);
    let uri = host.uri();
    wait_until(
        "stop で診断が消える",
        Duration::from_secs(10),
        || {
            host.manager
                .document_diagnostics(&uri)
                .is_some_and(|d| d.diagnostics.items.is_empty())
        },
    );
    assert_eq!(host.manager.diagnostics_retained(), (0, 0));

    // 起こし直す → 開き直しの didOpen で publish され直す
    host.manager.restart(None);
    wait_published(&host, 6);

    // 閉じる（GUI ではペインを閉じると `EditState` ごと `DocLink` が落ちる）
    host.link = DocLink::default();
    host.editing = false;
    assert_eq!(host.manager.diagnostics_retained(), (0, 0));
    let out = host.diagnostics(None, None);
    assert_eq!(out["retained"], json!({"diagnostics": 0, "documents": 0}));
    assert_eq!(out["documents"], json!([]));
    assert!(out["note"].is_string(), "つながった文書が無い旨を返す");
}

/// ペインの名指し: 編集モードでないプレビューは理由と次の一手を返し、
/// プレビューでないペイン・綴り違いの重大度は弾く
#[test]
fn 名指しと引数の検査() {
    let scratch = Scratch::new("args", &fixture());
    let manager = LspManager::new(config(&scratch));
    let path = scratch.file();
    let mut host = Host::new(manager, &path);
    let pane = host.preview.as_u64();
    let out = host.diagnostics(Some(pane), None);
    let doc = &out["documents"][0];
    assert_eq!(doc["editing"], json!(false));
    assert_eq!(
        doc["reason"],
        json!(tako_control::lsp::text::NOT_EDITING_REASON.text())
    );
    assert!(doc["next_step"]
        .as_str()
        .unwrap()
        .contains(&format!("--pane {pane}")));
    assert!(matches!(
        host.try_diagnostics(Some(pane), Some("warn")),
        Err(DispatchError::InvalidParams(_))
    ));
    host.preview = PaneId::from_raw(9_999);
    let real = host.workspace.active_tab().tree().focused().as_u64();
    assert!(matches!(
        host.try_diagnostics(Some(real), None),
        Err(DispatchError::InvalidParams(_))
    ));
}

/// UI へのキューは上限つき（設計書 §2 / §18）。受け手が読まないあいだに publish が
/// 上限を超えても積み増さず、溢れた印が立つ（UI は印を見て全部を読み直す）
#[test]
fn ui_へのキューは上限で頭打ちになり溢れた印が立つ() {
    let scratch = Scratch::new("queue", &fixture());
    let manager = LspManager::new(config(&scratch));
    let mut events = manager
        .diagnostics_events()
        .expect("有効な manager は口を返す");
    let path = scratch.file();
    let mut host = Host::new(manager, &path);
    host.edit(SOURCE, 1);
    wait_published(&host, 6);
    let first = events.try_recv().expect("知らせが来ている");
    assert_eq!(first, host.uri());
    // 読まずに上限を超える回数だけ publish させる（didChange のたびに偽サーバが publish する）
    let rounds = DIAGNOSTICS_EVENT_CAPACITY as u64 + 40;
    for version in 2..2 + rounds {
        let text = format!("{SOURCE}// {version}\n");
        host.manager
            .sync(&mut host.link, true, &path, &text, version);
    }
    let uri = host.uri();
    let last = (1 + rounds) as i32;
    wait_until(
        "最後の版の publish が表へ入る",
        Duration::from_secs(30),
        || {
            host.manager
                .document_diagnostics(&uri)
                .is_some_and(|d| d.diagnostics.version == Some(last))
        },
    );
    let mut queued = 0;
    while events.try_recv().is_ok() {
        queued += 1;
    }
    assert!(
        queued <= DIAGNOSTICS_EVENT_CAPACITY + 1,
        "キューが上限を超えて積まれた: {queued}"
    );
    assert!(host.manager.take_events_overflow(), "溢れた印が立つ");
    assert!(!host.manager.take_events_overflow(), "読むと倒れる");
}
