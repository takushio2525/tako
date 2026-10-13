//! #1685 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、参照検索・呼び出し階層・
//! 型階層の問い合わせ（`LspManager::xref` / `xref_expand`）が送るもの・受けて直すものを測る。
//!
//! 結果の保持・ツリーの開閉・行を開く着地は GUI / dispatch の側（`dispatch` の単体 = MockHost と
//! visual-test `xref-1685`）が持つ。ここは「答え」と、答えから組んだ一覧の行（`result_json`）まで。
//!
//! 文言は `tako_control::lsp::text` の定数から読んで比べる（理由文を直書きしない）。
//! 上限つきの要求で起動の後の経路を見るときは起動済みを待ってから測る（`common/lsp_fake_e2e.rs`。#1922）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    text, xref, DocLink, ExpandRequest, GotoError, Launch, LspConfig, LspManager, XrefAnswer,
    XrefRequest,
};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::lsp::xref::{Body, XrefKind, XrefResult, MAX_REFERENCES};
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::PaneId;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1685-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        // 部品ごとに join する（`join("src/main.rs")` は Windows で `…\src/main.rs` と区切りが混ざり、
        // 製品が返す OS の区切りのパスと文字列で比べると食い違う。CI の Windows で実際に落ちた）
        let path = rel.split('/').fold(self.0.clone(), |p, part| p.join(part));
        std::fs::write(&path, body).unwrap();
        path
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `xref` = `--xref` の規則（`None` なら渡さない = 参照・階層の能力を申告しないサーバ）
fn config(scratch: &Scratch, xref: Option<&Value>) -> LspConfig {
    let mut args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
    ];
    if let Some(rules) = xref {
        let file = scratch.0.join("xref.json");
        std::fs::write(&file, rules.to_string()).unwrap();
        args.push("--xref".into());
        args.push(file.display().to_string());
    }
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

fn received(scratch: &Scratch) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn methods(scratch: &Scratch) -> Vec<String> {
    received(scratch)
        .iter()
        .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn request(kind: XrefKind, path: &Path, line: usize, column: usize, secs: u64) -> XrefRequest {
    XrefRequest {
        kind,
        path: path.to_path_buf(),
        line,
        column,
        timeout: Duration::from_secs(secs),
        document: None,
    }
}

fn location(path: &Path, line: u64, character: u64) -> Value {
    json!({
        "uri": tako_core::file_uri::from_path(path),
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 6 },
        },
    })
}

fn hierarchy_item(name: &str, path: &Path, line: u64, character: u64) -> Value {
    json!({
        "name": name,
        "kind": 12,
        "detail": format!("fn {name}()"),
        "uri": tako_core::file_uri::from_path(path),
        "range": {
            "start": { "line": line, "character": 0 },
            "end": { "line": line + 1, "character": 1 },
        },
        "selectionRange": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + name.len() as u64 },
        },
        "data": { "opaque": name },
    })
}

/// 答えを問い合わせたペインの結果にして応答の形へ（dispatch の着地と同じ組み立て）
fn result_of(answer: XrefAnswer, kind: XrefKind, path: &Path) -> XrefResult {
    let server = answer.server().to_string();
    let (body, total) = answer.into_body();
    XrefResult {
        pane: PaneId::from_raw(4),
        path: path.to_path_buf(),
        line: 2,
        column: 7,
        symbol: "helper".into(),
        kind,
        server,
        body,
        total,
        seq: 1,
    }
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path, text: &str) -> DocLink {
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, text, 1);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

/// 受け入れ条件: 偽サーバの `Location` 配列に対する一覧の行（パス / 行 / 抜粋）が固定値と一致する。
/// 並びは（ファイル, 行, 桁）・重複は畳む・桁は UTF-16 から直す・宣言も含めて問う
#[test]
fn 参照の_location_配列が一覧の行の固定値になる() {
    let scratch = Scratch::new("refs");
    let main = scratch.write(
        "src/main.rs",
        "mod util;\nfn main() {\n    util::helper();\n}\n",
    );
    // 2 行目の絵文字（UTF-16 で 2・UTF-8 で 4）の後ろの参照 = 桁の変換を見る
    let util = scratch.write(
        "src/util.rs",
        "pub fn helper() {}\n\nfn other() { /* 😀 */ helper(); }\n",
    );
    let rules = json!([{ "method": "textDocument/references", "result": [
        location(&util, 2, 22),
        location(&main, 2, 10),
        location(&util, 0, 7),
        location(&main, 2, 10),
    ] }]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let answer = manager
        .xref(&request(XrefKind::References, &main, 2, 10, 10))
        .expect("答えが来る");
    let result = result_of(answer, XrefKind::References, &main);
    let out = xref::result_json(&result, None);
    let (m, u) = (main.display().to_string(), util.display().to_string());
    assert_eq!(
        out["items"],
        json!([
            { "id": 1, "path": m, "line": 3, "column": 10, "text": "util::helper();" },
            { "id": 2, "path": u, "line": 1, "column": 7, "text": "pub fn helper() {}" },
            // `fn other() { /* 😀 */ ` = UTF-16 で 22 → バイトは 24
            { "id": 3, "path": u, "line": 3, "column": 24, "text": "fn other() { /* 😀 */ helper(); }" },
        ])
    );
    assert_eq!(
        (out["total"].clone(), out["truncated"].clone()),
        (json!(3), json!(0))
    );
    // 送ったもの: 一時的な didOpen → references（宣言を含める・UTF-16 の位置）→ didClose
    lsp_fake_e2e::stop_and_settle(&manager, &scratch.log());
    let seen = methods(&scratch);
    let at = seen
        .iter()
        .position(|m| m == "textDocument/references")
        .expect("references を送った");
    assert_eq!(seen[at - 1], "textDocument/didOpen");
    assert_eq!(seen[at + 1], "textDocument/didClose");
    let asked = &received(&scratch)[at]["params"];
    assert_eq!(asked["position"], json!({ "line": 2, "character": 10 }));
    assert_eq!(asked["context"], json!({ "includeDeclaration": true }));
}

/// 巨大な結果は保持の上限で切り、切る前の件数を `total` に残す
#[test]
fn 巨大な結果は上限で切り全件数を残す() {
    let scratch = Scratch::new("huge");
    let body: String = (0..3000).map(|i| format!("// line {i}\n")).collect();
    let main = scratch.write("src/main.rs", &body);
    let rules = json!([{ "method": "textDocument/references", "generate": 3000 }]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let answer = manager
        .xref(&request(XrefKind::References, &main, 0, 3, 20))
        .unwrap();
    let result = result_of(answer, XrefKind::References, &main);
    assert_eq!(result.body.len(), MAX_REFERENCES);
    assert_eq!(result.total, 3000);
    let out = xref::result_json(&result, None);
    assert_eq!(out["shown"], json!(xref::DEFAULT_LIMIT));
    assert_eq!(out["truncated"], json!(3000 - xref::DEFAULT_LIMIT));
    assert_eq!(out["items"][0]["text"], json!("// line 0"));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 0 件は失敗ではなく答え（`status: none` と理由・次の一手）
#[test]
fn 参照_0_件は理由つきの答え() {
    let scratch = Scratch::new("none");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let rules = json!([{ "method": "textDocument/references", "result": [] }]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let answer = manager
        .xref(&request(XrefKind::References, &main, 0, 3, 10))
        .unwrap();
    assert!(answer.is_empty());
    let out = xref::result_json(&result_of(answer, XrefKind::References, &main), None);
    assert_eq!(out["status"], json!("none"));
    assert_eq!(
        out["reason"],
        json!(text::fill(
            text::XREF_NONE_REASON,
            &[("kind", xref::kind_label(XrefKind::References))]
        ))
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 能力を申告しないサーバには送らない（`unsupported`）。理由は種類ごとの呼び名
#[test]
fn 能力の無いサーバは未対応と答えて送らない() {
    let scratch = Scratch::new("unsupported");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(config(&scratch, None));
    for kind in XrefKind::ALL {
        let error = manager
            .xref(&request(kind, &main, 0, 3, 10))
            .expect_err("能力が無い");
        assert!(
            matches!(error, GotoError::Unsupported { .. }),
            "{kind:?}: {error:?}"
        );
        let out = xref::error_json(&error, kind);
        assert_eq!(out["status"], json!("unsupported"));
        assert!(
            out["reason"]
                .as_str()
                .is_some_and(|r| r.contains(xref::kind_label(kind))),
            "{out}"
        );
    }
    lsp_fake_e2e::stop_and_settle(&manager, &scratch.log());
    let seen = methods(&scratch);
    assert!(
        !seen
            .iter()
            .any(|m| m.contains("ierarchy") || m.ends_with("references")),
        "{seen:?}"
    );
}

/// 未導入（理由と導入コマンド）
#[test]
fn 未導入は導入コマンドつきで答える() {
    let scratch = Scratch::new("missing");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let manager = LspManager::new(LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(|spec: &ServerSpec| Launch::NotFound {
            program: spec.program.to_string(),
            override_env: None,
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy::default(),
        raw_log_dir: None,
    });
    let error = manager
        .xref(&request(XrefKind::References, &main, 0, 3, 10))
        .unwrap_err();
    let out = xref::error_json(&error, XrefKind::References);
    assert_eq!(out["status"], json!("not-installed"));
    assert!(out["install_command"]
        .as_str()
        .is_some_and(|c| !c.is_empty()));
}

/// 問い合わせの途中で落ちた / 上限まで答えない（起動の後の経路 = 起動済みを待ってから測る）
#[test]
fn 落ちたと未応答を区別する() {
    let scratch = Scratch::new("down");
    let main = scratch.write("src/main.rs", "fn main() {}\nfn quiet() {}\n");
    let rules = json!([
        { "method": "textDocument/references", "line": 0, "crash": true },
        { "method": "textDocument/references", "line": 1, "silent": true },
    ]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let link = open_editing(&manager, &main, "fn main() {}\nfn quiet() {}\n");
    lsp_fake_e2e::wait_running(&manager);
    let silent = manager
        .xref(&request(XrefKind::References, &main, 1, 3, 1))
        .unwrap_err();
    assert!(
        matches!(
            silent,
            GotoError::Timeout {
                secs: 1,
                starting: false,
                ..
            }
        ),
        "{silent:?}"
    );
    let crashed = manager
        .xref(&request(XrefKind::References, &main, 0, 3, 10))
        .unwrap_err();
    assert!(matches!(crashed, GotoError::Crashed { .. }), "{crashed:?}");
    assert_ne!(
        xref::error_json(&silent, XrefKind::References)["reason"],
        xref::error_json(&crashed, XrefKind::References)["reason"]
    );
    drop(link);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 呼び出し元: prepare の根を 1 段展開して返す。展開は受けた要素（`data` を含む）をそのまま送り返し、
/// 呼び出し元のノードは呼び出しの位置（`fromRanges` の先頭）へ着地する。2 段目は `xref_expand`
#[test]
fn 呼び出し元は根を_1_段開き展開は要素を送り返す() {
    let scratch = Scratch::new("calls");
    let lib = scratch.write(
        "src/lib.rs",
        "pub fn helper() {}\n\nfn caller() {\n    helper();\n}\n\nfn top() { caller(); }\n",
    );
    let root = hierarchy_item("helper", &lib, 0, 7);
    let caller = hierarchy_item("caller", &lib, 2, 3);
    let top = hierarchy_item("top", &lib, 6, 3);
    let site = |line: u64, character: u64| json!([{ "start": { "line": line, "character": character }, "end": { "line": line, "character": character + 6 } }]);
    let rules = json!([
        { "method": "textDocument/prepareCallHierarchy", "result": [root] },
        { "method": "callHierarchy/incomingCalls", "item_name": "helper",
          "result": [{ "from": caller, "fromRanges": site(3, 4) }] },
        { "method": "callHierarchy/incomingCalls", "item_name": "caller",
          "result": [{ "from": top, "fromRanges": site(6, 11) }] },
    ]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let answer = manager
        .xref(&request(XrefKind::IncomingCalls, &lib, 0, 8, 10))
        .unwrap();
    let mut result = result_of(answer, XrefKind::IncomingCalls, &lib);
    let out = xref::result_json(&result, None);
    assert_eq!(out["expanded"], json!([1]));
    let nodes = out["nodes"].as_array().unwrap();
    assert_eq!(
        nodes
            .iter()
            .map(|n| (
                n["name"].as_str().unwrap(),
                n["line"].as_u64().unwrap(),
                n["column"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        // 根は定義（選択範囲）・呼び出し元は呼び出しの位置（4 行目の 4 桁目 = `helper();`）
        vec![("helper", 1, 7), ("caller", 4, 4)]
    );
    assert_eq!(nodes[1]["text"], json!("helper();"));
    assert_eq!(
        nodes[1]["children"],
        Value::Null,
        "2 段目はまだ問い合わせない"
    );
    // 2 段目: 受けた要素をそのまま送り返す（ツリーへ足すのは dispatch の着地）
    let Body::Tree(tree) = &result.body else {
        panic!("ツリー");
    };
    let raw = tree.node(2).unwrap().raw.clone();
    let expanded = manager
        .xref_expand(&ExpandRequest {
            kind: XrefKind::IncomingCalls,
            path: lib.clone(),
            item: raw,
            timeout: Duration::from_secs(10),
        })
        .unwrap();
    assert_eq!(expanded.children.len(), 1);
    assert_eq!(expanded.children[0].name, "top");
    assert_eq!(
        (
            expanded.children[0].place.line,
            expanded.children[0].place.column
        ),
        (6, 11)
    );
    let Body::Tree(tree) = &mut result.body else {
        panic!("ツリー");
    };
    tree.set_children(2, expanded.children).unwrap();
    assert_eq!(tree.expanded(), vec![1, 2]);
    // 送ったもの: prepare → incomingCalls（helper）→ … → incomingCalls（caller・data もそのまま）
    lsp_fake_e2e::stop_and_settle(&manager, &scratch.log());
    let calls: Vec<Value> = received(&scratch)
        .into_iter()
        .filter(|m| {
            m["method"]
                .as_str()
                .is_some_and(|m| m.contains("allHierarchy"))
        })
        .collect();
    let names: Vec<(&str, &str)> = calls
        .iter()
        .map(|m| {
            (
                m["method"].as_str().unwrap(),
                m["params"]["item"]["name"].as_str().unwrap_or("-"),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("textDocument/prepareCallHierarchy", "-"),
            ("callHierarchy/incomingCalls", "helper"),
            ("callHierarchy/incomingCalls", "caller"),
        ]
    );
    assert_eq!(
        calls[2]["params"]["item"]["data"],
        json!({ "opaque": "caller" })
    );
}

/// 呼び出し先は呼ばれる側の定義へ着地し、型階層は prepareTypeHierarchy → supertypes / subtypes
#[test]
fn 呼び出し先と型階層() {
    let scratch = Scratch::new("types");
    let lib = scratch.write(
        "src/lib.rs",
        "trait Base {}\nstruct Leaf;\nimpl Base for Leaf {}\nfn run() { leaf(); }\nfn leaf() {}\n",
    );
    let rules = json!([
        { "method": "textDocument/prepareCallHierarchy", "result": [hierarchy_item("run", &lib, 3, 3)] },
        { "method": "callHierarchy/outgoingCalls", "result": [{
            "to": hierarchy_item("leaf", &lib, 4, 3),
            "fromRanges": [{ "start": { "line": 3, "character": 11 }, "end": { "line": 3, "character": 15 } }],
        }] },
        { "method": "textDocument/prepareTypeHierarchy", "result": [hierarchy_item("Leaf", &lib, 1, 7)] },
        { "method": "typeHierarchy/supertypes", "result": [hierarchy_item("Base", &lib, 0, 6)] },
        { "method": "typeHierarchy/subtypes", "result": [] },
    ]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let leaf_of = |answer: XrefAnswer| -> Vec<(String, usize)> {
        let XrefAnswer::Tree { roots, .. } = answer else {
            panic!("ツリー");
        };
        roots
            .into_iter()
            .flat_map(|(_, kids)| kids.unwrap_or_default())
            .map(|k| (k.name, k.place.line))
            .collect()
    };
    let outgoing = manager
        .xref(&request(XrefKind::OutgoingCalls, &lib, 3, 4, 10))
        .unwrap();
    assert_eq!(leaf_of(outgoing), vec![("leaf".to_string(), 4)]);
    let supers = manager
        .xref(&request(XrefKind::Supertypes, &lib, 1, 8, 10))
        .unwrap();
    assert_eq!(leaf_of(supers), vec![("Base".to_string(), 0)]);
    // サブタイプ 0 件でも根は返る（根を開いた 0 件の子）
    let subs = manager
        .xref(&request(XrefKind::Subtypes, &lib, 1, 8, 10))
        .unwrap();
    let result = result_of(subs, XrefKind::Subtypes, &lib);
    let out = xref::result_json(&result, None);
    assert_eq!(out["nodes"][0]["name"], json!("Leaf"));
    assert_eq!(out["nodes"][0]["children"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));
}

/// prepare が 0 件 = 階層の対象でない位置（空の答え。ツリーは空で `status: none`）
#[test]
fn 階層の対象でない位置は空の答え() {
    let scratch = Scratch::new("noprep");
    let main = scratch.write("src/main.rs", "fn main() {}\n");
    let rules = json!([{ "method": "textDocument/prepareCallHierarchy", "result": null }]);
    let manager = LspManager::new(config(&scratch, Some(&rules)));
    let answer = manager
        .xref(&request(XrefKind::IncomingCalls, &main, 0, 3, 10))
        .unwrap();
    assert!(answer.is_empty());
    let out = xref::result_json(&result_of(answer, XrefKind::IncomingCalls, &main), None);
    assert_eq!(out["status"], json!("none"));
    assert_eq!(out["nodes"], json!([]));
    manager.shutdown_all(Duration::from_secs(2));
}
