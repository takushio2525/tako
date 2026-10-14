//! #1686 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、名前の変更の問い合わせ
//! （`LspManager::rename`）が送るもの・受けて直すもの・段階 1 の当てる範囲を測る。
//!
//! 当てるのは dispatch が `TextBuffer::apply_changes` で行うので、ここでは同じ 1 実装へ答えを渡して
//! 本文を固定値と比べる（undo 1 回で前へ戻ることも）。dry-run か・版が変わっていないか・別のペインは
//! dispatch の単体（MockHost）が持つ。
//!
//! 文言は `tako_control::lsp::text` の定数から読んで比べる（理由文を直書きしない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    text, Launch, LspConfig, LspManager, RenameAnswer, RenameError, RenameRequest,
};
use tako_core::lsp::rename::Blocker;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::text_edit::TextBuffer;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）。
/// プロジェクトは `proj/` の下に置き、偽サーバのログと規則はその外に置く（`--disk-model` が
/// 根の配下を読むので、ログの中の語を書き換え先として数えない）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1686-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj/src")).unwrap();
        std::fs::write(dir.join("proj/Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.0.join("proj").join(rel);
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

/// 偽サーバの設定。`rules` が `None` なら `--rename` を渡さない（能力を申告しない）
fn config(scratch: &Scratch, rules: Option<&Value>, extra: &[&str]) -> LspConfig {
    let mut args = vec!["--log".to_string(), scratch.log().display().to_string()];
    if let Some(rules) = rules {
        let file = scratch.0.join("rename.json");
        std::fs::write(&file, rules.to_string()).unwrap();
        args.push("--rename".into());
        args.push(file.display().to_string());
    }
    args.extend(extra.iter().map(|s| s.to_string()));
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

fn methods(scratch: &Scratch) -> Vec<String> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// `line`（0 起点）行目の `needle` の先頭で頼む
fn request(path: &Path, text: &str, needle: &str, nth: usize, new_name: &str) -> RenameRequest {
    let (line, column) = text
        .lines()
        .enumerate()
        .flat_map(|(i, l)| l.match_indices(needle).map(move |(c, _)| (i, c)))
        .nth(nth)
        .unwrap_or_else(|| panic!("{needle} の {nth} 番目が本文に無い"));
    RenameRequest {
        path: path.to_path_buf(),
        line,
        column,
        new_name: new_name.to_string(),
        document: Some(text.to_string()),
        timeout: Duration::from_secs(10),
    }
}

/// 答えを編集バッファへ当てた本文（GUI / dispatch と同じ 1 実装 = `apply_changes`）
fn applied(path: &Path, text: &str, answer: &RenameAnswer) -> TextBuffer {
    let mut buffer = TextBuffer::from_text(path.to_path_buf(), text.to_string());
    buffer
        .apply_changes(answer.source_changes.clone(), Some(buffer.version()))
        .unwrap();
    buffer
}

/// #1653 の事故の形: 小文字の `value` だけを変え、`Value::new()` と `values` は変えない
const MAIN: &str = "struct Value;\nimpl Value { fn new() -> Self { Value } }\n\
fn main() {\n    let value = Value::new();\n    let values = 1;\n    drop(value);\n    let _ = (value, values);\n}\n";
const RENAMED: &str = "struct Value;\nimpl Value { fn new() -> Self { Value } }\n\
fn main() {\n    let item = Value::new();\n    let values = 1;\n    drop(item);\n    let _ = (item, values);\n}\n";

/// 受け入れ条件（単体）: 答えに「ファイル数」「箇所数」が入り、偽サーバの `WorkspaceEdit`
/// （既定の `documentChanges` / 規則の `changes`）と一致する。当てると語としての `value` だけが変わり、
/// undo 1 回で元の全文とバイト一致へ戻る。ディスクの中身は 1 バイトも変わらない（manager は書かない）
#[test]
fn 頼んだファイルだけのリネームは件数が答えと一致しundo1回で戻る() {
    for shape in [
        json!([]),
        json!([{ "method": "textDocument/rename", "shape": "changes" }]),
    ] {
        let scratch = Scratch::new("source");
        let path = scratch.write("src/main.rs", MAIN);
        let manager = LspManager::new(config(&scratch, Some(&shape), &[]));
        let answer = manager
            .rename(&request(&path, MAIN, "value", 0, "item"))
            .unwrap_or_else(|e| panic!("{shape}: 答えが来る: {e:?}"));
        assert_eq!(answer.file_count(), 1, "{shape}");
        assert_eq!(answer.edit_count(), 3, "{shape}");
        assert_eq!(answer.blocker, None, "{shape}");
        assert_eq!(answer.placeholder.as_deref(), Some("value"), "{shape}");
        let file = &answer.files[0];
        assert!(file.source, "{shape}");
        let places: Vec<(usize, usize, &str)> = file
            .spots
            .iter()
            .map(|s| (s.line, s.column, s.old.as_str()))
            .collect();
        assert_eq!(
            places,
            [(3, 8, "value"), (5, 9, "value"), (6, 13, "value")],
            "{shape}"
        );
        let mut buffer = applied(&path, MAIN, &answer);
        assert_eq!(buffer.text(), RENAMED, "{shape}");
        assert!(buffer.undo(), "{shape}");
        assert_eq!(
            buffer.text().as_bytes(),
            MAIN.as_bytes(),
            "{shape}: undo 1 回"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            MAIN.as_bytes(),
            "{shape}: ディスクは不変"
        );
        manager.shutdown_all(Duration::from_secs(2));
        // 送るもの: prepareRename（申告どおり）→ rename の順
        let sent = methods(&scratch);
        let prepare = sent.iter().position(|m| m == "textDocument/prepareRename");
        let rename = sent.iter().position(|m| m == "textDocument/rename");
        assert!(prepare.is_some() && prepare < rename, "{shape}: {sent:?}");
    }
}

/// 受け入れ条件（単体）: 別のファイルを含む `WorkspaceEdit` は段階 1 の制限（`Blocker::OtherFiles`）を
/// 件数つきで返し、理由は `text` の日英の文。頼んだファイルの書き換えは持つが当てるかは dispatch が決める
#[test]
fn 別のファイルを含むリネームは段階1の制限を件数つきで返す() {
    let scratch = Scratch::new("others");
    let path = scratch.write("src/main.rs", MAIN);
    scratch.write("src/lib.rs", "pub fn f(value: u8) -> u8 { value }\n");
    scratch.write("src/util.rs", "// value\n");
    let manager = LspManager::new(config(&scratch, Some(&json!([])), &["--disk-model"]));
    let answer = manager
        .rename(&request(&path, MAIN, "value", 0, "item"))
        .expect("答えが来る");
    assert_eq!(answer.file_count(), 3);
    assert_eq!(answer.edit_count(), 6);
    assert_eq!(
        answer.blocker,
        Some(Blocker::OtherFiles { files: 2, edits: 3 })
    );
    assert_eq!(
        answer.blocker_reason().unwrap(),
        text::fill(
            text::RENAME_OTHER_FILES_REASON,
            &[("files", "2"), ("edits", "3")]
        )
    );
    // 開いていないファイルの場所はディスクの中身で tako の座標へ直す
    let lib = answer
        .files
        .iter()
        .find(|f| f.path.as_ref().is_some_and(|p| p.ends_with("lib.rs")))
        .expect("lib.rs が並ぶ");
    assert!(!lib.source);
    assert_eq!(lib.spots[0].column, 9);
    assert_eq!(lib.spots[0].old, "value");
    // ファイルの操作を含むものも当てない
    let scratch = Scratch::new("resource");
    let path = scratch.write("src/main.rs", MAIN);
    let rules = json!([{ "method": "textDocument/rename", "resource_op": true }]);
    let manager = LspManager::new(config(&scratch, Some(&rules), &[]));
    let answer = manager
        .rename(&request(&path, MAIN, "value", 0, "item"))
        .expect("答えが来る");
    assert_eq!(answer.blocker, Some(Blocker::ResourceOps { count: 1 }));
    assert_eq!(answer.resource_ops, ["rename"]);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件（単体）: `prepareRename` が拒否した位置（`null` / エラー）では `rename` を送らない
#[test]
fn prepare_rename_が拒否した位置では名前の変更を始めない() {
    // 語の上でない位置（空白）= 偽サーバの既定は null
    let scratch = Scratch::new("rejected");
    let path = scratch.write("src/main.rs", MAIN);
    let manager = LspManager::new(config(&scratch, Some(&json!([])), &[]));
    let mut req = request(&path, MAIN, "    let value", 0, "item");
    req.column = 1;
    let error = manager.rename(&req).expect_err("断られる");
    assert_eq!(
        error,
        RenameError::Rejected {
            server: "rust-analyzer",
            detail: None,
        }
    );
    assert_eq!(error.status(), "rejected");
    manager.shutdown_all(Duration::from_secs(2));
    let sent = methods(&scratch);
    assert!(
        sent.iter().any(|m| m == "textDocument/prepareRename"),
        "{sent:?}"
    );
    assert!(!sent.iter().any(|m| m == "textDocument/rename"), "{sent:?}");

    // エラーで断る（rust-analyzer の「No references found at position」の形）
    let scratch = Scratch::new("rejected-error");
    let path = scratch.write("src/main.rs", MAIN);
    let rules = json!([{
        "method": "textDocument/prepareRename",
        "error": { "code": -32602, "message": "No references found at position" },
    }]);
    let manager = LspManager::new(config(&scratch, Some(&rules), &[]));
    let error = manager
        .rename(&request(&path, MAIN, "Value", 1, "Item"))
        .expect_err("断られる");
    assert_eq!(
        error.reason(),
        text::fill(
            text::RENAME_REJECTED_DETAIL_REASON,
            &[
                ("server", "rust-analyzer"),
                ("detail", "No references found at position")
            ]
        )
    );
    manager.shutdown_all(Duration::from_secs(2));
    assert!(!methods(&scratch).iter().any(|m| m == "textDocument/rename"));
}

/// 境界: 識別子として正しくない名前はサーバが断り（`refused`）、サーバの文が理由に載る
#[test]
fn 不正な名前はサーバの断りを理由つきで返す() {
    let scratch = Scratch::new("refused");
    let path = scratch.write("src/main.rs", MAIN);
    let manager = LspManager::new(config(&scratch, Some(&json!([])), &[]));
    let error = manager
        .rename(&request(&path, MAIN, "value", 0, "1abc"))
        .expect_err("断られる");
    let RenameError::Refused { detail, .. } = &error else {
        panic!("refused ではない: {error:?}");
    };
    assert_eq!(detail, "Invalid name `1abc`: not an identifier");
    assert_eq!(error.status(), "refused");
    assert!(!error.reason().contains('{'));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 境界: 名前の変更に対応していないサーバ（`renameProvider` なし）は何も問わずに `unsupported`。
/// `prepareProvider` を申告しないサーバには `prepareRename` を送らずに `rename` だけを送る
#[test]
fn 能力が無ければ問わず_prepare_を申告しなければ_rename_だけを送る() {
    let scratch = Scratch::new("unsupported");
    let path = scratch.write("src/main.rs", MAIN);
    let manager = LspManager::new(config(&scratch, None, &[]));
    let error = manager
        .rename(&request(&path, MAIN, "value", 0, "item"))
        .expect_err("断られる");
    assert_eq!(
        error,
        RenameError::Unsupported {
            server: "rust-analyzer"
        }
    );
    assert_eq!(
        error.reason(),
        text::fill(
            text::RENAME_UNSUPPORTED_REASON,
            &[("server", "rust-analyzer")]
        )
    );
    manager.shutdown_all(Duration::from_secs(2));
    let sent = methods(&scratch);
    assert!(
        !sent.iter().any(|m| m.contains("ename")),
        "能力が無いのに問い合わせた: {sent:?}"
    );

    let scratch = Scratch::new("no-prepare");
    let path = scratch.write("src/main.rs", MAIN);
    let manager = LspManager::new(config(
        &scratch,
        Some(&json!([])),
        &["--providers", r#"{"renameProvider":true}"#],
    ));
    let answer = manager
        .rename(&request(&path, MAIN, "value", 0, "item"))
        .expect("答えが来る");
    assert_eq!(answer.edit_count(), 3);
    assert_eq!(answer.placeholder, None);
    manager.shutdown_all(Duration::from_secs(2));
    let sent = methods(&scratch);
    assert!(
        !sent.iter().any(|m| m == "textDocument/prepareRename"),
        "{sent:?}"
    );
    assert!(sent.iter().any(|m| m == "textDocument/rename"), "{sent:?}");
}

/// 握手の申告: `prepareSupport` と `workspaceEdit.documentChanges`（ファイルの操作は受けない）を送る
#[test]
fn 握手で名前の変更の受け方を申告する() {
    let scratch = Scratch::new("initialize");
    let path = scratch.write("src/main.rs", MAIN);
    let manager = LspManager::new(config(&scratch, Some(&json!([])), &[]));
    manager
        .rename(&request(&path, MAIN, "value", 0, "item"))
        .expect("答えが来る");
    manager.shutdown_all(Duration::from_secs(2));
    let initialize: Value = std::fs::read_to_string(scratch.log())
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|m| m["method"] == json!("initialize"))
        .expect("initialize が届く");
    let caps = &initialize["params"]["capabilities"];
    assert_eq!(
        caps["textDocument"]["rename"]["prepareSupport"],
        json!(true)
    );
    assert_eq!(
        caps["workspace"]["workspaceEdit"],
        json!({ "documentChanges": true, "resourceOperations": [] })
    );
}

/// `placeholder` を付けずに範囲だけを返すサーバ（rust-analyzer の形）は、その範囲の字面を今の名前にする
#[test]
fn 範囲だけの_prepare_は字面を今の名前にする() {
    let scratch = Scratch::new("range-only");
    let path = scratch.write("src/main.rs", MAIN);
    let rules = json!([{
        "method": "textDocument/prepareRename",
        "result": {
            "start": { "line": 3, "character": 8 },
            "end": { "line": 3, "character": 13 },
        },
    }]);
    let manager = LspManager::new(config(&scratch, Some(&rules), &[]));
    let answer = manager
        .rename(&request(&path, MAIN, "value", 0, "item"))
        .expect("答えが来る");
    assert_eq!(answer.placeholder.as_deref(), Some("value"));
    assert_eq!(answer.edit_count(), 3);
    manager.shutdown_all(Duration::from_secs(2));
}
