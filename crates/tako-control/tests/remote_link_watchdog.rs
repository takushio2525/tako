//! 番犬: Remote Control の session URL / id を診断ログへ出さない（#1069）
//!
//! ## なぜ止めるのか
//!
//! `https://claude.ai/code/session_…` は**開くには claude.ai ログインが要る**
//! （実測 403）ので秘密そのものではない。ただしセッション id は
//! `claude -p --cloud <id>` の**宛先**になるので、共有されると
//! 「その会話へ指示を送れる」入口になる。tako は診断ログ（persist.log / perf.log /
//! audit.log / flow log / stderr）にペイン内容を書かないのと**同じ基準**で扱う
//! （AGENTS.md の絶対ルール）。
//!
//! ## 2 本立て（片方だけでは穴が残る）
//!
//! 1. [`リンク解決の実装は診断ログへ書かない`] — 解決層の関数本文をソース走査する。
//!    ここが唯一 URL / id を握っている層なので、書くならまずここに現れる。
//! 2. [`アカウント_uuid_を読む経路がリンク解決に無い`] — `bridge-session` 行の
//!    `ownerAccountUuid` / `ownerOrganizationUuid` を**読むコード自体**を禁じる。
//!    「読んだが返していない」状態を許すと、次の変更で漏れる余地が残る。
//!
//! ## 検出力
//!
//! どちらも「1 行足すと落ちる」ことを実測して入れている（PR 本文に A/B を記録）。

use std::path::{Path, PathBuf};

fn link_rs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/claude_remote_link.rs")
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない。
/// #913 の `open_files` 番犬と同じ作法）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]") {
        Some(i) => &src[..i],
        None => src,
    }
}

/// コメント行を落とす（説明文の中の `eprintln!` / `ownerAccountUuid` を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn リンク解決の実装は診断ログへ書かない() {
    let src = std::fs::read_to_string(link_rs()).expect("claude_remote_link.rs を読む");
    let body = without_comments(production_source(&src));
    // 診断ログの入口すべて。**新しい記録先を作ったらここへ足す**
    const FORBIDDEN: &[&str] = &[
        "println!",
        "eprintln!",
        "print!",
        "eprint!",
        "persist_diag",
        "perf_log",
        "flow_log",
        "audit_",
        "log::",
        "tracing::",
    ];
    for (n, line) in body.lines().enumerate() {
        for bad in FORBIDDEN {
            assert!(
                !line.contains(bad),
                "claude_remote_link.rs の {} 行目に {bad} がある。\n\
                 session URL / id は `claude -p --cloud <id>` の宛先になるので、\n\
                 ペイン内容と同じ基準で診断ログへ出さない（#1069 / AGENTS.md の絶対ルール）:\n  {line}",
                n + 1
            );
        }
    }
}

#[test]
fn アカウント_uuid_を読む経路がリンク解決に無い() {
    let src = std::fs::read_to_string(link_rs()).expect("claude_remote_link.rs を読む");
    let body = without_comments(production_source(&src));
    for key in ["ownerAccountUuid", "ownerOrganizationUuid"] {
        assert!(
            !body.contains(key),
            "claude_remote_link.rs の production コードが {key} を読んでいる。\n\
             アカウント UUID は**保持も返却もしない**（どのアカウントかは accounts.yaml の\n\
             名前 = account_label で表す。#1069 / #927）"
        );
    }
}

/// リンクを配る 3 経路が**同じ 1 実装**を通る（値が食い違わないことの構造的な担保）。
///
/// `RemoteLink::to_json` 以外で `remote_link` のキーを組んでいる箇所があれば、
/// そこは別の形を返しうる = PWA / CLI / MCP のどれかで表示が変わる
#[test]
fn remote_link_の形は1実装だけが組む() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート");
    // 走査対象は Rust のソース全体（PWA 側は表示なので対象外）
    let mut builders: Vec<String> = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // テストは対象外（期待値として形を書くのは正当）
            if path.components().any(|c| c.as_os_str() == "tests") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let production = without_comments(production_source(&text));
            // **代入だけを見る**（`&result["remote_link"]` のような読み出しは正当）。
            // 値が次の行へ折り返すので、代入の直後 240 文字を窓にして `to_json` を探す
            let mut from = 0usize;
            while let Some(rel) = production[from..].find("[\"remote_link\"] =") {
                let at = from + rel;
                from = at + 1;
                let window_end = (at + 240).min(production.len());
                let window = &production[at..window_end];
                if window.contains("to_json") {
                    continue;
                }
                let line_no = production[..at].lines().count();
                let line = production[at..]
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                builders.push(format!("{}:{}: {}", path.display(), line_no, line));
            }
        }
    }
    assert!(
        builders.is_empty(),
        "remote_link の形を RemoteLink::to_json 以外で組んでいる箇所がある\n\
         （3 経路で表示が食い違う。#1069 の FR-2.35.4）:\n{}",
        builders.join("\n")
    );
}
