//! **#1746 の番犬**: tako-cli の本番コードに、理由コメントの無い範囲添字（バイト切り）が無い。
//!
//! ## なぜ止めるのか
//!
//! `&s[..N]` / `&s[a..]` の添字は**バイト位置**なので、多バイト文字の途中に当たると panic する。
//! 実物（#1746）: `tako task gate show` が証拠を `&ev[..120]` で切っていて、
//! `exit 0; stdout: ` + 日本語の出力だと 120 バイト目が「あ」（118..121）の途中に当たり、
//! `gate check` / `gate show` が exit 101 で落ちた（修正前のビルドで実測。`check` は結果を
//! 保存したあとで落ちるので、ゲートは記録されるのに CLI は失敗を返す）。
//! CLI の出力はコマンドの出力・ファイル名・session id のような任意の文字列を載せるので、
//! 同じ形は書くたびに戻りうる。#1728（プレビューの描画中の panic）と同型。
//!
//! ## 何を固定するか
//!
//! 1. `crates/tako-cli/src/` 配下の本番コードの範囲添字（`[a..]` / `[..b]` / `[a..b]` /
//!    `[..=b]`）は、次のどちらかを満たす
//!    - 添字を `floor_char_boundary(` / `ceil_char_boundary(` で丸めてある
//!    - 同じ行か 1 行上に `切り出し安全:` で始まる理由コメントがある
//!      （例: 直前の `starts_with('-')` で先頭は ASCII / `Vec` のスライス）
//!
//!    表示の切り詰めは `tako_core::text::truncate_chars`（文字数）を通す。`Vec` のスライスも
//!    同じ字面なので巻き込むが、理由コメント 1 行で通せる（字面から型は読めないため）
//! 2. `gate` の証拠の見出し（`gate_evidence_preview`）は `truncate_chars` を通す
//!
//! ファイルは起動のたびに列挙するので、tako-cli へ足したファイルも自動で視界に入る。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で本番コードの眺めに既知の範囲添字が見えていることを、
//! [`修正前の形を実際に落とす`] で**修正前ソースの断片が `file:line` で名指しされる**ことを
//! 確かめる（注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中の `#[cfg(test)]`（tako-cli の `main.rs` には 4 つある）で走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const CLI_SRC: &str = "crates/tako-cli/src";
const CLI_MAIN: &str = "crates/tako-cli/src/main.rs";

/// 理由コメントの目印（同じ行か 1 行上）
const REASON_MARK: &str = "切り出し安全:";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// tako-cli の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn cli_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} が読める: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("ディレクトリ項目").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join(CLI_SRC), &mut files);
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();
    rels
}

/// 本番コードだけの「コードの眺め」（テスト領域・コメント・文字列を空白へ潰す）。
///
/// どちらもバイト長と行番号が保たれるので `file:line` がそのまま使える。文字列を潰すのは、
/// 製品側の文言や注釈に書いた `[..N]` を拾わないため
fn code_of(src: &str) -> String {
    production_range::code_view_of(&production_range::scan(src).text)
}

/// 範囲添字の開き括弧の位置と、括弧の中身。
///
/// 添字として数えるのは「直前が識別子・`)`・`]`・`?`」の `[` だけ（配列リテラル・型・
/// 属性 `#[`・マクロ `vec![` は直前がそれ以外）。中身の最上位に `..` があり、`,` / `;` が
/// 無いもの（`[a, .., b]` のスライスパターンや `[0; n]` を外す）で、`[..]`（全体）は数えない
fn range_indexes(code: &str) -> Vec<(usize, String)> {
    let bytes = code.as_bytes();
    let mut hits = Vec::new();
    for (open, &b) in bytes.iter().enumerate() {
        if b != b'[' || open == 0 {
            continue;
        }
        let prev = bytes[open - 1];
        if !(prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b')' | b']' | b'?')) {
            continue;
        }
        let mut depth = 0i32;
        let mut close = None;
        for (at, &c) in bytes.iter().enumerate().skip(open) {
            match c {
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(at);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let inner = &code[open + 1..close];
        let mut nest = 0i32;
        let mut top = String::new();
        for c in inner.chars() {
            match c {
                '[' | '(' | '{' => nest += 1,
                ']' | ')' | '}' => nest -= 1,
                _ if nest == 0 => top.push(c),
                _ => {}
            }
        }
        if !top.contains("..") || top.contains(',') || top.contains(';') || inner.trim() == ".." {
            continue;
        }
        hits.push((open, inner.to_string()));
    }
    hits
}

/// バイト位置の行番号（1 始まり）
fn line_at(text: &str, at: usize) -> usize {
    text.as_bytes()[..at]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// 関数の窓（宣言行の 1 始まりの行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(code: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = code.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.trim_end() == close)
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 範囲添字の行ごとの判定（理由コメント付きか・丸めてあるか）
struct Site {
    line: usize,
    text: String,
    excused: bool,
}

fn sites(raw: &str, code: &str) -> Vec<Site> {
    let raw_lines: Vec<&str> = raw.lines().collect();
    range_indexes(code)
        .into_iter()
        .map(|(open, inner)| {
            let line = line_at(code, open);
            let here = raw_lines.get(line - 1).copied().unwrap_or("");
            let above = line
                .checked_sub(2)
                .and_then(|i| raw_lines.get(i))
                .copied()
                .unwrap_or("");
            let rounded =
                inner.contains("floor_char_boundary(") || inner.contains("ceil_char_boundary(");
            let commented = here.contains(REASON_MARK)
                || (above.trim_start().starts_with("//") && above.contains(REASON_MARK));
            Site {
                line,
                text: here.trim().to_string(),
                excused: rounded || commented,
            }
        })
        .collect()
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。
/// 違反は `file:line — 理由` の形で返す
fn offenders(raw: &str, code: &str, rel: &str) -> Vec<String> {
    let mut out: Vec<String> = sites(raw, code)
        .into_iter()
        .filter(|s| !s.excused)
        .map(|s| {
            format!(
                "{rel}:{} — 範囲添字に理由コメントが無い（`{}`）。文字列の添字はバイト位置なので、\
                 多バイト文字の途中に当たると CLI ごと panic する（#1746）。表示の切り詰めは \
                 `tako_core::text::truncate_chars`（文字数）、分けるなら `floor_char_boundary` で\
                 丸めてから。安全なら（既知の ASCII / Vec のスライス等）同じ行か 1 行上に \
                 `// {REASON_MARK} <理由>` を書く",
                s.line, s.text
            )
        })
        .collect();

    if rel == CLI_MAIN {
        match fn_window(code, "fn gate_evidence_preview(") {
            None => out.push(format!(
                "{rel}:0 — `gate_evidence_preview` が見つからない（走査が空振り）"
            )),
            Some((at, window)) if !window.contains("truncate_chars(") => out.push(format!(
                "{rel}:{at} — gate の証拠の見出しが `tako_core::text::truncate_chars` を通っていない。\
                 証拠はコマンドの出力そのものなので日本語が来る（#1746）"
            )),
            Some(_) => {}
        }
    }
    out
}

// --------------------------------------------------------------- テスト

#[test]
fn tako_cliの本番コードに理由の無いバイト切りが無い() {
    let files = cli_sources();
    let mut found = Vec::new();
    for rel in &files {
        let raw = read(rel);
        let code = code_of(&raw);
        found.extend(offenders(&raw, &code, rel));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていないこと。
///
/// ファイル移動・改名や、テスト領域の潰し方の破損で黙って 0 件になるのを防ぐ。
/// **潰したあとの眺め**で確かめる
#[test]
fn 走査が空振りしていない() {
    let files = cli_sources();
    for must in [CLI_MAIN, "crates/tako-cli/src/setup.rs"] {
        assert!(
            files.iter().any(|f| f == must),
            "{must} が列挙に無い（{CLI_SRC} の列挙が壊れている）: {files:?}"
        );
        // 本番コードが下限を割って縮んでいない（#1420 の範囲取りの確認）
        let _ = production_range::production(&read(must), must);
    }

    let raw = read(CLI_MAIN);
    let code = code_of(&raw);
    // 理由コメント付きで残した既知の範囲添字（master / solo の `-<名前>` と ledger の Vec）が
    // 範囲添字として見えている = 検出器が効いている
    let seen = sites(&raw, &code);
    let known: Vec<&Site> = seen
        .iter()
        .filter(|s| s.text.contains("&s[1..]") || s.text.contains("*limit..]"))
        .collect();
    assert_eq!(
        known.len(),
        3,
        "{CLI_MAIN}: 既知の範囲添字 3 か所（`&s[1..]` × 2 / `entries[... - *limit..]`）が\
         見えていない（検出器か走査範囲が壊れている）: {:?}",
        seen.iter().map(|s| (s.line, &s.text)).collect::<Vec<_>>()
    );
    assert!(
        known.iter().all(|s| s.excused),
        "理由コメントが読めていない"
    );
    // 証拠の見出しの関数とその呼び出し元が本番コードに居る
    for needle in ["fn print_gate_result(", "fn gate_evidence_preview("] {
        assert!(
            code.contains(needle),
            "{CLI_MAIN} の本番コードに `{needle}` が無い"
        );
    }
    // 単体テストの中の範囲添字は本番コードではないので潰れている
    // （テスト側には居て、その行は本番の検出結果に 1 つも混ざらない）
    let tests = production_range::code_view_of(&production_range::tests_only(&raw));
    let test_lines: Vec<usize> = range_indexes(&tests)
        .into_iter()
        .map(|(open, _)| line_at(&tests, open))
        .collect();
    assert!(
        !test_lines.is_empty(),
        "テストの眺めに範囲添字が 1 つも無い（#1420 の範囲取りが壊れている）"
    );
    assert!(
        seen.iter().all(|s| !test_lines.contains(&s.line)),
        "テスト領域の範囲添字が本番コードとして数えられている: {test_lines:?}"
    );
}

/// 番犬に検出力があること: **#1746 以前の形を実際に落とす**（`file:line` の名指しまで）
#[test]
fn 修正前の形を実際に落とす() {
    let check = |snippet: &str| offenders(snippet, &code_of(snippet), "crates/tako-cli/src/x.rs");

    // #1746 の実物（gate の証拠の見出し）
    let before = "fn print_gate_result(ev: &str) {\n\
                  \x20   let ev_short = if ev.len() > 120 {\n\
                  \x20       format!(\"{}...\", &ev[..120])\n\
                  \x20   } else {\n\
                  \x20       ev.to_string()\n\
                  \x20   };\n\
                  }\n";
    let found = check(before);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("crates/tako-cli/src/x.rs:3 — ") && found[0].contains("&ev[..120]"),
        "file:line で名指ししていない: {found:?}"
    );

    // 同じファイルに居た session id の先頭 8 バイト（#1746 で文字単位へ直した）
    let sid = "fn f(sid: &str) {\n    eprintln!(\"{}\", &sid[..sid.len().min(8)]);\n}\n";
    assert!(check(sid)[0].starts_with("crates/tako-cli/src/x.rs:2 — "));

    // 後ろ・中間・`..=` の形も同じ危険なので落とす
    for bad in [
        "&s[1..]",
        "&s[a..b]",
        "&s[..=n]",
        "s[i + 1..].trim()",
        "&v.x()[..n]",
    ] {
        let src = format!("fn f() {{\n    let _ = {bad};\n}}\n");
        let found = check(&src);
        assert_eq!(found.len(), 1, "`{bad}` を見逃した: {found:?}");
        assert!(
            found[0].starts_with("crates/tako-cli/src/x.rs:2 — "),
            "{found:?}"
        );
    }

    // 理由コメント（1 行上 / 同じ行）と文字境界への丸めは通す
    let excused = "fn f(s: &str) {\n\
                   \x20   // 切り出し安全: 直前の starts_with('-') で先頭は ASCII\n\
                   \x20   let a = &s[1..];\n\
                   \x20   let b = &s[2..]; // 切り出し安全: 先頭 2 バイトは ASCII の `~/`\n\
                   \x20   let c = &s[..s.floor_char_boundary(120)];\n\
                   }\n";
    assert!(check(excused).is_empty(), "{:?}", check(excused));
    // 目印が 2 行上にあるだけでは通さない（理由は切り出しのすぐそばに置く）
    let far = "fn f(s: &str) {\n    // 切り出し安全: 先頭は ASCII\n    let x = 1;\n    let a = &s[1..];\n}\n";
    assert_eq!(check(far).len(), 1, "{:?}", check(far));

    // 範囲添字でないものは数えない（全体・スライスパターン・属性・マクロ・型・
    // 配列リテラル・添字 1 つ・文字列 / コメントの中の字面）
    let clean = "#[derive(Debug)]\n\
                 fn f(s: &str, v: &[u8], m: &std::collections::HashMap<u8, u8>) {\n\
                 \x20   let _ = &s[..];\n\
                 \x20   if let [a, .., b] = v { let _ = (a, b); }\n\
                 \x20   let _ = vec![0u8; 4];\n\
                 \x20   let _ = [S { a: 1, ..Default::default() }];\n\
                 \x20   let _ = (v[0], m[&1]);\n\
                 \x20   let _ = \"&ev[..120]\"; // &ev[..120]\n\
                 \x20   let _ = s.get(..120);\n\
                 }\n";
    assert!(check(clean).is_empty(), "{:?}", check(clean));

    // gate の証拠の見出しが truncate_chars を通らない形（バイトで切り直した）も落とす
    let reimpl = "fn gate_evidence_preview(ev: &str) -> String {\n\
                  \x20   ev.get(..120).unwrap_or(ev).to_string()\n\
                  }\n";
    let found = offenders(reimpl, &code_of(reimpl), CLI_MAIN);
    assert!(
        found.len() == 1 && found[0].starts_with(&format!("{CLI_MAIN}:1 — ")),
        "{found:?}"
    );
}
