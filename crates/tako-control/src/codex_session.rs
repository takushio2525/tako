//! codex_session — codex の**構造化された状態の出口**（Issue #984 / エピック #975）
//!
//! ## 何を解決するか
//!
//! codex / agy の worker は「完了検知が遅い / 誤爆する」と体感されていた。構造は
//! `wait.rs` の `need_streak`（画面推定 = 8 回連続・claude の構造化ソース = 3 回連続）で、
//! **claude だけが一次シグナル（`claude agents --json`）を持っていた**のが理由。
//!
//! 棚卸し（#975 §2）は「codex に status API は無い → 画面推定」と記録していたが、
//! それは **codex 0.144.1 時点**の調査だった。0.150.1 を実物で調べると
//! **セッションの実況が JSONL で残っており、しかも逐次書き込まれている**。
//!
//! ## 実測（2026-08-27 / codex-cli 0.150.1）
//!
//! 置き場は `$CODEX_HOME/sessions/<YYYY>/<MM>/<DD>/rollout-<ts>-<thread_id>.jsonl`
//! （`CODEX_HOME` 既定 `~/.codex`）。1 行 1 イベントで
//! `{timestamp, ordinal, type, payload}` の形。使うのは 3 種類:
//!
//! | payload.type | 意味 | 使い道 |
//! |---|---|---|
//! | `task_started` | ターン開始（`turn_id` / `model_context_window`） | busy |
//! | `task_complete` | ターン完了（`turn_id` / `last_agent_message` / `duration_ms`） | idle |
//! | `token_count` | `info.last_token_usage.total_tokens` / `info.model_context_window` | ctx% |
//!
//! `type: "response_item"` の `payload.role == "assistant"` が発話本文 = **transcript**
//! （`orchestrator report --messages N` の第 2 層。`dispatch.rs` の拡張点へ載せる）。
//!
//! **逐次書き込みであることを実測した**（これが成立しないとライブ監視に使えない）:
//! 250 語の生成を投げて 1 秒ごとに観測したところ
//! `t=1s task_started` → `t=14〜26s response_item/reasoning が増える` →
//! **`t=27s task_complete`**。ターン中の busy → idle 遷移がそのまま見える。
//!
//! ## ペイン → セッションの写像
//!
//! codex は rollout 本体を**開いたままにしない**（追記して閉じる）ので `lsof` で
//! rollout を捕まえることはできない。代わりに**生きている codex プロセスは
//! `$CODEX_HOME/thread-writer-locks/<thread_id>.lock` を開いたまま持つ**（実測）。
//! ペイン → 子孫 pid（既存の pid 祖先辿り）→ その pid が握るロック → `thread_id`。
//!
//! `lsof` は 1 回 40〜70ms なので**解決は 1 ペインにつき 1 回だけ**行い、
//! 結果は sticky に持つ（`agents.rs` の #466 と同じ方針。毎ポーリングで叩くと
//! #772 / #779 / #816 で削った subprocess を復活させてしまう）。
//!
//! ## agy はここに乗らない
//!
//! agy の会話は `~/.gemini/antigravity-cli/conversations/<id>.db`（**SQLite**）で、
//! 読むには新しい依存が要る。presence（`presence/<id>.lock`）で生存は分かるが
//! ターンの開始・完了は取れない。よって agy は画面推定のままで、
//! 精度は**弱マーカーの agent 別分離**（`wait.rs`）で上げる。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// 状態を判定するために読む rollout の末尾バイト数。
/// ターンの開始・完了は末尾に来るので全文を読む必要が無い（発話本文は別途 tail する）
const STATE_TAIL_BYTES: u64 = 256 * 1024;

/// `CODEX_HOME`（既定 `~/.codex`）。
/// **環境変数を先に見る**のは codex 自身がそうしているため（`--help` の記述）
pub fn codex_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("CODEX_HOME") {
        let p = PathBuf::from(v);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    tako_core::paths::home_dir().map(|h| h.join(".codex"))
}

/// codex のターン状態（claude の `agents --json` に相当するもの）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnState {
    /// ターンが進行中か（最後の `task_started` が最後の `task_complete` より後）
    pub busy: bool,
    /// 直近のターンで返した本文（`task_complete.last_agent_message`）
    pub last_agent_message: Option<String>,
    /// コンテキスト使用率（0–100）。`last_token_usage / model_context_window`
    pub ctx_percent: Option<u32>,
    /// 観測できたイベント数（0 なら「まだ 1 ターンも走っていない」= 起動直後）
    pub events: usize,
    /// 直近の `token_count` に載っていたレート制限（#985）。
    /// **画面スクレイピングではなく構造化データ**なので、プランによる表示揺れが無い
    pub rate_limits: Option<RateLimits>,
}

/// レート制限の 1 枠（codex の `rate_limits.primary` / `.secondary`）。
///
/// **実採取（2026-08-27 / codex-cli 0.150.1 / plan_type = plus）**:
/// `primary` = `window_minutes: 300`（5 時間枠）、`secondary` = `10080`（週枠）。
/// tako のステータスバーが持つ 5h / 7d の 2 枠とそのまま対応する
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RateWindow {
    /// 使用率（0–100 に丸めた整数。元は小数）
    pub used_percent: u32,
    /// 枠の長さ（分）。300 = 5 時間 / 10080 = 週
    pub window_minutes: u64,
    /// 枠が空くまでの時刻（**unix 秒**）。文言パースと違い日付ごと確定している
    pub resets_at: Option<i64>,
}

impl RateWindow {
    /// この枠を使い切っているか。**閾値は 100 ちょうど**（99% は「まだ動ける」）
    pub fn exhausted(&self) -> bool {
        self.used_percent >= 100
    }
}

/// codex の `token_count` イベントに載るレート制限のスナップショット。
///
/// フィールド名は codex 0.150.1 の `RateLimitSnapshot`（バイナリ内の構造体名から確認）に
/// そろえてある。**知らないフィールドは読まない**ので、上流が足しても壊れない
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RateLimits {
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
    /// プラン種別（実採取: `"plus"`）。無料プランでも構造は同じ
    pub plan_type: Option<String>,
    /// **どの枠で上限に当たったか**（`rate_limit_reached_type`。上限前は null）。
    /// 値は HTTP ヘッダ `x-codex-rate-limit-reached-type` 由来
    pub reached: Option<String>,
}

impl RateLimits {
    /// 上限に当たっているか。上流の申告（`reached`）を優先し、無ければ使用率で見る
    pub fn limited(&self) -> bool {
        self.reached.is_some()
            || self.primary.is_some_and(|w| w.exhausted())
            || self.secondary.is_some_and(|w| w.exhausted())
    }

    /// **止まっている枠が空く時刻**（unix 秒）。
    ///
    /// 上流が `reached` でどの枠かを言っていればその枠、言っていなければ
    /// 「使い切っている枠のうち最も早く空くもの」。どちらも決まらなければ `None`
    /// （= まだ上限ではない。ここで primary を返すと「上限でもないのに待つ」になる）
    pub fn reset_at(&self) -> Option<i64> {
        if let Some(w) = self.reached_window() {
            return w.resets_at;
        }
        [self.primary, self.secondary]
            .into_iter()
            .flatten()
            .filter(|w| w.exhausted())
            .filter_map(|w| w.resets_at)
            .min()
    }

    /// `reached` が名指ししている枠（`"primary"` / `"secondary"` の前方一致で見る）
    fn reached_window(&self) -> Option<RateWindow> {
        let kind = self.reached.as_deref()?.to_ascii_lowercase();
        if kind.contains("secondary") {
            self.secondary
        } else if kind.contains("primary") {
            self.primary
        } else {
            None
        }
    }
}

/// `rate_limits` オブジェクトを型へ落とす（**純粋関数**）。
/// 枠が 1 つも読めなければ `None`（= レート制限の情報が無い、と扱う）
fn parse_rate_limits(v: &Value) -> Option<RateLimits> {
    let window = |key: &str| -> Option<RateWindow> {
        let w = v.get(key)?;
        // `used_percent` は小数（実採取: 4.0）。**四捨五入せず切り捨てる**のは
        // 99.7% を 100（= 使い切り）と言わないため
        let used = w.get("used_percent")?.as_f64()?;
        Some(RateWindow {
            used_percent: used.clamp(0.0, 100.0) as u32,
            window_minutes: w.get("window_minutes").and_then(Value::as_u64).unwrap_or(0),
            resets_at: w
                .get("resets_at")
                .and_then(Value::as_i64)
                .filter(|t| *t > 0),
        })
    };
    let primary = window("primary");
    let secondary = window("secondary");
    if primary.is_none() && secondary.is_none() {
        return None;
    }
    Some(RateLimits {
        primary,
        secondary,
        plan_type: v
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        reached: v
            .get("rate_limit_reached_type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

impl TurnState {
    /// dispatch の語彙（`normalize_agent_status` と同じ値）へ写す。
    /// **イベントが 1 つも無い（起動直後でまだターンが無い）ときは `None`** を返し、
    /// 呼び出し側に「構造化ソースでは何も言えない」ことを伝える
    /// （ここで idle と言い切ると、起動直後のプロンプト投入前を完了と誤認する）
    pub fn status(&self) -> Option<&'static str> {
        if self.events == 0 {
            return None;
        }
        Some(if self.busy { "busy" } else { "idle" })
    }
}

/// rollout の 1 行から `payload.type` を取り出す（`type` が `event_msg` のときだけ）
fn event_kind(v: &Value) -> Option<&str> {
    if v.get("type")?.as_str()? != "event_msg" {
        return None;
    }
    v.get("payload")?.get("type")?.as_str()
}

/// JSONL の行群からターン状態を組み立てる（**純粋関数**。fixture でテストできる）。
///
/// 判定は「最後の `task_started` と最後の `task_complete` のどちらが後か」。
/// `turn_id` を突き合わせないのは、次のターンが始まった直後に前のターンの
/// `task_complete` が遅れて書かれる形を実測していないため
/// （順序だけで足り、`ordinal` は単調増加であることも実測済み）
pub fn parse_turn_state(lines: &[&str]) -> TurnState {
    let mut st = TurnState::default();
    let mut last_started: Option<u64> = None;
    let mut last_complete: Option<u64> = None;
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            // 書き込み途中の行（末尾が切れている）は読み飛ばす。
            // 逐次追記されるファイルを読むので必ず起こりうる
            continue;
        };
        let ordinal = v.get("ordinal").and_then(Value::as_u64).unwrap_or(0);
        match event_kind(&v) {
            Some("task_started") => {
                st.events += 1;
                last_started = Some(ordinal);
            }
            Some("task_complete") => {
                st.events += 1;
                last_complete = Some(ordinal);
                if let Some(msg) = v["payload"]["last_agent_message"].as_str() {
                    if !msg.trim().is_empty() {
                        st.last_agent_message = Some(msg.to_string());
                    }
                }
            }
            Some("token_count") => {
                // #985: 同じイベントにレート制限が載っている。**最後の 1 件が最新**
                if let Some(rl) = parse_rate_limits(&v["payload"]["rate_limits"]) {
                    st.rate_limits = Some(rl);
                }
                let info = &v["payload"]["info"];
                let used = info["last_token_usage"]["total_tokens"]
                    .as_u64()
                    .or_else(|| info["total_token_usage"]["total_tokens"].as_u64());
                let window = info["model_context_window"].as_u64();
                if let (Some(used), Some(window)) = (used, window) {
                    if let Some(pct) = used.saturating_mul(100).checked_div(window) {
                        st.ctx_percent = Some(pct.min(100) as u32);
                    }
                }
            }
            _ => {}
        }
    }
    st.busy = match (last_started, last_complete) {
        (Some(s), Some(c)) => s > c,
        (Some(_), None) => true,
        _ => false,
    };
    st
}

/// JSONL の行群から assistant の発話本文を古い順に取り出す（**純粋関数**）。
///
/// transcript アダプタ（`report --messages N`）が使う。claude 側の
/// `transcript::last_assistant_texts` と**同じ「古い順で最大 N 件」の約束**にそろえる
pub fn parse_assistant_texts(lines: &[&str], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let p = &v["payload"];
        if p.get("type").and_then(Value::as_str) != Some("message")
            || p.get("role").and_then(Value::as_str) != Some("assistant")
        {
            continue;
        }
        let mut text = String::new();
        for item in p["content"].as_array().into_iter().flatten() {
            if let Some(t) = item["text"].as_str() {
                text.push_str(t);
            }
        }
        let text = text.trim().to_string();
        if !text.is_empty() {
            out.push(text);
        }
    }
    if out.len() > limit {
        out.drain(..out.len() - limit);
    }
    out
}

/// `thread_id` の形式検証（UUID 想定）。**パストラバーサル防止**。
/// claude 側 `transcript::is_valid_session_id` と同じ約束
pub fn is_valid_thread_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `lsof` の出力（`-Fn` 形式）から thread_id を拾う（**純粋関数**）。
///
/// 行は `n<パス>` の形。`thread-writer-locks/<id>.lock` だけを見る
/// （`.coordination.lock` のような id でないものは弾く）
pub fn thread_id_from_lsof(out: &str) -> Option<String> {
    for line in out.lines() {
        let path = line.strip_prefix('n').unwrap_or(line);
        let Some(rest) = path.split("thread-writer-locks/").nth(1) else {
            continue;
        };
        let id = rest.strip_suffix(".lock").unwrap_or(rest);
        if is_valid_thread_id(id) && id.contains('-') {
            return Some(id.to_string());
        }
    }
    None
}

/// 生きている codex プロセスが握るロックから thread_id を得る。
/// **1 ペインにつき 1 回だけ呼ぶ**（呼び出し側が sticky に持つ）
pub fn thread_id_for_pid(pid: u32) -> Option<String> {
    // GUI から到達する経路なのでコンソール窓の抑止を通す（#628 / #586）。
    // なお `lsof` は POSIX の道具で Windows には無い（Windows の codex 対応は
    // ロックの持ち主を別の手段で引く必要がある）
    let mut cmd = std::process::Command::new("lsof");
    cmd.args(["-p", &pid.to_string(), "-Fn"]);
    tako_core::platform::process::no_console_window(&mut cmd);
    let out = cmd.output().ok()?;
    thread_id_from_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// thread_id の rollout ファイルを探す。
///
/// 名前は `rollout-<ts>-<thread_id>.jsonl` なので**ファイル名の接尾辞**で判定する
/// （中身を開かずに絞れる）。日付ディレクトリを新しい順に見る
pub fn find_rollout(thread_id: &str) -> Option<PathBuf> {
    if !is_valid_thread_id(thread_id) {
        return None;
    }
    let root = codex_home()?.join("sessions");
    let suffix = format!("-{thread_id}.jsonl");
    // sessions/<YYYY>/<MM>/<DD>/ の 3 段。各段を名前の降順（= 新しい順）で辿る
    fn descend(dir: &Path, depth: usize, suffix: &str) -> Option<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort_unstable();
        entries.reverse();
        if depth == 0 {
            return entries
                .into_iter()
                .find(|p| p.to_string_lossy().ends_with(suffix));
        }
        entries
            .into_iter()
            .filter(|p| p.is_dir())
            .find_map(|p| descend(&p, depth - 1, suffix))
    }
    descend(&root, 3, &suffix)
}

/// ファイル末尾から最大 `max` バイトを読む（**逐次追記されるファイルを安く読む**）。
/// 先頭が行の途中で切れることがあるので、最初の改行までは捨てる
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
    // 途中から読んだので最初の行は捨てる
    Some(
        s.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default(),
    )
}

/// thread_id のターン状態を読む（`None` = rollout がまだ無い = ターン未実行）
pub fn read_turn_state(thread_id: &str) -> Option<TurnState> {
    let path = find_rollout(thread_id)?;
    let text = read_tail(&path, STATE_TAIL_BYTES)?;
    let lines: Vec<&str> = text.lines().collect();
    Some(parse_turn_state(&lines))
}

/// 複数ペインぶんの thread_id を **1 個の `ProcessSnapshot` から**まとめて解決する（#985）。
///
/// 単発の [`resolve_thread_id_for_backend`] は 1 回ごとに tmux と ps を起こすので、
/// ステータスバーのような**定期処理からペイン数ぶん呼ぶと #772 / #779 で削った
/// subprocess が戻る**。ここは呼び出し側が既に採った 1 枚のスナップショットを使うので、
/// 対象が何ペインでもプロセス起動は増えない（`lsof` だけは codex を見つけたペインに
/// 1 回ずつ要るが、結果は sticky なので次回以降は 0 回）。
///
/// **background executor 専用**（`lsof` を起こしうる）
pub fn resolve_thread_ids_with(
    backends: &[String],
    snap: &crate::agents::ProcessSnapshot,
) -> HashMap<String, String> {
    if legacy_screen_only() {
        return HashMap::new();
    }
    let mut out = HashMap::new();
    for backend in backends {
        if let Some(id) = sticky_lookup(backend) {
            out.insert(backend.clone(), id);
            continue;
        }
        // ペイン配下でいちばん新しい codex プロセスを snapshot から拾う
        let mut hits: Vec<u32> = snap
            .descendant_pids(backend)
            .into_iter()
            .filter(|pid| snap.argv(*pid).is_some_and(is_codex_command))
            .collect();
        hits.sort_unstable();
        let Some(pid) = hits.pop() else { continue };
        if let Some(id) = thread_id_for_pid(pid) {
            sticky_insert(backend, &id);
            out.insert(backend.clone(), id);
        }
    }
    out
}

/// ペイン（backend セッション）の codex のレート制限を読む（#985）。
///
/// **呼び出し側の責務**: これは `lsof`（sticky なので初回だけ）+ rollout 末尾の読み取りを
/// 伴う I/O なので、**2 秒 tick の UI スレッドから直接呼ばない**
/// （#772 / #779 / #816 で削った subprocess を復活させない）。tako-app は background で呼ぶ
pub fn rate_limits_for_backend(backend_session: &str) -> Option<RateLimits> {
    let tid = resolve_thread_id_for_backend(backend_session)?;
    read_turn_state(&tid)?.rate_limits
}

/// thread_id の直近 N 件の assistant 発話（transcript アダプタの入口）
pub fn last_assistant_texts(thread_id: &str, limit: usize) -> Result<Vec<String>, String> {
    let path = find_rollout(thread_id)
        .ok_or_else(|| format!("codex の会話ログが見つからない: {thread_id}"))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("codex の会話ログを読めない {}: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    Ok(parse_assistant_texts(&lines, limit.max(1)))
}

/// backend セッション（= ペイン）配下の codex プロセスの pid を探す（**純粋関数**）。
///
/// 材料は既存の pid 祖先辿りと同じ（親子表 + コマンド表 + ペインの pid）。
/// 「コマンド名に codex を含む子孫」を新しい方（= pid の大きい方）から選ぶ
pub fn find_codex_pid(
    pane_pid: u32,
    parents: &HashMap<u32, u32>,
    commands: &HashMap<u32, String>,
) -> Option<u32> {
    let mut hits: Vec<u32> = commands
        .iter()
        .filter(|(_, cmd)| is_codex_command(cmd))
        .map(|(pid, _)| *pid)
        .filter(|pid| is_descendant(*pid, pane_pid, parents))
        .collect();
    hits.sort_unstable();
    hits.pop()
}

/// コマンド行が codex CLI か。**`codex` を含むだけでは採らない**
/// （`codex.system` のような別物や、tako 自身の引数に混ざった文字列を拾わないため
/// 実行ファイル名の位置で見る）
pub fn is_codex_command(cmd: &str) -> bool {
    cmd.split_whitespace().next().is_some_and(|prog| {
        let base = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
        base == "codex" || base == "codex.exe"
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

/// ペイン（backend セッション）→ thread_id の解決。**sticky**（#466 と同じ方針）。
///
/// `lsof` は 1 回 40〜70ms かかるので**毎ポーリングでは叩かない**。
/// 一度当てた対応はペインが生きているあいだ持ち続け、ペインが消えたら捨てる
/// （#772 / #779 / #816 で削った subprocess を復活させないため）。
///
/// `TAKO_984_LEGACY=1` で構造化ソースを無効化し、画面推定のみ（旧挙動）へ戻せる
pub fn resolve_thread_id_for_backend(backend_session: &str) -> Option<String> {
    if legacy_screen_only() {
        return None;
    }
    let panes = crate::agents::backend_pane_pids();
    // 消えたペインの記憶は捨てる（pane ID 再利用で別 worker の会話を返さない）。
    // **ID は `session:window.pane` 形式**なので接頭辞で見る（下の探索と同じ規則）
    sticky_forget_gone(&panes);
    if let Some(id) = sticky_lookup(backend_session) {
        return Some(id);
    }
    // **器のペイン ID は `session:window.pane`**（`agents::tmux_pane_pids` の -F 書式）。
    // 素の等値で比べると必ず外れる（実測で踏んだ）。claude 側
    // `resolve_session_id_for_backend` と同じ接頭辞判定にそろえる
    let pane_pid = pane_pid_of(&panes, backend_session);
    let diag = std::env::var_os("TAKO_984_DIAG").is_some();
    if diag {
        eprintln!(
            "[984] backend={backend_session} panes={} pane_pid={pane_pid:?}",
            panes.len()
        );
    }
    let resolved = pane_pid.and_then(|pane_pid| {
        let (parents, commands) = crate::agents::capture_process_table();
        if diag {
            let codex: Vec<u32> = commands
                .iter()
                .filter(|(_, c)| is_codex_command(c))
                .map(|(p, _)| *p)
                .collect();
            eprintln!(
                "[984] parents={} commands={} codex候補={codex:?}",
                parents.len(),
                commands.len()
            );
        }
        let codex_pid = find_codex_pid(pane_pid, &parents, &commands);
        if diag {
            eprintln!("[984] codex_pid={codex_pid:?}");
        }
        let id = thread_id_for_pid(codex_pid?);
        if diag {
            eprintln!("[984] thread_id={id:?}");
        }
        id
    });
    if let Some(ref id) = resolved {
        sticky_insert(backend_session, id);
    }
    resolved
}

/// 器のペイン一覧（`session:window.pane`, pane_pid）から、そのセッションの
/// 代表ペインの pid を取る（**純粋関数**）
pub fn pane_pid_of(panes: &[(String, u32)], backend_session: &str) -> Option<u32> {
    let prefix = format!("{backend_session}:");
    panes
        .iter()
        .find(|(id, _)| id.starts_with(&prefix))
        .map(|(_, pid)| *pid)
}

/// そのセッションのペインが 1 つでも残っているか（**純粋関数**）
pub fn session_has_pane(panes: &[(String, u32)], backend_session: &str) -> bool {
    pane_pid_of(panes, backend_session).is_some()
}

/// 構造化ソースを使わず画面推定だけに戻す逃げ道（#984 の A/B）
/// 解決済み backend → thread_id の記憶（sticky）。
/// `resolve_thread_id_for_backend`（単発）と `resolve_thread_ids_with`（一括）が共有する
fn sticky() -> &'static std::sync::Mutex<HashMap<String, String>> {
    static STICKY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> =
        std::sync::OnceLock::new();
    STICKY.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn sticky_lookup(backend_session: &str) -> Option<String> {
    let map = sticky().lock().unwrap_or_else(|e| e.into_inner());
    map.get(backend_session).cloned()
}

fn sticky_insert(backend_session: &str, thread_id: &str) {
    let mut map = sticky().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(backend_session.to_string(), thread_id.to_string());
}

/// 消えたペインの記憶を捨てる
fn sticky_forget_gone(panes: &[(String, u32)]) {
    let mut map = sticky().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|b, _| session_has_pane(panes, b));
}

pub fn legacy_screen_only() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os("TAKO_984_LEGACY").is_some())
}

#[cfg(test)]
mod tests {

    // --- #985: レート制限（実採取。2026-08-27 / codex-cli 0.150.1 / plan_type = plus） ---

    /// **実採取の 1 行をそのまま**（本文は含まないので個人情報なし。#927）。
    /// この行は `token_count` イベントで、ctx% の材料と **同じイベントに**
    /// `rate_limits` が載っていることを示す
    const REAL_TOKEN_COUNT: &str = r#"{"timestamp": "2026-08-27T09:57:16.392Z", "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"total_tokens": 158324}, "last_token_usage": {"total_tokens": 32564}, "model_context_window": 258400}, "rate_limits": {"limit_id": "codex", "limit_name": null, "primary": {"used_percent": 4.0, "window_minutes": 300, "resets_at": 1787840583}, "secondary": {"used_percent": 1.0, "window_minutes": 10080, "resets_at": 1788427383}, "credits": {"has_credits": false, "unlimited": false, "balance": "0"}, "individual_limit": null, "plan_type": "plus", "rate_limit_reached_type": null}}}"#;

    /// 5 時間枠を使い切った形。`rate_limit_reached_type` は HTTP ヘッダ
    /// `x-codex-rate-limit-reached-type`（バイナリ内文字列で確認）由来なので
    /// **上流が「どの枠か」を名指しする**。実採取の枠組みに使い切りの値を入れた合成
    const REACHED_PRIMARY: &str = r#"{"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"total_tokens": 10}, "model_context_window": 100}, "rate_limits": {"limit_id": "codex", "primary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": 1787840583}, "secondary": {"used_percent": 62.0, "window_minutes": 10080, "resets_at": 1788427383}, "plan_type": "plus", "rate_limit_reached_type": "primary"}}}"#;

    #[test]
    fn issue985_実採取のtoken_countからレート制限を取り出す() {
        let st = parse_turn_state(&[REAL_TOKEN_COUNT]);
        let rl = st.rate_limits.expect("rate_limits が読める");
        let p = rl.primary.expect("primary");
        let s = rl.secondary.expect("secondary");
        assert_eq!(p.used_percent, 4, "used_percent は小数 4.0 → 4");
        assert_eq!(p.window_minutes, 300, "primary = 5 時間枠");
        assert_eq!(p.resets_at, Some(1_787_840_583), "epoch 秒がそのまま取れる");
        assert_eq!(s.used_percent, 1);
        assert_eq!(s.window_minutes, 10_080, "secondary = 週枠");
        assert_eq!(
            rl.plan_type.as_deref(),
            Some("plus"),
            "有料プランの実データ"
        );
        assert_eq!(rl.reached, None, "上限前は null");
        // ctx% は従来どおり同じイベントから取れる（#984 の回帰）
        assert_eq!(st.ctx_percent, Some(12));
    }

    #[test]
    fn issue985_上限前はリセット待ちの時刻を出さない() {
        let rl = parse_turn_state(&[REAL_TOKEN_COUNT])
            .rate_limits
            .expect("読める");
        assert!(!rl.limited(), "4% / 1% は上限ではない");
        assert_eq!(
            rl.reset_at(),
            None,
            "上限でないのに primary の resets_at を返すと「上限でもないのに待つ」になる"
        );
    }

    #[test]
    fn issue985_使い切った枠の解除時刻を名指しで返す() {
        let rl = parse_turn_state(&[REACHED_PRIMARY])
            .rate_limits
            .expect("読める");
        assert!(rl.limited());
        assert_eq!(
            rl.reset_at(),
            Some(1_787_840_583),
            "reached=primary なので **週枠ではなく** 5 時間枠の解除時刻"
        );
    }

    #[test]
    fn issue985_reachedが無くても使い切った枠から解除時刻を選ぶ() {
        // 上流が種別を返さない版・プランでも、使用率だけで決められる
        let src = REACHED_PRIMARY.replace(r#", "rate_limit_reached_type": "primary""#, "");
        let rl = parse_turn_state(&[&src]).rate_limits.expect("読める");
        assert!(rl.limited(), "100% は使い切り");
        assert_eq!(rl.reset_at(), Some(1_787_840_583));
        // 両方使い切っていれば「先に空くほう」
        let both = src.replace(r#""used_percent": 62.0"#, r#""used_percent": 100.0"#);
        let rl = parse_turn_state(&[&both]).rate_limits.expect("読める");
        assert_eq!(rl.reset_at(), Some(1_787_840_583), "早く空くほう = primary");
    }

    #[test]
    fn issue985_レート制限が無い版でも壊れない() {
        // `rate_limits` を持たない rollout（旧版 / 別プラン）でも ctx% は従来どおり
        let src = REAL_TOKEN_COUNT
            .split(r#", "rate_limits""#)
            .next()
            .unwrap()
            .to_string()
            + "}}";
        let st = parse_turn_state(&[&src]);
        assert_eq!(st.rate_limits, None, "無いものを捏造しない");
        assert_eq!(st.ctx_percent, Some(12), "#984 の経路は無傷");
    }

    #[test]
    fn issue985_使用率は切り捨てで99を100と言わない() {
        let src = REACHED_PRIMARY.replace(r#""used_percent": 100.0"#, r#""used_percent": 99.7"#);
        let rl = parse_turn_state(&[&src]).rate_limits.expect("読める");
        let p = rl.primary.expect("primary");
        assert_eq!(
            p.used_percent, 99,
            "四捨五入すると 100 = 使い切りと誤認する"
        );
        assert!(!p.exhausted());
    }

    use super::*;

    /// 実採取した rollout の形（codex-cli 0.150.1・2026-08-27）。
    /// **本物の 1 行をそのまま持つ**ので、上流が形を変えたらここが古くなる = 実装も直す合図
    const STARTED: &str = r#"{"timestamp":"2026-08-27T11:10:30.535Z","ordinal":1,"type":"event_msg","payload":{"type":"task_started","turn_id":"01a042ea-0e83-7ac1-b564-1ffd56f0308d","started_at":1787829030,"model_context_window":258400,"collaboration_mode_kind":"default"}}"#;
    const COMPLETE: &str = r#"{"timestamp":"2026-08-27T11:10:35.077Z","ordinal":13,"type":"event_msg","payload":{"type":"task_complete","turn_id":"01a042ea-0e83-7ac1-b564-1ffd56f0308d","last_agent_message":"ok","started_at":1787829030,"completed_at":1787829035,"duration_ms":4543}}"#;
    const TOKENS: &str = r#"{"timestamp":"2026-08-27T11:10:35.065Z","ordinal":12,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":21302},"last_token_usage":{"total_tokens":25840},"model_context_window":258400}}}"#;
    const ASSISTANT: &str = r#"{"timestamp":"2026-08-27T11:10:35.042Z","ordinal":11,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#;
    const USER: &str = r#"{"timestamp":"2026-08-27T11:10:32.423Z","ordinal":8,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"say ok"}]}}"#;
    const REASONING: &str = r#"{"timestamp":"2026-08-27T11:10:33.000Z","ordinal":10,"type":"response_item","payload":{"type":"reasoning","summary":[]}}"#;

    #[test]
    fn ターン開始だけならbusy() {
        let st = parse_turn_state(&[STARTED]);
        assert!(st.busy);
        assert_eq!(st.status(), Some("busy"));
    }

    #[test]
    fn ターン完了までそろえばidle() {
        let st = parse_turn_state(&[STARTED, ASSISTANT, TOKENS, COMPLETE]);
        assert!(!st.busy);
        assert_eq!(st.status(), Some("idle"));
        assert_eq!(st.last_agent_message.as_deref(), Some("ok"));
        // last_token_usage を優先する（25840 / 258400 = 10%）
        assert_eq!(st.ctx_percent, Some(10));
    }

    #[test]
    fn 次のターンが始まれば再びbusy() {
        let started2 = STARTED.replace("\"ordinal\":1", "\"ordinal\":20");
        let st = parse_turn_state(&[STARTED, COMPLETE, &started2]);
        assert!(st.busy, "完了の後に開始が来たら busy に戻る");
    }

    /// **イベントが 1 つも無い = 起動直後**。ここで idle と言うと
    /// 「プロンプト投入前」を完了と誤認するので `None` を返す
    #[test]
    fn イベントが無ければ何も言わない() {
        let st = parse_turn_state(&[]);
        assert_eq!(st.status(), None);
        let st = parse_turn_state(&[USER, REASONING]);
        assert_eq!(st.status(), None, "ターンのイベントが無ければ判定しない");
    }

    /// 逐次追記されるファイルを読むので**行が途中で切れている**ことがある。
    /// 壊れた行で全体が読めなくなってはいけない
    #[test]
    fn 途中で切れた行は読み飛ばす() {
        let broken = r#"{"timestamp":"2026-08-27T11:10:36.0"#;
        let st = parse_turn_state(&[STARTED, broken]);
        assert!(st.busy);
        let st = parse_turn_state(&[STARTED, COMPLETE, broken]);
        assert!(!st.busy);
    }

    #[test]
    fn assistantの発話だけを古い順に取る() {
        let a2 = ASSISTANT.replace("\"text\":\"ok\"", "\"text\":\"second\"");
        let got = parse_assistant_texts(&[USER, ASSISTANT, REASONING, &a2], 5);
        assert_eq!(got, vec!["ok".to_string(), "second".to_string()]);
        // limit は「直近 N 件」（claude 側と同じ約束）
        assert_eq!(
            parse_assistant_texts(&[USER, ASSISTANT, &a2], 1),
            vec!["second".to_string()]
        );
        // user / reasoning は入らない
        assert!(parse_assistant_texts(&[USER, REASONING], 5).is_empty());
    }

    #[test]
    fn lsofの出力からthread_idを拾う() {
        let out = "p12345\n\
                   n/Users/testuser/.codex/state_5.sqlite\n\
                   n/Users/testuser/.codex/thread-writer-locks/01a042fa-b623-7232-87dc-c141f5e20b58.lock\n";
        assert_eq!(
            thread_id_from_lsof(out).as_deref(),
            Some("01a042fa-b623-7232-87dc-c141f5e20b58")
        );
        // 調整用ロックは thread_id ではない
        let coord = "n/Users/testuser/.codex/thread-writer-locks/.coordination.lock\n";
        assert_eq!(thread_id_from_lsof(coord), None);
        assert_eq!(thread_id_from_lsof(""), None);
    }

    #[test]
    fn thread_idの形式検証がパストラバーサルを弾く() {
        assert!(is_valid_thread_id("01a042fa-b623-7232-87dc-c141f5e20b58"));
        assert!(!is_valid_thread_id("../../etc/passwd"));
        assert!(!is_valid_thread_id("a/b"));
        assert!(!is_valid_thread_id(""));
        assert!(!is_valid_thread_id(&"x".repeat(129)));
        assert!(find_rollout("../../etc/passwd").is_none());
    }

    /// コマンド名の**位置**で見る（引数に codex が混ざっただけでは採らない）
    #[test]
    fn codexのコマンド判定は実行ファイル名で見る() {
        assert!(is_codex_command("/Users/testuser/.local/bin/codex"));
        assert!(is_codex_command(
            "codex --dangerously-bypass-approvals-and-sandbox"
        ));
        assert!(!is_codex_command("/usr/bin/tako send --text codex"));
        assert!(!is_codex_command(
            "/var/run/com.apple.security.cryptexd/codex.system/bootstrap/usr/bin/foo"
        ));
        assert!(!is_codex_command(""));
    }

    #[test]
    fn 子孫の判定でペイン配下のcodexだけを選ぶ() {
        let parents: HashMap<u32, u32> = [(200, 100), (300, 200), (900, 800)].into();
        let commands: HashMap<u32, String> = [
            (300, "/Users/testuser/.local/bin/codex".to_string()),
            (900, "/Users/testuser/.local/bin/codex".to_string()),
        ]
        .into();
        assert_eq!(find_codex_pid(100, &parents, &commands), Some(300));
        // 別ペイン配下（800）の codex は拾わない
        assert_eq!(find_codex_pid(700, &parents, &commands), None);
    }
}

#[cfg(test)]
mod pane_lookup_tests {
    use super::*;

    /// **器のペイン ID は `session:window.pane`**。素の等値で比べると必ず外れる
    /// （実測で踏んだ回帰。claude 側の `resolve_session_id_for_backend` と同じ規則にする）
    #[test]
    fn セッション名の接頭辞でペインpidを引く() {
        let panes = vec![
            ("tako-82fbb28dec29:0.0".to_string(), 4242_u32),
            ("tako-other:0.0".to_string(), 9999_u32),
        ];
        assert_eq!(pane_pid_of(&panes, "tako-82fbb28dec29"), Some(4242));
        assert!(session_has_pane(&panes, "tako-82fbb28dec29"));
        // 素の等値だった頃の形（`tako-82fbb28dec29` そのもの）では引けない
        assert_eq!(
            pane_pid_of(&panes, "tako-82fbb28dec2"),
            None,
            "接頭辞の途中一致で誤ヒットしない"
        );
        assert_eq!(pane_pid_of(&panes, "tako-missing"), None);
        assert!(!session_has_pane(&panes, "tako-missing"));
    }
}
