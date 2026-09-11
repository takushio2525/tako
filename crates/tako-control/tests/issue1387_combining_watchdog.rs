//! 番犬: 0 幅の結合文字を読むのは 1 実装だけ / 画面テキストの 3 経路がそこを通る（#1387）
//!
//! ## なぜ止めるのか
//!
//! alacritty は 0 幅の結合文字（NFD の濁点 `U+3099`・アクセント `U+0301` 等）を
//! `Cell::c` ではなく `Cell::extra.zerowidth` に持つ。`c` だけを読むと結合文字が
//! 画面テキストから**黙って落ちる**。修正前は落ちる実装が 3 つ在り
//! （`screen::resolve_cell` / `TerminalSession::compose_grid_row` /
//! `TerminalSession::history_plain_lines`）、**選択コピーだけ**が alacritty 自身の
//! `selection_to_string` を通っていたため、同じセルの内容が
//! 「見た目 / `read_pane` / ペインログ」と「Cmd+C」で食い違っていた。
//!
//! 実害（macOS で実測。#1387）: NFD のファイル名 `画像か\u{3099}ある.txt` が
//! ペインに出ると濁点が消えて見え、`links` は**実在しないパス**
//! （`画像かある.txt`）を見るので Cmd+クリックが無反応になる
//! （#153 / #1283 で通した経路がここで死ぬ）。ペインログ / `read_pane` も
//! 端末が実際に受け取ったバイト列と違う文字列を記録する。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 1. [`zerowidthを読む実装は1箇所だけ`] — 製品コードで `zerowidth` を読むのは
//!    `screen::cell_text` の 1 行だけ。読む側が増えると、片方だけ結合文字を
//!    落とす形へ戻る（#651 / #875 でマーカーの読み手を 1 本へ寄せたのと同じ理由）
//! 2. [`画面テキストを組む3経路は1実装を通す`] — 3 経路が `cell_text` /
//!    `push_cell_text` を通し、`cell.c` を直接積む形（修正前そのもの）へ戻っていないこと
//! 3. [`compose_lineは結合文字ぶんもcell_colsへ積む`] — `text` へ足した結合文字は
//!    `cell_cols` へ**同じ列**を積む。積まないと描画のセル写像と links のスパンが
//!    1 文字ずつずれる
//! 4. [`is_plain_blankは結合文字を近道から外す`] — #801 の空白セルの近道が
//!    「結合文字を載せた空白セル」を飛ばさないこと（行頭の単独アクセントは
//!    `' '` のセルへ載るので、素の空白扱いにすると落ちる）
//!
//! 挙動そのものは `tako-core` の単体 / 実 PTY のテストが見る
//! （`screen::tests::nfdの結合文字はtextに残りcell_colsへ同じ列を積む` /
//! `terminal::tests::結合文字は画面テキストの全経路に残る` /
//! `links::tests::nfdのファイル名が画面からリンクになる_1387`）。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// テストモジュールより手前だけを見る（fixture や期待値の文字列に当たらない）。
/// **`mod tests` の直前で切る**（`#[cfg(test)]` はテスト用ヘルパにも付く）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
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

fn rel_of(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .expect("リポジトリ内")
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

// ---------------------------------------------------------------- 1 実装の位置

/// `zerowidth` を読んでよい場所（file の相対パス, **その行に必ず在る目印**）。
/// 目印は行ごとなので、**同じファイルに別の読み手が生えたら落ちる**
const ALLOWED_READERS: &[(&str, &str)] = &[(
    "crates/tako-core/src/screen.rs",
    "combining: cell.zerowidth()",
)];

/// `rel` の製品コードで「許されていない `zerowidth` の直読み」を file:line で並べる
fn offending_readers(rel: &str, src: &str) -> Vec<String> {
    let allowed: Vec<&str> = ALLOWED_READERS
        .iter()
        .filter(|(f, _)| *f == rel)
        .map(|(_, mark)| *mark)
        .collect();
    production_source(src)
        .lines()
        .enumerate()
        .filter(|(_, line)| !is_comment(line))
        .filter(|(_, line)| line.contains("zerowidth"))
        .filter(|(_, line)| !allowed.iter().any(|mark| line.contains(mark)))
        .map(|(i, line)| format!("{rel}:{}: {}", i + 1, line.trim()))
        .collect()
}

#[test]
fn zerowidthを読む実装は1箇所だけ() {
    let mut found = Vec::new();
    let mut allowed_hits = 0usize;
    for path in production_files() {
        let rel = rel_of(&path);
        let src = std::fs::read_to_string(&path).expect("ソースを読む");
        if !src.contains("zerowidth") {
            continue;
        }
        if ALLOWED_READERS.iter().any(|(f, _)| *f == rel) {
            allowed_hits += 1;
        }
        found.extend(offending_readers(&rel, &src));
    }
    assert_eq!(
        allowed_hits,
        ALLOWED_READERS.len(),
        "許可した置き場が消えている（`screen::cell_text` を改名したら番犬も直す）"
    );
    assert!(
        found.is_empty(),
        "0 幅の結合文字を読む実装が増えている（読む側は `screen::cell_text` の 1 実装へ\n\
         寄せる。テキストを組む側は `screen::push_cell_text` を通す）:\n{}",
        found.join("\n")
    );
}

#[test]
fn i1387_注入_zerowidthの直読みを名指しで落とせる() {
    // 修正前の形（各経路が自分で `cell.c` と zerowidth を触る）が生えたら落ちる
    let injected = "fn compose(cell: &Cell, out: &mut String) {\n    \
        out.push(cell.c);\n    \
        for c in cell.zerowidth().unwrap_or(&[]) { out.push(*c); }\n}\n";
    let got = offending_readers("crates/tako-core/src/other.rs", injected);
    assert_eq!(got.len(), 1, "注入を拾えていない: {got:?}");
    assert!(got[0].contains(":3:"), "行番号が合っていない: {got:?}");

    // 許可した目印は「同じファイル」でしか効かない（別ファイルへ写したら落ちる）
    let moved = "        combining: cell.zerowidth().unwrap_or(&[]),\n";
    assert_eq!(
        offending_readers("crates/tako-app/src/main.rs", moved).len(),
        1,
        "1 実装を別ファイルへ写したのに通った"
    );
    assert!(
        offending_readers("crates/tako-core/src/screen.rs", moved).is_empty(),
        "許可した置き場を落としている"
    );

    // コメントとテストモジュールは対象外（番犬自身の説明文や fixture を拾わない）
    assert!(offending_readers("crates/x/src/a.rs", "// cell.zerowidth()\n").is_empty());
    let in_tests = "fn f() {}\n#[cfg(test)]\nmod tests {\n    cell.zerowidth();\n}\n";
    assert!(offending_readers("crates/x/src/a.rs", in_tests).is_empty());
}

// ---------------------------------------------------------------- 3 経路の配線

/// 切り出した関数本体（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: String,
    name: String,
    line: usize,
    text: String,
}

/// `<indent>[pub ]fn <name>(` から同じインデントの閉じ括弧までを本体として切り出す
/// （`rustfmt` がこの形を保証する）。ライフタイム引数つき（`fn f<'a>(`）も拾う
fn body_of(rel: &str, src: &str, name: &str) -> Body {
    let lines: Vec<&str> = production_source(src).lines().collect();
    let head = format!("fn {name}");
    let declares = |l: &str| {
        let t = l.trim_start();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
        t.strip_prefix(&head)
            .is_some_and(|rest| rest.starts_with('(') || rest.starts_with('<'))
    };
    let start = lines
        .iter()
        .position(|l| declares(l))
        .unwrap_or_else(|| panic!("{rel} の fn {name} が見つからない（改名したら番犬も直す）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let mut end = start + 1;
    while end < lines.len() && lines[end] != close {
        end += 1;
    }
    Body {
        rel: rel.to_string(),
        name: name.to_string(),
        line: start + 1,
        text: lines[start..=end.min(lines.len() - 1)].join("\n"),
    }
}

/// 違反の名指し（`file:line fn 名: 理由`）
fn at(body: &Body, offset: usize, why: &str) -> String {
    format!(
        "{}:{} fn {}: {why}",
        body.rel,
        body.line + offset,
        body.name
    )
}

/// 画面テキストを組む 3 経路（file, 関数名）
const TEXT_PATHS: &[(&str, &str)] = &[
    ("crates/tako-core/src/screen.rs", "resolve_cell"),
    ("crates/tako-core/src/terminal.rs", "compose_grid_row"),
    ("crates/tako-core/src/terminal.rs", "history_plain_lines"),
];

/// 修正前そのものの形（セルの本体文字を自分で積む）
const RAW_PUSH: [&str; 2] = ["push(cell.c)", "text.push(cell.c)"];

fn offending_paths() -> Vec<String> {
    let mut offenders = Vec::new();
    for (rel, name) in TEXT_PATHS {
        let body = body_of(rel, &read(rel), name);
        let code: Vec<(usize, &str)> = body
            .text
            .lines()
            .enumerate()
            .filter(|(_, l)| !is_comment(l))
            .collect();
        let joined = code.iter().map(|(_, l)| *l).collect::<Vec<_>>().join("\n");
        if !joined.contains("cell_text(") && !joined.contains("push_cell_text(") {
            offenders.push(at(
                &body,
                0,
                "結合文字を読む 1 実装（`screen::cell_text` / `screen::push_cell_text`）を\
                 通していない",
            ));
        }
        for (n, line) in &code {
            if let Some(pat) = RAW_PUSH.iter().find(|p| line.contains(**p)) {
                offenders.push(at(
                    &body,
                    *n,
                    &format!("セルの本体文字を自分で積んでいる（`{pat}`）= 結合文字が落ちる"),
                ));
            }
        }
    }
    offenders
}

#[test]
fn 画面テキストを組む3経路は1実装を通す() {
    let offenders = offending_paths();
    assert!(
        offenders.is_empty(),
        "画面テキストを組む経路は `screen::cell_text` / `screen::push_cell_text` を通す\n\
         （3 経路のうち 1 つでも `cell.c` だけを読むと、その経路だけ結合文字が落ちて\n\
         「見た目 / read_pane / ペインログ」と「Cmd+C」が食い違う = #1387）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn compose_lineは結合文字ぶんもcell_colsへ積む() {
    let rel = "crates/tako-core/src/screen.rs";
    let body = body_of(rel, &read(rel), "compose_line");
    let mut offenders = Vec::new();
    // 結合文字を積むループ（`for z in zero_width {` 〜 同じインデントの閉じ）
    let lines: Vec<&str> = body.text.lines().collect();
    let head = lines
        .iter()
        .position(|l| l.trim_start().starts_with("for z in zero_width {"));
    match head {
        None => offenders.push(at(
            &body,
            0,
            "結合文字を `text` へ積むループが無い（`zero_width` を捨てている）",
        )),
        Some(head) => {
            let indent = " ".repeat(lines[head].len() - lines[head].trim_start().len());
            let close = format!("{indent}}}");
            let mut end = head + 1;
            while end < lines.len() && lines[end] != close {
                end += 1;
            }
            let loop_body = lines[head..end.min(lines.len())].join("\n");
            if !loop_body.contains("cell_cols.push(col)") {
                offenders.push(at(
                    &body,
                    head,
                    "結合文字を `text` へ積みながら `cell_cols` へ同じ列を積んでいない\
                     （描画のセル写像と links のスパンが 1 文字ずつずれる）",
                ));
            }
            if !loop_body.contains("text.push(*z)") {
                offenders.push(at(&body, head, "結合文字を `text` へ積んでいない"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`compose_line` は結合文字を `text` と `cell_cols` の**両方**へ積む（#1387）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn is_plain_blankは結合文字を近道から外す() {
    let rel = "crates/tako-core/src/screen.rs";
    let body = body_of(rel, &read(rel), "is_plain_blank");
    let code = body
        .text
        .lines()
        .filter(|l| !is_comment(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("combining.is_empty()"),
        "#801 の空白セルの近道が「結合文字を載せた空白セル」を飛ばしている\n\
         （行頭の単独アクセントは `' '` のセルへ載るので、素の空白扱いにすると落ちる）:\n  {}",
        at(&body, 0, "結合文字の有無を見ていない")
    );
}
