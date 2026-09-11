//! **#1420 の番犬**: 走査型の番犬が「本番コードの範囲」を雑に切らない。
//!
//! ## なぜ止めるのか
//!
//! 番犬はテストコードを検査対象から外すためにソースを `#[cfg(test)]` で切っていた。
//! この切り方はファイル中で**最初に現れる `#[cfg(test)]` まで**しか残さないので、
//! テスト用ヘルパの `#[cfg(test)]` が 1 つ途中にあるだけで、それ以降の本番コードが
//! 番犬の視界から丸ごと消える。**しかも番犬は緑のまま**なので、検出力を失ったことに
//! 気づけない（#1403 の作業中に実測。詳細は `common/production_range.rs`）。
//!
//! 実測した被害は #1401 の番犬だけではなかった。4 クレートの `src` を丸ごと走査する
//! `platform_parity` は、`orchestrator/mod.rs` を **1.5%**・`mcp/mod.rs` を **10.5%**
//! しか見ていなかった（どちらもファイル冒頭にテスト用の `#[cfg(test)]` があるため）。
//!
//! ## 何を固定するか
//!
//! 1. [`途中のテスト用ヘルパで走査範囲が消えない`] ほか — 1 実装
//!    （`common/production_range.rs`）の振る舞い。**旧実装なら見失う**ことも同時に測る
//! 2. [`本番コードを飲み込む範囲は下限で落ちる`] — 「黙って縮んだ」を番犬自身が
//!    `file:line` で名指して落とす（#1420 の受け入れ条件 2）
//! 3. [`雑な切り出しが戻っていない`] — `find("#[cfg(test)]")` 型の切り方が
//!    リポジトリへ戻っていない（許可リストは件数まで固定するので、増えても減っても落ちる）
//! 4. [`寄せた番犬が1実装を通り続けている`] — 寄せ先を黙って外した変更を落とす

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 途中にテスト用ヘルパがあり、末尾にテストモジュールがある形（#1403 が踏んだ形）
const MID_HELPER: &str = "\
pub fn alpha() -> u32 {
    1
}

#[cfg(test)]
fn テスト用ヘルパ() -> u32 {
    0
}

pub fn beta() -> u32 {
    2
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        assert_eq!(super::beta(), 2);
    }
}
";

/// 旧実装（最初の `#[cfg(test)]` で切る）の再現。A/B の旧アーム。
/// ここだけは意図的に雑な切り方をするので、[`LEGACY_MARKER`] で番犬の対象から外す
fn legacy_cut(src: &str) -> &str {
    // 目印は切り出しと**同じ行**に置く（走査は行単位。`rustfmt` が動かさない形にするため
    // `match` の後ろではなく `let` の後ろへ書く）
    let cut = src.find("\n#[cfg(test)]"); // #1420-legacy-arm
    match cut {
        Some(i) => &src[..i],
        None => src,
    }
}

#[test]
fn 途中のテスト用ヘルパで走査範囲が消えない() {
    let kept = production_range::scan(MID_HELPER).text;
    assert!(
        kept.contains("pub fn beta"),
        "ヘルパより後ろの本番コードが消えている:\n{kept}"
    );
    assert!(
        !kept.contains("テスト用ヘルパ") && !kept.contains("mod tests"),
        "テスト領域が残っている:\n{kept}"
    );
    // 旧アーム: 同じ入力で「見失う」ことを測る（測れないなら この検査に意味が無い）
    assert!(
        !legacy_cut(MID_HELPER).contains("pub fn beta"),
        "旧実装が見失う形になっていない = A/B が成立していない"
    );
}

#[test]
fn テストモジュールが複数あってもあいだの本番コードが残る() {
    let src = "pub fn a() {}\n\n#[cfg(test)]\nmod t1 {\n    fn x() {}\n}\n\n\
               pub fn b() {}\n\n#[cfg(test)]\nmod t2 {\n    fn y() {}\n}\n\npub fn c() {}\n";
    let kept = production_range::scan(src).text;
    for needle in ["pub fn a()", "pub fn b()", "pub fn c()"] {
        assert!(kept.contains(needle), "{needle} が消えている:\n{kept}");
    }
    for needle in ["mod t1", "mod t2"] {
        assert!(!kept.contains(needle), "{needle} が残っている:\n{kept}");
    }
}

#[test]
fn テストが1つも無いファイルは丸ごと残る() {
    let src = "pub fn only() -> u32 {\n    7\n}\n";
    let scanned = production_range::scan(src);
    assert_eq!(scanned.text, src);
    assert!(scanned.regions.is_empty());
    assert_eq!(scanned.removed_bytes, 0);
    assert!((scanned.coverage() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn 行番号とバイト長が保たれる() {
    let scanned = production_range::scan(MID_HELPER);
    assert_eq!(
        scanned.text.len(),
        MID_HELPER.len(),
        "バイト長が変わっている"
    );
    assert_eq!(
        scanned.text.lines().count(),
        MID_HELPER.lines().count(),
        "行数が変わっている（file:line の名指しがずれる）"
    );
    // 潰した領域の行番号が実際の位置と合っていること
    let helper_line = MID_HELPER
        .lines()
        .position(|l| l.contains("#[cfg(test)]"))
        .expect("目印")
        + 1;
    assert_eq!(scanned.regions.first().map(|(a, _)| *a), Some(helper_line));
}

#[test]
fn 文字列リテラルは原文のまま残る() {
    // `code_view` は位置決めにしか使わない。中身まで潰すと
    // 「この文言を組んでいるか」を見る番犬（#1401 の `tako remote start`）が壊れる
    let src = "pub fn msg() -> &'static str {\n    \"tako remote start\"\n}\n\
               \n#[cfg(test)]\nmod tests {\n    const X: &str = \"消える\";\n}\n";
    let kept = production_range::scan(src).text;
    assert!(
        kept.contains("\"tako remote start\""),
        "本番の文字列が消えた"
    );
    assert!(!kept.contains("消える"), "テストの文字列が残った");
}

#[test]
fn コメントや文字列の中の綴りでは潰さない() {
    let src = "// #[cfg(test)] と書いただけの説明\n\
               pub fn keep() -> &'static str {\n    \"#[cfg(test)]\"\n}\n";
    let scanned = production_range::scan(src);
    assert!(scanned.regions.is_empty(), "説明文で潰している");
    assert_eq!(scanned.text, src);
}

#[test]
fn 宣言はセミコロンで終わる() {
    let src = "#[cfg(test)]\nuse std::fmt::Debug;\n\npub fn after() -> u32 {\n    3\n}\n";
    let kept = production_range::scan(src).text;
    assert!(!kept.contains("use std::fmt::Debug"), "宣言が残っている");
    assert!(
        kept.contains("pub fn after"),
        "宣言の後ろが消えている:\n{kept}"
    );
}

#[test]
fn tests_onlyは裏返しで同じ境界を使う() {
    let tests = production_range::tests_only(MID_HELPER);
    assert!(tests.contains("テスト用ヘルパ") && tests.contains("mod tests"));
    assert!(!tests.contains("pub fn alpha") && !tests.contains("pub fn beta"));
    assert_eq!(tests.len(), MID_HELPER.len(), "行番号がずれる");
    // 本番側とテスト側は重ならない（同じ 1 実装が境界を決めている証拠）。
    // **バイトで**突き合わせる: 多バイト文字を潰すと空白が N 個になり、char ではずれる
    let kept = production_range::scan(MID_HELPER).text;
    for (i, (a, b)) in kept.bytes().zip(tests.bytes()).enumerate() {
        assert!(
            a == b' ' || b == b' ' || a == b'\n',
            "{i} バイト目が両方に残っている（{a:?} / {b:?}）"
        );
    }
}

#[test]
fn 本番コードを飲み込む範囲は下限で落ちる() {
    // 受け入れ条件 2: 走査範囲が想定より縮んだら番犬自身が file:line で落ちる。
    // `#[cfg(test)]` が本番コードの大半を飲み込む形を作って測る。
    // **本番は 1 KB 以上残す**（そうしないと「走査範囲が空」の側で落ちてしまい、
    // 下限の検査そのものを測れない = 検出力を測ったつもりになる）
    let mut src = String::new();
    for i in 0..40 {
        src.push_str(&format!("pub fn kept_{i}() {{ let _ = {i}; }}\n"));
    }
    assert!(src.len() > 1000, "本番側が 1 KB 未満だと空判定の側で落ちる");
    src.push_str("\n#[cfg(test)]\nmod tests {\n");
    for i in 0..400 {
        src.push_str(&format!("    fn filler_{i}() {{ let _ = {i}; }}\n"));
    }
    src.push_str("}\n");
    let scanned = production_range::scan(&src);
    assert!(
        scanned.kept_bytes() > 1000 && scanned.coverage() < production_range::MIN_COVERAGE,
        "作った入力が測りたい形になっていない（本番 {} B / {:.1}%）",
        scanned.kept_bytes(),
        scanned.coverage() * 100.0
    );
    let panicked = std::panic::catch_unwind(|| {
        production_range::production(&src, "偽/ファイル.rs");
    })
    .expect_err("飲み込まれた範囲を見逃している");
    let msg = panicked
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    let head = src
        .lines()
        .position(|l| l.trim() == "#[cfg(test)]")
        .expect("目印")
        + 1;
    assert!(
        msg.contains(&format!("偽/ファイル.rs:{head}")),
        "潰した領域を file:line で名指していない: {msg}"
    );
    assert!(
        msg.contains("下限"),
        "「空」ではなく**下限**で落ちていない（検出力を測れていない）: {msg}"
    );
    // 下限の手前（本番が十分残る形）では落ちないこと
    let ok = format!("{}\n{}", "pub fn big() { let _ = 0; }\n".repeat(400), src);
    production_range::production(&ok, "偽/ファイル.rs");
}

/// 作り物の入力だけでなく**リポジトリの実ファイル全部**で境界が壊れないこと。
///
/// 潰す範囲は `code_view` の位置決めに乗っているので、`r#"…"#` の中の `#[cfg(test)]`
/// や入れ子の波括弧で崩れると、静かに本番コードを削る / テストを残す事故になる。
/// 254 本を毎回舐めても 1 秒かからないので、番犬として常設する
#[test]
fn 実ファイルでも境界が壊れない() {
    let root = repo_root();
    let mut files = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        rust_files(&root.join("crates").join(crate_dir).join("src"), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 100,
        "走査先が間違っている（{} 本）",
        files.len()
    );

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        let scanned = production_range::scan(&src);

        // 長さと行数が保たれる（`file:line` の名指しがずれない）
        assert_eq!(scanned.text.len(), src.len(), "{rel}: バイト長が変わった");
        assert_eq!(
            scanned.text.lines().count(),
            src.lines().count(),
            "{rel}: 行数が変わった"
        );
        // テストが 1 つも無いファイルは丸ごと残る
        if scanned.regions.is_empty() {
            assert_eq!(scanned.text, src, "{rel}: 潰す領域が無いのに変わった");
            continue;
        }
        // 潰した領域は重ならず、前から順に並ぶ
        let mut prev = 0usize;
        for &(start, end) in &scanned.regions {
            assert!(
                start > prev && end >= start,
                "{rel}: 潰した領域が重なっている（{:?}）",
                scanned.regions
            );
            prev = end;
        }
        // コードとしての `#[cfg(test)]` は残らない（文字列の中の綴りは原文のまま残る）
        let view = production_range::code_view_of(&scanned.text);
        assert!(
            !view.contains("#[cfg(test)]"),
            "{rel}: コードの `#[cfg(test)]` が残っている（範囲の取り方が崩れた）"
        );
        // 潰した範囲は item ちょうど = 波括弧が釣り合う（宣言なら括弧を持たない）。
        // 深さを数え違えると途中で切れて、残りに壊れたテストコードが残る
        let original = production_range::code_view_of(&src);
        for &(start, end) in &scanned.regions {
            let region: String = original
                .lines()
                .skip(start - 1)
                .take(end - start + 1)
                .collect::<Vec<_>>()
                .join("\n");
            let opens = region.matches('{').count();
            let closes = region.matches('}').count();
            assert_eq!(
                opens, closes,
                "{rel}:{start} 潰した範囲の波括弧が釣り合わない（{opens} 対 {closes}）"
            );
        }
    }
}

// --- 番犬の番犬（雑な切り出しがリポジトリへ戻っていない）-------------------------

/// 切り出しに使われる呼び出し。この後ろに `"#[cfg(test)]"` を直書きする形が対象
const CUTTERS: &[&str] = &["find(", "split_once(", "rfind(", "splitn("];

/// 寄せていない切り出し（**件数まで固定する**ので増えても減っても落ちる）。
///
/// どちらも製品コードの中に住む番犬（`include_str!` で自分自身を読む形）で、
/// `crates/tako-app` からは `crates/tako-control/tests/common/` を取り込めない。
/// 共有部品へ寄せるには置き場を変える設計判断が要るので、#1420 では触らずに
/// ここへ理由つきで載せる（同ファイルを別 worker が編集中でもある）
const KNOWN_COARSE: &[(&str, usize, &str)] = &[
    (
        "crates/tako-app/src/main.rs",
        3,
        "製品コード内の番犬（include_str! で自分と chat_view.rs を読む）",
    ),
    (
        "crates/tako-app/src/open_files.rs",
        1,
        "製品コード内の番犬（include_str! で自分を読む）",
    ),
];

/// `.rs` を集める（`target` は見ない）
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 旧アームを意図的に残す 1 行に付ける目印（A/B の旧経路まで禁じると測れなくなる）
const LEGACY_MARKER: &str = "#1420-legacy-arm";

/// 雑な切り出し（`#[cfg(test)]` の直後が `\nmod` でない）の行番号。
/// コメント行と旧アームは飛ばす（説明文で落ちると番犬が書けなくなる）
fn coarse_cuts(src: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    for (idx, line) in src.lines().enumerate() {
        if line.trim_start().starts_with("//") || line.contains(LEGACY_MARKER) {
            continue;
        }
        for cutter in CUTTERS {
            for prefix in ["(\"", "(\"\\n"] {
                let needle = format!("{}{}#[cfg(test)]", cutter.trim_end_matches('('), prefix);
                for (at, _) in line.match_indices(&needle) {
                    if line[at + needle.len()..].starts_with("\\nmod") {
                        continue;
                    }
                    lines.push(idx + 1);
                }
            }
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

#[test]
fn 雑な切り出しが戻っていない() {
    let root = repo_root();
    let mut files = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        for sub in ["src", "tests"] {
            rust_files(&root.join("crates").join(crate_dir).join(sub), &mut files);
        }
    }
    files.sort();

    let mut found: Vec<(String, Vec<usize>)> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let hits = coarse_cuts(&src);
        if hits.is_empty() {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        found.push((rel, hits));
    }

    let mut offenders = Vec::new();
    for (rel, hits) in &found {
        match KNOWN_COARSE.iter().find(|(f, _, _)| f == rel) {
            Some((_, allowed, _)) if *allowed == hits.len() => {}
            Some((_, allowed, _)) => offenders.push(format!(
                "{rel}: 許可リストは {allowed} 件だが {} 件ある（{}）",
                hits.len(),
                hits.iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            None => offenders.extend(
                hits.iter()
                    .map(|l| format!("{rel}:{l} `#[cfg(test)]` で切っている（#1420）")),
            ),
        }
    }
    // 許可リストの取りこぼし（寄せ終わったのに残っている行）も落とす
    for (rel, allowed, _) in KNOWN_COARSE {
        if !found.iter().any(|(f, _)| f == rel) {
            offenders.push(format!(
                "{rel}: 許可リストに {allowed} 件あるが 1 件も無い（寄せ終わったら消す）"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "本番コードの範囲を「最初の `#[cfg(test)]`」で切っている:\n  {}\n\
         → `crates/tako-control/tests/common/production_range.rs` の 1 実装を通してください\
         （切らずにテスト領域だけを潰すので、途中のヘルパで走査範囲が消えない。#1420）",
        offenders.join("\n  ")
    );
}

#[test]
fn 寄せた番犬が1実装を通り続けている() {
    let dir = repo_root().join("crates/tako-control/tests");
    let mut missing = Vec::new();
    for name in [
        "issue1370_ipc_redraw_watchdog.rs",
        "issue1371_open_url_watchdog.rs",
        "issue1376_url_scheme_watchdog.rs",
        "issue1401_stale_stop_identity_watchdog.rs",
        "agent_resume_watchdog.rs",
        "platform_parity.rs",
        "remote_files_audit_watchdog.rs",
        "remote_link_watchdog.rs",
    ] {
        let src = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("{name} を読めない: {e}"));
        // **属性そのもの**を見る（doc コメントで名前に触れているだけでは通さない）
        if !src.contains("#[path = \"common/production_range.rs\"]") {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "1 実装（common/production_range.rs）を通らなくなった番犬がある: {missing:?}\n\
         範囲取りを書き直すと、片方だけ途中のテスト用ヘルパで盲目になる（#1420）"
    );
}
