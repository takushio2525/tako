//! 狭いペインの選択肢ダイアログで、ラベルの折り返しと説明の境目を読む（Issue #2302）
//!
//! # 症状（#1847 / PR #2298 の worker の報告。実採取の fixture で再現した）
//!
//! 32 桁で説明の長い AskUserQuestion を構造解析すると、#1131 の折り返しの結合が
//!
//! - 選択肢 1 のラベルの続き（`の判定を先に直す（推奨）`）を落として `status` だけにする
//! - 選択肢 3 のラベルへ説明 7 行を丸ごと混ぜる
//!
//! のに、どちらも `label_truncated=false` だった。respond のラベル指定は部分一致なので、
//! 説明の文言で別の選択肢を確定しうる（番号の respond は正しく効いていた）。
//!
//! # 真因
//!
//! ラベルの折り返しと説明は**同じ桁へ字下げされ、同じ右端で折り返す**ので字下げでは
//! 見分けられない。旧実装は「前の行に次の語がまだ入るか」を char 数と「画面幅 - 3」で
//! 見積もっていたので、全角の続きを「まだ入る = 別の要素」と読み（選択肢 1）、説明の行を
//! 「入らない = 続き」と読んでいた（選択肢 3）。判定は `dialog::label_ends`（表示幅で測った
//! 段落の切れ目・説明つきの並びかどうか）。A/B は `TAKO_2302_LEGACY=1`。
//!
//! # fixture（すべて実採取・個人情報なし）
//!
//! - `issue1847-ask-*.txt`: #2298 が採った 32 / 60 桁（60 桁のラベルが正解）
//! - `issue2302-*.txt`: claude 2.1.301 を一時の設定 dir + 偽の Messages API（AskUserQuestion /
//!   Bash の tool_use を返す）で隔離 tmux に立て、幅だけ変えて `capture-pane -p` で採った画面。
//!   ヘッダのパスだけ `/…/work` に置き換えた
//!   - `ascii-*`: 英語の 4 択（選択肢 1 のラベルは 32 桁で 2 行に割れる）
//!   - `cjk-*`: 日本語の 2 択（どの選択肢にも段落の切れ目が出ない = 説明の証拠が無い形）
//!   - `perm-*`: Bash の permission（説明なし = 続きは全部ラベルの折り返し。#1131 の形）
//!   - `nl-32x70`: 説明に改行を含む 2 択（改行は U+FFFD で 1 行に流れる = 説明は 1 段落）

use std::process::Command;

use tako_core::dialog::{detect_choice_list, ChoiceList};

const ASK_32X46: &str = include_str!("fixtures/issue1847-ask-32x46.txt");
const ASK_32X70: &str = include_str!("fixtures/issue1847-ask-32x70.txt");
const ASK_32X70_LONG: &str = include_str!("fixtures/issue1847-ask-32x70-long.txt");
const ASK_60X50: &str = include_str!("fixtures/issue1847-ask-60x50.txt");
const ASCII_25: &str = include_str!("fixtures/issue2302-ascii-25x80.txt");
const ASCII_32: &str = include_str!("fixtures/issue2302-ascii-32x70.txt");
const ASCII_80: &str = include_str!("fixtures/issue2302-ascii-80x40.txt");
const CJK_32: &str = include_str!("fixtures/issue2302-cjk-32x70.txt");
const CJK_80: &str = include_str!("fixtures/issue2302-cjk-80x40.txt");
const PERM_25: &str = include_str!("fixtures/issue2302-perm-25x80.txt");
const PERM_32: &str = include_str!("fixtures/issue2302-perm-32x70.txt");
const PERM_80: &str = include_str!("fixtures/issue2302-perm-80x40.txt");
const NL_32: &str = include_str!("fixtures/issue2302-nl-32x70.txt");

const LEGACY_ENV: &str = "TAKO_2302_LEGACY";
const LEGACY_TEST: &str = "issue2302_legacyで境目の崩れが戻る";

#[track_caller]
fn detect(name: &str, screen: &str) -> ChoiceList {
    let rows: Vec<&str> = screen.lines().collect();
    detect_choice_list(&rows).unwrap_or_else(|| panic!("{name}: 選択肢ダイアログを検知できない"))
}

/// `(ラベル, label_truncated)` の並び
fn labels(list: &ChoiceList) -> Vec<(String, bool)> {
    list.options
        .iter()
        .map(|o| (o.label.clone(), o.label_truncated))
        .collect()
}

fn expect(rows: &[(&str, bool)]) -> Vec<(String, bool)> {
    rows.iter().map(|(l, t)| (l.to_string(), *t)).collect()
}

/// 受け入れ条件 2 の形: 狭い画面の各選択肢は、広い画面で採った同じダイアログのラベルと
/// 一致するか、`label_truncated` を立てる（広い画面では立てない）
#[track_caller]
fn assert_matches_wide(name: &str, narrow: &ChoiceList, wide: &ChoiceList) {
    let numbers = |l: &ChoiceList| l.options.iter().map(|o| o.number).collect::<Vec<_>>();
    assert_eq!(numbers(narrow), numbers(wide), "{name}: 番号の並びが違う");
    for (n, w) in narrow.options.iter().zip(&wide.options) {
        assert!(
            !w.label_truncated,
            "{name}: 広い画面の選択肢 {:?} まで申告している: {:?}",
            w.number, w.label
        );
        assert!(
            n.label == w.label || n.label_truncated,
            "{name}: 選択肢 {:?} のラベル {:?} が広い画面の {:?} と食い違うのに \
             label_truncated が立っていない",
            n.number,
            n.label,
            w.label
        );
    }
}

#[test]
fn issue2302_32桁のラベルは60桁と一致するか境目が読めないと申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return; // 子プロセス（A/B の legacy の腕）では既定の腕を観ない
    }
    let wide = detect("60x50", ASK_60X50);
    for (name, screen) in [
        ("32x46", ASK_32X46),
        ("32x70", ASK_32X70),
        ("32x70-long", ASK_32X70_LONG),
    ] {
        let narrow = detect(name, screen);
        assert_matches_wide(name, &narrow, &wide);
        assert_eq!(
            labels(&narrow),
            expect(&[
                // 続き `の判定を先に直す（推奨）` は説明の 1 行目とも取れる = 1 行目だけ + 申告
                ("status", true),
                // 1 行で収まっているが、次の行（説明）が「入らなかった続き」とも取れる
                ("ダイアログの検知を先に直す", true),
                // 次の行が右端まで埋まった区切りの無い長い語 = 説明の始まり（修正前は説明 7 行が混ざった）
                ("両方を 1 つの PR で直す", false),
                // 次の行の `今の` は前の行に入る = 段落の切れ目（説明つきの並びの証拠）
                ("今回は調査だけにする", false),
                ("Type something.", false),
                ("Chat about this", false),
            ]),
            "{name}"
        );
    }
}

#[test]
fn issue2302_60桁は修正前と同じに読めて申告もしない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    assert_eq!(
        labels(&detect("60x50", ASK_60X50)),
        expect(&[
            ("status の判定を先に直す（推奨）", false),
            ("ダイアログの検知を先に直す", false),
            // 説明は区切りの無い長い語で始まる（表示幅では入らない = 長い語の規則で説明と読む）
            ("両方を 1 つの PR で直す", false),
            ("今回は調査だけにする", false),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
}

#[test]
fn issue2302_英語のラベルは折り返しを繋ぎ説明を混ぜない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let wide = detect("ascii-80", ASCII_80);
    let narrow = detect("ascii-32", ASCII_32);
    assert_matches_wide("ascii-32", &narrow, &wide);
    assert_eq!(
        labels(&narrow),
        expect(&[
            // `first (Recommended)` の後ろの `Replace` は前の行に入る = そこが説明の始まり
            ("Fix the status detection first (Recommended)", false),
            ("Fix dialogs", false),
            // ラベルが右端まで埋まっていて、次の行が続きか説明か読めない
            ("Rewrite both in a single PR", true),
            ("Investigate only", false),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
    // 25 桁: 右端ちょうどまで埋まった行の次の行は 1 桁狭い（`long footer does not` の次の
    // `hide the error` に `line.` が入らない）。その境目を段落の切れ目と読むと、選択肢 1 に
    // 説明が丸ごと混ざる（実装中に踏んだ）
    let narrow = detect("ascii-25", ASCII_25);
    assert_matches_wide("ascii-25", &narrow, &wide);
    assert_eq!(
        labels(&narrow),
        expect(&[
            ("Fix the status", true),
            ("Fix dialogs", false),
            ("Rewrite both in a single PR", false),
            ("Investigate only", true),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
}

#[test]
fn issue2302_説明の証拠が無い日本語の並びは説明を飲むとき申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let wide = detect("cjk-80", CJK_80);
    let narrow = detect("cjk-32", CJK_32);
    assert_matches_wide("cjk-32", &narrow, &wide);
    // 選択肢 1 は申告する（#2324 で期待を直した）: 次の行の最初の語 `32` は表示幅では 1 行目
    // （31 桁）の後ろに入らない = 段落の切れ目ではなく、ラベルの続きとも説明の 1 行目とも取れる。
    // 旧期待（申告なし）は char 数の見積もりが「入る = 別の要素」と読んだ偶然の一致で、同じ形で
    // ラベルの続きを申告なしで落とす実採取がある（`issue2324_narrow_cjk_label` の `sp` 31〜39 桁）
    assert_eq!(
        (
            narrow.options[0].label.as_str(),
            narrow.options[0].label_truncated
        ),
        ("ダイアログの検知を先に直す", true)
    );
    // 選択肢 2 は申告しない（#2324 で期待を直した。旧期待は「長い語の行を飲むかもしれない」で
    // 申告）: 続きの 2 行目が `）` で始まる = 1 行目 `判定の材料（画面の末尾の窓` は字の間で割った
    // 長い語の途中 = 説明の始まり（32x46 の選択肢 3 と同じ証拠）。ラベルは 80 桁と同じ
    assert_eq!(
        (
            narrow.options[1].label.as_str(),
            narrow.options[1].label_truncated
        ),
        ("両方を 1 つの PR で直す", false)
    );
    // 80 桁: 選択肢 2 の続きは 1 行だけ = 説明（説明つきの並びでは説明が必ずある）
    assert_eq!(
        labels(&wide),
        expect(&[
            ("ダイアログの検知を先に直す", false),
            ("両方を 1 つの PR で直す", false),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
}

#[test]
fn issue2302_説明の無いpermissionは従来どおりラベルを繋いで申告しない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let wide = detect("perm-80", PERM_80);
    let expected = expect(&[
        ("Yes", false),
        ("Yes, and don’t ask again for: cargo test *", false),
        (
            "Yes, and switch to auto mode · auto mode handles these prompts for you",
            false,
        ),
        ("No", false),
    ]);
    assert_eq!(labels(&wide), expected);
    for (name, screen) in [("perm-32", PERM_32), ("perm-25", PERM_25)] {
        // 右端に余白があり、段落の切れ目も無い = #1131 の結合のまま（ラベル一致も効く）
        assert_eq!(labels(&detect(name, screen)), expected, "{name}");
    }
}

#[test]
fn issue2302_説明の改行は1行に流れるので1段落として読む() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    assert_eq!(
        labels(&detect("nl-32", NL_32)),
        expect(&[
            // 右端まで埋まったラベルの次の行（`Pros: one change.�Cons: the`）は続きとも取れる
            ("Rewrite both in a single PR", true),
            ("Fix dialogs", false),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
}

#[test]
fn issue2302_結合の上限を超えるラベルは申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 合成: 選択肢 1 のラベルが 10 行に割れ（どの行も次の語が入らない = 折り返し）、
    // `zz` の後ろの `Qq` は前の行に入る = そこから説明。結合は上限（8 行）までしか
    // 繋がらないので、ラベルは途中まで = 申告が要る
    let mut lines: Vec<String> = vec!["❯ 1. wwww wwwwww".into()];
    lines.extend((0..8).map(|_| "     wwwww wwwww".to_string()));
    lines.push("     zz".into());
    // 説明の行もラベルと同じ右端（16 桁）に収める（広い行があると右端の実測が広がり、
    // `zz` も 8 行目の後ろに入る = 境目が 8 行目に来てしまう）
    lines.push("     Qq is the".into());
    lines.push("     desc here".into());
    lines.push("  2. Short".into());
    lines.push("     Desc of two".into());
    lines.push(String::new());
    lines.push("Enter to select · ↑/↓ to navigate · Esc to cancel".into());
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let list = detect_choice_list(&refs).expect("検知できる");
    assert!(list.options[0].label_truncated, "{:?}", list.options[0]);
    assert!(
        !list.options[0].label.contains("Qq"),
        "説明が混ざった: {:?}",
        list.options[0].label
    );
    assert_eq!(
        (
            list.options[1].label.as_str(),
            list.options[1].label_truncated
        ),
        ("Short", false)
    );
}

/// 同一バイナリの A/B: `TAKO_2302_LEGACY=1` で **#2302 の崩れ（続きが落ちる / 説明が混ざる /
/// 申告なし）が戻る**。env は `OnceLock` でプロセス起動時に 1 度だけ読むので、legacy の腕は
/// 自分自身を子プロセスで起動して観る（`issue1847_legacy` と同じ作法）
#[test]
fn issue2302_legacyで境目の崩れが戻る() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        let list = detect("32x46", ASK_32X46);
        assert_eq!(list.options[0].label, "status", "続きが落ちる");
        assert!(
            list.options[2].label.contains("判定の材料"),
            "説明が混ざる: {:?}",
            list.options[2].label
        );
        assert!(
            list.options.iter().all(|o| !o.label_truncated),
            "申告が立たない: {:?}",
            labels(&list)
        );
        eprintln!("[2302-ab] legacy: 32x46 -> status / 説明入りの選択肢 3 / 申告なし");
        return;
    }
    let exe = std::env::current_exe().expect("テストバイナリの場所");
    let out = Command::new(exe)
        .args([LEGACY_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(LEGACY_ENV, "1")
        .output()
        .expect("子プロセスを起動できる");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "legacy の腕が落ちた:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("[2302-ab] legacy:") && stdout.contains("1 passed"),
        "legacy の腕が実際に走っていない（フィルタ外れ）:\n{stdout}\n{stderr}"
    );
}
