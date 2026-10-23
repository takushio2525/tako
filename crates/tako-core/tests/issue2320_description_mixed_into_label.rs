//! 狭いペインの AskUserQuestion で、段落の切れ目が並びのどこにも出ないとき説明がラベルへ
//! 混ざる（Issue #2320 = #2302 の残り）
//!
//! # 症状（実採取の fixture で再現した）
//!
//! #2302 の `dialog::label_ends` は、選択肢の下に「前の行に入る語が次の行へ回った境目」
//! （段落の切れ目）が 1 つでもある並びを説明つきと読む。**どの選択肢のラベルも右端近くまで
//! 届き、説明の 1 行目の最初の語が入らない**と切れ目が並びのどこにも出ないので、#1131 の結合
//! （char 数の見積もり）のままになり、説明が丸ごとラベルへ混ざって `label_truncated=false`
//! だった。respond のラベル指定は部分一致なので、説明の語（`error line`）で選択肢を確定した。
//!
//! # 判定
//!
//! 字下げ・右端・語の区切りは、説明の 1 行目とラベルの折り返しで同じ（どちらも同じ桁から
//! 同じ右端で語単位に折り返す）。説明は色（256 色の 246）で描き分けられているが、検知器が
//! 受け取る画面テキスト（GUI のグリッド・`capture-pane -p`）には残らない。そこで並びの側の
//! 手がかりを使う: AskUserQuestion は claude が自由入力の選択肢 `Type something.` を必ず足し、
//! 選択肢は説明を必ず持つ（schema の `description` は required）。この並びで従来の結合が
//! 続きの行を飲むなら、飲んだ行は説明かもしれない = **ラベルの 1 行目だけ + `label_truncated`**。
//! 続きが 1 行だけならそれが説明（#2302 の規則 2 と同じ）。A/B は `TAKO_2320_LEGACY=1`。
//!
//! # fixture（すべて実採取・個人情報なし）
//!
//! claude 2.1.301 を一時の設定 dir + 偽の Messages API（AskUserQuestion の tool_use を返す）で
//! 隔離 tmux に立て、幅だけ変えて `capture-pane -p` で採った画面。ヘッダのパスは `/…/work`。
//!
//! - `full-*`: 英語の 3 択。ラベルはどれも 27 字で、32 桁では右端ちょうどまで埋まる
//!   （修正前は 32〜37 桁の 6 幅すべてで 3 つとも説明が混ざった）。37 桁はラベルの行に余白が
//!   残るが、説明の最初の語が入らない形
//! - `mix-*`: 日本語と英語の混ざった 3 択（修正前は 33〜35 桁で選択肢 1 に説明が混ざった。
//!   選択肢 2・3 は char 数の見積もりが結合しないので正しく読めていた。#2324 からは、表示幅では
//!   次の行が入らずラベルの続きとも取れるので境目が読めないとして申告する）
//! - `one-*`: `full` と同じラベルで説明が 1 行に収まる 3 択（修正前は 32〜36 桁で 3 つとも
//!   説明が混ざった。続きが 1 行だけならそれが説明 = 申告しない）

use std::process::Command;

use tako_core::dialog::{detect_choice_list, ChoiceList};

const FULL_32: &str = include_str!("fixtures/issue2320-full-32x90.txt");
const FULL_37: &str = include_str!("fixtures/issue2320-full-37x90.txt");
const FULL_60: &str = include_str!("fixtures/issue2320-full-60x60.txt");
const MIX_33: &str = include_str!("fixtures/issue2320-mix-33x90.txt");
const MIX_60: &str = include_str!("fixtures/issue2320-mix-60x60.txt");
const ONE_32: &str = include_str!("fixtures/issue2320-one-32x70.txt");
const ONE_60: &str = include_str!("fixtures/issue2320-one-60x60.txt");

const LEGACY_ENV: &str = "TAKO_2320_LEGACY";
const LEGACY_TEST: &str = "issue2320_legacyで説明の混入が戻る";

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

/// 受け入れ条件の形: 狭い画面の各選択肢は、60 桁で採った同じダイアログのラベルと一致するか、
/// `label_truncated` を立てる（60 桁では立てない）
#[track_caller]
fn assert_matches_wide(name: &str, narrow: &ChoiceList, wide: &ChoiceList) {
    let numbers = |l: &ChoiceList| l.options.iter().map(|o| o.number).collect::<Vec<_>>();
    assert_eq!(numbers(narrow), numbers(wide), "{name}: 番号の並びが違う");
    for (n, w) in narrow.options.iter().zip(&wide.options) {
        assert!(
            !w.label_truncated,
            "{name}: 60 桁の選択肢 {:?} まで申告している: {:?}",
            w.number, w.label
        );
        assert!(
            n.label == w.label || n.label_truncated,
            "{name}: 選択肢 {:?} のラベル {:?} が 60 桁の {:?} と食い違うのに \
             label_truncated が立っていない",
            n.number,
            n.label,
            w.label
        );
    }
}

/// 説明の文言がどの選択肢のラベルにも入っていない（respond のラベル指定 = 部分一致が
/// 説明の語で選択肢を掴まない前提）
#[track_caller]
fn assert_no_description_words(name: &str, list: &ChoiceList, words: &[&str]) {
    for o in &list.options {
        for w in words {
            assert!(
                !o.label.contains(w),
                "{name}: 選択肢 {:?} のラベルに説明の語 {w:?} が混ざった: {:?}",
                o.number,
                o.label
            );
        }
    }
}

const FULL_DESCRIPTION_WORDS: &[&str] = &["Shared inputs", "error line", "already fixed"];

#[test]
fn issue2320_60桁は説明を混ぜず申告もしない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return; // 子プロセス（A/B の legacy の腕）では既定の腕を観ない
    }
    assert_eq!(
        labels(&detect("full-60", FULL_60)),
        expect(&[
            ("Rewrite both in a single PR", false),
            ("Fix the status anchor first", false),
            ("Investigate the main branch", false),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
    assert_eq!(
        labels(&detect("mix-60", MIX_60)),
        expect(&[
            ("worker の status を先に直す", false),
            ("respond の一致を先頭から見る", false),
            ("Issue を閉じて調査だけにする", false),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
}

#[test]
fn issue2320_切れ目の無い並びでも説明をラベルへ混ぜず申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let wide = detect("full-60", FULL_60);
    for (name, screen) in [("full-32", FULL_32), ("full-37", FULL_37)] {
        let narrow = detect(name, screen);
        assert_matches_wide(name, &narrow, &wide);
        assert_no_description_words(name, &narrow, FULL_DESCRIPTION_WORDS);
        assert_eq!(
            labels(&narrow),
            expect(&[
                // ラベルの次の行は「入らなかったラベルの続き」とも「説明の 1 行目」とも取れる
                // = 1 行目だけ + 申告（修正前は説明が丸ごと混ざって申告なし）
                ("Rewrite both in a single PR", true),
                ("Fix the status anchor first", true),
                ("Investigate the main branch", true),
                ("Type something.", false),
                ("Chat about this", false),
            ]),
            "{name}"
        );
    }
}

#[test]
fn issue2320_日本語混じりの並びは説明を飲む選択肢だけ申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let wide = detect("mix-60", MIX_60);
    let narrow = detect("mix-33", MIX_33);
    assert_matches_wide("mix-33", &narrow, &wide);
    assert_no_description_words("mix-33", &narrow, &["supervisor", "PR は小さく", "main で"]);
    assert_eq!(
        labels(&narrow),
        expect(&[
            // 従来の結合が説明 3 行を飲んでいた選択肢
            ("worker の status を先に直す", true),
            // 従来の結合が続きを飲まない（char 数の見積もりで「別の要素」）選択肢も申告する
            // （#2324 で期待を直した）: どちらも 1 行目が右端まで埋まり、次の行の最初の語
            // （`PR` / `main`）が表示幅では入らない = ラベルの続きとも説明の 1 行目とも取れる。
            // 旧期待（申告なし）は見積もりの偶然の一致で、同じ形でラベルの続きを申告なしで
            // 落とす実採取がある（`issue2324_narrow_cjk_label` の `sp` 31〜39 桁）
            ("respond の一致を先頭から見る", true),
            ("Issue を閉じて調査だけにする", true),
            ("Type something.", false),
            ("Chat about this", false),
        ])
    );
}

#[test]
fn issue2320_続きが1行ならそれが説明なので申告しない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let wide = detect("one-60", ONE_60);
    let narrow = detect("one-32", ONE_32);
    assert_matches_wide("one-32", &narrow, &wide);
    assert_no_description_words("one-32", &narrow, &["Shared inputs", "error line", "Close"]);
    // 説明は必ずある（schema の required）ので、1 行だけの続きはラベルの続きではありえない =
    // 境目が読める（ラベル指定の respond も効く）
    assert_eq!(labels(&narrow), labels(&wide));
}

#[test]
fn issue2320_自由入力の選択肢が無い並びは従来の結合のまま() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 合成: 32 桁の実採取から `Type something.` を外した並び。説明を持つと分からない並び
    // （permission など）の扱いは変えない = 従来（#1131）の結合のまま、申告もしない。
    // 規則が自由入力の選択肢だけを手がかりにしていることの固定（範囲外の並びへ波及しない）
    let screen = FULL_32.replace("4. Type something.", "4. Something else");
    let list = detect("full-32-no-free-text", &screen);
    assert!(
        list.options.iter().all(|o| !o.label_truncated),
        "{:?}",
        labels(&list)
    );
    assert_eq!(
        list.options[0].label,
        "Rewrite both in a single PR Shared inputs mean one change, but the diff gets larger."
    );
}

/// 同一バイナリの A/B: `TAKO_2320_LEGACY=1` で **#2320 の混入（説明が丸ごとラベルへ・申告なし）が
/// 戻る**。env は `OnceLock` でプロセス起動時に 1 度だけ読むので、legacy の腕は自分自身を
/// 子プロセスで起動して観る（`issue2302_legacy` と同じ作法）
#[test]
fn issue2320_legacyで説明の混入が戻る() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        let list = detect("full-32", FULL_32);
        assert!(
            list.options[1].label.contains("error line"),
            "説明が混ざる: {:?}",
            list.options[1].label
        );
        assert!(
            list.options.iter().all(|o| !o.label_truncated),
            "申告が立たない: {:?}",
            labels(&list)
        );
        eprintln!("[2320-ab] legacy: full-32 -> 3 つとも説明入り / 申告なし");
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
        stderr.contains("[2320-ab] legacy:") && stdout.contains("1 passed"),
        "legacy の腕が実際に走っていない（フィルタ外れ）:\n{stdout}\n{stderr}"
    );
}
