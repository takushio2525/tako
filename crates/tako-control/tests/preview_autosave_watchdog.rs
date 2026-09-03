//! プレビュー編集の自動保存が「フラグを立てたのに誰も回さない」にならない番犬（#973）
//!
//! 旧実装は保留フラグを立てる `schedule_autosave` と 500ms 後に保存を回す
//! `start_autosave_timer` が **2 本に分かれていた**。タイマーを始めるのは GUI の
//! 入力経路（キー / ペースト / IME）だけだったので、dispatch 経路
//! （`edit replace` / `apply` / `undo` / `redo` = CLI / MCP）は保留に入ったまま
//! 誰も保存せず、`EditState::open` の既定が `autosave: true` であるにもかかわらず
//! **一度も自動保存されなかった**（#973）。
//!
//! 直し方は「呼び忘れを止める」ではなく**呼び忘れを作れなくする**こと:
//!   1. 保留フラグとタイマーを 1 本の入口（`drive_autosave`）へ寄せる
//!   2. 対象は「誰が編集したか」ではなく**編集セッションの状態**から導く
//!      （`preview::autosave_due`）ので、新しい編集経路は何もしなくてよい
//!   3. すべての dispatch が通る 1 箇所（IPC の 1 ターンの後処理）が消化する
//!
//! ソース走査で見張るのはこの 3 つ。`main.rs` は production と隔離セルフテストが
//! 同居するが、ここで見るのは**識別子の在り処**なので両者を区別する必要がない。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn app_main(root: &Path) -> String {
    std::fs::read_to_string(root.join("crates/tako-app/src/main.rs")).expect("main.rs が読める")
}

/// `fn <name>` の本文（同じインデントの次の `fn` まで）を切り出す
fn fn_body(src: &str, name: &str) -> String {
    let head = format!("    fn {name}(");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("`fn {name}` が main.rs に無い（#973）"));
    let rest = &src[at + head.len()..];
    let end = rest.find("\n    fn ").unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn 自動保存の保留フラグは唯一の入口の中でしか立たない() {
    let src = app_main(&workspace_root());

    // ① 保留へ入れる箇所は 1 つだけ
    let inserts = src.matches("autosave_pending.insert(").count();
    assert_eq!(
        inserts, 1,
        "自動保存の保留フラグを立てる箇所が {inserts} 個ある（#973）。\
         フラグとタイマーが離れると『立てたのに誰も回さない』が戻るので、\
         `drive_autosave` の中だけで立てること"
    );

    // ② その 1 つは `drive_autosave` の中（= 同じ関数の中でタイマーも始まる）
    let body = fn_body(&src, "drive_autosave");
    assert!(
        body.contains("autosave_pending.insert("),
        "`drive_autosave` の中で保留フラグを立てていない（#973）"
    );
    assert!(
        body.contains("run_autosave("),
        "`drive_autosave` が 500ms 後の保存（`run_autosave`）を始めていない（#973）。\
         フラグだけ立ててタイマーを別経路に任せると #973 に戻る"
    );

    // ③ 保存を回すのはその 1 箇所だけ（別経路のタイマーを増やさない）
    let calls = src.matches("run_autosave(").count() - src.matches("fn run_autosave(").count();
    assert_eq!(
        calls, 1,
        "`run_autosave` の呼び出しが {calls} 箇所ある（#973）。想定は \
         `drive_autosave` の中の 1 つだけ。別経路からタイマーを回すと \
         デバウンス（#195）とフラグの整合が崩れる"
    );

    // ④ 2 本に分かれていた旧 API を復活させない。
    //    **コメント行は落とす**: 旧名は `drive_autosave` の doc（何が壊れていたか）に
    //    出てくるので、素の文字列一致だと自分の説明で落ちる
    let code = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for gone in ["schedule_autosave", "start_autosave_timer"] {
        assert!(
            !code.contains(gone),
            "`{gone}` が復活している（#973）。保留フラグとタイマーを分けると \
             dispatch 経路の自動保存が黙って死ぬ"
        );
    }
}

#[test]
fn ipcの1ターンが自動保存を消化する() {
    let src = app_main(&workspace_root());
    let lines: Vec<&str> = src.lines().collect();

    // IPC のリクエストループ（= すべての dispatch が通る 1 箇所）を探す
    let loop_at = lines
        .iter()
        .position(|l| l.contains("while let Some(incoming) = control_rx.next().await"))
        .expect("IPC のリクエストループが main.rs にある（#973）");
    // 1 ターンの後処理の最後は永続化（`save_layout`）
    let end_at = lines[loop_at..]
        .iter()
        .position(|l| l.contains("app.save_layout();"))
        .map(|i| loop_at + i)
        .expect("IPC の 1 ターンの後処理に save_layout がある（#973）");
    let turn = lines[loop_at..end_at]
        .iter()
        .filter(|l| !l.trim_start().starts_with("//"))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        turn.contains("drive_autosave("),
        "IPC の 1 ターンの後処理が自動保存を消化していない（#973）。\
         dispatch はペインの状態を変えるところまでで、500ms のタイマーには \
         GPUI の Context が要る。ここで消化しないと CLI / MCP の編集は \
         autosave: true でも永久に保存されない"
    );
}

#[test]
fn 自動保存の対象は編集セッションの状態から導く() {
    let root = workspace_root();
    let src = app_main(&root);
    let preview = std::fs::read_to_string(root.join("crates/tako-app/src/preview.rs"))
        .expect("preview.rs が読める");

    // 判定の正本は preview.rs（GPUI 非依存・単体テストできる純粋関数）
    assert!(
        preview.contains("pub fn autosave_due<"),
        "`preview::autosave_due` が無い（#973）。対象を状態から導く判定は \
         1 箇所に置くこと"
    );
    // main.rs 側はそれを呼ぶだけ（条件式を書き直すと 2 つの規則が並ぶ）
    let body = fn_body(&src, "drive_autosave");
    assert!(
        body.contains("preview::autosave_due("),
        "`drive_autosave` が `preview::autosave_due` を通っていない（#973）"
    );
    assert!(
        !body.contains("edit.autosave &&"),
        "`drive_autosave` が判定を書き直している（#973）。\
         規則が 2 箇所に並ぶと #966（リモート既定 OFF）のような例外が片方だけに効く"
    );
}
