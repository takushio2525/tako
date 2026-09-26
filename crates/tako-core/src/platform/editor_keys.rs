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
//!   飛ばさない
//! - **Windows は Windows 標準の慣習**（Ctrl = 語、Ctrl+Home / End = 文書端）。
//!   **Alt+矢印はペインのフォーカス移動に張ってある**（`keybindings.rs`。GPUI は
//!   キーバインドを入口より先に発火して伝播を止める）ので、ここで使うと届かない。
//!   衝突しないことは tako-app の番犬 `エディタの打鍵表はキーバインドと衝突しない` が
//!   **両 OS について**検査する
//! - **その OS で案内できない打鍵は受理もしない**（#763 と同じ規則）。macOS の
//!   ⌃←→ は Mission Control が持っていき、Windows の Win キーは OS のシェルが持って
//!   いくので、どちらも列に書かない
//! - **shift は「選択を伸ばす」**。移動の行だけが shift を読み、削除・改行・
//!   編集終了は shift の有無を問わない（従来どおり ⇧⌫ は ⌫ と同じ）
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
    /// 改行を入れる
    Newline,
    /// 編集モードを抜ける
    ExitEditing,
}

impl EditorCommand {
    /// バッファへ当てる（#1652）。**GUI の打鍵と CLI / MCP が同じここを通る**。
    ///
    /// 編集モードを抜ける（[`Self::ExitEditing`]）はバッファの操作ではないので
    /// 何もせず `false` を返す（編集モードを閉じるのは呼び手 = GUI の仕事）
    pub fn apply(self, buffer: &mut TextBuffer) -> bool {
        match self {
            Self::Move { movement, extend } => buffer.move_cursor(movement, extend),
            Self::Delete(motion) => buffer.delete(motion),
            Self::Newline => buffer.newline(),
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

use Action::{Delete, ExitEditing, Move, Newline};
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
    // --- 行（Home は smart Home。macOS は ⌘←→ も）---
    row(
        Move(CursorMovement::SmartLineStart),
        &[K::plain("home"), K::cmd("left")],
        &[K::plain("home")],
    ),
    row(
        Move(CursorMovement::LineEnd),
        &[K::plain("end"), K::cmd("right")],
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
    // 行頭 / 行末までの削除は macOS の慣習だけ（Windows に標準の打鍵が無い）
    row(
        Delete(DeleteMotion::ToLineStart),
        &[K::cmd("backspace")],
        &[],
    ),
    row(Delete(DeleteMotion::ToLineEnd), &[K::cmd("delete")], &[]),
    // --- 改行・編集終了 ---
    row(Newline, &[K::plain("enter")], &[K::plain("enter")]),
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
            (Platform::MacOs, "escape", Some(EditorCommand::ExitEditing)),
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
            (Platform::Windows, "ctrl-a"),
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
        for movement in CursorMovement::ALL {
            // 桁 0 の行頭（line-start）は CLI / MCP から指す口で、打鍵は smart Home が持つ
            if movement == CursorMovement::LineStart {
                continue;
            }
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
}
