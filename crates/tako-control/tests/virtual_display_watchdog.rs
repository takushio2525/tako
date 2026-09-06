//! 仮想ディスプレイまわりの番犬（#1141）
//!
//! 守りたい不変条件は 3 つ。どれも「壊れても動いているように見える」ので、
//! 人間の記憶ではなくテストで固定する。
//!
//! 1. **ヘルパは消す機能を持たない**。`tako-vd` は常設で、検証のたびに作り直さない
//!    （消す手段があると、いつか「後片付け」のつもりで消され、次の検証で
//!    ユーザーの画面に窓が出る）
//! 2. **既定名が Rust とシェルでずれない**（ずれると隔離起動だけ黙ってメイン画面へ落ちる）
//! 3. **tako が開く窓は全部同じ面へ出す**。1 枚でも素の中央寄せが残ると、
//!    セルフテストが開く設定画面などがユーザーの画面へ飛び出す

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn helper_source() -> String {
    let p = repo_root().join("scripts/lib/virtual-display.sh");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("読めない {}: {e}", p.display()))
}

/// 行コメントを落とした実行部分だけ（説明文の中の語で誤検知しないため）
fn code_lines(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with('#'))
        .collect()
}

#[test]
fn 仮想ディスプレイのヘルパは消す機能を持たない() {
    // 器へ「消す / 切る」を頼む語。ヘルパの実行部分に 1 つも現れてはいけない
    const DESTRUCTIVE: &[&str] = &["discard", "-connected=off", "connected=off"];
    let src = helper_source();
    let mut hits = Vec::new();
    for (line_no, line) in code_lines(&src) {
        for needle in DESTRUCTIVE {
            if line.contains(needle) {
                hits.push(format!(
                    "scripts/lib/virtual-display.sh:{line_no}: {needle}"
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "仮想ディスプレイを消す / 切る操作がヘルパに入っている:\n  {}\n\
         → tako-vd は常設（#1141）。消す手段を持たせない。\n\
           後片付けのつもりで消されると、次の検証でユーザーの画面に窓が出る",
        hits.join("\n  "),
    );
}

#[test]
fn ヘルパは必要なサブコマンドをそろえている() {
    let src = helper_source();
    for sub in ["ensure)", "bounds)", "status)", "move-window)"] {
        assert!(
            src.contains(sub),
            "ヘルパにサブコマンド {sub} が無い（#1141 の受け入れ条件）"
        );
    }
}

#[test]
fn 既定の仮想ディスプレイ名がrustとシェルでそろっている() {
    let name = tako_core::platform::display::DEFAULT_VIRTUAL_DISPLAY_NAME;
    let src = helper_source();
    // シェル側の既定値（TAKO_VD_NAME の :- 右辺）
    let shell_default = src
        .lines()
        .find_map(|l| l.trim().strip_prefix("VD_NAME=${TAKO_VD_NAME:-"))
        .and_then(|rest| rest.strip_suffix('}'))
        .map(str::to_string);
    assert_eq!(
        shell_default.as_deref(),
        Some(name),
        "既定の仮想ディスプレイ名が Rust（{name}）とシェルでずれている。\n\
         → ずれると隔離起動だけが黙ってメイン画面へ落ちる（症状が出るのは\n\
           「窓が邪魔」という報告だけで、ログは正常に見える）"
    );
    // 案内する docs も同じ名前であること（#1139 以降、コマンドの全文は
    // `.agent/commands.md` に移り AGENTS.md は 1 行の索引だけになった）
    for doc in [".agent/commands.md", ".agent/conventions.md"] {
        let text = std::fs::read_to_string(repo_root().join(doc)).unwrap_or_default();
        assert!(
            text.contains(name),
            "{doc} が既定名 {name} を案内していない（#1141）"
        );
    }
}

#[test]
fn takoが開く窓は全部置き先を通る() {
    let p = repo_root().join("crates/tako-app/src/main.rs");
    let src = std::fs::read_to_string(&p).expect("main.rs を読む");
    let mut hits = Vec::new();
    for (i, line) in src.lines().enumerate() {
        // doc コメントの中の見本は対象外（説明でこの形を書けるようにしておく）
        if line.trim_start().starts_with("///") || line.trim_start().starts_with("//") {
            continue;
        }
        if line.contains("Bounds::centered(None") {
            hits.push(format!("crates/tako-app/src/main.rs:{}", i + 1));
        }
    }
    assert!(
        hits.is_empty(),
        "置き先を通さない中央寄せが残っている:\n  {}\n\
         → `centered_on_target()` を使うこと（#1141 / FR-4.8.5）。\n\
           1 枚でも素の中央寄せが残ると、セルフテストが開く設定画面などが\n\
           ユーザーのメイン画面へ飛び出す",
        hits.join("\n  "),
    );
}
