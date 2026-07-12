//! ターミナル画面上のリンク検出（URL / ファイルパス）
//!
//! `Screen` のテキストから URL やパスを検出し、グリッド座標のスパンとして返す。
//! GPUI 非依存。UI 層は検出結果を使って cmd+ホバー下線や cmd+クリック開く処理を行う。

use crate::screen::Screen;

/// 検出されたリンク 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedLink {
    /// リンクの種別
    pub kind: LinkKind,
    /// 解決済みのターゲット文字列（URL ならそのまま、パスなら絶対パス）
    pub target: String,
    /// 画面上のスパン（行をまたぐ場合は複数）。各要素は (row, start_col, end_col)
    /// end_col は exclusive
    pub spans: Vec<(usize, usize, usize)>,
}

/// リンクの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Url,
    Path,
}

/// 画面上のリンクを検出する。行折り返しをまたぐ URL も連結して検出する。
pub fn detect_links(screen: &Screen) -> Vec<DetectedLink> {
    let mut links = Vec::new();
    detect_urls(screen, &mut links);
    links
}

/// URL（http:// / https://）を検出する。
/// 行末で折り返された URL は次行の先頭と連結して 1 つの URL として扱う。
fn detect_urls(screen: &Screen, out: &mut Vec<DetectedLink>) {
    // 全行のテキストを連結し、行折り返しの境界を記録
    let mut combined = String::new();
    // (combined 上の byte offset, row, col) の写像
    let mut byte_map: Vec<(usize, usize, usize)> = Vec::new();

    for (row, line) in screen.lines.iter().enumerate() {
        let trimmed = line.text.trim_end();
        for (char_idx, ch) in trimmed.chars().enumerate() {
            let col = if char_idx < line.cell_cols.len() {
                line.cell_cols[char_idx]
            } else {
                char_idx
            };
            let offset = combined.len();
            combined.push(ch);
            byte_map.push((offset, row, col));
        }
        // 行末が画面幅いっぱいなら折り返しの可能性がある → 連結
        // そうでなければ区切りを入れる
        let line_fills_width = trimmed.chars().count() >= screen.cols
            || (!trimmed.is_empty()
                && line.cell_cols.last().copied().unwrap_or(0) + 1 >= screen.cols);
        if !line_fills_width {
            let offset = combined.len();
            combined.push('\n');
            byte_map.push((offset, row, screen.cols));
        }
    }

    // URL パターンの検出
    let mut search_start = 0;
    while search_start < combined.len() {
        // http:// または https:// を探す
        let rest = &combined[search_start..];
        let scheme_pos = find_url_scheme(rest);
        let Some(scheme_offset) = scheme_pos else {
            break;
        };
        let abs_start = search_start + scheme_offset;

        // URL の終端を見つける
        let url_end = find_url_end(&combined, abs_start);
        let url = &combined[abs_start..url_end];

        if url.len() > 10 {
            // byte_map から spans を構築
            let spans = byte_offsets_to_spans(&byte_map, abs_start, url_end, screen.cols);
            if !spans.is_empty() {
                out.push(DetectedLink {
                    kind: LinkKind::Url,
                    target: url.to_string(),
                    spans,
                });
            }
        }

        search_start = url_end;
    }
}

/// "http://" or "https://" の開始位置を返す
fn find_url_scheme(text: &str) -> Option<usize> {
    let lower = text.to_lowercase();
    let pos_http = lower.find("http://");
    let pos_https = lower.find("https://");
    match (pos_http, pos_https) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// URL の終端 byte offset を返す。
/// RFC 3986 の unreserved + reserved 文字に基づくが、ターミナル表示向けに調整。
fn find_url_end(text: &str, start: usize) -> usize {
    let mut end = start;
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;

    for ch in text[start..].chars() {
        match ch {
            // URL として有効な文字
            'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '-'
            | '.'
            | '_'
            | '~'
            | ':'
            | '/'
            | '?'
            | '#'
            | '@'
            | '!'
            | '$'
            | '&'
            | '\''
            | '*'
            | '+'
            | ','
            | ';'
            | '='
            | '%' => {
                end += ch.len_utf8();
            }
            '(' => {
                paren_depth += 1;
                end += 1;
            }
            ')' => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                    end += 1;
                } else {
                    break;
                }
            }
            '[' => {
                bracket_depth += 1;
                end += 1;
            }
            ']' => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                    end += 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    // 末尾の句読点を剥がす（"http://example.com." のピリオド等）
    while end > start {
        let last = text[start..end].chars().last().unwrap();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= last.len_utf8();
        } else {
            break;
        }
    }

    end
}

/// combined テキストの byte 範囲を画面上の (row, start_col, end_col) スパン列に変換する
fn byte_offsets_to_spans(
    byte_map: &[(usize, usize, usize)],
    start: usize,
    end: usize,
    cols: usize,
) -> Vec<(usize, usize, usize)> {
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();

    for &(offset, row, col) in byte_map {
        if offset >= end {
            break;
        }
        if offset < start {
            continue;
        }
        // 改行マーカーはスキップ
        if col >= cols {
            continue;
        }

        match spans.last_mut() {
            Some(last) if last.0 == row => {
                // 同じ行: end_col を拡張（+1 は次の文字の開始位置に基づく近似。
                // 全角文字なら +2 だが、cell_cols の差から正確な幅は取れないので
                // 後で修正する）
                last.2 = col + 1;
            }
            _ => {
                spans.push((row, col, col + 1));
            }
        }
    }

    spans
}

/// 指定セル座標にリンクがあるかを返す
pub fn link_at(links: &[DetectedLink], row: usize, col: usize) -> Option<&DetectedLink> {
    links.iter().find(|link| {
        link.spans
            .iter()
            .any(|&(r, sc, ec)| r == row && col >= sc && col < ec)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{Screen, ScreenLine};

    fn make_screen(lines: &[&str], cols: usize) -> Screen {
        Screen {
            cols,
            rows: lines.len(),
            lines: lines
                .iter()
                .map(|text| {
                    let text = text.to_string();
                    let cell_cols: Vec<usize> = text.chars().enumerate().map(|(i, _)| i).collect();
                    ScreenLine {
                        text,
                        runs: Vec::new(),
                        cell_cols,
                        has_wide: false,
                    }
                })
                .collect(),
            cursor: None,
            ime_cursor: None,
            display_offset: 0,
        }
    }

    #[test]
    fn detect_simple_url() {
        let screen = make_screen(&["Visit https://example.com for info"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::Url);
        assert_eq!(links[0].target, "https://example.com");
        assert_eq!(links[0].spans, vec![(0, 6, 25)]);
    }

    #[test]
    fn detect_url_with_path() {
        let screen = make_screen(&["Open https://github.com/user/repo/issues/123 now"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://github.com/user/repo/issues/123");
    }

    #[test]
    fn detect_url_with_query_and_fragment() {
        let screen = make_screen(&["https://example.com/search?q=hello&lang=en#results"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            "https://example.com/search?q=hello&lang=en#results"
        );
    }

    #[test]
    fn detect_url_strips_trailing_punctuation() {
        let screen = make_screen(&["See https://example.com."], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com");
    }

    #[test]
    fn detect_url_preserves_parens_in_wikipedia() {
        let screen = make_screen(
            &["https://en.wikipedia.org/wiki/Rust_(programming_language)"],
            80,
        );
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn detect_url_wrapping_across_lines() {
        // 画面幅ちょうどで URL が折り返される場合
        let url_part1 = "https://github.com/takushio2525/tako/";
        let screen = make_screen(
            &[
                url_part1,    // ちょうど行末まで（37 文字 = cols）
                "issues/146", // 次の行に続く
            ],
            url_part1.len(), // 1行目がちょうど cols に達する幅
        );
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            "https://github.com/takushio2525/tako/issues/146"
        );
        // 2行にまたがるスパン
        assert_eq!(links[0].spans.len(), 2);
        assert_eq!(links[0].spans[0].0, 0); // row 0
        assert_eq!(links[0].spans[1].0, 1); // row 1
    }

    #[test]
    fn detect_multiple_urls() {
        let screen = make_screen(&["https://a.com and http://b.com/path end"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "https://a.com");
        assert_eq!(links[1].target, "http://b.com/path");
    }

    #[test]
    fn no_url_detected_in_plain_text() {
        let screen = make_screen(&["just some plain text", "no links here"], 80);
        let links = detect_links(&screen);
        assert!(links.is_empty());
    }

    #[test]
    fn link_at_finds_correct_link() {
        let screen = make_screen(&["Visit https://example.com for info"], 80);
        let links = detect_links(&screen);
        // 列 6〜24 が URL
        assert!(link_at(&links, 0, 6).is_some());
        assert!(link_at(&links, 0, 15).is_some());
        assert!(link_at(&links, 0, 24).is_some());
        // 列 5 と 25 は URL 外
        assert!(link_at(&links, 0, 5).is_none());
        assert!(link_at(&links, 0, 25).is_none());
    }

    #[test]
    fn detect_http_url() {
        let screen = make_screen(&["http://localhost:3000/api/test"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "http://localhost:3000/api/test");
    }

    #[test]
    fn url_in_parentheses() {
        // マークダウン等で (https://example.com) のようにカッコで囲まれている場合
        let screen = make_screen(&["(https://example.com)"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com");
    }

    #[test]
    fn url_in_angle_brackets() {
        let screen = make_screen(&["<https://example.com>"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com");
    }
}
