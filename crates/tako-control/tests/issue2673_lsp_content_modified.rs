//! #2673 の e2e: 言語サーバが `-32801`（ContentModified = 変更で古くなった答え）で答えた要求を、
//! manager が最終の失敗にしない。定義ジャンプ・ホバー・参照と階層・名前の変更・整形は問い直し、
//! 補完は問い直さずに 0 件（未完）で返す。
//!
//! ## 真因（実測。rust-analyzer 1.95.0・新しいプロジェクトを開いた直後）
//!
//! rust-analyzer は `Cargo.lock` ができたことの知らせなどでワークスペースを読み込み直し、その切り替え
//! （`switch_workspaces`）の最中に受けた要求を `-32801 content modified` で取り消す。tako は
//! `rpc_failure` でエラー応答のコードを見ずに一律 `server-error` として即座に諦めていた
//! （#1930 の「読み込み中は待って問い直す」は空の答えにしか効かない）。GUI の右クリックメニューの
//! 「定義へ移動」が握手の 1.5 秒後に押されると 2/2 で飛ばず、同じ順序の CLI も `server-error` だった。
//!
//! ## ここで縛ること
//!
//! 偽サーバ（`tako-lsp-fake --content-modified`）で同じ形を演じ、6 種それぞれの 1 回の問い合わせを
//! 「答えの要約 / サーバが受けた回数」の 1 行にして、修正後（既定）と A/B の旧腕（`TAKO_2673_LEGACY=1`）で
//! 突き合わせる（[`CASES`]）。読み込み中の `-32801` は読み込みの終わりを**テストが決める**
//! （`LoadingGate`。要求が届いたのを偽サーバのログで確かめてから合図する = 終わるまで manager は
//! 問い直さない）。読み込み中でない `-32801` は待たずに 1 回だけ問い直す（2 回目は受けたエラーのまま）。
//! 回数の上限（`MAX_CONTENT_MODIFIED_RETRIES`）は単体（`lsp::goto` の `ContentModified`）が測る。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    CompletionError, CompletionRequest, DocLink, ExpandRequest, FormatError, FormatRequest,
    GotoError, GotoRequest, HoverError, HoverRequest, Launch, LspConfig, LspManager, RenameError,
    RenameRequest, XrefAnswer, XrefRequest,
};
use tako_core::lsp::completion::Trigger;
use tako_core::lsp::format::FormatOptions;
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::lsp::xref::XrefKind;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::LoadingGate;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 行末の空白は整形（偽サーバの既定 = 行末の空白を消す）の答えを 1 件にするため
const TEXT: &str = "fn main() { ab }  \n";

/// 偽サーバが `-32801` で答える時機
#[derive(Clone, Copy)]
enum Modified {
    /// `loading` の読み込み中に受けた要求すべて（rust-analyzer の読み込み直しの形）
    WhileLoading,
    /// 読み込み中でないサーバの、最初の N 本（打鍵の直後などの形）
    First(u32),
}

/// 1 件の問い合わせの舞台（使い捨ての置き場・開いた文書・偽サーバ）
struct Stage {
    scratch: ScratchDir,
    main: PathBuf,
    other: PathBuf,
}

impl Stage {
    fn new(tag: &str) -> Self {
        let scratch = ScratchDir::new(&format!("tako-2673-{tag}"));
        let root = scratch.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let main = root.join("src/main.rs");
        std::fs::write(&main, TEXT).unwrap();
        let other = root.join("src/other.rs");
        std::fs::write(&other, "pub fn ab() {}\n").unwrap();
        Self {
            scratch,
            main,
            other,
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.scratch.path().join(name)
    }

    fn log(&self) -> PathBuf {
        self.file("received.jsonl")
    }

    fn gate(&self) -> LoadingGate {
        LoadingGate::new(self.scratch.path())
    }

    /// `other.rs` の `ab` を指す呼び出し階層の要素（展開の要求にそのまま渡す）
    fn item(&self, name: &str) -> Value {
        json!({
            "name": name,
            "kind": 12,
            "uri": tako_core::file_uri::from_path(&self.other),
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 14 } },
            "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 9 } },
        })
    }

    /// 6 種とも、答えるときは見つかる規則を渡した偽サーバ（`providers` を渡せば能力の組を差し替える）
    fn manager(&self, modified: Modified, providers: Option<Value>) -> LspManager {
        // 初回 exec の検査を時計の外で済ませる（#2228）
        tako_core::fake_exec::warm_binary(FAKE);
        let args = fake_args(self, modified, providers);
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
            idle_grace: DEFAULT_IDLE_GRACE,
            restart: RestartPolicy {
                max_restarts: 3,
                base_delay_ms: 50,
            },
            raw_log_dir: None,
        })
    }

    /// 偽サーバを起こし、`main.rs` を編集モードで開いて（GUI の編集セッションと同じ）、`modified` の
    /// 時機で `query` を 1 回問い合わせる。返すのは「答えの要約 / `method` を受けた回数」
    fn ask(
        &self,
        modified: Modified,
        providers: Option<Value>,
        method: &str,
        query: &(dyn Fn(&LspManager, &Stage) -> String + Sync),
    ) -> String {
        let manager = self.manager(modified, providers);
        let mut link = DocLink::default();
        manager.sync(&mut link, true, &self.main, TEXT, 1);
        assert!(matches!(link, DocLink::Open(_)), "文書が開く");
        let summary = match modified {
            Modified::WhileLoading => {
                lsp_fake_e2e::wait_loading_known(&manager);
                std::thread::scope(|scope| {
                    let asked = scope.spawn(|| query(&manager, self));
                    // 要求が読み込み中に届いた（= `-32801` で答えた）のを確かめてから読み込みを終える。
                    // 終わるまでは manager は問い直さない（読み込み中の `-32801` は済むのを待つ）
                    lsp_fake_e2e::wait_received(&self.log(), method, 1);
                    self.gate().finish();
                    asked.join().expect("問い合わせのスレッド")
                })
            }
            Modified::First(_) => {
                lsp_fake_e2e::wait_settled(&manager);
                query(&manager, self)
            }
        };
        // 数える前に区切る（止めて shutdown が届いた = それより前に送った要求はログに載っている）
        lsp_fake_e2e::stop_and_settle(&manager, &self.log());
        drop(link);
        format!(
            "{summary} / asked {}",
            lsp_fake_e2e::received(&self.log(), method)
        )
    }
}

/// 偽サーバの引数（規則のファイルは `stage` の置き場へ書く）。`loading` シナリオは合図（`LoadingGate`）で
/// 終わらせる = 読み込みの終わりを実時間で決めない（#1922 の番犬が 0 桁目の関数の窓で見る）
fn fake_args(stage: &Stage, modified: Modified, providers: Option<Value>) -> Vec<String> {
    let location = json!({
        "uri": tako_core::file_uri::from_path(&stage.other),
        "range": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 9 } },
    });
    let rules = [
        ("--goto", json!([{ "result": location }])),
        (
            "--hover",
            json!([{ "result": { "contents": { "kind": "markdown", "value": "**ab**" } } }]),
        ),
        (
            "--completion",
            json!({ "items": [{ "label": "abc" }, { "label": "abd" }] }),
        ),
        (
            "--xref",
            json!([
                { "method": "textDocument/references", "result": [location] },
                { "method": "textDocument/prepareCallHierarchy", "result": [stage.item("ab")] },
                { "method": "callHierarchy/incomingCalls",
                  "result": [{ "from": stage.item("caller"), "fromRanges": [location["range"]] }] },
            ]),
        ),
        ("--rename", json!([])),
    ];
    let mut args = vec!["--log".to_string(), stage.log().display().to_string()];
    for (flag, value) in rules {
        let path = stage.file(&format!("{}.json", flag.trim_start_matches('-')));
        std::fs::write(&path, value.to_string()).unwrap();
        args.push(flag.to_string());
        args.push(path.display().to_string());
    }
    match modified {
        Modified::WhileLoading => {
            args.extend(["--scenario".to_string(), "loading".to_string()]);
            args.extend(stage.gate().args());
            args.extend(lsp_fake_e2e::status_delay_args());
            args.extend(["--content-modified".to_string(), "loading".to_string()]);
        }
        Modified::First(n) => {
            args.extend(["--scenario".to_string(), "normal".to_string()]);
            args.extend(["--content-modified".to_string(), n.to_string()]);
        }
    }
    if let Some(providers) = providers {
        args.extend(["--providers".to_string(), providers.to_string()]);
    }
    args
}

/// 失敗の要約（`-32801` はコードまで）
fn failure(error: &GotoError) -> String {
    match error {
        GotoError::ServerError { code, .. } => format!("server-error {code}"),
        other => other.status().to_string(),
    }
}

fn timeout() -> Duration {
    Duration::from_secs(60)
}

fn goto(manager: &LspManager, stage: &Stage) -> String {
    match manager.goto(&GotoRequest {
        kind: GotoKind::Definition,
        path: stage.main.clone(),
        line: 0,
        column: 12,
        timeout: timeout(),
        document: None,
    }) {
        Ok(answer) => format!("found {}", answer.targets.len()),
        Err(e) => failure(&e),
    }
}

fn hover(manager: &LspManager, stage: &Stage) -> String {
    match manager.hover(&HoverRequest {
        path: stage.main.clone(),
        line: 0,
        column: 13,
        timeout: timeout(),
        document: None,
        superseding: false,
        open: true,
        ticket: None,
    }) {
        Ok(answer) => match answer.content {
            Some(content) => format!("hover {}", content.value),
            None => "hover none".to_string(),
        },
        Err(HoverError::Query(e)) => failure(&e),
        Err(other) => format!("{other:?}"),
    }
}

fn xref(manager: &LspManager, stage: &Stage, kind: XrefKind) -> String {
    match manager.xref(&XrefRequest {
        kind,
        path: stage.main.clone(),
        line: 0,
        column: 12,
        timeout: timeout(),
        document: None,
    }) {
        Ok(XrefAnswer::List { items, .. }) => format!("list {}", items.len()),
        Ok(XrefAnswer::Tree { roots, .. }) => format!("tree {}", roots.len()),
        Err(e) => failure(&e),
    }
}

fn expand(manager: &LspManager, stage: &Stage) -> String {
    match manager.xref_expand(&ExpandRequest {
        kind: XrefKind::IncomingCalls,
        path: stage.main.clone(),
        item: stage.item("ab"),
        timeout: timeout(),
    }) {
        Ok(answer) => format!("children {}", answer.children.len()),
        Err(e) => failure(&e),
    }
}

fn rename(manager: &LspManager, stage: &Stage) -> String {
    match manager.rename(&RenameRequest {
        path: stage.main.clone(),
        line: 0,
        column: 12,
        new_name: "cd".to_string(),
        document: Some(TEXT.to_string()),
        timeout: timeout(),
    }) {
        Ok(answer) => format!("edits {}", answer.source_changes.len()),
        Err(RenameError::Lsp(e)) => failure(&e),
        Err(RenameError::Rejected { .. }) => "rejected".to_string(),
        Err(RenameError::Refused { .. }) => "refused".to_string(),
        Err(other) => format!("{other:?}"),
    }
}

fn format(manager: &LspManager, stage: &Stage) -> String {
    match manager.format(&FormatRequest {
        path: stage.main.clone(),
        text: TEXT.to_string(),
        range: None,
        options: FormatOptions {
            tab_size: 4,
            insert_spaces: true,
        },
        timeout: timeout(),
    }) {
        Ok(answer) => format!("changes {}", answer.changes.len()),
        Err(FormatError::Lsp(e)) => failure(&e),
        Err(other) => format!("{other:?}"),
    }
}

fn completion(manager: &LspManager, stage: &Stage, superseding: bool) -> String {
    match manager.completion(&CompletionRequest {
        path: stage.main.clone(),
        line: 0,
        column: 14,
        timeout: timeout(),
        document: None,
        trigger: Trigger::Word,
        superseding,
        ticket: None,
        resolve_top: 0,
    }) {
        Ok(answer) => format!(
            "items {} incomplete={}",
            answer.items.len(),
            answer.is_incomplete
        ),
        Err(CompletionError::Query(e)) => failure(&e),
        Err(other) => format!("{other:?}"),
    }
}

/// 名前の変更の能力だけ（`prepareProvider` 無し = 本体の `rename` が最初の要求になる）
fn rename_without_prepare() -> Option<Value> {
    Some(json!({ "renameProvider": true }))
}

/// 1 件: 名前・偽サーバの時機・能力の差し替え・数える method・問い合わせ・修正後・旧腕（#2673 の前）
struct Case {
    name: &'static str,
    modified: Modified,
    providers: fn() -> Option<Value>,
    method: &'static str,
    ask: fn(&LspManager, &Stage) -> String,
    fixed: &'static str,
    legacy: &'static str,
}

const CASES: &[Case] = &[
    // 定義ジャンプ: 読み込み中の -32801 は済むのを待って問い直す（実の rust-analyzer の ④ の形）
    Case {
        name: "goto-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/definition",
        ask: goto,
        fixed: "found 1 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    // 読み込み中でない -32801 は待たずに 1 回だけ問い直す
    Case {
        name: "goto-idle-once",
        modified: Modified::First(1),
        providers: || None,
        method: "textDocument/definition",
        ask: goto,
        fixed: "found 1 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    // 2 回続けば 2 回目は受けたエラーのまま（すぐの問い直しは 1 回だけ）
    Case {
        name: "goto-idle-twice",
        modified: Modified::First(2),
        providers: || None,
        method: "textDocument/definition",
        ask: goto,
        fixed: "server-error -32801 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    Case {
        name: "hover-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/hover",
        ask: hover,
        fixed: "hover **ab** / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    Case {
        name: "references-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/references",
        ask: |m, s| xref(m, s, XrefKind::References),
        fixed: "list 1 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    // 階層は prepare で受けても同じ（根を問い直して 1 段開く）
    Case {
        name: "incoming-calls-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/prepareCallHierarchy",
        ask: |m, s| xref(m, s, XrefKind::IncomingCalls),
        fixed: "tree 1 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    // ツリーの 2 段目の展開（`xref_expand`）も同じ判定
    Case {
        name: "expand-idle-once",
        modified: Modified::First(1),
        providers: || None,
        method: "callHierarchy/incomingCalls",
        ask: expand,
        fixed: "children 1 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    // 名前の変更の本体（prepare を申告しないサーバ）: 旧腕は断り（`Refused`）で返していた
    Case {
        name: "rename-loading",
        modified: Modified::WhileLoading,
        providers: rename_without_prepare,
        method: "textDocument/rename",
        ask: rename,
        fixed: "edits 1 / asked 2",
        legacy: "refused / asked 1",
    },
    // 名前の変更の prepare（読み込み中でない -32801）: 旧腕は「名前を変えられない位置」で返していた
    Case {
        name: "rename-prepare-idle-once",
        modified: Modified::First(1),
        providers: || None,
        method: "textDocument/prepareRename",
        ask: rename,
        fixed: "edits 1 / asked 2",
        legacy: "rejected / asked 1",
    },
    Case {
        name: "format-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/formatting",
        ask: format,
        fixed: "changes 1 / asked 2",
        legacy: "server-error -32801 / asked 1",
    },
    // 補完は問い直さず、エラーにもせず未完の 0 件（次の打鍵で置き換わる）
    Case {
        name: "completion-cli-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/completion",
        ask: |m, s| completion(m, s, false),
        fixed: "items 0 incomplete=true / asked 1",
        legacy: "server-error -32801 / asked 1",
    },
    Case {
        name: "completion-typing-loading",
        modified: Modified::WhileLoading,
        providers: || None,
        method: "textDocument/completion",
        ask: |m, s| completion(m, s, true),
        fixed: "items 0 incomplete=true / asked 1",
        legacy: "server-error -32801 / asked 1",
    },
];

fn case(name: &str) -> &'static Case {
    CASES
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("{name} が CASES に無い"))
}

/// 1 件を走らせて要約を返す
fn run(case: &Case) -> String {
    let stage = Stage::new(case.name);
    stage.ask(case.modified, (case.providers)(), case.method, &case.ask)
}

#[track_caller]
fn assert_fixed(name: &str) {
    let case = case(name);
    let got = run(case);
    println!("TAKO_2673_CASE fixed {name}: {got}");
    assert_eq!(got, case.fixed, "{name}（修正後）");
}

#[test]
fn 定義ジャンプ_読み込み中の_content_modified_は済むのを待って問い直して見つける() {
    assert_fixed("goto-loading");
}

#[test]
fn 定義ジャンプ_読み込み中でない_content_modified_はすぐ_1_回だけ問い直す() {
    assert_fixed("goto-idle-once");
    assert_fixed("goto-idle-twice");
}

#[test]
fn ホバー_読み込み中の_content_modified_は済むのを待って問い直す() {
    assert_fixed("hover-loading");
}

#[test]
fn 参照と階層_読み込み中の_content_modified_は済むのを待って問い直す() {
    assert_fixed("references-loading");
    assert_fixed("incoming-calls-loading");
    assert_fixed("expand-idle-once");
}

#[test]
fn 名前の変更_content_modified_は問い直した答えを返す() {
    assert_fixed("rename-loading");
    assert_fixed("rename-prepare-idle-once");
}

#[test]
fn 整形_読み込み中の_content_modified_は済むのを待って問い直す() {
    assert_fixed("format-loading");
}

#[test]
fn 補完_content_modified_は問い直さずエラーにもせず未完の_0_件で返す() {
    assert_fixed("completion-cli-loading");
    assert_fixed("completion-typing-loading");
}

/// 表の全件が上の `#[test]` のどれかに載っている（足した件を測り忘れない）
#[test]
fn 表の全件を測っている() {
    // `file!()` はワークスペースの根からの相対（テストは crate の dir で走る）なので crate から引く
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("issue2673_lsp_content_modified.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()));
    for case in CASES {
        let needle = format!("assert_fixed(\"{}\")", case.name);
        assert!(
            src.contains(&needle),
            "{} を修正後の検査で測っていない（{needle} が無い）",
            case.name
        );
    }
}

/// A/B の入口が効いている: `TAKO_2673_LEGACY=1` では 6 種とも #2673 の前の形（`-32801` を最終の失敗・
/// 断りとして 1 回で返す）に戻る = この e2e が検出力を持つ
#[test]
fn 旧挙動は_content_modified_を最終の失敗として返す() {
    // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(旧挙動は_content_modified_を最終の失敗として返す) {
        return;
    }
    std::env::set_var("TAKO_2673_LEGACY", "1");
    let got: Vec<(&str, String)> = CASES.iter().map(|c| (c.name, run(c))).collect();
    for (name, summary) in &got {
        println!("TAKO_2673_CASE legacy {name}: {summary}");
    }
    let want: Vec<(&str, String)> = CASES
        .iter()
        .map(|c| (c.name, c.legacy.to_string()))
        .collect();
    assert_eq!(got, want);
}
