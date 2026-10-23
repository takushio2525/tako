//! 24 桁のペインで背の高い AskUserQuestion を検知できなかった（Issue #2343 = #2324 の申し送り）
//!
//! # 症状（実採取の fixture で再現した）
//!
//! `fixtures/issue2324-huge.txt` の 24 桁の節（claude 2.1.301・6 択・選択カーソルが下端から
//! 40 行より上）で `choice_dialog` が null。25 桁以上は検知できていた。
//!
//! # 真因
//!
//! 背の高い並びを拾う経路 5（#1847 の `numbered_above_scan`）は、並びの最後から画面の下端までが
//! ダイアログの一部だけのときに採る。確定キーの案内は 1 行ずつ `is_key_hint` で読んでいたので、
//! 24 桁で 3 行に折れた案内のうち読めるのは 1 行目だけだった:
//!
//! ```text
//! Enter to select · ↑/↓ to     ← 案内（`Enter to` で始まる）
//! navigate · Esc to            ← ダイアログの外の行（修正前）
//! cancel                       ← ダイアログの外の行（修正前）
//! ```
//!
//! 全 fixture の AskUserQuestion の案内の折れ方は 7 通り（1 行 / 2 行 × 5 / 3 行）で、行単位で
//! 読めないのは「前の行が `to` で終わり、次の行が動詞から始まる」形だけ（`… Esc to` / `cancel` は
//! 42〜44 桁でも出る。経路 1 は並びの後を見ないので、背の高い並びの経路 5 と入力欄の上の経路 4
//! だけが落ちていた）。permission の `Esc to cancel · Tab to` / `amend`（#2302 の 25 桁）も同じ形。
//!
//! # 修正
//!
//! `dialog::WrappedHint`: 案内は `<キー> to <動詞>` を ` · ` で並べた 1 文なので、繋いだ文が
//! その形のままで、まだ文が終わっていないあいだだけ、直下の同じ桁の行を案内の続きとして読む。
//! ラベルは #2324 の規則（`seam` / `label_ends`）のまま決め、読めないものは `label_truncated`。
//! A/B は `TAKO_2343_LEGACY=1`。

use std::path::{Path, PathBuf};
use std::process::Command;

use tako_core::dialog::{detect_choice_list, is_choice_dialog, ChoiceList};

const HUGE: &str = include_str!("fixtures/issue2324-huge.txt");

const LEGACY_ENV: &str = "TAKO_2343_LEGACY";
const LEGACY_TEST: &str = "issue2343_legacyで24桁の不検知が戻り他の画面は変わらない";

/// 24 桁で 3 行に折れた案内（fixture の末尾 3 行）
const WRAPPED_24: &str = "Enter to select · ↑/↓ to\nnavigate · Esc to\ncancel";

/// fixture の `=== <幅>` の節を `(幅, 画面)` に分ける（節が無ければ全体を 1 画面）
fn sections(text: &str) -> Vec<(String, String)> {
    if !text.lines().any(|l| l.starts_with("=== ")) {
        return vec![(String::new(), text.to_string())];
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(w) = line.strip_prefix("=== ") {
            out.push((w.to_string(), String::new()));
        } else if let Some((_, screen)) = out.last_mut() {
            screen.push_str(line);
            screen.push('\n');
        }
    }
    out
}

fn huge(width: &str) -> String {
    sections(HUGE)
        .into_iter()
        .find(|(w, _)| w == width)
        .map(|(_, s)| s)
        .unwrap_or_else(|| panic!("huge-{width}: 節が無い"))
}

fn detect(screen: &str) -> Option<ChoiceList> {
    let rows: Vec<&str> = screen.lines().collect();
    detect_choice_list(&rows)
}

/// 画面の最下部（確定キーの案内）を差し替える
fn replace_tail(screen: &str, tail: &str) -> String {
    let body: Vec<&str> = screen.lines().collect();
    let cut = body
        .iter()
        .rposition(|l| l.starts_with("Enter to select"))
        .expect("案内の行がある");
    format!("{}\n{tail}\n", body[..cut].join("\n"))
}

fn numbers(list: &ChoiceList) -> Vec<Option<u32>> {
    list.options.iter().map(|o| o.number).collect()
}

#[test]
fn issue2343_24桁の背の高いダイアログを検知する() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return; // 子プロセス（A/B の legacy の腕）では既定の腕を観ない
    }
    let screen = huge("24");
    assert!(screen.ends_with(&format!("{WRAPPED_24}\n")), "{screen}");
    let narrow = detect(&screen).expect("huge-24: 選択肢ダイアログを検知できない");
    let wide = detect(&huge("60")).expect("huge-60");
    assert!(narrow.numbered, "番号つき");
    assert_eq!(numbers(&narrow), (1..=6).map(Some).collect::<Vec<_>>());
    assert_eq!(numbers(&narrow), numbers(&wide), "60 桁と同じ番号の並び");
    assert_eq!(narrow.highlighted, Some(0), "ハイライトは選択肢 1");
    assert!(
        narrow.cursor_row.is_some(),
        "経路 5 = カーソルは画面に描かれている"
    );
    assert!(is_choice_dialog(&screen.lines().collect::<Vec<_>>()));
    // ラベルは 60 桁と一致するか申告する（#2324 の規則のまま）
    for (n, w) in narrow.options.iter().zip(&wide.options) {
        assert!(
            n.label == w.label || n.label_truncated,
            "選択肢 {:?}: {:?} が 60 桁の {:?} と食い違うのに申告なし",
            n.number,
            n.label,
            w.label
        );
        assert!(
            w.label.starts_with(&n.label),
            "選択肢 {:?}: 申告したラベル {:?} が本当のラベル {:?} の接頭辞でない",
            n.number,
            n.label,
            w.label
        );
    }
    // 自由入力と相談の選択肢はラベルで指せる
    assert_eq!(
        narrow.options[4..]
            .iter()
            .map(|o| (o.label.as_str(), o.label_truncated))
            .collect::<Vec<_>>(),
        vec![("Type something.", false), ("Chat about this", false)]
    );
}

#[test]
fn issue2343_実採取の案内の折れ方はどれも読める() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    for tail in [
        // 全 fixture に出る AskUserQuestion の案内の折れ方 7 通り（1 行 / 2 行 × 5 / 3 行）
        "Enter to select · ↑/↓ to navigate · Esc to cancel",
        "Enter to select · ↑/↓ to\nnavigate · Esc to cancel",
        "Enter to select · ↑/↓ to navigate ·\nEsc to cancel",
        "Enter to select · ↑/↓ to navigate\n· Esc to cancel",
        "Enter to select · ↑/↓ to navigate · Esc\nto cancel",
        "Enter to select · ↑/↓ to navigate · Esc to\ncancel",
        WRAPPED_24,
        // 実採取の別の案内の折れ方（preview つき = #1447 の 60 桁 / permission = #2302 の 25 桁）
        "Enter to select · ↑/↓ to navigate · n to add notes · Esc to\ncancel",
        "Esc to cancel · Tab to\namend",
        // 合成: 同じ案内がもっと細かく折れた形
        "Enter to select · ↑/↓ to\nnavigate · n to add notes\n· Esc to cancel",
        "Esc to cancel · Tab to\namend · ctrl+e to\nexplain",
    ] {
        let list = detect(&replace_tail(&huge("24"), tail))
            .unwrap_or_else(|| panic!("{tail:?}: 検知できない"));
        assert_eq!(list.options.len(), 6, "{tail:?}");
        assert_eq!(list.highlighted, Some(0), "{tail:?}");
    }
}

#[test]
fn issue2343_案内に似た語があってもダイアログの外の行は拾わない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        return;
    }
    let screen = huge("24");
    for (name, tail) in [
        // 文が終わった後に本文が続く
        ("本文が続く", format!("{WRAPPED_24}\n⏺ 了解です。続けます")),
        // 文の途中で本文になる（動詞の位置に本文）
        (
            "途中で本文",
            "Enter to select · ↑/↓ to\nnavigate · Esc to\n⏺ 了解です".to_string(),
        ),
        // 動詞の位置に長い英文
        (
            "長い英文",
            "Enter to select · ↑/↓ to\nnavigate · Esc to\ncancel the operation and go back"
                .to_string(),
        ),
        // 続きの行の桁が案内の 1 行目と違う
        (
            "桁違い",
            "Enter to select · ↑/↓ to\n  navigate · Esc to\ncancel".to_string(),
        ),
        // 空行を挟んだ行は折り返しの続きではない
        (
            "空行の後",
            "Enter to select · ↑/↓ to\n\nnavigate · Esc to\ncancel".to_string(),
        ),
        // `·` で折れた案内の下の短い本文（キーの位置に本文）
        (
            "キーの位置に本文",
            "Enter to select · ↑/↓ to navigate ·\n了解です".to_string(),
        ),
    ] {
        assert_eq!(
            detect(&replace_tail(&screen, &tail)),
            None,
            "{name}: 案内の続きでない行を案内と読んだ"
        );
    }
    // ダイアログの無い画面（会話の本文に案内と同じ語が折れて並ぶ）
    let no_dialog = format!(
        "❯ 案内の文言を教えて\n\n⏺ claude の選択肢ダイアログの下には\n  次の案内が出ます:\n\n{WRAPPED_24}\n"
    );
    assert_eq!(detect(&no_dialog), None, "ダイアログの無い画面を拾った");
}

/// 検知の結果を比べるための 1 行（`Debug` の全体 = 選択肢・ハイライト・本文・カーソル行）
fn describe(screen: &str) -> String {
    format!("{:?}", detect(screen))
}

/// fixture の全画面（tako-core / tako-control の `tests/fixtures/*.txt`）と、
/// 誤検知を見るための変種（末尾に会話の本文 / 選択カーソルを消した = 会話ログへ流れた並び）
fn all_screens() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in ["tako-core/tests/fixtures", "tako-control/tests/fixtures"] {
        for entry in std::fs::read_dir(root.join(dir)).expect("fixtures") {
            let path = entry.expect("fixture").path();
            if path.extension().is_some_and(|x| x == "txt") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut out = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .expect("名前")
            .to_string_lossy()
            .to_string();
        let text = std::fs::read_to_string(&file).expect("fixture を読める");
        for (width, screen) in sections(&text) {
            let id = format!("{name}#{width}");
            out.push((
                format!("{id} +text"),
                format!("{screen}⏺ 了解です。続けます\n"),
            ));
            out.push((format!("{id} -cursor"), screen.replace('❯', " ")));
            out.push((format!("{id} orig"), screen));
        }
    }
    out
}

/// 同一バイナリの A/B: `TAKO_2343_LEGACY=1` で **huge-24 の不検知が戻り**、それ以外の画面は
/// 1 つも変わらない（= 修正で変わったのは huge-24 だけ・悪化 0・誤検知 0）。
/// env は `OnceLock` でプロセス起動時に 1 度だけ読むので、legacy の腕は自分自身を子プロセスで
/// 起動して観る（`issue2324_legacy` と同じ作法）
#[test]
fn issue2343_legacyで24桁の不検知が戻り他の画面は変わらない() {
    if std::env::var_os(LEGACY_ENV).is_some() {
        assert_eq!(detect(&huge("24")), None, "legacy で検知できてしまう");
        for (id, screen) in all_screens() {
            println!("[2343-dump] {id}\t{}", describe(&screen));
        }
        eprintln!("[2343-ab] legacy: huge-24 -> None");
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
        stderr.contains("[2343-ab] legacy:") && stdout.contains("1 passed"),
        "legacy の腕が実際に走っていない（フィルタ外れ）:\n{stdout}\n{stderr}"
    );
    // 1 行目は libtest の `test <名前> ... ` と同じ行に続くので、行の途中から印を探す
    let legacy: std::collections::HashMap<&str, &str> = stdout
        .lines()
        .filter_map(|l| l.split_once("[2343-dump] ").map(|(_, rest)| rest))
        .filter_map(|l| l.split_once('\t'))
        .collect();
    let screens = all_screens();
    assert_eq!(
        legacy.len(),
        screens.len(),
        "legacy の腕が全画面を出していない"
    );
    let mut changed: Vec<String> = Vec::new();
    for (id, screen) in &screens {
        let now = describe(screen);
        let before = legacy.get(id.as_str()).expect("同じ画面の legacy の結果");
        if *before != now {
            changed.push(format!("{id}\n  legacy: {before}\n  now:    {now}"));
        }
    }
    assert_eq!(
        changed
            .iter()
            .map(|c| c.lines().next().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["issue2324-huge.txt#24 orig"],
        "修正で変わってよいのは huge-24 だけ:\n{}",
        changed.join("\n")
    );
    // 変わり方は「None → 検知」だけ（悪化ではない）
    assert!(changed[0].contains("legacy: None"), "{}", changed[0]);
}
