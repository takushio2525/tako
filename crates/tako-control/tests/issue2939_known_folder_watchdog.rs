//! **#2939 の番犬**: スマホのファイル閲覧の既定のショートカット（デスクトップ・ダウンロード）を、
//! Windows では Known Folder（境界 B39 `tako_core::platform::known_folder`）で引く。
//!
//! ## 何が起きていたか
//!
//! `remote_shortcuts::defaults_for` は `home.join("Desktop")` / `home.join("Downloads")` の実在だけを見ていた。
//! デスクトップを OneDrive へ移した Windows（Known Folder Move。Windows 11 では多い構成）では
//! `~\Desktop` が無く、実際のデスクトップ（`~\OneDrive\デスクトップ`）が既定に出なかった。
//!
//! ## 何を固定するか（どの OS の CI でも落ちる）
//!
//! 1. [`既定のショートカットはホームの下の名前を直書きしない`] — `remote_shortcuts.rs` の本番コードに
//!    `"Desktop"` / `"Downloads"` の直書きが戻ると `file:line` で名指す（慣例の名前は境界の
//!    `KnownFolder::home_relative` の 1 か所）
//! 2. [`既定のショートカットは境界の口と検証判定を通る`] — 本番の引き方が境界の `known_folder::os_path` と
//!    検証プロセスの判定の 1 実装を引き、既定の並びがデスクトップ・ダウンロードの既知フォルダを持つ
//! 3. [`windowsの腕はshgetknownfolderpathを呼ぶ`] — 境界の Windows の腕が API を呼ぶ（`None` を返すだけの
//!    形へ戻ると、macOS / Windows のビルドとテストは緑のまま「Windows の解決」だけが消える）
//! 4. [`既知フォルダを引く口は境界の1本だけ`] — `SHGetKnownFolderPath` を呼ぶ本番コードは境界の 1 ファイルだけ
//!    （#2432 の `shell_integration` の写しが戻ると名指す）
//!
//! 実際に Known Folder の答えが .NET / シェルの答えと一致することは、Windows の CI の名指しの段
//! （`platform::known_folder` の `実機の既知フォルダはnetとシェルの答えと一致する`）が見る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const SHORTCUTS: &str = "crates/tako-core/src/remote_shortcuts.rs";
const BOUNDARY: &str = "crates/tako-core/src/platform/known_folder.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる）
fn production(rel: &str) -> Vec<String> {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::production(&src, rel)
        .lines()
        .map(str::to_string)
        .collect()
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
}

/// `<頭>` を含む行から、頭と同じ字下げの `}` までの行番号の範囲（0 始まり・両端を含む）
fn block(lines: &[String], rel: &str, head: &str) -> (usize, usize) {
    let start = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains(head))
        .unwrap_or_else(|| panic!("{rel} に `{head}` が見つからない（番犬の前提が崩れた）"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let closing = format!("{}}}", " ".repeat(indent));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with(&closing))
        .map(|i| start + 1 + i)
        .unwrap_or_else(|| panic!("{rel}:{} の `{head}` の終わりが見つからない", start + 1));
    (start, end)
}

/// 本番コードの、ホームの下の慣例の名前の直書き（`"Desktop"` / `"Downloads"`）を `file:line` で
fn hard_coded_names(lines: &[String], rel: &str) -> Vec<String> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .filter(|(_, l)| l.contains("\"Desktop\"") || l.contains("\"Downloads\""))
        .map(|(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
        .collect()
}

#[test]
fn 既定のショートカットはホームの下の名前を直書きしない() {
    let lines = production(SHORTCUTS);
    let offenders = hard_coded_names(&lines, SHORTCUTS);
    assert!(
        offenders.is_empty(),
        "#2939: 既定のショートカットがホームの下の名前を直書きしている。Windows ではデスクトップ・ダウンロードの\
         置き場は Known Folder で決まり（OneDrive へ移すと `~\\Desktop` は無い）、慣例の名前は境界の\
         `KnownFolder::home_relative` の 1 か所に置く:\n{}",
        offenders.join("\n")
    );
}

/// 番犬 1 が戻した形（修正前の `for (rel, label) in [("Desktop", …)]`）を実際に名指すこと
#[test]
fn 番犬1は修正前の形を名指す() {
    let before: Vec<String> = [
        "pub fn defaults_for(home: Option<&Path>) -> Vec<Shortcut> {",
        "    // 修正前の形（コメントの \"Desktop\" は数えない）",
        "    for (rel, label) in [(\"Desktop\", \"デスクトップ\"), (\"Downloads\", \"ダウンロード\")] {",
        "        let p = home.join(rel);",
        "    }",
        "}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let named = hard_coded_names(&before, SHORTCUTS);
    assert_eq!(named.len(), 1, "{named:?}");
    assert!(
        named[0].starts_with(&format!("{SHORTCUTS}:3:")),
        "{named:?}"
    );
}

#[test]
fn 既定のショートカットは境界の口と検証判定を通る() {
    let lines = production(SHORTCUTS);
    let mut missing = Vec::new();

    // 本番の引き方は境界の口・検証プロセスの判定・A/B の env の 3 つから組む
    let (s, e) = block(&lines, SHORTCUTS, "pub fn system() -> Self {");
    let system = lines[s..=e].join("\n");
    for needle in [
        "os_answer: &known_folder::os_path",
        "verification: paths::is_verification_process()",
        "legacy: legacy()",
    ] {
        if !system.contains(needle) {
            missing.push(format!(
                "{SHORTCUTS}:{}: `FolderLookup::system` が `{needle}` を通っていない",
                s + 1
            ));
        }
    }

    // 既定（`defaults_for`）と一覧・削除は本番の引き方を渡す
    for head in [
        "pub fn defaults_for(",
        "pub fn listing(&self",
        "pub fn remove(&mut self",
    ] {
        let (s, e) = block(&lines, SHORTCUTS, head);
        let body = lines[s..=e].join("\n");
        if !body.contains("FolderLookup::system()") {
            missing.push(format!(
                "{SHORTCUTS}:{}: `{head}` が本番の引き方（`FolderLookup::system()`）を渡していない",
                s + 1
            ));
        }
    }

    // 既定の並びはデスクトップ・ダウンロードの既知フォルダ（配列の頭から `];` まで）
    let s = lines
        .iter()
        .position(|l| l.starts_with("const DEFAULT_FOLDERS"))
        .unwrap_or_else(|| panic!("{SHORTCUTS} に `const DEFAULT_FOLDERS` が見つからない"));
    let e = lines[s..]
        .iter()
        .position(|l| l.starts_with("];"))
        .map(|i| s + i)
        .unwrap_or_else(|| panic!("{SHORTCUTS}:{} の既定の並びの終わりが見つからない", s + 1));
    let table = lines[s..=e].join("\n");
    for needle in ["KnownFolder::Desktop", "KnownFolder::Downloads"] {
        if !table.contains(needle) {
            missing.push(format!(
                "{SHORTCUTS}:{}: 既定の並びに `{needle}` が無い",
                s + 1
            ));
        }
    }

    // 置き場は引き方を通して決める（ホームの下を直に組まない）
    let (s, e) = block(&lines, SHORTCUTS, "pub fn defaults_with(");
    let body = lines[s..=e].join("\n");
    for needle in ["lookup.os_answers_apply(", "lookup.folder_under("] {
        if !body.contains(needle) {
            missing.push(format!(
                "{SHORTCUTS}:{}: `defaults_with` が `{needle}` を通っていない",
                s + 1
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "#2939: 既定のショートカットが Known Folder（境界 B39）を通っていない:\n{}",
        missing.join("\n")
    );
}

/// 境界の Windows の腕（`#[cfg(windows)]` の直後の `mod imp {` から字下げ 0 の `}` まで）
fn windows_arm(lines: &[String]) -> Option<(usize, usize)> {
    let start = lines.windows(2).position(|w| {
        w[0].trim() == "#[cfg(windows)]" && w[1].trim_start().starts_with("mod imp")
    })? + 1;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with('}'))
        .map(|i| start + 1 + i)?;
    Some((start, end))
}

/// Windows の腕が API を呼んでいない理由（呼んでいれば `None`）
fn windows_arm_problem(lines: &[String], rel: &str) -> Option<String> {
    let Some((s, e)) = windows_arm(lines) else {
        return Some(format!("{rel}: `#[cfg(windows)] mod imp` が見つからない"));
    };
    let calls = lines[s..=e]
        .iter()
        .any(|l| !is_comment(l) && l.contains("SHGetKnownFolderPath(&id"));
    (!calls).then(|| {
        format!(
            "{rel}:{}: Windows の腕が `SHGetKnownFolderPath` を呼んでいない（既知フォルダを答えない形）",
            s + 1
        )
    })
}

#[test]
fn windowsの腕はshgetknownfolderpathを呼ぶ() {
    let lines = production(BOUNDARY);
    if let Some(problem) = windows_arm_problem(&lines, BOUNDARY) {
        panic!(
            "#2939: {problem}。macOS / Windows のビルドとテストは緑のまま、OneDrive へ移したデスクトップが\
             既定から消える"
        );
    }
}

/// 番犬 3 が戻した形（Windows の腕が `None` を返すだけ）を実際に名指すこと
#[test]
fn 番犬3はnoneを返すだけの腕を名指す() {
    let stub: Vec<String> = [
        "#[cfg(not(windows))]",
        "mod imp {",
        "    pub(super) fn os_path(_id: super::Guid) -> Option<PathBuf> { None }",
        "}",
        "",
        "#[cfg(windows)]",
        "mod imp {",
        "    // SHGetKnownFolderPath(&id, 0, 0, &mut raw) はやめた",
        "    pub(super) fn os_path(_id: super::Guid) -> Option<PathBuf> { None }",
        "}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let problem = windows_arm_problem(&stub, BOUNDARY).expect("名指す");
    assert!(problem.starts_with(&format!("{BOUNDARY}:7:")), "{problem}");
}

/// 本番の Rust ソース（`crates/*/src` の下。テスト領域は潰す）。ファイルまるごとテストのもの
/// （`mcp/tests.rs` 等）も読むので、下限の確認を持たない `scan` を使う（`platform_parity` と同じ）
fn production_sources(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<String>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            production_sources(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            let lines = production_range::scan(&src)
                .text
                .lines()
                .map(str::to_string)
                .collect();
            out.push((rel, lines));
        }
    }
}

#[test]
fn 既知フォルダを引く口は境界の1本だけ() {
    let root = workspace_root();
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates を読める")
        .flatten()
    {
        let src = entry.path().join("src");
        if src.is_dir() {
            production_sources(&src, &root, &mut sources);
        }
    }
    assert!(
        sources.iter().any(|(rel, _)| rel == BOUNDARY),
        "番犬の前提が崩れた: {BOUNDARY} が走査に無い"
    );
    let offenders: Vec<String> = sources
        .iter()
        .filter(|(rel, _)| rel != BOUNDARY)
        .flat_map(|(rel, lines)| {
            lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !is_comment(l) && touches_known_folder_api(l))
                .map(move |(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "#2939: 既知フォルダを境界 B39（{BOUNDARY}）の外で引いている。GUID の表と呼び方を 1 か所に\
         置き、`known_folder::os_path` を通す:\n{}",
        offenders.join("\n")
    );
}

/// API の宣言か呼び出し（`SHGetKnownFolderPath(`）。対応表の証拠の文のような地の文の名前は数えない
fn touches_known_folder_api(line: &str) -> bool {
    line.contains("SHGetKnownFolderPath(")
}

/// 番犬 4 が #2432 の写し（宣言と呼び出し）を名指し、地の文の名前は名指さないこと
#[test]
fn 番犬4は写しの宣言と呼び出しだけを名指す() {
    for copied in [
        "            fn SHGetKnownFolderPath(",
        "        let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_DOCUMENTS, 0, 0, &mut raw) };",
    ] {
        assert!(touches_known_folder_api(copied), "{copied}");
    }
    assert!(!touches_known_folder_api(
        "            \"… Known Folder（`SHGetKnownFolderPath`・境界 B39）から引く …\","
    ));
}
