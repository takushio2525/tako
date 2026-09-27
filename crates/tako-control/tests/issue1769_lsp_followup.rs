//! #1769 の e2e（S1 の続き）: 偽の言語サーバ（`tako-lsp-fake`）を実プロセスで起こして測る。
//!
//! 1. **単独の `\r`**: 偽サーバは受けた本文を**自前の模型**で当てて持ち、位置を自前で数える
//!    （tako の変換を使わない）。`spec`（`\n` / `\r\n` / 単独の `\r`。pyright / TypeScript の実測）と
//!    `lf`（`\n` だけ。rust-analyzer / clangd の問い合わせの実測）の**両方の数え方**で、
//!    診断の位置・定義ジャンプの往復・編集後のサーバの本文が tako と一致することを見る
//! 2. **同じファイルの 2 ペイン目**: 1 URI = 1 文書を持ち手で共有する（didOpen は最初の 1 回・
//!    didClose は最後の 1 つが閉じたとき・どちらの編集も同じ文書の版を進める）
//! 3. **サーバの解決のキャッシュ**: 2 回目以降の `servers` は解決（ログインシェル）を起こさない。
//!    シェル統合の合図（cwd の変化・コマンドの終わり）と `restart` で引き直す

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{DocLink, GotoRequest, Launch, LspConfig, LspManager};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 1 件ぶんの置き場（スコープを抜けると消える = #1312）
struct Scratch {
    dir: ScratchDir,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = ScratchDir::new(&format!("tako-1769-{tag}"));
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        Self { dir }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn log(&self) -> PathBuf {
        self.path("received.jsonl")
    }

    fn doc_log(&self) -> PathBuf {
        self.path("docs.jsonl")
    }
}

fn fake_args(scratch: &Scratch, breaks: &str, extra: &[&str]) -> Vec<String> {
    let rules = scratch.path("goto.json");
    std::fs::write(&rules, json!([{ "echo": true }]).to_string()).unwrap();
    let mut args: Vec<String> = [
        "--scenario",
        "normal",
        "--log",
        &scratch.log().display().to_string(),
        "--doc-log",
        &scratch.doc_log().display().to_string(),
        "--goto",
        &rules.display().to_string(),
        "--line-breaks",
        breaks,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn config_with(args: Vec<String>) -> LspConfig {
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

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn methods(scratch: &Scratch) -> Vec<String> {
    read_jsonl(&scratch.log())
        .iter()
        .filter_map(|m| m.get("method").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn count(scratch: &Scratch, method: &str) -> usize {
    methods(scratch).iter().filter(|m| *m == method).count()
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

/// サーバが持つべき本文 = 単独の `\r` を `\n` に替えたもの（tako の実装を使わずに組む）
fn wire(text: &str) -> String {
    text.replace("\r\n", "\u{0}")
        .replace('\r', "\n")
        .replace('\u{0}', "\r\n")
}

/// tako の座標（`\n` 区切りの 0 起点の行・行内の UTF-8 バイト桁）での `needle` の出現位置
/// （tako の実装を使わずに組む）
fn tako_positions(text: &str, needle: &str) -> Vec<(usize, usize)> {
    text.match_indices(needle)
        .map(|(at, _)| {
            let line = text[..at].matches('\n').count();
            let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            (line, at - start)
        })
        .collect()
}

/// 偽サーバが持っている本文（その版の didOpen / didChange を当てた後）
fn server_text(scratch: &Scratch, version: u64) -> Option<String> {
    read_jsonl(&scratch.doc_log())
        .iter()
        .rev()
        .find(|e| e["fake_doc"]["version"] == json!(version))
        .and_then(|e| e["fake_doc"]["text"].as_str().map(str::to_string))
}

/// 開いている文書の診断を `(番号, tako の行, 桁)` の並びで（`MARK#<番号>` の順）
fn mark_diagnostics(manager: &LspManager, link: &DocLink) -> Vec<(usize, usize, usize)> {
    let DocLink::Open(lease) = link else {
        return Vec::new();
    };
    let Some(doc) = manager.document_diagnostics(lease.uri()) else {
        return Vec::new();
    };
    let mut out: Vec<(usize, usize, usize)> = doc
        .diagnostics
        .items
        .iter()
        .filter_map(|d| {
            let n = d.message.strip_prefix("MARK#")?.parse().ok()?;
            Some((n, d.start.line, d.start.col))
        })
        .collect();
    out.sort();
    out
}

fn expected_marks(text: &str) -> Vec<(usize, usize, usize)> {
    tako_positions(text, "MARK")
        .into_iter()
        .enumerate()
        .map(|(n, (line, col))| (n, line, col))
        .collect()
}

// --- 1. 単独の `\r` ---------------------------------------------------------

/// CRLF と単独 CR と LF が混ざり、単独 CR の後ろに多バイト文字と絵文字が並ぶ本文
const MIXED: &str = "fn a() {}\rMARK x;\r\n// 日本😀\rlet MARK = 1;\n\rMARK\r";

/// 受け入れ条件 1（診断の向き = サーバ → tako）: どちらの数え方のサーバでも、サーバが
/// 自分の本文で数えた位置を tako の座標へ写すと、本文の中の実際の位置と一致する。
/// 編集（単独 CR の直後へ打つ / 単独 CR を CRLF にする / CRLF を単独 CR にする /
/// 単独 CR を消す / 末尾へ足す）のあとも、サーバの本文は今の本文の送る形と一致し続ける
#[test]
fn 単独の_cr_を含む本文で診断の位置とサーバの本文が一致する() {
    for breaks in ["spec", "lf"] {
        let scratch = Scratch::new(&format!("diag-{breaks}"));
        let manager = LspManager::new(config_with(fake_args(
            &scratch,
            breaks,
            &["--mark", "MARK"],
        )));
        let path = scratch.path("src/main.rs");
        let mut link = DocLink::default();
        let mut text = MIXED.to_string();
        let mut version = 1;
        manager.sync(&mut link, true, &path, &text, version);
        let edits: [&dyn Fn(&str) -> String; 6] = [
            // 単独 CR の直後へ打つ
            &|t| t.replacen("\rMARK x;", "\rzz MARK x;", 1),
            // 単独 CR を CRLF にする
            &|t| t.replacen("😀\rlet", "😀\r\nlet", 1),
            // CRLF を単独 CR にする
            &|t| t.replacen("x;\r\n", "x;\r", 1),
            // 単独 CR を消す
            &|t| t.replacen("{}\r", "{}", 1),
            // 末尾へ単独 CR と語を足す
            &|t| format!("{t}😀MARK\r"),
            // 多バイト文字の前の単独 CR の手前へ打つ
            &|t| t.replacen("\n\rMARK", "\n\r日MARK", 1),
        ];
        let mut step = 0;
        loop {
            let want = wire(&text);
            wait_until(
                &format!("{breaks} 版 {version} の本文"),
                Duration::from_secs(10),
                || server_text(&scratch, version).as_deref() == Some(want.as_str()),
            );
            let expected = expected_marks(&text);
            wait_until(
                &format!("{breaks} 版 {version} の診断 {expected:?}"),
                Duration::from_secs(10),
                || mark_diagnostics(&manager, &link) == expected,
            );
            let Some(edit) = edits.get(step) else {
                break;
            };
            text = edit(&text);
            version += 1;
            step += 1;
            manager.sync(&mut link, true, &path, &text, version);
        }
        // 開いた本文は送る形（単独 CR を LF に替えたもの）で届いている
        let opened = read_jsonl(&scratch.log())
            .into_iter()
            .find(|m| m["method"] == json!("textDocument/didOpen"))
            .unwrap();
        assert_eq!(opened["params"]["textDocument"]["text"], json!(wire(MIXED)));
        assert_eq!(count(&scratch, "textDocument/didChange"), edits.len());
        manager.shutdown_all(Duration::from_secs(2));
    }
}

fn goto(path: &Path, line: usize, column: usize) -> GotoRequest {
    GotoRequest {
        kind: GotoKind::Definition,
        path: path.to_path_buf(),
        line,
        column,
        timeout: Duration::from_secs(10),
        document: None,
    }
}

/// 受け入れ条件 1（問い合わせの向き = tako → サーバ → tako）: tako の座標で問うと、サーバは
/// 自分の本文でその位置の語を引き、その範囲を返す。着地は問うた語の頭 = 位置が往復で一致する。
/// 文書が開いていない（ディスクの中身を一時的に開く）ときと、編集中（送った写し）のときの両方
#[test]
fn 単独の_cr_の後ろの語へ位置を往復させても同じ語へ着く() {
    let text = "a\rMARKa b\r\n😀\rc MARKb\n\r日 MARKc\r";
    let words = ["MARKa", "MARKb", "MARKc"];
    for breaks in ["spec", "lf"] {
        let scratch = Scratch::new(&format!("goto-{breaks}"));
        let path = scratch.path("src/main.rs");
        std::fs::write(&path, text).unwrap();
        let manager = LspManager::new(config_with(fake_args(&scratch, breaks, &[])));
        let mut link = DocLink::default();
        for editing in [false, true] {
            if editing {
                manager.sync(&mut link, true, &path, text, 1);
            }
            for word in words {
                let (line, col) = tako_positions(text, word)[0];
                // 語の途中（頭から 2 バイト）を問う
                let answer = manager.goto(&goto(&path, line, col + 2)).unwrap();
                assert_eq!(answer.targets.len(), 1, "{breaks} {word}");
                let target = &answer.targets[0];
                assert_eq!(
                    (target.line, target.column),
                    (line, col),
                    "{breaks} 編集中={editing} {word}: 着地は語の頭"
                );
                let asked = read_jsonl(&scratch.doc_log())
                    .into_iter()
                    .rev()
                    .find_map(|e| e.get("fake_goto").cloned())
                    .unwrap();
                assert_eq!(asked["word"], json!(word), "{breaks} 編集中={editing}");
            }
        }
        // 一時的に開いた文書は閉じ、編集中の文書は開いたまま
        assert_eq!(count(&scratch, "textDocument/didOpen"), 1 + words.len());
        assert_eq!(count(&scratch, "textDocument/didClose"), words.len());
        manager.shutdown_all(Duration::from_secs(2));
    }
}
