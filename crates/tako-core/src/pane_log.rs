//! pane_log — ペインのターミナル出力を平文でローテーション保存する（Issue #112 B）
//!
//! 目的: ペイン / タブ / アプリが死んでも「素のシェル・worker のビルド / テスト出力」を
//! 遡れるようにする。保存するのは**確定行**（スクロールバック履歴へ押し出された行）のみ:
//!
//! - 直接ペイン: alacritty Term の history 増分（メモリ読み取り）
//! - tmux バックエンドペイン: `#{history_size}` 増分 + `capture-pane -p`（ANSI 除去済み平文）
//!
//! この方式は TUI（claude 等）の描画スパムを構造的に除外する: alt screen 中は
//! history が増えないため、TUI 区間は「TUI 実行中」マーカーだけが残る（Issue 記載の要件）。
//! 会話ログ自体はセッションカタログ（Issue #112 A）が claude の transcript を参照する。
//!
//! tmux バックエンドペインの取り込みは子プロセスを起こすので、毎 tick ではなく
//! [`BackendPace`] が決めた tick だけ行う（Issue #2018）。閉じるときは器を kill する直前に
//! 最後の増分を取り込んでから可視画面とクローズマーカーを書く（tako-app 側）。
//!
//! サイズ管理（target 44GB 肥大の教訓）:
//! - ペインあたり上限（既定 5MB）超過で `.1` へ 1 世代ローテーション
//! - ログディレクトリ全体の上限（既定 200MB）超過で古いファイルから削除
//!
//! プライバシー: ログにはペイン内容（トークンの写り込み等）が含まれ得るため、
//! 保存先はユーザーローカルのデータディレクトリ限定（`<data_dir>/pane-logs/`）。
//! リポジトリ・同期対象に置かないこと。`TAKO_PANE_LOG_DIR` で隔離検証用に上書き可能。

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 1 tick で取り込む最大行数（洪水時の上限。超過分は省略マーカーを残す）
pub const CAPTURE_CHUNK: usize = 400;

/// 重複判定アンカーの行数（履歴カウンタ飽和時のオーバーラップ照合に使う）
const TAIL_ANCHOR: usize = 8;

/// ローテーションで残す世代数（`.1` のみ = 直前世代）
const KEEP_GENERATIONS: u32 = 1;

/// ログ保存の設定（settings.json 由来。既定 ON / 5MB / 200MB）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneLogConfig {
    pub enabled: bool,
    /// ペインあたりのファイルサイズ上限（超過でローテーション）
    pub max_bytes_per_pane: u64,
    /// ログディレクトリ全体の上限（超過で古いファイルから削除）
    pub max_total_bytes: u64,
}

impl Default for PaneLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bytes_per_pane: 5 * 1024 * 1024,
            max_total_bytes: 200 * 1024 * 1024,
        }
    }
}

/// ペインが閉じた発生源（Issue #566）。クローズマーカーに書き残し、
/// 「誰がこのペインを閉じたか」を事後に追跡できるようにする。
///
/// 2026-07-27 の master ペイン消失調査では `close` / `exit` の 2 値しか無く、
/// GUI 操作・キーバインド・CLI / MCP・プロセス終了を区別できずに消去法でしか
/// 結論を出せなかった。ここで区別を型に落として同じ調査を再発させない
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOrigin {
    /// キーバインド（Cmd+W）
    Keyboard,
    /// GUI のペイン × ボタン
    PaneButton,
    /// GUI のタブ × ボタン（タブごと閉じる）
    TabButton,
    /// CLI（`tako close` 等）からの dispatch
    Cli,
    /// MCP ツール（`tako_close_pane` 等）からの dispatch
    Mcp,
    /// 経路不明の dispatch（GUI が dispatch を直接呼ぶ経路・提案チップ等）
    Dispatch,
    /// アプリ内部の後始末（PTY 起動失敗のロールバック・死亡ビューの掃除等）
    Internal,
    /// 実行ペインの auto_close（#1662 / #1778）。終わった実行ペインを方針どおり閉じた。
    /// 気づいたのが GUI の終了検知でも `--wait` の聞き直しでも、閉じたのは人ではないので
    /// 1 つにまとめる（手で閉じた `close:gui` / `close:dispatch(…)` と後から見分ける）
    AutoClose,
    /// アプリ終了に伴う最終フラッシュ
    AppQuit,
    /// プロセス終了・PTY 死亡（ユーザー操作ではない）
    ProcessExit,
}

impl CloseOrigin {
    /// クローズマーカーに書く発生源文字列
    pub fn marker(self) -> &'static str {
        match self {
            CloseOrigin::Keyboard => "close:kbd",
            CloseOrigin::PaneButton => "close:gui",
            CloseOrigin::TabButton => "close:gui-tab",
            CloseOrigin::Cli => "close:dispatch(cli)",
            CloseOrigin::Mcp => "close:dispatch(mcp)",
            CloseOrigin::Dispatch => "close:dispatch",
            CloseOrigin::Internal => "close:internal",
            CloseOrigin::AutoClose => "close:auto",
            CloseOrigin::AppQuit => "close:app-quit",
            CloseOrigin::ProcessExit => "exit",
        }
    }

    /// dispatch 経路（CLI / MCP）なら呼び出し元 role を添えた発生源文字列を作る。
    /// role はマーカー書式を壊さないよう正規化する（診断ログにペイン内容・
    /// トークンを出さない絶対ルールがあるため、素通しはしない）
    pub fn marker_with_caller(self, caller: Option<&str>) -> String {
        let base = self.marker();
        if !self.is_dispatch() {
            return base.to_string();
        }
        match caller.map(sanitize_caller).filter(|c| !c.is_empty()) {
            Some(caller) => match base.strip_suffix(')') {
                // "close:dispatch(cli)" → "close:dispatch(cli, caller=...)"
                Some(head) => format!("{head}, caller={caller})"),
                // "close:dispatch" → "close:dispatch(caller=...)"
                None => format!("{base}(caller={caller})"),
            },
            None => base.to_string(),
        }
    }

    /// CLI / MCP など制御プレーン由来か
    pub fn is_dispatch(self) -> bool {
        matches!(
            self,
            CloseOrigin::Cli | CloseOrigin::Mcp | CloseOrigin::Dispatch
        )
    }
}

/// クローズマーカー行の前後。書き出しと読み取りで同じ定数を使う
const CLOSE_MARKER_PREFIX: &str = "--- [クローズ: ";
const CLOSE_MARKER_SUFFIX: &str = "] ---";

/// クローズマーカー行から発生源（`CloseOrigin::marker_with_caller` が作った文字列）を
/// 取り出す。マーカー行でなければ None。
///
/// caller つき（`close:dispatch(cli, caller=…)`）でもそのまま返すので、呼び出し側は
/// `marker()` / `marker_with_caller()` と突き合わせて発生源を厳密に判定できる。
/// 発生源の前方一致で判定すると `close:gui` が `close:gui-tab` を拾ってしまうため、
/// 判定材料は「発生源そのもの」として切り出す（Issue #599）
pub fn close_marker_reason(line: &str) -> Option<&str> {
    let body = line
        .strip_prefix(CLOSE_MARKER_PREFIX)?
        .strip_suffix(CLOSE_MARKER_SUFFIX)?;
    // 末尾は必ず UTC タイムスタンプ（空白を含まない）。caller も正規化済みで空白を含まない
    body.rsplit_once(' ').map(|(reason, _utc)| reason)
}

/// caller（role 等）をマーカーへ埋め込める文字だけに落とす。
/// 許可: 英数字 / `-` `_` `:` `.` / それ以外は `_`。長さは 64 文字で打ち切る
fn sanitize_caller(caller: &str) -> String {
    caller
        .chars()
        .take(64)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// ファイル命名・ヘッダに使うペインのメタ情報（作成時点のスナップショット）
#[derive(Debug, Clone, Default)]
pub struct PaneLogMeta {
    pub tab: u64,
    /// role（優先）または title。ファイル名のスラグとヘッダに使う
    pub label: Option<String>,
}

/// 1 ペイン分の保存状態
#[derive(Debug)]
struct PaneState {
    path: PathBuf,
    /// 現ファイルの概算サイズ（ローテーション判定。書き込み成功時に加算）
    bytes: u64,
    /// 直近に保存した行（履歴カウンタ飽和時のオーバーラップ照合アンカー）
    tail: Vec<String>,
    /// 直近に観測した履歴行数（増分検知の基準）
    last_history: usize,
    /// 直近に観測した履歴バイト数（tmux の飽和後の変化検知。直接ペインは 0 のまま）
    last_bytes: u64,
    /// alt screen（TUI）区間の観測状態
    alt_screen: bool,
}

/// ペインログの管理体。GUI プロセスが 1 つ保持し、UI / background の両スレッドから
/// Mutex 越しに使う（クリティカルセクションは小さな追記のみ）
#[derive(Debug)]
pub struct PaneLogManager {
    dir: PathBuf,
    config: PaneLogConfig,
    states: HashMap<u64, PaneState>,
    /// クローズフラッシュ済みのペイン（#2018）。器のペインの取り込みは background で
    /// 走るので、閉じた後に届いた観測が状態を作り直し、新しいログファイルへ同じ行を
    /// 書き始めないようにする（ペイン ID はプロセスの中で使い回されない）
    closed: HashSet<u64>,
}

/// ペイン 1 回分の走査結果（呼び出し側が Term / tmux から観測して詰める）
#[derive(Debug)]
pub struct PaneObservation {
    /// 現在の履歴行数
    pub history: usize,
    /// 履歴の保持上限（飽和判定に使う。直接ペインは
    /// [`crate::TerminalSession::scrollback_limit`] = 設定値（#818。既定
    /// [`crate::scrollback::DEFAULT_LINES`]）、tmux は器の `history-limit`）
    pub history_limit: usize,
    /// 履歴バイト数（tmux `#{history_bytes}`。飽和後の変化検知用。直接ペインは 0）
    pub bytes: u64,
    /// alt screen（TUI）中か
    pub alt_screen: bool,
    /// 新規行の取り込み内容
    pub chunk: ChunkKind,
}

/// 取り込み方法。履歴カウンタが単調増加のうちは Counted、飽和後は Overlap で照合する
#[derive(Debug)]
pub enum ChunkKind {
    /// 取り込みなし（増分ゼロ）
    None,
    /// 増分 `delta` のうち末尾 `lines`（`delta > lines.len()` なら省略マーカーを残す）
    Counted { lines: Vec<String>, delta: usize },
    /// 履歴カウンタ飽和時: 末尾チャンクを渡し、保存済み tail との重複を除いて追記する
    Overlap { captured: Vec<String> },
}

impl PaneLogManager {
    pub fn new(dir: PathBuf, config: PaneLogConfig) -> Self {
        Self {
            dir,
            config,
            states: HashMap::new(),
            closed: HashSet::new(),
        }
    }

    pub fn config(&self) -> PaneLogConfig {
        self.config
    }

    pub fn set_config(&mut self, config: PaneLogConfig) {
        self.config = config;
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 復元ペインの履歴基準値をシードする（layout.json の `logged_history`。
    /// tako 停止中に積もった履歴を次回 tick で差分として取り込むため）
    pub fn seed_history(&mut self, pane: u64, meta: &PaneLogMeta, logged_history: usize) {
        self.ensure_state(pane, meta).last_history = logged_history;
    }

    /// 現在の履歴基準値（layout.json への保存用）。状態が無いペインは None
    pub fn logged_history(&self, pane: u64) -> Option<usize> {
        self.states.get(&pane).map(|s| s.last_history)
    }

    /// 全ペインの履歴基準値（layout 保存の一括取得用）
    pub fn all_logged_history(&self) -> Vec<(u64, usize)> {
        self.states
            .iter()
            .map(|(pane, s)| (*pane, s.last_history))
            .collect()
    }

    /// 走査前のコンテキスト（前回履歴行数・前回履歴バイト数）。状態が無ければ None
    pub fn scan_baseline(&self, pane: u64) -> Option<(usize, u64)> {
        self.states
            .get(&pane)
            .map(|s| (s.last_history, s.last_bytes))
    }

    /// 走査結果を反映する（マーカー・新規行の追記 + 状態更新）。
    /// `enabled` が false なら状態だけ更新して書き込まない（OFF 中の再 ON で
    /// 巨大な差分を一気に取り込まないため、基準値は追従させ続ける）
    pub fn apply(&mut self, pane: u64, meta: &PaneLogMeta, obs: PaneObservation) {
        if self.closed.contains(&pane) {
            return;
        }
        let enabled = self.config.enabled;
        let max_bytes = self.config.max_bytes_per_pane;
        let state = self.ensure_state(pane, meta);

        // alt screen（TUI）区間のマーカー
        if obs.alt_screen != state.alt_screen {
            state.alt_screen = obs.alt_screen;
            if enabled {
                let marker = if obs.alt_screen {
                    format!("--- [TUI 実行中 {}] ---", now_utc())
                } else {
                    format!("--- [TUI 終了 {}] ---", now_utc())
                };
                append_lines_to(state, std::slice::from_ref(&marker), max_bytes);
            }
        }

        // 履歴カウンタの巻き戻り（clear-history / ED3）は基準をリセットして続行する
        if obs.history < state.last_history {
            state.last_history = obs.history;
        }
        state.last_bytes = obs.bytes;

        match obs.chunk {
            ChunkKind::None => {
                state.last_history = obs.history;
            }
            ChunkKind::Counted { lines, delta } => {
                if enabled {
                    if delta > lines.len() {
                        let marker = format!("--- [省略: {} 行] ---", delta - lines.len());
                        append_lines_to(state, std::slice::from_ref(&marker), max_bytes);
                    }
                    append_lines_to(state, &lines, max_bytes);
                }
                update_tail(&mut state.tail, &lines);
                state.last_history = obs.history;
            }
            ChunkKind::Overlap { captured } => {
                let new_lines = split_new_suffix(&state.tail, &captured);
                if !new_lines.is_empty() {
                    if enabled {
                        append_lines_to(state, new_lines, max_bytes);
                    }
                    let owned: Vec<String> = new_lines.to_vec();
                    update_tail(&mut state.tail, &owned);
                }
                state.last_history = obs.history;
            }
        }
    }

    /// ペイン close / exit 時の最終フラッシュ: 可視画面の残り（履歴に落ちていない行）を
    /// 追記し、クローズマーカーを書いて状態を破棄する。
    /// `origin` / `caller` は発生源の記録（Issue #566）。文字列ではなく型で受け取り、
    /// 呼び出し側が発生源を書き忘れられないようにする
    pub fn flush_close(
        &mut self,
        pane: u64,
        meta: &PaneLogMeta,
        visible: &[String],
        origin: CloseOrigin,
        caller: Option<&str>,
    ) {
        // 閉じたペインを 2 回閉じない（2 回目は新しいファイルを作ってマーカーだけを書く）
        if !self.closed.insert(pane) {
            return;
        }
        let reason = origin.marker_with_caller(caller);
        let enabled = self.config.enabled;
        let max_bytes = self.config.max_bytes_per_pane;
        let state = self.ensure_state(pane, meta);
        if enabled {
            let mut lines: Vec<&str> = visible.iter().map(String::as_str).collect();
            while lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.pop();
            }
            let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            if !owned.is_empty() {
                append_lines_to(state, &owned, max_bytes);
            }
            let marker = format!(
                "{CLOSE_MARKER_PREFIX}{reason} {}{CLOSE_MARKER_SUFFIX}",
                now_utc()
            );
            append_lines_to(state, std::slice::from_ref(&marker), max_bytes);
        }
        self.states.remove(&pane);
    }

    /// クローズフラッシュ済みか（#2018。background の取り込みが閉じたペインの器を読まない）
    pub fn is_closed(&self, pane: u64) -> bool {
        self.closed.contains(&pane)
    }

    /// 取り込み済みの末尾（最大 [`TAIL_ANCHOR`] 行。#2018 の揃えの印）。状態が無ければ空
    pub fn anchor_of(&self, pane: u64) -> Vec<String> {
        self.states
            .get(&pane)
            .map(|s| s.tail.clone())
            .unwrap_or_default()
    }

    /// 現在ログを開いているペインのファイルパス（`tako logs` の対応付け・削除除外用）
    pub fn open_paths(&self) -> Vec<PathBuf> {
        self.states.values().map(|s| s.path.clone()).collect()
    }

    /// ペインの現在のログファイルパス（状態が無ければ None）
    pub fn path_of(&self, pane: u64) -> Option<PathBuf> {
        self.states.get(&pane).map(|s| s.path.clone())
    }

    /// ディレクトリ全体の上限を強制する。現在開いているファイルは削除対象から除外し、
    /// それ以外を古い順（更新時刻）に削除する。削除したファイル数を返す
    pub fn enforce_total_cap(&self) -> usize {
        let open: Vec<PathBuf> = self.open_paths();
        enforce_total_cap_at(&self.dir, self.config.max_total_bytes, &open)
    }

    fn ensure_state(&mut self, pane: u64, meta: &PaneLogMeta) -> &mut PaneState {
        self.states.entry(pane).or_insert_with(|| {
            let name = file_name(&now_utc_compact(), meta.tab, pane, meta.label.as_deref());
            PaneState {
                path: self.dir.join(name),
                bytes: 0,
                tail: Vec::new(),
                last_history: 0,
                last_bytes: 0,
                alt_screen: false,
            }
        })
    }
}

/// ペインログの定期走査の周期（tako-app の 2 秒ポーリング）。[`capture_budget`] の単位
pub const POLL_TICK: Duration = Duration::from_secs(2);

/// 出力が続いている器のペインを取り込む間隔（#2018）。
///
/// 器のペインの取り込みは `tmux list-panes -a`（増分の有無）+ `tmux capture-pane`
/// （増えた履歴）の子プロセスで、毎 tick（2 秒）回すと出力中の器 1 枚につき
/// tako の回収済みの子プロセスの CPU が約 1% になった（#2011 S0 の実測）。
/// 出力が止まれば次の tick で取り込む（[`BackendPace::decide`]）ので、
/// 遅れるのは出力が続いている間だけ
pub const BUSY_INTERVAL: Duration = Duration::from_secs(10);

/// 出力の合図が無い器のペインを念のため見る間隔（#2018）。
///
/// 合図は tako のペインへ届いた tmux クライアントの出力なので、クライアントが
/// 描き直さない間の増分（コピーモードで止めている間・クライアントが繋がっていない間）は
/// 合図に出ない。それを取りこぼさないための保険（`list-panes -a` 1 本で全ペインぶん）
pub const IDLE_INSURANCE: Duration = Duration::from_secs(30);

/// 1 回に読む行数の上限の tick 数の上限（[`CAPTURE_CHUNK`] × 25 = 器の
/// `history-limit` の 10000 行。それより前の行は器にもう無い）
const MAX_BUDGET_TICKS: u128 = 25;

/// #2018 の A/B（`TAKO_2018_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// 器のペインを毎 tick・1 tick 分の上限で取り込み、閉じるときは最後の増分を
/// 取り込まずに可視画面とマーカーだけを書く。**ゲートはこの 1 か所**（tako-app もここを読む）
pub fn issue2018_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2018_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 前回の取り込みから `elapsed` 経った器のペインで 1 回に読む行数の上限（#2018）。
///
/// 洪水の上限（1 tick あたり [`CAPTURE_CHUNK`] 行・超えた分は省略マーカー）は
/// **時間あたり**で保つ。間隔を延ばしたのに上限を据え置くと、100 行/秒の出力を
/// 10 秒ごとに読むだけで 1000 行のうち 600 行が省略になる。
/// `None`（初回・再起動の直後）は 1 tick 分（修正前と同じ）
pub fn capture_budget(elapsed: Option<Duration>) -> usize {
    let ticks = elapsed
        .map(|e| e.as_millis().div_ceil(POLL_TICK.as_millis()))
        .unwrap_or(1)
        .clamp(1, MAX_BUDGET_TICKS);
    CAPTURE_CHUNK * ticks as usize
}

/// [`BackendPace::decide`] の答え（この tick で取り込む）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestDue {
    /// 1 回に読む行数の上限（[`capture_budget`]）
    pub budget: usize,
    /// 前回の取り込みの後に出力があった（初回を含む）。増分がある見込みなので、
    /// probe と同じ tmux の起動で履歴の末尾も読んでおく（#2018）。保険の取り込みは false
    pub output: bool,
}

/// 器のペイン 1 枚の取り込みの間合い（#2018）。tako-app がペインごとに持ち、
/// 2 秒ポーリングのたびに [`Self::decide`] へ「出力の合図」を渡す
#[derive(Debug, Clone, Copy, Default)]
pub struct BackendPace {
    /// 最後に取り込んだ時刻と、そのときの合図の値
    last: Option<(Instant, u64)>,
    /// 直前の tick で見た合図の値（出力が止まったかの判定）
    seen: Option<u64>,
}

impl BackendPace {
    /// この tick で取り込むなら `Some`、見送るなら `None`。
    ///
    /// `activity` は出力があるたびに変わる値（tako-app ではペインの PTY から届いた
    /// Wakeup の回数）。前の値と等しいかしか見ないので、増え方は問わない。
    ///
    /// - 初回（tako の起動・復元の直後を含む）: 取り込む（止まっている間に積もった分）
    /// - 前回の取り込みの後に出力があった: 直前の tick から合図が動いていない
    ///   （= 出力が止まった）なら取り込む。続いているなら [`BUSY_INTERVAL`] ごと
    /// - 出力が無い: [`IDLE_INSURANCE`] ごとに念のため
    /// - `legacy`（[`issue2018_legacy`]）: 毎 tick・1 tick 分（修正前）
    pub fn decide(&mut self, activity: u64, now: Instant, legacy: bool) -> Option<IngestDue> {
        let prev_seen = self.seen.replace(activity);
        let output = self.last.is_none_or(|(_, mark)| activity != mark);
        if legacy {
            self.last = Some((now, activity));
            return Some(IngestDue {
                budget: CAPTURE_CHUNK,
                output,
            });
        }
        let elapsed = self.last.map(|(at, _)| now.saturating_duration_since(at));
        let due = match elapsed {
            None => true,
            Some(elapsed) if output => prev_seen == Some(activity) || elapsed >= BUSY_INTERVAL,
            Some(elapsed) => elapsed >= IDLE_INSURANCE,
        };
        if !due {
            return None;
        }
        self.last = Some((now, activity));
        Some(IngestDue {
            budget: capture_budget(elapsed),
            output,
        })
    }

    /// 同じ tick に別の器の取り込みがある（= 全ペインぶんの probe を起こす）とき、出力の合図が
    /// 無い器も相乗りで確かめるか（#2018）。probe は 1 本で全ペインぶん出るので子プロセスは
    /// 増えない。保険の間隔の半分を過ぎた器だけで、相乗りが続く限り単独の保険は起きない。
    /// [`Self::decide`] がこの tick に `None` を返した後に呼ぶ
    pub fn ride_along(&mut self, now: Instant, legacy: bool) -> Option<IngestDue> {
        let (at, mark) = self.last?;
        let elapsed = now.saturating_duration_since(at);
        if legacy || self.seen != Some(mark) || elapsed < IDLE_INSURANCE / 2 {
            return None;
        }
        self.last = Some((now, mark));
        Some(IngestDue {
            budget: capture_budget(Some(elapsed)),
            output: false,
        })
    }

    /// 閉じる直前の取り込み（最後の増分）で読む行数の上限。前回の取り込みからの経過で決める
    pub fn catch_up_budget(&self, now: Instant) -> usize {
        capture_budget(self.last.map(|(at, _)| now.saturating_duration_since(at)))
    }
}

/// 揃えの余白（#2018）。器の増分は「probe（`list-panes`）で行数を知る → capture で読む」の
/// 2 つの子プロセスに分かれていて、その数 ms の間に伸びた行のぶん capture の範囲がずれる
/// （修正前はそのまま境目ごとに 0〜数行の欠け・重複になっていた）。増分よりこの行数だけ多く読み、
/// 取り込み済みの末尾（[`TAIL_ANCHOR`] 行）を探してずれを突き止める。
/// 揃えられるずれは `ALIGN_MARGIN - TAIL_ANCHOR` 行まで
pub const ALIGN_MARGIN: usize = 40;

/// 器の 1 回の取り込みで tmux から読む範囲（#2018。[`plan_backend_capture`] が決める）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendCapture {
    /// 読まない（増分なし・履歴の巻き戻り）
    Skip,
    /// 増分 `delta` のうち末尾 `take` 行を、揃えの余白 `margin` 行つきで読む
    Counted {
        delta: usize,
        take: usize,
        margin: usize,
    },
    /// 履歴が保持上限で飽和して行数が増えない: 末尾 `take` 行を読み、取り込み済みの末尾と照合する
    Overlap { take: usize },
}

impl BackendCapture {
    /// 履歴の末尾から読む行数（`capture-pane -S -<この値>`）
    pub fn lines_to_read(self) -> usize {
        match self {
            Self::Skip => 0,
            Self::Counted { take, margin, .. } => take + margin,
            Self::Overlap { take } => take,
        }
    }
}

/// 器の probe と基準から、tmux のどこを読むかを決める（#2018。純関数）。
/// `legacy`（[`issue2018_legacy`]）は余白なし = 修正前と同じ範囲
pub fn plan_backend_capture(
    probe: &crate::tmux::PaneLogProbe,
    last_history: usize,
    last_bytes: u64,
    budget: usize,
    legacy: bool,
) -> BackendCapture {
    if probe.history < last_history {
        return BackendCapture::Skip;
    }
    let delta = probe.history - last_history;
    if delta > 0 {
        let take = delta.min(budget);
        // 省略が出る（読み切れない）ときは揃えても欠けが残るので、修正前と同じ範囲
        let margin = if legacy || take < delta {
            0
        } else {
            ALIGN_MARGIN
        };
        return BackendCapture::Counted {
            delta,
            take,
            margin,
        };
    }
    // 履歴が history-limit で飽和するとカウンタが増えない。バイト数の変化を
    // 合図に末尾チャンクを取り、取り込み済み tail と照合して新規行だけ追記する
    if probe.history >= probe.limit && !probe.alternate && probe.bytes != last_bytes {
        return BackendCapture::Overlap { take: budget };
    }
    BackendCapture::Skip
}

/// 読んだ履歴（`hist` = 末尾 [`BackendCapture::lines_to_read`] 行）から観測を作る（#2018。純関数）。
///
/// `Counted` は余白の中から取り込み済みの末尾（`anchor`）を探し、probe の後に伸びた行まで
/// 含めて新しい行を切り出す。見つからない（初回・末尾が空行だけ・ずれが余白を超えた）なら
/// 修正前と同じく末尾 `take` 行
pub fn backend_observation(
    probe: &crate::tmux::PaneLogProbe,
    plan: BackendCapture,
    anchor: &[String],
    mut hist: Vec<String>,
) -> PaneObservation {
    let mut obs = PaneObservation {
        history: probe.history,
        history_limit: probe.limit,
        bytes: probe.bytes,
        alt_screen: probe.alternate,
        chunk: ChunkKind::None,
    };
    match plan {
        BackendCapture::Skip => {}
        BackendCapture::Counted {
            delta,
            take,
            margin,
        } => {
            // 余白なし（A/B・読み切れない増分）は末尾 `take` 行 = 修正前と同じ範囲。
            // probe と同じ tmux で多めに読んだ履歴（`hist` が `take` より長い）でも同じ
            let (lines, shift) = match margin {
                0 => {
                    let start = hist.len().saturating_sub(take);
                    (hist.split_off(start), 0)
                }
                _ => slice_counted(anchor, hist, take),
            };
            obs.history = (probe.history + shift).min(probe.limit.max(probe.history));
            obs.chunk = ChunkKind::Counted {
                lines,
                delta: delta + shift,
            };
        }
        BackendCapture::Overlap { .. } => obs.chunk = ChunkKind::Overlap { captured: hist },
    }
    obs
}

/// 閉じる直前の取り込み（最後の増分）で読む範囲（#2018 / #2085。純関数）。
///
/// [`plan_backend_capture`] と同じだが、増分が無くても probe の後に伸びた行を揃えで拾うため
/// 余白だけは読む（器を kill する閉じ方は probe と capture が別の起動になる）
pub fn backend_close_plan(
    probe: &crate::tmux::PaneLogProbe,
    last_history: usize,
    last_bytes: u64,
    budget: usize,
) -> BackendCapture {
    match plan_backend_capture(probe, last_history, last_bytes, budget, false) {
        BackendCapture::Skip if probe.history >= last_history => BackendCapture::Counted {
            delta: 0,
            take: 0,
            margin: ALIGN_MARGIN,
        },
        plan => plan,
    }
}

/// 器が自ら閉じたときの書き出し（`pane-died` フックの全履歴 + 画面。#2085）から、
/// 最後の取り込みの後の行の観測と、閉じる直前の画面を作る（純関数）。
///
/// 読む範囲と揃えは閉じる直前の取り込みと同じ 1 実装（[`backend_close_plan`] +
/// [`backend_observation`]）。書き出しは死んだ瞬間の probe と履歴が同じ時点のものなので
/// ずれは出ないが、取り込み済みの末尾で揃えることで最後の tick と重ならない
pub fn pane_died_observation(
    dump: crate::tmux_backend::PaneDiedDump,
    last_history: usize,
    last_bytes: u64,
    anchor: &[String],
    budget: usize,
) -> (PaneObservation, Vec<String>) {
    let crate::tmux_backend::PaneDiedDump {
        probe,
        mut history,
        screen,
    } = dump;
    let plan = backend_close_plan(&probe, last_history, last_bytes, budget);
    let tail = history.split_off(history.len().saturating_sub(plan.lines_to_read()));
    (backend_observation(&probe, plan, anchor, tail), screen)
}

/// 余白つきで読んだ履歴から、取り込み済みの末尾（`anchor`）の直後を新しい行として切り出す。
/// 戻り値は (新しい行, probe の後に伸びた行数)。ずれの小さい側を優先する（同じ行の繰り返しで
/// 複数の位置が合うときは、修正前と同じ範囲 = ずれ 0 に倒れる）
fn slice_counted(anchor: &[String], mut hist: Vec<String>, take: usize) -> (Vec<String>, usize) {
    let n = hist.len();
    let alen = anchor.len().min(TAIL_ANCHOR);
    let anchor = &anchor[anchor.len() - alen..];
    if anchor.iter().any(|l| !l.trim().is_empty()) {
        for shift in 0..=ALIGN_MARGIN - TAIL_ANCHOR {
            let Some(start) = n.checked_sub(take + shift) else {
                break;
            };
            if start < alen {
                break;
            }
            if hist[start - alen..start]
                .iter()
                .map(|l| l.trim_end())
                .eq(anchor.iter().map(|l| l.trim_end()))
            {
                return (hist.split_off(start), shift);
            }
        }
    }
    let start = n.saturating_sub(take);
    (hist.split_off(start), 0)
}

/// 行群をファイルへ追記する（ヘッダ・ローテーション込み。失敗は握りつぶす =
/// ログ機能の失敗で本体を巻き込まない）
fn append_lines_to(state: &mut PaneState, lines: &[String], max_bytes: u64) {
    if lines.is_empty() {
        return;
    }
    // ローテーション: 上限超過なら現ファイルを `.1` へ退避して新しく始める
    if state.bytes > max_bytes {
        rotate(&state.path);
        state.bytes = 0;
    }
    let Some(dir) = state.path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let existed = state.path.is_file();
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&state.path)
    else {
        return;
    };
    let mut buf = String::new();
    if !existed {
        buf.push_str(&format!("# tako pane log — 開始 {}\n", now_utc()));
    }
    for line in lines {
        buf.push_str(line.trim_end());
        buf.push('\n');
    }
    if f.write_all(buf.as_bytes()).is_ok() {
        state.bytes += buf.len() as u64;
    }
}

/// `path` を `.1` へ退避する（既存 `.1` は上書き = 世代 1 つ）
fn rotate(path: &Path) {
    for generation in (1..=KEEP_GENERATIONS).rev() {
        let to = rotated_path(path, generation);
        let from = if generation == 1 {
            path.to_path_buf()
        } else {
            rotated_path(path, generation - 1)
        };
        if from.is_file() {
            let _ = std::fs::rename(&from, &to);
        }
    }
}

/// ローテーション世代のパス（`x.log` → `x.log.1`）
fn rotated_path(path: &Path, generation: u32) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{generation}"));
    path.with_file_name(name)
}

/// 保存済み tail の末尾アンカーを captured の中から探し、その続き（新規行）を返す。
/// アンカーが見つからなければ captured 全体を返す（取りこぼしより重複を許容する側に倒す。
/// ただし tail が空 = 初回はそのまま全体）
fn split_new_suffix<'a>(tail: &[String], captured: &'a [String]) -> &'a [String] {
    if tail.is_empty() || captured.is_empty() {
        return captured;
    }
    let max_anchor = tail.len().min(TAIL_ANCHOR).min(captured.len());
    // 長いアンカーから順に試す（capture 窓の先頭がアンカー途中を切ることがあるため、
    // 完全長で見つからなければ短い接尾辞で再試行する）
    for anchor_len in (1..=max_anchor).rev() {
        let anchor = &tail[tail.len() - anchor_len..];
        // 空行だけのアンカーは誤マッチしやすいためスキップ
        if !anchor.iter().any(|l| !l.trim().is_empty()) {
            continue;
        }
        // 最後に一致する位置を探す（新しい側の一致を優先）
        for start in (0..=captured.len() - anchor_len).rev() {
            if captured[start..start + anchor_len]
                .iter()
                .map(|l| l.trim_end())
                .eq(anchor.iter().map(|l| l.trim_end()))
            {
                return &captured[start + anchor_len..];
            }
        }
    }
    captured
}

/// tail アンカーを最新行で更新する（最大 TAIL_ANCHOR 行保持）
fn update_tail(tail: &mut Vec<String>, new_lines: &[String]) {
    for line in new_lines {
        tail.push(line.trim_end().to_string());
    }
    if tail.len() > TAIL_ANCHOR {
        tail.drain(..tail.len() - TAIL_ANCHOR);
    }
}

/// ログディレクトリ（`TAKO_PANE_LOG_DIR` 上書き → `<data_dir>/pane-logs`）
pub fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TAKO_PANE_LOG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    crate::paths::data_dir().map(|d| d.join("pane-logs"))
}

/// ログファイル 1 件の情報（`tako logs list` 用）
#[derive(Debug, Clone)]
pub struct LogFileInfo {
    pub path: PathBuf,
    pub pane: Option<u64>,
    pub tab: Option<u64>,
    pub size: u64,
    /// 最終更新（unix 秒）
    pub modified: i64,
}

/// ディレクトリ内のログファイルを列挙する（更新時刻の新しい順）
pub fn list_files(dir: &Path) -> Vec<LogFileInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<LogFileInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.contains(".log") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Some(LogFileInfo {
                pane: parse_marker(name, "_pane"),
                tab: parse_marker(name, "_tab"),
                size: meta.len(),
                modified,
                path,
            })
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.modified));
    files
}

/// ペイン ID に対応する最新のログファイル（`.1` 世代を除く現行ファイル優先）
pub fn latest_for_pane(dir: &Path, pane: u64) -> Option<PathBuf> {
    let files = list_files(dir);
    files
        .iter()
        .find(|f| f.pane == Some(pane) && f.path.extension().is_some_and(|e| e == "log"))
        .or_else(|| files.iter().find(|f| f.pane == Some(pane)))
        .map(|f| f.path.clone())
}

/// ファイル末尾の `max_lines` 行を返す（ペインあたり上限があるため全読みで足りる）。
///
/// 読めなかった理由は OS のエラーのまま返す。利用者へ出す断りの文（日英）は呼び手
/// （`tako logs read` = tako-control の `SessionRefusal::LogUnreadable`）が表示言語で組む（#2365）
pub fn read_tail(path: &Path, max_lines: usize) -> std::io::Result<String> {
    let content = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = content.lines().collect();
    let skip = lines.len().saturating_sub(max_lines);
    Ok(lines[skip..].join("\n"))
}

/// ディレクトリ全体の上限強制（純粋なファイル操作部。open 中のファイルは除外）。
/// 削除したファイル数を返す
pub fn enforce_total_cap_at(dir: &Path, max_total_bytes: u64, exclude: &[PathBuf]) -> usize {
    let files = list_files(dir);
    let mut total: u64 = files.iter().map(|f| f.size).sum();
    if total <= max_total_bytes {
        return 0;
    }
    let mut removed = 0;
    // list_files は新しい順 → 逆順（古い順）に削る
    for file in files.iter().rev() {
        if total <= max_total_bytes {
            break;
        }
        if exclude.contains(&file.path) {
            continue;
        }
        if std::fs::remove_file(&file.path).is_ok() {
            total = total.saturating_sub(file.size);
            removed += 1;
        }
    }
    removed
}

/// ファイル名の `_pane12` / `_tab3` マーカーから数値を取り出す
fn parse_marker(name: &str, marker: &str) -> Option<u64> {
    let start = name.find(marker)? + marker.len();
    let digits: String = name[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// ログファイル名を組み立てる: `<日時>_tab<T>_pane<P>[_<スラグ>].log`
fn file_name(timestamp: &str, tab: u64, pane: u64, label: Option<&str>) -> String {
    let slug = label.map(sanitize_slug).filter(|s| !s.is_empty());
    match slug {
        Some(s) => format!("{timestamp}_tab{tab}_pane{pane}_{s}.log"),
        None => format!("{timestamp}_tab{tab}_pane{pane}.log"),
    }
}

/// ラベルをファイル名に安全なスラグへ変換する（英数と `.-` のみ・最大 40 文字）
fn sanitize_slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if out.len() >= 40 {
            break;
        }
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`（外部クレートを増やさない自前変換。diag.rs と同方式）
fn format_utc(secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = civil_utc(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// unix 秒 → `YYYYmmdd-HHMMSS`（ファイル名用）
fn format_utc_compact(secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = civil_utc(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

fn civil_utc(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, tod / 3_600, (tod % 3_600) / 60, tod % 60)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_utc() -> String {
    format_utc(unix_now())
}

fn now_utc_compact() -> String {
    format_utc_compact(unix_now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-pane-log-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    fn meta(tab: u64, label: &str) -> PaneLogMeta {
        PaneLogMeta {
            tab,
            label: Some(label.into()),
        }
    }

    #[test]
    fn 増分行が追記されヘッダとファイル名が正しい() {
        let dir = temp_dir("append");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        mgr.apply(
            7,
            &meta(3, "worker:tako:112-log"),
            PaneObservation {
                bytes: 0,
                history: 2,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::Counted {
                    lines: lines(&["one", "two"]),
                    delta: 2,
                },
            },
        );
        let files = list_files(&dir);
        assert_eq!(files.len(), 1);
        let name = files[0].path.file_name().unwrap().to_str().unwrap();
        assert!(name.contains("_tab3_pane7_"), "{name}");
        assert!(name.contains("worker-tako-112-log"), "{name}");
        assert_eq!(files[0].pane, Some(7));
        assert_eq!(files[0].tab, Some(3));
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(content.starts_with("# tako pane log"));
        assert!(content.contains("one\ntwo\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn alt_screen遷移でマーカーが入り内容は書かれない() {
        let dir = temp_dir("alt");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        let m = meta(1, "claude");
        mgr.apply(
            1,
            &m,
            PaneObservation {
                bytes: 0,
                history: 0,
                history_limit: 10_000,
                alt_screen: true,
                chunk: ChunkKind::None,
            },
        );
        mgr.apply(
            1,
            &m,
            PaneObservation {
                bytes: 0,
                history: 0,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::None,
            },
        );
        let files = list_files(&dir);
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(content.contains("[TUI 実行中"));
        assert!(content.contains("[TUI 終了"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 省略マーカーと巻き戻りリセット() {
        let dir = temp_dir("skip");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        let m = meta(1, "x");
        mgr.apply(
            5,
            &m,
            PaneObservation {
                bytes: 0,
                history: 1000,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::Counted {
                    lines: lines(&["tail-1", "tail-2"]),
                    delta: 500,
                },
            },
        );
        assert_eq!(mgr.logged_history(5), Some(1000));
        // clear-history 相当（巻き戻り）→ 基準リセット
        mgr.apply(
            5,
            &m,
            PaneObservation {
                bytes: 0,
                history: 0,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::None,
            },
        );
        assert_eq!(mgr.logged_history(5), Some(0));
        let files = list_files(&dir);
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(content.contains("--- [省略: 498 行] ---"));
        assert!(content.contains("tail-1\ntail-2\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn オーバーラップ照合で新規行だけ追記される() {
        // 履歴カウンタ飽和時（history-limit 到達）の重複除去
        let dir = temp_dir("overlap");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        let m = meta(1, "sat");
        mgr.apply(
            9,
            &m,
            PaneObservation {
                bytes: 0,
                history: 10_000,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::Counted {
                    lines: lines(&["L-1", "L-2", "L-3"]),
                    delta: 3,
                },
            },
        );
        // 飽和後の capture: 既存 L-2, L-3 + 新規 L-4, L-5
        mgr.apply(
            9,
            &m,
            PaneObservation {
                bytes: 0,
                history: 10_000,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::Overlap {
                    captured: lines(&["L-2", "L-3", "L-4", "L-5"]),
                },
            },
        );
        let files = list_files(&dir);
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(content.contains("L-1\nL-2\nL-3\nL-4\nL-5\n"));
        assert_eq!(content.matches("L-3").count(), 1, "重複しない: {content}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_new_suffixの照合規則() {
        let tail = lines(&["a", "b", "c"]);
        // capture 窓がアンカー途中から始まる場合は短い接尾辞（b,c）で照合する
        let captured = lines(&["b", "c", "d", "e"]);
        assert_eq!(split_new_suffix(&tail, &captured), &lines(&["d", "e"])[..]);
        // 完全一致がある場合
        let captured2 = lines(&["x", "a", "b", "c", "d"]);
        assert_eq!(split_new_suffix(&tail, &captured2), &lines(&["d"])[..]);
        // 一致が全く無ければ全体を返す（取りこぼしより重複を許容）
        let captured4 = lines(&["p", "q"]);
        assert_eq!(split_new_suffix(&tail, &captured4), &captured4[..]);
        // 空 tail は全体
        assert_eq!(split_new_suffix(&[], &captured), &captured[..]);
        // 空行だけの tail は誤マッチ防止で全体
        let blank_tail = lines(&["", "  "]);
        assert_eq!(split_new_suffix(&blank_tail, &captured), &captured[..]);
        // 同一アンカーが複数あれば最後（新しい側）を採る
        let tail2 = lines(&["m"]);
        let captured3 = lines(&["m", "1", "m", "2"]);
        assert_eq!(split_new_suffix(&tail2, &captured3), &lines(&["2"])[..]);
    }

    #[test]
    fn ローテーションと世代() {
        let dir = temp_dir("rotate");
        let config = PaneLogConfig {
            max_bytes_per_pane: 64,
            ..Default::default()
        };
        let mut mgr = PaneLogManager::new(dir.clone(), config);
        let m = meta(1, "rot");
        for i in 0..30 {
            mgr.apply(
                2,
                &m,
                PaneObservation {
                    bytes: 0,
                    history: i + 1,
                    history_limit: 10_000,
                    alt_screen: false,
                    chunk: ChunkKind::Counted {
                        lines: lines(&[&format!("line-{i:04} 0123456789")]),
                        delta: 1,
                    },
                },
            );
        }
        let current = mgr.path_of(2).unwrap();
        assert!(current.is_file());
        let rotated = rotated_path(&current, 1);
        assert!(rotated.is_file(), "1 世代残る");
        // 現行ファイルは上限の近傍で止まる（上限 + 1 追記分以内）
        assert!(std::fs::metadata(&current).unwrap().len() < 200);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn クローズフラッシュで可視画面とマーカーが残り状態が消える() {
        let dir = temp_dir("close");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        let m = meta(4, "sh");
        mgr.apply(
            11,
            &m,
            PaneObservation {
                bytes: 0,
                history: 1,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::Counted {
                    lines: lines(&["scrolled"]),
                    delta: 1,
                },
            },
        );
        mgr.flush_close(
            11,
            &m,
            &lines(&["visible-1", "visible-2", "", ""]),
            CloseOrigin::Keyboard,
            None,
        );
        assert!(mgr.path_of(11).is_none(), "状態が破棄される");
        let files = list_files(&dir);
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(content.contains("scrolled\nvisible-1\nvisible-2\n"));
        assert!(content.contains("--- [クローズ: close:kbd"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #566: クローズマーカーの発生源表記。GUI × / キーバインド /
    /// CLI・MCP / プロセス終了を事後に区別できることを固定する
    #[test]
    fn クローズマーカーに発生源が入る() {
        assert_eq!(CloseOrigin::Keyboard.marker(), "close:kbd");
        assert_eq!(CloseOrigin::PaneButton.marker(), "close:gui");
        assert_eq!(CloseOrigin::TabButton.marker(), "close:gui-tab");
        assert_eq!(CloseOrigin::Cli.marker(), "close:dispatch(cli)");
        assert_eq!(CloseOrigin::Mcp.marker(), "close:dispatch(mcp)");
        assert_eq!(CloseOrigin::Dispatch.marker(), "close:dispatch");
        assert_eq!(CloseOrigin::Internal.marker(), "close:internal");
        assert_eq!(CloseOrigin::AutoClose.marker(), "close:auto");
        assert_eq!(CloseOrigin::AppQuit.marker(), "close:app-quit");
        assert_eq!(CloseOrigin::ProcessExit.marker(), "exit");
        // 全て相異なる = 事後の区別がつく
        let all = [
            CloseOrigin::Keyboard,
            CloseOrigin::PaneButton,
            CloseOrigin::TabButton,
            CloseOrigin::Cli,
            CloseOrigin::Mcp,
            CloseOrigin::Dispatch,
            CloseOrigin::Internal,
            CloseOrigin::AutoClose,
            CloseOrigin::AppQuit,
            CloseOrigin::ProcessExit,
        ];
        let uniq: std::collections::HashSet<&str> = all.iter().map(|o| o.marker()).collect();
        assert_eq!(uniq.len(), all.len(), "発生源ごとに別のマーカーになる");
    }

    #[test]
    fn dispatch_のマーカーにだけ_caller_が付く() {
        assert_eq!(
            CloseOrigin::Mcp.marker_with_caller(Some("orchestrator-master:takodev")),
            "close:dispatch(mcp, caller=orchestrator-master:takodev)"
        );
        assert_eq!(
            CloseOrigin::Dispatch.marker_with_caller(Some("solo")),
            "close:dispatch(caller=solo)"
        );
        // GUI / キーバインド / プロセス終了は caller を持たない
        assert_eq!(
            CloseOrigin::Keyboard.marker_with_caller(Some("orchestrator-master")),
            "close:kbd"
        );
        assert_eq!(
            CloseOrigin::ProcessExit.marker_with_caller(Some("x")),
            "exit"
        );
        // auto_close は `--wait` の聞き直しが引き金でも「人が閉じた」形にしない（#1778）
        assert_eq!(
            CloseOrigin::AutoClose.marker_with_caller(Some("orchestrator-master")),
            "close:auto"
        );
        // 空・空白のみは付かない
        assert_eq!(
            CloseOrigin::Cli.marker_with_caller(None),
            "close:dispatch(cli)"
        );
        assert_eq!(
            CloseOrigin::Cli.marker_with_caller(Some("   ")),
            "close:dispatch(cli)"
        );
    }

    #[test]
    fn caller_はマーカー書式を壊さないよう正規化される() {
        // 改行・括弧・空白は潰す（1 行 1 マーカーの前提と括弧の対応を守る）
        assert_eq!(
            CloseOrigin::Cli.marker_with_caller(Some("bad)\nrole (x")),
            "close:dispatch(cli, caller=bad__role__x)"
        );
        // 長い caller は打ち切る
        let long = "a".repeat(200);
        let marker = CloseOrigin::Cli.marker_with_caller(Some(&long));
        assert!(marker.contains(&"a".repeat(64)));
        assert!(!marker.contains(&"a".repeat(65)));
        assert!(!marker.contains('\n'));
    }

    /// Issue #566: 発生源が実際にログファイルへ書かれる（マーカーの結線確認）
    #[test]
    fn クローズマーカーの発生源がファイルへ書かれる() {
        let dir = temp_dir("close-origin");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        let m = meta(9, "master");
        mgr.flush_close(
            21,
            &m,
            &lines(&["last-line"]),
            CloseOrigin::Mcp,
            Some("orchestrator-master:tako"),
        );
        let files = list_files(&dir);
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(
            content.contains("--- [クローズ: close:dispatch(mcp, caller=orchestrator-master:tako)"),
            "発生源つきマーカーが書かれる: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #599: 書いたマーカーから発生源を読み戻せる。判定側が
    /// caller の有無に振り回されないための土台（前方一致で判定していたため
    /// `TAKO_ORCHESTRATOR_ROLE` のある worker ペインから回すと必ず落ちていた）
    #[test]
    fn クローズマーカーから発生源を読み戻せる() {
        // caller 無し / 有りのどちらも「発生源そのもの」が返る
        for caller in [None, Some("worker:tako:599")] {
            let reason = CloseOrigin::Cli.marker_with_caller(caller);
            let line =
                format!("{CLOSE_MARKER_PREFIX}{reason} 2026-07-27T10:23:42Z{CLOSE_MARKER_SUFFIX}");
            assert_eq!(close_marker_reason(&line), Some(reason.as_str()), "{line}");
        }
        // 発生源の完全一致で見れば close:gui が close:gui-tab を拾わない
        let tab = CloseOrigin::TabButton.marker_with_caller(None);
        let line = format!("{CLOSE_MARKER_PREFIX}{tab} 2026-07-27T10:23:42Z{CLOSE_MARKER_SUFFIX}");
        assert_eq!(close_marker_reason(&line), Some("close:gui-tab"));
        assert_ne!(
            close_marker_reason(&line),
            Some(CloseOrigin::PaneButton.marker())
        );
        // マーカー以外の行は None
        assert_eq!(close_marker_reason("普通のログ行"), None);
        assert_eq!(close_marker_reason("--- [クローズ: close:kbd] ---"), None);
    }

    #[test]
    fn 全体上限で古いファイルから削除される() {
        let dir = temp_dir("cap");
        // 3 ファイル（各 ~100B）を作り、上限 150B で古い 2 つが消える
        for (i, name) in [
            "20260101-000000_tab1_pane1_old.log",
            "20260102-000000_tab1_pane2_mid.log",
            "20260103-000000_tab1_pane3_new.log",
        ]
        .iter()
        .enumerate()
        {
            let path = dir.join(name);
            std::fs::write(&path, "x".repeat(100)).unwrap();
            // mtime を過去に倒す代わりに書き込み順で担保できないため filetime を使わず、
            // modified が同一でも size 合計での削減が働くことだけ確認する
            let _ = i;
        }
        let removed =
            enforce_total_cap_at(&dir, 150, &[dir.join("20260103-000000_tab1_pane3_new.log")]);
        assert!(removed >= 1, "少なくとも 1 つ削除される");
        let total: u64 = list_files(&dir).iter().map(|f| f.size).sum();
        assert!(total <= 200, "上限近傍まで削減: {total}");
        // 除外指定（open 中）のファイルは残る
        assert!(dir.join("20260103-000000_tab1_pane3_new.log").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 無効時は書かないが基準は追従する() {
        let dir = temp_dir("disabled");
        let config = PaneLogConfig {
            enabled: false,
            ..Default::default()
        };
        let mut mgr = PaneLogManager::new(dir.clone(), config);
        let m = meta(1, "off");
        mgr.apply(
            3,
            &m,
            PaneObservation {
                bytes: 0,
                history: 42,
                history_limit: 10_000,
                alt_screen: false,
                chunk: ChunkKind::Counted {
                    lines: lines(&["secret"]),
                    delta: 42,
                },
            },
        );
        assert_eq!(mgr.logged_history(3), Some(42), "基準は進む");
        assert!(list_files(&dir).is_empty(), "ファイルは作られない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_tailとlatest_for_pane() {
        let dir = temp_dir("read");
        let path = dir.join("20260101-000000_tab1_pane5_a.log");
        std::fs::write(&path, "1\n2\n3\n4\n5\n").unwrap();
        assert_eq!(read_tail(&path, 2).unwrap(), "4\n5");
        assert_eq!(read_tail(&path, 99).unwrap(), "1\n2\n3\n4\n5");
        assert_eq!(latest_for_pane(&dir, 5), Some(path));
        assert_eq!(latest_for_pane(&dir, 6), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn スラグとマーカーのパース() {
        assert_eq!(sanitize_slug("worker:tako:112-log"), "worker-tako-112-log");
        assert_eq!(sanitize_slug("日本語 ラベル!"), "");
        assert_eq!(sanitize_slug("a b/c"), "a-b-c");
        assert_eq!(parse_marker("x_tab3_pane12_y.log", "_pane"), Some(12));
        assert_eq!(parse_marker("x_tab3_pane12_y.log", "_tab"), Some(3));
        assert_eq!(parse_marker("nomarker.log", "_pane"), None);
        // 復元ペインのシード
        let dir = temp_dir("seed");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        mgr.seed_history(8, &meta(1, "s"), 123);
        assert_eq!(mgr.scan_baseline(8), Some((123, 0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn counted(history: usize, raw: &[&str], delta: usize) -> PaneObservation {
        PaneObservation {
            history,
            history_limit: 10_000,
            bytes: 0,
            alt_screen: false,
            chunk: ChunkKind::Counted {
                lines: lines(raw),
                delta,
            },
        }
    }

    /// #2018: 器の取り込みは background で走るので、閉じた後に届いた観測が状態を作り直して
    /// 新しいファイルへ同じ行を書かない。2 回目の close もファイルを増やさない
    #[test]
    fn 閉じたペインには後から届いた観測も二度目のクローズも書かない() {
        let dir = temp_dir("closed");
        let mut mgr = PaneLogManager::new(dir.clone(), PaneLogConfig::default());
        let m = meta(2, "w");
        mgr.apply(21, &m, counted(1, &["a"], 1));
        mgr.flush_close(21, &m, &lines(&["v"]), CloseOrigin::Cli, None);
        mgr.apply(21, &m, counted(3, &["a", "b"], 2));
        mgr.flush_close(21, &m, &lines(&["v"]), CloseOrigin::ProcessExit, None);
        let files = list_files(&dir);
        assert_eq!(files.len(), 1, "閉じた後にファイルが増えた: {files:?}");
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert_eq!(content.matches("--- [クローズ: ").count(), 1, "{content}");
        assert!(
            !content.contains("\nb\n"),
            "閉じた後の観測が書かれた: {content}"
        );
        assert!(mgr.path_of(21).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 読む行数の上限は経過時間に比例し器の保持上限で頭打ち() {
        assert_eq!(capture_budget(None), CAPTURE_CHUNK, "初回は 1 tick 分");
        assert_eq!(capture_budget(Some(Duration::ZERO)), CAPTURE_CHUNK);
        assert_eq!(capture_budget(Some(POLL_TICK)), CAPTURE_CHUNK);
        assert_eq!(
            capture_budget(Some(Duration::from_millis(2_100))),
            CAPTURE_CHUNK * 2
        );
        // 10 秒ごとに読むなら 100 行/秒を 1 行も省略しない（1000 行 < 2000 行）
        assert_eq!(capture_budget(Some(BUSY_INTERVAL)), CAPTURE_CHUNK * 5);
        assert_eq!(capture_budget(Some(Duration::from_secs(3_600))), 10_000);
    }

    /// #2018: 出力が続く間は BUSY_INTERVAL ごと、止まったら次の tick、無ければ保険の間隔
    #[test]
    fn 器の取り込みの間合い() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut pace = BackendPace::default();
        let due = |budget, output| Some(IngestDue { budget, output });
        // 初回は必ず取り込む（再起動の間に積もった分）
        assert_eq!(pace.decide(0, at(0), false), due(CAPTURE_CHUNK, true));
        // 出力が無い間は保険の間隔まで見送る（保険は出力なし = 履歴を先読みしない）
        assert_eq!(pace.decide(0, at(2), false), None);
        assert_eq!(pace.decide(0, at(28), false), None);
        assert_eq!(
            pace.decide(0, at(30), false),
            due(capture_budget(Some(IDLE_INSURANCE)), false)
        );
        // 出力が続く間（合図が tick ごとに動く）は BUSY_INTERVAL ごと
        let mut a = 0;
        let mut taken = Vec::new();
        for s in (32..=70).step_by(2) {
            a += 7;
            if let Some(d) = pace.decide(a, at(s), false) {
                taken.push((s, d.budget, d.output));
            }
        }
        let busy = (CAPTURE_CHUNK * 5, true);
        assert_eq!(
            taken,
            vec![
                (40, busy.0, busy.1),
                (50, busy.0, busy.1),
                (60, busy.0, busy.1),
                (70, busy.0, busy.1)
            ],
            "出力中は 10 秒ごと"
        );
        // 出力が止まった: 合図が動かなくなった最初の tick で取り込む
        a += 3;
        assert_eq!(pace.decide(a, at(72), false), None, "まだ出力中に見える");
        assert_eq!(
            pace.decide(a, at(74), false),
            due(capture_budget(Some(Duration::from_secs(4))), true),
            "止まったら次の tick"
        );
        assert_eq!(pace.decide(a, at(76), false), None, "取り込んだ後は静か");
    }

    /// #2018: 出力の無い器は、他の器の取り込みに相乗りして確かめる（単独の保険を起こさない）
    #[test]
    fn 出力の無い器は他の取り込みに相乗りする() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut pace = BackendPace::default();
        assert_eq!(
            pace.ride_along(at(0), false),
            None,
            "一度も取り込んでいない"
        );
        assert!(pace.decide(5, at(0), false).is_some());
        assert_eq!(pace.decide(5, at(10), false), None);
        assert_eq!(
            pace.ride_along(at(10), false),
            None,
            "保険の半分に届いていない"
        );
        assert_eq!(pace.decide(5, at(16), false), None);
        assert_eq!(
            pace.ride_along(at(16), false),
            Some(IngestDue {
                budget: capture_budget(Some(Duration::from_secs(16))),
                output: false
            })
        );
        // 相乗りした時刻から数え直す = 単独の保険（30 秒）はまだ来ない
        assert_eq!(pace.decide(5, at(30), false), None);
        // 出力があった器は相乗りしない（出力の側の間合い = decide が決める）
        let mut busy = pace;
        assert_eq!(busy.decide(9, at(18), false), None);
        assert_eq!(busy.ride_along(at(18), false), None);
        // A/B は相乗りしない（毎 tick 取り込む）
        assert_eq!(pace.ride_along(at(60), true), None);
    }

    #[test]
    fn 旧挙動は毎_tick_1_tick_分() {
        let t0 = Instant::now();
        let mut pace = BackendPace::default();
        for s in 0..5u64 {
            let d = pace.decide(0, t0 + Duration::from_secs(s * 2), true);
            assert_eq!(d.map(|d| d.budget), Some(CAPTURE_CHUNK));
        }
    }

    fn probe(history: usize, bytes: u64, alternate: bool) -> crate::tmux::PaneLogProbe {
        crate::tmux::PaneLogProbe {
            history,
            limit: 10_000,
            bytes,
            alternate,
        }
    }

    fn numbered(from: usize, to: usize) -> Vec<String> {
        (from..to).map(|i| format!("L{i:05}")).collect()
    }

    #[test]
    fn 読む範囲は増分と余白で決まり旧挙動は余白なし() {
        let p = probe(120, 0, false);
        assert_eq!(
            plan_backend_capture(&p, 100, 0, 400, false),
            BackendCapture::Counted {
                delta: 20,
                take: 20,
                margin: ALIGN_MARGIN
            }
        );
        assert_eq!(
            plan_backend_capture(&p, 100, 0, 400, true),
            BackendCapture::Counted {
                delta: 20,
                take: 20,
                margin: 0
            },
            "A/B は修正前と同じ範囲"
        );
        // 読み切れない（省略が出る）ときは揃えない
        assert_eq!(
            plan_backend_capture(&p, 0, 0, 100, false),
            BackendCapture::Counted {
                delta: 120,
                take: 100,
                margin: 0
            }
        );
        assert_eq!(
            plan_backend_capture(&p, 120, 0, 400, false),
            BackendCapture::Skip
        );
        assert_eq!(
            plan_backend_capture(&p, 130, 0, 400, false),
            BackendCapture::Skip,
            "巻き戻り"
        );
        // 飽和: 行数が増えずバイト数だけ動く
        let full = probe(10_000, 9, false);
        assert_eq!(
            plan_backend_capture(&full, 10_000, 8, 2_000, false),
            BackendCapture::Overlap { take: 2_000 }
        );
        assert_eq!(
            plan_backend_capture(&full, 10_000, 9, 2_000, false),
            BackendCapture::Skip
        );
        assert_eq!(
            plan_backend_capture(&probe(10_000, 9, true), 10_000, 8, 2_000, false),
            BackendCapture::Skip,
            "TUI 中は照合しない"
        );
    }

    /// #2018: probe と capture の間に伸びた行を、取り込み済みの末尾で揃えて欠け・重複なく切り出す
    #[test]
    fn probeの後に伸びた行を末尾の印で揃える() {
        let anchor = numbered(92, 100); // 取り込み済みの末尾 = L00092..L00099
        let p = probe(120, 0, false); // probe の時点で 100 → 120
        let plan = plan_backend_capture(&p, 100, 0, 400, false);
        // capture の時点ではさらに 3 行伸びていた（L00120..L00122）
        let hist = numbered(123 - plan.lines_to_read(), 123);
        let obs = backend_observation(&p, plan, &anchor, hist);
        assert_eq!(obs.history, 123, "伸びた分まで基準を進める");
        match obs.chunk {
            ChunkKind::Counted { lines, delta } => {
                assert_eq!(lines, numbered(100, 123), "L00100 から欠け・重複なし");
                assert_eq!(delta, 23);
            }
            other => panic!("Counted でない: {other:?}"),
        }
        // 1 回の tmux で多めに読んだ（増分より長い）履歴でも同じ
        let hist = numbered(0, 120);
        match backend_observation(&p, plan, &anchor, hist).chunk {
            ChunkKind::Counted { lines, .. } => assert_eq!(lines, numbered(100, 120)),
            other => panic!("Counted でない: {other:?}"),
        }
    }

    #[test]
    fn 揃えられないときは修正前と同じ末尾の範囲() {
        let p = probe(120, 0, false);
        let plan = plan_backend_capture(&p, 100, 0, 400, false);
        let hist = numbered(123 - plan.lines_to_read(), 123);
        // 印が無い（初回）・空行だけ・見つからない
        for anchor in [
            vec![],
            vec![String::new(), " ".into()],
            numbered(5000, 5008),
        ] {
            match backend_observation(&p, plan, &anchor, hist.clone()).chunk {
                ChunkKind::Counted { lines, delta } => {
                    assert_eq!(lines, numbered(103, 123), "末尾 take 行");
                    assert_eq!(delta, 20);
                }
                other => panic!("Counted でない: {other:?}"),
            }
        }
        // 余白なし（A/B）は渡された履歴の末尾 take 行
        let legacy = plan_backend_capture(&p, 100, 0, 400, true);
        match backend_observation(&p, legacy, &numbered(92, 100), numbered(0, 120)).chunk {
            ChunkKind::Counted { lines, .. } => assert_eq!(lines, numbered(100, 120)),
            other => panic!("Counted でない: {other:?}"),
        }
        // 同じ行の繰り返しで複数の位置が合うときは、ずれ 0（修正前と同じ範囲）に倒れる
        let same = vec!["x".to_string(); 60];
        match backend_observation(&p, plan, &vec!["x".to_string(); 8], same).chunk {
            ChunkKind::Counted { lines, delta } => assert_eq!((lines.len(), delta), (20, 20)),
            other => panic!("Counted でない: {other:?}"),
        }
    }

    fn dump(history: usize, screen: usize) -> crate::tmux_backend::PaneDiedDump {
        crate::tmux_backend::PaneDiedDump {
            probe: probe(history, 0, false),
            history: numbered(0, history),
            screen: numbered(history, history + screen),
        }
    }

    /// #2085: 器が自ら閉じたときの書き出しから、最後の取り込みの後の行と画面を作る
    #[test]
    fn 自ら閉じた器の書き出しは最後の取り込みの後の行と画面になる() {
        // 100 行まで取り込み済み → 書き出しは履歴 281 行 + 画面 20 行
        let (obs, screen) = pane_died_observation(dump(281, 20), 100, 0, &numbered(92, 100), 4000);
        assert_eq!(screen, numbered(281, 301));
        assert_eq!(obs.history, 281);
        match obs.chunk {
            ChunkKind::Counted { lines, delta } => {
                assert_eq!(lines, numbered(100, 281));
                assert_eq!(delta, 181);
            }
            other => panic!("Counted でない: {other:?}"),
        }
        // 取り込みの後に履歴が伸びていない（出力が止まってから終わった）: 何も足さず画面だけ
        let (obs, screen) = pane_died_observation(dump(281, 20), 281, 0, &numbered(273, 281), 4000);
        assert_eq!(screen.len(), 20);
        match obs.chunk {
            ChunkKind::Counted { lines, delta } => assert_eq!((lines.len(), delta), (0, 0)),
            other => panic!("Counted でない: {other:?}"),
        }
        // 一度も取り込んでいない（初回の tick より先に終わった）: 履歴を頭から
        let (obs, _) = pane_died_observation(dump(50, 20), 0, 0, &[], 4000);
        match obs.chunk {
            ChunkKind::Counted { lines, .. } => assert_eq!(lines, numbered(0, 50)),
            other => panic!("Counted でない: {other:?}"),
        }
    }

    /// 洪水の上限（時間あたり）は閉じる直前の取り込みと同じ: 超えた分は省略マーカーになる
    #[test]
    fn 自ら閉じた器の書き出しも読む行数の上限に従う() {
        let (obs, _) = pane_died_observation(dump(3000, 20), 100, 0, &numbered(92, 100), 400);
        match obs.chunk {
            ChunkKind::Counted { lines, delta } => {
                assert_eq!(delta, 2900);
                assert_eq!(lines, numbered(2600, 3000), "末尾 400 行");
            }
            other => panic!("Counted でない: {other:?}"),
        }
        // 履歴が巻き戻った（clear-history）なら読まない
        let (obs, screen) = pane_died_observation(dump(10, 20), 100, 0, &numbered(92, 100), 400);
        assert!(matches!(obs.chunk, ChunkKind::None));
        assert_eq!(screen.len(), 20);
    }
}
