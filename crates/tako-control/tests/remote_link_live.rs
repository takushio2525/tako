//! 実 transcript に対するリンク解決の実測テスト（#1069）
//!
//! 合成 fixture（`claude_remote_link` の unit テスト）は形を固定するが、
//! **上流が実際に書く形**とはずれうる。ここは `~/.claude/projects/` の
//! 実ファイルを読んで、`connected` / `not_connected` の両方が実物で成立することを見る。
//!
//! ## 実行環境に依存する
//!
//! claude を使っていないマシン（CI）では材料が無いので**理由を出して skip する**
//! （落とすとクリーンな環境で常に赤くなる）。材料があるときだけ本物を検査する。
//!
//! ## 出力に実値を出さない
//!
//! 見つけた session id / URL は**ログへ出さない**（#1069 の番犬と同じ基準）。
//! 出すのは件数と判定結果だけ。

use tako_control::claude_remote_link::{self, LinkState};

/// 実 transcript を (bridge 行あり, bridge 行なし) に分けて session id を返す。
/// **中身は返さない**（呼び出し側が id を出さないようにするため件数だけ持たせる）
fn sample_sessions() -> (Vec<String>, Vec<String>) {
    let mut with_bridge = Vec::new();
    let mut without_bridge = Vec::new();
    for dir in tako_control::transcript::claude_config_dirs() {
        let Ok(projects) = std::fs::read_dir(dir.join("projects")) else {
            continue;
        };
        for project in projects.flatten() {
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                // 巨大な会話は読み飛ばす（このテストの目的は形の確認）
                if path.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > 8 * 1024 * 1024 {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !tako_control::transcript::is_valid_session_id(stem) {
                    continue;
                }
                let has_bridge = text.contains("\"bridge-session\"")
                    || text.contains("\"bridge_status\"");
                if has_bridge {
                    if with_bridge.len() < 5 {
                        with_bridge.push(stem.to_string());
                    }
                } else if without_bridge.len() < 5 {
                    without_bridge.push(stem.to_string());
                }
                if with_bridge.len() >= 5 && without_bridge.len() >= 5 {
                    return (with_bridge, without_bridge);
                }
            }
        }
    }
    (with_bridge, without_bridge)
}

#[test]
fn 実transcriptで接続済みと未接続を言い分ける() {
    let (with_bridge, without_bridge) = sample_sessions();
    if with_bridge.is_empty() && without_bridge.is_empty() {
        eprintln!(
            "skip: この環境に claude の transcript が無い（材料が無いので実測できない）"
        );
        return;
    }

    // ① bridge 行を持つ会話は connected になり、**claude.ai/code の形の URL** が出る
    for sid in &with_bridge {
        let link = claude_remote_link::link_for_agent_session("claude", Some(sid));
        assert_eq!(
            link.state,
            LinkState::Connected,
            "bridge 行を持つ会話が connected にならない（id は出さない）"
        );
        let url = link.url.as_deref().expect("connected なら URL が要る");
        assert!(
            url.starts_with("https://claude.ai/code/session_"),
            "URL の形が違う（先頭 30 文字: {}）",
            &url[..url.len().min(30)]
        );
        let session_id = link.session_id.as_deref().expect("id が要る");
        assert!(session_id.starts_with("session_"), "互換形式になっていない");
        // **アカウント UUID を混ぜていない**（実ファイルには入っている）
        let rendered = link.to_json().to_string();
        assert!(
            !rendered.contains("owner"),
            "応答にアカウント情報が混ざっている"
        );
    }

    // ② bridge 行が無い会話は not_connected（**URL を捏造しない**）。
    // ただしこのマシン全体に阻害要因（DISABLE_TELEMETRY 等）があると
    // ineligible になるのが正しいので、どちらかであることを見る
    for sid in &without_bridge {
        let link = claude_remote_link::link_for_agent_session("claude", Some(sid));
        assert!(
            matches!(
                link.state,
                LinkState::NotConnected | LinkState::Ineligible { .. }
            ),
            "bridge 行が無い会話が {:?} になっている（URL を捏造している）",
            link.state
        );
        assert!(link.url.is_none(), "未接続なのに URL がある");
        assert!(link.session_id.is_none(), "未接続なのに id がある");
    }

    eprintln!(
        "実測: connected {} 件 / not_connected（または ineligible）{} 件",
        with_bridge.len(),
        without_bridge.len()
    );
}

/// claude 以外の系統は**会話を特定できても** ineligible（マトリクスの宣言と同じ）
#[test]
fn claude以外の系統はineligibleになる() {
    let (with_bridge, _) = sample_sessions();
    let sid = with_bridge.first().cloned();
    for agent in ["codex", "agy", "local", "plain"] {
        let link = claude_remote_link::link_for_agent_session(agent, sid.as_deref());
        assert!(
            matches!(link.state, LinkState::Ineligible { .. }),
            "{agent} が {:?} になっている",
            link.state
        );
        assert_eq!(link.state.as_wire(), "ineligible: agent_unsupported");
        assert!(link.url.is_none(), "{agent} に URL を出している");
    }
}

/// 存在しない会話は unknown（**not_connected と言い切らない**）
#[test]
fn 見つからない会話はunknownになる() {
    // UUID の形だが実在しない
    let link = claude_remote_link::link_for_agent_session(
        "claude",
        Some("00000000-0000-4000-8000-000000000000"),
    );
    assert_eq!(link.state, LinkState::Unknown);
    assert!(link.url.is_none());
    // 空・None も unknown
    assert_eq!(
        claude_remote_link::link_for_agent_session("claude", None).state,
        LinkState::Unknown
    );
    assert_eq!(
        claude_remote_link::link_for_agent_session("claude", Some("")).state,
        LinkState::Unknown
    );
}
