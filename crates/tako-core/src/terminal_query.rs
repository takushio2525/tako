//! terminal_query — 端末の問い合わせ（色・寸法・ライト / ダーク）への応答（Issue #2350）
//!
//! ペインの中のプログラムは端末へ「背景色は何か」（OSC 11 ; ?）「文字領域は何ピクセルか」
//! （CSI 14 t）のように尋ねる。背景色で明暗を決める TUI（エディタ・ページャ・CLI）は、
//! 答えが来なければ既定へ落ちるか、答え待ちのタイムアウトで起動が遅れる。
//!
//! alacritty_terminal は問い合わせのうち、自分で答えられるもの（DA・DSR・CSI 18 t のセル数）は
//! `PtyWrite` で返すが、**色と文字領域のピクセル数は端末の持ち主（tako）へ回す**
//! （`ColorRequest` / `TextAreaSizeRequest`）。#2350 前の tako はこれらを受けても何も返して
//! いなかった（直接のペインで OSC 10 / 11 / 12 / 4 と CSI 14 t が無応答。器 = tmux 越しの
//! ペインでは tmux が tako から色を学べず、中のプログラムの色の問い合わせに答えられなかった）。
//!
//! このモジュールが答え方の正本:
//! - 色は**画面に描いている色と同じ解決**で返す: プログラムが OSC 4 / 10 / 11 / 12 で色を
//!   差し替えていればその値、無ければ今のテーマ（`tako theme colors` と同じ解決 = UI 層の
//!   `Theme`）。テーマを切り替えた後の問い合わせには切り替えた後の色が返る
//! - 寸法はペインのセル数 × セルのピクセル数（= 子へ TIOCSWINSZ で見せている値と同じ）
//! - **ライト / ダークの通知（mode 2031）**: `CSI ? 996 n` には背景色の明暗で
//!   `CSI ? 997 ; 1|2 n` を返し、`CSI ? 2031 h` で購読したペインへはテーマを切り替えたときに
//!   同じ形で知らせる。**器（tmux）は attach 時に外側の端末へこれを購読し、知らせを受けると
//!   色を問い合わせ直す**（tmux は最初に学んだ色をキャッシュするので、知らせが無いと
//!   テーマを切り替えても器越しのペインは古い色を答え続ける）。購読していないペインへは
//!   送らない（997 を知らない古い tmux へ送ると、中のペインへ文字として漏れ得る）
//! - **OSC 52 の読み出し（クリップボードの中身）には答えない**。ペインの中のプログラム
//!   （ssh 先・信頼できないスクリプトを含む）が、利用者がコピーした秘密を読めてしまうため。
//!   1 段目は Term の設定（`terminal::term_config` の `osc52` = コピーだけ）で、読み出しは
//!   tako まで届かずに捨てられる。`ClipboardLoad` を断るのはその内側の 2 段目（届いた分だけ数える）
//!
//! 何に答えるかは `tako check-health` / MCP `tako_check_health` の `terminal_queries` 節で
//! 読める（[`report`]。開発不変条件）。A/B は `TAKO_2350_LEGACY=1`（#2350 前 = 色・
//! CSI 14 t・mode 2031 に答えない）

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{NamedColor, Rgb as AnsiRgb};

use crate::i18n::Text;
use crate::theme::{Rgb, Theme};

/// tako が答え方を決めている問い合わせの種類（`terminal_queries` 節の行。並びもこの順）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// OSC 10 ; ? — 前景色
    Foreground,
    /// OSC 11 ; ? — 背景色
    Background,
    /// OSC 12 ; ? — カーソルの色
    Cursor,
    /// OSC 4 ; n ; ? — 256 色のパレット
    Palette,
    /// CSI ? 996 n — 今ライトかダークか（mode 2031）
    ColorScheme,
    /// CSI 14 t — 文字領域のピクセル数
    TextAreaPixels,
    /// CSI 18 t — 文字領域のセル数（alacritty_terminal の Term が自分で答える）
    TextAreaCells,
    /// OSC 52 ; c ; ? — クリップボードの読み出し（答えない）
    ClipboardRead,
}

/// その種類に誰がどう答えるか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Responder {
    /// tako が答える（色・明暗・CSI 14 t）
    Tako,
    /// alacritty_terminal の Term が自分で答える（tako は書き戻すだけ）
    Emulator,
    /// 答えない（理由は [`QueryKind::decline_reason`]）
    Declined,
}

impl Responder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tako => "tako",
            Self::Emulator => "emulator",
            Self::Declined => "declined",
        }
    }
}

impl QueryKind {
    pub const ALL: [Self; 8] = [
        Self::Foreground,
        Self::Background,
        Self::Cursor,
        Self::Palette,
        Self::ColorScheme,
        Self::TextAreaPixels,
        Self::TextAreaCells,
        Self::ClipboardRead,
    ];

    /// 機械キー（`terminal_queries.kinds[].kind`）
    pub fn key(self) -> &'static str {
        match self {
            Self::Foreground => "osc10",
            Self::Background => "osc11",
            Self::Cursor => "osc12",
            Self::Palette => "osc4",
            Self::ColorScheme => "dsr996",
            Self::TextAreaPixels => "csi14t",
            Self::TextAreaCells => "csi18t",
            Self::ClipboardRead => "osc52_read",
        }
    }

    /// 問い合わせの形（人が読む）
    pub fn sequence(self) -> &'static str {
        match self {
            Self::Foreground => "OSC 10 ; ?",
            Self::Background => "OSC 11 ; ?",
            Self::Cursor => "OSC 12 ; ?",
            Self::Palette => "OSC 4 ; n ; ?",
            Self::ColorScheme => "CSI ? 996 n",
            Self::TextAreaPixels => "CSI 14 t",
            Self::TextAreaCells => "CSI 18 t",
            Self::ClipboardRead => "OSC 52 ; c ; ?",
        }
    }

    /// 答えの出どころ（`terminal_queries.kinds[].from`。答えない種類は None）
    pub fn source(self) -> Option<&'static str> {
        match self {
            Self::Foreground => Some("theme.foreground"),
            Self::Background => Some("theme.background"),
            Self::Cursor => Some("theme.cursor"),
            Self::Palette => Some("theme.palette"),
            Self::ColorScheme => Some("theme.background.luma"),
            Self::TextAreaPixels => Some("pane.pixels"),
            Self::TextAreaCells => Some("pane.cells"),
            Self::ClipboardRead => None,
        }
    }

    /// 誰が答えるか（**`legacy` では tako が答えていた種類が答えなくなる** = #2350 前）
    pub fn responder(self, legacy: bool) -> Responder {
        match self {
            Self::TextAreaCells => Responder::Emulator,
            Self::ClipboardRead => Responder::Declined,
            _ if legacy => Responder::Declined,
            _ => Responder::Tako,
        }
    }

    /// 答えない理由（答える種類は None）
    pub fn decline_reason(self, legacy: bool) -> Option<Text> {
        match self.responder(legacy) {
            Responder::Tako | Responder::Emulator => None,
            Responder::Declined if self == Self::ClipboardRead => Some(Text::new(
                "クリップボードの中身を渡すと、ペインの中のプログラム（ssh 先や信頼できない\
                 スクリプトを含む）が利用者のコピーした秘密を読める",
                "handing over the clipboard would let any program in the pane (including \
                 over ssh or an untrusted script) read secrets the user copied",
            )),
            Responder::Declined => Some(Text::new(
                "TAKO_2350_LEGACY のため答えない（#2350 前の挙動）",
                "not answered because of TAKO_2350_LEGACY (behavior before #2350)",
            )),
        }
    }

    /// 色の問い合わせの添字（alacritty の `Colors` の添字）から種類を引く。
    /// 問い合わせで届き得ない添字（減光色など 259 以上）は None
    pub fn of_color_index(index: usize) -> Option<Self> {
        match index {
            0..=255 => Some(Self::Palette),
            i if i == NamedColor::Foreground as usize => Some(Self::Foreground),
            i if i == NamedColor::Background as usize => Some(Self::Background),
            i if i == NamedColor::Cursor as usize => Some(Self::Cursor),
            _ => None,
        }
    }

    fn slot(self) -> usize {
        Self::ALL
            .iter()
            .position(|k| *k == self)
            .expect("ALL は全種類を持つ")
    }
}

/// 今のテーマが要る問い合わせ。tako-core は UI 層へ渡し（`SessionNotice::ThemeQuery`）、
/// UI 層が**受け取ったその場で**テーマを添えて `TerminalSession::answer_theme_query` へ戻す
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeQuery {
    /// OSC 10 / 11 / 12 / 4 の `?`
    Color(ColorQuery),
    /// `CSI ? 996 n`（タップが拾う。vte は捨てる）
    ColorScheme,
}

impl ThemeQuery {
    /// 答えの文字列。`overrides` はプログラムが OSC で差し替えた色（`Term::colors`）
    pub fn reply(&self, overrides: &Colors, theme: &Theme) -> Option<String> {
        match self {
            Self::Color(query) => query.reply(overrides, theme),
            Self::ColorScheme => {
                note(QueryKind::ColorScheme, Outcome::Answered);
                Some(color_scheme_report(theme).to_string())
            }
        }
    }
}

/// 色の問い合わせ（OSC 10 / 11 / 12 / 4 の `?`）。答えの書式（OSC の番号・終端の BEL / ST）は
/// alacritty_terminal が問い合わせの形から作った `format` が持つ
#[derive(Clone)]
pub struct ColorQuery {
    index: usize,
    format: Arc<dyn Fn(AnsiRgb) -> String + Sync + Send + 'static>,
}

impl ColorQuery {
    pub fn new(
        index: usize,
        format: Arc<dyn Fn(AnsiRgb) -> String + Sync + Send + 'static>,
    ) -> Self {
        Self { index, format }
    }

    /// alacritty の `Colors` の添字（0..=255 パレット / 256 前景 / 257 背景 / 258 カーソル）
    pub fn index(&self) -> usize {
        self.index
    }

    /// 答えの文字列。解決できない添字（問い合わせで届き得ない番号）は None = 答えない（数は数える）
    pub fn reply(&self, overrides: &Colors, theme: &Theme) -> Option<String> {
        let Some(rgb) = color_at(self.index, overrides, theme) else {
            DECLINED_COLOR.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        if let Some(kind) = QueryKind::of_color_index(self.index) {
            note(kind, Outcome::Answered);
        }
        Some((self.format)(AnsiRgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }))
    }
}

impl std::fmt::Debug for ColorQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ColorQuery({})", self.index)
    }
}

impl PartialEq for ColorQuery {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && Arc::ptr_eq(&self.format, &other.format)
    }
}

impl Eq for ColorQuery {}

/// 色の問い合わせの添字を RGB へ解決する。**画面の色の解決（`screen::resolve_color`）と同じ規則**:
/// プログラムが差し替えた色があればそれ、無ければテーマ（0–15 は ANSI、16–255 は xterm の
/// 立方体とグレースケール、256 前景 / 257 背景 / 258 カーソル）。それ以外の添字は None
pub fn color_at(index: usize, overrides: &Colors, theme: &Theme) -> Option<Rgb> {
    let kind = QueryKind::of_color_index(index)?;
    if let Some(c) = overrides[index] {
        return Some(Rgb::new(c.r, c.g, c.b));
    }
    Some(match kind {
        QueryKind::Palette => theme.indexed_color(index as u8),
        QueryKind::Foreground => theme.foreground,
        QueryKind::Background => theme.background,
        QueryKind::Cursor => theme.cursor,
        _ => return None,
    })
}

/// 背景色が暗いか（mode 2031 の 1 = ダーク / 2 = ライト）。明暗はテーマの名前ではなく
/// **OSC 11 で答える背景色**の輝度で決める（プリセットの背景がどちらでも、色の答えと食い違わない）。
/// 重みは Rec. 709 の輝度（ガンマのまま）で、中間の 128 未満をダークとする
pub fn is_dark(background: Rgb) -> bool {
    let luma = 2126 * u32::from(background.r)
        + 7152 * u32::from(background.g)
        + 722 * u32::from(background.b);
    luma < 128 * 10_000
}

/// ライト / ダークの知らせ（`CSI ? 997 ; 1|2 n`）。`CSI ? 996 n` の答えと、購読した
/// ペインへのテーマ切り替えの通知で同じ形を使う
pub fn color_scheme_report(theme: &Theme) -> &'static str {
    if is_dark(theme.background) {
        "\x1b[?997;1n"
    } else {
        "\x1b[?997;2n"
    }
}

/// テーマの切り替えで、問い合わせの答え（前景・背景・カーソル・ANSI 16 色）が変わるか。
/// 変わらない切り替え（フォントだけ等）では購読したペインへ知らせない
pub fn answers_differ(old: &Theme, new: &Theme) -> bool {
    old.background != new.background
        || old.foreground != new.foreground
        || old.cursor != new.cursor
        || old.ansi != new.ansi
}

/// 購読したペインへテーマの切り替えを知らせたことを数える
pub fn note_scheme_notice() {
    SCHEME_NOTICES.fetch_add(1, Ordering::Relaxed);
}

/// CSI 14 t（文字領域のピクセル数）の答え `ESC [ 4 ; 高さ ; 幅 t`。値は子へ TIOCSWINSZ で
/// 見せている `ws_ypixel` / `ws_xpixel` と同じ（行数 × セルの高さ・列数 × セルの幅）。
/// alacritty の書式の関数は u16 の掛け算でデバッグビルドだと桁あふれで止まり得るので、
/// 書式（xterm の標準）はこちらで u32 で組む
pub fn text_area_pixels_reply(size: WindowSize) -> String {
    let height = u32::from(size.num_lines) * u32::from(size.cell_height);
    let width = u32::from(size.num_cols) * u32::from(size.cell_width);
    note(QueryKind::TextAreaPixels, Outcome::Answered);
    format!("\x1b[4;{height};{width}t")
}

/// OSC 52 の読み出しを断ったことを数える（答えは書かない）
pub fn decline_clipboard_read() {
    note(QueryKind::ClipboardRead, Outcome::Declined);
}

/// #2350 の A/B（`TAKO_2350_LEGACY=1`）。同一バイナリのまま修正前の挙動（色・CSI 14 t・
/// mode 2031 に答えない）へ戻す。**ゲートはこの 1 か所**
pub fn legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2350_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

#[derive(Clone, Copy)]
enum Outcome {
    Answered,
    Declined,
}

const KINDS: usize = QueryKind::ALL.len();

/// 起動からの回数（プロセス全体。ペインを跨いで数える = 診断用）
static ANSWERED: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
static DECLINED: [AtomicU64; KINDS] = [const { AtomicU64::new(0) }; KINDS];
/// 解決できない添字の色の問い合わせ（種類に割り当てられないので別に数える）
static DECLINED_COLOR: AtomicU64 = AtomicU64::new(0);
/// 購読したペインへテーマの切り替えを知らせた回数
static SCHEME_NOTICES: AtomicU64 = AtomicU64::new(0);

fn note(kind: QueryKind, outcome: Outcome) {
    let table = match outcome {
        Outcome::Answered => &ANSWERED,
        Outcome::Declined => &DECLINED,
    };
    table[kind.slot()].fetch_add(1, Ordering::Relaxed);
}

/// 1 種類ぶんの記録（`terminal_queries.kinds[]`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindReport {
    pub kind: QueryKind,
    pub responder: Responder,
    /// tako が答えた回数（起動から）。答えるのが Term（CSI 18 t）なら None = tako は数えない
    pub answered: Option<u64>,
    /// 断った回数（起動から）。Term の設定が先に捨てた問い合わせ（OSC 52 の読み出し）は
    /// tako まで届かないので数えない
    pub declined: u64,
}

/// `tako check-health` の `terminal_queries` 節の材料
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub legacy: bool,
    pub kinds: Vec<KindReport>,
    /// 解決できない添字の色の問い合わせを断った回数（OSC 4 の範囲外など）
    pub declined_color: u64,
    /// 購読したペインへテーマの切り替えを知らせた回数
    pub scheme_notices: u64,
}

/// 今の答え方と起動からの回数
pub fn report() -> Report {
    report_with(legacy())
}

fn report_with(legacy: bool) -> Report {
    let kinds = QueryKind::ALL
        .iter()
        .map(|&kind| {
            let responder = kind.responder(legacy);
            KindReport {
                kind,
                responder,
                answered: (responder != Responder::Emulator)
                    .then(|| ANSWERED[kind.slot()].load(Ordering::Relaxed)),
                declined: DECLINED[kind.slot()].load(Ordering::Relaxed),
            }
        })
        .collect();
    Report {
        legacy,
        kinds,
        declined_color: DECLINED_COLOR.load(Ordering::Relaxed),
        scheme_notices: SCHEME_NOTICES.load(Ordering::Relaxed),
    }
}

impl Report {
    /// tako が答えた回数の合計（Term が答える CSI 18 t は数えていないので入らない）
    pub fn answered_total(&self) -> u64 {
        self.kinds.iter().filter_map(|k| k.answered).sum()
    }

    /// 断った回数の合計
    pub fn declined_total(&self) -> u64 {
        self.kinds.iter().map(|k| k.declined).sum::<u64>() + self.declined_color
    }

    /// 人が読む 1 行（`tako check-health` の要約。CLI はこの文字列をそのまま出す）
    pub fn summary(&self) -> Text {
        let (answered, declined) = (self.answered_total(), self.declined_total());
        if self.legacy {
            return Text::new(
                format!(
                    "端末の問い合わせ: TAKO_2350_LEGACY のため色（OSC 10 / 11 / 12 / 4）・明暗（CSI ? 996 n）・\
                     CSI 14 t に答えない（#2350 前の挙動。CSI 18 t は端末が答える）・起動から断り {declined} 回"
                ),
                format!(
                    "Terminal queries: colors (OSC 10 / 11 / 12 / 4), light/dark (CSI ? 996 n) and \
                     CSI 14 t are not answered because of TAKO_2350_LEGACY (behavior before #2350; \
                     CSI 18 t is answered by the emulator); {declined} declined since launch"
                ),
            );
        }
        let notices = self.scheme_notices;
        Text::new(
            format!(
                "端末の問い合わせ: 色（OSC 10 / 11 / 12 / 4）・明暗（CSI ? 996 n）・寸法（CSI 14 t / 18 t）に\
                 答える・OSC 52 の読み出しには答えない（起動から応答 {answered} 回・断り {declined} 回・\
                 テーマ切り替えの通知 {notices} 回）"
            ),
            format!(
                "Terminal queries: colors (OSC 10 / 11 / 12 / 4), light/dark (CSI ? 996 n) and size \
                 (CSI 14 t / 18 t) are answered; OSC 52 reads are not ({answered} answered, \
                 {declined} declined, {notices} theme-change notices since launch)"
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osc_format(prefix: &'static str) -> Arc<dyn Fn(AnsiRgb) -> String + Sync + Send> {
        // alacritty_terminal の `dynamic_color_sequence` と同じ書式（BEL 終端）
        Arc::new(move |c: AnsiRgb| {
            format!(
                "\x1b]{prefix};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\x07",
                c.r, c.g, c.b
            )
        })
    }

    #[test]
    fn 色の問い合わせはテーマの色で答える() {
        let theme = Theme::default_dark();
        let colors = Colors::default();
        let bg = ColorQuery::new(NamedColor::Background as usize, osc_format("11"));
        let b = theme.background;
        let want = format!(
            "\x1b]11;rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\x07",
            b.r, b.g, b.b
        );
        assert_eq!(bg.reply(&colors, &theme), Some(want.clone()));
        assert_eq!(
            ThemeQuery::Color(bg).reply(&colors, &theme),
            Some(want),
            "ThemeQuery も同じ答え"
        );
        let light = Theme::default_light();
        assert_ne!(theme.background, light.background, "前提: 明暗で背景が違う");
        assert_eq!(
            color_at(NamedColor::Background as usize, &colors, &light),
            Some(light.background),
            "テーマを切り替えたら切り替えた後の色"
        );
        assert_eq!(
            color_at(NamedColor::Foreground as usize, &colors, &theme),
            Some(theme.foreground)
        );
        assert_eq!(
            color_at(NamedColor::Cursor as usize, &colors, &theme),
            Some(theme.cursor)
        );
        assert_eq!(color_at(1, &colors, &theme), Some(theme.ansi[1]));
        // 立方体の (5,0,4) = rgb(255,0,215)（xterm の標準）
        assert_eq!(color_at(200, &colors, &theme), Some(Rgb::new(255, 0, 215)));
    }

    #[test]
    fn プログラムが差し替えた色はその値で答える() {
        let theme = Theme::default_dark();
        let mut colors = Colors::default();
        colors[NamedColor::Background] = Some(AnsiRgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        });
        colors[3] = Some(AnsiRgb { r: 1, g: 2, b: 3 });
        assert_eq!(
            color_at(NamedColor::Background as usize, &colors, &theme),
            Some(Rgb::new(0x12, 0x34, 0x56))
        );
        assert_eq!(color_at(3, &colors, &theme), Some(Rgb::new(1, 2, 3)));
    }

    #[test]
    fn 問い合わせで届き得ない添字には答えない() {
        let theme = Theme::default_dark();
        let colors = Colors::default();
        for index in [259, 267, 268, 269, 999] {
            assert_eq!(color_at(index, &colors, &theme), None, "添字 {index}");
            let q = ColorQuery::new(index, osc_format("4"));
            assert_eq!(q.reply(&colors, &theme), None, "添字 {index}");
        }
    }

    #[test]
    fn 明暗は背景色の輝度で決まりテーマの明暗と一致する() {
        let dark = Theme::default_dark();
        let light = Theme::default_light();
        assert_eq!(color_scheme_report(&dark), "\x1b[?997;1n");
        assert_eq!(color_scheme_report(&light), "\x1b[?997;2n");
        assert_eq!(
            ThemeQuery::ColorScheme
                .reply(&Colors::default(), &light)
                .as_deref(),
            Some("\x1b[?997;2n")
        );
        assert!(is_dark(Rgb::new(0, 0, 0)));
        assert!(!is_dark(Rgb::new(255, 255, 255)));
        // 境界: 灰色 127 はダーク・128 はライト
        assert!(is_dark(Rgb::new(127, 127, 127)));
        assert!(!is_dark(Rgb::new(128, 128, 128)));
    }

    #[test]
    fn 答えが変わる切り替えだけを知らせる() {
        let dark = Theme::default_dark();
        let light = Theme::default_light();
        assert!(answers_differ(&dark, &light));
        let mut font_only = dark.clone();
        font_only.font_size += 2.0;
        font_only.accent = Rgb::new(1, 2, 3);
        assert!(
            !answers_differ(&dark, &font_only),
            "問い合わせの答えに出ない色やフォントだけの変更は知らせない"
        );
        let mut red = dark.clone();
        red.ansi[1] = Rgb::new(9, 9, 9);
        assert!(answers_differ(&dark, &red));
    }

    #[test]
    fn 文字領域のピクセル数はセル数とセルの寸法の積() {
        let size = WindowSize {
            num_lines: 27,
            num_cols: 119,
            cell_width: 8,
            cell_height: 17,
        };
        assert_eq!(text_area_pixels_reply(size), "\x1b[4;459;952t");
        // u16 の掛け算なら桁あふれする寸法でも止まらない
        let huge = WindowSize {
            num_lines: 2000,
            num_cols: 3000,
            cell_width: 40,
            cell_height: 80,
        };
        assert_eq!(text_area_pixels_reply(huge), "\x1b[4;160000;120000t");
    }

    #[test]
    fn osc52の読み出しには答えない() {
        assert_eq!(
            QueryKind::ClipboardRead.responder(false),
            Responder::Declined
        );
        assert_eq!(
            QueryKind::ClipboardRead.responder(true),
            Responder::Declined
        );
        assert!(QueryKind::ClipboardRead.decline_reason(false).is_some());
        assert_eq!(QueryKind::ClipboardRead.source(), None);
    }

    #[test]
    fn 答え方の表はlegacyでtakoが答える種類だけが断りへ変わる() {
        for kind in QueryKind::ALL {
            let now = kind.responder(false);
            let old = kind.responder(true);
            match kind {
                QueryKind::TextAreaCells => {
                    assert_eq!((now, old), (Responder::Emulator, Responder::Emulator))
                }
                QueryKind::ClipboardRead => {
                    assert_eq!((now, old), (Responder::Declined, Responder::Declined))
                }
                _ => assert_eq!(
                    (now, old),
                    (Responder::Tako, Responder::Declined),
                    "{kind:?}"
                ),
            }
            assert_eq!(
                kind.decline_reason(false).is_some(),
                now == Responder::Declined
            );
            assert_eq!(
                kind.decline_reason(true).is_some(),
                old == Responder::Declined
            );
            assert_eq!(kind.source().is_none(), kind == QueryKind::ClipboardRead);
        }
        let mut keys: Vec<_> = QueryKind::ALL.iter().map(|k| k.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), QueryKind::ALL.len(), "機械キーは重ならない");
    }

    #[test]
    fn 報告はterm任せの種類を数えない() {
        let r = report_with(false);
        assert_eq!(r.kinds.len(), QueryKind::ALL.len());
        for k in &r.kinds {
            assert_eq!(
                k.answered.is_none(),
                k.kind == QueryKind::TextAreaCells,
                "{:?}",
                k.kind
            );
        }
        assert!(r.summary().ja().contains("OSC 52 の読み出しには答えない"));
        assert!(r.summary().en().contains("OSC 52 reads are not"));
        assert!(report_with(true)
            .summary()
            .ja()
            .contains("TAKO_2350_LEGACY"));
    }
}
