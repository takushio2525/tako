//! claude_tui — エージェント TUI の画面状態検出とプロンプト送達確認（Issue #32 / #120）
//!
//! spawn / send のプロンプト送達を「書いて祈る」から「見て・貼って・送って・確かめる」へ
//! 変えるための部品。実 TUI（claude v2.1.198 / codex 0.144.1 / agy 1.1.0）の
//! tmux capture で採取した画面を根拠にしている。
//!
//! - **対象 TUI**（Issue #120 で codex / agy に拡張）: 検出パターンは 3 種の**和集合**で、
//!   送達フロー（PromptFlow / deliver_via_tmux）はエージェント非依存。
//!   入力欄プロンプトは claude `❯`(U+276F) / codex `›`(U+203A) / agy `>`(ASCII)。
//!   `>` はシェルの PS2 等と衝突しうるため「`>` 単独 or `> `＋内容」のみ入力欄とみなす
//! - **検出**: 画面テキスト（`visible_lines` / `capture-pane`）から TUI 状態を判定する純関数群。
//!   信頼ダイアログは選択カーソルに `❯` を含むため「`❯` があれば送信可」という旧判定は誤爆する
//! - **送達**: テキスト本体は bracketed paste で貼り付け、送信の Enter は貼り付けと分離した
//!   単独キーとして遅延送信する（一括書き込みは改行が「送信」と解釈されず入力欄に残留する）。
//!   送信後に入力欄が空へ戻ったことを検証し、残っていれば Enter を単独再送する
//! - **事前信頼**: `~/.claude.json` の `projects.<cwd>.hasTrustDialogAccepted` を spawn 前に
//!   立てることで信頼ダイアログ自体を出さない。ダイアログ検出 → 承諾はそのフォールバック
//!   （codex / agy の事前信頼は `orchestrator::agent::ensure_trusted` が対応）
//!
//! 検出はヒューリスティック（TUI の文言はバージョンで変わり得る）だが、誤検知時の副作用が
//! 無害になるよう設計している: 空の入力欄への Enter は claude / codex / agy いずれも no-op
//! （3 種とも実測確認済み）。

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

// --- 画面状態の検出（純関数） ---

// --- permission ダイアログ検知（#319） ---

/// Claude Code / codex / agy の permission ダイアログ（ツール実行の承認要求）の構造化情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDialog {
    /// 承認を求めている操作の説明（画面から抽出した要約行）
    pub command: String,
    /// 選択肢のリスト（表示順。番号は含まず、テキスト部分のみ）
    pub options: Vec<String>,
    /// 現在ハイライトされている選択肢のインデックス（0-based。`❯` / `>` マーカー位置）
    pub highlighted: Option<usize>,
}

/// 画面から permission ダイアログを検知し、構造化情報を返す。
///
/// 検知パターン（実採取画面由来。claude v2.x / codex 0.x / agy 1.x）:
/// - 「Allow once」または「Allow for this session」を含む選択肢行
/// - agy の「Do you want to proceed?」+ 選択肢
/// - 信頼ダイアログ（`is_trust_dialog`）は除外（別経路で自動承諾済み）
/// - rate limit ダイアログ（`Approaching rate limits`）は除外（#157 で WORKER_ERROR）
pub fn detect_permission_dialog(lines: &[String]) -> Option<PermissionDialog> {
    if is_trust_dialog(lines) {
        return None;
    }
    if lines.iter().any(|l| l.contains("Approaching rate limits")) {
        return None;
    }

    // permission ダイアログの選択肢パターンを探す
    let has_permission_marker = lines.iter().any(|l| {
        l.contains("Allow once")
            || l.contains("Allow for this session")
            || l.contains("Always allow")
            || (l.contains("Do you want to proceed?"))
    });
    if !has_permission_marker {
        return None;
    }

    // コマンド/操作の説明を抽出: 選択肢行より上の内容行
    let mut command_parts = Vec::new();
    let mut options = Vec::new();
    let mut highlighted: Option<usize> = None;
    let mut in_choices = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Press enter")
            || trimmed.starts_with("Esc ")
        {
            continue;
        }
        // 選択カーソル ❯ / > を除去した内容テキスト。
        // ❯ は UTF-8 で 3 バイト（U+276F）、> は ASCII 1 バイト
        let (is_highlighted, inner) = if let Some(rest) = trimmed.strip_prefix("❯ ") {
            (true, rest.trim_start())
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            (true, rest.trim_start())
        } else {
            (false, trimmed)
        };

        // 番号付き選択肢行: 「N. テキスト」（N=1〜9）
        let is_numbered_choice = inner.len() > 2
            && inner.as_bytes()[0].is_ascii_digit()
            && inner.as_bytes()[1] == b'.'
            && inner.as_bytes()[2] == b' ';

        if is_numbered_choice {
            in_choices = true;
            if is_highlighted {
                highlighted = Some(options.len());
            }
            options.push(inner[3..].trim().to_string());
        } else if !in_choices {
            // 選択肢の前 = コマンド説明部分
            let desc = trimmed
                .trim_start_matches("? ")
                .trim_start_matches("❯ ")
                .trim_start_matches("> ");
            if !desc.is_empty()
                && !desc.starts_with("──")
                && !desc.contains("Navigate")
                && !desc.contains("ctrl+g")
            {
                command_parts.push(desc.to_string());
            }
        }
    }

    if options.is_empty() {
        return None;
    }

    let command = command_parts.join(" ").trim().to_string();
    Some(PermissionDialog {
        command,
        options,
        highlighted,
    })
}

/// claude TUI の画面状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeScreen {
    /// 信頼確認ダイアログ表示中（キー入力はダイアログに食われる。プロンプト送信不可）
    TrustDialog,
    /// 入力欄（❯）が空で送信可能
    Ready,
    /// 入力欄にテキストが残っている（Enter が「送信」と解釈されなかった等）
    InputPending,
    /// 応答生成中に見える（入力欄が見えない場合のみ。入力欄が見えていれば claude は
    /// busy 中でも入力を受け付ける = Ready / InputPending を優先する）
    Busy,
    /// claude TUI と判定できない（シェル・別 TUI・起動前）
    Unknown,
}

/// 画面から claude TUI の状態を判定する
pub fn detect(lines: &[String]) -> ClaudeScreen {
    if is_trust_dialog(lines) {
        return ClaudeScreen::TrustDialog;
    }
    match input_line(lines) {
        Some(content) if input_content_is_empty(content) => ClaudeScreen::Ready,
        Some(_) => ClaudeScreen::InputPending,
        None if is_busy(lines) => ClaudeScreen::Busy,
        None => ClaudeScreen::Unknown,
    }
}

/// 信頼確認ダイアログが表示されているか。
/// claude（v2.1.198: 「❯ 1. Yes, I trust this folder」）・旧文言
/// （"Do you trust the files in this folder?"）に加え、codex
/// （"Do you trust the contents of this directory?"）と agy
/// （"Do you trust the contents of this project?"）を拾う（Issue #120）。
/// いずれも承諾候補が選択済みで Enter 承諾できる。
/// 誤検知して Enter を送っても、通常画面の空入力欄では no-op なので無害。
/// agy の**許可**ダイアログ（"Do you want to proceed?"）はここに含めない
/// （コマンド実行の自動承諾はしない。skip_permissions opt-in か master の対応に委ねる）
pub fn is_trust_dialog(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        l.contains("trust this folder")
            || l.contains("trust the files")
            || l.contains("trust the contents")
    })
}

/// 入力欄の内容を返す。会話ログの送信済みメッセージも同じプロンプト文字で始まるため、
/// 入力欄 = **画面の一番下にある**プロンプト行とみなし、プロンプト文字以降を trim して返す。
/// プロンプト文字は claude `❯` / codex `›` / agy `>` の和集合（Issue #120）。
/// ASCII の `>` はシェルの PS2・リダイレクト・引用と衝突しうるため
/// 「`>` 単独 or `> `＋内容」の形のみ入力欄とみなす。
/// プロンプト行が無ければ None（エージェント TUI ではない）
pub fn input_line(lines: &[String]) -> Option<&str> {
    lines.iter().rev().find_map(|l| prompt_content(l))
}

/// 1 行がエージェント TUI の入力欄（プロンプト行）ならその内容を返す
fn prompt_content(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.strip_prefix('❯')
        .or_else(|| t.strip_prefix('›'))
        .or_else(|| match t.strip_prefix('>') {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => Some(rest),
            _ => None,
        })
        .map(str::trim)
}

/// 入力欄の内容が「空」か。空の入力欄は `❯ ` 単独、または `Try "..."` の
/// プレースホルダ付きで描画される（実画面採取より）。
/// Enter 単独送達（Issue #95）の残留判定にも使うため公開
pub fn input_content_is_empty(content: &str) -> bool {
    content.is_empty() || content.starts_with("Try \"")
}

/// 応答生成中に見えるか（advisory）。claude / codex の「esc to interrupt」ヒント、
/// agy の「esc to cancel」＋スピナー行「Generating...」、または
/// スピナーの経過秒表示（`(2s · thinking)` / `Baked for 3s` / `Working (3s` 等）を拾う
pub fn is_busy(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        l.contains("esc to interrupt")
            || l.contains("esc to cancel")
            || l.contains("Generating")
            || has_elapsed_marker(l)
    })
}

/// 「3s」のような経過秒トークンを含むか（`for 3s` / `(2s · thinking)`）
fn has_elapsed_marker(line: &str) -> bool {
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .any(|tok| {
            tok.len() >= 2
                && tok.ends_with('s')
                && tok[..tok.len() - 1].chars().all(|c| c.is_ascii_digit())
        })
}

/// プロンプト照合用の先頭断片（最初の非空行の先頭 10 文字）。
/// 画面上での折り返し・省略に耐えるよう短い断片で照合する
pub fn prompt_head(prompt: &str) -> String {
    prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim_start()
        .chars()
        .take(10)
        .collect()
}

/// 貼り付けたテキストが入力欄へ反映されたか。マルチラインの bracketed paste は
/// `[Pasted text #N +M lines]` に畳まれるため、その表示も反映とみなす
pub fn text_in_input(lines: &[String], prompt: &str) -> bool {
    let head = prompt_head(prompt);
    match input_line(lines) {
        Some(content) => {
            (!head.is_empty() && content.contains(head.as_str()))
                || content.contains("[Pasted text")
        }
        None => false,
    }
}

/// 送信（Enter）後の残留検証: 入力欄にまだプロンプト断片 / paste 表示が残っているか。
/// 残っていれば Enter が「送信」でなく「次の行」と解釈された等で未送信
pub fn input_residual(lines: &[String], prompt: &str) -> bool {
    text_in_input(lines, prompt)
}

// --- 事前信頼（Issue #32 問題 1） ---

/// spawn 前の事前信頼: `~/.claude.json` の `projects.<cwd>.hasTrustDialogAccepted` を
/// true にする。claude 起動前に呼ぶことで信頼ダイアログ自体を出さない（実機で
/// スキップされることを確認済み）。実行中の別 claude が設定ファイルを書き戻す
/// レースで負ける可能性があるため best-effort とし、失敗しても呼び出し側は
/// ダイアログ検出 → 承諾のフォールバックで継続する。
/// 戻り値: 新たに書き込んだ / 既に信頼済みなら Ok(true)
pub fn ensure_trusted(cwd: &str) -> Result<bool, String> {
    let home = crate::orchestrator::home_dir().ok_or("ホームディレクトリを特定できない")?;
    ensure_trusted_at(&home.join(".claude.json"), cwd)
}

fn ensure_trusted_at(path: &Path, cwd: &str) -> Result<bool, String> {
    let mut root: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| format!("{} を解釈できない: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("{} を読めない: {e}", path.display())),
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("{} のトップレベルがオブジェクトでない", path.display()))?;
    let projects = obj
        .entry("projects")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{} の projects がオブジェクトでない", path.display()))?;
    let entry = projects
        .entry(cwd.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{} の projects.{cwd} がオブジェクトでない", path.display()))?;
    if entry
        .get("hasTrustDialogAccepted")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Ok(true); // 既に信頼済み（書き込み不要）
    }
    entry.insert("hasTrustDialogAccepted".into(), json!(true));

    // claude 本体も読み書きするファイルのため、一時ファイル + rename で原子的に置き換える
    let tmp = path.with_extension("json.tako-tmp");
    let serialized =
        serde_json::to_string_pretty(&root).map_err(|e| format!("設定を直列化できない: {e}"))?;
    std::fs::write(&tmp, serialized).map_err(|e| format!("{} を書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} を置換できない: {e}", path.display()))?;
    Ok(true)
}

// --- tmux 経由の送達確認つき配送 ---

/// 送達レポート（E2E 検証とログ用。規約により送信テキスト自体は含めない）
#[derive(Debug, Default, Clone, Copy)]
pub struct DeliveryReport {
    /// 承諾した信頼ダイアログの回数
    pub trust_dialogs_accepted: u32,
    /// 入力欄残留に対する Enter 単独再送の回数
    pub enter_retries: u32,
    /// 入力欄が空へ戻ったことを確認できたか（false = 未検証のまま打ち切り）
    pub verified: bool,
}

/// tmux セッションへの送達確認つきプロンプト配送。
/// capture-pane で画面を見ながら 信頼ダイアログ承諾 → 貼り付け（bracketed paste）→
/// 分離 Enter → 入力欄の空検証 → Enter 単独再送 を行う。
/// `wait_ready` = true で claude TUI の入力欄（❯）表示まで待ってから貼る
/// （spawn / await_prompt 用）。false は現画面へ即貼り付け（シェル等の汎用送信。
/// 信頼ダイアログが見えている場合の承諾だけは行う）。
/// `text` が空（改行のみ含む）なら Enter 単独送達（Issue #95）: 貼り付けを
/// スキップして Enter を送り、入力欄が空へ戻るまで単独再送する
/// （入力欄に残留したテキストの送信代行）。
///
/// **ブロッキング関数**（内部で sleep する）。UI スレッドから直接呼ばず、
/// バックグラウンドスレッドで実行すること
pub fn deliver_via_tmux(
    socket: Option<&str>,
    session: &str,
    text: &str,
    wait_ready: bool,
) -> Result<DeliveryReport, String> {
    let text = text.trim_end_matches(['\n', '\r']); // 送信の Enter は分離して送るため末尾改行は落とす
    let mut report = DeliveryReport::default();

    // ① 信頼ダイアログの処理と（必要なら）入力欄待ち
    let ready_deadline = Instant::now()
        + if wait_ready {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(4)
        };
    loop {
        let lines = tako_core::tmux::capture_session(socket, session)?;
        if is_trust_dialog(&lines) {
            if report.trust_dialogs_accepted >= 3 {
                return Err("信頼ダイアログを承諾しても消えない".into());
            }
            tako_core::tmux::send_key(socket, session, "Enter")?;
            report.trust_dialogs_accepted += 1;
            std::thread::sleep(Duration::from_millis(700));
            continue;
        }
        if input_line(&lines).is_some() {
            break; // claude TUI の入力欄あり → 貼り付け可
        }
        if Instant::now() >= ready_deadline {
            if wait_ready {
                return Err("claude TUI の入力欄（❯）が現れない（タイムアウト）".into());
            }
            break; // 汎用送信: claude TUI でなくても貼り付けは通す（シェル等）
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    // ①' Enter 単独送達（Issue #95）: 入力欄の残留テキストの送信代行。
    //    素の CR 1 発は claude TUI に取りこぼされることがある（busy 中に
    //    入力欄へ溜まったテキスト等）ため、入力欄が空へ戻るまで再送する
    if text.is_empty() {
        loop {
            tako_core::tmux::send_key(socket, session, "Enter")?;
            std::thread::sleep(Duration::from_millis(700));
            let lines = tako_core::tmux::capture_session(socket, session)?;
            if input_line(&lines)
                .map(input_content_is_empty)
                .unwrap_or(true)
            {
                report.verified = true;
                return Ok(report);
            }
            if report.enter_retries >= 4 {
                return Ok(report); // verified = false のまま返す（呼び出し側がログ）
            }
            report.enter_retries += 1;
        }
    }

    // ② 本体を bracketed paste で貼り付け（アプリが要求していれば tmux -p が括りを付ける）
    tako_core::tmux::paste_text(socket, session, text)?;

    // ③ 反映確認（最大 3 秒）: 入力欄 or 画面のどこかに断片が見えるまで
    let head = prompt_head(text);
    let reflect_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < reflect_deadline {
        let lines = tako_core::tmux::capture_session(socket, session)?;
        if text_in_input(&lines, text)
            || (!head.is_empty() && lines.iter().any(|l| l.contains(head.as_str())))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // ④ 送信の Enter は貼り付けと分離した単独キーとして遅延送信する
    //    （貼り付けバーストに混ざると「次の行」と解釈される）
    std::thread::sleep(Duration::from_millis(400));
    tako_core::tmux::send_key(socket, session, "Enter")?;

    // ⑤ 検証: 入力欄が空へ戻ったか。残っていれば Enter を単独再送（最大 4 回）
    loop {
        std::thread::sleep(Duration::from_millis(700));
        let lines = tako_core::tmux::capture_session(socket, session)?;
        if !input_residual(&lines, text) {
            report.verified = true;
            return Ok(report);
        }
        if report.enter_retries >= 4 {
            return Ok(report); // verified = false のまま返す（呼び出し側がログ）
        }
        tako_core::tmux::send_key(socket, session, "Enter")?;
        report.enter_retries += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    // 実 claude TUI（v2.1.198）の tmux capture-pane から採取（個人情報はサニタイズ済み）

    const TRUST_DIALOG: &str = r#"────────────────────────────────────────────────
 Accessing workspace:

 /private/tmp/example/workdir

 Quick safety check: Is this a project you created or one you trust? (Like your own code, a
 well-known open source project, or work from your team). If not, take a moment to review what's in
 this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 ❯ 1. Yes, I trust this folder
   2. No, exit

 Enter to confirm · Esc to cancel"#;

    const READY_PLACEHOLDER: &str = r#"╭─── Claude Code v2.1.198 ───────────────────────╮
│  Welcome back ユーザー!                        │
╰────────────────────────────────────────────────╯
────────────────────────────────────────────────────
❯ Try "refactor <filepath>"
────────────────────────────────────────────────────
  ctx   0% ░░░░░░░░░░"#;

    const READY_BARE: &str = r#"❯ say only: ok

⏺ ok

✻ Baked for 3s

────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────
  ctx  20% ██░░░░░░░░"#;

    const INPUT_PENDING: &str = r#"────────────────────────────────────────────────────
❯ say only: ok
────────────────────────────────────────────────────
  ctx   0% ░░░░░░░░░░"#;

    const INPUT_PENDING_PASTED: &str = r#"────────────────────────────────────────────────────
❯ [Pasted text #1 +3 lines]
────────────────────────────────────────────────────
  paste again to expand"#;

    const INPUT_PENDING_STUCK: &str = r#"────────────────────────────────────────────────────
❯ first line of burstsecond line of burstsay only: BURST2
────────────────────────────────────────────────────
  ctx  20% ██░░░░░░░░"#;

    #[test]
    fn 信頼ダイアログを検出する() {
        let lines = screen(TRUST_DIALOG);
        assert!(is_trust_dialog(&lines));
        assert_eq!(detect(&lines), ClaudeScreen::TrustDialog);
        // ダイアログの選択カーソル ❯ を入力欄と誤認しない（旧実装の誤爆点）
        assert_ne!(detect(&lines), ClaudeScreen::Ready);
    }

    #[test]
    fn 旧文言の信頼ダイアログも検出する() {
        let lines = screen("Do you trust the files in this folder?\n❯ 1. Yes, proceed");
        assert!(is_trust_dialog(&lines));
    }

    #[test]
    fn 空入力欄をreadyと判定する() {
        // プレースホルダ付き（起動直後）と素の ❯（送信直後）の両方
        assert_eq!(detect(&screen(READY_PLACEHOLDER)), ClaudeScreen::Ready);
        assert_eq!(detect(&screen(READY_BARE)), ClaudeScreen::Ready);
    }

    #[test]
    fn 入力欄は画面最下部の行を採用する() {
        // READY_BARE は会話ログに送信済みメッセージの ❯ 行を含むが、
        // 入力欄は一番下の空の ❯ 行
        assert_eq!(input_line(&screen(READY_BARE)), Some(""));
    }

    #[test]
    fn 入力欄のテキスト残留を検出する() {
        let lines = screen(INPUT_PENDING);
        assert_eq!(detect(&lines), ClaudeScreen::InputPending);
        assert!(input_residual(&lines, "say only: ok"));
        // 別のプロンプトの断片では残留と判定しない
        assert!(!input_residual(&lines, "全く別のテキスト"));
    }

    #[test]
    fn マルチライン貼り付けはpasted_text表示で反映と判定する() {
        let lines = screen(INPUT_PENDING_PASTED);
        assert!(text_in_input(&lines, "line one\nline two\nline three"));
        assert!(input_residual(&lines, "line one\nline two\nline three"));
    }

    #[test]
    fn 改行が食われた残留テキストも先頭断片で検出する() {
        // 一括書き込みで改行が連結された実採取画面（Issue #32 問題 2 の再現）
        let lines = screen(INPUT_PENDING_STUCK);
        assert!(input_residual(
            &lines,
            "first line of burst\nsecond line of burst\nsay only: BURST2"
        ));
    }

    #[test]
    fn シェル画面はunknownと判定する() {
        let lines = screen("$ ls\nfoo bar\n$ ");
        assert_eq!(detect(&lines), ClaudeScreen::Unknown);
        assert_eq!(input_line(&lines), None);
    }

    #[test]
    fn 入力欄の空判定はプレースホルダも空とみなす() {
        // Enter 単独送達（Issue #95）の残留判定: 空 / プレースホルダ = 送信済み
        assert!(input_content_is_empty(""));
        assert!(input_content_is_empty("Try \"refactor <filepath>\""));
        assert!(!input_content_is_empty("PR #73 をマージして"));
        // 画面と組み合わせた判定（入力欄行 → 空 / 残留）
        assert_eq!(
            input_line(&screen(READY_PLACEHOLDER)).map(input_content_is_empty),
            Some(true)
        );
        assert_eq!(
            input_line(&screen(INPUT_PENDING)).map(input_content_is_empty),
            Some(false)
        );
        // ❯ 行が無い画面（シェル等）は None = 検証不能
        assert_eq!(
            input_line(&screen("$ ls")).map(input_content_is_empty),
            None
        );
    }

    #[test]
    fn busyはスピナー経過秒とescヒントで判定する() {
        assert!(is_busy(&screen("✽ Coalescing… (2s · thinking)")));
        assert!(is_busy(&screen("✻ Baked for 3s")));
        assert!(is_busy(&screen("Press esc to interrupt")));
        assert!(!is_busy(&screen("$ ls -la")));
        // 「80s」のような単語も経過秒とみなす誤検知は許容（advisory 用途のため）
    }

    #[test]
    fn prompt_headはマルチラインの最初の非空行から取る() {
        assert_eq!(
            prompt_head("\n\n  こんにちは世界これはテスト\n次の行"),
            "こんにちは世界これは"
        );
        assert_eq!(prompt_head("short"), "short");
        assert_eq!(prompt_head(""), "");
    }

    #[test]
    fn ensure_trustedは新規エントリを追加し既存キーを保持する() {
        let dir = std::env::temp_dir().join(format!("tako-trust-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude.json");
        std::fs::write(
            &path,
            r#"{"installMethod":"brew","projects":{"/existing":{"hasTrustDialogAccepted":false,"history":[1,2]}}}"#,
        )
        .unwrap();

        // 新規プロジェクトの追加
        assert_eq!(ensure_trusted_at(&path, "/new/project"), Ok(true));
        // 既存プロジェクト（false）の昇格
        assert_eq!(ensure_trusted_at(&path, "/existing"), Ok(true));
        // 冪等
        assert_eq!(ensure_trusted_at(&path, "/new/project"), Ok(true));

        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["installMethod"], "brew"); // 無関係キーを保持
        assert_eq!(
            root["projects"]["/new/project"]["hasTrustDialogAccepted"],
            true
        );
        assert_eq!(
            root["projects"]["/existing"]["hasTrustDialogAccepted"],
            true
        );
        assert_eq!(root["projects"]["/existing"]["history"], json!([1, 2])); // 既存の他キーを保持

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_trustedはファイル不在でも新規作成する() {
        let dir = std::env::temp_dir().join(format!("tako-trust-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude.json");
        assert_eq!(ensure_trusted_at(&path, "/fresh"), Ok(true));
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["projects"]["/fresh"]["hasTrustDialogAccepted"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- codex / agy の実採取画面（Issue #120。0.144.1 / 1.1.0 の tmux capture-pane より） ---

    const CODEX_TRUST_DIALOG: &str = r#"> You are in /private/tmp/example/workdir

  Do you trust the contents of this directory? Working with untrusted contents comes with higher
  risk of prompt injection. Trusting the directory allows project-local config, hooks, and exec
  policies to load.

› 1. Yes, continue
  2. No, quit

  Press enter to continue"#;

    const CODEX_READY: &str = r#"╭─────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.144.1)                      │
│                                                 │
│ model:     gpt-5.6-sol high   /model to change  │
│ directory: /private/tmp/…/scratchpad/agentprobe │
╰─────────────────────────────────────────────────╯

  Tip: When the composer is empty, press Esc to step back and edit your last message; Enter
  confirms.


› Summarize recent commits

  gpt-5.6-sol high · /private/tmp/example/workdir"#;

    const CODEX_BUSY: &str = r#"› Run this shell command: sleep 8 && echo DONE_PROBE
• I’m running the requested command now.
• Working (3s • esc to interrupt) · 1 background terminal running · /ps to view · /stop to close
› Summarize recent commits
  gpt-5.6-sol high · /private/tmp/example/workdir"#;

    const CODEX_INPUT_PENDING: &str = r#"• DONE_PROBE
────────────────────────────────────────────────────
› Reply with exactly: PROBE_OK (nothing else)
  gpt-5.6-sol high · /private/tmp/example/workdir"#;

    const AGY_TRUST_DIALOG: &str = r#"Accessing workspace:
/private/tmp/example/workdir
Do you trust the contents of this project?
Antigravity CLI requires permission to read, edit, and execute files here.
> Yes, I trust this folder
  No, exit
  ↑/↓ Navigate · enter Confirm
                                                    Claude Opus 4.6 (Thinking)"#;

    const AGY_READY: &str = r#"  Antigravity CLI 1.1.0
  Claude Opus 4.6 (Thinking)
  /private/tmp/example/workdir
────────────────────────────────────────────────────
>
────────────────────────────────────────────────────
? for shortcuts                                     Claude Opus 4.6 (Thinking)"#;

    const AGY_BUSY: &str = r#"> Run this shell command: sleep 8 && echo AGY_DONE
▸ Thought Process
  The user wants me to run a simple shell command.
⣻  Generating...
────────────────────────────────────────────────────
>
────────────────────────────────────────────────────
esc to cancel                                       Claude Opus 4.6 (Thinking)"#;

    const AGY_PERMISSION_DIALOG: &str = r#"Requesting permission for:
   sleep 8
Full command:
   sleep 8 && echo AGY_DONE
Do you want to proceed?
> 1. Yes
  2. Yes, and always allow in this conversation for commands that start with 'sleep'
  3. Yes, and always allow for commands that start with 'sleep' (Persist to settings.json)
  4. No
  ↑/↓ Navigate · tab Amend · ctrl+g edit/expand command
esc to cancel                                       Claude Opus 4.6 (Thinking)"#;

    #[test]
    fn codexの信頼ダイアログを検出する() {
        let lines = screen(CODEX_TRUST_DIALOG);
        assert!(is_trust_dialog(&lines));
        assert_eq!(detect(&lines), ClaudeScreen::TrustDialog);
    }

    #[test]
    fn codexの入力欄を検出する() {
        // プレースホルダ（動的サジェスト）付きの起動直後画面。
        // codex のプレースホルダは動的で空とは判定できないが、残留検証は
        // text_in_input（貼ったプロンプト断片との一致）なので干渉しない
        let lines = screen(CODEX_READY);
        assert_eq!(input_line(&lines), Some("Summarize recent commits"));
        // 枠線内の ">_ OpenAI Codex" を入力欄と誤認しない
        let pending = screen(CODEX_INPUT_PENDING);
        assert!(input_residual(
            &pending,
            "Reply with exactly: PROBE_OK (nothing else)"
        ));
        assert!(!input_residual(&pending, "全く別のテキスト"));
    }

    #[test]
    fn codexのbusyを検出する() {
        let lines = screen(CODEX_BUSY);
        assert!(is_busy(&lines), "Working (3s • esc to interrupt) を拾う");
        assert!(!is_busy(&screen(CODEX_READY)));
    }

    #[test]
    fn agyの信頼ダイアログを検出する() {
        let lines = screen(AGY_TRUST_DIALOG);
        assert!(is_trust_dialog(&lines));
        assert_eq!(detect(&lines), ClaudeScreen::TrustDialog);
    }

    #[test]
    fn agyの入力欄を検出する() {
        // 空入力欄（`>` 単独行）を Ready と判定する
        let lines = screen(AGY_READY);
        assert_eq!(input_line(&lines), Some(""));
        assert_eq!(detect(&lines), ClaudeScreen::Ready);
    }

    #[test]
    fn agyのbusyを検出する() {
        let lines = screen(AGY_BUSY);
        assert!(is_busy(&lines), "Generating... / esc to cancel を拾う");
        assert!(!is_busy(&screen(AGY_READY)));
    }

    #[test]
    fn agyの許可ダイアログは信頼ダイアログと誤認しない() {
        // コマンド実行の許可（Do you want to proceed?）は自動承諾の対象外。
        // trust 系マーカーに一致しないことを固定する（誤って Enter 自動承諾すると
        // 任意コマンドが承認されてしまう）
        let lines = screen(AGY_PERMISSION_DIALOG);
        assert!(!is_trust_dialog(&lines));
    }

    #[test]
    fn ascii山括弧の誤検知を防ぐ() {
        // シェルの PS2・リダイレクト・引用行を入力欄と誤認しない（`>` 直後に
        // 空白か行末が必要）。ただし PS2 の "> " は構造上区別できず許容
        assert_eq!(prompt_content(">foo"), None, "リダイレクト風は不一致");
        assert_eq!(prompt_content(">>file"), None);
        assert_eq!(prompt_content("> quoted text"), Some("quoted text"));
        assert_eq!(prompt_content(">"), Some(""));
        // 全角・枠線行は不一致
        assert_eq!(prompt_content("│ >_ OpenAI Codex │"), None);
    }

    // --- #319: permission ダイアログ検知 ---

    /// claude の Bash 承認ダイアログ（実採取相当。#312 の worker 停止時の画面）
    const CLAUDE_BASH_PERMISSION: &str = r#"  Claude wants to run:

  TAKO_ISOLATED=1 cargo run -p tako-app

  Allow this command?

❯ 1. Allow once
  2. Always allow for this session
  3. Deny

  Press enter to confirm · Esc to cancel"#;

    /// claude の Read/Write 承認ダイアログ（wait.rs の PERMISSION_DIALOG_SCREEN と同等）
    const CLAUDE_FILE_PERMISSION: &str = r#"? Claude requested permissions to write to .../main.aux
  (suspicious Windows path pattern)
❯ 1. Allow once
  2. Always allow
  3. Deny

  Press enter to confirm"#;

    #[test]
    fn claudeのbash承認ダイアログを検知する() {
        let lines = screen(CLAUDE_BASH_PERMISSION);
        let dialog = detect_permission_dialog(&lines).expect("検知される");
        assert!(
            dialog.command.contains("TAKO_ISOLATED"),
            "コマンド部分を抽出: {}",
            dialog.command
        );
        assert_eq!(dialog.options.len(), 3);
        assert_eq!(dialog.options[0], "Allow once");
        assert_eq!(dialog.options[1], "Always allow for this session");
        assert_eq!(dialog.options[2], "Deny");
        assert_eq!(dialog.highlighted, Some(0), "❯ が 1. を指している");
    }

    #[test]
    fn claudeのファイル承認ダイアログを検知する() {
        let lines = screen(CLAUDE_FILE_PERMISSION);
        let dialog = detect_permission_dialog(&lines).expect("検知される");
        assert!(dialog.command.contains("write to"));
        assert_eq!(dialog.options.len(), 3);
        assert_eq!(dialog.highlighted, Some(0));
    }

    #[test]
    fn agyの許可ダイアログを検知する() {
        let lines = screen(AGY_PERMISSION_DIALOG);
        let dialog = detect_permission_dialog(&lines).expect("検知される");
        assert!(dialog.command.contains("Do you want to proceed?"));
        assert_eq!(dialog.options.len(), 4);
        assert_eq!(dialog.options[0], "Yes");
        assert_eq!(dialog.options[3], "No");
        assert_eq!(dialog.highlighted, Some(0), "> 1. を指している");
    }

    #[test]
    fn 信頼ダイアログをpermission_dialogとして誤検知しない() {
        assert!(detect_permission_dialog(&screen(TRUST_DIALOG)).is_none());
        assert!(detect_permission_dialog(&screen(CODEX_TRUST_DIALOG)).is_none());
        assert!(detect_permission_dialog(&screen(AGY_TRUST_DIALOG)).is_none());
    }

    #[test]
    fn 通常画面をpermission_dialogとして誤検知しない() {
        assert!(detect_permission_dialog(&screen(READY_BARE)).is_none());
        assert!(detect_permission_dialog(&screen(CODEX_READY)).is_none());
        assert!(detect_permission_dialog(&screen(AGY_READY)).is_none());
        assert!(detect_permission_dialog(&screen(AGY_BUSY)).is_none());
    }
}
