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
pub const CARD_SETUP_COMMAND: &str = "tako setup";

/// スターター下部の控えめなリンク（#720）。カードと同格にはしない
pub fn starter_setup_link() -> &'static str {
    tr!("初期設定をやり直す", "Run the initial setup again")
}

// --- 準備中プレースホルダ（#720） ---

/// 表示種別が確定するまでの見出し
pub fn preparing_title() -> &'static str {
    tr!("準備中…", "Getting ready...")
}

/// 素のシェルの起動待ち（新しいペインを作った直後）
pub fn preparing_shell() -> &'static str {
    tr!("この画面を用意しています", "Setting this pane up for you")
}

/// エージェント TUI の起動待ち（master / solo / worker）
pub fn preparing_agent() -> &'static str {
    tr!("AI を起動しています", "Starting the AI")
}

/// スターター下部の脚注。**アイコンの位置には言及しない**
/// （「右上のボタン」と書くとペインの × と紛れる。実機スクショで確認して差し替えた）
pub fn starter_footnote() -> &'static str {
    tr!(
        "これは表示の切り替えだけです。ターミナルはいつでも使えます",
        "This only changes what is shown - the terminal is always available"
    )
}

// --- チャットビュー（#702 / G2） ---

/// 入力欄が上限行数に達して隠れているぶん（#718 / #719。無音で切り捨てない）
pub fn chat_input_more_rows(rows: usize) -> String {
    tr!(format!("上に {rows} 行"), format!("{rows} more above"))
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

// --- #715: システム注入コンテンツの表示 ---

/// 画像添付のプレースホルダ（本文が空でも「画像を送った」ことを伝える）
pub fn chat_image_attachment(count: usize) -> String {
    if count <= 1 {
        tr!("画像".to_string(), "Image".to_string())
    } else {
        tr!(format!("画像 {count} 件"), format!("{count} images"))
    }
}

/// システム通知の 1 行（`summary` は正規化層が生 XML を除いた要約）。
/// `count` は連続してまとめられた件数
pub fn chat_system_notice(summary: &str, count: u64) -> String {
    let label = tr!("システム通知", "System notice");
    let body = if summary.is_empty() {
        label.to_string()
    } else {
        format!("{label}: {summary}")
    };
    if count > 1 {
        tr!(format!("{body}（{count} 件）"), format!("{body} ({count})"))
    } else {
        body
    }
}

/// worker ペインの説明行（入力欄の代わり。§2.4）
pub fn chat_worker_note() -> &'static str {
    // #719 追加要件 5: 直接指示もできるようになったので「入力できない」とは言わない
    tr!(
        "通常は司令塔の AI が指示します（ここから直接お願いすることもできます）",
        "The lead AI usually drives this one - you can also ask it directly here"
    )
}

// --- #716 / G3: 入力・承認・スラッシュボタン ---

/// 入力欄のプレースホルダ（生成中は「あとで届く」ことを先に伝える）
pub fn chat_placeholder(busy: bool) -> String {
    if busy {
        tr!(
            "いま考え中です。続けて書くと、終わったら届きます".to_string(),
            "Still thinking - anything you write now is delivered when it finishes".to_string()
        )
    } else {
        tr!(
            "やってほしいことを書いてください".to_string(),
            "Tell the AI what you would like done".to_string()
        )
    }
}

/// 送信キーの案内（初心者向けの学習経路）
pub fn chat_send_hint() -> &'static str {
    tr!(
        "Enter で送信・Shift+Enter で改行",
        "Enter to send, Shift+Enter for a new line"
    )
}

pub fn chat_slash_compact() -> &'static str {
    tr!("会話を軽くする", "Shrink the conversation")
}

pub fn chat_slash_clear() -> &'static str {
    tr!("新しい会話", "Start over")
}

pub fn chat_slash_help() -> &'static str {
    tr!("ヘルプ", "Help")
}

/// 「新しい会話」の確認（何が失われるかを言い切る）
pub fn chat_clear_confirm_title() -> &'static str {
    tr!("新しい会話を始めますか？", "Start a new conversation?")
}

pub fn chat_clear_confirm_body() -> &'static str {
    tr!(
        "いまの会話の内容を AI が忘れます（ファイルや実行中の作業は消えません）",
        "The AI forgets this conversation. Your files and running work are untouched"
    )
}

pub fn chat_clear_cancel() -> &'static str {
    tr!("やめる", "Cancel")
}

pub fn chat_clear_ok() -> &'static str {
    tr!("新しく始める", "Start over")
}

/// 承認カードの見出し
pub fn chat_approval_title() -> &'static str {
    tr!("確認が必要です", "Your approval is needed")
}

/// 送信に失敗した（dispatch のエラーをそのまま添える）
pub fn chat_send_failed(reason: &str) -> String {
    tr!(
        format!("送信できませんでした: {reason}"),
        format!("Could not send: {reason}")
    )
}

/// 承認の応答に失敗した
pub fn chat_respond_failed(reason: &str) -> String {
    tr!(
        format!("応答できませんでした: {reason}"),
        format!("Could not respond: {reason}")
    )
}

/// 長い発話を畳んでいるときの「続きを表示」（`chars` は全体の文字数）
pub fn chat_expand_long(chars: usize) -> String {
    tr!(
        format!("続きを表示（全 {chars} 文字）"),
        format!("Show all ({chars} characters)")
    )
}

/// 展開済みの長い発話を畳む
pub fn chat_collapse_long() -> &'static str {
    tr!("折りたたむ", "Collapse")
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
                starter_setup_link().to_string(),
                preparing_title().to_string(),
                preparing_shell().to_string(),
                preparing_agent().to_string(),
                chat_input_more_rows(3),
                chat_status_busy().to_string(),
                chat_status_idle().to_string(),
                chat_status_queued().to_string(),
                chat_ctx_label(42),
                chat_empty().to_string(),
                chat_transcript_pending().to_string(),
                chat_thinking().to_string(),
                chat_worker_note().to_string(),
                chat_image_attachment(1),
                chat_image_attachment(3),
                chat_system_notice("Monitor event", 1),
                chat_system_notice("Monitor event", 4),
                chat_placeholder(false),
                chat_placeholder(true),
                chat_send_hint().to_string(),
                chat_slash_compact().to_string(),
                chat_slash_clear().to_string(),
                chat_slash_help().to_string(),
                chat_clear_confirm_title().to_string(),
                chat_clear_confirm_body().to_string(),
                chat_clear_cancel().to_string(),
                chat_clear_ok().to_string(),
                chat_approval_title().to_string(),
                // 差し込む理由は dispatch のエラー文字列なので、検査には言語非依存の値を使う
                chat_send_failed("timeout"),
                chat_respond_failed("timeout"),
                chat_expand_long(9000),
                chat_collapse_long().to_string(),
            ]
        });
    }
}
