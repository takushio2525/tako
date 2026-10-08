//! テスト専用の偽 LSP サーバ（#1678。stdio のモック）
//!
//! 配布物には入らない（`scripts/build-app.sh` / Windows インストーラは `tako-app` と
//! `tako` だけを名指しで同梱する）。テストからは `env!("CARGO_BIN_EXE_tako-lsp-fake")` で
//! 絶対パスを取る（Cargo 標準。Windows でも同じ形）。
//!
//! 受け取ったメッセージを 1 行 1 JSON で `--log` へ追記し、起動のたびに自分の pid を
//! `--spawns` へ追記する。フレーミングは**tako 側の実装を使わず自前で書く**
//! （tako の読み書きに穴があっても偽サーバが同じ穴で辻褄を合わせないように）。
//!
//! シナリオ（`--scenario` か `TAKO_LSP_FAKE_SCENARIO`）:
//!
//! | 名前 | 振る舞い |
//! |---|---|
//! | `normal` | 握手して黙る。didOpen を受けたら診断を 1 件 publish する |
//! | `slow` | initialize に応答しない |
//! | `crash` | initialized を受けたら自分で落ちる |
//! | `split-header` | 応答の `Content-Length` を 2 回に分けて書く |
//! | `garbage` | initialize の応答の前に壊れた JSON を 1 つ混ぜる |
//! | `chatty` | initialized の後に通知を大量に投げる |
//! | `ask` | initialized の後に `workspace/configuration` を問い合わせる |
//! | `die` | 起動直後に即死する（initialize を読まない） |
//! | `no-goto` | 定義ジャンプの能力（`definitionProvider` 等）を申告しない（#1680） |
//! | `loading` | initialized の後 `experimental/serverStatus`（`quiescent: false`）を送り、読み込み（既定 0.8 秒。`--loading-ms` / `TAKO_LSP_FAKE_LOADING_MS`）が済むまで定義ジャンプに空（`[]`）で、補完に `null` で答え（補完の規則が `hold_while_loading` なら答えずに済むまで待たせる）、済んだら `quiescent: true` を送る（rust-analyzer の振る舞い。#1680 / #1869） |
//! | `full-sync` | `normal` と同じだが全文同期（`change: 1`）を申告する |
//! | `no-format` | 整形の能力（`documentFormattingProvider` / `documentRangeFormattingProvider`）を申告しない（#1683） |
//! | `no-range-format` | 文書全体の整形だけを申告する（範囲の整形は申告しない。#1683） |
//! | `no-completion` | 補完の能力（`completionProvider`）を申告しない（#1682） |
//! | `no-hover` | ホバーの能力（`hoverProvider`）を申告しない（#1681） |
//!
//! `--providers <json>` か `TAKO_LSP_FAKE_PROVIDERS`（JSON のオブジェクト）を渡すと、シナリオの
//! 能力（定義ジャンプ・整形・補完の `…Provider`）を捨てて**そのオブジェクトの組だけ**を申告する
//! （#1684。右クリックメニューの「能力の組み合わせ → 出る項目」を実プロセスで固定する。
//! 例: `{"definitionProvider":true}` = 定義のみ / `{}` = 能力ゼロ）。`@<file>` ならそのファイルの中身を
//! 起動のたびに読む（GUI を立てたまま組を替えて `tako lsp restart` で測る）。
//!
//! `--diagnostics <file>` か `TAKO_LSP_FAKE_DIAGNOSTICS`（LSP の `Diagnostic` の JSON 配列）を
//! 渡すと、didOpen と didChange のたびに**その配列をそのまま** publish する（#1679。
//! 固定の配列と tako 側の応答を突き合わせるため。didChange の回は文書の版も付ける）。
//! 渡さなければ didOpen で 1 件だけ publish する（#1678 の既定）。
//!
//! 定義ジャンプの 4 種（`textDocument/definition` / `declaration` / `typeDefinition` /
//! `implementation`。#1680）は `--goto <file>` か `TAKO_LSP_FAKE_GOTO` の規則で答える。
//! 規則は JSON の配列で、**上から順に最初に当たった 1 つ**を使う:
//!
//! ```json
//! [{ "method": "textDocument/definition", "uri_suffix": "main.rs", "line": 3,
//!    "result": { "uri": "file:///…/other.rs", "range": { … } } },
//!  { "line": 9, "silent": true },
//!  { "line": 11, "crash": true }]
//! ```
//!
//! `method` / `uri_suffix` / `line`（要求の 0 起点の行）は省けば何にでも当たる。
//! `silent` は答えない（未応答の検査）、`crash` は自分で落ちる（途中で落ちたときの検査）。
//! `echo` は問われた位置の語（英数字と `_`）の範囲をそのまま `Location` で返す（#1769。
//! 位置を往復させて tako の変換とサーバの数え方が揃っているかを見る）。
//! 当たる規則が無ければ `null`（= 見つからない）で答える。
//!
//! 整形の 2 種（`textDocument/formatting` / `rangeFormatting`。#1683）は `--format <file>` か
//! `TAKO_LSP_FAKE_FORMAT` の規則で答える。形は定義ジャンプの規則と同じく**上から順に最初に
//! 当たった 1 つ**（`method` / `uri_suffix` で絞る）で、`result`（`TextEdit` の配列をそのまま返す）/
//! `error`（`{ code, message }` でエラー応答）/ `silent` / `crash` / `delay_ms`（答える前に待つ。
//! 待つあいだに届いた通知は答えた後に読む）/ `reverse`（既定の答えを逆順で返す）を持つ。
//! 当たる規則が無い・`result` も `error` も無ければ**既定の整形**: 自分の本文の模型（下記）から
//! 行末の空白（スペースとタブ）を消す `TextEdit` を作る（範囲の整形は範囲に収まる行末だけ）。
//! 本文の模型から作るので、tako が送った `didChange` がずれていれば答えもずれる。
//! 補完（`textDocument/completion` / `completionItem/resolve`。#1682）は `--completion <file>` か
//! `TAKO_LSP_FAKE_COMPLETION` の JSON オブジェクトで答える（渡さなければ空の一覧）:
//!
//! ```json
//! { "items": [{ "label": "alpha", "kind": 3 }],   // そのまま返す候補
//!   "generate": 1000,                              // 代わりに cand0000.. を N 件作る
//!   "incomplete": false,                           // CompletionList.isIncomplete
//!   "delay_ms": 0,                                 // 答えるまでの待ち（待つ間に $/cancelRequest が来たら -32800 で答える）
//!   "hold_while_loading": false,                   // `loading` の読み込み中は null で即答せず、済むまで答えない（#1869）
//!   "word_edit": true,                             // 候補ごとにカーソルの直前の語を置き換える textEdit を付ける（自前の本文の模型で数える）
//!   "resolve_doc": "doc of {label}" }              // resolve で documentation を足す（{label} は候補の label）
//! ```
//!
//! ホバー（`textDocument/hover`。#1681）は `--hover <file>` か `TAKO_LSP_FAKE_HOVER` の規則で答える。
//! 形は定義ジャンプの規則と同じく**上から順に最初に当たった 1 つ**（`uri_suffix` / `line` で絞る）で、
//! `result`（`Hover` をそのまま返す。`null` も可）/ `silent` / `crash` / `delay_ms`（答えるまでの待ち。
//! 待つ間に `$/cancelRequest` が来たら -32800 で答える = 補完と同じ）/ `echo`（問われた位置の語を
//! 自前の本文の模型で引き、`**<語>**` の Markdown とその語の範囲を返す = 位置の往復を見る）を持つ。
//! 当たる規則が無ければ `null`（= 表示するものが無い）で答える。
//!
//! ## 本文の模型（#1769）
//!
//! 受けた本文（didOpen の全文・didChange の範囲 / 全文）を**自前で当てて**持つ。位置の数え方は
//! **tako の実装を使わずに**ここで組む（tako の変換に穴があっても偽サーバが同じ穴で辻褄を
//! 合わせないように）。`--line-breaks <spec|lf>`（`TAKO_LSP_FAKE_LINE_BREAKS`。既定 `spec`）:
//!
//! - `spec` = `\n` / `\r\n` / 単独の `\r` で行を区切る（LSP の仕様。pyright / TypeScript の実測）
//! - `lf` = `\n` だけで区切る（rust-analyzer / clangd が問い合わせの位置を数える形の実測）
//!
//! `--mark <語>`（`TAKO_LSP_FAKE_MARK`）を渡すと、didOpen / didChange のたびに自分の本文で
//! その語が現れる所すべてへ診断を publish する（`message` は `<語>#<0 起点の番号>`）。
//! `--doc-log <file>`（`TAKO_LSP_FAKE_DOC_LOG`）を渡すと、当てた後の全文（`fake_doc`）と
//! `echo` で問われた位置の語（`fake_goto`）を 1 行 1 JSON で残す（`--log` とは別のファイル =
//! 受けたメッセージの並びを数えるテストの添字をずらさない）。

use std::io::{BufRead, BufReader, Write};

fn arg_or_env(args: &[String], flag: &str, env: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var(env).ok().filter(|v| !v.is_empty()))
}

fn append(path: &Option<String>, line: &str) {
    if let Some(path) = path {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn read_one(input: &mut impl BufRead) -> Option<serde_json::Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

struct Out {
    split: bool,
}

impl Out {
    fn send(&self, message: serde_json::Value) {
        let body = message.to_string();
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdout = std::io::stdout().lock();
        if self.split {
            // ヘッダを 2 回に分けて書く（間を空けて、読み手の 1 回の read に乗らないように）
            let (a, b) = header.split_at(9);
            let _ = stdout.write_all(a.as_bytes());
            let _ = stdout.flush();
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = stdout.write_all(b.as_bytes());
        } else {
            let _ = stdout.write_all(header.as_bytes());
        }
        let _ = stdout.write_all(body.as_bytes());
        let _ = stdout.flush();
    }

    fn raw(&self, bytes: &[u8]) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(bytes);
        let _ = stdout.flush();
    }
}

/// 定義ジャンプの 4 種の method（#1680）
const GOTO_METHODS: [&str; 4] = [
    "textDocument/definition",
    "textDocument/declaration",
    "textDocument/typeDefinition",
    "textDocument/implementation",
];

/// 定義ジャンプの 4 種の能力のキー
const GOTO_PROVIDERS: [&str; 4] = [
    "definitionProvider",
    "declarationProvider",
    "typeDefinitionProvider",
    "implementationProvider",
];

/// 整形の 2 種の method（#1683）
const FORMAT_METHODS: [&str; 2] = ["textDocument/formatting", "textDocument/rangeFormatting"];

/// 行の数え方（#1769。模型の説明は冒頭）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Breaks {
    Spec,
    Lf,
}

/// 各行の先頭バイト位置
fn line_starts(text: &str, breaks: Breaks) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => starts.push(i + 1),
            b'\r' if breaks == Breaks::Spec => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                starts.push(i + 1);
            }
            _ => {}
        }
        i += 1;
    }
    starts
}

/// `line` 行目の中身の範囲（改行を除く。`lf` でも `\r\n` の `\r` は除く）
fn line_span(text: &str, starts: &[usize], line: usize, breaks: Breaks) -> (usize, usize) {
    let start = starts[line];
    let bytes = text.as_bytes();
    let mut end = starts.get(line + 1).copied().unwrap_or(text.len());
    if end > start && bytes[end - 1] == b'\n' {
        end -= 1;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
    } else if end > start && bytes[end - 1] == b'\r' && breaks == Breaks::Spec {
        end -= 1;
    }
    (start, end)
}

/// `(行, UTF-16 桁)` → バイト位置（行末・本文の末尾で頭打ち）
fn offset_at(text: &str, breaks: Breaks, line: usize, character: usize) -> usize {
    let starts = line_starts(text, breaks);
    if line >= starts.len() {
        return text.len();
    }
    let (start, end) = line_span(text, &starts, line, breaks);
    let mut units = 0;
    for (i, ch) in text[start..end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += ch.len_utf16();
    }
    end
}

/// バイト位置 → `(行, UTF-16 桁)`
fn position_at(text: &str, breaks: Breaks, offset: usize) -> (usize, usize) {
    let starts = line_starts(text, breaks);
    let line = starts.partition_point(|&s| s <= offset) - 1;
    let (start, end) = line_span(text, &starts, line, breaks);
    let character = text[start..offset.min(end)]
        .chars()
        .map(char::len_utf16)
        .sum();
    (line, character)
}

fn range_json(text: &str, breaks: Breaks, from: usize, to: usize) -> serde_json::Value {
    let (sl, sc) = position_at(text, breaks, from);
    let (el, ec) = position_at(text, breaks, to);
    serde_json::json!({
        "start": { "line": sl, "character": sc },
        "end": { "line": el, "character": ec },
    })
}

/// didOpen / didChange を自前の模型へ当てる
fn apply_document(
    docs: &mut std::collections::HashMap<String, String>,
    breaks: Breaks,
    method: &str,
    params: &serde_json::Value,
) -> Option<String> {
    let uri = params["textDocument"]["uri"].as_str()?.to_string();
    if method == "textDocument/didOpen" {
        let text = params["textDocument"]["text"].as_str()?.to_string();
        docs.insert(uri.clone(), text);
        return Some(uri);
    }
    let text = docs.get_mut(&uri)?;
    for change in params["contentChanges"].as_array()? {
        let new = change["text"].as_str()?;
        match change.get("range") {
            Some(range) => {
                let at = |key: &str| {
                    let p = &range[key];
                    offset_at(
                        text,
                        breaks,
                        p["line"].as_u64().unwrap_or(0) as usize,
                        p["character"].as_u64().unwrap_or(0) as usize,
                    )
                };
                let (from, to) = (at("start"), at("end"));
                text.replace_range(from..to.max(from), new);
            }
            None => *text = new.to_string(),
        }
    }
    Some(uri)
}

/// 語の範囲（英数字と `_`）。位置が語の上に無ければ `None`
fn word_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    (end > start).then_some((start, end))
}

/// 規則のうち、この要求に最初に当たるもの
fn goto_rule<'a>(
    rules: &'a [serde_json::Value],
    method: &str,
    params: &serde_json::Value,
) -> Option<&'a serde_json::Value> {
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64();
    rules.iter().find(|rule| {
        rule["method"].as_str().is_none_or(|m| m == method)
            && rule["uri_suffix"].as_str().is_none_or(|s| uri.ends_with(s))
            && rule["line"].as_u64().is_none_or(|l| Some(l) == line)
    })
}

/// 規則のうち、この整形の要求に最初に当たるもの（#1683）
fn format_rule<'a>(
    rules: &'a [serde_json::Value],
    method: &str,
    params: &serde_json::Value,
) -> Option<&'a serde_json::Value> {
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    rules.iter().find(|rule| {
        rule["method"].as_str().is_none_or(|m| m == method)
            && rule["uri_suffix"].as_str().is_none_or(|s| uri.ends_with(s))
    })
}

/// 既定の整形（#1683）: 行末の空白を消す。`within` は範囲の整形の範囲（バイト位置）
fn trailing_space_edits(
    text: &str,
    breaks: Breaks,
    within: Option<(usize, usize)>,
) -> Vec<serde_json::Value> {
    let starts = line_starts(text, breaks);
    let mut edits = Vec::new();
    for line in 0..starts.len() {
        let (start, end) = line_span(text, &starts, line, breaks);
        let content = &text[start..end];
        let kept = content.trim_end_matches([' ', '\t']).len();
        if kept == content.len() {
            continue;
        }
        let (from, to) = (start + kept, end);
        if within.is_some_and(|(a, b)| from < a || to > b) {
            continue;
        }
        // 位置はこの行の中だけで数える（`range_json` は呼ぶたびに行頭の表を作り直すので、
        // 10 万行の答えを作ると本文の長さ × 10 万になる = #1869 の 10 万行の整形の検証で詰まった）
        let utf16 = |s: &str| s.chars().map(char::len_utf16).sum::<usize>();
        let (first, whole) = (utf16(&content[..kept]), utf16(content));
        edits.push(serde_json::json!({
            "range": {
                "start": { "line": line, "character": first },
                "end": { "line": line, "character": whole },
            },
            "newText": "",
        }));
    }
    edits
}

/// 補完の候補（#1682。冒頭の説明の `items` / `generate` / `word_edit`）
fn completion_items(
    rules: &serde_json::Value,
    docs: &std::collections::HashMap<String, String>,
    breaks: Breaks,
    params: &serde_json::Value,
) -> serde_json::Value {
    let mut items: Vec<serde_json::Value> = match rules["generate"].as_u64() {
        Some(n) => (0..n)
            .map(|i| {
                serde_json::json!({
                    "label": format!("cand{i:04}"),
                    "kind": 6,
                    "sortText": format!("{i:04}"),
                    "detail": format!("detail {i}"),
                })
            })
            .collect(),
        None => rules["items"].as_array().cloned().unwrap_or_default(),
    };
    if rules["word_edit"].as_bool() == Some(true) {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = docs.get(uri).cloned().unwrap_or_default();
        let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
        let character = params["position"]["character"].as_u64().unwrap_or(0) as usize;
        let offset = offset_at(&text, breaks, line, character);
        let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let start = text[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(offset, |(i, _)| i);
        let range = range_json(&text, breaks, start, offset);
        for item in &mut items {
            let new_text = item["insertText"]
                .as_str()
                .or_else(|| item["label"].as_str())
                .unwrap_or("")
                .to_string();
            item["textEdit"] = serde_json::json!({ "range": range, "newText": new_text });
        }
    }
    serde_json::json!({
        "isIncomplete": rules["incomplete"].as_bool().unwrap_or(false),
        "items": items,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = arg_or_env(&args, "--scenario", "TAKO_LSP_FAKE_SCENARIO")
        .unwrap_or_else(|| "normal".into());
    let log = arg_or_env(&args, "--log", "TAKO_LSP_FAKE_LOG");
    let spawns = arg_or_env(&args, "--spawns", "TAKO_LSP_FAKE_SPAWNS");
    let fixed: Option<serde_json::Value> =
        arg_or_env(&args, "--diagnostics", "TAKO_LSP_FAKE_DIAGNOSTICS")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok());
    let goto_rules: Vec<serde_json::Value> = arg_or_env(&args, "--goto", "TAKO_LSP_FAKE_GOTO")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let format_rules: Vec<serde_json::Value> =
        arg_or_env(&args, "--format", "TAKO_LSP_FAKE_FORMAT")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
    // #1769: 本文の模型（冒頭の説明）
    let breaks = match arg_or_env(&args, "--line-breaks", "TAKO_LSP_FAKE_LINE_BREAKS").as_deref() {
        Some("lf") => Breaks::Lf,
        _ => Breaks::Spec,
    };
    let mark = arg_or_env(&args, "--mark", "TAKO_LSP_FAKE_MARK");
    // #1682: 補完の答え方と、待っている（まだ答えていない）補完の要求の id
    let completion: serde_json::Value =
        arg_or_env(&args, "--completion", "TAKO_LSP_FAKE_COMPLETION")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(serde_json::Value::Null);
    let waiting: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
        Default::default();
    // #1681: ホバーの規則
    let hover_rules: Vec<serde_json::Value> = arg_or_env(&args, "--hover", "TAKO_LSP_FAKE_HOVER")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let doc_log = arg_or_env(&args, "--doc-log", "TAKO_LSP_FAKE_DOC_LOG");
    let mut docs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    append(&spawns, &std::process::id().to_string());
    eprintln!("tako-lsp-fake: scenario={scenario}");
    if scenario == "die" {
        // 起動直後に即死する（initialize を 1 通も読まない）
        std::process::exit(2);
    }

    let out = std::sync::Arc::new(Out {
        split: scenario == "split-header",
    });
    // `loading`: 読み込みが済んだか（済むまでは定義ジャンプに空で答える）
    let loaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(scenario != "loading"));
    let loading_ms: u64 = arg_or_env(&args, "--loading-ms", "TAKO_LSP_FAKE_LOADING_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(800);
    // #1684: 申告する能力の組をそのまま差し替える（シナリオの能力は捨てる）
    let providers: Option<serde_json::Map<String, serde_json::Value>> =
        arg_or_env(&args, "--providers", "TAKO_LSP_FAKE_PROVIDERS")
            .and_then(|v| match v.strip_prefix('@') {
                Some(file) => std::fs::read_to_string(file).ok(),
                None => Some(v),
            })
            .and_then(|v| serde_json::from_str(&v).ok());
    let mut input = BufReader::new(std::io::stdin());
    while let Some(message) = read_one(&mut input) {
        append(&log, &message.to_string());
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = message.get("id").cloned();
        if matches!(method, "textDocument/didOpen" | "textDocument/didChange") {
            let params = &message["params"];
            if let Some(uri) = apply_document(&mut docs, breaks, method, params) {
                let text = docs.get(&uri).cloned().unwrap_or_default();
                let version = params["textDocument"]["version"].clone();
                append(
                    &doc_log,
                    &serde_json::json!({
                        "fake_doc": { "uri": uri, "version": version, "text": text }
                    })
                    .to_string(),
                );
                // `--mark`: 自分の本文でその語が現れる所すべてへ（`--diagnostics` が先に勝つ）
                if let (Some(mark), None) = (&mark, &fixed) {
                    let diagnostics: Vec<serde_json::Value> = text
                        .match_indices(mark.as_str())
                        .enumerate()
                        .map(|(n, (at, _))| {
                            serde_json::json!({
                                "range": range_json(&text, breaks, at, at + mark.len()),
                                "severity": 2,
                                "message": format!("{mark}#{n}"),
                            })
                        })
                        .collect();
                    out.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": { "uri": uri, "version": version, "diagnostics": diagnostics },
                    }));
                    continue;
                }
            }
        }
        match (method, id) {
            ("initialize", Some(id)) => {
                if scenario == "slow" {
                    continue;
                }
                if scenario == "garbage" {
                    out.raw(b"Content-Length: 5\r\n\r\n{nope");
                }
                // `full-sync` は全文同期（change = 1）を申告する（#1660 の計測で
                // 大きいファイルの 1 打鍵が全文を送るときの負荷を測る）
                let change = if scenario == "full-sync" { 1 } else { 2 };
                let mut capabilities = serde_json::json!({
                    "positionEncoding": "utf-16",
                    "textDocumentSync": { "openClose": true, "change": change },
                });
                if scenario != "no-goto" {
                    for key in GOTO_PROVIDERS {
                        capabilities[key] = serde_json::json!(true);
                    }
                }
                // #1683: 整形の能力（`no-format` は申告しない・`no-range-format` は全体だけ）
                if scenario != "no-format" {
                    capabilities["documentFormattingProvider"] = serde_json::json!(true);
                    if scenario != "no-range-format" {
                        capabilities["documentRangeFormattingProvider"] = serde_json::json!(true);
                    }
                }
                if scenario != "no-hover" {
                    capabilities["hoverProvider"] = serde_json::json!(true);
                }
                if scenario != "no-completion" {
                    capabilities["completionProvider"] = serde_json::json!({
                        "triggerCharacters": [".", ":"],
                        "resolveProvider": true,
                    });
                }
                if let Some(providers) = &providers {
                    if let Some(map) = capabilities.as_object_mut() {
                        map.retain(|key, _| !key.ends_with("Provider"));
                        map.extend(providers.clone());
                    }
                }
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "capabilities": capabilities,
                        "serverInfo": { "name": "tako-lsp-fake", "version": "1" },
                    },
                }));
            }
            ("initialized", None) => match scenario.as_str() {
                "crash" => std::process::exit(3),
                "loading" => {
                    out.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "experimental/serverStatus",
                        "params": { "health": "ok", "quiescent": false },
                    }));
                    let (out, loaded) = (out.clone(), loaded.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(loading_ms));
                        loaded.store(true, std::sync::atomic::Ordering::SeqCst);
                        out.send(serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "experimental/serverStatus",
                            "params": { "health": "ok", "quiescent": true },
                        }));
                    });
                }
                "chatty" => {
                    for n in 0..5000 {
                        out.send(serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "window/logMessage",
                            "params": { "type": 4, "message": format!("n={n}") },
                        }));
                    }
                }
                "ask" => out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "cfg-1",
                    "method": "workspace/configuration",
                    "params": { "items": [{ "section": "a" }, { "section": "b" }] },
                })),
                _ => {}
            },
            ("textDocument/didOpen" | "textDocument/didChange", None) if fixed.is_some() => {
                let document = &message["params"]["textDocument"];
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": document["uri"].clone(),
                        "version": document["version"].clone(),
                        "diagnostics": fixed.clone(),
                    },
                }));
            }
            ("textDocument/didOpen", None) => {
                let uri = message["params"]["textDocument"]["uri"].clone();
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": uri,
                        "diagnostics": [{
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": { "line": 0, "character": 1 },
                            },
                            "severity": 2,
                            "message": "fake",
                        }],
                    },
                }));
            }
            (method, Some(id)) if GOTO_METHODS.contains(&method) => {
                if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": [] }));
                    continue;
                }
                let Some(rule) = goto_rule(&goto_rules, method, &message["params"]) else {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                    continue;
                };
                if rule["crash"].as_bool() == Some(true) {
                    std::process::exit(4);
                }
                if rule["silent"].as_bool() == Some(true) {
                    continue;
                }
                if rule["echo"].as_bool() == Some(true) {
                    // 問われた位置の語を自分の本文で引き、その範囲を返す（#1769）
                    let params = &message["params"];
                    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                    let text = docs.get(uri).cloned().unwrap_or_default();
                    let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
                    let character = params["position"]["character"].as_u64().unwrap_or(0) as usize;
                    let offset = offset_at(&text, breaks, line, character);
                    let word = word_at(&text, offset);
                    append(
                        &doc_log,
                        &serde_json::json!({ "fake_goto": {
                            "line": line,
                            "character": character,
                            "word": word.map(|(a, b)| text[a..b].to_string()),
                        }})
                        .to_string(),
                    );
                    let result = match word {
                        Some((a, b)) => serde_json::json!({
                            "uri": uri,
                            "range": range_json(&text, breaks, a, b),
                        }),
                        None => serde_json::Value::Null,
                    };
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    continue;
                }
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": rule.get("result").cloned().unwrap_or(serde_json::Value::Null),
                }));
            }
            (method, Some(id)) if FORMAT_METHODS.contains(&method) => {
                let params = &message["params"];
                let rule = format_rule(&format_rules, method, params);
                if let Some(rule) = rule {
                    if rule["crash"].as_bool() == Some(true) {
                        std::process::exit(5);
                    }
                    if rule["silent"].as_bool() == Some(true) {
                        continue;
                    }
                    if let Some(ms) = rule["delay_ms"].as_u64() {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                    }
                    if let Some(error) = rule.get("error") {
                        out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error }));
                        continue;
                    }
                    if let Some(result) = rule.get("result") {
                        out.send(
                            serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        );
                        continue;
                    }
                }
                // 既定: 自分の本文の模型から行末の空白を消す
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let text = docs.get(uri).cloned().unwrap_or_default();
                let within = params.get("range").map(|range| {
                    let at = |key: &str| {
                        let p = &range[key];
                        offset_at(
                            &text,
                            breaks,
                            p["line"].as_u64().unwrap_or(0) as usize,
                            p["character"].as_u64().unwrap_or(0) as usize,
                        )
                    };
                    (at("start"), at("end"))
                });
                let mut edits = trailing_space_edits(&text, breaks, within);
                if rule.is_some_and(|r| r["reverse"].as_bool() == Some(true)) {
                    edits.reverse();
                }
                out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": edits }));
            }
            // #1682: 補完。`delay_ms` のあいだに取り消されたら -32800 で答える（LSP の作法 =
            // 取り消されても応答は返す）。待ちの表は答えた / 取り消した時点で外す
            ("textDocument/completion", Some(id)) => {
                let result = completion_items(&completion, &docs, breaks, &message["params"]);
                // #1869: 読み込み中（実測した rust-analyzer 1.95 の 2 段）。前半は即座に `null`、
                // `hold_while_loading` なら答えずに済むまで待たせ、済んだ直後に答える
                if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                    if completion["hold_while_loading"].as_bool() != Some(true) {
                        out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                        continue;
                    }
                    let key = id.to_string();
                    waiting.lock().unwrap().insert(key.clone());
                    let (out, waiting, loaded) = (out.clone(), waiting.clone(), loaded.clone());
                    std::thread::spawn(move || {
                        while !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        if waiting.lock().unwrap().remove(&key) {
                            out.send(
                                serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                            );
                        }
                    });
                    continue;
                }
                let delay = completion["delay_ms"].as_u64().unwrap_or(0);
                if delay == 0 {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    continue;
                }
                let key = id.to_string();
                waiting.lock().unwrap().insert(key.clone());
                let (out, waiting) = (out.clone(), waiting.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    if waiting.lock().unwrap().remove(&key) {
                        out.send(
                            serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        );
                    }
                });
            }
            // #1681: ホバー。`delay_ms` のあいだに取り消されたら -32800 で答える（補完と同じ作法）
            ("textDocument/hover", Some(id)) => {
                let params = &message["params"];
                let Some(rule) = goto_rule(&hover_rules, "textDocument/hover", params) else {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                    continue;
                };
                if rule["crash"].as_bool() == Some(true) {
                    std::process::exit(6);
                }
                if rule["silent"].as_bool() == Some(true) {
                    continue;
                }
                let result = if rule["echo"].as_bool() == Some(true) {
                    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                    let text = docs.get(uri).cloned().unwrap_or_default();
                    let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
                    let character = params["position"]["character"].as_u64().unwrap_or(0) as usize;
                    let offset = offset_at(&text, breaks, line, character);
                    match word_at(&text, offset) {
                        Some((a, b)) => serde_json::json!({
                            "contents": { "kind": "markdown", "value": format!("**{}**", &text[a..b]) },
                            "range": range_json(&text, breaks, a, b),
                        }),
                        None => serde_json::Value::Null,
                    }
                } else {
                    rule.get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                };
                let delay = rule["delay_ms"].as_u64().unwrap_or(0);
                if delay == 0 {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    continue;
                }
                let key = id.to_string();
                waiting.lock().unwrap().insert(key.clone());
                let (out, waiting) = (out.clone(), waiting.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    if waiting.lock().unwrap().remove(&key) {
                        out.send(
                            serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        );
                    }
                });
            }
            ("$/cancelRequest", None) => {
                let id = message["params"]["id"].clone();
                if waiting.lock().unwrap().remove(&id.to_string()) {
                    out.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32800, "message": "cancelled" },
                    }));
                }
            }
            ("completionItem/resolve", Some(id)) => {
                let mut item = message["params"].clone();
                if let Some(doc) = completion["resolve_doc"].as_str() {
                    let label = item["label"].as_str().unwrap_or("").to_string();
                    item["documentation"] = serde_json::json!(doc.replace("{label}", &label));
                }
                out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": item }));
            }
            ("shutdown", Some(id)) => out.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null,
            })),
            ("exit", None) => std::process::exit(0),
            _ => {}
        }
    }
    // stdin が閉じた（親が落ちた）ら自分で終わる = 孤児にならない
}
