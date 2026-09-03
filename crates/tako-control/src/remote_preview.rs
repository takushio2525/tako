//! ペインカード用の画面要約（#621）。
//!
//! リモート PWA の**ペイン選択画面**は「どれがどれだか分かる」ことが最優先で、
//! そのために各カードへ (1) 中身のスニペット (2) 今どういう状態か を出す。
//! このモジュールはその 2 つを **1 枚の画面キャプチャから純粋関数で導く**層で、
//! tmux 呼び出しは呼び出し側（`remote.rs`）に置く。実 tmux 無しでテストを回すため。
//!
//! スニペットの肝は「TUI のクロム（入力欄 + フッター）を落として会話の末尾を出す」こと。
//! 素の `capture-pane` 末尾はどのエージェントでも入力欄とフッターで、
//! カードに出しても全ペインが同じ見た目になり識別に一切寄与しない（#433 で増量しても
//! 「どれがどれだか分からない」が残った実機フィードバックの根本原因）。
//!
//! 絶対ルール: ここで扱う画面テキストは**診断ログに出さない**（AGENTS.md）。
//! API 応答としてユーザー自身の端末へ返すだけ。

use crate::orchestrator::wait::{detect_worker_error, screen_looks_busy};
use serde_json::{json, Value};

/// カードに載せるスニペットの最大行数
pub const PREVIEW_MAX_LINES: usize = 8;

/// エージェントペインの活動状態。カードの色・ラベルの正になる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// permission ダイアログが画面に実在する = ユーザーの応答待ち（#425 / #577）
    Permission,
    /// 生成・ツール実行中
    Busy,
    /// 異常停止（usage limit / API エラー / 選択ダイアログ。#157）
    Error,
    /// 入力待ち
    Idle,
}

impl Activity {
    /// JSON に載せる機械可読 slug
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Busy => "busy",
            Self::Error => "error",
            Self::Idle => "idle",
        }
    }
}

/// 画面 1 枚からカード用フィールド（`preview` / `activity` / `error`）を組み立てる。
///
/// `is_agent` が false（素のシェル）なら活動状態は判定しない。
/// シェルの状態は OSC 133（`state` フィールド）が正で、エージェント TUI 向けの
/// スピナー検知をかけると意味のない結果になるため。
pub fn summarize(screen: &[String], is_agent: bool, has_permission_dialog: bool) -> Value {
    let preview = if is_agent {
        agent_preview_lines(screen, PREVIEW_MAX_LINES)
    } else {
        preview_lines(screen, PREVIEW_MAX_LINES)
    };
    let mut out = json!({ "preview": preview });
    if !is_agent {
        return out;
    }
    let text = screen.join("\n");
    let (activity, err) = agent_activity(&text, has_permission_dialog);
    out["activity"] = json!(activity.as_str());
    if let Some((kind, detail)) = err {
        out["error"] = json!({
            "kind": kind.as_str(),
            "detail": detail,
            "recommended_action": kind.recommended_action(),
        });
    }
    out
}

/// エージェント画面から活動状態を導く。エラー時は種別と該当行も返す。
///
/// 優先順位は permission > busy > error > idle。
/// `detect_worker_error` は**停止確定後の画面**に対して使う契約なので
/// busy 判定より後ろに置く（busy 中に呼ぶとツール実行ログへ誤検知する）。
pub fn agent_activity(
    screen: &str,
    has_permission_dialog: bool,
) -> (
    Activity,
    Option<(crate::orchestrator::wait::WorkerErrorKind, String)>,
) {
    if has_permission_dialog {
        return (Activity::Permission, None);
    }
    if screen_looks_busy(screen) {
        return (Activity::Busy, None);
    }
    if let Some((kind, detail)) = detect_worker_error(screen) {
        return (Activity::Error, Some((kind, detail)));
    }
    (Activity::Idle, None)
}

/// エージェント TUI 向けのスニペット。
///
/// 1. 最後の入力欄プロンプト行から下（入力欄 + フッター）を落とす
/// 2. その直上に続く枠線・空行も落とす
/// 3. あとは `preview_lines` と同じ整形
///
/// permission ダイアログで入力欄が奪われている画面では 1 が何もしないので、
/// ダイアログ本体がそのままスニペットに出る（カードで最も知りたい情報）。
pub fn agent_preview_lines(screen: &[String], max: usize) -> Vec<String> {
    preview_lines(strip_agent_chrome(screen), max)
}

/// 画面から「中身が分かる末尾 `max` 行」を切り出す。
///
/// 末尾の空行・罫線を落とし、連続空行を 1 行に畳み、末尾 `max` 行を返す
/// （先頭に残った空行は捨てる）。
///
/// 素のシェルにはこちらを使う。プロンプト文字の切り落とし（`agent_preview_lines`）を
/// かけてはいけない: npm スクリプトのエコー `> vite dev` のような**普通の出力**が
/// プロンプト行に見え、そこから下の出力が丸ごと消える
pub fn preview_lines(screen: &[String], max: usize) -> Vec<String> {
    let mut lines: Vec<String> = screen.iter().map(|l| l.trim_end().to_string()).collect();
    while lines
        .last()
        .is_some_and(|l| l.trim().is_empty() || is_rule(l))
    {
        lines.pop();
    }

    let mut compact: Vec<String> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.trim().is_empty() && compact.last().is_some_and(|p| p.trim().is_empty()) {
            continue;
        }
        compact.push(line);
    }

    let start = compact.len().saturating_sub(max.max(1));
    let mut out = compact.split_off(start);
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    out
}

/// 入力欄プロンプト行より下（入力欄 + フッター）を切り落とす
fn strip_agent_chrome(screen: &[String]) -> &[String] {
    let Some(cut) = screen.iter().rposition(|l| is_prompt_row(l)) else {
        return screen;
    };
    let mut end = cut;
    while end > 0 && (is_rule(&screen[end - 1]) || screen[end - 1].trim().is_empty()) {
        end -= 1;
    }
    &screen[..end]
}

/// 枠線 `│ … │` の内側を取り出す（枠なしの行はそのまま trim して返す）
fn frame_inner(line: &str) -> &str {
    let t = line.trim();
    match t.strip_prefix('│') {
        Some(rest) => rest.trim_end_matches('│').trim(),
        None => t,
    }
}

/// 入力欄のプロンプト行か。プロンプト文字の語彙は
/// `orchestrator::wait::screen_looks_idle` と同じ（claude `❯` / codex `›` /
/// agy・シェル `>`）。枠つき TUI では `│ > … │` の形も取る。
///
/// ただし `❯ 1. Yes` のような**番号つき選択行は入力欄ではない**。
/// permission ダイアログの選択肢を入力欄と誤認すると、カードに出すべき
/// ダイアログ本体ごと切り落としてしまう
fn is_prompt_row(line: &str) -> bool {
    let inner = frame_inner(line);
    let rest = if let Some(r) = inner.strip_prefix('❯') {
        r
    } else if let Some(r) = inner.strip_prefix('›') {
        r
    } else if inner == ">" {
        ""
    } else if let Some(r) = inner.strip_prefix("> ") {
        r
    } else {
        return false;
    };
    !starts_with_numbered_choice(rest.trim_start())
}

/// `1. Yes` のような番号つき選択肢で始まるか
fn starts_with_numbered_choice(s: &str) -> bool {
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && s[digits..].starts_with('.')
}

/// 罫線・枠線だけの行か（区切り線、枠の上下辺）
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && t.chars().all(|c| {
            matches!(
                c,
                '─' | '│'
                    | '╭'
                    | '╮'
                    | '╰'
                    | '╯'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
                    | '├'
                    | '┤'
                    | '┬'
                    | '┴'
                    | '┼'
                    | '━'
                    | '┃'
                    | '═'
                    | '║'
                    | '-'
                    | '='
                    | ' '
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    /// 実採取した claude TUI（#571 が wait.rs へ残した本物のフッター 8 行構成）。
    /// 入力待ちで、直前の応答が会話末尾に残っている状態
    const CLAUDE_IDLE: &str = "\
  エディタは時間をおけば戻る見込みです。今すぐ
  再試行するか、少し待ってから続行するかご指示ください。

✻ Cogitated for 2h 3m 31s
         new task? /clear to save 500k tokens
──────────────────────────────────────────────
❯
──────────────────────────────────────────────
  [Opus 5 (1M context) · MAX]  🔧 worker: bu…
  ctx  50% █████░░░░░
  5h   36% ███░░░░░░░ (→2h06m)
  7d    7% ░░░░░░░░░░ (→6d20h)
  ⏵⏵ auto mode on (shift+tab to cycle) · ← f…";

    /// 実採取した claude TUI の**作業中**画面
    const CLAUDE_BUSY: &str = "\
⏺ 実測プローブ用に、本番の生画面を採取します。

⏺ Searching for 5 patterns, reading 2 files, calling tako, running 13
  shell commands…

✽ Misting… (10m 49s · ↓ 35.8k tokens)

──────────────────────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────────────────────
  [Opus 5 · MAX]  🔧 worker: tako:571
  ctx  23% ██░░░░░░░░
  5h   20% ██░░░░░░░░ (→2h33m)
  7d   16% █░░░░░░░░░ (→5d10h)
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents";

    /// 枠つき入力欄（codex / 旧 claude 系）
    const BOXED_INPUT: &str = "\
▌ テストは全て通りました。

╭──────────────────────────────────────────────╮
│ ›                                            │
╰──────────────────────────────────────────────╯
  ⏎ send   ⌃J newline   ⌃T transcript";

    #[test]
    fn 入力欄とフッターを落として会話の末尾を出す() {
        let out = agent_preview_lines(&lines(CLAUDE_IDLE), PREVIEW_MAX_LINES);
        assert_eq!(
            out.last().map(String::as_str),
            Some("         new task? /clear to save 500k tokens"),
            "入力欄・区切り線・フッター 5 行が落ちていること"
        );
        assert!(
            out.iter().any(|l| l.contains("エディタは時間をおけば")),
            "直前の応答本文が残ること"
        );
        assert!(
            !out.iter().any(|l| l.contains("ctx  50%")),
            "フッターのメーターは中身ではない"
        );
        assert!(!out.iter().any(|l| l.trim() == "❯"), "入力欄は落とす");
    }

    #[test]
    fn 作業中はスピナー行までがスニペットに入る() {
        let out = agent_preview_lines(&lines(CLAUDE_BUSY), PREVIEW_MAX_LINES);
        assert_eq!(
            out.last().map(String::as_str),
            Some("✽ Misting… (10m 49s · ↓ 35.8k tokens)"),
            "何をしているかが分かる行がスニペット末尾に来る"
        );
        assert!(out.iter().any(|l| l.contains("Searching for 5 patterns")));
    }

    #[test]
    fn 枠つき入力欄も落とす() {
        let out = agent_preview_lines(&lines(BOXED_INPUT), PREVIEW_MAX_LINES);
        assert_eq!(out, vec!["▌ テストは全て通りました。".to_string()]);
    }

    /// シェル出力にはプロンプトに見える行（npm スクリプトのエコー `> vite dev`）が
    /// 普通に現れる。切り落とし判定をかけると以降の出力が丸ごと消える
    #[test]
    fn シェルの出力はプロンプトに見える行があっても切らない() {
        let shell = lines(
            "$ npm run dev\n\
             \n\
             > vite dev\n\
             \n\
             VITE v6.0.0 ready in 320 ms\n\
             ➜ Local: http://localhost:5173/\n",
        );
        let out = preview_lines(&shell, PREVIEW_MAX_LINES);
        assert_eq!(
            out.last().map(String::as_str),
            Some("➜ Local: http://localhost:5173/")
        );
        assert!(out.iter().any(|l| l.contains("VITE v6.0.0")));
        // 逆に agent 用の切り落としをかけると出力が消える = 使い分けが要る証拠
        assert_eq!(
            agent_preview_lines(&shell, PREVIEW_MAX_LINES),
            vec!["$ npm run dev".to_string()]
        );
    }

    #[test]
    fn permissionダイアログの選択肢は入力欄と見なさない() {
        let dialog = lines(
            "⏺ ビルド成果物を消します。\n\
             \n\
             ╭──────────────────────────────────╮\n\
             │ Bash command                     │\n\
             │ rm -rf build                     │\n\
             │ Do you want to proceed?          │\n\
             │ ❯ 1. Yes                         │\n\
             │   2. No                          │\n\
             ╰──────────────────────────────────╯",
        );
        let out = agent_preview_lines(&dialog, PREVIEW_MAX_LINES);
        assert!(
            out.iter().any(|l| l.contains("rm -rf build")),
            "ダイアログ本体を切り落としてはいけない: {out:?}"
        );
        assert!(out.iter().any(|l| l.contains("1. Yes")));
    }

    #[test]
    fn 最大行数を超えたら末尾だけ残す() {
        let many: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
        let out = preview_lines(&many, 5);
        assert_eq!(out.len(), 5);
        assert_eq!(out.first().map(String::as_str), Some("line 36"));
        assert_eq!(out.last().map(String::as_str), Some("line 40"));
    }

    #[test]
    fn 連続空行は畳んで情報量を稼ぐ() {
        let sparse = lines("A\n\n\n\n\nB\n\n\n\n\nC\n\n\n");
        let out = preview_lines(&sparse, PREVIEW_MAX_LINES);
        assert_eq!(out, vec!["A", "", "B", "", "C"]);
    }

    #[test]
    fn 画面が空でも落ちない() {
        assert!(preview_lines(&[], PREVIEW_MAX_LINES).is_empty());
        assert!(agent_preview_lines(&[], PREVIEW_MAX_LINES).is_empty());
        assert!(preview_lines(&lines("\n\n\n"), PREVIEW_MAX_LINES).is_empty());
        // 入力欄しか無い起動直後の画面
        assert!(agent_preview_lines(&lines("──────\n❯ \n──────"), PREVIEW_MAX_LINES).is_empty());
    }

    #[test]
    fn 活動状態の優先順位はpermissionが最上位() {
        // busy に見える画面でもダイアログ実在が優先
        assert_eq!(agent_activity(CLAUDE_BUSY, true).0, Activity::Permission);
        assert_eq!(agent_activity(CLAUDE_BUSY, false).0, Activity::Busy);
        assert_eq!(agent_activity(CLAUDE_IDLE, false).0, Activity::Idle);
    }

    #[test]
    fn 停止中のusage_limitはerrorとして種別つきで返る() {
        let screen = "Claude usage limit reached. Your limit will reset at 3am.\n❯ ";
        let (activity, err) = agent_activity(screen, false);
        assert_eq!(activity, Activity::Error);
        let (kind, detail) = err.expect("種別が付くこと");
        assert_eq!(kind.as_str(), "usage_limit");
        assert!(detail.contains("usage limit reached"));
    }

    #[test]
    fn issue1106_時間で解けない阻害もerrorとして種別つきで返る() {
        // リモート（PWA）のペイン一覧も同じ関数を通る。ここで idle に落ちると
        // スマホ側でも「作業完了」に見える（#1106 の実害がそのまま出る）
        let screen = "  ⎿  Your usage allocation has been disabled by your admin\n❯ ";
        let (activity, err) = agent_activity(screen, false);
        assert_eq!(activity, Activity::Error);
        let (kind, detail) = err.expect("種別が付くこと");
        assert_eq!(kind.as_str(), "entitlement_blocked");
        assert_eq!(kind.recommended_action(), "needs_human");
        assert!(detail.contains("usage allocation has been disabled"));
    }

    #[test]
    fn 作業中の画面はエラー判定にかけない() {
        // busy 中の detect_worker_error はツール実行ログへ誤検知する契約
        let screen = format!("{CLAUDE_BUSY}\nClaude usage limit reached. reset at 3am");
        assert_eq!(agent_activity(&screen, false).0, Activity::Busy);
    }

    #[test]
    fn summarizeはエージェントにだけ活動状態を付ける() {
        let agent = summarize(&lines(CLAUDE_BUSY), true, false);
        assert_eq!(agent["activity"].as_str(), Some("busy"));
        assert!(agent["preview"].as_array().is_some_and(|a| !a.is_empty()));
        assert!(agent["error"].is_null());

        let shell = summarize(&lines("$ ls\nREADME.md"), false, false);
        assert!(shell["activity"].is_null(), "シェルの状態は OSC 133 が正");
        assert_eq!(
            shell["preview"].as_array().map(Vec::len),
            Some(2),
            "スニペットはシェルにも出す"
        );
    }

    #[test]
    fn summarizeのerrorはmasterと同じ形で返す() {
        let screen = lines("Claude usage limit reached. Your limit will reset at 3am.\n❯ ");
        let out = summarize(&screen, true, false);
        assert_eq!(out["activity"].as_str(), Some("error"));
        assert_eq!(out["error"]["kind"].as_str(), Some("usage_limit"));
        assert_eq!(
            out["error"]["recommended_action"].as_str(),
            Some("wait_reset"),
            "worker_status の error フィールドと同じ語彙にする"
        );
    }
}
