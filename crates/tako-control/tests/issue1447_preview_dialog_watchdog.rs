//! preview パネルつきダイアログの検知が逆戻りしないための番犬（Issue #1447）
//!
//! # なぜ要るか
//!
//! `tako_core::dialog` の選択肢検知は、AskUserQuestion の `preview` つき
//! （選択肢の右へ枠つきの副画面が並ぶ side-by-side layout）で `None` を返していた。
//! 真因は 2 つあって、**どちらか片方だけ戻しても症状が丸ごと再発する**:
//!
//! 1. **折り返しの続きの下限**が「中身の桁」だった。実採取では続きが**中身の桁より
//!    1 桁浅い**（`❯ 1. …` の中身は 5 桁・続きは 4 桁）ので、あいだの行が
//!    「ダイアログの外」に分類され、`numbered_block` が選択肢 1 個で打ち切られる
//! 2. **副画面が同じ行に同居する**。剥がさないとラベルへ枠が丸ごと入り、
//!    `respond` のラベル一致と #813 の安全な選択肢の選別が静かに外れる
//!
//! 逆戻りは「1 行の書き戻し」で起きる（`wrap_indent_floor(` を
//! `content_start_column(` へ戻す / `side_panel_cuts(` の呼び出しを外す）ので、
//! 判定がどこから出ているかを構造で縛る。
//!
//! # 何を縛るか
//!
//! 1. `detect_choice_list` が副画面の剥がし（`side_panel_cuts` → `strip_side_panel`）を通る
//! 2. あいだの行の分類（`numbered_gap_ok` / `dialog_reaches_input_box`）は
//!    `wrap_indent_floor` の 1 実装から下限を採る（`content_start_column` を直に使わない）
//! 3. `gap_line_kind` は**別の要素**（選択カーソル行・番号つき行）を続きにしない
//!    （下限を緩めたぶんの歯止め。#1263 / #1293 の安全側を保つ）
//! 4. `panel_cut` は「行まるごと副画面」を先に外す（副画面の**内側**の余白を掴むと、
//!    残った枠線が直前の選択肢のラベルへ結合される）
//! 5. `side_panel_cuts` は 3 条件（左上角 / 2 行以上 / 番号つき選択肢と同居）を
//!    **すべて**要求する（全幅の箱の右端を副画面と誤認しない）
//! 6. 副画面の枠線の**文字集合はリテラルで散らさない**（`PANEL_EDGE` /
//!    `PANEL_TOP_LEFT` と `is_rule_line` の外に箱の文字を書かない）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜6 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で窓が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**修正前ソースを再現した 8 通りの注入**が
//! file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

const DIALOG: &str = "crates/tako-core/src/dialog.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug, PartialEq, Eq)]
struct Offender {
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{DIALOG}:{} — {}", self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言から N 行」で切ると隣を巻き込む）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
/// （「なぜそう書くか」を注釈で残す規約なので、説明で落ちる形にはしない）
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// 副画面の枠に使う箱の文字（`PANEL_EDGE` の外へ散らばっていないかを見る目印）
const BOX_CHARS: [char; 2] = ['┌', '╭'];

fn scan(src: &str) -> Vec<Offender> {
    let mut out: Vec<Offender> = Vec::new();
    let mut push = |line: usize, why: String| out.push(Offender { line, why });

    // 1. detect_choice_list が副画面の剥がしを通る
    match fn_window(src, "pub fn detect_choice_list(") {
        None => push(
            0,
            "`detect_choice_list` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            for needle in ["side_panel_cuts(", "strip_side_panel("] {
                if !code.contains(needle) {
                    push(
                        at,
                        format!(
                            "`{needle}` を通っていない（副画面つきの画面で選択肢の行末に \
                             枠線が混ざったまま判定される。#1447）"
                        ),
                    );
                }
            }
        }
    }

    // 2. あいだの行の下限は `wrap_indent_floor` の 1 実装から採る
    for name in ["fn numbered_gap_ok(", "fn dialog_reaches_input_box("] {
        match fn_window(src, name) {
            None => push(0, format!("`{name}` が見つからない（走査が空振り）")),
            Some((at, window)) => {
                if let Some(i) = code_line_with(&window, "content_start_column(") {
                    push(
                        at + i,
                        format!(
                            "`{name}` が中身の桁を直に下限にしている（副画面つきの続きは \
                             中身の桁より浅いので「ダイアログの外」に落ちる = #1447 の真因。\
                             `wrap_indent_floor` を通すこと）"
                        ),
                    );
                }
                if !code_only(&window).contains("wrap_indent_floor(") {
                    push(
                        at,
                        format!("`{name}` が `wrap_indent_floor` を通っていない（下限の 1 実装）"),
                    );
                }
            }
        }
    }

    // 3. gap_line_kind の歯止め（別の要素を続きにしない）
    match fn_window(src, "fn gap_line_kind(") {
        None => push(0, "`gap_line_kind` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            let guarded = code.contains("cursor_content(line).is_some()")
                && code.contains("numbered_choice(strip_indent(line))");
            if !guarded {
                push(
                    at,
                    "`gap_line_kind` に選択カーソル行 / 番号つき行を続き（`Continuation`）から \
                     外す歯止めが無い（#1447 で下限を緩めたので、別の一覧をまたいで \
                     「あいだはダイアログの一部だった」と言えてしまう。#1263 / #1293 の \
                     安全側が消える）"
                        .into(),
                );
            }
        }
    }

    // 4. panel_cut は「行まるごと副画面」を先に外す
    match fn_window(src, "fn panel_cut(") {
        None => push(0, "`panel_cut` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            match code_line_with(&window, "panel_only_cut(") {
                None => push(
                    at,
                    "`panel_only_cut` で「行まるごと副画面」を外していない（副画面の**内側**の \
                     余白を掴み、残った枠線が直前の選択肢のラベルへ結合される。実測: \
                     `あとで反映する│ src/a.rs │ src/b.rs`）"
                        .into(),
                ),
                Some(i) => {
                    // 外すのは走査より**前**（後ろに置くと内側の余白を先に返す）
                    let scan_at = code_line_with(&window, "for (idx, c) in line.char_indices()");
                    if scan_at.is_some_and(|s| s < i) {
                        push(at + i, "`panel_only_cut` の除外が走査より後ろにある".into());
                    }
                }
            }
            if !code.contains("PANEL_GUTTER") {
                push(
                    at,
                    "副画面の左の余白の下限が `PANEL_GUTTER` から出ていない".into(),
                );
            }
        }
    }

    // 5. side_panel_cuts は 3 条件をすべて要求する
    match fn_window(src, "fn side_panel_cuts(") {
        None => push(0, "`side_panel_cuts` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            // 3 条件は**関門の 1 行**（`if !(… && … && …)`）で揃って要求されていること。
            // 判定に使う識別子がどこかに在るだけでは足りない（関門から外すと発火する）
            let gate = code
                .lines()
                .find(|l| l.contains("if !(") && l.contains("return None"))
                .map(str::to_string)
                .or_else(|| {
                    code.lines()
                        .find(|l| l.contains("if !("))
                        .map(str::to_string)
                })
                .unwrap_or_default();
            let gate_at = code_line_with(&window, "if !(").unwrap_or(0);
            for (needle, why) in [
                (
                    "top_left",
                    "枠の**左上角**（`PANEL_TOP_LEFT`）を関門で要求していない（全幅の箱の \
                     右端 `│` が並ぶだけの画面を副画面と誤認し、箱の中の一覧まで切ってしまう）",
                ),
                (
                    "rows >= 2",
                    "副画面の行数の下限（箱は上端と下端で最低 2 行）を関門で要求していない",
                ),
                (
                    "with_option",
                    "**番号つき選択肢と同居している**ことを関門で要求していない（これが \
                     「2 カラム配置」の定義。会話ログの罫線で発火する）",
                ),
            ] {
                if !gate.contains(needle) {
                    push(
                        at + gate_at,
                        format!("`side_panel_cuts` が {why}（`{needle}`）"),
                    );
                }
            }
            if !code.contains("PANEL_TOP_LEFT") {
                push(
                    at,
                    "`PANEL_TOP_LEFT` を見ていない（左上角の判定が消えている）".into(),
                );
            }
        }
    }

    // 6. 箱の文字はリテラルで散らさない
    let mut allowed: Vec<(usize, usize)> = Vec::new();
    // 枠の定義（`const PANEL_… = &[…];`）と罫線判定の窓だけが書いてよい場所。
    // 定義は複数行に折り返される（rustfmt）ので**宣言から `];` まで**を許す
    let lines: Vec<&str> = src.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("const PANEL_EDGE") && !line.contains("const PANEL_TOP_LEFT") {
            continue;
        }
        let end = lines
            .iter()
            .enumerate()
            .skip(i)
            .find(|(_, l)| l.trim_end().ends_with("];"))
            .map(|(j, _)| j)
            .unwrap_or(i);
        allowed.push((i + 1, end + 1));
    }
    if let Some((at, w)) = fn_window(src, "pub fn is_rule_line(") {
        allowed.push((at, at + w.lines().count()));
    }
    for (i, line) in lines.iter().enumerate() {
        if !BOX_CHARS.iter().any(|c| line.contains(*c)) {
            continue;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        let at = i + 1;
        if allowed.iter().any(|(from, to)| at >= *from && at <= *to) {
            continue;
        }
        push(
            at,
            "箱の文字をリテラルで書いている（副画面の枠の定義は `PANEL_EDGE` / \
             `PANEL_TOP_LEFT`、罫線判定は `is_rule_line` の 1 実装に寄せる）"
                .into(),
        );
    }

    out
}

fn report(offenders: &[Offender]) -> String {
    offenders
        .iter()
        .map(Offender::report)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn 走査が空振りしていない() {
    let src = production(DIALOG);
    for needle in [
        "pub fn detect_choice_list(",
        "fn numbered_gap_ok(",
        "fn dialog_reaches_input_box(",
        "fn gap_line_kind(",
        "fn panel_cut(",
        "fn panel_only_cut(",
        "fn side_panel_cuts(",
        "fn wrap_indent_floor(",
        "fn marker_start_column(",
        "pub fn is_rule_line(",
    ] {
        assert!(
            fn_window(&src, needle).is_some(),
            "{DIALOG} に `{needle}` の窓が無い（番犬が何も見ていない）"
        );
    }
    // 本番コードが丸ごと潰れていないこと（#1420）
    let kept = production_range::scan(&read(DIALOG));
    assert!(
        kept.coverage() > 0.4,
        "本番コードの割合が {:.1}% しか無い（走査範囲が縮んだ）: {}",
        kept.coverage() * 100.0,
        kept.describe_regions()
    );
}

#[test]
fn 現行ソースは違反ゼロ() {
    let offenders = scan(&production(DIALOG));
    assert!(
        offenders.is_empty(),
        "#1447 の逆戻り:\n{}",
        report(&offenders)
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let src = production(DIALOG);
    // (注入の名前, 置換前, 置換後, 名指しに含まれるべき語)
    let injections: [(&str, &str, &str, &str); 8] = [
        (
            "副画面の剥がしを外す（detect_choice_list が素の行で判定する）",
            "match side_panel_cuts(lines) {",
            "match None::<Vec<Option<usize>>> {",
            "side_panel_cuts(",
        ),
        (
            "剥がしの適用を外す",
            "let stripped = strip_side_panel(lines, &cuts);",
            "let stripped: Vec<String> = lines.iter().map(|l| l.to_string()).collect();",
            "strip_side_panel(",
        ),
        (
            "numbered_gap_ok の下限を中身の桁へ戻す（#1447 の真因そのもの）",
            "fn numbered_gap_ok(lines: &[&str], upper: usize, lower: usize) -> bool {\n    let content_col = wrap_indent_floor(lines[upper]).unwrap_or(0);",
            "fn numbered_gap_ok(lines: &[&str], upper: usize, lower: usize) -> bool {\n    let content_col = content_start_column(lines[upper]).unwrap_or(0);",
            "numbered_gap_ok",
        ),
        (
            "dialog_reaches_input_box の下限を中身の桁へ戻す",
            "fn dialog_reaches_input_box(lines: &[&str], last_row: usize, input_row: usize) -> bool {\n    let content_col = wrap_indent_floor(lines[last_row]).unwrap_or(0);",
            "fn dialog_reaches_input_box(lines: &[&str], last_row: usize, input_row: usize) -> bool {\n    let content_col = content_start_column(lines[last_row]).unwrap_or(0);",
            "dialog_reaches_input_box",
        ),
        (
            "gap_line_kind の歯止め（別の要素を続きにしない）を外す",
            "    let is_element = !legacy_side_panel()\n        && (cursor_content(line).is_some() || numbered_choice(strip_indent(line)).is_some());",
            "    let is_element = false;",
            "gap_line_kind",
        ),
        (
            "panel_cut の「行まるごと副画面」の除外を外す",
            "    if panel_only_cut(line).is_some() {\n        return None;\n    }\n    let mut seen_content = false;",
            "    let mut seen_content = false;",
            "panel_only_cut",
        ),
        (
            "side_panel_cuts から左上角の条件を落とす",
            "    if !(top_left && rows >= 2 && with_option) {",
            "    if !(rows >= 2 && with_option) {",
            "top_left",
        ),
        (
            "side_panel_cuts から「番号つき選択肢と同居」の条件を落とす",
            "    if !(top_left && rows >= 2 && with_option) {",
            "    if !(top_left && rows >= 2) {",
            "with_option",
        ),
    ];

    for (name, from, to, want) in injections {
        assert!(
            src.contains(from),
            "注入«{name}»の置換前が現行ソースに無い（番犬の前提が古い）:\n{from}"
        );
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入«{name}»が効いていない");
        let offenders = scan(&injected);
        assert!(
            !offenders.is_empty(),
            "注入«{name}»を検出できない（番犬が見逃す側へ倒れている）"
        );
        let text = report(&offenders);
        assert!(
            text.contains(want),
            "注入«{name}»の名指しに `{want}` が無い:\n{text}"
        );
        assert!(
            offenders.iter().all(|o| o.line > 0),
            "注入«{name}»で行番号を名指しできていない:\n{text}"
        );
        eprintln!("[1447-watchdog] «{name}» -> {}", text.replace('\n', " / "));
    }
}
