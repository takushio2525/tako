//! リポジトリの Markdown の表で、列数が見出しと食い違う行を落とす番犬（Issue #2051）。
//!
//! # なぜ要るか
//!
//! GFM の表は行を**エスケープされていない `|`** で割る。表の行は inline の解析より前に割るので、
//! **コードスパンの中の `|` も区切りになる**（GFM の仕様）。`` `tako mouse [on|off]` `` を
//! セルへそのまま書くと見出しより列が多い行になり、GitHub・docs サイト（remark-gfm）・
//! tako のプレビュー（pulldown-cmark）のどれもが**はみ出したセルを捨てる** = コマンドの後半や
//! 行の残りが描かれない。#2051 の時点で `AGENTS.md` のコマンド表 22 行・`.agent/commands.md`
//! 10 行・`.agent/requirements.md` 23 行がこの形だった。
//!
//! 書くときは `\|` にする（上の 3 つの描画系はどれも表の中の `\|` を `|` として描き、
//! コードスパンの中でも同じ。素のテキストを読む AI にも `on\|off` は「on か off」と読める）。
//! 列が**足りない**行は空セルで埋めて描かれるので見た目は崩れないが、どの列が抜けたのか
//! （FR-6 の表の「状態」・FR-3 の表の「優先度」）が読めなくなるので、同じ番犬で揃える。
//!
//! # 走査の範囲
//!
//! git が追跡しているか、未追跡で ignore もされていない `*.md` / `*.mdx` の全部
//! （`AGENTS.md`・`.agent/`・docs サイト・CHANGELOG・テンプレ・system prompt の手順書）。
//! `fixtures/` の下だけ外す（描画のテストが崩れた表を意図して持てるように）。
//! git が動かなければ**落とす**（0 件の走査で緑にしない）。
//!
//! # 見逃す側へ倒れないための作り
//!
//! - [`崩れた行を差し込むと_file_line_で名指しする`] — 実物の `AGENTS.md` のコマンド表へ
//!   #2051 と同じ形の行を差し込み、その行番号で名指しされることを確かめる
//! - [`走査が空振りしていない`] — `AGENTS.md` のコマンド表が 100 行以上の 2 列の表として採れ、
//!   走査したファイルと表が一定数あることを確かめる。判定が壊れて表を 1 つも採れなくなると
//!   本体の検査は無条件に緑になるので、そこを塞ぐ
//! - [`gfm_の割り方を固定する`] — コードスパンでも割る・`\|` は割らない・先頭と末尾の `|` は
//!   省ける・フェンスの中は表でない・空行で終わる・続く段落の行は 1 列の行として吸われる

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 表の 1 行を GFM と同じ規則で割ったときのセル数。
///
/// 前後の空白と、先頭の `|`・末尾の（エスケープされていない）`|` を落としてから、
/// 直前が `\` でない `|` を数える。コードスパンは見ない（GFM は inline より先に割る）。
fn cell_count(line: &str) -> usize {
    let mut s = line.trim();
    if let Some(rest) = s.strip_prefix('|') {
        s = rest;
    }
    if s.ends_with('|') && !s.ends_with("\\|") {
        s = &s[..s.len() - 1];
    }
    let mut count = 1;
    let mut prev = '\0';
    for c in s.chars() {
        if c == '|' && prev != '\\' {
            count += 1;
        }
        prev = c;
    }
    count
}

/// 行にエスケープされていない `|` が 1 つでもあるか
fn has_bare_pipe(line: &str) -> bool {
    let mut prev = '\0';
    for c in line.chars() {
        if c == '|' && prev != '\\' {
            return true;
        }
        prev = c;
    }
    false
}

/// 区切り行（`|---|:--:|`）か。セルは `:?-+:?` だけ・`|` を少なくとも 1 つ含む
/// （`---` 単独は区切り線か setext 見出しで、表ではない）
fn is_delimiter_row(line: &str) -> bool {
    let t = line.trim();
    if !has_bare_pipe(t) {
        return false;
    }
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').all(|cell| {
        let c = cell.trim();
        let c = c.strip_prefix(':').unwrap_or(c);
        let c = c.strip_suffix(':').unwrap_or(c);
        !c.is_empty() && c.chars().all(|ch| ch == '-')
    })
}

/// フェンス（```` ``` ```` / `~~~`）の開始・終了の記号列（3 個以上）
fn fence_marker(line: &str) -> Option<&str> {
    let t = line.trim_start();
    for ch in ['`', '~'] {
        let n = t.chars().take_while(|c| *c == ch).count();
        if n >= 3 {
            return Some(&t[..n]);
        }
    }
    None
}

/// 表を終わらせる行（空行・別のブロックの始まり）。これ以外の行は、`|` が無くても
/// 表の 1 行として吸われる（GFM: 表の直後に続けた段落の行は 1 列の行になる）
fn ends_table(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() || fence_marker(line).is_some() {
        return true;
    }
    if t.starts_with('#') || t.starts_with('>') || t.starts_with('<') {
        return true;
    }
    if ["- ", "* ", "+ "].iter().any(|m| t.starts_with(m)) {
        return true;
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") "))
}

/// 採れた表 1 つ（見出しの行番号・列数・本体の行数）
#[derive(Debug)]
struct Table {
    header_line: usize,
    columns: usize,
    rows: usize,
}

/// 食い違い 1 件（`file:line` で名指しする材料）
#[derive(Debug)]
struct Mismatch {
    line: usize,
    expected: usize,
    actual: usize,
    text: String,
}

/// 1 ファイルの表を採り、見出しと列数が食い違う行を返す。
/// 見出しと区切り行の列数が違う（= 表として認識されない）ものも食い違いとして返す
fn scan(text: &str) -> (Vec<Table>, Vec<Mismatch>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut tables = Vec::new();
    let mut bad = Vec::new();
    let mut fence: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(open) = &fence {
            if fence_marker(line).is_some_and(|m| m.starts_with(open.as_str())) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if let Some(m) = fence_marker(line) {
            fence = Some(m.to_string());
            i += 1;
            continue;
        }
        let starts_table = has_bare_pipe(line)
            && !ends_table(line)
            && lines.get(i + 1).is_some_and(|next| is_delimiter_row(next));
        if !starts_table {
            i += 1;
            continue;
        }
        let columns = cell_count(line);
        let delimiter = cell_count(lines[i + 1]);
        if delimiter != columns {
            bad.push(Mismatch {
                line: i + 2,
                expected: columns,
                actual: delimiter,
                text: lines[i + 1].to_string(),
            });
        }
        let mut table = Table {
            header_line: i + 1,
            columns,
            rows: 0,
        };
        let mut j = i + 2;
        while j < lines.len() && !ends_table(lines[j]) {
            let actual = cell_count(lines[j]);
            if actual != columns {
                bad.push(Mismatch {
                    line: j + 1,
                    expected: columns,
                    actual,
                    text: lines[j].to_string(),
                });
            }
            table.rows += 1;
            j += 1;
        }
        tables.push(table);
        i = j;
    }
    (tables, bad)
}

/// 走査する md の一覧（リポジトリ相対・`/` 区切り）。git が動かなければ落とす
fn markdown_files(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "*.md",
            "*.mdx",
        ])
        .output()
        .unwrap_or_else(|e| panic!("git ls-files を起動できない（走査を 0 件で緑にしない）: {e}"));
    assert!(
        out.status.success(),
        "git ls-files が失敗した（走査を 0 件で緑にしない）: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|p| !p.split('/').any(|seg| seg == "fixtures"))
        .collect();
    files.sort();
    files.dedup();
    files
}

/// 食い違いを `file:line` つきの 1 行にする（行の本文は長いので先頭だけ）
fn describe(file: &str, m: &Mismatch) -> String {
    let head: String = m.text.chars().take(80).collect();
    let more = if m.text.chars().count() > 80 {
        "…"
    } else {
        ""
    };
    let why = if m.actual > m.expected {
        "はみ出したセルは描かれない。セルの中の `|` はコードスパンの中でも `\\|` にする"
    } else {
        "足りない列は空で描かれる。抜けた列を空セル（`|  |`）で揃える"
    };
    format!(
        "{file}:{}: 見出しは {} 列なのにこの行は {} 列（{why}）: {head}{more}",
        m.line, m.expected, m.actual
    )
}

#[test]
fn mdの表の列数が見出しと一致する() {
    let root = repo_root();
    let mut problems = Vec::new();
    for file in markdown_files(&root) {
        let path = root.join(&file);
        // 一覧にあっても作業ツリーから消した直後のものは読めない（削除の途中）
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for m in scan(&text).1 {
            problems.push(describe(&file, &m));
        }
    }
    assert!(
        problems.is_empty(),
        "Markdown の表で列数が見出しと食い違う行が {} 件ある（#2051）:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn 崩れた行を差し込むと_file_line_で名指しする() {
    let root = repo_root();
    let text = std::fs::read_to_string(root.join("AGENTS.md")).expect("AGENTS.md");
    assert!(
        scan(&text).1.is_empty(),
        "前提: 現行の AGENTS.md に食い違いが無い"
    );
    // コマンド表の最初の本体の行の直後へ、#2051 の修正前と同じ形（コードスパンの中の素の `|`）を差し込む
    let header = text
        .lines()
        .position(|l| l.starts_with("| 操作 | コマンド |"))
        .expect("AGENTS.md のコマンド表の見出し");
    let injected_at = header + 3; // 見出し + 区切り + 本体 1 行の次（0 起点）
    let mut lines: Vec<&str> = text.lines().collect();
    lines.insert(injected_at, "| 注入（#2051） | `tako mouse [on|off]` |");
    let broken = lines.join("\n");
    let bad = scan(&broken).1;
    assert_eq!(bad.len(), 1, "差し込んだ 1 行だけを名指しする: {bad:?}");
    assert_eq!(
        bad[0].line,
        injected_at + 1,
        "行番号は 1 起点で差し込んだ行"
    );
    assert_eq!((bad[0].expected, bad[0].actual), (2, 3));
    let msg = describe("AGENTS.md", &bad[0]);
    assert!(
        msg.starts_with(&format!("AGENTS.md:{}: ", injected_at + 1)),
        "file:line で始まる: {msg}"
    );

    // `\|` に直すと通る（直し方の案内どおりで緑になる）
    lines[injected_at] = "| 注入（#2051） | `tako mouse [on\\|off]` |";
    assert!(scan(&lines.join("\n")).1.is_empty());

    // 列が足りない行（FR-3.44 の修正前の形）も名指しする
    lines[injected_at] = "| 注入（#2051） |";
    let short = scan(&lines.join("\n")).1;
    assert_eq!(short.len(), 1);
    assert_eq!((short[0].expected, short[0].actual), (2, 1));
}

#[test]
fn 走査が空振りしていない() {
    let root = repo_root();
    let files = markdown_files(&root);
    assert!(
        files.len() >= 100,
        "走査した md が少なすぎる（{} 件）: 一覧の採り方が壊れていないか",
        files.len()
    );
    assert!(files.iter().any(|f| f == "AGENTS.md"));
    assert!(files.iter().any(|f| f == ".agent/requirements.md"));
    assert!(files.iter().any(|f| f.starts_with("docs/src/content/")));

    let text = std::fs::read_to_string(root.join("AGENTS.md")).expect("AGENTS.md");
    let (tables, _) = scan(&text);
    let commands = tables
        .iter()
        .find(|t| text.lines().nth(t.header_line - 1) == Some("| 操作 | コマンド |"))
        .expect("AGENTS.md のコマンド表が表として採れる");
    assert_eq!(commands.columns, 2);
    assert!(
        commands.rows >= 100,
        "コマンド表の本体が {} 行しか採れていない",
        commands.rows
    );

    let total: usize = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(root.join(f)).ok())
        .map(|t| scan(&t).0.len())
        .sum();
    assert!(total >= 200, "採れた表が {total} 個しかない");
}

#[test]
fn gfm_の割り方を固定する() {
    // コードスパンの中でも割る / `\|` は割らない
    assert_eq!(cell_count("| a | `x|y` |"), 3);
    assert_eq!(cell_count("| a | `x\\|y` |"), 2);
    // 先頭と末尾の `|` は省ける。末尾の `\|` はセルの中身
    assert_eq!(cell_count("a | b"), 2);
    assert_eq!(cell_count("| a | b"), 2);
    assert_eq!(cell_count("| a | b \\|"), 2);
    assert_eq!(cell_count("| a | b \\||"), 2);
    // 区切り行
    assert!(is_delimiter_row("|---|:--:|--:|"));
    assert!(is_delimiter_row("--- | ---"));
    assert!(!is_delimiter_row("---"));
    assert!(!is_delimiter_row("| a | --- |"));

    // フェンスの中は表でない
    let fenced = "```\n| a | b |\n|---|---|\n| 1 | 2 | 3 |\n```\n";
    assert_eq!(scan(fenced).0.len(), 0);
    assert!(scan(fenced).1.is_empty());

    // 空行で終わる（後ろの段落は表の行ではない）
    let ended = "| a | b |\n|---|---|\n| 1 | 2 |\n\n本文 | 3 | 4\n";
    let (t, bad) = scan(ended);
    assert_eq!((t.len(), t[0].rows), (1, 1));
    assert!(bad.is_empty());

    // 空行を挟まずに続けた段落の行は 1 列の行として吸われる（#2051 で plans に 1 件あった形）
    let glued = "| a | b |\n|---|---|\n| 1 | 2 |\n続きの文\n";
    let bad = scan(glued).1;
    assert_eq!(bad.len(), 1);
    assert_eq!((bad[0].line, bad[0].actual), (4, 1));

    // 見出しと区切り行の列数が違う = 表として認識されない
    let header_mismatch = "| a | b | c |\n|---|---|\n";
    let bad = scan(header_mismatch).1;
    assert_eq!((bad.len(), bad[0].line), (1, 2));

    // 見出しや箇条書きは表を終わらせる
    let list_after = "| a | b |\n|---|---|\n| 1 | 2 |\n- 箇条書き\n## 見出し\n";
    assert!(scan(list_after).1.is_empty());
}
