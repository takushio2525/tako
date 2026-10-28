//! sessions — セッションカタログ（Issue #112 A）
//!
//! claude が `~/.claude/projects/<dir>/<uuid>.jsonl` に永続化している会話ログへの
//! **参照**（session_id）とメタデータだけを記録するインデックス。会話本文は保存しない
//! （二重保存の回避。Issue #112 の設計方針）。解決する問題は保存ではなく**発見性**:
//! `claude --resume` の一覧は起動ディレクトリ単位 + 冒頭プロンプト抜粋のみで、
//! tako の worker は冒頭が全部同一テンプレのため見分けられない。
//!
//! 記録の流れ:
//! 1. spawn / master / solo 起動時: session_id はまだ無いため **pending 記録**
//!    （project / label / prompt 由来の Issue 番号等）を残す（spawn は dispatch 側、
//!    master / solo はペイン role からの解析で足りるため省略）
//! 2. GUI の定期スキャン（`claude agents --json` × pid 祖先辿り）が session_id を
//!    検出した時点で pending をエントリへ**昇格**し、ペインのメタ情報と統合する
//! 3. `tako sessions list / show / resume` と MCP `tako_sessions` が参照する
//!
//! ## 突き合わせキー（#728）
//!
//! pending と検出結果を結ぶキーは**器（tmux / psmux）のセッション名**。
//! ただし器の導入は任意なので（Windows の psmux・macOS の tmux はどちらも
//! 「入れれば深く復元できる」もの）、器が無い構成では**ペイン ID** に倒す。
//! 器が無いときペインのシェルは tako-app の直接の子なので、claude の対応付けは
//! `TerminalSession::child_pid` からの pid 祖先辿りになる（`agents` の #592 経路）。
//! カタログ本体・復元手順は器に一切依存しない（復元は tako 自身のペイン生成 +
//! `claude --resume` であって、器への送出は要らない）
//!
//! ファイルは `<data_dir>/sessions.yaml`（`TAKO_SESSIONS_FILE` で上書き可。隔離検証用）。
//! 書き込みは config_io（排他 flock + アトミック書き込み + 世代バックアップ。#169）。
//!
//! ## 走査ごとの書き込みの間引き（#2020）
//!
//! GUI の走査は 30 秒ごとに検出した会話の `last_seen_at` を今の時刻へ進める。修正前は
//! それだけで毎回「全体を YAML 化 → 本体を `.bak.1` へ丸ごとコピー（3 世代の回転）→
//! tmp へ書いて rename」をしていた（500 件・395 KB で約 0.8 MB × 2,880 回/日）。
//! いまは [`LastSeenThrottle`] が `last_seen_at` だけの変化を溜め、次のどれかで書く:
//!
//! - `last_seen_at` 以外の変化（新しい会話・pane / tab / 器 / cwd / model 等・pending・GC）
//!   = 即時に書き、`.bak` も回す（#169 の「消失時に直前へ戻せる」はこの変化のためにある）
//! - **検出した会話の集合が変わった**（会話が始まった / 終わった / 戻ってきた）
//!   = 即時に書く。溜めていた各会話の「最後に見えた時刻」をそのまま書くので、
//!   書いた時点のファイルは修正前と同じ内容になり、並び順（`tako sessions list`）・
//!   GC（古い順に落とす）・ペインからの逆引き（同じペインの最新の世代。#466）が変わらない
//! - 最後に書いてから [`LAST_SEEN_FLUSH_SECS`] が過ぎた（時刻の値そのものの鮮度）
//! - GUI の終了（[`flush_last_seen`]。`on_app_quit` から）
//!
//! `last_seen_at` だけの書き込みは `.bak` を回さない（中身の会話も項目も変わらないので
//! 退避する意味が無い）。A/B は `TAKO_2020_LEGACY=1`（毎回 `.bak` つきで書く修正前）

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use tako_core::agent_support::Agent;
use tako_core::platform::support::Platform;

/// カタログに保持する最大エントリ数（last_seen_at の新しい順に残す）
const MAX_ENTRIES: usize = 500;

/// pending 記録の保持期間（秒）。session_id が検出されないまま古びたものを掃除する
/// （codex / agy worker は claude の session 検出に乗らないため、寿命付きで残す）
const PENDING_TTL_SECS: i64 = 7 * 24 * 3600;

/// `last_seen_at` だけの変化を溜めておける最長の時間（秒。#2020）。
///
/// 並び順・GC・ペインの逆引きは「検出集合が変わった時点で書く」ことで修正前と同じに
/// 保たれるので、この窓が効くのは**時刻の値そのもの**だけ = `tako sessions list` の
/// `last_seen_at` の表示と、異常終了（kill -9・電源断）で終了時の書き出しを通らなかった
/// ときに残る値の古さ（最大でこの秒数）。30 秒の走査 20 回ぶん
const LAST_SEEN_FLUSH_SECS: i64 = 600;

/// カタログファイルのパス（`TAKO_SESSIONS_FILE` 上書き → `<data_dir>/sessions.yaml`）
pub fn catalog_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAKO_SESSIONS_FILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    tako_core::paths::data_dir().map(|d| d.join("sessions.yaml"))
}

/// カタログ本体（sessions.yaml のスキーマ）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionCatalog {
    /// session_id → エントリ
    #[serde(default)]
    pub entries: BTreeMap<String, SessionEntry>,
    /// session_id 検出前の spawn 記録（キーは器のセッション名 / 器が無ければペイン ID。#728）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingSpawn>,
}

/// カタログの 1 エントリ（会話本文は持たない。claude jsonl への参照 + メタデータのみ）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// 種別: master / worker / solo / pane（手動起動などロール無しペイン）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// エージェント種別（claude / codex / agy。session_id 検出経路は claude のみ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// ラベル・プロンプトから抽出した Issue 番号（`#123` 形式）。ラベル → プロンプトの書いた順で、
    /// 先頭が主の Issue（#2199。それより前の記録は昇順 = 先頭は最小の番号）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    /// spawn プロンプトの冒頭抜粋（発見性のため。ローカルファイル限定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    /// 会話の作業ディレクトリ（resume 時の起動 cwd。claude agents の cwd を優先）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    /// tmux バックエンドセッション名（最後に観測したもの）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    /// このセッションのペインログファイル（Issue #112 B との突き合わせ）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started_at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_seen_at: String,
}

/// spawn 時点の記録（session_id 検出前）。
///
/// 突き合わせキーは**器のセッション名、器が無ければペイン ID**（#728）。
/// psmux / tmux が無い構成（Windows の既定・tmux 不在の macOS）では器の名前が
/// 付かないので `tmux_session` は `None` になる。キーの解釈は
/// [`PendingSpawn::matches`] に一本化してあり、直接比較してはいけない。
///
/// **旧形式（`tmux_session` が素の文字列）はそのまま読める**: serde は
/// `Option<String>` へ文字列をそのまま入れ、欠けていれば `default` で `None` になる。
/// 移行 Step は要らない（#916 の判断）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PendingSpawn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_session: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recorded_at: String,
}

impl PendingSpawn {
    /// 検出結果との突き合わせ（#728）。
    ///
    /// 器があるときは**セッション名だけ**で見る（ペイン ID は tako 再起動をまたいで
    /// 振り直されるので、器の名前の方が強い同一性を持つ）。器が無いときだけ
    /// ペイン ID で見る。**両方 None のエントリは何にも一致しない**
    /// （キーの無い記録が最初の検出を横取りするのを防ぐ）
    pub fn matches(&self, session: Option<&str>, pane: Option<u64>) -> bool {
        match (self.tmux_session.as_deref(), session) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => match (self.pane, pane) {
                (Some(mine), Some(theirs)) => mine == theirs,
                _ => false,
            },
        }
    }
}

impl SessionCatalog {
    /// pending 記録を「同じキーのものだけ置き換えて」追加する（#728）。
    ///
    /// キーの解釈は [`PendingSpawn::matches`]（器のセッション名 / 器が無ければペイン ID）。
    /// 単純な `tmux_session` 比較だと、器なし（どちらも `None`）の記録が互いを消し合って
    /// **最後の 1 件しか残らない**。テストが実装をなぞらず**この経路そのもの**を
    /// 通せるように、production の重複排除をここへ切り出してある
    pub fn upsert_pending(&mut self, record: PendingSpawn) {
        self.pending
            .retain(|p| !p.matches(record.tmux_session.as_deref(), record.pane));
        self.pending.push(record);
    }

    /// パス指定 load。不在は空、パース失敗は Err（0 件に丸めない。#169）
    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("sessions.yaml の読み取りに失敗: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| {
            format!(
                "sessions.yaml のパースに失敗: {}",
                tako_core::load_issue::yaml_error_reason(&e, &content)
            )
        })
    }

    pub fn load() -> Result<Self, String> {
        let path = catalog_path().ok_or("ホームディレクトリが取得できない")?;
        Self::load_from(&path)
    }

    /// ロック付き read-modify-write（config_io。#169 と同型）
    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut catalog = Self::load_from(path)?;
        let result = f(&mut catalog);
        let content = serde_yaml::to_string(&catalog)
            .map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)?;
        Ok(result)
    }

    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = catalog_path().ok_or("ホームディレクトリが取得できない")?;
        Self::mutate_at(&path, f)
    }

    /// エントリを last_seen_at の新しい順に返す
    pub fn sorted_entries(&self) -> Vec<(&String, &SessionEntry)> {
        let mut items: Vec<_> = self.entries.iter().collect();
        items.sort_by(|a, b| b.1.last_seen_at.cmp(&a.1.last_seen_at));
        items
    }

    /// id の前方一致でエントリを解決する。複数一致は候補一覧つきのエラー
    pub fn resolve_id(&self, id_prefix: &str) -> Result<(&String, &SessionEntry), String> {
        // 完全一致を最優先（他 ID の前方部分と衝突しても曖昧にならない）
        if let Some((id, entry)) = self.entries.get_key_value(id_prefix) {
            return Ok((id, entry));
        }
        let matches: Vec<_> = self
            .entries
            .iter()
            .filter(|(id, _)| id.starts_with(id_prefix))
            .collect();
        match matches.len() {
            0 => Err(format!(
                "セッション '{id_prefix}' がカタログに見つからない（tako sessions list で確認）"
            )),
            1 => Ok(matches[0]),
            n => Err(format!(
                "セッション '{id_prefix}' の候補が {n} 件ある（もう少し長い ID を指定）: {}",
                matches
                    .iter()
                    .map(|(id, _)| short_id(id))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// spawn 時の pending 記録（dispatch_orchestrator_spawn から呼ぶ）。
/// 同じ tmux セッション名の既存 pending は置き換える。失敗は呼び出し側で警告のみ
/// （カタログの失敗で spawn を止めない）
pub fn record_spawn(record: PendingSpawn) -> Result<(), String> {
    SessionCatalog::mutate(|catalog| catalog.upsert_pending(record))
}

/// GUI の定期スキャンが検出した claude セッション 1 件分
#[derive(Debug, Clone)]
pub struct DetectedSession {
    pub session_id: String,
    /// 器のセッション名（pending / ペインとの対応キー）。器が無ければ `None`（#728）
    pub tmux_session: Option<String>,
    /// tako のペイン ID。器が無いペインはこれが唯一の対応キーになる（#728）
    pub pane: Option<u64>,
    /// claude agents --json の cwd（resume の起動ディレクトリとして最優先）
    pub agent_cwd: Option<String>,
    pub model: Option<String>,
}

/// スキャン時点のペインのメタ情報（GUI が workspace から収集する）
#[derive(Debug, Clone, Default)]
pub struct PaneMetaSnapshot {
    pub pane: u64,
    pub tab: u64,
    /// 器のセッション名。器を持たないペインは `None`（#728。この場合の対応付けは
    /// 検出側が PTY 直下の子 pid で解決済みなので、ここには pid を持たない）
    pub tmux_session: Option<String>,
    pub role: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// このペインの現在のペインログファイル（Issue #112 B）
    pub log_file: Option<String>,
}

/// `agents::list_agents_with_panes` の結果（`{"agents": [...]}`）から検出リストを作る。
/// pane 対応（`session:window.pane`）と有効な session_id を持つエントリだけを拾う
pub fn detect_from_agents_value(value: &Value) -> Vec<DetectedSession> {
    value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| {
            let tmux_session = agent["pane"]
                .as_str()
                .and_then(|pane| pane.split_once(':'))
                .map(|(backend, _)| backend.to_string());
            let pane = agent["tako_pane"].as_u64();
            if tmux_session.is_none() && pane.is_none() {
                return None;
            }
            let session_id = agent["session_id"].as_str()?;
            if !crate::transcript::is_valid_session_id(session_id) {
                return None;
            }
            Some(DetectedSession {
                session_id: session_id.to_string(),
                tmux_session,
                pane,
                agent_cwd: agent["cwd"].as_str().map(str::to_string),
                model: agent["model"].as_str().map(str::to_string),
            })
        })
        .collect()
}

/// mod の報告が答えたペインの検出（#2017。GUI の定期スキャンは、mod の報告に会話 ID が
/// 新しく載っているペインを `claude agents --json` に問わない）。
///
/// 対応キーは走査の検出（[`detect_from_agents_value`]）と同じ: 器があれば器のセッション名、
/// 無ければ tako のペイン ID。`agent_cwd` は報告に無いので `None`（同期は前回値かペインの cwd を使う）
pub fn detected_from_mod(
    pane: u64,
    backend: Option<&str>,
    session_id: &str,
    model: Option<&str>,
) -> Option<DetectedSession> {
    if !crate::transcript::is_valid_session_id(session_id) {
        return None;
    }
    Some(DetectedSession {
        session_id: session_id.to_string(),
        tmux_session: backend.map(str::to_string),
        pane: backend.is_none().then_some(pane),
        agent_cwd: None,
        model: model.map(str::to_string),
    })
}

/// 走査の検出と mod の報告の検出を 1 本にする（#2017。純関数）。
///
/// 同じペイン（器のセッション名 / ペイン ID）で会話 ID も同じなら走査の検出を採る（`cwd` を持つ）。
/// 会話 ID が食い違えば mod の報告を採る（走査は最大 30 秒古い = UI の鮮度。mod は 20 秒以内で、
/// `/clear` / `/resume` の後の会話 ID を先に運ぶ）。走査が拾わなかったペインは mod の側を足す
pub fn merge_detected(
    scanned: Vec<DetectedSession>,
    from_mod: Vec<DetectedSession>,
) -> Vec<DetectedSession> {
    let same_pane =
        |a: &DetectedSession, b: &DetectedSession| match (&a.tmux_session, &b.tmux_session) {
            (Some(x), Some(y)) => x == y,
            (None, None) => a.pane.is_some() && a.pane == b.pane,
            _ => false,
        };
    let mut merged = scanned;
    for d in from_mod {
        match merged.iter_mut().find(|s| same_pane(s, &d)) {
            Some(s) if s.session_id == d.session_id => {}
            Some(s) => *s = d,
            None => merged.push(d),
        }
    }
    merged
}

/// 検出結果をカタログへ同期する（pending の昇格 + エントリ更新 + GC）。
/// GUI の定期スキャン（background）から呼ばれる。**検出が空でも呼んでよい**
/// （溜めた `last_seen_at` が無ければ I/O しない。会話がすべて終わった走査で
/// 溜めた分を書くために呼ぶ。#2020）
pub fn sync_detected(
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
) -> Result<(), String> {
    let path = catalog_path().ok_or("ホームディレクトリが取得できない")?;
    sync_detected_at(&path, detected, panes)
}

/// パス指定版 sync（テスト用に公開）。間引きの状態はプロセスに 1 つ（パスごと）
pub fn sync_detected_at(
    path: &Path,
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
) -> Result<(), String> {
    if legacy_2020() {
        return sync_detected_legacy(path, detected, panes);
    }
    let mut throttles = throttles().lock().unwrap_or_else(|e| e.into_inner());
    let throttle = throttles.entry(path.to_path_buf()).or_default();
    sync_detected_throttled(path, detected, panes, unix_now(), throttle).map(|_| ())
}

/// 溜めた `last_seen_at` を書き出す（GUI の終了時。#2020）。溜めた分が無ければ I/O しない
pub fn flush_last_seen() -> Result<(), String> {
    if legacy_2020() {
        return Ok(());
    }
    let mut throttles = throttles().lock().unwrap_or_else(|e| e.into_inner());
    let now = unix_now();
    let mut first_err = None;
    for (path, throttle) in throttles.iter_mut() {
        if let Err(e) = flush_last_seen_at(path, now, throttle) {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// #2020 の A/B。`TAKO_2020_LEGACY=1` で**同一バイナリのまま**修正前
/// （検出のたびに全体を書き、毎回 `.bak` を回す）へ戻す
pub fn legacy_2020() -> bool {
    std::env::var_os("TAKO_2020_LEGACY").is_some_and(|v| !v.is_empty())
}

/// 1 回の同期で何を書いたか（#2020。単体テスト・計測が見る）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncWrite {
    /// 書かなかった（変化なし / `last_seen_at` だけの変化を溜めた）
    Skipped,
    /// `last_seen_at` だけが変わった内容を書いた（`.bak` は回さない）
    LastSeenOnly,
    /// `last_seen_at` 以外も変わった内容を書いた（`.bak` を回してから書く）
    Structural,
}

/// GUI の走査がまだ書いていない `last_seen_at`（#2020）。
///
/// 正本はファイルで、ここは**ファイルとの差分だけ**を持つ（毎回ロックを取って読み直すので、
/// 他プロセスの書き込み = spawn の pending 記録・手で直した内容を潰さない）
#[derive(Debug, Default)]
pub struct LastSeenThrottle {
    /// 直前の走査で検出した会話の集合（変わったら即時に書く）
    detected: BTreeSet<String>,
    /// まだ書いていない「最後に見えた時刻」（session_id → ISO）
    unflushed: BTreeMap<String, String>,
    /// このプロセスが最後に書いた時刻（unix 秒）。`None` = まだ書いていない
    last_flush: Option<i64>,
}

impl LastSeenThrottle {
    /// まだ書いていない `last_seen_at` があるか
    pub fn has_unflushed(&self) -> bool {
        !self.unflushed.is_empty()
    }
}

/// 間引きの状態（プロセスに 1 つ。カタログのパスごと）
fn throttles() -> &'static Mutex<BTreeMap<PathBuf, LastSeenThrottle>> {
    static THROTTLES: OnceLock<Mutex<BTreeMap<PathBuf, LastSeenThrottle>>> = OnceLock::new();
    THROTTLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 間引きつきの同期の本体（時刻と状態を引数で受ける = 単体テストが時間を進められる。#2020）
pub fn sync_detected_throttled(
    path: &Path,
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
    now_secs: i64,
    throttle: &mut LastSeenThrottle,
) -> Result<SyncWrite, String> {
    let ids: BTreeSet<String> = detected
        .iter()
        .filter(|d| crate::transcript::is_valid_session_id(&d.session_id))
        .map(|d| d.session_id.clone())
        .collect();
    // 何も検出せず、溜めた分も無い = 修正前と同じく I/O しない
    if detected.is_empty() && !throttle.has_unflushed() {
        throttle.detected = ids;
        return Ok(SyncWrite::Skipped);
    }
    let now = crate::diag::format_utc(now_secs);
    let _lock = crate::config_io::lock_exclusive(path)?;
    let base = SessionCatalog::load_from(path)?;
    let mut next = base.clone();
    // 溜めていた時刻（修正前ならもう書いてあった値）を先に当てる。GC が修正前と
    // 同じ値で古い順を選び、書いたときの中身が修正前のファイルと一致する
    apply_unflushed(&mut next, &throttle.unflushed);
    // 修正前と同じく、検出が空の走査では昇格も GC もしない
    if !detected.is_empty() {
        apply_detected(&mut next, detected, panes, &now);
    }
    let structural = !same_except_last_seen(&base, &next);
    let set_changed = ids != throttle.detected;
    let due = throttle
        .last_flush
        .is_none_or(|t| now_secs - t >= LAST_SEEN_FLUSH_SECS);
    // 書けなかったとき（Err）も、この走査で見えた時刻を失わないよう先に溜める
    for id in &ids {
        throttle.unflushed.insert(id.clone(), now.clone());
    }
    throttle.detected = ids;
    if !(structural || set_changed || due) {
        return Ok(SyncWrite::Skipped);
    }
    let written = write_catalog(path, &base, &next, structural)?;
    throttle.unflushed.clear();
    throttle.last_flush = Some(now_secs);
    Ok(written)
}

/// 溜めた `last_seen_at` だけを書き出す（[`flush_last_seen`] の本体。#2020）
pub fn flush_last_seen_at(
    path: &Path,
    now_secs: i64,
    throttle: &mut LastSeenThrottle,
) -> Result<SyncWrite, String> {
    if !throttle.has_unflushed() {
        return Ok(SyncWrite::Skipped);
    }
    let _lock = crate::config_io::lock_exclusive(path)?;
    let base = SessionCatalog::load_from(path)?;
    let mut next = base.clone();
    apply_unflushed(&mut next, &throttle.unflushed);
    let written = write_catalog(path, &base, &next, false)?;
    throttle.unflushed.clear();
    throttle.last_flush = Some(now_secs);
    Ok(written)
}

/// 溜めていた「最後に見えた時刻」を当てる。修正前はその走査の時点で同じ値を
/// 書いていたので、当てた後の内容は修正前のファイルと一致する
/// （カタログから消えた会話 = GC・手で消した、には当てない）
fn apply_unflushed(catalog: &mut SessionCatalog, unflushed: &BTreeMap<String, String>) {
    for (id, seen) in unflushed {
        if let Some(entry) = catalog.entries.get_mut(id) {
            entry.last_seen_at.clone_from(seen);
        }
    }
}

/// 中身が同じなら書かない。`last_seen_at` 以外が変わったときだけ `.bak` を回す
/// （#169 の世代バックアップは会話・項目の消失から戻すためのもの）
fn write_catalog(
    path: &Path,
    base: &SessionCatalog,
    next: &SessionCatalog,
    structural: bool,
) -> Result<SyncWrite, String> {
    if next == base {
        return Ok(SyncWrite::Skipped);
    }
    let content =
        serde_yaml::to_string(next).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
    if structural {
        crate::config_io::rotate_backups(path)?;
    }
    crate::config_io::atomic_write(path, &content)?;
    Ok(if structural {
        SyncWrite::Structural
    } else {
        SyncWrite::LastSeenOnly
    })
}

/// `last_seen_at` を除いて同じ内容か（#2020）
fn same_except_last_seen(a: &SessionCatalog, b: &SessionCatalog) -> bool {
    a.pending == b.pending
        && a.entries.len() == b.entries.len()
        && a.entries
            .iter()
            .zip(&b.entries)
            .all(|((id_a, a), (id_b, b))| id_a == id_b && entry_eq_except_last_seen(a, b))
}

/// エントリ 1 件を `last_seen_at` を除いて比べる。
///
/// **全フィールドを名指しで分解する**（`..` を使わない）: フィールドを足すとここが
/// コンパイルエラーになり、新しいフィールドの変化を「`last_seen_at` だけの変化」と
/// 取り違えて書き込みを遅らせる・`.bak` を回し損ねることが起きない
fn entry_eq_except_last_seen(a: &SessionEntry, b: &SessionEntry) -> bool {
    let SessionEntry {
        kind,
        label,
        project,
        profile,
        agent,
        model,
        effort,
        issues,
        prompt_head,
        cwd,
        tab,
        pane,
        tmux_session,
        log_file,
        started_at,
        last_seen_at: _,
    } = a;
    *kind == b.kind
        && *label == b.label
        && *project == b.project
        && *profile == b.profile
        && *agent == b.agent
        && *model == b.model
        && *effort == b.effort
        && *issues == b.issues
        && *prompt_head == b.prompt_head
        && *cwd == b.cwd
        && *tab == b.tab
        && *pane == b.pane
        && *tmux_session == b.tmux_session
        && *log_file == b.log_file
        && *started_at == b.started_at
}

/// 修正前の同期（#2020 の A/B）: 検出のたびに全体を書き、毎回 `.bak` を回す
fn sync_detected_legacy(
    path: &Path,
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
) -> Result<(), String> {
    if detected.is_empty() {
        return Ok(());
    }
    let now = now_iso();
    SessionCatalog::mutate_at(path, |catalog| {
        apply_detected(catalog, detected, panes, &now)
    })
}

/// 検出結果をカタログへ当てる（pending の昇格 + エントリ更新 + GC）
fn apply_detected(
    catalog: &mut SessionCatalog,
    detected: &[DetectedSession],
    panes: &[PaneMetaSnapshot],
    now: &str,
) {
    let now = now.to_string();
    for d in detected {
        if !crate::transcript::is_valid_session_id(&d.session_id) {
            continue;
        }
        // 対応付けは器のセッション名優先、器が無ければペイン ID（#728）
        let pane_meta = panes
            .iter()
            .find(|p| match (&p.tmux_session, &d.tmux_session) {
                (Some(mine), Some(theirs)) => mine == theirs,
                _ => d.pane.is_some_and(|pane| p.pane == pane),
            });
        let pending_idx = catalog
            .pending
            .iter()
            .position(|p| p.matches(d.tmux_session.as_deref(), d.pane));
        let pending = pending_idx.map(|i| catalog.pending.remove(i));

        let entry = catalog.entries.entry(d.session_id.clone()).or_default();
        if entry.started_at.is_empty() {
            entry.started_at = pending
                .as_ref()
                .map(|p| p.recorded_at.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| now.clone());
        }
        entry.last_seen_at = now.clone();
        // 器が無い構成では None のまま（「観測できていない」ではなく「器が無い」）
        if d.tmux_session.is_some() {
            entry.tmux_session = d.tmux_session.clone();
        }
        if let Some(pane) = d.pane {
            entry.pane = Some(pane);
        }
        if d.agent_cwd.is_some() {
            entry.cwd = d.agent_cwd.clone();
        }
        if d.model.is_some() {
            entry.model = d.model.clone();
        }
        // spawn 時のメタ（プロンプト由来の情報）は pending が最も正確
        if let Some(p) = pending {
            entry.kind = p.kind;
            entry.label = p.label.or(entry.label.take());
            entry.project = p.project.or(entry.project.take());
            entry.agent = p.agent.or(entry.agent.take());
            if entry.model.is_none() {
                entry.model = p.model;
            }
            entry.effort = p.effort.or(entry.effort.take());
            if !p.issues.is_empty() {
                entry.issues = p.issues;
            }
            entry.prompt_head = p.prompt_head.or(entry.prompt_head.take());
            if entry.cwd.is_none() {
                entry.cwd = p.cwd;
            }
        }
        // ペインの現在情報で補完・更新する
        if let Some(meta) = pane_meta {
            entry.pane = Some(meta.pane);
            entry.tab = Some(meta.tab);
            if let Some(role) = meta.role.as_deref() {
                let parsed = parse_role(role);
                if entry.kind.is_empty() {
                    entry.kind = parsed.kind.to_string();
                }
                if entry.project.is_none() {
                    entry.project = parsed.project;
                }
                if entry.label.is_none() {
                    entry.label = parsed.label;
                }
                if entry.profile.is_none() {
                    entry.profile = parsed.profile;
                }
            }
            if entry.label.is_none() {
                entry.label = meta.title.clone();
            }
            if entry.cwd.is_none() {
                entry.cwd = meta.cwd.clone();
            }
            if meta.log_file.is_some() {
                entry.log_file = meta.log_file.clone();
            }
        }
        if entry.kind.is_empty() {
            entry.kind = "pane".into();
        }
        if entry.agent.is_none() {
            entry.agent = Some("claude".into());
        }
        if entry.issues.is_empty() {
            entry.issues = spawn_issues(
                entry.label.as_deref(),
                entry.prompt_head.as_deref().unwrap_or(""),
            );
        }
    }
    gc(catalog, &now);
}

/// pending の期限切れ掃除 + エントリ数の上限強制
fn gc(catalog: &mut SessionCatalog, now: &str) {
    let cutoff = parse_iso(now).unwrap_or(0) - PENDING_TTL_SECS;
    catalog
        .pending
        .retain(|p| parse_iso(&p.recorded_at).unwrap_or(i64::MAX) >= cutoff);
    if catalog.entries.len() > MAX_ENTRIES {
        let mut ids: Vec<(String, String)> = catalog
            .entries
            .iter()
            .map(|(id, e)| (e.last_seen_at.clone(), id.clone()))
            .collect();
        ids.sort(); // last_seen_at 昇順 = 古い順
        let drop_count = catalog.entries.len() - MAX_ENTRIES;
        for (_, id) in ids.into_iter().take(drop_count) {
            catalog.entries.remove(&id);
        }
    }
}

/// role 文字列の解析結果
#[derive(Debug, PartialEq)]
pub struct ParsedRole {
    pub kind: &'static str,
    pub project: Option<String>,
    pub label: Option<String>,
    pub profile: Option<String>,
}

/// ペインの role 文字列（spawn / master / solo が設定する）を分類する。
/// - `orchestrator-master[:suffix]` → master（suffix = プロファイル）
/// - `solo[:suffix]` → solo
/// - `orchestrator-worker:<project>[:<label>]` → worker
/// - それ以外 → pane
pub fn parse_role(role: &str) -> ParsedRole {
    if let Some(rest) = role.strip_prefix("orchestrator-worker") {
        let mut it = rest.strip_prefix(':').unwrap_or("").splitn(2, ':');
        let project = it.next().filter(|s| !s.is_empty()).map(str::to_string);
        let label = it.next().filter(|s| !s.is_empty()).map(str::to_string);
        return ParsedRole {
            kind: "worker",
            project,
            label,
            profile: None,
        };
    }
    if let Some(rest) = role.strip_prefix("orchestrator-master") {
        let profile = rest.strip_prefix(':').filter(|s| !s.is_empty());
        return ParsedRole {
            kind: "master",
            project: None,
            label: None,
            profile: Some(profile.unwrap_or("default").to_string()),
        };
    }
    if role == "solo" || role.starts_with("solo:") {
        let profile = role.strip_prefix("solo:").filter(|s| !s.is_empty());
        return ParsedRole {
            kind: "solo",
            project: None,
            label: None,
            profile: Some(profile.unwrap_or("default").to_string()),
        };
    }
    ParsedRole {
        kind: "pane",
        project: None,
        label: None,
        profile: None,
    }
}

/// #2199 の A/B。`TAKO_2199_LEGACY=1` で**同一バイナリのまま**修正前
/// （拾った番号を昇順に並べる = 台帳の `issue` とカタログの先頭が最小の番号になる）へ戻す
pub fn legacy_2199() -> bool {
    std::env::var_os("TAKO_2199_LEGACY").is_some_and(|v| !v.is_empty())
}

/// テキストから Issue 番号（`#123`）を抽出する（**書いた順**・重複は最初の出現だけ）。
///
/// 先頭が主の Issue になる（#2199）。修正前は昇順に並べていたので、プロンプトが前例や規約として
/// 触れた小さい番号が先頭に来て、委任台帳の `issue` がラベルの Issue と食い違っていた
/// （直近 7 日の 176 件中 170 件。`.agent/plans/2026-10-usage-analytics.md` §1.4）
pub fn extract_issues(text: &str) -> Vec<u32> {
    extract_issues_with(text, legacy_2199())
}

/// spawn の記録（セッションカタログ・委任台帳・worker レジストリ）に載せる Issue 番号（#2199）。
/// ラベル → プロンプトの順に書いた順で並べるので、先頭 = 主の Issue
/// （ラベルに書いた最初の `#N`。ラベルに番号が無ければプロンプトに書いた最初の `#N`）
pub fn spawn_issues(label: Option<&str>, prompt: &str) -> Vec<u32> {
    extract_issues(&format!("{} {prompt}", label.unwrap_or_default()))
}

/// [`extract_issues`] の本体。`legacy` は #2199 の修正前（昇順に並べ替える）
fn extract_issues_with(text: &str, legacy: bool) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start && end - start <= 6 {
                if let Ok(n) = text[start..end].parse::<u32>() {
                    if n > 0 && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if legacy {
        out.sort_unstable();
    }
    out
}

/// プロンプト冒頭の抜粋（1 行に正規化して最大 `max` 文字 + `…`）。
///
/// 切るのは `tako_core::text::truncate_chars` の 1 実装（#1790）。`…` を上限の外に付ける
/// 契約なので上限を `max + 1` で渡す = `…` 込みで最大 `max + 1` 文字
pub fn prompt_head(prompt: &str, max: usize) -> String {
    let flat = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    tako_core::text::truncate_chars(&flat, max.saturating_add(1))
}

/// 表示用の短縮 ID（先頭 8 文字）
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// `list` の応答ペイロード（CLI / MCP 共通）
pub fn list_payload(
    role: Option<&str>,
    project: Option<&str>,
    limit: usize,
) -> Result<Value, String> {
    let catalog = SessionCatalog::load()?;
    let sessions: Vec<Value> = catalog
        .sorted_entries()
        .into_iter()
        .filter(|(_, e)| role.is_none_or(|r| e.kind == r))
        .filter(|(_, e)| project.is_none_or(|p| e.project.as_deref() == Some(p)))
        .take(limit)
        .map(|(id, e)| {
            json!({
                "session_id": id,
                "short_id": short_id(id),
                "kind": e.kind,
                "label": e.label,
                "project": e.project,
                "profile": e.profile,
                "agent": e.agent,
                "model": e.model,
                "issues": e.issues,
                "cwd": e.cwd,
                "tab": e.tab,
                "pane": e.pane,
                "tmux_session": e.tmux_session,
                "log_file": e.log_file,
                "started_at": e.started_at,
                "last_seen_at": e.last_seen_at,
                "resumable": crate::transcript::find_transcript(id).is_some(),
            })
        })
        .collect();
    // codex / agy など session_id 未検出の spawn 記録（resume 不可の制限つきで可視化）
    let pending: Vec<Value> = catalog
        .pending
        .iter()
        .map(|p| {
            json!({
                "tmux_session": p.tmux_session,
                // 器が無い構成の突き合わせキー（#728）。表示・診断のために出す
                "pane": p.pane,
                "kind": p.kind,
                "label": p.label,
                "project": p.project,
                "agent": p.agent,
                "issues": p.issues,
                "recorded_at": p.recorded_at,
            })
        })
        .collect();
    Ok(json!({
        "sessions": sessions,
        "pending": pending,
        "catalog_path": catalog_path(),
    }))
}

/// `show` の応答ペイロード（メタ + 会話冒頭の抜粋 + transcript 情報）
pub fn show_payload(id_prefix: &str) -> Result<Value, String> {
    let catalog = SessionCatalog::load()?;
    let (id, entry) = catalog.resolve_id(id_prefix)?;
    let transcript = crate::transcript::find_transcript(id);
    let transcript_info = transcript.as_ref().map(|path| {
        let meta = std::fs::metadata(path).ok();
        json!({
            "path": path,
            "size": meta.as_ref().map(|m| m.len()),
        })
    });
    let first_user = crate::transcript::first_user_text(id, 300);
    Ok(json!({
        "session_id": id,
        "entry": serde_json::to_value(entry).unwrap_or(Value::Null),
        "resumable": transcript.is_some(),
        "transcript": transcript_info,
        "first_user_message": first_user,
    }))
}

/// resume 用の起動コマンドを組み立てる。
///
/// **系統ごとの書式は `tako_core::agent_resume::resume_spec` が正本**（#1238）。
/// claude だけが `--model` / `--effort` を載せられる（`codex resume` は受け取らず、
/// agy は会話に記録された設定を上書きしないため足さない）。
/// 会話が既定以外の config ディレクトリにあれば `CLAUDE_CONFIG_DIR` を前置する（Issue #652）。
///
/// `mcp_args` は worker の MCP の断片（[`resume_worker_mcp`] + [`resume_mcp_args`] = spawn と
/// 同じ組み立て。#2447）。worker 以外は空
pub fn resume_command(id: &str, entry: &SessionEntry, mcp_args: &str) -> Result<String, String> {
    let agent = agent_of(entry);
    resume_command_with_env(id, entry, conversation_env(agent, id).as_deref(), mcp_args)
}

/// 再開（resume）で起こす worker の MCP を決める（#2447。I/O あり = プロジェクトの設定・user scope・
/// 会話の所在を読む）。**spawn と同じ [`crate::worker_mcp::prepare`] を通す**ので、再起動をまたいで
/// 戻った worker も spawn と同じ MCP だけを持つ（#2412 / #2418 の対策が再起動で外れない）。
///
/// 読む設定の置き場は**会話の記録がある場所**（claude = 会話の config dir・codex = rollout を探した
/// `CODEX_HOME`）。resume が実際に開くのもそこなので、spawn のプロファイルとずれても今の会話に合う。
/// `None` = worker でない（master / solo / 役割の無いペイン）・worker になれない系統・`TAKO_2447_LEGACY=1`
pub fn resume_worker_mcp(
    agent: Agent,
    id: &str,
    entry: &SessionEntry,
) -> Option<(
    crate::orchestrator::agent::WorkerAgent,
    crate::worker_mcp::WorkerMcp,
)> {
    if entry.kind != "worker" || crate::worker_mcp::legacy_resume() {
        return None;
    }
    let worker = crate::orchestrator::agent::WorkerAgent::try_from(agent).ok()?;
    let mut env = crate::orchestrator::EnvPlan::default();
    match agent {
        Agent::Claude => {
            if let Some(location) = crate::transcript::locate_transcript(id) {
                if !location.is_default {
                    env.exports.push((
                        crate::orchestrator::CLAUDE_CONFIG_DIR_ENV.to_string(),
                        location.config_dir.display().to_string(),
                    ));
                }
            }
        }
        Agent::Codex => {
            if let Some(home) = crate::codex_session::codex_home() {
                env.exports
                    .push(("CODEX_HOME".to_string(), home.display().to_string()));
            }
        }
        Agent::Agy | Agent::Local => {}
    }
    let mcp = crate::worker_mcp::prepare_for_resume(
        worker,
        entry.project.as_deref(),
        entry.cwd.as_deref(),
        &env,
        &[],
    );
    Some((worker, mcp))
}

/// 再開のコマンドへ足す MCP の断片（spawn と同じ [`crate::orchestrator::agent::worker_mcp_args`]）
pub fn resume_mcp_args(
    agent: crate::orchestrator::agent::WorkerAgent,
    mcp: &crate::worker_mcp::WorkerMcp,
    tako_bin: &str,
    dialect: crate::launch_cmd::ShellDialect,
) -> String {
    crate::orchestrator::agent::worker_mcp_args(
        agent,
        Some(tako_bin),
        mcp.settings_file.as_deref(),
        &mcp.codex_off,
        dialect,
    )
}

/// カタログの記録が指す系統（未記録は claude = 既定）
fn agent_of(entry: &SessionEntry) -> Agent {
    entry
        .agent
        .as_deref()
        .and_then(Agent::parse)
        .unwrap_or(Agent::Claude)
}

/// 会話の所在を確かめ、必要なら起動コマンドへ前置きする env を返す（#1238）。
///
/// `None` = **どこにも会話が無い**（resume しても「見つからない」で終わるので起こさない）。
/// claude だけが所在によって `CLAUDE_CONFIG_DIR` の指定を要する（#652）ので、
/// 他系統は「見つかった」を空文字で表す
fn conversation_env(agent: Agent, id: &str) -> Option<String> {
    if agent == Agent::Claude {
        return crate::transcript::resume_env_prefix(id);
    }
    // claude 以外は config ディレクトリの概念が無いので、
    // 「見つかった」を空文字で表す（所在の判定は agent_resume が系統ごとに持つ）
    crate::agent_resume::conversation_exists(agent, id)
        .unwrap_or(false)
        .then(String::new)
}

/// `resume_command` の本体（env プレフィクスと MCP の断片を引数で受け取るテスト可能版）
fn resume_command_with_env(
    id: &str,
    entry: &SessionEntry,
    env_prefix: Option<&str>,
    mcp_args: &str,
) -> Result<String, String> {
    let agent = agent_of(entry);
    let spec = tako_core::agent_resume::resume_spec(agent).ok_or_else(|| {
        format!(
            "agent '{}' のセッションは resume 非対応（対応: claude / codex / agy）",
            agent.as_str()
        )
    })?;
    // 書式検証は全系統共通（パストラバーサル防止。codex / agy の会話 ID も UUID）
    if !crate::transcript::is_valid_session_id(id) {
        return Err("session_id の形式が不正".into());
    }
    let role_env = match entry.kind.as_str() {
        "worker" => {
            let project = entry.project.as_deref().unwrap_or("resumed");
            match entry.label.as_deref() {
                Some(l) => Some(format!("worker:{project}:{l}")),
                None => Some(format!("worker:{project}")),
            }
        }
        "master" => Some(match entry.profile.as_deref() {
            Some(p) if p != "default" => format!("master:{p}"),
            _ => "master".into(),
        }),
        "solo" => Some(match entry.profile.as_deref() {
            Some(p) if p != "default" => format!("solo:{p}"),
            _ => "solo".into(),
        }),
        _ => None,
    };
    let mut cmd = String::new();
    // アカウントの config ディレクトリ指定は先頭（rc / direnv より後に効く。#500 / #512 と同型）
    if let Some(prefix) = env_prefix {
        cmd.push_str(prefix);
    }
    if let Some(role) = role_env {
        cmd.push_str(&format!(
            "TAKO_ORCHESTRATOR_ROLE={} ",
            crate::orchestrator::agent::sh_quote(&role)
        ));
    }
    cmd.push_str(spec.program);
    if spec.accepts_launch_flags {
        if let Some(model) = entry.model.as_deref() {
            cmd.push_str(&format!(
                " --model {}",
                crate::orchestrator::agent::sh_quote(model)
            ));
        }
        if let Some(effort) = entry.effort.as_deref() {
            cmd.push_str(&format!(" --effort {effort}"));
        }
    }
    for head in spec.head {
        cmd.push(' ');
        cmd.push_str(head);
    }
    // worker の MCP の断片（#2447。spawn と同じ `worker_mcp_args` が組んだもの）。
    // codex は `resume` の後（`codex resume -c …` = 0.154.0 の --help）・claude は `--resume` の前
    cmd.push_str(mcp_args);
    if let Some(flag) = spec.id_flag {
        cmd.push(' ');
        cmd.push_str(flag);
    }
    cmd.push(' ');
    cmd.push_str(id);
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// 復元（PC 再起動後）の 1 ペインぶんの判断（Issue #1076 / #1238）
// ---------------------------------------------------------------------------

/// 新規シェルで開き直す理由（#1076。復元内訳のログに出す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshShellReason {
    /// 保存時に会話を検出できていなかった（= 復元すべき会話が記録されていない）
    NoSessionId,
    /// 保存値の形式が不正（パストラバーサル対策で弾いた）
    InvalidSessionId,
    /// 会話ファイル（transcript / rollout / conversations の db）が無い
    TranscriptMissing,
    /// **その系統をこの環境では復元できない**（#1238）。
    /// 「保存されていない」（`NoSessionId`）と混ぜると原因が読めない
    ResumeUnsupported,
}

impl FreshShellReason {
    /// 診断ログ用の日本語ラベル
    pub fn label(self) -> &'static str {
        match self {
            Self::NoSessionId => "ID なし",
            Self::InvalidSessionId => "形式不正",
            Self::TranscriptMissing => "会話が見つからない",
            Self::ResumeUnsupported => "resume 非対応",
        }
    }
}

/// 復元時に 1 ペインをどう起こすか（#1076）。**理由まで返す**ので、
/// 復元内訳のログが「なぜエージェントが出なかったか」を言える
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePlan {
    /// 器（tmux セッション）が生きている = 実行中プロセスごと再 attach する
    Reattach,
    /// 保存済みの会話を明示 resume する（#1238 で claude 以外へ広げた）
    Resume {
        /// どの系統として起こすか（診断ログ・件数の内訳に出す）
        agent: Agent,
        /// シェルへ投入するコマンド（改行は投入側が付ける）
        command: String,
        /// カタログ / spawn 記録から役割・モデルまで組み立てられたか（診断ログ用）
        with_role: bool,
    },
    /// 保存 cwd で新しいシェルを開くだけ
    FreshShell(FreshShellReason),
}

/// 復元対象のペインに紐づく「どの系統の・どの会話か」（#1238）。
///
/// `id` が `None` = **系統は分かるが会話 ID を採れていない**
/// （agy は最初のターンまで会話が生まれない / Windows には `lsof` が無い）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneResume<'a> {
    pub agent: Agent,
    pub id: Option<&'a str>,
}

/// 復元時の 1 ペインの判断材料（#1076 / #1238）
#[derive(Debug, Clone, Copy)]
pub struct RestoreInput<'a> {
    /// 器（tmux セッション）が生きているか
    pub backend_alive: bool,
    /// `layout.json` に保存されていた系統と会話 ID
    pub resume: Option<PaneResume<'a>>,
    /// 会話が見つかったか。`Some` の中身は claude の `CLAUDE_CONFIG_DIR` 前置き
    /// （[`crate::transcript::resume_env_prefix`]）で、他系統は空文字。
    /// `None` = 会話ファイルがどこにも無い = resume しても失敗するので起こさない（#652）
    pub conversation: Option<&'a str>,
    /// 起動条件（役割 / モデル / effort）の記録
    pub catalog: Option<&'a SessionCatalog>,
    /// 起動条件を spawn 記録から引くためのキー（#728 と同じ「器の名前 → ペイン ID」）
    pub backend_session: Option<&'a str>,
    pub pane: Option<u64>,
    /// 判定する OS（**引数で受ける**ので macOS 上から Windows 側を検証できる）
    pub platform: Platform,
    /// 復元する会話が worker のとき、起動コマンドへ足す MCP の断片（#2447）。
    /// 決めるのは実環境版の [`restore_plan`]（spawn と同じ判断 = [`resume_worker_mcp`]）で、
    /// ここは差し込むだけ。`None` は何も足さない
    pub worker_mcp_args: Option<&'a str>,
}

/// 復元時の 1 ペインの判断（材料を引数で受け取る純粋版）。
///
/// コマンドの形は [`resume_command`] が正本（役割 env / `--model` / `--effort` /
/// config dir までカタログの記録から復元する）。カタログにも spawn 記録にも
/// 無いときだけ最小形（config dir + resume の引数）へ落ちる
pub fn restore_plan_in(input: RestoreInput<'_>) -> RestorePlan {
    // 器が生きているなら実行中プロセスごと戻る（エージェントを二重起動しない）
    if input.backend_alive {
        return RestorePlan::Reattach;
    }
    let Some(resume) = input.resume else {
        return RestorePlan::FreshShell(FreshShellReason::NoSessionId);
    };
    // **「保存されていない」と「手段が無い」を分ける**（#1238）。判定の正本は
    // tako_core::agent_resume（Windows に lsof が無い = codex / agy の ID を採れない）
    if !tako_core::agent_resume::restore_support(resume.agent, input.platform).is_wired() {
        return RestorePlan::FreshShell(FreshShellReason::ResumeUnsupported);
    }
    let Some(id) = resume.id else {
        return RestorePlan::FreshShell(FreshShellReason::NoSessionId);
    };
    if !crate::transcript::is_valid_session_id(id) {
        return RestorePlan::FreshShell(FreshShellReason::InvalidSessionId);
    }
    let Some(env_prefix) = input.conversation else {
        return RestorePlan::FreshShell(FreshShellReason::TranscriptMissing);
    };
    // 起動条件が分かれば復元する。**役割 env が復元されないと、戻ってきた
    // エージェントは master / worker として認識されない**（オーケストレーターからも
    // MCP からも見えない）ので、ここは最小形へ落とさずに済ませたい
    if let Some(entry) = launch_record(&input, id, resume.agent) {
        // MCP の断片は worker だけ（master / solo は spawn でも絞らない = #2412 の対象外）
        let mcp_args = match entry.kind.as_str() {
            "worker" => input.worker_mcp_args.unwrap_or(""),
            _ => "",
        };
        if let Ok(command) = resume_command_with_env(id, &entry, Some(env_prefix), mcp_args) {
            let with_role = matches!(entry.kind.as_str(), "master" | "worker" | "solo");
            return RestorePlan::Resume {
                agent: resume.agent,
                command,
                with_role,
            };
        }
    }
    let minimal = SessionEntry {
        agent: Some(resume.agent.as_str().to_string()),
        ..SessionEntry::default()
    };
    match resume_command_with_env(id, &minimal, Some(env_prefix), "") {
        Ok(command) => RestorePlan::Resume {
            agent: resume.agent,
            command,
            with_role: false,
        },
        // 系統が resume を持たない場合はここに来ない（上で弾いてある）が、
        // ID の書式で弾かれた場合の保険
        Err(_) => RestorePlan::FreshShell(FreshShellReason::InvalidSessionId),
    }
}

/// 起動条件の記録を引く（**カタログ本体が先・spawn 記録が後**）。
///
/// claude はセッション検出でカタログ本体へ昇格するが、codex / agy は
/// 昇格経路が無く `pending`（spawn 時の記録）にしか残らない（FR-5.12 の制限）。
/// 器の名前 / ペイン ID で引けば役割・プロジェクト・ラベルは復元できる
fn launch_record(input: &RestoreInput<'_>, id: &str, agent: Agent) -> Option<SessionEntry> {
    let catalog = input.catalog?;
    if let Some(entry) = catalog.entries.get(id) {
        // **系統は layout 由来が正**（記録が食い違っていても、実際に動いていた
        // 系統のコマンドを組む。claude の ID に codex の記録が付いている等の齟齬で
        // 会話への入口を失わない）
        return Some(SessionEntry {
            agent: Some(agent.as_str().to_string()),
            ..entry.clone()
        });
    }
    let pending = catalog
        .pending
        .iter()
        .find(|p| p.matches(input.backend_session, input.pane))?;
    // 系統が食い違う記録は使わない（別のエージェントを起動していたペイン）
    if pending
        .agent
        .as_deref()
        .and_then(Agent::parse)
        .is_some_and(|a| a != agent)
    {
        return None;
    }
    Some(SessionEntry {
        kind: pending.kind.clone(),
        label: pending.label.clone(),
        project: pending.project.clone(),
        // spawn 記録はプロファイル名を持たない（master:<profile> は既定名へ落ちる）
        profile: None,
        agent: Some(agent.as_str().to_string()),
        model: pending.model.clone(),
        effort: pending.effort.clone(),
        // worker の MCP の判断（`.mcp.json` / `.codex/config.toml` の同名）は spawn の cwd で見る（#2447）
        cwd: pending.cwd.clone(),
        ..SessionEntry::default()
    })
}

/// [`restore_plan_in`] の実環境版（会話の所在の走査とカタログの読み込みを行う）。
/// カタログは復元ループの外で 1 回だけ読んで渡す
pub fn restore_plan(
    backend_alive: bool,
    resume: Option<PaneResume<'_>>,
    catalog: Option<&SessionCatalog>,
    backend_session: Option<&str>,
    pane: Option<u64>,
) -> RestorePlan {
    // 器が生きているなら会話ファイルの走査（read_dir）自体が要らない
    if backend_alive {
        return RestorePlan::Reattach;
    }
    // A/B（#1238）: 修正前は claude 以外の会話参照を保存も復元もしていなかった
    let resume = resume.filter(|r| r.agent == Agent::Claude || !crate::agent_resume::legacy_1238());
    let conversation = resume
        .and_then(|r| Some((r.agent, r.id?)))
        .and_then(|(agent, id)| conversation_env(agent, id));
    // A/B（#1076）: 修正前は起動条件を復元せず最小形 `claude --resume <id>` だった
    let catalog = if tako_core::claude_resume::legacy_1076() {
        None
    } else {
        catalog
    };
    let input = RestoreInput {
        backend_alive,
        resume,
        conversation: conversation.as_deref(),
        catalog,
        backend_session,
        pane,
        platform: Platform::current(),
        worker_mcp_args: None,
    };
    let mcp_args = restore_worker_mcp_args(&input);
    restore_plan_in(RestoreInput {
        worker_mcp_args: mcp_args.as_deref(),
        ..input
    })
}

/// 復元する会話が worker なら、spawn と同じ MCP の断片を決める（#2447。設定ファイルもここで書く）。
///
/// 復元は起こしたその場で打つので、spawn と同じく**書いてから使い**、書けなければ絞り込みをやめる
/// （[`crate::worker_mcp::WorkerMcp::materialized`]）。効かない理由は persist.log へ（起動は止めない）
fn restore_worker_mcp_args(input: &RestoreInput<'_>) -> Option<String> {
    let resume = input.resume?;
    let id = resume
        .id
        .filter(|id| crate::transcript::is_valid_session_id(id))?;
    // 会話が無い・この環境で復元できないなら起こさない（restore_plan_in と同じ前提）
    input.conversation?;
    if !tako_core::agent_resume::restore_support(resume.agent, input.platform).is_wired() {
        return None;
    }
    let entry = launch_record(input, id, resume.agent)?;
    let (worker, mcp) = resume_worker_mcp(resume.agent, id, &entry)?;
    let mcp = mcp.materialized();
    for w in &mcp.warnings {
        crate::diag::persist_log(&format!(
            "[restore] pane={} worker の MCP: {w}",
            input
                .pane
                .map_or_else(|| "-".to_string(), |p| p.to_string())
        ));
    }
    Some(resume_mcp_args(
        worker,
        &mcp,
        &crate::dispatch::resolve_tako_binary(),
        crate::launch_cmd::launch_dialect(),
    ))
}

/// 現在時刻の ISO 表記（カタログの記録時刻用）
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    crate::diag::format_utc(secs)
}

/// `YYYY-MM-DDTHH:MM:SSZ` → unix 秒（GC の期限計算・レジストリの経過時間判定用。失敗は None）
pub(crate) fn parse_iso(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // days_from_civil（civil_utc の逆変換）
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// ペイン ID から session_id を逆引きする（カタログの pane フィールドを使用。#284）。
/// カタログが無い・読めない場合は None
pub fn resolve_session_for_pane(pane_id: &str) -> Option<String> {
    let catalog = SessionCatalog::load().ok()?;
    resolve_session_for_pane_in(&catalog, pane_id)
}

/// カタログ指定版（テスト用に分離）。
/// 同一ペイン番号のエントリは世代を跨いで複数堆積する（spawn / 検出のたびに記録が
/// 増える。実運用で同一 pane に 20 世代超を確認）。BTreeMap の辞書順先勝ちだと
/// session_id（uuid）の並び次第で stale な旧世代を毎回返し続けるため（#466）、
/// 最後に生存確認された世代（last_seen_at、無ければ started_at）を選ぶ
pub fn resolve_session_for_pane_in(catalog: &SessionCatalog, pane_id: &str) -> Option<String> {
    let pane_num: u64 = pane_id.parse().ok()?;
    catalog
        .entries
        .iter()
        .filter(|(_, entry)| entry.pane == Some(pane_num))
        .max_by_key(|(_, entry)| {
            if entry.last_seen_at.is_empty() {
                entry.started_at.as_str()
            } else {
                entry.last_seen_at.as_str()
            }
        })
        .map(|(session_id, _)| session_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-sessions-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("sessions.yaml")
    }

    #[test]
    fn agents一覧からの検出はpane対応と有効idだけを拾う() {
        // 旧 agents::claude_session_ids_by_backend のマッピング検証を新 API で維持
        let value = serde_json::json!({"agents": [
            {"pane": "tako-a:0.0", "session_id": "session-a", "cwd": "/w/a", "model": "claude-fable-5"},
            {"pane": "tako-b:1.2", "session_id": "session-b"},
            {"pane": "tako-c:0.0", "session_id": "../../invalid"},
            {"session_id": "pane-missing"},
        ]});
        let detected = detect_from_agents_value(&value);
        assert_eq!(detected.len(), 2);
        assert_eq!(detected[0].session_id, "session-a");
        assert_eq!(detected[0].tmux_session.as_deref(), Some("tako-a"));
        assert_eq!(detected[0].agent_cwd.as_deref(), Some("/w/a"));
        assert_eq!(detected[0].model.as_deref(), Some("claude-fable-5"));
        assert_eq!(detected[1].tmux_session.as_deref(), Some("tako-b"));
        // 空 agents は空
        assert!(detect_from_agents_value(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn 器がないペインはtako_paneで検出される() {
        // #728: 器（psmux / tmux）が無い構成では `pane` が付かない。
        // `tako_pane`（tako のペイン ID）が唯一の対応キーになる
        let value = serde_json::json!({"agents": [
            {"tako_pane": 7, "session_id": "11111111-2222-3333-4444-555555555555",
             "cwd": "C:\\w", "model": "claude-opus-5"},
            {"tako_pane": 9, "session_id": "../../invalid"},
            {"session_id": "22222222-3333-4444-5555-666666666666"},
            // 器あり側が優先（両方付くことは無いが、付いたらセッション名を採る）
            {"pane": "tako-x:0.0", "tako_pane": 3,
             "session_id": "33333333-4444-5555-6666-777777777777"},
        ]});
        let detected = detect_from_agents_value(&value);
        assert_eq!(detected.len(), 2);
        assert_eq!(
            detected[0].session_id,
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(detected[0].tmux_session, None);
        assert_eq!(detected[0].pane, Some(7));
        assert_eq!(detected[0].agent_cwd.as_deref(), Some("C:\\w"));
        assert_eq!(detected[1].tmux_session.as_deref(), Some("tako-x"));
        assert_eq!(detected[1].pane, Some(3));
    }

    #[test]
    fn 器なしのpending記録は互いを消さない() {
        // #728: 旧実装は `p.tmux_session != record.tmux_session` で重複排除していた。
        // 器なし（どちらも None）だと全件が「同じキー」に見え、spawn のたびに
        // 直前の記録が消えて最後の 1 件しか残らなかった
        let mut catalog = SessionCatalog::default();
        let make = |pane: u64| PendingSpawn {
            tmux_session: None,
            pane: Some(pane),
            recorded_at: "2026-08-01T00:00:00Z".into(),
            ..Default::default()
        };
        // **production の重複排除そのもの**を通す（実装をなぞると検出力が出ない）
        for pane in [1u64, 2, 3] {
            catalog.upsert_pending(make(pane));
        }
        assert_eq!(catalog.pending.len(), 3, "別ペインの記録は残る");
        // 同じペインの再 spawn は置き換わる
        catalog.upsert_pending(make(2));
        assert_eq!(catalog.pending.len(), 3);
        assert_eq!(
            catalog.pending.iter().filter(|p| p.pane == Some(2)).count(),
            1,
            "同じキーは 1 件に畳まれる"
        );
    }

    #[test]
    fn 突き合わせキーは器の有無で切り替わる() {
        // #728: 器があるときはセッション名だけで見る（ペイン ID は tako 再起動を
        // またいで振り直されるので、器の名前の方が強い同一性を持つ）
        let with_container = PendingSpawn {
            tmux_session: Some("tako-abc".into()),
            pane: Some(5),
            ..Default::default()
        };
        assert!(with_container.matches(Some("tako-abc"), None));
        assert!(!with_container.matches(Some("tako-xyz"), Some(5)));
        // 器ありの記録は、器なしの検出（ペイン ID だけ）には一致しない
        assert!(!with_container.matches(None, Some(5)));

        let without = PendingSpawn {
            tmux_session: None,
            pane: Some(5),
            ..Default::default()
        };
        assert!(without.matches(None, Some(5)));
        assert!(!without.matches(None, Some(6)));
        assert!(!without.matches(Some("tako-abc"), Some(5)));

        // キーを 1 つも持たない記録は何にも一致しない
        let keyless = PendingSpawn::default();
        assert!(!keyless.matches(None, None));
        assert!(!keyless.matches(Some("tako-abc"), Some(5)));
    }

    #[test]
    fn 旧形式のpending記録がそのまま読める() {
        // #728 でスキーマを String -> Option<String> にしたが、serde は
        // 素の文字列を Some(...) として読む。移行 Step は要らない（#916 の判断）
        let old = "entries: {}\npending:\n  - tmux_session: tako-s42\n    kind: worker\n\
                   \n    recorded_at: '2026-08-01T00:00:00Z'\n";
        let catalog: SessionCatalog = serde_yaml::from_str(old).expect("旧形式が読める");
        assert_eq!(catalog.pending.len(), 1);
        assert_eq!(
            catalog.pending[0].tmux_session.as_deref(),
            Some("tako-s42"),
            "旧形式の素の文字列が Some として読める"
        );
        // 器の名前が無い（= 新形式で器なし）記録も読める
        let containerless =
            "pending:\n  - kind: worker\n    pane: 12\n    recorded_at: '2026-08-01T00:00:00Z'\n";
        let catalog: SessionCatalog = serde_yaml::from_str(containerless).expect("器なしが読める");
        assert_eq!(catalog.pending[0].tmux_session, None);
        assert_eq!(catalog.pending[0].pane, Some(12));
    }

    #[test]
    fn issue番号の抽出() {
        assert_eq!(
            extract_issues("Issue #112 を実装。#157 と衝突注意"),
            vec![112, 157]
        );
        assert_eq!(extract_issues("#112 #112 #112"), vec![112]);
        assert_eq!(extract_issues("番号なし # だけ"), Vec::<u32>::new());
        assert_eq!(
            extract_issues("色コード #ffffff は数字なら拾う"),
            Vec::<u32>::new()
        );
        assert_eq!(
            extract_issues("fix/112-log は # なしなので拾わない"),
            Vec::<u32>::new()
        );
    }

    /// #2199: 書いた順で並べ、先頭が主の Issue になる（修正前は昇順で先頭が最小の番号）
    #[test]
    fn issue2199_書いた順で先頭が主のissue() {
        // ラベルの番号が先頭。プロンプトが前例として触れた小さい番号は後ろへ
        let issues = spawn_issues(
            Some("#2192 usage S0"),
            "前例は #322 と #1877。#2192 の設計書を読む",
        );
        assert_eq!(issues, vec![2192, 322, 1877]);
        assert_eq!(issues.first(), Some(&2192));
        // ラベルに番号が複数あれば書いた順で最初（束ねた worker の `#A / #B: …`）
        assert_eq!(
            spawn_issues(Some("#2181 / #2157: ネット復帰"), "#100 を参照").first(),
            Some(&2181)
        );
        // ラベルに番号が無ければプロンプトに書いた順で最初
        assert_eq!(
            spawn_issues(Some("ledger-primary"), "Issue #2199 を直す。#322 の原則"),
            vec![2199, 322]
        );
        assert_eq!(
            spawn_issues(None, "#2199 を直す。#322 の原則").first(),
            Some(&2199)
        );
        // どこにも番号が無ければ空（台帳の `issue` は付かない）
        assert!(spawn_issues(Some("調査"), "番号の無い依頼").is_empty());
        // 修正前の対照（`TAKO_2199_LEGACY=1` の腕）: 昇順で先頭が最小の番号
        let legacy = extract_issues_with("#2192 usage S0 前例は #322 と #1877", true);
        assert_eq!(legacy, vec![322, 1877, 2192]);
        assert_eq!(
            extract_issues_with("#2192 usage S0 前例は #322 と #1877", false),
            vec![2192, 322, 1877]
        );
    }

    #[test]
    fn roleの分類() {
        let w = parse_role("orchestrator-worker:tako:112-session-log");
        assert_eq!(w.kind, "worker");
        assert_eq!(w.project.as_deref(), Some("tako"));
        assert_eq!(w.label.as_deref(), Some("112-session-log"));

        let w2 = parse_role("orchestrator-worker:tako");
        assert_eq!(w2.kind, "worker");
        assert_eq!(w2.project.as_deref(), Some("tako"));
        assert_eq!(w2.label, None);

        let m = parse_role("orchestrator-master");
        assert_eq!(m.kind, "master");
        assert_eq!(m.profile.as_deref(), Some("default"));
        assert_eq!(
            parse_role("orchestrator-master:sol").profile.as_deref(),
            Some("sol")
        );

        assert_eq!(parse_role("solo").kind, "solo");
        assert_eq!(parse_role("solo:fast").profile.as_deref(), Some("fast"));
        assert_eq!(parse_role("dev-server").kind, "pane");
    }

    #[test]
    fn prompt_headの正規化と切り詰め() {
        assert_eq!(prompt_head("a\nb\n  c", 100), "a b c");
        let long = "あ".repeat(50);
        let head = prompt_head(&long, 10);
        assert_eq!(head.chars().count(), 11);
        assert!(head.ends_with('…'));
    }

    /// プロンプト冒頭の抜粋の固定値（#1790）。内容は最大 `max` 文字で `…` は上限の外・
    /// 空白は畳んでから数える。本番の呼び手は spawn の記録（`max` = 200）
    #[test]
    fn prompt_headの切り詰めの固定値() {
        // ちょうど max 文字は切らない
        assert_eq!(prompt_head(&"あ".repeat(10), 10), "あ".repeat(10));
        // max + 1 文字ちょうどは原文のまま（寄せる前は「max 文字 + `…`」= 同じ幅で
        // 末尾 1 文字を `…` に置き換えていた。#1790 で寄せた後の唯一の差）
        assert_eq!(prompt_head(&"あ".repeat(11), 10), "あ".repeat(11));
        // max + 2 文字以上は内容 max 文字 + `…`
        assert_eq!(
            prompt_head(&"あ".repeat(12), 10),
            format!("{}…", "あ".repeat(10))
        );
        // 4 バイト文字も 1 文字・結合文字（NFD の「が」）は濁点を別の 1 文字に数える
        assert_eq!(
            prompt_head(&"🎉".repeat(300), 200),
            format!("{}…", "🎉".repeat(200))
        );
        assert_eq!(
            prompt_head(&"か\u{3099}".repeat(10), 5),
            "か\u{3099}か\u{3099}か…"
        );
        // 改行・タブ・全角空白は半角空白 1 個へ畳み、前後は落としてから数える
        assert_eq!(
            prompt_head("\n  Issue #12\tを\u{3000}直す  \n", 200),
            "Issue #12 を 直す"
        );
        assert_eq!(prompt_head("a\n\nb c d", 3), "a b…");
        // max = 0 は `…` だけ（1 文字ちょうどは上の差と同じで原文）・max が最大でもあふれない
        assert_eq!(prompt_head("abc", 0), "…");
        assert_eq!(prompt_head("a", 0), "a");
        assert_eq!(prompt_head("abc", usize::MAX), "abc");
        // 空・空白だけは空
        assert_eq!(prompt_head("", 10), "");
        assert_eq!(prompt_head("", 0), "");
        assert_eq!(prompt_head(" \n\t", 10), "");
    }

    #[test]
    fn pending記録と昇格の統合() {
        let path = temp_path("promote");
        // spawn 時の pending 記録
        SessionCatalog::mutate_at(&path, |c| {
            c.pending.push(PendingSpawn {
                tmux_session: Some("tako-s42".into()),
                kind: "worker".into(),
                label: Some("112-session-log".into()),
                project: Some("tako".into()),
                agent: Some("claude".into()),
                model: None,
                effort: Some("max".into()),
                issues: vec![112],
                prompt_head: Some("Issue #112 を実装する".into()),
                cwd: Some("/work/tako".into()),
                tab: Some(3),
                pane: Some(7),
                recorded_at: "2026-07-13T00:00:00Z".into(),
            });
        })
        .unwrap();

        // 検出 → 昇格（テストは mutate_at ベースの sync を直接再現する）
        let detected = vec![DetectedSession {
            session_id: "11111111-2222-3333-4444-555555555555".into(),
            tmux_session: Some("tako-s42".into()),
            pane: Some(7),
            agent_cwd: Some("/work/tako".into()),
            model: Some("claude-fable-5".into()),
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 7,
            tab: 3,
            tmux_session: Some("tako-s42".into()),
            role: Some("orchestrator-worker:tako:112-session-log".into()),
            title: Some("tako: 112-session-log".into()),
            cwd: Some("/work/tako".into()),
            log_file: Some("/logs/x.log".into()),
        }];
        sync_detected_at(&path, &detected, &panes).unwrap();

        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert!(catalog.pending.is_empty(), "pending が昇格で消える");
        let entry = &catalog.entries["11111111-2222-3333-4444-555555555555"];
        assert_eq!(entry.kind, "worker");
        assert_eq!(entry.project.as_deref(), Some("tako"));
        assert_eq!(entry.label.as_deref(), Some("112-session-log"));
        assert_eq!(entry.issues, vec![112]);
        assert_eq!(entry.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(entry.effort.as_deref(), Some("max"));
        assert_eq!(entry.cwd.as_deref(), Some("/work/tako"));
        assert_eq!(entry.pane, Some(7));
        assert_eq!(entry.started_at, "2026-07-13T00:00:00Z");
        assert!(!entry.last_seen_at.is_empty());
        assert_eq!(entry.log_file.as_deref(), Some("/logs/x.log"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 器がない構成でもpending記録がペインidで昇格する() {
        // #728: Windows で psmux 未導入（backend=none）の主経路。
        // 器の名前が付かないので、spawn 記録も検出もペイン ID がキーになる
        let path = temp_path("promote-container-less");
        SessionCatalog::mutate_at(&path, |c| {
            c.pending.push(PendingSpawn {
                tmux_session: None,
                kind: "worker".into(),
                label: Some("728-sessions".into()),
                project: Some("tako".into()),
                agent: Some("claude".into()),
                issues: vec![728],
                prompt_head: Some("Issue #728 を実装する".into()),
                cwd: Some("C:\\work\\tako".into()),
                tab: Some(1),
                pane: Some(12),
                recorded_at: "2026-08-01T00:00:00Z".into(),
                ..Default::default()
            });
        })
        .unwrap();

        let detected = vec![DetectedSession {
            session_id: "22222222-3333-4444-5555-666666666666".into(),
            tmux_session: None,
            pane: Some(12),
            agent_cwd: Some("C:\\work\\tako".into()),
            model: Some("claude-opus-5".into()),
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 12,
            tab: 1,
            tmux_session: None,
            role: Some("orchestrator-worker:tako:728-sessions".into()),
            title: Some("tako: 728-sessions".into()),
            cwd: Some("C:\\work\\tako".into()),
            log_file: None,
        }];
        sync_detected_at(&path, &detected, &panes).unwrap();

        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert!(catalog.pending.is_empty(), "pending が昇格で消える");
        let entry = &catalog.entries["22222222-3333-4444-5555-666666666666"];
        assert_eq!(entry.kind, "worker");
        assert_eq!(entry.label.as_deref(), Some("728-sessions"));
        assert_eq!(entry.issues, vec![728]);
        assert_eq!(entry.pane, Some(12));
        assert_eq!(entry.tab, Some(1));
        assert_eq!(entry.cwd.as_deref(), Some("C:\\work\\tako"));
        // 器が無いので名前は付かない（「観測漏れ」ではなく「器が無い」）
        assert_eq!(entry.tmux_session, None);
        assert_eq!(entry.started_at, "2026-08-01T00:00:00Z");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pending無しでもroleから分類される() {
        let path = temp_path("role-only");
        let detected = vec![DetectedSession {
            session_id: "aaaaaaaa-1111-2222-3333-444444444444".into(),
            tmux_session: Some("tako-s9".into()),
            pane: None,
            agent_cwd: None,
            model: None,
        }];
        let panes = vec![PaneMetaSnapshot {
            pane: 2,
            tab: 1,
            tmux_session: Some("tako-s9".into()),
            role: Some("orchestrator-master:sol".into()),
            title: None,
            cwd: Some("/home/u".into()),
            log_file: None,
        }];
        sync_detected_at(&path, &detected, &panes).unwrap();
        let catalog = SessionCatalog::load_from(&path).unwrap();
        let entry = &catalog.entries["aaaaaaaa-1111-2222-3333-444444444444"];
        assert_eq!(entry.kind, "master");
        assert_eq!(entry.profile.as_deref(), Some("sol"));
        assert_eq!(entry.agent.as_deref(), Some("claude"));
        assert_eq!(entry.cwd.as_deref(), Some("/home/u"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn resolve_idの前方一致と曖昧エラー() {
        let mut catalog = SessionCatalog::default();
        catalog
            .entries
            .insert("abc123".into(), SessionEntry::default());
        catalog
            .entries
            .insert("abd456".into(), SessionEntry::default());
        assert!(catalog.resolve_id("abc").is_ok());
        assert!(catalog.resolve_id("abc123").is_ok());
        let err = catalog.resolve_id("ab").unwrap_err();
        assert!(err.contains("2 件"), "{err}");
        assert!(catalog.resolve_id("zzz").is_err());
    }

    #[test]
    fn resume_commandの組み立てと系統ごとの書式() {
        let entry = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("112-log".into()),
            agent: Some("claude".into()),
            model: Some("claude-fable-5".into()),
            effort: Some("max".into()),
            ..Default::default()
        };
        let cmd = resume_command_with_env("11111111-2222-3333-4444-555555555555", &entry, None, "")
            .unwrap();
        assert_eq!(
            cmd,
            "TAKO_ORCHESTRATOR_ROLE='worker:tako:112-log' claude --model claude-fable-5 --effort max --resume 11111111-2222-3333-4444-555555555555"
        );

        let solo = SessionEntry {
            kind: "solo".into(),
            profile: Some("default".into()),
            ..Default::default()
        };
        let cmd = resume_command_with_env("abc", &solo, None, "").unwrap();
        assert!(
            cmd.starts_with("TAKO_ORCHESTRATOR_ROLE=solo claude"),
            "{cmd}"
        );

        // #1238: codex は `resume` サブコマンド + 位置引数。**`--model` は付けない**
        // （`codex resume --help` に無い = 付けると起動しない）
        let codex = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            agent: Some("codex".into()),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env("01a08347-543b-7a20-8e49-a93380efb375", &codex, None, "").unwrap(),
            "TAKO_ORCHESTRATOR_ROLE='worker:tako' codex resume 01a08347-543b-7a20-8e49-a93380efb375"
        );

        // #1238: agy はグローバルフラグ。会話に記録された設定を上書きしない
        let agy = SessionEntry {
            agent: Some("agy".into()),
            model: Some("Claude Opus 4.6 (Thinking)".into()),
            ..Default::default()
        };
        assert_eq!(
            resume_command_with_env("874efd16-af95-4b2e-8036-84f6c3eaded3", &agy, None, "")
                .unwrap(),
            "agy --conversation 874efd16-af95-4b2e-8036-84f6c3eaded3"
        );

        // ローカル LLM は会話の器が定まっていない（#991）ので断る
        let local = SessionEntry {
            agent: Some("local".into()),
            ..Default::default()
        };
        let err = resume_command_with_env("abc", &local, None, "").unwrap_err();
        assert!(err.contains("resume 非対応"), "{err}");

        // 書式検証（パストラバーサル防止）は全系統に効く
        for agent in ["claude", "codex", "agy"] {
            let invalid = SessionEntry {
                agent: Some(agent.into()),
                ..Default::default()
            };
            assert!(
                resume_command_with_env("../etc", &invalid, None, "").is_err(),
                "{agent} が不正な ID を通した"
            );
        }
    }

    /// Issue #652: アカウントの会話は `CLAUDE_CONFIG_DIR` を前置しないと
    /// `No conversation found with session ID` で resume に失敗する。
    /// 前置はコマンド先頭（rc / direnv より後に効く位置。#500 / #512 と同型）
    #[test]
    fn resume_commandはconfigdirを先頭に前置する() {
        let entry = SessionEntry {
            kind: "master".into(),
            profile: Some("takodev".into()),
            agent: Some("claude".into()),
            ..Default::default()
        };
        let cmd = resume_command_with_env(
            "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4",
            &entry,
            Some("export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; "),
            "",
        )
        .unwrap();
        assert_eq!(
            cmd,
            "export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; \
             TAKO_ORCHESTRATOR_ROLE='master:takodev' claude \
             --resume e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4"
        );
    }

    /// テスト用の最小の判断材料（claude・macOS・器は死んでいる）
    fn input<'a>(id: Option<&'a str>, env: Option<&'a str>) -> RestoreInput<'a> {
        RestoreInput {
            backend_alive: false,
            resume: id.map(|id| PaneResume {
                agent: Agent::Claude,
                id: Some(id),
            }),
            conversation: env,
            catalog: None,
            backend_session: None,
            pane: None,
            platform: Platform::MacOs,
            worker_mcp_args: None,
        }
    }

    /// Issue #1076: PC 再起動後の復元は、器が消えたペインを保存済みの会話へ戻す。
    /// **役割 env / `--model` まで復元する**（最小形 `claude --resume <id>` だと
    /// 戻ってきた claude が master / worker として認識されない）
    #[test]
    fn restore_planはカタログの起動条件ごと復元する() {
        let id = "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4";
        let env = "export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; ";
        let mut catalog = SessionCatalog::default();
        catalog.entries.insert(
            id.into(),
            SessionEntry {
                kind: "master".into(),
                profile: Some("takodev".into()),
                agent: Some("claude".into()),
                model: Some("claude-opus-5".into()),
                ..Default::default()
            },
        );
        let plan = restore_plan_in(RestoreInput {
            catalog: Some(&catalog),
            ..input(Some(id), Some(env))
        });
        assert_eq!(
            plan,
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!(
                    "{env}TAKO_ORCHESTRATOR_ROLE='master:takodev' \
                     claude --model claude-opus-5 --resume {id}"
                ),
                with_role: true,
            }
        );
    }

    /// カタログに記録が無いペインは最小形へ落ちる（従来の挙動）
    #[test]
    fn restore_planはカタログ不在なら最小形へ落ちる() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        let env = "unset CLAUDE_CONFIG_DIR; ";
        let empty = SessionCatalog::default();
        let minimal = RestorePlan::Resume {
            agent: Agent::Claude,
            command: format!("{env}claude --resume {id}"),
            with_role: false,
        };
        assert_eq!(
            restore_plan_in(RestoreInput {
                catalog: Some(&empty),
                ..input(Some(id), Some(env))
            }),
            minimal
        );
        // カタログそのものが読めなかった場合も同じ
        assert_eq!(restore_plan_in(input(Some(id), Some(env))), minimal);
    }

    /// 新規シェルへ落ちる理由が区別できる（復元内訳のログに出す。#1076 / #1238）
    #[test]
    fn restore_planは新規シェルの理由を返す() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        let env = "unset CLAUDE_CONFIG_DIR; ";
        // 器が生きている = 実行中プロセスごと再 attach（claude を二重起動しない）
        assert_eq!(
            restore_plan_in(RestoreInput {
                backend_alive: true,
                ..input(Some(id), Some(env))
            }),
            RestorePlan::Reattach
        );
        assert_eq!(
            restore_plan_in(input(None, Some(env))),
            RestorePlan::FreshShell(FreshShellReason::NoSessionId)
        );
        assert_eq!(
            restore_plan_in(input(Some("../../bad"), Some(env))),
            RestorePlan::FreshShell(FreshShellReason::InvalidSessionId)
        );
        // transcript が見つからない = resume しても `No conversation found` になる
        assert_eq!(
            restore_plan_in(input(Some(id), None)),
            RestorePlan::FreshShell(FreshShellReason::TranscriptMissing)
        );
        // #1238: 系統は分かるが ID が採れていない（agy の最初のターン前）
        assert_eq!(
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Agy,
                    id: None
                }),
                ..input(None, Some(env))
            }),
            RestorePlan::FreshShell(FreshShellReason::NoSessionId)
        );
    }

    /// #1238: 「保存されていない」と「この環境には手段が無い」を別の理由として出す。
    /// Windows には `lsof` が無いので codex / agy の ID をそもそも採れない
    #[test]
    fn restore_planはwindowsのcodexとagyをresume非対応と言う() {
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        for agent in [Agent::Codex, Agent::Agy] {
            assert_eq!(
                restore_plan_in(RestoreInput {
                    resume: Some(PaneResume {
                        agent,
                        id: Some(id)
                    }),
                    platform: Platform::Windows,
                    ..input(None, Some(""))
                }),
                RestorePlan::FreshShell(FreshShellReason::ResumeUnsupported),
                "{agent:?}"
            );
        }
        // claude の ID は OS に依らず取れる（`claude agents --json`）
        assert!(matches!(
            restore_plan_in(RestoreInput {
                platform: Platform::Windows,
                ..input(Some(id), Some(""))
            }),
            RestorePlan::Resume { .. }
        ));
        assert_eq!(
            FreshShellReason::ResumeUnsupported.label(),
            "resume 非対応",
            "内訳ログのラベルが変わると commands.md の読み方が合わなくなる"
        );
    }

    /// #1238: codex / agy は spawn 記録（pending）から役割ごと復元する。
    /// claude と違ってカタログ本体へ昇格しない（FR-5.12 の制限）ので、
    /// ここを見ないと戻ってきた worker がオーケストレーターから見えない
    #[test]
    fn restore_planはcodexとagyをpending記録の役割ごと復元する() {
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        let mut catalog = SessionCatalog::default();
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("tako-w2".into()),
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("1238".into()),
            agent: Some("codex".into()),
            ..Default::default()
        });
        assert_eq!(
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Codex,
                    id: Some(id)
                }),
                catalog: Some(&catalog),
                backend_session: Some("tako-w2"),
                ..input(None, Some(""))
            }),
            RestorePlan::Resume {
                agent: Agent::Codex,
                command: format!("TAKO_ORCHESTRATOR_ROLE='worker:tako:1238' codex resume {id}"),
                with_role: true,
            }
        );
        // 別の系統を起動していたペインの記録は使わない（役割を取り違えない）
        assert_eq!(
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Agy,
                    id: Some(id)
                }),
                catalog: Some(&catalog),
                backend_session: Some("tako-w2"),
                ..input(None, Some(""))
            }),
            RestorePlan::Resume {
                agent: Agent::Agy,
                command: format!("agy --conversation {id}"),
                with_role: false,
            }
        );
    }

    /// #2447: 復元する会話が worker なら、決めた MCP の断片を会話 ID の前へ差し込む
    /// （codex は `codex resume` の後・claude は `--resume` の前）。worker 以外と最小形には足さない
    #[test]
    fn restore_planはworkerの再開へmcpの断片を差し込む() {
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        let frag = " -c 'mcp_servers.x.enabled=false' -c 'mcp_servers.x.command=tako-disabled'";
        let mut catalog = SessionCatalog::default();
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("tako-w2".into()),
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("2447".into()),
            agent: Some("codex".into()),
            cwd: Some("/work/tako".into()),
            ..Default::default()
        });
        let codex = |args: Option<&str>| {
            restore_plan_in(RestoreInput {
                resume: Some(PaneResume {
                    agent: Agent::Codex,
                    id: Some(id),
                }),
                catalog: Some(&catalog),
                backend_session: Some("tako-w2"),
                worker_mcp_args: args,
                ..input(None, Some(""))
            })
        };
        assert_eq!(
            codex(Some(frag)),
            RestorePlan::Resume {
                agent: Agent::Codex,
                command: format!(
                    "TAKO_ORCHESTRATOR_ROLE='worker:tako:2447' codex resume{frag} {id}"
                ),
                with_role: true,
            }
        );
        // 断片が決まらない（`TAKO_2447_LEGACY=1` 等）なら #2447 前と 1 バイトも変わらない
        assert_eq!(
            codex(None),
            RestorePlan::Resume {
                agent: Agent::Codex,
                command: format!("TAKO_ORCHESTRATOR_ROLE='worker:tako:2447' codex resume {id}"),
                with_role: true,
            }
        );
        // spawn 記録の cwd は判断の材料として引き継ぐ（.codex/config.toml の同名を見る場所）
        let probe = RestoreInput {
            catalog: Some(&catalog),
            backend_session: Some("tako-w2"),
            ..input(None, Some(""))
        };
        assert_eq!(
            launch_record(&probe, id, Agent::Codex).and_then(|e| e.cwd),
            Some("/work/tako".to_string())
        );

        // claude の worker は `--resume` の前（spawn と同じ `--settings`）
        let cid = "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4";
        let env = "export CLAUDE_CONFIG_DIR=/Users/me/.claude-univ; ";
        let settings = " --settings /data/worker-mcp/deny-0123456789abcdef.json";
        let mut claude = SessionCatalog::default();
        claude.entries.insert(
            cid.into(),
            SessionEntry {
                kind: "worker".into(),
                project: Some("tako".into()),
                agent: Some("claude".into()),
                model: Some("claude-opus-5".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            restore_plan_in(RestoreInput {
                catalog: Some(&claude),
                worker_mcp_args: Some(settings),
                ..input(Some(cid), Some(env))
            }),
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!(
                    "{env}TAKO_ORCHESTRATOR_ROLE='worker:tako' claude --model claude-opus-5{settings} --resume {cid}"
                ),
                with_role: true,
            }
        );
        // master は spawn でも絞らない（#2412 の対象外）ので、断片が渡っても足さない
        let mut master = SessionCatalog::default();
        master.entries.insert(
            cid.into(),
            SessionEntry {
                kind: "master".into(),
                agent: Some("claude".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            restore_plan_in(RestoreInput {
                catalog: Some(&master),
                worker_mcp_args: Some(settings),
                ..input(Some(cid), Some(env))
            }),
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!("{env}TAKO_ORCHESTRATOR_ROLE=master claude --resume {cid}"),
                with_role: true,
            }
        );
        // 記録の無い会話（最小形）にも足さない
        assert_eq!(
            restore_plan_in(RestoreInput {
                worker_mcp_args: Some(settings),
                ..input(Some(cid), Some(env))
            }),
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!("{env}claude --resume {cid}"),
                with_role: false,
            }
        );
    }

    /// #2447 の主題: 同じ判断（`worker_mcp::prepare` の結果）から組んだ spawn と再開の起動コマンドは、
    /// **同じ MCP の断片**を持つ（組み立ては `agent::worker_mcp_args` の 1 実装）。
    /// codex = tako の MCP の一時注入（#986）+ user 設定を止める `-c`（#2418）、claude = `--settings`（#2412）
    #[test]
    fn spawnと再開は同じmcpの断片を持つ() {
        use crate::launch_cmd::ShellDialect;
        use crate::orchestrator::agent::{
            build_worker_cmd_in, worker_mcp_args, WorkerAgent, WorkerLaunch,
        };
        let t = tako_core::test_residue::ScratchDir::new("t2447");
        let cwd = t.path().join("p");
        std::fs::create_dir_all(&cwd).unwrap();
        let tako_bin = "/Applications/tako.app/Contents/MacOS/tako";

        // codex: user 設定に stdio と http の server（どちらも止まる）+ プロジェクトで足した名前
        let config = t.path().join("config.toml");
        std::fs::write(
            &config,
            "[mcp_servers.user-alpha]\ncommand = \"npx\"\n\n\
             [mcp_servers.user-web]\nurl = \"http://127.0.0.1:1/mcp\"\n\n\
             [mcp_servers.user-keep]\ncommand = \"uvx\"\n",
        )
        .unwrap();
        let mcp = crate::worker_mcp::prepare_codex_from(&cwd, &config, &["user-keep".to_string()]);
        let frag = worker_mcp_args(
            WorkerAgent::Codex,
            Some(tako_bin),
            None,
            &mcp.codex_off,
            ShellDialect::Posix,
        );
        for needle in [
            "mcp_servers.tako.command=",
            "mcp_servers.user-alpha.enabled=false",
            "mcp_servers.user-web.url=http://127.0.0.1:9/tako-disabled",
        ] {
            assert!(frag.contains(needle), "{needle} が無い: {frag}");
        }
        assert!(!frag.contains("user-keep"), "足した名前は止めない: {frag}");
        let spawn = build_worker_cmd_in(
            &WorkerLaunch {
                agent: WorkerAgent::Codex,
                role: "worker:tako:2447",
                tako_bin: Some(tako_bin),
                mcp_codex_off: &mcp.codex_off,
                ..Default::default()
            },
            ShellDialect::Posix,
        );
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        let entry = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            label: Some("2447".into()),
            agent: Some("codex".into()),
            ..Default::default()
        };
        let resume = resume_command_with_env(
            id,
            &entry,
            Some(""),
            &resume_mcp_args(WorkerAgent::Codex, &mcp, tako_bin, ShellDialect::Posix),
        )
        .unwrap();
        assert_eq!(
            spawn,
            format!("TAKO_ORCHESTRATOR_ROLE='worker:tako:2447' codex{frag}"),
            "spawn の断片が worker_mcp_args と違う"
        );
        assert_eq!(
            resume,
            format!("TAKO_ORCHESTRATOR_ROLE='worker:tako:2447' codex resume{frag} {id}"),
            "再開の断片が spawn と違う"
        );

        // claude: user scope の止める名前が `--settings` のファイルになる（spawn と同じパス）
        let claude_json = t.path().join(".claude.json");
        std::fs::write(
            &claude_json,
            r#"{"mcpServers": {"user-alpha": {"command": "npx"}, "tako": {"command": "tako", "args": ["mcp", "serve"]}}}"#,
        )
        .unwrap();
        let mcp = crate::worker_mcp::prepare_from(&cwd, &[claude_json], &[], &[]);
        assert_eq!(mcp.plan.denied, ["user-alpha"]);
        let settings = mcp
            .settings_file
            .clone()
            .expect("止める名前があれば設定ファイルが決まる");
        let spawn = build_worker_cmd_in(
            &WorkerLaunch {
                agent: WorkerAgent::Claude,
                role: "worker:tako",
                tako_bin: Some(tako_bin),
                mcp_settings: Some(&settings),
                ..Default::default()
            },
            ShellDialect::Posix,
        );
        let cid = "e16cde37-c0e0-4126-9ef4-9c6b0bfeccc4";
        let entry = SessionEntry {
            kind: "worker".into(),
            project: Some("tako".into()),
            agent: Some("claude".into()),
            ..Default::default()
        };
        let resume = resume_command_with_env(
            cid,
            &entry,
            None,
            &resume_mcp_args(WorkerAgent::Claude, &mcp, tako_bin, ShellDialect::Posix),
        )
        .unwrap();
        let want = format!(
            " --settings {}",
            crate::launch_cmd::quote(ShellDialect::Posix, &settings)
        );
        assert!(spawn.ends_with(&want), "{spawn}");
        assert_eq!(
            resume,
            format!("TAKO_ORCHESTRATOR_ROLE='worker:tako' claude{want} --resume {cid}")
        );
    }

    /// #2447: 再開の MCP を決めるのは worker だけ（master / solo / 役割の無いペインは spawn でも絞らない）
    #[test]
    fn 再開のmcpはworkerだけが決める() {
        let id = "01a08347-543b-7a20-8e49-a93380efb375";
        for kind in ["master", "solo", "pane", ""] {
            let entry = SessionEntry {
                kind: kind.into(),
                agent: Some("codex".into()),
                ..Default::default()
            };
            assert!(
                resume_worker_mcp(Agent::Codex, id, &entry).is_none(),
                "{kind} に MCP の判断が付いた"
            );
        }
        // worker になれない系統（ローカル LLM）も決めない
        let local = SessionEntry {
            kind: "worker".into(),
            agent: Some("local".into()),
            ..Default::default()
        };
        assert!(resume_worker_mcp(Agent::Local, id, &local).is_none());
    }

    /// カタログの記録の系統が layout と食い違っても、**実際に動いていた系統**の
    /// コマンドを組む（記録の齟齬で会話への入口を失わない）
    #[test]
    fn restore_planは記録の齟齬よりlayoutの系統を優先する() {
        let id = "a45899a8-96a6-4fa6-9bf6-71df53307878";
        let env = "unset CLAUDE_CONFIG_DIR; ";
        let mut catalog = SessionCatalog::default();
        catalog.entries.insert(
            id.into(),
            SessionEntry {
                agent: Some("codex".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            restore_plan_in(RestoreInput {
                catalog: Some(&catalog),
                ..input(Some(id), Some(env))
            }),
            RestorePlan::Resume {
                agent: Agent::Claude,
                command: format!("{env}claude --resume {id}"),
                with_role: false,
            }
        );
    }

    #[test]
    fn gcはpending期限とエントリ上限を強制する() {
        let mut catalog = SessionCatalog::default();
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("old".into()),
            recorded_at: "2026-07-01T00:00:00Z".into(),
            ..Default::default()
        });
        catalog.pending.push(PendingSpawn {
            tmux_session: Some("fresh".into()),
            recorded_at: "2026-07-13T00:00:00Z".into(),
            ..Default::default()
        });
        for i in 0..(MAX_ENTRIES + 10) {
            catalog.entries.insert(
                format!("session-{i:04}"),
                SessionEntry {
                    last_seen_at: format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
                    ..Default::default()
                },
            );
        }
        gc(&mut catalog, "2026-07-13T12:00:00Z");
        assert_eq!(catalog.pending.len(), 1);
        assert_eq!(catalog.pending[0].tmux_session.as_deref(), Some("fresh"));
        assert_eq!(catalog.entries.len(), MAX_ENTRIES);
        // 最も古い 10 件が消えている
        assert!(!catalog.entries.contains_key("session-0000"));
        assert!(catalog
            .entries
            .contains_key(&format!("session-{:04}", MAX_ENTRIES + 9)));
    }

    #[test]
    fn iso時刻の往復() {
        // civil_utc（format）↔ parse_iso の整合
        let secs = 1_784_000_000; // 2026-07 前後
        let iso = crate::diag::format_utc(secs);
        assert_eq!(parse_iso(&iso), Some(secs));
        assert_eq!(
            parse_iso("2026-07-13T00:00:00Z").map(|s| s % 86_400),
            Some(0)
        );
        assert_eq!(parse_iso("broken"), None);
    }

    #[test]
    fn 破損カタログはerrで丸めない() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "entries:\n  x:\n    kind: [broken").unwrap();
        assert!(SessionCatalog::load_from(&path).is_err());
        let result = SessionCatalog::mutate_at(&path, |c| c.entries.clear());
        assert!(result.is_err(), "破損時は書き込まない");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn ペイン解決は同一ペインの複数世代から最新を選ぶ() {
        // #466: 同一 pane 番号のエントリは世代を跨いで堆積する。辞書順の
        // 先勝ちだと stale な旧世代を返すことがあった。last_seen_at 最新を選ぶ
        let mut catalog = SessionCatalog::default();
        for (sid, seen) in [
            ("session-old", "2026-07-20T00:00:00Z"),
            ("session-new", "2026-07-22T00:00:00Z"),
            ("session-mid", "2026-07-21T00:00:00Z"),
        ] {
            catalog.entries.insert(
                sid.to_string(),
                SessionEntry {
                    pane: Some(648),
                    last_seen_at: seen.to_string(),
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            resolve_session_for_pane_in(&catalog, "648").as_deref(),
            Some("session-new")
        );
        // 該当ペインなし・数値でない ID は None
        assert_eq!(resolve_session_for_pane_in(&catalog, "649"), None);
        assert_eq!(resolve_session_for_pane_in(&catalog, "tako-a:0.0"), None);
    }

    #[test]
    fn ペイン解決はlast_seen_at欠落時にstarted_atへフォールバックする() {
        let mut catalog = SessionCatalog::default();
        catalog.entries.insert(
            "session-seen".to_string(),
            SessionEntry {
                pane: Some(7),
                started_at: "2026-07-01T00:00:00Z".to_string(),
                last_seen_at: "2026-07-10T00:00:00Z".to_string(),
                ..Default::default()
            },
        );
        catalog.entries.insert(
            "session-started-only".to_string(),
            SessionEntry {
                pane: Some(7),
                started_at: "2026-07-15T00:00:00Z".to_string(),
                last_seen_at: String::new(),
                ..Default::default()
            },
        );
        assert_eq!(
            resolve_session_for_pane_in(&catalog, "7").as_deref(),
            Some("session-started-only")
        );
    }

    // --- #2017: mod の報告からの検出と走査の検出の合成 ---------------------------------

    const SID_2017_A: &str = "2017aaaa-0000-4000-8000-00000000000a";
    const SID_2017_B: &str = "2017aaaa-0000-4000-8000-00000000000b";

    #[test]
    fn issue2017_modの報告からの検出は器があれば器名_無ければペインidで対応付ける() {
        let d =
            detected_from_mod(3, Some("tako-s3"), SID_2017_A, Some("claude-haiku-5-5")).unwrap();
        assert_eq!(d.tmux_session.as_deref(), Some("tako-s3"));
        assert_eq!(
            d.pane, None,
            "器があれば走査の検出（detect_from_agents_value）と同じ器名のキー"
        );
        assert_eq!(d.agent_cwd, None);
        assert_eq!(d.model.as_deref(), Some("claude-haiku-5-5"));
        let d = detected_from_mod(4, None, SID_2017_A, None).unwrap();
        assert_eq!((d.tmux_session, d.pane), (None, Some(4)));
        assert!(
            detected_from_mod(4, None, "../../etc", None).is_none(),
            "会話 ID として使えない値は載せない（transcript の参照キー）"
        );
    }

    #[test]
    fn issue2017_同じペイン同じ会話は走査の検出を採り食い違えばmodを採る() {
        let scan = |backend: &str, sid: &str| DetectedSession {
            session_id: sid.into(),
            tmux_session: Some(backend.into()),
            pane: None,
            agent_cwd: Some("/w".into()),
            model: None,
        };
        let scanned = vec![scan("tako-s1", SID_2017_A), scan("tako-s3", SID_2017_A)];
        let from_mod = vec![
            // 同じ会話 = 走査の検出（cwd つき）のまま
            detected_from_mod(1, Some("tako-s1"), SID_2017_A, None).unwrap(),
            // 食い違い（/clear の後）= mod の会話 ID を採る
            detected_from_mod(3, Some("tako-s3"), SID_2017_B, None).unwrap(),
            // 走査が拾わなかったペイン = 足す
            detected_from_mod(2, Some("tako-s2"), SID_2017_B, None).unwrap(),
            detected_from_mod(9, None, SID_2017_B, None).unwrap(),
        ];
        let merged = merge_detected(scanned, from_mod);
        assert_eq!(merged.len(), 4);
        assert_eq!(
            (
                merged[0].session_id.as_str(),
                merged[0].agent_cwd.as_deref()
            ),
            (SID_2017_A, Some("/w"))
        );
        assert_eq!(
            (
                merged[1].session_id.as_str(),
                merged[1].tmux_session.as_deref()
            ),
            (SID_2017_B, Some("tako-s3"))
        );
        assert_eq!(merged[2].tmux_session.as_deref(), Some("tako-s2"));
        assert_eq!(merged[3].pane, Some(9));
    }

    // ---------------------------------------------------------------------
    // #2020: last_seen_at だけの更新の間引き
    // ---------------------------------------------------------------------

    /// 走査の基準時刻（2026-09 頃）。走査の間隔は本番と同じ 30 秒で進める
    const T0: i64 = 1_790_000_000;
    const SCAN: i64 = 30;
    /// 並び順の比較でキーの順が効くように、先頭の文字で順を決めておく
    const ID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const ID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    const ID_C: &str = "cccccccc-0000-4000-8000-000000000003";
    const ID_D: &str = "dddddddd-0000-4000-8000-000000000004";
    const ID_E: &str = "eeeeeeee-0000-4000-8000-000000000005";
    const ID_F: &str = "ffffffff-0000-4000-8000-000000000006";

    /// 器つきの検出 1 件（器の名前はペイン番号から作る）
    fn det(id: &str, pane: u64) -> DetectedSession {
        DetectedSession {
            session_id: id.to_string(),
            tmux_session: Some(format!("tako-{pane}")),
            pane: Some(pane),
            agent_cwd: Some(format!("/tmp/w{pane}")),
            model: Some("claude-opus-5".into()),
        }
    }

    fn iso(secs: i64) -> String {
        crate::diag::format_utc(secs)
    }

    /// 書いたかどうかの目印。serde_yaml は書き出しでコメントを残さないので、
    /// **ファイルを書き直すと消える**（inode や mtime に頼らず OS をまたいで同じ判定）
    const MARK: &str = "# issue2020-mark\n";

    fn mark(path: &Path) {
        let content = std::fs::read_to_string(path).unwrap();
        std::fs::write(path, format!("{MARK}{content}")).unwrap();
    }

    fn rewritten(path: &Path) -> bool {
        !std::fs::read_to_string(path).unwrap().starts_with(MARK)
    }

    fn bak(path: &Path, generation: u32) -> Option<String> {
        std::fs::read_to_string(crate::config_io::backup_path(path, generation)).ok()
    }

    #[test]
    fn issue2020_last_seen_atだけの変化では書かずbakも回さない() {
        let path = temp_path("2020-skip");
        let mut th = LastSeenThrottle::default();
        let ab = [det(ID_A, 1), det(ID_B, 2)];
        assert_eq!(
            sync_detected_throttled(&path, &ab, &[], T0, &mut th).unwrap(),
            SyncWrite::Structural,
            "最初の走査は新しい会話 = 即時に書く"
        );
        // `.bak` が 1 世代ある状態から始める（回したら中身が変わる / 2 世代目ができる）
        let abc = [det(ID_A, 1), det(ID_B, 2), det(ID_C, 3)];
        sync_detected_throttled(&path, &abc, &[], T0 + SCAN, &mut th).unwrap();
        let bak1 = bak(&path, 1).expect("新しい会話の保存は .bak を回す");
        mark(&path);
        // 窓（600 秒）の中の走査はどれも last_seen_at だけの変化 = 書かない・.bak を回さない
        for i in 2..=20 {
            let now = T0 + SCAN * i;
            assert_eq!(
                sync_detected_throttled(&path, &abc, &[], now, &mut th).unwrap(),
                SyncWrite::Skipped,
                "{i} 回目の走査（last_seen_at だけの変化）で書いた"
            );
            assert!(!rewritten(&path), "{i} 回目の走査でファイルを書き直した");
            assert_eq!(
                bak(&path, 1).as_ref(),
                Some(&bak1),
                "{i} 回目で .bak を回した"
            );
            assert_eq!(bak(&path, 2), None, "{i} 回目で .bak を回した");
        }
        // 最後に書いてから 600 秒 = last_seen_at だけを書く。.bak は回さない
        let due = T0 + SCAN + LAST_SEEN_FLUSH_SECS;
        assert_eq!(
            sync_detected_throttled(&path, &abc, &[], due, &mut th).unwrap(),
            SyncWrite::LastSeenOnly
        );
        assert!(rewritten(&path), "窓が過ぎたのに書いていない");
        assert_eq!(
            bak(&path, 1).as_ref(),
            Some(&bak1),
            "last_seen_at だけの保存で .bak を回した"
        );
        assert_eq!(bak(&path, 2), None);
        let catalog = SessionCatalog::load_from(&path).unwrap();
        for id in [ID_A, ID_B, ID_C] {
            assert_eq!(catalog.entries[id].last_seen_at, iso(due));
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn issue2020_last_seen_at以外の変化では即時に書きbakを回す() {
        // (名前, 2 回目の走査の検出, 2 回目の走査のペインのメタ)
        type Case = (&'static str, Vec<DetectedSession>, Vec<PaneMetaSnapshot>);
        let moved = DetectedSession {
            tmux_session: Some("tako-9".into()),
            pane: Some(9),
            ..det(ID_A, 1)
        };
        let cases: Vec<Case> = vec![
            ("新しい会話", vec![det(ID_A, 1), det(ID_B, 2)], vec![]),
            (
                "model",
                vec![DetectedSession {
                    model: Some("claude-fable-5".into()),
                    ..det(ID_A, 1)
                }],
                vec![],
            ),
            ("pane / 器", vec![moved], vec![]),
            (
                "cwd",
                vec![DetectedSession {
                    agent_cwd: Some("/tmp/other".into()),
                    ..det(ID_A, 1)
                }],
                vec![],
            ),
            (
                "tab / ペインログ",
                vec![det(ID_A, 1)],
                vec![PaneMetaSnapshot {
                    pane: 1,
                    tab: 4,
                    tmux_session: Some("tako-1".into()),
                    log_file: Some("/tmp/pane-1.log".into()),
                    ..Default::default()
                }],
            ),
            // pending の寿命切れ（GC）。検出集合は変わらない
            ("pending の GC", vec![det(ID_A, 1)], vec![]),
        ];
        for (i, (name, second, panes)) in cases.into_iter().enumerate() {
            let path = temp_path(&format!("2020-struct-{i}"));
            // GC のケースだけ、7 日の寿命が 1 回目と 2 回目の走査の間で切れる pending を置く
            // （他のケースに置くと、どのケースも pending の GC で緑になってしまう）
            if name == "pending の GC" {
                SessionCatalog::mutate_at(&path, |c| {
                    c.upsert_pending(PendingSpawn {
                        tmux_session: Some("tako-old".into()),
                        recorded_at: iso(T0 - PENDING_TTL_SECS + 10),
                        ..Default::default()
                    })
                })
                .unwrap();
            }
            let mut th = LastSeenThrottle::default();
            sync_detected_throttled(&path, &[det(ID_A, 1)], &[], T0, &mut th).unwrap();
            let before = std::fs::read_to_string(&path).unwrap();
            mark(&path);
            let now = T0 + SCAN;
            assert_eq!(
                sync_detected_throttled(&path, &second, &panes, now, &mut th).unwrap(),
                SyncWrite::Structural,
                "{name} の変化を即時に書いていない"
            );
            assert!(rewritten(&path), "{name}: ファイルを書いていない");
            assert_eq!(
                bak(&path, 1),
                Some(format!("{MARK}{before}")),
                "{name}: 書く前の内容を .bak.1 へ退避していない"
            );
            let catalog = SessionCatalog::load_from(&path).unwrap();
            assert_eq!(catalog.entries[ID_A].last_seen_at, iso(now), "{name}");
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    /// 修正前（走査のたびに全体を書く）の内容を手元で作り、間引いた側のファイルと突き合わせる。
    /// **どの走査の後でも**並び順（`tako sessions list`）・GC で残る集合・ペインからの逆引き
    /// （#466）・last_seen_at 以外の内容が一致し、**書いた時点では中身まで一致する**こと
    #[test]
    fn issue2020_検出集合の変化で溜めた時刻ごと書き_並び順とgcと逆引きが修正前と一致する() {
        let path = temp_path("2020-equiv");
        // GC（500 件の上限）が走るように、古い会話で上限の手前まで埋めておく
        let mut seed = SessionCatalog::default();
        for i in 0..(MAX_ENTRIES - 3) {
            seed.entries.insert(
                format!("00000000-0000-4000-8000-{i:012}"),
                SessionEntry {
                    kind: "worker".into(),
                    pane: Some(100 + (i as u64 % 7)),
                    started_at: iso(T0 - 90_000 + i as i64),
                    last_seen_at: iso(T0 - 86_400 + i as i64),
                    ..Default::default()
                },
            );
        }
        crate::config_io::atomic_write(&path, &serde_yaml::to_string(&seed).unwrap()).unwrap();
        let mut reference = seed;
        let mut th = LastSeenThrottle::default();
        // (走査の回数, 検出): 始まる / 終わる / 同じペインで別の会話 / 戻ってくる /
        // 全部終わる / 窓をまたいで続く / 上限を超えて GC
        let phases: Vec<(i64, Vec<DetectedSession>)> = vec![
            (6, vec![det(ID_A, 1), det(ID_B, 2)]),
            (4, vec![det(ID_A, 1), det(ID_B, 2), det(ID_C, 3)]),
            (5, vec![det(ID_B, 2), det(ID_C, 3)]),
            (3, vec![det(ID_B, 2), det(ID_C, 3), det(ID_D, 1)]),
            (3, vec![det(ID_A, 1), det(ID_B, 2), det(ID_C, 3)]),
            (3, vec![]),
            (25, vec![det(ID_E, 4)]),
            (3, vec![det(ID_E, 4), det(ID_F, 5)]),
        ];
        let panes_of = |c: &SessionCatalog| -> Vec<Option<String>> {
            (1..=5)
                .chain(100..107)
                .map(|p| resolve_session_for_pane_in(c, &p.to_string()))
                .collect()
        };
        let order_of = |c: &SessionCatalog| -> Vec<String> {
            c.sorted_entries()
                .into_iter()
                .map(|(id, _)| id.clone())
                .collect()
        };
        let mut now = T0;
        let mut writes = 0;
        for (scans, detected) in phases {
            for _ in 0..scans {
                // 修正前: 検出が空でなければその走査で全体を書いていた
                if !detected.is_empty() {
                    apply_detected(&mut reference, &detected, &[], &iso(now));
                }
                let written = sync_detected_throttled(&path, &detected, &[], now, &mut th).unwrap();
                let file = SessionCatalog::load_from(&path).unwrap();
                let at = format!("{}（走査 {} 回目）", iso(now), (now - T0) / SCAN);
                assert_eq!(order_of(&file), order_of(&reference), "並び順が違う: {at}");
                assert_eq!(
                    panes_of(&file),
                    panes_of(&reference),
                    "ペインの逆引きが違う: {at}"
                );
                assert!(
                    same_except_last_seen(&file, &reference),
                    "last_seen_at 以外の内容（GC の結果を含む）が違う: {at}"
                );
                if written != SyncWrite::Skipped {
                    writes += 1;
                    assert_eq!(file, reference, "書いた時点の中身が修正前と違う: {at}");
                }
                now += SCAN;
            }
        }
        assert_eq!(
            reference.entries.len(),
            MAX_ENTRIES,
            "GC が走る筋書きになっていない"
        );
        assert!(
            writes < 52 / 2,
            "間引けていない（52 回の走査で {writes} 回書いた）"
        );
        // 終了時の書き出しの後は中身まで修正前と一致する
        flush_last_seen_at(&path, now, &mut th).unwrap();
        assert_eq!(SessionCatalog::load_from(&path).unwrap(), reference);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn issue2020_溜めた分は終了時の書き出しで最後に見えた時刻まで残りbakは回さない() {
        let path = temp_path("2020-flush");
        let mut th = LastSeenThrottle::default();
        let ab = [det(ID_A, 1), det(ID_B, 2)];
        sync_detected_throttled(&path, &ab, &[], T0, &mut th).unwrap();
        for i in 1..=3 {
            sync_detected_throttled(&path, &ab, &[], T0 + SCAN * i, &mut th).unwrap();
        }
        assert!(th.has_unflushed());
        mark(&path);
        // 終了は走査の間（最後の走査の 15 秒後）に来る
        assert_eq!(
            flush_last_seen_at(&path, T0 + SCAN * 3 + 15, &mut th).unwrap(),
            SyncWrite::LastSeenOnly
        );
        assert!(rewritten(&path), "終了時に溜めた分を書いていない");
        assert_eq!(
            bak(&path, 1),
            None,
            "last_seen_at だけの保存で .bak を回した"
        );
        let catalog = SessionCatalog::load_from(&path).unwrap();
        for id in [ID_A, ID_B] {
            // 書き出した時刻ではなく、最後に見えた走査の時刻
            assert_eq!(catalog.entries[id].last_seen_at, iso(T0 + SCAN * 3));
        }
        // 2 回目は溜めた分が無い = I/O しない
        mark(&path);
        assert_eq!(
            flush_last_seen_at(&path, T0 + SCAN * 4, &mut th).unwrap(),
            SyncWrite::Skipped
        );
        assert!(!rewritten(&path));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn issue2020_会話がすべて終わった走査で溜めた分を書く() {
        let path = temp_path("2020-empty");
        let mut th = LastSeenThrottle::default();
        let ab = [det(ID_A, 1), det(ID_B, 2)];
        sync_detected_throttled(&path, &ab, &[], T0, &mut th).unwrap();
        sync_detected_throttled(&path, &ab, &[], T0 + SCAN, &mut th).unwrap();
        mark(&path);
        assert_eq!(
            sync_detected_throttled(&path, &[], &[], T0 + SCAN * 2, &mut th).unwrap(),
            SyncWrite::LastSeenOnly
        );
        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert_eq!(catalog.entries[ID_A].last_seen_at, iso(T0 + SCAN));
        // その後の空の走査は（修正前と同じく）I/O しない
        mark(&path);
        assert_eq!(
            sync_detected_throttled(&path, &[], &[], T0 + SCAN * 3, &mut th).unwrap(),
            SyncWrite::Skipped
        );
        assert!(!rewritten(&path));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn issue2020_同じ内容なら書かない() {
        let path = temp_path("2020-same");
        let mut th = LastSeenThrottle::default();
        let ab = [det(ID_A, 1), det(ID_B, 2)];
        sync_detected_throttled(&path, &ab, &[], T0, &mut th).unwrap();
        mark(&path);
        // 同じ秒の 2 回目の走査（イベント駆動の臨時走査）と、その直後の書き出しは
        // どちらもファイルと同じ内容 = 書かない
        assert_eq!(
            sync_detected_throttled(&path, &ab, &[], T0, &mut th).unwrap(),
            SyncWrite::Skipped
        );
        assert!(th.has_unflushed());
        assert_eq!(
            flush_last_seen_at(&path, T0, &mut th).unwrap(),
            SyncWrite::Skipped
        );
        assert!(!rewritten(&path), "同じ内容を書き直した");
        assert!(!th.has_unflushed());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn issue2020_外から書き換えられた内容を潰さない() {
        let path = temp_path("2020-external");
        let mut th = LastSeenThrottle::default();
        let ab = [det(ID_A, 1), det(ID_B, 2)];
        sync_detected_throttled(&path, &ab, &[], T0, &mut th).unwrap();
        // 別プロセスの書き込み（spawn の pending 記録 + 手で直したラベル）
        SessionCatalog::mutate_at(&path, |c| {
            c.upsert_pending(PendingSpawn {
                tmux_session: Some("tako-7".into()),
                recorded_at: iso(T0 + 5),
                ..Default::default()
            });
            c.entries.get_mut(ID_A).unwrap().label = Some("手で直した".into());
        })
        .unwrap();
        mark(&path);
        assert_eq!(
            sync_detected_throttled(&path, &ab, &[], T0 + SCAN, &mut th).unwrap(),
            SyncWrite::Skipped
        );
        assert!(!rewritten(&path));
        // 窓が過ぎて書くときも、読み直した内容（外の書き込み）の上へ時刻だけを当てる
        let due = T0 + LAST_SEEN_FLUSH_SECS;
        assert_eq!(
            sync_detected_throttled(&path, &ab, &[], due, &mut th).unwrap(),
            SyncWrite::LastSeenOnly
        );
        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert_eq!(catalog.entries[ID_A].label.as_deref(), Some("手で直した"));
        assert_eq!(catalog.pending.len(), 1);
        assert_eq!(catalog.entries[ID_A].last_seen_at, iso(due));
        // ファイルが消された = 次の走査で検出中の会話を即時に書き直す
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            sync_detected_throttled(&path, &ab, &[], due + SCAN, &mut th).unwrap(),
            SyncWrite::Structural
        );
        let catalog = SessionCatalog::load_from(&path).unwrap();
        assert!(catalog.entries.contains_key(ID_A) && catalog.entries.contains_key(ID_B));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn issue2020_last_seen_at以外のどの項目の変化も構造の変化と読む() {
        let base = SessionEntry {
            kind: "worker".into(),
            label: Some("l".into()),
            project: Some("p".into()),
            profile: Some("pr".into()),
            agent: Some("claude".into()),
            model: Some("m".into()),
            effort: Some("high".into()),
            issues: vec![1],
            prompt_head: Some("h".into()),
            cwd: Some("/w".into()),
            tab: Some(1),
            pane: Some(2),
            tmux_session: Some("t".into()),
            log_file: Some("/l".into()),
            started_at: iso(T0),
            last_seen_at: iso(T0),
        };
        /// (項目名, その項目だけを変える)
        type FieldEdit = (&'static str, fn(&mut SessionEntry));
        let edits: Vec<FieldEdit> = vec![
            ("kind", |e| e.kind = "master".into()),
            ("label", |e| e.label = None),
            ("project", |e| e.project = None),
            ("profile", |e| e.profile = None),
            ("agent", |e| e.agent = None),
            ("model", |e| e.model = None),
            ("effort", |e| e.effort = None),
            ("issues", |e| e.issues.clear()),
            ("prompt_head", |e| e.prompt_head = None),
            ("cwd", |e| e.cwd = None),
            ("tab", |e| e.tab = None),
            ("pane", |e| e.pane = None),
            ("tmux_session", |e| e.tmux_session = None),
            ("log_file", |e| e.log_file = None),
            ("started_at", |e| e.started_at.clear()),
        ];
        for (name, edit) in edits {
            let mut changed = base.clone();
            edit(&mut changed);
            assert!(
                !entry_eq_except_last_seen(&base, &changed),
                "{name} の変化を last_seen_at だけの変化と読んだ"
            );
        }
        let mut seen = base.clone();
        seen.last_seen_at = iso(T0 + SCAN);
        assert!(entry_eq_except_last_seen(&base, &seen));
    }
}
