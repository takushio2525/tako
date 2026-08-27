//! agent 能力マトリクス（#982）のパリティテスト。
//!
//! 狙いは 2 つ。
//!
//! 1. **同じ概念の enum が 5 つ並存している**（棚卸し §8.1 の表）。統合は段階的にしか
//!    できないので、その間のズレをテストで押さえる。どれかに値が増減したらここが落ち、
//!    「正本のマトリクスと `.agent/agent-enums.md` も直せ」と言う
//! 2. 正本 `tako_core::agent_support` の**列挙そのもの**が、既存 enum の和と食い違わないこと
//!
//! `tako-cli::setup::SetupAgent` は**非公開 enum** なので型としては見えない。
//! 5 つを同じ 1 つの規則で見張るために、判定は**ソース走査**でそろえている
//! （この方式はリポジトリの既存作法。`platform_parity.rs` の番犬群と同じ）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tako_control::agents_sync;
use tako_control::orchestrator::agent::WorkerAgent;
use tako_core::agent_support::{self, Agent};
use tako_core::platform::agent_install;
use tako_core::terminal::LimitService;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control から 2 階層上がリポジトリルート")
        .to_path_buf()
}

/// 見張る対象の enum。**5 つ + 正本**。
///
/// `why` は「なぜこの値の集合なのか」。増減したときに何を直すべきかが分かるようにする
struct Watched {
    path: &'static str,
    name: &'static str,
    variants: &'static [&'static str],
    why: &'static str,
}

const WATCHED: &[Watched] = &[
    Watched {
        path: "crates/tako-core/src/agent_support.rs",
        name: "Agent",
        variants: &["Claude", "Codex", "Agy", "Local"],
        why: "能力マトリクスの正本。TUI 3 系統 + ローカル LLM（#990 / #991）",
    },
    Watched {
        path: "crates/tako-control/src/orchestrator/agent.rs",
        name: "WorkerAgent",
        variants: &["Claude", "Codex", "Agy"],
        why: "worker として起動できる系統。TUI をキー操作で駆動する前提なので Local を持たない",
    },
    Watched {
        path: "crates/tako-cli/src/setup.rs",
        name: "SetupAgent",
        variants: &["Claude", "Codex", "Agy"],
        why: "setup を進行できる系統。非公開 enum なのでソース走査でしか見張れない",
    },
    Watched {
        path: "crates/tako-control/src/agents_sync.rs",
        name: "AgentKind",
        variants: &["Claude", "Codex", "Agy"],
        why: "共通ルールの同期先（#136）。グローバル指示ファイルを持つ系統",
    },
    Watched {
        path: "crates/tako-core/src/platform/agent_install.rs",
        name: "AgentKind",
        variants: &["Claude"],
        why: "自動インストールに対応する系統（#868）。codex / agy への拡張は #989",
    },
    Watched {
        path: "crates/tako-core/src/terminal.rs",
        name: "LimitService",
        variants: &["Claude", "Codex", "Agy"],
        why: "利用制限の表示対象（#357）。ローカルモデルに利用制限の概念が無い",
    },
];

/// ソースから `enum <name> { ... }` のバリアント名を拾う。
/// 対象はいずれも単純な unit variant なので、属性・doc コメントを飛ばせば足りる
fn variants_of(src: &str, name: &str) -> BTreeSet<String> {
    let needle = format!("enum {name} {{");
    let start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("enum {name} の定義が見つからない"))
        + needle.len();
    let mut out = BTreeSet::new();
    for line in src[start..].lines() {
        let t = line.trim();
        if t == "}" {
            break;
        }
        if t.is_empty() || t.starts_with("//") || t.starts_with("#[") || t.starts_with("/*") {
            continue;
        }
        // `Claude,` / `Claude` / `Claude(..)` のいずれか
        let ident: String = t
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !ident.is_empty() && ident.starts_with(|c: char| c.is_ascii_uppercase()) {
            out.insert(ident);
        }
    }
    assert!(
        !out.is_empty(),
        "enum {name} のバリアントを 1 つも拾えなかった"
    );
    out
}

/// **どれかの enum に値が増減したら落ちる**。
///
/// 増減自体は正しい変更なので、落ちたときにやることは 3 つ:
/// ① `WATCHED` の期待値を直す ② `agent_support::MATRIX` の列と根拠を見直す
/// ③ `.agent/agent-enums.md` の対応表を直す
#[test]
fn agent系統のenumが5つとも期待どおりの値を持つ() {
    let root = repo_root();
    for w in WATCHED {
        let src = std::fs::read_to_string(root.join(w.path))
            .unwrap_or_else(|e| panic!("{} が読めない: {e}", w.path));
        let found = variants_of(&src, w.name);
        let want: BTreeSet<String> = w.variants.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            found, want,
            "{}::{} の値が変わった（{}）。\
             ① このテストの WATCHED ② crates/tako-core/src/agent_support.rs の MATRIX と根拠 \
             ③ .agent/agent-enums.md の対応表 の 3 つを直すこと",
            w.path, w.name, w.why
        );
    }
}

/// 5 つの enum の**和**が正本の列挙に収まっていること。
///
/// どれかが正本より広い値を持ったら（例: 誰かが `Gemini` を足したら）
/// マトリクスに列が無いまま能力を語れることになるので落とす
#[test]
fn 既存enumの和が正本の列挙に収まっている() {
    let root = repo_root();
    let canonical: BTreeSet<String> = Agent::ALL.iter().map(|a| format!("{a:?}")).collect();
    for w in WATCHED.iter().filter(|w| w.name != "Agent") {
        let src = std::fs::read_to_string(root.join(w.path)).expect("読める");
        for v in variants_of(&src, w.name) {
            assert!(
                canonical.contains(&v),
                "{}::{} に正本（agent_support::Agent）が知らない値 {v} がある。\
                 マトリクスへ列を足してから使うこと",
                w.path,
                w.name
            );
        }
    }
}

/// 型として見える 4 つは**網羅 match の変換**でも押さえる。
/// ソース走査が壊れても（正規表現の穴・書式の変化）こちらが残る二重化
#[test]
fn 型で見える4つのenumが正本へ写る() {
    // WorkerAgent（tako-control）: TUI 3 系統と 1:1
    let from_worker: Vec<Agent> = WorkerAgent::ALL.iter().map(|v| Agent::from(*v)).collect();
    assert_eq!(from_worker, Agent::TUI.to_vec());
    for v in WorkerAgent::ALL {
        assert_eq!(Agent::from(v).as_str(), v.as_str());
        // 逆向きは Local を落とす部分写像
        assert_eq!(WorkerAgent::try_from(Agent::from(v)), Ok(v));
    }
    assert!(
        WorkerAgent::try_from(Agent::Local).is_err(),
        "ローカル LLM を worker として起動できるようになったら、\
         WorkerAgent へ値を足してマトリクスの local 列も更新すること（#991）"
    );

    // agents_sync（tako-control）: ラベルが正本の種別名と一致する
    let sync: Vec<&str> = agents_sync::AgentKind::all()
        .iter()
        .map(|k| k.label())
        .collect();
    assert_eq!(sync, vec!["claude", "codex", "agy"]);
    for k in agents_sync::AgentKind::all() {
        assert!(
            Agent::parse(k.label()).is_some(),
            "agents_sync::AgentKind::{k:?} に対応する正本の値が無い"
        );
    }

    // tako-core 側の 2 つは From を持つ（詳細は tako-core の単体テスト）
    assert_eq!(Agent::from(LimitService::Codex), Agent::Codex);
    assert_eq!(Agent::from(agent_install::AgentKind::Claude), Agent::Claude);
}

/// 正本の種別名（`as_str`）が 5 つの enum の表記とそろっていること。
/// 表記がずれると設定ファイル・CLI 引数・プロファイルの読み書きで取り違える
#[test]
fn 種別名の表記が全経路でそろっている() {
    for a in Agent::TUI {
        let s = a.as_str();
        assert_eq!(WorkerAgent::parse(s).map(Agent::from), Ok(a));
        assert_eq!(LimitService::parse(s).map(Agent::from), Some(a));
        assert_eq!(agents_sync::AgentKind::parse(s).map(|k| k.label()), Some(s));
    }
    // ローカル LLM は既存 enum のどれにも無い（まだ成立していない枠）
    assert!(WorkerAgent::parse(Agent::Local.as_str()).is_err());
    assert_eq!(LimitService::parse(Agent::Local.as_str()), None);
}

/// マトリクスの各行が**最低 1 系統ぶんの実のある情報**を持っていること。
/// 「全部 Supported」や「全部 Pending + 根拠なし」の行は、書いた意味が無い
#[test]
fn マトリクスの行が情報を持っている() {
    for f in agent_support::MATRIX {
        let statuses: Vec<&str> = Agent::ALL.iter().map(|a| f.on(*a).status()).collect();
        assert!(
            statuses.iter().any(|s| *s != "supported"),
            "{} は 4 系統すべて supported。系統差が無いなら能力マトリクスへ載せる必要が無い",
            f.key
        );
        assert!(
            !f.summary.ja().trim().is_empty(),
            "{} に利用者向けの説明が無い",
            f.key
        );
    }
}
