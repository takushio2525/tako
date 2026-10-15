//! #2113 の番犬: 「この文書を受け持つ言語サーバはどれか」を問う口を `LspManager::resolve` の 1 実装に寄せる
//!
//! 検出表の解決（`tako_core::lsp::servers::resolve` / `resolve_in` / `resolve_with`）は、Arduino のスケッチ内の
//! `.h` / `.cpp` のためにファイルシステムを見る（`<フォルダ名>.ino` があれば ALS の行）。表を直接引く呼び手が
//! 残ると、開いた後で本体を足し引きしたときにホバー・補完・定義ジャンプの有効判定・状態の帯が開いた器と
//! 別の行を指し、マウスの移動のたびにディスクを見る。開いている文書は開いたときに決めた器で引く
//! （`LspManager::resolve` → `Shared::resolve_document`）。
//!
//! 本番コード（テスト領域とコメントを潰した眺め。`common/production_range.rs` の 1 実装）で表を直接引いて
//! よいのは次だけ:
//!
//! - `crates/tako-control/src/lsp/manager.rs` の `fn resolve` / `fn resolve_document`（寄せた先そのもの）
//! - `crates/tako-app/src/self_test/`（素材のサーバの差し替え口 `TAKO_LSP_BIN_<ID>` を表から引くだけ）
//! - `crates/tako-app/src/main.rs` の `#[cfg(feature = "visual-test")]` の関数（同上）
//!
//! 走査が空振りしていないこと（読んだファイル数・寄せた先に当たること）と、注入した違反を名指すことも見る。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments_checked;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 表を直接引く綴り（`servers::resolve(` は `servers::resolve_in(` と別に数える）
const CALLS: [&str; 3] = [
    "servers::resolve(",
    "servers::resolve_in(",
    "servers::resolve_with(",
];

/// 寄せた先（manager.rs の中でだけ表を引いてよい関数）
const OWNERS: [&str; 2] = ["resolve", "resolve_document"];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// 本番コードだけ（テスト領域とコメントを空白へ潰す。バイト長と行番号は原文のまま）
fn production(src: &str, rel: &str) -> String {
    without_comments_checked(&production_range::scan(src).text, rel)
}

/// `line`（0 起点）を囲む関数の名前と、その関数に付いた属性の行（上へ辿って最初の `fn` の行から）
fn enclosing_fn(lines: &[&str], line: usize) -> Option<(String, Vec<String>)> {
    let at = (0..=line).rev().find(|&i| fn_name(lines[i]).is_some())?;
    let name = fn_name(lines[at])?;
    let mut attrs = Vec::new();
    for prev in lines[..at].iter().rev() {
        let t = prev.trim();
        if t.is_empty() {
            // 潰したコメント（doc コメント）は空行になる
            continue;
        }
        if t.starts_with("#[") {
            attrs.push(t.to_string());
            continue;
        }
        break;
    }
    Some((name, attrs))
}

fn fn_name(line: &str) -> Option<String> {
    let t = line.trim_start();
    let t = t
        .strip_prefix("pub(crate) ")
        .or_else(|| t.strip_prefix("pub(super) "))
        .or_else(|| t.strip_prefix("pub "))
        .unwrap_or(t);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let rest = t.strip_prefix("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// 1 ファイルの違反（`rel:line 綴り`）と、寄せた先に当たった数
fn violations(rel: &str, view: &str) -> (Vec<String>, usize) {
    let lines: Vec<&str> = view.lines().collect();
    let mut found = Vec::new();
    let mut owned = 0;
    for (i, line) in lines.iter().enumerate() {
        for call in CALLS {
            if !line.contains(call) {
                continue;
            }
            let allowed = if rel.starts_with("crates/tako-app/src/self_test/") {
                true
            } else if rel == "crates/tako-control/src/lsp/manager.rs" {
                let ok = enclosing_fn(&lines, i)
                    .is_some_and(|(name, _)| OWNERS.contains(&name.as_str()));
                owned += usize::from(ok);
                ok
            } else if rel == "crates/tako-app/src/main.rs" {
                enclosing_fn(&lines, i).is_some_and(|(_, attrs)| {
                    attrs
                        .iter()
                        .any(|a| a.replace(' ', "") == "#[cfg(feature=\"visual-test\")]")
                })
            } else {
                false
            };
            if !allowed {
                found.push(format!("{rel}:{} {call}", i + 1));
            }
        }
    }
    (found, owned)
}

#[test]
fn 文書を受け持つ行を問う口は_manager_の_resolve_だけ() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["crates/tako-app/src", "crates/tako-control/src"] {
        rs_files(&root.join(dir), &mut files);
    }
    assert!(
        files.len() >= 100,
        "走査したファイルが少なすぎる: {}",
        files.len()
    );
    let mut found = Vec::new();
    let mut owned = 0;
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = rel.trim_start_matches("./").to_string();
        let src = std::fs::read_to_string(path).unwrap();
        if !CALLS.iter().any(|c| src.contains(c)) {
            continue;
        }
        let (bad, hits) = violations(&rel, &production(&src, &rel));
        found.extend(bad);
        owned += hits;
    }
    assert!(
        found.is_empty(),
        "検出表を直接引いている（開いている文書は開いたときの器で引く = `LspManager::resolve` を通す。#2113）:\n{}",
        found.join("\n")
    );
    // 寄せた先に当たらない = 走査の取り違え（読み先が違う・綴りが変わった）
    assert_eq!(
        owned, 2,
        "manager.rs の resolve / resolve_document が表を引く 2 か所に当たらない"
    );
}

/// 注入: UI の有効判定が表を直接引く形・visual-test でない関数・寄せた先でない manager の関数は名指す。
/// visual-test の関数とセルフテストは通す
#[test]
fn 注入した直接の呼び手を名指す() {
    let ui = "impl App {\n    pub(crate) fn lsp_hover_enabled_for(&self) -> bool {\n        tako_core::lsp::servers::resolve(&p).is_some()\n    }\n}\n";
    let (bad, _) = violations("crates/tako-app/src/lsp_hover_ui.rs", ui);
    assert_eq!(
        bad,
        ["crates/tako-app/src/lsp_hover_ui.rs:3 servers::resolve("]
    );

    let visual = "    #[cfg(feature = \"visual-test\")]\n    async fn hover_real_visual() {\n        let spec = tako_core::lsp::servers::resolve_in(SERVERS, p);\n    }\n";
    assert!(violations("crates/tako-app/src/main.rs", visual)
        .0
        .is_empty());
    let plain = "    async fn hover_real() {\n        let spec = tako_core::lsp::servers::resolve_in(SERVERS, p);\n    }\n";
    assert_eq!(
        violations("crates/tako-app/src/main.rs", plain).0,
        ["crates/tako-app/src/main.rs:2 servers::resolve_in("]
    );
    // 属性が別の関数のもの（間に別の関数の本体がある）なら通さない
    let other = "    #[cfg(feature = \"visual-test\")]\n    fn a() {}\n    fn b() {\n        servers::resolve_in(T, p);\n    }\n";
    assert_eq!(violations("crates/tako-app/src/main.rs", other).0.len(), 1);

    assert!(violations("crates/tako-app/src/self_test/x.rs", ui)
        .0
        .is_empty());

    let manager = "    fn resolve_document(&self) {\n        servers::resolve_in(self.table, path)\n    }\n    fn hover(&self) {\n        servers::resolve_in(self.table, path)\n    }\n";
    let (bad, owned) = violations("crates/tako-control/src/lsp/manager.rs", manager);
    assert_eq!(owned, 1);
    assert_eq!(
        bad,
        ["crates/tako-control/src/lsp/manager.rs:5 servers::resolve_in("]
    );

    let dispatch = "fn lsp_format_prepare() {\n    if tako_core::lsp::servers::resolve_with(T, p, &f).is_none() {}\n}\n";
    assert_eq!(
        violations("crates/tako-control/src/dispatch.rs", dispatch)
            .0
            .len(),
        1
    );
}
