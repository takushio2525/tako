//! Screen — Term グリッドの「色解決済みスナップショット」抽出（GPUI 非依存）
//!
//! UI 層が描画にそのまま使える形（行テキスト + スタイルラン）まで tako-core 側で解決する:
//! 256 色 / truecolor / INVERSE / DIM / 選択ハイライト / ブロックカーソルをここで処理し、
//! UI 層はランを描画プリミティブへ写すだけにする。色は必ず [`Theme`] から引く（FR-4）。
//!
//! `Term` を直接受ける純関数なので、PTY を起動せずに ANSI 列を流してテストできる。

use std::ops::Range;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{point_to_viewport, Term};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb as AnsiRgb};

use crate::theme::{Rgb, Theme};

/// DIM（SGR 2）の減光係数
const DIM_FACTOR: f32 = 0.66;

/// 同一スタイルが連続する区間。`range` は行テキスト内のバイト範囲
#[derive(Debug, Clone, PartialEq)]
pub struct StyleRun {
    pub range: Range<usize>,
    pub fg: Rgb,
    /// None はデフォルト背景（描画スキップ可能）
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub dim: bool,
}

/// 1 行分の表示内容
#[derive(Debug, Clone)]
pub struct ScreenLine {
    pub text: String,
    pub runs: Vec<StyleRun>,
    /// `text` の各文字（char 順）が占めるグリッド列。全角文字はスペーサーを
    /// テキストから除いているため、次の文字との col 差が 2 になる。
    /// 描画（プロポーショナルな実フォント幅）とグリッド座標の写像に使う
    pub cell_cols: Vec<usize>,
    /// 行に全角（2 セル幅）文字が含まれるか。描画時にセル幅固定レイアウトへ
    /// 切り替える判定に使う
    pub has_wide: bool,
}

/// 表示中グリッドのスナップショット
#[derive(Debug, Clone)]
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub lines: Vec<ScreenLine>,
    /// ブロックカーソルの表示位置（col, row）。非表示・画面外なら None。
    /// カーソル色はラン側にも反映済みなので、描画はランだけ見れば足りる
    pub cursor: Option<(usize, usize)>,
    /// IME 候補ウィンドウ用カーソル位置。CursorShape::Hidden でもビューポート内なら返す。
    /// カーソルが表示領域外（スクロールバック中）のときだけ None
    pub ime_cursor: Option<(usize, usize)>,
    /// スクロールバック表示中のオフセット（0 = 最下部）
    pub display_offset: usize,
    /// サブライン表示の下方向端数（0.0..1.0 行）。表示位置 = display_offset - fract。
    /// 描画側は行スタック全体を fract 行ぶん上へずらす（ピクセル単位スクロール #159）
    pub fract: f32,
    /// fract > 0 のとき viewport 最下行の 1 行下（上下端の部分行描画用）。
    /// display_offset == 0 か fract == 0 では None
    pub extra_bottom: Option<ScreenLine>,
}

impl Screen {
    /// IME の未確定文字列を重ねる基準セル（#497）。
    ///
    /// 表示中カーソル → IME 用カーソルの順に解決する。claude 等の TUI は
    /// DECTCEM（`\x1b[?25l`）でカーソルを消したまま idle に落ちることがあり、
    /// そのペインでは `cursor` が None になる。**下線オーバーレイも候補ウィンドウも
    /// このフォールバックを使わなければならない**。片方だけ使っていたために、
    /// カーソル非表示ペインで「候補ウィンドウは出るが下線だけ出ない」という
    /// 非対称な壊れ方をしていた（#29 が候補側だけ直した取りこぼし）。
    ///
    /// スクロールバック中はどちらも None になり、アンカーなし = 表示しないが正しい。
    pub fn ime_anchor_cell(&self) -> Option<(usize, usize)> {
        self.cursor.or(self.ime_cursor)
    }
}

/// セル単位の解決済みスタイル（ラン合成前の中間表現）
#[derive(Debug, Clone, PartialEq)]
struct CellStyle {
    fg: Rgb,
    bg: Option<Rgb>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikeout: bool,
    dim: bool,
}

/// Term の表示内容を色解決済みスナップショットへ変換する
pub fn snapshot<T: EventListener>(term: &Term<T>, theme: &Theme) -> Screen {
    snapshot_opts(term, theme, true, 0.0)
}

/// `show_cursor = false` でカーソルセルの強調を抑止する版。
/// tmux copy-mode でスクロール中のバックエンドペインは、tmux が報告する
/// copy-mode カーソルが画面に固定表示されて不自然なため UI 層が隠す
/// （2026-06-12 実機フィードバック (b)）。
/// `fract` はサブライン表示の下方向端数（`TerminalSession::scroll_pixels` が管理）
pub(crate) fn snapshot_opts<T: EventListener>(
    term: &Term<T>,
    theme: &Theme,
    show_cursor: bool,
    fract: f32,
) -> Screen {
    let cols = term.columns();
    let rows = term.screen_lines();
    let content = term.renderable_content();
    let display_offset = content.display_offset;

    let default_style = CellStyle {
        fg: theme.foreground,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
        strikeout: false,
        dim: false,
    };
    // フラット配列（rows 個の内側 Vec 割り当てを回避）
    let mut grid: Vec<(char, CellStyle)> = vec![(' ', default_style.clone()); cols * rows];

    let cursor = (show_cursor && content.cursor.shape != CursorShape::Hidden)
        .then(|| point_to_viewport(display_offset, content.cursor.point))
        .flatten()
        .map(|p| (p.column.0, p.line))
        .filter(|&(col, row)| col < cols && row < rows);

    // IME 用: CursorShape::Hidden でもビューポート内なら位置を返す
    let ime_cursor = point_to_viewport(display_offset, content.cursor.point)
        .map(|p| (p.column.0, p.line))
        .filter(|&(col, row)| col < cols && row < rows);

    let selection = content.selection;
    let colors = content.colors;
    // display_iter が content を部分 move するため、追加行の構築で使う
    // カーソルのグリッド座標はここで取り出しておく
    let cursor_visible_at = (show_cursor && content.cursor.shape != CursorShape::Hidden)
        .then_some(content.cursor.point);

    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let (row, col) = (vp.line, vp.column.0);
        if row >= rows || col >= cols {
            continue;
        }
        let selected = selection.is_some_and(|range| range.contains(indexed.point));
        let is_cursor = cursor == Some((col, row));
        grid[row * cols + col] = resolve_cell(indexed.cell, selected, is_cursor, colors, theme);
    }

    let lines = (0..rows)
        .map(|row| compose_line(&grid[row * cols..(row + 1) * cols]))
        .collect();

    // fract > 0 のとき viewport 最下行の 1 行下を追加で切り出す（部分行の描画用）。
    // display_offset d の viewport は grid の Line(-d..rows-d) なので、その下は Line(rows-d)。
    // d == 0（最下部）では存在しない
    let extra_bottom = (fract > 0.0 && display_offset >= 1).then(|| {
        use alacritty_terminal::index::{Column, Line, Point};
        let line = Line(rows as i32 - display_offset as i32);
        let grid_ref = term.grid();
        let mut cells: Vec<(char, CellStyle)> = Vec::with_capacity(cols);
        for col in 0..cols {
            let point = Point::new(line, Column(col));
            let cell = &grid_ref[line][Column(col)];
            let selected = selection.is_some_and(|range| range.contains(point));
            // カーソルが追加行（viewport の 1 行下）にある場合も焼き込む
            let is_cursor = cursor_visible_at == Some(point);
            cells.push(resolve_cell(cell, selected, is_cursor, colors, theme));
        }
        compose_line(&cells)
    });

    Screen {
        cols,
        rows,
        lines,
        cursor,
        ime_cursor,
        display_offset,
        fract,
        extra_bottom,
    }
}

/// セル 1 つを色解決済みの (文字, スタイル) へ変換する。
/// display_iter のセルと grid 直接アクセスのセル（追加行）で共用する
fn resolve_cell(
    cell: &alacritty_terminal::term::cell::Cell,
    selected: bool,
    is_cursor: bool,
    colors: &Colors,
    theme: &Theme,
) -> (char, CellStyle) {
    let flags = cell.flags;
    let c = if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
        '\0'
    } else {
        cell.c
    };

    let mut fg = resolve_color(&cell.fg, colors, theme);
    let mut bg = resolve_color(&cell.bg, colors, theme);
    if flags.contains(Flags::DIM) {
        fg = fg.dim(DIM_FACTOR);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    let mut bg = (bg != theme.background).then_some(bg);

    if selected {
        bg = Some(theme.selection_background);
    }
    if is_cursor {
        fg = theme.cursor_text;
        bg = Some(theme.cursor);
    }

    (
        c,
        CellStyle {
            fg,
            bg,
            bold: flags.intersects(Flags::BOLD),
            italic: flags.intersects(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strikeout: flags.intersects(Flags::STRIKEOUT),
            dim: flags.contains(Flags::DIM),
        },
    )
}

/// 1 行分のセル列を ScreenLine（text + StyleRun + cell_cols）へ合成する
fn compose_line(cells: &[(char, CellStyle)]) -> ScreenLine {
    let cols = cells.len();
    let mut text = String::with_capacity(cols);
    let mut runs: Vec<StyleRun> = Vec::new();
    let mut cell_cols = Vec::with_capacity(cols);
    for (col, (c, style)) in cells.iter().enumerate() {
        if *c == '\0' {
            continue;
        }
        cell_cols.push(col);
        let start = text.len();
        text.push(*c);
        let end = text.len();
        match runs.last_mut() {
            Some(last)
                if last.fg == style.fg
                    && last.bg == style.bg
                    && last.bold == style.bold
                    && last.italic == style.italic
                    && last.underline == style.underline
                    && last.strikeout == style.strikeout
                    && last.dim == style.dim =>
            {
                last.range.end = end;
            }
            _ => runs.push(StyleRun {
                range: start..end,
                fg: style.fg,
                bg: style.bg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                strikeout: style.strikeout,
                dim: style.dim,
            }),
        }
    }
    let has_wide = cell_cols.windows(2).any(|w| w[1] - w[0] > 1);
    ScreenLine {
        text,
        runs,
        cell_cols,
        has_wide,
    }
}

fn from_ansi(c: AnsiRgb) -> Rgb {
    Rgb::new(c.r, c.g, c.b)
}

/// セルの Color をテーマと OSC 4 等の動的パレット（`colors`）で RGB に解決する
fn resolve_color(color: &Color, colors: &Colors, theme: &Theme) -> Rgb {
    match color {
        Color::Spec(c) => from_ansi(*c),
        Color::Indexed(i) => colors[*i as usize]
            .map(from_ansi)
            .unwrap_or_else(|| theme.indexed_color(*i)),
        Color::Named(n) => colors[*n as usize]
            .map(from_ansi)
            .unwrap_or_else(|| named_color(*n, theme)),
    }
}

fn named_color(n: NamedColor, theme: &Theme) -> Rgb {
    let idx = n as usize;
    if idx < 16 {
        return theme.ansi[idx];
    }
    match n {
        NamedColor::Foreground | NamedColor::BrightForeground => theme.foreground,
        NamedColor::Background => theme.background,
        NamedColor::Cursor => theme.cursor,
        NamedColor::DimForeground => theme.foreground.dim(DIM_FACTOR),
        // DimBlack..=DimWhite は対応する通常色の減光
        _ => theme.ansi[idx - NamedColor::DimBlack as usize].dim(DIM_FACTOR),
    }
}

/// Claude TUI 入力行のテキストの属性分類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStyle {
    /// 全セルが dim — 自動提案（ゴーストテキスト）
    Ghost,
    /// 全セルが non-dim — ユーザー手動入力
    User,
    /// dim / non-dim が混在
    Mixed,
    /// 入力テキストなし（❯ の右が空）
    None,
}

/// Claude TUI 入力行の分析結果
#[derive(Debug, Clone)]
pub struct InputStatus {
    /// ❯ を含む行全体
    pub line: String,
    /// ❯ の右側のテキスト（trim 済み）
    pub text: String,
    /// テキストの属性分類
    pub style: InputStyle,
}

/// Screen の行リストから Claude TUI の入力行（❯）を探し、入力テキストの
/// dim 状態を分析する。❯ 行が見つからなければ None
pub fn analyze_input_line(screen: &Screen) -> Option<InputStatus> {
    // Claude TUI は ❯ の下にフッター（区切り線・モデル情報・ctx%）が 4〜6 行あるため、
    // 末尾 10 行の範囲で最後の ❯ 行を探す（wait.rs の screen_looks_idle と同じ走査範囲）。
    // 起点は「行数」ではなく**中身がある最後の行**にする。ビューポートを埋めない画面では
    // 下端に空行が並び、行数基準だと入力行が範囲外に落ちる（#719 の実スクショで発覚）
    let bottom = screen
        .lines
        .iter()
        .rposition(|l| !l.text.trim().is_empty())
        .map(|i| i + 1)?;
    let start = bottom.saturating_sub(10);
    let mut found: Option<usize> = None;
    for i in start..bottom {
        if screen.lines[i].text.trim_start().starts_with('❯') {
            found = Some(i);
        }
    }
    analyze_input_line_at(screen, found?)
}

/// 行 index を指定して入力行を分析する（#719）。
///
/// チャット入力欄は [`input_region`] が決めた**まさにその行**を見る必要がある。
/// 探し直すと走査範囲の違いで「箱は見つかったのに入力テキストは無いことになる」
/// という食い違いが起きる（実スクショでプレースホルダが本文に重なって発覚した）
pub fn analyze_input_line_at(screen: &Screen, line_idx: usize) -> Option<InputStatus> {
    let line = screen.lines.get(line_idx)?;
    let trimmed = line.text.trim_start();
    if !trimmed.starts_with('❯') {
        return None;
    }
    let prompt_byte_pos = line.text.len() - trimmed.len();
    let full_line = line.text.trim_end().to_string();

    // ❯ の右側のテキストを抽出
    let after_prompt = &line.text[prompt_byte_pos..];
    let after_char = after_prompt
        .strip_prefix('❯')
        .unwrap_or(after_prompt)
        .trim_start();
    let input_text = after_char.trim_end().to_string();

    if input_text.is_empty() {
        return Some(InputStatus {
            line: full_line,
            text: input_text,
            style: InputStyle::None,
        });
    }

    // 入力テキスト部分のバイト範囲を特定
    // ❯ の後のスペースを飛ばした位置が入力テキストの開始
    let prompt_str = &line.text[prompt_byte_pos..];
    let after_prompt_marker = &prompt_str['❯'.len_utf8()..];
    let trimmed_len = after_prompt_marker.trim_start().len();
    let input_byte_start = line.text.len() - trimmed_len;
    // trim_end 後のテキスト長が input_text
    let input_byte_end = input_byte_start + input_text.len();

    // 入力テキスト範囲に重なるランの dim 状態を集計
    let mut has_dim = false;
    let mut has_normal = false;
    for run in &line.runs {
        // ランと入力テキスト範囲が重なるかチェック
        if run.range.end <= input_byte_start || run.range.start >= input_byte_end {
            continue;
        }
        // 重なる範囲のテキストが空白だけならスキップ
        let overlap_start = run.range.start.max(input_byte_start);
        let overlap_end = run.range.end.min(input_byte_end);
        if line.text[overlap_start..overlap_end].trim().is_empty() {
            continue;
        }
        if run.dim {
            has_dim = true;
        } else {
            has_normal = true;
        }
    }

    let style = match (has_dim, has_normal) {
        (true, false) => InputStyle::Ghost,
        (false, true) => InputStyle::User,
        (true, true) => InputStyle::Mixed,
        (false, false) => InputStyle::None,
    };

    Some(InputStatus {
        line: full_line,
        text: input_text,
        style,
    })
}

/// エージェント TUI の入力ボックスが占めている**画面行の範囲**（#719）。
///
/// チャットビューの入力欄はこの範囲を実画面からミラーする（下書きを別に持たない）ので、
/// 「入力欄が何行あるか」= 箱の高さ、がここで決まる（#718 のオートグローもこれに従う）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputRegion {
    /// 入力ボックスの先頭行（画面行 index。上の罫線は含まない）
    pub start: usize,
    /// 入力ボックスの終端行の**次**（下の罫線は含まない）
    pub end: usize,
    /// プロンプト記号（`❯` 等）がある行
    pub prompt_row: usize,
}

impl InputRegion {
    /// 行数（必ず 1 以上）
    pub fn rows(&self) -> usize {
        self.end.saturating_sub(self.start).max(1)
    }
}

/// 入力ボックスの中に **TUI 自身が何か描いているか**（Issue #737）。
///
/// 「ユーザーが打った文字があるか」とは別物で、claude 自身の dim な案内文も
/// true になる（空欄時の `Try "how does <filepath> work?"`、キュー滞留時の
/// `Press up to edit queued messages` を実採取で確認）。
///
/// GUI モードのチャット入力欄は TUI の入力行をそのまま映すので、ここが true の
/// ときに tako 自前のプレースホルダを重ねると**同じ座標に 2 つの文字列が出て
/// 読めなくなる**（#737 の実測根因。dim テキストは列 2 から始まり、
/// プレースホルダの絶対配置とほぼ一致していた）
pub fn input_box_has_content(screen: &Screen, region: &InputRegion) -> bool {
    // プロンプト行は記号（`❯`）を除いた中身を見る。記号だけの行は「空」
    if analyze_input_line_at(screen, region.prompt_row).is_some_and(|s| !s.text.trim().is_empty()) {
        return true;
    }
    // 箱が複数行なら 2 行目以降の本文も見る（プロンプト行は上で判定済み）
    let end = region.end.min(screen.lines.len());
    (region.start..end).any(|i| i != region.prompt_row && !screen.lines[i].text.trim().is_empty())
}

/// 入力ボックスの中でのキャレット位置 `(列, 映した行の先頭から数えた行)`（Issue #737）。
///
/// `shown_rows` は実際に映している行数（上限で頭打ちになり、超えたぶんは
/// **頭が落ちる**）。返す行番号は映した行の中での index なので、描画側は
/// 「行の高さ × これ」でそのままキャレットの y が出る。
///
/// GUI モードのチャット表示はターミナルグリッドを描かないため、IME の未確定文字列と
/// 候補ウィンドウをセル座標（`pane_text_areas` 由来）へ向けると画面上のどこも
/// 指さない。**入力欄の実座標へ向けるための唯一の写像**がこれ
pub fn input_caret_cell(
    screen: &Screen,
    region: &InputRegion,
    shown_rows: usize,
) -> Option<(usize, usize)> {
    let (col, row) = screen.ime_anchor_cell()?;
    let shown = region.rows().min(shown_rows.max(1));
    let first = region.end.saturating_sub(shown);
    // 箱の外（会話ログ側・フッター側）にカーソルがあるなら入力欄には出さない
    (first..region.end).position(|r| r == row).map(|w| (col, w))
}

/// 罫線だけでできた行か（`────` / `╭────╮` / `│` 単独は除く）。
///
/// claude は入力欄を上下の水平罫線で挟んで描く（実採取画面 v2.1 系）。
/// バージョンによっては角丸ボックス（`╭─╮` / `╰─╯`）になるのでどちらも受ける
fn is_frame_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let mut horizontal = 0usize;
    for c in t.chars() {
        match c {
            '─' | '━' | '═' | '╌' | '┄' | '┈' | '⎯' => horizontal += 1,
            // 角・接続・縦棒は許すが、水平線の本数には数えない
            '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '│' | '┃' | ' ' =>
                {}
            _ => return false,
        }
    }
    horizontal >= 3
}

/// エージェント TUI のプロンプト記号で始まる行か。
///
/// 枠線つきで描かれるバージョン（`│ ❯ hello │`）でも拾えるよう、行頭の縦罫線は
/// 1 つだけ剥がしてから見る。記号は claude `❯` / codex `›` / agy `>` の和集合（#120）
fn starts_with_prompt(line: &str) -> bool {
    let t = line.trim_start();
    let t = t
        .strip_prefix('│')
        .or_else(|| t.strip_prefix('┃'))
        .unwrap_or(t)
        .trim_start();
    // ASCII の `>` はシェルの PS2 と衝突するので「`>` 単独 or `> `＋内容」だけ
    t.starts_with('❯') || t.starts_with('›') || t.starts_with("> ") || t.trim_end() == ">"
}

/// 画面から入力ボックスの行範囲を求める（#719 のミラー描画の基準）。
///
/// 手順は「プロンプト行を見つける → 上下の一番近い罫線で挟む」。罫線が無い
/// バージョンでもプロンプト行 1 行として成立するので、TUI の描き方が変わっても
/// **入力欄が消えることはない**（最悪 1 行に縮退するだけ）。
/// 番号付き選択ダイアログの選択カーソルはプロンプトではないので除外する（#530 と同じ判断）
pub fn input_region_in_lines(lines: &[&str]) -> Option<InputRegion> {
    // 走査の基準は「行数」ではなく**中身がある最後の行**。ビューポートを埋めない
    // TUI（起動直後・出力が短いとき）では下端に空行が続き、行数基準だと
    // 入力ボックスが走査範囲から丸ごと外れる（セルフテストで実測して直した）
    let bottom = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)?;
    // 下端 24 行の中の**最後の**プロンプト行。会話ログ側の `❯` を拾わないための範囲制限。
    // フッター（区切り線 + モデル / ctx / モード行で最大 8 行）を挟んでも、入力が
    // 十数行に伸びたところまで届く幅にしてある（表示上限 8 行より広い）
    let scan_from = bottom.saturating_sub(24);
    let prompt_row = (scan_from..bottom)
        .rev()
        .find(|&i| starts_with_prompt(lines[i]))?;
    // 上へ: 一番近い罫線の 1 つ下が入力ボックスの先頭
    let start = (scan_from..prompt_row)
        .rev()
        .find(|&i| is_frame_line(lines[i]))
        .map(|i| i + 1)
        .unwrap_or(prompt_row);
    // 下へ: 一番近い罫線の手前が終端。罫線が無ければプロンプト行だけ
    let end = (prompt_row + 1..bottom)
        .find(|&i| is_frame_line(lines[i]))
        .unwrap_or(prompt_row + 1);
    Some(InputRegion {
        start,
        end: end.max(prompt_row + 1),
        prompt_row,
    })
}

/// [`input_region_in_lines`] の `Screen` 版（描画側はこちらを使う）。
///
/// 返す index は `screen.lines` の添字なので、**同じ `Screen` から作った描画行**と
/// 1:1 で対応する（ミラーの行ズレを構造的に防ぐ）
pub fn input_region(screen: &Screen) -> Option<InputRegion> {
    let texts: Vec<&str> = screen.lines.iter().map(|l| l.text.as_str()).collect();
    input_region_in_lines(&texts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::term::{test::TermSize, Config};
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    const COLS: usize = 20;
    const ROWS: usize = 5;

    fn term_with(bytes: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(COLS, ROWS), VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    fn theme() -> Theme {
        Theme::default_dark()
    }

    /// 行内で text 部分文字列に一致するランを探す
    fn run_for<'a>(line: &'a ScreenLine, needle: &str) -> &'a StyleRun {
        let start = line.text.find(needle).expect("テキストが行内にある");
        line.runs
            .iter()
            .find(|r| r.range.start <= start && start < r.range.end)
            .expect("ランが存在する")
    }

    #[test]
    fn ansi16色が解決される() {
        let term = term_with(b"\x1b[31mRED");
        let s = snapshot(&term, &theme());
        let run = run_for(&s.lines[0], "RED");
        assert_eq!(run.fg, theme().ansi[1]);
        assert_eq!(run.bg, None);
    }

    #[test]
    fn 連続する同スタイルセルは1ランへ合成される() {
        let term = term_with(b"\x1b[31mAB\x1b[0mCD");
        let s = snapshot(&term, &theme());
        let line = &s.lines[0];
        // 赤 AB / デフォルト CD（+カーソルセル+残り空白）に分かれる
        let red = run_for(line, "AB");
        assert_eq!(&line.text[red.range.clone()], "AB");
        let plain = run_for(line, "CD");
        assert_eq!(plain.fg, theme().foreground);
        assert!(plain.range.len() >= 2);
    }

    #[test]
    fn 表示256色とtruecolorが解決される() {
        let term = term_with(b"\x1b[38;5;196mX\x1b[38;2;1;2;3mY");
        let s = snapshot(&term, &theme());
        assert_eq!(run_for(&s.lines[0], "X").fg, Rgb::new(255, 0, 0));
        assert_eq!(run_for(&s.lines[0], "Y").fg, Rgb::new(1, 2, 3));
    }

    #[test]
    fn inverseで前景背景が入れ替わる() {
        let term = term_with(b"\x1b[7mX");
        let s = snapshot(&term, &theme());
        let run = run_for(&s.lines[0], "X");
        assert_eq!(run.fg, theme().background);
        assert_eq!(run.bg, Some(theme().foreground));
    }

    #[test]
    fn 装飾フラグがランへ写る() {
        let term = term_with(b"\x1b[1;3;4;9mX");
        let s = snapshot(&term, &theme());
        let run = run_for(&s.lines[0], "X");
        assert!(run.bold && run.italic && run.underline && run.strikeout);
        assert!(!run.dim);
    }

    #[test]
    fn dimフラグがランへ写る() {
        let term = term_with(b"\x1b[2mDIM\x1b[0mNORMAL");
        let s = snapshot(&term, &theme());
        let dim_run = run_for(&s.lines[0], "DIM");
        assert!(dim_run.dim);
        let normal_run = run_for(&s.lines[0], "NORMAL");
        assert!(!normal_run.dim);
    }

    fn make_screen(text_bytes: &[u8]) -> Screen {
        let term = term_with(text_bytes);
        snapshot(&term, &theme())
    }

    #[test]
    fn 入力行分析_ゴーストテキスト検出() {
        // ❯ の後に dim テキスト = ghost
        let s = make_screen("output\r\n❯ \x1b[2mghost suggestion\x1b[0m".as_bytes());
        let status = analyze_input_line(&s).expect("❯ 行がある");
        assert_eq!(status.text, "ghost suggestion");
        assert_eq!(status.style, InputStyle::Ghost);
    }

    #[test]
    fn 入力行分析_ユーザー入力検出() {
        // ❯ の後に通常テキスト = user
        let s = make_screen("output\r\n❯ user typed text".as_bytes());
        let status = analyze_input_line(&s).expect("❯ 行がある");
        assert_eq!(status.text, "user typed text");
        assert_eq!(status.style, InputStyle::User);
    }

    #[test]
    fn 入力行分析_空入力() {
        let s = make_screen("output\r\n❯ ".as_bytes());
        let status = analyze_input_line(&s).expect("❯ 行がある");
        assert_eq!(status.text, "");
        assert_eq!(status.style, InputStyle::None);
    }

    #[test]
    fn 入力行分析_プロンプトなし() {
        let s = make_screen(b"just some output\r\nno prompt here");
        assert!(analyze_input_line(&s).is_none());
    }

    #[test]
    fn 入力行分析_混在() {
        // dim + non-dim の混在
        let s = make_screen("❯ \x1b[2mghost\x1b[0m real".as_bytes());
        let status = analyze_input_line(&s).expect("❯ 行がある");
        assert_eq!(status.style, InputStyle::Mixed);
    }

    #[test]
    fn show_cursor_falseでカーソル強調が消える() {
        // tmux copy-mode スクロール中のカーソル居残り対策（2026-06-12 実機 (b)）。
        // DECTCEM（\e[?25l）による非表示は alacritty 側が処理する
        let term = term_with(b"ab");
        let s = snapshot_opts(&term, &theme(), false, 0.0);
        assert_eq!(s.cursor, None);
        // IME 用カーソルは show_cursor=false でも返る
        assert_eq!(s.ime_cursor, Some((2, 0)));
        let hidden = term_with(b"\x1b[?25lab");
        let sh = snapshot(&hidden, &theme());
        assert_eq!(sh.cursor, None);
        // DECTCEM 非表示でも IME 用カーソルは返る（#29 修正の核心）
        assert_eq!(sh.ime_cursor, Some((2, 0)));
    }

    /// #497: IME のアンカーはカーソル非表示でも解決できなければならない。
    /// ここが None になると未確定文字列の下線オーバーレイが丸ごと消える
    #[test]
    fn ime_anchor_cellはカーソル非表示でもフォールバックする() {
        // 通常（カーソル表示）: 表示中カーソルをそのまま使う
        let term = term_with(b"ab");
        let s = snapshot(&term, &theme());
        assert_eq!(s.cursor, Some((2, 0)));
        assert_eq!(s.ime_anchor_cell(), Some((2, 0)));

        // DECTCEM でカーソルを消したペイン（claude の TUI と同条件）
        let hidden = term_with(b"\x1b[?25lab");
        let sh = snapshot(&hidden, &theme());
        assert_eq!(sh.cursor, None, "前提: 表示中カーソルは無い");
        assert_eq!(
            sh.ime_anchor_cell(),
            Some((2, 0)),
            "カーソル非表示でも ime_cursor へフォールバックすること"
        );

        // show_cursor=false（copy-mode スクロール中）でも同様
        let s2 = snapshot_opts(&term, &theme(), false, 0.0);
        assert_eq!(s2.cursor, None);
        assert_eq!(s2.ime_anchor_cell(), Some((2, 0)));
    }

    #[test]
    fn カーソルセルはカーソル色になる() {
        let term = term_with(b"ab");
        let t = theme();
        let s = snapshot(&term, &t);
        assert_eq!(s.cursor, Some((2, 0)));
        let line = &s.lines[0];
        let run = line
            .runs
            .iter()
            .find(|r| r.range.start == 2)
            .expect("カーソル位置のラン");
        assert_eq!(run.bg, Some(t.cursor));
        assert_eq!(run.fg, t.cursor_text);
    }

    #[test]
    fn スクロールバック表示中はオフセットがつきカーソルが画面外になる() {
        let mut text = Vec::new();
        for i in 0..20 {
            text.extend_from_slice(format!("line{i}\r\n").as_bytes());
        }
        let mut term = term_with(&text);
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(10));
        let s = snapshot(&term, &theme());
        assert_eq!(s.display_offset, 10);
        assert_eq!(s.cursor, None);
        // スクロールバック中は IME 用カーソルもビューポート外
        assert_eq!(s.ime_cursor, None);
        // 10 行ぶん過去が見えている
        assert!(s.lines[0].text.starts_with("line6"));
    }

    #[test]
    fn 選択範囲に選択背景がつく() {
        let mut term = term_with(b"hello");
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Side::Left,
        );
        sel.update(Point::new(Line(0), Column(2)), Side::Right);
        term.selection = Some(sel);
        let t = theme();
        let s = snapshot(&term, &t);
        let run = run_for(&s.lines[0], "hel");
        assert_eq!(run.bg, Some(t.selection_background));
    }

    #[test]
    fn 太幅文字のスペーサーはテキスト化されない() {
        let term = term_with("あい".as_bytes());
        let s = snapshot(&term, &theme());
        // 2 文字 + 残り空白（スペーサー 2 セルはスキップされ、列数 - 2 の空白が残る）
        assert!(s.lines[0].text.starts_with("あい"));
        assert_eq!(s.lines[0].text.chars().count(), 2 + (COLS - 4));
    }

    #[test]
    fn 全行が常にcols幅で埋まる() {
        let term = term_with(b"x");
        let s = snapshot(&term, &theme());
        assert_eq!(s.lines.len(), ROWS);
        for line in &s.lines {
            assert_eq!(line.text.chars().count(), COLS);
        }
    }

    /// 20 行出力して 10 行遡った term（extra_bottom テスト共用）
    fn scrolled_term() -> Term<VoidListener> {
        let mut text = Vec::new();
        for i in 0..20 {
            text.extend_from_slice(format!("line{i}\r\n").as_bytes());
        }
        let mut term = term_with(&text);
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(10));
        term
    }

    #[test]
    fn fract付きではviewport最下行の1行下が追加される() {
        let term = scrolled_term();
        let s = snapshot_opts(&term, &theme(), true, 0.5);
        assert_eq!(s.display_offset, 10);
        assert_eq!(s.fract, 0.5);
        // viewport は line6..line10（ROWS=5）なので追加行は line11
        assert!(s.lines[0].text.starts_with("line6"));
        assert!(s.lines[ROWS - 1].text.starts_with("line10"));
        let extra = s.extra_bottom.expect("追加行が付く");
        assert!(extra.text.starts_with("line11"), "text={:?}", extra.text);
        assert_eq!(extra.text.chars().count(), COLS);
    }

    #[test]
    fn fractゼロでは追加行なし() {
        let term = scrolled_term();
        let s = snapshot_opts(&term, &theme(), true, 0.0);
        assert!(s.extra_bottom.is_none());
        assert_eq!(s.fract, 0.0);
    }

    #[test]
    fn 最下部では追加行なし() {
        // display_offset 0 では fract があっても追加行は構築しない（防御）
        let term = term_with(b"hello");
        let s = snapshot_opts(&term, &theme(), true, 0.5);
        assert!(s.extra_bottom.is_none());
    }

    #[test]
    fn 追加行にも選択ハイライトが写る() {
        let mut term = scrolled_term();
        // 追加行 = grid 座標 Line(rows - display_offset) = Line(-5)（line11 の行）
        let row = Line(ROWS as i32 - 10);
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(row, Column(0)),
            Side::Left,
        );
        sel.update(Point::new(row, Column(3)), Side::Right);
        term.selection = Some(sel);
        let t = theme();
        let s = snapshot_opts(&term, &t, true, 0.5);
        let extra = s.extra_bottom.expect("追加行が付く");
        let run = run_for(&extra, "line");
        assert_eq!(run.bg, Some(t.selection_background));
    }

    // --- 入力ボックスの行範囲（#719 のミラー描画。#718 の高さもここが決める） ---

    /// 実採取した claude v2.1 系の下端（罫線で挟まれた `❯` + フッター）
    fn claude_bottom(input: &[&str]) -> Vec<String> {
        let mut lines: Vec<String> = vec![
            "  ⎿  Tip: Use /btw to ask a quick side question".into(),
            "".into(),
            "────────────────────────────────".into(),
        ];
        lines.extend(input.iter().map(|s| s.to_string()));
        lines.extend(
            [
                "────────────────────────────────",
                "  [Opus 5 · MAX]  user@example.com",
                "  ctx  18% █░░░░░░░░░",
                "  ⏵⏵ auto mode on (shift+tab to cycle)",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        lines
    }

    fn region_of(lines: &[String]) -> Option<InputRegion> {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        input_region_in_lines(&refs)
    }

    #[test]
    fn 入力ボックスは罫線に挟まれた範囲になる() {
        let lines = claude_bottom(&["❯ こんにちは"]);
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(r.rows(), 1, "1 行入力は 1 行ぶんの高さ");
        assert_eq!(lines[r.prompt_row], "❯ こんにちは");
        assert_eq!(r.start, r.prompt_row);
    }

    #[test]
    fn 複数行入力では行数が増える() {
        let lines = claude_bottom(&["❯ 1 行目", "  2 行目", "  3 行目"]);
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(r.rows(), 3, "TUI の行数にそのまま追従する");
        assert_eq!(lines[r.start], "❯ 1 行目");
        assert_eq!(lines[r.end - 1], "  3 行目");
    }

    #[test]
    fn 空の入力欄でも1行として取れる() {
        let lines = claude_bottom(&["❯"]);
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(r.rows(), 1);
    }

    #[test]
    fn 角丸ボックスの描き方でも挟める() {
        // 将来 claude が枠線に変えても縮退しないこと
        let lines: Vec<String> = [
            "text above",
            "╭──────────────────────╮",
            "│ ❯ hello              │",
            "│   world              │",
            "╰──────────────────────╯",
            "  footer",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(r.rows(), 2);
    }

    #[test]
    fn 罫線が無くてもプロンプト行だけに縮退する() {
        let lines: Vec<String> = ["output line", "❯ hello"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(r.rows(), 1);
        assert_eq!(r.start, 1);
    }

    #[test]
    fn プロンプトが無ければ範囲は取れない() {
        let lines: Vec<String> = ["$ ls", "a.txt  b.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            region_of(&lines).is_none(),
            "素のシェルは入力ボックス扱いしない"
        );
    }

    #[test]
    fn 会話ログの古いプロンプト行は拾わない() {
        // 画面上端に残った過去の `❯` ではなく、下端の入力欄を採る
        let mut lines: Vec<String> = vec!["❯ 昔の入力".into()];
        lines.extend((0..26).map(|i| format!("出力 {i}")));
        lines.extend(claude_bottom(&["❯ いまの入力"]));
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(lines[r.prompt_row], "❯ いまの入力");
    }

    #[test]
    fn 画面下端に空行が続いても入力ボックスを見つけられる() {
        // ビューポートを埋めない TUI（起動直後・出力が短いとき）。
        // 行数基準で下端 24 行を切ると入力ボックスごと走査範囲から外れる
        let mut lines = claude_bottom(&["❯ こんにちは"]);
        lines.extend((0..30).map(|_| String::new()));
        let r = region_of(&lines).expect("空行の上にある入力ボックスを見つける");
        assert_eq!(r.rows(), 1);
        assert_eq!(lines[r.prompt_row], "❯ こんにちは");
    }

    /// 行テキストだけから最小の Screen を組む（走査範囲の検査用。runs は空でよい）
    fn screen_of(texts: &[&str]) -> Screen {
        Screen {
            cols: 80,
            rows: texts.len(),
            lines: texts
                .iter()
                .map(|t| ScreenLine {
                    text: (*t).to_string(),
                    runs: Vec::new(),
                    cell_cols: Vec::new(),
                    has_wide: false,
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
    fn 入力行の分析も末尾空行に耐える() {
        // ビューポートを埋めない画面（下端に空行が並ぶ）。行数基準で末尾 10 行を切ると
        // 入力行を見失い、「箱はあるのにテキストは無い」食い違いが起きる（#719 実スクショ）
        let mut texts: Vec<&str> = vec!["out", "────────", "❯ hello", "────────", "footer"];
        texts.extend(std::iter::repeat_n("", 12));
        let s = screen_of(&texts);
        let status = analyze_input_line(&s).expect("末尾に空行があっても入力行を見つける");
        assert_eq!(status.text, "hello");
        // 箱の判定と同じ行を指定した場合も同じ結果になる（食い違いを構造的に防ぐ）
        let region = input_region(&s).expect("入力ボックスがある");
        let at = analyze_input_line_at(&s, region.prompt_row).expect("同じ行から取れる");
        assert_eq!(at.text, status.text);
    }

    #[test]
    fn 実採取した_claude_の複数行入力ボックスに追従する() {
        // claude v2.1.220 を隔離 tmux で動かし Shift+Enter で 4 行入れたときの実採取。
        // 箱の中には「次の行」用の空行が 1 つ入るので 5 行になる（TUI の見た目どおり）
        let lines: Vec<String> = [
            "                                        ctrl+g to edit in VS Code",
            "──────────────────────────────────────────────────────────────",
            "❯ line1",
            "  line2",
            "  line3",
            "  line4",
            "",
            "──────────────────────────────────────────────────────────────",
            "  [Opus 5 (1M context) · xH]  user@example.com",
            "  ctx   0% ░░░░░░░░░░",
            "  5h   --",
            "  7d   --",
            "  ⏵⏵ auto mode on (shift+tab to cycle)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(
            r.rows(),
            5,
            "TUI が描いた行数（末尾の空行を含む）に一致する"
        );
        assert_eq!(lines[r.start], "❯ line1");
        assert_eq!(lines[r.end - 1], "");
    }

    #[test]
    fn 実採取した_claude_の画像プレースホルダ入り入力行を拾う() {
        // ⌘V（= Ctrl+V 素通し）で claude 自身が差し込む形（実採取: `❯ abc[Image #1]`）
        let lines = claude_bottom(&["❯ abc[Image #1]"]);
        let r = region_of(&lines).expect("入力ボックスがある");
        assert_eq!(r.rows(), 1);
        assert!(lines[r.prompt_row].contains("[Image #1]"));
    }

    #[test]
    fn 罫線判定は本文を誤検出しない() {
        assert!(is_frame_line("────────"));
        assert!(is_frame_line("╭──────╮"));
        assert!(!is_frame_line("─"), "1〜2 本の水平線は罫線扱いしない");
        assert!(!is_frame_line("│"), "縦棒だけは罫線ではない");
        assert!(!is_frame_line("ハイフン--- 区切り"));
        assert!(!is_frame_line(""));
    }

    #[test]
    fn screen_からも同じ範囲が取れる() {
        // 描画行と同じ添字で返ること（ミラーの行ズレ防止）
        let term = term_with("out\n────────\n❯ hi\n────────\nfooter".as_bytes());
        let s = snapshot_opts(&term, &theme(), true, 0.0);
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(s.lines[r.prompt_row].text.trim_start().starts_with('❯'));
        assert_eq!(r.rows(), 1);
    }

    // ─────── #737: 入力欄の重なり描画と IME キャレット ───────

    /// 実 claude と同じ広さの端末を作る（20 列だと案内文が折り返してしまう）
    fn wide_term(bytes: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(60, 12), VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    /// 実採取した claude の下端（`737probe` の tmux キャプチャそのまま）を組む。
    /// `input` はプロンプト行の生バイト列（SGR 込み）
    fn claude_screen(input: &str) -> Screen {
        let rule = "─".repeat(40);
        let bytes = format!(
            "out\r\n\x1b[38;5;244m{rule}\r\n{input}\r\n\x1b[38;5;244m{rule}\r\n\
             \x1b[39m  \x1b[32m[Opus 5 (1M context) \u{b7} xH]\x1b[0m\r\n  ctx   0%",
        );
        let term = wide_term(bytes.as_bytes());
        snapshot_opts(&term, &theme(), true, 0.0)
    }

    /// 空欄の claude は **自前の dim な案内文**を箱の中に描く（実採取）。
    /// これを「中身なし」と扱うと、tako のプレースホルダを同じ座標へ重ねてしまう
    /// （#737 の実測根因 O1）
    #[test]
    fn 空欄のclaudeは自前の案内文を箱に描いている() {
        // 実採取: '\x1b[39m❯\xa0\x1b[2mTry "how does <filepath> work?"\x1b[0m'
        let s = claude_screen("\x1b[39m❯\u{a0}\x1b[2mTry \"how does <filepath> work?\"\x1b[0m");
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(
            input_box_has_content(&s, &r),
            "dim の案内文も「箱に何か描いてある」として扱う: {:?}",
            s.lines[r.prompt_row].text
        );
    }

    /// キュー滞留中の案内文（実採取）も同じ扱い。
    /// ここを取りこぼすと busy キューの状態で重なりが起きる（受け入れ条件 1 の 1 状態）
    #[test]
    fn キュー滞留の案内文も箱の中身として扱う() {
        let s = claude_screen(
            "\x1b[38;5;246m❯\u{a0}\x1b[2m\x1b[39mPress up to edit queued messages\x1b[0m",
        );
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(input_box_has_content(&s, &r));
    }

    /// 生成中で本当に空の箱（`❯ ` だけ。実採取）は「中身なし」。
    /// ここまで true にしてしまうと tako の案内文が一切出なくなる
    #[test]
    fn 本当に空の箱は中身なしと判定する() {
        let s = claude_screen("\x1b[38;5;246m❯\u{a0}\x1b[39m");
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(
            !input_box_has_content(&s, &r),
            "記号だけの行は空: {:?}",
            s.lines[r.prompt_row].text
        );
    }

    /// ユーザーが打った文字は当然「中身あり」
    #[test]
    fn 打った文字は箱の中身として扱う() {
        let s = claude_screen("\x1b[38;5;246m❯\u{a0}\x1b[39mbusy中の追加指示です");
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(input_box_has_content(&s, &r));
    }

    /// 複数行入力の 2 行目以降だけに本文があるときも中身ありとする
    #[test]
    fn 複数行入力の2行目の本文も拾う() {
        let rule = "─".repeat(40);
        let bytes =
            format!("out\r\n\x1b[38;5;244m{rule}\r\n❯\u{a0}\r\n  2行目の本文\r\n\x1b[38;5;244m{rule}\r\n  ctx 0%");
        let term = wide_term(bytes.as_bytes());
        let s = snapshot_opts(&term, &theme(), true, 0.0);
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(r.rows() >= 2, "2 行の箱として取れている: {r:?}");
        assert!(input_box_has_content(&s, &r));
    }

    /// キャレットは「箱の中の (列, 行)」へ写る。
    /// 実採取の値（空欄 = 列 2 / 14 文字打つと列 16）と同じ関係になることを見る
    #[test]
    fn キャレットは箱の中の座標へ写る() {
        // プロンプト行は上から 2 行目（0 始まりで index 2）。CSI H は 1 始まり
        let rule = "─".repeat(40);
        let bytes = format!(
            "out\r\n\x1b[38;5;244m{rule}\r\n❯\u{a0}G4 mo susumete\r\n\x1b[38;5;244m{rule}\r\n  ctx 0%\x1b[3;17H",
        );
        let term = wide_term(bytes.as_bytes());
        let s = snapshot_opts(&term, &theme(), true, 0.0);
        let r = input_region(&s).expect("入力ボックスがある");
        let (col, row) = input_caret_cell(&s, &r, CHAT_INPUT_MAX_ROWS_FOR_TEST)
            .expect("箱の中にキャレットがある");
        assert_eq!(col, 16, "実採取と同じ列（`❯ ` の 2 列 + 14 文字）");
        assert_eq!(row, 0, "1 行の箱なので先頭行");
    }

    /// カーソルが箱の外（会話ログ側）にあるときは入力欄へ出さない。
    /// ここで Some を返すと、スクロールバック中に未確定文字列が入力欄へ化けて出る
    #[test]
    fn 箱の外のカーソルは入力欄のキャレットにしない() {
        let rule = "─".repeat(40);
        // カーソルを 1 行目（会話ログ側）へ置く
        let bytes = format!(
            "out\r\n\x1b[38;5;244m{rule}\r\n❯\u{a0}hi\r\n\x1b[38;5;244m{rule}\r\n  ctx 0%\x1b[1;1H",
        );
        let term = wide_term(bytes.as_bytes());
        let s = snapshot_opts(&term, &theme(), true, 0.0);
        let r = input_region(&s).expect("入力ボックスがある");
        assert_eq!(input_caret_cell(&s, &r, CHAT_INPUT_MAX_ROWS_FOR_TEST), None);
    }

    /// 上限行数を超えて**頭が落ちている**ときは、落ちたぶんだけ行番号が詰まる。
    /// ここを region.start 基準で数えると、映していない行のぶん下へずれる
    #[test]
    fn 頭が落ちた箱でも行番号は映した行を基準にする() {
        let rule = "─".repeat(40);
        let mut body = String::new();
        for i in 0..4 {
            body.push_str(&format!("❯\u{a0}row{i}\r\n"));
        }
        // 4 行の箱。カーソルは最終行（画面 index 5 = CSI H の 6 行目）
        let bytes = format!(
            "out\r\n\x1b[38;5;244m{rule}\r\n{body}\x1b[38;5;244m{rule}\r\n  ctx 0%\x1b[6;3H"
        );
        let term = wide_term(bytes.as_bytes());
        let s = snapshot_opts(&term, &theme(), true, 0.0);
        let r = input_region(&s).expect("入力ボックスがある");
        assert_eq!(r.rows(), 4, "4 行の箱: {r:?}");
        // 全部映すなら最終行 = index 3
        assert_eq!(input_caret_cell(&s, &r, 4).map(|(_, row)| row), Some(3));
        // 2 行しか映さない（頭 2 行が落ちる）なら最終行 = index 1
        assert_eq!(input_caret_cell(&s, &r, 2).map(|(_, row)| row), Some(1));
    }

    /// 描画側の上限（`chat_view::CHAT_INPUT_MAX_ROWS`）と同値。
    /// core は GPUI に依存しないのでテスト用に持つ
    const CHAT_INPUT_MAX_ROWS_FOR_TEST: usize = 8;
}
