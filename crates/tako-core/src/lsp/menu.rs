//! 右クリックメニューの LSP 項目（S7 / #1684）の純粋部分
//!
//! コードプレビューの本文を右クリックしたとき、**識別子の上でだけ**言語サーバ由来の項目を
//! ペインのメニューへ足す（それ以外の位置は従来のメニューのまま）。
//!
//! 出し分けは**サーバの申告（`ServerCapabilities`）だけ**で決める。申告に無い機能を出すと
//! 押しても何も起きない（過大申告）。GUI のメニューと CLI `tako lsp menu` / MCP `tako_lsp` の
//! `action=menu` は同じ [`items`] を通るので、画面に出る項目と AI が読む項目は食い違わない。
//!
//! - [`MenuItem`] — 項目（安定 ID と、応じるかを決める能力）
//! - [`items`] — 能力・位置の種類・選択の有無から出す項目の列（並びもここが決める）
//! - [`selection_covers`] — 「選択範囲を整形」を出すか（右クリックした位置が選択の中か）
//!
//! 押したときに通る dispatch（`tako lsp definition` / `tako lsp format` 等）への対応は
//! `tako_control::lsp::menu` が持つ（tako-core は dispatch の綴りを知らない）。
//! **言語名はここに書かない**（検出表 `servers.rs` だけが持つ）。

use serde_json::Value;

use super::goto::{self, GotoKind, SymbolKind};
use super::{format, hover};

/// 右クリックメニューの LSP 項目
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuItem {
    /// 定義ジャンプの 4 種（⌘クリックと同じ問い合わせ。#1680）
    Goto(GotoKind),
    /// その語の型・doc のカード（編集メニューの「ホバー情報を表示」と同じ。#1681 / #1893）
    Hover,
    /// 文書全体の整形（編集メニューの「コードを整形」と同じ。#1683）
    Format,
    /// 選択範囲の整形（編集メニューの「選択範囲を整形」と同じ。#1683）
    FormatSelection,
}

impl MenuItem {
    /// 全項目。**メニューに出る順**（VSCode / Zed の並びに合わせ、移動 → 情報 → 整形）
    pub const ALL: [MenuItem; 7] = [
        MenuItem::Goto(GotoKind::Definition),
        MenuItem::Goto(GotoKind::Declaration),
        MenuItem::Goto(GotoKind::TypeDefinition),
        MenuItem::Goto(GotoKind::Implementation),
        MenuItem::Hover,
        MenuItem::Format,
        MenuItem::FormatSelection,
    ];

    /// 安定 ID（GUI のメニュー項目の id・CLI / MCP の応答の `id`）。
    /// `lsp-` で始めてペインの他の項目（`copy-path` 等）と衝突させない。
    /// `/` を含めない（`tako menu invoke` のパス区切りで割れる = #1820）
    pub fn id(self) -> &'static str {
        match self {
            MenuItem::Goto(GotoKind::Definition) => "lsp-definition",
            MenuItem::Goto(GotoKind::Declaration) => "lsp-declaration",
            MenuItem::Goto(GotoKind::TypeDefinition) => "lsp-type-definition",
            MenuItem::Goto(GotoKind::Implementation) => "lsp-implementation",
            MenuItem::Hover => "lsp-hover",
            MenuItem::Format => "lsp-format",
            MenuItem::FormatSelection => "lsp-format-selection",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.id() == id)
    }

    /// `ServerCapabilities` のキー（無い / `false` なら出さない）
    pub fn provider_key(self) -> &'static str {
        match self {
            MenuItem::Goto(kind) => kind.provider_key(),
            MenuItem::Hover => hover::PROVIDER_KEY,
            MenuItem::Format => format::provider_key(false),
            MenuItem::FormatSelection => format::provider_key(true),
        }
    }
}

/// サーバがこの項目の操作に応じるか（`true` / 登録オプションのオブジェクト = 応じる）
pub fn server_supports(capabilities: &Value, item: MenuItem) -> bool {
    match item {
        MenuItem::Goto(kind) => goto::server_supports(capabilities, kind),
        MenuItem::Hover => hover::server_supports(capabilities),
        MenuItem::Format => format::server_supports(capabilities, false),
        MenuItem::FormatSelection => format::server_supports(capabilities, true),
    }
}

/// 位置の種類から見て、その項目を出してよいか（能力は別に見る）。
///
/// - 識別子: すべて（「選択範囲を整形」は右クリックした位置が選択の中のときだけ）
/// - `#include` のパス: 定義へ移動とホバー（⌘クリックと同じくヘッダを開く・マウスを乗せたときと
///   同じくパスのカード = clangd は解決したヘッダのパスを返す。宣言・型定義・実装・整形はパスに
///   対して意味を持たない）
fn fits(item: MenuItem, kind: SymbolKind, selection_at_click: bool) -> bool {
    match kind {
        SymbolKind::IncludePath => {
            matches!(item, MenuItem::Goto(GotoKind::Definition) | MenuItem::Hover)
        }
        SymbolKind::Identifier => item != MenuItem::FormatSelection || selection_at_click,
    }
}

/// 右クリックした位置に出す LSP 項目の列（[`MenuItem::ALL`] の順）。
///
/// `symbol` は [`goto::symbol_at`] の答え（`None` = 識別子でない位置 = **0 件**）。
/// `selection_at_click` は [`selection_covers`] の答え
pub fn items(
    capabilities: &Value,
    symbol: Option<SymbolKind>,
    selection_at_click: bool,
) -> Vec<MenuItem> {
    let Some(kind) = symbol else {
        return Vec::new();
    };
    MenuItem::ALL
        .into_iter()
        .filter(|item| fits(*item, kind, selection_at_click))
        .filter(|item| server_supports(capabilities, *item))
        .collect()
}

/// 選択（`anchor` / `head` は 0 起点の行と行内バイト。向きは問わない）が位置 `at` を含むか。
/// 空の選択は何も含まない（「選択範囲を整形」を出さない）
pub fn selection_covers(anchor: (usize, usize), head: (usize, usize), at: (usize, usize)) -> bool {
    let (start, end) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    start != end && start <= at && at <= end
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(items: &[MenuItem]) -> Vec<&'static str> {
        items.iter().map(|item| item.id()).collect()
    }

    /// 受け入れ条件: 能力の 4 通り（ゼロ / 定義のみ / 全部 / 一部）で項目 ID の列が固定値と一致する
    #[test]
    fn 能力の4通りで項目の列が固定値と一致する() {
        let id = Some(SymbolKind::Identifier);
        // 能力ゼロ（握手は済んだが何も申告しない）
        assert_eq!(ids(&items(&json!({}), id, true)), Vec::<&str>::new());
        // 定義のみ
        assert_eq!(
            ids(&items(&json!({ "definitionProvider": true }), id, true)),
            ["lsp-definition"]
        );
        // 全部あり（登録オプションのオブジェクトも「応じる」）
        let all = json!({
            "definitionProvider": true,
            "declarationProvider": {},
            "typeDefinitionProvider": true,
            "implementationProvider": { "id": "impl" },
            "hoverProvider": true,
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": {},
            "completionProvider": { "triggerCharacters": ["."] },
        });
        assert_eq!(
            ids(&items(&all, id, true)),
            [
                "lsp-definition",
                "lsp-declaration",
                "lsp-type-definition",
                "lsp-implementation",
                "lsp-hover",
                "lsp-format",
                "lsp-format-selection",
            ]
        );
        // 一部だけ（`false` は申告していないのと同じ）
        let some = json!({
            "definitionProvider": false,
            "typeDefinitionProvider": true,
            "implementationProvider": false,
            "hoverProvider": false,
            "documentFormattingProvider": false,
            "documentRangeFormattingProvider": true,
        });
        assert_eq!(
            ids(&items(&some, id, true)),
            ["lsp-type-definition", "lsp-format-selection"]
        );
        // #1893: ホバーだけ（options のオブジェクトも「応じる」）
        assert_eq!(
            ids(&items(
                &json!({ "hoverProvider": { "workDoneProgress": false } }),
                id,
                false
            )),
            ["lsp-hover"]
        );
    }

    /// 受け入れ条件: 識別子でない位置では能力が全部あっても 0 件
    #[test]
    fn 識別子でない位置では0件() {
        let all = json!({
            "definitionProvider": true,
            "declarationProvider": true,
            "typeDefinitionProvider": true,
            "implementationProvider": true,
            "hoverProvider": true,
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": true,
        });
        // 位置の判定は ⌘ホバーと同じ `symbol_at`（空白・記号・数値・行末・空行）
        for (line, col) in [
            ("let a = 1;", 3),  // 空白
            ("let a = 1;", 6),  // `=`
            ("let a = 42;", 8), // 数値
            ("fn f() {}", 9),   // 行末
            ("", 0),            // 空行
            ("    // x", 2),    // 字下げ
        ] {
            let symbol = goto::symbol_at(line, col).map(|s| s.kind);
            assert_eq!(symbol, None, "{line:?} の {col} は識別子ではない");
            assert!(items(&all, symbol, true).is_empty(), "{line:?} の {col}");
        }
        assert!(items(&all, None, false).is_empty());
    }

    #[test]
    fn includeのパスは定義へ移動とホバーだけ() {
        let all = json!({
            "definitionProvider": true,
            "declarationProvider": true,
            "hoverProvider": true,
            "documentFormattingProvider": true,
        });
        let symbol = goto::symbol_at("#include \"foo.h\"", 11).map(|s| s.kind);
        assert_eq!(symbol, Some(SymbolKind::IncludePath));
        assert_eq!(
            ids(&items(&all, symbol, true)),
            ["lsp-definition", "lsp-hover"]
        );
        // 定義もホバーも能力に無ければ何も出ない
        assert!(items(&json!({ "declarationProvider": true }), symbol, true).is_empty());
    }

    #[test]
    fn 選択範囲を整形は位置が選択の中のときだけ() {
        let caps = json!({
            "documentFormattingProvider": true,
            "documentRangeFormattingProvider": true,
        });
        let id = Some(SymbolKind::Identifier);
        assert_eq!(ids(&items(&caps, id, false)), ["lsp-format"]);
        assert_eq!(
            ids(&items(&caps, id, true)),
            ["lsp-format", "lsp-format-selection"]
        );
        // 判定: 向きは問わない・端を含む・空の選択は含まない
        assert!(selection_covers((2, 4), (5, 0), (3, 9)));
        assert!(selection_covers((5, 0), (2, 4), (2, 4)));
        assert!(selection_covers((2, 4), (5, 0), (5, 0)));
        assert!(!selection_covers((2, 4), (5, 0), (2, 3)));
        assert!(!selection_covers((2, 4), (5, 0), (5, 1)));
        assert!(!selection_covers((2, 4), (2, 4), (2, 4)));
    }

    #[test]
    fn idは一意でスラッシュを含まずfrom_idで往復する() {
        let mut seen = std::collections::HashSet::new();
        for item in MenuItem::ALL {
            assert!(seen.insert(item.id()), "{} が重複", item.id());
            assert!(item.id().starts_with("lsp-"), "{}", item.id());
            // #1820: `tako menu invoke` のパス区切り
            assert!(!item.id().contains('/'), "{}", item.id());
            assert_eq!(MenuItem::from_id(item.id()), Some(item));
            // 能力のキーは申告の形（`…Provider`）
            assert!(item.provider_key().ends_with("Provider"));
        }
        assert_eq!(MenuItem::from_id("copy-path"), None);
        // 定義ジャンプの種類はすべて項目になっている（足したら項目も足す）
        for kind in GotoKind::ALL {
            assert!(MenuItem::ALL.contains(&MenuItem::Goto(kind)), "{kind:?}");
        }
    }
}
