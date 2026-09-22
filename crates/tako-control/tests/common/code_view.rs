//! ソースを「コードだけの眺め」へ潰す（番犬テストの共有部品）
//!
//! 走査型の番犬は、**自分の説明文や他のテストの期待値文字列**を拾わないために
//! コメントと文字列を先に潰す必要がある。この 1 実装を
//! `test_timing_watchdog`（#1220）・`psmux_cleanup_timeout_watchdog`（#1271）・
//! `tmux_e2e_watchdog`（#1300）が
//! 共有する（走査の前処理を番犬ごとに書き直すと、片方だけ生文字列を取りこぼす）。

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

/// 眺め方（同じ走査から 2 つの眺めを作る）。
///
/// 番犬ごとに正規化の写しを持たないための軸（#1441 / #1445）。
/// [`Keep::Code`] と [`Keep::Literals`] は**同じ 1 つの走査**の出し分けなので、
/// 片方だけが生文字列を取りこぼす形にならない
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keep {
    /// コメントと文字列 / 文字リテラルを潰し、**コードだけ**残す
    Code,
    /// コメントとコードを潰し、**文字列 / 文字リテラルの中身だけ**残す
    Literals,
}

/// 空白で潰したバイト（改行は残す = 行番号が保たれる）
fn blank_byte(byte: u8) -> u8 {
    if byte == b'\n' {
        b'\n'
    } else {
        b' '
    }
}

/// コメントと文字列 / 文字リテラルを空白へ潰した「コードだけの眺め」。
///
/// **バイト長を変えない**ので、見つけた位置から行番号をそのまま数えられる。
/// 潰しておかないと、この番犬自身の説明文や他のテストの期待値文字列
/// （`code.contains("elapsed")` 等）を拾ってしまう
pub fn code_view(src: &str) -> String {
    view(src, Keep::Code)
}

/// [`code_view`] の裏返し: **文字列 / 文字リテラルの中身だけ**を残し、
/// コードとコメントを空白へ潰した眺め。
///
/// 「画面へ出る文字に何が入っているか」を見張る番犬（#1536 の UI 絵文字）が使う。
/// コメントを潰すのは、**規約や理由の説明文に禁止文字そのものを書けるようにする**ため
/// （説明文まで落とすと「何を禁じているか」をソースに書けなくなる）。
/// 囲みの `"` / `'` と生文字列の `r#` は潰し、**エスケープはバイトのまま残す**
/// （`\u{1F310}` のような書き方も読み手が復号して見張れるようにするため）。
/// 行番号が保たれるので、こちらも `file:line` でそのまま名指しできる。
///
/// 既知の穴: **多バイト文字の文字リテラル**（`'⏺'`）は下の文字リテラル判定に
/// 当たらないのでコード扱いになる。`char` は `.child()` へ渡せず UI の文字列には
/// ならないので、UI を見張る用途では実害が無い
pub fn literals_only(src: &str) -> String {
    view(src, Keep::Literals)
}

/// [`code_view`] / [`literals_only`] の共有走査（出し分けは `keep` だけ）
pub fn view(src: &str, keep: Keep) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    // 空白で潰す（改行は残す = 行番号が保たれる）
    let blank = |out: &mut Vec<u8>, byte: u8| out.push(blank_byte(byte));
    // `kind` の領域は `keep` と一致するときだけ原文のまま出す
    let emit = |out: &mut Vec<u8>, byte: u8, kind: Keep| {
        out.push(if kind == keep { byte } else { blank_byte(byte) });
    };
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
                if closes {
                    blank(&mut out, b[i]);
                } else {
                    emit(&mut out, b[i], Keep::Literals);
                }
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
                    emit(&mut out, b[i], Keep::Literals);
                    i += 1;
                    if i < b.len() {
                        emit(&mut out, b[i], Keep::Literals);
                        i += 1;
                    }
                    continue;
                }
                let done = b[i] == b'"';
                if done {
                    blank(&mut out, b[i]);
                } else {
                    emit(&mut out, b[i], Keep::Literals);
                }
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
                        emit(&mut out, b[i], Keep::Literals);
                        i += 1;
                    }
                } else {
                    for _ in 0..1 {
                        emit(&mut out, b[i], Keep::Literals);
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
        emit(&mut out, b[i], Keep::Code);
        i += 1;
    }
    String::from_utf8(out).expect("空白で潰しても UTF-8 は壊れない")
}

/// 行まるごとのコメントだけ落とした眺め（**文字列リテラルは残す**）。
///
/// [`code_view`] は文字列も潰すので、「どんな文字列を書いたか」を見張る番犬
/// （#1271 の `taskkill /PID` / #1300 の固定ソケット名）はこちらを使う。
/// 説明文の中の同じ語を実装と読み違えないよう、コメント行だけは落とす
pub fn without_comment_lines(src: &str) -> String {
    src.lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
