//! ゼロスタート導入が「claude だけ通る形」へ戻らないための番犬（Issue #989）
//!
//! #868 は `AgentKind` を Claude 1 値で作り、`setup_bootstrap` の全 API を
//! 引数なし（= claude 固定）で置いた。その形が 1 年残り、codex / agy は
//! 「URL を見せるだけ」のままだった（#975 の基準 2 のギャップ）。
//!
//! 同じ形に戻る道は 3 つある。3 つとも塞ぐ:
//!
//! 1. **手順の穴**: `AgentKind` に値を足したのに `recipe` の中身が claude の写しになる
//! 2. **入口の復活**: `setup_bootstrap` に引数なしの claude 既定 API が生える
//!    （新しい機能がそこへ吸い寄せられて 1 系統だけ通る形へ戻る）
//! 3. **判定の散り**: 呼び出し側が `AgentKind::Claude` を直に書いて分岐する

use std::path::Path;
use tako_core::platform::agent_install::{self, AgentKind};
use tako_core::platform::support::Platform;

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 1. 手順が系統ごとに**実際に違う**こと。
///
/// 「値を足したが中身は claude の写し」だと、状態照会は 3 行出るのに
/// 全部 claude を入れようとする（一番たちの悪い壊れ方）
#[test]
fn 系統ごとの手順が実際に違う() {
    let home = Path::new("/tmp/watchdog-home");
    for platform in [Platform::MacOs, Platform::Windows] {
        let recipes: Vec<_> = AgentKind::ALL
            .into_iter()
            .map(|a| agent_install::recipe(platform, a))
            .collect();
        for (i, a) in recipes.iter().enumerate() {
            for b in recipes.iter().skip(i + 1) {
                assert_ne!(
                    a.source.url,
                    b.source.url,
                    "{platform:?}: 取得元が同じ（{} と {}）",
                    a.agent.as_str(),
                    b.agent.as_str()
                );
                assert_ne!(
                    a.launcher_path_in(home),
                    b.launcher_path_in(home),
                    "{platform:?}: 置き場所が同じ（{} と {}）",
                    a.agent.as_str(),
                    b.agent.as_str()
                );
            }
            // 取得元 URL にその系統を示す語が入っている（claude の URL の使い回しを弾く）
            assert!(
                !(a.agent != AgentKind::Claude && a.source.url.contains("claude.ai")),
                "{}: claude の取得元を使い回している（{}）",
                a.agent.as_str(),
                a.source.url
            );
        }
    }
}

/// 2. 引数なしの claude 既定 API が生えていないこと。
///
/// `pub fn status()` のような入口があると、新しい呼び出し側がそこへ流れて
/// また claude だけ通る形へ戻る。**系統を受け取るか、受け取らない理由が
/// はっきりしているか**のどちらかにする
#[test]
fn claude既定の引数なし入口が生えていない() {
    let src = read("crates/tako-control/src/setup_bootstrap.rs");
    // 系統を引数で受けるべき操作（3 系統で答えが変わるもの）
    const AGENT_SCOPED: &[&str] = &[
        "recipe",
        "status",
        "install",
        "ensure_path",
        "undo_path",
        "install_plan",
        "resolve_binary",
        "is_authenticated",
        "auth_instructions",
        "handoff_plan",
        "handoff_candidates",
    ];
    let mut offenders = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("pub fn ") else {
            continue;
        };
        let Some((name, args)) = rest.split_once('(') else {
            continue;
        };
        if !AGENT_SCOPED.contains(&name) {
            continue;
        }
        // 引数で系統を受けているなら OK（`_for` 付きの名前も上のリストに当たらない）
        if args.contains("AgentKind") {
            continue;
        }
        offenders.push(name.to_string());
    }
    assert!(
        offenders.is_empty(),
        "系統を受け取らない claude 既定の入口が生えている: {offenders:?}\n\
         `<名前>_for(agent: AgentKind, …)` の形にすること（#989）"
    );
    // 期待する形が実際に在ることも見る（名前を全部消して通す抜け道を塞ぐ）
    for expected in [
        "pub fn status_for(agent: AgentKind)",
        "pub fn install_for(agent: AgentKind",
        "pub fn ensure_path_for(agent: AgentKind)",
        "pub fn auth_instructions_for(agent: AgentKind)",
        "pub fn resolve_binary_for(agent: AgentKind)",
    ] {
        assert!(src.contains(expected), "{expected} が無い");
    }
}

/// 3. 呼び出し側に `AgentKind::Claude` の直書き分岐が散っていないこと。
///
/// 許すのは「既定値としての claude」（引数省略時 / A/B の legacy 経路）だけで、
/// **能力の有無を claude かどうかで判断しない**（#982 と同じ規律）
#[test]
fn 呼び出し側にclaude直比較が散っていない() {
    const FILES: &[&str] = &[
        "crates/tako-cli/src/setup.rs",
        "crates/tako-control/src/setup_bootstrap.rs",
        "crates/tako-control/src/dispatch.rs",
        "crates/tako-control/src/orchestrator/agent_cli.rs",
    ];
    let mut offenders = Vec::new();
    for file in FILES {
        for (i, line) in read(file).lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            // 比較（`== AgentKind::Claude` / `!= …`）だけを見る。
            // 既定値としての代入（`None => AgentKind::Claude`）や
            // 網羅 match（`AgentKind::Claude => …`）は対象にしない
            let is_comparison = trimmed.contains("== AgentKind::Claude")
                || trimmed.contains("!= AgentKind::Claude")
                || trimmed.contains("matches!(agent, AgentKind::Claude)");
            if !is_comparison {
                continue;
            }
            // A/B の legacy 経路（#1129）と最簡形の判定（#322）は理由つきで許す
            if line.contains("legacy_auth_launch") || line.contains("agent_option") {
                continue;
            }
            offenders.push(format!("{file}:{}: {trimmed}", i + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "claude かどうかで能力を分けている箇所がある（#989 / #982）:\n{}",
        offenders.join("\n")
    );
}

/// `TAKO_989_LEGACY=1` が効くこと（同一バイナリで #989 前へ戻せる）。
/// **A/B の逃げ道が消えていないか**を見る
#[test]
fn legacy_envの逃げ道が残っている() {
    let src = read("crates/tako-control/src/setup_bootstrap.rs");
    assert!(src.contains("TAKO_989_LEGACY"), "A/B の env が消えている");
    assert!(
        src.contains("pub fn bootstrap_agents()"),
        "面倒を見る系統の入口が消えている"
    );
}
