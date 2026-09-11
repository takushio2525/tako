//! 明示 close が worker レジストリへ「誰が閉じたか」を残す番犬（#775）
//!
//! `workers.yaml` のエントリを closed へ倒すのは close 経路の責務だが、経路は 5 本ある
//! （ペイン × / タブ × / cmd+W / たまり場カードの kill / dispatch の Close）。
//! #658 は前の 3 本を配線したが、**たまり場カードの kill だけが残っていた**
//! ——たまり場のペインはどのタブにも居ないので `remove_pane_with` を通らず、
//! drawer の on_click が後始末を独自に並べていたためで、後付けの記録はこの経路だけ
//! 抜ける（#821 / #826 / #766 / #1259 が同型で踏んだ「経路ごとの独自列挙」）。
//!
//! 残っていたもう 1 つの穴が `close_reason` の綴り。全経路が固定文字列
//! `"explicit_close"` を渡していたので、「タブ × で閉じた」「cmd+W で閉じた」が
//! 残らず、#770 の調査では消去法でしか発生源を絞れなかった。
//!
//! ソース走査で見張るのはこの 2 つ:
//!   1. 明示 close の経路はすべて記録フック（`mark_worker_closed` /
//!      `mark_closed_by_origin`）を通る
//!   2. `close_reason` の綴りは `registry::close_reason_for` の 1 実装が持つ
//!      （経路ごとに文字列を直書きしない）

use std::path::{Path, PathBuf};

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

/// `fn <name>(` から次のトップレベル `    fn ` までを切り出す（同じ impl 内の関数本体）。
/// main.rs の `preview_cleanup_watchdog` と同じ形を使う（束縛名に依存しない）
fn body_of<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("{name} が見つからない"));
    let rest = &src[start..];
    let end = rest[1..]
        .find("\n    fn ")
        .map(|at| at + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}

/// GUI の明示 close 経路はすべて記録フックを通る。
///
/// `kill_shelved_pane` が並ぶのが #775 の本体（たまり場カードの kill）。
/// ここから `mark_worker_closed(` が消えると、GUI で殺した worker が
/// `tako orchestrator workers` に 5 分間（GC の確認期間）生き続けて見える
#[test]
fn gui_の明示close経路はworkerレジストリの記録フックを通る() {
    let root = workspace_root();
    let main_rs = read(&root, "crates/tako-app/src/main.rs");
    for name in ["remove_pane_with", "remove_tab_with", "kill_shelved_pane"] {
        let body = body_of(&main_rs, name);
        assert!(
            body.contains("mark_worker_closed("),
            "{name} が mark_worker_closed を呼んでいない（#775。\
             この経路で閉じた worker が workers.yaml に active のまま残る）"
        );
    }
}

/// dispatch（CLI / MCP）の close 系も同じ記録フックを通る。
///
/// `Request::Close` は #390 から記録していたが、**`Request::BackgroundKill`
/// （たまり場 kill の CLI / MCP 側）は記録していなかった**。GUI と AI で
/// `workers.yaml` の見え方が変わる = 開発不変条件「UI でできることは AI からも
/// 同じに見える」から外れる
#[test]
fn dispatchのclose系はworkerレジストリの記録フックを通る() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/dispatch.rs");
    for (arm, what) in [
        ("Request::Close {", "ペイン / タブの close"),
        ("Request::BackgroundKill {", "たまり場の kill"),
    ] {
        let start = src
            .find(arm)
            .unwrap_or_else(|| panic!("{arm} が見つからない"));
        let rest = &src[start..];
        // 次の match アーム（同じインデントの `Request::`）までを本体とする
        let end = rest[1..]
            .find("\n        Request::")
            .map(|at| at + 1)
            .unwrap_or(rest.len());
        assert!(
            rest[..end].contains("mark_closed_by_origin("),
            "{what}（{arm}）が mark_closed_by_origin を呼んでいない（#775）"
        );
    }
}

/// たまり場カードの kill は後始末を並べ直さず集約関数へ任せる。
///
/// drawer の on_click が `remove_shelved` から自分で後始末を始めた形が #775 の
/// 症状そのものなので、退避中ペインの撤去は `kill_shelved_pane` の 1 実装に限る
#[test]
fn たまり場のkillは後始末を集約関数に任せている() {
    let root = workspace_root();
    let drawer = read(&root, "crates/tako-app/src/drawer.rs");
    assert!(
        drawer.contains("kill_shelved_pane("),
        "drawer が kill_shelved_pane を呼んでいない（#775）"
    );
    assert!(
        !drawer.contains("remove_shelved("),
        "drawer が退避中ペインの撤去を独自に始めている（#775。\
         後始末は kill_shelved_pane（main.rs）へ足すこと）"
    );

    // 撤去の本体は 1 箇所だけ（UI 側へ二重化していない）
    let main_rs = read(&root, "crates/tako-app/src/main.rs");
    let holders: Vec<&str> = ["main.rs", "drawer.rs"]
        .into_iter()
        .zip([&main_rs, &drawer])
        .filter(|(_, src)| src.contains("remove_shelved("))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        holders,
        vec!["main.rs"],
        "退避中ペインの撤去が 1 箇所に収まっていない（#775）"
    );
}

/// `close_reason` の綴りは 1 実装（`registry::close_reason_for`）が持つ。
///
/// 製品コードが `mark_closed_by_pane`（理由を文字列で渡す形）を直呼びすると、
/// 経路ごとに語彙がずれて `workers.yaml` とペインログ / persist.log の
/// 突き合わせができなくなる（発生源つきの入口は `mark_closed_by_origin`）
#[test]
fn close_reasonの綴りは経路ごとに直書きしない() {
    let root = workspace_root();
    for rel in [
        "crates/tako-app/src/main.rs",
        "crates/tako-control/src/dispatch.rs",
    ] {
        let src = read(&root, rel);
        let strays: Vec<&str> = src
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && !l.starts_with("///"))
            .filter(|l| l.contains("mark_closed_by_pane("))
            .collect();
        assert!(
            strays.is_empty(),
            "{rel} が理由を文字列で渡す旧 API を直呼びしている: {strays:?}。\
             発生源つきの mark_closed_by_origin を使うこと（#775）"
        );
        let literals: Vec<&str> = src
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//") && !l.starts_with("///"))
            .filter(|l| l.contains("\"explicit_close\""))
            .collect();
        assert!(
            literals.is_empty(),
            "{rel} が close_reason を固定文字列で綴っている: {literals:?}。\
             綴りは registry::close_reason_for の 1 実装へ寄せること（#775）"
        );
    }
}

/// 記録フックの語彙がペインログのクローズマーカーと同一であること。
///
/// 「語彙が揃っている」= `workers.yaml` の `close_reason` と persist.log /
/// ペインログの `--- [クローズ: …] ---` を文字列一致で突き合わせられること。
/// 別の綴りを作ると #770 の調査手順（両者の照合）が成り立たない
#[test]
fn 記録の語彙はペインログのクローズマーカーと一致する() {
    use tako_control::orchestrator::registry::close_reason_for;
    use tako_core::pane_log::{close_marker_reason, CloseOrigin};

    for origin in [
        CloseOrigin::Keyboard,
        CloseOrigin::PaneButton,
        CloseOrigin::TabButton,
        CloseOrigin::Cli,
        CloseOrigin::Mcp,
    ] {
        let reason = close_reason_for(origin, None);
        // ペインログへ実際に書かれるマーカー行（末尾は UTC タイムスタンプ）から
        // 読み戻した発生源と同じ文字列
        let marker_line = format!(
            "--- [クローズ: {} 2026-09-11T00:00:00Z] ---",
            origin.marker()
        );
        assert_eq!(
            close_marker_reason(&marker_line),
            Some(reason.as_str()),
            "{origin:?} の close_reason がペインログの発生源と違う（#775）"
        );
    }
    // PTY 死亡（exit）は明示 close ではないので記録側の語彙に入らない
    assert_eq!(CloseOrigin::ProcessExit.marker(), "exit");
}
