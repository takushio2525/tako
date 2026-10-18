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
//! | `loading` | initialized の後 `experimental/serverStatus`（`quiescent: false`）を送り、読み込み（既定 0.8 秒。`--loading-ms` / `TAKO_LSP_FAKE_LOADING_MS`。`--loading-until` を渡せばその合図まで = #1922）が済むまで定義ジャンプに空（`[]`）で、補完とホバーに `null` で答え（補完の規則・当たったホバーの規則が `hold_while_loading` なら答えずに済むまで待たせる）、済んだら `quiescent: true` を送る（rust-analyzer の振る舞い。#1680 / #1869 / #1893） |
//! | `quiet-loading` | `loading` と同じ読み込み（`--loading-ms` / `--loading-until`）だが**状態を 1 度も送らず**、読み込み中は補完に `{"isIncomplete":true,"items":[]}`・定義ジャンプに `[]`・ホバーに `null` で答え、didOpen の診断を送らずに溜めて済んだ時点で送る（pyright 1.1.414 の実測の形 = 自動 import の索引ができるまで空、索引ができる頃に最初の診断。#1977）。`--diagnostics` / `--mark` とは併せない |
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
//! `error`（`{ code, message }` でエラー応答）/ `silent` / `crash` / `hold_until`（そのファイルが
//! できるまで答えない。待つあいだに届いた通知は答えた後に読む = #1930）/ `reverse`（既定の答えを
//! 逆順で返す）を持つ。
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
//!   "hold_until": "/path/open",                    // そのファイルができるまで答えない（待つ間に $/cancelRequest が来たら -32800 で答える。配列なら k 本目の要求が k 番目を待つ。#1930）
//!   "hold_while_loading": false,                   // `loading` の読み込み中は null で即答せず、済むまで答えない（#1869）
//!   "word_edit": true,                             // 候補ごとにカーソルの直前の語を置き換える textEdit を付ける（自前の本文の模型で数える）
//!   "resolve_doc": "doc of {label}" }              // resolve で documentation を足す（{label} は候補の label）
//! ```
//!
//! ホバー（`textDocument/hover`。#1681）は `--hover <file>` か `TAKO_LSP_FAKE_HOVER` の規則で答える。
//! 形は定義ジャンプの規則と同じく**上から順に最初に当たった 1 つ**（`uri_suffix` / `line` で絞る）で、
//! `result`（`Hover` をそのまま返す。`null` も可）/ `silent` / `crash` / `hold_until`（そのファイルが
//! できるまで答えない。待つ間に `$/cancelRequest` が来たら -32800 で答える = 補完と同じ。#1930）/
//! `echo`（問われた位置の語を
//! 自前の本文の模型で引き、`**<語>**` の Markdown とその語の範囲を返す = 位置の往復を見る）/
//! `hold_while_loading`（`loading` の読み込み中は null で即答せず、済むまで答えない。#1893）を持つ。
//! 当たる規則が無ければ `null`（= 表示するものが無い）で答える。
//!
//! 参照検索・呼び出し階層・型階層（`textDocument/references` / `prepareCallHierarchy` /
//! `callHierarchy/incomingCalls` / `outgoingCalls` / `prepareTypeHierarchy` / `typeHierarchy/supertypes` /
//! `subtypes`。#1685）は `--xref <file>` か `TAKO_LSP_FAKE_XREF` の規則で答える。**渡したときだけ**
//! `referencesProvider` / `callHierarchyProvider` / `typeHierarchyProvider` を申告する（渡さない既存の
//! テストの能力の組は変わらない）。形は定義ジャンプの規則と同じく**上から順に最初に当たった 1 つ**で、
//! `method` / `uri_suffix` / `line`（位置で問う要求の 0 起点の行）/ `item_name`（展開の要求の
//! `item.name`）で絞り、`result`（そのまま返す）/ `silent` / `crash` / `generate`（参照の要求に
//! 問われた文書の 0..N 行目の `Location` を N 件作って返す = 巨大な結果）を持つ。
//! `loading` の読み込み中は参照と prepare に空（`[]`）で答える（定義ジャンプと同じ）。
//! 当たる規則が無ければ `null` で答える。
//!
//! 名前の変更（`textDocument/prepareRename` / `textDocument/rename`。#1686）は `--rename <file>` か
//! `TAKO_LSP_FAKE_RENAME` の規則で答える。**渡したときだけ** `renameProvider: { prepareProvider: true }` を
//! 申告する（`prepareProvider` の無い形は `--providers` で差し替える）。形は定義ジャンプの規則と同じく
//! **上から順に最初に当たった 1 つ**（`method` / `uri_suffix` / `line`）で、`result`（そのまま返す。`null` も可）/
//! `error`（`{ code, message }`）/ `silent` / `crash` を持つ。当たる規則が無い・どれも無ければ**既定の答え**:
//! prepare は問われた位置の語（英数字と `_`）の範囲と `placeholder`（語の上でなければ `null`）、rename は
//! 新しい名前が `[A-Za-z_][A-Za-z0-9_]*` でなければ -32602（rust-analyzer の文の形）、そうでなければ
//! 本文の模型（`--disk-model` ならディスクの模型も）でその語が**語として**現れる所すべてを書き換える
//! `WorkspaceEdit`（既定は `documentChanges`。規則の `shape: "changes"` で `changes` の形、
//! `resource_op: true` でファイルの名前の変更を 1 つ足す）。
//!
//! ## tako の外で変わったファイル（#2034）
//!
//! `--watchers <file>`（`TAKO_LSP_FAKE_WATCHERS`。`FileSystemWatcher` の JSON 配列。字面の `{root}` は
//! initialize の `rootUri` に置き換える）を渡すと、`initialized` を受けた後に `client/registerCapability`
//! （id `tako-fake-watch`）でその監視を登録する。`--unwatch-on <接尾辞>`（`TAKO_LSP_FAKE_UNWATCH_ON`）を
//! 渡すと、`workspace/didChangeWatchedFiles` の URI がその接尾辞で終わったら `client/unregisterCapability`
//! で外す（解除の後に知らせが来ないことを見る）。
//!
//! `--disk-model`（`TAKO_LSP_FAKE_DISK_MODEL=1`）を渡すと、**実の rust-analyzer と同じ持ち方**をする:
//! `initialized` の時点で根の配下のファイル（500 個・1 つ 1 MB まで）を読み、その後は
//! `workspace/didChangeWatchedFiles` を受けたファイルだけを読み直す（種別 3 は捨てる）。開いている文書は
//! 本文の模型（下記）が勝つ。参照（`--xref` の規則の `find: <語>`）はこの模型の全ファイルでその語
//! （前後が英数字と `_` でない所）が現れる所を、定義（`--goto` の規則の `find: <字面>`）は URI 順で最初に
//! 現れる所を返す = 知らせが届かなければ、外で書き換えた後も古い行番号で答える（#2034 の症状）。
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
//!
//! ## 読み込みの終わりと順序の注入（#1922）
//!
//! `--loading-until <file>`（`TAKO_LSP_FAKE_LOADING_UNTIL`）を渡すと、`loading` の読み込みは
//! `--loading-ms` が過ぎたうえで**そのファイルができるまで**終わらない（テストが「要求が届いた」を
//! 確かめてから作る = 読み込みの終わりを実時間で決めない）。
//!
//! 次の 2 つは CI の遅い機で入れ替わる順序を、どの機でも起こすための遅延:
//!
//! - `--start-delay-ms <ms>`（`TAKO_LSP_FAKE_START_DELAY_MS`）: 起動してから stdin を読み始めるまで
//!   待つ（Windows のランナーで起動が上限の 1 秒を越えた形）
//! - `--status-delay-ms <ms>`（`TAKO_LSP_FAKE_STATUS_DELAY_MS`）: `loading` が `initialized` を受けて
//!   から `quiescent: false` を送るまで待つ（tako が知らせを受け取る前に要求を送った形）。
//!   待つあいだは次のメッセージを読まないので、知らせ → didOpen の診断の順は変わらない
//!
//! ## 答えの合図と順序の注入（#1930）
//!
//! 答えを遅らせる口は**実時間を持たない**: 補完・ホバー・整形の規則の `hold_until`（上記）は、
//! テストが「次の操作が済んだ」を確かめてから合図のファイルを作る（`delay_ms` の実時間の間に次の
//! 操作が間に合う前提を置かない。遅い機では順序が入れ替わる）。
//!
//! 次の 3 つは CI の遅い機で入れ替わる順序を、どの機でも起こすための遅延・入れ替え:
//!
//! - `--empty-delay-ms <ms>`（`TAKO_LSP_FAKE_EMPTY_DELAY_MS`）: `loading` の読み込み中に即座に返す
//!   空の答え（定義ジャンプの `[]`・補完とホバーの `null`）を送る前に待つ（テストが要求の届いたのを
//!   見て読み込みを終えると、manager が空を見る前に済む形）
//! - `--settle-on-empty <before|after>`（`TAKO_LSP_FAKE_SETTLE_ON_EMPTY`）: その空の答えの直前
//!   （`before`）/ 直後（`after`）に、合図と無関係に読み込みを終える（`quiescent: true`）。
//!   `before` は「読み込み中に受けた要求の空を、済んだ後に送る」= manager は空を受けた時点で
//!   必ず済んだと知っている（#1930 の問い直しを決定的に通す）
//! - `--log-delay-ms <ms>`（`TAKO_LSP_FAKE_LOG_DELAY_MS`）: 受けたメッセージを `--log` へ書く前に
//!   待つ（manager が「送った」ものがログに「届いた」と載るのが遅れる = #1930 の 1684 の形）
//!
//! ## 診断の遅れの注入（#2312）
//!
//! - `--publish-delay-ms <ms>`（`TAKO_LSP_FAKE_PUBLISH_DELAY_MS`）: didOpen / didChange を `--log`
//!   へ書いた**後**、それへの診断を送る前に待つ（テストがログで didOpen を見た時点では、診断が
//!   まだ tako に届いていない形。CI の遅い機で `issue1678_lsp_e2e.rs` のアイドルの検査が受信数
//!   1 → 2 で落ちた順序を、どの機でも起こす）
//!
//! `TAKO_LSP_FAKE_ARGV_LOG=<ファイル>`（#1951）: 起動のたびに自分の起動引数（0 番 = 実行ファイルを含む）を
//! 1 行 1 JSON の配列で追記する。知らない引数は読み飛ばすので、検出表の行が組んだ引数（`-cli <パス>` 等）を
//! そのまま受けて、何が並んだかを e2e が読める（env だけで指定する = 起動引数を汚さない）

use std::io::{BufRead, BufReader, Write};

fn arg_or_env(args: &[String], flag: &str, env: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var(env).ok().filter(|v| !v.is_empty()))
}

/// 1 行を追記する。**本文と改行を 1 回の書き込みで**（#1951）: `writeln!` は書式の部品ごとに書く
/// （本文・改行の 2 回）ので、同じログを 2 つの偽サーバが同時に書く（器が 2 つ = shutdown を同時に受ける）と
/// 「本文 A・本文 B・改行・改行」と混ざってどちらの行も読めなくなる（Windows の CI で shutdown が 0 件と
/// 数えられた）。追記モードの 1 回の書き込みはまとめて末尾へ付く
fn append(path: &Option<String>, line: &str) {
    if let Some(path) = path {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(format!("{line}\n").as_bytes());
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

/// 参照検索・呼び出し階層・型階層の method（#1685）
const XREF_METHODS: [&str; 7] = [
    "textDocument/references",
    "textDocument/prepareCallHierarchy",
    "callHierarchy/incomingCalls",
    "callHierarchy/outgoingCalls",
    "textDocument/prepareTypeHierarchy",
    "typeHierarchy/supertypes",
    "typeHierarchy/subtypes",
];

/// 名前の変更の method（#1686）
const RENAME_METHODS: [&str; 2] = ["textDocument/prepareRename", "textDocument/rename"];

/// 名前の変更の既定の答え（#1686。冒頭の説明）。`texts` は本文の模型（とディスクの模型）
fn rename_answer(
    texts: &std::collections::BTreeMap<String, String>,
    breaks: Breaks,
    method: &str,
    params: &serde_json::Value,
    rule: Option<&serde_json::Value>,
) -> Result<serde_json::Value, serde_json::Value> {
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let text = texts.get(uri).cloned().unwrap_or_default();
    let line = params["position"]["line"].as_u64().unwrap_or(0) as usize;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as usize;
    let Some((a, b)) = word_at(&text, offset_at(&text, breaks, line, character)) else {
        return Ok(serde_json::Value::Null);
    };
    let word = text[a..b].to_string();
    if method == RENAME_METHODS[0] {
        return Ok(serde_json::json!({
            "range": range_json(&text, breaks, a, b),
            "placeholder": word,
        }));
    }
    let new_name = params["newName"].as_str().unwrap_or("");
    let identifier = new_name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && new_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !identifier {
        return Err(serde_json::json!({
            "code": -32602,
            "message": format!("Invalid name `{new_name}`: not an identifier"),
        }));
    }
    let mut per_uri: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    for location in find_locations(texts, breaks, &word, true) {
        let uri = location["uri"].as_str().unwrap_or("").to_string();
        let edit = serde_json::json!({ "range": location["range"], "newText": new_name });
        match per_uri.iter_mut().find(|(u, _)| *u == uri) {
            Some((_, edits)) => edits.push(edit),
            None => per_uri.push((uri, vec![edit])),
        }
    }
    let rule = rule.cloned().unwrap_or(serde_json::Value::Null);
    if rule["shape"].as_str() == Some("changes") {
        let changes: serde_json::Map<String, serde_json::Value> = per_uri
            .into_iter()
            .map(|(uri, edits)| (uri, serde_json::Value::Array(edits)))
            .collect();
        return Ok(serde_json::json!({ "changes": changes }));
    }
    let mut document_changes: Vec<serde_json::Value> = per_uri
        .into_iter()
        .map(|(uri, edits)| {
            serde_json::json!({ "textDocument": { "uri": uri, "version": null }, "edits": edits })
        })
        .collect();
    if rule["resource_op"].as_bool() == Some(true) {
        document_changes.push(serde_json::json!({
            "kind": "rename",
            "oldUri": uri,
            "newUri": format!("{uri}.renamed"),
        }));
    }
    Ok(serde_json::json!({ "documentChanges": document_changes }))
}

/// 参照検索・階層の能力のキー（`--xref` を渡したときだけ申告する。#1685）
const XREF_PROVIDERS: [&str; 3] = [
    "referencesProvider",
    "callHierarchyProvider",
    "typeHierarchyProvider",
];

/// 規則のうち、この参照・階層の要求に最初に当たるもの（#1685）
fn xref_rule<'a>(
    rules: &'a [serde_json::Value],
    method: &str,
    params: &serde_json::Value,
) -> Option<&'a serde_json::Value> {
    let uri = params["textDocument"]["uri"].as_str();
    let line = params["position"]["line"].as_u64();
    let name = params["item"]["name"].as_str();
    rules.iter().find(|rule| {
        rule["method"].as_str().is_none_or(|m| m == method)
            && rule["uri_suffix"]
                .as_str()
                .is_none_or(|s| uri.is_some_and(|u| u.ends_with(s)))
            && rule["line"].as_u64().is_none_or(|l| Some(l) == line)
            && rule["item_name"].as_str().is_none_or(|n| Some(n) == name)
    })
}

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
    // 切り出し安全: offset は offset_at / word_at / match_indices の位置で、end は行末 = 文字境界
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

/// ディスクの模型（#2034。冒頭の説明）: 根の URI からの相対パス → 中身
struct Disk {
    root_uri: String,
    root: std::path::PathBuf,
    files: std::collections::BTreeMap<String, String>,
}

/// `file://` URI → パス（tako の実装を使わずにここで組む。Windows の `/C:/` の先頭の `/` は落とす）
fn path_of_file_uri(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            // バイト列から読む（`&rest[…]` で切ると `%` の直後の多バイト文字の途中で panic する。#1790）
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let text = String::from_utf8(out).ok()?;
    let b = text.as_bytes();
    let text = if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        // 切り出し安全: 先頭は直前で確かめた ASCII の `/`
        text[1..].to_string()
    } else {
        text
    };
    Some(std::path::PathBuf::from(text))
}

impl Disk {
    const MAX_FILES: usize = 500;
    const MAX_BYTES: u64 = 1 << 20;

    /// 根の配下を読む（`initialized` の時点 = 実サーバのワークスペースの読み込み）
    fn load(root_uri: &str) -> Option<Self> {
        let root = path_of_file_uri(root_uri)?;
        let mut disk = Self {
            root_uri: root_uri.trim_end_matches('/').to_string(),
            root: root.clone(),
            files: Default::default(),
        };
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if disk.files.len() < Self::MAX_FILES {
                    if let Some(rel) = disk.rel_of_path(&path) {
                        disk.read(&rel);
                    }
                }
            }
        }
        Some(disk)
    }

    fn rel_of_path(&self, path: &std::path::Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }

    fn read(&mut self, rel: &str) {
        let path = rel
            .split('/')
            .fold(self.root.clone(), |p, part| p.join(part));
        let small =
            std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= Self::MAX_BYTES);
        match std::fs::read_to_string(&path) {
            Ok(text) if small => {
                self.files.insert(rel.to_string(), text);
            }
            _ => {
                self.files.remove(rel);
            }
        }
    }

    /// 知らせ 1 件を当てる（根の外・読めない URI は捨てる）
    fn apply(&mut self, uri: &str, kind: u64) {
        let Some(rel) = uri
            .strip_prefix(self.root_uri.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
        else {
            return;
        };
        if kind == 3 {
            self.files.remove(rel);
        } else {
            self.read(rel);
        }
    }

    /// 模型の全ファイル（URI → 中身）。開いている文書は本文の模型が勝つ
    fn texts(
        &self,
        docs: &std::collections::HashMap<String, String>,
    ) -> std::collections::BTreeMap<String, String> {
        let mut all: std::collections::BTreeMap<String, String> = self
            .files
            .iter()
            .map(|(rel, text)| (format!("{}/{rel}", self.root_uri), text.clone()))
            .collect();
        for (uri, text) in docs {
            all.insert(uri.clone(), text.clone());
        }
        all
    }
}

/// `find` の語が現れる所（前後が英数字と `_` でない所）を `Location` で（#2034）
fn find_locations(
    texts: &std::collections::BTreeMap<String, String>,
    breaks: Breaks,
    word: &str,
    whole_word: bool,
) -> Vec<serde_json::Value> {
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out = Vec::new();
    for (uri, text) in texts {
        for (at, _) in text.match_indices(word) {
            let end = at + word.len();
            if whole_word
                && (is_word(text[..at].chars().next_back()) || is_word(text[end..].chars().next()))
            {
                continue;
            }
            out.push(serde_json::json!({ "uri": uri, "range": range_json(text, breaks, at, end) }));
        }
    }
    out
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

/// 注入の遅れ（0 なら何もしない = 渡さないテストのスケジューリングを変えない）
fn pause(delay: std::time::Duration) {
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
}

/// 答えを待たせる表（まだ答えていない補完・ホバーの要求の id。取り消されたら外れる）
type Waiting = std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>;

/// `path` ができるまで待つ（`still` が偽になったら待たずに抜ける）。テストが合図を作る（#1930）
fn wait_file(path: &str, still: impl Fn() -> bool) {
    while !std::path::Path::new(path).exists() && still() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// `hold_until` の合図のファイル。文字列ならそれ、配列なら**その method の `seq` 本目**（0 起点）の
/// 要求が `seq` 番目を待つ（足りなければ最後。要求を 1 本ずつ返させる = #1930）
fn hold_path(hold_until: &serde_json::Value, seq: usize) -> Option<String> {
    match hold_until {
        serde_json::Value::Array(paths) => paths.get(seq).or(paths.last())?.as_str(),
        other => other.as_str(),
    }
    .map(str::to_string)
}

/// 補完・ホバーの答え: `hold_until` が無ければすぐ、あればその合図まで待たせて答える（#1930）。
/// 待つあいだに `$/cancelRequest` が来たら答えない（取り消しの腕が -32800 で答えて表から外す）
fn hold_answer(
    out: &std::sync::Arc<Out>,
    waiting: &Waiting,
    hold: Option<String>,
    id: serde_json::Value,
    result: serde_json::Value,
) {
    let Some(open) = hold else {
        out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        return;
    };
    let key = id.to_string();
    waiting.lock().unwrap().insert(key.clone());
    let (out, waiting) = (out.clone(), waiting.clone());
    std::thread::spawn(move || {
        wait_file(&open, || waiting.lock().unwrap().contains(&key));
        if waiting.lock().unwrap().remove(&key) {
            out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    });
}

/// `quiet-loading` の読み込み中に開かれた文書（済んだら診断を送る。#1977）
type Pending = std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

/// didOpen へ返す既定の診断 1 件（#1678）
fn fake_diagnostics(out: &Out, uri: serde_json::Value) {
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

/// `loading` / `quiet-loading` の読み込みを終える（合図と `--settle-on-empty` のどちらが先でも
/// 1 回だけ）。`loading` は `quiescent: true` を送り、`quiet-loading`（pyright 型。#1977）は
/// 状態を送らず、読み込み中に開かれた文書の診断を送る（pyright は索引ができた頃に最初の診断を送る）
fn finish_loading(
    out: &Out,
    loaded: &std::sync::atomic::AtomicBool,
    quiet: bool,
    pending: &Pending,
) {
    // didOpen の取り込みとすれ違わない（済んだ印と溜めた文書を同じ錠の中で見る）
    let mut held = pending.lock().unwrap();
    if loaded.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if quiet {
        for uri in held.drain(..) {
            fake_diagnostics(out, uri);
        }
        return;
    }
    out.send(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "health": "ok", "quiescent": true },
    }));
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
    let waiting: Waiting = Default::default();
    // #1930: 読み込みの後に答える補完・ホバーの要求の通し番号（`hold_until` の配列の何番目を待つか）
    let (mut completion_seq, mut hover_seq) = (0usize, 0usize);
    // #1681: ホバーの規則
    let hover_rules: Vec<serde_json::Value> = arg_or_env(&args, "--hover", "TAKO_LSP_FAKE_HOVER")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    // #1685: 参照検索・階層の規則（渡したときだけ能力を申告する）
    let xref_rules: Option<Vec<serde_json::Value>> =
        arg_or_env(&args, "--xref", "TAKO_LSP_FAKE_XREF")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok());
    // #1686: 名前の変更の規則（渡したときだけ能力を申告する）
    let rename_rules: Option<Vec<serde_json::Value>> =
        arg_or_env(&args, "--rename", "TAKO_LSP_FAKE_RENAME")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok());
    let doc_log = arg_or_env(&args, "--doc-log", "TAKO_LSP_FAKE_DOC_LOG");
    // #2034: 監視の登録・解除の合図・ディスクの模型（冒頭の説明）
    let watchers_text: Option<String> = arg_or_env(&args, "--watchers", "TAKO_LSP_FAKE_WATCHERS")
        .and_then(|path| std::fs::read_to_string(path).ok());
    let unwatch_on = arg_or_env(&args, "--unwatch-on", "TAKO_LSP_FAKE_UNWATCH_ON");
    let disk_model = args.iter().any(|a| a == "--disk-model")
        || matches!(
            std::env::var("TAKO_LSP_FAKE_DISK_MODEL").ok().as_deref(),
            Some("1")
        );
    let mut root_uri = String::new();
    let mut disk: Option<Disk> = None;
    let mut docs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    append(&spawns, &std::process::id().to_string());
    append(
        &std::env::var("TAKO_LSP_FAKE_ARGV_LOG")
            .ok()
            .filter(|v| !v.is_empty()),
        &serde_json::Value::from(args.clone()).to_string(),
    );
    eprintln!("tako-lsp-fake: scenario={scenario}");
    if scenario == "die" {
        // 起動直後に即死する（initialize を 1 通も読まない）
        std::process::exit(2);
    }

    let out = std::sync::Arc::new(Out {
        split: scenario == "split-header",
    });
    // `loading` / `quiet-loading`: 読み込みが済んだか（済むまでは定義ジャンプに空で答える）
    let quiet = scenario == "quiet-loading";
    let loaded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        scenario != "loading" && !quiet,
    ));
    // #1977: `quiet-loading` の読み込み中に開かれた文書（済んだら診断を送る）
    let pending: Pending = Default::default();
    let loading_ms: u64 = arg_or_env(&args, "--loading-ms", "TAKO_LSP_FAKE_LOADING_MS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(800);
    // #1922: 読み込みを終わらせる合図のファイルと、順序を入れ替える遅延（冒頭の説明）
    let loading_until = arg_or_env(&args, "--loading-until", "TAKO_LSP_FAKE_LOADING_UNTIL");
    let delay_of = |flag: &str, env: &str| {
        std::time::Duration::from_millis(
            arg_or_env(&args, flag, env)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        )
    };
    let status_delay = delay_of("--status-delay-ms", "TAKO_LSP_FAKE_STATUS_DELAY_MS");
    // #1930: 答えの順序を入れ替える注入（冒頭の説明）
    let empty_delay = delay_of("--empty-delay-ms", "TAKO_LSP_FAKE_EMPTY_DELAY_MS");
    let log_delay = delay_of("--log-delay-ms", "TAKO_LSP_FAKE_LOG_DELAY_MS");
    // #2312: didOpen / didChange をログへ書いた後、診断を送るまでの遅延（冒頭の説明）
    let publish_delay = delay_of("--publish-delay-ms", "TAKO_LSP_FAKE_PUBLISH_DELAY_MS");
    let settle_on_empty = arg_or_env(&args, "--settle-on-empty", "TAKO_LSP_FAKE_SETTLE_ON_EMPTY");
    let settle_before = settle_on_empty.as_deref() == Some("before");
    let settle_after = settle_on_empty.as_deref() == Some("after");
    std::thread::sleep(delay_of("--start-delay-ms", "TAKO_LSP_FAKE_START_DELAY_MS"));
    // #1684: 申告する能力の組をそのまま差し替える（シナリオの能力は捨てる）
    let providers: Option<serde_json::Map<String, serde_json::Value>> =
        arg_or_env(&args, "--providers", "TAKO_LSP_FAKE_PROVIDERS")
            .and_then(|v| match v.strip_prefix('@') {
                Some(file) => std::fs::read_to_string(file).ok(),
                None => Some(v),
            })
            .and_then(|v| serde_json::from_str(&v).ok());
    let mut input = BufReader::new(std::io::stdin());
    // #1930: 読み込み中の前半の空の答え（定義ジャンプ・補完・ホバーの 3 つが通る 1 実装）
    let early_empty = |id: serde_json::Value, empty: serde_json::Value| {
        pause(empty_delay);
        if settle_before {
            finish_loading(&out, &loaded, quiet, &pending);
        }
        out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": empty }));
        if settle_after {
            finish_loading(&out, &loaded, quiet, &pending);
        }
    };
    while let Some(message) = read_one(&mut input) {
        pause(log_delay);
        append(&log, &message.to_string());
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = message.get("id").cloned();
        if matches!(method, "textDocument/didOpen" | "textDocument/didChange") {
            pause(publish_delay);
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
        if method == "initialize" {
            root_uri = message["params"]["rootUri"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        if method == "initialized" {
            if disk_model {
                disk = Disk::load(&root_uri);
            }
            if let Some(text) = &watchers_text {
                let watchers: serde_json::Value =
                    serde_json::from_str(&text.replace("{root}", &root_uri))
                        .unwrap_or(serde_json::Value::Null);
                out.send(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "watch-register",
                    "method": "client/registerCapability",
                    "params": { "registrations": [{
                        "id": "tako-fake-watch",
                        "method": "workspace/didChangeWatchedFiles",
                        "registerOptions": { "watchers": watchers },
                    }]},
                }));
            }
        }
        if method == "workspace/didChangeWatchedFiles" {
            let changes = message["params"]["changes"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for change in &changes {
                let uri = change["uri"].as_str().unwrap_or("");
                if let Some(disk) = disk.as_mut() {
                    disk.apply(uri, change["type"].as_u64().unwrap_or(2));
                }
                if unwatch_on
                    .as_deref()
                    .is_some_and(|suffix| uri.ends_with(suffix))
                {
                    out.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": "watch-unregister",
                        "method": "client/unregisterCapability",
                        "params": { "unregisterations": [{
                            "id": "tako-fake-watch",
                            "method": "workspace/didChangeWatchedFiles",
                        }]},
                    }));
                }
            }
            continue;
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
                if xref_rules.is_some() {
                    for key in XREF_PROVIDERS {
                        capabilities[key] = serde_json::json!(true);
                    }
                }
                if rename_rules.is_some() {
                    capabilities["renameProvider"] = serde_json::json!({ "prepareProvider": true });
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
                    std::thread::sleep(status_delay);
                    out.send(serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "experimental/serverStatus",
                        "params": { "health": "ok", "quiescent": false },
                    }));
                    let (out, loaded, until, pending) = (
                        out.clone(),
                        loaded.clone(),
                        loading_until.clone(),
                        pending.clone(),
                    );
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(loading_ms));
                        while until
                            .as_deref()
                            .is_some_and(|path| !std::path::Path::new(path).exists())
                        {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        finish_loading(&out, &loaded, quiet, &pending);
                    });
                }
                // #1977: pyright 型。状態を 1 度も送らず、読み込み（合図まで）が済んだら溜めた診断を送る
                "quiet-loading" => {
                    let (out, loaded, until, pending) = (
                        out.clone(),
                        loaded.clone(),
                        loading_until.clone(),
                        pending.clone(),
                    );
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(loading_ms));
                        while until
                            .as_deref()
                            .is_some_and(|path| !std::path::Path::new(path).exists())
                        {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        finish_loading(&out, &loaded, quiet, &pending);
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
                if quiet {
                    // #1977: 読み込み中は診断を送らずに溜める（済んだら `finish_loading` が送る）
                    let mut held = pending.lock().unwrap();
                    if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                        held.push(uri);
                        continue;
                    }
                }
                fake_diagnostics(&out, uri);
            }
            (method, Some(id)) if GOTO_METHODS.contains(&method) => {
                if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                    early_empty(id, serde_json::json!([]));
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
                if let Some(needle) = rule["find"].as_str() {
                    // #2034: ディスクの模型 + 開いている文書で URI 順に最初に現れる所
                    let texts = disk.as_ref().map(|d| d.texts(&docs)).unwrap_or_default();
                    let result = find_locations(&texts, breaks, needle, false)
                        .into_iter()
                        .next()
                        .unwrap_or(serde_json::Value::Null);
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
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
            (method, Some(id)) if XREF_METHODS.contains(&method) => {
                let params = &message["params"];
                // 読み込み中は位置で問う要求（参照・prepare）に空で答える（定義ジャンプと同じ）
                if !loaded.load(std::sync::atomic::Ordering::SeqCst)
                    && params.get("position").is_some()
                {
                    early_empty(id, serde_json::json!([]));
                    continue;
                }
                let rules = xref_rules.as_deref().unwrap_or_default();
                let Some(rule) = xref_rule(rules, method, params) else {
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                    continue;
                };
                if rule["crash"].as_bool() == Some(true) {
                    std::process::exit(6);
                }
                if rule["silent"].as_bool() == Some(true) {
                    continue;
                }
                if let Some(word) = rule["find"].as_str() {
                    // #2034: ディスクの模型 + 開いている文書でその語が現れる所すべて
                    let texts = disk.as_ref().map(|d| d.texts(&docs)).unwrap_or_default();
                    let result =
                        serde_json::Value::Array(find_locations(&texts, breaks, word, true));
                    out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                    continue;
                }
                let result = match rule["generate"].as_u64() {
                    // 巨大な結果: 問われた文書の 0..N 行目を指す Location を N 件
                    Some(n) => serde_json::Value::Array(
                        (0..n)
                            .map(|line| {
                                serde_json::json!({
                                    "uri": params["textDocument"]["uri"].clone(),
                                    "range": {
                                        "start": { "line": line, "character": 0 },
                                        "end": { "line": line, "character": 1 },
                                    },
                                })
                            })
                            .collect(),
                    ),
                    None => rule
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                };
                out.send(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
            // #1686: 名前の変更（読み込み中の空の答えは持たない = 既定の答えは本文の模型から作る）
            (method, Some(id)) if RENAME_METHODS.contains(&method) => {
                let params = &message["params"];
                let rules = rename_rules.as_deref().unwrap_or_default();
                let rule = goto_rule(rules, method, params);
                if let Some(rule) = rule {
                    if rule["crash"].as_bool() == Some(true) {
                        std::process::exit(7);
                    }
                    if rule["silent"].as_bool() == Some(true) {
                        continue;
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
                let texts = match disk.as_ref() {
                    Some(disk) => disk.texts(&docs),
                    None => docs.iter().map(|(u, t)| (u.clone(), t.clone())).collect(),
                };
                let reply = match rename_answer(&texts, breaks, method, params, rule) {
                    Ok(result) => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
                    }
                    Err(error) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                };
                out.send(reply);
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
                    if let Some(open) = hold_path(&rule["hold_until"], 0) {
                        wait_file(&open, || true);
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
            // #1682: 補完。`hold_until` の合図を待つあいだに取り消されたら -32800 で答える（LSP の作法 =
            // 取り消されても応答は返す）。待ちの表は答えた / 取り消した時点で外す（#1930）
            ("textDocument/completion", Some(id)) => {
                let result = completion_items(&completion, &docs, breaks, &message["params"]);
                // #1869: 読み込み中（実測した rust-analyzer 1.95 の 2 段）。前半は即座に `null`、
                // `hold_while_loading` なら答えずに済むまで待たせ、済んだ直後に答える
                if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                    if completion["hold_while_loading"].as_bool() != Some(true) {
                        // #1977: pyright 型は索引ができるまで空の一覧で答える（実測した 1.1.414 の形）
                        let empty = if quiet {
                            serde_json::json!({ "isIncomplete": true, "items": [] })
                        } else {
                            serde_json::Value::Null
                        };
                        early_empty(id, empty);
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
                let hold = hold_path(&completion["hold_until"], completion_seq);
                completion_seq += 1;
                hold_answer(&out, &waiting, hold, id, result);
            }
            // #1681: ホバー。`hold_until` の合図を待つあいだに取り消されたら -32800 で答える（補完と同じ作法）
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
                // #1893: 読み込み中（`loading`）は補完と同じ 2 段。前半は即座に `null`、規則が
                // `hold_while_loading` なら答えずに済むまで待たせ（取り消されたら -32800）、済んだ直後に答える
                if !loaded.load(std::sync::atomic::Ordering::SeqCst) {
                    if rule["hold_while_loading"].as_bool() != Some(true) {
                        early_empty(id, serde_json::Value::Null);
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
                let hold = hold_path(&rule["hold_until"], hover_seq);
                hover_seq += 1;
                hold_answer(&out, &waiting, hold, id, result);
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

#[cfg(test)]
mod tests {
    use super::path_of_file_uri;

    /// #1790: `%` の直後が多バイト文字の URI でも落ちない（修正前は `&rest[i + 1..i + 3]` が
    /// 「あ」の途中を指して panic した）。復号できない `%` はそのまま残す
    #[test]
    fn percentの直後が多バイト文字でも落ちない() {
        assert_eq!(
            path_of_file_uri("file:///tmp/%あ.rs"),
            Some(std::path::PathBuf::from("/tmp/%あ.rs"))
        );
        assert_eq!(
            path_of_file_uri("file:///tmp/a%e3%81%82.rs"),
            Some(std::path::PathBuf::from("/tmp/aあ.rs"))
        );
        // 末尾の `%` / 16 進でない 2 文字は復号しない（境界）
        assert_eq!(
            path_of_file_uri("file:///tmp/x%"),
            Some(std::path::PathBuf::from("/tmp/x%"))
        );
        assert_eq!(
            path_of_file_uri("file:///tmp/%zz"),
            Some(std::path::PathBuf::from("/tmp/%zz"))
        );
    }
}
