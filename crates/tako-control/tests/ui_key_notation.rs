//! UI 文言の打鍵表記の番犬（Issue #1203）。
//!
//! **何を止めるか**: 画面 / CLI / MCP へ出す文章に `⌘K` / `Cmd+Enter` のような
//! macOS のキー表記を**直書き**すること。GPUI の `cmd` は platform 修飾で、
//! Windows では Win キーへ解決される（#585）。`Win+K` は OS のキャストが開くので、
//! 書いてあるとおりに押すと**別のことが起きる** = 案内として有害になる。
//!
//! リポジトリ自身が `crates/tako-app/src/keybindings.rs` に
//! 「非 macOS で `cmd-` のバインドを案内してはいけない」と書いてあるのに、
//! 2026-09-09 の Windows 実機レビューで 6 か所の直書きが見つかった（#1203）。
//! 仕組み（`shortcut_hint_for` / `platform::keys`）はあるので、**通し忘れ**を
//! ここで落とす。
//!
//! # 検査の形
//!
//! ソースからコメントを落として、残り（= コード）に macOS 固有の打鍵表記が
//! 現れないことを見る。コメントは対象外（`⌘+クリックで開く` のような説明は
//! 実装の意図を書くのに要るし、画面には出ない）。
//!
//! 「実際に画面へ出る文字列」側の検査は `tako-app` の
//! `keybindings::tests::windowsの案内文にmacosのキー表記が出ない` が持つ。
//! **2 本立てなのは片方だけでは穴が残るから**: こちらは新しい直書きを未然に止め、
//! あちらは組み上がった文言が Windows で正しいことを見る。

use std::path::{Path, PathBuf};

/// リポジトリルート（`crates/tako-control` から 2 つ上）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// 検査対象（規則 A）。**画面 / CLI / MCP に出る文章を組む場所**。
///
/// `crates/tako-app/src` を丸ごと入れないのは、そこにセルフテストの項目名
/// （`check(cond, "visual-test md: ⌘C 相当のコピーで…")`）が大量にあるため。
/// あれは診断文で画面には出ない（`.agent/conventions.md`「UI 文字列の i18n」でも
/// 診断ログは対象外）。tako-app の render 直書きは規則 B が別に見る
const SCAN_DIRS: &[&str] = &[
    "crates/tako-app/src/ui_text",
    "crates/tako-cli/src",
    "crates/tako-control/src",
    "crates/tako-core/src",
];

/// 検査対象（規則 B）。render コードへ直書きされた文言
const RENDER_DIR: &str = "crates/tako-app/src";

/// 文言を画面へ渡す呼び出し。この行に文字列リテラルがあれば「render 直書き」
const RENDER_CALLS: &[&str] = &[".child(", ".placeholder(", ".tooltip(", ".label("];

/// 直書きを許す場所（**打鍵表記そのものを組み立てる側**）。
///
/// ここを増やすときは「なぜそのファイルが表記の正本側なのか」を必ず添える。
/// 案内文を書くファイルは絶対に足さない（足した瞬間に #1203 が再発する）。
const ALLOWED: &[(&str, &str)] = &[
    (
        "crates/tako-core/src/platform/keys.rs",
        "案内文へ載せる打鍵表記の正本。プラットフォーム別の文字列はここだけが持つ",
    ),
    (
        "crates/tako-control/src/remote_preview.rs",
        "claude の TUI から採ったフッター（`⏎ send ⌃J newline`）のテスト fixture。\
         tako の文言ではないので直せない・直してはいけない",
    ),
];

/// macOS 固有のキー表記。**エスケープ形（`\u{2318}`）も同じ扱い**にする
/// （リテラルだけ見張ると `"\u{2318}K"` と書いて素通りできてしまう）
const FORBIDDEN: &[(&str, &str)] = &[
    ("\u{2318}", "command 記号"),
    ("\\u{2318}", "command 記号（エスケープ形）"),
    ("\u{2325}", "option 記号"),
    ("\\u{2325}", "option 記号（エスケープ形）"),
    ("\u{2303}", "control 記号"),
    ("\\u{2303}", "control 記号（エスケープ形）"),
    ("\u{21e7}", "shift 記号"),
    ("\\u{21e7}", "shift 記号（エスケープ形）"),
    ("Cmd+", "Cmd 表記"),
    ("Cmd-", "Cmd 表記"),
];

/// Rust ソースからコメント（`//` 行コメント / `/* */` ブロックコメント）を落とす。
///
/// 文字列リテラルの中の `//`（`"https://…"`）をコメントと誤認しないよう、
/// 素朴に「いま文字列の中か」を追う。生文字列（`r"…"` / `r#"…"#`）も扱う。
/// 落とした部分は空白へ置き換えるので、行番号と桁はずれない
fn strip_comments(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        // 行コメント
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        // ブロックコメント（入れ子対応）
        if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 1usize;
            out.push(' ');
            out.push(' ');
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else {
                    out.push(if b[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
            continue;
        }
        // 生文字列 r"…" / r#"…"#
        if c == 'r' {
            let mut j = i + 1;
            let mut hashes = 0usize;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                // 直前が識別子の一部（`char` の r 等）でないこと
                let prev_ident = i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_');
                if !prev_ident {
                    out.extend_from_slice(&b[i..=j]);
                    i = j + 1;
                    loop {
                        if i >= b.len() {
                            break;
                        }
                        if b[i] == '"' {
                            let mut closing = 0usize;
                            while b.get(i + 1 + closing) == Some(&'#') && closing < hashes {
                                closing += 1;
                            }
                            if closing == hashes {
                                out.extend_from_slice(&b[i..=(i + hashes)]);
                                i += hashes + 1;
                                break;
                            }
                        }
                        out.push(b[i]);
                        i += 1;
                    }
                    continue;
                }
            }
        }
        // 通常の文字列
        if c == '"' {
            out.push(c);
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    out.push(b[i]);
                    if i + 1 < b.len() {
                        out.push(b[i + 1]);
                    }
                    i += 2;
                    continue;
                }
                out.push(b[i]);
                i += 1;
                if b[i - 1] == '"' {
                    break;
                }
            }
            continue;
        }
        // 文字リテラル（`'"'` が文字列開始に見えるのを避ける。ライフタイムは素通し）
        if c == '\'' {
            if b.get(i + 1) == Some(&'\\') {
                let mut j = i + 2;
                while j < b.len() && b[j] != '\'' && b[j] != '\n' {
                    j += 1;
                }
                if b.get(j) == Some(&'\'') {
                    out.extend_from_slice(&b[i..=j]);
                    i = j + 1;
                    continue;
                }
            } else if b.get(i + 2) == Some(&'\'') {
                out.extend_from_slice(&b[i..=(i + 2)]);
                i += 3;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out.into_iter().collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 検出した違反 1 件（ファイル:行: 何）
fn scan(path: &Path, root: &Path, render_only: bool, hits: &mut Vec<String>) {
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    // 早期スキップ（大半のファイルはここで抜ける）
    if !FORBIDDEN.iter().any(|(needle, _)| src.contains(needle)) {
        return;
    }
    let code = strip_comments(&src);
    for (lineno, line) in code.lines().enumerate() {
        if render_only && !RENDER_CALLS.iter().any(|c| line.contains(c)) {
            continue;
        }
        for (needle, what) in FORBIDDEN {
            if line.contains(needle) {
                let rel = path.strip_prefix(root).unwrap_or(path).display();
                hits.push(format!("{rel}:{}: {what} `{needle}`", lineno + 1));
            }
        }
    }
}

/// 規則 A: 画面 / CLI / MCP に出る文章を組む場所へ macOS の打鍵表記を直書きしない
#[test]
fn ui文言にmacosのキー表記を直書きしていない() {
    let root = repo_root();
    let allowed: Vec<PathBuf> = ALLOWED.iter().map(|(p, _)| root.join(p)).collect();
    let mut files = Vec::new();
    for d in SCAN_DIRS {
        rust_files(&root.join(d), &mut files);
    }
    assert!(
        files.len() > 50,
        "走査対象が少なすぎる（パスの解決に失敗している）: {}",
        files.len()
    );

    let mut hits: Vec<String> = Vec::new();
    let mut allowed_seen = 0usize;
    for f in &files {
        if allowed.contains(f) {
            allowed_seen += 1;
            continue;
        }
        scan(f, &root, false, &mut hits);
    }

    assert_eq!(
        allowed_seen,
        ALLOWED.len(),
        "許可リストのパスが実在しない（リネーム漏れ）: {ALLOWED:?}"
    );
    assert!(
        hits.is_empty(),
        "案内文に macOS のキー表記が直書きされている（Windows では別の動作をする / 存在しない）:\n  {}\n\
         → 打鍵は `tako_core::platform::keys`（案内文）か \
         `keybindings::shortcut_hint_for`（バインド由来）から引いてください。\n\
         実装の意図を書くコメントは対象外なので、案内文ではなくコメントに書くのも可。",
        hits.join("\n  ")
    );
}

/// 規則 B: GPUI の render コードへ打鍵表記を直書きしない
/// （#1203 のタブバー `⌘K` バッジがこの形だった。`ui_text` を経由しないので規則 A では拾えない）
#[test]
fn renderコードにmacosのキー表記を直書きしていない() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join(RENDER_DIR), &mut files);
    assert!(
        files.len() > 20,
        "走査対象が少なすぎる（パスの解決に失敗している）: {}",
        files.len()
    );
    let mut hits: Vec<String> = Vec::new();
    for f in &files {
        scan(f, &root, true, &mut hits);
    }
    assert!(
        hits.is_empty(),
        "render コードへ macOS のキー表記が直書きされている:\n  {}\n\
         → 文言は `ui_text` へ出し、打鍵は `tako_core::platform::keys` か \
         `keybindings::shortcut_hint_for` から引いてください。",
        hits.join("\n  ")
    );
}

/// 番犬自身の検出力（コメントを落とす処理が効きすぎて何も見なくなっていないこと）
#[test]
fn コメントだけを落として文字列は残す() {
    let src = r#"
// ⌘K はコメントなので対象外
/// ドキュメントコメントの ⌘S も対象外
/* ブロックコメントの Cmd+Enter も対象外 */
let url = "https://example.com//path"; // 文字列内の // をコメント扱いしない
let hint = "⌘K";
"#;
    let code = strip_comments(src);
    assert!(
        !code.contains("\u{2318}K\u{20}はコメント"),
        "行コメントが残っている: {code}"
    );
    assert!(!code.contains("Cmd+Enter"), "ブロックコメントが残っている");
    assert!(
        code.contains("https://example.com//path"),
        "文字列の中身まで落としている: {code}"
    );
    assert!(
        code.contains("let hint = \"\u{2318}K\""),
        "コードの文字列リテラルが落ちている: {code}"
    );
    // ドキュメントコメントの ⌘S は落ちて、リテラルの ⌘K だけが残る
    assert_eq!(
        code.matches('\u{2318}').count(),
        1,
        "落とし残し / 落としすぎ: {code}"
    );
}
