//! synthetic_input — tako 自身が描く**合成入力欄**の形（Issue #1913）
//!
//! セルフテスト / visual-test は claude の入力欄に似た箱（罫線 + `❯` の行）を
//! 自分で描いて、入力欄の検査（ミラー・高さ・IME・再起動の関門）に使う。
//! これを `dialog` がダイアログと誤判定すると tako の検査自体が止まるので、
//! #754 の単体テスト（`dialog` の `takoが自分で描く合成入力欄をダイアログと誤判定しない`）が
//! 形を固定している。
//!
//! 描く側（`tako-app` の `self_test`）と単体テストが**同じ組み立て**から形を作るよう、
//! ここへ寄せた。手で写していた頃は描く側の変更（罫線の桁数・本文・フッター）に
//! テストが追従せず、古い形を検査したまま緑になっていた（8 月の版で実際にずれていた）。
//!
//! - `*_lines`: 画面に出る行（単体テストが `detect_choice_list` へ渡す形）
//! - `*_command` / `*_paint`: 描く側がペインへ送るコマンド / 疑似 TUI のファイルの中身。
//!   **必ず `*_lines` から作る**（行とバイトが別々に組まれるとまたずれる）
//!
//! 本文（状態ごとの文言）は呼び手が渡す。キュー滞留の案内のように
//! `tako-control` の定数を使う状態があり、依存の向き上ここからは引けないため

/// 罫線の 1 桁
const RULE: char = '\u{2500}';
/// 入力行の先頭の `❯`
const PROMPT: char = '\u{276F}';
/// `❯` の後の空白（#737 / #1067）。NBSP = 実 claude の採取どおりの形
const NBSP: char = '\u{a0}';

/// visual-test の高さ検査（#718）が描き分ける行数。1 → 2 → 4 行と伸ばして、
/// 行が増えたぶんだけ入力欄が伸びることを見る
pub const AUTOGROW_ROWS: [usize; 3] = [1, 2, 4];

fn rule(width: usize) -> String {
    RULE.to_string().repeat(width)
}

/// 上下を罫線で挟んだ入力ボックス
fn boxed(rule_width: usize, body: impl IntoIterator<Item = String>) -> Vec<String> {
    let rule = rule(rule_width);
    std::iter::once(rule.clone())
        .chain(body)
        .chain(std::iter::once(rule))
        .collect()
}

/// #719（visual-test のチャット G3）: `clear` 直後の空行 + 罫線 16 桁の 2 行入力
pub fn chat_g3_lines() -> Vec<String> {
    let mut lines = vec![String::new()];
    lines.extend(boxed(
        16,
        [
            format!("{PROMPT} テストの依頼を書きました"),
            "  2 行目もあります".to_string(),
        ],
    ));
    lines
}

/// #719 をペインへ描くコマンド（`clear` してから printf で [`chat_g3_lines`] を出す）
pub fn chat_g3_command() -> String {
    format!("clear; printf '{}\\n'", chat_g3_lines().join("\\n"))
}

/// #718（visual-test の高さ検査）: 罫線 16 桁の `rows` 行入力
pub fn autogrow_lines(rows: usize) -> Vec<String> {
    boxed(
        16,
        (0..rows).map(|i| {
            if i == 0 {
                format!("{PROMPT} row0")
            } else {
                format!("  row{i}")
            }
        }),
    )
}

/// #718 をペインへ描くコマンド。非 ASCII は `\uXXXX` にして `printf '%b'` に展開させる
pub fn autogrow_command(rows: usize) -> String {
    let escaped: Vec<String> = autogrow_lines(rows)
        .iter()
        .map(|line| escape_non_ascii(line))
        .collect();
    format!("printf '%b' '{}\\n'", escaped.join("\\n"))
}

fn escape_non_ascii(line: &str) -> String {
    line.chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_string()
            } else {
                format!("\\u{:04X}", c as u32)
            }
        })
        .collect()
}

/// #737（セルフテストの GUI 入力欄）: 罫線 20 桁 + `❯` と NBSP + 本文の 1 行
pub fn gui_input_lines(body: &str) -> Vec<String> {
    boxed(20, [format!("{PROMPT}{NBSP}{body}")])
}

/// #737 の疑似 TUI のファイルの中身。箱を描いたあとカーソルを**入力行の末尾へ戻す**
/// （`\e[2A` で 2 行上、`\e[NC` で N 桁右）。末尾がカーソル移動なので、それでは IME の
/// キャレットが「箱の外」になって位置検査ができない
pub fn gui_input_paint(body: &str) -> String {
    let width: usize = body.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    let col = 2 + width;
    format!(
        "{}\n\u{1b}[2A\u{1b}[{col}C",
        gui_input_lines(body).join("\n")
    )
}

/// #1067（セルフテストのセッション再起動）: 直前の応答 + 罫線 30 桁の 1 行 + フッター 2 行。
/// `footer` に生成中ヒントや上限行を差し替えて関門を作る
pub fn restart_tui_lines(body: &str, footer: &str) -> Vec<String> {
    let mut lines = vec!["⏺ 直前の応答".to_string(), String::new()];
    lines.extend(boxed(30, [format!("{PROMPT}{NBSP}{body}")]));
    lines.push("  [Opus 5 · xH]  ▸ 2.1.258".to_string());
    lines.push(format!("  {footer}"));
    lines
}

/// #1067 の疑似 TUI のファイルの中身（画面を消して左上から [`restart_tui_lines`] を描く）
pub fn restart_tui_paint(body: &str, footer: &str) -> String {
    format!(
        "\u{1b}[2J\u{1b}[H{}\n",
        restart_tui_lines(body, footer).join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // 寄せる前（#1913 の直前の main）の描く側が送っていたバイトそのもの。
    // 罫線の桁数・NBSP・カーソル移動の桁まで 1 バイトでも変わったら落ちる
    // （寄せた結果として画面の字面が変わっていないことの固定）

    #[test]
    fn issue719の描くコマンドは寄せる前とバイト一致() {
        assert_eq!(
            chat_g3_command(),
            "clear; printf '\\n────────────────\\n❯ テストの依頼を書きました\\n  2 行目もあります\\n────────────────\\n'"
        );
    }

    #[test]
    fn issue718の描くコマンドは寄せる前とバイト一致() {
        let rule = "\\u2500".repeat(16);
        let want = |body: &str| format!("printf '%b' '{rule}\\n{body}\\n{rule}\\n'");
        assert_eq!(autogrow_command(1), want("\\u276F row0"));
        assert_eq!(autogrow_command(2), want("\\u276F row0\\n  row1"));
        assert_eq!(
            autogrow_command(4),
            want("\\u276F row0\\n  row1\\n  row2\\n  row3")
        );
        assert_eq!(AUTOGROW_ROWS, [1, 2, 4]);
    }

    #[test]
    fn issue737の描く中身は寄せる前とバイト一致() {
        let rule = "─".repeat(20);
        // 本文は描き分ける 4 状態。カーソルを戻す桁は「`❯` + NBSP の 2 桁 + 本文の幅」
        for (body, col) in [
            ("Try \"how does <filepath> work?\"", 33),
            ("", 2),
            ("G4 mo", 7),
            ("busy中の追加指示", 18),
        ] {
            assert_eq!(
                gui_input_paint(body),
                format!("{rule}\n❯\u{a0}{body}\n{rule}\n\u{1b}[2A\u{1b}[{col}C"),
                "本文 {body:?}"
            );
        }
    }

    #[test]
    fn issue1067の描く中身は寄せる前とバイト一致() {
        let rule = "─".repeat(30);
        let footer = "⏵⏵ auto mode on (shift+tab to cycle) · ← for agents";
        assert_eq!(
            restart_tui_paint("TAKO1067IDLE", footer),
            format!(
                "\u{1b}[2J\u{1b}[H⏺ 直前の応答\n\n{rule}\n❯\u{a0}TAKO1067IDLE\n{rule}\n  \
                 [Opus 5 · xH]  ▸ 2.1.258\n  {footer}\n"
            )
        );
    }

    // 描くバイトから画面に出る行を戻すと `*_lines` に一致する = 単体テストが検査する行と
    // 描く側が出す行は同じもの（片方だけ組み直すと、ここで落ちる）

    #[test]
    fn 描くバイトは単体テストが見る行そのもの() {
        let g3 = chat_g3_command();
        let inner = g3
            .strip_prefix("clear; printf '")
            .and_then(|s| s.strip_suffix("\\n'"))
            .expect("#719 のコマンドの形");
        assert_eq!(
            inner.split("\\n").map(String::from).collect::<Vec<_>>(),
            chat_g3_lines()
        );

        for rows in AUTOGROW_ROWS {
            let cmd = autogrow_command(rows);
            let inner = cmd
                .strip_prefix("printf '%b' '")
                .and_then(|s| s.strip_suffix("\\n'"))
                .expect("#718 のコマンドの形");
            let decoded: Vec<String> = inner.split("\\n").map(unescape_u).collect();
            assert_eq!(decoded, autogrow_lines(rows), "{rows} 行");
        }

        for body in ["案内", "", "G4 mo"] {
            let paint = gui_input_paint(body);
            let (drawn, tail) = paint
                .split_once("\n\u{1b}[2A")
                .expect("#737 のカーソル戻し");
            assert!(tail.starts_with("\u{1b}[") && tail.ends_with('C'));
            assert_eq!(
                drawn.split('\n').map(String::from).collect::<Vec<_>>(),
                gui_input_lines(body)
            );
        }

        let paint = restart_tui_paint("TAKO1067BUSY", "esc to interrupt");
        let drawn = paint
            .strip_prefix("\u{1b}[2J\u{1b}[H")
            .and_then(|s| s.strip_suffix('\n'))
            .expect("#1067 の画面消去と末尾の改行");
        assert_eq!(
            drawn.split('\n').map(String::from).collect::<Vec<_>>(),
            restart_tui_lines("TAKO1067BUSY", "esc to interrupt")
        );
    }

    /// `\uXXXX`（4 桁）を文字へ戻す（`printf '%b'` の展開の写し。テスト専用）
    fn unescape_u(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find("\\u") {
            out.push_str(&rest[..i]);
            let hex = &rest[i + 2..i + 6];
            out.push(char::from_u32(u32::from_str_radix(hex, 16).expect("16 進")).expect("文字"));
            rest = &rest[i + 6..];
        }
        out.push_str(rest);
        out
    }
}
