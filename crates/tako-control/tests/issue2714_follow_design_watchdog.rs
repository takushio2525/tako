//! **#2714 の番犬**: visual-test の追従の判定（#1741 viewport-lines / #1772 editor-font）の設計値を、
//! 固定の余白で引く形へ戻さない。viewport-lines の fixture を言語サーバの受け持つ拡張子へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! 製品の追従（`tako_core::editor_scroll`）は狭い器で余白を `effective_margin` まで詰める
//! （上下 m 行 + カーソル行の 2m + 1 行が収まる m。#1649）。ところが判定は「器が動き始める行 =
//! 可視 − `FOLLOW_MARGIN`」「以降の余白 = `FOLLOW_MARGIN` 行」と固定の 3 行で引いていたので、
//! 可視 7 行以下の器（窓を固定しない全節実行の文字の最大）で製品の設計どおりの動きを NG と数え、
//! 可視 3 行未満では `visible - FOLLOW_MARGIN` が溢れて debug で panic し、全節実行が後ろの節へ
//! 届かなかった。さらに fixture が `.rs` だったので、全節実行の `TAKO_LSP_AUTOSTART=1` で隔離 HOME の
//! 動かない rust-analyzer の未導入の帯が節の途中に出て、器の高さを 129px 取った。
//!
//! いまは設計値を `follow_design`（`effective_margin` から引く 1 実装）に寄せ、両方の判定がそこを通る。
//! fixture の拡張子は `VLINES_FIXTURE_EXT`（言語サーバの受け持たない `txt`）。
//!
//! ## ここで止めるもの
//!
//! 1. [`余白を固定値で引く形が無い`] — tako-app / tako-core / tako-control の本文に
//!    `- FOLLOW_MARGIN` / `-= FOLLOW_MARGIN` / `…sub(FOLLOW_MARGIN)` があれば file:line で名指す
//!    （溢れる引き算も、溢れを隠す `saturating_sub` も、狭い器の設計を見ない形なので同じく止める）
//! 2. [`判定の設計値は詰めた余白の1実装から引く`] — `follow_design` が `effective_margin` を通り、
//!    viewport-lines の `judge_down` と editor-font の `walk_down` が `follow_design` を通る
//! 3. [`viewport_linesのfixtureは言語サーバを呼ばない`] — `VLINES_FIXTURE_EXT` の拡張子を受け持つ
//!    言語サーバが無い（判定は製品と同じ `tako_core::lsp::servers::resolve`）。節がその定数を使う
//! 4. [`修正前の形を名指しできる`] — 修正前の形を注入すると 1〜3 のどれかが名指しで落ちる
//!
//! ## 相方
//!
//! 設計そのもの（狭い器での動き始めと余白）は `tako_core::editor_scroll` の単体テスト
//! `先頭から下へ進むと詰めた余白の行で動き始める`、実 GUI は `scripts/test-viewport-lines-1741.sh`
//! （1400×880・狭い器・帯の注入・途中で縮める注入と、`TAKO_2714_LEGACY=1` の旧の腕）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";

/// 1 の走査範囲（製品と visual-test の本文）
const SCAN_ROOTS: &[&str] = &[
    "crates/tako-app/src",
    "crates/tako-core/src",
    "crates/tako-control/src",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// 規則 1: 余白を固定値で引く行（コメントと文字列を潰した眺めで、空白を詰めて探す）
fn fixed_margin_subtractions(rel: &str, src: &str) -> Vec<String> {
    let view = code_view::code_view(src);
    let mut hits = Vec::new();
    for (ix, line) in view.lines().enumerate() {
        let packed: String = line
            .split_whitespace()
            .collect::<String>()
            .replace("tako_core::editor_scroll::", "")
            .replace("editor_scroll::", "");
        let bad = packed.contains("-FOLLOW_MARGIN")
            || packed.contains("-=FOLLOW_MARGIN")
            || packed.contains("sub(FOLLOW_MARGIN");
        if bad {
            hits.push(format!(
                "{rel}:{}: {}",
                ix + 1,
                src.lines().nth(ix).unwrap_or("").trim()
            ));
        }
    }
    hits
}

/// `head` の後ろの最初の `{` から対の `}` までの本文（コードだけの眺めで括弧を数える）
fn block_after(code: &str, head: &str) -> Option<String> {
    let at = code.find(head)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in code.as_bytes()[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(code[open..=open + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// 規則 2: 判定が設計値の 1 実装を通っているか（足りないものを返す）
fn design_routing(src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let mut problems = Vec::new();
    let body = |head: &str| block_after(&code, head);
    match body("fn follow_design(") {
        Some(b) if b.contains("effective_margin(") => {}
        Some(_) => problems.push(
            "`follow_design` が `effective_margin` を通っていない（狭い器で余白が詰まる製品の設計を見ない）"
                .to_string(),
        ),
        None => problems.push("`fn follow_design(` が無い（設計値の 1 実装）".to_string()),
    }
    for (head, what) in [
        ("fn judge_down(", "viewport-lines の `judge_down`"),
        ("let walk_down = |", "editor-font の `walk_down`"),
    ] {
        match body(head) {
            Some(b) if b.contains("follow_design(") => {}
            Some(_) => problems.push(format!(
                "{what} が `follow_design` を通っていない（設計値を判定の中で引き直している）"
            )),
            None => problems.push(format!("{what}（`{head}`）が見つからない")),
        }
    }
    problems
}

/// 規則 3: fixture の拡張子（定数の値）と、それを受け持つ言語サーバ
fn fixture_ext_problems(src: &str) -> Vec<String> {
    let view = code_view::without_comments(src);
    let head = "const VLINES_FIXTURE_EXT: &str = \"";
    let Some(at) = view.find(head) else {
        return vec![
            "`const VLINES_FIXTURE_EXT` が無い（viewport-lines の fixture の拡張子）".into(),
        ];
    };
    let rest = &view[at + head.len()..];
    let ext = &rest[..rest.find('"').unwrap_or(0)];
    let mut problems = Vec::new();
    let probe = format!("/tako-2714-watchdog/long.{ext}");
    if let Some(found) = tako_core::lsp::servers::resolve(Path::new(&probe)) {
        problems.push(format!(
            "fixture の拡張子 `{ext}` を言語サーバ `{}` が受け持つ（全節実行の TAKO_LSP_AUTOSTART=1 で\
             帯が節の途中に出て器の高さが変わる）",
            found.spec.id
        ));
    }
    let code = code_view::code_view(src);
    match block_after(&code, "async fn viewport_lines_visual(") {
        Some(b) if b.contains("VLINES_FIXTURE_EXT") => {}
        Some(_) => problems.push(
            "viewport-lines の節が `VLINES_FIXTURE_EXT` を使っていない（拡張子を直書きしている）"
                .into(),
        ),
        None => problems.push("`async fn viewport_lines_visual(` が見つからない".into()),
    }
    problems
}

#[test]
fn 余白を固定値で引く形が無い() {
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in SCAN_ROOTS {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 50,
        "走査範囲が空振りしている（{} ファイル）",
        files.len()
    );
    let mut hits = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .expect("ワークスペースの中")
            .to_string_lossy()
            .replace('\\', "/");
        hits.extend(fixed_margin_subtractions(&rel, &read(&rel)));
    }
    assert!(
        hits.is_empty(),
        "余白を固定の FOLLOW_MARGIN で引いている（#2714。製品は狭い器で余白を `effective_margin` まで\
         詰めるので、可視 7 行以下で設計と食い違い、3 行未満では引き算が溢れて panic する。\
         設計値は visual-test の `follow_design` から引く）:\n{}",
        hits.join("\n")
    );
}

#[test]
fn 判定の設計値は詰めた余白の1実装から引く() {
    let src = read(MAIN);
    code_view::without_comments_checked(&src, MAIN);
    let problems = design_routing(&src);
    assert!(
        problems.is_empty(),
        "{MAIN}（#2714）:\n{}",
        problems.join("\n")
    );
}

#[test]
fn viewport_linesのfixtureは言語サーバを呼ばない() {
    let src = read(MAIN);
    let problems = fixture_ext_problems(&src);
    assert!(
        problems.is_empty(),
        "{MAIN}（#2714）:\n{}",
        problems.join("\n")
    );
}

#[test]
fn 修正前の形を名指しできる() {
    let src = read(MAIN);
    let replace_once = |from: &str, to: &str| -> String {
        assert!(
            src.contains(from),
            "注入の起点 `{from}` が {MAIN} に無い（番犬を今の形へ直す）"
        );
        src.replacen(from, to, 1)
    };

    // (a) 修正前の viewport-lines: 溢れる引き算
    let a = replace_once(
        "let visible = base.count;",
        "let visible = base.count;\n            let want = visible - FOLLOW_MARGIN;",
    );
    let hits = fixed_margin_subtractions(MAIN, &a);
    assert!(
        hits.iter().any(|h| h.contains("visible - FOLLOW_MARGIN")),
        "(a) 溢れる引き算を名指せない: {hits:?}"
    );

    // (b) 修正前の editor-font: 溢れを隠す saturating_sub
    let b = replace_once(
        "let (want, margin) = match follow_design(visible) {",
        "let _old = visible.saturating_sub(FOLLOW_MARGIN);\n            \
         let (want, margin) = match follow_design(visible) {",
    );
    assert!(
        !fixed_margin_subtractions(MAIN, &b).is_empty(),
        "(b) saturating_sub(FOLLOW_MARGIN) を名指せない"
    );

    // (c) 設計値の 1 実装が詰めた余白を見ない
    let c = replace_once(
        "effective_margin(visible, FOLLOW_MARGIN)",
        "FOLLOW_MARGIN.min(visible)",
    );
    assert!(
        design_routing(&c)
            .iter()
            .any(|p| p.contains("`effective_margin` を通っていない")),
        "(c) effective_margin を外した形を名指せない"
    );

    // (d) 判定が設計値の 1 実装を通らない（judge_down が自前で引き直す）
    let d = replace_once(
        "let (want, margin) = match follow_design(visible) {\n                Ok(design) => design,",
        "let (want, margin) = match old_design(visible) {\n                Ok(design) => design,",
    );
    assert!(
        design_routing(&d)
            .iter()
            .any(|p| p.contains("`judge_down` が `follow_design` を通っていない")),
        "(d) judge_down の引き直しを名指せない: {:?}",
        design_routing(&d)
    );

    // (e) fixture の拡張子を言語サーバの受け持つ `.rs` へ戻す
    let e = replace_once(
        "const VLINES_FIXTURE_EXT: &str = \"txt\";",
        "const VLINES_FIXTURE_EXT: &str = \"rs\";",
    );
    assert!(
        fixture_ext_problems(&e)
            .iter()
            .any(|p| p.contains("rust-analyzer")),
        "(e) `.rs` の fixture を名指せない: {:?}",
        fixture_ext_problems(&e)
    );
}
