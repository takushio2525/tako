//! `tako list` の `backend_windows` が UI の表示状態に依存する形へ戻らないための番犬（#1191）
//!
//! 落とすのは 2 つの再発形:
//!
//! 1. **要求時に採らない**。旧実装は `Request::List => Ok(list_json(host))` で、値の更新は
//!    右パネル（fleet ビュー）の 2 秒ポーリングだけだった。パネルを開いたことがなければ
//!    常に `null`、閉じているあいだは最後に見た値が残る = **UI の表示状態で API の中身が
//!    変わる**。`tako list` / MCP `tako_list_panes` は AI が状況把握に使う一次情報なので、
//!    これは開発不変条件（設計原則 5）に反する
//! 2. **採取の可否とペインの種別を混ぜる**。`null`（backend でない / 採取できなかった）と
//!    `[]`（backend だが window が無い）を区別できないと、`remote.rs` のように
//!    「`backend_windows` がある = tmux backend ペイン」で判定している内部利用者が黙って劣化する
//!
//! ソース走査なので、実装が別の書き方に変わっても「その材料を見ているか」で落ちる。

use std::path::{Path, PathBuf};

/// 走査するツリー。`TAKO_1191_WATCHDOG_ROOT` で差し替えられる（A/B 専用:
/// 修正前のソースを取り出した一時ツリーを指すと、この番犬が落ちることを実測できる）
fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("TAKO_1191_WATCHDOG_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読めない: {e}"))
}

/// `needle` で始まるブロックを、波括弧の対応で切り出す（見つからなければ panic）
fn block_after(src: &str, needle: &str) -> String {
    let start = src
        .find(needle)
        .unwrap_or_else(|| panic!("目印 `{needle}` がソースに無い（実装が動いた?）"));
    let after = start + needle.len();
    let open = after + src[after..].find('{').expect("ブロックの開き波括弧が無い");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[start..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("ブロックが閉じていない: {needle}");
}

/// コメント行を落とす（説明文が検査対象の形と一致して誤検知するのを防ぐ）
fn code_only(block: &str) -> String {
    block
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// legacy（A/B 用の再現アーム）の行は検査対象から外す。
/// `TAKO_1191_LEGACY=1` は**修正前の挙動をわざと再現する**ので、
/// そこに旧実装の形が残っているのは正しい
fn without_legacy(block: &str) -> String {
    block
        .lines()
        .filter(|l| !l.to_lowercase().contains("legacy"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 1 の再発（要求時に採らない）を落とす
#[test]
fn listはbackend_windowsを要求時に採り直す() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/dispatch.rs");
    let arm = code_only(&block_after(&src, "Request::List =>"));
    assert!(
        arm.contains("refresh_backend_windows()"),
        "Request::List が backend_windows を採り直していない = 右パネルの表示状態に\
         依存した値をそのまま返している（#1191 の再発）:\n{arm}"
    );

    // trait 側に窓口があること（消えると 1 の再発）
    let host = read(&root, "crates/tako-control/src/host.rs");
    assert!(
        host.contains("fn refresh_backend_windows(&mut self)"),
        "TmuxHost に refresh_backend_windows の窓口が無い（#1191）"
    );

    // UI 実装が窓口を埋めていること（既定の no-op のままだと GUI では直らない）
    let app = read(&root, "crates/tako-app/src/main.rs");
    assert!(
        app.contains("fn refresh_backend_windows(&mut self)"),
        "tako-app が refresh_backend_windows を実装していない = 既定の no-op のまま\
         （trait だけ足しても GUI の応答は直らない。#1191）"
    );
}

/// 2 の再発（採取の可否とペイン種別を混ぜる）を落とす。
///
/// 採取結果を反映する 1 実装（`apply_backend_windows`）が
/// **「採取できなかった」を `Option` で受ける**形を保っているかを見る。
/// ここが `HashMap` 直受けへ戻ると、tmux が落ちているのか window が無いのかを
/// 応答側で区別できなくなる
#[test]
fn 採取不能とwindow無しを混ぜていない() {
    let root = workspace_root();
    let app = read(&root, "crates/tako-app/src/main.rs");
    let apply = block_after(&app, "fn apply_backend_windows(");
    assert!(
        apply.contains("Option<&HashMap<String, Vec<tako_core::TmuxWindow>>>"),
        "apply_backend_windows が「採取できなかった」を Option で受けていない\
         （null と [] を区別できなくなる。#1191）:\n{apply}"
    );
    // 採取できたら backend ペイン**全件**に載せる（1 window でも落とさない）ことを、
    // 「2+ window のときだけ insert する」旧形が無いことで縛る
    let checked = without_legacy(&code_only(&apply));
    assert!(
        !checked.contains("if windows.len() > 1 {\n                    self.backend_windows"),
        "2+ window のときだけ backend_windows へ載せる旧形が戻っている（#1191）:\n{apply}"
    );
    assert!(
        checked.contains("self.backend_windows.insert(*pane_id, windows);"),
        "backend ペイン全件へ載せる形が見当たらない（#1191）:\n{apply}"
    );
}

/// 採取そのものが右パネルの表示状態でゲートされていないこと。
///
/// 旧実装は `app.panel_visible && app.panel_view == PanelView::Fleet` の中でしか
/// tmux 情報を採っておらず、その副作用が API の戻り値に出ていた。
/// ゲート自体（capture-pane の削減）は正しいので残すが、
/// **要求時採取の経路がそれを見ていない**ことを縛る
#[test]
fn 要求時採取がパネルの表示状態を見ていない() {
    let root = workspace_root();
    let app = read(&root, "crates/tako-app/src/main.rs");
    let refresh = code_only(&block_after(
        &app,
        "fn refresh_backend_windows_now(&mut self)",
    ));
    for forbidden in ["panel_visible", "PanelView"] {
        assert!(
            !refresh.contains(forbidden),
            "要求時採取が `{forbidden}` を見ている = UI の表示状態に依存する（#1191）:\n{refresh}"
        );
    }
    // 器を叩くのは 1 回（`list-windows -a` 相当）。3 コマンドの `list_sessions` や
    // 6 コマンドの `fetch_tmux_sessions` へ戻ると、`tako list` のたびに重くなる
    assert!(
        refresh.contains("list_windows_by_session("),
        "要求時採取が軽量経路（list-windows 1 回）を通っていない（#1191 / #1001）:\n{refresh}"
    );
    for heavy in ["fetch_tmux_sessions(", "list_sessions("] {
        assert!(
            !refresh.contains(heavy),
            "要求時採取が重い経路 `{heavy}` を呼んでいる（#1191 / #1001）:\n{refresh}"
        );
    }
}
