//! 初回起動のウェルカムバナーの文言（Issue #549。キー: welcome.*）

pub fn title() -> &'static str {
    tr!("tako へようこそ", "Welcome to tako")
}

/// ステップ 1 の説明。コマンド名は言語非依存なので本文に直接埋める（#322 の最簡形）
pub fn step_setup() -> &'static str {
    tr!(
        "1. tako setup — 初期設定と AI 連携の登録を自動で済ませる",
        "1. tako setup - configure tako and register the AI integration automatically"
    )
}

pub fn step_master() -> &'static str {
    tr!(
        "2. tako master — AI 司令塔を起動して、あとは日本語で頼む",
        "2. tako master - start the AI orchestrator and just ask in plain language"
    )
}

pub fn run_setup_button() -> &'static str {
    tr!("セットアップを実行", "Run setup")
}

pub fn run_master_button() -> &'static str {
    tr!("master を起動", "Start master")
}

pub fn open_settings_button() -> &'static str {
    tr!("設定を開く", "Open settings")
}

pub fn dismiss_hint() -> &'static str {
    tr!("閉じる（次回から表示しない）", "Dismiss (won't show again)")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                title().to_string(),
                step_setup().to_string(),
                step_master().to_string(),
                run_setup_button().to_string(),
                run_master_button().to_string(),
                open_settings_button().to_string(),
                dismiss_hint().to_string(),
            ]
        });
    }
}
