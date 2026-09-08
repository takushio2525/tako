//! ホーム解決と `~` 短縮の入口が 1 本であることの番犬（#893）
//!
//! #870（ターミナルリンクの `~/` が Windows で無反応）の真因は、ホーム解決が
//! `links.rs` と `terminal.rs` の 2 か所にあって**片方だけ `HOME` 決め打ち**だった
//! ことだった。`HOME` は Windows に通常無いので、その経路は必ず `None` を返す。
//!
//! #870 は名指しされた 2 か所を寄せて閉じたが、番犬もその 2 ファイルしか見て
//! いなかった。棚卸ししたら同型が 15 箇所残っていた（#893）ので、走査を
//! **ワークスペース全体**へ広げてここで止める。
//!
//! # 何を禁じるか
//!
//! 1. `HOME` / `USERPROFILE` を**ホーム組み立ての材料として直接読む**形
//!    → 正本は [`tako_core::paths::home_dir`]
//! 2. `~` 短縮（home 接頭辞を `~` へ置き換える規則）を各所で組み立てる形
//!    → 正本は [`tako_core::paths::shorten_home`]
//!
//! 散文（doc コメント・規約の説明）は実装ではないので拾わない。
//!
//! # 例外を「ファイル + needle」で持つ理由
//!
//! `cfg(test)` の中だけ許す、という書き方を最初に試したが、
//! テスト本文の文字列リテラルに `{` が 1 つあるだけで波括弧の対応が崩れ、
//! **そのファイルの残り全部が例外になる**（= 本番コードの直読みを見逃す）。
//! 数え違いが「見逃す」側へ倒れる検査は番犬として使えないので、
//! 例外は [`ALLOWED_HOME`] に**ファイル名と needle の組**で列挙する形にした。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// ホーム組み立ての材料として env を読む形（`var_os` / `var` の両方）
const HOME_NEEDLES: &[&str] = &[
    "var_os(\"HOME\")",
    "var(\"HOME\")",
    "var_os(\"USERPROFILE\")",
    "var(\"USERPROFILE\")",
];

/// ファイルごとに直読みを許す needle。ここに無いファイルは 1 つも許さない
const ALLOWED_HOME: &[(&str, &[&str])] = &[
    // 境界そのもの（`home_dir()` の正本と、`data_dir()` の OS 別データ配置規約）
    ("crates/tako-core/src/paths.rs", HOME_NEEDLES),
    // cfg(windows) の中で**ネイティブの `%USERPROFILE%` を名指しで**欲しい場所
    // （PowerShell の `$PROFILE` の既定位置・Windows 版インストーラの置き場）。
    // `home_dir()` は `HOME` を優先するので、Git Bash から起動した Windows で
    // 意図がずれる。`HOME` の直読みはここでも禁止（許すのは USERPROFILE だけ）
    (
        "crates/tako-core/src/platform/exe.rs",
        &["var_os(\"USERPROFILE\")"],
    ),
    (
        "crates/tako-core/src/shell_integration.rs",
        &["var_os(\"USERPROFILE\")"],
    ),
    // テストが `HOME` を差し替えて元へ戻すためのガード。控えを取る目的で
    // 読むのはここだけ（`tako_control::test_home::HomeGuard`）
    (
        "crates/tako-control/src/test_home.rs",
        &["var_os(\"HOME\")"],
    ),
];

/// `~` 短縮を自前で組み立ててよい場所
const ALLOWED_SHORTEN: &[&str] = &[
    // 正本
    "crates/tako-core/src/paths.rs",
    // #513 の**可搬表記**（デバイス間で設定を共有するため、区切りを `/` へ
    // 正規化した形を作る）。表示用の短縮とは役目が違うので別実装のまま
    "crates/tako-control/src/config_share/env.rs",
];

/// `crates/**/*.rs` を集める（`src/` も `tests/` も見る。#893 の C 群
/// `issue652_resume_e2e.rs` は統合テスト側にあった）
fn crate_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(&repo_root().join("crates"), &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// その行が needle を**読み取りとして**含むか。
///
/// 素朴な `contains` だと `remove_var("HOME")` が `var("HOME")` に当たる
/// （書き込み・削除は読み取りではないので落としてはいけない）。needle の
/// 直前が識別子の文字なら別の関数名の一部と見なして無視する
fn is_direct_read(line: &str, needle: &str) -> bool {
    let mut rest = line;
    while let Some(at) = rest.find(needle) {
        let before = rest[..at].chars().next_back();
        if !before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return true;
        }
        rest = &rest[at + needle.len()..];
    }
    false
}

/// 走査対象の (リポジトリ相対パス, 実装行) を列挙する（`//` 始まりの散文は落とす）
fn implementation_lines() -> Vec<(String, usize, String)> {
    let root = repo_root();
    let mut out = Vec::new();
    for path in crate_sources() {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in body.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            out.push((rel.clone(), n + 1, line.to_string()));
        }
    }
    out
}

#[test]
fn ホーム解決の入口がpathsだけである() {
    let mut offenders = Vec::new();
    for (rel, line_no, line) in implementation_lines() {
        let allowed: &[&str] = ALLOWED_HOME
            .iter()
            .find(|(file, _)| *file == rel.as_str())
            .map(|(_, needles)| *needles)
            .unwrap_or(&[]);
        for needle in HOME_NEEDLES {
            if is_direct_read(&line, needle) && !allowed.contains(needle) {
                offenders.push(format!("{rel}:{line_no} {needle}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ホーム解決を直接書いている: {offenders:#?}\n\
         ホームは tako_core::paths::home_dir()（`HOME` → `%USERPROFILE%`）を通すこと。\n\
         `HOME` 決め打ちは Windows で必ず None を返し、その機能がまるごと落ちる（#870 / #893）"
    );
}

#[test]
fn ホーム短縮の入口がpathsだけである() {
    let mut offenders = Vec::new();
    for (rel, line_no, line) in implementation_lines() {
        if ALLOWED_SHORTEN.contains(&rel.as_str()) {
            continue;
        }
        // `~` を頭に付けて残りを繋ぐ形（`format!("~{rest}")` / `format!("~/{}", …)`）
        if line.contains("format!(\"~{") || line.contains("format!(\"~/{") {
            offenders.push(format!("{rel}:{line_no}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "`~` 短縮を組み立てている: {offenders:#?}\n\
         表示用の短縮は tako_core::paths::shorten_home()（純粋版は shorten_home_with）\
         の 1 本を通すこと。前は 10 か所にあって、ホーム解決の欠け・ホーム自身の\
         扱い（`~` と `~/`）・前方一致の食い込み（`/Users/alice2` → `~2`）を\
         別々に持っていた（#893）"
    );
}

/// 番犬自身が「何も走査していない」状態で緑にならないこと。
///
/// `repo_root()` の解決や `crates/` の場所が変わると、走査対象 0 件のまま
/// 2 本とも通ってしまう。実装行と needle の見つかる件数に下限を置く
#[test]
fn 番犬が走査対象を見つけている() {
    let lines = implementation_lines();
    assert!(
        lines.len() > 100_000,
        "実装行が少なすぎる（走査に失敗している）: {} 行",
        lines.len()
    );
    let allowed_hits = lines
        .iter()
        .filter(|(rel, _, line)| {
            rel == "crates/tako-core/src/paths.rs"
                && HOME_NEEDLES
                    .iter()
                    .any(|needle| is_direct_read(line, needle))
        })
        .count();
    assert!(
        allowed_hits >= 4,
        "正本の直読みを 1 つも見つけていない（needle が実装とずれている）: {allowed_hits} 件"
    );
}

/// 読み取りと書き込みを取り違えないこと（番犬自身の検出力）
#[test]
fn 書き込みは直読みとして数えない() {
    // 読み取り = 落とす対象
    assert!(is_direct_read(
        "    home_from(std::env::var_os(\"HOME\"), x)",
        "var_os(\"HOME\")"
    ));
    assert!(is_direct_read(
        "let h = var(\"HOME\").ok();",
        "var(\"HOME\")"
    ));
    // 書き込み・削除 = 落とさない（テストがホームを差し替えるのは正当）
    assert!(!is_direct_read(
        "std::env::remove_var(\"HOME\");",
        "var(\"HOME\")"
    ));
    assert!(!is_direct_read(
        "std::env::set_var(\"HOME\", &dir);",
        "var(\"HOME\")"
    ));
    // 1 行に両方あれば読み取りとして拾う
    assert!(is_direct_read(
        "let o = std::env::var_os(\"HOME\"); std::env::remove_var(\"HOME\");",
        "var_os(\"HOME\")"
    ));
}
