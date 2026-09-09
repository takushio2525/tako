//! codex / agy の復元（会話ごと戻す）の番犬（#1238）
//!
//! #1076 で claude の復元を直したときの壊れ方は 2 つあった。
//! **同じ 2 つが、系統を広げた経路で作り直される**のを機械的に禁じる。
//!
//! 1. 保存済み ID を「検出できなかった」だけで捨てる（再起動直後は起動途中で
//!    どの系統も検出に出ない → 復元の種を自分で失う）
//! 2. resume コマンドを呼び出し側が自前で組む（役割 env / 起動条件が落ちる・
//!    系統ごとの書式が 2 か所に分かれる）
//!
//! さらに #982 の要求として、**系統ごとの差はマトリクスの 1 マス**で宣言し、
//! 呼び出し側が `if agent == "codex"` を持たないことも見張る。

use std::path::{Path, PathBuf};

use tako_core::agent_resume::restore_support;
use tako_core::agent_support::{keys, supports, Agent};
use tako_core::platform::support::Platform;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 行コメント（規約の説明文・doc コメント）を落とした本文
fn code_lines(source: &str) -> Vec<(usize, &str)> {
    source
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

/// `#[cfg(test)]` 以降（テストは期待値としてコマンド文字列を書くので走査対象外）
fn without_tests(source: &str) -> String {
    match source.find("\n#[cfg(test)]\n") {
        Some(i) => source[..i].to_string(),
        None => source.to_string(),
    }
}

const MAIN: &str = "crates/tako-app/src/main.rs";
const SESSIONS: &str = "crates/tako-control/src/sessions.rs";
const CORE_RULE: &str = "crates/tako-core/src/agent_resume.rs";

/// 1: GUI 側は保持規則の API 越しにしか codex / agy の ID マップを触らない
#[test]
fn codexとagyのidマップを丸ごと置き換えない() {
    let source = read(MAIN);
    assert!(
        source.contains("agent_resume_sessions"),
        "codex / agy の復元用 ID を保持していない（#1238）。\n\
         保持規則は tako_core::agent_resume::AgentResumeIds（確認してから外す）が正本"
    );
    let mut bad = Vec::new();
    for (line_no, line) in code_lines(&source) {
        if !line.contains("agent_resume_sessions") {
            continue;
        }
        let assigns = [
            "self.agent_resume_sessions =",
            "app.agent_resume_sessions =",
        ]
        .iter()
        .any(|pat| line.contains(pat))
            || line.contains("agent_resume_sessions.clear()");
        if assigns {
            bad.push(format!("{MAIN}:{line_no}: {}", line.trim()));
        }
    }
    assert!(
        bad.is_empty(),
        "codex / agy の復元用 ID マップを丸ごと置き換え / 全消ししている（#1076 と同じ根因）。\n\
         seed / apply_scan / remove 越しに触ること:\n  {}",
        bad.join("\n  ")
    );
}

/// 1': 「子プロセスが無い」だけで全消しする形が、**両方の系統で**戻ってこないこと
#[test]
fn 子プロセス不在の分岐は両系統の保持規則を通る() {
    let source = read(MAIN);
    let idx = source
        .find("if !has_children {")
        .expect("has_children の前段ガードが見つからない（#368 の構造が変わった）");
    let block = &source[idx..idx + 1200];
    for call in [
        "apply_claude_resume_sessions(&[])",
        "apply_agent_resume_sessions(",
    ] {
        assert!(
            block.contains(call),
            "has_children が false の分岐が {call} を通っていない（#1076 / #1238）。\n\
             再起動直後はどの系統も起動途中で子を持たないので、\n\
             ここで捨てると復元の種を自分で失う:\n{block}"
        );
    }
}

/// 2: 復元経路は系統ごとの resume コマンドを自前で組まない（正本は sessions 側 1 か所）
#[test]
fn 復元経路は系統ごとのresumeコマンドを自前で組まない() {
    let body = without_tests(&read(MAIN));
    // 上流 CLI の resume 書式そのもの。`agent_resume::resume_spec` の外に出てはいけない
    const LITERALS: &[&str] = &["codex resume", "--conversation", "--resume"];
    let mut bad = Vec::new();
    for (line_no, line) in code_lines(&body) {
        for lit in LITERALS {
            if line.contains(lit) {
                bad.push(format!("{MAIN}:{line_no}: {}", line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "復元経路が resume コマンドを自前で組んでいる（#1076 / #1238）。\n\
         書式の正本は tako_core::agent_resume::resume_spec で、\n\
         組み立ては tako_control::sessions::resume_command 1 か所:\n  {}",
        bad.join("\n  ")
    );
}

/// 3: 系統ごとの書式は正本（`resume_spec`）から引く。
/// 組み立て側に上流 CLI の綴りを直書きしない
#[test]
fn resumeの書式は正本から引く() {
    let body = without_tests(&read(SESSIONS));
    assert!(
        body.contains("agent_resume::resume_spec("),
        "sessions が系統ごとの書式を正本（tako_core::agent_resume::resume_spec）から引いていない（#1238）"
    );
    let mut bad = Vec::new();
    for (line_no, line) in code_lines(&body) {
        for lit in ["codex resume", "--conversation"] {
            if line.contains(lit) {
                bad.push(format!("{SESSIONS}:{line_no}: {}", line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "上流 CLI の resume の綴りが組み立て側に直書きされている（#982 / #1238）。\n\
         系統差は resume_spec の 1 マスに置くこと:\n  {}",
        bad.join("\n  ")
    );
}

/// 4: 「保存されていない」と「この環境には手段が無い」を分ける判定は 1 か所を通る
#[test]
fn 復元の可否はひとつの判定を通る() {
    let source = read(SESSIONS);
    let idx = source
        .find("pub fn restore_plan_in(")
        .expect("restore_plan_in が見つからない");
    let end = source[idx..]
        .find("\n/// 起動条件の記録を引く")
        .map(|e| idx + e)
        .expect("restore_plan_in の本体の終わりが見つからない");
    let body = &source[idx..end];
    assert!(
        body.contains("agent_resume::restore_support("),
        "復元の可否を tako_core::agent_resume::restore_support 経由で判定していない（#982 / #1238）:\n{body}"
    );
    assert!(
        body.contains("resume_command_with_env("),
        "restore_plan が resume_command の組み立てを共有していない（#1076）:\n{body}"
    );
}

/// 5: マトリクスの宣言（#982）と実装の判定が一致する。
/// **過大にも過小にも申告しない**（AGENTS.md の不変条件）
#[test]
fn マトリクスの宣言と実装の判定が一致する() {
    for agent in [Agent::Claude, Agent::Codex, Agent::Agy] {
        assert_eq!(
            supports(agent, keys::RESTORE_AFTER_REBOOT),
            restore_support(agent, Platform::MacOs).is_wired(),
            "{agent:?} のマトリクス宣言（restore_after_reboot）が実装と食い違っている"
        );
    }
    // ローカル LLM はまだ会話の器が定まっていない（#991）
    assert!(!supports(Agent::Local, keys::RESTORE_AFTER_REBOOT));
    assert!(!restore_support(Agent::Local, Platform::MacOs).is_wired());
}

/// 6: 会話 ID の在り処は**実測の記録つき**で 1 か所に書いてある
/// （綴りを直す人が、何を見て決めたのかを追える）
#[test]
fn 会話idの在り処が実測つきで記録されている() {
    let doc = read(CORE_RULE);
    for marker in [
        "thread-writer-locks",
        "presence/",
        "codex resume",
        "--conversation",
        "lsof",
    ] {
        assert!(
            doc.contains(marker),
            "{CORE_RULE} に `{marker}` の記録が無い（#1238 の調査結果が失われている）"
        );
    }
}
