//! 引き継ぎの後任 master が前任の worker 領域を継ぐ番犬（#2133）
//!
//! # なぜ要るか
//!
//! master-reserved（既定）の worker 領域は、各ペインの `spawned_by` が spawn 元へ届くかで
//! 決まる。引き継ぎ（#749 / #917）の後任は `spawned_by = 前任` で立ち、前任の worker は
//! 前任を指したまま残るので、前任を閉じたあと後任から spawn すると**後任自身が右分割され**、
//! 本番では master が 14 桁・新しい worker が 28 桁まで潰れた。
//!
//! 直し方は 2 段で、どちらが欠けても戻る:
//!
//! 1. 前任 master を閉じた経路が `Workspace::hand_over_spawned` を通す
//!    （dispatch の `Request::Close` と、GUI の × / exit の `remove_pane_with`）。
//!    通さないと引き継ぎを重ねたときに worker が誰の領域にも入らなくなる
//! 2. 木の中の判定（`PaneTree::spawn_descendants_of`）が後任の系譜
//!    （`spawn_owners`）を見て、閉じた spawn 元を後任へ読み替える（`live_spawn_anchor`）。
//!    前任と後任が並んでいる間と、付け替えの前に保存した木の復元はこちらが受ける
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部緑になるので [`走査が空振りしていない`] で窓を固定し、
//! [`逆戻りを名指しできる`] で修正前を再現した注入が `file:line` で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const PANE_TREE: &str = "crates/tako-core/src/pane_tree.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（テスト領域は空白へ潰れる。行番号は原文のまま）
fn production(rel: &'static str, src: &str) -> String {
    production_range::production_with_floor(src, rel, 0.2)
}

/// 関数（match の腕）の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close || **l == format!("{close},"))
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// 閉じる経路の窓: `.close(` の後に `hand_over_spawned(` があること
fn scan_close_path(
    rel: &'static str,
    src: &str,
    needle: &str,
    close_call: &str,
    out: &mut Vec<String>,
) {
    let view = production(rel, src);
    let Some((at, window)) = fn_window(&view, needle) else {
        out.push(format!(
            "{rel}:0 — `{needle}` の窓が採れない（走査が空振り）"
        ));
        return;
    };
    let Some(close) = code_line_with(&window, close_call) else {
        out.push(format!(
            "{rel}:{at} — `{needle}` の中に `{close_call}` が無い（走査が空振り）"
        ));
        return;
    };
    match code_line_with(&window, "hand_over_spawned(") {
        Some(h) if h > close => {}
        Some(h) => out.push(format!(
            "{rel}:{} — 木から外す前に引き渡している（閉じたペインの role / spawned_by が要る。#2133）",
            at + h
        )),
        None => out.push(format!(
            "{rel}:{} — 前任 master を閉じても worker の spawn 元を後任へ付け替えていない（後任からの spawn が後任を割る。#2133）",
            at + close
        )),
    }
}

/// 木の中の判定（`pane_tree.rs`）: 後任の系譜を見て、閉じた spawn 元を読み替えること
fn scan_pane_tree(src: &str, out: &mut Vec<String>) {
    let view = production(PANE_TREE, src);
    for (needle, must, why) in [
        (
            "fn spawn_descendants_of(",
            "self.spawn_owners(",
            "worker の判定が spawn 元 1 枚だけを見ている（後任は前任の worker を自分の worker と見なせない）",
        ),
        (
            "pub fn reflow_workers(",
            "self.live_spawn_anchor(",
            "閉じた前任を指す worker を閉じたとき、後任の領域を組み直さない",
        ),
        (
            "pub fn worker_area_panes(",
            "self.live_spawn_anchor(",
            "閉じた前任を渡されたとき、後任の領域を返さない（フォントの当て直しが空振りする）",
        ),
    ] {
        match fn_window(&view, needle) {
            None => out.push(format!("{PANE_TREE}:0 — `{needle}` の窓が採れない（走査が空振り）")),
            Some((at, window)) => {
                if code_line_with(&window, must).is_none() {
                    out.push(format!("{PANE_TREE}:{at} — {why}（#2133）"));
                }
            }
        }
    }
}

fn all(dispatch: &str, app: &str, pane_tree: &str) -> Vec<String> {
    let mut out = Vec::new();
    scan_close_path(
        DISPATCH,
        dispatch,
        "Request::Close {",
        ".close(target)",
        &mut out,
    );
    scan_close_path(
        APP,
        app,
        "fn remove_pane_with(",
        ".close(pane_id)",
        &mut out,
    );
    scan_pane_tree(pane_tree, &mut out);
    out
}

fn sources() -> (String, String, String) {
    let root = workspace_root();
    (
        read(&root, DISPATCH),
        read(&root, APP),
        read(&root, PANE_TREE),
    )
}

/// 走査が窓を採れていること（空振りなら他の検査は全部無意味に緑）
#[test]
fn 走査が空振りしていない() {
    let (dispatch, app, pane_tree) = sources();
    for (rel, src, needle, inner) in [
        (DISPATCH, &dispatch, "Request::Close {", ".close(target)"),
        (APP, &app, "fn remove_pane_with(", ".close(pane_id)"),
        (
            PANE_TREE,
            &pane_tree,
            "fn spawn_descendants_of(",
            "workers.insert(id)",
        ),
        (
            PANE_TREE,
            &pane_tree,
            "pub fn reflow_workers(",
            "rebuild_worker_area(",
        ),
        (
            PANE_TREE,
            &pane_tree,
            "pub fn worker_area_panes(",
            "all_workers(far",
        ),
    ] {
        let view = production(rel, src);
        let (_, window) =
            fn_window(&view, needle).unwrap_or_else(|| panic!("{rel} の窓 `{needle}` が採れない"));
        assert!(
            window.contains(inner),
            "{rel} の窓 `{needle}` に `{inner}` が無い（窓の終わりの判定が壊れている）"
        );
    }
}

/// 現行のソースは違反ゼロ
#[test]
fn 現行実装は後任へ引き渡している() {
    let (dispatch, app, pane_tree) = sources();
    let found = all(&dispatch, &app, &pane_tree);
    assert!(found.is_empty(), "#2133 の逆戻り:\n{}", found.join("\n"));
}

/// 修正前を再現した注入が file:line で名指しされること（見逃す側へ倒れない）
#[test]
fn 逆戻りを名指しできる() {
    let (dispatch, app, pane_tree) = sources();

    // 注入 1: dispatch の Close が引き渡さない（修正前）
    let inj = dispatch.replace(
        "let _ = host.workspace_mut().hand_over_spawned(&closed_pane);",
        "let _ = &closed_pane;",
    );
    assert!(inj != dispatch, "注入 1 の対象が見つからない");
    let found = all(&inj, &app, &pane_tree);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(DISPATCH) && o.contains("付け替えていない")),
        "dispatch の Close の引き渡し漏れを名指しできていない: {found:?}"
    );

    // 注入 2: GUI の × / exit が引き渡さない（修正前）
    let inj = app.replace(
        "let _ = self.workspace.hand_over_spawned(&closed_pane);",
        "let _ = &closed_pane;",
    );
    assert!(inj != app, "注入 2 の対象が見つからない");
    let found = all(&dispatch, &inj, &pane_tree);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(APP) && o.contains("付け替えていない")),
        "GUI の close の引き渡し漏れを名指しできていない: {found:?}"
    );

    // 注入 3: worker の判定が spawn 元 1 枚だけに戻る（修正前の `parent == anchor`）
    let inj = pane_tree.replace(
        "        let owners = self.spawn_owners(anchor);\n        let panes = self.panes();",
        "        let owners = std::collections::HashSet::from([anchor]);\n        let panes = self.panes();",
    );
    assert!(inj != pane_tree, "注入 3 の対象が見つからない");
    let found = all(&dispatch, &app, &inj);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(PANE_TREE) && o.contains("spawn 元 1 枚だけ")),
        "系譜を見ない判定への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 4: リフローが閉じた spawn 元を読み替えない（修正前の `contains` だけ）
    let inj = pane_tree.replace(
        "        let Some(anchor) = self.live_spawn_anchor(anchor) else {\n            return false;\n        };",
        "        if !self.contains(anchor) {\n            return false;\n        }",
    );
    assert!(inj != pane_tree, "注入 4 の対象が見つからない");
    let found = all(&dispatch, &app, &inj);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(PANE_TREE) && o.contains("組み直さない")),
        "リフローの読み替え漏れを名指しできていない: {found:?}"
    );
}
