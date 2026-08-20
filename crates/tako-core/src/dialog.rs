//! 選択肢ダイアログの**構造検知**（Issue #748）。
//!
//! エージェント TUI（claude / codex / agy）は「選択カーソル + 選択肢の並び」で
//! 入力欄を奪うダイアログを出す。種類は permission（ツール承認）だけではなく、
//! usage limit の対処選択・`/model` のモデル選択・`/mcp` のサーバー一覧・
//! plan モードの実行確認・AskUserQuestion の質問など多岐にわたる。
//!
//! **文言リストでは網羅できない**（#530 の教訓）。ここでは画面テキストの
//! 構造だけを見て「選択肢の並びが入力欄を奪っているか」を判定する。
//! 種別の分類（permission / usage_limit …）は文言に依るので、
//! そちらは `tako-control::claude_tui::detect_choice_dialog` の責務に分ける。
//!
//! # 検知の 2 経路（いずれも実採取画面が根拠。証拠は #748）
//!
//! 1. **番号つき**: カーソル行の中身が `N. …` で、画面に番号つき行が 2 つ以上
//!    ```text
//!     Do you want to proceed?
//!     ❯ 1. Yes
//!       2. Yes, and don’t ask again for: perl *
//!       3. No
//!    ```
//! 2. **番号なし**（`/mcp` の一覧・agy の信頼ダイアログ）: カーソル行の中身の
//!    開始列に**揃った兄弟行が隣接して 2 つ以上**ある。ただしカーソル行が
//!    上下とも罫線で挟まれていれば入力欄とみなして棄却する（複数行入力の誤検知防止）
//!    ```text
//!       ❯ context7 · ✔ connected · 2 tools
//!         coplay-mcp · ✔ connected · 98 tools
//!         filesystem · ✔ connected · 14 tools
//!    ```
//!
//! # 番号キーの実測（#748。claude v2.1.220）
//!
//! - 番号つきダイアログは**番号キーだけで確定する**（Enter 不要。permission /
//!   AskUserQuestion の実ダイアログで観測）。余分な Enter は入力欄へ抜けるので送らない
//! - 番号なしダイアログでは番号キーは**無反応**。`↑`/`↓` でカーソルを動かして Enter

/// 選択肢 1 個
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// 画面に出ている番号（番号なしダイアログでは None）
    pub number: Option<u32>,
    /// 選択肢のテキスト（番号と選択カーソルを除いた 1 行ぶん）
    pub label: String,
    /// 選択カーソル（`❯` / `›` / `>`）が指しているか
    pub highlighted: bool,
}

/// 画面に実在する選択肢の並び
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceList {
    /// 表示順の選択肢
    pub options: Vec<ChoiceOption>,
    /// ハイライト位置（`options` の添字）
    pub highlighted: Option<usize>,
    /// 番号キーで選べるか（false = 矢印移動が必要）
    pub numbered: bool,
    /// カーソル行より上のダイアログ本文（罫線で区切った直近ブロック）
    pub header: Vec<String>,
    /// 選択カーソルがある画面行の添字
    pub cursor_row: usize,
}

impl ChoiceList {
    /// ハイライトされている選択肢のラベル
    pub fn highlighted_label(&self) -> Option<&str> {
        self.highlighted
            .and_then(|i| self.options.get(i))
            .map(|o| o.label.as_str())
    }
}

/// 選択肢ダイアログが画面に実在するか（= 入力欄が奪われているか）。
///
/// `tako-control::claude_tui::is_choice_dialog` はこれに委譲する（実装を 1 本にして、
/// 送達フロー・入力欄判定・worker 状態がすべて同じ判定を通るようにする）
pub fn is_choice_dialog(lines: &[&str]) -> bool {
    detect_choice_list(lines).is_some()
}

/// 画面テキストから選択肢の並びを検知する。
///
/// 判定は最下部の選択カーソル行を起点にする（会話ログに残っている過去の
/// `❯ 送信済みメッセージ` を拾わないため）
pub fn detect_choice_list(lines: &[&str]) -> Option<ChoiceList> {
    let bottom = lines.iter().rposition(|l| !l.trim().is_empty())? + 1;
    let scan_from = bottom.saturating_sub(SCAN_ROWS);
    let cursor_row = (scan_from..bottom)
        .rev()
        .find(|&i| cursor_content(lines[i]).is_some())?;
    let content = cursor_content(lines[cursor_row])?;

    // 経路 1: 番号つき（カーソルが `N. …` を指している + 画面に 2 つ以上）
    if numbered_choice(content).is_some() {
        let numbered: Vec<usize> = (0..bottom)
            .filter(|&i| {
                let inner = cursor_content(lines[i]).unwrap_or_else(|| strip_indent(lines[i]));
                numbered_choice(inner).is_some()
            })
            .collect();
        if numbered.len() >= 2 {
            let options = numbered
                .iter()
                .map(|&i| {
                    let highlighted = cursor_content(lines[i]).is_some();
                    let inner = cursor_content(lines[i]).unwrap_or_else(|| strip_indent(lines[i]));
                    let (number, label) = numbered_choice(inner).unwrap_or((0, inner));
                    ChoiceOption {
                        number: Some(number),
                        label: label.trim().to_string(),
                        highlighted,
                    }
                })
                .collect::<Vec<_>>();
            let highlighted = options.iter().position(|o| o.highlighted);
            return Some(ChoiceList {
                options,
                highlighted,
                numbered: true,
                header: header_block(lines, numbered[0]),
                cursor_row,
            });
        }
    }

    // 経路 2: 番号なし（開始列の揃った兄弟行が隣接している）
    if content.trim().is_empty() {
        return None; // 空のカーソル行（= 空の入力欄）は選択肢ではない
    }
    let indent = content_column(lines[cursor_row])?;
    if framed_as_input_box(lines, cursor_row, indent, scan_from, bottom) {
        return None; // 上下を罫線で挟まれている = 入力ボックス（複数行入力）
    }
    let mut rows = vec![cursor_row];
    for i in (scan_from..cursor_row).rev() {
        if aligned_sibling(lines[i], indent) {
            rows.push(i);
        } else {
            break;
        }
    }
    rows.reverse();
    for (i, line) in lines.iter().enumerate().take(bottom).skip(cursor_row + 1) {
        if aligned_sibling(line, indent) {
            rows.push(i);
        } else {
            break;
        }
    }
    // 兄弟が 2 つ未満なら選択肢の並びとみなさない。codex の起動画面は
    // 入力行の直下にモデル / cwd のステータス行が同じ桁で 1 行だけ並ぶため、
    // 「兄弟 1 つ」を許すとそれを選択肢と誤検知する（実採取 fixture で固定）
    if rows.len() < 3 {
        return None;
    }
    let options: Vec<ChoiceOption> = rows
        .iter()
        .filter(|&&i| !is_key_hint(lines[i]))
        .map(|&i| ChoiceOption {
            number: None,
            label: cursor_content(lines[i])
                .unwrap_or_else(|| strip_indent(lines[i]))
                .trim()
                .to_string(),
            highlighted: i == cursor_row,
        })
        .collect();
    if options.len() < 2 {
        return None;
    }
    let highlighted = options.iter().position(|o| o.highlighted);
    Some(ChoiceList {
        options,
        highlighted,
        numbered: false,
        header: header_block(lines, *rows.first().unwrap_or(&cursor_row)),
        cursor_row,
    })
}

/// 走査する下端からの行数。ダイアログは選択肢 + 説明で 20 行を超えることがある
/// （実採取: `/mcp` の 12 サーバー一覧で 20 行）ので入力欄判定より広く取る
const SCAN_ROWS: usize = 40;

/// 選択カーソル（`❯` / `›` / `>`）で始まる行なら、カーソルを除いた中身を返す。
///
/// 枠線つきで描かれる TUI（`│ ❯ hello │`）でも拾えるよう行頭の縦罫線は 1 つ剥がす。
/// ASCII の `>` はシェルの PS2・リダイレクトと衝突するため
/// 「`>` 単独 or `> `＋内容」だけをカーソルとみなす（`screen::starts_with_prompt` と同じ規則）
pub fn cursor_content(line: &str) -> Option<&str> {
    let t = strip_indent(line);
    t.strip_prefix('❯')
        .or_else(|| t.strip_prefix('›'))
        .or_else(|| match t.strip_prefix('>') {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => Some(rest),
            _ => None,
        })
        .map(str::trim)
}

/// 行頭の空白（と縦罫線 1 つ）を落とす
fn strip_indent(line: &str) -> &str {
    let t = line.trim_start();
    t.strip_prefix('│')
        .or_else(|| t.strip_prefix('┃'))
        .unwrap_or(t)
        .trim_start()
}

/// 番号つき選択肢なら `(番号, ラベル)` を返す。
/// 番号は 1 桁に限らない（10 個超の選択肢を持つ TUI のため。#748 のエッジ検証）
pub fn numbered_choice(inner: &str) -> Option<(u32, &str)> {
    let digits = inner.len() - inner.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = &inner[digits..];
    let label = rest.strip_prefix(". ")?;
    inner[..digits].parse().ok().map(|n| (n, label))
}

/// その行の「中身が始まる桁」（行頭空白 + 縦罫線 + 選択カーソルを除いた位置）。
/// 桁は**表示幅ではなく char 数**で数える（全角の選択肢でも兄弟行の空白は半角なので一致する）
fn content_column(line: &str) -> Option<usize> {
    let inner = cursor_content(line)?;
    let offset = line.len() - inner.len();
    Some(line[..offset].chars().count())
}

/// 選択カーソルのない兄弟行か（中身の開始桁がカーソル行と一致する非空行）
fn aligned_sibling(line: &str, indent: usize) -> bool {
    if line.trim().is_empty() || cursor_content(line).is_some() {
        return false;
    }
    let stripped = line.trim_start();
    let leading = line.len() - stripped.len();
    line[..leading].chars().count() == indent
}

/// 操作キーの案内行か（選択肢一覧から除くため。ラベル付けにのみ使う文言判定で、
/// **ダイアログの存在判定には使わない**）
pub fn is_key_hint(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("↑/↓")
        || t.starts_with("Press enter")
        || t.starts_with("Enter to")
        || t.starts_with("Esc ")
        || t.contains("to navigate")
        || t.contains("Navigate ·")
        || t.contains("enter Confirm")
        || t.contains("to cancel")
}

/// 罫線だけでできた行か（`screen::is_frame_line` と同じ役割。ダイアログの箱の境界）。
/// claude は `────` / `╭─╮` のほか `▔▔▔`（`/model` / `/mcp`）も使う（実採取）
pub fn is_rule_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let mut horizontal = 0usize;
    for c in t.chars() {
        match c {
            '─' | '━' | '═' | '╌' | '╍' | '┄' | '┈' | '⎯' | '▔' | '▁' | '▀' | '▄' => {
                horizontal += 1
            }
            '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '│' | '┃' | ' ' =>
                {}
            _ => return false,
        }
    }
    horizontal >= 3
}

/// カーソル行（と同じ桁に揃った行の塊）が上下とも罫線で挟まれているか
/// （= claude / codex / agy の入力ボックス）。
///
/// 番号なし経路の誤検知を防ぐ拒否条件。**複数行入力の継続行は選択肢と同じ桁**に
/// 描かれるので、塊の内側を飛ばして外側の境界を見る必要がある（実測: 3 行入力）
fn framed_as_input_box(
    lines: &[&str],
    cursor_row: usize,
    indent: usize,
    from: usize,
    to: usize,
) -> bool {
    let outside = |i: usize| !lines[i].trim().is_empty() && !aligned_sibling(lines[i], indent);
    let above = (from..cursor_row).rev().find(|&i| outside(i));
    let below = (cursor_row + 1..to).find(|&i| outside(i));
    match (above, below) {
        (Some(a), Some(b)) => is_rule_line(lines[a]) && is_rule_line(lines[b]),
        _ => false,
    }
}

/// 選択肢の直前にあるダイアログ本文を集める。
///
/// 画面全体を採ると上端のバナー・cwd・ユーザー発話まで入るため、**罫線に当たったら
/// それまでのブロックを捨てる**（ダイアログの箱の内側だけを残す。#425 の実採取由来）。
/// 空行は境界にしない（claude の実ダイアログは本体に空行を挟む）
fn header_block(lines: &[&str], first_option_row: usize) -> Vec<String> {
    let mut block: Vec<String> = Vec::new();
    for line in lines.iter().take(first_option_row) {
        let t = line.trim();
        if t.is_empty() || is_key_hint(line) {
            continue;
        }
        let desc = t
            .trim_start_matches("? ")
            .trim_start_matches("❯ ")
            .trim_start_matches("> ");
        if is_rule_line(desc) {
            block.clear();
        } else if !desc.contains("ctrl+g") {
            block.push(desc.to_string());
        }
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    // --- 実採取画面（claude v2.1.220 / 2026-08-04。個人情報はサニタイズ済み） ---

    /// Bash ツールの承認ダイアログ（`perl` は許可リスト外）
    const PERMISSION: &str = r#"❯ Bash ツールで「perl -e "print 42"」をそのまま実行して。他のツールは使わないで

  Running 1 shell command…
  ⎿  $ perl -e "print 42"

────────────────────────────────────────────────────────────────────────
 Bash command

   perl -e "print 42"
   perl で 42 を出力

 This command requires approval

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don’t ask again for: perl *
   3. No

 Esc to cancel · Tab to amend · ctrl+e to explain"#;

    /// `/model` のモデル選択（`▔▔▔` 罫線 + 深いインデント + `✔` つき既定）
    const MODEL_SELECT: &str = r#"✻ Churned for 9s

❯ Bash ツールで du -sh /tmp/work を実行して

▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Select model
   Switch between Claude models. Your pick becomes the default for new sessions.

     1. Default (recommended)  Opus 5 with 1M context
     2. Opus (1M context)      Opus 5 with 1M context
   ❯ 3. Fable ✔                Fable 5 · Most capable for your hardest tasks
     4. Sonnet                 Sonnet 5 · Efficient for routine tasks
     5. Haiku                  Haiku 4.5 · Fastest for quick answers
     6. Opus 4.6 (1M)          Opus 4.6 with 1M context

   ● High effort (default) ←/→ to adjust

   Enter to set as default · s to use this session only · Esc to cancel"#;

    /// plan モードの実行確認（`Would you like to proceed?` = permission と別文言）
    const PLAN_CONFIRM: &str = r#"  ────────────────────────────────────────────────────────────────────
   Claude has written up a plan and is ready to execute. Would you like to proceed?

   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this feedback

   ctrl+g to edit in VS Code · ~/.claude/plans/example.md"#;

    /// `/mcp` のサーバー一覧（**番号なし** + セクション見出し混在 + 12 項目）
    const MCP_LIST: &str = r#"▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔
   Manage MCP servers
   12 servers

     User MCPs (<home>/.claude.json)
   ❯ context7 · ✔ connected · 2 tools
     coplay-mcp · ✔ connected · 98 tools
     filesystem · ✔ connected · 14 tools
     github · ✔ connected · 26 tools

     claude.ai
     claude.ai Canva · ✔ connected · 32 tools

   https://code.claude.com/docs/en/mcp for help
   ↑/↓ to navigate · Enter to confirm · Esc to cancel"#;

    /// AskUserQuestion の質問（全角ラベル + 説明の継続行 + 罫線をまたぐ選択肢）
    const ASK_USER_QUESTION: &str = r#"✻ Churned for 53s

❯ AskUserQuestion ツールで質問して
  ⎿  Invalid tool parameters
────────────────────────────────────────────────────────────────────────
 ☐ 進め方

この作業をどう進めますか?

❯ 1. そのまま変更する（推奨）
     println!("hi") を println!("hello") に 1 行編集し、コンパイル確認まで行う
  2. 変更してコミットまで
     編集後に /commit スキルでコミットを作成する
  3. 今回は見送る
     変更せず現状のままにする
  4. Type something.
────────────────────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel"#;

    /// 通常の入力欄（空 + dim プレースホルダ）
    const INPUT_READY: &str = r#"⏺ 直しました

────────────────────────────────────────────────────────────────────────
❯ Try "refactor <filepath>"
────────────────────────────────────────────────────────────────────────
  ctx   5% ░░░░░░░░░░"#;

    /// 複数行入力（継続行のインデントが選択肢と同じ 2 桁 = 番号なし経路の誤検知源）
    const INPUT_MULTILINE: &str = r#"────────────────────────────────────────────────────────────────────────
❯ 1 行目のテキスト
  2 行目のテキスト
  3 行目のテキスト
────────────────────────────────────────────────────────────────────────
  ctx  12% █░░░░░░░░░"#;

    /// codex の起動画面（入力行の直下にモデル / cwd のステータス行が同じ桁で並ぶ。
    /// 「兄弟 1 つ」を選択肢とみなすとこれが誤検知される = 閾値 3 行の根拠）
    const CODEX_READY: &str = r#"╭─────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.144.1)                      │
│                                                 │
│ model:     gpt-5.6-sol high   /model to change  │
╰─────────────────────────────────────────────────╯

  Tip: When the composer is empty, press Esc to step back and edit your last message; Enter
  confirms.


› Summarize recent commits

  gpt-5.6-sol high · /private/tmp/example/workdir"#;

    /// agy の信頼ダイアログ（**番号なし**の実採取。番号なし経路が拾うべき下限）
    const AGY_TRUST: &str = r#"Accessing workspace:
/private/tmp/example/workdir
Do you trust the contents of this project?
Antigravity CLI requires permission to read, edit, and execute files here.
> Yes, I trust this folder
  No, exit
  ↑/↓ Navigate · enter Confirm
                                                    Claude Opus 4.6 (Thinking)"#;

    /// agy の空入力欄（罫線で挟まれた `>` 単独行）
    const AGY_READY: &str = r#"  Antigravity CLI 1.1.0
  Claude Opus 4.6 (Thinking)
  /private/tmp/example/workdir
────────────────────────────────────────────────────
>
────────────────────────────────────────────────────
? for shortcuts                                     Claude Opus 4.6 (Thinking)"#;

    /// 応答本文の箇条書き（入力欄は空 = ダイアログではない。#577）
    const QUESTION_IN_BODY: &str = r#"⏺ 移行スクリプトの準備ができました。

  Do you want to proceed?
  1. Yes, run the migration now
  2. No, stop here

────────────────────────────────────────────────────────────────────────
❯
────────────────────────────────────────────────────────────────────────
  claude-opus-5 · ctx 23%"#;

    #[test]
    fn 実採取のpermissionダイアログを番号つきで検知する() {
        let list = detect_choice_list(&rows(PERMISSION)).expect("検知される");
        assert!(list.numbered);
        assert_eq!(list.options.len(), 3);
        assert_eq!(list.options[0].label, "Yes");
        assert_eq!(list.options[0].number, Some(1));
        assert_eq!(list.options[2].label, "No");
        assert_eq!(list.highlighted, Some(0));
        // 罫線より上のユーザー発話・ツールログは本文に混ぜない
        let header = list.header.join(" ");
        assert!(header.contains("perl -e \"print 42\""), "{header}");
        assert!(!header.contains("Bash ツールで"), "{header}");
    }

    #[test]
    fn 実採取のモデル選択を検知する() {
        let list = detect_choice_list(&rows(MODEL_SELECT)).expect("検知される");
        assert!(list.numbered);
        assert_eq!(list.options.len(), 6);
        assert_eq!(list.highlighted, Some(2), "❯ が 3. を指している");
        assert!(list.options[2].label.starts_with("Fable ✔"));
        assert!(list.header.join(" ").contains("Select model"));
    }

    #[test]
    fn 実採取のplan確認を検知する() {
        let list = detect_choice_list(&rows(PLAN_CONFIRM)).expect("検知される");
        assert_eq!(list.options.len(), 3);
        assert_eq!(list.highlighted, Some(0));
        // 選択肢の説明の継続行（shift+tab …）は選択肢に混ぜない
        assert!(list.options.iter().all(|o| !o.label.contains("shift+tab")));
    }

    #[test]
    fn 実採取のmcp一覧を番号なしで検知する() {
        let list = detect_choice_list(&rows(MCP_LIST)).expect("検知される");
        assert!(!list.numbered, "番号キーでは選べない");
        // ハイライトは context7。セクション見出しは構造上区別できないので混ざるが、
        // ハイライト位置とラベルが取れることが respond の前提
        assert_eq!(
            list.highlighted_label(),
            Some("context7 · ✔ connected · 2 tools")
        );
        assert!(list.options.len() >= 4, "{:?}", list.options);
        // キー案内行は選択肢に含めない
        assert!(list
            .options
            .iter()
            .all(|o| !o.label.contains("to navigate")));
    }

    #[test]
    fn 実採取のaskuserquestionを検知する() {
        let list = detect_choice_list(&rows(ASK_USER_QUESTION)).expect("検知される");
        assert!(list.numbered);
        // 罫線をまたぐ「5. Chat about this」まで拾う
        assert_eq!(list.options.len(), 5);
        assert_eq!(list.options[0].label, "そのまま変更する（推奨）");
        assert_eq!(list.options[4].label, "Chat about this");
        assert_eq!(list.highlighted, Some(0));
    }

    #[test]
    fn 通常の入力欄はダイアログと判定しない() {
        for (name, s) in [
            ("空 + プレースホルダ", INPUT_READY),
            ("複数行入力", INPUT_MULTILINE),
            ("本文の箇条書き", QUESTION_IN_BODY),
            ("codex 起動画面", CODEX_READY),
            ("agy 空入力欄", AGY_READY),
        ] {
            assert!(
                detect_choice_list(&rows(s)).is_none(),
                "{name} は入力欄（ダイアログではない）"
            );
        }
    }

    #[test]
    fn 番号なしのagy信頼ダイアログを検知する() {
        let list = detect_choice_list(&rows(AGY_TRUST)).expect("検知される");
        assert!(!list.numbered);
        // キー案内行を落として選択肢 2 つ
        assert_eq!(list.options.len(), 2, "{:?}", list.options);
        assert_eq!(list.options[0].label, "Yes, I trust this folder");
        assert_eq!(list.options[1].label, "No, exit");
        assert_eq!(list.highlighted, Some(0));
    }

    #[test]
    fn 番号は多桁も解釈する() {
        assert_eq!(numbered_choice("1. Yes"), Some((1, "Yes")));
        assert_eq!(numbered_choice("12. 十二番目"), Some((12, "十二番目")));
        assert_eq!(numbered_choice("1.Yes"), None, "ドット直後の空白が必要");
        assert_eq!(numbered_choice("Yes"), None);
        assert_eq!(numbered_choice("1"), None);
    }

    #[test]
    fn 選択肢が十個を超えても全件と位置が取れる() {
        // 実 claude は 9 個までしか番号を振らない画面が多いので合成（多桁の回帰固定）
        let mut screen = vec![" どれにしますか?".to_string()];
        for n in 1..=12 {
            screen.push(if n == 11 {
                format!(" ❯ {n}. 選択肢 {n}")
            } else {
                format!("   {n}. 選択肢 {n}")
            });
        }
        screen.push(" Press enter to confirm".to_string());
        let refs: Vec<&str> = screen.iter().map(String::as_str).collect();
        let list = detect_choice_list(&refs).expect("検知される");
        assert_eq!(list.options.len(), 12);
        assert_eq!(list.options[11].number, Some(12));
        assert_eq!(list.highlighted, Some(10));
        assert_eq!(list.highlighted_label(), Some("選択肢 11"));
    }

    #[test]
    fn 全角混じりの選択肢もラベルが崩れない() {
        let refs = [
            " 全角の質問です？",
            " ❯ 1. 変更する（推奨）",
            "   2. 変更しない",
        ];
        let list = detect_choice_list(&refs).expect("検知される");
        assert_eq!(list.options[0].label, "変更する（推奨）");
        assert_eq!(list.options[1].label, "変更しない");
    }

    #[test]
    fn 番号なし経路は罫線に挟まれた入力欄を棄却する() {
        // INPUT_MULTILINE は「兄弟行が 2 つ」あるので、罫線の拒否条件が無いと誤検知する
        let lines = rows(INPUT_MULTILINE);
        let bottom = lines.iter().rposition(|l| !l.trim().is_empty()).unwrap() + 1;
        let cursor = (0..bottom)
            .rev()
            .find(|&i| cursor_content(lines[i]).is_some())
            .unwrap();
        let indent = content_column(lines[cursor]).unwrap();
        assert!(framed_as_input_box(&lines, cursor, indent, 0, bottom));
        assert!(detect_choice_list(&lines).is_none());
    }

    #[test]
    fn 罫線判定は実採取の罫線文字を網羅する() {
        assert!(is_rule_line("────────"));
        assert!(is_rule_line("▔▔▔▔▔▔▔▔"), "/model / /mcp の上罫線");
        assert!(is_rule_line("╭──────╮"));
        assert!(is_rule_line("╌╌╌╌╌╌╌╌"));
        assert!(!is_rule_line(""));
        assert!(!is_rule_line("Select model"));
        assert!(!is_rule_line("❯ 1. Yes"));
    }
}
