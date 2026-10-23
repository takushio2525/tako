//! 狭いペインの選択肢ダイアログで、全角の語がちょうど行を埋めたラベルの続きを落とし（形 1）、
//! 全角の結合に空白を挟んでいた（形 2）のに `label_truncated=false` だった（Issue #2324 =
//! #2320 の調査の残り）
//!
//! # 症状（実採取の fixture で再現した）
//!
//! - 形 1: `dialog::long_word_start`（#2302）は「続きの 1 行目が右端まで埋まり区切りが無い」を
//!   行幅を超える語の途中と読んで説明の始まりにする。全角の語が**ちょうど**行を埋めただけの
//!   ラベルの続きもこの形になる（27 桁の `3. Issue` + `を閉じて調査だけにする`（22 桁 = 行幅
//!   ちょうど）+ 説明）。ラベルは `Issue` だけ・申告なし
//! - 形 2: 副画面の無い画面の結合は #1131 のまま空白を挟む。全角の長いラベルが字の間で割れると
//!   ラベルの途中に空白が入る（28 桁の `ダイアログの検知を先に` + `直す` →
//!   `ダイアログの検知を先に 直す`）。申告なし
//!
//! どちらも本当のラベルの先頭部分か空白違いなので別の選択肢を確定することは無いが、status /
//! read_pane のラベルが 60 桁と違い、本来のラベルでの respond が「一致しない」になる。
//!
//! # 手がかりの実測と判定
//!
//! claude は空白で折り返すと空白を消し、空白の無い語は行幅を超えるときだけ字の間で割る
//! （行幅に収まる語は次の行へ丸ごと送る）。そこで「前の行が右端まで埋まっているか」を手がかりに
//! できるかを全採取で確かめた。全角どうしの繋ぎ目で前の行が埋まっていた 40 か所のうち 38 か所は
//! 語の途中の割れ（空白なし）だったが、探りの `sp`（全角どうしの間に空白のあるラベル）の
//! 27・28 桁の 2 か所は空白で折り返した形で、**字面は同じ**（前の行が埋まり、全角どうしで繋がる）。
//! 埋まり方だけでは決まらないので、次の証拠があるときだけ決め、無ければ推測で繋がず申告する:
//!
//! - 繋ぎ目（`dialog::seam`）: 前の行に次の行の 1 文字目がまだ入る / 繋ぎ目をまたぐ語が行幅に
//!   収まる → 空白で折り返した。次の行が閉じ括弧・句読点で始まる → 語の途中。それ以外は決まらない =
//!   その手前までをラベルにして `label_truncated`
//! - 長い語の規則（`dialog::long_word_evidence`）: 2 行目が閉じ括弧・句読点で始まる（字の間で
//!   割った長い語の途中）か、1 行目の語がラベルに入る語の幅（40 桁 = 全角 20 字。schema の
//!   「concise (1-5 words)」）を超えるときだけ説明の始まりと読む。それ以外は申告
//! - 説明を必ず持つ並び（#2320）で char 数の見積もりが続きを「別の要素」と読んでも、表示幅では
//!   段落の切れ目が無い = 申告（探りの `sp` の 31〜39 桁で、ラベルの続きを申告なしで落としていた）
//!
//! A/B は `TAKO_2324_LEGACY=1`。
//!
//! # fixture（すべて実採取・個人情報なし）
//!
//! claude 2.1.301 を一時の設定 dir + 偽の Messages API（AskUserQuestion の tool_use を返す）で
//! 隔離 tmux に立て、同じダイアログを 24〜44 桁（1 桁刻み）・50・60・80 桁へ幅だけ変えて
//! `capture-pane -p` で採った画面（高さ 150 行）。種類ごとに 1 ファイルで `=== <幅>` の節に分けた。
//! 末尾の空行を落とし、ヘッダのパスは `/…/work` に置き換えた（24 桁で折れたパスの 2 行目も消した）。
//!
//! - `full` / `one` / `ascii`: 英語（#2320 / #2302 と同じ質問。半角だけのラベルは従来どおり空白を戻す）
//! - `mix`: 日本語と英語の混ざった 3 択（#2320 と同じ質問。形 1・形 2）
//! - `cjk`: 日本語の 2 択（#2302 と同じ質問。形 2）
//! - `huge`: 長い説明の 4 択（#1847 と同じ質問。形 1・形 2）
//! - `sp`: 探り。全角どうしの間に空白のあるラベル（形 2 と字面が同じで空白がある形）・半角と全角が
//!   空白なしで続くラベル・短い全角の語を空白で区切ったラベル
//! - `sp2`: 探り。全角の語を空白で 3 つに区切ったラベル・半角と全角が交互に続くラベル・区切りの
//!   無い長いラベル

use std::process::Command;

use tako_core::dialog::{detect_choice_list, ChoiceList};

const KINDS: &[(&str, &str)] = &[
    ("full", include_str!("fixtures/issue2324-full.txt")),
    ("one", include_str!("fixtures/issue2324-one.txt")),
    ("ascii", include_str!("fixtures/issue2324-ascii.txt")),
    ("mix", include_str!("fixtures/issue2324-mix.txt")),
    ("cjk", include_str!("fixtures/issue2324-cjk.txt")),
    ("huge", include_str!("fixtures/issue2324-huge.txt")),
    ("sp", include_str!("fixtures/issue2324-sp.txt")),
    ("sp2", include_str!("fixtures/issue2324-sp2.txt")),
];

/// 選択肢として検知できないことが分かっている画面（この Issue とは別の原因）。
/// 24 桁では確定キーの案内が 3 行に折れて最後の `cancel` が案内として読めず、カーソルが走査の窓の
/// 外にある背の高いダイアログの経路 5（#1847）が「ダイアログの外の行がある」と判断する
const KNOWN_UNDETECTED: &[(&str, u32)] = &[("huge", 24)];

const LEGACY_ENV: &str = "TAKO_2324_LEGACY";
const LEGACY_TEST: &str = "issue2324_legacyで形1と形2が戻る";

/// fixture の `=== <幅>` の節を `(幅, 画面)` に分ける
fn sections(text: &str) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for line in text.lines() {
        if let Some(w) = line.strip_prefix("=== ") {
            out.push((w.parse().expect("幅"), String::new()));
        } else if let Some((_, screen)) = out.last_mut() {
            screen.push_str(line);
            screen.push('\n');
        }
    }
    out
}

fn screen(kind: &str, width: u32) -> String {
    let text = KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, t)| *t)
        .unwrap_or_else(|| panic!("{kind}: fixture が無い"));
    sections(text)
        .into_iter()
        .find(|(w, _)| *w == width)
        .map(|(_, s)| s)
        .unwrap_or_else(|| panic!("{kind}-{width}: 節が無い"))
}

fn detect(screen: &str) -> Option<ChoiceList> {
    let rows: Vec<&str> = screen.lines().collect();
    detect_choice_list(&rows)
}

#[track_caller]
fn detect_at(kind: &str, width: u32) -> ChoiceList {
    detect(&screen(kind, width))
        .unwrap_or_else(|| panic!("{kind}-{width}: 選択肢ダイアログを検知できない"))
}

/// `(ラベル, label_truncated)`
#[track_caller]
fn option(kind: &str, width: u32, number: u32) -> (String, bool) {
    let list = detect_at(kind, width);
    let o = list
        .options
        .iter()
        .find(|o| o.number == Some(number))
        .unwrap_or_else(|| panic!("{kind}-{width}: 選択肢 {number} が無い"));
    (o.label.clone(), o.label_truncated)
}

#[test]
fn issue2324_全採取で60桁と一致するか申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return; // 子プロセス（A/B の legacy の腕）では既定の腕を観ない
    }
    let mut violations: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (kind, text) in KINDS {
        let wide = detect_at(kind, 60);
        for (width, screen) in sections(text) {
            let Some(narrow) = detect(&screen) else {
                if !KNOWN_UNDETECTED.contains(&(kind, width)) {
                    violations.push(format!("{kind}-{width}: 検知できない"));
                }
                continue;
            };
            let numbers = |l: &ChoiceList| l.options.iter().map(|o| o.number).collect::<Vec<_>>();
            if numbers(&narrow) != numbers(&wide) {
                violations.push(format!("{kind}-{width}: 番号の並びが 60 桁と違う"));
                continue;
            }
            for (n, w) in narrow.options.iter().zip(&wide.options) {
                checked += 1;
                if w.label_truncated {
                    violations.push(format!(
                        "{kind}-60: 選択肢 {:?} まで申告している: {:?}",
                        w.number, w.label
                    ));
                }
                if n.label != w.label && !n.label_truncated {
                    violations.push(format!(
                        "{kind}-{width}: 選択肢 {:?} のラベル {:?} が 60 桁の {:?} と食い違うのに申告なし",
                        n.number, n.label, w.label
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "{} 件:\n{}",
        violations.len(),
        violations.join("\n")
    );
    // 8 種 × 24 幅（検知できない 1 画面を除く）の選択肢をすべて見た
    assert_eq!(checked, 978, "見た選択肢の数が採取と合わない");
}

#[test]
fn issue2324_形1_行幅ちょうどの全角の語は説明の始まりと言い切らない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 続きの 1 行目が行幅ちょうどの全角の語で、2 行目が説明。長い語の形だが、ラベルの続きでも
    // ありうる（語の途中で割った証拠が無い）= 1 行目だけ + 申告（修正前は申告なし）
    for (kind, width, number, first_line) in [
        ("mix", 27, 3, "Issue"),
        ("mix", 28, 3, "Issue"),
        ("mix", 25, 2, "respond"),
        ("mix", 26, 2, "respond"),
        ("huge", 29, 1, "status"),
        ("huge", 30, 1, "status"),
    ] {
        assert_eq!(
            option(kind, width, number),
            (first_line.to_string(), true),
            "{kind}-{width}"
        );
    }
    // 60 桁は本当のラベル・申告なし
    assert_eq!(
        option("mix", 60, 3),
        ("Issue を閉じて調査だけにする".to_string(), false)
    );
    assert_eq!(
        option("huge", 60, 1),
        ("status の判定を先に直す（推奨）".to_string(), false)
    );
}

#[test]
fn issue2324_形1_割れた語の続きか幅の証拠があれば説明の始まりと読む() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 2 行目が閉じ括弧で始まる = 1 行目は字の間で割った長い語の途中（31 桁:
    // `判定の材料（画面の末尾の窓` + `）が共通なので…`）。修正前も同じ答え（申告なし）
    assert_eq!(
        option("cjk", 31, 2),
        ("両方を 1 つの PR で直す".to_string(), false)
    );
    // 1 行目の語がラベルに入る語の幅（40 桁）を超える = 50 / 60 / 80 桁の説明の始まり
    for width in [50, 60, 80] {
        assert_eq!(
            option("huge", width, 3),
            ("両方を 1 つの PR で直す".to_string(), false),
            "huge-{width}"
        );
    }
}

#[test]
fn issue2324_形2_全角の繋ぎ目は証拠が無ければ繋がずに申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 28 桁の 2 つの選択肢は字面が同じ形（1 行目が右端まで埋まり、全角どうしで繋がる）なのに、
    // 本当のラベルは片方が空白なし・片方が空白あり。画面からは決まらない = 繋がずに申告
    let cjk = screen("cjk", 28);
    let sp = screen("sp", 28);
    assert!(
        cjk.contains("❯ 1. ダイアログの検知を先に\n     直す\n"),
        "{cjk}"
    );
    assert!(
        sp.contains("❯ 1. はい、ぜんぶ進めますよ\n     つづきも見る\n"),
        "{sp}"
    );
    assert_eq!(
        option("cjk", 28, 1),
        ("ダイアログの検知を先に".to_string(), true)
    );
    assert_eq!(
        option("sp", 28, 1),
        ("はい、ぜんぶ進めますよ".to_string(), true)
    );
    assert_eq!(
        option("cjk", 60, 1),
        ("ダイアログの検知を先に直す".to_string(), false)
    );
    assert_eq!(
        option("sp", 60, 1),
        ("はい、ぜんぶ進めますよ つづきも見る".to_string(), false)
    );
    // ラベルの途中に空白を挟んだ形はどの幅にも残らない（修正前は cjk の 24〜30 桁すべて）
    for width in 24..=30 {
        let (label, truncated) = option("cjk", width, 1);
        assert!(
            !label.contains(' ') && truncated,
            "cjk-{width}: {label:?} / {truncated}"
        );
    }
}

#[test]
fn issue2324_半角だけのラベルは従来どおり空白を戻す() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 英語の折り返しは「繋ぎ目をまたぐ語が行幅に収まる」で空白と決まる。修正前と同じラベル・
    // 同じ申告（#2302 / #2320 の境目の申告だけ）で、#2324 の申告は 1 つも増えない
    for (kind, width, number, label) in [
        (
            "ascii",
            32,
            1,
            "Fix the status detection first (Recommended)",
        ),
        ("ascii", 25, 3, "Rewrite both in a single PR"),
        ("one", 27, 2, "Fix the status anchor first"),
        ("one", 30, 1, "Rewrite both in a single PR"),
    ] {
        assert_eq!(
            option(kind, width, number),
            (label.to_string(), false),
            "{kind}-{width}"
        );
    }
}

#[test]
fn issue2324_前の行にまだ入る繋ぎ目は空白で繋ぎ申告しない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 半角と全角の境目: 前の行に次の行の 1 文字目がまだ入る = 字で割るなら入れていた =
    // 空白で折り返した（`seam` の規則 1）。60 桁と同じラベル・申告なし
    for (kind, width, number, label) in [
        ("mix", 28, 2, "respond の一致を先頭から見る"),
        ("mix", 32, 3, "Issue を閉じて調査だけにする"),
        ("mix", 26, 1, "worker の status を先に直す"),
        ("huge", 34, 1, "status の判定を先に直す（推奨）"),
    ] {
        assert_eq!(
            option(kind, width, number),
            (label.to_string(), false),
            "{kind}-{width}"
        );
    }
}

#[test]
fn issue2324_説明を持つ並びで見積もりが落とした続きも申告する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    // 31〜39 桁: `はい、ぜんぶ進めますよ` の後ろの `つづきも見る` は表示幅では前の行に入らない
    // （段落の切れ目ではない）のに、char 数の見積もりは「入る = 別の要素」と読んでいた
    for width in 31..=39 {
        assert_eq!(
            option("sp", width, 1),
            ("はい、ぜんぶ進めますよ".to_string(), true),
            "sp-{width}"
        );
    }
    assert_eq!(
        option("sp", 40, 1),
        ("はい、ぜんぶ進めますよ つづきも見る".to_string(), false)
    );
}

/// 同一バイナリの A/B: `TAKO_2324_LEGACY=1` で **形 1（続きを落とす）と形 2（空白を挟む）が
/// 申告なしで戻る**。env は `OnceLock` でプロセス起動時に 1 度だけ読むので、legacy の腕は
/// 自分自身を子プロセスで起動して観る（`issue2320_legacy` と同じ作法）
#[test]
fn issue2324_legacyで形1と形2が戻る() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        assert_eq!(option("mix", 27, 3), ("Issue".to_string(), false), "形 1");
        assert_eq!(
            option("cjk", 28, 1),
            ("ダイアログの検知を先に 直す".to_string(), false),
            "形 2"
        );
        eprintln!("[2324-ab] legacy: mix-27 -> Issue / cjk-28 -> 空白入り / 申告なし");
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
        stderr.contains("[2324-ab] legacy:") && stdout.contains("1 passed"),
        "legacy の腕が実際に走っていない（フィルタ外れ）:\n{stdout}\n{stderr}"
    );
}
