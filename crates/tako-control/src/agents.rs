//! agents — `claude agents --json` のプロキシと tmux ペイン対応付け
//!
//! スマホリモート（Issue #23）の「エージェント一覧」用。`claude agents --json` の
//! 出力（pid / cwd / sessionId / status / name 等）を正規化し、tmux ペインの
//! `pane_pid` からプロセス祖先を辿って「どのペインで動いているか」を対応付ける。

use std::collections::HashMap;

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
}
