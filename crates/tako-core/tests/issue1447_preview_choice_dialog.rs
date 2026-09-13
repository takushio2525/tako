//! preview パネルつき AskUserQuestion（2 カラム配置）の検知（Issue #1447）
//!
//! # 症状（2026-09-13 21:50 の実運用・claude 2.1.258・幅 60 桁）
//!
//! AskUserQuestion の `preview` オプションつき（選択肢の**右**に枠つきの副画面が
//! 並ぶ side-by-side layout）の 3 択で worker が止まっているのに、
//! `read_pane` / `worker_status` の `choice_dialog` が `null`・watch が
//! `WORKER_DIALOG` を出さず・`respond` が「ダイアログが画面に存在しない」で落ちた。
//!
//! # 真因（このテストの注入 A/B で名指しした）
//!
//! 枠が混ざること**ではなく**、**折り返しの継続行の字下げ**だった。実採取では
//!
//! ```text
//! ❯ 1. 権限を許可して私が流す（    ┌────────────────────────┐   ← 中身の桁 = 5
//!     推奨）                       │ 許可が要る aws         │   ← 続きの桁 = 4
//! ```
//!
//! 続きが**中身の桁より浅い**ので `gap_line_kind` が「ダイアログの外」と分類し、
//! `numbered_block` が選択肢 1 個で打ち切られて `< 2` で降りていた。
//! 枠を剥がしただけでは直らない（[`枠を剥がしただけでは直らない`] で固定）。
//!
//! 枠そのものはラベルを汚していた（`権限を許可して私が流す（    ┌───┐`）ので、
//! 副画面の剥がしも要る。修正はその 2 つで、A/B は `TAKO_1447_LEGACY=1`
//! （`issue1447_legacy_ab.rs`）。

use tako_core::dialog::{detect_choice_list, ChoiceList};

/// Issue #1447 本文の画面テキストそのまま（幅 60 桁・個人情報なし）
const PREVIEW_ASK_60: &str = include_str!("fixtures/issue1447-preview-ask-60.txt");

/// 同じダイアログを更に狭い 38 桁で描いた形（選択肢 1 が **3 行**に折れる）
const PREVIEW_ASK_38: &str = r#"──────────────────────────────────────
 ☐ AWS 適用

│ #62 は完了して PR #66 を出しま
│ した。どう進めますか？

❯ 1. 権限を許可し   ┌────────────────┐
    て私が流す      │ 許可が要る aws │
    （推奨）        ├─── ✂ ─── 17
  2. 自分で流す     ┤
  3. AWS は後回     │ lines hidden   │
    し、PR だけ進   └────────────────┘
    める

                    Notes: press n to add

──────────────────────────────────────
  Chat about this

Enter to select · ↑/↓ to navigate ·
Esc to cancel"#;

/// エッジ: 副画面が選択肢より**高い**（2 択・パネル 6 行。左に本文が無い行が並ぶ）
const PANEL_TALLER: &str = r#"────────────────────────────────────────────────────────────
 ☐ 反映するか

│ どうしますか？

❯ 1. いますぐ反映する         ┌────────────────────────┐
  2. あとで反映する           │ 差分 3 ファイル        │
                              │ src/a.rs               │
                              │ src/b.rs               │
                              │ src/c.rs               │
                              └────────────────────────┘

Enter to select · ↑/↓ to navigate · Esc to cancel"#;

/// エッジ: 副画面が選択肢より**低い**（4 択・ハイライトは 2 番目）
const PANEL_SHORTER: &str = r#"────────────────────────────────────────────────────────────
 ☐ どの版を使うか

│ 4 つあります。

  1. 安定版を使う             ┌────────────────────────┐
❯ 2. テスト版を使う           │ v0.8.12 (prerelease)   │
  3. 自分でビルドする         └────────────────────────┘
  4. 決めない

Enter to select · ↑/↓ to navigate · Esc to cancel"#;

/// エッジ: 副画面の**中身が空**（枠だけ）
const PANEL_EMPTY: &str = r#"────────────────────────────────────────────────────────────
 ☐ 空のプレビュー

│ 中身がありません。

❯ 1. 続ける                   ┌────────────────────────┐
  2. やめる                   │                        │
  3. あとで決める             └────────────────────────┘

Enter to select · ↑/↓ to navigate · Esc to cancel"#;

/// 反例: **全幅の箱**の中に番号つき選択肢が並ぶ形（右端の `│` が縦に揃う）。
/// 副画面と誤認して右端で切ると、箱の中の一覧まで前処理の対象になる
const FULL_WIDTH_BOX: &str = r#"╭──────────────────────────────────────────────╮
│   Select model                               │
│                                              │
│ ❯ 1. Default (recommended)                   │
│   2. Opus (1M context)                       │
│   3. Sonnet                                  │
│                                              │
│ Enter to set as default · Esc to cancel      │
╰──────────────────────────────────────────────╯"#;

fn rows(s: &str) -> Vec<&str> {
    s.lines().collect()
}

fn detect(name: &str, screen: &str) -> ChoiceList {
    detect_choice_list(&rows(screen)).unwrap_or_else(|| {
        panic!("{name} がダイアログとして検知されない（#1447 の症状）:\n{screen}")
    })
}

fn labels(list: &ChoiceList) -> Vec<String> {
    list.options.iter().map(|o| o.label.clone()).collect()
}

/// 受け入れ条件 1: Issue の画面原文で 3 択（番号つき・ハイライト 1 番目）が採れる
#[test]
fn issue1447_実採取のpreviewつき三択を検知する() {
    let list = detect("Issue #1447 の画面原文", PREVIEW_ASK_60);
    assert!(list.numbered, "番号つきとして採れていない: {list:?}");
    assert_eq!(list.highlighted, Some(0), "ハイライトが 1 番目でない");
    assert!(list.cursor_row.is_some(), "選択カーソルが読めていない");
    assert_eq!(
        list.options
            .iter()
            .filter_map(|o| o.number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
        "番号が 1..3 で揃っていない: {:?}",
        labels(&list)
    );
    // 副画面の枠がラベルへ混ざっていない（respond のラベル一致・#813 の安全な
    // 選択肢の選別がここを読む）
    assert_eq!(
        labels(&list),
        vec![
            "権限を許可して私が流す（推奨）",
            "自分で流す",
            "AWS は後回し、PR だけ進める",
        ],
        "ラベルに副画面が残っている / 折り返しが繋がっていない"
    );
    assert!(
        !list.options.iter().any(|o| o.label_truncated),
        "切り詰め扱いになっている（`…` は画面に無い）"
    );
    assert!(
        list.header.join(" ").contains("どう進めますか？"),
        "本文が採れていない: {:?}",
        list.header
    );
    eprintln!("[1447] 原文 60 桁: {:?}", labels(&list));
}

/// 受け入れ条件 2: もっと狭い幅で折り返しが増えても同じ 3 択になる
#[test]
fn issue1447_狭いペインで折り返しが増えても同じ三択になる() {
    let wide = detect("60 桁", PREVIEW_ASK_60);
    let narrow = detect("38 桁", PREVIEW_ASK_38);
    assert_eq!(narrow.options.len(), 3, "38 桁で 3 択にならない");
    assert_eq!(narrow.highlighted, Some(0));
    assert!(narrow.numbered);
    assert_eq!(
        labels(&narrow),
        labels(&wide),
        "幅が違うと別のラベルになる（折り返しの結合が効いていない）"
    );
    eprintln!("[1447] 38 桁: {:?}", labels(&narrow));
}

/// エッジ: 副画面が選択肢より高い / 低い / 空でも、採るのは選択肢だけ
#[test]
fn issue1447_副画面の高さが選択肢と食い違っても選択肢だけを採る() {
    let taller = detect("パネルが高い", PANEL_TALLER);
    assert_eq!(
        labels(&taller),
        vec!["いますぐ反映する", "あとで反映する"],
        "左に本文が無いパネル行がラベルへ流れ込んでいる"
    );
    assert_eq!(taller.highlighted, Some(0));

    let shorter = detect("パネルが低い", PANEL_SHORTER);
    assert_eq!(
        labels(&shorter),
        vec![
            "安定版を使う",
            "テスト版を使う",
            "自分でビルドする",
            "決めない",
        ]
    );
    assert_eq!(shorter.highlighted, Some(1), "ハイライトが 2 番目でない");

    let empty = detect("パネルが空", PANEL_EMPTY);
    assert_eq!(labels(&empty), vec!["続ける", "やめる", "あとで決める"]);
    assert_eq!(empty.highlighted, Some(0));
    eprintln!(
        "[1447] エッジ: 高い={:?} 低い={:?} 空={:?}",
        labels(&taller),
        labels(&shorter),
        labels(&empty)
    );
}

/// 前処理が**副画面のときだけ**効くこと。全幅の箱の右端は副画面ではない。
///
/// 副画面と誤認すると右端の `│` の手前で全行が切られ、箱の中の一覧まで前処理の
/// 対象になる。ここでは **#1447 前とまったく同じ結果**（ラベルの末尾に箱の
/// 右罫線が残る）であることを固定する = 前処理が 1 文字も触っていない証拠。
/// 罫線が残るのは #1447 以前からの挙動で、この Issue の範囲では変えない
#[test]
fn issue1447_全幅の箱の右端を副画面と誤認しない() {
    let list = detect("全幅の箱", FULL_WIDTH_BOX);
    assert_eq!(list.options.len(), 3);
    assert_eq!(list.highlighted, Some(0));
    for (label, want) in
        labels(&list)
            .iter()
            .zip(["Default (recommended)", "Opus (1M context)", "Sonnet"])
    {
        assert!(label.starts_with(want), "ラベルが変わっている: {label:?}");
        assert!(
            label.ends_with('│'),
            "右端の罫線が消えている = 副画面として切られた: {label:?}"
        );
    }
}

/// 真因の名指し: **枠を剥がしただけでは直らない**（継続行の字下げが本体）
#[test]
fn issue1447_枠を剥がしただけでは直らない() {
    // 副画面を人手で落とした画面（= 前処理だけが効いた状態）。継続行の字下げは
    // 実採取のまま 4 桁で、中身の桁 5 より浅い
    let stripped: Vec<String> = PREVIEW_ASK_60
        .lines()
        .map(|l| {
            match l
                .find("  ┌")
                .or_else(|| l.find("  │"))
                .or_else(|| l.find("  ├"))
                .or_else(|| l.find("  ┤"))
                .or_else(|| l.find("  └"))
            {
                Some(i) => l[..i].trim_end().to_string(),
                None => l.to_string(),
            }
        })
        .collect();
    let refs: Vec<&str> = stripped.iter().map(String::as_str).collect();
    assert!(
        refs.iter().any(|l| l.trim() == "推奨）"),
        "枠を落とした画面が組めていない: {refs:?}"
    );
    // 継続行がそのままなら 3 択で採れる（下限を緩めた側が効いている証拠）
    let list = detect_choice_list(&refs).expect("枠なしでも 3 択が採れる");
    assert_eq!(list.options.len(), 3);
    // 継続行を**中身の桁へ揃え直す**と、修正前でも採れていた形になる
    // （= 4 桁だったことが不検知の原因という名指し）
    let realigned: Vec<String> = refs
        .iter()
        .map(|l| match l.trim() {
            "推奨）" | "だけ進める" => format!(" {l}"),
            _ => (*l).to_string(),
        })
        .collect();
    let refs2: Vec<&str> = realigned.iter().map(String::as_str).collect();
    assert_eq!(
        detect_choice_list(&refs2).map(|l| l.options.len()),
        Some(3),
        "字下げを 5 桁へ揃えた形が採れない（前提が崩れている）"
    );
}
