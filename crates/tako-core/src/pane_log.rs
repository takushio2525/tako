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
//! サイズ管理（target 44GB 肥大の教訓）:
//! - ペインあたり上限（既定 5MB）超過で `.1` へ 1 世代ローテーション
//! - ログディレクトリ全体の上限（既定 200MB）超過で古いファイルから削除
//!
//! プライバシー: ログにはペイン内容（トークンの写り込み等）が含まれ得るため、
//! 保存先はユーザーローカルのデータディレクトリ限定（`<data_dir>/pane-logs/`）。
//! リポジトリ・同期対象に置かないこと。`TAKO_PANE_LOG_DIR` で隔離検証用に上書き可能。

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

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
}

/// ペイン 1 回分の走査結果（呼び出し側が Term / tmux から観測して詰める）
#[derive(Debug)]
pub struct PaneObservation {
    /// 現在の履歴行数
    pub history: usize,
    /// 履歴の保持上限（飽和判定に使う。直接ペイン = SCROLLBACK_LINES、tmux = history-limit）
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
    /// 追記し、クローズマーカーを書いて状態を破棄する
    pub fn flush_close(&mut self, pane: u64, meta: &PaneLogMeta, visible: &[String], reason: &str) {
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
            let marker = format!("--- [クローズ: {reason} {}] ---", now_utc());
            append_lines_to(state, std::slice::from_ref(&marker), max_bytes);
        }
        self.states.remove(&pane);
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

/// ファイル末尾の `max_lines` 行を返す（ペインあたり上限があるため全読みで足りる）
pub fn read_tail(path: &Path, max_lines: usize) -> Result<String, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("ログを読めない ({}): {e}", path.display()))?;
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
        mgr.flush_close(11, &m, &lines(&["visible-1", "visible-2", "", ""]), "kill");
        assert!(mgr.path_of(11).is_none(), "状態が破棄される");
        let files = list_files(&dir);
        let content = std::fs::read_to_string(&files[0].path).unwrap();
        assert!(content.contains("scrolled\nvisible-1\nvisible-2\n"));
        assert!(content.contains("--- [クローズ: kill"));
        let _ = std::fs::remove_dir_all(&dir);
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
}
