//! コード編集の打鍵表（プラットフォーム別。Issue #1652）
//!
//! **何のためにあるか**: プレビュー編集のキー入口は `⌥` / `⌃` / `⌘` 付きの打鍵を
//! 入口で**全部捨てていた**（`if modifiers.platform || control || alt { return false }`）。
//! ⌥← の単語移動も ⌥⌫ の単語削除も ⌘↑ の文書端も、押しても何も起きなかった。
//! 修飾キーでの移動・削除は手書き編集の最低条件なので、どの打鍵が何をするかを
//! **この 1 枚の表**に集め、GUI の入口はここを引くだけにする。
//!
//! ## 両 OS の列を 1 枚に並べる（#763 / #1655 の作法）
//!
//! 同じ操作でも OS で打鍵が違う（単語移動は macOS = ⌥← / Windows = Ctrl+←）。
//! 属性の `#[cfg]` で出し分けると macOS のビルドに Windows の列が存在せず、
//! 「Windows で何が起きるか」を macOS の単体から 1 マスも検査できない。
//! [`resolve`] は [`Platform`] を**引数で受ける純粋関数**にしてあり、両列を
//! どちらの OS でもコンパイルする。修飾キーも GPUI の `Modifiers` ではなく
//! [`KeyMods`]（bool の写し）で受けるので、tako-core は GPUI に依存しない。
//!
//! ## 列を決めた規則
//!
//! - **macOS は Cocoa のテキスト欄の慣習**（⌥ = 語、⌘ = 行 / 文書）。Home / End は
//!   コードエディタの慣習（VS Code / Zed / Xcode）どおり行頭・行末で、文書端へは
//!   飛ばさない。Cocoa のテキスト欄が持つ Emacs 系の ⌃A（行の先頭 = 桁 0）/ ⌃E（行末）/
//!   ⌃K（行末まで削除）も macOS の列にだけ置く（#1742。Windows に置かない理由は表の行に書く）
//! - **Windows は Windows 標準の慣習**（Ctrl = 語、Ctrl+Home / End = 文書端）。
//!   **Alt+矢印はペインのフォーカス移動に張ってある**（`keybindings.rs`。GPUI は
//!   キーバインドを入口より先に発火して伝播を止める）ので、ここで使うと届かない。
//!   衝突しないことは tako-app の番犬 `エディタの打鍵表はキーバインドと衝突しない` が
//!   **両 OS について**検査する
//! - **その OS で案内できない打鍵は受理もしない**（#763 と同じ規則）。macOS の
//!   ⌃←→ は Mission Control が持っていき、Windows の Win キーは OS のシェルが持って
//!   いくので、どちらも列に書かない
//! - **shift は「選択を伸ばす」**。移動の行だけが shift を読み、削除・改行・
//!   編集終了は shift の有無を問わない（従来どおり ⇧⌫ は ⌫ と同じ）。
//!   **例外は Tab だけ**で、shift は「浅くする」（⇧Tab = アンインデント。#1654）
//! - GPUI の `function` 修飾（macOS は矢印キーにも立つ）は**見ない**

use super::support::Platform;
use crate::text_edit::{CursorMovement, DeleteMotion, TextBuffer};

/// 打鍵が編集バッファに対して意味するもの（shift を解いた後の最終形）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCommand {
    /// カーソルを動かす。`extend` なら選択を伸ばす
    Move {
        movement: CursorMovement,
        extend: bool,
    },
    /// 消す（選択があれば選択を消す）
    Delete(DeleteMotion),
    /// 改行を入れる（前の行のインデントを引き継ぐ。#1654）
    Newline,
    /// 選択行を 1 段深く / 選択が無ければカーソル位置へ 1 段ぶん挿す（Tab。#1654）
    Indent,
    /// 選択行（無ければカーソルの行）を 1 段浅く（⇧Tab。#1654）
    Outdent,
    /// 編集モードを抜ける
    ExitEditing,
}

/// CLI / MCP から名前で指す編集コマンド（#1654）。
///
/// 移動と削除は [`CursorMovement::name`] / [`DeleteMotion::name`] の名前表を使い、
/// それ以外で本文を変える打鍵（Tab / ⇧Tab / Enter）の綴りはここ 1 か所
/// （`tako edit indent` / MCP `tako_preview_edit` の `command` がすべてここを引く）
const EDIT_COMMAND_NAMES: [(&str, EditorCommand); 3] = [
    ("indent", EditorCommand::Indent),
    ("outdent", EditorCommand::Outdent),
    ("newline", EditorCommand::Newline),
];

impl EditorCommand {
    /// CLI / MCP の綴りから引く（`indent` / `outdent` / `newline`）
    pub fn from_edit_name(name: &str) -> Option<Self> {
        EDIT_COMMAND_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| *c)
    }

    /// 綴りの一覧（MCP の inputSchema の enum とエラー文の候補）
    pub fn edit_names() -> Vec<&'static str> {
        EDIT_COMMAND_NAMES.iter().map(|(n, _)| *n).collect()
    }

    /// バッファへ当てる（#1652）。**GUI の打鍵と CLI / MCP が同じここを通る**。
    ///
    /// 編集モードを抜ける（[`Self::ExitEditing`]）はバッファの操作ではないので
    /// 何もせず `false` を返す（編集モードを閉じるのは呼び手 = GUI の仕事）
    pub fn apply(self, buffer: &mut TextBuffer) -> bool {
        match self {
            Self::Move { movement, extend } => buffer.move_cursor(movement, extend),
            Self::Delete(motion) => buffer.delete(motion),
            Self::Newline => buffer.newline_and_indent(),
            Self::Indent => buffer.indent(),
            Self::Outdent => buffer.outdent(),
            Self::ExitEditing => return false,
        }
        true
    }
}

/// 表の 1 行が指す操作（shift はまだ解いていない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Move(CursorMovement),
    Delete(DeleteMotion),
    Newline,
    /// Tab。shift で向きが変わる（⇧Tab = 浅く。#1654）
    Indent,
    ExitEditing,
}

impl Action {
    fn with_shift(self, shift: bool) -> EditorCommand {
        match self {
            Self::Move(movement) => EditorCommand::Move {
                movement,
                extend: shift,
            },
            Self::Delete(motion) => EditorCommand::Delete(motion),
            Self::Newline => EditorCommand::Newline,
            Self::Indent if shift => EditorCommand::Outdent,
            Self::Indent => EditorCommand::Indent,
            Self::ExitEditing => EditorCommand::ExitEditing,
        }
    }
}

/// 打鍵 1 つ（キー名 + shift 以外の修飾）。キー名は GPUI の `Keystroke::key` の綴り
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub key: &'static str,
    pub alt: bool,
    pub control: bool,
    /// GPUI の platform 修飾（macOS = ⌘ / Windows = Win キー）
    pub platform: bool,
}

impl Chord {
    const fn plain(key: &'static str) -> Self {
        Self {
            key,
            alt: false,
            control: false,
            platform: false,
        }
    }

    const fn alt(key: &'static str) -> Self {
        Self {
            alt: true,
            ..Self::plain(key)
        }
    }

    const fn ctrl(key: &'static str) -> Self {
        Self {
            control: true,
            ..Self::plain(key)
        }
    }

    const fn cmd(key: &'static str) -> Self {
        Self {
            platform: true,
            ..Self::plain(key)
        }
    }

    fn matches(&self, key: &str, mods: KeyMods) -> bool {
        self.key == key
            && self.alt == mods.alt
            && self.control == mods.control
            && self.platform == mods.platform
    }

    /// GPUI の `Keystroke::parse` が読める綴り（`alt-left` / `cmd-backspace`）。
    /// キーバインド表との突き合わせ・セルフテストの合成打鍵に使う
    pub fn gpui_spec(&self) -> String {
        let mut spec = String::new();
        if self.control {
            spec.push_str("ctrl-");
        }
        if self.alt {
            spec.push_str("alt-");
        }
        if self.platform {
            spec.push_str("cmd-");
        }
        spec.push_str(self.key);
        spec
    }
}

/// 押されている修飾キー（GPUI の `Modifiers` から `function` を除いた写し）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyMods {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub platform: bool,
}

/// 操作 1 つぶんの行（両 OS の打鍵を並べて持つ。空の列 = その OS では打鍵を割り当てない）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub macos: &'static [Chord],
    pub windows: &'static [Chord],
}

impl Entry {
    pub fn chords(&self, platform: Platform) -> &'static [Chord] {
        match platform {
            Platform::MacOs => self.macos,
            Platform::Windows => self.windows,
        }
    }
}

const fn row(action: Action, macos: &'static [Chord], windows: &'static [Chord]) -> Entry {
    Entry {
        action,
        macos,
        windows,
    }
}

use Action::{Delete, ExitEditing, Indent, Move, Newline};
use Chord as K;

/// 打鍵表（**正本**）。GUI の入口はここだけを引く
pub const TABLE: &[Entry] = &[
    // --- 1 文字・1 行 ---
    row(
        Move(CursorMovement::Left),
        &[K::plain("left")],
        &[K::plain("left")],
    ),
    row(
        Move(CursorMovement::Right),
        &[K::plain("right")],
        &[K::plain("right")],
    ),
    row(
        Move(CursorMovement::Up),
        &[K::plain("up")],
        &[K::plain("up")],
    ),
    row(
        Move(CursorMovement::Down),
        &[K::plain("down")],
        &[K::plain("down")],
    ),
    // --- 語（macOS = ⌥ / Windows = Ctrl）---
    row(
        Move(CursorMovement::WordLeft),
        &[K::alt("left")],
        &[K::ctrl("left")],
    ),
    row(
        Move(CursorMovement::WordRight),
        &[K::alt("right")],
        &[K::ctrl("right")],
    ),
    // --- 行（Home は smart Home。macOS は ⌘←→ と Emacs 系の ⌃A / ⌃E も）---
    //
    // ⌃A / ⌃E / ⌃K（下の削除の行）は macOS の列にだけ置く（#1742）。
    // Windows 列に置かない理由: Windows の Ctrl+A は「すべて選択」で、Ctrl+E / Ctrl+K も
    // 行頭・行末・行末までの削除の意味を持たない（Emacs 系の打鍵の慣習が OS に無い）。
    // その OS で案内できない打鍵は受理もしない（#763 の規則）
    row(
        Move(CursorMovement::SmartLineStart),
        &[K::plain("home"), K::cmd("left")],
        &[K::plain("home")],
    ),
    // ⌃A は smart Home ではなく桁 0 へ（Cocoa の moveToBeginningOfParagraph: / Emacs の C-a と同じ）
    row(Move(CursorMovement::LineStart), &[K::ctrl("a")], &[]),
    row(
        Move(CursorMovement::LineEnd),
        &[K::plain("end"), K::cmd("right"), K::ctrl("e")],
        &[K::plain("end")],
    ),
    // --- 文書端（macOS = ⌘↑↓ / Windows = Ctrl+Home / End）---
    row(
        Move(CursorMovement::DocumentStart),
        &[K::cmd("up")],
        &[K::ctrl("home")],
    ),
    row(
        Move(CursorMovement::DocumentEnd),
        &[K::cmd("down")],
        &[K::ctrl("end")],
    ),
    // --- ページ ---
    row(
        Move(CursorMovement::PageUp),
        &[K::plain("pageup")],
        &[K::plain("pageup")],
    ),
    row(
        Move(CursorMovement::PageDown),
        &[K::plain("pagedown")],
        &[K::plain("pagedown")],
    ),
    // --- 削除 ---
    row(
        Delete(DeleteMotion::CharBackward),
        &[K::plain("backspace")],
        &[K::plain("backspace")],
    ),
    row(
        Delete(DeleteMotion::CharForward),
        &[K::plain("delete")],
        &[K::plain("delete")],
    ),
    row(
        Delete(DeleteMotion::WordBackward),
        &[K::alt("backspace")],
        &[K::ctrl("backspace")],
    ),
    row(
        Delete(DeleteMotion::WordForward),
        &[K::alt("delete")],
        &[K::ctrl("delete")],
    ),
    // 行頭 / 行末までの削除は macOS の慣習だけ（Windows に標準の打鍵が無い）。
    // ⌃K は行末にいれば改行を 1 つ消す（Cocoa の deleteToEndOfParagraph: と同じ。#1742）
    row(
        Delete(DeleteMotion::ToLineStart),
        &[K::cmd("backspace")],
        &[],
    ),
    row(
        Delete(DeleteMotion::ToLineEnd),
        &[K::cmd("delete"), K::ctrl("k")],
        &[],
    ),
    // --- 改行・インデント・編集終了 ---
    row(Newline, &[K::plain("enter")], &[K::plain("enter")]),
    // Tab = 深く / ⇧Tab = 浅く（#1654）。Windows の Ctrl+Tab はタブ切替に張ってあるので素の Tab だけ
    row(Indent, &[K::plain("tab")], &[K::plain("tab")]),
    row(ExitEditing, &[K::plain("escape")], &[K::plain("escape")]),
];

/// 打鍵を操作へ解く。表に無い打鍵は `None`（入口はそのキーを編集に使わない）
pub fn resolve(platform: Platform, key: &str, mods: KeyMods) -> Option<EditorCommand> {
    TABLE.iter().find_map(|entry| {
        entry
            .chords(platform)
            .iter()
            .any(|chord| chord.matches(key, mods))
            .then(|| entry.action.with_shift(mods.shift))
    })
}

/// その OS で表に載っている打鍵すべて（キーバインドとの衝突検査・セルフテスト用）
pub fn all_chords(platform: Platform) -> impl Iterator<Item = (Action, Chord)> {
    TABLE.iter().flat_map(move |entry| {
        entry
            .chords(platform)
            .iter()
            .map(move |chord| (entry.action, *chord))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [Platform; 2] = [Platform::MacOs, Platform::Windows];

    /// `alt-shift-left` のような綴りを (キー, 修飾) へ。GPUI の `Keystroke::parse` と同じ語順を読む
    fn parse(spec: &str) -> (&str, KeyMods) {
        let mut mods = KeyMods::default();
        let mut parts: Vec<&str> = spec.split('-').collect();
        let key = parts.pop().expect("キーがある");
        for part in parts {
            match part {
                "shift" => mods.shift = true,
                "alt" => mods.alt = true,
                "ctrl" => mods.control = true,
                "cmd" => mods.platform = true,
                other => panic!("未知の修飾 {other}"),
            }
        }
        (key, mods)
    }

    fn resolve_spec(platform: Platform, spec: &str) -> Option<EditorCommand> {
        let (key, mods) = parse(spec);
        resolve(platform, key, mods)
    }

    fn mv(movement: CursorMovement, extend: bool) -> Option<EditorCommand> {
        Some(EditorCommand::Move { movement, extend })
    }

    fn del(motion: DeleteMotion) -> Option<EditorCommand> {
        Some(EditorCommand::Delete(motion))
    }

    /// 表そのものを両 OS について固定する（Issue の打鍵がすべて効くこと）
    #[test]
    fn 打鍵表は両osの慣習どおりに解ける() {
        use CursorMovement as M;
        use DeleteMotion as D;
        let cases: &[(Platform, &str, Option<EditorCommand>)] = &[
            // macOS
            (Platform::MacOs, "alt-left", mv(M::WordLeft, false)),
            (Platform::MacOs, "alt-right", mv(M::WordRight, false)),
            (Platform::MacOs, "alt-shift-left", mv(M::WordLeft, true)),
            (Platform::MacOs, "cmd-up", mv(M::DocumentStart, false)),
            (Platform::MacOs, "cmd-down", mv(M::DocumentEnd, false)),
            (Platform::MacOs, "cmd-shift-down", mv(M::DocumentEnd, true)),
            (Platform::MacOs, "cmd-left", mv(M::SmartLineStart, false)),
            (Platform::MacOs, "cmd-right", mv(M::LineEnd, false)),
            (Platform::MacOs, "home", mv(M::SmartLineStart, false)),
            (Platform::MacOs, "shift-home", mv(M::SmartLineStart, true)),
            (Platform::MacOs, "end", mv(M::LineEnd, false)),
            (Platform::MacOs, "pageup", mv(M::PageUp, false)),
            (Platform::MacOs, "shift-pagedown", mv(M::PageDown, true)),
            (Platform::MacOs, "alt-backspace", del(D::WordBackward)),
            (Platform::MacOs, "alt-delete", del(D::WordForward)),
            (Platform::MacOs, "cmd-backspace", del(D::ToLineStart)),
            (Platform::MacOs, "cmd-delete", del(D::ToLineEnd)),
            (Platform::MacOs, "backspace", del(D::CharBackward)),
            (Platform::MacOs, "shift-backspace", del(D::CharBackward)),
            (Platform::MacOs, "left", mv(M::Left, false)),
            (Platform::MacOs, "shift-up", mv(M::Up, true)),
            (Platform::MacOs, "enter", Some(EditorCommand::Newline)),
            (Platform::MacOs, "tab", Some(EditorCommand::Indent)),
            (Platform::MacOs, "shift-tab", Some(EditorCommand::Outdent)),
            (Platform::Windows, "tab", Some(EditorCommand::Indent)),
            (Platform::Windows, "shift-tab", Some(EditorCommand::Outdent)),
            (Platform::MacOs, "escape", Some(EditorCommand::ExitEditing)),
            // #1742: Cocoa の Emacs 系（⌃A = 桁 0 / ⌃E = 行末 / ⌃K = 行末まで削除）
            (Platform::MacOs, "ctrl-a", mv(M::LineStart, false)),
            (Platform::MacOs, "ctrl-shift-a", mv(M::LineStart, true)),
            (Platform::MacOs, "ctrl-e", mv(M::LineEnd, false)),
            (Platform::MacOs, "ctrl-shift-e", mv(M::LineEnd, true)),
            (Platform::MacOs, "ctrl-k", del(D::ToLineEnd)),
            // Windows
            (Platform::Windows, "ctrl-left", mv(M::WordLeft, false)),
            (Platform::Windows, "ctrl-right", mv(M::WordRight, false)),
            (
                Platform::Windows,
                "ctrl-shift-right",
                mv(M::WordRight, true),
            ),
            (Platform::Windows, "ctrl-home", mv(M::DocumentStart, false)),
            (Platform::Windows, "ctrl-end", mv(M::DocumentEnd, false)),
            (
                Platform::Windows,
                "ctrl-shift-home",
                mv(M::DocumentStart, true),
            ),
            (Platform::Windows, "home", mv(M::SmartLineStart, false)),
            (Platform::Windows, "end", mv(M::LineEnd, false)),
            (Platform::Windows, "pagedown", mv(M::PageDown, false)),
            (Platform::Windows, "ctrl-backspace", del(D::WordBackward)),
            (Platform::Windows, "ctrl-delete", del(D::WordForward)),
            (Platform::Windows, "delete", del(D::CharForward)),
            (Platform::Windows, "shift-down", mv(M::Down, true)),
        ];
        for (platform, spec, want) in cases {
            assert_eq!(
                resolve_spec(*platform, spec),
                *want,
                "{platform:?} の {spec} の解き方が表と違う"
            );
        }
    }

    /// その OS で押せない / 別の役目を持つ打鍵は受理しない（#763 の規則）
    #[test]
    fn そのosの慣習に無い打鍵は受理しない() {
        let rejected: &[(Platform, &str)] = &[
            // macOS の ⌃←→ は Mission Control。Windows 流の Ctrl+← を混ぜない
            (Platform::MacOs, "ctrl-left"),
            (Platform::MacOs, "ctrl-home"),
            (Platform::MacOs, "ctrl-backspace"),
            // Windows の Alt+矢印はペインのフォーカス移動（キーバインド側が持つ）
            (Platform::Windows, "alt-left"),
            (Platform::Windows, "alt-backspace"),
            // Windows の Win キーは OS のシェルが持っていく
            (Platform::Windows, "cmd-up"),
            (Platform::Windows, "cmd-left"),
            (Platform::Windows, "cmd-backspace"),
            // 表に無い組み合わせ（修飾の足し過ぎ）
            (Platform::MacOs, "cmd-alt-left"),
            (Platform::MacOs, "alt-enter"),
            (Platform::MacOs, "cmd-escape"),
            (Platform::Windows, "ctrl-alt-left"),
            // 印字文字は表の外（入力は IME / テキスト入力の経路）
            (Platform::MacOs, "a"),
            (Platform::MacOs, "alt-a"),
            // #1742: Emacs 系は macOS だけ（Windows の Ctrl+A は「すべて選択」の慣習）
            (Platform::Windows, "ctrl-a"),
            (Platform::Windows, "ctrl-e"),
            (Platform::Windows, "ctrl-k"),
            (Platform::MacOs, "cmd-a"),
            (Platform::MacOs, "alt-k"),
            // Ctrl+Tab はタブ切替（キーバインド側が持つ。#1654 で Tab を足しても受理しない）
            (Platform::Windows, "ctrl-tab"),
            (Platform::MacOs, "alt-tab"),
        ];
        for (platform, spec) in rejected {
            assert_eq!(
                resolve_spec(*platform, spec),
                None,
                "{platform:?} の {spec} を受理してしまう"
            );
        }
    }

    /// 1 つの打鍵が 2 つの操作に割り当たっていない（先に並んだ行が黙って勝つ）
    #[test]
    fn 同じ打鍵を2つの操作へ割り当てない() {
        for platform in BOTH {
            let chords: Vec<(Action, Chord)> = all_chords(platform).collect();
            for (i, (action, chord)) in chords.iter().enumerate() {
                for (other_action, other) in &chords[i + 1..] {
                    assert!(
                        chord != other,
                        "{platform:?} の {} が {action:?} と {other_action:?} の両方にある",
                        chord.gpui_spec()
                    );
                }
            }
        }
    }

    /// 移動・削除の全種類がどちらかの OS で押せる（表から落ちた操作が無い）
    #[test]
    fn 移動と削除の全種類が表に載っている() {
        // 桁 0 の行頭（line-start）も macOS の ⌃A が持つので例外は無い（#1742）
        for movement in CursorMovement::ALL {
            assert!(
                TABLE.iter().any(|e| e.action == Action::Move(movement)
                    && !(e.macos.is_empty() && e.windows.is_empty())),
                "{movement:?} に打鍵が無い"
            );
        }
        for motion in DeleteMotion::ALL {
            assert!(
                TABLE.iter().any(|e| e.action == Action::Delete(motion)),
                "{motion:?} に打鍵が無い"
            );
        }
        // 語の移動・削除と文書端は**両 OS で**押せる（Issue の本体）
        for action in [
            Action::Move(CursorMovement::WordLeft),
            Action::Move(CursorMovement::WordRight),
            Action::Move(CursorMovement::DocumentStart),
            Action::Move(CursorMovement::DocumentEnd),
            Action::Move(CursorMovement::PageUp),
            Action::Move(CursorMovement::PageDown),
            Action::Move(CursorMovement::SmartLineStart),
            Action::Delete(DeleteMotion::WordBackward),
            Action::Delete(DeleteMotion::WordForward),
        ] {
            let entry = TABLE.iter().find(|e| e.action == action).expect("行がある");
            for platform in BOTH {
                assert!(
                    !entry.chords(platform).is_empty(),
                    "{action:?} が {platform:?} で押せない"
                );
            }
        }
    }

    /// #1742: ⌃K は行末まで消し、行末にいれば改行を 1 つ（CRLF なら CR ごと）消す。
    /// どれも undo 1 回で戻る。打鍵表から引いて `apply` で当てる（GUI と CLI / MCP の口）
    #[test]
    fn ctrl_kは行末まで消し行末では改行を1つ消す() {
        use crate::text_edit::TextBuffer;
        use std::path::PathBuf;
        let kill = resolve_spec(Platform::MacOs, "ctrl-k").expect("ctrl-k は macOS の表にある");
        // (本文, カーソル, ⌃K の後の本文, カーソル)
        let cases: &[(&str, usize, &str, usize)] = &[
            // 行の途中: 行末まで消す（次の行は触らない）
            ("hello world\nnext\n", 6, "hello \nnext\n", 6),
            // 行末: 改行を 1 つ消して次の行とつなげる
            ("hello\nnext\n", 5, "hellonext\n", 5),
            // CRLF の行末: CR と LF をまとめて 1 つの改行として消す
            ("ab\r\ncd\r\n", 2, "abcd\r\n", 2),
            // CRLF の行の途中: CR は残す（行末までは CR の手前まで）
            ("abcd\r\nef", 1, "a\r\nef", 1),
            // 空行: その改行を消す
            ("a\n\nb", 2, "a\nb", 2),
            // 行頭: 行の中身だけ消えて空行が残る
            ("abc\ndef", 4, "abc\n", 4),
            // 全角を含む行
            (
                "日本語のテキスト\n",
                "日本語".len(),
                "日本語\n",
                "日本語".len(),
            ),
        ];
        for (text, cursor, want, want_cursor) in cases {
            let mut buffer = TextBuffer::from_text(PathBuf::from("kill.txt"), text.to_string());
            buffer.set_cursor(*cursor, false);
            assert!(kill.apply(&mut buffer));
            assert_eq!(buffer.text(), *want, "{text:?} の {cursor} で ctrl-k");
            assert_eq!(
                buffer.cursor(),
                *want_cursor,
                "{text:?} の ctrl-k 後のカーソル"
            );
            assert!(buffer.undo(), "{text:?} の ctrl-k を undo できない");
            assert_eq!(buffer.text(), *text, "{text:?} が undo 1 回で戻らない");
        }
        // 文書の末尾: 消すものが無い = 本文も版も変わらない
        let mut buffer = TextBuffer::from_text(PathBuf::from("kill.txt"), "abc".into());
        buffer.set_cursor(3, false);
        let version = buffer.version();
        kill.apply(&mut buffer);
        assert_eq!((buffer.text(), buffer.version()), ("abc", version));
        // 続けて押すと 1 回ずつ undo で戻る（語・行単位の削除はまとめない = #1652）
        let mut buffer = TextBuffer::from_text(PathBuf::from("kill.txt"), "ab\ncd\n".into());
        kill.apply(&mut buffer);
        kill.apply(&mut buffer);
        assert_eq!(buffer.text(), "cd\n");
        assert!(buffer.undo());
        assert_eq!(buffer.text(), "\ncd\n");
    }

    /// #1742: ⌃A は smart Home ではなく**桁 0**（インデントの直後でも行き来しない）、
    /// ⌃E は行末（CRLF なら CR の手前）
    #[test]
    fn ctrl_aは桁0へctrl_eは行末へ() {
        use crate::text_edit::TextBuffer;
        use std::path::PathBuf;
        let home = resolve_spec(Platform::MacOs, "ctrl-a").expect("ctrl-a");
        let end = resolve_spec(Platform::MacOs, "ctrl-e").expect("ctrl-e");
        let mut buffer =
            TextBuffer::from_text(PathBuf::from("ae.rs"), "    let x = 1;\r\nnext".into());
        buffer.set_cursor("    let".len(), false);
        home.apply(&mut buffer);
        assert_eq!(buffer.cursor(), 0);
        // もう 1 回押しても桁 0 のまま（Home はインデントの直後へ戻る）
        home.apply(&mut buffer);
        assert_eq!(buffer.cursor(), 0);
        buffer.set_cursor(4, false);
        home.apply(&mut buffer);
        assert_eq!(buffer.cursor(), 0, "インデントの直後からも桁 0");
        end.apply(&mut buffer);
        assert_eq!(buffer.cursor(), "    let x = 1;".len(), "行末は CR の手前");
        // ⇧ 付きは選択を伸ばす
        let select_home = resolve_spec(Platform::MacOs, "ctrl-shift-a").expect("ctrl-shift-a");
        select_home.apply(&mut buffer);
        assert_eq!(buffer.selection(), Some(0.."    let x = 1;".len()));
    }

    /// `gpui_spec` の綴りが読み戻せる（キーバインドとの突き合わせはこの綴りで行う）
    #[test]
    fn 打鍵の綴りは往復する() {
        for platform in BOTH {
            for (action, chord) in all_chords(platform) {
                let spec = chord.gpui_spec();
                let (key, mods) = parse(&spec);
                assert_eq!(key, chord.key, "{spec}");
                assert!(!mods.shift);
                assert_eq!(
                    resolve(platform, key, mods),
                    Some(action.with_shift(false)),
                    "{platform:?} の {spec}"
                );
            }
        }
    }

    /// #1654: CLI / MCP の編集コマンド名は一意に往復する
    #[test]
    fn 編集コマンドの名前は往復する() {
        for name in EditorCommand::edit_names() {
            let command = EditorCommand::from_edit_name(name).expect("表にある名前");
            assert_ne!(command, EditorCommand::ExitEditing);
        }
        assert_eq!(
            EditorCommand::from_edit_name("indent"),
            Some(EditorCommand::Indent)
        );
        assert_eq!(EditorCommand::from_edit_name("tab"), None);
        let mut names = EditorCommand::edit_names();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), EDIT_COMMAND_NAMES.len());
    }
}
