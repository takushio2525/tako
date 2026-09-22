//! 番犬が**自分（や走査先）の説明文**で緑になる型を止める（#1609）
//!
//! ## 何が起きていたか
//!
//! 走査型の番犬の多くは「この呼び出しが在る」= **肯定の存在確認**を
//! `fs::read_to_string` の**全文**へ `contains` して書いていた。この形は、
//! 実体が消えても**同じ綴りを書いた doc コメントが残っていれば緑のまま**になる。
//! 壊れ方が「落ちない」なので、検出力を失ったことに気づけない。
//!
//! - #1536 の番犬 `判定の写しを持たない` は、自分の doc コメントに書いた
//!   `tako_core::emoji::is_emoji` で `contains` が真になっていた（#1578 が発見）
//! - #1308 の番犬は「A/B の旧経路（`TAKO_1308_LEGACY`）が在る」を
//!   ドライバ本体への全文一致で見ていたが、本体にあるのは A/B アームの
//!   **目印コメント**（`// TAKO_1308_LEGACY_ARM 開始`）だけで、実体の
//!   `env::var("TAKO_1308_LEGACY")` は兄弟の関数にあった（#1609 で実測）
//!
//! ## 寄せ先
//!
//! 肯定の存在確認は `common/code_view.rs` の [`without_comments`] を通す。
//! **コメントだけ**を空白へ潰し、コードと文字列リテラルは囲みごと残すので、
//! 識別子を見る番犬（`fn wait_osc7_cwd(`）も文言を見る番犬（`env::var("TAKO_…")`）も
//! 同じ 1 実装で書ける。バイト長と行番号が保たれるので `file:line` の名指しと、
//! コメントの目印で測った区間の切り出しがそのまま効く。
//!
//! **不在**を確かめる番犬（個人情報が無い / 絵文字が無い）はこれを通さない。
//! コメントの中の違反も違反なので、全文を見るのが正しい。

#[path = "common/code_view.rs"]
mod code_view;

use code_view::{
    code_view as code_only, literals_only, without_comments, without_comments_checked,
};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読める: {e}"))
}

/// #1609 で肯定の存在確認を共有部品へ寄せた番犬。
///
/// ここから外れる（= 生の全文へ戻す）と、また自分の説明文で緑になる。
/// **`tests/` 以外・不在検査・非 Rust の走査は載せない**（棚卸しの区分は PR 本文）
const MOVED: &[&str] = &[
    "crates/tako-control/tests/backend_windows_watchdog.rs",
    "crates/tako-control/tests/bundle_install_watchdog.rs",
    "crates/tako-control/tests/dialog_respond_route_watchdog.rs",
    "crates/tako-control/tests/issue1308_pty_wait_watchdog.rs",
    "crates/tako-control/tests/issue1403_http_workers_watchdog.rs",
    "crates/tako-control/tests/issue1425_save_layout_change_key_watchdog.rs",
    "crates/tako-control/tests/issue1449_launch_routes_watchdog.rs",
    "crates/tako-control/tests/issue1505_diagnostics_single_source.rs",
    "crates/tako-control/tests/issue1554_restore_breakdown_watchdog.rs",
    "crates/tako-control/tests/issue757_login_expired_watchdog.rs",
    "crates/tako-control/tests/lid_guard_ownership.rs",
    "crates/tako-control/tests/mouse_report_wait_watchdog.rs",
    "crates/tako-control/tests/osc7_wait_watchdog.rs",
    "crates/tako-control/tests/osc_sink_writer_watchdog.rs",
    "crates/tako-control/tests/preview_autosave_watchdog.rs",
    "crates/tako-control/tests/psmux_cleanup_timeout_watchdog.rs",
    "crates/tako-control/tests/psmux_e2e_wait_watchdog.rs",
    "crates/tako-control/tests/quit_signal_watchdog.rs",
    "crates/tako-control/tests/setup_action_parity.rs",
    "crates/tako-control/tests/setup_bootstrap_agent_watchdog.rs",
    "crates/tako-control/tests/test_residue_watchdog.rs",
    "crates/tako-control/tests/tick_snapshot_watchdog.rs",
    "crates/tako-control/tests/tmux_cleanup_watchdog.rs",
    "crates/tako-control/tests/tmux_named_cleanup_watchdog.rs",
    "crates/tako-control/tests/trust_auto_accept_watchdog.rs",
    "crates/tako-control/tests/verification_isolation_watchdog.rs",
    "crates/tako-control/tests/windows_stack_reserve_watchdog.rs",
    "crates/tako-control/tests/worker_close_registry_watchdog.rs",
];

// ------------------------------------------------ 眺めそのもの（3 つの出し分け）

#[test]
fn コメントの綴りは眺めに残らない() {
    let src = "\
/// 入口は foo::bar( を通る
fn f() {
    foo::bar(1); // ここでも foo::bar(
}
";
    let view = without_comments(src);
    assert_eq!(
        view.matches("foo::bar(").count(),
        1,
        "コメントの中の綴りが残っている:\n{view}"
    );
    assert!(view.contains("fn f()"), "コードが消えている:\n{view}");
    // 実体を消してコメントだけ残すと、眺めからは消える（= 番犬が落ちる）
    let broken = src.replace("    foo::bar(1); // ここでも foo::bar(\n", "");
    assert!(
        !without_comments(&broken).contains("foo::bar("),
        "実体を消してもコメントで真になっている（#1609 の型）"
    );
    assert!(
        broken.contains("foo::bar("),
        "全文にはコメントとして残る（= これが偽の緑の材料）"
    );
}

#[test]
fn 文字列リテラルは囲みごと残る() {
    let src = "\
/// A/B は TAKO_X_LEGACY
fn f() {
    std::env::var(\"TAKO_X_LEGACY\");
    let s = r#\"生の \"引用\" ごと\"#;
}
";
    let view = without_comments(src);
    assert!(
        view.contains("std::env::var(\"TAKO_X_LEGACY\")"),
        "囲みつきの文字列が壊れている:\n{view}"
    );
    assert!(
        view.contains("r#\"生の \"引用\" ごと\"#"),
        "生文字列が壊れている"
    );
    assert_eq!(
        view.matches("TAKO_X_LEGACY").count(),
        1,
        "コメントの中の綴りが残っている:\n{view}"
    );
    // `code_view` は文字列も潰すので、文言を見る番犬はこちらを使えない
    assert!(
        !code_only(src).contains("TAKO_X_LEGACY"),
        "code_view が文字列を残している（出し分けが壊れた）"
    );
}

#[test]
fn バイト長と行番号が保たれる() {
    for rel in [
        "crates/tako-core/src/paths.rs",
        "crates/tako-control/src/dispatch.rs",
        "crates/tako-app/src/limit_autoresume.rs",
    ] {
        let src = read(rel);
        let view = without_comments(&src);
        assert_eq!(view.len(), src.len(), "{rel}: バイト長が変わった");
        assert_eq!(
            view.lines().count(),
            src.lines().count(),
            "{rel}: 行数が変わった"
        );
    }
}

#[test]
fn 眺めは3つとも同じ1つの走査から出ている() {
    // 同じ位置は「コード」「リテラル」「コメント」のどれか 1 つに割り当てられ、
    // `without_comments` はそのうち前の 2 つ（= コメント以外）を返す
    let src = read("crates/tako-control/src/reach.rs");
    let (code, lits, both) = (code_only(&src), literals_only(&src), without_comments(&src));
    let (cb, lb, bb) = (code.as_bytes(), lits.as_bytes(), both.as_bytes());
    for i in 0..src.len() {
        let kept = cb[i] != b' ' || lb[i] != b' ';
        if kept {
            assert_eq!(bb[i], src.as_bytes()[i], "{i} バイト目が落ちている");
        }
        // 囲み（`"` / `'` / `r#`）はコード側にもリテラル側にも出ないが、
        // `without_comments` は残す。ここでは「落ちていない」ことだけを見る
    }
    assert!(both.len() == src.len());
}

// ------------------------------------------------ 走査が空振りしていない

#[test]
fn 空振りした眺めはその場で落ちる() {
    // コメントしか無いファイル = 読み先の取り違え / 眺めの破損
    let only_comments = "//! 説明だけ\n// fn f() と書いてあるが実体は無い\n";
    let err = std::panic::catch_unwind(|| without_comments_checked(only_comments, "dummy.rs"))
        .expect_err("コメントだけの入力は落ちるべき");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "?".to_string());
    assert!(msg.contains("dummy.rs"), "走査先を名指ししていない: {msg}");
    assert!(msg.contains("#1609"), "由来を示していない: {msg}");
    // 空のファイルも落ちる
    assert!(std::panic::catch_unwind(|| without_comments_checked("", "empty.rs")).is_err());
    // 実在のソースは通る
    let rel = "crates/tako-core/src/paths.rs";
    assert!(!without_comments_checked(&read(rel), rel).is_empty());
}

// ------------------------------------------------ 寄せ先から戻っていない

#[test]
fn 寄せた番犬は共有部品を通している() {
    let mut missing = Vec::new();
    for rel in MOVED {
        // この検査自身が #1609 の型にはまらないよう、**コメントを落とした眺め**で見る
        // （説明文に `without_comments` と書いただけで緑になっては意味が無い）
        let code = without_comments_checked(&read(rel), rel);
        // 取り込みは 2 通り: 直に `mod` するか、`production_range`（#1420）が
        // 抱えている方を `use` するか。**同じファイルを 2 度 `mod` すると
        // `clippy::duplicate_mod` で落ちる**ので、両方を取り込む番犬は後者を通る
        let declares = code.contains("#[path = \"common/code_view.rs\"]")
            || code.contains("use production_range::code_view;");
        let uses = code.contains("without_comments");
        if !declares || !uses {
            missing.push(format!("{rel}（取り込み={declares} / 呼び出し={uses}）"));
        }
    }
    assert!(
        missing.is_empty(),
        "肯定の存在確認が生の全文へ戻っている:\n  {}\n\
         → `code_view::without_comments_checked` を通すこと（#1609）",
        missing.join("\n  ")
    );
}

#[test]
fn 棚卸しの一覧は実在して重複しない() {
    // 一覧が腐ると「0 件を検査して緑」になる（#1609 が止めたかった形そのもの）
    let mut sorted = MOVED.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), MOVED.len(), "MOVED に重複がある");
    assert_eq!(
        sorted.as_slice(),
        MOVED,
        "MOVED が辞書順でない（差分が読みにくい）"
    );
    assert!(MOVED.len() >= 27, "一覧が痩せている: {} 件", MOVED.len());
    for rel in MOVED {
        assert!(
            repo_root().join(rel).is_file(),
            "一覧のファイルが無い（改名したら一覧も直す）: {rel}"
        );
    }
}
