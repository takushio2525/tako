//! 右クリックメニューの LSP 項目（S7 / #1684）の問い合わせの型と、項目 → dispatch の対応
//!
//! 出し分け（能力・位置の種類・選択 → 項目の列）の正本は `tako_core::lsp::menu::items`。
//! ここが持つのは:
//!
//! - [`MenuRequest`] / [`MenuCapabilities`] — 能力を読む問い合わせの入出力（読むのは
//!   [`super::LspManager::menu_capabilities_now`]（待たない）と
//!   [`super::LspManager::menu_capabilities`]（起動を待つ））
//! - [`action_of`] / [`item_request`] / [`item_args`] — 項目を押したときに通る dispatch。
//!   GUI のクリックも CLI / MCP も**同じ要求**になる（UI 限定の操作を作らない = 開発不変条件）
//! - [`NOT_IN_MENU`] — メニューに載せない言語機能の action と理由（番犬が
//!   「項目の action + これ = `dispatch::LSP_FEATURE_ACTIONS`」を縛る。片方だけの action を落とす）

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::lsp::menu::MenuItem;

use super::GotoError;
use crate::protocol::{LineColRange, Request};

/// 能力を読む問い合わせ 1 回ぶん（UI スレッドで作り、背景スレッドで
/// [`super::LspManager::menu_capabilities`] へ渡す）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRequest {
    /// 右クリックした文書
    pub path: PathBuf,
    /// 編集セッションがあればその全文（サーバを起こすために一時的に開くときの本文。
    /// 無ければディスクの中身 = 定義ジャンプと同じ）
    pub document: Option<String>,
    /// 起動と握手を待つ上限
    pub timeout: Duration,
}

/// 能力を**待たずに**読んだ答え
#[derive(Debug, Clone)]
pub enum MenuCapabilities {
    /// 握手が済んでいる（申告された能力）
    Ready {
        server: &'static str,
        capabilities: Value,
    },
    /// まだ握手していない（起こして待つなら [`super::LspManager::menu_capabilities`]）
    Pending,
    /// 使えないと分かっている（受け持つサーバが無い・未導入・止めた・諦めた・LSP を止めている）
    Failed(GotoError),
}

/// 項目 → `tako_lsp` の action（CLI は `tako lsp <action>`）
pub fn action_of(item: MenuItem) -> &'static str {
    match item {
        MenuItem::Goto(kind) => kind.slug(),
        MenuItem::Format | MenuItem::FormatSelection => crate::dispatch::LSP_FORMAT_ACTION,
    }
}

/// メニューに**載せない**言語機能の action と理由。
///
/// 言語機能の action（`dispatch::LSP_FEATURE_ACTIONS`）は「メニューの項目の action」か
/// 「ここ」のどちらか一方に必ず入る（番犬はこのモジュールのテスト）。新しい action
/// （ホバー・参照の検索・リネーム…）を足したら、項目にするかここへ理由を書くかを決める
pub const NOT_IN_MENU: &[(&str, &str)] = &[
    (
        "diagnostics",
        "位置ではなく文書ごとの一覧を読む（入口は波線と右パネルの diagnostics ビュー）",
    ),
    (
        crate::dispatch::LSP_FORMAT_ON_SAVE_ACTION,
        "位置の操作ではなく保存の振る舞いの設定",
    ),
    (
        crate::dispatch::LSP_COMPLETION_ACTION,
        "入口は打鍵中の一覧（右クリックから呼ぶ操作ではない）",
    ),
    (
        crate::dispatch::LSP_MENU_ACTION,
        "このメニューの中身そのものを読む口",
    ),
];

/// 項目を押したときの位置と選択（行 1 始まり・桁 0 始まりの行内 UTF-8 バイト = `tako edit replace-range`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTarget {
    pub pane: u64,
    pub line: usize,
    /// 識別子の先頭の桁（⌘クリックと同じ位置で問い合わせる）
    pub column: usize,
    /// 右クリックした位置を含む選択（「選択範囲を整形」の範囲）
    pub selection: Option<LineColRange>,
}

/// 項目を押したときの dispatch の要求。GUI のクリックはこれを `prepare_offload` へ渡す
/// （CLI `tako lsp <action>` / MCP `tako_lsp` が組む要求と同じ = 番犬が [`item_args`] を MCP の
/// 入口へ通して突き合わせる）。`focus` は GUI だけが `Some(true)`（利用者の操作は着地へ移る）。
///
/// 「選択範囲を整形」で選択が無ければ `None`（項目自体が出ない組み合わせ = [`tako_core::lsp::menu::items`]）
pub fn item_request(item: MenuItem, target: &ItemTarget, focus: Option<bool>) -> Option<Request> {
    Some(match item {
        MenuItem::Goto(kind) => Request::LspGoto {
            action: kind.slug().to_string(),
            pane: Some(target.pane),
            line: target.line,
            column: target.column,
            open: None,
            choice: None,
            focus,
        },
        MenuItem::Format => Request::LspFormat {
            pane: Some(target.pane),
            range: None,
        },
        MenuItem::FormatSelection => Request::LspFormat {
            pane: Some(target.pane),
            range: Some(target.selection?),
        },
    })
}

/// 項目の MCP 引数（`tako_lsp` へそのまま渡せる形。CLI / MCP の応答の `items[].args`）
pub fn item_args(item: MenuItem, target: &ItemTarget) -> Option<Value> {
    Some(match item {
        MenuItem::Goto(kind) => json!({
            "action": kind.slug(),
            "pane": target.pane,
            "line": target.line,
            "column": target.column,
        }),
        MenuItem::Format => json!({
            "action": action_of(item),
            "pane": target.pane,
        }),
        MenuItem::FormatSelection => {
            let range = target.selection?;
            json!({
                "action": action_of(item),
                "pane": target.pane,
                "line": range.start_line,
                "column": range.start_col,
                "end_line": range.end_line,
                "end_column": range.end_col,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::lsp::goto::GotoKind;

    fn target(selection: Option<LineColRange>) -> ItemTarget {
        ItemTarget {
            pane: 7,
            line: 3,
            column: 4,
            selection,
        }
    }

    /// 受け入れ条件（番犬）: メニューの項目の action の集合と、項目にしない action の集合を合わせると
    /// 言語機能の dispatch の action（`LSP_FEATURE_ACTIONS`）にちょうど一致し、重ならない。
    /// 片方だけの action（dispatch に足したのにメニューで扱いを決めていない・メニューにあるのに
    /// dispatch に無い）を落とす
    #[test]
    fn 項目の_action_と載せない_action_で言語機能の_action_が尽きる() {
        use std::collections::BTreeSet;
        let in_menu: BTreeSet<&str> = MenuItem::ALL.into_iter().map(action_of).collect();
        let not_in_menu: BTreeSet<&str> = NOT_IN_MENU.iter().map(|(a, _)| *a).collect();
        assert_eq!(
            not_in_menu.len(),
            NOT_IN_MENU.len(),
            "載せない action が重複"
        );
        let overlap: Vec<_> = in_menu.intersection(&not_in_menu).collect();
        assert!(
            overlap.is_empty(),
            "項目にも載せないにも入っている: {overlap:?}"
        );
        let union: BTreeSet<&str> = in_menu.union(&not_in_menu).copied().collect();
        let dispatch: BTreeSet<&str> = crate::dispatch::LSP_FEATURE_ACTIONS
            .iter()
            .copied()
            .collect();
        assert_eq!(
            union, dispatch,
            "メニューの項目の action + NOT_IN_MENU が言語機能の action と一致しない（足した action は\
             項目にするか NOT_IN_MENU へ理由つきで足す）"
        );
        for (action, reason) in NOT_IN_MENU {
            assert!(!reason.trim().is_empty(), "{action} の理由が空");
        }
    }

    #[test]
    fn 選択が無ければ選択範囲を整形の要求は作らない() {
        assert_eq!(
            item_request(MenuItem::FormatSelection, &target(None), None),
            None
        );
        assert_eq!(item_args(MenuItem::FormatSelection, &target(None)), None);
        let range = LineColRange {
            start_line: 2,
            start_col: 0,
            end_line: 5,
            end_col: 1,
        };
        assert_eq!(
            item_request(MenuItem::FormatSelection, &target(Some(range)), None),
            Some(Request::LspFormat {
                pane: Some(7),
                range: Some(range),
            })
        );
    }

    #[test]
    fn 定義ジャンプの項目は識別子の位置で問い合わせる() {
        assert_eq!(
            item_request(
                MenuItem::Goto(GotoKind::TypeDefinition),
                &target(None),
                Some(true)
            ),
            Some(Request::LspGoto {
                action: "type-definition".into(),
                pane: Some(7),
                line: 3,
                column: 4,
                open: None,
                choice: None,
                focus: Some(true),
            })
        );
    }
}
