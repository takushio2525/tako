//! ターミナル画面上のリンク検出（URL / ファイルパス）
//!
//! `Screen` のテキストから URL やパスを検出し、グリッド座標のスパンとして返す。
//! GPUI 非依存。UI 層は検出結果を使って cmd+ホバー下線や cmd+クリック開く処理を行う。

use crate::screen::Screen;
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

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
    let mut links = Vec::new();
    detect_urls(screen, &mut links);
    links
}

/// 画面上のリンクを検出する。cwd を渡すとファイル/ディレクトリパスも検出する。
/// パスは cwd 基準の相対解決 + `~` 展開 + 絶対パスを実在チェックしてリンク化する。
pub fn detect_links_with_cwd(screen: &Screen, cwd: Option<&Path>) -> Vec<DetectedLink> {
    detect_links_with_home(screen, cwd, dirs_hint().as_deref())
}

/// ホームを明示して検出する。
///
/// `~` 展開に使うホームを引数で受け取るので、**プロセスの `HOME` を差し替えずに**
/// 「このホームならどう解決されるか」を試せる（単体テストと
/// [`detect_in_lines`] の材料）。本番の入口は [`detect_links_with_cwd`]。
pub fn detect_links_with_home(
    screen: &Screen,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Vec<DetectedLink> {
    detect_links_with_home_in(screen, cwd, home, legacy_1283())
}

/// [`detect_links_with_home`] の本体（#1283 の A/B 用に判断を引数で受け取る形）。
/// 公開関数はこれに [`legacy_1283`] を渡すだけなので、env が唯一の差になる
pub fn detect_links_with_home_in(
    screen: &Screen,
    cwd: Option<&Path>,
    home: Option<&Path>,
    legacy: bool,
) -> Vec<DetectedLink> {
    let mut links = Vec::new();
    detect_urls(screen, &mut links);
    let url_links = links.clone();
    detect_paths(screen, cwd, home, &url_links, &mut links, legacy);
    links
}

/// 画面テキスト（行の配列）からリンクを検出する（#1283）。
///
/// 実画面と同じ**行末まで空白詰めした行**（`screen::compose_line` は未使用セルも
/// 押し込む = #1182）を組み立ててから [`detect_links_with_home`] へ通すので、
/// GUI の cmd+クリックと**同じ判定**になる。CLI / MCP が「この画面テキストで
/// リンクになるパス」を答えるための入口で、人手のクリック無しに検証できる形にするため。
///
/// 列は「ASCII = 1 列 / それ以外 = 2 列」で数える（東アジア幅の近似）。返る
/// スパンの列番号はこの近似に従うので、実画面の列と 1 列ずれることがある
/// （どのパスがリンクになるかの判定には影響しない）。
pub fn detect_in_lines(
    lines: &[String],
    cols: usize,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Vec<DetectedLink> {
    detect_in_lines_in(lines, cols, cwd, home, legacy_1283())
}

/// [`detect_in_lines`] の本体（#1283 の A/B 用に判断を引数で受け取る形）
pub fn detect_in_lines_in(
    lines: &[String],
    cols: usize,
    cwd: Option<&Path>,
    home: Option<&Path>,
    legacy: bool,
) -> Vec<DetectedLink> {
    detect_links_with_home_in(&screen_from_lines(lines, cols), cwd, home, legacy)
}

/// [`detect_in_lines`] 用に実画面と同じ形の `Screen` を組む。
/// dispatch（`tako links` / `tako_links` の `text` 経路）も同じ形を使う
pub fn screen_from_lines(lines: &[String], cols: usize) -> Screen {
    use crate::screen::ScreenLine;
    let cols = cols.max(1);
    // 幅を超える行は**切り捨てずに次の行へ折り返す**（実端末と同じ）。
    // 黙って切ると、囲みの閉じ引用符やパスの末尾が消えて判定が変わる
    // （実測: 120 桁で `見て`<108 桁のパス>`、開いて。` の閉じ記号が落ちて
    // リンクにならなかった。切り捨てが「無言で答えを変える」形になっていた）
    let mut rows: Vec<ScreenLine> = Vec::new();
    let mut push_row = |cell_cols: Vec<usize>, text: String, filled: usize| {
        let mut cell_cols = cell_cols;
        let mut text = text;
        let mut col = filled;
        // 行末まで空白で埋める（実画面と同じ形）
        while col < cols {
            cell_cols.push(col);
            text.push(' ');
            col += 1;
        }
        let has_wide = cell_cols.windows(2).any(|w| w[1] - w[0] > 1);
        rows.push(ScreenLine {
            text,
            runs: Vec::new(),
            cell_cols,
            has_wide,
        });
    };
    for line in lines {
        let mut cell_cols: Vec<usize> = Vec::new();
        let mut text = String::new();
        let mut col = 0usize;
        for ch in line.chars() {
            let width = if ch.is_ascii() { 1 } else { 2 };
            // 残り 1 列に全角は入らない（実端末は次の行へ送る）
            if col + width > cols {
                push_row(
                    std::mem::take(&mut cell_cols),
                    std::mem::take(&mut text),
                    col,
                );
                col = 0;
            }
            cell_cols.push(col);
            text.push(ch);
            col += width;
        }
        push_row(cell_cols, text, col);
    }
    let lines = rows;
    Screen {
        cols,
        rows: lines.len(),
        lines,
        cursor: None,
        ime_cursor: None,
        display_offset: 0,
        fract: 0.0,
        extra_bottom: None,
    }
}

/// 画面行を論理テキストへ連結し、各文字の画面座標を記録する。
///
/// 右端まで埋まった行はターミナルの soft wrap とみなして次行へ直結し、それ以外は
/// 改行で区切る。URL とパスで同じ規則を使うことで、TUI の装飾付き折り返しでも
/// 検出範囲と hit-test 座標を一致させる。
fn combined_screen_text(screen: &Screen) -> (String, Vec<(usize, usize, usize)>) {
    let mut combined = String::new();
    // (combined 上の byte offset, row, col) の写像
    let mut byte_map = Vec::new();

    for (row, line) in screen.lines.iter().enumerate() {
        let trimmed = line.text.trim_end();
        for (char_idx, ch) in trimmed.chars().enumerate() {
            let col = line.cell_cols.get(char_idx).copied().unwrap_or(char_idx);
            let offset = combined.len();
            combined.push(ch);
            byte_map.push((offset, row, col));
        }
        // 右端まで埋まっているか = **最後の「空白でない」文字が右端の列に在るか**。
        //
        // `line.cell_cols.last()` を見てはいけない（#1182）: 実画面の `ScreenLine.text`
        // は**行末まで空白で埋まっている**（`screen::compose_line` は未使用セルも
        // 押し込む）ので、`cell_cols.last()` は常に最終列 = **どの行も soft wrap 扱い**に
        // なる。すると画面全体が改行なしの 1 本へ連結され、パストークンが次の行の
        // 先頭と融合して実在しなくなる（実測: 隣接する 2 行に何か書かれているだけで
        // ターミナル内のパスリンクが 1 つも検出されなかった）。
        // 単体テストの `make_screen` は空白詰めをしないので、この形は再現しない
        let last_visible_col = trimmed
            .chars()
            .count()
            .checked_sub(1)
            .and_then(|last_idx| line.cell_cols.get(last_idx).copied());
        let line_fills_width = trimmed.chars().count() >= screen.cols
            || last_visible_col.is_some_and(|col| col + 1 >= screen.cols);
        if !line_fills_width {
            let offset = combined.len();
            combined.push('\n');
            byte_map.push((offset, row, screen.cols));
        }
    }

    (combined, byte_map)
}

/// URL（http:// / https://）を検出する。
/// 行末で折り返された URL は次行の先頭と連結して 1 つの URL として扱う。
fn detect_urls(screen: &Screen, out: &mut Vec<DetectedLink>) {
    let (combined, byte_map) = combined_screen_text(screen);

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
///
/// 小文字化した写しを作ってその位置を返すことはしない。小文字化はバイト長を
/// 変えうる（`İ` U+0130 は 2 → 3 バイト、`ẞ` U+1E9E は 3 → 2 バイト）ので、
/// 該当文字が手前に 1 つあるだけで、返した位置が元テキストの別の場所を指す（#1016）。
/// スキームは ASCII なので、元テキストを走査して ASCII 限定の大文字小文字無視で
/// 突き合わせれば、位置は定義上つねに元テキスト基準になる。
fn find_url_scheme(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    text.char_indices().find_map(|(idx, ch)| {
        // スキームは必ず h / H で始まる（安い足切り）
        if ch != 'h' && ch != 'H' {
            return None;
        }
        let rest = &bytes[idx..];
        let hit = [b"http://".as_slice(), b"https://".as_slice()]
            .iter()
            .any(|scheme| {
                rest.len() >= scheme.len() && rest[..scheme.len()].eq_ignore_ascii_case(scheme)
            });
        hit.then_some(idx)
    })
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
    cwd: Option<&Path>,
    home: Option<&Path>,
    existing_links: &[DetectedLink],
    out: &mut Vec<DetectedLink>,
    legacy: bool,
) {
    let (text, byte_map) = combined_screen_text(screen);
    for (token, byte_range) in extract_path_tokens(&text, legacy) {
        let full_spans =
            byte_offsets_to_spans(&byte_map, byte_range.start, byte_range.end, screen.cols);
        if full_spans.is_empty() || overlaps_existing(existing_links, &full_spans) {
            continue;
        }

        // 地の文が食い込んだトークンに備えて候補を長い順に試す（#1283）。
        // トークン全体が最初の候補なので、今まで引けていた形は定義上そのまま引ける
        for (rel_start, candidate) in path_candidates_in(&token, legacy) {
            // 行番号サフィックスを分離（`src/main.rs:42:5` → `src/main.rs`）
            let path_part = strip_line_col_suffix(candidate);
            let Some(resolved) = resolve_path(path_part, cwd, home) else {
                continue;
            };
            let start = byte_range.start + rel_start;
            let spans =
                byte_offsets_to_spans(&byte_map, start, start + candidate.len(), screen.cols);
            if spans.is_empty() {
                continue;
            }
            out.push(DetectedLink {
                kind: LinkKind::Path,
                target: resolved.to_string_lossy().into_owned(),
                spans,
            });
            break;
        }
    }
}

/// 削った候補の上限（実在チェックの syscall は cmd+ホバー毎に走る）
const MAX_TRIMMED_CANDIDATES: usize = 12;
/// 切り出し位置の上限（前後それぞれ）
const MAX_CUTS: usize = 6;

/// 実在チェックに掛ける候補を**長い順**に返す（`(トークン内の byte 開始位置, 部分文字列)`）。
///
/// #1283 の真因はここに繋がる: トークンの区切りは ASCII の空白と
/// `()[]{}<>,;` だけなので、**日本語の地の文にパスが埋まっていると文ごと 1 トークン**に
/// なり、実在チェックで落ちて 1 つもリンクにならなかった（実測: master の応答 1 行が
/// 丸ごと `動画は`~/…mp4`、確認して。` の 1 トークン / ユーザーの入力行は
/// `~/…mp4このリンクが`）。
///
/// **区切り集合を増やして機械的に切ることはしない**。全角の括弧や読点は
/// `資料（最新）.pdf` のように**実在するファイル名**にも現れるので、区切りにすると
/// 今まで引けていた形を落とす。代わりに**トークン全体を先に試し、駄目なときだけ**
/// 文字種の切り替わり位置で前後を削った候補を試す（実在するかどうかで決めるので、
/// 引けていた形は定義上そのまま引ける）。
/// #1283 の A/B。`TAKO_1283_LEGACY=1` で**同一バイナリのまま**修正前の挙動
/// （トークン全体だけを試す）へ戻す
pub fn legacy_1283() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1283_LEGACY").map(|v| v == "1") == Ok(true))
}

/// [`path_candidates`] の本体（判断を引数で受け取る形。env は唯一の差）
fn path_candidates_in(token: &str, legacy: bool) -> Vec<(usize, &str)> {
    let mut out = vec![(0usize, token)];
    // 地の文が食い込む余地があるのは「ASCII だけで書かれていないトークン」だけ。
    // 全 ASCII のトークンでは候補を増やさない（実在チェックの syscall を増やさない）
    if legacy || token.is_ascii() {
        return out;
    }
    let starts = candidate_starts(token);
    let ends = candidate_ends(token);
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for &start in &starts {
        for &end in &ends {
            // トークン全体は既に out の先頭に在る
            if end > start && !(start == 0 && end == token.len()) {
                pairs.push((start, end));
            }
        }
    }
    pairs.sort_by_key(|&(start, end)| std::cmp::Reverse(end - start));
    for (start, end) in pairs.into_iter().take(MAX_TRIMMED_CANDIDATES) {
        let sub = &token[start..end];
        if is_path_like(sub) {
            out.push((start, sub));
        }
    }
    out
}

/// パスの本体に現れうる文字か。
///
/// 非 ASCII は「本体にも地の文にも現れる」ので**本体扱いにしない**
/// （切り出し位置の手がかりとして使う）。引用符・バッククォートも本体扱いしない
fn is_path_body_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '.' | '_' | '-' | '~' | '/' | '+' | '=' | '%' | '@' | '#' | '$' | '&' | '!' | ':'
        )
}

/// 先頭に地の文が食い込んでいる場合の切り出し位置（先頭 = 0 を必ず含む）。
///
/// **絶対パス / ホーム起点 / `./` `../` が始まる位置だけ**を候補にする
/// （途中の `/` から切ると意味の無い断片が大量に増える）
fn candidate_starts(token: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut prev_is_body = true;
    for (idx, ch) in token.char_indices() {
        if idx > 0 && !prev_is_body {
            let rest = &token[idx..];
            if rest.starts_with("~/")
                || rest.starts_with('/')
                || rest.starts_with("./")
                || rest.starts_with("../")
            {
                starts.push(idx);
                if starts.len() > MAX_CUTS {
                    break;
                }
            }
        }
        prev_is_body = is_path_body_char(ch);
    }
    starts
}

/// 末尾に地の文が食い込んでいる場合の切り出し位置（末尾 = 長さを必ず含む）。
///
/// 「本体の文字 → 本体でない文字」へ切り替わる位置だけを候補にする
/// （`…mp4このリンクが` なら `こ` の位置 1 つだけ）。`/` で終わる候補は
/// **パスの区切りを跨いで切った断片**なので採らない（存在するディレクトリへ
/// 誤って飛ばさないため）
fn candidate_ends(token: &str) -> Vec<usize> {
    let mut ends = vec![token.len()];
    let mut prev_is_body = false;
    for (idx, ch) in token.char_indices() {
        let is_body = is_path_body_char(ch);
        if !is_body && prev_is_body && idx >= 2 && !token[..idx].ends_with('/') {
            ends.push(idx);
            if ends.len() > MAX_CUTS {
                break;
            }
        }
        prev_is_body = is_body;
    }
    ends
}

/// パスらしきトークンを行テキストから抽出する。
/// `/` `.` `~` で始まるか、内部に `/` を含む空白区切りトークンを候補とする。
///
/// `legacy` は #1283 の A/B（`TAKO_1283_LEGACY=1` で修正前の切り出しへ戻す）
fn extract_path_tokens(text: &str, legacy: bool) -> Vec<(String, Range<usize>)> {
    let mut tokens = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();

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
                // #1283: **途中のバッククォートでも切る**（囲みの外に地の文が在る形）。
                // 引用符が「トークンの先頭に在るとき」しか開き記号として扱われないので、
                // `見て`/path with space.txt`、開いて。` のように前に文がくっつくと
                // **空白でトークンが割れて**囲みの意味が失われていた（実測: リンクなし）。
                // 切るのはバッククォートだけ: `'` / `"` は `John's.txt` のように
                // **実在するファイル名**に現れる（バッククォートはシェルで扱えないので
                // 実質現れない）。残る既知の穴は「バッククォートを含むファイル名」
                || (!legacy && quote_end.is_none() && ch == '`')
            {
                break;
            }
            j += 1;
        }

        // 区切り文字そのもの（`(`, `[`, `{` 等）から始まった場合は空トークンになる。
        // index を進めないと同じ文字を永久に再走査するため、必ず 1 文字消費する。
        if j == token_start {
            i = j + 1;
            continue;
        }

        let token_text: String = chars[token_start..j].iter().map(|&(_, c)| c).collect();
        let byte_start = chars
            .get(token_start)
            .map(|&(offset, _)| offset)
            .unwrap_or(text.len());

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
                tokens.push((cleaned.to_string(), byte_start..byte_start + cleaned.len()));
            }
        }
    }
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
fn resolve_path(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
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
        cwd.filter(|_| !raw.starts_with('/') && !raw.starts_with('~'))
            .map(|cwd| cwd.join(raw)),
    ]
    .into_iter()
    .flatten()
    .collect();

    candidates.into_iter().find(|p| p.exists())
}

/// 既存リンクと範囲が重複するか判定する
fn overlaps_existing(links: &[DetectedLink], spans: &[(usize, usize, usize)]) -> bool {
    links.iter().any(|link| {
        link.spans.iter().any(|&(row, start, end)| {
            spans
                .iter()
                .any(|&(r, sc, ec)| r == row && start < ec && end > sc)
        })
    })
}

/// ホームディレクトリのヒント（`~/` 展開に使う）。
///
/// 解決は [`crate::paths::home_dir`] 1 本に寄せている（#870）。以前はここが
/// **`HOME` 決め打ち**で、`HOME` を持たない Windows では必ず `None` になり
/// `~/…` のリンクが無反応だった（絶対パスだけ効く、という症状）
fn dirs_hint() -> Option<PathBuf> {
    crate::paths::home_dir()
}

/// リンクが画面上に見えている文字列（cmd+ホバーで下線が付く範囲そのもの）を返す。
/// 行をまたぐ場合は連結する。CLI / MCP の応答へ「何をクリックすることになるか」を
/// 出すために使う（#1283）
pub fn screen_text_of(screen: &Screen, link: &DetectedLink) -> String {
    let mut out = String::new();
    for &(row, start_col, end_col) in &link.spans {
        let Some(line) = screen.lines.get(row) else {
            continue;
        };
        for (idx, ch) in line.text.chars().enumerate() {
            let col = line.cell_cols.get(idx).copied().unwrap_or(idx);
            if col >= start_col && col < end_col {
                out.push(ch);
            }
        }
    }
    out
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
            fract: 0.0,
            extra_bottom: None,
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
    fn 小文字化で伸びる文字が手前にあってもurlを取り違えない() {
        // `İ`(U+0130) は小文字化で 2 → 3 バイトに伸びる。小文字化した写しの
        // バイト位置を元テキストの位置として流用すると、URL の切り出しがずれる（#1016）
        let screen = make_screen(&["İstanbul https://example.com/x"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com/x");
    }

    #[test]
    fn 小文字化で縮む文字が手前にあってもurlを取り違えない() {
        // `ẞ`(U+1E9E) は小文字化で 3 → 2 バイトに縮む
        let screen = make_screen(&["ẞ https://example.com/y"], 80);
        let links = detect_links(&screen);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com/y");
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
        let Some(home) = crate::paths::home_dir() else {
            eprintln!("skip: この環境はホームを解決できない（HOME / USERPROFILE とも空）");
            return;
        };
        let test_file = home.join(".tako_test_link_detect");
        std::fs::write(&test_file, "").unwrap();
        let screen = make_screen(&["cat ~/.tako_test_link_detect"], 80);
        let links = detect_links_with_cwd(&screen, Some(Path::new("/tmp")));
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(path_links.len(), 1);
        assert_eq!(path_links[0].target, test_file.to_str().unwrap());
        let _ = std::fs::remove_file(&test_file);
    }

    #[test]
    fn cwd不明でも絶対パスとホーム起点は検出する() {
        let dir = setup_test_dir("without_cwd");
        let abs = dir.join("README.md");
        let Some(home) = crate::paths::home_dir() else {
            eprintln!("skip: この環境はホームを解決できない（HOME / USERPROFILE とも空）");
            cleanup_test_dir(&dir);
            return;
        };
        let line = format!("{} ~/", abs.display());
        let screen = make_screen(&[&line], 240);
        let links = detect_links_with_cwd(&screen, None);
        let targets: Vec<_> = links
            .iter()
            .filter(|link| link.kind == LinkKind::Path)
            .map(|link| link.target.as_str())
            .collect();
        assert!(targets.contains(&abs.to_str().unwrap()));
        assert!(
            targets
                .iter()
                .any(|target| Path::new(target) == Path::new(&home)),
            "targets={targets:?}"
        );

        let relative = make_screen(&["src/main.rs"], 80);
        assert!(detect_links_with_cwd(&relative, None).is_empty());
        cleanup_test_dir(&dir);
    }

    #[test]
    fn tuiの装飾付きsoft_wrapをまたぐパスを検出する() {
        use crate::screen::StyleRun;

        let dir = setup_test_dir("tui_wrapped");
        let abs = dir.join("deep/nested/file.txt");
        let quoted = format!("`{}`", abs.display());
        let cols = 24;
        let (first, second) = quoted.split_at(cols);
        let mut screen = make_screen(&[first, second], cols);
        for line in &mut screen.lines {
            line.runs.push(StyleRun {
                range: 0..line.text.len(),
                fg: crate::Theme::default().foreground,
                bg: None,
                bold: true,
                italic: false,
                underline: false,
                strikeout: false,
                dim: false,
            });
        }

        let links = detect_links_with_cwd(&screen, None);
        let link = links
            .iter()
            .find(|link| link.kind == LinkKind::Path)
            .expect("折り返された絶対パスを検出する");
        assert_eq!(link.target, abs.to_string_lossy());
        assert_eq!(link.spans.len(), 2);
        assert_eq!(link.spans[0].0, 0);
        assert_eq!(link.spans[1].0, 1);
        cleanup_test_dir(&dir);
    }

    /// **実画面と同じ形**（行末まで空白で埋まった `text` + 全列ぶんの `cell_cols`）の
    /// スクリーンを作る。`screen::compose_line` は未使用セルも押し込むので、
    /// 実機の `ScreenLine` はこの形になる（#1182）
    fn make_padded_screen(lines: &[&str], cols: usize) -> Screen {
        Screen {
            cols,
            rows: lines.len(),
            lines: lines
                .iter()
                .map(|text| {
                    // 幅は「列」で数える（全角は 2 列）
                    let mut cell_cols: Vec<usize> = Vec::new();
                    let mut col = 0usize;
                    let mut padded = String::new();
                    for ch in text.chars() {
                        cell_cols.push(col);
                        padded.push(ch);
                        col += if ch.is_ascii() { 1 } else { 2 };
                    }
                    // 行末まで空白で埋める（実画面と同じ）
                    while col < cols {
                        cell_cols.push(col);
                        padded.push(' ');
                        col += 1;
                    }
                    let has_wide = cell_cols.windows(2).any(|w| w[1] - w[0] > 1);
                    ScreenLine {
                        text: padded,
                        runs: Vec::new(),
                        cell_cols,
                        has_wide,
                    }
                })
                .collect(),
            cursor: None,
            ime_cursor: None,
            display_offset: 0,
            fract: 0.0,
            extra_bottom: None,
        }
    }

    /// #1182: **隣り合う行がどちらも空でない実画面**でパスを検出できること。
    ///
    /// 旧実装は soft wrap の判定に「空白詰めを含む最終セルの列」を使っていたため、
    /// 実画面ではどの行も「右端まで埋まっている」= 折り返し扱いになり、画面全体が
    /// 改行なしの 1 本へ連結された。結果、パストークンが次の行の先頭と融合して
    /// 実在しなくなり、**ターミナル内のパスリンクが 1 つも検出されなかった**
    /// （プロンプト行が続く実画面では常に踏む）
    #[test]
    fn 空白詰めの実画面でも隣接行のパスを検出する() {
        let dir = setup_test_dir("padded");
        // **画面に出す形は `/` 区切り**にする（`is_path_like` は `/` を含まない
        // Windows のバックスラッシュ形式を候補にしない = #153 の既存制約。
        // ここで見たいのは soft wrap の判定なので、両 OS で成立する形で書く）
        let lines = [
            // 素のファイル名は候補にならない（`is_path_like`）ので `./` を付ける
            "./README.md 更新",
            "src/main.rs:12 で失敗",
            // 続くプロンプト行（実画面では空でない行が必ず来る）
            "~ ❯",
        ];
        let screen = make_padded_screen(&lines, 120);
        let links = detect_links_with_cwd(&screen, Some(dir.as_path()));
        let targets: Vec<String> = links
            .iter()
            .filter(|l| l.kind == LinkKind::Path)
            .map(|l| l.target.clone())
            .collect();
        let a = dir.join("README.md");
        let b = dir.join("src/main.rs");
        assert!(
            targets.iter().any(|t| std::path::Path::new(t) == a),
            "1 行目のパスが検出されない: {targets:?}"
        );
        assert!(
            targets.iter().any(|t| std::path::Path::new(t) == b),
            "2 行目のパスが検出されない: {targets:?}"
        );
        // ヒットテスト（クリック位置）も行ごとに正しく引ける
        assert_eq!(
            link_at(&links, 0, 2).map(|l| std::path::Path::new(&l.target).to_path_buf()),
            Some(a)
        );
        assert_eq!(
            link_at(&links, 1, 2).map(|l| std::path::Path::new(&l.target).to_path_buf()),
            Some(b)
        );
        cleanup_test_dir(&dir);
    }

    /// 空白詰めでも**本当に右端まで埋まった行**は従来どおり次行へ直結する（折り返し）。
    ///
    /// 折り返しを起こすには**絶対パスの長さ**が要るので unix 限定
    /// （Windows のバックスラッシュ形式は `is_path_like` が候補にしない = #153 の既存制約）
    #[cfg(unix)]
    #[test]
    fn 空白詰めでも右端まで埋まった行は折り返しとして繋ぐ() {
        let dir = setup_test_dir("padded_wrap");
        let abs = dir.join("deep/nested/file.txt");
        let quoted = format!("`{}`", abs.display());
        let cols = 24;
        let (first, second) = quoted.split_at(cols);
        // 1 行目はちょうど 24 列（詰める余地が無い）、2 行目は空白詰めされる
        let screen = make_padded_screen(&[first, second], cols);
        let links = detect_links_with_cwd(&screen, None);
        let link = links
            .iter()
            .find(|l| l.kind == LinkKind::Path)
            .expect("折り返された絶対パスを検出する");
        assert_eq!(link.target, abs.to_string_lossy());
        assert_eq!(link.spans.len(), 2);
        cleanup_test_dir(&dir);
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

    #[test]
    fn 区切り文字だけの画面でも走査が停止する() {
        let screen = make_screen(&["() [] {} <> ,; plain"], 80);
        assert!(detect_links_with_cwd(&screen, None).is_empty());
    }

    // ---- #1283: 日本語の地の文に埋まった `~` 始まりのパス ----

    /// 仮のホームを temp に作り、`~/Desktop/tako-promo/tako-explainer-v4.mp4` を置く。
    /// プロセスの `HOME` は触らない（[`detect_in_lines`] へホームを渡す）
    fn setup_fake_home(name: &str) -> std::path::PathBuf {
        let home =
            std::env::temp_dir().join(format!("tako_links_1283_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("Desktop/tako-promo")).unwrap();
        std::fs::write(home.join("Desktop/tako-promo/tako-explainer-v4.mp4"), "").unwrap();
        home
    }

    fn path_targets(links: &[DetectedLink]) -> Vec<String> {
        links
            .iter()
            .filter(|l| l.kind == LinkKind::Path)
            .map(|l| l.target.clone())
            .collect()
    }

    /// #1283 の実物 3 形。**master のペインに出ていた形そのまま**を材料にする。
    ///
    /// 旧実装は 1 も 2 も落ちた（トークンの区切りが ASCII の空白と `()[]{}<>,;` だけなので、
    /// 日本語の地の文にパスが埋まると**文ごと 1 トークン**になり、実在チェックで落ちる）。
    /// 実測のトークン: 1 = `動画は`~/…mp4`、確認して。` / 2 = `~/…mp4このリンクが`
    #[test]
    fn 地の文に埋まったホーム起点のパスを検出する_1283() {
        let home = setup_fake_home("three_forms");
        let want = home.join("Desktop/tako-promo/tako-explainer-v4.mp4");
        let forms = [
            // 1. master の応答内: バッククォート囲み + 前後に全角句読点
            "動画は`~/Desktop/tako-promo/tako-explainer-v4.mp4`、確認して。",
            // 2. ユーザーの入力行: 区切り無しで CJK が続く
            "> ~/Desktop/tako-promo/tako-explainer-v4.mp4このリンクが CMD＋クリックで飛べない",
            // 3. 素の形
            "~/Desktop/tako-promo/tako-explainer-v4.mp4",
        ];
        for (i, line) in forms.iter().enumerate() {
            let links = detect_in_lines(&[line.to_string()], 120, None, Some(home.as_path()));
            let targets = path_targets(&links);
            assert_eq!(
                targets.len(),
                1,
                "形 {} が 1 本のリンクにならない: targets={targets:?}",
                i + 1
            );
            assert_eq!(
                std::path::Path::new(&targets[0]),
                want.as_path(),
                "形 {} の解決先が違う",
                i + 1
            );
            // 下線・クリック判定に使うスパンが**パスの部分だけ**を指すこと
            // （地の文まで下線が伸びない）
            let path_link = links
                .iter()
                .find(|l| l.kind == LinkKind::Path)
                .expect("パスリンク");
            let width: usize = path_link.spans.iter().map(|&(_, sc, ec)| ec - sc).sum();
            assert_eq!(
                width,
                "~/Desktop/tako-promo/tako-explainer-v4.mp4".len(),
                "形 {} のスパン幅がパスの長さと違う: spans={:?}",
                i + 1,
                path_link.spans
            );
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 全角の括弧・読点を**区切り文字にしてはいけない**（実在するファイル名に現れる）。
    /// 候補は「トークン全体 → 削った形」の順なので、この形は今までどおり引ける
    #[test]
    fn 全角記号を含む実在ファイル名は落とさない_1283() {
        let home = setup_fake_home("fullwidth_name");
        let dir = home.join("Desktop");
        std::fs::write(dir.join("資料（最新）.pdf"), "").unwrap();
        let links = detect_in_lines(
            &["~/Desktop/資料（最新）.pdf を見て".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        let targets = path_targets(&links);
        assert_eq!(targets.len(), 1, "targets={targets:?}");
        assert_eq!(
            std::path::Path::new(&targets[0]),
            dir.join("資料（最新）.pdf").as_path()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 日本語のファイル名の**後ろに**地の文が続く形も、実在する側が採られる
    #[test]
    fn 日本語ファイル名の後ろに地の文が続いても検出する_1283() {
        let home = setup_fake_home("cjk_name_then_prose");
        std::fs::write(home.join("Desktop/読み込み.txt"), "").unwrap();
        let links = detect_in_lines(
            &["~/Desktop/読み込み.txtを開いて".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        let targets = path_targets(&links);
        assert_eq!(targets.len(), 1, "targets={targets:?}");
        assert_eq!(
            std::path::Path::new(&targets[0]),
            home.join("Desktop/読み込み.txt").as_path()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// エッジ: 存在しない `~/…` / `~user/…` 形 / 行番号つき / 空白を含むパス
    #[test]
    fn ホーム起点のエッジケース_1283() {
        let home = setup_fake_home("edges");
        std::fs::write(home.join("Desktop/読み 込み.txt"), "").unwrap();

        // 存在しないものはリンクにしない（地の文を削った断片も実在しない）
        let links = detect_in_lines(
            &["~/nope.mp4 は無い".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        assert!(path_targets(&links).is_empty(), "存在しない `~/nope.mp4`");

        // `~user/…` はホーム展開しない（ユーザー名の解決はしない = リンクにしない）
        let links = detect_in_lines(
            &["~testuser/Desktop/tako-promo/tako-explainer-v4.mp4".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        assert!(path_targets(&links).is_empty(), "`~user/` 形は展開しない");

        // 行番号つき（`:12`）は行番号を落として解決する。地の文が続いても同じ
        let links = detect_in_lines(
            &["~/Desktop/tako-promo/tako-explainer-v4.mp4:12 の位置".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        let targets = path_targets(&links);
        assert_eq!(targets.len(), 1, "targets={targets:?}");
        assert_eq!(
            std::path::Path::new(&targets[0]),
            home.join("Desktop/tako-promo/tako-explainer-v4.mp4")
                .as_path()
        );

        // 空白を含むパスは引用符で囲まれていれば引ける（従来どおり）
        let links = detect_in_lines(
            &["`~/Desktop/読み 込み.txt` を開く".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        let targets = path_targets(&links);
        assert_eq!(targets.len(), 1, "targets={targets:?}");
        assert_eq!(
            std::path::Path::new(&targets[0]),
            home.join("Desktop/読み 込み.txt").as_path()
        );

        // **囲みの前に地の文がくっついていても引ける**（#1283）。
        // 引用符は「トークンの先頭に在るとき」しか開き記号にならないので、
        // 途中のバッククォートで切らないと**空白でトークンが割れて**囲みが効かない
        let links = detect_in_lines(
            &["見て`~/Desktop/読み 込み.txt`、開いて。".to_string()],
            120,
            None,
            Some(home.as_path()),
        );
        let targets = path_targets(&links);
        assert_eq!(targets.len(), 1, "targets={targets:?}");
        assert_eq!(
            std::path::Path::new(&targets[0]),
            home.join("Desktop/読み 込み.txt").as_path()
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// #1283: `--text` の行が幅を超えたら**切り捨てずに折り返す**（実端末と同じ）。
    /// 黙って切ると囲みの閉じ記号が落ちて判定が変わる
    #[test]
    fn 幅を超える画面テキストは折り返す_1283() {
        let home = setup_fake_home("wrap");
        let rel = "Desktop/tako-promo/a-fairly-long-file-name.txt";
        std::fs::write(home.join(rel), "").unwrap();
        let line = format!("見て`~/{rel}`、開いて。");
        // 1 行に収まらない狭い幅にする（切り捨てなら閉じ記号が落ちて引けなくなる）
        let links = detect_in_lines(&[line], 30, None, Some(home.as_path()));
        let targets = path_targets(&links);
        assert_eq!(targets.len(), 1, "折り返しても引ける: {targets:?}");
        assert_eq!(std::path::Path::new(&targets[0]), home.join(rel).as_path());
        // 折り返したぶんスパンが行をまたぐ
        let link = links
            .iter()
            .find(|l| l.kind == LinkKind::Path)
            .expect("パスリンク");
        assert!(link.spans.len() >= 2, "spans={:?}", link.spans);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 候補の切り出し規則そのもの（実在チェック抜き）。
    /// **トークン全体が必ず先頭**で、削った候補は長い順に並ぶ
    #[test]
    fn 候補は長い順でトークン全体が先頭_1283() {
        let token = "動画は`~/Desktop/x.mp4`、確認して。";
        let cands = path_candidates_in(token, false);
        assert_eq!(cands[0], (0, token), "先頭はトークン全体");
        assert!(
            cands.iter().any(|&(_, c)| c == "~/Desktop/x.mp4"),
            "地の文を削った候補が出る: {cands:?}"
        );
        let widths: Vec<usize> = cands.iter().map(|&(_, c)| c.len()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] >= w[1]),
            "長い順に並ぶ: {widths:?}"
        );

        // 全 ASCII のトークンでは候補を増やさない（実在チェックの syscall を増やさない）
        assert_eq!(path_candidates_in("src/main.rs:42:5", false).len(), 1);

        // `/` で終わる断片は採らない（存在するディレクトリへ誤って飛ばさないため）
        let cands = path_candidates_in("~/Desktop/tako-promo/存在しない", false);
        assert!(
            cands.iter().all(|&(_, c)| !c.ends_with('/')),
            "`/` 終わりの候補が混ざる: {cands:?}"
        );
    }
}
