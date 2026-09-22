//! **#1278 の番犬**: テストの skip には必ず理由が書かれている。
//!
//! ## なぜ要るか
//!
//! #1278 / #583 で CI の Windows ジョブの `cargo test --workspace` を blocking にした。
//! blocking になると「落ちるテストを `#[ignore]` で黙らせる」が最短経路になり、
//! **緑だが何も見ていない CI** へ静かに退行しうる。
//!
//! そこで skip そのものは許し（Windows に存在しない仕組みを見るテストは実際にある）、
//! **理由の無い skip だけを落とす**。理由が書いてあれば
//!
//! - `cargo test` の出力と `--ignored` の一覧にそのまま出る
//! - レビューで「これは本当に Windows で意味が無いのか」を判断できる
//! - 後から「まだ塞がっているのか」を Issue 番号で追える
//!
//! ## 対象
//!
//! - `#[ignore]`（理由なし）→ `#[ignore = "理由"]` にする
//! - `#[cfg_attr(<条件>, ignore)]`（理由なし）→ `#[cfg_attr(<条件>, ignore = "理由")]` にする
//!
//! `#[cfg(...)]` によるコンパイル時の除外はここでは見ない（そもそもテストとして
//! 存在しなくなるので `--ignored` の一覧にも出ない）。代わりに**すぐ上の行に
//! doc コメント（`///`）か通常コメント（`//`）で理由を書く**ことを規約とし、
//! `.agent/conventions.md` の「CI の Windows ジョブの契約」節に置いた。

use std::path::{Path, PathBuf};

/// この番犬自身（禁止形の文字列そのものを本文に持つので走査から外す）
const SELF_FILE: &str = "issue1278_ignore_reason_watchdog.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// `crates/` 配下の `.rs` を全部集める（`target/` は入らない）
fn rust_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&repo_root().join("crates"), &mut out);
    out.sort();
    assert!(
        out.len() > 100,
        "走査が壊れている（*.rs が {} 件しか見つからない）",
        out.len()
    );
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn rel(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 理由の無い `#[ignore]` / `#[cfg_attr(…, ignore)]` を `file:line` で拾う。
///
/// 属性は 1 行に書かれる前提（このリポジトリの全 48 箇所がそう）。
/// 行の中の `ignore` は**属性の位置にあるものだけ**を見るので、
/// `#[allow(…)]` の中の語や文字列リテラルの `ignore` には当たらない
fn reasonless_ignores(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let Some(inner) = trimmed.strip_prefix("#[").and_then(|r| r.strip_suffix("]")) else {
            continue;
        };
        // `#[ignore]`（属性そのもの）
        if inner.trim() == "ignore" {
            out.push((i + 1, trimmed.to_string()));
            continue;
        }
        // `#[cfg_attr(<条件>, ignore)]` — 最後の要素が理由なしの `ignore`
        if let Some(args) = inner
            .trim()
            .strip_prefix("cfg_attr(")
            .and_then(|r| r.strip_suffix(')'))
        {
            let last = args.rsplit(',').next().unwrap_or("").trim();
            if last == "ignore" {
                out.push((i + 1, trimmed.to_string()));
            }
        }
    }
    out
}

#[test]
fn 理由の無いignoreがリポジトリに無い() {
    let mut violations: Vec<String> = Vec::new();
    for path in rust_sources() {
        if path.file_name().is_some_and(|n| n == SELF_FILE) {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        for (line, text) in reasonless_ignores(&source) {
            violations.push(format!("{}:{line}  {text}", rel(&path)));
        }
    }
    assert!(
        violations.is_empty(),
        "理由の無い skip がある（#1278）。`#[ignore = \"なぜ走らせないか\"]` の形にすること。\n\
         CI の Windows は結果まで blocking（#1278 / #583）なので、理由なしの skip は\n\
         「緑だが何も見ていない」への最短経路になる:\n{}",
        violations.join("\n")
    );
}

/// 検出力の実証。**この番犬が本当に落ちる**ことを同じ実行の中で見せる
/// （走査が壊れて 0 件になっても上のテストは緑になってしまうため）
#[test]
fn 検出力_理由の有無で判定が分かれる() {
    // 理由なし = 違反
    assert_eq!(reasonless_ignores("    #[ignore]\n").len(), 1);
    assert_eq!(
        reasonless_ignores("#[cfg_attr(windows, ignore)]\n").len(),
        1
    );
    assert_eq!(
        reasonless_ignores("    #[cfg_attr(target_os = \"windows\", ignore)]\n").len(),
        1
    );
    // 理由あり = 違反ではない
    assert!(reasonless_ignores("    #[ignore = \"実機が要る\"]\n").is_empty());
    assert!(
        reasonless_ignores("#[cfg_attr(windows, ignore = \"Windows に tmux が無い\")]\n")
            .is_empty()
    );
    // 属性でない `ignore` には当たらない
    assert!(reasonless_ignores("let ignore = 1; // ignore\n").is_empty());
    assert!(reasonless_ignores("    #[allow(clippy::ignored_unit_patterns)]\n").is_empty());
    assert!(reasonless_ignores("//! すべて `#[ignore]`。CI では走らない\n").is_empty());
}
