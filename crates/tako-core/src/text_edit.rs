//! 軽量テキスト編集モデル（FR-3.5）。
//!
//! UTF-8 バイト境界を不変条件としてカーソル・選択を管理し、保存時は読み込み時の
//! 内容と現在のファイルを比較して外部変更を検知する。GPUI に依存しないため、GUI・
//! dispatch・CLI・MCP の全経路が同じ編集セマンティクスを使える。
//!
//! undo/redo（#195 / #1651）: 積むのは**差分**（置き換えた範囲 + 置換前後の文字列）で、
//! 連続した打鍵・連続した削除は 1 塊にまとまる（`hello` は undo 1 回で消える）。
//! 上限は「操作数」と「履歴のバイト数」の**先に効いたほう**。
//! 検索（#195 / #1653）: バイト位置ベースのインクリメンタル検索と置換。条件は
//! [`SearchOptions`] で、**既定は大文字小文字を区別する**（コード編集で `value` と `Value` を
//! 同じ語として扱うと全置換がデータを壊す）。区別しない検索の小文字写しは検索したときだけ作り、
//! 本文が変わるまで使い回す。
//!
//! 改行コード（#1650）: バッファは**ファイルのバイト列をそのまま**持つ。`\r\n` は
//! 「1 つの行区切り」として扱い、カーソルは CR と LF のあいだに入らない。新しく足す
//! 改行（Enter・挿入テキスト・置換テキスト）だけが [`LineEnding`] の多数派に揃うので、
//! 既存行の改行は 1 バイトも書き換わらない（混在ファイルを開いて閉じてもバイト一致）。

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use thiserror::Error;

use crate::platform::support::Platform;

/// undo 履歴に積む操作の数の上限（#195）
const UNDO_LIMIT: usize = 1000;

/// undo 履歴が持ってよい本文のバイト数（#1651）。
///
/// 差分にしたので打鍵では 1 操作 1 バイト前後しか積まないが、`set_text` や全置換は
/// 1 操作で「置換前 + 置換後」= 本文 2 本ぶんを持つ。**操作数だけでは上限にならない**
/// ので、バイト数の上限を併せて置く（画像は FR-3.17 で 512MiB の予算を持っているのに
/// テキストだけ無予算だったのが #1651）。
///
/// 8 MiB は「編集対象になる現実的なテキストファイル（数 MB）を丸ごと差し替える
/// 操作が 1 回ぶん収まる」大きさ。これを 1 操作で超える編集は履歴 1 件だけ残す
/// （直前の操作を取り消せることのほうが、上限を厳密に守ることより大事）
const UNDO_BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// 連続した編集を 1 塊とみなす時間の幅（ミリ秒。#1651）。
///
/// これより間が空いた打鍵は別の塊にする（考えながら打った区切りで undo が止まる）。
/// 自動保存のデバウンス（500ms。FR-3.5）と揃えてあるので、
/// 「保存が走るほど手が止まった」ところが塊の切れ目になる
const COALESCE_WINDOW_MS: u64 = 500;

/// 改行コード（#1650）。
///
/// tako が行区切りとして扱うのは `\n` と `\r\n` の 2 つだけ。単独の `\r`
/// （Classic Mac OS の流儀）は行内の制御文字として素通しする
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    /// 外（CLI / MCP / 診断）へ出す名前（#1658）。`as_str` は改行そのものなので、
    /// そのまま JSON へ入れると応答の中で行が割れる
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "lf",
            Self::Crlf => "crlf",
        }
    }

    /// 本文の**多数派**を返す。改行が 1 つも無ければ `None`。
    ///
    /// 同数のときは LF を選ぶ（移植性の高い側へ倒す）。多数派で決めるのは、
    /// 1 行だけ流儀の違う行が混ざっているファイルで Enter の挿入文字が
    /// その 1 行に引きずられないようにするため
    pub fn detect(text: &str) -> Option<Self> {
        let lf = text.matches('\n').count();
        if lf == 0 {
            return None;
        }
        let crlf = text.matches("\r\n").count();
        Some(if crlf > lf - crlf {
            Self::Crlf
        } else {
            Self::Lf
        })
    }

    /// 改行を 1 つも持たないファイルで使う改行コード（#1650）。
    ///
    /// ここに来るのは**空ファイルと「改行 0 の 1 行ファイル」だけ**（改行が 1 つでも
    /// あれば [`Self::detect`] が勝つ）。本文の中に手掛かりが無いので、主要エディタと
    /// 同じく **OS の流儀に従う**（VS Code の `files.eol: auto` / Visual Studio /
    /// メモ帳）。Windows の利用者が tako で作った新規ファイルを他の Windows ツールで
    /// 開いたときに驚きが無いことを優先する（tako はゼロコンフィグが最優先の設計原則）。
    ///
    /// `platform` を**引数で受ける**ので、macOS 上からでも Windows 側の腕を検査できる
    /// （`platform::keys` と同じ型。これが無いと「Windows でどうなるか」を
    /// 実機無しに固定できない = #1650 で実際に CI だけが落ちた）
    pub fn for_new_file(platform: Platform) -> Self {
        match platform {
            Platform::MacOs => Self::Lf,
            Platform::Windows => Self::Crlf,
        }
    }

    /// 実行中のプラットフォームでの [`Self::for_new_file`]
    pub fn platform_default() -> Self {
        Self::for_new_file(Platform::current())
    }

    /// 検出できなければ [`Self::platform_default`] へ倒す
    pub fn detect_or_default(text: &str) -> Self {
        Self::detect(text).unwrap_or_else(Self::platform_default)
    }
}

/// インデント 1 段の単位（#1654）。Tab / ⇧Tab が足し引きし、開き括弧の直後の Enter が 1 段深くする量
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentUnit {
    Tab,
    /// 半角スペース n 個（2〜8）
    Spaces(usize),
}

/// タブ 1 文字を何桁と数えるか（#1654。スペースでインデントするファイルに混ざったタブを
/// 1 段ぶんとして扱うときと、タブでインデントするファイルに混ざったスペースを外すときの幅）。
///
/// 上下移動の桁の記憶（#1742）では**タブストップの間隔**として使う（タブは次の
/// 4 の倍数の桁まで進む。VS Code / Zed の既定と同じ）。タブ幅の正本はここ 1 か所
const TAB_COLUMNS: usize = 4;

/// 1 文字ぶん進んだあとの表示桁（#1742）。`column` はその文字の手前の桁。
///
/// 全角（UAX #11 の W / F）= 2・タブ = 次のタブストップ（[`TAB_COLUMNS`] 桁ごと）・
/// 結合文字など幅を持たない文字 = 0・制御文字（単独の CR など）= 0。
/// 全角の判定は端末のセル幅と同じ `unicode-width`（alacritty_terminal が使う版）で、
/// **エディタの表示幅はこの関数 1 つが決める**
fn advance_display_column(column: usize, c: char) -> usize {
    match c {
        '\t' => column + TAB_COLUMNS - column % TAB_COLUMNS,
        _ => column + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0),
    }
}

/// 行頭から `line_prefix` の終わりまでの表示幅（桁。#1742）
fn display_width(line_prefix: &str) -> usize {
    line_prefix.chars().fold(0, advance_display_column)
}

/// 行の中で表示桁 `goal` にいちばん近い文字境界（行頭からのバイト。#1742）。
///
/// 全角文字やタブの途中に当たったら近いほうの端へ寄せ、等距離なら手前へ寄せる
/// （VS Code の `columnFromVisibleColumn` と同じ）。行が短ければ行末
fn byte_for_display_column(line: &str, goal: usize) -> usize {
    let mut column = 0;
    for (i, c) in line.char_indices() {
        let next = advance_display_column(column, c);
        if next > goal {
            return if next - goal < goal - column {
                i + c.len_utf8()
            } else {
                i
            };
        }
        column = next;
    }
    line.len()
}

/// インデントの推定に見る行数の上限（#1654。VS Code の `guessIndentation` と同じく先頭から標本を取る）
const INDENT_SAMPLE_LINES: usize = 10_000;

impl IndentUnit {
    /// 本文の既存行から推定する（#1654）。インデントされた行が 1 つも無ければ `None`。
    ///
    /// **タブで始まる行がスペースで始まる行より多ければタブ**。そうでなければスペースで、
    /// 幅は「隣り合う行のインデントの差」（2〜8 だけを数える）の**最頻値**にする。
    /// 差を見るのは、深い入れ子しか無いファイル（全行 8 桁）や継続行の桁揃えに
    /// 引きずられないため。1 の差（`/** … */` の ` * ` など）は数えない。
    /// 差が 1 つも取れなければ、いちばん浅いインデント幅（2〜8 のとき）を使う
    pub fn detect(text: &str) -> Option<Self> {
        let mut tab_lines = 0usize;
        let mut space_lines = 0usize;
        let mut deltas = [0usize; 9];
        let mut previous: Option<usize> = None;
        let mut shallowest: Option<usize> = None;
        for line in text.split('\n').take(INDENT_SAMPLE_LINES) {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() {
                continue;
            }
            let width = if line.starts_with('\t') {
                tab_lines += 1;
                previous = None;
                continue;
            } else {
                line.len() - line.trim_start_matches(' ').len()
            };
            if width > 0 {
                space_lines += 1;
                shallowest = Some(shallowest.map_or(width, |s: usize| s.min(width)));
            }
            if let Some(prev) = previous {
                let delta = width.abs_diff(prev);
                if (2..=8).contains(&delta) {
                    deltas[delta] += 1;
                }
            }
            previous = Some(width);
        }
        if tab_lines == 0 && space_lines == 0 {
            return None;
        }
        if tab_lines > space_lines {
            return Some(Self::Tab);
        }
        // 同数なら 4 → 2 → 8 → その他の順に倒す（多いものから `max_by_key` は最後の最大を返すので
        // 優先度の低い順に並べる）
        let best = [3usize, 5, 6, 7, 8, 2, 4]
            .into_iter()
            .filter(|w| deltas[*w] > 0)
            .max_by_key(|w| deltas[*w]);
        match best {
            Some(w) => Some(Self::Spaces(w)),
            None => shallowest.filter(|w| (2..=8).contains(w)).map(Self::Spaces),
        }
    }

    /// 本文に手掛かりが無いときの既定（#1654）。**タブが文法で決まっているもの**だけタブ
    /// （Makefile はレシピ行がタブ必須・Go は gofmt がタブ）。それ以外は 4 スペース
    /// （VS Code / Zed の既定と同じ）
    pub fn for_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(name.as_str(), "makefile" | "gnumakefile")
            || matches!(ext.as_str(), "mk" | "go")
        {
            Self::Tab
        } else {
            Self::Spaces(4)
        }
    }

    /// 1 段ぶんの文字列
    pub fn text(self) -> String {
        match self {
            Self::Tab => "\t".to_string(),
            Self::Spaces(n) => " ".repeat(n),
        }
    }

    /// 外（CLI / MCP の `document`）へ出す名前（`tab` / `spaces:4`）
    pub fn label(self) -> String {
        match self {
            Self::Tab => "tab".to_string(),
            Self::Spaces(n) => format!("spaces:{n}"),
        }
    }
}

/// Enter で 1 段深くする規則（#1654）。拡張子で決める
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IndentRules {
    /// 行が `{` / `[` / `(` で終わっていたら深くする
    brackets: bool,
    /// 行が `:` で終わっていたら深くする（Python のブロック・YAML の入れ子）
    colon: bool,
}

impl IndentRules {
    /// Markdown・プレーンテキストは**継承だけ**（`[` はリンク、`(` は括弧書きで、
    /// 入れ子の始まりではない）。Python と YAML は `:` でも深くする。
    /// それ以外（コード全般）は開き括弧で深くする
    fn for_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" | "mdx" | "txt" | "rst" | "adoc" => Self {
                brackets: false,
                colon: false,
            },
            "py" | "pyi" | "pyw" | "yml" | "yaml" => Self {
                brackets: true,
                colon: true,
            },
            _ => Self {
                brackets: true,
                colon: false,
            },
        }
    }
}

/// 開き括弧に対応する閉じ括弧
fn closing_bracket(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
}

/// カーソルの動かし方（FR-3.5 / #1652）。
///
/// GUI の打鍵（`platform::editor_keys` の表）と CLI / MCP（`tako edit move` /
/// `tako_preview_move`）が**同じ値**を指す。外から名前で指すときの綴りは
/// [`Self::name`] の 1 か所（CLI の引数・MCP の enum・エラー文がすべてここを引く）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMovement {
    /// 1 文字左へ。選択中（⇧ なし）は動かずに選択の始点へ畳む（#1742）
    Left,
    /// 1 文字右へ。選択中（⇧ なし）は動かずに選択の終点へ畳む（#1742）
    Right,
    Up,
    Down,
    /// 前の語の頭へ（#1652）。行頭では前の行へまたぐ
    WordLeft,
    /// 次の語の末尾へ（#1652）。行末では次の行へまたぐ
    WordRight,
    /// 行頭（桁 0。macOS の ⌃A。#1742）
    LineStart,
    /// 最初の非空白 ⇄ 桁 0 を行き来する行頭（smart Home。#1652）
    SmartLineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    /// 1 画面ぶん上へ（行数は [`TextBuffer::set_viewport_lines`]。#1652）
    PageUp,
    /// 1 画面ぶん下へ（#1652）
    PageDown,
}

impl CursorMovement {
    /// 全種類（名前の表・MCP の enum・単体の網羅に使う）
    pub const ALL: [Self; 13] = [
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::WordLeft,
        Self::WordRight,
        Self::LineStart,
        Self::SmartLineStart,
        Self::LineEnd,
        Self::DocumentStart,
        Self::DocumentEnd,
        Self::PageUp,
        Self::PageDown,
    ];

    /// 外（CLI / MCP）から指すときの綴り（#1652）
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
            Self::Down => "down",
            Self::WordLeft => "word-left",
            Self::WordRight => "word-right",
            Self::LineStart => "line-start",
            Self::SmartLineStart => "smart-home",
            Self::LineEnd => "line-end",
            Self::DocumentStart => "doc-start",
            Self::DocumentEnd => "doc-end",
            Self::PageUp => "page-up",
            Self::PageDown => "page-down",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// 綴りの一覧（MCP の inputSchema の enum とエラー文の候補）
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|m| m.name()).collect()
    }

    /// 上下方向の移動か。**桁の記憶（desired column）を使い、保つ**のはこれだけ
    fn is_vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::PageUp | Self::PageDown)
    }
}

/// 消し方（FR-3.5 / #1652）。選択があればどれも「選択を消す」になる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMotion {
    /// Backspace
    CharBackward,
    /// Delete
    CharForward,
    /// 前の語の頭まで（⌥⌫ / Ctrl+Backspace）。行頭では改行だけを消す
    WordBackward,
    /// 次の語の末尾まで（⌥⌦ / Ctrl+Delete）。行末では改行だけを消す
    WordForward,
    /// 行頭まで（⌘⌫）。行頭では改行だけを消す
    ToLineStart,
    /// 行末まで（⌘⌦）。行末では改行だけを消す
    ToLineEnd,
}

impl DeleteMotion {
    pub const ALL: [Self; 6] = [
        Self::CharBackward,
        Self::CharForward,
        Self::WordBackward,
        Self::WordForward,
        Self::ToLineStart,
        Self::ToLineEnd,
    ];

    /// 外（CLI / MCP）から指すときの綴り（#1652）
    pub fn name(self) -> &'static str {
        match self {
            Self::CharBackward => "char-backward",
            Self::CharForward => "char-forward",
            Self::WordBackward => "word-backward",
            Self::WordForward => "word-forward",
            Self::ToLineStart => "to-line-start",
            Self::ToLineEnd => "to-line-end",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|m| m.name()).collect()
    }
}

/// 1 画面の行数がまだ測れていないとき（CLI から開いた直後で、器を 1 度も描いて
/// いないなど）のページ移動の行数（#1652）
const DEFAULT_PAGE_STEP: usize = 20;

#[derive(Debug, Error)]
pub enum TextEditError {
    #[error("ファイルを読み込めない: {0}")]
    Read(#[source] std::io::Error),
    #[error("UTF-8 テキストではないため編集できない")]
    InvalidUtf8,
    #[error("ファイルが外部で変更されたため保存しなかった")]
    ExternalChanged,
    /// 開いた後にファイルが消された（#1659）。上書き保存なら作り直せる
    #[error("ファイルが外部で削除されている")]
    ExternalDeleted,
    /// 読み直そうとしたディスクの中身が編集できない（上限超過・バイナリ。#1659）
    #[error("{0}")]
    NotEditable(String),
    #[error("ファイルへ保存できない: {0}")]
    Write(#[source] std::io::Error),
}

/// ディスク上のファイルと、バッファの**基準**（開いた / 最後に保存した / 読み直した時点の
/// ファイルの中身）の関係（#1659）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    /// 基準のまま（外から変わっていない）
    Unchanged,
    /// 外から書き換わった（中身が基準と違う）
    Changed,
    /// 外から消された
    Deleted,
}

impl DiskState {
    /// 外（CLI / MCP の応答）へ出す綴り
    pub fn name(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Changed => "changed",
            Self::Deleted => "deleted",
        }
    }
}

/// ディスク上のファイルと編集中の本文の差分（#1659）。
///
/// 向きは「ディスク → 編集中」= **上書き保存すると何が変わるか**（`-` がディスクにあって
/// 消える行、`+` が自分の変更）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskDiff {
    pub state: DiskState,
    pub diff: crate::text_diff::TextDiff,
}

/// 編集の種類（#1651）。**同じ種類どうししかまとまらない**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// 打鍵・IME・貼り付け（選択の無い挿入）
    Insert,
    /// Backspace（左へ伸びる削除）
    DeleteBackward,
    /// Delete（右へ伸びる削除）
    DeleteForward,
    /// 語・行単位の削除（⌥⌫ / ⌥⌦ / ⌘⌫ / ⌘⌦。#1652）。**まとめない**
    /// （1 打鍵で意味のある単位を消すので、undo 1 回で 1 単位ずつ戻るのが自然）
    DeleteSpan,
    /// 選択の差し替え・範囲置換・全置換・全文差し替え。**まとめない**
    /// （1 回の操作として意味が閉じているので、まとめると戻しすぎになる）
    Replace,
}

/// undo/redo 1 件ぶんの差分（#1651）。
///
/// 全文のスナップショットを持たない。`start` から始まる範囲を `before` ⇄ `after` で
/// 入れ替えるだけなので、1 打鍵の記録は数十バイトで済む
/// （スナップショットは 1 MB のファイルなら 1 打鍵 1 MB だった = Issue の症状）。
///
/// 改行コードも一緒に戻す。`set_text` はファイルの流儀を取り直すので、
/// これを戻さないと undo した後の Enter が別の改行を挿す（#1650 の契約）
#[derive(Debug, Clone, PartialEq, Eq)]
struct EditDelta {
    /// 置き換えた範囲の開始バイト位置
    start: usize,
    /// 置換前の中身（undo で書き戻す）
    before: String,
    /// 置換後の中身（redo で書き直す）
    after: String,
    cursor_before: usize,
    anchor_before: Option<usize>,
    cursor_after: usize,
    anchor_after: Option<usize>,
    line_ending_before: LineEnding,
    line_ending_after: LineEnding,
    kind: EditKind,
    /// この塊に最後に足した時刻（まとめの判定用。[`TextBuffer::now_millis`] の刻み）
    at_millis: u64,
}

impl EditDelta {
    /// この 1 件が抱えている本文のバイト数（予算の勘定。#1651）
    fn bytes(&self) -> usize {
        self.before.len() + self.after.len() + std::mem::size_of::<Self>()
    }
}

/// 1 回の編集の指示（#1651）。本文を書き換える口はこれを [`TextBuffer::apply_edit`] へ渡す
struct Edit<'a> {
    /// 置き換える範囲
    range: Range<usize>,
    /// 置き換えた後にそこへ入る本文
    replacement: &'a str,
    /// 編集後のカーソル
    cursor: usize,
    /// 編集後の選択端
    anchor: Option<usize>,
    kind: EditKind,
    /// この編集でファイルの改行コードが変わるなら `Some`（全文差し替えだけ）
    line_ending: Option<LineEnding>,
}

/// プロセス開始からの経過ミリ秒（#1651）。
///
/// `Instant` から絶対時刻は作れないので、起点を 1 つだけ持って引き算する。
/// **`Instant::now() - Duration` は書かない**（ブートより前へ巻き戻すと panic = #1627）。
/// 単調なので NTP の補正でも巻き戻らない
fn process_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// 検索ヒット 1 件（バイト範囲）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub start: usize,
    pub end: usize,
}

/// 検索・置換の条件（#1653）。
///
/// **既定は「大文字小文字を区別する・単語単位でない」**。コード編集では `value`（変数）と
/// `Value`（型）は別の名前なので、区別しないのを既定にすると
/// `replace_all("value", "item")` が `let value = Value::new();` を
/// `let item = item::new();` に書き換える（#1653 で実測したデータ破壊）。
/// GUI の検索欄・CLI `tako edit search|replace`・MCP `tako_preview_search|replace` が
/// どれもこの既定を使う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// 大文字小文字を区別する（既定 true）
    pub case_sensitive: bool,
    /// 単語単位（一致の前後が単語の文字に続かない位置だけを当てる。既定 false）
    pub whole_word: bool,
}

impl SearchOptions {
    /// 既定（区別する・単語単位でない）
    pub const DEFAULT: Self = Self {
        case_sensitive: true,
        whole_word: false,
    };

    /// 指定のあった項目だけを差し替える（`None` の項目はそのまま）
    pub fn with(self, case_sensitive: Option<bool>, whole_word: Option<bool>) -> Self {
        Self {
            case_sensitive: case_sensitive.unwrap_or(self.case_sensitive),
            whole_word: whole_word.unwrap_or(self.whole_word),
        }
    }

    /// 検索の指示（CLI / MCP / GUI）から、使う条件を決める（#1653）。
    ///
    /// - **新しいクエリを渡した**: 既定から組み直す = 省略した項目は既定
    ///   （区別する・単語単位でない）。呼び手が前の検索の条件を知らなくても結果が決まる
    /// - **クエリを省略した**（今の検索の中で次へ / 前へ動く・条件だけ変える）: 今の条件を
    ///   引き継ぐ。`tako edit search --direction prev` が画面のトグルを黙って戻さない
    pub fn resolve(
        current: Self,
        new_query: bool,
        case_sensitive: Option<bool>,
        whole_word: Option<bool>,
    ) -> Self {
        let base = if new_query { Self::DEFAULT } else { current };
        base.with(case_sensitive, whole_word)
    }
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `from` 以降で最初のヒット（なければ先頭へラップ）。
///
/// ヒットの一覧は 1 回の [`TextBuffer::find_all`] で得たものを使い回す（#1653。
/// 次へ / 前へのたびに全文を探し直さない）
pub fn next_hit(hits: &[SearchHit], from: usize) -> Option<&SearchHit> {
    hits.iter()
        .find(|h| h.start >= from)
        .or_else(|| hits.first())
}

/// `from` より前で最後のヒット（なければ末尾へラップ）
pub fn prev_hit(hits: &[SearchHit], from: usize) -> Option<&SearchHit> {
    hits.iter()
        .rev()
        .find(|h| h.start < from)
        .or_else(|| hits.last())
}

/// 文書内の 1 点を行と桁で表す（#1658）。
///
/// **行は 1 始まり・桁は 0 始まりの UTF-8 バイト**。この 2 つの向きが違うのは
/// 見た目の都合ではなく、`line` が人と AI が読む行番号（エディタ・`grep -n`・
/// コンパイラの診断がすべて 1 始まり）で、`column` が本文のバイト列を切る位置
/// （`&text[..column]` がそのまま通る）だから。0 行目は存在しない。
///
/// **UTF-16 の桁へは変換しない**。LSP は `positionEncoding` の既定が UTF-16 だが、
/// 変換の責務は LSP クライアント（#1007 S1）が持つ —— tako の編集 API は本文の
/// バイト列を扱う層なので、ここで UTF-16 を持つと本文を触るたびに再計算が要る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    /// 1 始まりの行番号
    pub line: usize,
    /// 0 始まりの行内 UTF-8 バイト位置
    pub column: usize,
}

impl TextPosition {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

/// 行・桁で指定した範囲編集 1 回ぶんの指示（#1658）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeEdit {
    pub start: TextPosition,
    pub end: TextPosition,
    /// 範囲に入る本文。改行はバッファの流儀へ揃う（#1650）
    pub text: String,
    /// 指定すると、文書の版がこれと違うときに**何もせず**拒否する（楽観ロック）
    pub expected_version: Option<u64>,
}

/// カーソルと選択の置き場所（#1658）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPlacement {
    /// 選択の起点。`select_to` が無ければここがカーソル
    pub cursor: TextPosition,
    /// 指定すると `cursor` からここまでを選択し、**カーソルはこちら側**へ置く
    /// （shift+クリックと同じ = 「ここまで選ぶ」の "ここ" にキャレットが来る）
    pub select_to: Option<TextPosition>,
    /// 指定すると、文書の版がこれと違うときに**何もせず**拒否する
    pub expected_version: Option<u64>,
}

/// 行・桁の指定を解けなかった理由（#1658）。
///
/// **黙って丸めない**。外（CLI / MCP）から来る範囲指定を丸めると、送り手は
/// 「3 行目を直した」と思っているのに別の場所が変わる。版（`expected_version`）で
/// 競合を弾いても、指定そのものがずれていては意味が無いので、解けない指定は
/// 本文を 1 バイトも触らずに理由を返す
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RangeEditError {
    #[error("行は 1 始まり（0 行目は無い）")]
    ZeroLine,
    #[error("行 {line} は文書の範囲外（全 {total} 行）")]
    LineOutOfRange { line: usize, total: usize },
    #[error("行 {line} 桁 {column} は行の長さ {length} を超えている")]
    ColumnOutOfRange {
        line: usize,
        column: usize,
        length: usize,
    },
    #[error("行 {line} 桁 {column} は文字の途中を指している")]
    NotCharBoundary { line: usize, column: usize },
    #[error("範囲の終わり {end_line}:{end_column} が始まり {start_line}:{start_column} より前")]
    InvertedRange {
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
    },
    #[error("文書の版が違う（指定 {expected} / 現在 {actual}）")]
    VersionMismatch { expected: u64, actual: u64 },
}

/// 1 ファイル分の編集バッファ。カーソルと選択端は常に UTF-8 バイト境界に置く。
#[derive(Debug, Clone)]
pub struct TextBuffer {
    path: PathBuf,
    text: String,
    /// 行頭のバイト位置（#1660）。`[0]` は常に 0、以後は `\n` の直後。
    ///
    /// 行・桁の換算（`line_byte_col` / `offset_for_line_byte_col` / `line_count`）を
    /// 文書の先頭から数え直さないための索引。10 万行 / 10 MB の文書では数え直しが
    /// 1 回数 ms かかり、GUI は 1 打鍵・1 フレームにこれを何度も呼ぶ（表示中の行ごと・
    /// 選択の往復・追従スクロール）。本文を書き換える 3 つの口（`apply_edit` /
    /// `undo` / `redo`）が [`Self::splice_line_starts`] で追従させる
    line_starts: Vec<usize>,
    baseline: Vec<u8>,
    cursor: usize,
    anchor: Option<usize>,
    /// 新しく足す改行に使うコード（#1650）。既存行の改行は書き換えない
    line_ending: LineEnding,
    /// 差分の履歴（#1651）。古いものから捨てるので `VecDeque`
    undo_stack: VecDeque<EditDelta>,
    redo_stack: Vec<EditDelta>,
    /// 末尾の塊へまだ足してよいか（#1651）。
    /// カーソル移動・保存・undo / redo で閉じる = そこが undo の切れ目になる
    group_open: bool,
    /// まとめ判定の時計を手で進めるための仮想時刻（#1651。テスト専用）。
    ///
    /// `None` なら実時間。**効果を実時間で比べるテストは負荷で反転する**
    /// （`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）ので、
    /// 「時間が空くと塊が切れる」は時計を注入して状態値で固定する
    manual_millis: Option<u64>,
    /// 文書の版（#1658）。**本文が変わるたびに 1 つ増える**単調増加のカウンタ。
    ///
    /// 外（CLI / MCP）から範囲編集を送るとき、送り手が読んだ版と食い違っていれば
    /// 拒否できる（楽観ロック）。GUI の打鍵も dispatch の編集も同じ書き換え口を
    /// 通るので、**どちらで変わっても進む**。LSP の `didChange` が要求する
    /// 「文書の版」もこれをそのまま使える（#1007 の前払い）
    version: u64,
    /// 上下移動で狙い続ける桁（行頭からの**表示幅**。desired column。#1652 / #1742）。
    ///
    /// ↓ を続けて押す途中で短い行を通っても、その先の長い行では元の桁へ戻る。
    /// 文字数ではなく表示幅（全角 = 2・タブ = タブストップ）で持つので、全角やタブが
    /// 混ざった行をまたいでも**見た目の桁**が保たれる（#1742）。
    /// **上下以外でカーソルが動いたら捨てる**（`set_cursor` / `set_selection` が
    /// 実際に動いたとき・編集・undo / redo・全選択）。読んで置き直すのは上下の移動だけ
    goal_column: Option<usize>,
    /// インデント 1 段の単位（#1654）。**開いたときと全文を差し替えたときに**本文から推定する。
    ///
    /// 打つたびに推定し直すと、小さなファイルでは Tab で行を深くしただけで推定が変わる
    /// （4 桁の 2 行を 8 桁にすると隣り合う行の差が 8 になり、次の ⇧Tab が 8 桁外す =
    /// visual-test で実測）。VS Code も推定はモデルを作ったときに 1 回だけ
    indent: IndentUnit,
    /// 器（GUI の表示域）に同時に見える行数（#1652。ページ移動の歩幅に使う）。
    ///
    /// 本文ではなく「どれだけ見えているか」なので GUI が測って渡す
    /// （Zed の `Editor::visible_line_count` と同じ置き方）。`None` は未計測
    viewport_lines: Option<usize>,
    /// 大文字小文字を区別しない検索のための小文字写し（#1653）
    fold: FoldCache,
}

/// 大文字小文字を区別しない検索のための小文字写しの置き場（#1653）。
///
/// 写しを作るのは**区別しない検索をしたときだけ**で、本文が変わるまで使い回す。
/// 修正前は検索欄の 1 打鍵ごとに全文の写しを 2 回（`find_all` と `find_next`）作り直していた
/// （release 実測: 1 MB で 1 打鍵 4.4ms）。捨てるのは版が進むとき
/// （[`TextBuffer::bump_version`]。本文が変わる経路はすべてそこを通る）と
/// [`TextBuffer::release_search_cache`] を呼んだとき
#[derive(Default)]
struct FoldCache {
    lowered: OnceLock<Lowered>,
    /// 写しを作った回数（#1653）。「1 打鍵ごとに作り直さない」を時間ではなく回数で固定する
    builds: AtomicUsize,
}

impl Clone for FoldCache {
    /// 写しは本文から作り直せる控えなので複製しない（1 MB の本文なら 1 MB を余計に抱える）
    fn clone(&self) -> Self {
        Self {
            lowered: OnceLock::new(),
            builds: AtomicUsize::new(self.builds.load(Ordering::Relaxed)),
        }
    }
}

impl std::fmt::Debug for FoldCache {
    /// 写しの中身は出さない（本文と同じ大きさがある）
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoldCache")
            .field("built", &self.lowered.get().is_some())
            .field("builds", &self.builds.load(Ordering::Relaxed))
            .finish()
    }
}

impl TextBuffer {
    pub fn open(path: &Path) -> Result<Self, TextEditError> {
        let bytes = std::fs::read(path).map_err(TextEditError::Read)?;
        let text = String::from_utf8(bytes.clone()).map_err(|_| TextEditError::InvalidUtf8)?;
        Ok(Self {
            line_ending: LineEnding::detect_or_default(&text),
            indent: IndentUnit::detect(&text).unwrap_or_else(|| IndentUnit::for_path(path)),
            path: path.to_path_buf(),
            line_starts: compute_line_starts(&text),
            text,
            baseline: bytes,
            cursor: 0,
            anchor: None,
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            group_open: false,
            manual_millis: None,
            version: 0,
            goal_column: None,
            viewport_lines: None,
            fold: FoldCache::default(),
        })
    }

    pub fn from_text(path: PathBuf, text: String) -> Self {
        let baseline = text.as_bytes().to_vec();
        Self {
            line_ending: LineEnding::detect_or_default(&text),
            indent: IndentUnit::detect(&text).unwrap_or_else(|| IndentUnit::for_path(&path)),
            path,
            line_starts: compute_line_starts(&text),
            text,
            baseline,
            cursor: 0,
            anchor: None,
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            group_open: false,
            manual_millis: None,
            version: 0,
            goal_column: None,
            viewport_lines: None,
            fold: FoldCache::default(),
        }
    }

    /// このバッファが新しい改行に使うコード（#1650）
    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        selection_range(self.anchor, self.cursor)
    }

    pub fn dirty(&self) -> bool {
        self.text.as_bytes() != self.baseline
    }

    /// 本文を丸ごと差し替える（外部変更の取り込み・dispatch からの適用）。
    ///
    /// 渡された本文の改行コードを取り直す（#1650）。呼び出し側は「保存されるべき
    /// 全文」を渡しているので、そこに書かれている流儀がこのファイルの流儀になる。
    /// 改行を 1 つも含まない本文では直前の流儀を保つ
    pub fn set_text(&mut self, text: String) {
        let line_ending = LineEnding::detect(&text).unwrap_or(self.line_ending);
        // インデントも同じ考え方で取り直す（手掛かりが無ければ直前の単位のまま。#1654）
        if let Some(indent) = IndentUnit::detect(&text) {
            self.indent = indent;
        }
        self.apply_edit(Edit {
            range: 0..self.text.len(),
            replacement: &text,
            cursor: text.len(),
            anchor: None,
            kind: EditKind::Replace,
            line_ending: Some(line_ending),
        });
    }

    pub fn set_cursor(&mut self, offset: usize, extend_selection: bool) {
        let offset = snap_cursor(&self.text, offset.min(self.text.len()));
        let anchor = if extend_selection {
            Some(self.anchor.unwrap_or(self.cursor))
        } else {
            None
        };
        // カーソルが**実際に動いたら**打鍵の連なりは切れる（#1651）。
        //
        // 動いていない呼び出しで切らないのが大事: GUI の打鍵経路は 1 打鍵ごとに
        // 「画面の選択をバッファへ写す」ため `set_cursor` を 2 回通る
        // （`sync_editor_selection_from_preview`。書き戻しは同じ位置なので素で切ると
        // **GUI だけ 1 文字粒度のまま**になる）。キャレットだけの `anchor` の有無は
        // 選択範囲として同じものなので、見るのは位置と選択範囲
        if offset != self.cursor || selection_range(anchor, offset) != self.selection() {
            self.seal_undo_group();
            // 上下以外で動いたら、狙っていた桁も捨てる（#1652。上下の移動は
            // `move_cursor` がこのあと置き直す）
            self.goal_column = None;
        }
        self.anchor = anchor;
        self.cursor = offset;
    }

    /// 選択の起点と先端を**まとめて**置く（#1652）。`anchor == head` なら選択なし。
    ///
    /// `set_cursor(anchor, false)` → `set_cursor(head, true)` の 2 段で置くと、途中で
    /// カーソルが一度 `anchor` へ飛ぶので「動いた」と数えられ、undo の塊と
    /// 上下移動の桁の記憶が**選択があるときだけ毎回切れる**。GUI は 1 打鍵ごとに
    /// 画面の選択をバッファへ写す（`sync_editor_selection_from_preview`）ので、
    /// 2 段のままだと ⇧↓ を続けたときだけ桁が短い行に吸われる。ここは最終形だけを比べる
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        let anchor = snap_cursor(&self.text, anchor.min(self.text.len()));
        let head = snap_cursor(&self.text, head.min(self.text.len()));
        let anchor = (anchor != head).then_some(anchor);
        if head != self.cursor || selection_range(anchor, head) != self.selection() {
            self.seal_undo_group();
            self.goal_column = None;
        }
        self.anchor = anchor;
        self.cursor = head;
    }

    pub fn select_all(&mut self) {
        self.seal_undo_group();
        self.goal_column = None;
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// 器に同時に見える行数を渡す（#1652。ページ移動の歩幅になる）。**本文は変えない**
    pub fn set_viewport_lines(&mut self, lines: usize) {
        self.viewport_lines = Some(lines.max(1));
    }

    /// ページ移動 1 回で進む行数（#1652）。
    ///
    /// 見えている行数 − 1。前の画面のいちばん端の 1 行が次の画面にも残るので、
    /// どこから続きを読めばよいか見失わない。未計測なら [`DEFAULT_PAGE_STEP`]
    fn page_step(&self) -> usize {
        self.viewport_lines
            .map(|lines| lines.saturating_sub(1).max(1))
            .unwrap_or(DEFAULT_PAGE_STEP)
    }

    /// 本文を差し込む。**入ってくる改行はこのバッファの流儀へ揃える**（#1650）。
    ///
    /// 打鍵・IME・貼り付け・dispatch（CLI / MCP）の挿入がすべてここを通るので、
    /// 揃えるのをここ 1 か所にしておけば、どの経路から入れても混在改行にならない
    pub fn insert(&mut self, text: &str) {
        let text = normalize_line_endings(text, self.line_ending);
        // 選択があれば「選択の差し替え」= 1 回の操作（まとめない。#1651）
        let (range, kind) = match self.selection() {
            Some(range) => (range, EditKind::Replace),
            None => (self.cursor..self.cursor, EditKind::Insert),
        };
        let cursor = range.start + text.len();
        self.apply_edit(Edit {
            range,
            replacement: &text,
            cursor,
            anchor: None,
            kind,
            line_ending: None,
        });
    }

    /// Enter。挿す文字はこのファイルの改行コード（#1650）
    pub fn newline(&mut self) {
        self.insert(self.line_ending.as_str());
    }

    pub fn delete_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            self.anchor = None;
            return;
        }
        // `\r\n` は 1 つの行区切り。片方だけ消すと裸の CR が残る（#1650）
        let previous = if self.text[..self.cursor].ends_with("\r\n") {
            self.cursor - 2
        } else {
            self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        self.apply_edit(Edit {
            range: previous..self.cursor,
            replacement: "",
            cursor: previous,
            anchor: None,
            kind: EditKind::DeleteBackward,
            line_ending: None,
        });
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == self.text.len() {
            self.anchor = None;
            return;
        }
        // `\r\n` は 1 つの行区切り。片方だけ消すと裸の CR が残る（#1650）
        let next = if self.text[self.cursor..].starts_with("\r\n") {
            self.cursor + 2
        } else {
            self.cursor
                + self.text[self.cursor..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(0)
        };
        self.apply_edit(Edit {
            range: self.cursor..next,
            replacement: "",
            cursor: self.cursor,
            anchor: None,
            kind: EditKind::DeleteForward,
            line_ending: None,
        });
    }

    /// 単位を指定して消す（#1652）。GUI の打鍵と CLI / MCP の `delete` が同じ口を通る。
    ///
    /// 選択があれば単位によらず選択を消す。語・行単位の削除は**行の境目で止まる**:
    /// 行頭で「前へ」消すと改行だけが消えて前の行とつながり、行末で「後ろへ」
    /// 消すと改行だけが消える（`\r\n` は 1 つの行区切り。#1650）。
    /// 語の区切りは [`word_left_offset`] / [`word_right_offset`] の 1 実装で、
    /// ⌥←→ の移動と同じ境界を使う
    pub fn delete(&mut self, motion: DeleteMotion) {
        match motion {
            DeleteMotion::CharBackward => return self.delete_backward(),
            DeleteMotion::CharForward => return self.delete_forward(),
            _ => {}
        }
        if self.delete_selection() {
            return;
        }
        let cursor = self.cursor;
        let before = line_break_len_before(&self.text, cursor);
        let after = line_break_len_after(&self.text, cursor);
        let range = match motion {
            DeleteMotion::WordBackward if before > 0 => cursor - before..cursor,
            DeleteMotion::WordBackward => word_left_offset(&self.text, cursor)..cursor,
            DeleteMotion::WordForward if after > 0 => cursor..cursor + after,
            DeleteMotion::WordForward => cursor..word_right_offset(&self.text, cursor),
            DeleteMotion::ToLineStart if before > 0 => cursor - before..cursor,
            DeleteMotion::ToLineStart => self.line_start(cursor)..cursor,
            DeleteMotion::ToLineEnd if after > 0 => cursor..cursor + after,
            DeleteMotion::ToLineEnd => cursor..self.line_end(cursor),
            DeleteMotion::CharBackward | DeleteMotion::CharForward => return,
        };
        // 文書の端（先頭で前へ・末尾で後ろへ）は消すものが無い = 本文も版も変えない
        if range.is_empty() {
            return;
        }
        let start = range.start;
        self.apply_edit(Edit {
            range,
            replacement: "",
            cursor: start,
            anchor: None,
            kind: EditKind::DeleteSpan,
            line_ending: None,
        });
    }

    // --- インデント（#1654） -------------------------------------------------

    /// このバッファのインデント 1 段（#1654）。開いたとき（と全文を差し替えたとき）に
    /// 本文から推定した単位で、手掛かりが無ければファイル名で決めた既定
    pub fn indent_unit(&self) -> IndentUnit {
        self.indent
    }

    /// Tab（#1654）。選択が無ければカーソル位置へ 1 段ぶん挿し（スペースなら次のタブ位置まで）、
    /// 選択があれば選択が触れる行をまとめて 1 段深くする（undo 1 回で戻る）
    pub fn indent(&mut self) {
        if let Some(range) = self.selection() {
            self.shift_lines(range, true);
            return;
        }
        let unit = self.indent_unit();
        let text = match unit {
            IndentUnit::Tab => unit.text(),
            IndentUnit::Spaces(n) => {
                let column = self.visual_column(self.cursor, n);
                " ".repeat(n - column % n)
            }
        };
        self.insert(&text);
    }

    /// ⇧Tab（#1654）。選択が触れる行（選択が無ければカーソルの行）を 1 段浅くする。
    /// 浅くできる行が無ければ本文も版も変えない
    pub fn outdent(&mut self) {
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        self.shift_lines(range, false);
    }

    /// Enter（#1654）。改行を入れ、**カーソルより前にあるこの行のインデントを引き継ぐ**。
    ///
    /// - 行が開き括弧（Python / YAML は `:` も）で終わっていたら 1 段深くする
    ///   （規則は拡張子で決まる。Markdown とプレーンテキストは継承だけ = [`IndentRules`]）
    /// - `{|}` のように対応する閉じ括弧の手前で押したら、閉じ括弧を次の行へ送って
    ///   あいだに 1 段深い空行を作る
    /// - 空白だけの行で押したら、その行の空白は残さない（行末の空白を作らない）
    ///
    /// 挿す改行はこのファイルの改行コード（#1650）。選択があれば選択を差し替える。
    /// **括弧の自動閉じはしない**（理由は `.agent/requirements.md` FR-3.5 の #1654）
    pub fn newline_and_indent(&mut self) {
        let eol = self.line_ending.as_str();
        let range = self.selection().unwrap_or(self.cursor..self.cursor);
        let line_start = self.line_start(range.start);
        let before = &self.text[line_start..range.start];
        let indent = &before[..before.len() - before.trim_start_matches([' ', '\t']).len()];
        let line_end = self.line_end(range.end);
        let after = &self.text[range.end..line_end];
        let rules = IndentRules::for_path(&self.path);
        let last = before.trim_end_matches([' ', '\t']).chars().last();
        let deeper = match last {
            Some(c) if rules.brackets && closing_bracket(c).is_some() => true,
            Some(':') => rules.colon,
            _ => false,
        };
        let unit = if deeper {
            self.indent_unit().text()
        } else {
            String::new()
        };
        let mut start = range.start;
        let mut end = range.end;
        let mut replacement = format!("{eol}{indent}{unit}");
        let cursor_in_replacement = replacement.len();
        let after_trimmed = after.trim_start_matches([' ', '\t']);
        let closes = last
            .and_then(closing_bracket)
            .is_some_and(|close| deeper && after_trimmed.starts_with(close));
        if closes {
            // 閉じ括弧は元のインデントの行へ（あいだの空白は捨てる）
            end += after.len() - after_trimmed.len();
            replacement.push_str(eol);
            replacement.push_str(indent);
        } else if indent.len() == before.len() && !before.is_empty() && after.is_empty() {
            // 空白だけの行: この行の空白を消して、新しい行へ同じインデントを持っていく
            start = line_start;
        }
        let cursor = start + cursor_in_replacement;
        let kind = if start == end {
            EditKind::Insert
        } else {
            EditKind::Replace
        };
        self.apply_edit(Edit {
            range: start..end,
            replacement: &replacement,
            cursor,
            anchor: None,
            kind,
            line_ending: None,
        });
    }

    /// 行頭からその位置までの見た目の桁（タブは次の `tab` の倍数まで進む。#1654）
    fn visual_column(&self, offset: usize, tab: usize) -> usize {
        self.text[self.line_start(offset)..offset]
            .chars()
            .fold(0, |col, c| {
                if c == '\t' {
                    col + tab - col % tab
                } else {
                    col + 1
                }
            })
    }

    /// `range` が触れる行をまとめて 1 段深く / 浅くする（#1654）。**1 回の編集**なので undo 1 回で戻る。
    ///
    /// 選択の終わりが行頭（桁 0）にあるとき、その行は選んでいないものとして扱う
    /// （行を丸ごと選ぶと終わりが次の行頭に来る。VS Code / Sublime と同じ）。
    /// 深くするとき空行・空白だけの行は触らない（行末の空白を作らない）。
    /// カーソルと選択は同じ文字を指し続けるようにずらす
    fn shift_lines(&mut self, range: Range<usize>, deeper: bool) {
        let first = self.line_start(range.start);
        let mut last_point = range.end;
        if range.end > range.start && self.line_start(range.end) == range.end {
            last_point -= line_break_len_before(&self.text, range.end);
        }
        let block_end = self.line_end(last_point.max(first));
        let unit = self.indent_unit();
        let unit_text = unit.text();
        // (元の行頭, 行頭で足した / 引いたバイト数)
        let mut shifts: Vec<(usize, isize)> = Vec::new();
        let mut out = String::with_capacity(block_end - first + 64);
        let mut line_start = first;
        for piece in self.text[first..block_end].split_inclusive('\n') {
            let content = piece.trim_end_matches(['\r', '\n']);
            let tail = &piece[content.len()..];
            let delta: isize = if deeper {
                if content.trim().is_empty() {
                    0
                } else {
                    out.push_str(&unit_text);
                    unit_text.len() as isize
                }
            } else {
                -(outdent_width(content, unit) as isize)
            };
            let kept = if delta < 0 {
                &content[delta.unsigned_abs()..]
            } else {
                content
            };
            out.push_str(kept);
            out.push_str(tail);
            shifts.push((line_start, delta));
            line_start += piece.len();
        }
        if shifts.iter().all(|(_, d)| *d == 0) {
            return;
        }
        // 点のずらし方: 前の行ぶんの増減は全部足し、自分の行のぶんは位置しだい
        // （深くするとき行頭ちょうどの点は動かさない = 選択の起点が行頭なら足した
        // インデントも選択に入る。浅くするとき消した空白の中の点は行頭へ寄せる）。
        // ブロックより後ろの点（行頭で終わる選択の終わり）は全行ぶんずれる
        let shift_point = |p: usize| -> usize {
            let idx = shifts.partition_point(|(start, _)| *start <= p);
            if idx == 0 {
                return p;
            }
            let earlier: isize = shifts[..idx - 1].iter().map(|(_, d)| *d).sum();
            let (start, delta) = shifts[idx - 1];
            let into = p - start;
            let own = if delta < 0 {
                -(into.min(delta.unsigned_abs()) as isize)
            } else if into > 0 {
                delta
            } else {
                0
            };
            (p as isize + earlier + own) as usize
        };
        let cursor = shift_point(self.cursor);
        let anchor = self.anchor.map(shift_point);
        self.apply_edit(Edit {
            range: first..block_end,
            replacement: &out,
            cursor,
            anchor,
            kind: EditKind::Replace,
            line_ending: None,
        });
    }

    // --- undo / redo ---

    /// まとめ判定に使う「今」（#1651）。テストは仮想時刻を差し込む
    fn now_millis(&self) -> u64 {
        self.manual_millis.unwrap_or_else(process_millis)
    }

    /// 版を 1 つ進める（#1658）。**本文が変わる経路はすべてここを通る**。
    ///
    /// 呼ぶのは本文を書き換える唯一の口 [`Self::apply_edit`] と undo / redo だけ。
    /// ここ以外で `self.version` を書き換えると「版が進まない編集」ができてしまい、
    /// 楽観ロック（`expected_version`）が黙って素通りする。番犬
    /// `issue1658_edit_range_watchdog` が代入の散らばりを file:line で名指す
    fn bump_version(&mut self) {
        // 飽和させる（1 秒 1000 編集でも 5 億年かかる桁だが、巻き戻さないことを型で示す）
        self.version = self.version.saturating_add(1);
        // 本文が変わったので、区別しない検索の小文字写しは古くなった（#1653）。
        // 本文が変わる経路はすべてここを通るので、捨てる場所もここ 1 つ
        self.fold.lowered.take();
    }

    /// 文書の版（#1658）。編集のたびに増える
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 末尾の塊を閉じる（#1651）。
    ///
    /// 次の編集はここへ足さず、新しい 1 件として積まれる = ここが undo の切れ目。
    /// 呼ぶのは**カーソルが動いたとき・保存したとき・undo / redo したとき**
    fn seal_undo_group(&mut self) {
        self.group_open = false;
    }

    /// 本文を書き換える**唯一の口**（#1651）。
    ///
    /// ここを通さずに `self.text` を書き換えると、その編集は履歴に載らない
    /// （undo が本文とずれる）。番犬 `issue1651_undo_delta_watchdog` が
    /// 直接の書き換えを file:line で名指す
    fn apply_edit(&mut self, edit: Edit<'_>) {
        let Edit {
            range,
            replacement,
            cursor,
            anchor,
            kind,
            line_ending,
        } = edit;
        let line_ending_before = self.line_ending;
        let line_ending_after = line_ending.unwrap_or(line_ending_before);
        let start = range.start;
        // 全文ではなく**置き換える範囲だけ**を写す（1 打鍵で 1 MB 積まない）
        let before = self.text[range.clone()].to_string();
        let cursor_before = self.cursor;
        let anchor_before = self.anchor;
        let at_millis = self.now_millis();
        self.splice_line_starts(range.clone(), replacement);
        self.text.replace_range(range, replacement);
        // 編集後の本文で丸める。丸めずに持つと、その位置が差分へ記録されて
        // **undo / redo のたびに再現される**（`replace_all` は編集前のカーソルを
        // 長さで切り詰めるだけなので、多バイト文字の途中を指しうる = 次の編集で panic）
        let cursor = snap_cursor(&self.text, cursor.min(self.text.len()));
        let delta = EditDelta {
            start,
            before,
            after: replacement.to_string(),
            cursor_before,
            anchor_before,
            cursor_after: cursor,
            anchor_after: anchor,
            line_ending_before,
            line_ending_after,
            kind,
            at_millis,
        };
        self.cursor = cursor;
        self.anchor = anchor;
        self.line_ending = line_ending_after;
        // 編集したら上下移動の桁の記憶は捨てる（#1652。打った後の ↓ は打った桁から）
        self.goal_column = None;
        // 本文が変わった = 文書の版が進む（#1658）。書き換えの口はここ 1 つ
        self.bump_version();
        self.record(delta);
    }

    /// 選択があればそれを消す（#1651）。消したら `true`
    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let cursor = range.start;
        self.apply_edit(Edit {
            range,
            replacement: "",
            cursor,
            anchor: None,
            kind: EditKind::Replace,
            line_ending: None,
        });
        true
    }

    /// 差分を履歴へ積む（#1651）。直前の塊へ足せるなら足す
    fn record(&mut self, delta: EditDelta) {
        // 新しい編集をしたら、やり直せる先は無くなる
        self.redo_stack.clear();
        if !self.merge_into_last(&delta) {
            self.undo_stack.push_back(delta);
        }
        self.group_open = true;
        self.enforce_budget();
    }

    /// 直前の塊へこの編集を足せるなら足す（#1651）。足したら `true`。
    ///
    /// まとまる条件は **①塊が開いている（カーソル移動・保存・undo を挟んでいない）
    /// ②同じ種類 ③位置が連なっている ④前回から [`COALESCE_WINDOW_MS`] 以内
    /// ⑤改行をまたがない**。どれか 1 つでも欠ければ新しい塊にする
    fn merge_into_last(&mut self, delta: &EditDelta) -> bool {
        if !self.group_open {
            return false;
        }
        let Some(last) = self.undo_stack.back_mut() else {
            return false;
        };
        if last.kind != delta.kind || delta.line_ending_after != last.line_ending_after {
            return false;
        }
        if delta.at_millis.saturating_sub(last.at_millis) > COALESCE_WINDOW_MS {
            return false;
        }
        // 改行はここで塊を切る（段落の区切りで undo が止まるほうが直せる）
        if contains_newline(&delta.before)
            || contains_newline(&delta.after)
            || contains_newline(&last.before)
            || contains_newline(&last.after)
        {
            return false;
        }
        match delta.kind {
            // 打鍵は直前に書いた文字の**すぐ後ろ**に続くときだけ伸ばす
            EditKind::Insert if delta.start == last.start + last.after.len() => {
                last.after.push_str(&delta.after);
            }
            // Backspace は左へ伸びる。消した文字は前へ継ぎ足す
            EditKind::DeleteBackward if delta.start + delta.before.len() == last.start => {
                let mut before = delta.before.clone();
                before.push_str(&last.before);
                last.before = before;
                last.start = delta.start;
            }
            // Delete は同じ位置から右へ伸びる。消した文字は後ろへ継ぎ足す
            EditKind::DeleteForward if delta.start == last.start => {
                last.before.push_str(&delta.before);
            }
            _ => return false,
        }
        last.cursor_after = delta.cursor_after;
        last.anchor_after = delta.anchor_after;
        last.at_millis = delta.at_millis;
        true
    }

    /// 履歴が抱えている本文のバイト数（#1651）。undo 側と redo 側の合計。
    ///
    /// undo すると差分は redo 側へ**移るだけ**なので、この合計は往復で増えない
    pub fn undo_history_bytes(&self) -> usize {
        self.undo_stack
            .iter()
            .chain(self.redo_stack.iter())
            .map(EditDelta::bytes)
            .sum()
    }

    /// undo できる回数（= 塊の数。#1651）。`hello` を 1 塊で打てば 1
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    /// 上限（操作数 / バイト数）を超えたぶんを古い側から捨てる（#1651）。
    ///
    /// **先に効いたほうが効く**。1 件だけは必ず残す（1 回の編集が単独で予算を
    /// 超えていても、直前の操作は取り消せるべき）
    fn enforce_budget(&mut self) {
        while self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.pop_front();
        }
        let mut total = self.undo_history_bytes();
        while total > UNDO_BYTE_LIMIT && self.undo_stack.len() > 1 {
            let Some(dropped) = self.undo_stack.pop_front() else {
                break;
            };
            total = total.saturating_sub(dropped.bytes());
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(delta) = self.undo_stack.pop_back() else {
            return false;
        };
        // 本文が変わるので版は**戻らずに進む**（#1658）。「元へ戻す」も 1 つの変更で、
        // 版を戻すと「別の中身なのに同じ版」が生まれて楽観ロックが効かなくなる
        self.bump_version();
        let end = delta.start + delta.after.len();
        self.splice_line_starts(delta.start..end, &delta.before);
        self.text.replace_range(delta.start..end, &delta.before);
        self.cursor = delta.cursor_before;
        self.anchor = delta.anchor_before;
        self.line_ending = delta.line_ending_before;
        self.redo_stack.push(delta);
        self.seal_undo_group();
        self.goal_column = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(delta) = self.redo_stack.pop() else {
            return false;
        };
        self.bump_version();
        let end = delta.start + delta.before.len();
        self.splice_line_starts(delta.start..end, &delta.after);
        self.text.replace_range(delta.start..end, &delta.after);
        self.cursor = delta.cursor_after;
        self.anchor = delta.anchor_after;
        self.line_ending = delta.line_ending_after;
        self.undo_stack.push_back(delta);
        self.seal_undo_group();
        self.goal_column = None;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    // --- 検索・置換 ---

    /// 条件 `options` に合う全ヒットを返す（#195 / #1653）。
    ///
    /// 返す位置は**元テキストのバイト位置**で、常に文字境界に載る。
    ///
    /// - **区別する**（既定）: 本文そのものを探す。小文字写しは作らない
    /// - **区別しない**: 小文字写し（[`Self::folded`]）を探し、見つかった位置を
    ///   [`Lowered::to_original`] で元テキストへ戻す。小文字化はバイト長を変えうる
    ///   （`İ` U+0130 は 2 → 3 バイト、`ẞ` U+1E9E は 3 → 2 バイト）ので、写しの位置を
    ///   そのまま元テキストの位置として使うとずれる（#1016）。本文全体のバイト長が
    ///   一致していても安全ではない（伸縮が打ち消しあって総和だけ一致し、途中は食い違う）。
    ///   戻せない位置（展開された文字の途中で始まる／終わる一致）はヒットにしない
    /// - **単語単位**: 一致の前後を元テキストで確かめる（[`at_word_boundary`]）。
    ///   外れた候補は 1 文字だけ進めて探し直すので、`xfoo foo` の 2 つ目を取りこぼさない
    pub fn find_all(&self, query: &str, options: SearchOptions) -> Vec<SearchHit> {
        if query.is_empty() {
            return Vec::new();
        }
        let accept = |start: usize, end: usize| {
            !options.whole_word || at_word_boundary(&self.text, start, end)
        };
        let mut hits = Vec::new();
        if options.case_sensitive {
            let mut cursor = 0;
            while let Some(pos) = self.text[cursor..].find(query) {
                let start = cursor + pos;
                let end = start + query.len();
                if accept(start, end) {
                    hits.push(SearchHit { start, end });
                    cursor = end;
                } else {
                    cursor = start + char_len_at(&self.text, start);
                }
            }
            return hits;
        }
        let lower_query = lowercase_per_char(query);
        let lowered = self.folded();
        let mut cursor = 0;
        while let Some(pos) = lowered.text[cursor..].find(&lower_query) {
            let lower_start = cursor + pos;
            let lower_end = lower_start + lower_query.len();
            match (
                lowered.to_original(lower_start),
                lowered.to_original(lower_end),
            ) {
                (Some(start), Some(end)) if accept(start, end) => {
                    hits.push(SearchHit { start, end });
                    cursor = lower_end;
                }
                // 元テキストに対応する位置が無い一致は返せない（返すと slice が文字境界を
                // 割って panic する）。単語の切れ目に載らない一致も返さない。
                // どちらも次の文字境界から探し直す
                _ => cursor = lower_start + char_len_at(&lowered.text, lower_start),
            }
        }
        hits
    }

    /// 区別しない検索に使う小文字写し（#1653）。**作るのはここだけ**。
    ///
    /// 本文が変わるまで使い回すので、検索欄の打鍵・次へ / 前へでは作り直さない
    fn folded(&self) -> &Lowered {
        self.fold.lowered.get_or_init(|| {
            self.fold.builds.fetch_add(1, Ordering::Relaxed);
            Lowered::build(&self.text)
        })
    }

    /// 区別しない検索の小文字写しを作った回数（#1653。計測用）。
    ///
    /// 「検索欄の 1 打鍵ごとに全文を写し直さない」を時間ではなく回数で確かめる口
    /// （`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）
    pub fn search_fold_builds(&self) -> usize {
        self.fold.builds.load(Ordering::Relaxed)
    }

    /// 小文字写しを手放す（#1653）。検索欄を閉じたときに呼ぶ（本文と同じ大きさがある）
    pub fn release_search_cache(&mut self) {
        self.fold.lowered.take();
    }

    /// 指定範囲を置換文字列で置き換える（1 件置換）。
    /// 置換文字列の改行もこのバッファの流儀へ揃える（#1650）
    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        let replacement = normalize_line_endings(replacement, self.line_ending);
        let cursor = range.start + replacement.len();
        self.apply_edit(Edit {
            range,
            replacement: &replacement,
            cursor,
            anchor: None,
            kind: EditKind::Replace,
            line_ending: None,
        });
    }

    /// カーソル以降で最初のヒット（なければ先頭へラップ）を 1 件置換する（#195 / #1653）。
    /// 戻り値は置換件数（0 か 1）
    pub fn replace_next(
        &mut self,
        query: &str,
        replacement: &str,
        options: SearchOptions,
    ) -> usize {
        let hits = self.find_all(query, options);
        let Some(hit) = next_hit(&hits, self.cursor).cloned() else {
            return 0;
        };
        self.replace_range(hit.start..hit.end, replacement);
        1
    }

    /// 全置換。戻り値は置換件数
    pub fn replace_all(&mut self, query: &str, replacement: &str, options: SearchOptions) -> usize {
        let hits = self.find_all(query, options);
        if hits.is_empty() {
            return 0;
        }
        let replacement = normalize_line_endings(replacement, self.line_ending);
        // 差分 1 件で表す（undo 1 回で全件戻る）。**触る範囲は最初のヒットから
        // 最後のヒットまで**に留めるので、全文を 2 本持つことにはならない（#1651）
        let start = hits[0].start;
        let end = hits[hits.len() - 1].end;
        let mut after = String::new();
        let mut prev = start;
        for hit in &hits {
            after.push_str(&self.text[prev..hit.start]);
            after.push_str(&replacement);
            prev = hit.end;
        }
        let count = hits.len();
        let new_len = self.text.len() - (end - start) + after.len();
        let cursor = self.cursor.min(new_len);
        self.apply_edit(Edit {
            range: start..end,
            replacement: &after,
            cursor,
            anchor: None,
            kind: EditKind::Replace,
            line_ending: None,
        });
        count
    }

    pub fn move_cursor(&mut self, movement: CursorMovement, extend_selection: bool) {
        // 選択中の素の ← → は 1 文字動かずに選択の端へ畳む（#1742）
        if let Some(edge) = self.collapse_edge(movement, extend_selection) {
            self.set_cursor(edge, false);
            return;
        }
        // 上下の移動は「狙う桁」を持ち越す（#1652 の desired column）。
        // 短い行で行末へ寄せられても、記憶しているのは元の桁のまま
        let goal = movement.is_vertical().then(|| {
            self.goal_column
                .unwrap_or_else(|| self.display_column(self.cursor))
        });
        let goal_col = goal.unwrap_or(0);
        let page = self.page_step() as isize;
        let target = match movement {
            // `\r\n` は 1 つの行区切りなので、あいだで止まらずにまたぐ（#1650）
            CursorMovement::Left if self.text[..self.cursor].ends_with("\r\n") => self.cursor - 2,
            CursorMovement::Left => self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0),
            CursorMovement::Right if self.text[self.cursor..].starts_with("\r\n") => {
                self.cursor + 2
            }
            CursorMovement::Right => {
                self.cursor
                    + self.text[self.cursor..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(0)
            }
            CursorMovement::Up => self.vertical_target(-1, goal_col),
            CursorMovement::Down => self.vertical_target(1, goal_col),
            CursorMovement::PageUp => self.vertical_target(-page, goal_col),
            CursorMovement::PageDown => self.vertical_target(page, goal_col),
            CursorMovement::WordLeft => word_left_offset(&self.text, self.cursor),
            CursorMovement::WordRight => word_right_offset(&self.text, self.cursor),
            CursorMovement::LineStart => self.line_start(self.cursor),
            CursorMovement::SmartLineStart => self.smart_line_start(self.cursor),
            CursorMovement::LineEnd => self.line_end(self.cursor),
            CursorMovement::DocumentStart => 0,
            CursorMovement::DocumentEnd => self.text.len(),
        };
        self.set_cursor(target, extend_selection);
        // `set_cursor` は動いたら記憶を捨てるので、そのあとで置き直す
        // （上下以外の移動では `None` = 捨てたまま）
        self.goal_column = goal;
    }

    /// 0 起点の行と、その行内 UTF-8 バイト位置を返す。
    pub fn line_byte_col(&self, offset: usize) -> (usize, usize) {
        let offset = snap_boundary(&self.text, offset.min(self.text.len()));
        // 行頭索引の二分探索（#1660）。`[0] == 0` なので添字は必ず 1 以上になる
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        (line, offset - self.line_starts[line])
    }

    /// 行番号 + 行内バイト位置を文書全体の UTF-8 バイト位置へ変換する。
    pub fn offset_for_line_byte_col(&self, line: usize, byte_col: usize) -> usize {
        let start = self.line_start_of(line).unwrap_or(self.text.len());
        let end = line_end_offset(&self.text, start);
        snap_cursor(&self.text, (start + byte_col).min(end))
    }

    // --- 行・桁で指す編集 API（#1658） ---------------------------------------

    /// 文書の行数（#1658）。末尾が改行で終わるファイルは、その後ろの**空行も 1 行**と
    /// 数える（`"a\n"` は 2 行）。カーソルはそこへ置けるので、行として在る
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// 行頭のバイト位置の一覧（#1660）。`[行番号]` がその行の頭。
    /// 描画が表示行ごとの頭を引くのに使う（本文を数え直して作らない）
    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    /// その行（0 始まり）の頭のバイト位置（#1660）。文書の範囲外なら `None`
    fn line_start_of(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    /// 本文の `range` を `replacement` へ置き換えるのに合わせて行頭索引を直す（#1660）。
    ///
    /// **`self.text` を書き換える直前に呼ぶ**（位置はどちらも置き換え前の本文で数える）。
    /// 消える範囲の中にあった行頭を外し、`replacement` の改行ぶんを足して、
    /// 後ろの行頭を長さの差だけずらす。ずらすのは O(後ろの行数) の足し算だけで、
    /// 本文を読み直さない
    fn splice_line_starts(&mut self, range: Range<usize>, replacement: &str) {
        let first = self
            .line_starts
            .partition_point(|&start| start <= range.start);
        let last = self
            .line_starts
            .partition_point(|&start| start <= range.end);
        let inserted: Vec<usize> = replacement
            .match_indices('\n')
            .map(|(i, _)| range.start + i + 1)
            .collect();
        let added = inserted.len();
        self.line_starts.splice(first..last, inserted);
        let removed = range.end - range.start;
        let added_bytes = replacement.len();
        for start in &mut self.line_starts[first + added..] {
            *start = *start + added_bytes - removed;
        }
    }

    /// その行の長さ（バイト。**改行コードは含まない**。#1658）。
    ///
    /// CRLF の行では CR も含めない = 桁の上限が CR の手前になるので、
    /// 「CR と LF のあいだ」を指す桁は範囲外として弾かれる（#1650 の契約）
    fn line_span(&self, line: usize) -> Option<Range<usize>> {
        let start = self.line_start_of(line)?;
        Some(start..line_end_offset(&self.text, start))
    }

    /// 行・桁を文書全体のバイト位置へ**丸めずに**解く（#1658）。
    ///
    /// 解けない指定（範囲外の行・行より長い桁・文字の途中）は
    /// [`RangeEditError`] で返す。`min` や `snap_cursor` で黙って寄せない
    pub fn resolve_position(&self, position: TextPosition) -> Result<usize, RangeEditError> {
        if position.line == 0 {
            return Err(RangeEditError::ZeroLine);
        }
        let total = self.line_count();
        let Some(span) = self.line_span(position.line - 1) else {
            return Err(RangeEditError::LineOutOfRange {
                line: position.line,
                total,
            });
        };
        let length = span.end - span.start;
        if position.column > length {
            return Err(RangeEditError::ColumnOutOfRange {
                line: position.line,
                column: position.column,
                length,
            });
        }
        let offset = span.start + position.column;
        if !self.text.is_char_boundary(offset) {
            return Err(RangeEditError::NotCharBoundary {
                line: position.line,
                column: position.column,
            });
        }
        Ok(offset)
    }

    /// バイト位置を行・桁へ戻す（#1658）。[`Self::resolve_position`] の逆写像
    pub fn position_of(&self, offset: usize) -> TextPosition {
        let (line, column) = self.line_byte_col(offset);
        TextPosition::new(line + 1, column)
    }

    /// いまのカーソル位置（行・桁。#1658）
    pub fn cursor_position(&self) -> TextPosition {
        self.position_of(self.cursor)
    }

    /// いまの選択範囲（行・桁。#1658）。選択が無ければ `None`
    pub fn selection_positions(&self) -> Option<(TextPosition, TextPosition)> {
        let range = self.selection()?;
        Some((self.position_of(range.start), self.position_of(range.end)))
    }

    /// 外（CLI / MCP）から読む文書の状態（#1658）。**この 1 実装が応答の正本**。
    ///
    /// `version` は編集のたびに増える版、`cursor` / `selection` は行・桁
    /// （行 1 始まり / 桁 0 始まりの UTF-8 バイト）、`undo_depth` /
    /// `undo_history_bytes` は undo 履歴の状態。選択が無ければ `selection` は null
    pub fn document_state(&self) -> serde_json::Value {
        let position = |p: TextPosition| serde_json::json!({ "line": p.line, "column": p.column });
        serde_json::json!({
            "version": self.version,
            "line_count": self.line_count(),
            "bytes": self.text.len(),
            "line_ending": self.line_ending.label(),
            "indent": self.indent_unit().label(),
            "cursor": position(self.cursor_position()),
            "selection": self.selection_positions().map(|(start, end)| {
                serde_json::json!({ "start": position(start), "end": position(end) })
            }),
            "undo_depth": self.undo_depth(),
            "undo_history_bytes": self.undo_history_bytes(),
        })
    }

    /// 版が指定と食い違っていれば拒否する（#1658）
    fn check_version(&self, expected: Option<u64>) -> Result<(), RangeEditError> {
        match expected {
            Some(expected) if expected != self.version => Err(RangeEditError::VersionMismatch {
                expected,
                actual: self.version,
            }),
            _ => Ok(()),
        }
    }

    /// 行・桁で指定した範囲を置き換える（#1658）。
    ///
    /// 成功すると**カーソルは入れた本文の末尾**に来て、undo 1 回で丸ごと戻る
    /// （`replace_range` = 1 操作）。入れる本文の改行はバッファの流儀へ揃い、
    /// 範囲の外の改行は 1 バイトも変わらない（#1650 の契約）。
    ///
    /// 解けない指定・版違いのときは**本文を触らずに**エラーを返す
    /// （検査をすべて先に済ませてから 1 回だけ書き換える）
    pub fn replace_position_range(&mut self, edit: &RangeEdit) -> Result<(), RangeEditError> {
        self.check_version(edit.expected_version)?;
        let start = self.resolve_position(edit.start)?;
        let end = self.resolve_position(edit.end)?;
        if end < start {
            return Err(RangeEditError::InvertedRange {
                start_line: edit.start.line,
                start_column: edit.start.column,
                end_line: edit.end.line,
                end_column: edit.end.column,
            });
        }
        self.replace_range(start..end, &edit.text);
        Ok(())
    }

    /// カーソルと選択を行・桁で置く（#1658）。**本文は変えない**ので版も進まない
    pub fn set_cursor_placement(&mut self, place: &CursorPlacement) -> Result<(), RangeEditError> {
        self.check_version(place.expected_version)?;
        let anchor = self.resolve_position(place.cursor)?;
        let head = match place.select_to {
            Some(select_to) => self.resolve_position(select_to)?,
            None => anchor,
        };
        // 選択の起点を先に置いてから伸ばす（`set_cursor` は現在位置を anchor にする）
        self.set_cursor(anchor, false);
        if head != anchor {
            self.set_cursor(head, true);
        }
        Ok(())
    }

    /// 保存する。開いた後にファイルが外で変わっていれば**書かずに**
    /// [`TextEditError::ExternalChanged`]（消されていれば [`TextEditError::ExternalDeleted`]）を返す。
    ///
    /// ディスクの中身が既に編集中の本文と同じなら、書かずにそれを新しい基準にする
    /// （どちらを採っても同じ = 競合ではない。#1659）
    pub fn save(&mut self) -> Result<(), TextEditError> {
        self.save_inner(false)
    }

    /// 外部変更があっても**自分の変更で上書き**する（#1659。`tako edit save --force`）。
    ///
    /// 外で消されたファイルは作り直す（権限は新規ファイルの既定）。読み取り専用の
    /// ファイルは上書きしない（外部変更の検知を外すだけで、書けないものは書かない）
    pub fn save_overwrite(&mut self) -> Result<(), TextEditError> {
        self.save_inner(true)
    }

    fn save_inner(&mut self, overwrite: bool) -> Result<(), TextEditError> {
        match std::fs::metadata(&self.path) {
            Ok(metadata) if metadata.permissions().readonly() => {
                return Err(TextEditError::Write(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "読み取り専用ファイル",
                )));
            }
            Ok(_) => {
                let current = std::fs::read(&self.path).map_err(TextEditError::Read)?;
                if current != self.baseline {
                    if current == self.text.as_bytes() {
                        // 外で書かれた中身が自分の本文と同じ。書き直す必要も競合も無い
                        self.baseline = current;
                        self.seal_undo_group();
                        return Ok(());
                    }
                    if !overwrite {
                        return Err(TextEditError::ExternalChanged);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && overwrite => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(TextEditError::ExternalDeleted);
            }
            Err(e) => return Err(TextEditError::Read(e)),
        }
        write_file(&self.path, self.text.as_bytes()).map_err(TextEditError::Write)?;
        self.baseline = self.text.as_bytes().to_vec();
        // 保存したところが undo の切れ目（#1651）。
        // 「保存した姿」まで戻せる位置が履歴に残る
        self.seal_undo_group();
        Ok(())
    }

    /// ディスクの中身（消されていれば `None`）
    fn read_disk(&self) -> Result<Option<Vec<u8>>, TextEditError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(TextEditError::Read(e)),
        }
    }

    /// ディスクの今の中身と基準を突き合わせる（#1659）。
    ///
    /// ディスクの中身が編集中の本文と同じなら、**それを新しい基準として取り込み**
    /// `Unchanged` を返す（どちらを採っても同じ = 競合ではない）
    pub fn refresh_disk_state(&mut self) -> Result<DiskState, TextEditError> {
        let current = self.read_disk()?;
        Ok(self.observe_disk(current.as_deref()))
    }

    /// 読み済みのディスクの中身（消されていれば `None`）で [`Self::refresh_disk_state`] と
    /// 同じ判定をする（#1659）。ファイル監視は background で読んだバイト列を持っているので、
    /// UI スレッドで読み直さずに済む
    pub fn observe_disk(&mut self, current: Option<&[u8]>) -> DiskState {
        let Some(current) = current else {
            return DiskState::Deleted;
        };
        if current == self.baseline.as_slice() {
            return DiskState::Unchanged;
        }
        if current == self.text.as_bytes() {
            self.baseline = current.to_vec();
            return DiskState::Unchanged;
        }
        DiskState::Changed
    }

    /// ディスクから読み直す（#1659。`tako edit reload`）。**編集中の変更は捨てる**が、
    /// 読み直しは 1 回の編集として履歴に積むので **undo 1 回で自分の変更へ戻せる**。
    ///
    /// 書き換えるのは食い違っている範囲だけ（共通の先頭・末尾は触らない）なので、
    /// 10 MB のファイルでも差分が小さければ履歴は小さい。カーソルは同じ場所に残す
    /// （書き換えた範囲の中にいたらその範囲の中へ寄せる）。改行コードとインデントは
    /// 読み直した中身から取り直す（`set_text` と同じ考え方。#1650 / #1654）。
    ///
    /// 消されていれば [`TextEditError::ExternalDeleted`]、上限を超えた・バイナリになった
    /// なら [`TextEditError::NotEditable`] で、**どちらも本文を 1 バイトも触らない**
    pub fn reload_from_disk(&mut self) -> Result<(), TextEditError> {
        let bytes = self.read_disk()?.ok_or(TextEditError::ExternalDeleted)?;
        self.reload_from(bytes)
    }

    /// 読み済みのディスクの中身で [`Self::reload_from_disk`] と同じ読み直しをする（#1659。
    /// ファイル監視が background で読んだバイト列を使う口）
    pub fn reload_from(&mut self, bytes: Vec<u8>) -> Result<(), TextEditError> {
        let text = std::str::from_utf8(&bytes).map_err(|_| TextEditError::InvalidUtf8)?;
        if let Some(limit) = crate::preview_limit::Truncation::judge(
            bytes.len(),
            Some(bytes.len() as u64),
            text.lines().count(),
        ) {
            return Err(TextEditError::NotEditable(format!(
                "ディスクの中身が編集できる上限を超えるため読み直せない: {}",
                limit.detail_ja()
            )));
        }
        if text.contains('\0') {
            return Err(TextEditError::NotEditable(
                "バイナリファイルは編集できない".into(),
            ));
        }
        if text != self.text {
            let line_ending = LineEnding::detect(text).unwrap_or(self.line_ending);
            if let Some(indent) = IndentUnit::detect(text) {
                self.indent = indent;
            }
            let (range, replacement) = changed_span(&self.text, text);
            let cursor = follow_offset(self.cursor, &range, replacement.len());
            // 直前の打鍵とまとめない（読み直しは 1 回で丸ごと戻る単位）
            self.seal_undo_group();
            self.apply_edit(Edit {
                range,
                replacement,
                cursor,
                anchor: None,
                kind: EditKind::Replace,
                line_ending: Some(line_ending),
            });
            self.seal_undo_group();
        }
        self.baseline = bytes;
        Ok(())
    }

    /// ディスク上のファイルと編集中の本文の差分（#1659。`tako edit diff`）。
    ///
    /// 向きは「ディスク → 編集中」。消されていれば空の本文からの差分
    /// （全行が `+`）。ディスクが UTF-8 でなくなっていても落とさず、読める形に置き換えて比べる
    pub fn disk_diff(&self) -> Result<DiskDiff, TextEditError> {
        let current = self.read_disk()?;
        let state = match &current {
            None => DiskState::Deleted,
            Some(bytes) if *bytes == self.baseline => DiskState::Unchanged,
            Some(_) => DiskState::Changed,
        };
        let disk = current
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        Ok(DiskDiff {
            state,
            diff: crate::text_diff::diff_lines(&disk, &self.text),
        })
    }

    fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, offset: usize) -> usize {
        line_end_offset(&self.text, offset)
    }

    /// 選択中の素の ← → の行き先（#1742）。← は選択の始点・→ は終点へ畳む
    /// （VS Code / macOS のテキスト欄の標準）。
    ///
    /// 選択が無い・⇧ 付き（選択を伸ばす）・左右以外の移動なら `None`
    /// （語の移動や上下は従来どおりカーソルの位置から動く）
    fn collapse_edge(&self, movement: CursorMovement, extend_selection: bool) -> Option<usize> {
        let range = self.selection().filter(|_| !extend_selection)?;
        match movement {
            CursorMovement::Left => Some(range.start),
            CursorMovement::Right => Some(range.end),
            _ => None,
        }
    }

    /// その位置の桁（行頭からの**表示幅**。#1742）。上下移動の桁の記憶はこの単位で持つ
    fn display_column(&self, offset: usize) -> usize {
        display_width(&self.text[self.line_start(offset)..offset])
    }

    /// `delta` 行ぶん上下した行の、表示桁 `goal` にいちばん近い位置（#1742）。
    ///
    /// 行き先の行が短ければその行末へ寄せる（桁の記憶は呼び出し側が持ち越す）。
    /// 文書の端より先へは行かない: ページ移動は端の行で止まり、1 行の移動は
    /// 端の行で押しても動かない（↑ を先頭行で押しても桁 0 へは飛ばない = 従来どおり）
    fn vertical_target(&self, delta: isize, goal: usize) -> usize {
        let (line, _) = self.line_byte_col(self.cursor);
        let last = self.line_count() - 1;
        let target_line = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            line.saturating_add(delta.unsigned_abs()).min(last)
        };
        if target_line == line {
            return self.cursor;
        }
        let Some(start) = self.line_start_of(target_line) else {
            return self.cursor;
        };
        let line_text = &self.text[start..self.line_end(start)];
        start + byte_for_display_column(line_text, goal)
    }

    /// smart Home の行き先（#1652）。
    ///
    /// 最初の非空白（インデントの直後）にいれば桁 0 へ、それ以外なら最初の非空白へ。
    /// 押すたびに 2 点を行き来する（VS Code / Zed / Xcode と同じ）。インデントとして
    /// 数えるのは半角スペースとタブだけ。空行・空白だけの行では「最初の非空白」が
    /// 行末になる
    fn smart_line_start(&self, offset: usize) -> usize {
        let start = self.line_start(offset);
        let end = self.line_end(offset);
        let indent = self.text[start..end]
            .find(|c: char| c != ' ' && c != '\t')
            .unwrap_or(end - start);
        let first = start + indent;
        if offset == first {
            start
        } else {
            first
        }
    }
}

#[cfg(test)]
impl TextBuffer {
    /// まとめ判定の時計を固定する（#1651。テスト専用）。
    ///
    /// **まとめを検査するテストは必ずこれを呼ぶ**。実時間のままだと
    /// 「打鍵のあいだにスケジューラの待ちが入った回」だけ塊が切れて落ちる
    /// （`.agent/conventions.md`「効果を測る単体テストは実時間で比べない」）
    fn set_clock_millis(&mut self, millis: u64) {
        self.manual_millis = Some(millis);
    }
}

/// `offset` から始まる 1 文字のバイト長（末尾なら 1。探し直しの歩幅に使う）
fn char_len_at(text: &str, offset: usize) -> usize {
    text[offset..]
        .chars()
        .next()
        .map(char::len_utf8)
        .unwrap_or(1)
}

/// 単語単位の検索で「単語の文字」とみなすか（#1653）。
///
/// 識別子を作る文字 = Unicode の英数字と `_`。**漢字・かなも単語の文字に数える**ので、
/// `値value` の中の `value` は単語単位では当たらない（Rust などの識別子として 1 語だから）。
/// 日本語の文の中の語は空白で区切られないので、単語単位では切り出せない（区切るのは
/// 句読点・空白・記号だけ）。語で探したいときは単語単位を外す
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `start..end` の一致が単語の切れ目に載っているか（#1653）。
///
/// 端の文字が単語の文字でないとき（`.foo` や `foo(` をクエリにしたとき）は、その側の
/// 隣は問わない（VS Code の「単語単位」と同じ判定）
fn at_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let matched = &text[start..end];
    let joined = |outer: Option<char>, inner: Option<char>| matches!((outer, inner), (Some(o), Some(i)) if is_word_char(o) && is_word_char(i));
    !joined(text[..start].chars().next_back(), matched.chars().next())
        && !joined(text[end..].chars().next(), matched.chars().next_back())
}

/// 1 文字ずつ小文字化して連結する（#1016）
///
/// `str::to_lowercase` を使わないのは、あれが**文脈依存**だから。ギリシャ語の Σ は
/// 語末だけ ς になるので、本文とクエリを別々に丸ごと小文字化すると
/// 「本文にそのまま在る部分文字列を、それ自身で検索しても見つからない」ことが起きる
/// （本文 `ΟΔΟΣΧ` → `οδοσχ` / クエリ `ΟΔΟΣ` → `οδος`）。
/// 突き合わせる両側を同じ 1 文字単位の写像へ揃えることで、この食い違いを構造的に消す。
fn lowercase_per_char(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// 小文字化でバイト長が変わった文字 1 個ぶんの記録
struct Shift {
    /// 小文字化した写しでのその文字の開始位置
    lower_start: usize,
    /// 元テキストでのその文字の開始位置
    orig_start: usize,
    /// 小文字化後のバイト長
    lower_len: usize,
    /// 元のバイト長
    orig_len: usize,
}

/// 小文字化した本文の写しと、その位置を元テキストへ戻すための情報（#1016）
///
/// `shifts` は**バイト長が変わった文字だけ**を開始位置の昇順で持つ。
/// ASCII・日本語・ほとんどのラテン文字では空なので、位置の変換は恒等になる。
struct Lowered {
    text: String,
    shifts: Vec<Shift>,
}

impl Lowered {
    fn build(text: &str) -> Self {
        let mut lower = String::with_capacity(text.len());
        let mut shifts = Vec::new();
        for (orig_start, ch) in text.char_indices() {
            // ASCII の小文字化は必ず 1 バイト → 1 バイトなのでずれない
            if ch.is_ascii() {
                lower.push(ch.to_ascii_lowercase());
                continue;
            }
            let lower_start = lower.len();
            for lc in ch.to_lowercase() {
                lower.push(lc);
            }
            let lower_len = lower.len() - lower_start;
            let orig_len = ch.len_utf8();
            if lower_len != orig_len {
                shifts.push(Shift {
                    lower_start,
                    orig_start,
                    lower_len,
                    orig_len,
                });
            }
        }
        Self {
            text: lower,
            shifts,
        }
    }

    /// 写しのバイト位置 `pos` に対応する元テキストのバイト位置を返す。
    ///
    /// `pos` が「小文字化で複数文字へ展開された文字」の途中を指すときは `None`
    /// （元テキストに対応するバイト位置が存在しない）。
    fn to_original(&self, pos: usize) -> Option<usize> {
        if self.shifts.is_empty() {
            return Some(pos);
        }
        // `pos` 以下で始まる最後のずれを探す。それより後ろは 1:1 対応に戻る
        let idx = self.shifts.partition_point(|s| s.lower_start <= pos);
        let Some(shift) = idx.checked_sub(1).map(|i| &self.shifts[i]) else {
            return Some(pos);
        };
        if pos == shift.lower_start {
            return Some(shift.orig_start);
        }
        let lower_end = shift.lower_start + shift.lower_len;
        if pos < lower_end {
            return None;
        }
        Some(shift.orig_start + shift.orig_len + (pos - lower_end))
    }
}

/// 本文から行頭索引を作る（#1660。開いたときに 1 回だけ。以後は差分で追従する）
fn compute_line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

fn snap_boundary(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// 行末（改行の手前）のバイト位置。`\r\n` で終わる行では **CR の手前**を返す（#1650）。
///
/// `\n` の位置を返していたのが Issue #1650 の本体で、CRLF ファイルで End を押すと
/// カーソルが画面上の行末より右（CR の後ろ）へ行き、そこで打つと `abc\r!\n` になった
fn line_end_offset(text: &str, offset: usize) -> usize {
    match text[offset..].find('\n') {
        Some(i) => {
            let newline = offset + i;
            if text[..newline].ends_with('\r') {
                newline - 1
            } else {
                newline
            }
        }
        None => text.len(),
    }
}

/// ⇧Tab で行頭から外すバイト数（#1654）。
///
/// 先頭がタブならそのタブ 1 つ。スペースなら 1 つ前のタブ位置まで（6 桁で 4 スペース単位なら 2 つ、
/// 8 桁なら 4 つ）。タブでインデントするファイルに混ざったスペースは最大 [`TAB_COLUMNS`] 個
fn outdent_width(content: &str, unit: IndentUnit) -> usize {
    if content.starts_with('\t') {
        return 1;
    }
    let spaces = content.len() - content.trim_start_matches(' ').len();
    if spaces == 0 {
        return 0;
    }
    match unit {
        IndentUnit::Tab => spaces.min(TAB_COLUMNS),
        IndentUnit::Spaces(n) if spaces.is_multiple_of(n) => n,
        IndentUnit::Spaces(n) => spaces % n,
    }
}

/// `offset` の直前にある行区切りのバイト数（#1652）。`\r\n` なら 2、`\n` なら 1、無ければ 0
fn line_break_len_before(text: &str, offset: usize) -> usize {
    let head = &text[..offset];
    if head.ends_with("\r\n") {
        2
    } else if head.ends_with('\n') {
        1
    } else {
        0
    }
}

/// `offset` の直後にある行区切りのバイト数（#1652）。`\r\n` なら 2、`\n` なら 1、無ければ 0
fn line_break_len_after(text: &str, offset: usize) -> usize {
    let tail = &text[offset..];
    if tail.starts_with("\r\n") {
        2
    } else if tail.starts_with('\n') {
        1
    } else {
        0
    }
}

/// 語の区切りに使う文字の種類（#1652）。**同じ種類が続くあいだが 1 語**。
///
/// 空白をまたいで次の塊へ進み、塊の端で止まる。英数字と `_` は 1 種類
/// （`snake_case` は 1 語）、記号の並びは記号どうしで 1 語。日本語は分かち書きを
/// しないので、**文字種の切り替わり**を語の境目にする（`日本語のテキスト` は
/// 日本語 / の / テキスト の 3 語）。辞書による分割は持たない
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    /// 行内の空白（スペース・タブ・全角スペース・単独の CR）
    Space,
    /// 英数字と `_`（全角の英数字も含む）
    Word,
    /// 漢字（`々` を含む）
    Han,
    Hiragana,
    /// カタカナ（長音符 `ー`・半角カナを含む）
    Katakana,
    /// それ以外（記号・句読点・絵文字）
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        return CharClass::Space;
    }
    match u32::from(c) {
        0x3040..=0x309F => CharClass::Hiragana,
        0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => CharClass::Katakana,
        0x3005 | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => CharClass::Han,
        _ if c.is_alphanumeric() || c == '_' => CharClass::Word,
        _ => CharClass::Punct,
    }
}

/// ⌥← / Ctrl+← の行き先（#1652）: 前の語の頭。
///
/// 左へ空白を飛ばし、そこにある塊の頭で止まる。**行頭で押すと前の行の最後の語の
/// 頭へ**またぐ（改行をまたいでから同じ規則。VS Code / macOS のテキスト欄と同じ）。
/// 空白の途中で行頭に着いたらそこで止まる（インデントの手前で止まる）
fn word_left_offset(text: &str, offset: usize) -> usize {
    let mut p = offset;
    p -= line_break_len_before(text, p);
    let prev = |p: usize| text[..p].chars().next_back();
    while let Some(c) = prev(p) {
        if c == '\n' || char_class(c) != CharClass::Space {
            break;
        }
        p -= c.len_utf8();
    }
    let Some(first) = prev(p).filter(|c| *c != '\n') else {
        return p;
    };
    let class = char_class(first);
    while let Some(c) = prev(p) {
        if c == '\n' || char_class(c) != class {
            break;
        }
        p -= c.len_utf8();
    }
    p
}

/// ⌥→ / Ctrl+→ の行き先（#1652）: 次の語の末尾。
///
/// 右へ空白を飛ばし、そこにある塊の末尾で止まる。**行末で押すと次の行の最初の語の
/// 末尾へ**またぐ。空白のまま行末に着いたらそこで止まる。`\r\n` の CR は
/// 行区切りの一部として扱い、空白として飛ばさない（カーソルが CR と LF の
/// あいだへ入らない = #1650）
fn word_right_offset(text: &str, offset: usize) -> usize {
    let mut p = offset;
    p += line_break_len_after(text, p);
    let next = |p: usize| {
        if line_break_len_after(text, p) > 0 {
            None
        } else {
            text[p..].chars().next()
        }
    };
    while let Some(c) = next(p) {
        if char_class(c) != CharClass::Space {
            break;
        }
        p += c.len_utf8();
    }
    let Some(first) = next(p) else {
        return p;
    };
    let class = char_class(first);
    while let Some(c) = next(p) {
        if char_class(c) != class {
            break;
        }
        p += c.len_utf8();
    }
    p
}

/// `old` を `new` にするのに書き換える範囲と、そこへ入る本文（#1659）。
///
/// 共通の先頭・末尾を除いた中央だけを返す（文字の途中では切らない）。
/// 読み直しの履歴を「食い違った範囲」だけにするための計算
fn changed_span<'a>(old: &str, new: &'a str) -> (Range<usize>, &'a str) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    // 共通部分は同じバイト列なので、片方で文字境界なら他方でも境界
    while !old.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = a.len().min(b.len()) - prefix;
    let mut suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    while !old.is_char_boundary(a.len() - suffix) {
        suffix -= 1;
    }
    (prefix..a.len() - suffix, &new[prefix..b.len() - suffix])
}

/// `range` を長さ `inserted` の本文で置き換えた後、`offset` がどこへ移るか（#1659）。
///
/// 範囲より前はそのまま、後ろは長さの差だけずらす。範囲の中にいたら、
/// 範囲の頭からの距離を保ったまま置き換えた本文の中へ寄せる
fn follow_offset(offset: usize, range: &Range<usize>, inserted: usize) -> usize {
    if offset <= range.start {
        offset
    } else if offset >= range.end {
        offset - range.end + range.start + inserted
    } else {
        range.start + (offset - range.start).min(inserted)
    }
}

/// カーソルを置いてよい位置へ丸める。
///
/// UTF-8 の文字境界に加えて、**`\r\n` のあいだ**（CR の後ろ）を除く（#1650）。
/// そこは画面上どこにも対応しない位置で、行末判定を直しても外から
/// [`TextBuffer::set_cursor`]（マウスクリック・IME・dispatch）で指されうる
fn snap_cursor(text: &str, offset: usize) -> usize {
    let offset = snap_boundary(text, offset);
    let bytes = text.as_bytes();
    if offset > 0 && bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
        offset - 1
    } else {
        offset
    }
}

/// 差し込む本文の改行を `ending` へ揃える（#1650）。
///
/// 単独の `\r` は行区切りではないので触らない。揃える必要が無いときは
/// 借用のまま返すので、1 文字ずつの打鍵では何も確保しない
/// カーソルと選択端から選択範囲を作る（#1651 で 1 実装へ）。
///
/// 選択端が無い / カーソルと同じ位置なら「選択なし」。
/// [`TextBuffer::selection`] と [`TextBuffer::set_cursor`] の「動いたか」の判定が
/// 同じ定義を見るようにしてある（片方だけずれると塊の切れ目が食い違う）
fn selection_range(anchor: Option<usize>, cursor: usize) -> Option<Range<usize>> {
    let anchor = anchor?;
    (anchor != cursor).then(|| anchor.min(cursor)..anchor.max(cursor))
}

/// 改行を含むか（#1651 のまとめ判定）。
///
/// `\n` だけ見れば足りる（`\r\n` にも必ず `\n` が在る）。単独の `\r` は行区切りでは
/// ないので、そこで塊を切る理由も無い
fn contains_newline(text: &str) -> bool {
    text.contains('\n')
}

fn normalize_line_endings(text: &str, ending: LineEnding) -> Cow<'_, str> {
    match ending {
        LineEnding::Lf if text.contains('\r') => Cow::Owned(text.replace("\r\n", "\n")),
        LineEnding::Crlf if text.contains('\n') => {
            // 一度 LF へ潰してから広げる（既に `\r\n` の場所を二重化しないため）
            Cow::Owned(text.replace("\r\n", "\n").replace('\n', "\r\n"))
        }
        _ => Cow::Borrowed(text),
    }
}

#[cfg(unix)]
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".{name}.tako-save-{}-{nonce}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        match std::fs::metadata(path) {
            Ok(metadata) => std::fs::set_permissions(&temp, metadata.permissions())?,
            // 外で消されたファイルを上書き保存で作り直す（#1659）。引き継ぐ権限が無いので
            // 一時ファイルの既定（新規ファイルと同じ）のまま置く
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(unix))]
fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // `create`: 外で消されたファイルを上書き保存で作り直す（#1659）。
    // 消えたファイルへの通常の保存は `save_inner` が書く前に断る
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tako-text-edit-{}-{name}", std::process::id()))
    }

    /// 大文字小文字を区別しない検索（#1653 で既定ではなくなった側。#1016 の検査はこちらを通す）
    const IGNORE_CASE: SearchOptions = SearchOptions {
        case_sensitive: false,
        whole_word: false,
    };

    #[test]
    fn utf8の入力削除とカーソル移動は文字境界を保つ() {
        let mut buffer = TextBuffer::from_text(path("utf8"), "a日本語z".into());
        buffer.move_cursor(CursorMovement::Right, false);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), "a日".len());
        buffer.delete_backward();
        assert_eq!(buffer.text(), "a本語z");
        buffer.delete_forward();
        assert_eq!(buffer.text(), "a語z");
        // 本文に改行が 1 つも無いので改行コードは新規ファイルの既定（OS で変わる。#1650）。
        // 期待値は**製品の正**から作る（`\n` を直書きすると Windows でだけ落ちる）。
        // 既定そのものの腕は `新規ファイルの改行コードはプラットフォームで決まる` が固定する
        let eol = buffer.line_ending().as_str();
        buffer.insert("界\n");
        assert_eq!(buffer.text(), format!("a界{eol}語z"));
    }

    #[test]
    fn 選択置換と上下移動を扱える() {
        let mut buffer = TextBuffer::from_text(path("selection"), "abc\n日本語\nxy".into());
        buffer.set_cursor(1, false);
        buffer.set_cursor("abc\n日本".len(), true);
        buffer.insert("Z");
        assert_eq!(buffer.text(), "aZ語\nxy");
        buffer.move_cursor(CursorMovement::DocumentStart, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 0));
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (0, 2));

        buffer.move_cursor(CursorMovement::LineEnd, false);
        buffer.move_cursor(CursorMovement::Left, true);
        buffer.move_cursor(CursorMovement::Left, true);
        assert_eq!(buffer.anchor(), Some("aZ語".len()));
        assert_eq!(buffer.selection(), Some(1.."aZ語".len()));
    }

    #[test]
    fn 空ファイルを編集して保存できる() {
        let path = path("empty");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        // 空ファイルには改行コードの手掛かりが無いので新規ファイルの既定（OS で変わる。#1650）
        let eol = buffer.line_ending().as_str();
        assert_eq!(buffer.line_ending(), LineEnding::platform_default());
        buffer.insert("こんにちは\n");
        assert!(buffer.dirty());
        buffer.save().unwrap();
        assert!(!buffer.dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("こんにちは{eol}")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 外部変更を検知して上書きしない() {
        let path = path("external");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("mine".into());
        std::fs::write(&path, "external").unwrap();
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalChanged)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        assert!(buffer.dirty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 読み取り専用ファイルの保存は失敗して内容を保つ() {
        let path = path("readonly");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("after".into());
        assert!(matches!(buffer.save(), Err(TextEditError::Write(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(false);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 数千行の日本語バッファを末尾で編集できる() {
        let text = (0..5_000)
            .map(|i| format!("{i}: 日本語"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut buffer = TextBuffer::from_text(path("large"), text);
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.newline();
        buffer.insert("末尾");
        buffer.delete_backward();
        assert!(buffer.text().ends_with("末"));
        assert!(buffer.dirty());
    }

    #[test]
    fn undoとredoで編集を往復できる() {
        let mut buffer = TextBuffer::from_text(path("undo"), "abc".into());
        assert!(!buffer.can_undo());
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.insert("X");
        assert_eq!(buffer.text(), "abcX");
        assert!(buffer.can_undo());
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abc");
        assert!(buffer.can_redo());
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "abcX");
        // 新しい編集で redo スタックがクリアされる
        buffer.insert("Y");
        assert!(!buffer.can_redo());
    }

    #[test]
    fn undo上限を超えると古い差分が消える() {
        let mut buffer = TextBuffer::from_text(path("undo-limit"), String::new());
        buffer.set_clock_millis(0);
        for i in 0..UNDO_LIMIT + 10 {
            // 1 操作 = 1 塊にするため、毎回塊を閉じる（#1651。まとめると 1 件になる）
            buffer.seal_undo_group();
            buffer.insert(&i.to_string());
        }
        assert_eq!(buffer.undo_depth(), UNDO_LIMIT);
    }

    #[test]
    fn delete_backwardのundoが正しく復元する() {
        let mut buffer = TextBuffer::from_text(path("undo-del"), "日本語".into());
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "日本");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "日本語");
    }

    #[test]
    fn 選択削除のundoが正しく復元する() {
        let mut buffer = TextBuffer::from_text(path("undo-sel"), "abcdef".into());
        buffer.set_cursor(1, false);
        buffer.set_cursor(4, true);
        buffer.delete_forward();
        assert_eq!(buffer.text(), "aef");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abcdef");
    }

    #[test]
    fn 区別しない検索は大文字小文字を無視して当てる() {
        let buffer = TextBuffer::from_text(path("search"), "Hello hello HELLO".into());
        let hits = buffer.find_all("hello", IGNORE_CASE);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].start, 0);
        assert_eq!(hits[0].end, 5);
    }

    #[test]
    fn next_hitはラップ検索する() {
        let buffer = TextBuffer::from_text(path("search-wrap"), "aXbXc".into());
        let hits = buffer.find_all("x", IGNORE_CASE);
        let hit = next_hit(&hits, 3).unwrap();
        assert_eq!(hit.start, 3);
        // from を末尾にするとラップして先頭へ
        let hit = next_hit(&hits, 5).unwrap();
        assert_eq!(hit.start, 1);
    }

    #[test]
    fn prev_hitは逆ラップ検索する() {
        let buffer = TextBuffer::from_text(path("search-prev"), "aXbXc".into());
        let hits = buffer.find_all("x", IGNORE_CASE);
        let hit = prev_hit(&hits, 2).unwrap();
        assert_eq!(hit.start, 1);
        // from を先頭にするとラップして末尾へ
        let hit = prev_hit(&hits, 0).unwrap();
        assert_eq!(hit.start, 3);
    }

    #[test]
    fn 空クエリの検索は空を返す() {
        let buffer = TextBuffer::from_text(path("search-empty"), "abc".into());
        for options in [SearchOptions::DEFAULT, IGNORE_CASE] {
            let hits = buffer.find_all("", options);
            assert!(hits.is_empty());
            assert!(next_hit(&hits, 0).is_none());
        }
    }

    #[test]
    fn replace_rangeは1件を置き換えてundoできる() {
        let mut buffer = TextBuffer::from_text(path("replace1"), "foo bar foo".into());
        buffer.replace_range(0..3, "baz");
        assert_eq!(buffer.text(), "baz bar foo");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "foo bar foo");
    }

    #[test]
    fn replace_allは全件を置き換える() {
        let mut buffer = TextBuffer::from_text(path("replace-all"), "aXbXcX".into());
        let count = buffer.replace_all("x", "YY", IGNORE_CASE);
        assert_eq!(count, 3);
        assert_eq!(buffer.text(), "aYYbYYcYY");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "aXbXcX");
    }

    /// #1659: 外部変更を検知した後の 2 択（上書き / 読み直し）の往復
    #[test]
    fn 外部変更の後に上書き保存すると自分の変更が残る() {
        let path = path("overwrite-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before\n").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("mine\n".into());
        std::fs::write(&path, "external\n").unwrap();
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalChanged)));
        assert_eq!(buffer.refresh_disk_state().unwrap(), DiskState::Changed);
        buffer.save_overwrite().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine\n");
        assert!(!buffer.dirty());
        assert_eq!(buffer.refresh_disk_state().unwrap(), DiskState::Unchanged);
        // 競合が解けたので、以後は通常の保存がそのまま通る
        buffer.set_cursor(0, false);
        buffer.insert("more ");
        buffer.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "more mine\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 外部変更の後に読み直すとディスクの中身になりundoで自分の変更へ戻る() {
        let path = path("reload-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_cursor("line1\nline2".len(), false);
        buffer.insert(" mine");
        std::fs::write(&path, "line1\nline2\nline3 external\n").unwrap();
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalChanged)));
        let version = buffer.version();
        buffer.reload_from_disk().unwrap();
        assert_eq!(buffer.text(), "line1\nline2\nline3 external\n");
        assert!(!buffer.dirty());
        assert!(
            buffer.version() > version,
            "読み直しも本文の変更なので版が進む"
        );
        // 通常の保存が通る（基準がディスクの中身になった）
        buffer.save().unwrap();
        // 読み直しは 1 回の編集 = undo 1 回で自分の変更へ戻る
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "line1\nline2 mine\nline3\n");
        assert!(buffer.dirty());
        // 戻した自分の変更は、今度は競合せずに保存できる（基準はディスクの中身）
        buffer.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line1\nline2 mine\nline3\n"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 読み直しは食い違った範囲だけを履歴に積む() {
        let path = path("reload-span-1659");
        let _ = std::fs::remove_file(&path);
        let body: String = (0..50_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &body).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        // 自分の編集（25001 行目）と外部の編集（25000 行目）が近い = 食い違いは数十バイト
        let at = buffer.offset_for_line_byte_col(25_001, 0);
        buffer.set_cursor(at, false);
        buffer.insert("x");
        std::fs::write(&path, body.replace("line 25000\n", "changed\n")).unwrap();
        let before = buffer.undo_history_bytes();
        buffer.reload_from_disk().unwrap();
        // 全文（約 600 KB）を 2 本積まない
        assert!(
            buffer.undo_history_bytes() - before < 1024,
            "history={} before={before}",
            buffer.undo_history_bytes()
        );
        assert!(!buffer.dirty());
        assert_eq!(
            buffer.line_starts(),
            &compute_line_starts(buffer.text())[..]
        );

        // 離れていれば間ごと 1 範囲になるが、全文差し替え（本文 2 本ぶん）より大きくはならない
        buffer.set_cursor(0, false);
        buffer.insert("y");
        std::fs::write(&path, body.replace("line 49999\n", "tail\n")).unwrap();
        let before = buffer.undo_history_bytes();
        buffer.reload_from_disk().unwrap();
        // 余裕の 1 KiB は差分 1 件ぶんの器（`EditDelta` 自身の大きさ）
        assert!(buffer.undo_history_bytes() <= before + 2 * body.len() + 1024);
        assert_eq!(buffer.text(), body.replace("line 49999\n", "tail\n"));
        assert_eq!(
            buffer.line_starts(),
            &compute_line_starts(buffer.text())[..]
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 読み直してもカーソルは同じ場所に残る() {
        let path = path("reload-cursor-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "aaa\nbbb\nccc\n").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_cursor("aaa\nbbb\nc".len(), false);
        // カーソルより前の行が 1 行増える
        std::fs::write(&path, "aaa\nNEW\nbbb\nccc\n").unwrap();
        buffer.reload_from_disk().unwrap();
        assert_eq!(buffer.cursor(), "aaa\nNEW\nbbb\nc".len());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 外で消されたファイルは保存も読み直しも断り上書きなら作り直す() {
        let path = path("deleted-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("mine".into());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(buffer.refresh_disk_state().unwrap(), DiskState::Deleted);
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalDeleted)));
        assert!(matches!(
            buffer.reload_from_disk(),
            Err(TextEditError::ExternalDeleted)
        ));
        assert_eq!(buffer.text(), "mine", "読み直せないときは本文を触らない");
        assert!(!path.exists(), "通常の保存は作り直さない");
        let diff = buffer.disk_diff().unwrap();
        assert_eq!(diff.state, DiskState::Deleted);
        assert_eq!((diff.diff.added, diff.diff.removed), (1, 0));
        buffer.save_overwrite().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
        assert!(!buffer.dirty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 外で自分と同じ中身に書かれたら競合にしない() {
        let path = path("same-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "before").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("after".into());
        std::fs::write(&path, "after").unwrap();
        assert_eq!(buffer.refresh_disk_state().unwrap(), DiskState::Unchanged);
        assert!(!buffer.dirty());
        buffer.save().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 差分はディスクから編集中への向き() {
        let path = path("diff-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.set_text("a\nMINE\nc\n".into());
        std::fs::write(&path, "a\nDISK\nc\n").unwrap();
        let diff = buffer.disk_diff().unwrap();
        assert_eq!(diff.state, DiskState::Changed);
        assert_eq!(
            diff.diff.unified("disk", "buffer"),
            "--- disk\n+++ buffer\n@@ -1,3 +1,3 @@\n a\n-DISK\n+MINE\n c\n"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 読み直しは改行コードをディスクから取り直しcrlfを保つ() {
        let path = path("reload-crlf-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "a\nb\n").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("x");
        std::fs::write(&path, "a\r\nb\r\nc\r\n").unwrap();
        buffer.reload_from_disk().unwrap();
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        buffer.set_cursor(buffer.text().len(), false);
        buffer.newline();
        assert!(buffer.text().ends_with("c\r\n\r\n"), "新しい改行も CRLF");
        buffer.save().unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"a\r\nb\r\nc\r\n\r\n".to_vec()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 読み直せない中身なら本文を触らない() {
        let path = path("reload-binary-1659");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "text").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("mine ");
        std::fs::write(&path, b"bin\0ary").unwrap();
        assert!(matches!(
            buffer.reload_from_disk(),
            Err(TextEditError::NotEditable(_))
        ));
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(matches!(
            buffer.reload_from_disk(),
            Err(TextEditError::InvalidUtf8)
        ));
        assert_eq!(buffer.text(), "mine text");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 書き換える範囲は文字の途中で切らない() {
        // 「あ」と「い」は先頭 2 バイトが同じ（E3 81 82 / E3 81 84）
        let (range, inserted) = changed_span("xあy", "xいy");
        assert_eq!(range, 1..4);
        assert_eq!(inserted, "い");
        let (range, inserted) = changed_span("same", "same");
        assert_eq!((range, inserted), (4..4, ""));
        let (range, inserted) = changed_span("aaaa", "aa");
        assert_eq!((range.len(), inserted), (2, ""));
        assert_eq!(follow_offset(0, &(2..4), 1), 0);
        assert_eq!(follow_offset(9, &(2..4), 1), 8);
        assert_eq!(follow_offset(3, &(2..4), 0), 2);
    }

    #[test]
    fn 自動保存で外部変更を上書きしない() {
        let path = path("autosave-conflict");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "original").unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("edit");
        // 外部変更を模擬
        std::fs::write(&path, "external_change").unwrap();
        assert!(matches!(buffer.save(), Err(TextEditError::ExternalChanged)));
        // ファイルの中身は外部変更のまま
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external_change");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn set_textのundoでテキストが復元する() {
        let mut buffer = TextBuffer::from_text(path("set-text-undo"), "old".into());
        buffer.set_text("new".into());
        assert_eq!(buffer.text(), "new");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "old");
    }

    // --- エッジケーステスト（#195 受け入れ条件の検証） ---

    #[test]
    fn 空バッファでのundo_redoは安全にfalseを返す() {
        let mut buffer = TextBuffer::from_text(path("edge-empty-undo"), String::new());
        assert!(!buffer.undo());
        assert!(!buffer.redo());
        assert!(!buffer.can_undo());
        assert!(!buffer.can_redo());
    }

    #[test]
    fn 空バッファへの検索は空を返す() {
        let buffer = TextBuffer::from_text(path("edge-empty-search"), String::new());
        for options in [SearchOptions::DEFAULT, IGNORE_CASE] {
            let hits = buffer.find_all("abc", options);
            assert!(hits.is_empty());
            assert!(next_hit(&hits, 0).is_none());
            assert!(prev_hit(&hits, 0).is_none());
        }
    }

    #[test]
    fn 空バッファへの全置換は0件を返す() {
        let mut buffer = TextBuffer::from_text(path("edge-empty-replace"), String::new());
        assert_eq!(buffer.replace_all("a", "b", SearchOptions::DEFAULT), 0);
    }

    #[test]
    fn 五千行バッファでundo_redoが動く() {
        let text = (0..5_000)
            .map(|i| format!("{i}: テスト行"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut buffer = TextBuffer::from_text(path("edge-large-undo"), text.clone());
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.insert("追加");
        assert!(buffer.text().ends_with("追加"));
        assert!(buffer.undo());
        assert_eq!(buffer.text(), text);
    }

    #[test]
    fn 五千行バッファで検索が動く() {
        let text = (0..5_000)
            .map(|i| format!("{i}: テスト行"))
            .collect::<Vec<_>>()
            .join("\n");
        let buffer = TextBuffer::from_text(path("edge-large-search"), text);
        let hits = buffer.find_all("テスト行", SearchOptions::DEFAULT);
        assert_eq!(hits.len(), 5_000);
        let hits = buffer.find_all("4999", SearchOptions::DEFAULT);
        let hit = next_hit(&hits, 0).unwrap();
        assert!(hit.start > 0);
    }

    #[test]
    fn 読み取り専用ファイルの自動保存は失敗してバッファを保つ() {
        let path = path("edge-readonly-autosave");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "readonly content").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        buffer.insert("edit");
        assert!(buffer.dirty());
        assert!(buffer.save().is_err());
        assert!(buffer.dirty());
        assert!(buffer.text().contains("edit"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let _ = std::fs::remove_file(path);
    }

    // --- #1016: 小文字化でバイト長が変わる文字でも位置が元テキスト基準であること ---

    #[test]
    fn ascii高速路の前提が成り立つ() {
        // `Lowered::build` は「ASCII の小文字化は必ず 1 バイト → 1 バイト」を前提に
        // ずれの記録を省いている。前提が崩れたらここで落ちる
        for byte in 0u8..=127 {
            let ch = byte as char;
            let unicode: String = ch.to_lowercase().collect();
            assert_eq!(
                unicode,
                ch.to_ascii_lowercase().to_string(),
                "ASCII {byte:#04x} の小文字化が ASCII 版と食い違う"
            );
        }
    }

    #[test]
    fn 位置の変換が展開された文字の途中を弾く() {
        // `İ`(2 バイト) → `i` + U+0307(3 バイト)。写しの 1 バイト目は展開の途中
        let lowered = Lowered::build("aİb");
        assert_eq!(lowered.text, "ai\u{307}b");
        assert_eq!(lowered.to_original(0), Some(0)); // 'a'
        assert_eq!(lowered.to_original(1), Some(1)); // 'İ' の先頭
        assert_eq!(lowered.to_original(2), None); // U+0307 の途中 = 対応する位置が無い
        assert_eq!(lowered.to_original(4), Some(3)); // 'b'
        assert_eq!(lowered.to_original(5), Some(4)); // 末尾
    }

    #[test]
    fn ずれが無い本文では位置の変換が恒等になる() {
        let lowered = Lowered::build("Abc あいう");
        assert!(lowered.shifts.is_empty());
        for pos in 0..=lowered.text.len() {
            assert_eq!(lowered.to_original(pos), Some(pos));
        }
    }

    #[test]
    fn 小文字化で伸びる文字があってもヒット位置が元テキスト基準になる() {
        // `İ`（U+0130）は小文字化で 2 → 3 バイトに伸びる。小文字化した本文の
        // バイト位置を元テキストの位置として流用すると、後続のヒットがずれる
        let text = "İstanbul needle";
        let buffer = TextBuffer::from_text(path("u1016-grow"), text.into());
        let hits = buffer.find_all("needle", IGNORE_CASE);
        assert_eq!(hits.len(), 1);
        let expected = text.find("needle").unwrap();
        assert_eq!(hits[0].start, expected);
        assert_eq!(hits[0].end, expected + "needle".len());
        assert_eq!(&text[hits[0].start..hits[0].end], "needle");
    }

    #[test]
    fn 小文字化で縮む文字があってもヒット位置が元テキスト基準になる() {
        // `ẞ`（U+1E9E）は小文字化で 3 → 2 バイトに縮む
        let text = "ẞ needle";
        let buffer = TextBuffer::from_text(path("u1016-shrink"), text.into());
        let hits = buffer.find_all("needle", IGNORE_CASE);
        assert_eq!(hits.len(), 1);
        let expected = text.find("needle").unwrap();
        assert_eq!(hits[0].start, expected);
        assert_eq!(&text[hits[0].start..hits[0].end], "needle");
    }

    #[test]
    fn 伸縮が打ち消しあって総バイト長が同じでも位置がずれない() {
        // `İ` は +1 / `ẞ` は -1 なので本文全体のバイト長は変わらないが、
        // 途中のバイト位置は食い違う（= 総バイト長の一致は健全性の根拠にならない）
        let text = "İ needle ẞ";
        assert_eq!(text.len(), text.to_lowercase().len());
        let buffer = TextBuffer::from_text(path("u1016-cancel"), text.into());
        let hits = buffer.find_all("needle", IGNORE_CASE);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, text.find("needle").unwrap());
    }

    #[test]
    fn 行頭と行末と複数ヒットでも位置が元テキスト基準になる() {
        let text = "needle İ\nneedle İ needle";
        let buffer = TextBuffer::from_text(path("u1016-multi"), text.into());
        let hits = buffer.find_all("NEEDLE", IGNORE_CASE);
        assert_eq!(hits.len(), 3);
        let expected: Vec<usize> = text.match_indices("needle").map(|(i, _)| i).collect();
        assert_eq!(
            hits.iter().map(|h| h.start).collect::<Vec<_>>(),
            expected,
            "行頭・行中・行末のヒットがすべて元テキスト基準であること"
        );
        for hit in &hits {
            assert_eq!(&text[hit.start..hit.end], "needle");
        }
    }

    #[test]
    fn ヒットの終端が本文の範囲を超えない() {
        // 本文 `İ`（2 バイト）は小文字化すると `i` + U+0307（3 バイト）。
        // 終端に「元クエリのバイト長」を足すと本文の範囲外を指し、置換が panic する
        let text = "İ";
        let mut buffer = TextBuffer::from_text(path("u1016-range"), text.into());
        let hits = buffer.find_all("i\u{307}", IGNORE_CASE);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, 0);
        assert_eq!(
            hits[0].end,
            text.len(),
            "終端は元テキストのバイト長で決まる"
        );
        buffer.replace_range(hits[0].start..hits[0].end, "I");
        assert_eq!(buffer.text(), "I");
    }

    #[test]
    fn 小文字化した文字の途中から始まるクエリはヒットしない() {
        // `İ` の小文字化は `i` + U+0307。U+0307 から始まるクエリは元テキストの
        // 文字境界に対応しないので、返せる位置が存在しない（返すと slice が panic する）
        let buffer = TextBuffer::from_text(path("u1016-mid"), "İstanbul".into());
        assert!(buffer.find_all("\u{307}stanbul", IGNORE_CASE).is_empty());
    }

    #[test]
    fn 本文の部分文字列はそれ自身をクエリにすれば必ず見つかる() {
        // `str::to_lowercase` は文脈依存（語末の Σ だけ ς になる）なので、本文と
        // クエリを別々に丸ごと小文字化すると同じ文字列同士が一致しないことがある
        let text = "ΟΔΟΣΧ";
        let buffer = TextBuffer::from_text(path("u1016-sigma"), text.into());
        let hits = buffer.find_all("ΟΔΟΣ", IGNORE_CASE);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, 0);
        assert_eq!(&text[hits[0].start..hits[0].end], "ΟΔΟΣ");
    }

    #[test]
    fn 小文字化で伸びる文字を含む本文を壊さずに全置換できる() {
        let mut buffer =
            TextBuffer::from_text(path("u1016-replace-all"), "İstanbul foo İzmir foo".into());
        let count = buffer.replace_all("foo", "bar", IGNORE_CASE);
        assert_eq!(count, 2);
        assert_eq!(buffer.text(), "İstanbul bar İzmir bar");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "İstanbul foo İzmir foo");
    }

    #[test]
    fn 伸びる文字そのものを全置換しても本文が壊れない() {
        let mut buffer = TextBuffer::from_text(path("u1016-replace-char"), "aİbİc".into());
        let count = buffer.replace_all("İ", "-", IGNORE_CASE);
        assert_eq!(count, 2);
        assert_eq!(buffer.text(), "a-b-c");
    }

    #[test]
    fn asciiのみと日本語のみの検索置換は従来どおり動く() {
        // 回帰確認: 小文字化でバイト長が変わらない文字だけの本文
        let buffer = TextBuffer::from_text(path("u1016-ascii"), "Foo foo FOO".into());
        let hits = buffer.find_all("foo", IGNORE_CASE);
        assert_eq!(
            hits.iter().map(|h| (h.start, h.end)).collect::<Vec<_>>(),
            vec![(0, 3), (4, 7), (8, 11)]
        );

        let mut jp = TextBuffer::from_text(path("u1016-jp"), "あいうえお かきくけこ".into());
        let hits = jp.find_all("うえ", IGNORE_CASE);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, "あい".len());
        assert_eq!(hits[0].end, "あいうえ".len());
        assert_eq!(jp.replace_all("かき", "サシ", IGNORE_CASE), 1);
        assert_eq!(jp.text(), "あいうえお サシくけこ");
    }

    #[test]
    fn 伸びる文字を含む本文でも空クエリは空を返す() {
        let buffer = TextBuffer::from_text(path("u1016-empty"), "İstanbul".into());
        let hits = buffer.find_all("", IGNORE_CASE);
        assert!(hits.is_empty());
        assert!(next_hit(&hits, 0).is_none());
        assert!(prev_hit(&hits, 0).is_none());
    }

    // --- #1653: 大文字小文字の区別（既定 = 区別する）と単語単位 ---

    fn spans(hits: &[SearchHit]) -> Vec<(usize, usize)> {
        hits.iter().map(|h| (h.start, h.end)).collect()
    }

    /// 4 通りの条件を名前つきで並べる（落ちたときにどの組み合わせかが読める）
    fn matrix() -> [(&'static str, SearchOptions); 4] {
        let o = |case_sensitive, whole_word| SearchOptions {
            case_sensitive,
            whole_word,
        };
        [
            ("区別する・単語単位なし", o(true, false)),
            ("区別する・単語単位", o(true, true)),
            ("区別しない・単語単位なし", o(false, false)),
            ("区別しない・単語単位", o(false, true)),
        ]
    }

    #[test]
    fn 既定の検索は大文字小文字を区別する_1653() {
        // Issue #1653 の実測: 修正前は 3 件（Foo / foo / FOO）
        let buffer = TextBuffer::from_text(path("u1653-default"), "Foo foo FOO".into());
        assert_eq!(SearchOptions::default(), SearchOptions::DEFAULT);
        assert_eq!(
            SearchOptions::DEFAULT,
            SearchOptions {
                case_sensitive: true,
                whole_word: false,
            },
            "既定は「区別する・単語単位なし」"
        );
        let hits = buffer.find_all("foo", SearchOptions::DEFAULT);
        assert_eq!(
            spans(&hits),
            vec![(4, 7)],
            "既定で当たるのは小文字の foo だけ"
        );
    }

    #[test]
    fn 既定の全置換は型名を潰さない_1653() {
        // Issue #1653 の実測: 修正前は `let item = item::new();`（型名まで書き換わる）
        let mut buffer =
            TextBuffer::from_text(path("u1653-value"), "let value = Value::new();".into());
        assert_eq!(
            buffer.replace_all("value", "item", SearchOptions::DEFAULT),
            1
        );
        assert_eq!(buffer.text(), "let item = Value::new();");
    }

    #[test]
    fn 区別と単語単位の4通りで件数と置換結果が決まる_1653() {
        // 境界に置いたもの: 型名・全大文字・接尾（values）・接頭（_value）・漢字の直後・
        // かぎ括弧の中・小文字化で伸びる İ（2 → 3 バイト）の後ろ・縮む ẞ（3 → 2 バイト）の直後
        let text =
            "let value = Value::new(); // VALUE values _value 値value 「value」 İ value ẞvalue";
        let expected: [(&str, usize, &str); 4] = [
            (
                "区別する・単語単位なし",
                7,
                "let item = Value::new(); // VALUE items _item 値item 「item」 İ item ẞitem",
            ),
            (
                "区別する・単語単位",
                3,
                "let item = Value::new(); // VALUE values _value 値value 「item」 İ item ẞvalue",
            ),
            (
                "区別しない・単語単位なし",
                9,
                "let item = item::new(); // item items _item 値item 「item」 İ item ẞitem",
            ),
            (
                "区別しない・単語単位",
                5,
                "let item = item::new(); // item values _value 値value 「item」 İ item ẞvalue",
            ),
        ];
        for ((name, options), (expected_name, count, replaced)) in
            matrix().into_iter().zip(expected)
        {
            assert_eq!(name, expected_name);
            let mut buffer = TextBuffer::from_text(path("u1653-matrix"), text.into());
            let hits = buffer.find_all("value", options);
            assert_eq!(hits.len(), count, "{name}: 件数");
            for hit in &hits {
                // 位置は元テキスト基準で、当たった部分は大文字小文字を無視すれば value
                assert!(text.is_char_boundary(hit.start) && text.is_char_boundary(hit.end));
                assert_eq!(text[hit.start..hit.end].to_lowercase(), "value", "{name}");
            }
            assert_eq!(
                buffer.replace_all("value", "item", options),
                count,
                "{name}"
            );
            assert_eq!(buffer.text(), replaced, "{name}: 置換結果");
            // 全置換は undo 1 回で戻る（#1651 の差分 1 件）
            assert!(buffer.undo());
            assert_eq!(buffer.text(), text, "{name}: undo");
        }
    }

    #[test]
    fn 長さが変わる文字は区別の有無で当たり方が変わる_1653() {
        // トルコ語の İ は小文字化で i + U+0307（2 → 3 バイト）。区別しない検索でも
        // `istanbul` は `İstanbul` に当たらない（写しは `i̇stanbul` で、間に U+0307 が挟まる）
        let text = "İstanbul istanbul ISTANBUL";
        let buffer = TextBuffer::from_text(path("u1653-turkish"), text.into());
        let at = |needle: &str| {
            text.match_indices(needle)
                .map(|(i, m)| (i, i + m.len()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            spans(&buffer.find_all("istanbul", SearchOptions::DEFAULT)),
            at("istanbul")
        );
        let mut ignore = at("istanbul");
        ignore.extend(at("ISTANBUL"));
        assert_eq!(spans(&buffer.find_all("istanbul", IGNORE_CASE)), ignore);
        // İ そのものをクエリにすれば、区別する / しないのどちらでも元の 1 語に当たる
        for options in [SearchOptions::DEFAULT, IGNORE_CASE] {
            assert_eq!(
                spans(&buffer.find_all("İstanbul", options)),
                vec![(0, "İstanbul".len())]
            );
        }
        // 1 文字の i は区別しないとき İ の途中（展開の途中）に当たらない = panic しない
        let hits = buffer.find_all("i", IGNORE_CASE);
        for hit in &hits {
            assert!(text.is_char_boundary(hit.start) && text.is_char_boundary(hit.end));
        }
        assert!(
            hits.iter().all(|h| h.start != 0),
            "İ（位置 0）には当たらない"
        );
        // 縮む ẞ（3 → 2 バイト）: 区別しないと ß に当たり、位置は元テキスト基準
        let sharp = TextBuffer::from_text(path("u1653-sharp"), "ẞ ß".into());
        assert_eq!(
            spans(&sharp.find_all("ß", SearchOptions::DEFAULT)),
            vec![(4, 6)]
        );
        assert_eq!(
            spans(&sharp.find_all("ß", IGNORE_CASE)),
            vec![(0, 3), (4, 6)]
        );
    }

    #[test]
    fn 単語単位は日本語の境界で切れない_1653() {
        // 漢字・かなは単語の文字（識別子に使える）。空白・句読点・括弧だけが区切り
        let text = "東京都 東京 東京タワー、東京。";
        let buffer = TextBuffer::from_text(path("u1653-jp-word"), text.into());
        let whole = SearchOptions::DEFAULT.with(None, Some(true));
        assert_eq!(buffer.find_all("東京", SearchOptions::DEFAULT).len(), 4);
        let hits = buffer.find_all("東京", whole);
        let second = text.find(" 東京 ").unwrap() + 1;
        let fourth = text.find("、東京").unwrap() + "、".len();
        assert_eq!(
            spans(&hits),
            vec![
                (second, second + "東京".len()),
                (fourth, fourth + "東京".len())
            ]
        );
    }

    #[test]
    fn 一文字と記号で始まるクエリの単語単位_1653() {
        let buffer = TextBuffer::from_text(path("u1653-one"), "a A aa a_".into());
        let counts: Vec<usize> = matrix()
            .iter()
            .map(|(_, o)| buffer.find_all("a", *o).len())
            .collect();
        // 区別する: a / aa の 2 文字 / a_ = 4、単語単位は先頭の a だけ = 1、
        // 区別しない: A も足して 5、単語単位は a と A = 2
        assert_eq!(counts, vec![4, 1, 5, 2]);
        // 端が単語の文字でないクエリは、その側の隣を問わない（`.foo` は `x.foo` に当たる）
        let dotted = TextBuffer::from_text(path("u1653-dot"), "x.foo x.foobar".into());
        let whole = SearchOptions::DEFAULT.with(None, Some(true));
        assert_eq!(spans(&dotted.find_all(".foo", whole)), vec![(1, 5)]);
        // 外れた候補の後ろから探し直す（xfoo の中の foo で探索を打ち切らない）
        let retry = TextBuffer::from_text(path("u1653-retry"), "xfoo foo".into());
        assert_eq!(spans(&retry.find_all("foo", whole)), vec![(5, 8)]);
    }

    #[test]
    fn 区別する検索は本文の部分文字列探索と一致する_1653() {
        let text = "fn a() { a(); aa(); }\n// ａ全角 a\tA\r\na";
        let buffer = TextBuffer::from_text(path("u1653-plain"), text.into());
        for query in ["a", "a(", "aa", "ａ", "\r\n", "} ", "A"] {
            let expected: Vec<(usize, usize)> = text
                .match_indices(query)
                .map(|(i, m)| (i, i + m.len()))
                .collect();
            assert_eq!(
                spans(&buffer.find_all(query, SearchOptions::DEFAULT)),
                expected,
                "{query:?}"
            );
        }
    }

    #[test]
    fn 検索欄の打鍵では小文字写しを作り直さない_1653() {
        // Issue #1653 の実測（release・1 MB）: 修正前は検索欄の 1 打鍵ごとに全文の写しを
        // 2 回作っていた（1 打鍵 4.4ms）。時間ではなく「作った回数」で固定する
        let line = "    let value = Value::new(); // コメント İ\n";
        let mut text = String::new();
        while text.len() < 1024 * 1024 {
            text.push_str(line);
        }
        let lines = text.len() / line.len();
        let mut buffer = TextBuffer::from_text(path("u1653-fold"), text);
        let query = "Value::new";
        // 検索欄へ 1 文字ずつ打つ = クエリが 1 文字ずつ伸びる
        let type_query = |buffer: &TextBuffer, options: SearchOptions| {
            let mut last = 0;
            for n in 1..=query.len() {
                last = buffer.find_all(&query[..n], options).len();
            }
            last
        };
        // 区別する（既定）: 写しは 1 度も作らない
        assert_eq!(type_query(&buffer, SearchOptions::DEFAULT), lines);
        assert_eq!(buffer.search_fold_builds(), 0, "区別する検索が写しを作った");
        // 区別しない: 打鍵 10 回 × 2 往復でも写しは 1 回だけ
        assert_eq!(type_query(&buffer, IGNORE_CASE), lines);
        assert_eq!(type_query(&buffer, IGNORE_CASE), lines);
        assert_eq!(
            buffer.search_fold_builds(),
            1,
            "打鍵のたびに写しを作り直した"
        );
        // 本文が変わったら作り直す（古い写しで探すとヒットの位置がずれる）
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.insert("VALUE::NEW");
        assert_eq!(buffer.find_all(query, IGNORE_CASE).len(), lines + 1);
        assert_eq!(buffer.search_fold_builds(), 2);
        // undo / redo も本文を変えるので捨てる
        assert!(buffer.undo());
        assert_eq!(buffer.find_all(query, IGNORE_CASE).len(), lines);
        assert!(buffer.redo());
        assert_eq!(buffer.find_all(query, IGNORE_CASE).len(), lines + 1);
        assert_eq!(buffer.search_fold_builds(), 4);
        // カーソルが動いただけ（本文は同じ）なら作り直さない
        buffer.set_cursor(0, false);
        assert_eq!(buffer.find_all(query, IGNORE_CASE).len(), lines + 1);
        assert_eq!(buffer.search_fold_builds(), 4);
        // 検索欄を閉じたら手放し、次に区別しない検索をしたときにだけ作る
        buffer.release_search_cache();
        assert_eq!(buffer.find_all(query, SearchOptions::DEFAULT).len(), lines);
        assert_eq!(buffer.search_fold_builds(), 4);
        assert_eq!(buffer.find_all(query, IGNORE_CASE).len(), lines + 1);
        assert_eq!(buffer.search_fold_builds(), 5);
    }

    #[test]
    fn 一件置換はカーソル以降の最初を置き換えてラップする_1653() {
        let mut buffer = TextBuffer::from_text(path("u1653-next"), "foo Foo foo".into());
        buffer.set_cursor(1, false);
        assert_eq!(buffer.replace_next("foo", "bar", SearchOptions::DEFAULT), 1);
        assert_eq!(buffer.text(), "foo Foo bar", "区別するので Foo は飛ばす");
        // カーソル（bar の後ろ）より後ろに無いので先頭へラップ
        assert_eq!(buffer.replace_next("foo", "bar", SearchOptions::DEFAULT), 1);
        assert_eq!(buffer.text(), "bar Foo bar");
        assert_eq!(buffer.replace_next("foo", "bar", SearchOptions::DEFAULT), 0);
        assert_eq!(buffer.replace_next("foo", "bar", IGNORE_CASE), 1);
        assert_eq!(buffer.text(), "bar bar bar");
        assert_eq!(
            buffer.replace_next("", "x", IGNORE_CASE),
            0,
            "空クエリは何もしない"
        );
    }

    #[test]
    fn 条件の決め方は新しいクエリなら既定から組み直す_1653() {
        let ignore_whole = SearchOptions {
            case_sensitive: false,
            whole_word: true,
        };
        // 新しいクエリ: 省略した項目は既定（今の条件を引き継がない）
        assert_eq!(
            SearchOptions::resolve(ignore_whole, true, None, None),
            SearchOptions::DEFAULT
        );
        assert_eq!(
            SearchOptions::resolve(SearchOptions::DEFAULT, true, Some(false), None),
            IGNORE_CASE
        );
        // クエリ省略（次へ / 前へ・条件だけ変える）: 今の条件を引き継ぐ
        assert_eq!(
            SearchOptions::resolve(ignore_whole, false, None, None),
            ignore_whole
        );
        assert_eq!(
            SearchOptions::resolve(ignore_whole, false, None, Some(false)),
            IGNORE_CASE
        );
    }

    // --- #1650: 改行コード（LF / CRLF）の検出・保持 ---

    /// 往復保存の実測を 1 か所で取る（開く → 行末 → Enter → 文字 → 保存）。
    /// 戻り値は保存後の生バイト
    fn crlf_round_trip(name: &str, original: &[u8], go_to_line: usize) -> Vec<u8> {
        let path = path(name);
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, original).unwrap();
        let mut buffer = TextBuffer::open(&path).unwrap();
        for _ in 0..go_to_line {
            buffer.move_cursor(CursorMovement::Down, false);
        }
        buffer.move_cursor(CursorMovement::LineEnd, false);
        buffer.newline();
        buffer.insert("X");
        buffer.save().unwrap();
        let saved = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(path);
        saved
    }

    #[test]
    fn crlfファイルの行末はcrの手前へ止まる() {
        // HL3 の再現: `line_end` が `\n` の位置を返すと、カーソルが CR の後ろ
        // （画面上の行末より右）に来る。そこで打つと `abc\r!\n` になる
        let mut buffer = TextBuffer::from_text(path("crlf-end"), "abc\r\ndef\r\n".into());
        buffer.move_cursor(CursorMovement::LineEnd, false);
        assert_eq!(buffer.cursor(), 3, "行末は CR の手前（見た目の行末）");
        buffer.insert("!");
        assert_eq!(buffer.text(), "abc!\r\ndef\r\n");
    }

    #[test]
    fn crlfファイルの往復編集で改行コードが保たれる() {
        // E3 の再現: Enter が常に `\n` を挿すと CR 2 / LF 3 の混在になる
        let saved = crlf_round_trip("crlf-round", b"abc\r\ndef\r\n", 0);
        assert_eq!(saved, b"abc\r\nX\r\ndef\r\n");
        let cr = saved.iter().filter(|b| **b == b'\r').count();
        let lf = saved.iter().filter(|b| **b == b'\n').count();
        assert_eq!((cr, lf), (3, 3), "CRLF ファイルは CR の数 = LF の数");
    }

    #[test]
    fn lfファイルの往復編集でcrが混ざらない() {
        let saved = crlf_round_trip("lf-round", b"abc\ndef\n", 0);
        assert_eq!(saved, b"abc\nX\ndef\n");
        assert_eq!(saved.iter().filter(|b| **b == b'\r').count(), 0);
    }

    #[test]
    fn 混在ファイルは既存の改行を残し新しい行だけ多数派になる() {
        // `\r\n` が 2 / 裸の `\n` が 1 なので多数派は CRLF。
        // 2 行目（`b\n`）の行末で Enter を打っても、既存の `\n` は書き換えない
        let saved = crlf_round_trip("mixed-round", b"a\r\nb\nc\r\n", 1);
        assert_eq!(saved, b"a\r\nb\r\nX\nc\r\n");
    }

    #[test]
    fn 編集しなければどの改行コードでもバイト一致で保存される() {
        for (name, original) in [
            ("noop-crlf", &b"a\r\nb\r\n"[..]),
            ("noop-lf", &b"a\nb\n"[..]),
            ("noop-mixed", &b"a\r\nb\nc\r\n"[..]),
            ("noop-no-eol", &b"a\r\nb"[..]),
        ] {
            let path = path(name);
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, original).unwrap();
            let mut buffer = TextBuffer::open(&path).unwrap();
            assert!(!buffer.dirty(), "{name}: 開いただけで dirty にならない");
            buffer.save().unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                original,
                "{name}: バイト一致"
            );
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn 末尾改行の有無が往復で保たれる() {
        // 末尾に改行が無いファイルは、行末 Enter → 文字入力のあとも末尾改行なしのまま
        let saved = crlf_round_trip("no-eol-crlf", b"abc\r\ndef", 1);
        assert_eq!(saved, b"abc\r\ndef\r\nX");
        let saved = crlf_round_trip("no-eol-lf", b"abc\ndef", 1);
        assert_eq!(saved, b"abc\ndef\nX");
    }

    #[test]
    fn 改行コードの検出は多数派を選び改行が無ければ既定へ倒れる() {
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), Some(LineEnding::Crlf));
        assert_eq!(LineEnding::detect("a\nb\n"), Some(LineEnding::Lf));
        assert_eq!(LineEnding::detect("a\r\nb\nc\r\n"), Some(LineEnding::Crlf));
        assert_eq!(LineEnding::detect("a\r\nb\nc\n"), Some(LineEnding::Lf));
        // 同数は LF（移植性の高い側へ倒す）
        assert_eq!(LineEnding::detect("a\r\nb\n"), Some(LineEnding::Lf));
        assert_eq!(LineEnding::detect("abc"), None);
        assert_eq!(LineEnding::detect(""), None);
        // 改行を 1 つも持たないファイルはそのプラットフォームの流儀
        let buffer = TextBuffer::from_text(path("no-newline"), "abc".into());
        assert_eq!(buffer.line_ending(), LineEnding::platform_default());
    }

    #[test]
    fn 新規ファイルの改行コードはプラットフォームで決まる() {
        // **macOS 上からでも Windows の腕を固定する**（#1650。実機が無いと
        // 「Windows でどうなるか」をテストできず、CI だけが落ちる形になる）
        assert_eq!(LineEnding::for_new_file(Platform::MacOs), LineEnding::Lf);
        assert_eq!(
            LineEnding::for_new_file(Platform::Windows),
            LineEnding::Crlf
        );
        // 実行中の既定はこの純関数と同じ答えでなければならない
        assert_eq!(
            LineEnding::platform_default(),
            LineEnding::for_new_file(Platform::current())
        );
        // 改行を持たない本文へ入る `\n` は、その腕どおりに揃う
        for platform in [Platform::MacOs, Platform::Windows] {
            let ending = LineEnding::for_new_file(platform);
            assert_eq!(
                normalize_line_endings("a\nb", ending),
                format!("a{}b", ending.as_str()),
                "{} の新規ファイルへ入る改行",
                platform.as_str()
            );
        }
    }

    #[test]
    fn crlfバッファでは行区切りをひとまとまりとして越える() {
        let mut buffer = TextBuffer::from_text(path("crlf-move"), "abc\r\ndef".into());
        buffer.set_cursor(3, false);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), 5, "CR と LF のあいだで止まらない");
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!(buffer.cursor(), 3);
        // 外から CR と LF のあいだを指されても見た目の行末へ丸める
        buffer.set_cursor(4, false);
        assert_eq!(buffer.cursor(), 3);
        assert_eq!(buffer.offset_for_line_byte_col(0, 99), 3);
    }

    #[test]
    fn crlfバッファの削除は行区切りをひとまとまりで消す() {
        let mut buffer = TextBuffer::from_text(path("crlf-del-fwd"), "abc\r\ndef".into());
        buffer.set_cursor(3, false);
        buffer.delete_forward();
        assert_eq!(buffer.text(), "abcdef", "前方削除が裸の CR を残さない");

        let mut buffer = TextBuffer::from_text(path("crlf-del-back"), "abc\r\ndef".into());
        buffer.set_cursor(5, false);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "abcdef", "後方削除が裸の CR を残さない");
    }

    #[test]
    fn 挿入テキストの改行はバッファの改行コードへ揃う() {
        let mut crlf = TextBuffer::from_text(path("crlf-paste"), "a\r\n".into());
        crlf.move_cursor(CursorMovement::DocumentEnd, false);
        crlf.insert("x\ny");
        assert_eq!(crlf.text(), "a\r\nx\r\ny", "裸の LF が混ざらない");
        crlf.insert("\r\nz");
        assert_eq!(
            crlf.text(),
            "a\r\nx\r\ny\r\nz",
            "既に CRLF なら二重化しない"
        );

        let mut lf = TextBuffer::from_text(path("lf-paste"), "a\n".into());
        lf.move_cursor(CursorMovement::DocumentEnd, false);
        lf.insert("x\r\ny");
        assert_eq!(lf.text(), "a\nx\ny", "LF ファイルへ CR を持ち込まない");
    }

    #[test]
    fn 置換の改行もバッファの改行コードへ揃う() {
        let mut buffer = TextBuffer::from_text(path("crlf-replace"), "a\r\nQ\r\n".into());
        assert_eq!(buffer.replace_all("q", "1\n2", IGNORE_CASE), 1);
        assert_eq!(buffer.text(), "a\r\n1\r\n2\r\n");

        let mut buffer = TextBuffer::from_text(path("crlf-replace-range"), "a\r\nQ\r\n".into());
        buffer.replace_range(3..4, "1\n2");
        assert_eq!(buffer.text(), "a\r\n1\r\n2\r\n");
    }

    #[test]
    fn set_textは渡された本文から改行コードを取り直す() {
        let mut buffer = TextBuffer::from_text(path("set-text-eol"), "a\r\nb\r\n".into());
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        buffer.set_text("a\nb\n".into());
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
        buffer.newline();
        assert_eq!(buffer.text(), "a\nb\n\n");
        // 改行を持たない本文では直前の改行コードを保つ
        buffer.set_text("abc".into());
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
    }

    #[test]
    fn crlfバッファでも上下移動が見た目の行内へ収まる() {
        let mut buffer = TextBuffer::from_text(path("crlf-vertical"), "abcdef\r\ngh\r\nij".into());
        buffer.move_cursor(CursorMovement::LineEnd, false);
        assert_eq!(buffer.cursor(), 6);
        buffer.move_cursor(CursorMovement::Down, false);
        // 2 行目は 2 文字しかないので行末（CR の手前）で止まる
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 2));
        buffer.insert("!");
        assert_eq!(buffer.text(), "abcdef\r\ngh!\r\nij");
    }

    #[test]
    fn 行頭行末とカーソル位置がcrを行内文字に数えない() {
        let buffer = TextBuffer::from_text(path("crlf-col"), "abc\r\ndef\r\n".into());
        // 2 行目の先頭は LF の次
        assert_eq!(buffer.line_byte_col(5), (1, 0));
        // 2 行目の行末（CR の手前）は列 3
        assert_eq!(buffer.line_byte_col(8), (1, 3));
        assert_eq!(buffer.offset_for_line_byte_col(1, 3), 8);
    }

    #[test]
    fn 日本語を含む検索と置換が正しいバイト位置で動く() {
        let mut buffer = TextBuffer::from_text(path("edge-jp-search"), "あいうえお".into());
        let hits = buffer.find_all("うえ", SearchOptions::DEFAULT);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start, "あい".len());
        assert_eq!(hits[0].end, "あいうえ".len());
        buffer.replace_range(hits[0].start..hits[0].end, "カキ");
        assert_eq!(buffer.text(), "あいカキお");
    }

    // --- #1651: undo 履歴は差分・連続タイプは 1 塊・上限はバイトでも効く ---

    /// まとめを検査するバッファ（時計を固定して作る。#1651）
    fn coalescing(name: &str, text: &str) -> TextBuffer {
        let mut buffer = TextBuffer::from_text(path(name), text.into());
        buffer.set_clock_millis(0);
        buffer
    }

    #[test]
    fn 連続して打った文字はundo1回で消える() {
        let mut buffer = coalescing("coalesce-type", "");
        for ch in "hello".chars() {
            buffer.insert(&ch.to_string());
        }
        assert_eq!(buffer.text(), "hello");
        assert_eq!(buffer.undo_depth(), 1, "5 打鍵が 1 塊になっていない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "", "undo 1 回で hello が消えない");
        assert!(!buffer.can_undo());
        // redo も 1 回で戻る
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "hello");
    }

    #[test]
    fn gui打鍵経路の選択写しはundoの塊を切らない() {
        // GUI は 1 打鍵ごとに「画面の選択をバッファへ写す → 挿す → 位置を書き戻す」を回す
        // （`tako-app` の `sync_editor_selection_from_preview` / `..._from_editor`）。
        // 写しは同じ位置を指すので、ここで塊が切れると **GUI だけ 1 文字粒度**へ戻る
        let mut buffer = coalescing("gui-keystroke", "");
        for ch in "hello".chars() {
            // 写し（キャレットなので anchor = head = 現在位置）
            let (line, col) = buffer.line_byte_col(buffer.cursor());
            let offset = buffer.offset_for_line_byte_col(line, col);
            buffer.set_cursor(offset, false);
            buffer.set_cursor(offset, true);
            buffer.insert(&ch.to_string());
        }
        assert_eq!(buffer.text(), "hello");
        assert_eq!(
            buffer.undo_depth(),
            1,
            "選択の写しが塊を切っている（GUI だけ 1 文字粒度に戻る）"
        );
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "");
    }

    #[test]
    fn 同じ位置へのset_cursorは選択を畳んでも塊を切らない() {
        let mut buffer = coalescing("cursor-noop", "abcdef");
        buffer.set_cursor(3, false);
        buffer.insert("X");
        // 位置も選択範囲も変わらない呼び出し
        buffer.set_cursor(4, false);
        buffer.insert("Y");
        assert_eq!(buffer.text(), "abcXYdef");
        assert_eq!(buffer.undo_depth(), 1);
        // 位置が 1 バイトでも動けば切れる
        buffer.set_cursor(3, false);
        buffer.insert("Z");
        assert_eq!(buffer.undo_depth(), 2);
    }

    #[test]
    fn カーソル移動でundoの塊が切れる() {
        let mut buffer = coalescing("coalesce-cursor", "");
        buffer.insert("ab");
        buffer.move_cursor(CursorMovement::Left, false);
        buffer.insert("X");
        assert_eq!(buffer.text(), "aXb");
        assert_eq!(buffer.undo_depth(), 2, "カーソル移動が塊を切っていない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "ab");
    }

    #[test]
    fn 改行でundoの塊が切れる() {
        let mut buffer = coalescing("coalesce-newline", "");
        buffer.insert("ab");
        buffer.newline();
        buffer.insert("cd");
        let eol = buffer.line_ending().as_str();
        assert_eq!(buffer.text(), format!("ab{eol}cd"));
        // 「ab」「改行」「cd」の 3 塊
        assert_eq!(buffer.undo_depth(), 3, "改行が塊を切っていない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), format!("ab{eol}"));
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "ab");
    }

    #[test]
    fn 時間が空くとundoの塊が切れる() {
        let mut buffer = coalescing("coalesce-time", "");
        buffer.insert("a");
        // 窓の内側はまとまる
        buffer.set_clock_millis(COALESCE_WINDOW_MS);
        buffer.insert("b");
        assert_eq!(buffer.undo_depth(), 1, "窓の内側でまとまっていない");
        // 窓を 1ms 超えると切れる
        buffer.set_clock_millis(COALESCE_WINDOW_MS * 2 + 1);
        buffer.insert("c");
        assert_eq!(buffer.undo_depth(), 2, "窓の外でも塊が続いている");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "ab");
    }

    #[test]
    fn 種別が変わるとundoの塊が切れる() {
        let mut buffer = coalescing("coalesce-kind", "");
        buffer.insert("abc");
        buffer.delete_backward();
        assert_eq!(buffer.text(), "ab");
        assert_eq!(buffer.undo_depth(), 2, "挿入と削除が同じ塊になっている");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abc");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "");
    }

    #[test]
    fn 連続したbackspaceとdeleteはそれぞれ1塊になる() {
        let mut buffer = coalescing("coalesce-delete", "abcdef");
        buffer.set_cursor(3, false);
        buffer.delete_backward();
        buffer.delete_backward();
        assert_eq!(buffer.text(), "adef");
        assert_eq!(buffer.undo_depth(), 1, "連続 Backspace が 1 塊でない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abcdef");
        assert_eq!(buffer.cursor(), 3, "undo でカーソルが編集前へ戻らない");

        let mut buffer = coalescing("coalesce-delete-fwd", "abcdef");
        buffer.set_cursor(1, false);
        buffer.delete_forward();
        buffer.delete_forward();
        assert_eq!(buffer.text(), "adef");
        assert_eq!(buffer.undo_depth(), 1, "連続 Delete が 1 塊でない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abcdef");
    }

    #[test]
    fn 保存でundoの塊が切れる() {
        let file = path("coalesce-save");
        let _ = std::fs::remove_file(&file);
        std::fs::write(&file, "").unwrap();
        let mut buffer = TextBuffer::open(&file).unwrap();
        buffer.set_clock_millis(0);
        buffer.insert("ab");
        buffer.save().unwrap();
        buffer.insert("cd");
        assert_eq!(buffer.text(), "abcd");
        assert_eq!(buffer.undo_depth(), 2, "保存が塊を切っていない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "ab", "保存した姿まで戻れない");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn 選択の差し替えはまとめずに1回で戻る() {
        let mut buffer = coalescing("coalesce-replace", "abcdef");
        buffer.set_cursor(1, false);
        buffer.set_cursor(4, true);
        buffer.insert("X");
        buffer.insert("Y");
        assert_eq!(buffer.text(), "aXYef");
        // 選択の差し替え（Replace）と続く打鍵（Insert）は別の塊
        assert_eq!(buffer.undo_depth(), 2);
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "aXef");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abcdef");
    }

    #[test]
    fn 複数行の貼り付けはundo1回で戻る() {
        let mut buffer = coalescing("paste-multiline", "head\n");
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.insert("1\n2\n3\n");
        assert_eq!(buffer.text(), "head\n1\n2\n3\n");
        assert_eq!(buffer.undo_depth(), 1, "貼り付け 1 回が 1 塊でない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "head\n");
    }

    #[test]
    fn 空ファイルへの最初の打鍵をundoできる() {
        let mut buffer = coalescing("first-keystroke", "");
        assert!(!buffer.can_undo());
        buffer.insert("あ");
        assert_eq!(buffer.undo_depth(), 1);
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "");
        assert!(!buffer.can_undo());
        assert!(buffer.can_redo());
    }

    #[test]
    fn undoの途中で新しい編集をするとredoが捨てられる() {
        let mut buffer = coalescing("redo-discard", "");
        buffer.insert("ab");
        buffer.seal_undo_group();
        buffer.insert("cd");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "ab");
        assert!(buffer.can_redo());
        buffer.insert("Z");
        assert!(!buffer.can_redo(), "新しい編集で redo が捨てられていない");
        assert_eq!(buffer.text(), "abZ");
        // 履歴の勘定にも redo の残骸が残らない
        assert!(buffer.undo());
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "");
    }

    #[test]
    fn 千打鍵の履歴が全文のスナップショットより桁で小さい() {
        // 1 MB のバッファ（Issue の実測条件そのまま）
        let text = "0123456789abcdef".repeat(65_536);
        assert_eq!(text.len(), 1_048_576);
        let mut buffer = coalescing("history-bytes", &text);
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        // **塊を毎回切る = まとめの効果を除いた最悪値**を測る
        for i in 0..1_000 {
            buffer.seal_undo_group();
            buffer.insert(if i % 7 == 0 { "b" } else { "a" });
        }
        assert_eq!(buffer.undo_depth(), 1_000);
        let bytes = buffer.undo_history_bytes();
        // 全文スナップショットなら 1000 × 1 MB = 1 GB 超（#1651 の症状）。
        // 差分なら 1 件あたり数十バイト。**実時間ではなく状態値で固定する**
        assert!(
            bytes < 1024 * 1024,
            "1000 打鍵の履歴が 1 MiB を超えている: {bytes} バイト"
        );
        // まとめが効けば 1 塊まで落ちる
        let mut merged = coalescing("history-bytes-merged", &text);
        merged.move_cursor(CursorMovement::DocumentEnd, false);
        for _ in 0..1_000 {
            merged.insert("a");
        }
        assert_eq!(merged.undo_depth(), 1);
        assert!(
            merged.undo_history_bytes() < 8 * 1024,
            "まとめた 1000 打鍵の履歴が 8 KiB を超えている: {} バイト",
            merged.undo_history_bytes()
        );
    }

    #[test]
    fn 履歴のバイト上限が操作数より先に効く() {
        // 1 操作で 2 MiB（置換前 + 置換後）を積む編集を 8 回 = 16 MiB > 8 MiB
        let a = "a".repeat(1024 * 1024);
        let b = "b".repeat(1024 * 1024);
        let mut buffer = coalescing("history-budget", &a);
        for i in 0..8 {
            buffer.set_text(if i % 2 == 0 { b.clone() } else { a.clone() });
        }
        // 操作数の上限（1000）には遠く届かないのに、履歴は予算内に収まっている
        assert!(buffer.undo_depth() < 8, "バイト上限が効いていない");
        assert!(
            buffer.undo_history_bytes() <= UNDO_BYTE_LIMIT,
            "履歴が予算を超えている: {} バイト",
            buffer.undo_history_bytes()
        );
        // 直前の操作は必ず取り消せる
        assert!(buffer.undo());
        assert_eq!(buffer.text().len(), a.len());
    }

    #[test]
    fn 単独で予算を超える編集でも直前の1件は残る() {
        let huge = "x".repeat(UNDO_BYTE_LIMIT + 1);
        let mut buffer = coalescing("history-single-huge", "small");
        buffer.set_text(huge.clone());
        assert_eq!(buffer.undo_depth(), 1, "1 件も残らないと undo できない");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "small");
    }

    #[test]
    fn crlfファイルの挿入をundoredoしても既存行の改行が変わらない() {
        let original = "l1\r\nl2\r\n日本\r\n";
        let mut buffer = coalescing("crlf-undo", original);
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        buffer.set_cursor(2, false);
        buffer.insert("X"); // 1 塊目
        buffer.newline(); // 2 塊目（改行は塊を切る）
        buffer.delete_backward(); // 3 塊目（種別が変わる）
        assert_eq!(buffer.undo_depth(), 3);
        for _ in 0..3 {
            assert!(buffer.undo());
        }
        assert_eq!(
            buffer.text().as_bytes(),
            original.as_bytes(),
            "undo で既存行の改行が書き換わった"
        );
        // redo で戻した先も CRLF のまま
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "l1X\r\nl2\r\n日本\r\n");
        assert!(buffer.redo());
        assert_eq!(buffer.text(), "l1X\r\n\r\nl2\r\n日本\r\n");
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
    }

    #[test]
    fn set_textのundoで改行コードも戻る() {
        let mut buffer = coalescing("undo-eol", "a\r\nb\r\n");
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        buffer.set_text("a\nb\n".into());
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
        assert!(buffer.undo());
        assert_eq!(
            buffer.line_ending(),
            LineEnding::Crlf,
            "undo で改行コードが戻らないと、次の Enter が別の改行を挿す（#1650）"
        );
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.newline();
        assert_eq!(buffer.text(), "a\r\nb\r\n\r\n");
    }

    /// 固定シードの疑似乱数（外部クレートを増やさない。xorshift64*）
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn ランダムな編集列をundoで全部戻すと元の本文とバイト一致する() {
        // CRLF・LF・空・日本語をすべて通す（改行コードは #1650 の契約）
        let seeds = ["", "abc\ndef\n", "a日本語\nz", "l1\r\nl2\r\n日本\r\n"];
        for (i, original) in seeds.iter().enumerate() {
            let mut rng = Rng(0x1651_0000 + i as u64);
            let mut buffer = TextBuffer::from_text(path("prop"), (*original).into());
            for step in 0..200 {
                // 時計も決定的に進める（窓を跨ぐ回と跨がない回の両方を通す）
                buffer.set_clock_millis((step as u64) * 137);
                let len = buffer.text().len();
                let pos = if len == 0 { 0 } else { rng.below(len + 1) };
                match rng.below(8) {
                    0 => buffer.insert("a"),
                    1 => buffer.insert("日本\n語"),
                    2 => {
                        buffer.set_cursor(pos, false);
                        buffer.delete_backward();
                    }
                    3 => {
                        buffer.set_cursor(pos, false);
                        buffer.delete_forward();
                    }
                    4 => {
                        // 選択して差し替え
                        buffer.set_cursor(pos, false);
                        let end = if len == 0 { 0 } else { rng.below(len + 1) };
                        buffer.set_cursor(end, true);
                        buffer.insert("Z\r\nZ");
                    }
                    5 => {
                        buffer.select_all();
                        buffer.insert("全選択差し替え\n2 行目");
                    }
                    6 => {
                        let _ = buffer.replace_all("z", "ZZ", IGNORE_CASE);
                    }
                    _ => {
                        buffer.newline();
                    }
                }
            }
            let edited = buffer.text().to_string();
            let mut undone = 0;
            while buffer.undo() {
                undone += 1;
            }
            assert!(undone > 0, "seed {i}: 1 件も undo できていない");
            assert_eq!(
                buffer.text().as_bytes(),
                original.as_bytes(),
                "seed {i}: undo で元の本文へ戻らない（{undone} 件戻した）"
            );
            let mut redone = 0;
            while buffer.redo() {
                redone += 1;
            }
            assert_eq!(redone, undone, "seed {i}: redo の回数が undo と合わない");
            assert_eq!(
                buffer.text().as_bytes(),
                edited.as_bytes(),
                "seed {i}: redo で編集後の本文へ戻らない"
            );
            // 2 周目も同じ（状態が壊れていない）
            while buffer.undo() {}
            assert_eq!(buffer.text().as_bytes(), original.as_bytes());
        }
    }

    // --- #1660: 行頭索引 ---

    /// 行・桁の素朴な換算（索引を使わない正解）
    fn naive_line_byte_col(text: &str, offset: usize) -> (usize, usize) {
        let prefix = &text[..offset];
        let line = prefix.bytes().filter(|b| *b == b'\n').count();
        let start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
        (line, offset - start)
    }

    /// 行頭索引が本文から作り直したものと一致し、行・桁の換算が素朴な数え方と
    /// 一致することを確かめる（#1660）
    fn assert_line_index(buffer: &TextBuffer, rng: &mut Rng, label: &str) {
        assert_eq!(
            buffer.line_starts(),
            compute_line_starts(buffer.text()).as_slice(),
            "{label}: 行頭索引が本文とずれた"
        );
        assert_eq!(
            buffer.line_count(),
            buffer.text().bytes().filter(|b| *b == b'\n').count() + 1,
            "{label}: 行数"
        );
        // 行・桁の換算は素朴な数え方と一致する（文字の途中は手前の境界へ寄る）
        let len = buffer.text().len();
        let probe = if len == 0 { 0 } else { rng.below(len + 1) };
        let snapped = snap_boundary(buffer.text(), probe);
        let (line, col) = buffer.line_byte_col(probe);
        assert_eq!(
            (line, col),
            naive_line_byte_col(buffer.text(), snapped),
            "{label}: line_byte_col({probe})"
        );
        // CR と LF のあいだは行末（CR の手前）へ寄せる契約（#1650）なので往復から外す
        let between_crlf =
            buffer.text()[..snapped].ends_with('\r') && buffer.text()[snapped..].starts_with('\n');
        if !between_crlf {
            assert_eq!(
                buffer.offset_for_line_byte_col(line, col),
                snapped,
                "{label}: offset_for_line_byte_col({line}, {col})"
            );
        }
    }

    /// 検索条件の 4 通り（#1653: 大文字小文字の区別 × 単語単位）
    fn search_options(rng: &mut Rng) -> SearchOptions {
        SearchOptions {
            case_sensitive: rng.below(2) == 0,
            whole_word: rng.below(2) == 0,
        }
    }

    /// 本文を書き換える公開の口すべて（置換の 4 条件を含む）をランダムに混ぜても、
    /// 行頭索引は本文から作り直したものと常に一致する（#1660 × #1653 / #1654 / #1658 / #1742）。
    ///
    /// 構造の保証は番犬 2 本（#1651 = 本文を書き換えるのは `apply_edit` / `undo` / `redo` だけ・
    /// #1660 = その 3 つが索引を追従させる）で、ここは**実際の編集列**で確かめる。置換は
    /// 大文字小文字を畳むと長さが変わる字（`İ` = 2 バイト → `i̇` = 3 バイト・`ẞ` → `ß`）と
    /// 改行を含む置換文字列を混ぜ、ヒットの位置が元の本文のバイト位置へ正しく戻らないと
    /// 索引が本文とずれる形にしてある
    #[test]
    fn 行頭索引は置換を含むすべての書き換え経路のあとも本文から作り直したものと一致する() {
        let seeds = [
            "",
            "abc\ndef\n",
            "a日本語\nz",
            "l1\r\nl2\r\n日本\r\n",
            "\n\n\n",
            "let value = Value::new();\nVALUE value_x 値value\n",
            "İstanbul i̇ ẞtraße\r\nß SS İ\r\nVALUE\r\n",
            "\tfn a() {\n\t\tvalue\n\t}\n",
        ];
        const QUERIES: [&str; 7] = ["a", "value", "VALUE", "İ", "ẞ", "\n", "e\nV"];
        const REPLACEMENTS: [&str; 6] = ["", "\n", "x\r\ny", "İ\n", "Value", "\n\n改行\n"];
        const MOTIONS: [DeleteMotion; 6] = [
            DeleteMotion::CharBackward,
            DeleteMotion::CharForward,
            DeleteMotion::WordBackward,
            DeleteMotion::WordForward,
            DeleteMotion::ToLineStart,
            DeleteMotion::ToLineEnd,
        ];
        let mut replaced = 0usize;
        for (i, original) in seeds.iter().enumerate() {
            let mut rng = Rng(0x1660_0000 + i as u64);
            let mut buffer = TextBuffer::from_text(path("index"), (*original).into());
            for step in 0..600 {
                buffer.set_clock_millis((step as u64) * 97);
                let len = buffer.text().len();
                let pos = if len == 0 { 0 } else { rng.below(len + 1) };
                let query = QUERIES[rng.below(QUERIES.len())];
                let replacement = REPLACEMENTS[rng.below(REPLACEMENTS.len())];
                let op = rng.below(18);
                match op {
                    0 => buffer.insert("a"),
                    1 => buffer.insert("日本\n語\n"),
                    2 => {
                        buffer.set_cursor(pos, false);
                        buffer.delete_backward();
                    }
                    3 => {
                        buffer.set_cursor(pos, false);
                        buffer.delete_forward();
                    }
                    4 => {
                        let end = if len == 0 { 0 } else { rng.below(len + 1) };
                        buffer.set_selection(pos, end);
                        buffer.insert("Z\r\nZ\n");
                    }
                    5 => buffer.set_text("全文\n差し替え\r\n".into()),
                    // #1653: 全置換（4 条件）
                    6 | 7 => {
                        replaced +=
                            buffer.replace_all(query, replacement, search_options(&mut rng));
                    }
                    // #1653: 1 件置換（4 条件。カーソル以降の最初のヒット）
                    8 | 9 => {
                        buffer.set_cursor(pos, false);
                        replaced +=
                            buffer.replace_next(query, replacement, search_options(&mut rng));
                    }
                    10 => {
                        buffer.undo();
                    }
                    11 => {
                        buffer.redo();
                    }
                    12 => {
                        buffer.set_cursor(pos, false);
                        buffer.delete(MOTIONS[rng.below(MOTIONS.len())]);
                    }
                    13 => {
                        let end = if len == 0 { 0 } else { rng.below(len + 1) };
                        buffer.set_selection(pos, end);
                        if rng.below(2) == 0 {
                            buffer.indent();
                        } else {
                            buffer.outdent();
                        }
                    }
                    14 => {
                        buffer.set_cursor(pos, false);
                        buffer.newline_and_indent();
                    }
                    15 => {
                        let start = snap_boundary(buffer.text(), pos);
                        let end = snap_boundary(buffer.text(), rng.below(len + 1).max(start));
                        buffer.replace_range(start.min(end)..end.max(start), replacement);
                    }
                    // #1658: 行・桁で指す範囲編集（解けない指定は何もしないで拒否 = それも確かめる）
                    16 => {
                        let lines = buffer.line_count();
                        let edit = RangeEdit {
                            start: TextPosition::new(1 + rng.below(lines), rng.below(4)),
                            end: TextPosition::new(1 + rng.below(lines + 1), rng.below(6)),
                            text: replacement.into(),
                            expected_version: None,
                        };
                        let _ = buffer.replace_position_range(&edit);
                    }
                    _ => buffer.newline(),
                }
                let label = format!("seed {i} step {step} op {op} query {query:?}");
                assert_line_index(&buffer, &mut rng, &label);
            }
            // 最後に履歴を底まで戻し、また先頭まで進め直しても索引は本文と一致する
            let mut guard = 0;
            while buffer.undo() {
                guard += 1;
                assert!(guard < 10_000, "undo が止まらない");
                assert_line_index(&buffer, &mut rng, &format!("seed {i} undo {guard}"));
            }
            while buffer.redo() {
                assert_line_index(&buffer, &mut rng, &format!("seed {i} redo"));
            }
        }
        // 置換の経路を実際に通ったこと（当たらない列だけで緑になっていない）
        assert!(replaced > 100, "置換が {replaced} 件しか起きていない");
    }

    // --- #1658: 行・桁で指す範囲編集と文書の版 ---

    fn pos(line: usize, column: usize) -> TextPosition {
        TextPosition::new(line, column)
    }

    /// 範囲編集 1 回ぶんの指示（版の指定なし）
    fn range_edit(start: TextPosition, end: TextPosition, text: &str) -> RangeEdit {
        RangeEdit {
            start,
            end,
            text: text.into(),
            expected_version: None,
        }
    }

    #[test]
    fn 行桁は1始まりの行と0始まりのバイト桁で解ける() {
        let buffer = TextBuffer::from_text(path("resolve"), "abc\nあいう\n".into());
        assert_eq!(buffer.resolve_position(pos(1, 0)), Ok(0));
        assert_eq!(buffer.resolve_position(pos(1, 3)), Ok(3));
        // 2 行目は「あ」= 3 バイト単位
        assert_eq!(buffer.resolve_position(pos(2, 0)), Ok(4));
        assert_eq!(buffer.resolve_position(pos(2, 6)), Ok(10));
        // 末尾の改行の後ろも 1 行（カーソルを置ける）
        assert_eq!(buffer.line_count(), 3);
        assert_eq!(buffer.resolve_position(pos(3, 0)), Ok(buffer.text().len()));
        // 逆写像
        assert_eq!(buffer.position_of(10), pos(2, 6));
    }

    #[test]
    fn 範囲編集は指定した行だけを差し替えてカーソルを末尾へ置く() {
        let mut buffer = TextBuffer::from_text(path("range-basic"), "one\ntwo\nthree\n".into());
        let before = buffer.version();
        buffer
            .replace_position_range(&range_edit(pos(2, 0), pos(2, 3), "TWO!"))
            .expect("解ける範囲");
        assert_eq!(buffer.text(), "one\nTWO!\nthree\n");
        assert_eq!(
            buffer.cursor_position(),
            pos(2, 4),
            "カーソルが差し替え末尾に無い"
        );
        assert_eq!(buffer.version(), before + 1);
        // undo 1 回で戻る（1 操作 = 1 履歴）
        assert_eq!(buffer.undo_depth(), 1);
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "one\ntwo\nthree\n");
    }

    #[test]
    fn 範囲編集は行をまたいでも他の行をバイトで保つ() {
        let mut buffer = TextBuffer::from_text(path("range-multi"), "a\nb\nc\nd\ne\n".into());
        buffer
            .replace_position_range(&range_edit(pos(2, 0), pos(4, 0), "X\nY\n"))
            .expect("解ける範囲");
        assert_eq!(buffer.text(), "a\nX\nY\nd\ne\n");
    }

    #[test]
    fn 版は打鍵と全文置換と範囲編集とundoとredoで進む() {
        let mut buffer = TextBuffer::from_text(path("version"), "abc\n".into());
        let mut seen = vec![buffer.version()];
        // GUI の打鍵が通る口
        buffer.insert("x");
        seen.push(buffer.version());
        // 全文置換（dispatch の PreviewApply）
        buffer.set_text("abc\ndef\n".into());
        seen.push(buffer.version());
        // 範囲編集
        buffer
            .replace_position_range(&range_edit(pos(1, 0), pos(1, 3), "ABC"))
            .expect("解ける範囲");
        seen.push(buffer.version());
        assert!(buffer.undo());
        seen.push(buffer.version());
        assert!(buffer.redo());
        seen.push(buffer.version());
        // 削除も改行も版を進める
        buffer.delete_backward();
        seen.push(buffer.version());
        buffer.newline();
        seen.push(buffer.version());
        for pair in seen.windows(2) {
            assert!(pair[1] > pair[0], "版が単調増加していない: {seen:?}");
        }
    }

    #[test]
    fn カーソル移動は本文を変えないので版を進めない() {
        let mut buffer = TextBuffer::from_text(path("version-cursor"), "abc\ndef\n".into());
        let before = buffer.version();
        buffer
            .set_cursor_placement(&CursorPlacement {
                cursor: pos(1, 1),
                select_to: Some(pos(2, 2)),
                expected_version: None,
            })
            .expect("解ける位置");
        assert_eq!(buffer.version(), before);
        // 選択の起点が 1:1、カーソルは select_to 側
        assert_eq!(buffer.cursor_position(), pos(2, 2));
        assert_eq!(buffer.selection_positions(), Some((pos(1, 1), pos(2, 2))));
    }

    #[test]
    fn 版が違う範囲編集と移動は本文もカーソルも触らずに拒否される() {
        let mut buffer = TextBuffer::from_text(path("optimistic"), "abc\n".into());
        let stale = buffer.version();
        buffer.insert("x"); // 間に誰かが編集した
        let text = buffer.text().to_string();
        let cursor = buffer.cursor_position();
        let mut edit = range_edit(pos(1, 0), pos(1, 1), "Z");
        edit.expected_version = Some(stale);
        assert_eq!(
            buffer.replace_position_range(&edit),
            Err(RangeEditError::VersionMismatch {
                expected: stale,
                actual: buffer.version(),
            })
        );
        assert_eq!(buffer.text(), text, "拒否したのに本文が変わった");
        let place = CursorPlacement {
            cursor: pos(1, 0),
            select_to: None,
            expected_version: Some(stale),
        };
        assert!(buffer.set_cursor_placement(&place).is_err());
        assert_eq!(buffer.cursor_position(), cursor);
        // 現在の版を渡せば通る
        edit.expected_version = Some(buffer.version());
        assert!(buffer.replace_position_range(&edit).is_ok());
    }

    #[test]
    fn 範囲外の行と桁は丸めずに拒否する() {
        let mut buffer = TextBuffer::from_text(path("oob"), "abc\ndef\n".into());
        assert_eq!(
            buffer.resolve_position(pos(0, 0)),
            Err(RangeEditError::ZeroLine)
        );
        assert_eq!(
            buffer.resolve_position(pos(4, 0)),
            Err(RangeEditError::LineOutOfRange { line: 4, total: 3 })
        );
        assert_eq!(
            buffer.resolve_position(pos(1, 4)),
            Err(RangeEditError::ColumnOutOfRange {
                line: 1,
                column: 4,
                length: 3,
            })
        );
        // 逆順の範囲
        assert!(matches!(
            buffer.replace_position_range(&range_edit(pos(2, 2), pos(1, 0), "X")),
            Err(RangeEditError::InvertedRange { .. })
        ));
        assert_eq!(buffer.text(), "abc\ndef\n", "拒否したのに本文が変わった");
    }

    #[test]
    fn 文字の途中を指す桁は拒否する() {
        let mut buffer = TextBuffer::from_text(path("boundary"), "あいう\n".into());
        // 「あ」は 3 バイト。1 と 2 は文字の途中
        for column in [1usize, 2] {
            assert_eq!(
                buffer.resolve_position(pos(1, column)),
                Err(RangeEditError::NotCharBoundary { line: 1, column })
            );
        }
        assert!(buffer
            .replace_position_range(&range_edit(pos(1, 0), pos(1, 4), "X"))
            .is_err());
        assert_eq!(buffer.text(), "あいう\n");
        // 境界なら通る
        assert!(buffer
            .replace_position_range(&range_edit(pos(1, 0), pos(1, 3), "X"))
            .is_ok());
        assert_eq!(buffer.text(), "Xいう\n");
    }

    #[test]
    fn crlfのcrとlfのあいだは行の外なので拒否する() {
        let mut buffer = TextBuffer::from_text(path("crlf-range"), "abc\r\ndef\r\n".into());
        // 1 行目の長さは CR を含めず 3
        assert_eq!(buffer.resolve_position(pos(1, 3)), Ok(3));
        assert_eq!(
            buffer.resolve_position(pos(1, 4)),
            Err(RangeEditError::ColumnOutOfRange {
                line: 1,
                column: 4,
                length: 3,
            })
        );
        // 行の中身を差し替えても既存の改行は 1 バイトも変わらない（#1650 の契約）
        buffer
            .replace_position_range(&range_edit(pos(1, 0), pos(1, 3), "XY"))
            .expect("解ける範囲");
        assert_eq!(buffer.text(), "XY\r\ndef\r\n");
        // 入れる本文の改行はバッファの流儀へ揃う
        buffer
            .replace_position_range(&range_edit(pos(2, 0), pos(2, 3), "1\n2"))
            .expect("解ける範囲");
        assert_eq!(buffer.text(), "XY\r\n1\r\n2\r\n");
    }

    #[test]
    fn 空範囲は挿入になり全範囲の置換は全文を差し替える() {
        let mut buffer = TextBuffer::from_text(path("empty-range"), "abc\ndef\n".into());
        // 空範囲 = その位置へ挿入
        buffer
            .replace_position_range(&range_edit(pos(2, 1), pos(2, 1), "XY"))
            .expect("解ける範囲");
        assert_eq!(buffer.text(), "abc\ndXYef\n");
        // 全範囲（1:0 から最終行の末尾）の置換
        let last = buffer.line_count();
        buffer
            .replace_position_range(&range_edit(pos(1, 0), pos(last, 0), "new\n"))
            .expect("解ける範囲");
        assert_eq!(buffer.text(), "new\n");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "abc\ndXYef\n");
    }

    #[test]
    fn 文書状態は版とカーソルと選択とundo履歴を返す() {
        let mut buffer = TextBuffer::from_text(path("doc-state"), "abc\ndef\n".into());
        buffer
            .replace_position_range(&range_edit(pos(1, 3), pos(1, 3), "!"))
            .expect("解ける範囲");
        let state = buffer.document_state();
        assert_eq!(state["version"], 1);
        assert_eq!(state["line_count"], 3);
        assert_eq!(state["cursor"]["line"], 1);
        assert_eq!(state["cursor"]["column"], 4);
        assert!(state["selection"].is_null());
        assert_eq!(state["undo_depth"], 1);
        assert!(state["undo_history_bytes"].as_u64().unwrap() > 0);
        buffer
            .set_cursor_placement(&CursorPlacement {
                cursor: pos(1, 0),
                select_to: Some(pos(1, 2)),
                expected_version: None,
            })
            .expect("解ける位置");
        let state = buffer.document_state();
        assert_eq!(state["selection"]["start"]["column"], 0);
        assert_eq!(state["selection"]["end"]["column"], 2);
    }

    // --- 修飾キーの移動・削除（#1652） -----------------------------------------

    /// `|` をカーソル位置の印として読む（`foo |bar` → 本文 `foo bar` / カーソル 4）
    fn marked(src: &str) -> (String, usize) {
        let at = src.find('|').expect("カーソルの印 | がある");
        (src.replacen('|', "", 1), at)
    }

    /// 本文とカーソルを `|` 付きの 1 本の文字列へ戻す（失敗時に読める形で出す）
    fn show(buffer: &TextBuffer) -> String {
        let mut out = buffer.text().to_string();
        out.insert(buffer.cursor(), '|');
        out
    }

    fn buffer_at(src: &str) -> TextBuffer {
        let (text, cursor) = marked(src);
        let mut buffer = TextBuffer::from_text(path("keys"), text);
        buffer.set_cursor(cursor, false);
        buffer
    }

    /// 移動の表（多バイト・CRLF・空行・行末・文書端を含む）
    #[test]
    fn 語と行の移動は表どおりに止まる() {
        use CursorMovement as M;
        let cases: &[(&str, M, &str)] = &[
            // 語の右: 空白を飛ばして次の塊の末尾
            ("|foo bar", M::WordRight, "foo| bar"),
            ("foo| bar", M::WordRight, "foo bar|"),
            ("fo|o bar", M::WordRight, "foo| bar"),
            ("|snake_case x", M::WordRight, "snake_case| x"),
            // 記号の並びは記号どうしで 1 語
            ("self|.buffer", M::WordRight, "self.|buffer"),
            ("|a += 1", M::WordRight, "a| += 1"),
            ("a| += 1", M::WordRight, "a +=| 1"),
            // 日本語は文字種の切り替わりが境目
            ("|日本語のテキスト", M::WordRight, "日本語|のテキスト"),
            ("日本語|のテキスト", M::WordRight, "日本語の|テキスト"),
            ("日本語の|テキスト", M::WordRight, "日本語のテキスト|"),
            ("|全角　スペース", M::WordRight, "全角|　スペース"),
            // 行末では次の行の最初の語の末尾へまたぐ（インデントは飛ばす）
            ("foo|\n    bar baz", M::WordRight, "foo\n    bar| baz"),
            // 次の行が空なら、その空行で止まる
            ("foo|\n\nbar", M::WordRight, "foo\n|\nbar"),
            // 空白のまま行末に着いたらそこで止まる
            ("foo|   \nbar", M::WordRight, "foo   |\nbar"),
            // CRLF は 1 つの行区切り（CR と LF のあいだに止まらない）
            ("foo|\r\nbar", M::WordRight, "foo\r\nbar|"),
            ("foo|   \r\nbar", M::WordRight, "foo   |\r\nbar"),
            // 文書末では動かない
            ("foo bar|", M::WordRight, "foo bar|"),
            ("|", M::WordRight, "|"),
            // 語の左: 空白を飛ばして前の塊の頭
            ("foo bar|", M::WordLeft, "foo |bar"),
            ("foo |bar", M::WordLeft, "|foo bar"),
            ("foo ba|r", M::WordLeft, "foo |bar"),
            ("self.|buffer", M::WordLeft, "self|.buffer"),
            ("日本語のテキスト|", M::WordLeft, "日本語の|テキスト"),
            ("日本語|のテキスト", M::WordLeft, "|日本語のテキスト"),
            // 行頭では前の行の最後の語の頭へまたぐ
            ("foo bar\n|baz", M::WordLeft, "foo |bar\nbaz"),
            ("foo bar\r\n|baz", M::WordLeft, "foo |bar\r\nbaz"),
            // 前の行が空なら、その空行で止まる
            ("foo\n\n|bar", M::WordLeft, "foo\n|\nbar"),
            // インデントの途中では行頭で止まる（前の行へ抜けない）
            ("foo\n    |bar", M::WordLeft, "foo\n|    bar"),
            // 文書頭では動かない
            ("|foo", M::WordLeft, "|foo"),
            // smart Home: 最初の非空白 ⇄ 桁 0
            ("    let x|", M::SmartLineStart, "    |let x"),
            ("    |let x", M::SmartLineStart, "|    let x"),
            ("|    let x", M::SmartLineStart, "    |let x"),
            ("  |  let x", M::SmartLineStart, "    |let x"),
            ("\t\tfn|()", M::SmartLineStart, "\t\t|fn()"),
            ("a\r\n    b|c\r\nd", M::SmartLineStart, "a\r\n    |bc\r\nd"),
            // インデントの無い行は桁 0 のまま（トグル先も桁 0）
            ("ab|c", M::SmartLineStart, "|abc"),
            ("|abc", M::SmartLineStart, "|abc"),
            // 空白だけの行: 最初の非空白 = 行末
            ("x\n  |  \ny", M::SmartLineStart, "x\n    |\ny"),
            ("x\n    |\ny", M::SmartLineStart, "x\n|    \ny"),
            // 空行
            ("x\n|\ny", M::SmartLineStart, "x\n|\ny"),
            // 行末（CRLF の CR の手前 = #1650）
            ("a|b\r\nc", M::LineEnd, "ab|\r\nc"),
            // 桁 0 の行頭（CLI / MCP から指す口）はインデントを見ない
            ("    let |x", M::LineStart, "|    let x"),
            // 文書端
            ("ab\nc|d\nef", M::DocumentStart, "|ab\ncd\nef"),
            ("ab\nc|d\nef", M::DocumentEnd, "ab\ncd\nef|"),
        ];
        for (src, movement, want) in cases {
            let mut buffer = buffer_at(src);
            buffer.move_cursor(*movement, false);
            assert_eq!(
                show(&buffer),
                *want,
                "{src:?} で {movement:?} → {want:?} のはず"
            );
        }
    }

    /// 削除の表（語・行単位は行の境目で止まり、端では何も消さない）
    #[test]
    fn 語と行の削除は表どおりに消す() {
        use DeleteMotion as D;
        let cases: &[(&str, D, &str)] = &[
            ("foo bar|", D::WordBackward, "foo |"),
            ("foo |bar", D::WordBackward, "|bar"),
            ("foo ba|r", D::WordBackward, "foo |r"),
            ("日本語の|テキスト", D::WordBackward, "日本語|テキスト"),
            // 行頭では改行だけを消して前の行とつなぐ（LF / CRLF とも 1 単位）
            ("foo\n|bar", D::WordBackward, "foo|bar"),
            ("foo\r\n|bar", D::WordBackward, "foo|bar"),
            // インデントだけを消す（前の行へ抜けない）
            ("x\n    |y", D::WordBackward, "x\n|y"),
            // 文書頭では何も消さない
            ("|foo", D::WordBackward, "|foo"),
            ("|foo bar", D::WordForward, "| bar"),
            ("foo| bar", D::WordForward, "foo|"),
            ("|日本語のテキスト", D::WordForward, "|のテキスト"),
            // 行末では改行だけを消す
            ("foo|\nbar", D::WordForward, "foo|bar"),
            ("foo|\r\nbar", D::WordForward, "foo|bar"),
            // 空白のあとが行末なら空白だけ消える（次の行は消えない）
            ("foo|   \nbar", D::WordForward, "foo|\nbar"),
            // 文書末では何も消さない
            ("foo|", D::WordForward, "foo|"),
            // 行頭まで / 行末まで
            ("  ab|cd", D::ToLineStart, "|cd"),
            ("x\n|cd", D::ToLineStart, "x|cd"),
            ("ab|cd\r\nx", D::ToLineEnd, "ab|\r\nx"),
            ("ab|\r\nx", D::ToLineEnd, "ab|x"),
            // 1 文字（従来の Backspace / Delete と同じ）
            ("a日|b", D::CharBackward, "a|b"),
            ("a|日b", D::CharForward, "a|b"),
            ("a\r\n|b", D::CharBackward, "a|b"),
        ];
        for (src, motion, want) in cases {
            let mut buffer = buffer_at(src);
            buffer.delete(*motion);
            assert_eq!(
                show(&buffer),
                *want,
                "{src:?} で {motion:?} → {want:?} のはず"
            );
        }
    }

    /// 選択があれば単位によらず選択を消す
    #[test]
    fn 選択中の語削除は選択を消す() {
        for motion in DeleteMotion::ALL {
            let mut buffer = TextBuffer::from_text(path("sel-del"), "one two three".into());
            buffer.set_selection(4, 7);
            buffer.delete(motion);
            assert_eq!(buffer.text(), "one  three", "{motion:?}");
            assert_eq!(buffer.cursor(), 4, "{motion:?}");
            assert_eq!(buffer.selection(), None, "{motion:?}");
        }
    }

    /// 端で押しても本文も版も変わらない（楽観ロックを無駄に外さない）
    #[test]
    fn 端での語削除は版を進めない() {
        let mut buffer = TextBuffer::from_text(path("edge"), "ab".into());
        let before = buffer.version();
        buffer.set_cursor(0, false);
        buffer.delete(DeleteMotion::WordBackward);
        buffer.delete(DeleteMotion::ToLineStart);
        buffer.set_cursor(2, false);
        buffer.delete(DeleteMotion::WordForward);
        buffer.delete(DeleteMotion::ToLineEnd);
        assert_eq!(buffer.text(), "ab");
        assert_eq!(buffer.version(), before);
        assert_eq!(buffer.undo_depth(), 0);
    }

    /// 語削除は 1 回ずつ undo で戻る（連続しても 1 塊にまとめない）
    #[test]
    fn 語削除はundo1回で1語ずつ戻る() {
        let mut buffer = TextBuffer::from_text(path("undo-word"), "one two three".into());
        buffer.set_clock_millis(0);
        buffer.move_cursor(CursorMovement::DocumentEnd, false);
        buffer.delete(DeleteMotion::WordBackward);
        buffer.delete(DeleteMotion::WordBackward);
        assert_eq!(buffer.text(), "one ");
        assert_eq!(buffer.undo_depth(), 2);
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "one two |");
        assert!(buffer.undo());
        assert_eq!(show(&buffer), "one two three|");
    }

    /// Issue の実測の再現: (0,8) → ↓ (1,2) → ↓ は (2,8) へ戻る
    #[test]
    fn 上下移動は短い行をまたいでも元の桁へ戻る() {
        let mut buffer = TextBuffer::from_text(path("goal"), "0123456789\nab\n0123456789\n".into());
        buffer.set_cursor(8, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 2));
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 8));
        // 上へ戻っても同じ
        buffer.move_cursor(CursorMovement::Up, false);
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (0, 8));
        // 末尾の空行（改行で終わるファイルの後ろ）も 1 行として通る
        buffer.move_cursor(CursorMovement::Down, false);
        buffer.move_cursor(CursorMovement::Down, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (3, 0));
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 8));
    }

    /// 桁はバイト数に引きずられない（多バイト・CRLF の行）。全角だけの行どうしは
    /// 文字数で数えても表示幅で数えても同じ桁になる（全角と半角の混在は次のテスト）
    #[test]
    fn 桁の記憶は多バイトとcrlfでもバイトに引きずられない() {
        let mut buffer = TextBuffer::from_text(
            path("goal-mb"),
            "日本語テキスト\r\nab\r\nかなカナ漢字です\r\n".into(),
        );
        buffer.set_cursor("日本語テキ".len(), false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(
            show(&buffer),
            "日本語テキスト\r\nab|\r\nかなカナ漢字です\r\n"
        );
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(
            show(&buffer),
            "日本語テキスト\r\nab\r\nかなカナ漢|字です\r\n"
        );
    }

    /// #1742: 桁は**表示幅**（全角 = 2）で持つ。文字数で覚えると `abcd|` の ↓ が
    /// `あいう|`（4 文字目 = 行末）へ飛び、見た目で 2 桁右へずれる
    #[test]
    fn 桁の記憶は全角を2桁と数える_1742() {
        let text = "abcdef\nあいう\nabcdef\n";
        let mut buffer = TextBuffer::from_text(path("goal-wide"), text.into());
        buffer.set_cursor("abcd".len(), false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcdef\nあい|う\nabcdef\n");
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcdef\nあいう\nabcd|ef\n");
        // 全角 → 半角: `あ|` は見た目で 2 桁なので `ab|` へ（文字数なら `a|`）
        buffer.set_cursor("abcdef\nあ".len(), false);
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(show(&buffer), "ab|cdef\nあいう\nabcdef\n");
        // 全角の途中（桁 3）に当たったら近いほうへ、等距離なら手前へ寄せる。
        // 記憶している桁は 3 のままなので、次の半角の行では桁 3 へ戻る
        buffer.set_cursor(3, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcdef\nあ|いう\nabcdef\n");
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcdef\nあいう\nabc|def\n");
        // 全角スペース（U+3000）も 2 桁
        let mut buffer = TextBuffer::from_text(path("goal-ideo"), "\u{3000}x\nabcd".into());
        buffer.set_cursor("\u{3000}".len(), false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "\u{3000}x\nab|cd");
    }

    /// #1742: タブは**次のタブストップ**（4 桁ごと）まで進む幅で数える。
    /// 固定の 4 桁ではないので、行頭の空白 2 つの後ろのタブは 2 桁ぶんしか取らない
    #[test]
    fn 桁の記憶はタブをタブストップで数える_1742() {
        let text = "\tlet x = 1;\nabcdefgh\n  \tz\n";
        let mut buffer = TextBuffer::from_text(path("goal-tab"), text.into());
        // タブの直後 = 桁 4（文字数なら 1 で `a|`）
        buffer.set_cursor(1, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "\tlet x = 1;\nabcd|efgh\n  \tz\n");
        // `  \t` の後ろもタブストップの桁 4（固定 4 桁なら桁 6 で、`  |\tz` へずれる）
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "\tlet x = 1;\nabcdefgh\n  \t|z\n");
        // 桁 6 から上へ: タブ（0→4）+ `le` = 桁 6（文字数なら `\tlet x|`）
        buffer.set_cursor("\tlet x = 1;\nabcdef".len(), false);
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(show(&buffer), "\tle|t x = 1;\nabcdefgh\n  \tz\n");
        // タブの途中（桁 3）に当たったら近いほう = タブの後ろ（桁 4）へ寄せる
        let mut buffer = TextBuffer::from_text(path("goal-tab-mid"), "abc\n\tx".into());
        buffer.set_cursor(3, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abc\n\t|x");
        // 桁 1 なら近いほう = タブの手前（桁 0）
        buffer.set_cursor(1, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abc\n|\tx");
    }

    /// #1742: CRLF の行でも同じ（行末は CR の手前で、CR と LF のあいだへは入らない）
    #[test]
    fn 桁の記憶はcrlfの行でも表示幅で数える_1742() {
        let text = "abcd\r\nあいう\r\n\tx\r\nab\r\n";
        let mut buffer = TextBuffer::from_text(path("goal-crlf"), text.into());
        buffer.set_cursor(4, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcd\r\nあい|う\r\n\tx\r\nab\r\n");
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcd\r\nあいう\r\n\t|x\r\nab\r\n");
        // 短い行（桁 2）では CR の手前へ寄せ、記憶した桁 4 は持ち越す
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(show(&buffer), "abcd\r\nあいう\r\n\tx\r\nab|\r\n");
        buffer.move_cursor(CursorMovement::Up, false);
        buffer.move_cursor(CursorMovement::Up, false);
        buffer.move_cursor(CursorMovement::Up, false);
        assert_eq!(show(&buffer), "abcd|\r\nあいう\r\n\tx\r\nab\r\n");
    }

    /// #1742: 選択中の素の ← は選択の始点・→ は終点へ畳む（1 文字は動かない）。
    /// ⇧ 付きは従来どおり伸ばす。上下と語の移動はカーソルの位置から動く
    #[test]
    fn 選択中の素の左右は選択の端へ畳む_1742() {
        let text = "hello world\nsecond line\n";
        let select = |anchor: usize, head: usize| {
            let mut buffer = TextBuffer::from_text(path("collapse"), text.into());
            buffer.set_selection(anchor, head);
            buffer
        };
        // 前向きの選択（カーソルは終点）
        let mut buffer = select(0, 5);
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!((buffer.cursor(), buffer.selection()), (0, None));
        let mut buffer = select(0, 5);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!((buffer.cursor(), buffer.selection()), (5, None));
        // 後ろ向きの選択（カーソルは始点）
        let mut buffer = select(5, 0);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!((buffer.cursor(), buffer.selection()), (5, None));
        let mut buffer = select(5, 0);
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!((buffer.cursor(), buffer.selection()), (0, None));
        // 複数行にまたがる選択
        let end = "hello world\nsec".len();
        let mut buffer = select(3, end);
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!(buffer.cursor(), 3);
        let mut buffer = select(end, 3);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), end);
        // 文書の先頭・末尾を含む全選択
        let mut buffer = select(0, text.len());
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!(buffer.cursor(), 0);
        let mut buffer = select(0, text.len());
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), text.len());
        // 畳むのは 1 回だけ。次の → からは 1 文字ずつ動く
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!(buffer.cursor(), text.len() - 1);
        // ⇧ 付きは選択を伸ばす（畳まない）
        let mut buffer = select(0, 5);
        buffer.move_cursor(CursorMovement::Left, true);
        assert_eq!((buffer.cursor(), buffer.selection()), (4, Some(0..4)));
        let mut buffer = select(0, 5);
        buffer.move_cursor(CursorMovement::Right, true);
        assert_eq!(buffer.selection(), Some(0..6));
        // 上下・語の移動はカーソル（選択の先端）から動く
        let mut buffer = select(0, 5);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 5));
        let mut buffer = select(0, 5);
        buffer.move_cursor(CursorMovement::WordRight, false);
        assert_eq!(buffer.cursor(), "hello world".len());
        // CRLF をまたぐ選択でも CR と LF のあいだへは畳まない
        let mut buffer = TextBuffer::from_text(path("collapse-crlf"), "ab\r\ncd".into());
        buffer.set_selection(1, 4);
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.cursor(), 4);
        buffer.set_selection(4, 2);
        buffer.move_cursor(CursorMovement::Left, false);
        assert_eq!(buffer.cursor(), 2);
    }

    /// #1742: 畳んだら上下の桁の記憶は捨てる（横移動と同じ）。
    /// ⇧↓ で覚えた桁 8 を → で畳んだあとの ↓ は、畳んだ位置の桁から動く
    #[test]
    fn 選択を畳んだら桁の記憶は捨てる_1742() {
        let text = "0123456789\nab\n0123456789";
        let mut buffer = TextBuffer::from_text(path("collapse-goal"), text.into());
        buffer.set_cursor(8, false);
        buffer.move_cursor(CursorMovement::Down, true);
        assert_eq!(buffer.selection(), Some(8.."0123456789\nab".len()));
        buffer.move_cursor(CursorMovement::Right, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 2));
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 2));
    }

    /// 上下以外で動いたら・打ったら桁の記憶は捨てる
    #[test]
    fn 横移動と編集で桁の記憶は捨てる() {
        let text = "0123456789\nab\n0123456789";
        // 横移動
        let mut buffer = TextBuffer::from_text(path("goal-h"), text.into());
        buffer.set_cursor(8, false);
        buffer.move_cursor(CursorMovement::Down, false);
        buffer.move_cursor(CursorMovement::Left, false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 1));
        // 編集
        let mut buffer = TextBuffer::from_text(path("goal-e"), text.into());
        buffer.set_cursor(8, false);
        buffer.move_cursor(CursorMovement::Down, false);
        buffer.insert("Z");
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 3));
        // クリック（set_cursor）
        let mut buffer = TextBuffer::from_text(path("goal-c"), text.into());
        buffer.set_cursor(8, false);
        buffer.move_cursor(CursorMovement::Down, false);
        buffer.set_cursor("0123456789\na".len(), false);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 1));
    }

    /// GUI は 1 打鍵ごとに画面の選択をバッファへ写す（`set_selection`）。
    /// 同じ選択の写し戻しでは桁の記憶も undo の塊も切れない
    #[test]
    fn 同じ選択の写し戻しは桁の記憶を切らない() {
        let mut buffer =
            TextBuffer::from_text(path("goal-sel"), "0123456789\nab\n0123456789".into());
        buffer.set_cursor(8, false);
        buffer.move_cursor(CursorMovement::Down, true);
        // GUI の写し戻し（同じ選択）
        let (anchor, head) = (buffer.anchor().unwrap(), buffer.cursor());
        buffer.set_selection(anchor, head);
        buffer.move_cursor(CursorMovement::Down, true);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (2, 8));
        assert_eq!(buffer.selection(), Some(8..buffer.cursor()));
        // 違う選択を写したら切れる
        buffer.set_selection(0, 1);
        buffer.move_cursor(CursorMovement::Down, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 1));
    }

    /// ページ移動: 歩幅は「見えている行数 − 1」、端の行で止まり、桁の記憶を使う
    #[test]
    fn ページ移動は見えている行数から1行残して進む() {
        let text: String = (0..100)
            .map(|i| format!("line {i:03} xxxxxxxx\n"))
            .collect();
        let mut buffer = TextBuffer::from_text(path("page"), text);
        buffer.set_viewport_lines(10);
        buffer.set_cursor("line 000 xx".len(), false);
        buffer.move_cursor(CursorMovement::PageDown, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (9, 11));
        buffer.move_cursor(CursorMovement::PageDown, true);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (18, 11));
        assert_eq!(
            buffer.selection().map(|r| r.start),
            Some("line 000 xxxxxxxx\n".len() * 9 + 11)
        );
        buffer.move_cursor(CursorMovement::PageUp, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (9, 11));
        // 文書の端の行で止まる（末尾は改行の後ろの空行 = 行 100。短い行へは行末で着く）
        for _ in 0..20 {
            buffer.move_cursor(CursorMovement::PageDown, false);
        }
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (100, 0));
        // 端から戻ると桁は記憶どおり
        buffer.move_cursor(CursorMovement::PageUp, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (91, 11));
        for _ in 0..20 {
            buffer.move_cursor(CursorMovement::PageUp, false);
        }
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (0, 11));
        // 1 行しか見えない器でも止まらずに 1 行ずつ進む
        buffer.set_viewport_lines(1);
        buffer.move_cursor(CursorMovement::PageDown, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()), (1, 11));
    }

    /// 器を 1 度も測っていないときは既定の歩幅で進む（CLI から開いた直後など）
    #[test]
    fn 器が未計測ならページ移動は既定の歩幅() {
        let text: String = (0..50).map(|i| format!("{i}\n")).collect();
        let mut buffer = TextBuffer::from_text(path("page-default"), text);
        buffer.move_cursor(CursorMovement::PageDown, false);
        assert_eq!(buffer.line_byte_col(buffer.cursor()).0, DEFAULT_PAGE_STEP);
    }

    /// ⇧ 付きの移動は選択を伸ばし、⇧ なしの移動は選択を畳む
    #[test]
    fn 修飾付きの移動は選択を伸ばせる() {
        use CursorMovement as M;
        for (movement, src, want_sel) in [
            (M::WordRight, "a |foo bar", "foo"),
            (M::WordLeft, "a foo| bar", "foo"),
            (M::SmartLineStart, "  ab|c", "ab"),
            (M::LineEnd, "a|bc\nd", "bc"),
            (M::DocumentEnd, "a|b\ncd", "b\ncd"),
            (M::DocumentStart, "ab\nc|d", "ab\nc"),
        ] {
            let mut buffer = buffer_at(src);
            buffer.move_cursor(movement, true);
            let range = buffer.selection().expect("選択が伸びる");
            assert_eq!(&buffer.text()[range], want_sel, "{movement:?} {src:?}");
            buffer.move_cursor(movement, false);
            assert_eq!(buffer.selection(), None, "{movement:?} {src:?}");
        }
    }

    /// 移動は本文を変えないので版を進めない（楽観ロックが移動で外れない）
    #[test]
    fn 修飾付きの移動は版を進めない() {
        let mut buffer = TextBuffer::from_text(path("move-version"), "one two\nthree\n".into());
        buffer.set_viewport_lines(5);
        let before = buffer.version();
        for movement in CursorMovement::ALL {
            buffer.move_cursor(movement, false);
            buffer.move_cursor(movement, true);
        }
        assert_eq!(buffer.version(), before);
    }

    /// 名前の表: 全種類が一意に往復する（CLI / MCP はこの綴りだけを使う）
    #[test]
    fn 移動と削除の名前は往復する() {
        for movement in CursorMovement::ALL {
            assert_eq!(CursorMovement::from_name(movement.name()), Some(movement));
        }
        for motion in DeleteMotion::ALL {
            assert_eq!(DeleteMotion::from_name(motion.name()), Some(motion));
        }
        let mut names = CursorMovement::names();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CursorMovement::ALL.len());
        let mut names = DeleteMotion::names();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), DeleteMotion::ALL.len());
        assert_eq!(CursorMovement::from_name("word_left"), None);
    }

    // --- インデント（#1654） -------------------------------------------------

    /// `|` をカーソル、`^` を選択の起点として読む（`^ab|c` → 本文 `abc`・選択 0..2・カーソル 2）
    fn indent_buffer(name: &str, src: &str) -> TextBuffer {
        let mut text = String::new();
        let (mut cursor, mut anchor) = (None, None);
        for c in src.chars() {
            match c {
                '|' => cursor = Some(text.len()),
                '^' => anchor = Some(text.len()),
                _ => text.push(c),
            }
        }
        let mut buffer = TextBuffer::from_text(path(name), text);
        let cursor = cursor.expect("カーソルの印 | がある");
        buffer.set_selection(anchor.unwrap_or(cursor), cursor);
        buffer
    }

    /// 本文に `|`（カーソル）と `^`（選択の起点）を差し込んで返す
    fn show_sel(buffer: &TextBuffer) -> String {
        let mut marks = vec![(buffer.cursor(), '|')];
        if let Some(range) = buffer.selection() {
            let anchor = if range.start == buffer.cursor() {
                range.end
            } else {
                range.start
            };
            marks.push((anchor, '^'));
        }
        marks.sort_by_key(|m| std::cmp::Reverse(m.0));
        let mut out = buffer.text().to_string();
        for (at, mark) in marks {
            out.insert(at, mark);
        }
        out
    }

    #[test]
    fn インデントの単位は既存行から推定する() {
        let cases: &[(&str, Option<IndentUnit>)] = &[
            (
                "fn main() {\n    let x = 1;\n}\n",
                Some(IndentUnit::Spaces(4)),
            ),
            ("a:\n  b:\n    c: 1\n", Some(IndentUnit::Spaces(2))),
            ("int main() {\n\treturn 0;\n}\n", Some(IndentUnit::Tab)),
            // CRLF でも同じ
            ("fn a() {\r\n  x();\r\n}\r\n", Some(IndentUnit::Spaces(2))),
            // 深い入れ子しか無くても差で決まる（8 桁 → 12 桁 = 4）
            (
                "x\n        a\n            b\n        c\n",
                Some(IndentUnit::Spaces(4)),
            ),
            // 差が取れなければいちばん浅い幅
            ("- a\n  b\n", Some(IndentUnit::Spaces(2))),
            // ` * ` の 1 桁ずれは数えない
            (
                "/**\n * doc\n */\nfn f() {\n    g();\n}\n",
                Some(IndentUnit::Spaces(4)),
            ),
            // タブ行が多ければタブ（スペースの継続行が混ざっていても）
            (
                "f() {\n\ta();\n\tb();\n      c;\n}\n",
                Some(IndentUnit::Tab),
            ),
            // 手掛かりなし
            ("abc\ndef\n", None),
            ("", None),
        ];
        for (text, want) in cases {
            assert_eq!(IndentUnit::detect(text), *want, "{text:?}");
        }
        assert_eq!(IndentUnit::for_path(Path::new("Makefile")), IndentUnit::Tab);
        assert_eq!(
            IndentUnit::for_path(Path::new("a/main.go")),
            IndentUnit::Tab
        );
        assert_eq!(
            IndentUnit::for_path(Path::new("a.rs")),
            IndentUnit::Spaces(4)
        );
        assert_eq!(
            IndentUnit::for_path(Path::new("README.md")),
            IndentUnit::Spaces(4)
        );
        // 本文が無ければファイル名で決まる
        let buffer = TextBuffer::from_text(PathBuf::from("Makefile"), "all:\n".into());
        assert_eq!(buffer.indent_unit(), IndentUnit::Tab);
    }

    /// Rust: 括弧の直後は 1 段深く・継承・`{|}` の間・空白だけの行・Tab / ⇧Tab
    #[test]
    fn rustのインデント() {
        // Issue の実測 E8: `fn main() {` の中で改行すると次行が桁 0 だった
        let mut b = indent_buffer("i.rs", "fn main() {|\n}\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "fn main() {\n    |\n}\n");
        b.insert("let y = 2;");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "fn main() {\n    let y = 2;\n    |\n}\n");
        // 空白だけの行で Enter → その行の空白は残さない
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "fn main() {\n    let y = 2;\n\n    |\n}\n");
        // `{|}` の間 → 閉じ括弧を次の行へ
        let mut b = indent_buffer("i2.rs", "    if x {|}\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "    if x {\n        |\n    }\n");
        // `(` / `[` も同じ・閉じ括弧の手前の空白は捨てる
        let mut b = indent_buffer("i3.rs", "let v = vec![| ];\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "let v = vec![\n    |\n];\n");
        // 括弧で終わらない行は継承だけ・インデントの途中で押したらその桁まで
        let mut b = indent_buffer("i4.rs", "    foo();|\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "    foo();\n    |\n");
        let mut b = indent_buffer("i5.rs", "  |  foo();\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "  \n  |  foo();\n");
        // Tab: 次のタブ位置まで（桁 1 なら 3 つ）
        let mut b = indent_buffer("i6.rs", "fn f() {\n    a|b\n}\n");
        b.indent();
        assert_eq!(show_sel(&b), "fn f() {\n    a   |b\n}\n");
        // 選択した行をまとめて深く → 浅く（選択は同じ文字を指し続ける）
        let mut b = indent_buffer("i7.rs", "fn f() {\n    ^a();\n    b();|\n}\n");
        b.indent();
        assert_eq!(show_sel(&b), "fn f() {\n        ^a();\n        b();|\n}\n");
        b.outdent();
        b.outdent();
        assert_eq!(show_sel(&b), "fn f() {\n^a();\nb();|\n}\n");
        // 浅くできない行ばかりなら本文も版も変えない
        let version = b.version();
        b.outdent();
        assert_eq!(b.version(), version);
    }

    /// Python: `:` の直後は 1 段深く・継承・4 桁ずつ浅く
    #[test]
    fn pythonのインデント() {
        let mut b = indent_buffer("i.py", "def f(x):|\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "def f(x):\n    |\n");
        b.insert("if x:");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "def f(x):\n    if x:\n        |\n");
        b.insert("return 1");
        b.newline_and_indent();
        assert_eq!(
            show_sel(&b),
            "def f(x):\n    if x:\n        return 1\n        |\n"
        );
        // ⇧Tab で 1 段戻る
        b.outdent();
        assert_eq!(
            show_sel(&b),
            "def f(x):\n    if x:\n        return 1\n    |\n"
        );
        // 2 スペースのファイルは 2 桁で
        let mut b = indent_buffer("j.py", "if a:\n  b()\n  if c:|\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "if a:\n  b()\n  if c:\n    |\n");
        // 半端な桁（6）からの ⇧Tab は 1 つ前のタブ位置（4）へ
        let mut b = indent_buffer("k.py", "def f():\n    x = 1\n      |y\n");
        b.outdent();
        assert_eq!(show_sel(&b), "def f():\n    x = 1\n    |y\n");
    }

    /// Markdown: 継承だけ（`[` や `(` で深くしない）・リストの入れ子は 2 桁
    #[test]
    fn markdownのインデント() {
        let mut b = indent_buffer("i.md", "- a\n  - b|\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "- a\n  - b\n  |\n");
        let mut b = indent_buffer("j.md", "see [link](|\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "see [link](\n|\n");
        // Tab は推定した 2 桁で行を深くする
        let mut b = indent_buffer("k.md", "- a\n  - b\n^- c|\n");
        b.indent();
        assert_eq!(show_sel(&b), "- a\n  - b\n^  - c|\n");
    }

    /// タブでインデントするファイル: Tab はタブ 1 文字・⇧Tab はタブ 1 文字を外す
    #[test]
    fn タブのファイルはタブで足し引きする() {
        let mut b = indent_buffer("i.c", "int f() {|\n\treturn 0;\n}\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "int f() {\n\t|\n\treturn 0;\n}\n");
        b.indent();
        assert_eq!(show_sel(&b), "int f() {\n\t\t|\n\treturn 0;\n}\n");
        b.outdent();
        assert_eq!(show_sel(&b), "int f() {\n\t|\n\treturn 0;\n}\n");
    }

    /// CRLF のファイルでは挿す改行も CRLF（#1650 の契約）
    #[test]
    fn crlfのファイルでも継承した改行はcrlf() {
        let mut b = indent_buffer("i.rs", "fn f() {|\r\n}\r\n");
        b.newline_and_indent();
        assert_eq!(show_sel(&b), "fn f() {\r\n    |\r\n}\r\n");
        let mut b = indent_buffer("j.rs", "fn f() {\r\n    ^a();\r\n    b();\r\n|}\r\n");
        b.indent();
        assert_eq!(
            b.text(),
            "fn f() {\r\n        a();\r\n        b();\r\n}\r\n"
        );
        assert_eq!(
            b.text().matches('\n').count(),
            b.text().matches("\r\n").count()
        );
    }

    /// 行を丸ごと選ぶと終わりは次の行頭に来る。その行は触らない
    #[test]
    fn 行頭で終わる選択は次の行を含めない() {
        let mut b = indent_buffer("i.rs", "^a\nb\n|c\n");
        b.indent();
        assert_eq!(show_sel(&b), "^    a\n    b\n|c\n");
    }

    /// Tab / ⇧Tab / Enter はそれぞれ undo 1 回で戻る
    #[test]
    fn インデントはundo1回で戻る() {
        let src = "fn f() {\n    a();\n    b();\n}\n";
        let mut b = indent_buffer("u.rs", "fn f() {\n    ^a();\n    b();|\n}\n");
        b.indent();
        assert_eq!(b.undo_depth(), 1);
        assert!(b.undo());
        assert_eq!(b.text(), src);
        b.set_selection(9, src.len() - 2);
        b.outdent();
        assert!(b.undo());
        assert_eq!(b.text(), src);
        let mut b = indent_buffer("v.rs", "fn f() {|}\n");
        b.newline_and_indent();
        assert!(b.undo());
        assert_eq!(b.text(), "fn f() {}\n");
    }

    /// 空行・空白だけの行は深くしない（行末の空白を作らない）
    #[test]
    fn 空行は深くしない() {
        let mut b = indent_buffer("e.rs", "^a\n\n  \nb|\n");
        b.indent();
        assert_eq!(b.text(), "    a\n\n  \n    b\n");
    }

    /// 推定は開いたときに 1 回。Tab で深くしても単位は変わらない（visual-test の実測:
    /// 4 桁の 2 行を 8 桁にしたあと ⇧Tab が 8 桁外していた）
    #[test]
    fn 深くしてもインデントの単位は変わらない() {
        let mut b = indent_buffer("s.rs", "fn main() {\n    ^a();\n    b();\n|}\n");
        assert_eq!(b.indent_unit(), IndentUnit::Spaces(4));
        b.indent();
        assert_eq!(b.indent_unit(), IndentUnit::Spaces(4));
        b.outdent();
        assert_eq!(b.text(), "fn main() {\n    a();\n    b();\n}\n");
        // 全文の差し替えでは取り直す
        b.set_text("x:\n  y: 1\n".into());
        assert_eq!(b.indent_unit(), IndentUnit::Spaces(2));
        // 手掛かりの無い本文へ差し替えたら直前の単位のまま
        b.set_text("plain\n".into());
        assert_eq!(b.indent_unit(), IndentUnit::Spaces(2));
    }
}
