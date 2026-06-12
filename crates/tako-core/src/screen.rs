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
    /// スクロールバック表示中のオフセット（0 = 最下部）
    pub display_offset: usize,
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
}

/// Term の表示内容を色解決済みスナップショットへ変換する
pub fn snapshot<T: EventListener>(term: &Term<T>, theme: &Theme) -> Screen {
    snapshot_opts(term, theme, true)
}

/// `show_cursor = false` でカーソルセルの強調を抑止する版。
/// tmux copy-mode でスクロール中のバックエンドペインは、tmux が報告する
/// copy-mode カーソルが画面に固定表示されて不自然なため UI 層が隠す
/// （2026-06-12 実機フィードバック (b)）
pub fn snapshot_opts<T: EventListener>(term: &Term<T>, theme: &Theme, show_cursor: bool) -> Screen {
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
    };
    // '\0' は太幅文字のスペーサーセル（テキスト化時にスキップ）
    let mut grid: Vec<Vec<(char, CellStyle)>> =
        vec![vec![(' ', default_style.clone()); cols]; rows];

    let cursor = (show_cursor && content.cursor.shape != CursorShape::Hidden)
        .then(|| point_to_viewport(display_offset, content.cursor.point))
        .flatten()
        .map(|p| (p.column.0, p.line))
        .filter(|&(col, row)| col < cols && row < rows);

    for indexed in content.display_iter {
        let Some(vp) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let (row, col) = (vp.line, vp.column.0);
        if row >= rows || col >= cols {
            continue;
        }
        let cell = indexed.cell;
        let flags = cell.flags;

        let c = if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            '\0'
        } else {
            cell.c
        };

        let mut fg = resolve_color(&cell.fg, content.colors, theme);
        let mut bg = resolve_color(&cell.bg, content.colors, theme);
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

        let selected = content
            .selection
            .is_some_and(|range| range.contains(indexed.point));
        if selected {
            bg = Some(theme.selection_background);
        }
        if cursor == Some((col, row)) {
            fg = theme.cursor_text;
            bg = Some(theme.cursor);
        }

        grid[row][col] = (
            c,
            CellStyle {
                fg,
                bg,
                bold: flags.intersects(Flags::BOLD),
                italic: flags.intersects(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikeout: flags.intersects(Flags::STRIKEOUT),
            },
        );
    }

    let lines = grid
        .into_iter()
        .map(|cells| {
            let mut text = String::with_capacity(cols);
            let mut runs: Vec<StyleRun> = Vec::new();
            let mut cell_cols = Vec::with_capacity(cols);
            for (col, (c, style)) in cells.into_iter().enumerate() {
                if c == '\0' {
                    continue; // 太幅文字のスペーサー: 直前の文字が 2 セル分を占める
                }
                cell_cols.push(col);
                let start = text.len();
                text.push(c);
                let end = text.len();
                match runs.last_mut() {
                    Some(last)
                        if last.fg == style.fg
                            && last.bg == style.bg
                            && last.bold == style.bold
                            && last.italic == style.italic
                            && last.underline == style.underline
                            && last.strikeout == style.strikeout =>
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
                    }),
                }
            }
            ScreenLine {
                text,
                runs,
                cell_cols,
            }
        })
        .collect();

    Screen {
        cols,
        rows,
        lines,
        cursor,
        display_offset,
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
    }

    #[test]
    fn show_cursor_falseでカーソル強調が消える() {
        // tmux copy-mode スクロール中のカーソル居残り対策（2026-06-12 実機 (b)）。
        // DECTCEM（\e[?25l）による非表示は alacritty 側が処理する
        let term = term_with(b"ab");
        let s = snapshot_opts(&term, &theme(), false);
        assert_eq!(s.cursor, None);
        let hidden = term_with(b"\x1b[?25lab");
        assert_eq!(snapshot(&hidden, &theme()).cursor, None);
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
}
