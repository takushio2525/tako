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
//!
//! # 規則 C: clap のヘルプ（Issue #2854）
//!
//! 規則 A はコメントを落として見るので、**clap の doc comment**（そのまま `tako --help` の
//! 本文になる）への直書きが素通りしていた（#2844 の `confirm-close` の `cmd+W`、#2854 の
//! `tako edit` の `⌥← / ⌘⌫ / ⌘F`）。規則 C は clap の derive（`Parser` / `Subcommand` /
//! `Args` / `ValueEnum`）を付けた型の範囲だけを切り出し、その中の doc comment と属性の文字列を
//! 見る。普通の関数の doc comment（打鍵の表記の説明）は対象外のまま。組み上がったヘルプの
//! 側は tako-cli の `issue2854_editのヘルプは実行osの表の打鍵を出す` が Windows の CI で
//! ヘルプ全体を辿って見る

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

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

/// macOS 固有のキー記号（規則 A / B）。**エスケープ形（`\u{2318}`）も同じ扱い**にする
/// （リテラルだけ見張ると `"\u{2318}K"` と書いて素通りできてしまう）。エスケープ形は
/// 綴りではなく値で読む（`mark_hits`）ので、`\u{21E7}` / `\u{21e7}` / `\u{021e7}` のどれでも当たる
/// （#2867。それまでは小文字の綴りだけを探していて、大文字の `\u{21E7}` が素通りした）
const KEY_MARKS: &[(char, &str)] = &[
    ('\u{2318}', "command 記号"),
    ('\u{2325}', "option 記号"),
    ('\u{2303}', "control 記号"),
    ('\u{21e7}', "shift 記号"),
];

/// macOS 固有のキー表記の語形（規則 A / B）。綴りのまま見る（小文字の `cmd-up` は
/// GPUI のバインドの書式で、`editor_keys` の表などが使う）
const KEY_WORDS: &[(&str, &str)] = &[("Cmd+", "Cmd 表記"), ("Cmd-", "Cmd 表記")];

/// `text` に現れる `\u{…}` エスケープを（書かれた綴り, 表す文字）で返す。
///
/// 綴りの文字列一致ではなく値で読むので、大文字小文字・先頭の 0・`_` の区切りを問わない（#2867）。
/// 16 進として読めないもの（`format!` の `{}` の手前に偶然 `\u` があった等）は捨てる
fn unicode_escapes(text: &str) -> impl Iterator<Item = (String, char)> + '_ {
    text.match_indices("\\u{").filter_map(move |(at, _)| {
        let (digits, _) = text[at..].strip_prefix("\\u{")?.split_once('}')?;
        let hex: String = digits.chars().filter(|&c| c != '_').collect();
        let ch = u32::from_str_radix(&hex, 16)
            .ok()
            .and_then(char::from_u32)?;
        Some((format!("\\u{{{digits}}}"), ch))
    })
}

/// `text` に現れる記号を名前で返す（記号そのもの / `\u{…}` のエスケープ形。1 記号につき 1 件）。
/// 規則 A / B（`KEY_MARKS`）と規則 C（`HELP_MARKS`）が同じ照合を通る
fn mark_hits(text: &str, marks: &[(char, &str)]) -> Vec<String> {
    let escapes: Vec<(String, char)> = unicode_escapes(text).collect();
    let mut hits = Vec::new();
    for (mark, what) in marks {
        if text.contains(*mark) {
            hits.push(format!("{what} `{mark}`"));
        } else if let Some((written, _)) = escapes.iter().find(|(_, c)| c == mark) {
            hits.push(format!("{what}（エスケープ形） `{written}`"));
        }
    }
    hits
}

/// 規則 A / B の 1 行分（記号は `mark_hits`・語形は綴りのまま）
fn key_notation_in(line: &str) -> Vec<String> {
    let mut hits = mark_hits(line, KEY_MARKS);
    for (word, what) in KEY_WORDS {
        if line.contains(word) {
            hits.push(format!("{what} `{word}`"));
        }
    }
    hits
}

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
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    hits.extend(scan_key_notation(&src, &rel, render_only));
}

/// 規則 A / B の走査 1 ファイル分（`rel` は名指しに使うパス）
fn scan_key_notation(src: &str, rel: &str, render_only: bool) -> Vec<String> {
    // 早期スキップ（大半のファイルはここで抜ける）。行ごとと同じ照合で見るので、
    // ここだけ綴りに依って取りこぼすことはない
    if key_notation_in(src).is_empty() {
        return Vec::new();
    }
    let code = strip_comments(src);
    let mut hits = Vec::new();
    for (lineno, line) in code.lines().enumerate() {
        if render_only && !RENDER_CALLS.iter().any(|c| line.contains(c)) {
            continue;
        }
        for what in key_notation_in(line) {
            hits.push(format!("{rel}:{}: {what}", lineno + 1));
        }
    }
    hits
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

/// 規則 A / B の検出力: エスケープ形は綴り（大文字小文字・先頭の 0）を問わず値で拾い、
/// 隣の値・コメントの中は拾わない（#2867。大文字の `\u{21E7}` が素通りしていた）
#[test]
fn 規則aはエスケープ形を綴りを問わず拾う() {
    let src = r#"
// コメントの \u{21E7} は対象外
let a = "\u{21E7}Tab";
let b = "\u{21e7}Tab";
let c = "\u{021E7}Tab";
let d = "⇧Tab";
let e = "\u{2318}K";
let f = "\u{21E8} \u{2319}";
let g = "Cmd+K";
"#;
    let hits = scan_key_notation(src, "fixture.rs", false);
    let lines: Vec<&str> = hits
        .iter()
        .map(|h| h.split(": ").next().unwrap_or(""))
        .collect();
    assert_eq!(
        lines,
        [
            "fixture.rs:3", // 大文字のエスケープ形
            "fixture.rs:4", // 小文字のエスケープ形
            "fixture.rs:5", // 先頭の 0
            "fixture.rs:6", // 記号そのもの
            "fixture.rs:7", // command 記号のエスケープ形
            "fixture.rs:9", // 語形
        ],
        "{hits:#?}"
    );
    assert!(
        hits[0].ends_with("shift 記号（エスケープ形） `\\u{21E7}`"),
        "書かれた綴りのまま名指す: {hits:#?}"
    );
}

/// 規則 C の対象: この derive を付けた型のヘルプは doc comment と属性から組まれる
const CLAP_DERIVES: &[&str] = &["Parser", "Subcommand", "Args", "ValueEnum"];

/// 規則 C で見る macOS の打鍵記号（エスケープ形 `\u{2318}` / `\u{232B}` も規則 A と同じ
/// `mark_hits` で値で読む = 大文字小文字を問わない）。
/// 規則 A の 4 つに、ヘルプで書かれがちな削除・escape・return の記号を足した（#2854 の `⌘⌫`）
const HELP_MARKS: &[(char, &str)] = &[
    ('\u{2318}', "command 記号"),
    ('\u{2325}', "option 記号"),
    ('\u{2303}', "control 記号"),
    ('\u{21e7}', "shift 記号"),
    ('\u{232b}', "delete 記号"),
    ('\u{2326}', "forward delete 記号"),
    ('\u{238b}', "escape 記号"),
    ('\u{23ce}', "return 記号"),
];

/// 規則 C で見る語形（大文字小文字を問わない。直前が英数字・`-` なら語の一部として見ない =
/// `--cmd-file` のような引数名は当たらない）。#2844 の `cmd+W` がこの形
const HELP_WORDS: &[(&str, &str)] = &[
    ("cmd+", "cmd 表記"),
    ("cmd-", "cmd 表記"),
    ("command+", "command 表記"),
    ("option+", "option 表記"),
    ("opt+", "option 表記"),
];

/// `open` の位置の括弧に対応する閉じ括弧の位置（`code` は文字列とコメントを潰した眺め）
fn matching(code: &[u8], open_at: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &c) in code.iter().enumerate().skip(open_at) {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// clap の derive を付けた型の範囲（バイト範囲 `[start, end)`）。
///
/// `start` は直前の item の終わり（`}` / `;`）の後ろ = 型自身の doc comment と属性を含む。
/// `end` は型の本体の閉じ括弧の後ろ。コメントと文字列を潰した眺めで括弧を数えるので、
/// `format!("{}")` の波括弧や説明文の括弧で範囲がずれない
fn clap_item_spans(src: &str) -> Vec<(usize, usize)> {
    let code = code_view::code_view(src);
    let b = code.as_bytes();
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some(off) = code[from..].find("#[derive(") {
        let at = from + off;
        let paren = at + "#[derive".len();
        let Some(list_end) = matching(b, paren, b'(', b')') else {
            break;
        };
        from = list_end;
        let is_clap = code[paren + 1..list_end].split(',').any(|d| {
            let name = d.trim().rsplit("::").next().unwrap_or("");
            CLAP_DERIVES.contains(&name)
        });
        if !is_clap {
            continue;
        }
        // 型の頭（`struct 名前` / `enum 名前`）→ 本体の `{`（`;` なら本体の無い型）
        let head = ["struct ", "enum "]
            .iter()
            .filter_map(|kw| {
                code[list_end..]
                    .match_indices(kw)
                    .find(|(i, _)| {
                        let p = list_end + i;
                        p == 0 || !(b[p - 1].is_ascii_alphanumeric() || b[p - 1] == b'_')
                    })
                    .map(|(i, _)| list_end + i)
            })
            .min();
        let Some(head) = head else {
            continue;
        };
        let Some(open) = code[head..]
            .find(['{', ';'])
            .map(|i| head + i)
            .filter(|&i| b[i] == b'{')
        else {
            continue;
        };
        let Some(close) = matching(b, open, b'{', b'}') else {
            break;
        };
        let start = code[..at].rfind(['}', ';']).map_or(0, |p| p + 1);
        spans.push((start, close + 1));
        from = close + 1;
    }
    spans
}

/// ヘルプの 1 行に macOS の打鍵表記があれば、その名前を返す
fn help_marks_in(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut hits = mark_hits(text, HELP_MARKS);
    for (word, what) in HELP_WORDS {
        let as_word = lower.match_indices(word).any(|(i, _)| {
            let prev = lower[..i].chars().next_back();
            !prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
        if as_word {
            hits.push(format!("{what} `{word}`"));
        }
    }
    hits
}

/// 規則 C の走査 1 ファイル分。返すのは（clap の型の数, 見た doc comment の行数, 違反）
fn scan_clap_help(src: &str, rel: &str) -> (usize, usize, Vec<String>) {
    let spans = clap_item_spans(src);
    if spans.is_empty() {
        return (0, 0, Vec::new());
    }
    let literals = code_view::literals_only(src);
    let mut doc_lines = 0usize;
    let mut hits = Vec::new();
    let mut line_start = 0usize;
    for (lineno, line) in src.split('\n').enumerate() {
        let line_end = line_start + line.len();
        let inside = spans
            .iter()
            .find(|(s, e)| line_end > *s && line_start < *e)
            .map(|&(s, e)| (line_start.max(s), line_end.min(e)));
        if let Some((a, z)) = inside {
            let trimmed = line.trim_start();
            // doc comment（`///`。`////` は普通のコメント）
            if trimmed.starts_with("///") && !trimmed.starts_with("////") {
                doc_lines += 1;
                for what in help_marks_in(trimmed) {
                    hits.push(format!("{rel}:{}: doc comment に {what}", lineno + 1));
                }
            }
            // 属性の文字列（`#[arg(help = "…")]` / `#[command(about = "…")]` / `#[doc = "…"]`）
            if let Some(lit) = literals.get(a..z) {
                for what in help_marks_in(lit) {
                    hits.push(format!("{rel}:{}: 文字列に {what}", lineno + 1));
                }
            }
        }
        line_start = line_end + 1;
    }
    (spans.len(), doc_lines, hits)
}

/// 規則 C: clap のヘルプ（derive した型の doc comment と属性の文字列）へ macOS の打鍵表記を
/// 直書きしない（#2854）。doc comment はそのままヘルプの本文になるので、Windows では存在しない
/// 打鍵（Win キーは OS が持つ）を案内することになる
#[test]
fn clapのヘルプにmacosのキー表記を直書きしていない() {
    let root = repo_root();
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates"))
        .expect("crates/ を読めない")
        .flatten()
    {
        rust_files(&crate_dir.path().join("src"), &mut files);
    }
    files.sort();
    let mut hits = Vec::new();
    let mut seen: Vec<(String, usize, usize)> = Vec::new();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        if !src.contains("#[derive(") || !CLAP_DERIVES.iter().any(|d| src.contains(d)) {
            continue;
        }
        // Windows の CI でも同じ綴りで名指す（区切りを `/` に揃える）
        let rel = f
            .strip_prefix(&root)
            .unwrap_or(f)
            .display()
            .to_string()
            .replace('\\', "/");
        let (items, docs, file_hits) = scan_clap_help(&src, &rel);
        if items > 0 {
            seen.push((rel, items, docs));
        }
        hits.extend(file_hits);
    }
    // 走査が空振りしていない（clap の型を切り出せていない形を偽の緑にしない）。
    // tako-cli の main.rs は 2026-11-10 の実測で clap の型 116・doc comment 1,497 行
    let cli = seen
        .iter()
        .find(|(rel, _, _)| rel == "crates/tako-cli/src/main.rs")
        .unwrap_or_else(|| {
            panic!("tako-cli/src/main.rs の clap の型を 1 つも切り出せない: {seen:?}")
        });
    assert!(
        cli.1 >= 100 && cli.2 >= 1300,
        "tako-cli/src/main.rs の clap の型 / doc comment が少なすぎる（範囲の切り出しが壊れている）: \
         型 {} / doc comment {} 行",
        cli.1,
        cli.2
    );
    assert!(
        hits.is_empty(),
        "clap のヘルプに macOS のキー表記が直書きされている（Windows では存在しない打鍵を案内する）:\n  {}\n\
         → 打鍵は `tako_core::platform::keys` から実行 OS で引き、`#[command(about = …)]` / \
         `#[arg(help = …)]` へ入れてください（前例: tako-cli の `help_keys`・`close_pane_key`）。\n\
         doc comment は打鍵を書かない説明にしておく（clap は明示の about / help を優先する）",
        hits.join("\n  ")
    );
}

/// 規則 C の検出力: clap の型の doc comment・属性の文字列は拾い、普通の関数の doc comment・
/// clap の型の中の普通のコメント・引数名の `--cmd-file` は拾わない
#[test]
fn 規則cはclapのヘルプだけを見る() {
    let src = r#"
/// 打鍵の表記を組む（⌘F）。普通の関数の doc comment は対象外
fn helper() -> String { format!("{}", 1) }

/// 編集（⌥← と同じ）
#[derive(Subcommand)]
#[command(about = format!("{}", helper()))]
enum EditCommand {
    // 普通のコメントの ⌘⌫ は対象外
    /// 消す（Cmd+Backspace と同じ）
    Delete {
        /// 引数（--cmd-file は引数名なので当たらない）
        #[arg(long, help = "選択を伸ばす（\u{21E7} 付き）")]
        select: bool,
    },
    /// 検索（cmd+F と同じ）
    Search,
}

/// 型の外（⌫ は対象外）
struct Plain { a: u8 }

#[derive(Debug, clap::Args)]
struct Opts {
    /// 削除（⌫）
    x: u8,
}
"#;
    let (items, docs, hits) = scan_clap_help(src, "fixture.rs");
    assert_eq!(items, 2, "clap の型の数: {hits:?}");
    assert_eq!(docs, 5, "見た doc comment の行数: {hits:?}");
    let lines: Vec<&str> = hits
        .iter()
        .map(|h| h.split(": ").next().unwrap_or(""))
        .collect();
    assert_eq!(
        lines,
        [
            "fixture.rs:5",  // 型の doc comment の ⌥
            "fixture.rs:10", // Cmd+
            "fixture.rs:13", // 属性の文字列の \u{21E7}（大文字のエスケープ形）
            "fixture.rs:16", // cmd+
            "fixture.rs:25", // 別の型の ⌫
        ],
        "{hits:#?}"
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
