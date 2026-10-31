//! ターミナル画面上のリンク検出（URL / ファイルパス）
//!
//! `Screen` のテキストから URL やパスを検出し、グリッド座標のスパンとして返す。
//! GPUI 非依存。UI 層は検出結果を使って cmd+ホバー下線や cmd+クリック開く処理を行う。

use crate::platform::support::Platform;
use crate::screen::{Screen, ScreenLine};
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
    /// 行をまたいだときの折り返しの種類（`None` = 1 行に収まっている。#2578）
    pub wrap: Option<LinkWrap>,
}

/// リンクの種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Url,
    Path,
}

/// リンクがまたいだ折り返しの種類（#2578）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkWrap {
    /// 端末の折り返し（行が右端まで埋まって次の行の先頭へ続いた）
    Soft,
    /// アプリ自身の折り返し（claude などが改行して字下げした続き。字下げは飛ばしてつなぐ）
    App,
}

impl LinkWrap {
    /// CLI / MCP の応答の綴り
    pub fn as_str(self) -> &'static str {
        match self {
            LinkWrap::Soft => "soft",
            LinkWrap::App => "app",
        }
    }
}

/// 画面上のリンクを検出する（URL のみ。パス検出不要な呼び出し用）
pub fn detect_links(screen: &Screen) -> Vec<DetectedLink> {
    detect_all(screen, None, legacy_1283(), legacy_2578())
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
    detect_wrapped_in(screen, cwd, home, legacy, legacy_2578())
}

/// #2578 の A/B。`TAKO_2578_LEGACY=1` で**同一バイナリのまま**修正前の結合
/// （右端まで埋まった行だけを次の行の先頭へ直結する）へ戻す
pub fn legacy_2578() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_2578_LEGACY").map(|v| v == "1") == Ok(true))
}

/// [`detect_links_with_home_in`] の本体（#2578 の A/B 用に折り返しの判断も引数で受け取る形）。
/// GUI の cmd+クリック・ホバーの下線・cmd+右クリックのメニューと `tako links` は
/// すべてここを通る（`detect_links_with_cwd` / `detect_links_with_home` の 1 実装）
pub fn detect_wrapped_in(
    screen: &Screen,
    cwd: Option<&Path>,
    home: Option<&Path>,
    legacy: bool,
    wrap_legacy: bool,
) -> Vec<DetectedLink> {
    detect_all(screen, Some((cwd, home)), legacy, wrap_legacy)
}

/// 検出の本体。`paths` が `None` なら URL だけを探す。
///
/// # 折り返しをまたぐリンク（#2578）
///
/// 1 回目は修正前と同じ結合（右端まで埋まった行だけを直結 = [`CombineMode::Base`]）で探す。
/// **折り返しをまたがないリンクの答えはここで決まり、修正前と一致する**。
/// 2 回目は全角の折り返しとアプリ自身の折り返し（claude が改行 + 字下げで続けた形）も
/// つないだ結合（[`CombineMode::Extended`]）で探し、**新しくつないだ境目をまたぐリンク
/// だけ**を、種別ごとに確かめてから採る（パス = 実在する / URL = 行の幅より長い =
/// [`app_wrapped_url_plausible`]）。採ったリンクは 1 回目で途中まで見つかっていた
/// リンクを置き換える（[`merge_wrapped`]）
fn detect_all(
    screen: &Screen,
    paths: Option<(Option<&Path>, Option<&Path>)>,
    legacy: bool,
    wrap_legacy: bool,
) -> Vec<DetectedLink> {
    let cols = screen.cols;
    let base = combined_screen_text(screen, CombineMode::Base);
    let mut found = Vec::new();
    detect_urls_in(&base, cols, wrap_legacy, &mut found);
    // パスの形の判定は実行中の OS の形で行う（#2435。`C:\…` を候補にするのは Windows だけ）
    let scan = paths.map(|(cwd, home)| PathScan {
        cols,
        cwd,
        home,
        legacy,
        wrap_legacy,
        platform: Platform::current(),
    });
    if let Some(scan) = &scan {
        let url_links: Vec<DetectedLink> = found.iter().map(|f| f.link.clone()).collect();
        detect_paths_in(&base, scan, &url_links, false, &mut found);
    }
    let links: Vec<DetectedLink> = found.into_iter().map(|f| f.link).collect();
    if wrap_legacy {
        return links;
    }

    let extended = combined_screen_text(screen, CombineMode::Extended);
    if !extended.seams.iter().any(|(_, seam)| seam.is_new()) {
        return links;
    }
    let mut ext_found = Vec::new();
    detect_urls_in(&extended, cols, false, &mut ext_found);
    let ext_urls: Vec<DetectedLink> = ext_found.iter().map(|f| f.link.clone()).collect();
    let mut wrapped: Vec<DetectedLink> = ext_found
        .into_iter()
        .filter(|f| {
            extended.crosses_new_seam(&f.range) && app_wrapped_url_plausible(&extended, f, cols)
        })
        .map(|f| f.link)
        .collect();
    if let Some(scan) = &scan {
        let mut ext_paths = Vec::new();
        detect_paths_in(&extended, scan, &ext_urls, true, &mut ext_paths);
        wrapped.extend(
            ext_paths
                .into_iter()
                .filter(|f| extended.crosses_new_seam(&f.range))
                .map(|f| f.link),
        );
    }
    merge_wrapped(links, wrapped)
}

/// 画面テキスト（行の配列）からリンクを検出する（#1283）。
///
/// 実画面と同じ**行末まで空白詰めした行**（`screen::compose_line` は未使用セルも
/// 押し込む = #1182）を組み立ててから [`detect_links_with_home`] へ通すので、
/// GUI の cmd+クリックと**同じ判定**になる。CLI / MCP が「この画面テキストで
/// リンクになるパス」を答えるための入口で、人手のクリック無しに検証できる形にするため。
///
/// 列は実グリッドと同じ Unicode の東アジア幅で数える（#2578。以前は「ASCII = 1 列 /
/// それ以外 = 2 列」の近似で、claude の行頭の印 `⏺` `❯` を 2 桁と数えて折り返しの
/// 判定がずれた）。0 幅の結合文字は直前の文字と同じ列に積む。
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
        rows.push(ScreenLine {
            text,
            runs: Vec::new(),
            cell_cols,
        });
    };
    for line in lines {
        let mut cell_cols: Vec<usize> = Vec::new();
        let mut text = String::new();
        let mut col = 0usize;
        for ch in line.chars() {
            // 列は実グリッドと同じ東アジア幅で数える（#2578。alacritty も同じ表を引く）。
            // 以前は「ASCII = 1 / それ以外 = 2」の近似で、claude の行頭の印（`⏺` `❯` = 1 桁）を
            // 2 桁と数えて本文の列が 1 ずれ、アプリの折り返しの続きを見分けられなかった
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            if width == 0 {
                // 0 幅の結合文字は直前の文字と同じ列に積む（実画面と同じ = #1387）
                cell_cols.push(cell_cols.last().copied().unwrap_or(col));
                text.push(ch);
                continue;
            }
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

/// 画面を 1 本の論理テキストへつないだもの。
///
/// URL とパスで同じ規則を使うことで、TUI の装飾付き折り返しでも検出範囲と
/// hit-test 座標を一致させる。
struct Combined {
    text: String,
    /// (combined 上の byte offset, row, col) の写像。改行マーカーは col = 画面の桁数
    byte_map: Vec<(usize, usize, usize)>,
    /// 行の境目でつないだ位置（つないだ先の最初の文字の byte offset）とつなぎ方。
    /// 改行で区切った境目は持たない
    seams: Vec<(usize, Seam)>,
}

/// 行の境目のつなぎ方（#2578）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seam {
    /// 右端まで埋まった行の続き（#1182 以来の物差し）。次の行の先頭から直結する
    Soft,
    /// 全角の折り返し（#2578）。行末の全角が右端まで埋めた形と、右端 1 桁に全角が入らず
    /// 次の行へ送られた形。旧の物差し（最後の可視文字の**開始**列 + 1 が右端か）は
    /// どちらも「埋まっていない」と読んでいた（`conventions.md` #1283 節の既知の限界）
    WideSoft,
    /// アプリ自身の折り返し（#2578）。claude などが語の途中や `/` `-` の手前で切り、
    /// 続きを字下げして次の行に書いた形。**字下げを飛ばして**つなぐ（`indent` は飛ばす
    /// 文字数 = 字下げの桁数）
    App { indent: usize },
}

impl Seam {
    /// 修正前の結合（[`CombineMode::Base`]）には無い境目か
    fn is_new(self) -> bool {
        !matches!(self, Seam::Soft)
    }
}

/// 論理テキストの組み方（#2578）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CombineMode {
    /// 修正前と同じ: 右端まで埋まった行だけを次の行の先頭へ直結する。
    /// **折り返しをまたがないリンクの答えはこの結合だけで決まる**（受け入れ条件 4 の正本）
    Base,
    /// 全角の折り返しとアプリ自身の折り返しもつなぐ。ここで見つかったリンクは
    /// 新しい境目をまたぐものだけを、確かめてから採る（[`detect_wrapped_in`]）
    Extended,
}

/// 画面行を論理テキストへ連結し、各文字の画面座標を記録する。
///
/// 右端まで埋まった行はターミナルの soft wrap とみなして次行へ直結し、それ以外は
/// 改行で区切る（[`CombineMode::Base`]）。[`CombineMode::Extended`] は加えて
/// 全角の折り返しとアプリ自身の折り返し（[`Seam`]）もつなぐ。
fn combined_screen_text(screen: &Screen, mode: CombineMode) -> Combined {
    let mut out = Combined {
        text: String::new(),
        byte_map: Vec::new(),
        seams: Vec::new(),
    };
    // アプリの折り返しの境目（行の塊ごとの上限を見るので、先に画面全体で求める）
    let app = match mode {
        CombineMode::Base => Vec::new(),
        CombineMode::Extended => app_wraps(screen),
    };
    // 前の行とアプリの折り返しでつないだとき、この行の先頭で飛ばす字下げの文字数
    let mut skip = 0usize;
    for (row, line) in screen.lines.iter().enumerate() {
        let trimmed = line.text.trim_end();
        for (char_idx, ch) in trimmed.chars().enumerate().skip(skip) {
            out.byte_map
                .push((out.text.len(), row, char_col(line, char_idx)));
            out.text.push(ch);
        }
        skip = 0;
        let app_indent = app.get(row).copied().flatten();
        match seam_after(screen, row, mode, app_indent) {
            Some(seam) => {
                if let Seam::App { indent } = seam {
                    skip = indent;
                }
                out.seams.push((out.text.len(), seam));
            }
            None => {
                out.byte_map.push((out.text.len(), row, screen.cols));
                out.text.push('\n');
            }
        }
    }
    out
}

/// `text` の `idx` 番目の文字が居る列
fn char_col(line: &ScreenLine, idx: usize) -> usize {
    line.cell_cols.get(idx).copied().unwrap_or(idx)
}

/// 文字の桁数（0 幅の結合文字も 1 と数える = 列の見積もりにだけ使う）
fn char_cells(ch: char) -> usize {
    unicode_width::UnicodeWidthChar::width(ch)
        .unwrap_or(1)
        .max(1)
}

/// 最後の可視文字の右端（exclusive）の列（#2578）。
///
/// 幅はグリッドから採る: 実画面の `ScreenLine` は行末まで空白で埋まっているので、
/// 次のセルの列との差が文字の幅になる（全角は 2）。次のセルが無い = 右端の文字なので、
/// そのときだけ文字の幅（Unicode の東アジア幅）で数える
fn visible_end_col(line: &ScreenLine) -> Option<usize> {
    let trimmed = line.text.trim_end();
    let (idx, ch) = trimmed.chars().enumerate().last()?;
    let col = char_col(line, idx);
    let width = match line.cell_cols.get(idx + 1) {
        Some(&next) if next > col => next - col,
        _ => char_cells(ch),
    };
    Some(col + width)
}

/// 枠・罫線の文字か（表の列の区切り・引用の縦線。リンクの続きにはならない）
fn is_frame_char(ch: char) -> bool {
    matches!(ch, '|' | '\u{2500}'..='\u{259F}')
}

/// 行頭の印（`⏺` `❯` `-` `*` `•` `⎿` `1.` `12)` 等）か。
///
/// 英数字を含まない 1〜2 文字か、数字 + `.` / `)` を印とみなす。パスの始まりになる
/// 文字（`/` `~` `.` 引用符・括弧）は印にしない
fn is_marker(run: &str) -> bool {
    let n = run.chars().count();
    let symbol = (1..=2).contains(&n)
        && run.chars().all(|c| {
            !c.is_alphanumeric()
                && !matches!(
                    c,
                    '/' | '~' | '.' | '\\' | '\'' | '"' | '`' | '(' | '[' | '<' | '{'
                )
        });
    let numbered = (2..=4).contains(&n)
        && run
            .strip_suffix(['.', ')'])
            .is_some_and(|digits| digits.chars().all(|c| c.is_ascii_digit()));
    symbol || numbered
}

/// 行の本文が始まる列（#2578）。行頭の空白を飛ばし、続く印と空白も飛ばす。
///
/// claude の本文は `⏺ ` / `❯ ` / `  - ` の後ろから始まり、折り返した続きはこの列まで
/// 字下げされる。アプリの折り返しの続きは「字下げがこの列と一致する行」だけにする
fn content_col(line: &ScreenLine) -> Option<usize> {
    let trimmed = line.text.trim_end();
    let start = trimmed.chars().position(|c| c != ' ')?;
    let rest: Vec<char> = trimmed.chars().skip(start).collect();
    let run_len = rest.iter().take_while(|c| **c != ' ').count();
    let run: String = rest[..run_len].iter().collect();
    if is_marker(&run) {
        let spaces = rest[run_len..].iter().take_while(|c| **c == ' ').count();
        if spaces > 0 && run_len + spaces < rest.len() {
            return Some(char_col(line, start + run_len + spaces));
        }
    }
    Some(char_col(line, start))
}

/// `next` が形の上で `prev` の続き（アプリ自身の折り返し）に見えるなら、飛ばす字下げの
/// 文字数を返す（#2578。行の上限は見ない = [`app_wraps`] が塊ごとに見る）。
///
/// 続きに見える形（すべて満たす）:
/// - `prev` の最後の可視文字も `next` の本文の最初の文字も、空白・枠の文字ではない
/// - `next` は 1 桁以上字下げされ、その字下げが `prev` の本文の列（[`content_col`]）と一致する
/// - `next` の本文は新しい塊の印（`- ` `⏺ ` `1. `）でも、新しい URL（`https://`）でも始まらない
///
/// ここは**テキストの形**だけを見る。つないだ結果を採るかは種別ごとに確かめる
/// （URL = [`app_wrapped_url_plausible`]・パス = 実在）
fn app_continuation(prev: &ScreenLine, next: &ScreenLine) -> Option<usize> {
    let last = prev.text.trim_end().chars().last()?;
    if last.is_whitespace() || is_frame_char(last) {
        return None;
    }
    let next_trimmed = next.text.trim_end();
    let indent = next_trimmed.chars().take_while(|c| *c == ' ').count();
    // 字下げは ASCII の空白なので byte 数 = 文字数
    let body = &next_trimmed[indent..];
    let first = body.chars().next()?;
    if indent == 0 || is_frame_char(first) {
        return None;
    }
    if content_col(prev) != Some(char_col(next, indent)) {
        return None;
    }
    let word = body.split(' ').next().unwrap_or("");
    if (word.len() < body.len() && is_marker(word)) || find_url_scheme(body) == Some(0) {
        return None;
    }
    Some(indent)
}

/// アプリ自身の折り返しでつなぐ境目（#2578）。`out[row]` は `row` と次の行の境目で飛ばす
/// 字下げの文字数（つながないなら `None`）。
///
/// **行の上限まで詰まった行だけ**をつなぐ。claude 2.1.301 は長い語を `/` `-` の位置に関係なく
/// 行の上限の桁で（語の途中でも）切り、上限は返答（`⏺`）が右端・入力の写し（`❯`）が右端の
/// 1 桁手前（実測）。上限より手前で終わった行の次の行は、空白を挟んだ次の語が入らずに
/// 送られた形 = 別の語なのでつながない（長い URL の最後のかけらの後ろの `done.` を取り込まない）。
///
/// 上限は 1 行では分からない（右端の 1 桁手前で終わった行は、入力の写しなら上限・返答なら
/// 送られた語の手前）ので、形でつながる行の塊（[`app_continuation`] の鎖）ごとに
/// **いちばん長い行の右端**を上限とみなす（長い語を切った行は必ず上限まで詰まる）。
/// 上限が画面の桁数 - 1 より手前の塊（切った行の無い地の文）はつながない。
/// 全角は上限の 1 桁手前に入らなければ次の行へ送られるので、続きが全角なら 1 桁手前も上限とみなす
fn app_wraps(screen: &Screen) -> Vec<Option<usize>> {
    let lines = &screen.lines;
    let shape: Vec<Option<usize>> = (0..lines.len())
        .map(|row| {
            lines
                .get(row + 1)
                .and_then(|next| app_continuation(&lines[row], next))
        })
        .collect();
    let ends: Vec<Option<usize>> = lines.iter().map(visible_end_col).collect();
    let mut out = vec![None; lines.len()];
    let mut start = 0usize;
    while start < lines.len() {
        // 形でつながる行の塊 = start..=last
        let mut last = start;
        while shape[last].is_some() && last + 1 < lines.len() {
            last += 1;
        }
        let limit = (start..=last)
            .filter_map(|row| ends[row])
            .max()
            .unwrap_or(0);
        if limit + 1 >= screen.cols {
            for row in start..last {
                let (Some(indent), Some(end)) = (shape[row], ends[row]) else {
                    continue;
                };
                let wide = lines[row + 1]
                    .text
                    .chars()
                    .nth(indent)
                    .is_some_and(|c| char_cells(c) == 2);
                if end >= limit || (wide && end + 1 == limit) {
                    out[row] = Some(indent);
                }
            }
        }
        start = last + 1;
    }
    out
}

/// `row` の行と次の行の境目のつなぎ方（`None` = 改行で区切る）。
/// `app_indent` は [`app_wraps`] が求めたアプリの折り返しの字下げ（[`CombineMode::Extended`] だけ）
fn seam_after(
    screen: &Screen,
    row: usize,
    mode: CombineMode,
    app_indent: Option<usize>,
) -> Option<Seam> {
    let line = &screen.lines[row];
    let cols = screen.cols;
    let trimmed = line.text.trim_end();
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
    let filled =
        trimmed.chars().count() >= cols || last_visible_col.is_some_and(|col| col + 1 >= cols);
    let next = match (mode, screen.lines.get(row + 1)) {
        (CombineMode::Extended, Some(next)) => next,
        _ => return filled.then_some(Seam::Soft),
    };
    if let Some(indent) = app_indent {
        return Some(Seam::App { indent });
    }
    if filled {
        return Some(Seam::Soft);
    }
    // 全角の折り返し: 端末は字下げせずに行頭から続ける
    let end = visible_end_col(line)?;
    let first = next.text.chars().next().filter(|c| !c.is_whitespace())?;
    (end >= cols || (end + 1 == cols && char_cells(first) == 2)).then_some(Seam::WideSoft)
}

/// 検出の途中結果（論理テキスト上の byte 範囲つき）
struct Found {
    link: DetectedLink,
    range: Range<usize>,
}

impl Combined {
    /// byte 範囲がまたぐ境目（範囲の内側 = 先頭より後ろ・末尾より前にあるもの）
    fn seams_within(&self, range: &Range<usize>) -> impl Iterator<Item = (usize, Seam)> + '_ {
        let (start, end) = (range.start, range.end);
        self.seams
            .iter()
            .copied()
            .filter(move |&(offset, _)| start < offset && offset < end)
    }

    /// 修正前の結合に無い境目（[`Seam::is_new`]）をまたぐか
    fn crosses_new_seam(&self, range: &Range<usize>) -> bool {
        self.seams_within(range).any(|(_, seam)| seam.is_new())
    }

    /// byte 範囲がどの折り返しをまたいだか。アプリの折り返しを 1 つでもまたげば
    /// `App`、端末の折り返しだけなら `Soft`
    fn wrap_of(&self, range: &Range<usize>) -> Option<LinkWrap> {
        let mut wrap = None;
        for (_, seam) in self.seams_within(range) {
            match seam {
                Seam::App { .. } => return Some(LinkWrap::App),
                Seam::Soft | Seam::WideSoft => wrap = Some(LinkWrap::Soft),
            }
        }
        wrap
    }

    fn spans(&self, range: &Range<usize>, cols: usize) -> Vec<(usize, usize, usize)> {
        byte_offsets_to_spans(&self.byte_map, range.start, range.end, cols)
    }

    fn found(
        &self,
        kind: LinkKind,
        target: String,
        range: Range<usize>,
        cols: usize,
    ) -> Option<Found> {
        let spans = self.spans(&range, cols);
        if spans.is_empty() {
            return None;
        }
        let wrap = self.wrap_of(&range);
        Some(Found {
            link: DetectedLink {
                kind,
                target,
                spans,
                wrap,
            },
            range,
        })
    }
}

/// URL（http:// / https://）を検出する。
/// 行末で折り返された URL は次行の先頭と連結して 1 つの URL として扱う。
///
/// `wrap_legacy` は #2578 の A/B（真なら、つないだ境目の直後の別の URL で切らない）
fn detect_urls_in(combined: &Combined, cols: usize, wrap_legacy: bool, out: &mut Vec<Found>) {
    let text = &combined.text;
    let mut search_start = 0;
    while search_start < text.len() {
        // http:// または https:// を探す
        let Some(scheme_offset) = find_url_scheme(&text[search_start..]) else {
            break;
        };
        let abs_start = search_start + scheme_offset;

        // URL の終端を見つける
        let mut url_end = find_url_end(text, abs_start);
        // #2578: つないだ境目の直後で**別の URL が始まる**なら、そこで切る。
        // 行末ちょうどで終わった URL の次の行に別の URL が来る形（隣り合う 2 本）を
        // 1 本へ融合させない（`:` と `/` は URL の文字なので、終端の判定では止まらない）
        let next_url_at_seam = combined
            .seams_within(&(abs_start..url_end))
            .find(|&(offset, _)| find_url_scheme(&text[offset..]) == Some(0));
        if let Some((seam, _)) = next_url_at_seam.filter(|_| !wrap_legacy) {
            url_end = strip_url_trailing(text, abs_start, seam);
        }

        if url_end - abs_start > 10 {
            let target = text[abs_start..url_end].to_string();
            if let Some(found) = combined.found(LinkKind::Url, target, abs_start..url_end, cols) {
                out.push(found);
            }
        }

        search_start = url_end.max(abs_start + 1);
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

    strip_url_trailing(text, start, end)
}

/// URL の末尾の句読点を剥がした終端を返す（"http://example.com." のピリオド等）
fn strip_url_trailing(text: &str, start: usize, mut end: usize) -> usize {
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

/// パスの検出の材料（[`detect_paths_in`] の引数をまとめたもの）
struct PathScan<'a> {
    cols: usize,
    cwd: Option<&'a Path>,
    home: Option<&'a Path>,
    /// #1283 の A/B
    legacy: bool,
    /// #2578 の A/B
    wrap_legacy: bool,
    platform: Platform,
}

/// ファイル/ディレクトリパスを検出する。
/// 行ごとにパスらしきトークンを抽出し、cwd 基準の相対解決 / `~` 展開 / 絶対パスで
/// 実在チェックに通ったものだけリンク化する。URL と重複する範囲はスキップ。
///
/// `only_new_seams` が真なら、修正前の結合に無い境目（[`Seam::is_new`]）をまたぐ
/// トークンだけを調べる（#2578 の 2 回目の走査。実在チェックの syscall を増やさない）
fn detect_paths_in(
    combined: &Combined,
    scan: &PathScan<'_>,
    existing_links: &[DetectedLink],
    only_new_seams: bool,
    out: &mut Vec<Found>,
) {
    let PathScan {
        cols,
        cwd,
        home,
        legacy,
        wrap_legacy,
        platform,
    } = *scan;
    for (token, byte_range) in extract_path_tokens(&combined.text, legacy, platform) {
        if only_new_seams && !combined.crosses_new_seam(&byte_range) {
            continue;
        }
        let full_spans = combined.spans(&byte_range, cols);
        if full_spans.is_empty() || overlaps_existing(existing_links, &full_spans) {
            continue;
        }

        // 地の文が食い込んだトークンに備えて候補を長い順に試す（#1283）。
        // トークン全体が最初の候補なので、今まで引けていた形は定義上そのまま引ける
        let mut hit = false;
        for (rel_start, candidate) in path_candidates_in(&token, legacy, platform) {
            // 行番号サフィックスを分離（`src/main.rs:42:5` → `src/main.rs`）
            let path_part = strip_line_col_suffix(candidate);
            let Some(resolved) = resolve_path(path_part, cwd, home, platform) else {
                continue;
            };
            let start = byte_range.start + rel_start;
            let target = resolved.to_string_lossy().into_owned();
            if let Some(found) =
                combined.found(LinkKind::Path, target, start..start + candidate.len(), cols)
            {
                out.push(found);
                hit = true;
                break;
            }
        }
        if !hit && !wrap_legacy && !split_at_seams(combined, scan, &token, &byte_range, out) {
            file_before_last_seam(combined, scan, &token, &byte_range, out);
        }
    }
}

/// つないだトークンが実在しないとき、**最後の境目の手前が実在するファイル**ならそこまでを返す（#2578）。
///
/// 行より長いパスの最後のかけらがたまたま行の上限ちょうどで終わり、後ろの語（`done.`）だけが
/// 次の行へ送られた形は、テキストだけでは語の途中で切った形と見分けられず、つなぐと実在しなく
/// なってパスが 1 本も返らなかった。採るのは**ファイル**で、境目の後ろが区切り（`/`、Windows は `\` も）で
/// 始まらないときだけ（ディレクトリは採らない = `…/src` + `-old/x.txt` のように折り返した実在しない
/// パスの途中で、別のディレクトリを開いてしまうため）
fn file_before_last_seam(
    combined: &Combined,
    scan: &PathScan<'_>,
    token: &str,
    byte_range: &Range<usize>,
    out: &mut Vec<Found>,
) {
    let Some((offset, _)) = combined.seams_within(byte_range).last() else {
        return;
    };
    let cut = offset - byte_range.start;
    let rest = &token[cut..];
    if rest.starts_with('/') || (scan.platform == Platform::Windows && rest.starts_with('\\')) {
        return;
    }
    let prefix = &token[..cut];
    let resolved = is_path_like(prefix, scan.platform)
        .then(|| {
            resolve_path(
                strip_line_col_suffix(prefix),
                scan.cwd,
                scan.home,
                scan.platform,
            )
        })
        .flatten()
        .filter(|path| path.is_file());
    if let Some(resolved) = resolved {
        let target = resolved.to_string_lossy().into_owned();
        let range = byte_range.start..byte_range.start + cut;
        if let Some(found) = combined.found(LinkKind::Path, target, range, scan.cols) {
            out.push(found);
        }
    }
}

/// つないだトークンが実在しないとき、境目で分けた**どの片も**実在するなら別々のパスとして返す（#2578）。
///
/// 行末ちょうどで終わったパスの次の行が別のパスで始まる形（隣り合う 2 本）は、右端まで
/// 埋まった行を直結する物差しでは 1 本へ融合して実在しなくなり、**どちらもリンクに
/// ならなかった**。片方だけ実在する形は採らない（本当に折り返した実在しないパスの
/// 途中までを開いてしまう = #2578 の症状そのものになるため）
fn split_at_seams(
    combined: &Combined,
    scan: &PathScan<'_>,
    token: &str,
    byte_range: &Range<usize>,
    out: &mut Vec<Found>,
) -> bool {
    let mut cuts: Vec<usize> = combined
        .seams_within(byte_range)
        .map(|(offset, _)| offset - byte_range.start)
        .collect();
    if cuts.is_empty() {
        return false;
    }
    cuts.push(token.len());
    let mut pieces: Vec<(Range<usize>, PathBuf)> = Vec::new();
    let mut start = 0usize;
    for end in cuts {
        let piece = &token[start..end];
        let resolved = is_path_like(piece, scan.platform)
            .then(|| {
                resolve_path(
                    strip_line_col_suffix(piece),
                    scan.cwd,
                    scan.home,
                    scan.platform,
                )
            })
            .flatten();
        let Some(resolved) = resolved else {
            return false;
        };
        pieces.push((byte_range.start + start..byte_range.start + end, resolved));
        start = end;
    }
    for (range, resolved) in pieces {
        let target = resolved.to_string_lossy().into_owned();
        if let Some(found) = combined.found(LinkKind::Path, target, range, scan.cols) {
            out.push(found);
        }
    }
    true
}

/// アプリの折り返しをまたいでつないだ URL を採ってよいか（#2578）。
///
/// アプリ（claude の描画等）は語の境目で折り返し、**語の途中で切るのはその語が 1 行に
/// 収まらないときだけ**（収まる URL は行ごと次の行へ送る = 2.1.301 の実測）。だから本当に
/// 途中で切れた URL は、字下げを除いた行の幅より長い。行末でちょうど終わった URL の
/// 次の行に地の文が続く形（たまたま行末で終わっただけ）は、つないでも行の幅に収まるので採らない
fn app_wrapped_url_plausible(combined: &Combined, found: &Found, cols: usize) -> bool {
    let width = found.link.target.chars().map(char_cells).sum::<usize>();
    combined
        .seams_within(&found.range)
        .all(|(_, seam)| match seam {
            Seam::App { indent } => width > cols.saturating_sub(indent),
            Seam::Soft | Seam::WideSoft => true,
        })
}

/// 修正前の結合で見つけたリンクへ、折り返しをまたいで見つけたリンクを重ねる（#2578）。
///
/// 重なった修正前のリンク（途中で切れていたもの）は、最初の 1 本の位置を新しいリンクで
/// 置き換え、残りは外す。重なりが無ければ末尾に足す。新しいリンクが 1 本も無ければ
/// 修正前の答えをそのまま返す（受け入れ条件 4）
fn merge_wrapped(mut base: Vec<DetectedLink>, wrapped: Vec<DetectedLink>) -> Vec<DetectedLink> {
    for link in wrapped {
        let overlapped: Vec<usize> = base
            .iter()
            .enumerate()
            .filter(|(_, old)| overlaps_existing(std::slice::from_ref(*old), &link.spans))
            .map(|(idx, _)| idx)
            .collect();
        match overlapped.split_first() {
            Some((&first, rest)) => {
                base[first] = link;
                for &idx in rest.iter().rev() {
                    base.remove(idx);
                }
            }
            None => base.push(link),
        }
    }
    base
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

/// [`path_candidates`] の本体（判断を引数で受け取る形。env は唯一の差）。
/// `platform` はパスの形の判定に使う（#2435。Windows では `\` も区切り）
fn path_candidates_in(token: &str, legacy: bool, platform: Platform) -> Vec<(usize, &str)> {
    let mut out = vec![(0usize, token)];
    // 地の文が食い込む余地があるのは「ASCII だけで書かれていないトークン」だけ。
    // 全 ASCII のトークンでは候補を増やさない（実在チェックの syscall を増やさない）
    if legacy || token.is_ascii() {
        return out;
    }
    let starts = candidate_starts(token, platform);
    let ends = candidate_ends(token, platform);
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
        if is_path_like(sub, platform) {
            out.push((start, sub));
        }
    }
    out
}

/// パスの本体に現れうる文字か。
///
/// 非 ASCII は「本体にも地の文にも現れる」ので**本体扱いにしない**
/// （切り出し位置の手がかりとして使う）。引用符・バッククォートも本体扱いしない。
/// `\` は Windows でだけ本体（区切り）として扱う（#2435。macOS ではファイル名の 1 文字）
fn is_path_body_char(ch: char, platform: Platform) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '.' | '_' | '-' | '~' | '/' | '+' | '=' | '%' | '@' | '#' | '$' | '&' | '!' | ':'
        )
        || (ch == '\\' && platform == Platform::Windows)
}

/// 先頭に地の文が食い込んでいる場合の切り出し位置（先頭 = 0 を必ず含む）。
///
/// **絶対パス / ホーム起点 / `./` `../` が始まる位置だけ**を候補にする
/// （途中の `/` から切ると意味の無い断片が大量に増える）。Windows では
/// `C:\` `C:/` `\\`（UNC）`.\` `..\` `~\` の始まりも足す（#2435）
fn candidate_starts(token: &str, platform: Platform) -> Vec<usize> {
    let mut starts = vec![0usize];
    let mut prev_is_body = true;
    for (idx, ch) in token.char_indices() {
        if idx > 0 && !prev_is_body {
            let rest = &token[idx..];
            let windows_start = platform == Platform::Windows
                && (has_drive_prefix(rest)
                    || rest.starts_with(r"\\")
                    || rest.starts_with(r".\")
                    || rest.starts_with(r"..\")
                    || rest.starts_with(r"~\"));
            if rest.starts_with("~/")
                || rest.starts_with('/')
                || rest.starts_with("./")
                || rest.starts_with("../")
                || windows_start
            {
                starts.push(idx);
                if starts.len() > MAX_CUTS {
                    break;
                }
            }
        }
        prev_is_body = is_path_body_char(ch, platform);
    }
    starts
}

/// 末尾に地の文が食い込んでいる場合の切り出し位置（末尾 = 長さを必ず含む）。
///
/// 「本体の文字 → 本体でない文字」へ切り替わる位置だけを候補にする
/// （`…mp4このリンクが` なら `こ` の位置 1 つだけ）。`/`（Windows では `\` も）で
/// 終わる候補は**パスの区切りを跨いで切った断片**なので採らない（存在する
/// ディレクトリへ誤って飛ばさないため）
fn candidate_ends(token: &str, platform: Platform) -> Vec<usize> {
    let mut ends = vec![token.len()];
    let mut prev_is_body = false;
    for (idx, ch) in token.char_indices() {
        let is_body = is_path_body_char(ch, platform);
        let ends_at_separator = token[..idx].ends_with('/')
            || (platform == Platform::Windows && token[..idx].ends_with('\\'));
        if !is_body && prev_is_body && idx >= 2 && !ends_at_separator {
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
/// `/` `.` `~` で始まるか、内部に `/` を含む空白区切りトークンを候補とする
/// （Windows の形は [`is_path_like`] を参照）。
///
/// `legacy` は #1283 の A/B（`TAKO_1283_LEGACY=1` で修正前の切り出しへ戻す）
fn extract_path_tokens(
    text: &str,
    legacy: bool,
    platform: Platform,
) -> Vec<(String, Range<usize>)> {
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

        // パスらしさの判定: `/` `.` `~` で始まるか、内部に `/` を含む（Windows は `\` の形も）。
        // #2435: 地の文が前にくっついた Windows の形（`見てC:\…`）はトークン全体だと
        // `:` がドライブ以外の位置に在るので形の判定に通らない。地の文を削った候補
        // （#1283）にパスらしい形が在れば拾う（`/` の形はトークン全体が `/` を含むので
        // 従来どおり上の判定で通る = macOS の答えは変わらない）
        let windows_embedded = platform == Platform::Windows
            && !legacy
            && !token_text.is_ascii()
            && path_candidates_in(&token_text, legacy, platform).len() > 1;
        if is_path_like(&token_text, platform) || windows_embedded {
            // 末尾のコロンや句読点を剥がす（`path/to/file:` 等）
            let cleaned = token_text.trim_end_matches([':', '.', ',']);
            if !cleaned.is_empty() {
                tokens.push((cleaned.to_string(), byte_start..byte_start + cleaned.len()));
            }
        }
    }
    tokens
}

/// トークンがパスらしいかの簡易判定。
///
/// `/` の形（下記）は両 OS 共通。Windows ではさらに `\` の形（[`is_windows_path_like`]）も
/// 候補にする（#2435）。**macOS では `\` を区切りとして扱わない**（ファイル名に
/// 使える 1 文字なので、`\` を含む語を候補にすると判定が変わる）
fn is_path_like(token: &str, platform: Platform) -> bool {
    if token.is_empty() {
        return false;
    }
    let first = token.chars().next().unwrap();
    // `/` `~` `./` `../` で始まる、または内部に `/` を含む
    let slash_form = first == '/'
        || first == '~'
        || token.starts_with("./")
        || token.starts_with("../")
        || (token.contains('/') && !token.contains("://"));
    slash_form || (platform == Platform::Windows && is_windows_path_like(token))
}

/// Windows のファイル名に使えない文字（`:` はドライブと `:行:桁` があるので別に見る）。
/// これを含む語は glob・正規表現・リダイレクトであってパスではない
const WINDOWS_INVALID_CHARS: [char; 6] = ['*', '?', '"', '<', '>', '|'];

/// `C:\` / `C:/` で始まるか（ドライブレターの絶対パス。#2435）
fn has_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
}

/// Windows の絶対パスの形か（`C:\…` / `C:/…` / `\\server\share\…` / `\\?\C:\…`）。
/// 実在チェックの前に「cwd へ繋がず、そのまま引く」を決めるのに使う
fn is_windows_absolute(s: &str) -> bool {
    has_drive_prefix(s) || s.starts_with(r"\\")
}

/// Windows の `\` の形のパスらしいか（#2435）。
///
/// 候補にする形:
/// - ドライブレター（`C:\…` / `C:/…`）・UNC（`\\server\share…`）・`\\?\C:\…`
/// - `.\` `..\` `~\` で始まる相対
/// - それ以外の `\` を含む相対（`src\main.rs:10:5`）は、**最後の名前に拡張子があるか
///   `:行` が付くときだけ**。`Hello\nWorld` のようなエスケープ入りの文と見分ける
///   手がかりが他に無いため（拡張子の無いディレクトリは `.\src` の形なら候補になる）
///
/// 候補にしない形: `\` で始まる語（`\n` `\d+` などのエスケープ・正規表現）/
/// 途中に `\\` を含む語（エスケープされた文字列。Debug 表示の `C:\\Users\\…` も含む）/
/// `*?"<>|` を含む語 / ドライブ以外の位置に `:` を含む語
fn is_windows_path_like(token: &str) -> bool {
    // 行末の句読点と `:行:桁` を外した本体で見る（`src\main.rs:4:5:` の形）
    let trimmed = token.trim_end_matches([':', '.', ',']);
    let body = strip_line_col_suffix(trimmed);
    let has_line = body.len() < trimmed.len();
    // 絶対パスの形なら、接頭辞の後ろ（`rest`）だけを検査する
    let rest = if let Some(after) = body
        .strip_prefix(r"\\?\")
        .filter(|after| has_drive_prefix(after))
    {
        // 切り出し安全: has_drive_prefix が先頭 3 バイトを ASCII（`C:\`）と確かめている
        Some(&after[2..])
    } else if has_drive_prefix(body) {
        // 切り出し安全: has_drive_prefix が先頭 3 バイトを ASCII（`C:\`）と確かめている
        Some(&body[2..])
    } else if let Some(after) = body.strip_prefix(r"\\") {
        // UNC は `\\server\share` の 2 つが揃って初めて絶対パス
        let mut parts = after.split('\\');
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        if server.is_empty() || share.is_empty() {
            return false;
        }
        Some(after)
    } else {
        None
    };
    let checked = rest.unwrap_or(body);
    if checked.contains(WINDOWS_INVALID_CHARS) || checked.contains(':') || checked.contains(r"\\") {
        return false;
    }
    if rest.is_some()
        || body.starts_with(r".\")
        || body.starts_with(r"..\")
        || body.starts_with(r"~\")
    {
        return true;
    }
    if !body.contains('\\') || body.starts_with('\\') || body.ends_with('\\') {
        return false;
    }
    has_line || body.rsplit('\\').next().is_some_and(has_file_extension)
}

/// `main.rs` のように拡張子があるか（`.gitignore` のような先頭ドットだけの名前は除く）
fn has_file_extension(name: &str) -> bool {
    name.rfind('.').is_some_and(|dot| {
        let ext = &name[dot + 1..];
        dot > 0 && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
    })
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
/// 実在するものを返す。Windows では `~\` の展開と `C:\…` / UNC の絶対パスも扱う（#2435）
fn resolve_path(
    raw: &str,
    cwd: Option<&Path>,
    home: Option<&Path>,
    platform: Platform,
) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() < 2 {
        return None;
    }
    let windows = platform == Platform::Windows;

    // `~` 展開
    let expanded = if raw.starts_with("~/") || raw == "~" || (windows && raw.starts_with(r"~\")) {
        // 切り出し安全: 先頭 2 バイトは ASCII の `~/` / `~\`（`~` 単独は上の len < 2 で返している）
        home.map(|h| h.join(&raw[2..]))
    } else {
        None
    };

    // 試す順序: 展開済み → 絶対パス → cwd 相対
    let absolute = raw.starts_with('/') || (windows && is_windows_absolute(raw));
    let candidates: Vec<PathBuf> = [
        expanded,
        absolute.then(|| PathBuf::from(raw)),
        cwd.filter(|_| !absolute && !raw.starts_with('~'))
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

    /// fixture が `dir.join(...)` で作る絶対パスは Windows では `C:\…` の形。
    /// #2435 からはこれも候補になるので両 OS で走る（以前は Windows で skip していた）
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

    /// cwd が無くても絶対パス（Windows では `C:\…` = #2435）とホーム起点は引ける
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

    /// 折り返しの結合は OS 非依存。fixture の絶対パスは Windows では `C:\…` の形（#2435）
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

    /// #1388: 「行に全角が在るか」の旗を落としても、`screen_from_lines` が組む
    /// `text` / `cell_cols` は 1 文字も変わらない。
    ///
    /// 旗の書き手は `cell_cols.windows(2)` を畳んでいただけなので `text` /
    /// `cell_cols` に影響しないが、**右端だけが全角の行**（旧の物差しが構造的に
    /// 見落としていた形）・空行・全角だけの行を列まで固定して、描画と
    /// `tako links` の材料がここで変わらないことを見る
    #[test]
    fn screen_from_linesの列は全角のぶん飛ぶ_1388() {
        let dump = |line: &str, cols: usize| -> (String, Vec<usize>) {
            let s = screen_from_lines(&[line.to_string()], cols);
            let l = &s.lines[0];
            eprintln!(
                "i1388 cols={cols} in={line:?} text={:?} cell_cols={:?}",
                l.text, l.cell_cols
            );
            (l.text.clone(), l.cell_cols.clone())
        };

        // 右端だけが全角（次の可視文字が無いので、旧 `windows(2)` 版は旗を false にしていた）
        let (text, cols_of) = dump("abcdefghあ", 10);
        assert_eq!(text, "abcdefghあ");
        assert_eq!(cols_of, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
        // 空行は全列ぶんの空白詰め（#1182）
        let (text, cols_of) = dump("", 4);
        assert_eq!(text, "    ");
        assert_eq!(cols_of, vec![0, 1, 2, 3]);
        // 全角だけの行は 2 列ずつ飛び、余りは空白で埋まる
        let (text, cols_of) = dump("ああああ", 10);
        assert_eq!(text, "ああああ  ");
        assert_eq!(cols_of, vec![0, 2, 4, 6, 8, 9]);
        // 途中に全角（旧の物差しでも拾えていた形）
        let (text, cols_of) = dump("あbc", 5);
        assert_eq!(text, "あbc ");
        assert_eq!(cols_of, vec![0, 2, 3, 4]);
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
                    ScreenLine {
                        text: padded,
                        runs: Vec::new(),
                        cell_cols,
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
        // **画面に出す形は `/` 区切り**にする（ここで見たいのは soft wrap の判定なので、
        // 両 OS で同じ答えになる形で書く。`\` の形は `windowsの形…_2435` の単体が見る）
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
    /// 折り返しを起こすには**絶対パスの長さ**が要る。Windows の `C:\…` も #2435 から
    /// 候補になるので両 OS で走る（以前は unix 限定）
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
        // `/` の形は両 OS で同じ答え
        for platform in [Platform::MacOs, Platform::Windows] {
            assert!(is_path_like("/usr/bin/env", platform));
            assert!(is_path_like("~/config", platform));
            assert!(is_path_like("./local", platform));
            assert!(is_path_like("../parent", platform));
            assert!(is_path_like("src/main.rs", platform));
            assert!(is_path_like("C:/Users/winuser/x.txt", platform));
            assert!(!is_path_like("plain", platform));
            assert!(!is_path_like("http://example.com", platform));
            assert!(!is_path_like("", platform));
        }
        // `\` の形は Windows だけ（macOS では `\` はファイル名の 1 文字。#2435）
        for token in [r"C:\Users\winuser", r"src\main.rs:10:5", r"\\server\share"] {
            assert!(is_path_like(token, Platform::Windows), "{token}");
            assert!(!is_path_like(token, Platform::MacOs), "{token}");
        }
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

    /// #1387: NFD（結合文字）のファイル名が**実ペインの画面**から
    /// 実在チェックまで通ってリンクになること。
    ///
    /// macOS のファイル名には NFD が混ざる（HFS+ 由来・アーカイブ展開物）。
    /// 画面テキストから結合文字が落ちると、`links` は**実在しないパス**を見るので
    /// Cmd+クリックが無反応になる（#153 / #1283 で通した経路がここで死ぬ）。
    /// 画面は合成せず `Term` → `screen::snapshot` の実経路から組む
    #[cfg(unix)]
    #[test]
    fn nfdのファイル名が画面からリンクになる_1387() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::{test::TermSize, Config};
        use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
        use alacritty_terminal::Term;

        let dir = crate::test_residue::ScratchDir::new("i1387-links");
        // `画像か` + U+3099（濁点）+ `ある.txt` = macOS の NFD ファイル名
        let name = "画像か\u{3099}ある.txt";
        let file = dir.path().join(name);
        std::fs::write(&file, "x").expect("NFD 名のファイルを作れる");
        assert!(file.exists(), "作ったファイルが実在しない: {file:?}");

        let line = format!("open {} please", file.display());
        let mut term = Term::new(Config::default(), &TermSize::new(220, 3), VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        parser.advance(&mut term, line.as_bytes());
        let screen = crate::screen::snapshot(&term, &crate::theme::Theme::default_dark());
        assert!(
            screen.lines[0].text.contains('\u{3099}'),
            "画面テキストから濁点が落ちている: {:?}",
            screen.lines[0].text
        );

        let links = detect_links_with_home(&screen, Some(dir.path()), None);
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(
            path_links.len(),
            1,
            "NFD のファイル名がリンクにならない: {links:?}"
        );
        assert_eq!(path_links[0].target, file.to_string_lossy());
        assert!(
            std::path::Path::new(&path_links[0].target).exists(),
            "リンク先が実在しない（結合文字が落ちた形を掴んでいる）: {:?}",
            path_links[0].target
        );
        // 検出力の確認: 濁点を落とした形は実在しない = 落ちれば必ずリンクにならない
        let dropped: String = path_links[0]
            .target
            .chars()
            .filter(|c| *c != '\u{3099}')
            .collect();
        assert!(
            !std::path::Path::new(&dropped).exists(),
            "濁点を落としたパスが実在してしまい、このテストに検出力が無い: {dropped:?}"
        );
    }

    /// #1387: `tako links --text` の経路（画面テキストの行を受け取る
    /// [`detect_in_lines`]）でも同じ NFD のパスが引けること。
    /// GUI の cmd+クリックと CLI / MCP の答えを食い違わせない
    #[cfg(unix)]
    #[test]
    fn nfdのファイル名はtext経路でもリンクになる_1387() {
        let dir = crate::test_residue::ScratchDir::new("i1387-links-text");
        let file = dir.path().join("画像か\u{3099}ある.txt");
        std::fs::write(&file, "x").expect("NFD 名のファイルを作れる");

        let line = format!("open {} please", file.display());
        let links = detect_in_lines(&[line], 220, Some(dir.path()), None);
        let path_links: Vec<_> = links.iter().filter(|l| l.kind == LinkKind::Path).collect();
        assert_eq!(
            path_links.len(),
            1,
            "text 経路で NFD のファイル名がリンクにならない: {links:?}"
        );
        assert_eq!(path_links[0].target, file.to_string_lossy());
    }

    /// 候補の切り出し規則そのもの（実在チェック抜き）。
    /// **トークン全体が必ず先頭**で、削った候補は長い順に並ぶ
    #[test]
    fn 候補は長い順でトークン全体が先頭_1283() {
        let token = "動画は`~/Desktop/x.mp4`、確認して。";
        let cands = path_candidates_in(token, false, Platform::MacOs);
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
        assert_eq!(
            path_candidates_in("src/main.rs:42:5", false, Platform::MacOs).len(),
            1
        );

        // `/` で終わる断片は採らない（存在するディレクトリへ誤って飛ばさないため）
        let cands = path_candidates_in("~/Desktop/tako-promo/存在しない", false, Platform::MacOs);
        assert!(
            cands.iter().all(|&(_, c)| !c.ends_with('/')),
            "`/` 終わりの候補が混ざる: {cands:?}"
        );
    }

    // ---- #2435: Windows の `\` の形のパス ----

    /// 1 行のテキストから、実在チェックに掛ける候補（`:行:桁` を外した形）を出現順に返す。
    /// 実在チェックの手前までは `platform` を引数で受ける純関数なので、
    /// **macOS 上から Windows の形を検査できる**
    fn path_parts_on(line: &str, platform: Platform) -> Vec<String> {
        extract_path_tokens(line, false, platform)
            .iter()
            .flat_map(|(token, _)| {
                path_candidates_in(token, false, platform)
                    .into_iter()
                    .map(|(_, c)| strip_line_col_suffix(c).to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// cargo / rustc / Python / Node / MSVC / PowerShell が Windows で出す形が
    /// 1 本ずつ候補になる。macOS では同じ行から `\` の候補は出ない（`\` はファイル名の 1 文字）
    #[test]
    fn windowsの形はplatformを渡すと候補になる_2435() {
        let cases: &[(&str, &str)] = &[
            // rustc / cargo のエラー位置
            (r"   --> src\main.rs:10:5", r"src\main.rs"),
            (
                r"thread 'main' panicked at src\main.rs:4:5:",
                r"src\main.rs",
            ),
            // Python のトレースバック（引用符で囲まれる）
            (
                r#"  File "C:\Users\winuser\proj\main.py", line 10, in <module>"#,
                r"C:\Users\winuser\proj\main.py",
            ),
            // Node のスタック（括弧で囲まれる）
            (
                r"    at Object.<anonymous> (C:\Users\winuser\proj\index.js:10:5)",
                r"C:\Users\winuser\proj\index.js",
            ),
            // MSVC（`(行,桁)` は括弧で切れる）
            (
                r"C:\proj\main.cpp(10,5): error C2065: 'x': undeclared identifier",
                r"C:\proj\main.cpp",
            ),
            // PowerShell のプロンプト
            (r"PS C:\Users\winuser\proj>", r"C:\Users\winuser\proj"),
            // `/` 区切りのドライブ（MinGW の gcc など。両 OS で候補）
            (
                "C:/Users/winuser/proj/main.c:3:1: error",
                "C:/Users/winuser/proj/main.c",
            ),
            // UNC と `\\?\`
            (
                r"\\fileserver\share\docs\spec.md",
                r"\\fileserver\share\docs\spec.md",
            ),
            (
                r"\\?\C:\Users\winuser\proj\Cargo.toml",
                r"\\?\C:\Users\winuser\proj\Cargo.toml",
            ),
            // `.\` `..\` `~\` で始まる相対は拡張子が無くても候補
            (r"cd .\scripts", r".\scripts"),
            (r"..\other\lib.rs", r"..\other\lib.rs"),
            (r"~\proj\notes", r"~\proj\notes"),
            // エッジ: 行末の `:` / 括弧で囲まれたパス / 拡張子が無くても `:行` が付けば候補
            (r"error in src\lib.rs:", r"src\lib.rs"),
            (r"(src\lib.rs:3)", r"src\lib.rs"),
            (r"src\Makefile:12", r"src\Makefile"),
            // 空白を含むパスは引用符で囲まれていれば 1 本
            (
                r#"open "C:\Program Files\tako\tako.exe" now"#,
                r"C:\Program Files\tako\tako.exe",
            ),
        ];
        for &(line, want) in cases {
            let parts = path_parts_on(line, Platform::Windows);
            assert_eq!(parts, vec![want.to_string()], "line={line}");
            // macOS の答えは変えない（`~` 始まりは修正前から macOS でも候補 = 展開しないので
            // リンクにはならない。それ以外の `\` の形は macOS では候補にならない）
            if want.contains('\\') && !want.starts_with('~') {
                let mac = path_parts_on(line, Platform::MacOs);
                assert!(
                    !mac.iter().any(|p| p == want),
                    "macOS で `\\` の形が候補になった: {line} → {mac:?}"
                );
            }
        }

        // 日本語のファイル名 + 前後に地の文（#1283 の削り方が `\` の形でも効く）
        let parts = path_parts_on(
            r"見てC:\Users\winuser\資料\メモ.txtを開いて",
            Platform::Windows,
        );
        assert!(
            parts.contains(&r"C:\Users\winuser\資料\メモ.txt".to_string()),
            "{parts:?}"
        );
        // `\` で終わる断片（区切りを跨いで切った形）は採らない
        assert!(parts.iter().all(|p| !p.ends_with('\\')), "{parts:?}");
        // 全角の括弧で囲まれた形
        let parts = path_parts_on(r"（C:\proj\main.rs:10:5）を見て", Platform::Windows);
        assert!(parts.contains(&r"C:\proj\main.rs".to_string()), "{parts:?}");

        // 空白を含むパスを引用符なしで出すと空白で割れる（`/` の形と同じ扱い）。
        // 割れた前半 `C:\Program` は実在しないのでリンクにならない
        let parts = path_parts_on(r"C:\Program Files\tako\tako.exe", Platform::Windows);
        assert_eq!(
            parts,
            vec![
                r"C:\Program".to_string(),
                r"Files\tako\tako.exe".to_string()
            ]
        );
    }

    /// URL・`\n` を含む文・エスケープ・正規表現・glob は Windows でも `\` の候補にしない
    #[test]
    fn windowsでも誤検出は候補にしない_2435() {
        let lines = [
            // URL（`\` が混ざっても URL は URL）
            r"see https://example.com/a\b.html",
            // `\n` を含む普通の文・エスケープ
            r#"println!("Hello\nWorld");"#,
            r"line1\nline2\n",
            r#"echo "done\n""#,
            r"\n",
            r"\t",
            r"\r\n",
            // 正規表現
            r"\d+\.\d+",
            r"^\s*(\w+)$",
            r"foo\.bar",
            r"[A-Z]:\\",
            // エスケープされた文字列（Debug 表示の二重バックスラッシュ）
            r#""C:\\Users\\winuser\\x.toml""#,
            r"a\\b\\c.txt",
            // glob
            r"src\*.rs",
            // ドメイン\ユーザー・レジストリ（拡張子の無い相対 / `:` 入り）
            r"DOMAIN\winuser",
            r"HKLM\Software\Microsoft",
            r"HKLM:\Software",
            // `\` だけ・UNC の片割れ
            r"\",
            r"\\",
            r"\\server",
        ];
        for line in lines {
            let parts = path_parts_on(line, Platform::Windows);
            assert!(parts.is_empty(), "誤検出: {line} → {parts:?}");
        }
    }

    /// 修正前の結合でパスだけを探す（OS の形を引数で渡す単体の入口。#2435）
    fn detect_paths(
        screen: &Screen,
        cwd: Option<&Path>,
        home: Option<&Path>,
        existing: &[DetectedLink],
        out: &mut Vec<DetectedLink>,
        legacy: bool,
        platform: Platform,
    ) {
        let scan = PathScan {
            cols: screen.cols,
            cwd,
            home,
            legacy,
            wrap_legacy: false,
            platform,
        };
        let mut found = Vec::new();
        let base = combined_screen_text(screen, CombineMode::Base);
        detect_paths_in(&base, &scan, existing, false, &mut found);
        out.extend(found.into_iter().map(|f| f.link));
    }

    /// `~\` は Windows でだけホームへ展開する（実在チェックまで通す。ホームは引数で注入）。
    /// 1 階層の名前にしているのは、macOS では `\` がファイル名の 1 文字で、
    /// `~\a\b` を `join` すると別のパスになるため
    #[test]
    fn windowsではホーム起点の円記号も展開する_2435() {
        let home = setup_fake_home("win_tilde");
        std::fs::write(home.join("notes.txt"), "").unwrap();
        let screen = make_screen(&[r"see ~\notes.txt:3 now"], 80);

        let mut on_windows = Vec::new();
        detect_paths(
            &screen,
            None,
            Some(home.as_path()),
            &[],
            &mut on_windows,
            false,
            Platform::Windows,
        );
        let targets = path_targets(&on_windows);
        assert_eq!(targets.len(), 1, "targets={targets:?}");
        assert_eq!(Path::new(&targets[0]), home.join("notes.txt").as_path());
        // 下線は `:3` まで（`/` の形の `path:行` と同じ）
        assert_eq!(on_windows[0].spans, vec![(0, 4, 17)]);

        // macOS では展開しない（`~\` は `~` で始まる名前の 1 つ）
        let mut on_macos = Vec::new();
        detect_paths(
            &screen,
            None,
            Some(home.as_path()),
            &[],
            &mut on_macos,
            false,
            Platform::MacOs,
        );
        assert!(path_targets(&on_macos).is_empty(), "{on_macos:?}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Windows の実ファイルシステムで、cargo / rustc / Python / Node の出力の `\` の形が
    /// 実在チェックまで通ってリンクになる（#2435）。
    ///
    /// **Windows 限定**: `\` を区切りとして扱うのは Windows だけで、macOS では
    /// `cwd.join("src\\main.rs")` が別の 1 ファイル名になり実在チェックで落ちる。
    /// 同じ形の切り出しは `windowsの形はplatformを渡すと候補になる_2435` が macOS 上から見る
    #[cfg(windows)]
    #[test]
    fn windowsの出力のパスが実在チェックまで通ってリンクになる_2435() {
        let dir = setup_test_dir("win2435");
        let main_rs = dir.join("src").join("main.rs");
        let readme = dir.join("README.md");
        let cjk = dir.join("資料.txt");
        std::fs::write(&cjk, "").unwrap();
        let spaced = dir.join("Program Files").join("tool.exe");
        std::fs::create_dir_all(spaced.parent().unwrap()).unwrap();
        std::fs::write(&spaced, "").unwrap();

        let cases = [
            (r"   --> src\main.rs:10:5".to_string(), main_rs.clone()),
            (
                r"thread 'main' panicked at src\main.rs:4:5:".to_string(),
                main_rs.clone(),
            ),
            (
                format!("  File \"{}\", line 10, in <module>", readme.display()),
                readme.clone(),
            ),
            (
                format!("    at Object.<anonymous> ({}:10:5)", main_rs.display()),
                main_rs.clone(),
            ),
            (format!("見て{}を開いて", cjk.display()), cjk.clone()),
            (format!("open \"{}\" now", spaced.display()), spaced.clone()),
            (
                r".\deep\nested\file.txt".to_string(),
                dir.join("deep").join("nested").join("file.txt"),
            ),
        ];
        for (line, want) in &cases {
            let links = detect_in_lines(std::slice::from_ref(line), 240, Some(&dir), None);
            let targets = path_targets(&links);
            assert_eq!(targets.len(), 1, "line={line} targets={targets:?}");
            assert_eq!(Path::new(&targets[0]), want.as_path(), "line={line}");
        }

        // 実在しないものはリンクにならない
        let links = detect_in_lines(
            &[r"--> src\missing.rs:1:1".to_string()],
            240,
            Some(&dir),
            None,
        );
        assert!(path_targets(&links).is_empty());
        cleanup_test_dir(&dir);
    }

    // --- #2578: 折り返しをまたぐ URL / パス ---------------------------------------------
    //
    // 材料の形は claude 2.1.301 を 124 桁のペインで動かした実測から採った
    // （`scripts/test-wrapped-links-2578.sh` の claude の節で同じ形を実物に描かせる）:
    // - 返答（`⏺ `）は右端の桁まで詰めて語の途中で切るか、`/` `-` の手前で切る。続きは 2 桁の字下げ
    // - 入力の写し（`❯ `）は右端の 1 桁手前で切る。続きは 2 桁の字下げ
    // - 1 行に収まる URL / パスは切らずに行ごと次の行へ送る

    /// 実グリッドと同じ列の数え方（Unicode の東アジア幅）で、行末まで空白詰めした画面を作る
    fn grid_screen(lines: &[String], cols: usize) -> Screen {
        Screen {
            cols,
            rows: lines.len(),
            lines: lines
                .iter()
                .map(|text| {
                    let mut cell_cols = Vec::new();
                    let mut padded = String::new();
                    let mut col = 0usize;
                    for ch in text.chars() {
                        cell_cols.push(col);
                        padded.push(ch);
                        col += char_cells(ch);
                    }
                    while col < cols {
                        cell_cols.push(col);
                        padded.push(' ');
                        col += 1;
                    }
                    ScreenLine {
                        text: padded,
                        runs: Vec::new(),
                        cell_cols,
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

    /// 端末の自動折り返し（1 行を幅で折る。全角が残り 1 桁に入らなければ次の行へ送る）
    fn soft_rows(line: &str, cols: usize) -> Vec<String> {
        let mut rows = vec![String::new()];
        let mut col = 0usize;
        for ch in line.chars() {
            let width = char_cells(ch);
            if col + width > cols {
                rows.push(String::new());
                col = 0;
            }
            rows.last_mut().unwrap().push(ch);
            col += width;
        }
        rows
    }

    /// claude と同じ形の折り返し: 先頭行は `marker` + 空白 + `lead`、続きは本文の列まで
    /// 字下げして `edge` 桁まで詰める（語の途中で切る。全角は残り 1 桁に入らなければ送る）
    fn app_rows(marker: &str, lead: &str, token: &str, tail: &str, edge: usize) -> Vec<String> {
        let indent = marker.chars().map(char_cells).sum::<usize>() + 1;
        let pad = " ".repeat(indent);
        let mut row = format!("{marker} {lead}");
        let mut col: usize = row.chars().map(char_cells).sum();
        let mut rows = Vec::new();
        for ch in token.chars() {
            let width = char_cells(ch);
            if col + width > edge {
                rows.push(std::mem::replace(&mut row, pad.clone()));
                col = indent;
            }
            row.push(ch);
            col += width;
        }
        if col + 1 + tail.len() > edge {
            rows.push(std::mem::replace(&mut row, format!("{pad}{tail}")));
        } else {
            row.push(' ');
            row.push_str(tail);
        }
        rows.push(row);
        rows
    }

    /// 前後に別の行を挟んだ画面（実画面ではプロンプトや次の発話が必ず並ぶ）
    fn framed(rows: Vec<String>) -> Vec<String> {
        let mut lines = vec!["tako % run".to_string()];
        lines.extend(rows);
        lines.push(String::new());
        lines.push("❯".to_string());
        lines
    }

    /// 修正後と修正前（`TAKO_2578_LEGACY=1` と同じ判断）の答え
    fn both_arms(
        lines: &[String],
        cols: usize,
        cwd: Option<&Path>,
    ) -> (Vec<DetectedLink>, Vec<DetectedLink>) {
        let screen = grid_screen(lines, cols);
        (
            detect_wrapped_in(&screen, cwd, None, false, false),
            detect_wrapped_in(&screen, cwd, None, false, true),
        )
    }

    fn long_url() -> String {
        format!(
            "https://example.com/tako/issues/2578/wrap-check?first={}&second={}#end",
            "a".repeat(40),
            "b".repeat(60)
        )
    }

    /// 呼び手が作った置き場（`setup_test_dir`。後始末も呼び手）へ、長いパスと全角のパスの実物を置く
    fn put_2578_files(dir: &Path) -> (PathBuf, PathBuf) {
        let long = dir
            .join("a-very-long-directory-name-for-the-wrap-test")
            .join("another-quite-long-directory-segment-0123456789")
            .join("target-file-for-the-wrapped-link-check.txt");
        let wide = dir
            .join("日本語のフォルダ名を長くしたもの")
            .join("さらに深い階層のフォルダ")
            .join("資料（最新）の長いファイル名です.txt");
        for path in [&long, &wide] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        (long, wide)
    }

    /// 期待の全体が 1 本だけ返り、途中で切れた target（期待の真の接頭辞）が無い
    fn assert_whole<'a>(links: &'a [DetectedLink], want: &str, ctx: &str) -> &'a DetectedLink {
        let whole: Vec<&DetectedLink> = links.iter().filter(|l| l.target == want).collect();
        let cut: Vec<&str> = links
            .iter()
            .map(|l| l.target.as_str())
            .filter(|t| *t != want && want.starts_with(t))
            .collect();
        assert!(
            whole.len() == 1 && cut.is_empty(),
            "{ctx}: 全体 {} 本・途中で切れた target {cut:?}\n  links={links:#?}",
            whole.len()
        );
        whole[0]
    }

    #[test]
    fn claudeの返答が右端で切って字下げしたurlを1本で返す_2578() {
        let url = long_url();
        let cols = 60;
        let rows = app_rows("⏺", "See ", &url, "for details.", cols);
        assert!(
            rows.len() >= 4,
            "3 行以上にまたがる材料になっていない: {rows:?}"
        );
        let lines = framed(rows);
        let (new, legacy) = both_arms(&lines, cols, None);
        let link = assert_whole(&new, &url, "修正後");
        assert_eq!(link.wrap, Some(LinkWrap::App));
        // またがった各行の位置: 先頭は `⏺ See ` の後ろ（6 桁目）、続きは字下げの後ろ（2 桁目）
        let rows_of: Vec<usize> = link.spans.iter().map(|s| s.0).collect();
        assert_eq!(rows_of, (1..1 + link.spans.len()).collect::<Vec<_>>());
        let lead: usize = "⏺ See ".chars().map(char_cells).sum();
        let indent: usize = "⏺".chars().map(char_cells).sum::<usize>() + 1;
        assert_eq!(link.spans[0].1, lead);
        assert!(
            link.spans[1..].iter().all(|s| s.1 == indent),
            "{:?}",
            link.spans
        );
        // 画面に見えている文字列（下線の範囲）は字下げを含まない = target と一致する
        assert_eq!(screen_text_of(&grid_screen(&lines, cols), link), url);
        // 修正前は 1 行目までで切れていた（#2578 の症状）
        assert!(
            legacy
                .iter()
                .any(|l| l.target != url && url.starts_with(&l.target)),
            "修正前の腕が症状を再現しない: {legacy:#?}"
        );
    }

    #[test]
    fn 入力の写しは右端の1桁手前で切る形も1本で返す_2578() {
        let url = long_url();
        let cols = 70;
        let lines = framed(app_rows("❯", "Echo verbatim: See ", &url, "now", cols - 1));
        let (new, _) = both_arms(&lines, cols, None);
        assert_eq!(
            assert_whole(&new, &url, "右端の 1 桁手前").wrap,
            Some(LinkWrap::App)
        );
    }

    #[test]
    fn 行の上限で切れば区切りの手前でも1本で返し上限より手前で終わった行はつながない_2578() {
        // 2.1.301 の実測（P1 / P8 / P9〜P11）: claude は `/` `-` の位置に関係なく行の上限の桁で
        // 切る。境目がたまたま `/` や `-` の手前に来ても、上限まで詰めた行の続きとしてつなぐ
        let url = "https://example.com/seg-01-abcdefgh/seg-02-abcdefgh/seg-03-abcdefgh/seg-04-abcdefgh/end";
        let head = "⏺ P1 see https://example.com/seg-01-abcdefgh/seg-02-abcdefgh";
        let cols: usize = head.chars().map(char_cells).sum();
        let lines = framed(vec![
            head.to_string(),
            "  /seg-03-abcdefgh/seg-04-abcdefgh/end for more".to_string(),
        ]);
        let (new, _) = both_arms(&lines, cols, None);
        assert_whole(&new, url, "/ の手前で上限");

        let dir = setup_test_dir("2578_hyphen");
        let (long, _) = put_2578_files(&dir);
        let shown = long.display().to_string();
        let cut = shown.find("-0123456789").unwrap();
        let head = format!("⏺ Open {}", &shown[..cut]);
        let cols: usize = head.chars().map(char_cells).sum();
        let lines = framed(vec![head.clone(), format!("  {} next.", &shown[cut..])]);
        let (new, legacy) = both_arms(&lines, cols, None);
        assert_eq!(
            assert_whole(&new, &shown, "- の手前で上限").wrap,
            Some(LinkWrap::App)
        );
        assert!(!legacy.iter().any(|l| l.target == shown), "{legacy:#?}");

        // 上限より 2 桁以上手前で終わった行の次の行は、空白を挟んだ次の語が入らずに送られた形
        // （別の語）。長い URL / パスの最後のかけらの後ろの語を取り込まない
        let (new, legacy) = both_arms(&lines, cols + 2, None);
        assert_eq!(new, legacy, "上限まで詰まっていない行をつないだ");
        // 長い URL の最後のかけらが上限の手前で終わり、後ろの語（`done.`）だけが次の行へ
        // 送られた形（URL は行より長いので、URL の確かめだけでは止まらない）
        let url = long_url();
        let mut seen = 0;
        for cols in 40..=72 {
            let rows = app_rows("⏺", "See ", &url, "done.", cols);
            let short = rows.len() >= 2 && rows[rows.len() - 2].chars().count() + 2 <= cols;
            if rows.last().map(String::as_str) == Some("  done.") && short {
                seen += 1;
                let (new, _) = both_arms(&framed(rows), cols, None);
                assert_whole(&new, &url, &format!("cols={cols}"));
            }
        }
        assert!(seen > 0, "後ろの語だけが送られる幅が 1 つも無い");
        cleanup_test_dir(&dir);
    }

    /// 3 行以上 / 全角の混ざるパス / 幅を変えた直後（折り返し位置が変わる）を、
    /// 端末の折り返しとアプリの折り返しの両方で、**幅を 1 桁ずつ変えて**見る。
    /// 全角が右端にかかる 2 つの形（右端まで埋める / 残り 1 桁に入らず送る）はどちらかの幅で必ず踏む
    #[test]
    fn 幅を変えても折り返した全体を1本で返す_2578() {
        let dir = setup_test_dir("2578_widths");
        let (long, wide) = put_2578_files(&dir);
        let url = long_url();
        let targets = [
            url.clone(),
            long.display().to_string(),
            wide.display().to_string(),
        ];
        let mut wide_edges = 0usize;
        let mut known_limit = 0usize;
        for cols in 40..=72 {
            for want in &targets {
                let is_url = want == &url;
                let soft = framed(soft_rows(&format!("WIDE: {want}"), cols));
                let app = framed(app_rows("⏺", "Also ", want, "done.", cols));
                for (form, lines) in [("soft", &soft), ("app", &app)] {
                    // 既知の限界: URL の最後のかけらがたまたま行の上限ちょうどで終わり、後ろの語だけが
                    // 次の行へ送られた形は、語の途中で切った形と見分けられない（別の単体が固定する）
                    let tail_row = lines.iter().rposition(|l| l == "  done.");
                    if form == "app"
                        && is_url
                        && tail_row.is_some_and(|i| width_of(&lines[i - 1]) == cols)
                    {
                        known_limit += 1;
                        continue;
                    }
                    let (new, _) = both_arms(lines, cols, None);
                    let ctx = format!("{form} cols={cols} want={want}\n{}", lines.join("\n"));
                    let link = assert_whole(&new, want, &ctx);
                    assert_eq!(
                        link.kind,
                        if is_url {
                            LinkKind::Url
                        } else {
                            LinkKind::Path
                        },
                        "{ctx}"
                    );
                    assert!(link.spans.len() >= 2, "{ctx}: 折り返していない材料");
                }
                // 全角の右端の形を踏んだ回数（材料が穴を突いていることの確認）
                if want == &targets[2] {
                    let screen = grid_screen(&soft, cols);
                    wide_edges += combined_screen_text(&screen, CombineMode::Extended)
                        .seams
                        .iter()
                        .filter(|(_, seam)| *seam == Seam::WideSoft)
                        .count();
                }
            }
        }
        assert!(wide_edges > 0, "全角が右端にかかる形を 1 度も踏んでいない");
        // 外したのは限界の形だけ（33 幅のうち数幅 = 行の幅分の 1 程度）
        assert!(
            known_limit <= 3,
            "限界の形で外した幅が多すぎる: {known_limit}"
        );
        cleanup_test_dir(&dir);
    }

    fn width_of(line: &str) -> usize {
        line.chars().map(char_cells).sum()
    }

    /// **既知の限界**（#2578）: 行より長い URL の最後のかけらがたまたま行の上限ちょうどで終わり、
    /// 後ろの語だけが次の行へ送られると、その語までつなぐ。テキストだけでは「語の途中で上限で
    /// 切った」形と見分けられない（パスは実在で確かめるので起きない）。直したらこの単体を反転する
    #[test]
    fn 既知の限界_上限ちょうどで終わった長いurlは次の語までつなぐ_2578() {
        // 1 行目 = `⏺ <lead>` + 語の前半（上限まで）/ 2 行目 = 字下げ + 語の残り（ちょうど上限で終わる）/
        // 3 行目 = 送られた次の語。2 行目がちょうど上限になる桁数を計算で決める
        fn two_rows(lead_a: &str, lead_b: &str, token: &str) -> (Vec<String>, usize) {
            let lead = if (token.len() + 2 + 2 + lead_a.len()).is_multiple_of(2) {
                lead_a
            } else {
                lead_b
            };
            let head = 2 + lead.len();
            let cols = (token.len() + 2 + head) / 2;
            let cut = cols - head;
            let lines = framed(vec![
                format!("⏺ {lead}{}", &token[..cut]),
                format!("  {}", &token[cut..]),
                "  done.".to_string(),
            ]);
            assert_eq!(width_of(&lines[1]), cols, "{lines:?}");
            assert_eq!(width_of(&lines[2]), cols, "{lines:?}");
            (lines, cols)
        }
        let url = long_url();
        let (lines, cols) = two_rows("See ", "See: ", &url);
        let (new, _) = both_arms(&lines, cols, None);
        let targets: Vec<&str> = new.iter().map(|l| l.target.as_str()).collect();
        assert_eq!(
            targets,
            vec![format!("{url}done")],
            "限界の形が変わった（直したなら反転する）"
        );

        // パスは実在で確かめるので、同じ形でもつながない
        let dir = setup_test_dir("2578_limit_path");
        let (long, _) = put_2578_files(&dir);
        let shown = long.display().to_string();
        let (lines, cols) = two_rows("Open ", "Open: ", &shown);
        let (new, _) = both_arms(&lines, cols, None);
        assert_whole(&new, &shown, "上限ちょうどのパス");
        cleanup_test_dir(&dir);
    }

    #[test]
    fn 行末ちょうどで終わる別々の2本はつながない_2578() {
        // URL: 2 本とも幅ちょうど（右端まで埋まる = 端末の折り返しと区別が付かない形）
        let a = "https://a.example.com/first/xxxxxxxxxxxxxxxxxxx";
        let cols = a.len();
        let b_head = "https://b.example.com/second/";
        let b = format!("{b_head}{}", "y".repeat(cols - b_head.len()));
        let b = b.as_str();
        let lines = framed(vec![a.to_string(), b.to_string()]);
        let (new, _) = both_arms(&lines, cols, None);
        assert_whole(&new, a, "1 本目");
        assert_whole(&new, b, "2 本目");
        // アプリの形（字下げした次の行が別の URL で始まる）
        let lines = framed(vec![format!("⏺ {a}"), format!("  {b}")]);
        let (new, _) = both_arms(&lines, cols + 2, None);
        assert_whole(&new, a, "字下げの 1 本目");
        assert_whole(&new, b, "字下げの 2 本目");

        // パス: 1 本目が幅ちょうどで、次の行が別のパスで始まる（両 OS で同じ答えになる相対形）
        let dir = setup_test_dir("2578_two_paths");
        let (one, two) = ("deep/nested/file.txt", "src/main.rs");
        let lines = framed(vec![one.to_string(), two.to_string()]);
        let (new, legacy) = both_arms(&lines, one.len(), Some(&dir));
        assert_whole(&new, &dir.join(one).to_string_lossy(), "パスの 1 本目");
        assert_whole(&new, &dir.join(two).to_string_lossy(), "パスの 2 本目");
        // 修正前は 1 本へ融合して実在せず、どちらもリンクにならなかった
        assert!(path_targets(&legacy).is_empty(), "{legacy:#?}");
        cleanup_test_dir(&dir);
    }

    /// 誤ってつなげない形（受け入れ条件 3）。どれも修正前と同じ答えになる
    #[test]
    fn アプリの折り返しと読まない形はつながない_2578() {
        // 右端ちょうどで終わる URL（`⏺ ` + 前置き + URL = 画面の桁数）
        let url = "https://example.com/docs/page-0123456789";
        let head = format!("⏺ The doc is {url}");
        let cols: usize = head.chars().map(char_cells).sum();
        let cases: Vec<(&str, Vec<String>)> = vec![
            (
                // 文の途中の改行: たまたま右端で終わった URL の次の行に地の文が続く
                // （つないでも行の幅に収まる = アプリが語の途中で切る理由が無い）
                "文の途中の改行",
                vec![head.clone(), "  and the rest of the sentence.".to_string()],
            ),
            (
                // 字下げの違う行（本文の列 2 ではなく 4）
                "字下げの違う行",
                vec![head.clone(), "    deeper indented line".to_string()],
            ),
            (
                // 新しい塊の印で始まる行
                "新しい項目",
                vec![head.clone(), "  - next item".to_string()],
            ),
            (
                // 表の列（右端は枠の文字）
                "表の列",
                vec![
                    format!("│ {url}     │"),
                    "│ second cell text                             │".to_string(),
                ],
            ),
            (
                // 隣り合う別の行（右端に届かない行の次に、字下げの無い行）
                "隣り合う別の行",
                vec![format!("see {url}"), "continued-without-indent".to_string()],
            ),
            (
                // 行末の 1 桁手前で終わり、次の語は残りに入る（P6 の自然な形の縮小版）
                "次の語が入る行",
                vec![format!("⏺ Doc: {url}"), "  ok".to_string()],
            ),
        ];
        for (name, rows) in cases {
            let lines = framed(rows);
            let (new, legacy) = both_arms(&lines, cols, None);
            assert_eq!(new, legacy, "{name}: 修正前と答えが変わった");
            let urls: Vec<&str> = new
                .iter()
                .filter(|l| l.kind == LinkKind::Url)
                .map(|l| l.target.as_str())
                .collect();
            assert_eq!(urls, vec![url], "{name}");
        }

        // パス: つないだら実在しない（右端で終わったパスの次の行に地の文）
        let dir = setup_test_dir("2578_no_join_path");
        let file = dir.join("deep/nested/file.txt").display().to_string();
        let row = format!("⏺ Open {file}");
        let cols = row.chars().map(char_cells).sum::<usize>();
        let lines = framed(vec![row, "  next.".to_string()]);
        let (new, legacy) = both_arms(&lines, cols, None);
        assert_eq!(new, legacy);
        assert_whole(&new, &file, "つなぐと実在しないパス");
        cleanup_test_dir(&dir);
    }

    /// 折り返していないリンクの答えは修正前と 1 ビットも変わらない（受け入れ条件 4）
    #[test]
    fn 折り返していないリンクは修正前と一致する_2578() {
        let dir = setup_test_dir("2578_same");
        let abs = dir.join("deep/nested/file.txt").display().to_string();
        let screens: Vec<(Vec<String>, usize)> = vec![
            (
                vec![
                    "./README.md 更新".into(),
                    "src/main.rs:12 で失敗".into(),
                    "~ ❯".into(),
                ],
                120,
            ),
            (
                vec![
                    "Visit https://example.com for info".into(),
                    "  https://a.com and http://b.com/path end".into(),
                    format!("  ⎿  {abs}:3:5"),
                    format!("    {abs}"),
                ],
                120,
            ),
            (
                vec![
                    "| https://example.com/a | `src/main.rs` |".into(),
                    "見て `src/main.rs`、開いて。".into(),
                    "- item https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
                    "  continued text".into(),
                ],
                80,
            ),
        ];
        for (lines, cols) in screens {
            let (new, legacy) = both_arms(&lines, cols, Some(&dir));
            assert!(!new.is_empty(), "材料にリンクが無い: {lines:?}");
            assert_eq!(new, legacy, "{lines:?}");
            assert!(new.iter().all(|l| l.wrap.is_none()), "{new:#?}");
        }
        cleanup_test_dir(&dir);
    }

    /// `tako links --text` の経路（`screen_from_lines`）でも、実物の claude の画面の写し
    /// （行頭の印 `⏺` `❯` は実画面で 1 桁）をそのまま渡してつなげる
    #[test]
    fn text経路でもclaudeの印のままつなぐ_2578() {
        assert_eq!(
            (char_cells('⏺'), char_cells('❯')),
            (1, 1),
            "実画面の幅と違う"
        );
        let url = long_url();
        let cols = 58;
        let mut lines = app_rows("❯", "Echo verbatim: See ", &url, "now", cols - 1);
        lines.push(String::new());
        lines.extend(app_rows("⏺", "See ", &url, "now", cols));
        let links = detect_in_lines(&lines, cols, None, None);
        let whole: Vec<&DetectedLink> = links.iter().filter(|l| l.target == url).collect();
        assert_eq!(whole.len(), 2, "{links:#?}");
        assert!(whole.iter().all(|l| l.wrap == Some(LinkWrap::App)));
        assert_eq!(links.len(), 2, "途中で切れた URL が残っている: {links:#?}");
    }
}
