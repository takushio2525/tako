//! agents — `claude agents --json` のプロキシと tmux ペイン対応付け
//!
//! スマホリモート（Issue #23）の「エージェント一覧」用。`claude agents --json` の
//! 出力（pid / cwd / sessionId / status / name 等）を正規化し、tmux ペインの
//! `pane_pid` からプロセス祖先を辿って「どのペインで動いているか」を対応付ける。

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

/// プロセス祖先辿りの最大段数（暴走防止。シェル → claude は通常 1〜3 段）
const MAX_ANCESTOR_HOPS: usize = 20;

/// `claude agents --json` を実行してエージェント一覧を正規化して返す。
/// 各要素: `{ session_id, status, ctx_percent, model, name, kind, cwd, pid, started_at }`
/// （元 JSON に無いフィールドは null）
pub fn list_agents() -> Result<Vec<Value>, String> {
    let stdout = crate::orchestrator::run_claude_agents_json()
        .ok_or("claude agents --json の実行に失敗（claude CLI が見つからないか異常終了）")?;
    let text = String::from_utf8(stdout).map_err(|e| format!("出力が UTF-8 でない: {e}"))?;
    let parsed: Value =
        serde_json::from_str(&text).map_err(|e| format!("JSON パースエラー: {e}"))?;
    let Some(items) = parsed.as_array() else {
        return Err("claude agents --json の出力が配列でない".into());
    };
    Ok(items.iter().map(normalize_agent).collect())
}

/// `claude agents --json` の 1 エントリをリモート API 向けに正規化する
fn normalize_agent(raw: &Value) -> Value {
    json!({
        "session_id": raw["sessionId"],
        "status": raw["status"],
        "ctx_percent": raw["contextPercentUsed"],
        "model": raw["model"],
        "name": raw["name"],
        "kind": raw["kind"],
        "cwd": raw["cwd"],
        "pid": raw["pid"],
        "started_at": raw["startedAt"],
    })
}

/// 各エージェントへ `pane` フィールド（tmux ペイン ID `session:window.pane`）を付与する。
/// `panes` は (ペイン ID, pane_pid) のリスト、`parents` は全プロセスの pid → ppid マップ。
/// エージェントの pid から祖先を辿り、いずれかの pane_pid に到達したら対応付ける
pub fn attach_pane_ids(agents: &mut [Value], panes: &[(String, u32)], parents: &HashMap<u32, u32>) {
    let pane_by_pid: HashMap<u32, &str> =
        panes.iter().map(|(id, pid)| (*pid, id.as_str())).collect();
    for agent in agents.iter_mut() {
        let Some(pid) = agent["pid"].as_u64().map(|p| p as u32) else {
            continue;
        };
        if let Some(pane_id) = find_ancestor_pane(pid, parents, &pane_by_pid) {
            agent["pane"] = json!(pane_id);
        }
    }
}

/// pid の祖先チェーン（自身を含む）を辿り、pane_pid 集合に一致するものを探す
fn find_ancestor_pane(
    pid: u32,
    parents: &HashMap<u32, u32>,
    pane_by_pid: &HashMap<u32, &str>,
) -> Option<String> {
    let mut current = pid;
    for _ in 0..MAX_ANCESTOR_HOPS {
        if let Some(pane_id) = pane_by_pid.get(&current) {
            return Some(pane_id.to_string());
        }
        match parents.get(&current) {
            Some(&ppid) if ppid != 0 && ppid != current => current = ppid,
            _ => break,
        }
    }
    None
}

/// `ps -axo pid=,ppid=` で全プロセスの親子マップを作る
pub fn process_parent_map() -> HashMap<u32, u32> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output();
    let Ok(output) = output else {
        return HashMap::new();
    };
    parse_parent_map(&String::from_utf8_lossy(&output.stdout))
}

/// `ps -axo pid=,ppid=` の出力をパースする
fn parse_parent_map(text: &str) -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (it.next(), it.next()) else {
            continue;
        };
        if let (Ok(pid), Ok(ppid)) = (pid.parse(), ppid.parse()) {
            map.insert(pid, ppid);
        }
    }
    map
}

/// caller_pid の祖先チェーンを辿り、バックエンドセッションの pane_pid に到達時にセッション名を返す（#288）
fn find_ancestor_backend(
    caller_pid: u32,
    parents: &HashMap<u32, u32>,
    backend_pids: &[(String, u32)],
) -> Option<String> {
    let pid_to_session: HashMap<u32, &str> = backend_pids
        .iter()
        .map(|(name, pid)| (*pid, name.as_str()))
        .collect();
    let mut current = caller_pid;
    for _ in 0..MAX_ANCESTOR_HOPS {
        if let Some(session) = pid_to_session.get(&current) {
            return Some(session.to_string());
        }
        match parents.get(&current) {
            Some(&ppid) if ppid != 0 && ppid != current => current = ppid,
            _ => break,
        }
    }
    None
}

/// caller_pid のプロセス祖先を辿り、tako バックエンドの pane_pid に一致するペインを返す（#288）
pub fn resolve_pane_by_pid(caller_pid: u32, pane_backends: &[(u64, String)]) -> Option<u64> {
    let socket = tako_core::tmux_backend::socket_name();
    let all_tmux_panes = tmux_pane_pids(Some(&socket));
    if all_tmux_panes.is_empty() {
        return None;
    }
    let parents = process_parent_map();
    for (tako_pane_id, backend_session) in pane_backends {
        let backend_pids: Vec<(String, u32)> = all_tmux_panes
            .iter()
            .filter(|(id, _)| id.starts_with(&format!("{backend_session}:")))
            .cloned()
            .collect();
        if backend_pids.is_empty() {
            continue;
        }
        if find_ancestor_backend(caller_pid, &parents, &backend_pids).is_some() {
            return Some(*tako_pane_id);
        }
    }
    None
}

/// tmux バックエンドの全ペイン（ID と pane_pid）を列挙する。
/// ID は remote API のペイン ID 形式（`session:window.pane`）と一致させる
pub fn tmux_pane_pids(socket: Option<&str>) -> Vec<(String, u32)> {
    let output = tako_core::tmux::tmux_command(socket)
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_name}:#{window_index}.#{pane_index} #{pane_pid}",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (id, pid) = line.rsplit_once(' ')?;
            Some((id.to_string(), pid.trim().parse().ok()?))
        })
        .collect()
}

/// 特定 tmux セッション配下で動いている claude エージェントの session_id を解決する。
/// `backend_session` は tako の tmux バックエンドセッション名（例: `tako-s3`）。
/// そのセッション内の pane_pid から `claude agents --json` の pid を祖先辿りでマッチする
pub fn resolve_session_id_for_backend(backend_session: &str) -> Option<String> {
    let socket = tako_core::tmux_backend::socket_name();
    let panes = tmux_pane_pids(Some(&socket));
    let target_panes: Vec<_> = panes
        .into_iter()
        .filter(|(id, _)| id.starts_with(&format!("{backend_session}:")))
        .collect();
    if target_panes.is_empty() {
        return None;
    }

    let agents = list_agents().ok()?;
    if agents.is_empty() {
        return None;
    }

    let parents = process_parent_map();
    let pane_by_pid: HashMap<u32, &str> = target_panes
        .iter()
        .map(|(id, pid)| (*pid, id.as_str()))
        .collect();

    for agent in &agents {
        let Some(pid) = agent["pid"].as_u64().map(|p| p as u32) else {
            continue;
        };
        if find_ancestor_pane(pid, &parents, &pane_by_pid).is_some() {
            return agent["session_id"].as_str().map(|s| s.to_string());
        }
    }
    None
}

/// バックエンドセッションで**今まさに動いている** claude エージェント（live 解決）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveClaudeSession {
    /// claude の session_id（transcript 参照キー）
    pub session_id: String,
    /// 対話型 TUI か（`claude agents --json` の kind == "interactive"。
    /// `claude -p` 等の一時セッションと区別し、ペインの agent 種別判定に使う）
    pub interactive: bool,
}

/// 全バックエンドセッションの live claude セッションを一括解決する
/// （tmux list-panes / ps / claude agents を各 1 回だけ実行。#439）。
/// 返り値: バックエンドセッション名 → LiveClaudeSession。
/// pid 祖先辿りで実プロセスの存在を確認するため、role やカタログの stale 記録に
/// 依存しない ground truth になる
pub fn live_claude_sessions_by_backend() -> HashMap<String, LiveClaudeSession> {
    use std::sync::Mutex;
    // 最後に live 解決できた backend → セッションの記憶（#466）。
    // `claude agents --json` は一時失敗・実行中エージェントの列挙漏れが現実に起きる
    // （run_claude_agents_json は TTL 5 秒で失敗もキャッシュする）。素通しで空を返すと
    // 呼び出し側（remote の v2 panes）が sessions カタログの stale な旧世代セッションへ
    // フォールバックし、リモートのチャットビューが凍結した古い transcript を
    // 読み続ける。失敗・欠落時は直近の成功結果で補い、ペインごと消えた backend
    // だけを忘れる
    static STICKY: Mutex<Option<HashMap<String, LiveClaudeSession>>> = Mutex::new(None);

    let socket = tako_core::tmux_backend::socket_name();
    let panes = tmux_pane_pids(Some(&socket));
    if panes.is_empty() {
        return HashMap::new();
    }
    let fresh = match list_agents() {
        Ok(agents) if !agents.is_empty() => {
            let parents = process_parent_map();
            Some(live_sessions_inner(&panes, &agents, &parents))
        }
        _ => None,
    };
    let mut sticky = STICKY.lock().unwrap_or_else(|e| e.into_inner());
    let merged = merge_live_sticky(sticky.take().unwrap_or_default(), &panes, fresh);
    *sticky = Some(merged.clone());
    merged
}

/// sticky 記憶の更新（live_claude_sessions_by_backend のテスト可能な純関数部。#466）。
/// - `fresh` = Some（agents 取得成功）: 検出された backend は上書き。検出されなかった
///   backend もペインが生きていれば記憶を保持する（agents 列挙漏れへの耐性）
/// - `fresh` = None（agents 実行失敗）: ペインが生きている記憶をそのまま使う
/// - どちらの場合も、tmux ペインごと消えた backend の記憶は破棄する
fn merge_live_sticky(
    mut sticky: HashMap<String, LiveClaudeSession>,
    panes: &[(String, u32)],
    fresh: Option<HashMap<String, LiveClaudeSession>>,
) -> HashMap<String, LiveClaudeSession> {
    let alive: HashSet<&str> = panes
        .iter()
        .filter_map(|(id, _)| id.split(':').next())
        .collect();
    sticky.retain(|backend, _| alive.contains(backend.as_str()));
    if let Some(fresh) = fresh {
        for (backend, session) in fresh {
            sticky.insert(backend, session);
        }
    }
    sticky
}

/// 一括解決の内部ロジック（テスト可能な純関数部）。
/// 各 agent の pid 祖先がどれかの pane_pid に到達したら、そのペインの
/// セッション名（`session:w.p` の `:` より前）へ対応付ける
fn live_sessions_inner(
    panes: &[(String, u32)],
    agents: &[Value],
    parents: &HashMap<u32, u32>,
) -> HashMap<String, LiveClaudeSession> {
    let pane_by_pid: HashMap<u32, &str> =
        panes.iter().map(|(id, pid)| (*pid, id.as_str())).collect();
    let mut map = HashMap::new();
    for agent in agents {
        let Some(pid) = agent["pid"].as_u64().map(|p| p as u32) else {
            continue;
        };
        let Some(session_id) = agent["session_id"].as_str().filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(pane_id) = find_ancestor_pane(pid, parents, &pane_by_pid) else {
            continue;
        };
        let Some(backend) = pane_id.split(':').next().filter(|s| !s.is_empty()) else {
            continue;
        };
        let interactive = agent["kind"].as_str() == Some("interactive");
        // 同一セッションに複数 agent が居る場合は interactive を優先して残す
        map.entry(backend.to_string())
            .and_modify(|existing: &mut LiveClaudeSession| {
                if interactive && !existing.interactive {
                    *existing = LiveClaudeSession {
                        session_id: session_id.to_string(),
                        interactive,
                    };
                }
            })
            .or_insert(LiveClaudeSession {
                session_id: session_id.to_string(),
                interactive,
            });
    }
    map
}

/// tmux セッション配下に実行中の子プロセス（tmux クライアント除外）があるか。
/// worker_status の idle 判定を補正するために使う（Issue #224: 偽 IDLE 根治）。
/// `backend_session` は tako の tmux バックエンドセッション名（例: `tako-s3`）。
/// tmux ペインの pane_pid の子孫プロセスを走査し、tmux クライアント自体を除いた
/// 実ユーザープロセスが 1 つでもあれば true を返す
pub fn has_running_children(backend_session: &str) -> bool {
    let socket = tako_core::tmux_backend::socket_name();
    let panes = tmux_pane_pids(Some(&socket));
    let target_pids: Vec<u32> = panes
        .into_iter()
        .filter(|(id, _)| id.starts_with(&format!("{backend_session}:")))
        .map(|(_, pid)| pid)
        .collect();
    if target_pids.is_empty() {
        return false;
    }

    let parents = process_parent_map();
    // pane_pid の子孫を収集（pane_pid 自体 = シェルは除外）
    let pane_set: std::collections::HashSet<u32> = target_pids.iter().copied().collect();
    let mut child_count = 0u32;
    for (&pid, &ppid) in &parents {
        if pane_set.contains(&pid) {
            continue; // pane_pid 自体（シェル）は除外
        }
        // ppid が pane_pid のいずれか、または pane_pid の子孫か
        if is_descendant_of(ppid, &pane_set, &parents) {
            child_count += 1;
        }
    }
    // tmux クライアント分を差し引く: tmux セッション 1 つにつき attach 中の
    // クライアントが 1 つ居る（tako の backend ペイン構造）。ただしクライアントは
    // pane_pid の直接の子ではなくセッションに attach しているだけなので、
    // 上記の子孫走査ではカウントされない。安全のため 0 超で true を返す
    child_count > 0
}

/// バックエンドセッション群のうち実行中の子プロセスを持つものの数を返す。
/// tmux list-panes + ps を各 1 回だけ実行し、全セッションをバッチ判定する。
/// sleep_guard の busy_agents カウントで persist 復元後の worker を拾うために使う（#324）
pub fn count_sessions_with_running_children(sessions: &[&str]) -> usize {
    if sessions.is_empty() {
        return 0;
    }
    let socket = tako_core::tmux_backend::socket_name();
    let panes = tmux_pane_pids(Some(&socket));
    if panes.is_empty() {
        return 0;
    }
    let parents = process_parent_map();
    count_sessions_with_children_inner(sessions, &panes, &parents)
}

/// バッチ判定の内部ロジック（テスト可能）
fn count_sessions_with_children_inner(
    sessions: &[&str],
    panes: &[(String, u32)],
    parents: &HashMap<u32, u32>,
) -> usize {
    sessions
        .iter()
        .filter(|session| {
            let target_pids: Vec<u32> = panes
                .iter()
                .filter(|(id, _)| id.starts_with(&format!("{session}:")))
                .map(|(_, pid)| *pid)
                .collect();
            if target_pids.is_empty() {
                return false;
            }
            let pane_set: std::collections::HashSet<u32> = target_pids.into_iter().collect();
            parents.iter().any(|(&pid, &ppid)| {
                !pane_set.contains(&pid) && is_descendant_of(ppid, &pane_set, parents)
            })
        })
        .count()
}

/// pid が target_pids のいずれかの子孫（自身を含む）かどうか
fn is_descendant_of(
    pid: u32,
    target_pids: &std::collections::HashSet<u32>,
    parents: &HashMap<u32, u32>,
) -> bool {
    let mut current = pid;
    for _ in 0..MAX_ANCESTOR_HOPS {
        if target_pids.contains(&current) {
            return true;
        }
        match parents.get(&current) {
            Some(&ppid) if ppid != 0 && ppid != current => current = ppid,
            _ => break,
        }
    }
    false
}

/// エージェント一覧に tmux ペイン対応を付与した完全版を返す（remote / dispatch / CLI 共用）。
/// `socket` 省略時は tako バックエンドソケットを使う
pub fn list_agents_with_panes(socket: Option<&str>) -> Result<Value, String> {
    let mut agents = list_agents()?;
    let backend;
    let socket = match socket {
        Some(s) => s,
        None => {
            backend = tako_core::tmux_backend::socket_name();
            &backend
        }
    };
    let panes = tmux_pane_pids(Some(socket));
    if !panes.is_empty() {
        let parents = process_parent_map();
        attach_pane_ids(&mut agents, &panes, &parents);
    }
    Ok(json!({ "agents": agents }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agentの正規化() {
        let raw = json!({
            "pid": 123,
            "cwd": "/Users/me/proj",
            "kind": "interactive",
            "startedAt": 1782717863815u64,
            "sessionId": "abc-def",
            "name": "proj-85",
            "status": "idle",
        });
        let agent = normalize_agent(&raw);
        assert_eq!(agent["session_id"], "abc-def");
        assert_eq!(agent["status"], "idle");
        assert_eq!(agent["pid"], 123);
        assert_eq!(agent["cwd"], "/Users/me/proj");
        // 元 JSON に無いフィールドは null（キー自体は存在する）
        assert!(agent["ctx_percent"].is_null());
        assert!(agent["model"].is_null());
    }

    #[test]
    fn 祖先辿りでペインを対応付ける() {
        // ペインのシェル(100) → 中間(200) → claude(300) のプロセスチェーン
        let parents: HashMap<u32, u32> = [(300, 200), (200, 100), (100, 1)].into();
        let panes = vec![("sess:0.0".to_string(), 100u32)];
        let mut agents = vec![
            json!({ "session_id": "a", "pid": 300 }),
            json!({ "session_id": "b", "pid": 999 }), // どのペインにも属さない
            json!({ "session_id": "c" }),             // pid なし
        ];
        attach_pane_ids(&mut agents, &panes, &parents);
        assert_eq!(agents[0]["pane"], "sess:0.0");
        assert!(agents[1]["pane"].is_null());
        assert!(agents[2]["pane"].is_null());
    }

    #[test]
    fn find_ancestor_backend_pane_pid_ancestor() {
        let parents: HashMap<u32, u32> = [(300, 200), (200, 100), (100, 1)].into();
        let backend_pids = vec![("tako-s1".to_string(), 100u32)];
        assert_eq!(
            find_ancestor_backend(300, &parents, &backend_pids),
            Some("tako-s1".to_string())
        );
    }

    #[test]
    fn find_ancestor_backend_unrelated_pid_none() {
        let parents: HashMap<u32, u32> = [(999, 500), (500, 1)].into();
        let backend_pids = vec![("tako-s1".to_string(), 100u32)];
        assert_eq!(find_ancestor_backend(999, &parents, &backend_pids), None);
    }

    #[test]
    fn ancestor_traversal_stops_on_cycle() {
        let parents: HashMap<u32, u32> = [(10, 20), (20, 10)].into();
        let pane_by_pid = HashMap::new();
        assert_eq!(find_ancestor_pane(10, &parents, &pane_by_pid), None);
    }

    #[test]
    fn ps出力のパース() {
        let map = parse_parent_map("  1     0\n  345   1\n 9999 345\nbad line\n");
        assert_eq!(map.get(&345), Some(&1));
        assert_eq!(map.get(&9999), Some(&345));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn process_parent_mapは自プロセスを含む() {
        let map = process_parent_map();
        assert!(map.contains_key(&std::process::id()));
    }

    #[test]
    fn count_sessions_with_children_innerで子プロセスありをカウント() {
        // セッション tako-s1 に pane_pid=100、子プロセス 200→100
        let panes = vec![
            ("tako-s1:0.0".to_string(), 100u32),
            ("tako-s2:0.0".to_string(), 300u32),
        ];
        let parents: HashMap<u32, u32> = [
            (100, 1),
            (200, 100), // 100 の子 = claude 等
            (300, 1),
            // 300 には子なし
        ]
        .into();
        assert_eq!(
            count_sessions_with_children_inner(&["tako-s1", "tako-s2"], &panes, &parents),
            1 // tako-s1 だけ
        );
    }

    #[test]
    fn count_sessions_with_children_innerで空入力は0() {
        let panes = vec![("tako-s1:0.0".to_string(), 100u32)];
        let parents: HashMap<u32, u32> = [(100, 1), (200, 100)].into();
        assert_eq!(count_sessions_with_children_inner(&[], &panes, &parents), 0);
    }

    #[test]
    fn count_sessions_with_children_innerで複数セッションの子プロセスをカウント() {
        let panes = vec![
            ("tako-s1:0.0".to_string(), 100u32),
            ("tako-s2:0.0".to_string(), 300u32),
        ];
        let parents: HashMap<u32, u32> = [
            (100, 1),
            (200, 100), // s1 の子
            (300, 1),
            (400, 300), // s2 の子
        ]
        .into();
        assert_eq!(
            count_sessions_with_children_inner(&["tako-s1", "tako-s2"], &panes, &parents),
            2
        );
    }

    #[test]
    fn count_sessions_with_children_innerで存在しないセッションは無視() {
        let panes = vec![("tako-s1:0.0".to_string(), 100u32)];
        let parents: HashMap<u32, u32> = [(100, 1)].into();
        assert_eq!(
            count_sessions_with_children_inner(&["tako-nonexist"], &panes, &parents),
            0
        );
    }

    // --- #439: live claude セッションの一括解決 ---

    #[test]
    fn live_sessions_innerはpid祖先でバックエンドへ対応付ける() {
        // tako-s1 の pane_pid=100 → claude(300)、tako-s2 の pane_pid=500 → claude -p(600)
        let panes = vec![
            ("tako-s1:0.0".to_string(), 100u32),
            ("tako-s2:0.0".to_string(), 500u32),
        ];
        let parents: HashMap<u32, u32> =
            [(300, 200), (200, 100), (100, 1), (600, 500), (500, 1)].into();
        let agents = vec![
            json!({ "session_id": "sid-interactive", "pid": 300, "kind": "interactive" }),
            json!({ "session_id": "sid-headless", "pid": 600, "kind": "headless" }),
            json!({ "session_id": "sid-orphan", "pid": 999, "kind": "interactive" }),
            json!({ "session_id": "", "pid": 300 }), // 空 ID は無視
        ];
        let map = live_sessions_inner(&panes, &agents, &parents);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map["tako-s1"],
            LiveClaudeSession {
                session_id: "sid-interactive".into(),
                interactive: true
            }
        );
        assert_eq!(
            map["tako-s2"],
            LiveClaudeSession {
                session_id: "sid-headless".into(),
                interactive: false
            }
        );
    }

    #[test]
    fn live_sessions_innerは同一セッションでinteractiveを優先する() {
        // 同じペイン配下に headless（claude -p 子プロセス）と interactive が同居した場合
        let panes = vec![("tako-s1:0.0".to_string(), 100u32)];
        let parents: HashMap<u32, u32> = [(300, 100), (400, 100), (100, 1)].into();
        let agents_headless_first = vec![
            json!({ "session_id": "sid-p", "pid": 300, "kind": "headless" }),
            json!({ "session_id": "sid-tui", "pid": 400, "kind": "interactive" }),
        ];
        let map = live_sessions_inner(&panes, &agents_headless_first, &parents);
        assert_eq!(map["tako-s1"].session_id, "sid-tui");
        assert!(map["tako-s1"].interactive);

        // 逆順でも interactive が残る
        let agents_tui_first = vec![
            json!({ "session_id": "sid-tui", "pid": 400, "kind": "interactive" }),
            json!({ "session_id": "sid-p", "pid": 300, "kind": "headless" }),
        ];
        let map = live_sessions_inner(&panes, &agents_tui_first, &parents);
        assert_eq!(map["tako-s1"].session_id, "sid-tui");
    }

    fn live(sid: &str) -> LiveClaudeSession {
        LiveClaudeSession {
            session_id: sid.into(),
            interactive: true,
        }
    }

    #[test]
    fn merge_live_stickyはagents失敗時に生存ペインの記憶を返す() {
        // #466: agents --json の一時失敗で空を返すと呼び出し側が stale カタログへ
        // フォールバックする。生存ペインの直近 live 解決を保持する
        let sticky: HashMap<String, LiveClaudeSession> =
            [("tako-a".to_string(), live("sid-a"))].into();
        let panes = vec![("tako-a:0.0".to_string(), 100u32)];
        let merged = merge_live_sticky(sticky, &panes, None);
        assert_eq!(merged["tako-a"].session_id, "sid-a");
    }

    #[test]
    fn merge_live_stickyは成功時に検出backendを上書きする() {
        // /clear 等で session が変わったら新しい値へ追従する
        let sticky: HashMap<String, LiveClaudeSession> =
            [("tako-a".to_string(), live("sid-old"))].into();
        let panes = vec![("tako-a:0.0".to_string(), 100u32)];
        let fresh: HashMap<String, LiveClaudeSession> =
            [("tako-a".to_string(), live("sid-new"))].into();
        let merged = merge_live_sticky(sticky, &panes, Some(fresh));
        assert_eq!(merged["tako-a"].session_id, "sid-new");
    }

    #[test]
    fn merge_live_stickyは列挙漏れbackendの記憶を保持する() {
        // agents --json が実行中エージェントを取りこぼしても（実測で発生）、
        // ペインが生きている限り直近の解決を使い続ける
        let sticky: HashMap<String, LiveClaudeSession> =
            [("tako-a".to_string(), live("sid-a"))].into();
        let panes = vec![
            ("tako-a:0.0".to_string(), 100u32),
            ("tako-b:0.0".to_string(), 200u32),
        ];
        let fresh: HashMap<String, LiveClaudeSession> =
            [("tako-b".to_string(), live("sid-b"))].into();
        let merged = merge_live_sticky(sticky, &panes, Some(fresh));
        assert_eq!(merged["tako-a"].session_id, "sid-a");
        assert_eq!(merged["tako-b"].session_id, "sid-b");
    }

    #[test]
    fn merge_live_stickyはペイン消滅backendの記憶を破棄する() {
        // ペインごと閉じた backend の記憶を持ち続けない（誤った claude 判定の防止）
        let sticky: HashMap<String, LiveClaudeSession> =
            [("tako-gone".to_string(), live("sid-gone"))].into();
        let panes = vec![("tako-a:0.0".to_string(), 100u32)];
        let merged = merge_live_sticky(sticky, &panes, None);
        assert!(merged.is_empty());
    }
}
