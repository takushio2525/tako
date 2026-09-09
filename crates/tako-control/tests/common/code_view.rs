//! ソースを「コードだけの眺め」へ潰す（番犬テストの共有部品）
//!
//! 走査型の番犬は、**自分の説明文や他のテストの期待値文字列**を拾わないために
//! コメントと文字列を先に潰す必要がある。この 1 実装を
//! `test_timing_watchdog`（#1220）と `psmux_cleanup_timeout_watchdog`（#1271）が
//! 共有する（走査の前処理を番犬ごとに書き直すと、片方だけ生文字列を取りこぼす）。

/// コメントと文字列 / 文字リテラルを空白へ潰した「コードだけの眺め」。
///
/// **バイト長を変えない**ので、見つけた位置から行番号をそのまま数えられる。
/// 潰しておかないと、この番犬自身の説明文や他のテストの期待値文字列
/// （`code.contains("elapsed")` 等）を拾ってしまう
pub fn code_view(src: &str) -> String {
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
