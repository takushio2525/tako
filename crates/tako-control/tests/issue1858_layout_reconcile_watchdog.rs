//! recover の合流と復元時の刈り込みの番犬（Issue #1858）
//!
//! # 何が壊れていたか
//!
//! 2026-09-30 の本番の復旧（#1857）で 2 つを踏んだ:
//!
//! 1. `tako recover --apply <世代>` は世代のファイルを layout.json へ写すだけで、
//!    バックアップより後に作られたタブ（復旧をしている master 自身のタブを含む）が消えた
//! 2. 復元は tmux セッションが無いペインも新しいシェル / resume として開き直すので、
//!    完了済みの worker 3 枚が木に残り、手で外した
//!
//! 判断の正本は `tako_control::layout_reconcile`（単体テストはそちら）。ここは
//! **呼び手の配線**を縛る: 判断が正しくても、呼ぶ場所・呼ぶ順が崩れると黙って元へ戻る
//!
//! # 検査
//!
//! 1. [`復元は刈り込んでから組み立てる`] — 起動時の読み込みは `layout::restore` の前に
//!    刈り込む。器の一覧は**生死を区別できる口**（`live_session_names`）から取る
//!    （`has_session` は「問い合わせられない」と「器が無い」が同じ `false` になり、
//!    PC の再起動後に全ペインを刈り込む）
//! 2. [`器の一覧はspawnより前に取る`] — 刈り込みの位置が復元ループより前にあること
//!    （最初の spawn がサーバーを起こした後に聞くと、器ごと消えた全ペインを「消えた」と読む）
//! 3. [`recoverは合流してから書く`] — `recover --apply` は合流の判断を通し、
//!    世代のコピーは合流が無いときだけ
//! 4. [`結果がcliとmcpに載る`] — `tako persist` / MCP `tako_persist` の応答と persist.log

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// コメントを落とした本文（doc コメントの同じ綴りで真にならないように。#1609）
fn read_code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    code_view::without_comments_checked(&src, rel)
}

/// `start` を含む位置から `end` を含む位置までの区間。採れなければ**落とす**
/// （走査が空振りしたまま緑になるのを防ぐ）
fn region<'a>(source: &'a str, start: &str, end: &str, what: &str) -> &'a str {
    let from = source
        .find(start)
        .unwrap_or_else(|| panic!("{what}: 入口 `{start}` が見つからない"));
    let len = source[from..]
        .find(end)
        .unwrap_or_else(|| panic!("{what}: 出口 `{end}` が見つからない"));
    &source[from..from + len]
}

/// 起動時の読み込みの閉包（`try_load` から `layout::restore(&file)` まで）
fn startup_loader(main: &str) -> &str {
    region(
        main,
        "tako_control::layout::try_load().and_then(",
        "tako_control::layout::restore(&file)",
        "起動時の読み込み",
    )
}

#[test]
fn 復元は刈り込んでから組み立てる() {
    let main = read_code(MAIN);
    let loader = startup_loader(&main);
    assert!(
        loader.contains("layout_reconcile::prune_dead_backends("),
        "起動時の読み込みが layout::restore の前に刈り込んでいない（#1858）"
    );
    assert!(
        loader.contains("tako_core::tmux::live_session_names("),
        "刈り込みの材料が live_session_names（問い合わせられないを None で返す口）から来ていない"
    );
    assert!(
        !loader.contains("has_session("),
        "刈り込みの材料に has_session を使っている（問い合わせられない = 器が無い と読んでしまう）"
    );
    assert!(
        loader.contains("persist_diag(&pruned.log_line())")
            && loader.contains("restore_prune.held_line()"),
        "刈り込んだペイン・見送った理由を persist.log へ残していない"
    );
}

#[test]
fn 器の一覧はspawnより前に取る() {
    let main = read_code(MAIN);
    let prune = main
        .find("layout_reconcile::prune_dead_backends(")
        .expect("刈り込みの呼び出し");
    let spawn_loop = main
        .find("let mut breakdown = tako_control::restore_report::RestoreBreakdown::new();")
        .expect("復元ループの入口");
    assert!(
        prune < spawn_loop,
        "刈り込みが復元ループ（spawn）より後にある"
    );
    // 内訳へ件数を渡している（復元ループの入口の直後 = 最初の `for r in &restored` より前）
    let after = &main[spawn_loop..];
    let note = after.find("breakdown.note_pruned(");
    let first_loop = after.find("for r in &restored").expect("復元ループ");
    assert!(
        note.is_some_and(|n| n < first_loop),
        "刈り込んだ件数を復元の内訳へ渡していない"
    );
}

#[test]
fn recoverは合流してから書く() {
    let cli = read_code(CLI);
    let apply = region(
        &cli,
        "fn recover_apply(",
        "fn print_recover_merge(",
        "recover_apply",
    );
    assert!(
        apply.contains("layout_reconcile::merge_after_backup("),
        "recover --apply が合流の判断を通していない（#1858）"
    );
    assert!(
        apply.contains("tako_core::tmux::live_session_names("),
        "合流の材料が live_session_names から来ていない"
    );
    assert!(
        apply.contains("layout::write_file(path, merged)"),
        "合流した layout を書いていない"
    );
    assert!(
        apply.contains("report.log_line(generation)") && apply.contains("persist_log("),
        "合流の結果を persist.log へ残していない"
    );
    // 世代のコピーは合流が無い枝だけ（無条件のコピーが戻ると、合流した結果を上書きする）
    let copies = apply.matches("std::fs::copy(&bak, path)").count();
    assert_eq!(copies, 1, "世代のコピーが {copies} か所ある");
    let copy_at = apply.find("std::fs::copy(&bak, path)").unwrap();
    let write_at = apply.find("layout::write_file(path, merged)").unwrap();
    assert!(
        apply[write_at..copy_at].contains("_ =>"),
        "世代のコピーが合流の枝の外にある"
    );
}

#[test]
fn 結果がcliとmcpに載る() {
    let dispatch = read_code(DISPATCH);
    let persist = region(
        &dispatch,
        "Request::Persist { enabled } =>",
        "Request::Panel {",
        "Persist の dispatch",
    );
    assert!(
        persist.contains("\"restore_pruned\": host.persist_restore_pruned()"),
        "tako persist / MCP tako_persist の応答に restore_pruned が無い"
    );
    let main = read_code(MAIN);
    assert!(
        main.contains("fn persist_restore_pruned(&self)"),
        "GUI が刈り込んだペインを host へ渡していない（既定の空のまま）"
    );
}
