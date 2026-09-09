//! agy_session — agy（Antigravity CLI）の**構造化された状態の出口**（Issue #1033 / エピック #975）
//!
//! ## 何を解決するか
//!
//! #984 で codex は claude と同等になったが、agy だけが `status_source = "screen"` の
//! ままだった。`wait.rs` の `need_streak`（画面推定 = 8 回連続・一次シグナル = 3 回連続）
//! × ポーリング 5 秒がそのまま差になり、北極星実測（#975 / 2026-08-29）で
//! **完了検知が claude の 2.6 倍・+24 秒**（中央値 39.38s 対 15.40s）だった。
//! `report --messages N` も scrollback しか返せない（`messages` が 0 件）。
//!
//! ## 前提の訂正: agy の一次シグナルは SQLite ではない
//!
//! #984 の棚卸しは「agy の会話は SQLite（`conversations/<id>.db`）なので読むには
//! 新しい依存が要る」と記録していた。**agy 1.1.27 を実物で調べ直すとそれは会話の
//! 保管庫にすぎず、実況は平文の JSONL で別に書かれている**:
//!
//! ```text
//! ~/.gemini/antigravity-cli/brain/<conversation-id>/.system_generated/logs/transcript.jsonl
//! ```
//!
//! 1 行 1 ステップで `{step_index, source, type, status, created_at, content?, thinking?,
//! tool_calls?}` の形。**新しい依存は要らない**（serde_json で読める）。
//!
//! | source / type | 意味 | 使い道 |
//! |---|---|---|
//! | `USER_EXPLICIT` / `USER_INPUT` | ユーザーの投入 = ターン開始 | 送達の一次シグナル |
//! | `MODEL` / `PLANNER_RESPONSE` + `tool_calls` | ツール呼び出し | busy |
//! | `MODEL` / `PLANNER_RESPONSE` + `content` かつ `tool_calls` なし | **最終発話** | idle |
//! | `MODEL` / `GENERIC` 等 | ツール結果 | busy（モデルの応答待ち） |
//!
//! ## 実測（2026-09-09 / agy 1.1.27 / 隔離 tmux）
//!
//! **逐次書き込みであることを実測した**（これが成立しないとライブ監視に使えない）。
//! 北極星と同一のタスクを投げ、0.2 秒間隔で「画面に `RESULT: 137` が出た時刻」と
//! 「transcript に終端 `PLANNER_RESPONSE` が書かれた時刻」を採ったところ
//! **同一標本（差 0.00 秒）**だった。ツール実行のたびにステップが 1 行ずつ増える
//! （n=4 → 6 → 10）ことも観測している。
//!
//! ## 終端の判定に効く 2 つの罠（実物のコーパス 28 会話で確認）
//!
//! 1. **`content` があるだけでは完了ではない**。`PLANNER_RESPONSE` が本文とツール
//!    呼び出しを**同居させる行が 58 件**ある（喋りながらツールを呼ぶ）。
//!    完了は「`content` あり **かつ `tool_calls` なし**」でしか言えない
//! 2. **`step_index` はファイルの行順と一致しない**（28 会話中 5 件で非単調。
//!    例 `[0,1,3,4,2,5,…]`）。非同期ツールの結果が後から確定して追記されるため。
//!    終端は**最終行ではなく `step_index` が最大のステップ**で採る
//!
//! ## ペイン → 会話の写像
//!
//! 生きている agy プロセスは**その会話の `brain/<conversation-id>` ディレクトリを
//! 開いたまま持つ**（`lsof` で実測）。codex の `thread-writer-locks` と同じ形なので、
//! ペイン → 子孫 pid → その pid が握るディレクトリ → `conversation_id` で解決できる。
//!
//! `lsof` は 1 回 40〜70ms なので**解決は 1 ペインにつき 1 回だけ**行い、結果は
//! sticky に持つ（`codex_session` と同じ方針。毎ポーリングで叩くと #772 / #779 /
//! #816 で削った subprocess を復活させてしまう）。
//!
//! **会話ディレクトリは最初のプロンプトが入るまで作られない**（実測）ので、
//! 起動直後は解決に失敗して画面推定のままになる。これは codex と同じ振る舞いで、
//! 「まだ何も言えない」を idle と誤認しないための正しい側。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// 状態を判定するために読む transcript の末尾バイト数。
/// 終端ステップは末尾に来るので全文を読む必要が無い（発話本文は別途 tail する）
const STATE_TAIL_BYTES: u64 = 256 * 1024;

/// `lsof` の出力から会話 ID を拾うときの目印。
/// **`antigravity-cli/brain/` まで含めて見る**（`brain` だけでは別物に当たりうる）
const BRAIN_ANCHOR: &str = "antigravity-cli/brain/";

/// agy のホーム（`~/.gemini/antigravity-cli`）。
///
/// agy には `CODEX_HOME` に相当する env が無い（`agy --help` の全オプションを確認）ので
/// 固定パスで解決する。**テストはパスを渡す下位関数を直接叩く**ので env の穴は開けない
pub fn agy_home() -> Option<PathBuf> {
    tako_core::paths::home_dir().map(|h| h.join(".gemini").join("antigravity-cli"))
}

/// 会話 ID の形式検証（UUID 想定）。**パストラバーサル防止**。
/// codex 側 `is_valid_thread_id` と同じ約束
pub fn is_valid_conversation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `lsof` の出力（`-Fn` 形式）から会話 ID を拾う（**純粋関数**）。
///
/// 行は `n<パス>` の形。`antigravity-cli/brain/<id>` の**次の 1 区切り**だけを見る
/// （`brain/<id>/.system_generated/...` のような深い行から同じ ID が取れる）
pub fn conversation_id_from_lsof(out: &str) -> Option<String> {
    for line in out.lines() {
        let path = line.strip_prefix('n').unwrap_or(line);
        let Some(rest) = path.split(BRAIN_ANCHOR).nth(1) else {
            continue;
        };
        let id = rest.split('/').next().unwrap_or(rest);
        // `.system_generated` のような隠しディレクトリ名や空を弾く。
        // 会話 ID は UUID なので `-` を必ず含む
        if is_valid_conversation_id(id) && id.contains('-') {
            return Some(id.to_string());
        }
    }
    None
}

/// 生きている agy プロセスが開いているディレクトリから会話 ID を得る。
/// **1 ペインにつき 1 回だけ呼ぶ**（呼び出し側が sticky に持つ）
pub fn conversation_id_for_pid(pid: u32) -> Option<String> {
    // GUI から到達する経路なのでコンソール窓の抑止を通す（#628 / #586）。
    // なお `lsof` は POSIX の道具で Windows には無い（Windows の agy 対応は
    // 開いているディレクトリを別の手段で引く必要がある）
    let mut cmd = std::process::Command::new("lsof");
    cmd.args(["-p", &pid.to_string(), "-Fn"]);
    tako_core::platform::process::no_console_window(&mut cmd);
    let out = cmd.output().ok()?;
    conversation_id_from_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// 会話の実況 JSONL の置き場。**存在しなければ `None`**（= まだターンが無い）
pub fn transcript_path(conversation_id: &str) -> Option<PathBuf> {
    if !is_valid_conversation_id(conversation_id) {
        return None;
    }
    let p = agy_home()?
        .join("brain")
        .join(conversation_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl");
    p.is_file().then_some(p)
}

/// agy のターン状態（claude の `agents --json` / codex の rollout に相当するもの）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnState {
    /// ターンが進行中か（終端の最終発話がまだ書かれていない）
    pub busy: bool,
    /// 直近のターンで返した本文（終端 `PLANNER_RESPONSE` の `content`）
    pub last_agent_message: Option<String>,
    /// 観測できたステップ数（0 なら「まだ 1 ターンも走っていない」）
    pub events: usize,
    /// ユーザーの投入（`USER_INPUT`）を 1 件でも観測したか
    pub user_input_seen: bool,
    /// **モデルが 1 歩でも動いたか**（`source = MODEL` のステップを 1 件でも観測した）。
    ///
    /// #1034 のゲートが使う。送達は成立したのに agent 側の理由で実行が始まらない
    /// （アカウント検証待ち等）と、`USER_INPUT` はあっても `MODEL` のステップが
    /// 1 件も書かれない。**画面推定の busy は TUI の起動描画を拾う**ので使えない
    pub model_step_seen: bool,
}

impl TurnState {
    /// dispatch の語彙（`normalize_agent_status` と同じ値）へ写す。
    /// **ステップが 1 つも無いときは `None`** を返し、呼び出し側に
    /// 「構造化ソースでは何も言えない」ことを伝える
    /// （ここで idle と言い切ると、起動直後のプロンプト投入前を完了と誤認する）
    pub fn status(&self) -> Option<&'static str> {
        if self.events == 0 {
            return None;
        }
        Some(if self.busy { "busy" } else { "idle" })
    }

    /// **プロンプトが届いた証拠があるか**（#983 の変更 2 / #1015 と同じ約束）。
    ///
    /// `USER_INPUT`（`source = USER_EXPLICIT`）は**投入されたプロンプトでしか
    /// 書かれない**ので、1 件でも観測できれば「入力が届いて agy が受け取った」と
    /// 言い切れる。これは画面の送達確認（#32 / #640）より強い証拠
    pub fn prompt_arrived(&self) -> bool {
        self.user_input_seen
    }

    /// **agent が実際に作業を始めた証拠があるか**（#1034）。
    ///
    /// `MODEL` のステップ（思考・ツール呼び出し・発話）が 1 件でも書かれていれば、
    /// その worker は仕事に入っている。1 件も無ければ「起動も送達も成立したのに
    /// 1 文字も進んでいない」状態で、画面に拒否の文言があれば分類してよい
    pub fn agent_work_started(&self) -> bool {
        self.model_step_seen
    }
}

/// 1 ステップ（JSONL の 1 行）から判定に使う項目だけを取り出した形（**内部用**）
struct Step {
    step_index: i64,
    /// ファイルの何行目に現れたか（`step_index` が同値のときの決着用）
    line_no: usize,
    source: String,
    kind: String,
    status: String,
    content: String,
    has_tool_calls: bool,
}

impl Step {
    /// このステップが**ターンの終端**（モデルの最終発話）か。
    ///
    /// 3 条件すべてが要る:
    /// - `MODEL` の `PLANNER_RESPONSE`（ツール結果の `GENERIC` は本文を持つが終端ではない）
    /// - `content` が空でない（`thinking` や `tool_calls` だけの行は途中）
    /// - **`tool_calls` を持たない**（喋りながらツールを呼ぶ行が実物に 58 件ある）
    fn is_turn_end(&self) -> bool {
        self.source == "MODEL"
            && self.kind == "PLANNER_RESPONSE"
            && self.status == "DONE"
            && !self.content.trim().is_empty()
            && !self.has_tool_calls
    }
}

/// JSONL の 1 行を [`Step`] へ（壊れた行・途中まで書かれた行は `None`）
fn parse_step(line: &str, line_no: usize) -> Option<Step> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // 書き込み途中の行（末尾が切れている）は読み飛ばす。
    // 逐次追記されるファイルを読むので必ず起こりうる
    let v: Value = serde_json::from_str(line).ok()?;
    Some(Step {
        step_index: v.get("step_index").and_then(Value::as_i64).unwrap_or(-1),
        line_no,
        source: v
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: v
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: v
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        content: v
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        has_tool_calls: v
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty()),
    })
}

/// JSONL の行群からターン状態を組み立てる（**純粋関数**。fixture でテストできる）。
///
/// 判定は「**`step_index` が最大のステップ**が終端の最終発話かどうか」。
/// 最終行で見ないのは `step_index` がファイル順と一致しないため（実測。上の罠 2）
pub fn parse_turn_state(lines: &[&str]) -> TurnState {
    let steps: Vec<Step> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_step(l, i))
        .collect();
    let mut st = TurnState {
        events: steps.len(),
        ..Default::default()
    };
    if steps.is_empty() {
        return st;
    }
    st.user_input_seen = steps
        .iter()
        .any(|s| s.source == "USER_EXPLICIT" && s.kind == "USER_INPUT");
    st.model_step_seen = steps.iter().any(|s| s.source == "MODEL");
    // `step_index` 最大 → 同値ならファイルで後にある方（追記順が新しい）
    let last = steps
        .iter()
        .max_by_key(|s| (s.step_index, s.line_no))
        .expect("空でないことを上で確認済み");
    if last.is_turn_end() {
        st.busy = false;
        st.last_agent_message = Some(last.content.trim().to_string());
    } else {
        // 終端が見えないあいだは busy。**判定不能を idle 側へ倒さない**
        // （偽 idle は master が「完了した」と誤読する = #1034 と同じ実害）
        st.busy = true;
        // 途中でも、直近の最終発話は報告に使えるので拾っておく
        st.last_agent_message = steps
            .iter()
            .filter(|s| s.is_turn_end())
            .max_by_key(|s| (s.step_index, s.line_no))
            .map(|s| s.content.trim().to_string());
    }
    st
}

/// JSONL の行群から agent の発話本文を古い順に取り出す（**純粋関数**）。
///
/// transcript アダプタ（`report --messages N`）が使う。claude 側の
/// `transcript::last_assistant_texts` と**同じ「古い順で最大 N 件」の約束**にそろえる。
///
/// 拾うのは `MODEL` の `PLANNER_RESPONSE` で `content` が空でないもの
/// （**ツール呼び出しを同居させた行も発話には違いない**ので含める）。
/// ツール結果の `GENERIC` / `RUN_COMMAND` は agent の発話ではないので落とす
pub fn parse_agent_texts(lines: &[&str], limit: usize) -> Vec<String> {
    let mut steps: Vec<Step> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| parse_step(l, i))
        .filter(|s| {
            s.source == "MODEL" && s.kind == "PLANNER_RESPONSE" && !s.content.trim().is_empty()
        })
        .collect();
    steps.sort_by_key(|s| (s.step_index, s.line_no));
    let mut out: Vec<String> = steps
        .into_iter()
        .map(|s| s.content.trim().to_string())
        .collect();
    if out.len() > limit {
        out.drain(..out.len() - limit);
    }
    out
}

/// ファイル末尾から最大 `max` バイトを読む（**逐次追記されるファイルを安く読む**）。
///
/// 先頭が行の途中で切れることがあるので最初の改行までは捨てる。ただし
/// **1 ステップが `max` を超えると全部捨ててしまう**ので、そのときは全文へ戻す
/// （agy のツール結果は数百 KB になりうる。実物で 4 KB 級を観測）
fn read_tail(path: &Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(max);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let s = String::from_utf8_lossy(&buf).into_owned();
    if from == 0 {
        return Some(s);
    }
    let cut = s
        .split_once('\n')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    if cut.trim().is_empty() {
        // 末尾 max バイトが 1 行に満たない = 巨大なステップ。全文を読む
        return std::fs::read_to_string(path).ok();
    }
    Some(cut)
}

/// 会話 ID のターン状態を読む（`None` = transcript がまだ無い = ターン未実行）
pub fn read_turn_state(conversation_id: &str) -> Option<TurnState> {
    let path = transcript_path(conversation_id)?;
    let text = read_tail(&path, STATE_TAIL_BYTES)?;
    let lines: Vec<&str> = text.lines().collect();
    Some(parse_turn_state(&lines))
}

/// 会話 ID の直近 N 件の agent 発話（transcript アダプタの入口）
pub fn last_agent_texts(conversation_id: &str, limit: usize) -> Result<Vec<String>, String> {
    let path = transcript_path(conversation_id)
        .ok_or_else(|| format!("agy の会話ログが見つからない: {conversation_id}"))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("agy の会話ログを読めない {}: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    Ok(parse_agent_texts(&lines, limit.max(1)))
}

/// コマンド行が agy CLI か。**`agy` を含むだけでは採らない**
/// （引数に混ざった文字列を拾わないよう実行ファイル名の位置で見る）
pub fn is_agy_command(cmd: &str) -> bool {
    cmd.split_whitespace().next().is_some_and(|prog| {
        let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
        base == "agy" || base == "agy.exe"
    })
}

fn is_descendant(mut pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    for _ in 0..64 {
        if pid == ancestor {
            return true;
        }
        match parents.get(&pid) {
            Some(&p) if p != pid => pid = p,
            _ => return false,
        }
    }
    false
}

/// backend セッション（= ペイン）配下の agy プロセスの pid を探す（**純粋関数**）。
///
/// 材料は既存の pid 祖先辿りと同じ（親子表 + コマンド表 + ペインの pid）。
/// 「コマンド名が agy の子孫」を新しい方（= pid の大きい方）から選ぶ
pub fn find_agy_pid(
    pane_pid: u32,
    parents: &HashMap<u32, u32>,
    commands: &HashMap<u32, String>,
) -> Option<u32> {
    let mut hits: Vec<u32> = commands
        .iter()
        .filter(|(_, cmd)| is_agy_command(cmd))
        .map(|(pid, _)| *pid)
        .filter(|pid| is_descendant(*pid, pane_pid, parents))
        .collect();
    hits.sort_unstable();
    hits.pop()
}

/// ペイン（backend セッション）→ 会話 ID の解決。**sticky**（codex と同じ方針）。
///
/// `TAKO_1033_LEGACY=1` で構造化ソースを無効化し、画面推定のみ（旧挙動）へ戻せる
pub fn resolve_conversation_id_for_backend(backend_session: &str) -> Option<String> {
    if legacy_screen_only() {
        return None;
    }
    let panes = crate::agents::backend_pane_pids();
    // 消えたペインの記憶は捨てる（pane ID 再利用で別 worker の会話を返さない）
    sticky_forget_gone(&panes);
    if let Some(id) = sticky_lookup(backend_session) {
        return Some(id);
    }
    let pane_pid = crate::codex_session::pane_pid_of(&panes, backend_session);
    let resolved = pane_pid.and_then(|pane_pid| {
        let (parents, commands) = crate::agents::capture_process_table();
        let agy_pid = find_agy_pid(pane_pid, &parents, &commands)?;
        conversation_id_for_pid(agy_pid)
    });
    if let Some(ref id) = resolved {
        sticky_insert(backend_session, id);
    }
    resolved
}

/// 解決済み backend → 会話 ID の記憶（sticky）
fn sticky() -> &'static std::sync::Mutex<HashMap<String, String>> {
    static STICKY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> =
        std::sync::OnceLock::new();
    STICKY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn sticky_lookup(backend_session: &str) -> Option<String> {
    let map = sticky().lock().unwrap_or_else(|e| e.into_inner());
    map.get(backend_session).cloned()
}

fn sticky_insert(backend_session: &str, conversation_id: &str) {
    let mut map = sticky().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(backend_session.to_string(), conversation_id.to_string());
}

/// 消えたペインの記憶を捨てる
fn sticky_forget_gone(panes: &[(String, u32)]) {
    let mut map = sticky().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|b, _| crate::codex_session::session_has_pane(panes, b));
}

/// 構造化ソースを使わず画面推定だけに戻す逃げ道（#1033 の A/B）
pub fn legacy_screen_only() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_1033_LEGACY").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 実採取の行（2026-09-09 / agy 1.1.27）。#927 に従い
    //     ユーザー名・ホームパスはプレースホルダへ置換してある ---

    /// ユーザーの投入（ターン開始）。実採取の形
    const USER_INPUT: &str = r#"{"step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE", "created_at": "2026-09-08T22:05:11Z", "content": "<USER_REQUEST>\n作業フォルダにある sample.txt の行数を数えてください。\n</USER_REQUEST>"}"#;

    /// ツールを呼ぶだけの行（`thinking` はあるが `content` は無い）
    const PLANNER_TOOL: &str = r#"{"step_index": 1, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-09-08T22:05:11Z", "thinking": "**Confirming Instructions**", "tool_calls": [{"name": "view_file", "args": {"AbsolutePath": "\"/Users/testuser/proj/sample.txt\""}}]}"#;

    /// ツール結果（`GENERIC`）。**`content` を持つが終端ではない**
    const TOOL_RESULT: &str = r#"{"step_index": 2, "source": "MODEL", "type": "GENERIC", "status": "DONE", "created_at": "2026-09-08T22:05:14Z", "content": "Created At: 2026-09-09T07:05:14+09:00\nCompleted At: 2026-09-09T07:05:14+09:00\nTotal Lines: 138"}"#;

    /// **喋りながらツールを呼ぶ行**（実物のコーパスに 58 件。ここを終端と読むと偽 idle）
    const PLANNER_TALK_AND_TOOL: &str = r#"{"step_index": 3, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-09-08T22:05:14Z", "content": "念のため wc でも数えます。", "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"wc -l sample.txt\""}}]}"#;

    /// 終端の最終発話（`content` あり・`tool_calls` なし）
    const PLANNER_FINAL: &str = r#"{"step_index": 4, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "created_at": "2026-09-08T22:09:06Z", "content": "sample.txt の行数を確認しました。\n\nRESULT: 137"}"#;

    /// 実行中のツール（`status: RUNNING`）。実物のコーパスで観測
    const TOOL_RUNNING: &str = r#"{"step_index": 5, "source": "MODEL", "type": "RUN_COMMAND", "status": "RUNNING", "created_at": "2026-09-08T22:09:10Z", "content": "Created At: 2026-09-09T07:09:10+09:00"}"#;

    #[test]
    fn issue1033_終端の最終発話でidleになる() {
        let st = parse_turn_state(&[USER_INPUT, PLANNER_TOOL, TOOL_RESULT, PLANNER_FINAL]);
        assert_eq!(st.status(), Some("idle"));
        assert!(!st.busy);
        assert!(st.prompt_arrived());
        assert_eq!(st.events, 4);
        assert!(st
            .last_agent_message
            .as_deref()
            .unwrap()
            .contains("RESULT: 137"));
    }

    #[test]
    fn issue1033_喋りながらツールを呼ぶ行を完了と読まない() {
        // 罠 1: `content` があるだけで完了と読むと、ここで偽 idle になる
        let st = parse_turn_state(&[USER_INPUT, PLANNER_TALK_AND_TOOL]);
        assert_eq!(st.status(), Some("busy"), "本文 + tool_calls は途中");
    }

    #[test]
    fn issue1033_ツール結果の行を完了と読まない() {
        let st = parse_turn_state(&[USER_INPUT, PLANNER_TOOL, TOOL_RESULT]);
        assert_eq!(
            st.status(),
            Some("busy"),
            "ツール結果は agent の最終発話ではない"
        );
    }

    #[test]
    fn issue1033_実行中のツールがあればbusy() {
        let st = parse_turn_state(&[USER_INPUT, PLANNER_FINAL, TOOL_RUNNING]);
        assert_eq!(
            st.status(),
            Some("busy"),
            "終端より後のステップがあれば途中"
        );
    }

    #[test]
    fn issue1033_step_indexが行順と一致しなくても終端で決める() {
        // 罠 2: 実物の 5/28 会話で `step_index` は非単調。
        // **最終行**（step 2 のツール結果）で判定すると busy を返してしまう
        let st = parse_turn_state(&[USER_INPUT, PLANNER_TOOL, PLANNER_FINAL, TOOL_RESULT]);
        assert_eq!(st.status(), Some("idle"), "最大 step_index が終端なら idle");
    }

    #[test]
    fn issue1033_ステップがゼロなら何も言わない() {
        let st = parse_turn_state(&[]);
        assert_eq!(st.status(), None, "起動直後を完了と誤認しない");
        assert!(!st.prompt_arrived());
    }

    #[test]
    fn issue1033_投入前はプロンプト到達を主張しない() {
        // USER_INPUT が無い（= 会話が別経路で作られた）ときは送達を主張しない
        let st = parse_turn_state(&[PLANNER_TOOL]);
        assert!(!st.prompt_arrived());
        assert_eq!(st.status(), Some("busy"));
    }

    #[test]
    fn issue1033_壊れた行を読み飛ばす() {
        let broken = r#"{"step_index": 9, "source": "MODEL", "type": "PLANN"#;
        let st = parse_turn_state(&[USER_INPUT, PLANNER_FINAL, broken]);
        assert_eq!(st.status(), Some("idle"), "書き込み途中の行で壊れない");
        assert_eq!(st.events, 2);
    }

    #[test]
    fn issue1033_発話だけを古い順に件数ぶん返す() {
        let lines = [
            USER_INPUT,
            PLANNER_TOOL,
            TOOL_RESULT,
            PLANNER_TALK_AND_TOOL,
            PLANNER_FINAL,
        ];
        let all = parse_agent_texts(&lines, 10);
        assert_eq!(
            all.len(),
            2,
            "本文を持つ PLANNER_RESPONSE だけ（ツール結果は除く）"
        );
        assert!(all[0].contains("念のため"));
        assert!(all[1].contains("RESULT: 137"));
        // 直近 1 件
        let one = parse_agent_texts(&lines, 1);
        assert_eq!(one.len(), 1);
        assert!(one[0].contains("RESULT: 137"), "最新が残る");
    }

    #[test]
    fn issue1033_発話はstep_index順に並べ直す() {
        // 行順が入れ替わっていても会話の順で返す
        let out = parse_agent_texts(&[PLANNER_FINAL, PLANNER_TALK_AND_TOOL], 10);
        assert!(out[0].contains("念のため"), "step 3 が先");
        assert!(out[1].contains("RESULT: 137"), "step 4 が後");
    }

    #[test]
    fn issue1033_lsofから会話idを拾う() {
        // 実採取の形（#927 でホームパスはプレースホルダ）
        let out = "n/Users/testuser/.gemini/antigravity-cli/log/cli-20260909_070115.log\n\
                   n/Users/testuser/.gemini/config/projects\n\
                   n/Users/testuser/.gemini/antigravity-cli/brain/95b181d1-274e-4bf7-8b04-13f2d4a6bea1\n";
        assert_eq!(
            conversation_id_from_lsof(out).as_deref(),
            Some("95b181d1-274e-4bf7-8b04-13f2d4a6bea1")
        );
    }

    #[test]
    fn issue1033_lsofの深い行からも同じidが取れる() {
        let out = "n/Users/testuser/.gemini/antigravity-cli/brain/95b181d1-274e-4bf7-8b04-13f2d4a6bea1/.system_generated/logs/transcript.jsonl\n";
        assert_eq!(
            conversation_id_from_lsof(out).as_deref(),
            Some("95b181d1-274e-4bf7-8b04-13f2d4a6bea1")
        );
    }

    #[test]
    fn issue1033_会話が無ければ何も返さない() {
        let out = "n/Users/testuser/.gemini/antigravity-cli/log/cli-20260909_070115.log\n\
                   n/Users/testuser/.gemini/antigravity-cli/knowledge/knowledge.lock\n";
        assert_eq!(conversation_id_from_lsof(out), None);
    }

    #[test]
    fn issue1033_uuidでないディレクトリ名を拾わない() {
        let out = "n/Users/testuser/.gemini/antigravity-cli/brain/.tmp\n\
                   n/Users/testuser/other/brain/abc\n";
        assert_eq!(
            conversation_id_from_lsof(out),
            None,
            "`-` を含まない名前は会話 ID ではない"
        );
    }

    #[test]
    fn issue1033_agyコマンドの判定は実行ファイル名で見る() {
        assert!(is_agy_command(
            "agy --model gemini-3.7-flash-high --effort high"
        ));
        assert!(is_agy_command("/Users/testuser/.local/bin/agy"));
        assert!(is_agy_command("agy.exe --effort high"));
        assert!(!is_agy_command("tako orchestrator spawn --agent agy"));
        assert!(!is_agy_command("agyx --model x"));
        assert!(!is_agy_command(""));
    }

    #[test]
    fn issue1033_会話idの形式検証がパストラバーサルを弾く() {
        assert!(is_valid_conversation_id(
            "95b181d1-274e-4bf7-8b04-13f2d4a6bea1"
        ));
        assert!(!is_valid_conversation_id("../../etc/passwd"));
        assert!(!is_valid_conversation_id("a/b"));
        assert!(!is_valid_conversation_id(""));
        assert_eq!(transcript_path("../../etc"), None);
    }

    #[test]
    fn issue1033_子孫のagyだけを拾う() {
        let mut parents = HashMap::new();
        let mut commands = HashMap::new();
        // ペイン 100 → シェル 200 → agy 300
        parents.insert(200u32, 100u32);
        parents.insert(300u32, 200u32);
        // 別ペイン配下の agy 900 は拾わない
        parents.insert(900u32, 800u32);
        commands.insert(300u32, "agy --effort high".to_string());
        commands.insert(900u32, "agy --effort high".to_string());
        commands.insert(200u32, "-zsh".to_string());
        assert_eq!(find_agy_pid(100, &parents, &commands), Some(300));
        assert_eq!(find_agy_pid(700, &parents, &commands), None);
    }
}
