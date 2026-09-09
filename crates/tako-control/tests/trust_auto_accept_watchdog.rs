//! 自動承諾が「ハイライトを見ずに素の Enter」へ戻らないための番犬（Issue #1236）
//!
//! claude 2.x の信頼ダイアログは**既定ハイライトが `No, exit`** なので、
//! 素の Enter は拒否側を確定して claude を終了させる（プロンプトはシェルへ流れ、
//! worker が黙って死ぬ）。同じ形の Bypass 確認ダイアログ（#407）では
//! 「↓ + Enter」が必要だと 2026 年から分かっていたのに、信頼ダイアログ側だけが
//! 取り残されていた = **経路ごとに手順を書き分けたことが再発の原因**。
//!
//! そこでソース走査で次の 3 つを見張る:
//!   1. 送達フロー（tako-app）の信頼ダイアログ分岐が `claude_tui::accept_step` を通る
//!      （素の `b"\r"` を直書きしない）
//!   2. 器越しの送達（`claude_tui::deliver_via_tmux`）も同じ 1 実装を通る
//!   3. 自動承諾と respond の**両方**が `tako_core::dialog::confirm_step` を通る
//!      （移動の向き・歩数・「Enter を送ってよいか」の判断を 2 箇所に持たない）

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

/// `fn <name>(` の本文（**同じインデントの次の `fn ` まで**）を切り出す。
/// impl ブロック内の関連関数（`    fn …`）も対象にする
fn fn_body(src: &str, name: &str) -> String {
    let head = format!("fn {name}(");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("`fn {name}` が見つからない（#1236）"));
    // 宣言行の行頭からのインデント（`pub` があれば含む）
    let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let indent: String = src[line_start..at]
        .chars()
        .take_while(|c| *c == ' ')
        .collect();
    let rest = &src[at + head.len()..];
    let end = [
        rest.find(&format!("\n{indent}fn ")),
        rest.find(&format!("\n{indent}pub fn ")),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// `start` から次の `end` までを切り出す（分岐 1 本ぶんの走査に使う）
fn region(src: &str, what: &str, start: &str, end: &str) -> String {
    let at = src
        .find(start)
        .unwrap_or_else(|| panic!("{what} の起点 `{start}` が見つからない（#1236）"))
        + start.len();
    let rest = &src[at..];
    let to = rest
        .find(end)
        .unwrap_or_else(|| panic!("{what} の終点 `{end}` が見つからない（#1236）"));
    rest[..to].to_string()
}

#[test]
fn 送達フローの信頼ダイアログ分岐は素のenterを送らない() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-app/src/main.rs");
    let branch = region(
        &src,
        "送達フローの信頼ダイアログ分岐",
        "} else if claude_tui::is_trust_dialog(&lines) {",
        "} else if",
    );
    assert!(
        branch.contains("drive_trust_accept("),
        "信頼ダイアログ分岐が `drive_trust_accept`（= `accept_step` の 1 実装）を\
         通っていない（#1236）"
    );
    assert!(
        !branch.contains("write(b\"\\r\""),
        "信頼ダイアログ分岐が素の Enter を直書きしている（#1236。claude 2.x の\
         番号なしダイアログは既定が `No, exit` なので claude が終了する）"
    );
}

#[test]
fn 自動承諾はハイライトを確認できないときenterを送らない() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-app/src/main.rs");
    let body = fn_body(&src, "drive_trust_accept");
    assert!(
        body.contains("claude_tui::accept_step("),
        "`drive_trust_accept` が `accept_step` を通っていない（#1236）"
    );
    // Blocked（ハイライトが読めない）のアームでキーを送っていないこと
    let blocked = region(
        &body,
        "自動承諾の Blocked アーム",
        "AcceptStep::Blocked(reason) => {",
        "\n            }",
    );
    assert!(
        !blocked.contains("session.write("),
        "承諾側を特定できない画面へキーを送っている（#1236。ここで Enter を送ると\
         既定が拒否側のダイアログでエージェントを終了させる）"
    );
    assert!(
        blocked.contains("persist_log("),
        "諦めた理由が persist.log に残っていない（#1236。黙って待つと\
         「なぜ送達しなかったか」が後から分からない）"
    );
}

#[test]
fn 器越しの送達も同じ1実装で承諾する() {
    let root = workspace_root();
    let src = read(&root, "crates/tako-control/src/claude_tui.rs");
    let body = fn_body(&src, "deliver_via_tmux");
    let branch = region(
        &body,
        "器越し送達の自動承諾分岐",
        "if is_trust_dialog(&lines) || is_bypass_dialog(&lines) {",
        "\n            continue;",
    );
    assert!(
        branch.contains("accept_step(&lines)"),
        "器越しの自動承諾が `accept_step` を通っていない（#1236。経路ごとに\
         手順を書き分けると片方だけが素の Enter を送る形へ戻る）"
    );
    assert!(
        branch.contains("AcceptStep::Blocked"),
        "承諾側を特定できない画面の扱いが無い（#1236）"
    );
    // 旧実装（ハイライトを見ずに Enter / 決め打ちの Down + Enter）へ戻っていないこと
    assert!(
        !body.contains("if is_trust_dialog(&lines) {\n            if report"),
        "信頼ダイアログの分岐が #1236 前の形（素の Enter）へ戻っている"
    );
}

#[test]
fn 移動と確定の判断は1実装を共有する() {
    let root = workspace_root();
    let tui = read(&root, "crates/tako-control/src/claude_tui.rs");
    let dispatch = read(&root, "crates/tako-control/src/dispatch.rs");
    // 自動承諾側
    assert!(
        fn_body(&tui, "accept_step_with").contains("tako_core::dialog::confirm_step("),
        "自動承諾が `confirm_step` を通っていない（#1236）"
    );
    // respond 側（番号なし経路）
    let respond: String = fn_body(&dispatch, "respond_via")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        respond.contains("tako_core::dialog::confirm_step("),
        "respond の番号なし経路が `confirm_step` を通っていない（#1236。\
         自動承諾と規則が分かれると、片方だけが誤った選択肢を確定する）"
    );
}
