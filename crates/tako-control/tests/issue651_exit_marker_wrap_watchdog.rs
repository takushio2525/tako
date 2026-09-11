//! 番犬: 終了マーカーの契約を増やさない / 折り返しの判定を**列**で持つ（#651）
//!
//! ## なぜ止めるのか
//!
//! 実行ペインの終了コードは `__TAKO_EXIT=<code>` の 1 行が唯一の契約で、最短でも
//! 13 文字（`__TAKO_EXIT=0`）ある。**ペインの幅がそれより狭いと端末が必ず割る**ので、
//! 物理行 1 本の中だけを探す実装は幅 13 桁未満で必ず外れ、`--wait` が
//! 永久待機する（#651 の実測: 幅 10 桁で 40 秒待って返らない。ポーリングに上限が無い）。
//!
//! ## 2 本立て（片方だけでは穴が残る）
//!
//! 1. [`マーカーの文字列を持つ実装は1箇所だけ`] — 製品コードに新しい
//!    `"__TAKO_EXIT="` の literal が生えていないこと。読む側が 2 つに増えると、
//!    片方だけ折り返しに耐える形になる（#875 で組み立て側と読む側を 1 個の定数へ
//!    寄せたのと同じ理由）
//! 2. [`折り返しの判定は列で持つ`] — 「行が右端まで埋まっているか」を
//!    **文字数**で代用していないこと。全角が 1 つあるだけで `chars().count() < cols` に
//!    なるので、#325 の形（全角プロンプト + マーカー）が折り返したときに拾えない。
//!    `Screen` の `cell_cols` で測るのも禁止（空白詰めのせいで**どの行も** soft wrap
//!    扱いになる = #1182 の実害）

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// テストモジュールより手前だけを見る（fixture や期待値の文字列に当たらない）。
/// **`mod tests` の直前で切る**（`#[cfg(test)]` はテスト用ヘルパにも付くので、
/// 最初の出現で切ると製品コードのほとんどを見落とす）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

/// コメント行を落とす（説明文に書いた旧コードの引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `crates/*/src` 配下の `.rs` を全部集める
fn production_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root().join("crates");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).expect("crates を読む").flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            walk(&src, &mut out);
        }
    }
    out.sort();
    assert!(out.len() > 20, "走査対象が少なすぎる: {}", out.len());
    out
}

/// マーカーの literal を書いてよい場所（file の相対パス, **その行に必ず在る目印**）。
///
/// - 定数の定義 1 本（読む側 / 組み立てる側が共有する唯一の契約。#875）
/// - 対応マトリクスの根拠テキスト（実機の before/after の記録。判定には使わない）
/// - セルフテスト項目 143(c) の観測（#1031 の「終了コードが**画面に読める**」が
///   検査したいことそのものなので、画面テキストを直接見てよい。製品の判定ではない）
///
/// 目印は行ごとなので、**同じファイルに別の読み手が生えたら落ちる**
const ALLOWED: &[(&str, &str)] = &[
    (
        "crates/tako-control/src/dispatch.rs",
        "const EXIT_MARKER_PREFIX",
    ),
    (
        "crates/tako-core/src/platform/support.rs",
        "#875 の実機 before/after",
    ),
    (
        "crates/tako-app/src/main.rs",
        "joined.contains(\"__TAKO_EXIT=7\")",
    ),
];

/// `rel` の製品コードで「許されていないマーカー literal」を file:line で並べる
fn offending_lines(rel: &str, src: &str) -> Vec<String> {
    let allowed: Vec<&str> = ALLOWED
        .iter()
        .filter(|(f, _)| *f == rel)
        .map(|(_, mark)| *mark)
        .collect();
    let prod = production_source(src);
    // コメント行は落とすが、行番号は元のまま数える（潰さず素の行で判定する）
    prod.lines()
        .enumerate()
        .filter(|(_, line)| {
            let t = line.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .filter(|(_, line)| line.contains("__TAKO_EXIT="))
        .filter(|(_, line)| !allowed.iter().any(|mark| line.contains(mark)))
        .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
        .collect()
}

#[test]
fn マーカーの文字列を持つ実装は1箇所だけ() {
    let root = repo_root();
    let mut found = Vec::new();
    let mut allowed_hits = 0usize;
    for path in production_files() {
        let rel = path
            .strip_prefix(&root)
            .expect("リポジトリ内")
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(&path).expect("ソースを読む");
        if !src.contains("__TAKO_EXIT=") {
            continue;
        }
        if ALLOWED.iter().any(|(f, _)| *f == rel) {
            allowed_hits += 1;
        }
        found.extend(offending_lines(&rel, &src));
    }
    assert_eq!(
        allowed_hits,
        ALLOWED.len(),
        "許可した置き場のどれかが消えている（改名したら ALLOWED も直す）"
    );
    assert!(
        found.is_empty(),
        "マーカーの literal が増えている（読む側は dispatch::find_exit_marker の 1 実装に\
         寄せる。組み立て側は EXIT_MARKER_PREFIX を渡す）:\n{}",
        found.join("\n")
    );
}

#[test]
fn i651_注入_マーカー文字列の増殖を名指しで落とせる() {
    // 修正前の形（物理行 1 本の中を探す）が生えたら落ちる
    let injected = "fn find(line: &str) -> Option<i32> {\n    \
        line.find(\"__TAKO_EXIT=\").and_then(|p| line[p..].parse().ok())\n}\n";
    let got = offending_lines("crates/tako-control/src/other.rs", injected);
    assert_eq!(got.len(), 1, "注入を拾えていない: {got:?}");
    assert!(got[0].contains(":2:"), "行番号が合っていない: {got:?}");

    // 許可した目印は「同じファイル」でしか効かない（別ファイルへ写したら落ちる）
    let moved = "const EXIT_MARKER_PREFIX: &str = \"__TAKO_EXIT=\";\n";
    assert!(
        offending_lines("crates/tako-app/src/main.rs", moved).len() == 1,
        "定数を別ファイルへ写したのに通った"
    );
    assert!(
        offending_lines("crates/tako-control/src/dispatch.rs", moved).is_empty(),
        "許可した置き場を落としている"
    );

    // コメントとテストモジュールは対象外（番犬自身の説明文や fixture を拾わない）
    let commented = "// line.find(\"__TAKO_EXIT=\")\n";
    assert!(offending_lines("crates/x/src/a.rs", commented).is_empty());
    let in_tests = "fn f() {}\n#[cfg(test)]\nmod tests {\n    let s = \"__TAKO_EXIT=0\";\n}\n";
    assert!(offending_lines("crates/x/src/a.rs", in_tests).is_empty());
}

/// 指定した関数の本文（`signature` から `end_marker` まで）を切り出す
fn fn_body(src: &str, signature: &str, end_marker: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let rest = &src[start..];
    let end = rest.find(end_marker).map(|i| i + 1).unwrap_or(rest.len());
    without_comments(&rest[..end])
}

/// 「折り返しの続きがありうる行か」を**列**で測っているかを判定する。
/// 返すのは違反の理由（空なら合格）
fn width_measure_violations(body: &str) -> Vec<String> {
    let joined = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = Vec::new();
    if !joined.contains("line_length()") {
        out.push("line_length()（WRAPLINE + 占有列数）で測っていない".to_string());
    }
    for bad in [
        "chars().count()",
        "chars() .count()",
        "cell_cols",
        "self.screen(",
        "self.visible_lines()",
    ] {
        if joined.contains(bad) {
            out.push(format!("{bad} で折り返しを判定している"));
        }
    }
    out
}

#[test]
fn 折り返しの判定は列で持つ() {
    let terminal = std::fs::read_to_string(repo_root().join("crates/tako-core/src/terminal.rs"))
        .expect("terminal.rs を読む");
    let body = fn_body(
        production_source(&terminal),
        "pub fn visible_lines_filled(",
        "\n    }\n",
    );
    let violations = width_measure_violations(&body);
    assert!(
        violations.is_empty(),
        "visible_lines_filled の物差しが列でない: {}\n---\n{body}",
        violations.join(" / ")
    );

    // 読む側は自分で幅を測り直さない（渡された印を見る）
    let dispatch = std::fs::read_to_string(repo_root().join("crates/tako-control/src/dispatch.rs"))
        .expect("dispatch.rs を読む");
    let wrap = fn_body(production_source(&dispatch), "fn wrap_to_next(", "\n}\n");
    assert!(
        wrap.contains("rows[li].1"),
        "wrap_to_next が「右端まで埋まっているか」の印を見ていない:\n{wrap}"
    );
    for bad in ["cols", "chars().count()"] {
        assert!(
            !wrap.contains(bad),
            "wrap_to_next が幅を測り直している（{bad}）:\n{wrap}"
        );
    }
}

#[test]
fn i651_注入_文字数で測る実装を名指しで落とせる() {
    // #1182 / #651 で否定した 3 つの物差しが、それぞれ違反として挙がること
    let by_chars = "let fills = text.chars().count() >= cols;";
    assert!(!width_measure_violations(by_chars).is_empty());
    let by_cell_cols = "let fills = line.cell_cols.last().is_some_and(|c| c + 1 >= cols);";
    assert!(!width_measure_violations(by_cell_cols).is_empty());
    let via_screen = "let lines = self.screen(&Theme::default()).lines;";
    assert!(!width_measure_violations(via_screen).is_empty());
    // 合格の形（列で測る）は 1 件も挙がらない
    let ok = "let fills = row.line_length().0 >= cols;";
    assert!(
        width_measure_violations(ok).is_empty(),
        "合格の形を落としている: {:?}",
        width_measure_violations(ok)
    );
}
