//! GUI ライク表示モードの文言（Issue #691 / #694。キー: ui_mode.*）
//!
//! 初心者に向けた画面なので、専門語（ペイン・オーケストレーション・シェル）を
//! そのまま出さない。実コマンド（`tako master` 等）は学習経路として小さく併記する。

/// タブバートグルのツールチップ（現在ターミナル表示 → GUI 表示へ切り替える案内）
pub fn toggle_to_gui() -> &'static str {
    tr!("かんたん表示に切り替え", "Switch to the simple (GUI) view")
}

/// 同（現在 GUI 表示 → ターミナル表示へ）
pub fn toggle_to_terminal() -> &'static str {
    tr!("ターミナル表示に切り替え", "Switch to the terminal view")
}

/// スターターの見出し
pub fn starter_title() -> &'static str {
    tr!("何をしますか？", "What would you like to do?")
}

/// 見出しの補足（このペインが何なのかを 1 行で説明する）
pub fn starter_subtitle() -> &'static str {
    tr!(
        "ボタンを押すと、この画面で AI が動き始めます",
        "Pick one and the AI starts working right here"
    )
}

pub fn card_master_title() -> &'static str {
    tr!("AI チームに任せる", "Let an AI team handle it")
}

pub fn card_master_desc() -> &'static str {
    tr!(
        "司令塔の AI が話を聞いて、必要なだけ担当 AI を集めて進めます",
        "A lead AI listens to you and brings in as many helper AIs as the work needs"
    )
}

pub fn card_solo_title() -> &'static str {
    tr!("AI と 1 対 1 で話す", "Talk with one AI")
}

pub fn card_solo_desc() -> &'static str {
    tr!(
        "1 体の AI とじっくり相談したいときはこちら",
        "Best when you want to think something through with a single AI"
    )
}

pub fn card_terminal_title() -> &'static str {
    tr!("コマンド入力へ", "Go to the command line")
}

pub fn card_terminal_desc() -> &'static str {
    tr!(
        "この画面をターミナル表示に戻します（何も止まりません）",
        "Shows the terminal here instead - nothing gets stopped"
    )
}

/// カードに小さく併記する実行コマンド（言語非依存なので tr! しない）
pub const CARD_MASTER_COMMAND: &str = "tako master";
pub const CARD_SOLO_COMMAND: &str = "tako solo";

/// スターター下部の脚注。**アイコンの位置には言及しない**
/// （「右上のボタン」と書くとペインの × と紛れる。実機スクショで確認して差し替えた）
pub fn starter_footnote() -> &'static str {
    tr!(
        "これは表示の切り替えだけです。ターミナルはいつでも使えます",
        "This only changes what is shown - the terminal is always available"
    )
}

// --- チャットビュー（#702 / G2） ---

/// ヘッダ右の「ターミナルを表示」ボタン（スターターの「コマンド入力へ」と同じ動作）
pub fn chat_show_terminal() -> &'static str {
    tr!("ターミナルを表示", "Show terminal")
}

/// 生成中
pub fn chat_status_busy() -> &'static str {
    tr!("考え中…", "Thinking...")
}

/// 応答待ち（人の入力を待っている状態）
pub fn chat_status_idle() -> &'static str {
    tr!("待機中", "Ready")
}

/// busy 中に打たれた指示が claude のキューに滞留している（#572）
pub fn chat_status_queued() -> &'static str {
    tr!(
        "送信済み・生成後に届きます",
        "Sent - will be delivered after this reply"
    )
}

/// コンテキスト残量（残り N%）
pub fn chat_ctx_label(left_percent: i32) -> String {
    tr!(
        format!("残り {left_percent}%"),
        format!("{left_percent}% left")
    )
}

/// 会話がまだ 1 件も無いとき
pub fn chat_empty() -> &'static str {
    tr!(
        "まだ会話はありません。話しかけると、ここにやり取りが並びます",
        "No messages yet - your conversation will appear here"
    )
}

/// transcript ファイルがまだ作られていない（起動直後の新規セッション）
pub fn chat_transcript_pending() -> &'static str {
    tr!(
        "会話の記録を待っています",
        "Waiting for the conversation log"
    )
}

/// assistant の思考（既定は折りたたみ）
pub fn chat_thinking() -> &'static str {
    tr!("考えの過程", "Thinking")
}

/// worker ペインの説明行（入力欄の代わり。§2.4）
pub fn chat_worker_note() -> &'static str {
    tr!(
        "この AI は自動で動いています（指示は司令塔の AI から届きます）",
        "This AI runs on its own - instructions come from the lead AI"
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
                toggle_to_gui().to_string(),
                toggle_to_terminal().to_string(),
                starter_title().to_string(),
                starter_subtitle().to_string(),
                card_master_title().to_string(),
                card_master_desc().to_string(),
                card_solo_title().to_string(),
                card_solo_desc().to_string(),
                card_terminal_title().to_string(),
                card_terminal_desc().to_string(),
                starter_footnote().to_string(),
                chat_show_terminal().to_string(),
                chat_status_busy().to_string(),
                chat_status_idle().to_string(),
                chat_status_queued().to_string(),
                chat_ctx_label(42),
                chat_empty().to_string(),
                chat_transcript_pending().to_string(),
                chat_thinking().to_string(),
                chat_worker_note().to_string(),
            ]
        });
    }
}
