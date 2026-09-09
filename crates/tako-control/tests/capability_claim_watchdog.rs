//! 番犬: 「その系統では effort が効かない」と書いた説明文を、能力マトリクスの宣言と突き合わせる
//! （Issue #1248 / #1002）
//!
//! #1002 で agy にも `--effort low|medium|high` を渡すようになったのに、説明文だけが
//! **4 ファイルに散らばったまま**「agy は無視」と言い続けていた（CLI の `--help` /
//! `protocol.rs` の doc / MCP catalog の description / `orchestrator/agent.rs` の doc）。
//! 挙動は 1 実装に寄っていても、それを説明する文は寄っていないので、直した箇所以外は
//! 誰も気づかないまま古い姿を語り続ける。
//!
//! そこで**文どうしを比べず**、能力マトリクス（`agent_support::MATRIX` の
//! [`keys::EFFORT_CONTROL`]）が supported と宣言している系統について、
//! 「無視 / 非対応」と読める説明文が残っていないかを走査する。
//!
//! 走査は `crates/` と `.agent/` の全 rs / md。除外は [`is_excluded`] の 3 種だけ。

use std::path::{Path, PathBuf};
use tako_core::agent_support::{self, keys, Agent};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// 走査対象の拡張子（Rust のソースと AI 向け仕様書）
fn is_target(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "rs" || e == "md")
}

/// 走査から外すもの。
///
/// - 旧説を**わざと引用する** 2 ファイル: 宣言の置き場（`agent_support.rs` の根拠欄が
///   「読み違えるな」と注意している）と、同じ引用を含むこのファイル
/// - 追記専用の作業ログ（`progress.md` / `progress-archive.md`）: そのときの事実の記録なので、
///   番犬に合わせて過去の記述を書き換えるほうが間違い
fn is_excluded(path: &Path) -> bool {
    path.ends_with("crates/tako-core/src/agent_support.rs")
        || path.ends_with("crates/tako-control/tests/capability_claim_watchdog.rs")
        || matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("progress.md" | "progress-archive.md")
        )
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // 生成物・依存物は対象外
            let skip = matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("target" | "node_modules" | "dist" | "testdata")
            );
            if !skip {
                collect(&path, out);
            }
        } else if is_target(&path) && !is_excluded(&path) {
            out.push(path);
        }
    }
}

/// 「この系統では効かない」と読める言い回し
const NEGATIONS: &[&str] = &[
    "無視",
    "指定手段が無",
    "非対応",
    "対応していない",
    "効かない",
];

#[test]
fn effortが効かないという説明はマトリクスの宣言と一致する() {
    let root = repo_root();
    let mut files = Vec::new();
    collect(&root.join("crates"), &mut files);
    collect(&root.join(".agent"), &mut files);

    let supported: Vec<&str> = Agent::ALL
        .into_iter()
        .filter(|a| agent_support::supports(*a, keys::EFFORT_CONTROL))
        .map(|a| a.as_str())
        .collect();
    assert!(
        supported.contains(&"agy"),
        "この検査は agy = supported を前提にしている（#1002）"
    );

    let mut hits = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            // effort の話をしている行だけを見る（master 非対応など別機能の記述を巻き込まない）
            if !line.to_ascii_lowercase().contains("effort") {
                continue;
            }
            for name in &supported {
                if !line.contains(name) {
                    continue;
                }
                if let Some(neg) = NEGATIONS.iter().find(|n| line.contains(**n)) {
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    hits.push(format!(
                        "{}:{} [{name} / {neg}] {}",
                        rel.display(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        hits.is_empty(),
        "マトリクスは {supported:?} の {} を supported と宣言しているのに、\
         「効かない」と読める説明文が残っている（説明文かマトリクスのどちらかが古い）:\n{}",
        keys::EFFORT_CONTROL,
        hits.join("\n")
    );
}
