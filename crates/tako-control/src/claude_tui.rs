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
//! - **事前信頼**: claude の `.claude.json`（`config_json_paths` が解決。既定は
//!   `~/.claude/.claude.json`）の `projects.<cwd>.hasTrustDialogAccepted` を spawn 前に
//!   立てることで信頼ダイアログ自体を出さない。ダイアログ検出 → 承諾はそのフォールバック
//!   （codex / agy の事前信頼は `orchestrator::agent::ensure_trusted` が対応）
//!
//! 検出はヒューリスティック（TUI の文言はバージョンで変わり得る）だが、誤検知時の副作用が
//! 無害になるよう設計している: 空の入力欄への Enter は claude / codex / agy いずれも no-op
//! （3 種とも実測確認済み）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

// --- 画面状態の検出（純関数） ---

// --- 選択肢ダイアログ検知（#319 permission → #748 で一般化） ---

/// 選択肢ダイアログの種別（Issue #748）。
///
/// **存在判定は構造**（`tako_core::dialog`）、**種別だけが文言**という切り分けにしている。
/// 文言リストで種別が分からなくても `Select` に落ちるだけで、ダイアログとしては
/// 検知され続ける（未知のダイアログを素通りさせない = #530 の教訓）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// ツール実行の承認要求（Allow once / Do you want to proceed?）
    Permission,
    /// フォルダ信頼（tako が自動承諾する）
    Trust,
    /// Bypass Permissions の確認（tako が ↓ + Enter で承諾する）
    Bypass,
    /// usage limit / rate limit 到達時の対処選択
    /// （claude「What do you want to do?」/ codex「Approaching rate limits」）
    UsageLimit,
    /// plan モードの実行確認（`Would you like to proceed?`）
    PlanConfirm,
    /// それ以外の選択（`/model` のモデル選択・`/mcp` の一覧・AskUserQuestion 等）
    Select,
}

impl DialogKind {
    /// JSON / イベント行に載せる機械可読 slug
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Trust => "trust",
            Self::Bypass => "bypass",
            Self::UsageLimit => "usage_limit",
            Self::PlanConfirm => "plan_confirm",
            Self::Select => "select",
        }
    }

    /// master 向けの推奨アクション（worker_status / watch の JSON にそのまま載せる）
    pub fn recommended_action(self) -> &'static str {
        match self {
            // tako 自身が承諾するので master は待てばよい（勝手に respond しない）
            Self::Trust | Self::Bypass => "auto_accept",
            // 解除まで待つ選択肢を選ぶ（他の選択肢は課金・モデル変更を伴う）
            Self::UsageLimit => "respond_wait",
            _ => "respond",
        }
    }

    /// tako が自動で承諾する種別か（master が触ってはいけないもの）
    pub fn auto_accepted(self) -> bool {
        matches!(self, Self::Trust | Self::Bypass)
    }
}

/// 画面に実在する選択肢ダイアログの構造化情報（Issue #748）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceDialog {
    /// 種別（文言由来。不明なら `Select`）
    pub kind: DialogKind,
    /// ダイアログ本文（罫線の内側だけを連結したもの）
    pub title: String,
    /// 選択肢（表示順）
    pub options: Vec<tako_core::dialog::ChoiceOption>,
    /// ハイライト位置（`options` の添字）
    pub highlighted: Option<usize>,
    /// 番号キーで選べるか（false = `↑`/`↓` 移動 + Enter が必要）
    pub numbered: bool,
}

impl ChoiceDialog {
    /// worker_status / read / watch が返す共通の JSON 形（形が食い違うと master の分岐が壊れる）
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "kind": self.kind.as_str(),
            "title": self.title,
            "numbered": self.numbered,
            "highlighted": self.highlighted,
            "recommended_action": self.kind.recommended_action(),
            "auto_accepted": self.kind.auto_accepted(),
            "options": self.options.iter().map(|o| json!({
                "number": o.number,
                "label": o.label,
                "highlighted": o.highlighted,
            })).collect::<Vec<_>>(),
        })
    }

    /// 選択肢のラベル一覧（エラーメッセージ・後方互換の `PermissionDialog` 用）
    pub fn labels(&self) -> Vec<String> {
        self.options.iter().map(|o| o.label.clone()).collect()
    }
}

/// 画面から選択肢ダイアログを検知する（Issue #748）。
///
/// 存在判定は `tako_core::dialog::detect_choice_list`（構造）、種別は文言。
/// permission だけを見ていた `detect_permission_dialog` の一般化で、
/// usage limit の対処選択・`/model`・`/mcp`・plan 確認・AskUserQuestion を
/// **同じ形**で拾えるようにする（実採取画面は `tako_core::dialog` のテスト参照）
pub fn detect_choice_dialog(lines: &[String]) -> Option<ChoiceDialog> {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let list = tako_core::dialog::detect_choice_list(&refs)?;
    let kind = classify_dialog(lines, &list);
    Some(ChoiceDialog {
        kind,
        title: list.header.join(" ").trim().to_string(),
        options: list.options,
        highlighted: list.highlighted,
        numbered: list.numbered,
    })
}

/// ダイアログの種別を文言で分類する。判定順は「復帰手段が特殊なものから」
fn classify_dialog(lines: &[String], list: &tako_core::dialog::ChoiceList) -> DialogKind {
    if is_trust_dialog(lines) {
        return DialogKind::Trust;
    }
    if is_bypass_dialog(lines) {
        return DialogKind::Bypass;
    }
    // usage limit の対処選択。文言は claude v2.1.220 のバイナリ内文字列
    // （「What do you want to do?」「Stop and wait for limit to reset」
    // 「Wait for limit to reset」）と codex の実採取（「Approaching rate limits」）由来
    let limit_option = list
        .options
        .iter()
        .any(|o| o.label.to_lowercase().contains("wait for limit to reset"));
    let limit_title = list
        .header
        .iter()
        .any(|h| h.contains("What do you want to do?"))
        && lines.iter().any(|l| l.contains("limit"));
    // 狭いペインでは見出しも選択肢も claude 自身が折り返すので、物理行で外れたら
    // 結合した論理行でも見る（#1123）。**選択肢の構造そのもの**（`tako_core::dialog`）を
    // 狭幅で読み直すのは別の話なので、ここでは種別の手がかりだけを広げる
    let limit_wrapped = !limit_option
        && !limit_title
        && tako_core::limit_resume::unwrap_wrapped_lines(lines)
            .iter()
            .any(|l| {
                l.contains("Approaching rate limits")
                    || l.to_lowercase().contains("wait for limit to reset")
                    || (l.contains("What do you want to do?") && l.contains("limit"))
            });
    if lines.iter().any(|l| l.contains("Approaching rate limits"))
        || limit_option
        || limit_title
        || limit_wrapped
    {
        return DialogKind::UsageLimit;
    }
    if lines.iter().any(|l| {
        l.contains("Allow once")
            || l.contains("Allow for this session")
            || l.contains("Always allow")
            || l.contains("Do you want to proceed?")
    }) {
        return DialogKind::Permission;
    }
    // plan モードの実行確認（実採取: 「Claude has written up a plan and is ready to
    // execute. Would you like to proceed?」）
    if lines
        .iter()
        .any(|l| l.contains("ready to execute") || l.contains("Would you like to proceed?"))
    {
        return DialogKind::PlanConfirm;
    }
    DialogKind::Select
}

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
/// - **ダイアログが入力欄を奪っていること**（`is_choice_dialog`。#577）
/// - 信頼ダイアログ（`is_trust_dialog`）は除外（別経路で自動承諾済み）
/// - rate limit ダイアログ（`Approaching rate limits`）は除外（#157 で WORKER_ERROR）
///
/// **「実在検査」であることが本関数の契約**（#577）。worker の停止種別を
/// question から permission へ格上げする根拠になるため、文言だけで判定すると
/// エージェントが本文に書いた「Do you want to proceed? / 1. はい / 2. いいえ」を
/// ダイアログと誤認し、`WORKER_IDLE` が永久に出なくなる（#571 と同じ不検知）。
/// そこで「画面最下部のプロンプト行が選択カーソル」= 入力欄がダイアログに
/// 奪われている状態を必要条件にする。生成中・通常応答中は入力欄が最下部に
/// 見えている（claude は busy 中も入力を受け付ける）ので構造で切り分けられる
pub fn detect_permission_dialog(lines: &[String]) -> Option<PermissionDialog> {
    let dialog = detect_choice_dialog(lines)?;
    if dialog.kind != DialogKind::Permission {
        // trust / bypass は別経路で自動承諾、usage limit は #157 の WORKER_ERROR、
        // その他の選択は #748 の `choice_dialog` が扱う
        return None;
    }
    Some(PermissionDialog {
        command: dialog.title.clone(),
        options: dialog.labels(),
        highlighted: dialog.highlighted,
    })
}

/// 選択ダイアログが表示されているか（Issue #530 → #748 で番号なしも対象）。
///
/// **文言ではなく構造で判定する**のが要点。`is_trust_dialog` / `is_bypass_dialog` は
/// 既知の文言に依存するため、未知のダイアログを素通りさせる。実際 `CLAUDE_CONFIG_DIR`
/// を切り替えた初回起動では claude がテーマ選択（`❯ 2. Dark mode ✔`）とログイン方法選択
/// （`❯ 1. Claude account with subscription …`）を出し、これらの選択カーソル行が
/// 入力欄（`❯`）と同じ字面のため `input_line` が「入力欄あり」と誤認していた。
/// その結果プロンプトがダイアログに食われて消え、後段の「入力欄が空 = 送信成功」判定が
/// 偽陽性になる（#530 の根因）。
///
/// 判定の実装は `tako_core::dialog`（番号つき / 番号なしの 2 経路）に 1 本化してある。
/// **入力欄判定・送達フロー・worker 状態がすべて同じ関数を通る**のが要点で、
/// 片方だけ古い判定を使っていると `/mcp` の一覧行が入力欄に見える（#748 の観測 1）
pub fn is_choice_dialog(lines: &[String]) -> bool {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    tako_core::dialog::is_choice_dialog(&refs)
}

/// claude TUI の画面状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeScreen {
    /// 信頼確認ダイアログ表示中（キー入力はダイアログに食われる。プロンプト送信不可）
    TrustDialog,
    /// 番号付き選択ダイアログ表示中（テーマ選択・ログイン方法選択等。Issue #530）。
    /// 信頼ダイアログと同様にキー入力を食うのでプロンプト送信不可。ただし内容が
    /// 不明なため自動承諾はしない（勝手に選択を確定させると意図しない設定が入る）
    ChoiceDialog,
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
    if is_choice_dialog(lines) {
        return ClaudeScreen::ChoiceDialog;
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

/// Bypass Permissions 確認ダイアログが表示されているか（Issue #407）。
/// `claude --dangerously-skip-permissions` の初回起動で表示される:
/// ```text
/// WARNING: Claude Code running in Bypass Permissions mode
/// ...
/// ❯ 1. No, exit
///   2. Yes, I accept
/// ```
/// 既定選択が「No, exit」のため、信頼ダイアログと異なり Enter だけでは突破できず、
/// 「↓ + Enter」で「Yes, I accept」を確定する必要がある
pub fn is_bypass_dialog(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|l| l.contains("Bypass Permissions mode") || l.contains("Bypass permissions mode"))
        && lines.iter().any(|l| l.contains("Yes, I accept"))
}

/// 入力欄の内容を返す。会話ログの送信済みメッセージも同じプロンプト文字で始まるため、
/// 入力欄 = **画面の一番下にある**プロンプト行とみなし、プロンプト文字以降を trim して返す。
/// プロンプト文字は claude `❯` / codex `›` / agy `>` の和集合（Issue #120）。
/// ASCII の `>` はシェルの PS2・リダイレクト・引用と衝突しうるため
/// 「`>` 単独 or `> `＋内容」の形のみ入力欄とみなす。
/// プロンプト行が無ければ None（エージェント TUI ではない）。
///
/// 番号付き選択ダイアログ（`is_choice_dialog`）の選択カーソルは同じ字面だが
/// 入力欄ではないため None を返す（Issue #530。ここを Some で返していたため
/// プロンプトがダイアログに食われていた）
pub fn input_line(lines: &[String]) -> Option<&str> {
    if is_choice_dialog(lines) {
        return None;
    }
    bottom_prompt_content(lines)
}

/// 画面最下部のプロンプト記号行の内容（選択ダイアログのガード無し。内部用）
fn bottom_prompt_content(lines: &[String]) -> Option<&str> {
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

/// claude がメッセージキューの滞留時に入力欄へ出すヒント（#572）。
/// claude v2.1.220 のバイナリ内文字列および実採取画面の両方で確認済み
pub const QUEUED_MESSAGES_HINT: &str = "Press up to edit queued messages";

/// 入力欄に **ユーザー入力の代わりに** 表示されるプレースホルダの先頭（実採取画面より）。
///
/// claude はこれらを **dim（`ESC[2m`）** で描画する = ユーザーが打った文字ではない。
/// ここに載っていないプレースホルダを「残留テキスト」と誤認すると、Enter 単独送達
/// （#95）が永久に空振りし、送達検証も偽陰性になる（#572 の master の観測がこれ）
const INPUT_PLACEHOLDERS: &[&str] = &[
    // 空欄時の使用例（`Try "fix the build"` 等）
    "Try \"",
    // キューに未送信メッセージがある（#572）
    QUEUED_MESSAGES_HINT,
];

/// 入力欄の内容が「空」か。空の入力欄は `❯ ` 単独、または dim のプレースホルダ付きで
/// 描画される（実画面採取より）。
/// Enter 単独送達（Issue #95）の残留判定にも使うため公開
pub fn input_content_is_empty(content: &str) -> bool {
    content.is_empty() || is_input_placeholder(content)
}

/// 入力欄の内容が claude のプレースホルダ（ユーザー入力ではない）か（#572）
pub fn is_input_placeholder(content: &str) -> bool {
    INPUT_PLACEHOLDERS.iter().any(|p| content.starts_with(p))
}

/// claude のメッセージキューに **未送信のまま滞留した** メッセージがあるか（#572）。
///
/// busy 中に打った指示は claude のキューへ入り、通常はターン終了時に送信される。
/// ところがターン終了の直前に入ったものはドレインされずキューに残り、以後 Enter を
/// 何回送っても送信されない（入力欄自体は空なので Enter は no-op）。
/// この状態で claude は入力欄に `QUEUED_MESSAGES_HINT` を出すので、それを根拠に検知する。
///
/// 判定は「画面最下部のプロンプト行の内容がヒント文言」= 入力欄が空でキューが非空。
/// ユーザーが何か打ち始めるとヒントは消えるため、**滞留していても検知できない状態**は
/// 「ユーザーが今まさに入力中」= 触ってはいけない状態と一致する（自動復旧の安全弁）
pub fn queued_messages_pending(lines: &[String]) -> bool {
    bottom_prompt_content(lines).is_some_and(|c| c.starts_with(QUEUED_MESSAGES_HINT))
}

/// 画面が「もう動いていない」と言えるか（#572）。
///
/// `is_busy` はスピナー行の文言に依存するため、生成中でもその行が画面外・別表示に
/// なっていると false を返す（実測: 実 claude の 120 行リスト生成中に false）。
/// キュー救出のように **誤って割り込むと壊れる** 判定では、文言ではなく
/// **画面が一定時間まったく変化していないこと** を主たる根拠にする
pub fn screen_settled(before: &[String], after: &[String]) -> bool {
    before == after && !is_busy(after)
}

// --- 入力欄の dim 判定（#572。文言に依存しない構造判定） ---

/// 先頭が SGR（`ESC[<数字と ; >*m`）ならその「パラメータ部」と全体のバイト長を返す。
/// SGR 以外のエスケープ（`ESC[2J` 等）で本文を食い潰さないよう、
/// パラメータが数字と `;` だけで終端が `m` のときに限って SGR とみなす
fn sgr_at(s: &str) -> Option<(&str, usize)> {
    let body = s.strip_prefix("\u{1b}[")?;
    let end = body.find(|c: char| !c.is_ascii_digit() && c != ';')?;
    if body.as_bytes()[end] != b'm' {
        return None;
    }
    Some((&body[..end], "\u{1b}[".len() + end + 1))
}

/// エスケープ付き（`capture-pane -p -e`）の 1 行から SGR を取り除く
pub fn strip_sgr(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find('\u{1b}') {
        out.push_str(&rest[..i]);
        match sgr_at(&rest[i..]) {
            Some((_, len)) => rest = &rest[i + len..],
            None => {
                out.push('\u{1b}');
                rest = &rest[i + '\u{1b}'.len_utf8()..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// エスケープ付き画面から、**入力欄のテキストが全て dim か**を判定する（#572）。
///
/// claude は「ユーザーが打った文字」を通常輝度、「プレースホルダ / AI のゴースト提案」を
/// dim（`ESC[2m`）で描画する（#107 が GUI 側で使っている性質と同じ）。文言リストは
/// AI 生成のゴースト提案（例: `now do 1 to 50`）を原理的に網羅できないため、
/// tmux 経路でも属性を根拠にする。
///
/// 戻り値: `None` = 入力欄が見つからない / 内容が空、`Some(true)` = 全て dim
/// （＝ユーザー入力ではない）、`Some(false)` = 通常輝度の文字を含む（＝ユーザー入力あり）
pub fn input_text_is_all_dim(escaped_lines: &[String]) -> Option<bool> {
    // 素の行で入力欄の位置（下から最初のプロンプト行）を決め、同じ行を属性つきで見る
    let idx = escaped_lines
        .iter()
        .rposition(|l| prompt_content(&strip_sgr(l)).is_some())?;
    let raw = &escaped_lines[idx];

    let mut dim = false;
    let mut seen_prompt = false;
    let mut has_dim = false;
    let mut has_normal = false;
    // **属性の適用順に注意**: エスケープの手前の文字は「そのエスケープを適用する前」の
    // 属性で描かれている。先に SGR を反映すると 1 チャンク分ずれる
    let mut classify = |chunk: &str, dim: bool, seen_prompt: &mut bool| {
        for ch in chunk.chars() {
            if !*seen_prompt {
                if ch == '❯' || ch == '›' || ch == '>' {
                    *seen_prompt = true;
                }
                continue;
            }
            if ch.is_whitespace() {
                continue;
            }
            if dim {
                has_dim = true;
            } else {
                has_normal = true;
            }
        }
    };
    let mut rest = raw.as_str();
    while let Some(i) = rest.find('\u{1b}') {
        classify(&rest[..i], dim, &mut seen_prompt);
        match sgr_at(&rest[i..]) {
            Some((params, len)) => {
                for p in params.split(';') {
                    match p {
                        "2" => dim = true,
                        // 0（リセット）/ 22（通常輝度）/ 空（= 0 相当）
                        "0" | "22" | "" => dim = false,
                        _ => {}
                    }
                }
                rest = &rest[i + len..];
            }
            // SGR ではないエスケープ（tmux -e は出さない想定）は 1 バイト読み飛ばす
            None => rest = &rest[i + '\u{1b}'.len_utf8()..],
        }
    }
    classify(rest, dim, &mut seen_prompt);
    if !has_dim && !has_normal {
        return None; // 入力欄は空
    }
    Some(!has_normal)
}

/// エスケープ付き画面から「入力欄にユーザーが打った実テキストがあるか」を判定する（#572）。
/// dim だけの内容（プレースホルダ / ゴースト提案）は **無い** と扱う
pub fn input_has_user_text(escaped_lines: &[String]) -> bool {
    matches!(input_text_is_all_dim(escaped_lines), Some(false))
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

/// 生成中の**強いシグナル**（中断ヒントが画面に出ている）。#1067
///
/// [`is_busy`] は経過秒トークン（`for 3s`）も拾う advisory なので、
/// **生成が終わった後も画面に残る**完了行（実測: `✻ Brewed for 2s · done 9:22 PM`）を
/// busy と読んでしまう。アイドルなペインを永久に busy と申告するので、
/// **「人の操作でプロセスを終わらせてよいか」の判断には使えない**（#1067 で実測）。
///
/// ここでは中断の案内（= その瞬間に中断できる何かが走っている）だけを見る。
/// 取りこぼしても他の関門（キュー滞留・入力欄の下書き）が残るので、
/// 誤って「busy でない」と言う側より**誤って「busy」と言い続ける側**を避ける
pub fn interrupt_hint_visible(lines: &[String]) -> bool {
    lines.iter().any(|l| {
        l.contains("esc to interrupt") || l.contains("esc to cancel") || l.contains("Generating")
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

/// 生成中の**ライブなスピナー行**を返す（#719）。
///
/// claude は生成中、会話の下に `✻ Manifesting… (5m 16s · ↓ 16.4k tokens)` の 1 行を
/// 出す（実採取画面）。「何をしているか」「経過時間」「受信トークン数」が全部
/// 入っているので、チャットビューはこれをそのまま見せる（独自に数え直さない）。
///
/// 呼び出し側は**入力ボックスより上の行だけ**を渡すこと。フッターの `(→4h44m)`
/// のような括弧つき表示を拾わないための約束
pub fn activity_line(lines: &[String]) -> Option<String> {
    // 下から探して最初に見つかったものが「いまの」スピナー行
    lines.iter().rev().take(20).find_map(|line| {
        let t = line.trim();
        // 経過時間つきの短い 1 行だけを相手にする（本文の混入を避ける）
        if t.chars().count() > 120 || !t.contains('(') || !has_elapsed_marker(t) {
            return None;
        }
        // 先頭のスピナー記号（✻ ✽ ✢ · * 等）と余白を落として本文だけにする
        let body = t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
        (!body.is_empty()).then(|| body.to_string())
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

// --- 設定ファイル（.claude.json）の場所（Issue #558） ---

/// claude が読み書きする `.claude.json` のパスを解決する。
///
/// **claude は config ディレクトリ配下の `.claude.json` を使う**
/// （`$CLAUDE_CONFIG_DIR/.claude.json`、未設定なら `~/.claude/.claude.json`）。
/// v2.1.220 で実測: 未信頼フォルダの信頼を承諾すると
/// `~/.claude/.claude.json` の `projects` に記録され、ホーム直下の
/// `~/.claude.json` は一切変化しない。tako は長らくホーム直下へ書いていたため、
/// 事前信頼（#32）と bypass 事前承認（#407）が無効化されていた（#558）。
///
/// 返すのは書き込み対象のパス列（先頭が主）。ホーム直下の旧ファイルは
/// **既に存在する場合だけ**併せて更新する（旧バージョンの claude を使う環境への互換。
/// 存在しないなら新規作成はしない = 現行 claude が読まないファイルを増やさない）。
///
/// `config_dir` は呼び出し側が知っている config ディレクトリ（アカウント指定の
/// `CLAUDE_CONFIG_DIR` 等）。None なら環境変数 → 既定 `~/.claude` の順に解決する
pub fn config_json_paths(config_dir: Option<&str>) -> Vec<PathBuf> {
    let dir = config_dir
        .map(|d| PathBuf::from(crate::orchestrator::expand_tilde(d)))
        .or_else(|| {
            std::env::var_os(crate::orchestrator::CLAUDE_CONFIG_DIR_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .or_else(crate::orchestrator::claude_default_config_dir);
    let home = crate::orchestrator::home_dir();
    let legacy_exists = home
        .as_ref()
        .is_some_and(|h| h.join(".claude.json").is_file());
    resolve_config_json_paths(dir, home.as_deref(), legacy_exists)
}

/// `config_json_paths` の解決規則そのもの（ファイルシステムに触らない純関数）。
/// 実在判定は呼び出し側が済ませて `legacy_exists` で渡す
fn resolve_config_json_paths(
    config_dir: Option<PathBuf>,
    home: Option<&Path>,
    legacy_exists: bool,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = config_dir {
        paths.push(dir.join(".claude.json"));
    }
    // 旧世代の置き場所（ホーム直下）。現行 claude は読まないが、既存環境では
    // 旧バージョンの claude が残っている可能性があるので存在すれば併せて更新する
    if let Some(home) = home {
        let legacy = home.join(".claude.json");
        if legacy_exists && !paths.contains(&legacy) {
            paths.push(legacy);
        }
    }
    paths
}

/// 書き込み対象が 1 つも解決できないときのエラー文言
fn no_config_path_err() -> String {
    "claude の設定ファイル（.claude.json）の場所を特定できない".to_string()
}

// --- 事前信頼（Issue #32 問題 1） ---

/// spawn 前の事前信頼: claude の `.claude.json` の
/// `projects.<cwd>.hasTrustDialogAccepted` を true にする。claude 起動前に呼ぶことで
/// 信頼ダイアログ自体を出さない（実機でスキップされることを確認済み）。
/// 実行中の別 claude が設定ファイルを書き戻すレースで負ける可能性があるため
/// best-effort とし、失敗しても呼び出し側はダイアログ検出 → 承諾のフォールバックで継続する。
/// 戻り値: 新たに書き込んだ / 既に信頼済みなら Ok(true)
pub fn ensure_trusted(cwd: &str) -> Result<bool, String> {
    ensure_trusted_in(None, cwd)
}

/// config ディレクトリを明示する版（Issue #558）。アカウント指定で
/// `CLAUDE_CONFIG_DIR` を注入して spawn する場合、信頼はその config dir 配下の
/// `.claude.json` に書かないと効かない
pub fn ensure_trusted_in(config_dir: Option<&str>, cwd: &str) -> Result<bool, String> {
    let paths = config_json_paths(config_dir);
    if paths.is_empty() {
        return Err(no_config_path_err());
    }
    let mut ok = false;
    let mut last_err = None;
    for path in &paths {
        match ensure_trusted_at(path, cwd) {
            Ok(_) => ok = true,
            Err(e) => last_err = Some(e),
        }
    }
    // 主（config dir 配下）が書ければ成功。旧ファイル側の失敗だけなら握り潰さず
    // 呼び出し側の警告に載せる
    if ok {
        Ok(true)
    } else {
        Err(last_err.unwrap_or_else(no_config_path_err))
    }
}

// --- Bypass Permissions 事前承認（Issue #407） ---

/// `--dangerously-skip-permissions` の初回確認ダイアログを抑制する。
/// `.claude.json` のルートに `bypassPermissionsModeAccepted: true` を書き込む。
/// claude CLI のソースで `ensureAgentsBypassConsent` がこのフラグを参照し、
/// true ならダイアログ表示自体をスキップする（v2.1.215 で実測確認済み）。
/// ensure_trusted と同様に best-effort: 失敗時は deliver_via_tmux のダイアログ検出
/// → 承諾がフォールバックする
pub fn ensure_bypass_accepted() -> Result<bool, String> {
    ensure_bypass_accepted_in(None)
}

/// config ディレクトリを明示する版（Issue #558）
pub fn ensure_bypass_accepted_in(config_dir: Option<&str>) -> Result<bool, String> {
    let paths = config_json_paths(config_dir);
    if paths.is_empty() {
        return Err(no_config_path_err());
    }
    let mut ok = false;
    let mut last_err = None;
    for path in &paths {
        match ensure_bypass_accepted_at(path) {
            Ok(_) => ok = true,
            Err(e) => last_err = Some(e),
        }
    }
    if ok {
        Ok(true)
    } else {
        Err(last_err.unwrap_or_else(no_config_path_err))
    }
}

fn ensure_bypass_accepted_at(path: &Path) -> Result<bool, String> {
    let mut root: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| format!("{} を解釈できない: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("{} を読めない: {e}", path.display())),
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("{} のトップレベルがオブジェクトでない", path.display()))?;
    if obj
        .get("bypassPermissionsModeAccepted")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return Ok(true);
    }
    obj.insert("bypassPermissionsModeAccepted".into(), json!(true));

    let tmp = path.with_extension("json.tako-tmp");
    let serialized =
        serde_json::to_string_pretty(&root).map_err(|e| format!("設定を直列化できない: {e}"))?;
    std::fs::write(&tmp, serialized).map_err(|e| format!("{} を書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} を置換できない: {e}", path.display()))?;
    Ok(true)
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
    /// claude のメッセージキューから取り出して送った回数（#572。`Up` → `Enter`）
    pub queued_drained: u32,
}

/// キュー滞留からの復旧で取り出しに使うキー（#572）。claude 自身が
/// `Press up to edit queued messages` と案内している操作をそのまま使う
pub const QUEUE_RECALL_KEY: &str = "Up";

/// 復旧の試行上限（1 回の送達につき）。滞留が解けない場合に無限に叩かない
pub const QUEUE_DRAIN_MAX: u32 = 4;

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
        // Bypass Permissions 確認ダイアログ（#407）: 既定選択が「No, exit」のため
        // ↓ で「Yes, I accept」へ移動してから Enter で確定する
        if is_bypass_dialog(&lines) {
            if report.trust_dialogs_accepted >= 3 {
                return Err("Bypass 確認ダイアログを承諾しても消えない".into());
            }
            tako_core::tmux::send_key(socket, session, "Down")?;
            std::thread::sleep(Duration::from_millis(200));
            tako_core::tmux::send_key(socket, session, "Enter")?;
            report.trust_dialogs_accepted += 1;
            std::thread::sleep(Duration::from_millis(700));
            continue;
        }
        // 未知の番号付き選択ダイアログ（テーマ選択・ログイン方法選択等。#530）。
        // 内容が不明なため自動承諾はせず、消えるまで待つ。ここで貼るとテキストが
        // ダイアログに食われて消え、後段の空検証が偽陽性になる。
        // Enter 単独送達（text 空。#95）は「Enter を送れ」という明示要求なので対象外
        let choice_dialog = !text.is_empty() && is_choice_dialog(&lines);
        if !choice_dialog && input_line(&lines).is_some() {
            break; // エージェント TUI の入力欄あり → 貼り付け可
        }
        if Instant::now() >= ready_deadline {
            if choice_dialog {
                return Err(
                    "選択ダイアログ（テーマ・ログイン方法等）が表示されたままで入力欄が現れない\
                     （タイムアウト）。ペインで選択を確定してから再送する"
                        .into(),
                );
            }
            if wait_ready {
                return Err("claude TUI の入力欄（❯）が現れない（タイムアウト）".into());
            }
            break; // 汎用送信: claude TUI でなくても貼り付けは通す（シェル等）
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    // ①' Enter 単独送達（Issue #95）: 入力欄の残留テキストの送信代行。
    //    素の CR 1 発は claude TUI に取りこぼされることがある（busy 中に
    //    入力欄へ溜まったテキスト等）ため、入力欄が空へ戻るまで再送する。
    //
    //    #572: 「入力欄が空に見えない」原因の大半は残留ではなく **claude の dim
    //    プレースホルダ**（キュー滞留ヒント / ゴースト提案）だった。素の文字列だけを
    //    見ると残留と区別できず、Enter を 5 回空撃ちして未検証で終わっていた。
    //    ここでは属性つき採取（`-e`）で dim を見て「ユーザーが打った実テキスト」だけを
    //    残留とみなし、キューに未送信メッセージがあるなら Up で取り出して送る
    if text.is_empty() {
        tako_core::tmux::send_key(socket, session, "Enter")?;
        let mut prev_plain: Option<Vec<String>> = None;
        loop {
            std::thread::sleep(Duration::from_millis(700));
            let styled = tako_core::tmux::capture_session_styled(socket, session)?;
            let plain: Vec<String> = styled.iter().map(|l| strip_sgr(l)).collect();

            // キュー滞留（#572）: 入力欄は本当に空なので Enter は no-op。
            // claude 自身の案内どおり Up で取り出してから Enter で送る。
            // **生成中には絶対に触らない**（送り直しても再キューされるだけで、
            // ターン終了時に claude 自身がドレインする。実測: probe v9）。
            // 生成中かは文言ではなく「画面が 700ms 変化していない」で見る（#572）
            let settled = prev_plain
                .as_ref()
                .is_some_and(|before| screen_settled(before, &plain));
            if settled && queued_messages_pending(&plain) && report.queued_drained < QUEUE_DRAIN_MAX
            {
                tako_core::tmux::send_key(socket, session, QUEUE_RECALL_KEY)?;
                std::thread::sleep(Duration::from_millis(500));
                tako_core::tmux::send_key(socket, session, "Enter")?;
                report.queued_drained += 1;
                prev_plain = None; // 画面が動くので安定判定をやり直す
                continue;
            }

            // 入力欄に「ユーザーが打った実テキスト」が無ければ送信済み。
            // dim だけの内容（プレースホルダ / ゴースト提案）は残留ではない（#572）
            if !input_has_user_text(&styled) {
                // キューが残っているなら、生成中かどうかを 1 回だけ見極める
                // （生成中なら claude 自身がドレインするので触らない）
                if queued_messages_pending(&plain)
                    && prev_plain.is_none()
                    && report.queued_drained < QUEUE_DRAIN_MAX
                {
                    prev_plain = Some(plain);
                    continue;
                }
                report.verified = true;
                return Ok(report);
            }
            if report.enter_retries >= 4 {
                return Ok(report); // verified = false のまま返す（呼び出し側がログ）
            }
            tako_core::tmux::send_key(socket, session, "Enter")?;
            report.enter_retries += 1;
            prev_plain = None;
        }
    }

    // ② 本体を bracketed paste で貼り付け（アプリが要求していれば tmux -p が括りを付ける）
    tako_core::tmux::paste_text(socket, session, text)?;

    // ③ 反映確認（最大 3 秒）: 入力欄 or 画面のどこかに断片が見えるまで
    let head = prompt_head(text);
    let reflect_deadline = Instant::now() + Duration::from_secs(3);
    let mut reflected = false;
    while Instant::now() < reflect_deadline {
        let lines = tako_core::tmux::capture_session(socket, session)?;
        if text_in_input(&lines, text)
            || (!head.is_empty() && lines.iter().any(|l| l.contains(head.as_str())))
        {
            reflected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // ④ 送信の Enter は貼り付けと分離した単独キーとして遅延送信する
    //    （貼り付けバーストに混ざると「次の行」と解釈される）
    std::thread::sleep(Duration::from_millis(400));
    tako_core::tmux::send_key(socket, session, "Enter")?;

    // ⑤ 検証: 入力欄が空へ戻ったか。残っていれば Enter を単独再送（最大 4 回）。
    //    **入力欄が空であること単体は送信の証拠にならない**（貼り付け自体が届いて
    //    いなければ最初から空。Issue #530 の偽陽性）。③ で反映を確認できたときのみ
    //    verified を立てる
    loop {
        std::thread::sleep(Duration::from_millis(700));
        let lines = tako_core::tmux::capture_session(socket, session)?;
        if !input_residual(&lines, text) {
            report.verified = reflected;
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

    /// #1067: 「生成が終わった後も残る完了行」を busy と読まないこと。
    /// 実採取（claude 2.1.258 を SIGTERM で終わらせる直前のアイドル画面）
    #[test]
    fn 完了行を生成中と読まない() {
        let idle = screen(
            "⏺ 了解しました。合言葉「TAKO1067ZEBRA」を覚えました。\n\
             \n\
             ✻ Brewed for 2s · done 9:22 PM\n\
             ────────────────────────────────\n\
             ❯ \n\
             ────────────────────────────────\n\
               [Opus 5 (1M context) · xH]  ▸ 2.1.258\n\
               ctx   6% ░░░░░░░░░░\n\
               ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents",
        );
        // advisory な is_busy はここで true になる（経過秒 `2s` を拾う）
        assert!(
            is_busy(&idle),
            "is_busy は advisory なので完了行でも true（この事実が #1067 の理由）"
        );
        // プロセスを終わらせてよいかの判断はこちらを使う
        assert!(
            !interrupt_hint_visible(&idle),
            "アイドル画面を生成中と読んではいけない"
        );

        let generating = screen(
            "✻ Manifesting… (5m 16s · ↓ 16.4k tokens)\n\
             ────────────────────────────────\n\
             ❯ \n\
             ────────────────────────────────\n\
               esc to interrupt",
        );
        assert!(interrupt_hint_visible(&generating));
        // codex / agy の言い方も拾う
        assert!(interrupt_hint_visible(&screen("  esc to cancel")));
        assert!(interrupt_hint_visible(&screen("Generating...")));
        assert!(!interrupt_hint_visible(&screen("$ ls\nCargo.toml")));
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

    /// キュー滞留の実採取画面（claude v2.1.220。#572 のプローブ out572d/D-idle より）。
    /// 背景色つきの `❯` 行が **未送信のキュー**、最下部の dim 行が空の入力欄
    const QUEUED_STRANDED: &str = r#"⏺ 応答本文の最後の行

✻ Cooked for 4s

────────────────────────────────────────────────────────────────────────
❯ Write a numbered list from 1 to 50, one short English sentence each.
  ❯ ASCIIBURSTD572 fix it now
────────────────────────────────────────────────────────────────────────
❯ Press up to edit queued messages
────────────────────────────────────────────────────────────────────────
  ctx   5% ░░░░░░░░░░"#;

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
    fn キュー滞留のヒントは残留テキストではなく空とみなす() {
        // #572: claude はキューに未送信メッセージがあると入力欄へ dim のヒントを出す。
        // 旧実装はこれを「残留テキスト」と誤認し、Enter 単独送達（#95）が
        // 永久に空振りしていた（master の「Enter 代行が発火しない」の正体）
        assert!(input_content_is_empty(QUEUED_MESSAGES_HINT));
        assert_eq!(
            input_line(&screen(QUEUED_STRANDED)).map(input_content_is_empty),
            Some(true)
        );
    }

    #[test]
    fn キュー滞留を検知する() {
        // #572: 入力欄が空 + キュー非空 = 人間が busy 中に打った指示が未送信のまま残っている
        assert!(queued_messages_pending(&screen(QUEUED_STRANDED)));
        // 通常の空欄・入力中・シェルでは発火しない
        assert!(!queued_messages_pending(&screen(READY_PLACEHOLDER)));
        assert!(!queued_messages_pending(&screen(INPUT_PENDING)));
        assert!(!queued_messages_pending(&screen("$ ls")));
    }

    #[test]
    fn sgrを取り除ける() {
        assert_eq!(strip_sgr("\u{1b}[2mdim\u{1b}[0m text"), "dim text");
        assert_eq!(strip_sgr("plain"), "plain");
        assert_eq!(strip_sgr("\u{1b}[38;5;231m❯\u{1b}[39m a"), "❯ a");
        assert_eq!(strip_sgr("\u{1b}[m reset"), " reset");
        // SGR 以外のエスケープで本文を食い潰さない（`m` を含む語がある行）
        assert_eq!(strip_sgr("\u{1b}[2Jsome message"), "\u{1b}[2Jsome message");
    }

    #[test]
    fn 入力欄のdim判定でゴースト提案とユーザー入力を分ける() {
        // #572: 文言リストでは AI 生成のゴースト提案（`now do 1 to 50` 等）を
        // 網羅できない。属性（dim）を根拠にすることで文言非依存にする
        let ghost = vec!["\u{1b}[39m❯ \u{1b}[2mnow do 1 to 50\u{1b}[0m".to_string()];
        assert_eq!(input_text_is_all_dim(&ghost), Some(true));
        assert!(!input_has_user_text(&ghost));

        let hint = vec![
            "\u{1b}[38;5;246m❯ \u{1b}[2m\u{1b}[39mPress up to edit queued messages\u{1b}[0m"
                .to_string(),
        ];
        assert_eq!(input_text_is_all_dim(&hint), Some(true));
        assert!(!input_has_user_text(&hint));

        let user = vec!["\u{1b}[39m❯ HUMANTYPED busy message alpha".to_string()];
        assert_eq!(input_text_is_all_dim(&user), Some(false));
        assert!(input_has_user_text(&user));

        // 空欄は None（判定材料なし）
        assert_eq!(input_text_is_all_dim(&["❯ ".to_string()]), None);
        // ❯ 行が無ければ None
        assert_eq!(input_text_is_all_dim(&["$ ls".to_string()]), None);
        // 一部だけ dim（ゴースト補完 + 手入力）はユーザー入力ありとみなす
        let mixed = vec!["❯ \u{1b}[2mghost\u{1b}[22m real".to_string()];
        assert_eq!(input_text_is_all_dim(&mixed), Some(false));
        assert!(input_has_user_text(&mixed));
        // 入力欄は **画面最下部** のプロンプト行（キュー行に引きずられない）
        let with_queue = vec![
            "\u{1b}[48;5;237m❯ \u{1b}[38;5;231mqueued message\u{1b}[39m".to_string(),
            "\u{1b}[38;5;246m❯ \u{1b}[2m\u{1b}[39mPress up to edit queued messages\u{1b}[0m"
                .to_string(),
        ];
        assert_eq!(input_text_is_all_dim(&with_queue), Some(true));
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

    // --- #558: .claude.json の場所（claude は config dir 配下を読む） ---

    #[test]
    fn 設定ファイルはconfig_dir配下を主にする() {
        // 実測（claude v2.1.220）: 信頼の承諾は `<config dir>/.claude.json` に記録され、
        // ホーム直下の `~/.claude.json` は変化しない。旧実装はホーム直下だけへ
        // 書いていたため事前信頼が無効化されていた（#558）
        let paths = resolve_config_json_paths(
            Some(PathBuf::from("/opt/cfg")),
            Some(Path::new("/home/u")),
            false,
        );
        assert_eq!(paths, vec![PathBuf::from("/opt/cfg/.claude.json")]);
    }

    #[test]
    fn 旧ファイルは存在するときだけ併記する() {
        // 旧バージョンの claude が残っている環境への互換。存在しないなら
        // 現行 claude が読まないファイルを新規作成しない
        let with_legacy = resolve_config_json_paths(
            Some(PathBuf::from("/home/u/.claude")),
            Some(Path::new("/home/u")),
            true,
        );
        assert_eq!(
            with_legacy,
            vec![
                PathBuf::from("/home/u/.claude/.claude.json"),
                PathBuf::from("/home/u/.claude.json"),
            ]
        );
        let without_legacy = resolve_config_json_paths(
            Some(PathBuf::from("/home/u/.claude")),
            Some(Path::new("/home/u")),
            false,
        );
        assert_eq!(
            without_legacy,
            vec![PathBuf::from("/home/u/.claude/.claude.json")]
        );
    }

    #[test]
    fn config_dirがホーム直下を指しても重複しない() {
        // 病的な設定（CLAUDE_CONFIG_DIR=~ 等）でも同じパスを 2 回書かない
        let paths = resolve_config_json_paths(
            Some(PathBuf::from("/home/u")),
            Some(Path::new("/home/u")),
            true,
        );
        assert_eq!(paths, vec![PathBuf::from("/home/u/.claude.json")]);
    }

    #[test]
    fn ensure_trusted_inはconfig_dir配下へ書く() {
        let dir = std::env::temp_dir().join(format!("tako-t558-{}", std::process::id()));
        let cfg = dir.join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();

        assert_eq!(
            ensure_trusted_in(Some(cfg.to_str().unwrap()), "/work/proj"),
            Ok(true)
        );
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cfg.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(
            written["projects"]["/work/proj"]["hasTrustDialogAccepted"],
            true
        );

        // bypass 事前承認も同じ config dir 配下へ
        assert_eq!(
            ensure_bypass_accepted_in(Some(cfg.to_str().unwrap())),
            Ok(true)
        );
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cfg.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(written["bypassPermissionsModeAccepted"], true);
        // 既存の信頼エントリを潰していない
        assert_eq!(
            written["projects"]["/work/proj"]["hasTrustDialogAccepted"],
            true
        );

        let _ = std::fs::remove_dir_all(&dir);
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

    // --- #407: Bypass Permissions 事前承認 ---

    // claude v2.1.215 の実画面（tmux capture-pane -p -J より採取。2026-07-21）
    const BYPASS_DIALOG: &str = r#"
────────────────────────────────────────────────────────────────────────────────
  WARNING: Claude Code running in Bypass Permissions mode

  In Bypass Permissions mode, Claude Code will not ask for your approval
  before running potentially dangerous commands.
  This mode should only be used in a sandboxed container/VM that has
  restricted internet access and can easily be restored if damaged.

  By proceeding, you accept all responsibility for actions taken while running
  in Bypass Permissions mode.

  https://code.claude.com/docs/en/security

  ❯ 1. No, exit
    2. Yes, I accept

  Enter to confirm · Esc to cancel"#;

    #[test]
    fn bypassダイアログを検出する() {
        let lines = screen(BYPASS_DIALOG);
        assert!(is_bypass_dialog(&lines));
        // 信頼ダイアログとは誤認しない
        assert!(!is_trust_dialog(&lines));
        // permission ダイアログとも誤認しない
        assert!(detect_permission_dialog(&lines).is_none());
    }

    #[test]
    fn bypass以外の画面ではbypassと判定しない() {
        assert!(!is_bypass_dialog(&screen(TRUST_DIALOG)));
        assert!(!is_bypass_dialog(&screen(READY_PLACEHOLDER)));
        assert!(!is_bypass_dialog(&screen(CLAUDE_BASH_PERMISSION)));
    }

    #[test]
    fn ensure_bypass_acceptedは新規書き込みと冪等動作する() {
        let dir = std::env::temp_dir().join(format!("tako-bypass-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude.json");
        std::fs::write(
            &path,
            r#"{"installMethod":"brew","projects":{"/example":{"hasTrustDialogAccepted":true}}}"#,
        )
        .unwrap();

        // 新規書き込み
        assert_eq!(ensure_bypass_accepted_at(&path), Ok(true));
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["bypassPermissionsModeAccepted"], true);
        assert_eq!(root["installMethod"], "brew"); // 無関係キーを保持
        assert_eq!(root["projects"]["/example"]["hasTrustDialogAccepted"], true); // 他の事前信頼を保持

        // 冪等
        assert_eq!(ensure_bypass_accepted_at(&path), Ok(true));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_bypass_acceptedはファイル不在でも新規作成する() {
        let dir = std::env::temp_dir().join(format!("tako-bypass-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude.json");
        assert_eq!(ensure_bypass_accepted_at(&path), Ok(true));
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["bypassPermissionsModeAccepted"], true);
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

    /// #425 実採取相当: 画面上端のバナー・cwd・ユーザー発話が capture に含まれ、
    /// ダイアログ本体は罫線ボックスで囲まれる（tako read で観測した pane 5 の形）
    const CLAUDE_DIALOG_WITH_BANNER: &str = r#"▝▜█████▛▘  Fable 5 with xhigh effort · Claude Max
/Users/testuser
⚠ 1 MCP server needs authentication · run /mcp
Bash ツールで「touch /tmp/te439/approval-test.txt」を実行して
⏺ Running 1 shell command…
⎿  $ touch /tmp/te439/approval-test.txt
────────────────────────────────────────────────
 Bash command
   touch /tmp/te439/approval-test.txt
   Create empty file /tmp/te439/approval-test.txt
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and always allow access to te439/ from this project
   3. No
 Esc to cancel · Tab to amend · ctrl+e to explain"#;

    #[test]
    fn 罫線ボックスの外側のバナーをcommandに含めない() {
        let lines = screen(CLAUDE_DIALOG_WITH_BANNER);
        let dialog = detect_permission_dialog(&lines).expect("検知される");
        // options / highlighted は正確に取れる
        assert_eq!(dialog.options.len(), 3);
        assert_eq!(dialog.options[0], "Yes");
        assert_eq!(dialog.options[2], "No");
        assert_eq!(dialog.highlighted, Some(0));
        // command はボックス内だけ。バナー・cwd・MCP 警告・ユーザー発話は混入しない
        assert!(
            dialog
                .command
                .contains("touch /tmp/te439/approval-test.txt"),
            "ボックス内のコマンドは含む: {}",
            dialog.command
        );
        assert!(
            !dialog.command.contains("Fable 5")
                && !dialog.command.contains("MCP server")
                && !dialog.command.contains("を実行して")
                && !dialog.command.contains("testuser"),
            "罫線より上のバナー・発話は含まない: {}",
            dialog.command
        );
    }

    #[test]
    fn 罫線行の判定はcoreの実装を通る() {
        // #748: 罫線判定は `tako_core::dialog::is_rule_line` に 1 本化した
        // （`▔▔▔` 系を足したのは `/model` / `/mcp` の実採取に合わせたため）
        use tako_core::dialog::is_rule_line;
        assert!(is_rule_line("──────────"));
        assert!(is_rule_line("╭──────╮"));
        assert!(is_rule_line("  │───│  "));
        // 空行は境界にしない（本体に空行を挟む claude 版を壊さないため）
        assert!(!is_rule_line(""));
        assert!(!is_rule_line("Bash command"));
        assert!(!is_rule_line("Allow this command?"));
    }

    // --- #530: CLAUDE_CONFIG_DIR 切替時の初回ダイアログ（実採取。claude v2.1.220） ---

    /// 新しい CLAUDE_CONFIG_DIR で claude を起動した初回に出るテーマ選択。
    /// 選択カーソルが入力欄と同じ `❯` のため、旧実装は「入力欄あり」と誤認して
    /// プロンプトをここへ貼り、ダイアログに食わせて消していた（#530 の根因）
    const THEME_CHOICE: &str = r#"Welcome to Claude Code v2.1.220
 Let's get started.
 Choose the text style that looks best with your terminal
 To change this later, run /theme
   1. Auto (match terminal)
 ❯ 2. Dark mode ✔
   3. Light mode
   4. Dark mode (colorblind-friendly)
   5. Light mode (colorblind-friendly)
   6. Dark mode (ANSI colors only)
   7. Light mode (ANSI colors only)
 ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
  1  function greet() {
  2 -  console.log("Hello, World!");
  2 +  console.log("Hello, Claude!");
  3  }
 ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
  Syntax theme: Monokai Extended (ctrl+t to disable)"#;

    /// テーマ選択の次に出るログイン方法選択（未認証の config dir）
    const LOGIN_CHOICE: &str = r#"Welcome to Claude Code v2.1.220
 Claude Code can be used with your Claude subscription or billed based on API usage through your
 Console account.
 Select login method:
 ❯ 1. Claude account with subscription · Pro, Max, Team, or Enterprise
   2. Anthropic Console account · API usage billing
   3. 3rd-party platform · Amazon Bedrock, Microsoft Foundry, or Vertex AI"#;

    #[test]
    fn 初回テーマ選択を入力欄と誤認しない() {
        let lines = screen(THEME_CHOICE);
        assert!(is_choice_dialog(&lines), "選択ダイアログとして検出する");
        // 旧実装はここで Some("2. Dark mode ✔") を返し、プロンプトを貼って食わせていた
        assert_eq!(input_line(&lines), None, "入力欄としては返さない");
        assert_eq!(detect(&lines), ClaudeScreen::ChoiceDialog);
        // 文言ベースの既存判定では拾えない（構造判定が必要な理由の固定）
        assert!(!is_trust_dialog(&lines));
        assert!(!is_bypass_dialog(&lines));
    }

    #[test]
    fn ログイン方法選択を入力欄と誤認しない() {
        let lines = screen(LOGIN_CHOICE);
        assert!(is_choice_dialog(&lines));
        assert_eq!(input_line(&lines), None);
        assert_eq!(detect(&lines), ClaudeScreen::ChoiceDialog);
        assert!(!is_trust_dialog(&lines));
    }

    #[test]
    fn 選択ダイアログでは残留判定が常にfalseになる() {
        // 貼り付けたテキストはダイアログに食われて画面に出ない。旧実装は
        // 「入力欄に残っていない = 送信成功」と判定していた（偽陽性の構造）。
        // input_line が None になることで text_in_input / input_residual も false になり、
        // 呼び出し側は「反映を確認できていない」と扱える
        let lines = screen(THEME_CHOICE);
        assert!(!text_in_input(&lines, "PROBE530 と 1 行だけ返して"));
        assert!(!input_residual(&lines, "PROBE530 と 1 行だけ返して"));
    }

    #[test]
    fn 通常の入力欄は選択ダイアログと判定しない() {
        // 空 / プレースホルダ / 入力中のいずれも従来どおり入力欄として扱う
        assert!(!is_choice_dialog(&screen(READY_PLACEHOLDER)));
        assert!(!is_choice_dialog(&screen(READY_BARE)));
        assert!(!is_choice_dialog(&screen(INPUT_PENDING)));
        assert!(!is_choice_dialog(&screen(CODEX_READY)));
        assert!(!is_choice_dialog(&screen(AGY_READY)));
        assert_eq!(detect(&screen(READY_PLACEHOLDER)), ClaudeScreen::Ready);
        assert_eq!(input_line(&screen(AGY_READY)), Some(""));
    }

    #[test]
    fn 応答本文の箇条書きは選択ダイアログと判定しない() {
        // 会話ログに番号付きリストがあっても、最下部の入力欄が空なら通常画面
        let lines = screen(
            "⏺ 手順は次のとおり:\n  1. まず build する\n  2. 次に test する\n\n\
             ────────────\n❯\n────────────\n  ctx  20% ██░░░░░░░░",
        );
        assert!(!is_choice_dialog(&lines));
        assert_eq!(input_line(&lines), Some(""));
    }

    #[test]
    fn 既存の選択ダイアログも構造判定で拾える() {
        // trust / bypass / permission は専用判定が先に効くが、構造としても選択肢
        // （= 入力欄ではない）。input_line が None になり誤貼り付けを防ぐ
        for (name, s) in [
            ("trust", TRUST_DIALOG),
            ("bypass", BYPASS_DIALOG),
            ("permission", CLAUDE_BASH_PERMISSION),
            ("agy permission", AGY_PERMISSION_DIALOG),
            ("codex trust", CODEX_TRUST_DIALOG),
        ] {
            let lines = screen(s);
            assert!(is_choice_dialog(&lines), "{name} は選択ダイアログ");
            assert_eq!(input_line(&lines), None, "{name} は入力欄を返さない");
        }
    }

    #[test]
    fn 通常画面をpermission_dialogとして誤検知しない() {
        assert!(detect_permission_dialog(&screen(READY_BARE)).is_none());
        assert!(detect_permission_dialog(&screen(CODEX_READY)).is_none());
        assert!(detect_permission_dialog(&screen(AGY_READY)).is_none());
        assert!(detect_permission_dialog(&screen(AGY_BUSY)).is_none());
    }

    // --- #577: 「本文の問いかけ」と「実在するダイアログ」の切り分け ---

    /// worker が **本文で** 確認してきた画面（入力欄は空のまま最下部に見えている）。
    /// permission ダイアログの文言と番号付き選択肢を両方含むので、文言だけの判定では
    /// ダイアログと区別できない。これを permission 扱いすると master は
    /// `respond`（番号キー送信）を試み、実際には入力欄へ数字が打ち込まれる
    const QUESTION_WITH_CHOICES: &str = r#"⏺ 移行スクリプトの準備ができました。

  Do you want to proceed?
  1. Yes, run the migration now
  2. No, stop here

────────────────────────────────────────────────
❯
────────────────────────────────────────────────
  claude-opus-5 · ctx 23%"#;

    #[test]
    fn issue577_本文の問いかけをpermissionダイアログと誤検知しない() {
        let lines = screen(QUESTION_WITH_CHOICES);
        // 入力欄が最下部に生きている = ダイアログは入力を奪っていない
        assert_eq!(input_line(&lines), Some(""));
        assert!(!is_choice_dialog(&lines));
        assert!(
            detect_permission_dialog(&lines).is_none(),
            "文言が一致しても実在検査で落ちる"
        );
    }

    #[test]
    fn issue577_実在するダイアログは引き続き検知する() {
        // 実採取ベースの 4 画面（claude Bash / claude ファイル / claude 罫線ボックス /
        // agy）はいずれも選択カーソルが最下部 = 入力欄を奪っている
        for (name, s) in [
            ("claude bash", CLAUDE_BASH_PERMISSION),
            ("claude file", CLAUDE_FILE_PERMISSION),
            ("claude banner", CLAUDE_DIALOG_WITH_BANNER),
            ("agy", AGY_PERMISSION_DIALOG),
        ] {
            let lines = screen(s);
            assert!(is_choice_dialog(&lines), "{name} は入力欄を奪っている");
            assert!(
                detect_permission_dialog(&lines).is_some(),
                "{name} は permission ダイアログとして検知される"
            );
        }
    }

    // --- #748: 選択肢ダイアログの一般化（種別分類） ---

    /// claude の usage limit 対処ダイアログ。
    /// **文言は claude v2.1.220 バイナリ内の実文字列**（`What do you want to do?` /
    /// `Stop and wait for limit to reset` / `Upgrade to Max 20x for higher session limits
    /// every month`）、**レイアウトは実採取の `/model` 選択ダイアログ**から合成した
    /// （実機を limit まで使い切らずに再現できないため。#748 のコメントに経緯を記録）
    const CLAUDE_LIMIT_DIALOG: &str = r#"⏺ 続けて実装します
  ⎿  Claude usage limit reached. Your limit will reset at 3am.

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   What do you want to do?

   ❯ 1. Stop and wait for limit to reset
     2. Upgrade to Max 20x for higher session limits every month
     3. Continue with usage credits

   Enter to confirm · Esc to cancel"#;

    /// `/model` のモデル選択（#748 実採取。permission でも limit でもない一般の選択）
    const MODEL_SELECT: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between Claude models.

     1. Default (recommended)  Opus 5 with 1M context
   ❯ 2. Opus (1M context)      Opus 5 with 1M context
     3. Sonnet                 Sonnet 5 · Efficient for routine tasks

   Enter to set as default · Esc to cancel"#;

    /// plan モードの実行確認（#748 実採取）
    const PLAN_CONFIRM: &str = r#"  ────────────────────────────────────────────────────────────────────
   Claude has written up a plan and is ready to execute. Would you like to proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change"#;

    /// `/mcp` の一覧（#748 実採取。**番号なし** = 番号キーが無反応なダイアログ）
    const MCP_LIST: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Manage MCP servers
   3 servers

     User MCPs
   ❯ context7 · ✔ connected · 2 tools
     filesystem · ✔ connected · 14 tools
     tako · ✔ connected · 133 tools

   ↑/↓ to navigate · Enter to confirm · Esc to cancel"#;

    #[test]
    fn issue748_limitダイアログをusage_limitとして分類する() {
        let lines = screen(CLAUDE_LIMIT_DIALOG);
        let dialog = detect_choice_dialog(&lines).expect("検知される");
        assert_eq!(dialog.kind, DialogKind::UsageLimit);
        assert!(dialog.numbered, "番号キーで選べる");
        assert_eq!(dialog.options.len(), 3);
        assert_eq!(dialog.options[0].label, "Stop and wait for limit to reset");
        assert_eq!(dialog.highlighted, Some(0));
        assert_eq!(dialog.kind.recommended_action(), "respond_wait");
        // permission ではないので承認カード / permission_dialog には出さない（既存互換）
        assert!(detect_permission_dialog(&lines).is_none());
        // #748 観測 1: 選択肢テキストを入力欄と見なさない
        assert_eq!(input_line(&lines), None);
    }

    #[test]
    fn issue748_モデル選択とplan確認とmcp一覧を種別つきで検知する() {
        for (name, src, kind, numbered) in [
            ("model", MODEL_SELECT, DialogKind::Select, true),
            ("plan", PLAN_CONFIRM, DialogKind::PlanConfirm, true),
            ("mcp", MCP_LIST, DialogKind::Select, false),
        ] {
            let lines = screen(src);
            let dialog =
                detect_choice_dialog(&lines).unwrap_or_else(|| panic!("{name} が検知されない"));
            assert_eq!(dialog.kind, kind, "{name} の種別");
            assert_eq!(dialog.numbered, numbered, "{name} の番号キー可否");
            assert!(dialog.highlighted.is_some(), "{name} のハイライト");
            // permission ダイアログとしては出さない（master の対応が別）
            assert!(detect_permission_dialog(&lines).is_none(), "{name}");
            // 入力欄を奪っている = プロンプトを貼ってはいけない
            assert_eq!(input_line(&lines), None, "{name}");
            assert_eq!(detect(&lines), ClaudeScreen::ChoiceDialog, "{name}");
        }
    }

    #[test]
    fn issue748_permission系は種別permissionで既存apiと一致する() {
        for (name, src) in [
            ("claude bash", CLAUDE_BASH_PERMISSION),
            ("claude file", CLAUDE_FILE_PERMISSION),
            ("claude banner", CLAUDE_DIALOG_WITH_BANNER),
            ("agy", AGY_PERMISSION_DIALOG),
        ] {
            let lines = screen(src);
            let dialog = detect_choice_dialog(&lines).expect(name);
            assert_eq!(dialog.kind, DialogKind::Permission, "{name}");
            let legacy = detect_permission_dialog(&lines).expect(name);
            assert_eq!(legacy.options, dialog.labels(), "{name} の選択肢が一致");
            assert_eq!(legacy.highlighted, dialog.highlighted, "{name}");
            assert_eq!(legacy.command, dialog.title, "{name}");
        }
    }

    #[test]
    fn issue748_trustとbypassは自動承諾扱いで種別が分かれる() {
        let trust = detect_choice_dialog(&screen(TRUST_DIALOG)).expect("検知される");
        assert_eq!(trust.kind, DialogKind::Trust);
        assert!(
            trust.kind.auto_accepted(),
            "tako が承諾するので master は触らない"
        );
        let bypass = detect_choice_dialog(&screen(BYPASS_DIALOG)).expect("検知される");
        assert_eq!(bypass.kind, DialogKind::Bypass);
        assert!(bypass.kind.auto_accepted());
    }

    #[test]
    fn issue748_通常画面ではダイアログを検知しない() {
        for (name, src) in [
            ("claude ready", READY_BARE),
            ("claude placeholder", READY_PLACEHOLDER),
            ("claude 入力中", INPUT_PENDING),
            ("codex ready", CODEX_READY),
            ("agy ready", AGY_READY),
            ("キュー滞留", QUEUED_STRANDED),
            ("本文の問いかけ", QUESTION_WITH_CHOICES),
        ] {
            assert!(
                detect_choice_dialog(&screen(src)).is_none(),
                "{name} はダイアログではない"
            );
        }
    }

    #[test]
    fn issue748_ダイアログのjsonは共通形を返す() {
        let dialog = detect_choice_dialog(&screen(CLAUDE_LIMIT_DIALOG)).expect("検知される");
        let v = dialog.to_json();
        assert_eq!(v["kind"], "usage_limit");
        assert_eq!(v["numbered"], true);
        assert_eq!(v["highlighted"], 0);
        assert_eq!(v["recommended_action"], "respond_wait");
        assert_eq!(v["auto_accepted"], false);
        assert_eq!(v["options"][0]["number"], 1);
        assert_eq!(v["options"][0]["label"], "Stop and wait for limit to reset");
        assert_eq!(v["options"][0]["highlighted"], true);
        assert!(v["title"]
            .as_str()
            .unwrap()
            .contains("What do you want to do?"));
    }

    // --- 生成中のライブ表示（#719） ---

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn スピナー行から作業内容と経過時間とトークン数が取れる() {
        // 実採取（claude v2.1 系）
        let screen = lines(&[
            "⏺ 直しました",
            "",
            "✻ Manifesting… (5m 16s · ↓ 16.4k tokens)",
            "  ⎿  Tip: Use /btw to ask a quick side question",
        ]);
        assert_eq!(
            activity_line(&screen).as_deref(),
            Some("Manifesting… (5m 16s · ↓ 16.4k tokens)")
        );
    }

    #[test]
    fn 生成していないときはスピナー行を返さない() {
        let screen = lines(&["⏺ 直しました", "", "  ⎿  Tip: use /btw"]);
        assert!(activity_line(&screen).is_none());
    }

    #[test]
    fn フッターの残り時間表示はスピナーと誤認しない() {
        // 呼び出し側は入力欄より上だけを渡す約束だが、混ざっても拾わないこと
        let screen = lines(&[
            "  [Opus 5 · MAX]  user@example.com",
            "  5h   12% █░░░░░░░░░ (→4h44m)",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ]);
        assert!(activity_line(&screen).is_none());
    }

    #[test]
    fn 長い本文行はスピナーとして拾わない() {
        let long = format!("これは本文です (3s) {}", "あ".repeat(140));
        assert!(activity_line(&lines(&[&long])).is_none());
    }

    #[test]
    fn 直近のスピナー行を採る() {
        let screen = lines(&[
            "✻ Thinking… (1s · ↓ 0.1k tokens)",
            "⏺ 中間報告",
            "✻ Baking… (12s · ↓ 2.0k tokens)",
        ]);
        assert_eq!(
            activity_line(&screen).as_deref(),
            Some("Baking… (12s · ↓ 2.0k tokens)")
        );
    }
}
