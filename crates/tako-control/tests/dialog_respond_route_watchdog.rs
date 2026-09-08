//! ダイアログ応答が「tako-app が保持しているペイン」を先に見る番犬（#1200）
//!
//! 旧実装の `respond` は入口でバックエンドセッション名を必須にし、器越しの到達手段
//! （`reach::detached_session` → `DetachedAccess`）へ**直行**していた。tmux は
//! `send-keys` でアウトオブプロセスにも送れるので macOS では成功してしまい、
//! **psmux（Windows）では生きているペインに対して必ず失敗**していた
//! （`UnreachableReason::NoDetachedAccess`。psmux は画面採取だけができ入力送出を持たない）。
//! 器なしのペイン（`TAKO_PERSIST=0`）には最初から応答できなかった。
//!
//! 直し方は「Windows だけ分岐する」ではなく**届き方を型で分ける**こと:
//!   1. `reach::DialogAccess` が「画面を読む + キーを送る」の 2 手だけを要求する
//!   2. `reach::dialog_access` が **in-process を先に**見て口を返す（唯一の入口）
//!   3. 応答の手順（検知 → 確定 → 着地の検証 → 解消の検証 → 監査ログ）は
//!      `respond_via` の 1 実装が両経路で共有する
//!
//! ソース走査で見張るのはこの 3 つ。

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

/// `fn <name>(` の本文（次の同じインデントの `fn ` まで）を切り出す
fn fn_body(src: &str, name: &str) -> String {
    let at = [format!("\nfn {name}("), format!("\npub fn {name}(")]
        .iter()
        .filter_map(|head| src.find(head.as_str()).map(|at| at + head.len()))
        .min()
        .unwrap_or_else(|| panic!("`fn {name}` が見つからない（#1200）"));
    let rest = &src[at..];
    let end = [rest.find("\nfn "), rest.find("\npub fn ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn respondの入口はin_processを先に見る() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/dispatch.rs");
    let body = fn_body(&src, "dispatch_orchestrator_respond");

    assert!(
        body.contains("crate::reach::dialog_access("),
        "respond の入口が `reach::dialog_access` を通っていない（#1200。\
         器越しへ直行すると、入力送出を持たない器（psmux）では生きているペインに\
         対して必ず失敗する）"
    );
    // 旧実装の形（バックエンドセッション名が無ければ即エラー）へ戻っていないこと。
    // A/B の入口（`TAKO_1200_LEGACY`）の中だけは旧経路を残してある
    let outside_ab = body
        .split("TAKO_1200_LEGACY")
        .next()
        .expect("split は必ず 1 つ以上返す")
        .to_string();
    assert!(
        !outside_ab.contains("バックエンドセッションが見つからない"),
        "respond がバックエンドセッション名を必須にしている（#1200。\
         tako-app が保持しているペインには器が無くても応答できる）"
    );
}

#[test]
fn 到達の解決はin_processを先に試す() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/reach.rs");
    let body = fn_body(&src, "dialog_access");
    let live_at = body
        .find("host.session(")
        .expect("in-process を見ていない（#1200）");
    let detached_at = body
        .find("backend().detached()")
        .expect("器越しへの落とし所が無い（#1200）");
    assert!(
        live_at < detached_at,
        "器越しを先に試している（#1200。in-process が主経路）"
    );
}

#[test]
fn 応答の手順は両経路で1実装を共有する() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/dispatch.rs");
    assert!(
        src.contains("pub fn respond_via("),
        "応答の手順が口越しの 1 実装になっていない（#1200）"
    );
    // rustfmt が `access` と `.capture()` を行で割るので空白を潰して見る
    let body: String = fn_body(&src, "respond_via")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    for needle in ["access.capture()", "access.send_key(", "access.route()"] {
        assert!(
            body.contains(needle),
            "`respond_via` が {needle} を通っていない（#1200。経路ごとに手順を\
             書き分けると「macOS では番号キーで確定するが Windows では Enter も要る」\
             のような差が構造的に生まれる）"
        );
    }
    // 監査ログに経路を残す（どちらで届いたのかが後から分かる）
    assert!(
        body.contains("route={}"),
        "persist.log の監査に経路が残っていない（#1200）"
    );
}

#[test]
fn 自動復帰も保持しているペインへ届く() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-app/src/limit_autoresume.rs");
    assert!(
        src.contains("LiveDialogAccess::new("),
        "#813 の自動復帰が in-process 経路を持っていない（#1200。器が入力送出を\
         持たない環境ではダイアログ型の復帰が一生できない）"
    );
    assert!(
        src.contains("SessionHost::session(self, pane).map(|s| s.access())"),
        "in-process の手を **UI スレッドで**取り出していない（#1200。\
         `TerminalSession` はスレッドを越えられない）"
    );
}
