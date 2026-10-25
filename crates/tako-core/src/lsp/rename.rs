//! リネーム（S9 / #1686）の純粋部分: 能力の判定・新しい名前の検査・`prepareRename` と
//! `rename`（`WorkspaceEdit`）の答えの読み取り・段階 1 の当てる範囲の判定
//!
//! 問い合わせそのもの（文書の同期・サーバの起動待ち・2 つの要求）は `tako_control::lsp` の manager、
//! 答えを編集バッファへ当てるのは `text_edit::TextBuffer::apply_changes` の 1 実装（undo 1 回で戻る）。
//! 並べ方・重なりは `text_edit::order_changes` だけが持つ（ここでは書かない）。
//!
//! **段階 1 の境界**（#1686）: 当てるのは**頼んだペインの編集バッファ 1 つ**だけ。`WorkspaceEdit` が
//! それ以外のファイル（別のペインで開いていても）に及ぶ・ファイルの作成 / 名前の変更 / 削除を含むときは
//! 当てずに件数と理由を返す（[`blocker`]）。複数ファイルの適用・undo・競合検知は段階 2。
//!
//! **言語名はここに書かない**（識別子の規則は言語ごとに違うので、[`check_new_name`] は言語に依らない
//! 最低限だけを見て、それ以外はサーバに断らせる）。

use serde_json::Value;

use super::position;

/// 名前の変更を始めてよいかを問う method（`prepareProvider` を申告したサーバだけに送る）
pub const PREPARE_METHOD: &str = "textDocument/prepareRename";
/// 名前の変更の method（答えは `WorkspaceEdit | null`）
pub const METHOD: &str = "textDocument/rename";
/// 能力のキー（`boolean | RenameOptions`）
pub const PROVIDER_KEY: &str = "renameProvider";

/// サーバが名前の変更に対応しているか（`true` かオブジェクトなら対応）
pub fn server_supports(capabilities: &Value) -> bool {
    match capabilities.get(PROVIDER_KEY) {
        Some(Value::Bool(supported)) => *supported,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

/// `prepareRename` を送ってよいか（`RenameOptions.prepareProvider == true` のときだけ。
/// 申告の無いサーバへ送ると method not found が返る）
pub fn supports_prepare(capabilities: &Value) -> bool {
    capabilities
        .get(PROVIDER_KEY)
        .and_then(|options| options.get("prepareProvider"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 新しい名前を断った理由（言語に依らない最低限。識別子として正しいかはサーバが決める）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// 空・空白だけ
    Empty,
    /// 改行・タブなどの制御文字を含む（1 つの名前にならない）
    Control,
}

/// 新しい名前の検査。**サーバへ問い合わせる前に**断れるものだけを断る
pub fn check_new_name(name: &str) -> Result<(), NameError> {
    if name.trim().is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().any(char::is_control) {
        return Err(NameError::Control);
    }
    Ok(())
}

/// `prepareRename` の答え
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// `null`（その位置は名前を変えられない）
    Rejected,
    /// 変えられる。`placeholder` はサーバが示した今の名前（範囲だけの答えなら `None`）
    Accepted { placeholder: Option<String> },
}

/// `prepareRename` の答え（`Range | { range, placeholder } | { defaultBehavior } | null`）を読む。
/// 形が読めないものは「変えられる」に倒す（断る判断はサーバの `null` / エラーだけに任せる）
pub fn parse_prepare(result: &Value) -> Prepared {
    match result {
        Value::Null => Prepared::Rejected,
        Value::Object(map) => Prepared::Accepted {
            placeholder: map
                .get("placeholder")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        _ => Prepared::Accepted { placeholder: None },
    }
}

/// `prepareRename` の答えの範囲（`Range` そのもの か `{ range, placeholder }`）の LSP の座標。
/// `placeholder` を付けないサーバ（rust-analyzer は範囲だけを返す）の今の名前は、この範囲の字面で補う
pub fn prepare_range(result: &Value) -> Option<((usize, usize), (usize, usize))> {
    let range = result.get("range").unwrap_or(result);
    let at = |key: &str| -> Option<(usize, usize)> {
        let p = range.get(key)?;
        Some((
            usize::try_from(p.get("line")?.as_u64()?).ok()?,
            usize::try_from(p.get("character")?.as_u64()?).ok()?,
        ))
    };
    Some((at("start")?, at("end")?))
}

/// ファイル 1 つぶんの書き換え（`WorkspaceEdit` の 1 項目）
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdits {
    pub uri: String,
    /// LSP の `TextEdit`（`AnnotatedTextEdit` も同じ形）の生の値。座標は**サーバが見ている本文**
    pub edits: Vec<Value>,
}

/// `WorkspaceEdit` を読んだもの
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceEdit {
    /// 書き換えるファイル（`documentChanges` は現れた順・`changes` は URI の順。同じ URI は 1 つへまとめる）
    pub files: Vec<FileEdits>,
    /// ファイルの操作（`create` / `rename` / `delete`）の種類。段階 1 は当てない
    pub resource_ops: Vec<String>,
}

impl WorkspaceEdit {
    /// 書き換えの箇所の総数
    pub fn edit_count(&self) -> usize {
        self.files.iter().map(|f| f.edits.len()).sum()
    }

    fn push(&mut self, uri: &str, edits: &[Value]) {
        match self.files.iter_mut().find(|f| f.uri == uri) {
            Some(file) => file.edits.extend(edits.iter().cloned()),
            None => self.files.push(FileEdits {
                uri: uri.to_string(),
                edits: edits.to_vec(),
            }),
        }
    }
}

/// `WorkspaceEdit` を読めなかった理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// オブジェクトでも null でもない
    NotObject,
    /// `documentChanges` の `index` 番目（1 始まり）の形が違う
    MalformedChange { index: usize },
    /// `changes` の値が `TextEdit` の配列でない
    MalformedChanges,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

impl ParseError {
    /// 表示言語を明示しての文言（#2365。`TextEditError::text_in` と同じ形）。
    /// 日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: crate::i18n::Lang) -> String {
        use crate::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::NotObject, Ja) => "WorkspaceEdit ではない".to_string(),
            (Self::NotObject, En) => "not a WorkspaceEdit".to_string(),
            (Self::MalformedChange { index }, Ja) => {
                format!("documentChanges の {index} 番目の形が違う")
            }
            (Self::MalformedChange { index }, En) => {
                format!("documentChanges entry {index} is malformed")
            }
            (Self::MalformedChanges, Ja) => "changes の値が TextEdit の配列ではない".to_string(),
            (Self::MalformedChanges, En) => {
                "the value of changes is not an array of TextEdit".to_string()
            }
        }
    }
}

/// 答え（`WorkspaceEdit | null`）を読む。`null` は 0 件。
///
/// LSP の決まりどおり `documentChanges` があればそちらを使い、`changes` は見ない
/// （両方を足すと同じ書き換えを 2 回当てる）。`documentChanges` の要素は `TextDocumentEdit`
/// （`textDocument` + `edits`）か、`kind` を持つファイルの操作
pub fn parse_workspace_edit(result: &Value) -> Result<WorkspaceEdit, ParseError> {
    let map = match result {
        Value::Null => return Ok(WorkspaceEdit::default()),
        Value::Object(map) => map,
        _ => return Err(ParseError::NotObject),
    };
    let mut out = WorkspaceEdit::default();
    if let Some(changes) = map.get("documentChanges").filter(|v| !v.is_null()) {
        let changes = changes
            .as_array()
            .ok_or(ParseError::MalformedChange { index: 1 })?;
        for (i, change) in changes.iter().enumerate() {
            let malformed = ParseError::MalformedChange { index: i + 1 };
            if let Some(kind) = change.get("kind").and_then(Value::as_str) {
                out.resource_ops.push(kind.to_string());
                continue;
            }
            let uri = change
                .get("textDocument")
                .and_then(|d| d.get("uri"))
                .and_then(Value::as_str)
                .ok_or(malformed.clone())?;
            let edits = change
                .get("edits")
                .and_then(Value::as_array)
                .ok_or(malformed)?;
            out.push(uri, edits);
        }
        return Ok(out);
    }
    if let Some(changes) = map.get("changes").filter(|v| !v.is_null()) {
        let changes = changes.as_object().ok_or(ParseError::MalformedChanges)?;
        for (uri, edits) in changes {
            let edits = edits.as_array().ok_or(ParseError::MalformedChanges)?;
            out.push(uri, edits);
        }
    }
    Ok(out)
}

/// 段階 1 で当てられない理由（[`blocker`]）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// 頼んだファイル以外へ及ぶ（`files` は及んだファイルの数・`edits` はそこでの箇所の数）
    OtherFiles { files: usize, edits: usize },
    /// ファイルの作成 / 名前の変更 / 削除を含む
    ResourceOps { count: usize },
}

/// 段階 1 で当てられるか。`is_source` は「その URI が頼んだペインのファイルか」（URI の綴りの
/// 照合は呼び手が持つ。正規化の規則を 2 か所に書かない）。当てられるなら `None`
pub fn blocker(edit: &WorkspaceEdit, is_source: impl Fn(&str) -> bool) -> Option<Blocker> {
    if !edit.resource_ops.is_empty() {
        return Some(Blocker::ResourceOps {
            count: edit.resource_ops.len(),
        });
    }
    let others: Vec<&FileEdits> = edit
        .files
        .iter()
        .filter(|f| !f.edits.is_empty() && !is_source(&f.uri))
        .collect();
    (!others.is_empty()).then(|| Blocker::OtherFiles {
        files: others.len(),
        edits: others.iter().map(|f| f.edits.len()).sum(),
    })
}

/// 書き換え 1 箇所の見え方（dry-run の応答に並べる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    /// tako の 0 起点の行（`\n` 区切り）
    pub line: usize,
    /// 行内の UTF-8 バイト（`tako edit replace-range` と同じ桁）
    pub column: usize,
    /// 置き換える前の字面（本文が分からなければ空）
    pub old: String,
    pub new: String,
}

/// `edits`（生の `TextEdit`）の場所を、そのファイルの本文 `text`（サーバが見ている本文。分からなければ
/// `None` = LSP の行と UTF-16 の桁をそのまま使う）で tako の座標へ直す。形の違う項目は飛ばす
pub fn spots(text: Option<&str>, edits: &[Value]) -> Vec<Spot> {
    let index = text.map(position::LineIndex::new);
    edits
        .iter()
        .filter_map(|edit| {
            let at = |key: &str| -> Option<(usize, usize)> {
                let p = edit.get("range")?.get(key)?;
                let line = usize::try_from(p.get("line")?.as_u64()?).ok()?;
                let character = usize::try_from(p.get("character")?.as_u64()?).ok()?;
                Some((line, character))
            };
            let (start, end) = (at("start")?, at("end")?);
            let new = edit.get("newText")?.as_str()?.to_string();
            Some(match (&index, text) {
                (Some(index), Some(text)) => {
                    let (line, column) = index.line_byte_col(start.0, start.1);
                    let from = index.byte_offset(start.0, start.1);
                    let to = index.byte_offset(end.0, end.1).max(from);
                    Spot {
                        line,
                        column,
                        old: text[from..to].to_string(),
                        new,
                    }
                }
                _ => Spot {
                    line: start.0,
                    column: start.1,
                    old: String::new(),
                    new,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(sl: u64, sc: u64, el: u64, ec: u64, new_text: &str) -> Value {
        json!({
            "range": {
                "start": { "line": sl, "character": sc },
                "end": { "line": el, "character": ec },
            },
            "newText": new_text,
        })
    }

    #[test]
    fn 能力は真偽でもオブジェクトでも読み_prepare_は申告したときだけ() {
        assert!(server_supports(&json!({ "renameProvider": true })));
        assert!(!supports_prepare(&json!({ "renameProvider": true })));
        let caps = json!({ "renameProvider": { "prepareProvider": true } });
        assert!(server_supports(&caps));
        assert!(supports_prepare(&caps));
        let caps = json!({ "renameProvider": { "prepareProvider": false } });
        assert!(server_supports(&caps));
        assert!(!supports_prepare(&caps));
        assert!(!server_supports(&json!({ "renameProvider": false })));
        assert!(!server_supports(&json!({})));
        assert!(!supports_prepare(&json!({})));
    }

    #[test]
    fn 新しい名前は空と制御文字だけを断る() {
        assert_eq!(check_new_name(""), Err(NameError::Empty));
        assert_eq!(check_new_name("  \u{3000}"), Err(NameError::Empty));
        assert_eq!(check_new_name("a\nb"), Err(NameError::Control));
        assert_eq!(check_new_name("a\tb"), Err(NameError::Control));
        // 識別子として正しいかは言語ごとに違う = サーバに任せる（数字始まり・記号・日本語も通す）
        for ok in ["item", "1abc", "a-b", "名前", "_"] {
            assert_eq!(check_new_name(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn prepare_の範囲は2形とも読み_既定の振る舞いは範囲を持たない() {
        let range =
            json!({ "start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 9} });
        assert_eq!(prepare_range(&range), Some(((2, 4), (2, 9))));
        assert_eq!(
            prepare_range(&json!({ "range": range, "placeholder": "x" })),
            Some(((2, 4), (2, 9)))
        );
        assert_eq!(prepare_range(&json!({ "defaultBehavior": true })), None);
        assert_eq!(prepare_range(&Value::Null), None);
    }

    #[test]
    fn prepare_の答えは4形とも読む() {
        assert_eq!(parse_prepare(&Value::Null), Prepared::Rejected);
        assert_eq!(
            parse_prepare(
                &json!({ "start": {"line":0,"character":0}, "end": {"line":0,"character":1} })
            ),
            Prepared::Accepted { placeholder: None }
        );
        assert_eq!(
            parse_prepare(&json!({ "range": {}, "placeholder": "value" })),
            Prepared::Accepted {
                placeholder: Some("value".into())
            }
        );
        assert_eq!(
            parse_prepare(&json!({ "defaultBehavior": true })),
            Prepared::Accepted { placeholder: None }
        );
    }

    /// 受け入れ条件（単体）: `changes` と `documentChanges` の両方を読み、ファイル数・箇所数が答えと一致する
    #[test]
    fn workspace_edit_は_changes_と_document_changes_の両方を読む() {
        let changes = json!({ "changes": {
            "file:///p/src/a.rs": [edit(0, 4, 0, 9, "item"), edit(3, 0, 3, 5, "item")],
            "file:///p/src/b.rs": [edit(1, 2, 1, 7, "item")],
        }});
        let parsed = parse_workspace_edit(&changes).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.edit_count(), 3);
        assert!(parsed.resource_ops.is_empty());

        let document_changes = json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///p/src/a.rs", "version": 3 },
              "edits": [edit(0, 4, 0, 9, "item")] },
            { "textDocument": { "uri": "file:///p/src/b.rs", "version": null },
              "edits": [edit(1, 2, 1, 7, "item"), { "range": edit(2, 0, 2, 5, "")["range"], "newText": "item", "annotationId": "x" }] },
            // 同じ URI の 2 つ目はまとめる
            { "textDocument": { "uri": "file:///p/src/a.rs", "version": 3 },
              "edits": [edit(5, 0, 5, 5, "item")] },
            { "kind": "rename", "oldUri": "file:///p/src/b.rs", "newUri": "file:///p/src/c.rs" },
        ],
        // documentChanges があれば changes は見ない（両方足すと 2 回当てる）
        "changes": { "file:///p/src/z.rs": [edit(0, 0, 0, 1, "x")] } });
        let parsed = parse_workspace_edit(&document_changes).unwrap();
        let uris: Vec<&str> = parsed.files.iter().map(|f| f.uri.as_str()).collect();
        assert_eq!(uris, ["file:///p/src/a.rs", "file:///p/src/b.rs"]);
        assert_eq!(parsed.files[0].edits.len(), 2);
        assert_eq!(parsed.edit_count(), 4);
        assert_eq!(parsed.resource_ops, ["rename"]);

        assert_eq!(
            parse_workspace_edit(&Value::Null),
            Ok(WorkspaceEdit::default())
        );
        assert_eq!(parse_workspace_edit(&json!([])), Err(ParseError::NotObject));
        assert_eq!(
            parse_workspace_edit(&json!({ "documentChanges": [{ "edits": [] }] })),
            Err(ParseError::MalformedChange { index: 1 })
        );
        assert_eq!(
            parse_workspace_edit(&json!({ "changes": { "file:///a": {} } })),
            Err(ParseError::MalformedChanges)
        );
    }

    #[test]
    fn 段階1は頼んだファイルだけなら当て他のファイルやファイルの操作があれば当てない() {
        let source = |uri: &str| uri.ends_with("/a.rs");
        let only_source = parse_workspace_edit(&json!({ "changes": {
            "file:///p/a.rs": [edit(0, 0, 0, 1, "x"), edit(1, 0, 1, 1, "x")],
        }}))
        .unwrap();
        assert_eq!(blocker(&only_source, source), None);
        let spread = parse_workspace_edit(&json!({ "changes": {
            "file:///p/a.rs": [edit(0, 0, 0, 1, "x")],
            "file:///p/b.rs": [edit(0, 0, 0, 1, "x"), edit(2, 0, 2, 1, "x")],
            "file:///p/c.rs": [edit(0, 0, 0, 1, "x")],
            // 箇所が 0 のファイルは数えない
            "file:///p/d.rs": [],
        }}))
        .unwrap();
        assert_eq!(
            blocker(&spread, source),
            Some(Blocker::OtherFiles { files: 2, edits: 3 })
        );
        let ops = parse_workspace_edit(&json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///p/a.rs", "version": 1 }, "edits": [edit(0, 0, 0, 1, "x")] },
            { "kind": "create", "uri": "file:///p/new.rs" },
        ]}))
        .unwrap();
        assert_eq!(
            blocker(&ops, source),
            Some(Blocker::ResourceOps { count: 1 })
        );
    }

    /// 場所は tako の座標（行内バイト）へ・置き換える前の字面は本文から（日本語・絵文字の UTF-16 の桁）
    #[test]
    fn 場所は本文で_tako_の座標へ直す() {
        let text = "let 名前 = 1;\n😀 名前;\n";
        let spots = spots(
            Some(text),
            &[
                edit(0, 4, 0, 6, "item"),
                edit(1, 3, 1, 5, "item"),
                json!({}),
            ],
        );
        assert_eq!(
            spots,
            vec![
                Spot {
                    line: 0,
                    column: 4,
                    old: "名前".into(),
                    new: "item".into()
                },
                Spot {
                    line: 1,
                    column: 5,
                    old: "名前".into(),
                    new: "item".into()
                },
            ]
        );
        // 本文が分からなければ LSP の座標のまま・字面は空
        assert_eq!(
            super::spots(None, &[edit(2, 7, 2, 9, "x")]),
            vec![Spot {
                line: 2,
                column: 7,
                old: String::new(),
                new: "x".into()
            }]
        );
    }
}
