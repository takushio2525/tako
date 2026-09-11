//! 番犬: sleep guard の busy 判定から「器を持たないペイン」を落とさない（#372）
//!
//! ## なぜ止めるのか
//!
//! #372 は 2 段で直っている。1 段目（PR #389）は「器の中の子プロセスを取りこぼす」形で、
//! 2 段目がこれ = **そもそも器が無い構成**（tmux 未導入 / persist OFF = Homebrew cask の
//! 既定）で走査対象が常に空になり、`busy_agents` が無条件に 0 だった。
//! 既定モードが `while-agents-running` なので、配布先の既定構成で機能がまるごと死ぬ。
//!
//! 単体テストは「いまの判定結果」を固定するが、**同じ形の再発**はソースの形でしか
//! 止まらない。落ち方は 4 通りあり、どれも 1 行で戻せてしまう:
//!
//! 1. [`走査対象を器のセッションだけに戻せない`] — `tako-app` の収集部が
//!    `backend_sessions` の列挙へ戻る（= 器なしペインが対象から消える）
//! 2. [`器なしペインのpidを渡している`] — 対象に入れても `child_pid` を渡さなければ
//!    起点が無い = 永久に busy 0（本番では 1 ビットも変わらない）
//! 3. [`busy_agentsはbusy_countを通す`] — `busy_sessions.len()` を直に渡す形へ戻る
//!    （器なしペインぶんが落ちる）
//! 4. [`cliのstatusはアプリの値を採る`] — `assertion_held` / `busy_agents` は
//!    プロセスローカルな static なので、CLI が自前計算だけに戻ると
//!    「アプリが保持しているのに未保持・busy 0」という #372 と同じ嘘に戻る

use std::path::{Path, PathBuf};

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

const APP: &str = "crates/tako-app/src/main.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// 指定した関数の本文（シグネチャ行から次の同インデント `}` まで）を
/// **行番号つき**で切り出す。コメント行は落とす（説明文の引用に当たらないため）
fn fn_body(rel: &str, signature: &str) -> Vec<(usize, String)> {
    let source = read(rel);
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{rel} に `{signature}` が見つからない（改名したら番犬も直す）"));
    let head_line = source[..start].lines().count();
    let indent = " ".repeat(signature.len() - signature.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    source[start..end]
        .lines()
        .enumerate()
        .map(|(i, l)| (head_line + i, l.to_string()))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 空白を全部落とした本文。rustfmt が `self\n    .terminals` のように折るので、
/// 式の形を見る検査は改行に依存させない
fn compact(body: &[(usize, String)]) -> String {
    joined(body)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// 1: 収集部は全ペインを列挙する（`backend_sessions` の列挙へ戻さない）
#[test]
fn 走査対象を器のセッションだけに戻せない() {
    let body = fn_body(APP, "    fn running_children_scan_targets(");
    let code = compact(&body);
    assert!(
        code.contains("self.terminals.iter()"),
        "{APP}: running_children_scan_targets が `self.terminals`（= 全ペイン）を\n\
         列挙していない。器を持たないペインが対象から消えると、tmux 未導入 /\n\
         persist OFF の構成で busy_agents が無条件に 0 に戻る（#372）"
    );
    if code.contains("self.backend_sessions.iter()") {
        let at: Vec<String> = body
            .iter()
            .filter(|(_, l)| l.contains("backend_sessions"))
            .map(|(n, l)| format!("{APP}:{n}: {}", l.trim()))
            .collect();
        panic!(
            "{APP}: 走査対象を `backend_sessions` の列挙で作っている（#372 の根因）。\n\
             全ペイン（`self.terminals`）を列挙し、器の有無で辿り方を分けること:\n  {}",
            at.join("\n  ")
        );
    }
}

/// 2: 器なしペインには PTY 直下の子 pid を渡す（渡さなければ起点が無い）
#[test]
fn 器なしペインのpidを渡している() {
    let code = compact(&fn_body(APP, "    fn running_children_scan_targets("));
    assert!(
        code.contains("child_pid") && code.contains("session.child_pid()"),
        "{APP}: 走査対象へ `child_pid: session.child_pid()` を渡していない。\n\
         器なしペインは PTY 直下の子が唯一の起点なので、渡さないと判定側を\n\
         直しても本番では 1 ビットも変わらない（#372）"
    );
    // 判定側が両方の起点を見ていること（片方だけだと片方の構成で永久に 0）
    let scan = joined(&fn_body(AGENTS, "pub fn scan_running_children_in("));
    assert!(
        scan.contains("pane_pids_of") && scan.contains("target.child_pid"),
        "{AGENTS}: scan_running_children_in が器あり（pane_pids_of）/ \n\
         器なし（child_pid）の二段構えになっていない（#372）"
    );
    assert!(
        scan.contains("has_running_descendants"),
        "{AGENTS}: 子孫判定が共有の 1 実装（has_running_descendants）を通っていない（#372）"
    );
}

/// 3: sleep guard へ渡す busy は `busy_count()`（器あり + 器なしの合算）
#[test]
fn busy_agentsはbusy_countを通す() {
    let body = fn_body(APP, "    fn apply_sleep_guard(");
    let text = joined(&body);
    assert!(
        text.contains("busy_count()"),
        "{APP}: apply_sleep_guard が `busy_count()` を通していない。\n\
         器なしペインぶんが落ちて #372 が再発する"
    );
    let bad: Vec<String> = body
        .iter()
        .filter(|(_, l)| l.contains("busy_sessions.len()"))
        .map(|(n, l)| format!("{APP}:{n}: {}", l.trim()))
        .collect();
    assert!(
        bad.is_empty(),
        "{APP}: busy の数え上げに `busy_sessions.len()` を直に使っている（#372 の根因）。\n\
         数え方の正本は RunningChildrenScanState::busy_count():\n  {}",
        bad.join("\n  ")
    );
}

/// 4: CLI の status はアプリのプロセスが持つ実行時の値を採る
#[test]
fn cliのstatusはアプリの値を採る() {
    let body = fn_body(CLI, "fn sleep_guard_status_state(");
    let text = joined(&body);
    assert!(
        text.contains("Request::SleepGuard") && text.contains("from_json"),
        "{CLI}: sleep_guard_status_state が IPC（Request::SleepGuard + from_json）で\n\
         アプリの状態を採っていない。assertion_held / busy_agents はどちらも\n\
         プロセスローカルな static なので、自前計算だけでは常に「未保持 / busy 0」に\n\
         なる（#372 で実測）。MCP の tako_sleep_guard と同じ dispatch を通すこと"
    );
    assert!(
        text.contains("sleep_guard::status("),
        "{CLI}: アプリ未起動時のフォールバック（ローカル計算）が消えている。\n\
         `tako sleep-guard status` は GUI 無しでも引けることが要件（#372）"
    );
    // status アームが自前計算へ戻っていないこと
    let status_arm = joined(&fn_body(CLI, "fn sleep_guard_local("));
    assert!(
        status_arm.contains("sleep_guard_status_state()"),
        "{CLI}: status アームが sleep_guard_status_state() を通っていない（#372）"
    );
}
