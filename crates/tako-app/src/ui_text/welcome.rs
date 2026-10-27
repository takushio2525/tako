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

// --- 初回起動の導線（#1510）------------------------------------------------

/// 閉じる（いまだけ。導線が続いていれば次の起動でまた出る）
pub fn later() -> &'static str {
    tr!("あとで", "Later")
}

/// 以後出さない（`welcome_dismissed` を永続化する）
pub fn never_show() -> &'static str {
    tr!("今後表示しない", "Don't show again")
}

pub fn auto_running() -> &'static str {
    tr!(
        "初期設定を自動で進めています（PATH・AI 連携の登録・指示ファイル。ネットワークからの導入はしません）",
        "Setting things up automatically (PATH, AI integration, instruction file; nothing is downloaded)"
    )
}

pub fn auto_done(changes: Option<usize>) -> String {
    match changes {
        Some(0) => tr!(
            "初期設定は済んでいます（変更なし）".to_string(),
            "Initial setup is already in place (no changes)".to_string()
        ),
        Some(n) => tr!(
            format!("初期設定を自動で済ませました（変更 {n} 件）"),
            format!("Initial setup done automatically ({n} changes)")
        ),
        None => tr!(
            "初期設定を自動で済ませました".to_string(),
            "Initial setup done automatically".to_string()
        ),
    }
}

pub fn auto_failed(reason: &str) -> String {
    tr!(
        format!("初期設定の自動実行は途中で止まりました: {reason}"),
        format!("Automatic setup stopped partway: {reason}")
    )
}

pub fn view_log() -> &'static str {
    tr!("結果を見る", "View log")
}

pub fn checking() -> &'static str {
    tr!(
        "次にやることを確かめています…",
        "Checking what to do next..."
    )
}

/// 「次にやること: 」の見出し
pub fn next_label() -> &'static str {
    tr!("次にやること: ", "Next: ")
}

pub fn install_step(product: &str) -> String {
    tr!(
        format!("{product} を入れる（ボタンを押すと、新しいタブで次のコマンドを実行します）"),
        format!("Install {product} (the button runs the command below in a new tab)")
    )
}

/// 「何をどこに」の 1 行（コマンドと置き場所）
pub fn install_what_where(command: &str, launcher: &str) -> String {
    tr!(
        format!("{command}  →  {launcher}（管理者権限は使いません）"),
        format!("{command}  ->  {launcher} (no administrator rights)")
    )
}

pub fn install_button(product: &str) -> String {
    tr!(format!("{product} を入れる"), format!("Install {product}"))
}

/// tako が導入を代行できない環境（手順を案内するタブを開く）
pub fn install_manual_step(product: &str) -> String {
    tr!(
        format!(
            "{product} を入れる（この環境では自動で入れられないので、手順を新しいタブに出します）"
        ),
        format!("Install {product} (cannot be automated here; the steps open in a new tab)")
    )
}

pub fn install_manual_button() -> &'static str {
    tr!("手順を開く", "Show steps")
}

pub fn path_step(agent: &str) -> String {
    tr!(
        format!("{agent} コマンドをどのターミナルからも使えるようにする（自動で進めます）"),
        format!("Make the {agent} command available in every terminal (done automatically)")
    )
}

pub fn path_button() -> &'static str {
    tr!("PATH を通す", "Fix PATH")
}

pub fn login_step(account: &str) -> String {
    tr!(
        format!("{account}にログインする（ブラウザが開きます）"),
        format!("Sign in to your {account} (opens a browser)")
    )
}

pub fn login_button() -> &'static str {
    tr!("ログイン", "Sign in")
}

pub fn ready_step() -> &'static str {
    tr!(
        "準備ができました。master（AI 司令塔）を起動して、あとは日本語で頼むだけです",
        "All set. Start master (the AI orchestrator) and just ask in plain language"
    )
}

pub fn unknown_step(reason: &str) -> String {
    tr!(
        format!("導入状況を確かめられませんでした（{reason}）"),
        format!("Could not check the setup state ({reason})")
    )
}

/// ボタンで開いたペインの終わりを見張っているあいだの 1 行
pub fn watching() -> &'static str {
    tr!(
        "タブで進めています。終わると次の段へ進みます",
        "In progress in the tab. The next step appears when it finishes"
    )
}

/// アカウントの呼び名（`setup_bootstrap::account_label` の英語版。日本語はそちらが正本）
pub fn account(agent: tako_core::platform::agent_install::AgentKind) -> String {
    use tako_core::platform::agent_install::AgentKind;
    tr!(
        tako_control::setup_bootstrap::account_label(agent).to_string(),
        match agent {
            AgentKind::Claude => "Claude account",
            AgentKind::Codex => "ChatGPT account",
            AgentKind::Agy => "Google account",
        }
        .to_string()
    )
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

    /// #1510: 導線の文言も日英が揃い、絵文字を使わない
    #[test]
    fn onboarding_catalog_has_both_languages_and_no_emoji() {
        use tako_core::platform::agent_install::AgentKind;
        tests_support::check_ja_en(|| {
            let mut out = vec![
                later().to_string(),
                never_show().to_string(),
                auto_running().to_string(),
                auto_done(Some(0)),
                auto_done(Some(3)),
                auto_done(None),
                auto_failed("x"),
                view_log().to_string(),
                checking().to_string(),
                next_label().to_string(),
                install_step("Claude Code"),
                install_what_where("curl", "~/.local/bin/claude"),
                install_button("Claude Code"),
                install_manual_step("Claude Code"),
                install_manual_button().to_string(),
                path_step("claude"),
                path_button().to_string(),
                login_button().to_string(),
                ready_step().to_string(),
                unknown_step("x"),
                watching().to_string(),
            ];
            for agent in AgentKind::ALL {
                out.push(login_step(&account(agent)));
            }
            out
        });
    }
}
