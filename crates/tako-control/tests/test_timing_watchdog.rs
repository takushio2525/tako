//! 番犬: 効果を**実時間で比べている**テストがワークスペースに無い（#1220 / #1167）
//!
//! ## なぜ止めるのか
//!
//! 「速くなっている」を `Instant::elapsed` の**比較**で固定したテストは、片方の
//! 計測窓にだけスケジューリングの待ちが入った回に落ちる。実測で 2 件踏んだ:
//!
//! - #1167: `claude_remote_link` の `追記ぶんだけ読むと定常コストが増えない`
//!   （高負荷で 4 回に 1 回）
//! - #1220: `remote_link_live` の `一覧付与のコストが桁で問題ないこと`
//!   （初回 23.8ms / 2 回目 1.7ms なので、2 回目に 22ms 止まれば反転する）
//!
//! 直し方は**測る軸を替える**こと。守りたい性質を負荷に依らない量
//! （読み出しバイト数 / 走査回数 / 呼び出し回数）で書けば、混み具合に依らず
//! 同じ性質を桁で固定できる。規約は `.agent/conventions.md`
//! 「効果を測る単体テストは実時間で比べない」。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! `assert` 系マクロの**条件部**に、実時間の値が **2 つ以上**現れる形だけを落とす
//! （`assert!(warm <= cold, …)` / `assert!(t1.elapsed() < t0.elapsed())`）。
//! 実時間の値とは `.elapsed()` そのものと、`let x = ….elapsed();` で束縛された名前。
//!
//! 拾えないのは「`let` と `.elapsed()` が別の行に分かれた束縛」だけ（そのときは
//! `.elapsed()` が assert の条件部へ直接現れる形になりやすいので、そちらで捕まる）。
//!
//! 対象外（時間で書くのが正しい形。tests 配下の実際の使い方を分類して決めた）:
//!
//! | 形 | 例 | なぜ対象外か |
//! |---|---|---|
//! | タイムアウト / 待ち | `while t0.elapsed() < limit` / `Instant::now() < deadline` | 主題が「待つこと」。assert の条件部ではない |
//! | 絶対予算 | `assert!(elapsed < Duration::from_secs(3), …)`（`psmux_backend` の kill = 5.1 秒ブロックの検出） | 実時間の値が 1 つ = 2 窓の比較ではない。桁で開けてあれば負荷で反転しない |
//! | 報告のみ | `println!("所要={elapsed:?}")`（`issue1011_agents_scan_cost_e2e`。assert は回数） | 落ちる材料にしていない |
//!
//! ## 相棒
//!
//! `remote_link_watchdog.rs` の `追記ぶんだけ読む検査を実時間で測っていない` は
//! **1 ファイルに閉じた強い規則**（`claude_remote_link.rs` のテストは `Instant`
//! そのものを禁止）。こちらはワークスペースの tests 配下全体を、上の 1 形だけで見る

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// `crates/*/tests/` 配下の `.rs` を集める
fn test_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(crates) = std::fs::read_dir(repo_root().join("crates")) else {
        return out;
    };
    for entry in crates.flatten() {
        let mut stack = vec![entry.path().join("tests")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// コメントと文字列 / 文字リテラルを空白へ潰した「コードだけの眺め」。
///
/// **バイト長を変えない**ので、見つけた位置から行番号をそのまま数えられる。
/// 潰しておかないと、この番犬自身の説明文や他のテストの期待値文字列
/// （`code.contains("elapsed")` 等）を拾ってしまう
fn code_view(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    // 空白で潰す（改行は残す = 行番号が保たれる）
    let blank = |out: &mut Vec<u8>, byte: u8| out.push(if byte == b'\n' { b'\n' } else { b' ' });
    while i < b.len() {
        // 行コメント
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                blank(&mut out, b[i]);
                i += 1;
            }
            continue;
        }
        // ブロックコメント（入れ子は考えない = tests 配下に無い）
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            while i < b.len() && !(b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/') {
                blank(&mut out, b[i]);
                i += 1;
            }
            for _ in 0..2 {
                if i < b.len() {
                    blank(&mut out, b[i]);
                    i += 1;
                }
            }
            continue;
        }
        // 生文字列（`r"…"` / `r#"…"#` / `br#"…"#`）
        let raw_start = {
            let mut j = i;
            if j < b.len() && b[j] == b'b' {
                j += 1;
            }
            if j < b.len() && b[j] == b'r' {
                j += 1;
                let hashes = {
                    let mut n = 0;
                    while j + n < b.len() && b[j + n] == b'#' {
                        n += 1;
                    }
                    n
                };
                if j + hashes < b.len() && b[j + hashes] == b'"' {
                    Some((j + hashes + 1, hashes))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((body, hashes)) = raw_start {
            while i < body {
                blank(&mut out, b[i]);
                i += 1;
            }
            loop {
                if i >= b.len() {
                    break;
                }
                let closes = b[i] == b'"'
                    && (1..=hashes).all(|k| i + k < b.len() && b[i + k] == b'#')
                    && i + hashes < b.len();
                blank(&mut out, b[i]);
                i += 1;
                if closes {
                    for _ in 0..hashes {
                        if i < b.len() {
                            blank(&mut out, b[i]);
                            i += 1;
                        }
                    }
                    break;
                }
            }
            continue;
        }
        // 通常の文字列（`"…"` / `b"…"`）
        if b[i] == b'"' {
            blank(&mut out, b[i]);
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    blank(&mut out, b[i]);
                    i += 1;
                    if i < b.len() {
                        blank(&mut out, b[i]);
                        i += 1;
                    }
                    continue;
                }
                let done = b[i] == b'"';
                blank(&mut out, b[i]);
                i += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        // 文字リテラル（`'a'` / `'\''` / `'"'`）。**ライフタイム（`'static`）は素通し**
        if b[i] == b'\'' {
            let escaped = i + 1 < b.len() && b[i + 1] == b'\\';
            let plain = i + 2 < b.len() && b[i + 2] == b'\'';
            if escaped || plain {
                blank(&mut out, b[i]);
                i += 1;
                if escaped {
                    while i < b.len() && b[i] != b'\'' {
                        blank(&mut out, b[i]);
                        i += 1;
                    }
                } else {
                    for _ in 0..1 {
                        blank(&mut out, b[i]);
                        i += 1;
                    }
                }
                if i < b.len() {
                    blank(&mut out, b[i]); // 閉じ '
                    i += 1;
                }
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).expect("空白で潰しても UTF-8 は壊れない")
}

/// `let x = ….elapsed()…;` で束縛された名前（= 実時間の値）
fn duration_names(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in code.lines() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("let ") else {
            continue;
        };
        if !line.contains(".elapsed()") {
            continue;
        }
        let name: String = rest
            .trim_start_matches("mut ")
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

/// 名前が**識別子として**現れる回数（`elapsed` が `elapsed_ms` に当たらないように）
fn word_count(haystack: &str, word: &str) -> usize {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let b = haystack.as_bytes();
    haystack
        .match_indices(word)
        .filter(|(at, _)| {
            let before_ok =
                *at == 0 || !(b[at - 1] as char).is_ascii() || !ident(b[at - 1] as char);
            let after = at + word.len();
            let after_ok = after >= b.len() || !ident(b[after] as char);
            before_ok && after_ok
        })
        .count()
}

/// assert 系マクロの**条件部**を返す（`(byte offset, 条件のテキスト)`）。
///
/// `assert!` は第 1 引数、`assert_eq!` / `assert_ne!` は第 1〜2 引数。
/// **メッセージ部は見ない**（`assert!(a.scans <= n, "…{cold:?}/{warm:?}…")` を
/// 違反にしないため。文字列は潰してあるが、フォーマット引数は素の名前で並ぶ）
fn assert_conditions(code: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (name, args) in [("assert!(", 1), ("assert_eq!(", 2), ("assert_ne!(", 2)] {
        for (at, _) in code.match_indices(name) {
            let open = at + name.len();
            let mut depth = 1i32;
            let mut cuts: Vec<usize> = Vec::new();
            let mut end = code.len();
            for (rel, c) in code[open..].char_indices() {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + rel;
                            break;
                        }
                    }
                    ',' if depth == 1 => cuts.push(open + rel),
                    _ => {}
                }
            }
            let cut = cuts.get(args - 1).copied().unwrap_or(end).min(end);
            out.push((
                at,
                code[open..cut]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
    }
    out
}

#[test]
fn 効果を実時間で比べているテストが無い() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();
    for path in test_sources() {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let code = code_view(&src);
        let names = duration_names(&code);
        for (at, cond) in assert_conditions(&code) {
            let inline = cond.matches(".elapsed()").count();
            let named: usize = names.iter().map(|n| word_count(&cond, n)).sum();
            if inline + named < 2 {
                continue;
            }
            let line = code[..at].lines().count();
            let shown = path.strip_prefix(&root).unwrap_or(&path).display();
            violations.push(format!("  {shown}:{line}: assert(… {cond} …)"));
        }
    }
    assert!(
        violations.is_empty(),
        "実時間（`Instant::elapsed`）同士を assert の条件部で比べているテストがある\n\
         （片方の計測窓にだけ待ちが入った回に落ちる。#1167 は 4 回に 1 回・#1220 は\n\
         他 worker のビルドと同時で 1 回 FAILED）。守りたい性質を負荷に依らない量\n\
         （読み出しバイト数 / 走査回数 / 呼び出し回数）で書き直す:\n{}\n\n\
         直し方の実例: `remote_link_live` の `一覧付与のコストが桁で問題ないこと`\n\
         （`claude_remote_link::scan_counters` で回数とバイト数を数える）。\n\
         規約は `.agent/conventions.md`「効果を測る単体テストは実時間で比べない」",
        violations.join("\n")
    );
}
