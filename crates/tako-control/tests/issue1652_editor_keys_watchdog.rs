//! **#1652 の番犬**: エディタの入口が修飾キー付きの打鍵を捨てる形へ戻らないようにする。
//!
//! ## 何が起きていたのか（修正前）
//!
//! ```text
//! if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
//!     return false;
//! }
//! ```
//!
//! プレビュー編集のキー入口（`handle_preview_edit_key`）の頭にこれがあり、⌥ / ⌃ / ⌘ の
//! どれかが立った打鍵は**全部捨てられていた**。⌥← の単語移動・⌥⌫ の単語削除・⌘↑ の
//! 文書端・Windows の Ctrl+← のどれを押しても何も起きない。`CursorMovement` は 8 種しか
//! 無く、Home は桁 0 へ直行、↓ を続けると短い行に桁が吸われて戻らなかった
//! （実測 (0,8) → ↓ (1,2) → ↓ (2,2)）。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`入口は修飾キーを見て打鍵を捨てない`] — Issue の本体。入口の本体で
//!    `modifiers.platform / control / alt` を読んだら、その行を file:line で名指す
//! 2. [`打鍵の解釈は表の1か所だけ`] — 入口がキー名（`"left"` / `"backspace"`）を直書きして
//!    自前で解く形。表（`tako_core::platform::editor_keys`）を引く口は 1 つだけ
//! 3. [`移動と削除の中身はtako_coreが持つ`] — GUI が `move_cursor` / `delete_backward` を
//!    直接呼んで UI 層に編集ロジックを持つ形（CLI / MCP と挙動が割れる）
//! 4. [`打鍵とcliとmcpは同じ口を通る`] — protocol / dispatch / CLI / MCP 写像 / カタログの
//!    どれかが欠ける形と、dispatch が GUI の打鍵と別の口を呼ぶ形
//! 5. [`変換中は本文に触らずに飲み込む`] — IME の未確定中に修飾キーの移動・削除が
//!    確定文字列の挿入先を動かす形
//! 6. [`画面の選択の写し戻しは1回で置く`] — `set_cursor` の 2 段で写し戻して、選択が
//!    あるときだけ桁の記憶（desired column）が毎打鍵で切れる形
//! 7. [`利用者向けの一覧は打鍵表と一致する`] — docs の「編集中の操作」表と打鍵表のずれ
//! 8. [`enterはインデントを引き継ぐ口を通る`]（#1654）— Enter が素の改行へ戻り、
//!    `fn main() {` の中で改行すると次行が桁 0 になる形（Issue #1654 の実測 E8）
//! 9. [`tabは両osの表に載っている`]（#1654）— Tab / ⇧Tab が表から落ちて入口を素通りする形
//!
//! 表の中身（両 OS の列・衝突・受理しない打鍵）は `tako_core::platform::editor_keys` の
//! 単体、キーバインドとの衝突は tako-app の `エディタの打鍵表はキーバインドと衝突しない`、
//! 移動・削除の境界（多バイト・CRLF・空行・文書端）は `tako_core::text_edit` の単体、
//! 実 GUI の打鍵経路は visual-test 節 `editor-keys`。ここは**配線と入口の形**だけを見る。
//!
//! 落ちるときは **file:line で名指し**する。

use std::path::{Path, PathBuf};

use tako_core::platform::editor_keys::{self, EditorCommand, KeyMods};
use tako_core::platform::support::Platform;
use tako_core::source_scan::fn_head_name;
use tako_core::text_edit::{CursorMovement, DeleteMotion};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const APP: &str = "crates/tako-app/src/main.rs";
const PROTOCOL: &str = "crates/tako-control/src/protocol.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const EDITOR_KEYS: &str = "crates/tako-core/src/platform/editor_keys.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードだけの眺め（コメントは潰す / 文字列は囲みごと残す。#1609）
fn prod_view(rel: &str) -> String {
    let src = read(rel);
    without_comments_checked(&production_range::production(&src, rel), rel)
}

/// 関数 1 本の本体（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: &'static str,
    name: &'static str,
    line: usize,
    /// コメントだけ潰した眺めでの本体（リテラルの中身が残っている）
    text: String,
}

impl Body {
    /// 本体の中で `needle` が現れる行（file:line の line）をすべて返す
    fn lines_of(&self, needle: &str) -> Vec<usize> {
        self.text
            .match_indices(needle)
            .map(|(at, _)| self.line + self.text[..at].bytes().filter(|b| *b == b'\n').count())
            .collect()
    }

    fn at(&self, line: usize) -> String {
        format!("{}:{line}", self.rel)
    }

    fn head(&self) -> String {
        format!("{}:{} fn {}", self.rel, self.line, self.name)
    }
}

/// 関数の本体を名前で引く。**見つからなければ落とす**（改名で番犬が空振りしない）
fn body(rel: &'static str, name: &'static str) -> Body {
    let src = read(rel);
    let prod = production_range::production(&src, rel);
    // 波括弧の対応は「コメントも文字列も潰した眺め」で数える（中の括弧を数えない）
    let code = code_view(&prod);
    let view = without_comments_checked(&prod, rel);
    assert_eq!(
        code.len(),
        view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fn_head_name(line.trim_start()) != Some(name) {
            continue;
        }
        let open = code[start..]
            .find('{')
            .map(|i| i + start)
            .unwrap_or_else(|| panic!("{rel}: fn {name} の本体が無い"));
        let mut depth = 0usize;
        let mut end = open;
        for (i, byte) in code[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        return Body {
            rel,
            name,
            line: code[..start].bytes().filter(|b| *b == b'\n').count() + 1,
            text: view[open..end].to_string(),
        };
    }
    panic!("{rel}:1 fn {name} が見つからない（改名したらこの番犬も直す）");
}

// --- 1) Issue の本体 ---------------------------------------------------------

/// 入口の本体は修飾キーを**読まない**。修飾キーの意味は表だけが決める。
///
/// 修正前の `if modifiers.platform || control || alt { return false }` を戻すと、
/// その行を file:line で名指して落ちる（A/B の注入はこの形で検出力を確かめた）
#[test]
fn 入口は修飾キーを見て打鍵を捨てない() {
    let entry = body(APP, "handle_preview_edit_key");
    let mut found = Vec::new();
    for needle in [
        "modifiers.platform",
        "modifiers.control",
        "modifiers.alt",
        "modifiers.function",
    ] {
        for line in entry.lines_of(needle) {
            found.push(format!(
                "{} 入口の本体が `{needle}` を読んでいる（修飾キー付きの打鍵を入口で弾くと \
                 ⌥← / ⌥⌫ / ⌘↑ / Ctrl+← が届かない = #1652）",
                entry.at(line)
            ));
        }
    }
    assert!(
        found.is_empty(),
        "エディタの入口が修飾キーを見ている:\n{}\n→ 打鍵の意味は \
         crates/tako-core/src/platform/editor_keys.rs の表へ書く",
        found.join("\n")
    );
}

// --- 2) 表は 1 か所 ----------------------------------------------------------

/// 入口は表を引くだけで、キー名を直書きして自前で解かない
#[test]
fn 打鍵の解釈は表の1か所だけ() {
    let entry = body(APP, "handle_preview_edit_key");
    assert!(
        !entry.lines_of("editor_key_command(").is_empty(),
        "{} が表を引く口 editor_key_command を通っていない",
        entry.head()
    );
    let resolver = body(APP, "editor_key_command");
    assert!(
        !resolver.lines_of("editor_keys::resolve(").is_empty(),
        "{} が tako_core::platform::editor_keys::resolve を引いていない",
        resolver.head()
    );
    // 入口にキー名のリテラルが戻る = 表の外で打鍵を解いている
    let mut literal = Vec::new();
    for key in [
        "\"left\"",
        "\"right\"",
        "\"up\"",
        "\"down\"",
        "\"home\"",
        "\"end\"",
        "\"pageup\"",
        "\"pagedown\"",
        "\"backspace\"",
        "\"delete\"",
        "\"enter\"",
        "\"escape\"",
        "\"tab\"",
    ] {
        for line in entry.lines_of(key) {
            literal.push(format!(
                "{} キー名 {key} を入口で直書きしている",
                entry.at(line)
            ));
        }
    }
    assert!(
        literal.is_empty(),
        "打鍵を表の外で解いている:\n{}\n→ crates/tako-core/src/platform/editor_keys.rs の表へ足す",
        literal.join("\n")
    );
    // 表を引く口は 1 本（別の入口が自前で resolve すると、片方だけ A/B の口を持つ等で割れる）
    let view = prod_view(APP);
    let calls = view.matches("editor_keys::resolve(").count();
    assert_eq!(
        calls, 1,
        "{APP}:1 editor_keys::resolve の呼び出しが {calls} か所ある（入口は editor_key_command の 1 つ）"
    );
}

/// Issue の打鍵が表で両 OS とも解ける（表から Windows の列が落ちる形を止める）
#[test]
fn issueの打鍵は両osで表に載っている() {
    let mods = |alt: bool, control: bool, platform: bool| KeyMods {
        shift: false,
        alt,
        control,
        platform,
    };
    let mv = |movement| {
        Some(EditorCommand::Move {
            movement,
            extend: false,
        })
    };
    let cases = [
        (
            Platform::MacOs,
            "left",
            mods(true, false, false),
            mv(CursorMovement::WordLeft),
        ),
        (
            Platform::MacOs,
            "right",
            mods(true, false, false),
            mv(CursorMovement::WordRight),
        ),
        (
            Platform::MacOs,
            "up",
            mods(false, false, true),
            mv(CursorMovement::DocumentStart),
        ),
        (
            Platform::MacOs,
            "down",
            mods(false, false, true),
            mv(CursorMovement::DocumentEnd),
        ),
        (
            Platform::MacOs,
            "backspace",
            mods(true, false, false),
            Some(EditorCommand::Delete(DeleteMotion::WordBackward)),
        ),
        (
            Platform::MacOs,
            "delete",
            mods(true, false, false),
            Some(EditorCommand::Delete(DeleteMotion::WordForward)),
        ),
        (
            Platform::Windows,
            "left",
            mods(false, true, false),
            mv(CursorMovement::WordLeft),
        ),
        (
            Platform::Windows,
            "right",
            mods(false, true, false),
            mv(CursorMovement::WordRight),
        ),
        (
            Platform::Windows,
            "home",
            mods(false, true, false),
            mv(CursorMovement::DocumentStart),
        ),
        (
            Platform::Windows,
            "end",
            mods(false, true, false),
            mv(CursorMovement::DocumentEnd),
        ),
        (
            Platform::Windows,
            "backspace",
            mods(false, true, false),
            Some(EditorCommand::Delete(DeleteMotion::WordBackward)),
        ),
        (
            Platform::MacOs,
            "pagedown",
            mods(false, false, false),
            mv(CursorMovement::PageDown),
        ),
        (
            Platform::Windows,
            "pageup",
            mods(false, false, false),
            mv(CursorMovement::PageUp),
        ),
        (
            Platform::MacOs,
            "home",
            mods(false, false, false),
            mv(CursorMovement::SmartLineStart),
        ),
        (
            Platform::Windows,
            "home",
            mods(false, false, false),
            mv(CursorMovement::SmartLineStart),
        ),
    ];
    for (platform, key, m, want) in cases {
        assert_eq!(
            editor_keys::resolve(platform, key, m),
            want,
            "crates/tako-core/src/platform/editor_keys.rs:1 {platform:?} の {key}（{m:?}）が {want:?} に解けない"
        );
    }
}

// --- 3) 中身は tako-core ------------------------------------------------------

/// GUI は表の結果を `EditorCommand::apply` へ渡すだけで、移動・削除を自前で呼ばない
#[test]
fn 移動と削除の中身はtako_coreが持つ() {
    let runner = body(APP, "run_editor_command_local");
    assert!(
        !runner.lines_of(".apply(&mut edit.buffer)").is_empty(),
        "{} が EditorCommand::apply（tako-core の 1 実装）を通っていない",
        runner.head()
    );
    let mut direct = Vec::new();
    for needle in [
        ".move_cursor(",
        ".delete_backward(",
        ".delete_forward(",
        ".delete(",
        ".newline(",
    ] {
        for line in runner.lines_of(needle) {
            direct.push(format!(
                "{} {needle} を GUI が直接呼んでいる（打鍵の意味が CLI / MCP と割れる）",
                runner.at(line)
            ));
        }
    }
    let entry = body(APP, "handle_preview_edit_key");
    for needle in [
        ".move_cursor(",
        ".delete_backward(",
        ".delete_forward(",
        ".newline(",
    ] {
        for line in entry.lines_of(needle) {
            direct.push(format!(
                "{} {needle} を入口が直接呼んでいる",
                entry.at(line)
            ));
        }
    }
    assert!(direct.is_empty(), "{}", direct.join("\n"));
}

// --- 4) 1:1 の配線 -----------------------------------------------------------

/// 移動・削除は **tako-core + dispatch + CLI + MCP** の全部に在り、dispatch は
/// GUI の打鍵と**同じ口**（`run_editor_command_local`）へ届く
#[test]
fn 打鍵とcliとmcpは同じ口を通る() {
    let wiring: [(&str, &str, &str); 19] = [
        // #1654: Tab / ⇧Tab / Enter（MCP はツールを増やさず tako_preview_edit の command）
        (PROTOCOL, "PreviewEditCommand {", "protocol の Request"),
        (
            DISPATCH,
            "Request::PreviewEditCommand {",
            "dispatch のアーム",
        ),
        (
            DISPATCH,
            "EditorCommand::from_edit_name(",
            "綴りの解決（名前表 1 か所）",
        ),
        (
            MCP_REQUEST,
            "Request::PreviewEditCommand {",
            "MCP の command から Request への写像",
        ),
        (
            MCP_CATALOG,
            "EditorCommand::edit_names()",
            "MCP の enum（名前表から作る）",
        ),
        (CLI, "EditCommand::Indent {", "CLI サブコマンド"),
        (CLI, "EditCommand::Newline {", "CLI サブコマンド"),
        (PROTOCOL, "PreviewMove {", "protocol の Request"),
        (PROTOCOL, "PreviewDelete {", "protocol の Request"),
        (DISPATCH, "Request::PreviewMove {", "dispatch のアーム"),
        (DISPATCH, "Request::PreviewDelete {", "dispatch のアーム"),
        (
            DISPATCH,
            "host.run_preview_command(",
            "ControlHost 呼び出し",
        ),
        (
            MCP_REQUEST,
            "\"tako_preview_move\" => Request::PreviewMove",
            "MCP 名から Request への写像",
        ),
        (
            MCP_REQUEST,
            "\"tako_preview_delete\" => Request::PreviewDelete",
            "MCP 名から Request への写像",
        ),
        (
            MCP_CATALOG,
            "\"name\": \"tako_preview_move\"",
            "MCP カタログ",
        ),
        (
            MCP_CATALOG,
            "\"name\": \"tako_preview_delete\"",
            "MCP カタログ",
        ),
        (CLI, "EditCommand::Move {", "CLI サブコマンド"),
        (CLI, "EditCommand::Delete {", "CLI サブコマンド"),
        (
            APP,
            "fn run_preview_command(",
            "tako-app の ControlHost 実装",
        ),
    ];
    let mut missing = Vec::new();
    for (rel, needle, what) in wiring {
        if !prod_view(rel).contains(needle) {
            missing.push(format!("{rel}:1 {what}（{needle}）が無い"));
        }
    }
    assert!(
        missing.is_empty(),
        "移動 / 削除の 1:1 配線が欠けている:\n{}",
        missing.join("\n")
    );
    // CLI / MCP の口は打鍵と同じ run_editor_command_local へ届く
    let cli_path = body(APP, "run_preview_command_local");
    assert!(
        !cli_path
            .lines_of("self.run_editor_command_local(")
            .is_empty(),
        "{} が打鍵と同じ口（run_editor_command_local）を通っていない",
        cli_path.head()
    );
    let entry = body(APP, "handle_preview_edit_key");
    assert!(
        !entry.lines_of("self.run_editor_command_local(").is_empty(),
        "{} が run_editor_command_local を通っていない",
        entry.head()
    );
    // 綴りは tako-core の名前表 1 か所から解く（CLI と MCP で解釈が割れない）
    let view = prod_view(DISPATCH);
    for needle in ["CursorMovement::from_name(", "DeleteMotion::from_name("] {
        assert!(
            view.contains(needle),
            "{DISPATCH}:1 {needle} で綴りを解いていない"
        );
    }
    let catalog = prod_view(MCP_CATALOG);
    for needle in ["CursorMovement::names()", "DeleteMotion::names()"] {
        assert!(
            catalog.contains(needle),
            "{MCP_CATALOG}:1 MCP の enum を {needle} から作っていない（綴りを 2 か所に書くと割れる）"
        );
    }
}

// --- 5) IME ------------------------------------------------------------------

/// 変換中は編集コマンドを当てる前に飲み込む（未確定文字列の挿入先を動かさない）
#[test]
fn 変換中は本文に触らずに飲み込む() {
    let entry = body(APP, "handle_preview_edit_key");
    let guard = entry.lines_of("self.ime_composing_in(");
    let run = entry.lines_of("self.run_editor_command_local(");
    assert!(
        !guard.is_empty(),
        "{} が IME の変換中を見ていない（修飾キーの移動で確定文字列の挿入先が動く）",
        entry.head()
    );
    assert!(
        guard.iter().min() < run.iter().min(),
        "{} 変換中の判定（{:?}）が編集コマンドの実行（{:?}）より後ろにある",
        entry.head(),
        guard,
        run
    );
}

// --- 6) 桁の記憶 --------------------------------------------------------------

/// 画面の選択は `set_selection` で 1 回に置く（2 段の `set_cursor` は途中で「動いた」扱い）
#[test]
fn 画面の選択の写し戻しは1回で置く() {
    let sync = body(APP, "sync_editor_selection_from_preview");
    assert!(
        !sync.lines_of(".set_selection(").is_empty(),
        "{} が set_selection で写し戻していない",
        sync.head()
    );
    let two_step = sync.lines_of(".set_cursor(");
    assert!(
        two_step.is_empty(),
        "{} set_cursor の 2 段で写し戻している（選択があるとき ⇧↓ の桁の記憶が毎打鍵で切れる）",
        two_step
            .first()
            .map(|line| sync.at(*line))
            .unwrap_or_default()
    );
}

// --- 7) 利用者向けの一覧 -----------------------------------------------------

const DOC: &str = "docs/src/content/docs/guides/keyboard-shortcuts.md";

/// `<kbd>Alt</kbd>+<kbd>←</kbd> / <kbd>Home</kbd>` を `alt-left` / `home` の形へ。
/// 修飾の並びは `Chord::gpui_spec` と同じ（ctrl → alt → cmd）
fn doc_chords(cell: &str) -> Vec<String> {
    cell.split(" / ")
        .filter_map(|one| {
            let mut tokens = Vec::new();
            let mut rest = one;
            while let Some(i) = rest.find("<kbd>") {
                rest = &rest[i + "<kbd>".len()..];
                let j = rest.find("</kbd>")?;
                tokens.push(rest[..j].trim().to_string());
                rest = &rest[j + "</kbd>".len()..];
            }
            let (key, mods) = tokens.split_last()?;
            let key = match key.as_str() {
                "\u{2190}" => "left".to_string(),
                "\u{2192}" => "right".to_string(),
                "\u{2191}" => "up".to_string(),
                "\u{2193}" => "down".to_string(),
                other => other.to_lowercase().replace(' ', ""),
            };
            let mut spec = String::new();
            for (word, prefix) in [("Ctrl", "ctrl-"), ("Alt", "alt-"), ("Cmd", "cmd-")] {
                if mods.iter().any(|m| m == word) {
                    spec.push_str(prefix);
                }
            }
            // Shift は表の打鍵の外（移動では選択の拡張、Tab では向き）なので読んで捨てる
            assert!(
                mods.iter()
                    .all(|m| ["Ctrl", "Alt", "Cmd", "Shift"].contains(&m.as_str())),
                "{DOC}:1 編集中の操作の表に読めない修飾がある: {one}"
            );
            spec.push_str(&key);
            Some(spec)
        })
        .collect()
}

/// 利用者向けのショートカット一覧（docs）の「編集中の操作」表が打鍵表と一致する（両方向）。
///
/// 載っていない打鍵は発見できず、載っているのに表に無い打鍵は押しても効かない。
/// 修飾なしの矢印・Backspace・Delete・Enter・Esc は表の外の説明文で済ませる
#[test]
fn 利用者向けの一覧は打鍵表と一致する() {
    let md = read(DOC);
    let mut rows = 0usize;
    let mut in_table = false;
    let (mut mac, mut win) = (Vec::new(), Vec::new());
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            in_table = false;
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        if cells == ["編集中の操作", "macOS", "Windows"] {
            in_table = true;
            continue;
        }
        if !in_table || cells.iter().all(|c| c.chars().all(|ch| ch == '-')) {
            continue;
        }
        rows += 1;
        mac.extend(doc_chords(cells[1]));
        win.extend(doc_chords(cells[2]));
    }
    assert!(
        rows >= 10,
        "{DOC}:1 「編集中の操作」の表が見つからない（{rows} 行）"
    );
    let obvious = [
        "left",
        "right",
        "up",
        "down",
        "backspace",
        "delete",
        "enter",
        "escape",
    ];
    let mut problems = Vec::new();
    for (platform, docs) in [(Platform::MacOs, &mac), (Platform::Windows, &win)] {
        let table: Vec<String> = editor_keys::all_chords(platform)
            .map(|(_, chord)| chord.gpui_spec())
            .filter(|spec| !obvious.contains(&spec.as_str()))
            .collect();
        for spec in &table {
            if !docs.contains(spec) {
                problems.push(format!(
                    "{DOC}:1 {platform:?} の {spec} が表にあるのに docs に無い"
                ));
            }
        }
        for spec in docs.iter() {
            if !table.contains(spec) {
                problems.push(format!(
                    "{DOC}:1 {platform:?} の {spec} が docs にあるのに打鍵表に無い（押しても効かない）"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// --- 8) 9) インデント（#1654） -------------------------------------------------

/// Enter（表の `Newline`）はインデントを引き継ぐ `newline_and_indent` を通る。
/// 素の `newline()` へ戻すと、その行を file:line で名指して落ちる
#[test]
fn enterはインデントを引き継ぐ口を通る() {
    let apply = body(EDITOR_KEYS, "apply");
    // 素の改行へ戻した行を先に名指す（直す場所がそのまま分かる）
    let plain = apply.lines_of("buffer.newline()");
    assert!(
        plain.is_empty(),
        "{} Enter が素の改行を挿している（インデントを引き継がない = #1654 の実測 E8）",
        plain.first().map(|l| apply.at(*l)).unwrap_or_default()
    );
    assert!(
        !apply.lines_of("buffer.newline_and_indent()").is_empty(),
        "{} が Enter を newline_and_indent へ渡していない",
        apply.head()
    );
    // 挙動でも見る: Rust の `fn main() {` の直後で Enter を当てると 1 段深い行ができる
    let mut buffer = tako_core::TextBuffer::from_text(
        PathBuf::from("watchdog-1654.rs"),
        "fn main() {\n}\n".into(),
    );
    buffer.set_cursor("fn main() {".len(), false);
    let enter = editor_keys::resolve(Platform::MacOs, "enter", KeyMods::default())
        .expect("Enter は表にある");
    enter.apply(&mut buffer);
    assert_eq!(
        buffer.text(),
        "fn main() {\n    \n}\n",
        "{EDITOR_KEYS}:1 Enter の結果がインデントを引き継いでいない"
    );
}

/// Tab / ⇧Tab が両 OS の表に載っていて、それぞれ深く / 浅くへ解ける
#[test]
fn tabは両osの表に載っている() {
    for platform in [Platform::MacOs, Platform::Windows] {
        let tab = editor_keys::resolve(platform, "tab", KeyMods::default());
        let shift_tab = editor_keys::resolve(
            platform,
            "tab",
            KeyMods {
                shift: true,
                ..KeyMods::default()
            },
        );
        assert_eq!(
            tab,
            Some(EditorCommand::Indent),
            "{EDITOR_KEYS}:1 {platform:?} の Tab がインデントに解けない"
        );
        assert_eq!(
            shift_tab,
            Some(EditorCommand::Outdent),
            "{EDITOR_KEYS}:1 {platform:?} の Shift+Tab がアンインデントに解けない"
        );
    }
}

/// 走査が空振りしていないこと（関数が見つかり、本体に中身がある）
#[test]
fn 番犬の走査が成立している() {
    for name in [
        "handle_preview_edit_key",
        "editor_key_command",
        "run_editor_command_local",
        "run_preview_command_local",
        "sync_editor_selection_from_preview",
    ] {
        let b = body(APP, name);
        assert!(
            b.text.len() > 40,
            "{} の本体が短すぎる（走査が空振り）",
            b.head()
        );
    }
}
