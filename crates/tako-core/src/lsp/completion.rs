//! 補完（予測変換。S5 / #1682）の純粋部分
//!
//! プロセスも I/O も持たない部分だけをここに置く:
//!
//! - 応答の読み取り（`CompletionItem[]` / `CompletionList` / `null`）と tako の座標への写し
//!   （[`parse_response`] → [`locate`]）
//! - スニペットの平文化（[`snippet_to_plain`]。tako はスニペットを申告しないが、申告を無視して
//!   送ってくるサーバのために最低限 = 跳び先の印を落として既定の文字だけを残す）
//! - 入力中の語の頭（[`word_start`]）と、候補の絞り込み・並べ替え（[`rank`]）
//! - 件数の上限（[`truncate`]。CLI / MCP の `--limit`）
//! - 打鍵の振り分け表（[`route_key`] = 検索バーの有無 × 補完の有無 × 5 キー）
//! - 古い応答を捨てる版の照合（[`Session`]）
//! - 打鍵が補完を起こすか（[`trigger_for`]）
//!
//! 問い合わせ（デバウンス明けの要求・`$/cancelRequest`）は `tako_control::lsp::manager`、
//! 候補の一覧の描画は tako-app（`lsp_completion_ui.rs`）。**言語名はここに書かない**
//! （検出表だけが持つ = #1678 の番犬）。

use serde_json::Value;

use super::position::LineIndex;

/// 1 回の応答で受け取る候補の上限（壊れた / 巨大な応答で UI のメモリを食い尽くさない）。
/// 超えたぶんは [`Parsed::dropped`] に数える
pub const MAX_ITEMS: usize = 5000;

/// CLI / MCP が返す件数の既定（`--limit` の省略時）。何も付けずに読める件数にしておく
/// （#322 の最簡形。全件が要るときだけ `--limit` を大きくする）
pub const DEFAULT_LIMIT: usize = 50;

/// 候補ごとに持つ説明（documentation）の上限の文字数
pub const DOC_CHARS: usize = 2000;

/// tako の座標（0 起点の行・行内の UTF-8 バイト。行は `\n` 区切り）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct At {
    pub line: usize,
    pub col: usize,
}

impl At {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// 追加の編集 1 件（自動 import 等。tako の座標）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: At,
    pub end: At,
    pub text: String,
}

/// 候補 1 つ（tako の座標へ写したもの）
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    /// LSP の `CompletionItemKind`（1〜25。名前は [`kind_slug`]）
    pub kind: Option<u32>,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub preselect: bool,
    pub deprecated: bool,
    /// 入れる本文（`textEdit.newText` > `insertText` > `label`。スニペットは平文化済み）
    pub insert_text: String,
    /// 置き換えの始点。終点は**確定したときのカーソル** + [`Self::tail`]
    /// （一覧を出したまま打ち足した分も一緒に置き換わる）
    pub start: At,
    /// カーソルより後ろも置き換えるバイト数（`textEdit` の範囲がカーソルの後ろまで伸びている
    /// とき。同じ行の中だけ）。打鍵はカーソルへ入るので、後ろの本文は確定まで変わらない
    pub tail: usize,
    /// 追加の編集（`additionalTextEdits`。自動 import 等）
    pub additional: Vec<Edit>,
    /// `completionItem/resolve` へ渡す元の JSON（`data` を含む）
    pub raw: Value,
}

/// 応答の 1 候補（LSP の座標のまま = 0 起点の行・UTF-16 の桁）
#[derive(Debug, Clone, PartialEq)]
pub struct RawItem {
    pub label: String,
    pub kind: Option<u32>,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub preselect: bool,
    pub deprecated: bool,
    pub insert_text: String,
    /// `textEdit` の範囲（`InsertReplaceEdit` は `insert` 側 = VS Code / Zed の既定）
    pub range: Option<LspRange>,
    pub additional: Vec<(LspRange, String)>,
    pub raw: Value,
}

/// LSP の範囲（`((行, UTF-16 桁), (行, UTF-16 桁))`）
pub type LspRange = ((usize, usize), (usize, usize));

/// 応答を読んだもの
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Parsed {
    pub items: Vec<RawItem>,
    /// `CompletionList.isIncomplete`（打ち足すたびに問い直すべき一覧）
    pub is_incomplete: bool,
    /// [`MAX_ITEMS`] を超えて捨てた数
    pub dropped: usize,
}

fn lsp_point(v: &Value) -> Option<(usize, usize)> {
    Some((
        usize::try_from(v.get("line")?.as_u64()?).ok()?,
        usize::try_from(v.get("character")?.as_u64()?).ok()?,
    ))
}

fn lsp_range(v: &Value) -> Option<LspRange> {
    Some((lsp_point(v.get("start")?)?, lsp_point(v.get("end")?)?))
}

/// 置き換えの範囲。`TextEdit`（`range`）/ `InsertReplaceEdit`（`insert` と `replace`）/
/// `itemDefaults.editRange`（`Range` そのものか `{ insert, replace }`）を受ける。
/// `insert` / `replace` の組は `insert` を採る（カーソルより後ろの語を消さない = VS Code / Zed の既定）
fn edit_range(v: &Value) -> Option<LspRange> {
    v.get("range")
        .and_then(lsp_range)
        .or_else(|| lsp_range(v))
        .or_else(|| v.get("insert").and_then(lsp_range))
}

/// `documentation`（文字列か `MarkupContent`）の本文。[`DOC_CHARS`] 字で切る
fn documentation_text(v: Option<&Value>) -> Option<String> {
    let text = match v? {
        Value::String(s) => s.as_str(),
        Value::Object(o) => o.get("value")?.as_str()?,
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(DOC_CHARS).collect())
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `insertTextFormat` の 2 = Snippet
const SNIPPET_FORMAT: u64 = 2;

/// 応答を 1 つの形へ。`CompletionItem[]` / `CompletionList`（`itemDefaults` の
/// `editRange` / `insertTextFormat` を含む）/ `null` を受ける。`label` の無い候補は捨てる
pub fn parse_response(value: &Value) -> Parsed {
    let (items, is_incomplete, defaults) = match value {
        Value::Array(items) => (items.as_slice(), false, None),
        Value::Object(o) => (
            o.get("items")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice),
            o.get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            o.get("itemDefaults"),
        ),
        _ => (&[][..], false, None),
    };
    let mut out = Parsed {
        is_incomplete,
        ..Parsed::default()
    };
    for item in items {
        if out.items.len() >= MAX_ITEMS {
            out.dropped += 1;
            continue;
        }
        if let Some(item) = parse_item(item, defaults) {
            out.items.push(item);
        }
    }
    out
}

fn parse_item(v: &Value, defaults: Option<&Value>) -> Option<RawItem> {
    let label = v.get("label")?.as_str()?.to_string();
    let default = |key: &str| defaults.and_then(|d| d.get(key));
    let snippet = v
        .get("insertTextFormat")
        .or_else(|| default("insertTextFormat"))
        .and_then(Value::as_u64)
        == Some(SNIPPET_FORMAT);
    let (range, new_text) = match v.get("textEdit") {
        Some(edit) => (edit_range(edit), str_field(edit, "newText")),
        None => (
            default("editRange").and_then(edit_range),
            // `itemDefaults.editRange` のときの本文は `textEditText`（3.17）
            str_field(v, "textEditText"),
        ),
    };
    let text = new_text
        .or_else(|| str_field(v, "insertText"))
        .unwrap_or_else(|| label.clone());
    let insert_text = if snippet {
        snippet_to_plain(&text)
    } else {
        text
    };
    let additional = v
        .get("additionalTextEdits")
        .and_then(Value::as_array)
        .map(|edits| {
            edits
                .iter()
                // 本文が空（= 消すだけ）の編集も受ける
                .filter_map(|e| {
                    Some((
                        lsp_range(e.get("range")?)?,
                        e.get("newText")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let deprecated = v.get("deprecated").and_then(Value::as_bool) == Some(true)
        || v.get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| tags.iter().any(|t| t.as_u64() == Some(1)));
    Some(RawItem {
        label,
        kind: v
            .get("kind")
            .and_then(Value::as_u64)
            .and_then(|k| u32::try_from(k).ok()),
        detail: str_field(v, "detail"),
        documentation: documentation_text(v.get("documentation")),
        filter_text: str_field(v, "filterText"),
        sort_text: str_field(v, "sortText"),
        preselect: v.get("preselect").and_then(Value::as_bool) == Some(true),
        deprecated,
        insert_text,
        range,
        additional,
        raw: v.clone(),
    })
}

/// tako の `line` 行目（0 起点・`\n` 区切り。`\r\n` の `\r` は残る）
pub fn tako_line(text: &str, line: usize) -> &str {
    text.split('\n').nth(line).unwrap_or("")
}

/// 応答を tako の座標へ写す。
///
/// `text` は**サーバへ送った本文の写し**（= サーバが見ている本文。#1769 と同じく単独の `\r` の
/// 後ろは LSP では次の行なので、画面の 1 行では数えられない）、`cursor` は問い合わせた位置。
/// 行頭の索引は 1 回だけ作る（候補が 1000 件あっても本文 × 件数にしない）。
///
/// 範囲がカーソルより後ろから始まる候補（壊れた応答）は語の頭から置き換える。
/// 追加の編集は**置き換えの範囲と重なるもの**を捨てる（重なると確定で本文が壊れる）
pub fn locate(parsed: Parsed, text: &str, cursor: At) -> Vec<CompletionItem> {
    let index = LineIndex::new(text);
    let to_at = |(line, character): (usize, usize)| {
        let (line, col) = index.line_byte_col(line, character);
        At::new(line, col)
    };
    let word = At::new(
        cursor.line,
        word_start(tako_line(text, cursor.line), cursor.col),
    );
    parsed
        .items
        .into_iter()
        .map(|raw| {
            let (start, tail) = match raw.range {
                Some((s, e)) => {
                    let (s, e) = (to_at(s), to_at(e));
                    let start = if s > cursor { word } else { s };
                    let tail = if e.line == cursor.line && e.col > cursor.col {
                        e.col - cursor.col
                    } else {
                        0
                    };
                    (start, tail)
                }
                None => (word, 0),
            };
            let end = At::new(cursor.line, cursor.col + tail);
            let additional = raw
                .additional
                .into_iter()
                .map(|((s, e), text)| Edit {
                    start: to_at(s),
                    end: to_at(e),
                    text,
                })
                .filter(|edit| edit.end <= start || edit.start >= end)
                .collect();
            CompletionItem {
                label: raw.label,
                kind: raw.kind,
                detail: raw.detail,
                documentation: raw.documentation,
                filter_text: raw.filter_text,
                sort_text: raw.sort_text,
                preselect: raw.preselect,
                deprecated: raw.deprecated,
                insert_text: raw.insert_text,
                start,
                tail,
                additional,
                raw: raw.raw,
            }
        })
        .collect()
}

/// `completionItem/resolve` の答えで、説明（detail / documentation）だけを補う。
/// 本文や範囲は問い合わせたときのまま（確定の位置を答えのたびに動かさない）
pub fn apply_resolved(item: &mut CompletionItem, resolved: &Value) {
    if item.detail.is_none() {
        item.detail = str_field(resolved, "detail");
    }
    if item.documentation.is_none() {
        item.documentation = documentation_text(resolved.get("documentation"));
    }
}

// --- スニペット ----------------------------------------------------------------

/// スニペット（LSP の `insertTextFormat: 2`）を平文にする。
///
/// tako はクライアント能力で `snippetSupport: false` を申告するので、本来は届かない。
/// 申告を無視して送ってくるサーバのために、跳び先（`$1` / `$0` / `${1}`）と変数（`$NAME`）を
/// 落とし、プレースホルダ（`${1:既定}`。入れ子も可）は既定の文字、選択肢（`${1|a,b|}`）は
/// 最初の 1 つを残す。`\$` / `\}` / `\\` のエスケープは解く。形の崩れた `$` は字面のまま
pub fn snippet_to_plain(snippet: &str) -> String {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out = String::with_capacity(snippet.len());
    let mut i = 0;
    snippet_seq(&chars, &mut i, &mut out, false);
    out
}

fn snippet_seq(c: &[char], i: &mut usize, out: &mut String, nested: bool) {
    while *i < c.len() {
        match c[*i] {
            '\\' if c.get(*i + 1).is_some_and(|n| matches!(n, '$' | '}' | '\\')) => {
                out.push(c[*i + 1]);
                *i += 2;
            }
            '}' if nested => return,
            '$' => snippet_dollar(c, i, out),
            ch => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

fn snippet_dollar(c: &[char], i: &mut usize, out: &mut String) {
    let start = *i;
    *i += 1;
    let name = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    match c.get(*i) {
        // `$1` / `$NAME`
        Some(ch) if name(*ch) => {
            while c.get(*i).is_some_and(|ch| name(*ch)) {
                *i += 1;
            }
        }
        Some('{') => {
            *i += 1;
            let name_start = *i;
            while c.get(*i).is_some_and(|ch| name(*ch)) {
                *i += 1;
            }
            if *i == name_start {
                out.extend(&c[start..*i]);
                return;
            }
            match c.get(*i) {
                Some('}') => *i += 1,
                Some(':') => {
                    *i += 1;
                    snippet_seq(c, i, out, true);
                    if c.get(*i) == Some(&'}') {
                        *i += 1;
                    }
                }
                Some('|') => {
                    *i += 1;
                    let mut taking = true;
                    while *i < c.len() {
                        match c[*i] {
                            '\\' if *i + 1 < c.len() => {
                                if taking {
                                    out.push(c[*i + 1]);
                                }
                                *i += 2;
                            }
                            ',' => {
                                taking = false;
                                *i += 1;
                            }
                            '|' if c.get(*i + 1) == Some(&'}') => {
                                *i += 2;
                                break;
                            }
                            ch => {
                                if taking {
                                    out.push(ch);
                                }
                                *i += 1;
                            }
                        }
                    }
                }
                // 変数の変換（`${NAME/正規表現/書式/}`）は畳む
                Some('/') => {
                    while *i < c.len() && c[*i] != '}' {
                        *i += if c[*i] == '\\' { 2 } else { 1 };
                    }
                    *i = (*i + 1).min(c.len());
                }
                _ => out.extend(&c[start..*i]),
            }
        }
        _ => out.push('$'),
    }
}

// --- 語と絞り込み ----------------------------------------------------------------

/// 語を作る文字（定義ジャンプの `symbol_at` と同じ = 英数字・`_`・Unicode の英数字）
pub fn is_word_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

/// `line` の `col`（UTF-8 バイト。文字の境界）の直前の語の頭（UTF-8 バイト）。
/// 語の中に居なければ `col` そのもの
pub fn word_start(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    if !line.is_char_boundary(col) {
        return col;
    }
    line[..col]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| is_word_char(*ch))
        .last()
        .map_or(col, |(i, _)| i)
}

/// 合い方の段（小さいほど上に並ぶ）。`None` = 合わない（一覧から外す）
///
/// 0 = 大文字小文字まで含めた前方一致 / 1 = 大文字小文字を無視した前方一致 /
/// 2 = 大文字小文字を無視した部分一致 / 3 = 語の切れ目から始まる飛び飛びの一致
/// （`hmp` → `HashMap`・`tos` → `to_string`）。問い合わせが空なら全部 0
pub fn match_tier(query: &str, candidate: &str) -> Option<u8> {
    if query.is_empty() || candidate.starts_with(query) {
        return Some(0);
    }
    let q = query.to_lowercase();
    let c = candidate.to_lowercase();
    if c.starts_with(&q) {
        return Some(1);
    }
    if c.contains(&q) {
        return Some(2);
    }
    subsequence_from_boundary(&q, candidate).then_some(3)
}

/// `query`（小文字）の 1 文字目が `candidate` の語の切れ目（先頭・記号の後・小文字の後の大文字）
/// に当たり、残りが順に現れるか
fn subsequence_from_boundary(query: &str, candidate: &str) -> bool {
    let mut q = query.chars();
    let Some(first) = q.next() else {
        return true;
    };
    let chars: Vec<char> = candidate.chars().collect();
    let boundary = |i: usize| {
        i == 0
            || !chars[i - 1].is_alphanumeric()
            || (chars[i].is_uppercase() && chars[i - 1].is_lowercase())
    };
    let rest: Vec<char> = q.collect();
    (0..chars.len())
        .filter(|&i| boundary(i) && chars[i].to_lowercase().eq(first.to_lowercase()))
        .any(|i| {
            let mut tail = chars[i + 1..].iter().flat_map(|ch| ch.to_lowercase());
            rest.iter().all(|want| tail.any(|got| got == *want))
        })
}

/// 候補の絞り込みと並べ替え。答えは `items` の添字を表示の順に並べたもの。
///
/// 照合する問い合わせは**候補ごと**: 置き換えの始点がカーソルと同じ行の手前にあれば
/// 始点からカーソルまで（LSP の仕様。`textEdit` の範囲 = その候補の語）、そうでなければ
/// カーソルの直前の語。照合の相手は `filterText`（無ければ `label`）。
/// 並びは 合い方の段 → `sortText`（無ければ `label`）→ `label` → 元の順（同じ答えなら同じ並び）
pub fn rank(items: &[CompletionItem], line: &str, cursor: At) -> Vec<usize> {
    let cursor_col = cursor.col.min(line.len());
    let word = word_start(line, cursor_col);
    let query_of = |item: &CompletionItem| -> &str {
        let from = if item.start.line == cursor.line && item.start.col <= cursor_col {
            item.start.col
        } else {
            word
        };
        line.get(from..cursor_col).unwrap_or("")
    };
    let mut keyed: Vec<(u8, &str, &str, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let candidate = item.filter_text.as_deref().unwrap_or(&item.label);
            let tier = match_tier(query_of(item), candidate)?;
            let sort = item.sort_text.as_deref().unwrap_or(&item.label);
            Some((tier, sort, item.label.as_str(), i))
        })
        .collect();
    keyed.sort();
    keyed.into_iter().map(|(.., i)| i).collect()
}

/// 並びのうち最初に選ぶ位置（`preselect` の候補があればそれ、無ければ先頭）
pub fn initial_selection(order: &[usize], items: &[CompletionItem]) -> usize {
    order
        .iter()
        .position(|&i| items.get(i).is_some_and(|item| item.preselect))
        .unwrap_or(0)
}

/// 件数の上限（`--limit`）。答えは `(返す数, 切った数)`
pub fn truncate(total: usize, limit: usize) -> (usize, usize) {
    let shown = total.min(limit);
    (shown, total - shown)
}

/// LSP の `CompletionItemKind`（1〜25）の名前（CLI / MCP の `kind` と GUI の印の色分け）
pub fn kind_slug(kind: u32) -> &'static str {
    const NAMES: [&str; 25] = [
        "text",
        "method",
        "function",
        "constructor",
        "field",
        "variable",
        "class",
        "interface",
        "module",
        "property",
        "unit",
        "value",
        "enum",
        "keyword",
        "snippet",
        "color",
        "file",
        "reference",
        "folder",
        "enum-member",
        "constant",
        "struct",
        "event",
        "operator",
        "type-parameter",
    ];
    kind.checked_sub(1)
        .and_then(|i| NAMES.get(i as usize))
        .copied()
        .unwrap_or("text")
}

// --- 打鍵の振り分け ----------------------------------------------------------------

/// 補完と検索バーが取り合う 5 キー
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupKey {
    Escape,
    Enter,
    Up,
    Down,
    Tab,
}

impl PopupKey {
    pub const ALL: [Self; 5] = [Self::Escape, Self::Enter, Self::Up, Self::Down, Self::Tab];

    /// GPUI の `Keystroke::key` の綴りから
    pub fn parse(key: &str) -> Option<Self> {
        Some(match key {
            "escape" => Self::Escape,
            "enter" => Self::Enter,
            "up" => Self::Up,
            "down" => Self::Down,
            "tab" => Self::Tab,
            _ => return None,
        })
    }
}

/// 補完の一覧への操作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKey {
    /// 閉じる（何も入れない）
    Close,
    /// 選んでいる候補で確定する
    Accept,
    /// 1 つ上へ（先頭なら末尾へ回る）
    Previous,
    /// 1 つ下へ（末尾なら先頭へ回る）
    Next,
}

/// 打鍵の行き先
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRoute {
    /// 補完の一覧が取る
    Completion(CompletionKey),
    /// 検索バーが取る（`close_completion` = 出ている補完の一覧は閉じる。キーボードは
    /// 検索欄にあるので、本文に付いた一覧はもう打鍵に追従できない）
    SearchBar { close_completion: bool },
    /// 編集本文が取る（従来どおり `editor_keys` の表を引く）
    Editor,
}

/// 5 キーの振り分け表（**全 20 通りの正本**。単体テストが固定値で縛る）。
///
/// | 検索バー | 補完 | Esc | Enter | ↑ | ↓ | Tab |
/// |---|---|---|---|---|---|---|
/// | なし | なし | 本文 | 本文 | 本文 | 本文 | 本文 |
/// | なし | あり | 閉じる | 確定 | 上 | 下 | 確定 |
/// | あり | なし | 検索 | 検索 | 検索 | 検索 | 検索 |
/// | あり | あり | 補完を閉じる | 検索（補完は閉じる） | 同左 | 同左 | 同左 |
///
/// 決め方: キーボードを持つ側が取る。検索バーが開いている間は検索欄がキーボードを持つ
/// （#195 からの作法 = 打鍵はすべて検索欄へ）ので、補完の一覧がそれを奪わない。
/// 両方出ているとき（一覧を出したまま CLI / MCP が検索を開いた等）の Esc だけは、
/// **手前の一時的なものから閉じる**（補完 → 次の Esc で検索バー。VS Code と同じ順）。
/// 修飾キー付き（⇧ を除く）はこの表に乗らない（⌘F 等はキーバインドが先に取る）
pub fn route_key(search_open: bool, completion_open: bool, key: PopupKey) -> KeyRoute {
    match (search_open, completion_open, key) {
        (false, false, _) => KeyRoute::Editor,
        (false, true, PopupKey::Escape) => KeyRoute::Completion(CompletionKey::Close),
        (false, true, PopupKey::Enter | PopupKey::Tab) => {
            KeyRoute::Completion(CompletionKey::Accept)
        }
        (false, true, PopupKey::Up) => KeyRoute::Completion(CompletionKey::Previous),
        (false, true, PopupKey::Down) => KeyRoute::Completion(CompletionKey::Next),
        (true, false, _) => KeyRoute::SearchBar {
            close_completion: false,
        },
        (true, true, PopupKey::Escape) => KeyRoute::Completion(CompletionKey::Close),
        (true, true, _) => KeyRoute::SearchBar {
            close_completion: true,
        },
    }
}

// --- 古い応答を捨てる ----------------------------------------------------------------

/// 打鍵ごとの要求の通し番号と、出した要求の版（GUI が 1 ペインぶん持つ）。
///
/// 打鍵のたびに [`Self::schedule`] で番号を進め、デバウンスが明けたら [`Self::is_latest`] を
/// 見て最新の番号だけが要求を出す（[`Self::sent`]）。応答が届いたら [`Self::accept`] で
/// **番号と版の両方**を照合する: 後から打鍵した（= 番号が進んだ）・本文が変わった
/// （= 版が進んだ）なら、その応答は今の本文に対する答えではないので画面へ出さない
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Session {
    seq: u64,
    /// 出した要求（番号, そのときの本文の版）
    sent: Option<(u64, u64)>,
}

/// 届いた応答の扱い
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 今の本文への答え。画面へ出す
    Show,
    /// 後から打鍵した（新しい要求がデバウンス中か、もう出ている）
    Superseded,
    /// 要求を出した後に本文が変わった
    Edited,
}

impl Session {
    /// 打鍵した: 次の要求の番号を取る（古い番号のデバウンスは明けても要求を出さない）
    pub fn schedule(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// デバウンスが明けた番号がまだ最新か
    pub fn is_latest(&self, seq: u64) -> bool {
        seq == self.seq
    }

    /// 要求を出した（その時点の本文の版を覚える）
    pub fn sent(&mut self, seq: u64, version: u64) {
        self.sent = Some((seq, version));
    }

    /// 一覧を閉じた: 待っている応答をすべて捨てる
    pub fn cancel(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        self.sent = None;
    }

    /// 番号 `seq` の応答が届いた。`current_version` は今の本文の版
    pub fn accept(&mut self, seq: u64, current_version: u64) -> Verdict {
        if seq != self.seq {
            return Verdict::Superseded;
        }
        match self.sent {
            Some((sent_seq, version)) if sent_seq == seq && version == current_version => {
                self.sent = None;
                Verdict::Show
            }
            Some((sent_seq, _)) if sent_seq == seq => {
                self.sent = None;
                Verdict::Edited
            }
            // 出していない番号の応答（閉じた後に届いた等）
            _ => Verdict::Superseded,
        }
    }
}

// --- 打鍵が補完を起こすか ----------------------------------------------------------------

/// 補完を起こしたきっかけ（LSP の `CompletionContext.triggerKind`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// 語を打った（`Invoked` = 1）
    Word,
    /// サーバが申告した文字（`.` / `:` 等）を打った（`TriggerCharacter` = 2）
    Character(char),
    /// 出ている一覧が `isIncomplete` で、打ち足したので問い直す（`TriggerForIncompleteCompletions` = 3）
    Incomplete,
}

impl Trigger {
    /// 要求の `context`（クライアント能力で `contextSupport: true` を申告している）
    pub fn context(self) -> Value {
        match self {
            Self::Word => serde_json::json!({ "triggerKind": 1 }),
            Self::Character(ch) => {
                serde_json::json!({ "triggerKind": 2, "triggerCharacter": ch.to_string() })
            }
            Self::Incomplete => serde_json::json!({ "triggerKind": 3 }),
        }
    }
}

/// 1 文字打ったとき、補完を**新しく**起こすか。
///
/// 起こすのは 1 文字の打鍵だけ（貼り付けや IME の確定で入った長い本文では起こさない）。
/// サーバの申告した文字（`triggerCharacters`）なら [`Trigger::Character`]、ASCII の英数字か
/// `_` なら [`Trigger::Word`]。日本語の打鍵では起こさない（コメントや文字列の地の文で毎回
/// 問い合わせない。一覧が出ている間の打ち足しは [`is_word_char`] で追従する）
pub fn trigger_for(inserted: &str, trigger_characters: &[String]) -> Option<Trigger> {
    let mut chars = inserted.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if trigger_characters
        .iter()
        .any(|t| t.chars().eq(std::iter::once(ch)))
    {
        return Some(Trigger::Character(ch));
    }
    (ch.is_ascii_alphanumeric() || ch == '_').then_some(Trigger::Word)
}

/// サーバの能力に補完があるか（`completionProvider`）
pub fn server_supports(capabilities: &Value) -> bool {
    capabilities
        .get("completionProvider")
        .is_some_and(Value::is_object)
}

/// サーバが申告した `triggerCharacters`
pub fn trigger_characters(capabilities: &Value) -> Vec<String> {
    capabilities
        .pointer("/completionProvider/triggerCharacters")
        .and_then(Value::as_array)
        .map(|chars| {
            chars
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// サーバが `completionItem/resolve` を持つか
pub fn resolve_supported(capabilities: &Value) -> bool {
    capabilities
        .pointer("/completionProvider/resolveProvider")
        .and_then(Value::as_bool)
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            kind: None,
            detail: None,
            documentation: None,
            filter_text: None,
            sort_text: None,
            preselect: false,
            deprecated: false,
            insert_text: label.into(),
            start: At::new(0, 0),
            tail: 0,
            additional: Vec::new(),
            raw: Value::Null,
        }
    }

    /// 受け入れ条件: 表が全組み合わせ（検索バー有無 × 補完有無 × 5 キー = 20 通り）で
    /// 固定値と一致する。期待値は表をここで**別に書き下した**もの（`route_key` から作らない）
    #[test]
    fn キーの振り分け表は全_20_通りが固定値と一致する() {
        use CompletionKey::*;
        use KeyRoute::*;
        use PopupKey::*;
        let editor = Editor;
        let search = SearchBar {
            close_completion: false,
        };
        let search_closing = SearchBar {
            close_completion: true,
        };
        let expected = [
            // (検索バー, 補完, キー, 行き先)
            (false, false, Escape, editor),
            (false, false, Enter, editor),
            (false, false, Up, editor),
            (false, false, Down, editor),
            (false, false, Tab, editor),
            (false, true, Escape, Completion(Close)),
            (false, true, Enter, Completion(Accept)),
            (false, true, Up, Completion(Previous)),
            (false, true, Down, Completion(Next)),
            (false, true, Tab, Completion(Accept)),
            (true, false, Escape, search),
            (true, false, Enter, search),
            (true, false, Up, search),
            (true, false, Down, search),
            (true, false, Tab, search),
            (true, true, Escape, Completion(Close)),
            (true, true, Enter, search_closing),
            (true, true, Up, search_closing),
            (true, true, Down, search_closing),
            (true, true, Tab, search_closing),
        ];
        assert_eq!(expected.len(), 2 * 2 * PopupKey::ALL.len());
        let mut seen = std::collections::HashSet::new();
        for (search_open, completion_open, key, route) in expected {
            assert!(seen.insert((search_open, completion_open, key)), "重複");
            assert_eq!(
                route_key(search_open, completion_open, key),
                route,
                "検索バー={search_open} 補完={completion_open} {key:?}"
            );
        }
        // 綴りは GPUI の `Keystroke::key`
        for (name, key) in [
            ("escape", Escape),
            ("enter", Enter),
            ("up", Up),
            ("down", Down),
            ("tab", Tab),
        ] {
            assert_eq!(PopupKey::parse(name), Some(key));
        }
        assert_eq!(PopupKey::parse("backspace"), None);
    }

    /// 受け入れ条件: デバウンス中に届いた古い応答が画面へ反映されない（版の照合で捨てる）
    #[test]
    fn デバウンス中に届いた古い応答は捨てる() {
        let mut s = Session::default();
        // `fo` を打ってデバウンスが明け、版 5 で要求を出した
        let first = s.schedule();
        assert!(s.is_latest(first));
        s.sent(first, 5);
        // 応答を待つ間に `o` を打った（版 6。新しい要求はデバウンス中でまだ出ていない）
        let second = s.schedule();
        assert!(!s.is_latest(first), "古い番号のデバウンスは要求を出さない");
        // 古い要求の応答が届く → 番号が古いので捨てる
        assert_eq!(s.accept(first, 6), Verdict::Superseded);
        // 新しい要求を版 6 で出し、その応答は出す
        s.sent(second, 6);
        assert_eq!(s.accept(second, 6), Verdict::Show);
        // 同じ番号の応答は 2 度出さない
        assert_eq!(s.accept(second, 6), Verdict::Superseded);
    }

    #[test]
    fn 要求の後に本文が変わったら同じ番号でも捨てる() {
        let mut s = Session::default();
        let seq = s.schedule();
        s.sent(seq, 9);
        // 打鍵ではない変更（CLI / MCP の編集・undo）で版だけが進んだ
        assert_eq!(s.accept(seq, 10), Verdict::Edited);
    }

    #[test]
    fn 閉じた後に届いた応答は捨てる() {
        let mut s = Session::default();
        let seq = s.schedule();
        s.sent(seq, 1);
        s.cancel();
        assert_eq!(s.accept(seq, 1), Verdict::Superseded);
    }

    #[test]
    fn 応答は配列でも_completion_list_でも_null_でも読める() {
        let list = json!({
            "isIncomplete": true,
            "items": [{ "label": "a" }, { "label": "b", "kind": 3 }, { "nolabel": 1 }],
        });
        let parsed = parse_response(&list);
        assert!(parsed.is_incomplete);
        assert_eq!(parsed.items.len(), 2, "label の無い候補は捨てる");
        assert_eq!(parsed.items[1].kind, Some(3));
        let array = parse_response(&json!([{ "label": "x" }]));
        assert_eq!(array.items.len(), 1);
        assert!(!array.is_incomplete);
        assert!(parse_response(&Value::Null).items.is_empty());
    }

    #[test]
    fn 本文は_text_edit_から_insert_text_から_label_の順に採る() {
        let parsed = parse_response(&json!([
            { "label": "l1", "insertText": "i1",
              "textEdit": { "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2} }, "newText": "t1" } },
            { "label": "l2", "insertText": "i2" },
            { "label": "l3" },
            // InsertReplaceEdit は insert 側の範囲
            { "label": "l4", "textEdit": { "newText": "t4",
                "insert": { "start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 2} },
                "replace": { "start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 5} } } },
        ]));
        let texts: Vec<&str> = parsed
            .items
            .iter()
            .map(|i| i.insert_text.as_str())
            .collect();
        assert_eq!(texts, ["t1", "i2", "l3", "t4"]);
        assert_eq!(parsed.items[0].range, Some(((0, 0), (0, 2))));
        assert_eq!(parsed.items[3].range, Some(((0, 1), (0, 2))));
    }

    #[test]
    fn item_defaults_の範囲とスニペットの既定を使う() {
        let parsed = parse_response(&json!({
            "isIncomplete": false,
            "itemDefaults": {
                "editRange": { "start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 6} },
                "insertTextFormat": 2,
            },
            "items": [{ "label": "f", "textEditText": "f(${1:x})$0" }],
        }));
        assert_eq!(parsed.items[0].range, Some(((0, 4), (0, 6))));
        assert_eq!(parsed.items[0].insert_text, "f(x)");
    }

    #[test]
    fn 上限を超えた候補は捨てて数える() {
        let items: Vec<Value> = (0..MAX_ITEMS + 7)
            .map(|i| json!({ "label": format!("c{i}") }))
            .collect();
        let parsed = parse_response(&Value::Array(items));
        assert_eq!(parsed.items.len(), MAX_ITEMS);
        assert_eq!(parsed.dropped, 7);
    }

    #[test]
    fn スニペットは跳び先を落として既定の文字を残す() {
        for (snippet, plain) in [
            ("foo($1)$0", "foo()"),
            ("println!(\"${1:x}\")", "println!(\"x\")"),
            ("${1|first,second|}", "first"),
            ("\\$x \\}", "$x }"),
            ("${1:outer ${2:inner}}", "outer inner"),
            ("$TM_FILENAME.", "."),
            ("${TM_FILENAME/(.*)/$1/}x", "x"),
            ("a$", "a$"),
            ("${}", "${}"),
            ("plain", "plain"),
        ] {
            assert_eq!(snippet_to_plain(snippet), plain, "{snippet}");
        }
    }

    #[test]
    fn 語の頭はカーソルの直前の語() {
        assert_eq!(word_start("    let foo", 11), 8);
        assert_eq!(word_start("a.bar", 5), 2);
        assert_eq!(word_start("x(", 2), 2, "語の外ならカーソルそのもの");
        assert_eq!(word_start("名前x", 7), 0, "Unicode の英数字も語");
        assert_eq!(word_start("abc", 9), 0, "行末を超える桁は行末へ");
    }

    #[test]
    fn 座標は_utf16_から_tako_の行とバイトへ写す() {
        // 1 行目 `  😀 name` の `na` の後ろ（絵文字は UTF-16 で 2・UTF-8 で 4）を打っている。
        // `name` は UTF-16 で 5..9・バイトで 7..11。カーソルはバイト 9 = UTF-16 の 7
        let text = "fn a() {}\n  😀 name\nlast";
        let cursor = At::new(1, "  😀 na".len());
        let parsed = parse_response(&json!([
            // 範囲つき: UTF-16 の (1, 5) = バイトの 2 + 4 + 1 = 7
            { "label": "name",
              "textEdit": { "range": { "start": {"line": 1, "character": 5}, "end": {"line": 1, "character": 7} }, "newText": "name" },
              "additionalTextEdits": [
                { "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0} }, "newText": "use x;\n" },
                // 置き換えの範囲と重なる編集は捨てる（確定で本文が壊れる）
                { "range": { "start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 7} }, "newText": "!" },
              ] },
            // 範囲なし: 語の頭から
            { "label": "nap" },
            // 範囲がカーソルの後ろまで伸びる（同じ行）: `me` の 2 バイトも置き換える
            { "label": "nab", "textEdit": { "range": { "start": {"line": 1, "character": 5}, "end": {"line": 1, "character": 9} }, "newText": "nab" } },
            // 範囲がカーソルより後ろから始まる（壊れた応答）: 語の頭から
            { "label": "bad", "textEdit": { "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 1} }, "newText": "bad" } },
        ]));
        let items = locate(parsed, text, cursor);
        assert_eq!(items[0].start, At::new(1, 7));
        assert_eq!(items[0].tail, 0);
        assert_eq!(items[0].additional.len(), 1);
        assert_eq!(items[0].additional[0].start, At::new(0, 0));
        assert_eq!(items[0].additional[0].text, "use x;\n");
        assert_eq!(items[1].start, At::new(1, 7), "範囲なしは語の頭");
        assert_eq!(items[2].tail, 2);
        assert_eq!(items[3].start, At::new(1, 7));
    }

    #[test]
    fn 絞り込みは前方一致を上に並べ合わないものを外す() {
        let mut items: Vec<CompletionItem> = ["HashMap", "hash", "rehash", "HashSet", "other"]
            .into_iter()
            .map(|label| CompletionItem {
                start: At::new(0, 4),
                ..item(label)
            })
            .collect();
        // sortText で並べ替えの順を逆にする（段が同じなら sortText が勝つ）
        items[3].sort_text = Some("0".into());
        let order = rank(&items, "    Hash", At::new(0, 8));
        let labels: Vec<&str> = order.iter().map(|&i| items[i].label.as_str()).collect();
        // 段 0（大文字小文字まで前方一致）: HashSet(sort 0) / HashMap → 段 1: hash → 段 2: rehash
        assert_eq!(labels, ["HashSet", "HashMap", "hash", "rehash"]);
    }

    #[test]
    fn 飛び飛びの一致は語の切れ目から() {
        // `hm` は `hashmap` の部分文字列（has**hm**ap）なので段 2。飛び飛びは `hmp`
        assert_eq!(match_tier("hm", "HashMap"), Some(2));
        assert_eq!(match_tier("hmp", "HashMap"), Some(3));
        assert_eq!(match_tier("tos", "to_string"), Some(3));
        assert_eq!(match_tier("ap", "HashMap"), Some(2), "部分一致");
        assert_eq!(match_tier("xq", "HashMap"), None);
        assert_eq!(match_tier("", "anything"), Some(0));
    }

    #[test]
    fn 候補ごとの範囲で問い合わせを切る() {
        // `x.i|` で、範囲が `x.i` 全体の候補（照合は `filterText`）・範囲が `i` だけの候補・
        // 範囲が `x.i` 全体なのに `filterText` の無い候補（`x.i` は `iter2` に合わない）
        let mut a = item("if");
        a.filter_text = Some("x.if".into());
        let mut b = item("iter");
        b.start = At::new(0, 2);
        let c = item("iter2");
        let items = vec![a, b, c];
        assert_eq!(rank(&items, "x.i", At::new(0, 3)), vec![0, 1]);
        // 別の行から始まる範囲はカーソルの直前の語（`i`）で照合する
        let mut d = item("item");
        d.start = At::new(3, 0);
        assert_eq!(rank(&[d], "x.i", At::new(0, 3)), vec![0]);
    }

    #[test]
    fn 最初に選ぶのは_preselect_の候補() {
        let mut items: Vec<CompletionItem> = ["a", "b", "c"].into_iter().map(item).collect();
        assert_eq!(initial_selection(&[0, 1, 2], &items), 0);
        items[2].preselect = true;
        assert_eq!(initial_selection(&[0, 1, 2], &items), 2);
        assert_eq!(initial_selection(&[2, 0], &items), 0);
    }

    #[test]
    fn 件数の上限は返す数と切った数を返す() {
        assert_eq!(truncate(1000, 20), (20, 980));
        assert_eq!(truncate(5, 20), (5, 0));
        assert_eq!(truncate(0, 20), (0, 0));
    }

    #[test]
    fn 補完を起こすのは_1_文字の打鍵だけ() {
        let chars = vec![".".to_string(), ":".to_string()];
        assert_eq!(trigger_for("a", &chars), Some(Trigger::Word));
        assert_eq!(trigger_for("_", &chars), Some(Trigger::Word));
        assert_eq!(trigger_for(".", &chars), Some(Trigger::Character('.')));
        assert_eq!(trigger_for(" ", &chars), None);
        assert_eq!(trigger_for("ab", &chars), None, "貼り付けでは起こさない");
        assert_eq!(
            trigger_for("あ", &chars),
            None,
            "日本語の地の文では起こさない"
        );
        assert_eq!(trigger_for(".", &[]), None, "申告の無い文字では起こさない");
        assert_eq!(
            Trigger::Character('.').context(),
            json!({"triggerKind": 2, "triggerCharacter": "."})
        );
    }

    #[test]
    fn 能力の読み取り() {
        let caps = json!({ "completionProvider": { "triggerCharacters": [".", "::"], "resolveProvider": true } });
        assert!(server_supports(&caps));
        assert_eq!(trigger_characters(&caps), vec![".", "::"]);
        assert!(resolve_supported(&caps));
        assert!(!server_supports(&json!({})));
        assert!(!resolve_supported(&json!({ "completionProvider": {} })));
    }

    #[test]
    fn 説明は_resolve_の答えで補う() {
        let mut it = item("x");
        apply_resolved(
            &mut it,
            &json!({ "detail": "fn x()", "documentation": { "kind": "markdown", "value": "doc" } }),
        );
        assert_eq!(it.detail.as_deref(), Some("fn x()"));
        assert_eq!(it.documentation.as_deref(), Some("doc"));
    }

    #[test]
    fn 種類の名前は_lsp_の番号から() {
        assert_eq!(kind_slug(3), "function");
        assert_eq!(kind_slug(22), "struct");
        assert_eq!(kind_slug(0), "text");
        assert_eq!(kind_slug(99), "text");
    }
}
