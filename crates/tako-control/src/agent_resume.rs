//! agent_resume — codex / agy の会話 ID をペインへ紐づける（Issue #1238）
//!
//! 規則（保持・コマンドの骨格・可否）は [`tako_core::agent_resume`] が正本で、
//! ここは**実環境から材料を採る**層。claude の ID は `claude agents --json` が
//! 直接くれるが、codex / agy にはその口が無いので**生きたプロセスが開いている
//! ロックファイル**から引く。
//!
//! # 実測（2026-09-09 / codex-cli 0.153.0 / agy 1.1.27）
//!
//! | 系統 | 開いたまま持つファイル | 現れる時期 |
//! |---|---|---|
//! | codex | `$CODEX_HOME/thread-writer-locks/<thread_id>.lock` | **起動直後**（プロンプト前） |
//! | agy | `~/.gemini/antigravity-cli/presence/<conversation_id>.lock` | **最初のターンの後**（会話は遅延生成） |
//!
//! **ロックファイルは終了後もディスクに残る**（実測: 8 月のものが 20 件以上残っていた）。
//! だから「在る」ではなく「**生きた pid が開いている**」を根拠にする。
//! これは #984 が codex の状態検知で使っている経路と同じ考え方で、
//! `lsof` の読み取りもここへ 1 実装にまとめてある。
//!
//! # なぜ sticky にしないか
//!
//! `codex_session` の解決（#984 / #985）は 2 秒 tick のステータスバーから呼ばれるので
//! sticky（1 ペイン 1 回）が要る。こちらは **30 秒間隔の復元用スキャン**からしか
//! 呼ばれず、しかも「会話が途中で切り替わる」（codex の `/new`・agy の新規会話）と
//! **古い会話を復元してしまう**方が高くつく。毎回引き直す（1 回 40〜70ms × agent ペイン数）。

use std::collections::HashMap;

use tako_core::agent_support::Agent;

use crate::agents::ProcessSnapshot;

/// codex の会話ロックの置き場（`$CODEX_HOME` 配下）
const CODEX_LOCK_MARKER: &str = "thread-writer-locks/";
/// agy の会話ロックの置き場（`~/.gemini/antigravity-cli` 配下）
const AGY_LOCK_MARKER: &str = "presence/";

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

/// `lsof -Fn` の出力から `<marker><id>.lock` の `<id>` を拾う（**純粋関数**）。
///
/// 行は `n<パス>` の形。`.coordination.lock` のような ID でないものを弾くため
/// **ハイフンを含むこと**（UUID）まで確かめる。書式検証は
/// [`crate::codex_session::is_valid_thread_id`]（パストラバーサル防止と同じ規則）
pub fn lock_id_from_lsof(out: &str, marker: &str) -> Option<String> {
    for line in out.lines() {
        let path = line.strip_prefix('n').unwrap_or(line);
        let Some(rest) = path.split(marker).nth(1) else {
            continue;
        };
        let id = rest.strip_suffix(".lock").unwrap_or(rest);
        if crate::codex_session::is_valid_thread_id(id) && id.contains('-') {
            return Some(id.to_string());
        }
    }
    None
}

/// その pid が開いているロックから会話 ID を得る。
///
/// **`lsof` は POSIX の道具で Windows には無い**ので、Windows では常に `None` になる
/// （この事実は `tako_core::agent_resume::restore_support` が宣言していて、
/// 復元の内訳ログは `ID なし` ではなく `resume 非対応` と出す）
pub fn lock_id_for_pid(pid: u32, marker: &str) -> Option<String> {
    // GUI から到達する経路なのでコンソール窓の抑止を通す（#628 / #586）
    let mut cmd = std::process::Command::new("lsof");
    cmd.args(["-p", &pid.to_string(), "-Fn"]);
    tako_core::platform::process::no_console_window(&mut cmd);
    let out = cmd.output().ok()?;
    lock_id_from_lsof(&String::from_utf8_lossy(&out.stdout), marker)
}

/// agy のデータディレクトリ（`~/.gemini/antigravity-cli`）。
/// codex の `CODEX_HOME` に相当する環境変数は agy に無い（`agy --help` 実測 1.1.27）ので、
/// **HOME からの固定パス**（`orchestrator::agent` の事前信頼が書く先と同じ流儀）
pub fn agy_home() -> Option<std::path::PathBuf> {
    tako_core::paths::home_dir().map(|h| h.join(".gemini/antigravity-cli"))
}

/// agy の会話 1 件の実体（SQLite）。**存在確認にだけ使う**（中身は読まない = 依存を増やさない）
pub fn agy_conversation_db(conversation_id: &str) -> Option<std::path::PathBuf> {
    if !crate::codex_session::is_valid_thread_id(conversation_id) {
        return None;
    }
    Some(
        agy_home()?
            .join("conversations")
            .join(format!("{conversation_id}.db")),
    )
}

/// コマンド行が agy CLI か（**実行ファイル名の位置で見る**。
/// [`crate::codex_session::is_codex_command`] と同じ規則）
pub fn is_agy_command(cmd: &str) -> bool {
    cmd.split_whitespace().next().is_some_and(|prog| {
        let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
        base == "agy" || base == "agy.exe"
    })
}

/// コマンド行から系統を判定する（**純粋関数**。claude はここでは扱わない =
/// `claude agents --json` の経路が持つ）
pub fn agent_of_command(cmd: &str) -> Option<Agent> {
    if crate::codex_session::is_codex_command(cmd) {
        Some(Agent::Codex)
    } else if is_agy_command(cmd) {
        Some(Agent::Agy)
    } else {
        None
    }
}

/// ロックの置き場（系統ごと）
fn lock_marker(agent: Agent) -> Option<&'static str> {
    match agent {
        Agent::Codex => Some(CODEX_LOCK_MARKER),
        Agent::Agy => Some(AGY_LOCK_MARKER),
        _ => None,
    }
}

/// ペイン配下で動いている codex / agy を探す（**純粋関数**。`lsof` は起こさない）。
///
/// 候補が複数あれば**いちばん新しいもの**（pid の大きいもの）を採る
/// （`codex_session::resolve_thread_ids_with` と同じ選び方）
pub fn agent_pid_in(probe: &PaneProbe, snap: &ProcessSnapshot) -> Option<(Agent, u32)> {
    let mut pids: Vec<u32> = match (&probe.backend, probe.pty_pid) {
        (Some(backend), _) => snap.descendant_pids(backend),
        (None, Some(pid)) => snap.descendants_with_root(pid),
        (None, None) => return None,
    };
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
        let id = lock_marker(agent).and_then(|marker| lock_id_for_pid(pid, marker));
        out.insert(probe.pane, (agent, id));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **実採取の形**（2026-09-09 / codex-cli 0.153.0）。パスはプレースホルダ化（#927）
    const CODEX_LSOF: &str = "\
p12809
n/Users/testuser/.codex/thread-writer-locks/01a08347-543b-7a20-8e49-a93380efb375.lock
n/Users/testuser/.codex/log/codex-tui.log
";

    /// **実採取の形**（2026-09-09 / agy 1.1.27）。conversations の .db も同じ ID で開く
    const AGY_LSOF: &str = "\
n/Users/testuser/.gemini/antigravity-cli/log/cli-20260909_080446.log
n/Users/testuser/.gemini/antigravity-cli/conversations/874efd16-af95-4b2e-8036-84f6c3eaded3.db
n/Users/testuser/.gemini/antigravity-cli/presence/874efd16-af95-4b2e-8036-84f6c3eaded3.lock
";

    #[test]
    fn 実採取のlsofから会話idを拾う() {
        assert_eq!(
            lock_id_from_lsof(CODEX_LSOF, CODEX_LOCK_MARKER).as_deref(),
            Some("01a08347-543b-7a20-8e49-a93380efb375")
        );
        assert_eq!(
            lock_id_from_lsof(AGY_LSOF, AGY_LOCK_MARKER).as_deref(),
            Some("874efd16-af95-4b2e-8036-84f6c3eaded3")
        );
    }

    /// 会話でないロック（codex の調整用）を ID と取り違えない
    #[test]
    fn 会話でないロックは拾わない() {
        let out = "n/Users/testuser/.codex/thread-writer-locks/.coordination.lock\n";
        assert_eq!(lock_id_from_lsof(out, CODEX_LOCK_MARKER), None);
    }

    /// 置き場が違えば拾わない（agy の出力から codex の ID が出ない）
    #[test]
    fn 置き場が違えば拾わない() {
        assert_eq!(lock_id_from_lsof(AGY_LSOF, CODEX_LOCK_MARKER), None);
        assert_eq!(lock_id_from_lsof(CODEX_LSOF, AGY_LOCK_MARKER), None);
    }

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
        assert_eq!(agent_of_command("claude --resume abc"), None);
    }
}
