//! 貼り付けの確認ダイアログの文言（FR-4.12 / #2046。キー: paste_guard.*）

use tako_core::paste_guard::Rule;

pub fn title() -> &'static str {
    tr!(
        "鍵らしき文字を貼り付けようとしています",
        "This paste looks like it contains a secret"
    )
}

pub fn question() -> &'static str {
    tr!(
        "このまま送りますか？ 送ると、ペインで動いているプログラム（claude・シェル・SSH 先など）へそのまま届きます。",
        "Send it anyway? It goes straight to the program running in the pane (claude, a shell, an SSH host and so on)."
    )
}

/// 当たった規則の名前（画面に出す側。CLI / MCP は `Rule::as_str` の綴りを返す）
pub fn rule_label(rule: Rule) -> &'static str {
    match rule {
        Rule::AnthropicApiKey => tr!("Anthropic の API キー", "Anthropic API key"),
        Rule::OpenAiApiKey => tr!("OpenAI の API キー", "OpenAI API key"),
        Rule::GithubToken => tr!("GitHub のトークン", "GitHub token"),
        Rule::GithubPat => tr!("GitHub の個人用トークン", "GitHub personal access token"),
        Rule::GitlabToken => tr!("GitLab のトークン", "GitLab token"),
        Rule::SlackToken => tr!("Slack のトークン", "Slack token"),
        Rule::SlackWebhook => tr!("Slack の Webhook URL", "Slack webhook URL"),
        Rule::DiscordWebhook => tr!("Discord の Webhook URL", "Discord webhook URL"),
        Rule::AwsAccessKey => tr!("AWS のアクセスキー", "AWS access key"),
        Rule::GoogleApiKey => tr!("Google の API キー", "Google API key"),
        Rule::StripeKey => tr!("Stripe のキー", "Stripe key"),
        Rule::NpmToken => tr!("npm のトークン", "npm token"),
        Rule::HuggingfaceToken => tr!("Hugging Face のトークン", "Hugging Face token"),
        Rule::SendgridKey => tr!("SendGrid の API キー", "SendGrid API key"),
        Rule::PrivateKey => tr!("秘密鍵", "Private key"),
        Rule::Jwt => tr!("JWT（認証トークン）", "JWT (auth token)"),
        Rule::UrlCredentials => tr!("URL に埋め込んだパスワード", "Password in a URL"),
        Rule::Assignment => tr!("パスワード・トークンの値", "Password or token value"),
        Rule::HighEntropy => tr!(
            "ランダムな文字列（鍵の可能性）",
            "Random string (possibly a key)"
        ),
    }
}

/// 当たった行（1 始まり）
pub fn line(n: usize) -> String {
    tr!(format!("{n} 行目"), format!("line {n}"))
}

/// 表に載せきれなかった件数
pub fn more(n: usize) -> String {
    tr!(format!("ほか {n} 件"), format!("{n} more"))
}

/// 巨大な貼り付けで先頭だけを確かめたとき
pub fn truncated(kb: usize) -> String {
    tr!(
        format!("先頭 {kb} KB だけを確かめました（それより後ろは見ていません）"),
        format!("Only the first {kb} KB was checked (the rest was not)")
    )
}

/// 送り先のペイン（タイトルがあれば添える）
pub fn target(pane: u64, title: Option<&str>) -> String {
    match title {
        Some(t) => tr!(
            format!("送り先: ペイン {pane}（{t}）"),
            format!("Destination: pane {pane} ({t})")
        ),
        None => tr!(
            format!("送り先: ペイン {pane}"),
            format!("Destination: pane {pane}")
        ),
    }
}

pub fn cancel() -> &'static str {
    tr!("やめる (Esc)", "Don't send (Esc)")
}

pub fn send() -> &'static str {
    tr!("送る", "Send")
}

/// キーの案内。Enter は「やめる」側（貼り付けの直後に送信のつもりで押した Enter で鍵を送らない）
pub fn keys_hint() -> &'static str {
    tr!(
        "Enter と Esc はやめる側です。送るときはボタンを押してください。この確認は設定画面の「一般」で切れます。",
        "Enter and Esc don't send. Click Send to paste it. You can turn this check off in Settings > General."
    )
}

/// 「警告だけ」の段で送った貼り付けの通知の見出し（#2050）
pub fn warned_title() -> &'static str {
    tr!(
        "鍵らしき文字を含む貼り付けをそのまま送りました",
        "Sent a paste that looks like it contains a secret"
    )
}

/// 「警告だけ」の段の案内（確認へ戻す最簡のコマンド = #322）
pub fn warned_hint() -> &'static str {
    tr!(
        "警告だけの設定です。送る前に確かめるには tako paste-guard on。押すと閉じます。",
        "Warn-only mode. To check before sending, run tako paste-guard on. Click to close."
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            let mut texts = vec![
                title().to_string(),
                question().to_string(),
                line(3),
                more(2),
                truncated(64),
                target(3, Some("zsh")),
                target(3, None),
                cancel().to_string(),
                send().to_string(),
                keys_hint().to_string(),
                warned_title().to_string(),
                warned_hint().to_string(),
            ];
            texts.extend(Rule::ALL.iter().map(|r| rule_label(*r).to_string()));
            texts
        });
    }
}
