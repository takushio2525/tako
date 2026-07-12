//! ターミナル画面上のリンク検出（URL / ファイルパス）
//!
//! `Screen` のテキストから URL やパスを検出し、グリッド座標のスパンとして返す。
//! GPUI 非依存。UI 層は検出結果を使って cmd+ホバー下線や cmd+クリック開く処理を行う。

use crate::screen::Screen;
use std::path::{Path, PathBuf};

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

/// 画面上のリンクを検出する（URL のみ。パス検出不要な呼び出し用）
pub fn detect_links(screen: &Screen) -> Vec<DetectedLink> {
    detect_links_with_cwd(screen, None)
}

/// 画面上のリンクを検出する。cwd を渡すとファイル/ディレクトリパスも検出する。
/// パスは cwd 基準の相対解決 + `~` 展開 + 絶対パスを実在チェックしてリンク化する。
pub fn detect_links_with_cwd(screen: &Screen, cwd: Option<&Path>) -> Vec<DetectedLink> {
    let mut links = Vec::new();
    detect_urls(screen, &mut links);
    if let Some(cwd) = cwd {
        let url_links = links.clone();
        detect_paths(screen, cwd, &url_links, &mut links);
    }
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

/// ファイル/ディレクトリパスを検出する。
/// 行ごとにパスらしきトークンを抽出し、cwd 基準の相対解決 / `~` 展開 / 絶対パスで
/// 実在チェックに通ったものだけリンク化する。URL と重複する範囲はスキップ。
fn detect_paths(
    screen: &Screen,
    cwd: &Path,
    existing_links: &[DetectedLink],
    out: &mut Vec<DetectedLink>,
) {
    let home = dirs_hint();

    for (row, line) in screen.lines.iter().enumerate() {
        let text = line.text.trim_end();
        if text.is_empty() {
            continue;
        }

        for (token, start_col) in extract_path_tokens(text, &line.cell_cols) {
            // URL リンクと重複する範囲はスキップ
            let end_col = start_col + token.chars().count();
            if overlaps_existing(existing_links, row, start_col, end_col) {
                continue;
            }

            // 行番号サフィックスを分離（`src/main.rs:42:5` → `src/main.rs`）
            let path_part = strip_line_col_suffix(&token);

            if let Some(resolved) = resolve_path(path_part, cwd, home.as_deref()) {
                out.push(DetectedLink {
                    kind: LinkKind::Path,
                    target: resolved.to_string_lossy().into_owned(),
                    spans: vec![(row, start_col, end_col)],
                });
            }
        }
    }
}

/// パスらしきトークンを行テキストから抽出する。
/// `/` `.` `~` で始まるか、内部に `/` を含む空白区切りトークンを候補とする。
fn extract_path_tokens(text: &str, cell_cols: &[usize]) -> Vec<(String, usize)> {
    let mut tokens = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    // char_index → col の写像
    let col_of = |char_idx: usize| -> usize {
        if char_idx < cell_cols.len() {
            cell_cols[char_idx]
        } else {
            char_idx
        }
    };

    let mut i = 0;
    while i < chars.len() {
        // 空白・制御文字をスキップ
        if chars[i].1.is_whitespace() || chars[i].1.is_control() {
            i += 1;
            continue;
        }

        // トークンの開始。引用符・括弧で囲まれている場合は剥がす
        let (quote_end, start_skip) = match chars[i].1 {
            '\'' | '"' | '`' => (Some(chars[i].1), 1),
            _ => (None, 0),
        };
        let token_start = i + start_skip;

        // トークンの終端を探す
        let mut j = token_start;
        while j < chars.len() {
            let ch = chars[j].1;
            if let Some(q) = quote_end {
                if ch == q {
                    break;
                }
            } else if ch.is_whitespace()
                || ch.is_control()
                || matches!(
                    ch,
                    '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
                )
            {
                break;
            }
            j += 1;
        }

        let token_text: String = chars[token_start..j].iter().map(|&(_, c)| c).collect();
        let col = col_of(token_start);

        // 閉じ引用符があればスキップ
        let skip_end = if quote_end.is_some() && j < chars.len() {
            1
        } else {
            0
        };
        i = j + skip_end;

        // パスらしさの判定: `/` `.` `~` で始まるか、内部に `/` を含む
        if is_path_like(&token_text) {
            // 末尾のコロンや句読点を剥がす（`path/to/file:` 等）
            let cleaned = token_text.trim_end_matches([':', '.', ',']);
            if !cleaned.is_empty() {
                tokens.push((cleaned.to_string(), col));
            }
        }
    }

    // chars は使われていないが実際は上の while ループで走査済み
    let _ = chars;
    tokens
}

/// トークンがパスらしいかの簡易判定
fn is_path_like(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let first = token.chars().next().unwrap();
    // `/` `~` `./` `../` で始まる、または内部に `/` を含む
    first == '/'
        || first == '~'
        || token.starts_with("./")
        || token.starts_with("../")
        || (token.contains('/') && !token.contains("://"))
}

/// `src/main.rs:42:5` → `src/main.rs` のように行番号・列番号サフィックスを除去する
fn strip_line_col_suffix(token: &str) -> &str {
    // 末尾から `:数字` パターンを最大 2 つ剥がす
    let mut end = token.len();
    for _ in 0..2 {
        if let Some(colon_pos) = token[..end].rfind(':') {
            let suffix = &token[colon_pos + 1..end];
            if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                end = colon_pos;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    &token[..end]
}

/// パスを解決する。cwd 基準の相対 / `~` 展開 / 絶対パスの 3 戦略を試し、
/// 実在するものを返す。
fn resolve_path(raw: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() < 2 {
        return None;
    }

    // `~` 展開
    let expanded = if raw.starts_with("~/") || raw == "~" {
        home.map(|h| h.join(&raw[2..]))
    } else {
        None
    };

    // 試す順序: 展開済み → 絶対パス → cwd 相対
    let candidates: Vec<PathBuf> = [
        expanded,
        if raw.starts_with('/') {
            Some(PathBuf::from(raw))
        } else {
            None
        },
        if !raw.starts_with('/') && !raw.starts_with('~') {
            Some(cwd.join(raw))
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    .collect();

    candidates.into_iter().find(|p| p.exists())
}

/// 既存リンクと範囲が重複するか判定する
fn overlaps_existing(links: &[DetectedLink], row: usize, start: usize, end: usize) -> bool {
    links.iter().any(|link| {
        link.spans
            .iter()
            .any(|&(r, sc, ec)| r == row && start < ec && end > sc)
    })
}

/// ホームディレクトリのヒント。`HOME` 環境変数から取得
fn dirs_hint() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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

    // --- パス検出テスト ---

    fn setup_test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako_links_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("README.md"), "# readme").unwrap();
        std::fs::create_dir_all(dir.join("deep/nested")).unwrap();
        std::fs::write(dir.join("deep/nested/file.txt"), "").unwrap();
        dir
    }

    fn cleanup_test_dir(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn detect_relative_path_with_cwd() {
        let dir = setup_test_dir("relative");
        let screen = make_screen(&["edit src/main.rs please"], 80);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert_eq!(
            path_links[0].target,
            dir.join("src/main.rs").to_str().unwrap()
        );
        assert_eq!(path_links[0].spans, vec![(0, 5, 16)]);
        cleanup_test_dir(&dir);
    }

    #[test]
    fn detect_absolute_path() {
        let dir = setup_test_dir("absolute");
        let abs = dir.join("README.md");
        let line = format!("open {}", abs.display());
        let screen = make_screen(&[&line], 200);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert_eq!(path_links[0].target, abs.to_str().unwrap());
        cleanup_test_dir(&dir);
    }

    #[test]
    fn detect_tilde_path() {
        let home = std::env::var("HOME").unwrap();
        let test_file = std::path::PathBuf::from(&home).join(".tako_test_link_detect");
        std::fs::write(&test_file, "").unwrap();
        let screen = make_screen(&["cat ~/.tako_test_link_detect"], 80);
        let links = detect_links_with_cwd(&screen, Some(Path::new("/tmp")));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert_eq!(path_links[0].target, test_file.to_str().unwrap());
        let _ = std::fs::remove_file(&test_file);
    }

    #[test]
    fn nonexistent_path_excluded() {
        let dir = setup_test_dir("nonexistent");
        let screen = make_screen(&["open src/nonexistent.rs here"], 80);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert!(path_links.is_empty());
        cleanup_test_dir(&dir);
    }

    #[test]
    fn strip_line_col_suffix_works() {
        assert_eq!(strip_line_col_suffix("src/main.rs:42:5"), "src/main.rs");
        assert_eq!(strip_line_col_suffix("src/main.rs:42"), "src/main.rs");
        assert_eq!(strip_line_col_suffix("src/main.rs"), "src/main.rs");
        assert_eq!(strip_line_col_suffix("file.txt:"), "file.txt:");
    }

    #[test]
    fn detect_path_with_line_col_suffix() {
        let dir = setup_test_dir("linecol");
        let screen = make_screen(&["error at src/main.rs:42:5 bad"], 80);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert!(path_links[0].target.ends_with("src/main.rs"));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn path_detection_skips_url_range() {
        let dir = setup_test_dir("skipurl");
        let screen = make_screen(&["see https://github.com/user/repo for details"], 80);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::Url);
        cleanup_test_dir(&dir);
    }

    #[test]
    fn detect_nested_relative_path() {
        let dir = setup_test_dir("nested");
        let screen = make_screen(&["check deep/nested/file.txt"], 80);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert!(path_links[0].target.ends_with("deep/nested/file.txt"));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn detect_dot_slash_path() {
        let dir = setup_test_dir("dotslash");
        let screen = make_screen(&["run ./README.md"], 80);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert!(path_links[0].target.ends_with("README.md"));
        cleanup_test_dir(&dir);
    }

    #[test]
    fn is_path_like_checks() {
        assert!(is_path_like("/usr/bin/env"));
        assert!(is_path_like("~/config"));
        assert!(is_path_like("./local"));
        assert!(is_path_like("../parent"));
        assert!(is_path_like("src/main.rs"));
        assert!(!is_path_like("plain"));
        assert!(!is_path_like("http://example.com"));
        assert!(!is_path_like(""));
    }
}
