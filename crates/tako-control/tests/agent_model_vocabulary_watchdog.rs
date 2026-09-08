//! 番犬: claude 語彙のモデル / effort 既定を他系統の worker へ渡さない（#1013）
//!
//! ## なぜ止めるのか
//!
//! `codex --model claude-opus-5` は**存在しないモデル名で起動する**ので、codex が
//! モデル警告の画面で止まり、送ったプロンプトもそこで消える（#1013 の実発生）。
//! 単体テストは「今の解決結果」を固定するが、**同じ形の再発**（別の claude 語彙の
//! 既定を新しく足して、また明示指定と同じ段へ混ぜる）はソースの形でしか止められない。
//!
//! ## 2 本立て（片方だけでは穴が残る）
//!
//! 1. [`アカウント既定を明示指定と同じ段へ混ぜていない`] — 呼び出し側（`dispatch.rs`）が
//!    `model.or(<アカウント>)` の形で段を潰していないこと。修正前の実ファイルを名指しで落とす。
//! 2. [`モデル既定の継承を系統の直比較で決めていない`] — 解決の本体（`orchestrator/mod.rs`）が
//!    `agent == WorkerAgent::Claude` で分岐していないこと。判断は能力マトリクス
//!    （`keys::WORKER_MODEL_DEFAULT_INHERIT`）へ問う（AGENTS.md「agent 系統ごとの
//!    能力差はマトリクスへ書く（#982）」）。

use std::path::{Path, PathBuf};

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）。
/// **`mod tests` の直前で切る**（`#[cfg(test)]` はテスト用ヘルパにも付くので、
/// 最初の出現で切ると製品コードのほとんどを見落とす）
fn production_source(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

/// コメント行を落とす（説明文に書いた旧コードの引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の本文（`fn <name>` から次の行頭 `}` まで）を切り出す
fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let rest = &src[start..];
    let end = rest.find("\n    }\n").unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn アカウント既定を明示指定と同じ段へ混ぜていない() {
    let src = std::fs::read_to_string(crate_file("src/dispatch.rs")).expect("dispatch.rs を読む");
    let body = without_comments(production_source(&src));
    // 修正前は `model.or(resolved_account…default_model)` で spawn の明示指定と
    // アカウント既定を 1 つの Option へ潰し、agent 種別を見る前に確定させていた。
    // アカウント既定は `AccountDefaults` として**段のまま**渡す
    let joined = body.split_whitespace().collect::<Vec<_>>().join(" ");
    for pattern in [
        "model .or( resolved_account",
        "effort .or( resolved_account",
        "model.or(resolved_account",
        "effort.or(resolved_account",
    ] {
        assert!(
            !joined.contains(pattern),
            "アカウント既定を明示指定と同じ段へ潰している（`{pattern}`）。\
             AccountDefaults で段を分けて resolve_agent_launch_with_account へ渡すこと（#1013）"
        );
    }
    // 段を分けて渡している側が実在すること（番犬が空振りしないため）
    assert!(
        joined.contains("resolve_agent_launch_with_account"),
        "spawn がアカウント既定つきの解決を通っていない（#1013）"
    );
}

#[test]
fn モデル既定の継承を系統の直比較で決めていない() {
    let src = std::fs::read_to_string(crate_file("src/orchestrator/mod.rs"))
        .expect("orchestrator/mod.rs を読む");
    let body = fn_body(production_source(&src), "pub fn resolve_agent_launch_in(");
    let code = without_comments(&body);
    assert!(
        !code.contains("WorkerAgent::Claude"),
        "claude 語彙の既定を渡すかどうかを系統の直比較で決めている。\
         能力マトリクス（keys::WORKER_MODEL_DEFAULT_INHERIT）へ問うこと（#982 / #1013）:\n{code}"
    );
    assert!(
        code.contains("inherits_claude_vocabulary_defaults"),
        "マトリクスへ問う入口を通っていない（#1013）:\n{code}"
    );
}

#[test]
fn 継承の可否はマトリクスの1マスが答える() {
    // マトリクス側の宣言（#982）。claude だけが継承でき、他系統は「対象外」
    use tako_core::agent_support::{keys, supports, Agent};
    assert!(supports(Agent::Claude, keys::WORKER_MODEL_DEFAULT_INHERIT));
    for agent in [Agent::Codex, Agent::Agy, Agent::Local] {
        assert!(
            !supports(agent, keys::WORKER_MODEL_DEFAULT_INHERIT),
            "{agent:?} に claude 語彙のモデル既定を渡してはいけない（#1013）"
        );
    }
}
