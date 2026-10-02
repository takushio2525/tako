//! #1683 の e2e: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こし、整形の問い合わせ
//! （`LspManager::format`）が送るもの・受けて直すもの・当てた結果を測る。
//!
//! 当てるのは GUI / dispatch が `TextBuffer::apply_changes` で行うので、ここでは同じ 1 実装へ
//! 答えを渡して本文を固定値と比べる（undo 1 回で前へ戻ることも）。当てる場所（どのペインか・
//! 版が変わっていないか）は dispatch の単体（MockHost）が持つ。
//!
//! 文言は `tako_control::lsp::text` の定数から読んで比べる（理由文を直書きしない）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, FormatAnswer, FormatError, FormatRequest, GotoError, Launch, LspConfig, LspManager,
};
use tako_core::lsp::format::FormatOptions;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::text_edit::{IndentUnit, TextBuffer};

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1683-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self(dir)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }

    fn rules(&self) -> PathBuf {
        self.0.join("format.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(scratch: &Scratch, scenario: &str, rules: &Value) -> LspConfig {
    std::fs::write(scratch.rules(), rules.to_string()).unwrap();
    let args = vec![
        "--scenario".to_string(),
        scenario.to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--format".to_string(),
        scratch.rules().display().to_string(),
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

fn request(path: &Path, text: &str, range: Option<std::ops::Range<usize>>) -> FormatRequest {
    FormatRequest {
        path: path.to_path_buf(),
        text: text.to_string(),
        range,
        options: FormatOptions::from_indent(IndentUnit::Spaces(4)),
        timeout: Duration::from_secs(10),
    }
}

fn edit(sl: u64, sc: u64, el: u64, ec: u64, new_text: &str) -> Value {
    json!({
        "range": {
            "start": { "line": sl, "character": sc },
            "end": { "line": el, "character": ec },
        },
        "newText": new_text,
    })
}

/// 答えを編集バッファへ当てた本文（GUI / dispatch と同じ 1 実装 = `apply_changes`）
fn applied(path: &Path, text: &str, answer: &FormatAnswer) -> TextBuffer {
    let mut buffer = TextBuffer::from_text(path.to_path_buf(), text.to_string());
    buffer
        .apply_changes(answer.changes.clone(), Some(buffer.version()))
        .unwrap();
    buffer
}

/// 整形の前の本文（行頭が揃っておらず、`=` の前後に空白が無い）
const UNFORMATTED: &str = "fn main(){\nlet x=1;\nx\n}\n";
const FORMATTED: &str = "fn main() {\n    let x = 1;\nx\n}\n";

/// 重なりの無い 3 か所（`{` の前・行頭・`=` の前後）
fn three_edits() -> Vec<Value> {
    vec![
        edit(0, 9, 0, 9, " "),
        edit(1, 0, 1, 0, "    "),
        edit(1, 5, 1, 6, " = "),
    ]
}

/// 受け入れ条件: 偽サーバが返す `TextEdit` の配列（重なりなし / 隣接 / 逆順 / 空置換）を
/// 当てた結果が固定値と一致し、undo 1 回で整形の前の全文とバイト一致へ戻る
#[test]
fn 四つの形の答えを当てると固定値になりundo1回で戻る() {
    let mut reversed = three_edits();
    reversed.reverse();
    let cases: Vec<(&str, &str, Vec<Value>, &str)> = vec![
        ("重なりなし", UNFORMATTED, three_edits(), FORMATTED),
        ("逆順", UNFORMATTED, reversed, FORMATTED),
        (
            "隣接",
            UNFORMATTED,
            vec![
                edit(0, 9, 0, 10, " {"),
                edit(0, 10, 1, 0, "\n    "),
                edit(2, 0, 2, 1, "    x"),
                edit(2, 1, 2, 1, ";"),
            ],
            "fn main() {\n    let x=1;\n    x;\n}\n",
        ),
        (
            "空置換",
            "a  \nb \nc\n",
            vec![
                edit(0, 1, 0, 3, ""),
                edit(1, 1, 1, 2, ""),
                // 中身の同じ置き換え・空の挿入（何も変えない）
                edit(2, 0, 2, 1, "c"),
                edit(2, 1, 2, 1, ""),
            ],
            "a\nb\nc\n",
        ),
    ];
    for (name, before, edits, after) in cases {
        let scratch = Scratch::new("shapes");
        let path = scratch.write("src/main.rs", before);
        let manager = LspManager::new(config(&scratch, "normal", &json!([{ "result": edits }])));
        let answer = manager
            .format(&request(&path, before, None))
            .unwrap_or_else(|e| panic!("{name}: 答えが来る: {e:?}"));
        let mut buffer = applied(&path, before, &answer);
        assert_eq!(buffer.text(), after, "{name}");
        assert!(buffer.undo(), "{name}");
        assert_eq!(
            buffer.text().as_bytes(),
            before.as_bytes(),
            "{name}: undo 1 回"
        );
        manager.shutdown_all(Duration::from_secs(2));
    }
}

/// 送るもの: 握手 → 一時的な didOpen（頼んだ本文）→ formatting（FormattingOptions つき）→ didClose。
/// 既定の答え（偽サーバが自分の本文から作る行末の空白の削除）が当たる
#[test]
fn 開いていない文書は問い合わせのあいだだけ開き設定を添えて頼む() {
    let scratch = Scratch::new("transient");
    let text = "fn a() {  \n\tlet x = 1;\t\n}\n";
    let path = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let mut req = request(&path, text, None);
    req.options = FormatOptions::from_indent(IndentUnit::Tab);
    let answer = manager.format(&req).expect("答えが来る");
    assert_eq!(answer.changes.len(), 2);
    assert_eq!(
        applied(&path, text, &answer).text(),
        "fn a() {\n\tlet x = 1;\n}\n"
    );
    wait_until("didClose", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didClose".to_string())
    });
    assert_eq!(
        methods(&scratch),
        vec![
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/formatting",
            "textDocument/didClose",
        ]
    );
    let messages = received(&scratch);
    assert_eq!(messages[2]["params"]["textDocument"]["text"], json!(text));
    assert_eq!(
        messages[3]["params"]["options"],
        json!({ "tabSize": 4, "insertSpaces": false })
    );
    assert!(messages[3]["params"].get("range").is_none());
    // 初期化の申告に整形の 2 種がある
    let caps = &messages[0]["params"]["capabilities"]["textDocument"];
    assert!(caps.get("formatting").is_some() && caps.get("rangeFormatting").is_some());
    assert_eq!(manager.status(None)["documents"], json!(0));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 未保存の編集中: 写し（最後に同期した本文）と頼む本文が違えば、頼む前に didChange で揃える。
/// 偽サーバの答えは**自分の本文の模型**から作るので、揃っていなければ答えがずれる
#[test]
fn 同期がまだの本文は頼む前に揃えて答えの座標をずらさない() {
    let scratch = Scratch::new("align");
    let saved = "fn a() {\n    x();\n}\n";
    let path = scratch.write("src/main.rs", saved);
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &path, saved, 1);
    wait_until("didOpen", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/didOpen".to_string())
    });
    // 画面ではもう 2 行足してある（行末に空白・日本語つき）が、まだ同期していない
    let editing = "// 名前  \nfn a() {\n    x();   \n}\n";
    let answer = manager.format(&request(&path, editing, None)).unwrap();
    assert_eq!(
        applied(&path, editing, &answer).text(),
        "// 名前\nfn a() {\n    x();\n}\n"
    );
    let sent = methods(&scratch);
    let change = sent
        .iter()
        .position(|m| m == "textDocument/didChange")
        .expect("揃える didChange を送った");
    let format = sent
        .iter()
        .position(|m| m == "textDocument/formatting")
        .unwrap();
    assert!(change < format, "揃えてから頼む: {sent:?}");
    // 揃えた後にペインが同じ本文で同期しても送り直さない（写しと同じ）
    manager.sync(&mut link, true, &path, editing, 2);
    let changes = methods(&scratch)
        .iter()
        .filter(|m| *m == "textDocument/didChange")
        .count();
    assert_eq!(changes, 1);
    drop(link);
    manager.shutdown_all(Duration::from_secs(2));
}

/// 受け入れ条件: 範囲の整形は範囲の外を 1 バイトも変えない。範囲は UTF-16 の桁で送る
#[test]
fn 範囲の整形は範囲をサーバの座標で送り範囲の外を変えない() {
    let scratch = Scratch::new("range");
    // 各行の行末に空白。2〜3 行目（日本語・絵文字を含む）だけを整形する
    let text = "let a = 1;  \nlet 名 = 2;  \nlet 😀 = 3;  \nlet d = 4;  \n";
    let path = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([])));
    let start = text.find("let 名").unwrap();
    let end = text.find("let d").unwrap();
    let answer = manager
        .format(&request(&path, text, Some(start..end)))
        .unwrap();
    let after = applied(&path, text, &answer);
    assert_eq!(
        after.text(),
        "let a = 1;  \nlet 名 = 2;\nlet 😀 = 3;\nlet d = 4;  \n"
    );
    let bytes = after.text().as_bytes();
    assert_eq!(&bytes[..start], &text.as_bytes()[..start]);
    let tail = &text.as_bytes()[end..];
    assert_eq!(&bytes[bytes.len() - tail.len()..], tail);
    let asked = received(&scratch)
        .into_iter()
        .find(|m| m["method"] == json!("textDocument/rangeFormatting"))
        .expect("範囲の整形を頼んだ");
    assert_eq!(
        asked["params"]["range"],
        json!({
            "start": { "line": 1, "character": 0 },
            "end": { "line": 3, "character": 0 },
        })
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 範囲の外へ空白以外の書き換えが来たら全体を当てない / 空白だけなら捨てて残りを当てる
#[test]
fn 範囲の外へ来た書き換えは空白だけなら捨て空白以外なら全体をやめる() {
    let scratch = Scratch::new("outside");
    let text = "use b;\nuse a;\nfn f(){}\n";
    let path = scratch.write("src/main.rs", text);
    let rules = json!([
        { "uri_suffix": "main.rs", "result": [edit(0, 4, 0, 5, "a"), edit(1, 4, 1, 5, "b")] },
    ]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let error = manager
        .format(&request(&path, text, Some(0..6)))
        .unwrap_err();
    assert_eq!(
        error,
        FormatError::OutsideRange {
            server: "rust-analyzer",
            count: 1
        }
    );
    assert_eq!(error.to_json()["status"], json!("outside-range"));
    manager.shutdown_all(Duration::from_secs(2));

    let scratch = Scratch::new("outside-blank");
    let path = scratch.write("src/main.rs", text);
    let rules = json!([{ "result": [edit(1, 0, 1, 0, "  "), edit(2, 6, 2, 6, " ")] }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let range = text.find("fn").unwrap()..text.len();
    let answer = manager.format(&request(&path, text, Some(range))).unwrap();
    assert_eq!(answer.dropped, 1);
    assert_eq!(
        applied(&path, text, &answer).text(),
        "use b;\nuse a;\nfn f() {}\n"
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// 重なった答えは当てない（どちらを採っても誰かの意図を壊す）
#[test]
fn 重なった答えは当てずに理由を返す() {
    let scratch = Scratch::new("overlap");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let rules = json!([{ "result": [edit(0, 0, 0, 4, "x"), edit(0, 2, 0, 6, "y")] }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    let error = manager
        .format(&request(&path, UNFORMATTED, None))
        .unwrap_err();
    assert!(
        matches!(&error, FormatError::InvalidEdits { detail, .. } if detail.contains("重なって")),
        "{error:?}"
    );
    assert_eq!(error.to_json()["status"], json!("invalid-edits"));
    manager.shutdown_all(Duration::from_secs(2));
}

/// 能力に無い整形は頼まない（範囲の整形だけ無いサーバは全体の整形へ案内する）
#[test]
fn 能力に無い整形は頼まずに区別して返す() {
    let scratch = Scratch::new("no-format");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let manager = LspManager::new(config(&scratch, "no-format", &json!([])));
    assert_eq!(
        manager.format(&request(&path, UNFORMATTED, None)),
        Err(FormatError::Unsupported {
            server: "rust-analyzer",
            ranged: false
        })
    );
    manager.shutdown_all(Duration::from_secs(2));
    assert!(!methods(&scratch).iter().any(|m| m.ends_with("ormatting")));

    let scratch = Scratch::new("no-range");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let manager = LspManager::new(config(&scratch, "no-range-format", &json!([])));
    let error = manager
        .format(&request(&path, UNFORMATTED, Some(0..10)))
        .unwrap_err();
    assert_eq!(
        error,
        FormatError::Unsupported {
            server: "rust-analyzer",
            ranged: true
        }
    );
    assert_eq!(
        error.to_json()["next_step"],
        json!(tako_control::lsp::text::FORMAT_RANGE_UNSUPPORTED_NEXT_STEP.text())
    );
    // 全体の整形は通る
    assert!(manager.format(&request(&path, UNFORMATTED, None)).is_ok());
    manager.shutdown_all(Duration::from_secs(2));
}

/// 答えが来ない・エラーで答えた・途中で落ちた・未導入を区別して返す（無言で死なない）
#[test]
fn 未応答とエラーと落ちたと未導入を区別する() {
    let scratch = Scratch::new("silent");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "silent": true }])));
    let mut req = request(&path, UNFORMATTED, None);
    req.timeout = Duration::from_secs(2);
    // 上限で解けたことは「どの経路で抜けたか」（応答待ちの打ち切り = starting: false）で見る
    // （実時間の絶対予算は負荷で反転する = `.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）
    assert_eq!(
        manager.format(&req),
        Err(FormatError::Lsp(GotoError::Timeout {
            server: "rust-analyzer",
            secs: 2,
            starting: false
        }))
    );
    manager.shutdown_all(Duration::from_secs(2));

    let scratch = Scratch::new("error");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let rules = json!([{ "error": { "code": -32603, "message": "rustfmt failed" } }]);
    let manager = LspManager::new(config(&scratch, "normal", &rules));
    assert_eq!(
        manager.format(&request(&path, UNFORMATTED, None)),
        Err(FormatError::Lsp(GotoError::ServerError {
            server: "rust-analyzer",
            code: -32603,
            detail: "rustfmt failed".into()
        }))
    );
    manager.shutdown_all(Duration::from_secs(2));

    let scratch = Scratch::new("crash");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "crash": true }])));
    assert_eq!(
        manager.format(&request(&path, UNFORMATTED, None)),
        Err(FormatError::Lsp(GotoError::Crashed {
            server: "rust-analyzer"
        }))
    );
    manager.shutdown_all(Duration::from_secs(2));

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
    let scratch = Scratch::new("missing");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let error = manager
        .format(&request(&path, UNFORMATTED, None))
        .unwrap_err();
    let value = error.to_json();
    assert_eq!(value["status"], json!("not-installed"), "{value}");
    assert!(value["install_command"]
        .as_str()
        .is_some_and(|c| !c.is_empty()));
    // 受け持つサーバが無い種類
    let other = scratch.write("notes.txt", "x\n");
    assert_eq!(
        manager.format(&request(&other, "x\n", None)),
        Err(FormatError::Lsp(GotoError::NoServer))
    );
}

/// 答えを待つあいだに本文が変わったら当てない（古い本文への答えは位置がずれる）
#[test]
fn 待つあいだに本文が変わったら古い答えを返さない() {
    let scratch = Scratch::new("stale");
    let path = scratch.write("src/main.rs", UNFORMATTED);
    let manager = LspManager::new(config(
        &scratch,
        "normal",
        &json!([{ "delay_ms": 600, "result": three_edits() }]),
    ));
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &path, UNFORMATTED, 1);
    let worker = {
        let manager = manager.clone();
        let req = request(&path, UNFORMATTED, None);
        std::thread::spawn(move || manager.format(&req))
    };
    wait_until("formatting", Duration::from_secs(10), || {
        methods(&scratch).contains(&"textDocument/formatting".to_string())
    });
    // 答えを待つあいだに打鍵（ペインの同期）
    manager.sync(
        &mut link,
        true,
        &path,
        "// typed\nfn main(){\nlet x=1;\nx\n}\n",
        2,
    );
    assert_eq!(worker.join().unwrap(), Err(FormatError::Stale));
    drop(link);
    manager.shutdown_all(Duration::from_secs(2));
}

/// CRLF のファイル: 行末の空白だけが消え、改行は CRLF のまま（範囲の外の改行は 1 バイトも変わらない）
#[test]
fn crlfのファイルは改行を保ったまま整形する() {
    let scratch = Scratch::new("crlf");
    let text = "fn a() { \r\n    x(); \r\n}\r\n";
    let path = scratch.write("src/main.rs", text);
    let manager = LspManager::new(config(&scratch, "normal", &json!([{ "reverse": true }])));
    let answer = manager.format(&request(&path, text, None)).unwrap();
    let mut buffer = applied(&path, text, &answer);
    assert_eq!(buffer.text(), "fn a() {\r\n    x();\r\n}\r\n");
    assert!(buffer.undo());
    assert_eq!(buffer.text(), text);
    manager.shutdown_all(Duration::from_secs(2));
}
