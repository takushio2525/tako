//! agent_resume — codex / agy の会話 ID をペインへ紐づける（Issue #1238）
//!
//! 規則（保持・コマンドの骨格・可否）は [`tako_core::agent_resume`] が正本で、
//! ここは**実環境から材料を採る**層。claude の ID は `claude agents --json` が
//! 直接くれるが、codex / agy にはその口が無いので**生きたプロセスが開いているもの**
//! （ロックファイル / 会話ディレクトリ）から `lsof` 越しに引く。
//!
//! # ID を引く実装はここに持たない
//!
//! 系統ごとの所在は**その系統のモジュールが正本**で、ここは 2 つを束ねるだけ:
//!
//! | 系統 | 生きた pid が開いているもの | 正本 |
//! |---|---|---|
//! | codex | `$CODEX_HOME/thread-writer-locks/<id>.lock` | [`crate::codex_session`]（#984 / #985） |
//! | agy | `~/.gemini/antigravity-cli/brain/<id>` | [`crate::agy_session`]（#1033） |
//!
//! どちらも**プロセス終了後もディスクに残る**ので、「在る」ではなく
//! 「**生きた pid が開いている**」を根拠にする（実測 2026-09-09: codex-cli 0.153.0 は
//! 起動直後から / agy 1.1.27 は会話が生まれる最初のターンの後から）。
//!
//! # なぜ sticky にしないか
//!
//! 状態検知側の解決（#984 / #1033）は 2 秒 tick のステータスバーから呼ばれるので
//! sticky（1 ペイン 1 回）が要る。こちらは **30 秒間隔の復元用スキャン**からしか
//! 呼ばれず、しかも「会話が途中で切り替わる」（codex の `/new`・agy の新規会話）と
//! **古い会話を復元してしまう**方が高くつく。毎回引き直す（1 回 40〜70ms × agent ペイン数）。

use std::collections::HashMap;

use tako_core::agent_support::Agent;

use crate::agents::ProcessSnapshot;

/// #1238 の A/B。`TAKO_1238_LEGACY=1` で修正前の挙動
/// （codex / agy の会話 ID を一切採らない = 復元は claude だけ）へ戻す
pub fn legacy_1238() -> bool {
    std::env::var_os("TAKO_1238_LEGACY").is_some()
}

/// 1 ペインぶんの探索の材料。
///
/// 器（tmux バックエンド）を持つペインと持たないペインの両方を扱う（#728 と同じ二段構え）。
/// 器があればそのセッション配下の子孫、無ければ PTY 直下の子から辿る
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneProbe {
    pub pane: u64,
    pub backend: Option<String>,
    pub pty_pid: Option<u32>,
}

/// コマンド行から系統を判定する（**純粋関数**。claude はここでは扱わない =
/// `claude agents --json` の経路が持つ）。
/// 判定そのものは各系統のモジュールが持つ 1 実装を呼ぶ
pub fn agent_of_command(cmd: &str) -> Option<Agent> {
    if crate::codex_session::is_codex_command(cmd) {
        Some(Agent::Codex)
    } else if crate::agy_session::is_agy_command(cmd) {
        Some(Agent::Agy)
    } else {
        None
    }
}

/// 生きた pid から会話 ID を引く（**`lsof` を起こす**ので background 専用）。
/// 所在の知識は各系統のモジュールに置いてある
fn conversation_id_for_pid(agent: Agent, pid: u32) -> Option<String> {
    match agent {
        Agent::Codex => crate::codex_session::thread_id_for_pid(pid),
        Agent::Agy => crate::agy_session::conversation_id_for_pid(pid),
        _ => None,
    }
}

/// ペイン配下で動いている codex / agy を探す（**純粋関数**。`lsof` は起こさない）。
///
/// 候補が複数あれば**いちばん新しいもの**（pid の大きいもの）を採る
/// （`codex_session::resolve_thread_ids_with` と同じ選び方）
pub fn agent_pid_in(probe: &PaneProbe, snap: &ProcessSnapshot) -> Option<(Agent, u32)> {
    // **ペインのプロセスそのものも見る**（`descendants_with_root`）。器のペインは
    // ふつうログインシェルでエージェントはその子だが、コマンドを直接起動した
    // ペイン（`tako run` / 器へ直に流し込んだ場合）ではエージェントがペイン
    // プロセス自身になる。`descendant_pids` はペインの pid を除くので、そこだけを
    // 見ると採り逃す（実測で踏んだ）
    let mut pids: Vec<u32> = match (&probe.backend, probe.pty_pid) {
        (Some(backend), _) => snap
            .pane_pids_of(backend)
            .into_iter()
            .flat_map(|pid| snap.descendants_with_root(pid))
            .collect(),
        (None, Some(pid)) => snap.descendants_with_root(pid),
        (None, None) => return None,
    };
    pids.dedup();
    pids.sort_unstable();
    pids.reverse();
    pids.into_iter()
        .find_map(|pid| Some((agent_of_command(snap.argv(pid)?)?, pid)))
}

/// ペインごとの (系統, 会話 ID) を採る。
///
/// **`lsof` を起こすので background executor 専用**。ID が採れなくても
/// 「その系統が動いている」ことは返す（`None` の ID）ので、復元の内訳ログは
/// 「保存されていない」と「そもそも手段が無い」を言い分けられる（#1238 の要件 3）
pub fn detect_resume_ids(
    probes: &[PaneProbe],
    snap: &ProcessSnapshot,
) -> HashMap<u64, (Agent, Option<String>)> {
    if legacy_1238() {
        return HashMap::new();
    }
    let mut out = HashMap::new();
    for probe in probes {
        let Some((agent, pid)) = agent_pid_in(probe, snap) else {
            continue;
        };
        out.insert(probe.pane, (agent, conversation_id_for_pid(agent, pid)));
    }
    out
}

/// 会話そのものがまだ在るか（`None` = この系統はここでは判断しない）。
///
/// 復元の内訳が `会話が見つからない` を言うための判定で、見る先は
/// **`resume` が実際に開くファイル**（codex = rollout / agy = conversations の db）。
/// agy で実況 JSONL（`agy_session::transcript_path`）を見ないのは、
/// **ターンが無いと生まれない**ので「会話は在るがまだ喋っていない」を
/// 「会話が無い」と誤判定するため
pub fn conversation_exists(agent: Agent, id: &str) -> Option<bool> {
    match agent {
        Agent::Codex => Some(crate::codex_session::find_rollout(id).is_some()),
        Agent::Agy => Some(crate::agy_session::conversation_db(id).is_some_and(|p| p.is_file())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 系統の判定は各モジュールの 1 実装を通る（**実行ファイル名の位置で見る**）
    #[test]
    fn 実行ファイル名の位置で系統を見る() {
        assert_eq!(agent_of_command("/usr/local/bin/codex"), Some(Agent::Codex));
        assert_eq!(
            agent_of_command("/Users/testuser/.local/bin/agy --continue"),
            Some(Agent::Agy)
        );
        // 引数に名前が混ざっただけのものは拾わない
        assert_eq!(agent_of_command("vim agy.rs"), None);
        assert_eq!(agent_of_command("tail -f codex.log"), None);
        // claude はこの経路の担当ではない（`claude agents --json` が持つ）
        assert_eq!(agent_of_command("claude --resume abc"), None);
    }

    /// 器のペインの**プロセスそのもの**がエージェントでも見つける（実測で踏んだ穴）。
    ///
    /// ふつうペインはログインシェルでエージェントはその子だが、器へコマンドを
    /// 直接起動した形（`tmux new-session … "codex"`）ではエージェントが
    /// ペインプロセス自身になる。`descendant_pids` はペインの pid を除くので、
    /// そこだけを見ると「動いているのに採れない」になる
    #[test]
    fn ペインプロセス自身がエージェントでも見つける() {
        let snap = ProcessSnapshot::from_parts_for_test(
            vec![("w1:0.0".into(), 100)],
            HashMap::new(),
            HashMap::from([(100u32, "/Users/testuser/.local/bin/agy".to_string())]),
        );
        let probe = PaneProbe {
            pane: 1,
            backend: Some("w1".into()),
            pty_pid: None,
        };
        assert_eq!(agent_pid_in(&probe, &snap), Some((Agent::Agy, 100)));
    }

    /// 子として動いている通常の形（ログインシェル → codex）も従来どおり
    #[test]
    fn ペインの子で動くエージェントも見つける() {
        let snap = ProcessSnapshot::from_parts_for_test(
            vec![("w1:0.0".into(), 100)],
            HashMap::from([(200u32, 100u32)]),
            HashMap::from([
                (100u32, "-zsh".to_string()),
                (200u32, "/usr/local/bin/codex".to_string()),
            ]),
        );
        let probe = PaneProbe {
            pane: 1,
            backend: Some("w1".into()),
            pty_pid: None,
        };
        assert_eq!(agent_pid_in(&probe, &snap), Some((Agent::Codex, 200)));
        // 器を持たないペイン（PTY 直下の子から辿る）も同じ答えになる
        let no_backend = PaneProbe {
            pane: 1,
            backend: None,
            pty_pid: Some(100),
        };
        assert_eq!(agent_pid_in(&no_backend, &snap), Some((Agent::Codex, 200)));
    }

    /// 会話の有無を見る先は「resume が実際に開くファイル」。
    /// 形式不正な ID でパスを組み立てない（パストラバーサル防止）
    #[test]
    fn 会話の有無は系統ごとの置き場を見る() {
        assert_eq!(conversation_exists(Agent::Claude, "abc"), None);
        assert_eq!(conversation_exists(Agent::Local, "abc"), None);
        assert_eq!(conversation_exists(Agent::Codex, "../../etc"), Some(false));
        assert_eq!(conversation_exists(Agent::Agy, "../../etc"), Some(false));
    }
}
