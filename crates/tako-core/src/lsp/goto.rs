//! 定義ジャンプ（S3 / #1680）の純粋部分
//!
//! ⌘クリック（macOS。Windows は Ctrl）と CLI `tako lsp definition` / MCP `tako_lsp` が
//! 同じ 1 本を通る。ここに置くのは I/O を持たない判断だけ:
//!
//! - [`GotoKind`] — 4 種（定義 / 宣言 / 型定義 / 実装）と LSP の method・能力のキー
//! - [`parse_locations`] — 応答（`Location` / `Location[]` / `LocationLink[]` / `null`）を 1 つの形へ
//! - [`path_of_uri`] — 応答の `file://` URI をローカルパスへ
//! - [`symbol_at`] — ⌘ホバーで下線を引く範囲（識別子 / `#include` のパス）
//! - [`plan_landing`] — 着地の規則（同じファイル = 同じペイン / 開いている = 再利用 / それ以外 = 新しいペイン）
//!
//! 問い合わせ（プロセスと I/O）は `tako_control::lsp::manager`、着地（ペインを開く）は
//! `tako_control::dispatch` が持つ。**言語名はここに書かない**（検出表 `servers.rs` だけが持つ）。

use std::ops::Range;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::PaneId;

/// 応答から採る場所の上限（サーバが巨大な配列を返しても候補一覧を溢れさせない）
pub const MAX_LOCATIONS: usize = 50;

/// 定義ジャンプの 4 種（CLI のサブコマンド名 = MCP `tako_lsp` の `action`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GotoKind {
    Definition,
    Declaration,
    TypeDefinition,
    Implementation,
}

impl GotoKind {
    pub const ALL: [GotoKind; 4] = [
        GotoKind::Definition,
        GotoKind::Declaration,
        GotoKind::TypeDefinition,
        GotoKind::Implementation,
    ];

    /// 綴り（CLI / MCP が共有する）。順は [`Self::ALL`] と同じ
    pub const NAMES: [&'static str; 4] = [
        "definition",
        "declaration",
        "type-definition",
        "implementation",
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Definition => Self::NAMES[0],
            Self::Declaration => Self::NAMES[1],
            Self::TypeDefinition => Self::NAMES[2],
            Self::Implementation => Self::NAMES[3],
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.slug() == name)
    }

    /// LSP の method
    pub fn method(self) -> &'static str {
        match self {
            Self::Definition => "textDocument/definition",
            Self::Declaration => "textDocument/declaration",
            Self::TypeDefinition => "textDocument/typeDefinition",
            Self::Implementation => "textDocument/implementation",
        }
    }

    /// `ServerCapabilities` のキー（無い / `false` なら未対応）
    pub fn provider_key(self) -> &'static str {
        match self {
            Self::Definition => "definitionProvider",
            Self::Declaration => "declarationProvider",
            Self::TypeDefinition => "typeDefinitionProvider",
            Self::Implementation => "implementationProvider",
        }
    }
}

/// サーバがその種類の問い合わせに応じるか（`true` / 登録オプションのオブジェクト = 応じる）
pub fn server_supports(capabilities: &Value, kind: GotoKind) -> bool {
    match capabilities.get(kind.provider_key()) {
        Some(Value::Bool(on)) => *on,
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

/// 応答の 1 か所（LSP の座標のまま = 0 起点の行・UTF-16 の桁）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLocation {
    pub uri: String,
    pub line: usize,
    pub character: usize,
}

/// 応答を 1 つの形へ。`Location` 単体 / `Location[]` / `LocationLink[]` / `null` を受ける。
///
/// `LocationLink` は `targetSelectionRange`（識別子そのもの）を採る（`targetRange` は
/// 関数本体ごとの範囲で、先頭に飛ぶとドキュメントコメントの行へ落ちる）。
/// 同じ場所の重複は先勝ちで畳み、[`MAX_LOCATIONS`] で打ち切る
pub fn parse_locations(result: &Value) -> Vec<RawLocation> {
    let items: Vec<&Value> = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    let mut out: Vec<RawLocation> = Vec::new();
    for item in items {
        let (uri, range) = match (item.get("targetUri"), item.get("uri")) {
            (Some(uri), _) => (
                uri,
                item.get("targetSelectionRange")
                    .or_else(|| item.get("targetRange")),
            ),
            (None, Some(uri)) => (uri, item.get("range")),
            (None, None) => continue,
        };
        let (Some(uri), Some(start)) = (uri.as_str(), range.and_then(|r| r.get("start"))) else {
            continue;
        };
        let number = |key: &str| {
            start
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
        };
        let (Some(line), Some(character)) = (number("line"), number("character")) else {
            continue;
        };
        let location = RawLocation {
            uri: uri.to_string(),
            line,
            character,
        };
        if !out.contains(&location) {
            out.push(location);
        }
        if out.len() >= MAX_LOCATIONS {
            break;
        }
    }
    out
}

/// `file://` URI → ローカルパス（ローカルのファイルでなければ `None`）。
///
/// `%XX` は厳密に解く（壊れた `%` は `None` = 別のファイルを開くより「開けない」で止める）。
/// ドライブ文字の先頭 `/` を落とすのと区切り文字の変換は [`crate::file_uri`] の 1 実装を通す
/// （RFC 8089 の Windows 形式を 2 か所に書かない）。`windows` は OS の明示（macOS 上から
/// Windows 形式を検証できるように純粋関数にする）
pub fn path_of_uri(uri: &str, windows: bool) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let (host, path) = match rest.find('/') {
        Some(0) => ("", rest),
        Some(at) => (&rest[..at], &rest[at..]),
        None => return None,
    };
    let decoded = percent_decode_strict(path)?;
    let separator = if windows { '\\' } else { '/' };
    match host {
        "" | "localhost" => {
            let local = crate::file_uri::strip_drive_slash(&decoded);
            Some(PathBuf::from(
                crate::file_uri::native_separators_with(local, separator).into_owned(),
            ))
        }
        // UNC（`file://server/share/x` → `\\server\share\x`）は Windows だけ
        host if windows => Some(PathBuf::from(format!(
            r"\\{host}{}",
            crate::file_uri::native_separators_with(&decoded, separator)
        ))),
        _ => None,
    }
}

fn percent_decode_strict(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |b: Option<&u8>| b.and_then(|b| (*b as char).to_digit(16));
            let (hi, lo) = (hex(bytes.get(i + 1))?, hex(bytes.get(i + 2))?);
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// ⌘ホバーで下線を引く対象の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// 識別子（英数字・`_`・非 ASCII の文字の並び。数字で始まるものは数値なので除く）
    Identifier,
    /// `#include "foo.h"` / `#include <foo.h>` の引用符の中（ヘッダを開く）
    IncludePath,
}

/// ⌘ホバーの対象 1 つ（行内の UTF-8 バイト範囲）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub range: Range<usize>,
    pub kind: SymbolKind,
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

/// 行 `line` の `byte_col`（UTF-8 バイト。文字の途中なら手前の文字）にある ⌘ホバーの対象。
///
/// 検出は字面だけで行う（定義があるかはサーバに聞くまで分からない。コメントや文字列の中の
/// 語にも下線が付くのは他のエディタと同じ）。`#include` の行は引用符の中を 1 つの対象にする
/// （パスの途中の `/` や `.` で切らない）
pub fn symbol_at(line: &str, byte_col: usize) -> Option<Symbol> {
    if byte_col >= line.len() {
        return None;
    }
    let mut at = byte_col;
    while !line.is_char_boundary(at) {
        at -= 1;
    }
    if let Some(path) = include_path_range(line) {
        if path.contains(&at) {
            return Some(Symbol {
                range: path,
                kind: SymbolKind::IncludePath,
            });
        }
    }
    let ch = line[at..].chars().next()?;
    if !is_identifier_char(ch) {
        return None;
    }
    let start = line[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_identifier_char(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = line[at..]
        .char_indices()
        .find(|(_, c)| !is_identifier_char(*c))
        .map_or(line.len(), |(i, _)| at + i);
    if line[start..].starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(Symbol {
        range: start..end,
        kind: SymbolKind::Identifier,
    })
}

/// `#include "x"` / `#include <x>` の引用符の中の範囲（その行が include でなければ `None`）
fn include_path_range(line: &str) -> Option<Range<usize>> {
    let trimmed = line.trim_start();
    let after_hash = trimmed.strip_prefix('#')?.trim_start();
    let after_keyword = after_hash.strip_prefix("include")?;
    let rest = after_keyword.trim_start();
    // `#includefoo` のような別の語を取り違えない（キーワードの直後は空白か引用符）
    if rest.len() == after_keyword.len() && !rest.starts_with(['"', '<']) {
        return None;
    }
    let close = match rest.chars().next()? {
        '"' => '"',
        '<' => '>',
        _ => return None,
    };
    let open_at = line.len() - rest.len();
    let inner_start = open_at + 1;
    let inner_len = line[inner_start..].find(close)?;
    (inner_len > 0).then(|| inner_start..inner_start + inner_len)
}

/// 新しいペインを置く場所（CLI `--open` / MCP `open`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// 規則どおり（新しいペインは右へ）
    Auto,
    Right,
    Down,
    NewTab,
    /// 開かない（場所だけを返す）
    None,
}

impl Placement {
    /// 綴り。`auto` は省略時と同じ（案内は常に省略形 = #322）
    pub const NAMES: [&'static str; 5] = ["auto", "right", "down", "new-tab", "none"];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "auto" => Self::Auto,
            "right" => Self::Right,
            "down" => Self::Down,
            "new-tab" => Self::NewTab,
            "none" => Self::None,
            _ => return None,
        })
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Auto => Self::NAMES[0],
            Self::Right => Self::NAMES[1],
            Self::Down => Self::NAMES[2],
            Self::NewTab => Self::NAMES[3],
            Self::None => Self::NAMES[4],
        }
    }
}

/// 新しいペインの置き方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewPane {
    Right,
    Down,
    Tab,
}

/// 着地の決め（1 か所に決まったあと）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// 同じファイル = 問い合わせたペインのまま飛ぶ
    SamePane,
    /// そのファイルを開いているペインが同じタブにある = そこを使い回す
    Reuse(PaneId),
    /// 新しいペインで開く
    New(NewPane),
}

impl Landing {
    /// 応答の `landing`
    pub fn slug(self) -> &'static str {
        match self {
            Self::SamePane => "same-pane",
            Self::Reuse(_) => "reused",
            Self::New(NewPane::Tab) => "new-tab",
            Self::New(_) => "new-pane",
        }
    }
}

/// 着地の規則（Issue #1680）。
///
/// 1. 飛び先が問い合わせたペインと**同じファイル** → 同じペイン
/// 2. 飛び先のファイルを**同じタブの別のペイン**が開いている → そのペインを使い回す
///    （同じ定義へ 2 回飛んでもペインが増えない）
/// 3. それ以外 → 新しいペイン（`placement` の向き。省略は右）
///
/// `open_in_tab` は同じタブのプレビューペイン（問い合わせたペインを除く）とそのファイル。
/// ファイルの同一性は `same_file` に委ねる（シンボリックリンク・大文字小文字の扱いは
/// 呼び手が境界 `platform::path` で決める。ここは I/O を持たない）。
/// `placement` が [`Placement::None`] のときは呼ばない（開かない）
pub fn plan_landing(
    source: &Path,
    target: &Path,
    open_in_tab: &[(PaneId, PathBuf)],
    placement: Placement,
    same_file: impl Fn(&Path, &Path) -> bool,
) -> Landing {
    if same_file(source, target) {
        return Landing::SamePane;
    }
    if let Some((pane, _)) = open_in_tab.iter().find(|(_, path)| same_file(path, target)) {
        return Landing::Reuse(*pane);
    }
    Landing::New(match placement {
        Placement::Down => NewPane::Down,
        Placement::NewTab => NewPane::Tab,
        Placement::Auto | Placement::Right | Placement::None => NewPane::Right,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 種類の綴りと_method_は_1_対_1() {
        for kind in GotoKind::ALL {
            assert_eq!(GotoKind::parse(kind.slug()), Some(kind));
            assert!(kind.method().starts_with("textDocument/"));
            assert!(kind.provider_key().ends_with("Provider"));
        }
        assert_eq!(GotoKind::parse("type_definition"), None);
        assert_eq!(
            GotoKind::TypeDefinition.method(),
            "textDocument/typeDefinition"
        );
        assert_eq!(
            GotoKind::NAMES.to_vec(),
            GotoKind::ALL.iter().map(|k| k.slug()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn 能力は真偽でもオブジェクトでも読む() {
        let caps = json!({
            "definitionProvider": true,
            "declarationProvider": false,
            "typeDefinitionProvider": { "id": "x" },
        });
        assert!(server_supports(&caps, GotoKind::Definition));
        assert!(!server_supports(&caps, GotoKind::Declaration));
        assert!(server_supports(&caps, GotoKind::TypeDefinition));
        assert!(!server_supports(&caps, GotoKind::Implementation));
        assert!(!server_supports(&Value::Null, GotoKind::Definition));
    }

    fn range(line: u64, character: u64) -> Value {
        json!({
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 3 },
        })
    }

    #[test]
    fn 応答の_4_形を_1_つの形へ() {
        // 単体
        let one = parse_locations(&json!({ "uri": "file:///a.rs", "range": range(4, 2) }));
        assert_eq!(
            one,
            vec![RawLocation {
                uri: "file:///a.rs".into(),
                line: 4,
                character: 2
            }]
        );
        // 配列（重複は畳む）
        let many = parse_locations(&json!([
            { "uri": "file:///a.rs", "range": range(4, 2) },
            { "uri": "file:///b.rs", "range": range(0, 0) },
            { "uri": "file:///a.rs", "range": range(4, 2) },
        ]));
        assert_eq!(many.len(), 2);
        assert_eq!(many[1].uri, "file:///b.rs");
        // LocationLink は識別子の範囲（targetSelectionRange）を採る
        let link = parse_locations(&json!([{
            "targetUri": "file:///c.rs",
            "targetRange": range(10, 0),
            "targetSelectionRange": range(12, 7),
        }]));
        assert_eq!((link[0].line, link[0].character), (12, 7));
        // null / 空 / 壊れた要素
        assert!(parse_locations(&Value::Null).is_empty());
        assert!(parse_locations(&json!([])).is_empty());
        assert!(parse_locations(&json!([{ "uri": "file:///x" }, 3, "s"])).is_empty());
    }

    #[test]
    fn 候補は上限で打ち切る() {
        let items: Vec<Value> = (0..(MAX_LOCATIONS as u64 + 10))
            .map(|n| json!({ "uri": "file:///a.rs", "range": range(n, 0) }))
            .collect();
        assert_eq!(parse_locations(&Value::Array(items)).len(), MAX_LOCATIONS);
    }

    #[test]
    fn uri_をパスへ戻す() {
        assert_eq!(
            path_of_uri("file:///w/src/%E6%97%A5%20a.rs", false),
            Some(PathBuf::from("/w/src/日 a.rs"))
        );
        assert_eq!(
            path_of_uri("file://localhost/w/a.rs", false),
            Some(PathBuf::from("/w/a.rs"))
        );
        // Windows 形式（ドライブ文字・区切り・UNC）
        assert_eq!(
            path_of_uri("file:///C:/w/a.rs", true),
            Some(PathBuf::from(r"C:\w\a.rs"))
        );
        assert_eq!(
            path_of_uri("file:///c%3A/w/a.rs", true),
            Some(PathBuf::from(r"c:\w\a.rs"))
        );
        assert_eq!(
            path_of_uri("file://srv/share/a.h", true),
            Some(PathBuf::from(r"\\srv\share\a.h"))
        );
        // ローカルでない / 壊れた符号化 / file 以外
        assert_eq!(path_of_uri("file://srv/share/a.h", false), None);
        assert_eq!(path_of_uri("file:///w/%zz.rs", false), None);
        assert_eq!(path_of_uri("https://example.com/a.rs", false), None);
        assert_eq!(path_of_uri("file://", false), None);
    }

    #[test]
    fn 識別子の範囲を切り出す() {
        let line = "    let value_1 = compute(x);";
        let at = line.find("value_1").unwrap();
        for offset in 0..7 {
            assert_eq!(
                symbol_at(line, at + offset),
                Some(Symbol {
                    range: at..at + 7,
                    kind: SymbolKind::Identifier
                }),
                "offset {offset}"
            );
        }
        // 識別子の外（空白・記号）
        assert_eq!(symbol_at(line, 0), None);
        assert_eq!(symbol_at(line, line.find('=').unwrap()), None);
        assert_eq!(symbol_at(line, line.len()), None);
        // 行頭・行末の識別子
        assert_eq!(symbol_at("abc", 2).map(|s| s.range), Some(0..3));
        // 数値は対象にしない
        assert_eq!(symbol_at("let n = 42;", 8), None);
        assert_eq!(symbol_at("", 0), None);
    }

    #[test]
    fn 多バイトの識別子と文字の途中() {
        // 日本語の識別子（Rust は XID を許す）と、その手前の絵文字
        let line = "😀 let 名前 = 1;";
        let at = line.find("名前").unwrap();
        let symbol = symbol_at(line, at + 1).unwrap(); // 「名」の途中のバイト
        assert_eq!(&line[symbol.range.clone()], "名前");
        // 絵文字は識別子ではない
        assert_eq!(symbol_at(line, 1), None);
    }

    #[test]
    fn include_の引用符の中は_1_つの対象() {
        let line = "#include \"sub/foo.h\" // x";
        let start = line.find("sub").unwrap();
        let end = line.find(".h\"").unwrap() + 2;
        for at in [
            start,
            line.find('/').unwrap(),
            line.find('.').unwrap(),
            end - 1,
        ] {
            assert_eq!(
                symbol_at(line, at),
                Some(Symbol {
                    range: start..end,
                    kind: SymbolKind::IncludePath
                }),
                "at {at}"
            );
        }
        // 山括弧・字下げ・# の後の空白
        let angle = "  #  include <stdio.h>";
        let s = symbol_at(angle, angle.find("stdio").unwrap()).unwrap();
        assert_eq!(&angle[s.range], "stdio.h");
        assert_eq!(s.kind, SymbolKind::IncludePath);
        // キーワードの上は識別子として扱う（include という語）
        let on_keyword = symbol_at(line, 3).unwrap();
        assert_eq!(on_keyword.kind, SymbolKind::Identifier);
        // 行末のコメントの語は識別子
        assert_eq!(
            symbol_at(line, line.len() - 1).map(|s| s.kind),
            Some(SymbolKind::Identifier)
        );
        // include でない行・閉じていない引用符・空のパス・別の語
        assert_eq!(include_path_range("#define X \"a.h\""), None);
        assert_eq!(include_path_range("#include \"a.h"), None);
        assert_eq!(include_path_range("#include \"\""), None);
        assert_eq!(include_path_range("#includex \"a.h\""), None);
        assert_eq!(
            include_path_range("#include\"a.h\"").map(|r| r.len()),
            Some(3)
        );
    }

    #[test]
    fn 置き場所の綴り() {
        for name in Placement::NAMES {
            assert_eq!(Placement::parse(name).map(Placement::slug), Some(name));
        }
        assert_eq!(Placement::parse("left"), None);
    }

    #[test]
    fn 着地の規則() {
        let eq = |a: &Path, b: &Path| a == b;
        let src = Path::new("/w/src/main.rs");
        let other = Path::new("/w/src/other.rs");
        let open = vec![(PaneId::from_raw(9), PathBuf::from("/w/src/other.rs"))];
        // 同じファイル
        assert_eq!(
            plan_landing(src, src, &open, Placement::Auto, eq),
            Landing::SamePane
        );
        // 開いているペインを使い回す（置き場所の指定より優先 = 2 回飛んでも増えない）
        assert_eq!(
            plan_landing(src, other, &open, Placement::Down, eq),
            Landing::Reuse(PaneId::from_raw(9))
        );
        // 開いていなければ新しいペイン（既定は右）
        assert_eq!(
            plan_landing(src, other, &[], Placement::Auto, eq),
            Landing::New(NewPane::Right)
        );
        assert_eq!(
            plan_landing(src, other, &[], Placement::Down, eq),
            Landing::New(NewPane::Down)
        );
        assert_eq!(
            plan_landing(src, other, &[], Placement::NewTab, eq),
            Landing::New(NewPane::Tab)
        );
        // 同一性の判定は呼び手に委ねる（大文字小文字を同一視する FS の例）
        let ci = |a: &Path, b: &Path| {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        };
        assert_eq!(
            plan_landing(src, Path::new("/W/SRC/MAIN.RS"), &[], Placement::Auto, ci),
            Landing::SamePane
        );
        assert_eq!(Landing::Reuse(PaneId::from_raw(1)).slug(), "reused");
        assert_eq!(Landing::New(NewPane::Tab).slug(), "new-tab");
    }
}
