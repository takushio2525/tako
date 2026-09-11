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

/// 空白セルの近道（#801）を切って同じバイナリで A/B を取る逃げ道
/// （`TAKO_801_NO_FAST_CELLS=1`）。UI 側の `terminal_grid` の近道と同じ変数で一緒に切れる
fn fast_cells_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("TAKO_801_NO_FAST_CELLS").is_some())
}

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

/// セル単位の解決済みスタイル（ラン合成前の中間表現）。
/// `Copy` なのは cols*rows のグリッドを毎フレーム敷き直すため（#801）
#[derive(Debug, Clone, Copy, PartialEq)]
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
    let mut grid: Vec<(char, CellStyle)> = vec![(' ', default_style); cols * rows];
    // #1387: 0 幅の結合文字を持つセルは稀なので、**在るセルだけ**を横に持つ
    // （フラット配列を太らせない = 空なら 1 バイトも確保しない）。値は `Term` から
    // 借りたスライスなので文字の複製もしない
    let mut combining: Vec<(usize, &[char])> = Vec::new();
    // #801: 1 セルも書かれなかった行は、どの行でも合成結果が同じになる。
    // 1 本だけ組んで複製すれば、行ごとの `compose_line`（119 セルの走査 +
    // スタイル比較 + String / Vec の積み上げ）が丸ごと省ける
    let mut row_touched = vec![false; rows];

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
    // #801: 「素の空白セル」は `grid` の初期値とまったく同じ結果になるので、
    // 色解決も書き込みも要らない。空画面ではほぼ全セルがこれに当たり、
    // 毎フレーム cols*rows 回走っていた `resolve_cell` が丸ごと消える。
    // 既定色が OSC 4 でテーマ色から差し替えられているときは近道を使わない
    // （前景はスペースには見えないが、`compose_line` のラン分割に効くので同じ扱い）
    let plain_blank_matches_default = !fast_cells_disabled()
        && colors[NamedColor::Background as usize]
            .map(from_ansi)
            .unwrap_or(theme.background)
            == theme.background
        && colors[NamedColor::Foreground as usize]
            .map(from_ansi)
            .unwrap_or(theme.foreground)
            == theme.foreground;
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
        if plain_blank_matches_default && !selected && !is_cursor && is_plain_blank(indexed.cell) {
            continue;
        }
        let idx = row * cols + col;
        let (c, zero_width, style) = resolve_cell(indexed.cell, selected, is_cursor, colors, theme);
        grid[idx] = (c, style);
        if !zero_width.is_empty() {
            combining.push((idx, zero_width));
        }
        row_touched[row] = true;
    }

    // display_iter は行優先で走るので `combining` は既に昇順だが、順序に依存しない形にする
    combining.sort_unstable_by_key(|&(idx, _)| idx);
    // 素のままの行は 1 本組んで使い回す（#801。中身は行位置に依らず同じ）。
    // 結合文字を持つセルは `is_plain_blank` が近道から外すので、
    // 使い回す行には結合文字が無い（#1387）
    let mut blank_line: Option<ScreenLine> = None;
    let lines = (0..rows)
        .map(|row| {
            let base = row * cols;
            if row_touched[row] {
                let zw = row_combining(&combining, base, cols);
                return compose_line(&grid[base..base + cols], zw, base);
            }
            blank_line
                .get_or_insert_with(|| compose_line(&grid[base..base + cols], &[], base))
                .clone()
        })
        .collect();

    // fract > 0 のとき viewport 最下行の 1 行下を追加で切り出す（部分行の描画用）。
    // display_offset d の viewport は grid の Line(-d..rows-d) なので、その下は Line(rows-d)。
    // d == 0（最下部）では存在しない
    let extra_bottom = (fract > 0.0 && display_offset >= 1).then(|| {
        use alacritty_terminal::index::{Column, Line, Point};
        let line = Line(rows as i32 - display_offset as i32);
        let grid_ref = term.grid();
        let mut cells: Vec<(char, CellStyle)> = Vec::with_capacity(cols);
        let mut extra_combining: Vec<(usize, &[char])> = Vec::new();
        for col in 0..cols {
            let point = Point::new(line, Column(col));
            let cell = &grid_ref[line][Column(col)];
            let selected = selection.is_some_and(|range| range.contains(point));
            // カーソルが追加行（viewport の 1 行下）にある場合も焼き込む
            let is_cursor = cursor_visible_at == Some(point);
            let (c, zero_width, style) = resolve_cell(cell, selected, is_cursor, colors, theme);
            cells.push((c, style));
            if !zero_width.is_empty() {
                extra_combining.push((col, zero_width));
            }
        }
        compose_line(&cells, &extra_combining, 0)
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

/// セル 1 つが**画面テキストへ寄与する文字**（#1387 の 1 実装）。
///
/// alacritty は 0 幅の結合文字（NFD の濁点 `U+3099`・アクセント `U+0301` 等）を
/// `Cell::c` ではなく `Cell::extra.zerowidth` に持つ。`c` だけを読むと結合文字が
/// 画面テキストから**黙って落ちる**ので、テキストを組む経路は必ずここを通す。
///
/// 落ちていたのは 3 経路（`resolve_cell` = 描画 / GUI モード / links の材料・
/// `TerminalSession::compose_grid_row` = `tail_lines` / `visible_lines_filled`・
/// `TerminalSession::history_plain_lines` = ペインログ）で、**選択コピーだけ**
/// alacritty 自身の `selection_to_string` を通るため、同じセルの内容が
/// 「見た目 / `read_pane` / ペインログ」と「Cmd+C」で食い違っていた（#1387）。
/// 実害は NFD のファイル名が画面に出たときの Cmd+クリック不動作
/// （`links` の実在チェックが落ちる = #153 / #1283 の経路が死ぬ）。
pub(crate) struct CellText<'a> {
    /// 本体の文字。`None` = テキストへ出さないセル（全角の後続スペーサー・空セル）
    pub(crate) base: Option<char>,
    /// 0 幅の結合文字（本体の直後に**この順で**続く）。`base` が `None` なら常に空
    pub(crate) combining: &'a [char],
}

/// セルの [`CellText`] を取り出す（**`zerowidth` を読む唯一の場所**）。
///
/// 積む順序は alacritty の `selection_to_string`（`line_to_string` の
/// 「本体 → zerowidth」）と同じにしてあるので、Cmd+C と 1 バイトも変わらない。
/// 番犬 `crates/tako-control/tests/issue1387_combining_watchdog.rs` が
/// 他所での直読みと、3 経路がここを通っていないことを名指しで落とす
pub(crate) fn cell_text(cell: &alacritty_terminal::term::cell::Cell) -> CellText<'_> {
    // 全角の後続セル（スペーサー）と空セルはテキストへ出さない。alacritty は
    // 結合文字を**全角の先頭セル**へ寄せる（`Term::input` の `width == 0` 経路が
    // `WIDE_CHAR_SPACER` を 1 つ左へ辿る）ので、スペーサー側を読む必要は無い
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        || cell.c == '\0'
    {
        return CellText {
            base: None,
            combining: &[],
        };
    }
    CellText {
        base: Some(cell.c),
        combining: cell.zerowidth().unwrap_or(&[]),
    }
}

/// 画面テキストへセル 1 つぶんを積む（積んだ char 数を返す。0 = 出さないセル）。
///
/// `Cell` から直接テキストを組む経路（`terminal.rs` の `compose_grid_row` /
/// `history_plain_lines`）の入口。`Screen` 側は `resolve_cell` が色解決と一緒に
/// [`cell_text`] を通すので、3 経路の答えは定義上同じになる（#1387）
pub(crate) fn push_cell_text(
    cell: &alacritty_terminal::term::cell::Cell,
    out: &mut String,
) -> usize {
    let text = cell_text(cell);
    let Some(base) = text.base else {
        return 0;
    };
    out.push(base);
    out.extend(text.combining.iter().copied());
    1 + text.combining.len()
}

/// 行末の「詰め物」に見えるセルか（テキストを組む手前で右端の境界を探すのに使う）。
///
/// 空セル・全角スペーサー・素の半角スペースが該当する。**結合文字を持つセルは
/// 該当しない**（行頭に単独の結合文字が来ると本体が `' '` のセルへ載るので、
/// 空白扱いで切ると落ちる = #1387）
pub(crate) fn cell_is_trailing_blank(cell: &alacritty_terminal::term::cell::Cell) -> bool {
    let text = cell_text(cell);
    text.combining.is_empty() && matches!(text.base, None | Some(' '))
}

/// 「素の空白セル」か（#801）。
///
/// 文字が半角スペースで、属性フラグが 1 つも立っておらず、前景・背景が既定色のセル。
/// このセルの解決結果は `snapshot_opts` が `grid` に敷く初期値（`' '` + 既定スタイル）と
/// 一致するので、解決も書き込みも省ける。**判定を緩めてはいけない**:
/// フラグ（DIM / INVERSE / HIDDEN / 下線 / 全角スペーサー）や明示色が 1 つでもあれば
/// 見た目が変わる
fn is_plain_blank(cell: &alacritty_terminal::term::cell::Cell) -> bool {
    cell.c == ' '
        && cell.flags.is_empty()
        // 0 幅の結合文字を載せた空白セル（行頭の単独アクセント等）は素ではない。
        // 近道で飛ばすと結合文字が落ちる（#1387）
        && cell_text(cell).combining.is_empty()
        && matches!(cell.fg, Color::Named(NamedColor::Foreground))
        && matches!(cell.bg, Color::Named(NamedColor::Background))
}

/// セル 1 つを色解決済みの (文字, 0 幅の結合文字, スタイル) へ変換する。
/// display_iter のセルと grid 直接アクセスのセル（追加行）で共用する。
///
/// 文字の取り出しは [`cell_text`] の 1 実装を通す（#1387）。テキストへ出さない
/// セルは `'\0'` として持ち回り、`compose_line` が落とす
fn resolve_cell<'a>(
    cell: &'a alacritty_terminal::term::cell::Cell,
    selected: bool,
    is_cursor: bool,
    colors: &Colors,
    theme: &Theme,
) -> (char, &'a [char], CellStyle) {
    let flags = cell.flags;
    let text = cell_text(cell);
    let c = text.base.unwrap_or('\0');

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
        text.combining,
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

/// `combining`（`(グリッド添字, 0 幅の結合文字)` の昇順リスト）から
/// 1 行ぶん（`base..base + cols`）を切り出す。**結合文字が無い画面では
/// 探索そのものを行わない**（#1387 / #801 の hot path を太らせない）
fn row_combining<'a, 'c>(
    combining: &'c [(usize, &'a [char])],
    base: usize,
    cols: usize,
) -> &'c [(usize, &'a [char])] {
    if combining.is_empty() {
        return &[];
    }
    let lo = combining.partition_point(|&(idx, _)| idx < base);
    let hi = combining.partition_point(|&(idx, _)| idx < base + cols);
    &combining[lo..hi]
}

/// 1 行分のセル列を ScreenLine（text + StyleRun + cell_cols）へ合成する。
///
/// `combining` は `(グリッド添字, 0 幅の結合文字)` の昇順リストで、`base` は
/// この行の先頭セルの添字。結合文字は `text` へ本体の直後に積み、`cell_cols` へは
/// **同じ列**を積む（そうしないと描画のセル写像と links のスパンが 1 文字ずつずれる = #1387）
fn compose_line(
    cells: &[(char, CellStyle)],
    combining: &[(usize, &[char])],
    base: usize,
) -> ScreenLine {
    let cols = cells.len();
    let mut text = String::with_capacity(cols);
    let mut runs: Vec<StyleRun> = Vec::new();
    let mut cell_cols = Vec::with_capacity(cols);
    let mut next = 0usize;
    for (col, (c, style)) in cells.iter().enumerate() {
        while next < combining.len() && combining[next].0 < base + col {
            next += 1;
        }
        let zero_width = match combining.get(next) {
            Some(&(idx, chars)) if idx == base + col => chars,
            _ => &[][..],
        };
        if *c == '\0' {
            continue;
        }
        cell_cols.push(col);
        let start = text.len();
        text.push(*c);
        for z in zero_width {
            cell_cols.push(col);
            text.push(*z);
        }
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

impl InputStyle {
    /// 入力欄に**人が打った下書き**があるか（#1297）。
    ///
    /// 「空でない」と「下書きがある」は別物で、claude は AI のゴースト提案を
    /// dim で入力欄へ描く（文面は任意なので文字列リストでは網羅できない）。
    /// 下書きと言えるのは通常輝度の文字が 1 つでもあるとき = `User` / `Mixed` だけ。
    ///
    /// **この述語をここ 1 箇所に置く**のは、同じ `matches!(style, User | Mixed)` が
    /// 判定ごとに散ると「片方だけ Ghost を下書きに数える」形で安全側が崩れるため
    /// （#1297 はまさに、属性を見る判定と見ない判定が並存していたことで起きた）
    pub fn is_user_draft(self) -> bool {
        matches!(self, Self::User | Self::Mixed)
    }
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
    // 枠線つきで描くバージョン（`│ ❯ hello │`）のプロンプト行は
    // `analyze_input_line_at` が受けない（行頭が `│` なので `starts_with('❯')` が偽）。
    // 罫線を剥がしてから記号の右を見る。**ここを見ないと本文があるのに「空」**と読む
    // （1 行の箱には継続行も無いので、下の走査でも拾えない。#1390）
    if screen
        .lines
        .get(region.prompt_row)
        .and_then(|l| content_after_prompt(&l.text))
        .is_some_and(|rest| !rest.trim().is_empty())
    {
        return true;
    }
    // 箱が複数行なら 2 行目以降の本文も見る（プロンプト行は上で判定済み）。
    // **行頭・行末の縦罫線は剥がす**（`starts_with_prompt` と同じ作法）: 剥がさないと
    // 空の継続行 `│          │` が trim 後も空にならず、枠線つきの箱が常に
    // 「中身あり」になって tako 自前のプレースホルダが出なくなる（#1390）
    let end = region.end.min(screen.lines.len());
    (region.start..end).any(|i| {
        i != region.prompt_row && !strip_box_border(&screen.lines[i].text).trim().is_empty()
    })
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

/// 箱の行から**行頭・行末の縦罫線を 1 つずつ**剥がす（`│ ❯ hello │` → `❯ hello`）。
///
/// 枠線つきで描くバージョンでは、プロンプト行だけでなく**中身の有無を見る行**にも
/// 罫線が載る。剥がす作法をここ 1 箇所に置くのは、片方だけ剥がすと
/// 「プロンプト行は見つかるのに中身の判定が罫線に引っかかる」形で答えが割れるため
/// （#1390 の実害: 空の継続行 `│          │` が trim 後も空にならず、
/// [`input_box_has_content`] が常に真 = tako 自前のプレースホルダが出なくなる）
fn strip_box_border(line: &str) -> &str {
    let t = line.trim();
    let t = t
        .strip_prefix('│')
        .or_else(|| t.strip_prefix('┃'))
        .unwrap_or(t);
    t.strip_suffix('│')
        .or_else(|| t.strip_suffix('┃'))
        .unwrap_or(t)
        .trim()
}

/// プロンプト記号の**右側**（罫線と記号を剥がした残り）。記号で始まらない行は None。
///
/// 記号は claude `❯` / codex `›` / agy `>` の和集合（#120）。ASCII の `>` は
/// シェルの PS2 と衝突するので「`>` 単独 or `> `＋内容」だけを受ける
fn content_after_prompt(line: &str) -> Option<&str> {
    let t = strip_box_border(line);
    for marker in ['❯', '›'] {
        if let Some(rest) = t.strip_prefix(marker) {
            return Some(rest);
        }
    }
    if let Some(rest) = t.strip_prefix("> ") {
        return Some(rest);
    }
    (t == ">").then_some("")
}

/// エージェント TUI のプロンプト記号で始まる行か。
///
/// 枠線つきで描かれるバージョン（`│ ❯ hello │`）でも拾えるよう、縦罫線は
/// [`strip_box_border`] で剥がしてから見る
fn starts_with_prompt(line: &str) -> bool {
    content_after_prompt(line).is_some()
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

    /// #1297: 「人が打った下書きがあるか」の述語。**通常輝度が 1 文字でもあるか**が境界で、
    /// dim だけ（= AI のゴースト提案）と空欄はどちらも下書きではない
    #[test]
    fn 下書き述語はghostと空を下書きに数えない() {
        assert!(InputStyle::User.is_user_draft());
        assert!(InputStyle::Mixed.is_user_draft());
        assert!(!InputStyle::Ghost.is_user_draft());
        assert!(!InputStyle::None.is_user_draft());
        // 実画面から採った分類とも一致する（述語と分析器が食い違わない）
        for (bytes, want) in [
            ("❯ \x1b[2mmerge the PR once CI is green\x1b[0m", false),
            ("❯ draft by human", true),
            ("❯ \x1b[2mghost\x1b[0m real", true),
            ("❯ ", false),
        ] {
            let s = make_screen(bytes.as_bytes());
            let status = analyze_input_line(&s).expect("❯ 行がある");
            assert_eq!(status.style.is_user_draft(), want, "{bytes:?}");
        }
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

    // ---- #801: 空白セルの近道が「見た目が変わるセル」を飛ばさないこと ----
    //
    // `is_plain_blank` を緩めると、空白でも装飾が乗るセルが素の空白に化ける。
    // 以下は近道を壊したときに落ちる（= 検出力を持つ）不変条件

    #[test]
    fn 装飾つきの空白セルは近道で飛ばさない() {
        // 下線・取り消し線・反転・DIM は「空白でも見える」属性
        let t = theme();
        let s = snapshot(&term_with(b"\x1b[4m   \x1b[0m"), &t);
        assert!(
            s.lines[0].runs.iter().any(|r| r.underline),
            "SGR 4 の空白に下線ランが残る"
        );
        let s = snapshot(&term_with(b"\x1b[9m   \x1b[0m"), &t);
        assert!(s.lines[0].runs.iter().any(|r| r.strikeout));
        let s = snapshot(&term_with(b"\x1b[7m   \x1b[0m"), &t);
        assert!(
            s.lines[0].runs.iter().any(|r| r.bg == Some(t.foreground)),
            "反転した空白は前景色で塗られる"
        );
    }

    #[test]
    fn 明示背景色の空白セルは近道で飛ばさない() {
        let t = theme();
        let s = snapshot(&term_with(b"\x1b[48;2;10;20;30m   \x1b[0m"), &t);
        assert!(s.lines[0]
            .runs
            .iter()
            .any(|r| r.bg == Some(Rgb::new(10, 20, 30))));
    }

    #[test]
    fn 既定背景がosc11で変わったら空白も塗る() {
        // 近道は「既定背景 = テーマ背景」が前提。OSC 11 で変わったら全セル塗る必要がある
        let t = theme();
        let s = snapshot(&term_with(b"\x1b]11;#00ff00\x1b\\"), &t);
        // 先頭セルはカーソルなので除く。残りの素の空白すべてに新しい既定背景が乗る
        assert!(
            s.lines[0]
                .runs
                .iter()
                .filter(|r| r.range.start > 0)
                .all(|r| r.bg == Some(Rgb::new(0, 255, 0))),
            "既定背景が差し替わったら空白セルにも背景が乗る: {:?}",
            s.lines[0].runs
        );
        // 2 行目以降（カーソルが居ない行）も同じ
        assert!(s.lines[1]
            .runs
            .iter()
            .all(|r| r.bg == Some(Rgb::new(0, 255, 0))));
    }

    #[test]
    fn 空白セルの選択とカーソルは近道で飛ばさない() {
        let t = theme();
        // 何も入力していない行を選択する（全セルが素の空白）
        let mut term = term_with(b"");
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Side::Left,
        );
        sel.update(Point::new(Line(0), Column(2)), Side::Right);
        term.selection = Some(sel);
        let s = snapshot(&term, &t);
        // 先頭セルはカーソルが勝つので、選択背景はその隣に出る
        assert!(
            s.lines[0]
                .runs
                .iter()
                .any(|r| r.bg == Some(t.selection_background)),
            "空白セルの選択も塗られる: {:?}",
            s.lines[0].runs
        );
        // カーソルセル（行頭）はカーソル色で焼かれている
        let s = snapshot(&term_with(b""), &t);
        assert_eq!(s.cursor, Some((0, 0)));
        assert_eq!(s.lines[0].runs.first().map(|r| r.bg), Some(Some(t.cursor)));
    }

    #[test]
    fn 全角の右隣スペーサーは近道で飛ばさない() {
        // スペーサーを素の空白として書き戻すと、列数が 1 ずれて cell_cols が壊れる
        let s = snapshot(&term_with("あ".as_bytes()), &theme());
        assert_eq!(s.lines[0].text.chars().next(), Some('あ'));
        // 「あ」は 2 セル占有 = 次の文字の列が 2
        assert_eq!(s.lines[0].cell_cols.first().copied(), Some(0));
        assert_eq!(s.lines[0].cell_cols.get(1).copied(), Some(2));
        assert!(s.lines[0].has_wide);
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

    /// 枠線つきで描く TUI の下端を組む（`lines` をそのまま流す）。
    /// 実 claude v2.1 系は水平罫線だけだが、枠線つきのバージョンでも
    /// 判定が成り立つことを見るための fixture（#1390）
    fn framed_screen(lines: &[&str]) -> Screen {
        let bytes = lines.join("\r\n");
        let term = wide_term(bytes.as_bytes());
        snapshot_opts(&term, &theme(), true, 0.0)
    }

    /// 縦罫線つきの箱で**本文が無い**とき（`│ ❯      │` / `│        │`）は「中身なし」。
    ///
    /// 罫線を剥がさずに `trim` すると空の継続行が空にならないので、枠線つきの箱は
    /// **常に「中身あり」**になり、tako 自前のプレースホルダが一切出なくなる
    /// （#1390。#737 の逆側の壊れ方）
    #[test]
    fn 縦罫線つきの空の箱は中身なしと判定する() {
        let s = framed_screen(&[
            "text above",
            "╭──────────────────────╮",
            "│ ❯                    │",
            "│                      │",
            "╰──────────────────────╯",
            "  footer",
        ]);
        let r = input_region(&s).expect("入力ボックスがある");
        assert_eq!(r.rows(), 2, "2 行の箱として取れている: {r:?}");
        assert!(
            !input_box_has_content(&s, &r),
            "空の継続行を罫線のせいで「中身あり」と読んだ: {:?}",
            (region_lines(&s, &r), s.lines[r.prompt_row].text.clone())
        );
    }

    /// 縦罫線つきでも**プロンプト行の本文**は拾う。
    ///
    /// 「2 行目以降だけ罫線を剥がす」直し方だと、`analyze_input_line_at` が
    /// 行頭 `│` の行を受けないせいでここが false になる（本文があるのに
    /// プレースホルダを重ねる = #737 そのもの）
    #[test]
    fn 縦罫線つきでもプロンプト行の本文を拾う() {
        let s = framed_screen(&[
            "text above",
            "╭──────────────────────╮",
            "│ ❯ hello              │",
            "│                      │",
            "╰──────────────────────╯",
            "  footer",
        ]);
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(
            input_box_has_content(&s, &r),
            "プロンプト行の本文を取りこぼした: {:?}",
            region_lines(&s, &r)
        );
    }

    /// 縦罫線つきの 2 行目以降の本文も拾う（罫線の剥がしで本文まで消していないこと）
    #[test]
    fn 縦罫線つきでも2行目の本文を拾う() {
        let s = framed_screen(&[
            "text above",
            "╭──────────────────────╮",
            "│ ❯                    │",
            "│ world                │",
            "╰──────────────────────╯",
            "  footer",
        ]);
        let r = input_region(&s).expect("入力ボックスがある");
        assert!(
            input_box_has_content(&s, &r),
            "2 行目の本文を取りこぼした: {:?}",
            region_lines(&s, &r)
        );
    }

    /// 1 行の枠線つき箱（継続行が無い形）でも本文の有無が正しく出る。
    /// 継続行の走査に頼った判定だと、本文があっても「中身なし」になる
    #[test]
    fn 縦罫線つきの1行の箱でも中身の有無が出る() {
        let full = framed_screen(&[
            "╭──────────────────────╮",
            "│ ❯ hello              │",
            "╰──────────────────────╯",
            "  footer",
        ]);
        let r = input_region(&full).expect("入力ボックスがある");
        assert_eq!(r.rows(), 1, "1 行の箱: {r:?}");
        assert!(
            input_box_has_content(&full, &r),
            "1 行の箱の本文を取りこぼした: {:?}",
            region_lines(&full, &r)
        );
        let empty = framed_screen(&[
            "╭──────────────────────╮",
            "│ ❯                    │",
            "╰──────────────────────╯",
            "  footer",
        ]);
        let r = input_region(&empty).expect("入力ボックスがある");
        assert!(
            !input_box_has_content(&empty, &r),
            "1 行の空の箱を「中身あり」と読んだ: {:?}",
            region_lines(&empty, &r)
        );
    }

    /// 罫線を剥がす 1 実装（`strip_box_border`）の単体。
    /// 剥がすのは**行頭・行末の 1 つずつ**で、中身の縦棒は残す
    #[test]
    fn 罫線の剥がしは行頭行末の1つずつ() {
        assert_eq!(strip_box_border("│ ❯ hello              │"), "❯ hello");
        assert_eq!(strip_box_border("┃ ❯ hello              ┃"), "❯ hello");
        assert_eq!(strip_box_border("│                      │"), "");
        assert_eq!(strip_box_border("  ❯ hello"), "❯ hello");
        assert_eq!(strip_box_border("│"), "");
        // 中身として並ぶ縦棒は残す（表の行が箱の中に来ても中身と読む）
        assert_eq!(strip_box_border("│ a │ b │"), "a │ b");
        assert_eq!(strip_box_border(""), "");
    }

    /// 入力ボックスの行を診断へ出す（どの行を見て判定したかが失敗出力に残る）
    fn region_lines(screen: &Screen, region: &InputRegion) -> Vec<String> {
        (region.start..region.end.min(screen.lines.len()))
            .map(|i| screen.lines[i].text.clone())
            .collect()
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

    /// 桁数・行数を指定して `Term` を組む（#1387 の「ちょうど 1 文字ぶんの行」用）
    fn term_sized(cols: usize, rows: usize, bytes: &[u8]) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener);
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    /// ランがテキスト全体を隙間なく覆っていること（結合文字がランの外へ落ちない）
    fn assert_runs_cover(line: &ScreenLine) {
        let mut end = 0usize;
        for run in &line.runs {
            assert_eq!(run.range.start, end, "ランに隙間がある: {:?}", line.runs);
            end = run.range.end;
        }
        assert_eq!(end, line.text.len(), "ランがテキストの末尾まで届かない");
    }

    /// #1387: NFD の濁点（`か` + `U+3099`）が `text` に残り、
    /// `cell_cols` には**同じ列**が積まれる（全角 2 桁ちょうどの行で厳密に見る）
    #[test]
    fn nfdの結合文字はtextに残りcell_colsへ同じ列を積む() {
        let term = term_sized(2, 1, "\u{304B}\u{3099}".as_bytes());
        let s = snapshot(&term, &theme());
        let line = &s.lines[0];
        assert_eq!(line.text, "か\u{3099}", "濁点が落ちている: {:?}", line.text);
        assert_eq!(line.cell_cols, vec![0, 0], "結合文字は本体と同じ列");
        assert_runs_cover(line);
    }

    /// #1387: 全角・半角・結合文字が混ざっても `cell_cols` は `text` と同じ長さで単調。
    /// ずれると描画のセル写像と links のスパンが 1 文字ずつ狂う
    #[test]
    fn 結合文字を混ぜてもcell_colsはtextと同じ長さで単調() {
        let term = term_with("aか\u{3099}e\u{301}b".as_bytes());
        let s = snapshot(&term, &theme());
        let line = &s.lines[0];
        assert_eq!(
            line.text.trim_end(),
            "aか\u{3099}e\u{301}b",
            "結合文字が落ちている: {:?}",
            line.text
        );
        assert_eq!(
            line.cell_cols.len(),
            line.text.chars().count(),
            "cell_cols と text の長さがずれている"
        );
        assert!(
            line.cell_cols.windows(2).all(|w| w[0] <= w[1]),
            "cell_cols が単調でない: {:?}",
            line.cell_cols
        );
        // `a`=0 / `か`=1（濁点も 1）/ `e`=3（アクセントも 3）/ `b`=4
        assert_eq!(&line.cell_cols[..6], &[0, 1, 1, 3, 3, 4]);
        assert!(line.has_wide, "全角の判定は結合文字で変わらない");
        assert_runs_cover(line);
    }

    /// #1387: 行頭の単独の結合文字は**素の空白セル**へ載る（alacritty の
    /// `width == 0` 経路は 1 つ左のセルへ寄せ、列 0 ではその場に載る）。
    /// #801 の空白セルの近道で飛ばしてはいけない
    #[test]
    fn 空白セルへ載った結合文字も落ちない() {
        let term = term_with("\u{301}".as_bytes());
        let s = snapshot(&term, &theme());
        let line = &s.lines[0];
        assert_eq!(
            line.text.trim_end(),
            " \u{301}",
            "空白セルの結合文字が落ちている: {:?}",
            line.text
        );
        assert_eq!(&line.cell_cols[..2], &[0, 0]);
        assert_runs_cover(line);
    }

    /// #1387: 複数の結合文字（濁点 + アクセント相当を 2 つ積む）も順序どおり残る。
    /// 積む順序は alacritty の `selection_to_string` と同じ（本体 → zerowidth の順）
    #[test]
    fn 複数の結合文字も順序どおり残る() {
        let term = term_sized(4, 1, "e\u{301}\u{302}".as_bytes());
        let s = snapshot(&term, &theme());
        let line = &s.lines[0];
        assert_eq!(line.text.trim_end(), "e\u{301}\u{302}");
        assert_eq!(&line.cell_cols[..3], &[0, 0, 0]);
    }
}
