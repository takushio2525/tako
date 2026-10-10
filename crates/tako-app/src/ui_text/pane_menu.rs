//! ペインの右クリックメニューの項目名（キー: pane_menu.*）
//!
//! #813 で自動復帰のトグルを足すにあたり、それまで render へ直書きだった
//! 既存項目もここへ集約した（1 つだけ i18n 化すると英語表示で日本語が混ざるため）

use tako_control::platform::os_integration::FileManager;

pub fn copy_path() -> &'static str {
    tr!("パスをコピー", "Copy path")
}
/// ファイルマネージャの呼び名は OS ごとに違う（#617。`sidebar::menu_reveal` と同じ理由）
pub fn reveal(fm: FileManager) -> &'static str {
    match fm {
        FileManager::Finder => tr!("Finder で表示", "Reveal in Finder"),
        FileManager::Explorer => tr!("エクスプローラーで表示", "Reveal in Explorer"),
    }
}
pub fn open_default() -> &'static str {
    tr!("デフォルトアプリで開く", "Open with default app")
}
pub fn copy_cwd() -> &'static str {
    tr!("cwd をコピー", "Copy cwd")
}
/// ディレクトリ（ペインの cwd）をファイルマネージャで開く
pub fn reveal_cwd(fm: FileManager) -> &'static str {
    match fm {
        FileManager::Finder => tr!("Finder で開く", "Open in Finder"),
        FileManager::Explorer => tr!("エクスプローラーで開く", "Open in Explorer"),
    }
}
pub fn split_right() -> &'static str {
    tr!("右に分割", "Split right")
}
pub fn split_down() -> &'static str {
    tr!("下に分割", "Split down")
}
/// このペインを SSH 接続にする（#1006）。
///
/// ファイルメニューの「リモート接続…」（新しいペインを開く）とは動作が違うので、
/// **「このペインで」を文言に入れて**取り違えを防ぐ
pub fn connect_remote() -> &'static str {
    tr!("このペインでリモート接続…", "Connect this pane via SSH…")
}
/// セッションを引き継いで再起動する 2 種（#1067）。
///
/// **どちらも「引き継ぐ」だが残るものが違う**ので、文言で差が分かるようにしてある:
/// ハーネス更新は会話がそのまま残り、引き継ぎ再起動は引き継ぎファイルに書いた分だけ残る
pub fn restart_harness() -> &'static str {
    tr!(
        "会話を保って再起動（CLI 更新）",
        "Restart keeping the chat (CLI update)"
    )
}
pub fn restart_handoff() -> &'static str {
    tr!(
        "引き継ぎを書かせて再起動",
        "Restart after writing a handoff"
    )
}

/// コードの本文を識別子の上で右クリックしたときの言語サーバの項目（#1684）。
///
/// 整形の 2 つ（#1683）とホバー（#1893）は編集メニューと同じ操作なので**同じ関数を引く**
/// （同じ操作に別の言い回しを作らない）。定義ジャンプの 4 つは VSCode / Zed の並びと呼び名に合わせる
pub fn lsp_item(item: tako_core::lsp::menu::MenuItem) -> &'static str {
    use tako_core::lsp::goto::GotoKind;
    use tako_core::lsp::menu::MenuItem;
    match item {
        MenuItem::Goto(GotoKind::Definition) => tr!("定義へ移動", "Go to Definition"),
        MenuItem::Goto(GotoKind::Declaration) => tr!("宣言へ移動", "Go to Declaration"),
        MenuItem::Goto(GotoKind::TypeDefinition) => {
            tr!("型定義へ移動", "Go to Type Definition")
        }
        MenuItem::Goto(GotoKind::Implementation) => tr!("実装へ移動", "Go to Implementation"),
        MenuItem::Hover => super::menu::show_hover(),
        MenuItem::Format => super::menu::format_document(),
        MenuItem::FormatSelection => super::menu::format_selection(),
    }
}

/// 言語サーバの握手を待っているあいだ、LSP の項目の代わりに出す押せない 1 行（#1684）
pub fn lsp_pending() -> &'static str {
    tr!(
        "言語サーバに問い合わせています…",
        "Asking the language server…"
    )
}

pub fn background() -> &'static str {
    tr!("バックグラウンドへ", "Send to background")
}
pub fn close() -> &'static str {
    tr!("閉じる", "Close")
}

/// 利用上限後の自動復帰のトグル（#813）。現在値で文言が入れ替わる
pub fn limit_resume_toggle(enabled: bool) -> &'static str {
    if enabled {
        tr!(
            "リミット後の自動復帰を無効にする",
            "Disable auto-resume after limit"
        )
    } else {
        tr!(
            "リミット後の自動復帰を有効にする",
            "Enable auto-resume after limit"
        )
    }
}

/// ペインヘッダのインジケータの説明（#813。ホバー時のツールチップ相当）
pub fn limit_resume_indicator() -> &'static str {
    tr!(
        "リミット後の自動復帰が有効",
        "Auto-resume after limit is on"
    )
}

/// ステータスバーの自動復帰の一括ボタン（#1945）。隣の「スリープ防止中」と同じく、
/// いまの状態を短い言葉で言う（全部 ON / 全部 OFF / 一部は ON の数 / 対象の数）
pub fn limit_resume_all_chip(summary: &tako_core::limit_resume_all::BulkSummary) -> String {
    use tako_core::limit_resume_all::BulkState;
    let (on, total) = (summary.on, summary.total);
    match summary.state() {
        BulkState::AllOn => tr!("自動復帰 ON", "Auto-resume on").to_string(),
        BulkState::AllOff => tr!("自動復帰 OFF", "Auto-resume off").to_string(),
        BulkState::Partial => tr!(
            format!("自動復帰 一部 {on}/{total}"),
            format!("Auto-resume {on}/{total}")
        ),
    }
}

/// 一括ボタンのツールチップ（#1945）。数・以後に立つペインの既定・押したときの向きを言葉で添える
pub fn limit_resume_all_tooltip(summary: &tako_core::limit_resume_all::BulkSummary) -> String {
    let (on, total) = (summary.on, summary.total);
    let default = if summary.default {
        tr!("有効", "on")
    } else {
        tr!("無効", "off")
    };
    let action = if summary.next_enabled() {
        tr!(
            "クリックで全エージェントの自動復帰を ON",
            "Click to turn auto-resume on for every agent"
        )
    } else {
        tr!(
            "クリックで全エージェントの自動復帰を OFF",
            "Click to turn auto-resume off for every agent"
        )
    };
    tr!(
        format!(
            "リミット後の自動復帰: エージェント {on} / {total} ペインで有効（退避中を含む）\n以後に立つペイン: {default}\n{action}"
        ),
        format!(
            "Auto-resume after limit: on in {on} of {total} agent panes (including backgrounded)\nNew panes: {default}\n{action}"
        )
    )
}

/// 一括ボタンの操作名（#1945。失敗の通知に「何を押したか」として出す）
pub fn limit_resume_all_toggle_op(enabled: bool) -> &'static str {
    if enabled {
        tr!(
            "自動復帰を全エージェントで ON",
            "Turn auto-resume on for every agent"
        )
    } else {
        tr!(
            "自動復帰を全エージェントで OFF",
            "Turn auto-resume off for every agent"
        )
    }
}

/// 全体の既定を settings.json へ残せなかったときの操作名（#1945）
pub fn limit_resume_all_save() -> &'static str {
    tr!("自動復帰の既定の保存", "Saving the auto-resume default")
}

/// 実行ペインのタイトルバーのバッジ（#1657）: まだ終了コードが届いていない
pub fn run_badge_running() -> &'static str {
    tr!("実行中", "Running")
}

/// 実行ペインのタイトルバーのバッジ（#1657）: 終了コード 0 で終わった
pub fn run_badge_succeeded() -> &'static str {
    tr!("完了", "Done")
}

/// 実行ペインのタイトルバーのバッジ（#1657）: 0 以外で終わった（終了コードを添える）
pub fn run_badge_failed(code: i32) -> String {
    tr!(format!("失敗 ({code})"), format!("Failed ({code})"))
}

/// バッジのツールチップ（#1657）。終了コードと、Enter で閉じられることを言葉で添える
pub fn run_badge_tooltip(code: Option<i32>) -> String {
    match code {
        None => tr!(
            "実行中。終わると終了コードをここに表示します".to_string(),
            "Running. The exit code will appear here when it finishes".to_string()
        ),
        Some(code) => tr!(
            format!("終了コード {code}。ペインで Enter を押すと閉じます"),
            format!("Exit code {code}. Press Enter in the pane to close it")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                copy_path().to_string(),
                reveal(FileManager::Finder).to_string(),
                reveal(FileManager::Explorer).to_string(),
                open_default().to_string(),
                copy_cwd().to_string(),
                reveal_cwd(FileManager::Finder).to_string(),
                reveal_cwd(FileManager::Explorer).to_string(),
                split_right().to_string(),
                split_down().to_string(),
                connect_remote().to_string(),
                restart_harness().to_string(),
                restart_handoff().to_string(),
                background().to_string(),
                close().to_string(),
                limit_resume_toggle(false).to_string(),
                limit_resume_toggle(true).to_string(),
                limit_resume_indicator().to_string(),
                run_badge_running().to_string(),
                run_badge_succeeded().to_string(),
                run_badge_failed(1),
                run_badge_tooltip(None),
                run_badge_tooltip(Some(1)),
                lsp_pending().to_string(),
                limit_resume_all_chip(&summary(3, 3, true)),
                limit_resume_all_chip(&summary(0, 3, false)),
                limit_resume_all_chip(&summary(1, 3, true)),
                limit_resume_all_tooltip(&summary(3, 3, true)),
                limit_resume_all_tooltip(&summary(1, 3, false)),
                limit_resume_all_toggle_op(true).to_string(),
                limit_resume_all_toggle_op(false).to_string(),
                limit_resume_all_save().to_string(),
            ]
        });
    }

    fn summary(on: usize, total: usize, default: bool) -> tako_core::limit_resume_all::BulkSummary {
        tako_core::limit_resume_all::BulkSummary { on, total, default }
    }

    /// #1945: 一括ボタンの 3 状態は言葉で見分けられる（色だけに頼らない）
    #[test]
    fn 一括ボタンの3状態は文言が違う() {
        tests_support::for_each_lang(|| {
            let texts = [
                limit_resume_all_chip(&summary(3, 3, true)),
                limit_resume_all_chip(&summary(0, 3, false)),
                limit_resume_all_chip(&summary(1, 3, true)),
            ];
            assert_ne!(texts[0], texts[1]);
            assert_ne!(texts[0], texts[2]);
            assert_ne!(texts[1], texts[2]);
            assert!(texts[2].contains("1/3"), "{}", texts[2]);
        });
    }

    /// #1684: 右クリックメニューの LSP の項目すべてに日英の名前がある（足した項目の名前の漏れを落とす）
    #[test]
    fn lsp_の項目すべてに日英の名前がある() {
        tests_support::check_ja_en(|| {
            tako_core::lsp::menu::MenuItem::ALL
                .iter()
                .map(|item| lsp_item(*item).to_string())
                .collect()
        });
    }

    /// #1684: 言語を切り替えると項目の名前が切り替わり、項目どうしで名前が重ならない
    #[test]
    fn lsp_の項目は言語を切り替えると名前が変わり重ならない() {
        use std::cell::RefCell;
        use tako_core::i18n::Lang;
        let names = |lang: Lang| {
            let out = RefCell::new(Vec::new());
            tests_support::with_lang(lang, || {
                *out.borrow_mut() = tako_core::lsp::menu::MenuItem::ALL
                    .iter()
                    .map(|item| lsp_item(*item).to_string())
                    .chain([lsp_pending().to_string()])
                    .collect();
            });
            out.into_inner()
        };
        let (ja, en) = (names(Lang::Ja), names(Lang::En));
        for (j, e) in ja.iter().zip(&en) {
            assert_ne!(j, e, "言語を切り替えても同じ名前のまま");
        }
        for list in [&ja, &en] {
            let unique: std::collections::HashSet<_> = list.iter().collect();
            assert_eq!(unique.len(), list.len(), "名前が重なっている: {list:?}");
        }
    }
}
