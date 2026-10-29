//! `CHANGELOG.md` の `## [Unreleased]` が最上段に 1 つだけであることを守る番犬（Issue #1593）。
//!
//! # なぜ要るか
//!
//! 夜間リリース（`scripts/nightly-release.sh`）は版の節を CHANGELOG へ挿す。旧版は
//! 「最初の `## [` の直前」へ挿したので、最上段の `## [Unreleased]` 自体がそこに当たって
//! 新しい版の節の下へ押し出され、次の PR が最上段に新しい Unreleased を作る、を繰り返して
//! 10/30 には 5 つ並んでいた。下にある Unreleased は「未リリース」と誤読され、そこへ書いた
//! 手書きの項目はどの版のリリースノートにも載らなかった（`release.sh` の抽出は次の `## [` =
//! 押し出された Unreleased の見出しで止まる）。
//!
//! 挿し方は `scripts/lib/changelog.sh` で直した（モックは `scripts/test-release-retry.sh` の
//! Test 14〜15）。ここは CHANGELOG.md の実物を読み、手で書いた PR や手動リリースが同じ形を
//! 作ったら落とす。
//!
//! # 規則
//!
//! - `## [Unreleased]`（大文字小文字を問わない）は 0 か 1 個
//! - あるなら最初の版の節（`## [x.y.z]`）より上
//!
//! # 偽の緑を作らないための作り
//!
//! - [`二つ目を差し込むと_file_line_で名指しする`] — 実物の CHANGELOG.md の版の節の間へ
//!   #1593 と同じ形の 2 つ目を差し込み、その行番号で名指しされることを確かめる
//! - [`走査が空振りしていない`] — 実物から版の節を一定数以上採れることを確かめる。見出しの判定が
//!   壊れて 1 件も採れなくなると、本体の検査は無条件に緑になるので、そこを塞ぐ

use std::path::{Path, PathBuf};

const CHANGELOG: &str = "CHANGELOG.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read_changelog() -> String {
    let p = repo_root().join(CHANGELOG);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

/// 節の見出し（`## [名前]`）なら名前を返す。コードフェンスの中は見ない
fn headings(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some((name, _)) = line
            .strip_prefix("## [")
            .and_then(|rest| rest.split_once(']'))
        {
            out.push((i + 1, name));
        }
    }
    out
}

fn is_unreleased(name: &str) -> bool {
    name.eq_ignore_ascii_case("unreleased")
}

/// 規則に反する見出しを `file:line: 理由` の形で返す
fn violations(file: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut first_version: Option<usize> = None;
    let mut first_unreleased: Option<usize> = None;
    for (line, name) in headings(text) {
        if !is_unreleased(name) {
            first_version.get_or_insert(line);
            continue;
        }
        match (first_unreleased, first_version) {
            (Some(first), _) => out.push(format!(
                "{file}:{line}: 2 つ目の `## [Unreleased]`（1 つ目は {first} 行）"
            )),
            (None, Some(version)) => out.push(format!(
                "{file}:{line}: `## [Unreleased]` が最初の版の節（{version} 行）より下にある"
            )),
            (None, None) => {}
        }
        first_unreleased.get_or_insert(line);
    }
    out
}

#[test]
fn changelog_の_unreleased_は最上段に_1_つ() {
    let found = violations(CHANGELOG, &read_changelog());
    assert!(
        found.is_empty(),
        "CHANGELOG.md の `## [Unreleased]` は最初の版の節より上に 1 つだけ置く（#1593）\n{}\n\
         直し方: 項目は最上段の `## [Unreleased]` の下へ書き、下にできた見出しは消す。\n\
         下の見出しの項目が既にリリース済みなら、その版の節へ移す（未リリースなら最上段へ）。\n\
         夜間リリースの挿し方は scripts/lib/changelog.sh（モック: bash scripts/test-release-retry.sh）",
        found.join("\n")
    );
}

#[test]
fn 二つ目を差し込むと_file_line_で名指しする() {
    let text = read_changelog();
    let versions: Vec<usize> = headings(&text)
        .into_iter()
        .filter(|(_, name)| !is_unreleased(name))
        .map(|(line, _)| line)
        .collect();
    assert!(versions.len() >= 2, "版の節が 2 つ以上ない: {versions:?}");

    // #1593 の形: 2 つ目の版の節の直前（= 1 つ目の版の節の下）へ Unreleased が押し出されている
    let at = versions[1];
    let mut lines: Vec<&str> = text.lines().collect();
    for (k, l) in ["## [Unreleased]", "", "- 押し出された項目", ""]
        .into_iter()
        .enumerate()
    {
        lines.insert(at - 1 + k, l);
    }
    let injected = lines.join("\n");
    let found = violations(CHANGELOG, &injected);
    let has_top = headings(&text)
        .first()
        .is_some_and(|(_, name)| is_unreleased(name));
    let want = if has_top {
        format!("{CHANGELOG}:{at}: 2 つ目の `## [Unreleased]`")
    } else {
        format!("{CHANGELOG}:{at}: `## [Unreleased]` が最初の版の節")
    };
    assert!(
        found.iter().any(|v| v.starts_with(&want)),
        "差し込んだ {at} 行が名指しされない: {found:?}"
    );
}

#[test]
fn 判定の境界() {
    // 0 個は通す（手動リリースで Unreleased を版へ書き換えた直後の形）
    assert!(violations("C", "# C\n\n## [1.0.0] - 2026-01-01\n").is_empty());
    // 最上段に 1 つは通す
    assert!(violations("C", "# C\n\n## [Unreleased]\n\n## [1.0.0]\n").is_empty());
    // 1 つでも版の節より下なら落とす（手動で版の節を Unreleased の上へ書いた形）
    assert_eq!(
        violations("C", "# C\n\n## [1.0.0]\n\n## [Unreleased]\n"),
        vec!["C:5: `## [Unreleased]` が最初の版の節（3 行）より下にある".to_string()]
    );
    // 大文字小文字を問わない・2 つ目を名指しする
    assert_eq!(
        violations("C", "## [Unreleased]\n## [unreleased]\n## [1.0.0]\n"),
        vec!["C:2: 2 つ目の `## [Unreleased]`（1 つ目は 1 行）".to_string()]
    );
    // コードフェンスの中の見出しは数えない
    assert!(violations(
        "C",
        "## [Unreleased]\n```\n## [Unreleased]\n```\n## [1.0.0]\n"
    )
    .is_empty());
}

#[test]
fn 走査が空振りしていない() {
    let text = read_changelog();
    let all = headings(&text);
    let versions = all.iter().filter(|(_, name)| !is_unreleased(name)).count();
    assert!(
        versions >= 30,
        "CHANGELOG.md から版の節を {versions} 個しか採れない（見出しの判定が壊れている）"
    );
    assert!(
        all.iter().all(|(_, name)| !name.is_empty()),
        "空の見出しを採っている: {all:?}"
    );
}
