//! 復元の直後の全滅ガード（#641）
//!
//! 起動時に器（tmux / psmux のセッション）へ繋いだペインが、繋いだ直後に**入力も無いまま**すべて
//! PTY 死亡で閉じることがある（Windows の実機: 強制終了した旧クライアントの残骸が残った psmux へ
//! attach すると、読み取りが即 EOF になる。器の中の claude は生きている）。修正前は「最後のペインが
//! PTY 死亡で閉じた = 全ペイン終了」として layout.json を残したまま終了していたので、
//! **起動するたびに同じ復元を試して即終了する**ループになり、GUI が立ち上がらなかった。
//!
//! ここはその判断の正本（GPUI 非依存の純関数 + 記録ファイルの読み書き）を持つ:
//!
//! - [`Window`]: 起動時に器へ繋いでから [`WINDOW`] のあいだ、入力が無いまま最後のタブが PTY 死亡で
//!   閉じたら [`Decision::Rescue`]（終了せず、器を使わない新しいシェルのタブで起動を続ける）。
//!   それ以外（窓を過ぎた・入力があった・最後のタブに器のペインが無い・この起動で一度救済した）は
//!   [`Decision::Quit`] = 従来どおりの終了
//! - 記録 `<data_dir>/restore-guard.json`（[`GuardRecord`]）: 救済するたびに同じ理由の連続回数を
//!   1 つ進め、復元が窓を生き延びた・救済せずに終わった起動で消す。連続が [`SKIP_AFTER`] 回に
//!   達していたら、次の起動は layout の復元を見送る（[`skip_restore_if_due`]）
//! - 救済・見送りのどちらも、復元に使った layout を `layout.json.rescued` へ退避する
//!   （`tako recover --apply rescued` で戻せる。前の退避は `.bak.1`〜`.bak.3` へ回る）。
//!   **器は kill しない**（#30 と同じ。中のプロセスは生きていることがある = 次の起動の
//!   orphan 自動復帰がまた拾う）
//!
//! 「入力」は人のキー・貼り付け・明示の close と、CLI / MCP のペインへの送信・クリック
//! （[`counts_as_input`]）。`exit` で最後のシェルを閉じる普通の終了はここで従来どおりになる。
//! 記録は版数の番地を持たない Local のランタイム状態（読めなければ無いものとして扱う =
//! 見送らない側へ倒す）。欄の増減は `migration_registry` の指紋が止める。
//! A/B は `TAKO_641_LEGACY=1`（ガード・見送り・記録をすべて止める = 修正前の挙動）

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::layout::LayoutFile;

/// 置き場（`<data_dir>` の下）
pub const FILE: &str = "restore-guard.json";

/// 「復元の直後」とみなす長さ。器へ繋いでからこれを過ぎた全滅は普通の終了として扱う。
/// 実機の再現（#641）は繋いだその秒に全滅していた。macOS の再 attach（#1857）は器への
/// 問い合わせが上限 3 秒・待ちが最長 4 秒なので、器が無くなっていた全滅もこの内側に収まる
pub const WINDOW: Duration = Duration::from_secs(30);

/// 同じ理由の救済がこの回数続いたら、次の起動は復元を見送る
pub const SKIP_AFTER: u32 = 2;

/// 救済・見送りで復元に使った layout を退避する先（`layout.json` の隣）
pub const RESCUED_EXT: &str = "json.rescued";

/// #641 の A/B。`TAKO_641_LEGACY=1` で修正前の挙動（全ペイン PTY 死亡で必ず終了・見送りなし）
pub fn legacy() -> bool {
    std::env::var_os("TAKO_641_LEGACY").is_some()
}

/// 救済した理由（記録の `reason`）。同じ理由が続いたかの比較に使う
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// 器へ繋いだ直後に、入力が無いまま全ペインが PTY 死亡で閉じた
    AllExitedAfterRestore,
}

impl Reason {
    /// 記録に書く語
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::AllExitedAfterRestore => "all_exited_after_restore",
        }
    }

    /// 記録の語から戻す（知らない語は None = 別の理由として数え直す）
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all_exited_after_restore" => Some(Reason::AllExitedAfterRestore),
            _ => None,
        }
    }

    /// persist.log に書く説明（日本語固定）
    pub fn label(self) -> &'static str {
        match self {
            Reason::AllExitedAfterRestore => {
                "器へ繋いだ直後に入力の無いまま器のペインがすべて PTY 死亡で閉じた"
            }
        }
    }
}

/// `restore-guard.json` の中身（救済の連続回数）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardRecord {
    /// 理由（[`Reason::as_str`]）
    pub reason: String,
    /// 同じ理由で続いた救済の回数
    pub count: u32,
    /// 最後に救済した時刻（Unix ミリ秒）
    #[serde(default)]
    pub at_ms: u64,
}

/// 救済した後の記録（純関数）。前の記録と同じ理由なら回数を 1 進め、違えば 1 から数え直す
pub fn next_record(prev: Option<&GuardRecord>, reason: Reason, at_ms: u64) -> GuardRecord {
    let count = match prev {
        Some(p) if Reason::parse(&p.reason) == Some(reason) => p.count.saturating_add(1),
        _ => 1,
    };
    GuardRecord {
        reason: reason.as_str().to_string(),
        count,
        at_ms,
    }
}

/// 起動時に layout を復元するか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPlan {
    /// 従来どおり復元する
    Restore,
    /// 見送る（同じ理由の救済が `count` 回続いた）
    Skip { reason: Reason, count: u32 },
}

/// 記録から起動時の計画を決める（純関数）
pub fn startup_plan(record: Option<&GuardRecord>, legacy: bool) -> StartupPlan {
    if legacy {
        return StartupPlan::Restore;
    }
    match record {
        Some(r) if r.count >= SKIP_AFTER => match Reason::parse(&r.reason) {
            Some(reason) => StartupPlan::Skip {
                reason,
                count: r.count,
            },
            None => StartupPlan::Restore,
        },
        _ => StartupPlan::Restore,
    }
}

/// 最後のタブが PTY 死亡で閉じたときの判断
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 終了しない。器を使わない新しいシェルのタブで起動を続ける
    Rescue { elapsed: Duration },
    /// 従来どおり終了する
    Quit(Keep),
}

/// 従来どおり終了する理由
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// 起動時に器へ繋いだペインが無い（復元も orphan 復帰もしていない・セルフテスト等）
    NotArmed,
    /// `TAKO_641_LEGACY=1`
    Legacy,
    /// この起動で一度救済した（救済のシェルまで閉じたなら終わらせる = ループさせない）
    AlreadyRescued,
    /// 起動の後に入力があった（`exit` で閉じた等の普通の終了）
    InputSeen,
    /// 器へ繋いでから窓を過ぎていた
    Elapsed(Duration),
    /// 最後に閉じたタブに器のペインが無い
    NoContainer,
    /// 救済のシェルを起こせなかった
    RescueFailed,
}

impl Keep {
    /// 「全ペイン終了」の行へ添える注記（persist.log・日本語固定）。ガードに関係の無い
    /// 終了（[`Keep::NotArmed`]）は何も添えない = 修正前と同じ行
    pub fn note(self) -> Option<String> {
        match self {
            Keep::NotArmed => None,
            Keep::Legacy => Some("（#641 のガードは TAKO_641_LEGACY で止めている）".into()),
            Keep::AlreadyRescued => Some(
                "（#641: この起動で一度救済したシェルのタブまで閉じたので、今回は終了する）".into(),
            ),
            Keep::InputSeen => {
                Some("（#641: 起動の後に入力があったので普通の終了として扱う）".into())
            }
            Keep::Elapsed(d) => Some(format!(
                "（#641: 器へ繋いでから {:.1} 秒 = 復元の直後（{} 秒）を過ぎていたので普通の終了として扱う）",
                d.as_secs_f64(),
                WINDOW.as_secs()
            )),
            Keep::NoContainer => {
                Some("（#641: 最後のタブに器のペインが無いので普通の終了として扱う）".into())
            }
            Keep::RescueFailed => {
                Some("（#641: 救済のシェルを起こせなかったので終了する）".into())
            }
        }
    }
}

/// 起動時に器へ繋いでからの窓（GUI が 1 つ持つ実行時の状態）
#[derive(Debug, Clone, Default)]
pub struct Window {
    armed_at: Option<Instant>,
    container_panes: usize,
    input_seen: bool,
    rescued: bool,
    settled: bool,
    /// 窓の内側で PTY 死亡により外れた器の名前（救済の行に載せる = どの器を捨てたか）
    exited: Vec<String>,
    /// 最後の救済の行（`tako persist` の `restore_guard.last`）
    last_line: Option<String>,
}

impl Window {
    /// 器へ繋いだペインが `container_panes` 枚ある状態で窓を開く（0 枚なら開かない）
    pub fn arm(&mut self, now: Instant, container_panes: usize) {
        if container_panes == 0 {
            return;
        }
        self.armed_at = Some(now);
        self.container_panes = container_panes;
    }

    /// 窓が開いているか
    pub fn armed(&self) -> bool {
        self.armed_at.is_some()
    }

    /// 人 / AI の入力があった（以後この起動の全滅は普通の終了として扱う）
    pub fn note_input(&mut self) {
        if self.armed_at.is_some() {
            self.input_seen = true;
        }
    }

    /// ペインが PTY 死亡で器から外れた（窓の内側だけ名前を控える）
    pub fn note_exit(&mut self, now: Instant, session: &str) {
        if self.within(now) && !self.exited.iter().any(|s| s == session) {
            self.exited.push(session.to_string());
        }
    }

    fn within(&self, now: Instant) -> bool {
        self.armed_at
            .is_some_and(|at| now.saturating_duration_since(at) <= WINDOW)
    }

    /// 最後のタブが PTY 死亡で閉じたときの判断（純関数）。
    /// `dying_containers` は閉じるタブのうち器のペインの枚数
    pub fn decide(&self, now: Instant, dying_containers: usize, legacy: bool) -> Decision {
        if legacy {
            return Decision::Quit(Keep::Legacy);
        }
        let Some(at) = self.armed_at else {
            return Decision::Quit(Keep::NotArmed);
        };
        if self.rescued {
            return Decision::Quit(Keep::AlreadyRescued);
        }
        if self.input_seen {
            return Decision::Quit(Keep::InputSeen);
        }
        let elapsed = now.saturating_duration_since(at);
        if elapsed > WINDOW {
            return Decision::Quit(Keep::Elapsed(elapsed));
        }
        if dying_containers == 0 {
            return Decision::Quit(Keep::NoContainer);
        }
        Decision::Rescue { elapsed }
    }

    /// 救済した（同じ起動でもう一度は救済しない）。救済の行を控える
    pub fn mark_rescued(&mut self, line: String) {
        self.rescued = true;
        self.last_line = Some(line);
    }

    /// この起動で救済したか
    pub fn rescued(&self) -> bool {
        self.rescued
    }

    /// 窓の内側で外れた器の名前（救済の行に載せる）
    pub fn exited(&self) -> &[String] {
        &self.exited
    }

    /// 記録を消してよい時が来たか（窓を開かなかった起動は直ちに、開いた起動は窓を
    /// 救済なしで過ぎたら。どちらも 1 回だけ）
    pub fn settle_due(&self, now: Instant) -> bool {
        if self.settled || self.rescued {
            return false;
        }
        match self.armed_at {
            None => true,
            Some(at) => now.saturating_duration_since(at) > WINDOW,
        }
    }

    /// 記録を消した（以後 [`Window::settle_due`] は偽）
    pub fn mark_settled(&mut self) {
        self.settled = true;
    }

    /// 救済せずに終わる起動の後始末で記録を消すか（窓の途中で普通に終わった起動も、
    /// 連続を途切れさせる）
    pub fn clear_on_quit(&self) -> bool {
        !self.settled && !self.rescued
    }

    /// `tako persist` の `restore_guard`（窓の状態・記録・退避先）
    pub fn to_json(&self, now: Instant) -> serde_json::Value {
        let record = load();
        let rescued_layout = rescued_path().filter(|p| p.is_file());
        serde_json::json!({
            "armed": self.armed_at.is_some(),
            "window_secs": WINDOW.as_secs(),
            "elapsed_secs": self
                .armed_at
                .map(|at| now.saturating_duration_since(at).as_secs_f64()),
            "container_panes": self.container_panes,
            "input_seen": self.input_seen,
            "rescued": self.rescued,
            "exited": self.exited,
            "last": self.last_line,
            "record": record,
            "skip_after": SKIP_AFTER,
            "rescued_layout": rescued_layout.map(|p| p.display().to_string()),
        })
    }
}

/// CLI / MCP の要求のうち「人 / AI の入力」に数えるもの（ペインへの送信・クリック・
/// ダイアログへの応答と、ペイン・窓・器を明示で閉じる操作）。読み取り・問い合わせ
/// （master のポーリング）は数えない。閉じる操作は GUI の × と違って dispatch が器を片付ける
/// （`detach_session`）ので、GUI 側の明示 close の記録点を通らない = ここで数える
pub fn counts_as_input(request: &crate::protocol::Request) -> bool {
    use crate::protocol::Request;
    matches!(
        request,
        Request::Send { .. }
            | Request::PaneClick { .. }
            | Request::OrchestratorRespond { .. }
            | Request::Close { .. }
            | Request::WindowClose { .. }
            | Request::TmuxKill { .. }
    )
}

/// 記録の置き場（`TAKO_DATA_DIR` で隔離できる）。`join` の引数をリテラルで書くのは、
/// 共有分類カタログの被覆テスト（`config_share_catalog`）の走査に拾わせるため
pub fn path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("restore-guard.json"))
}

/// 退避先（`<data_dir>/layout.json.rescued`）
pub fn rescued_path() -> Option<PathBuf> {
    crate::layout::layout_path().map(|p| p.with_extension(RESCUED_EXT))
}

/// 記録を読む（無い・読めないなら None = 見送らない側へ倒す）
pub fn load() -> Option<GuardRecord> {
    path().and_then(|p| load_from(&p))
}

/// [`load`] の置き場を指定する版
pub fn load_from(path: &Path) -> Option<GuardRecord> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// 救済を 1 回記録する（排他の内側で 読む → 進める → 書く）。進めた後の記録を返す
pub fn record_rescue_at(path: &Path, reason: Reason, at_ms: u64) -> Result<GuardRecord, String> {
    let _lock = crate::config_io::lock_exclusive(path)?;
    let next = next_record(load_from(path).as_ref(), reason, at_ms);
    let text =
        serde_json::to_string_pretty(&next).map_err(|e| format!("記録を直列化できない: {e}"))?;
    crate::config_io::atomic_write(path, &text)?;
    Ok(next)
}

/// 記録を消す（無ければ何もしない）
pub fn clear_at(path: &Path) -> Result<(), String> {
    let _lock = crate::config_io::lock_exclusive(path)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{} を消せない: {e}", path.display())),
    }
}

/// 記録を消す（既定の置き場。平常時はファイルが無い）
pub fn clear() {
    if let Some(p) = path() {
        if let Err(e) = clear_at(&p) {
            eprintln!("warning: 復元の直後のガードの記録を消せない: {e}");
        }
    }
}

/// 救済で、復元に使った layout（器が無くて刈り込んだ後の、実際に復元した中身）を退避する。
/// 前の退避と中身が違えば前のものは `.bak.1`〜`.bak.3` へ回る
pub fn stash_layout_at(rescued: &Path, file: &LayoutFile) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(file).map_err(|e| format!("layout を直列化できない: {e}"))?;
    crate::config_io::atomic_write_with_backup(rescued, &text)
}

/// 見送りで layout.json を退避先へ移す（写せたら元を外す = 次の保存は新しい構成で始まる）。
/// layout.json が無ければ Ok(false)。写せなければ元はそのまま残す（消える経路を作らない）
pub fn move_layout_at(layout: &Path, rescued: &Path) -> Result<bool, String> {
    if !layout.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(layout)
        .map_err(|e| format!("{} を読めない: {e}", layout.display()))?;
    crate::config_io::atomic_write_with_backup(rescued, &text)?;
    std::fs::remove_file(layout).map_err(|e| format!("{} を外せない: {e}", layout.display()))?;
    Ok(true)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 起動時: 記録の連続が [`SKIP_AFTER`] に達していれば復元を見送る。見送ったら
/// persist.log へ書く 1 行（`last_restore` にも載る）を返す。記録は消す（次の起動は
/// 従来どおり復元を試す = 1 回見送るだけ）
pub fn skip_restore_if_due() -> Option<String> {
    let (Some(record_path), Some(layout), Some(rescued)) =
        (path(), crate::layout::layout_path(), rescued_path())
    else {
        return None;
    };
    skip_restore_at(&record_path, &layout, &rescued, legacy())
}

/// [`skip_restore_if_due`] の置き場を指定する版
pub fn skip_restore_at(
    record_path: &Path,
    layout: &Path,
    rescued: &Path,
    legacy: bool,
) -> Option<String> {
    let StartupPlan::Skip { reason, count } = startup_plan(load_from(record_path).as_ref(), legacy)
    else {
        return None;
    };
    let stash = match move_layout_at(layout, rescued) {
        Ok(true) => format!(
            "layout.json は {} へ退避した（`tako recover --apply rescued` で戻せる）",
            file_name(rescued)
        ),
        Ok(false) => "退避する layout.json は無い".to_string(),
        // 退避できないなら元のまま。復元は見送る（同じ全滅を繰り返さない）
        Err(e) => format!("layout.json を退避できないので元の場所に残した: {e}"),
    };
    if let Err(e) = clear_at(record_path) {
        eprintln!("warning: 復元の直後のガードの記録を消せない: {e}");
    }
    Some(format!(
        "復元見送り（#641）: 直前の {count} 回の起動が続けて「{}」で終わった。今回は layout を復元しない・\
         {stash}・器は kill していない（orphan 自動復帰が拾う）",
        reason.label()
    ))
}

/// 救済の 1 行（persist.log・日本語固定）。器の名前（`tako-<rand>`）は利用者の情報を含まない
pub fn rescue_line(
    elapsed: Duration,
    exited: &[String],
    stashed: Result<Option<&Path>, &str>,
    record: Result<&GuardRecord, &str>,
) -> String {
    let containers = if exited.is_empty() {
        "器の名前は控えられなかった".to_string()
    } else {
        format!("捨てた器 {}: {}", exited.len(), exited.join(", "))
    };
    let stash = match stashed {
        Ok(Some(p)) => format!(
            "復元に使った layout は {} へ退避（`tako recover --apply rescued` で戻せる）",
            file_name(p)
        ),
        Ok(None) => "退避する layout は無い（orphan 自動復帰だけの起動）".to_string(),
        Err(e) => format!("復元に使った layout を退避できない: {e}"),
    };
    let streak = match record {
        Ok(r) if r.count >= SKIP_AFTER => {
            format!("同じ理由で連続 {} 回目 = 次の起動は復元を見送る", r.count)
        }
        Ok(r) => format!(
            "同じ理由で連続 {} 回目（{SKIP_AFTER} 回続いたら次の起動は復元を見送る）",
            r.count
        ),
        Err(e) => format!("連続回数を記録できない: {e}"),
    };
    format!(
        "復元の直後の全滅（#641）: {}（器へ繋いでから {:.1} 秒）。終了せず、器を使わない新しいシェルの\
         タブで起動を続ける（{containers}・器は kill しない）。{stash}。{streak}",
        Reason::AllExitedAfterRestore.label(),
        elapsed.as_secs_f64()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> tako_core::test_residue::ScratchDir {
        tako_core::test_residue::ScratchDir::new(&format!("restore-guard-641-{tag}"))
    }

    fn armed(now: Instant) -> Window {
        let mut w = Window::default();
        w.arm(now, 6);
        w
    }

    #[test]
    fn 器へ繋いだ直後に入力なしで全滅したら救済する() {
        let t0 = Instant::now();
        let w = armed(t0);
        let d = w.decide(t0 + Duration::from_millis(800), 2, false);
        assert_eq!(
            d,
            Decision::Rescue {
                elapsed: Duration::from_millis(800)
            }
        );
    }

    #[test]
    fn 窓の境界は含み_過ぎたら普通の終了() {
        let t0 = Instant::now();
        let w = armed(t0);
        assert!(matches!(
            w.decide(t0 + WINDOW, 1, false),
            Decision::Rescue { .. }
        ));
        let late = WINDOW + Duration::from_millis(1);
        assert_eq!(
            w.decide(t0 + late, 1, false),
            Decision::Quit(Keep::Elapsed(late))
        );
    }

    #[test]
    fn 入力があれば窓の内側でも普通の終了() {
        let t0 = Instant::now();
        let mut w = armed(t0);
        w.note_input();
        assert_eq!(
            w.decide(t0 + Duration::from_secs(1), 1, false),
            Decision::Quit(Keep::InputSeen)
        );
    }

    #[test]
    fn 窓を開いていない起動と_legacy_は従来どおり終了する() {
        let t0 = Instant::now();
        let mut w = Window::default();
        // 器へ繋いだペインが 0 枚なら開かない（入力の記録もしない）
        w.arm(t0, 0);
        w.note_input();
        assert!(!w.armed());
        assert_eq!(w.decide(t0, 1, false), Decision::Quit(Keep::NotArmed));
        assert_eq!(
            Keep::NotArmed.note(),
            None,
            "ガードに関係の無い終了の行は変えない"
        );
        assert_eq!(armed(t0).decide(t0, 1, true), Decision::Quit(Keep::Legacy));
    }

    #[test]
    fn 器の無いタブの終了と_2回目の全滅は救済しない() {
        let t0 = Instant::now();
        let mut w = armed(t0);
        assert_eq!(w.decide(t0, 0, false), Decision::Quit(Keep::NoContainer));
        w.mark_rescued("救済".into());
        // 救済のシェルまで閉じたら終わらせる（同じ起動の中でループさせない）
        assert_eq!(
            w.decide(t0 + Duration::from_secs(1), 1, false),
            Decision::Quit(Keep::AlreadyRescued)
        );
    }

    #[test]
    fn 捨てた器は窓の内側だけ重複なく控える() {
        let t0 = Instant::now();
        let mut w = armed(t0);
        w.note_exit(t0 + Duration::from_secs(1), "tako-a");
        w.note_exit(t0 + Duration::from_secs(1), "tako-a");
        w.note_exit(t0 + Duration::from_secs(2), "tako-b");
        w.note_exit(t0 + WINDOW + Duration::from_secs(1), "tako-c");
        assert_eq!(w.exited(), ["tako-a", "tako-b"]);
    }

    #[test]
    fn 記録を消す時は_窓なしは直ちに_窓ありは救済なしで過ぎてから_1回だけ() {
        let t0 = Instant::now();
        let mut none = Window::default();
        assert!(none.settle_due(t0));
        none.mark_settled();
        assert!(!none.settle_due(t0));
        assert!(!none.clear_on_quit());

        let w = armed(t0);
        assert!(!w.settle_due(t0 + WINDOW));
        assert!(
            w.clear_on_quit(),
            "窓の途中で普通に終わった起動も連続を途切れさせる"
        );
        assert!(w.settle_due(t0 + WINDOW + Duration::from_millis(1)));

        let mut r = armed(t0);
        r.mark_rescued("救済".into());
        assert!(!r.settle_due(t0 + WINDOW * 2), "救済した起動は記録を残す");
        assert!(!r.clear_on_quit());
    }

    #[test]
    fn 同じ理由なら回数を進め_違う理由や読めない記録は1から() {
        let first = next_record(None, Reason::AllExitedAfterRestore, 10);
        assert_eq!(first.count, 1);
        let second = next_record(Some(&first), Reason::AllExitedAfterRestore, 20);
        assert_eq!((second.count, second.at_ms), (2, 20));
        let other = GuardRecord {
            reason: "unknown".into(),
            count: 5,
            at_ms: 0,
        };
        assert_eq!(
            next_record(Some(&other), Reason::AllExitedAfterRestore, 30).count,
            1
        );
    }

    #[test]
    fn 連続が閾値に達したら見送る() {
        let rec = |count| GuardRecord {
            reason: Reason::AllExitedAfterRestore.as_str().into(),
            count,
            at_ms: 0,
        };
        assert_eq!(startup_plan(None, false), StartupPlan::Restore);
        assert_eq!(
            startup_plan(Some(&rec(SKIP_AFTER - 1)), false),
            StartupPlan::Restore
        );
        assert_eq!(
            startup_plan(Some(&rec(SKIP_AFTER)), false),
            StartupPlan::Skip {
                reason: Reason::AllExitedAfterRestore,
                count: SKIP_AFTER
            }
        );
        assert_eq!(
            startup_plan(Some(&rec(SKIP_AFTER)), true),
            StartupPlan::Restore
        );
        let unknown = GuardRecord {
            reason: "unknown".into(),
            count: 9,
            at_ms: 0,
        };
        assert_eq!(startup_plan(Some(&unknown), false), StartupPlan::Restore);
    }

    #[test]
    fn 記録は書いて進めて消せる_読めない記録は無いものとして扱う() {
        let dir = scratch("record");
        let p = dir.path().join(FILE);
        assert_eq!(load_from(&p), None);
        let a = record_rescue_at(&p, Reason::AllExitedAfterRestore, 1).unwrap();
        let b = record_rescue_at(&p, Reason::AllExitedAfterRestore, 2).unwrap();
        assert_eq!((a.count, b.count), (1, 2));
        assert_eq!(load_from(&p), Some(b));
        clear_at(&p).unwrap();
        assert!(!p.exists());
        clear_at(&p).unwrap();
        std::fs::write(&p, "{ 壊れた").unwrap();
        assert_eq!(load_from(&p), None);
        assert_eq!(
            record_rescue_at(&p, Reason::AllExitedAfterRestore, 3)
                .unwrap()
                .count,
            1
        );
    }

    #[test]
    fn 見送りは_layout_を退避して記録を消し_次の起動は復元に戻る() {
        let dir = scratch("skip");
        let record = dir.path().join(FILE);
        let layout = dir.path().join("layout.json");
        let rescued = layout.with_extension(RESCUED_EXT);
        std::fs::write(&layout, "{\"version\":1}").unwrap();
        // 1 回目の救済の後では見送らない
        record_rescue_at(&record, Reason::AllExitedAfterRestore, 1).unwrap();
        assert_eq!(skip_restore_at(&record, &layout, &rescued, false), None);
        assert!(layout.is_file());
        // 2 回続いたら見送る（layout.json は消さずに退避 = 戻せる）
        record_rescue_at(&record, Reason::AllExitedAfterRestore, 2).unwrap();
        assert_eq!(
            skip_restore_at(&record, &layout, &rescued, true),
            None,
            "legacy は見送らない"
        );
        let line = skip_restore_at(&record, &layout, &rescued, false).expect("見送る");
        assert!(line.contains("復元見送り（#641）"), "{line}");
        assert!(line.contains("layout.json.rescued"), "{line}");
        assert!(!layout.exists());
        assert_eq!(
            std::fs::read_to_string(&rescued).unwrap(),
            "{\"version\":1}"
        );
        assert!(!record.exists(), "見送りは 1 回だけ（記録を消す）");
        assert_eq!(skip_restore_at(&record, &layout, &rescued, false), None);
    }

    #[test]
    fn 退避は前の退避を世代へ回して上書きで失わない() {
        let dir = scratch("stash");
        let rescued = dir.path().join("layout.json.rescued");
        std::fs::write(&rescued, "前の退避").unwrap();
        let layout = dir.path().join("layout.json");
        std::fs::write(&layout, "今回").unwrap();
        assert!(move_layout_at(&layout, &rescued).unwrap());
        assert_eq!(std::fs::read_to_string(&rescued).unwrap(), "今回");
        assert_eq!(
            std::fs::read_to_string(crate::config_io::backup_path(&rescued, 1)).unwrap(),
            "前の退避"
        );
        assert!(
            !move_layout_at(&layout, &rescued).unwrap(),
            "無ければ何もしない"
        );
    }

    #[test]
    fn 救済の行は捨てた器と退避先と連続回数を載せる() {
        let rec = GuardRecord {
            reason: Reason::AllExitedAfterRestore.as_str().into(),
            count: SKIP_AFTER,
            at_ms: 0,
        };
        let stash = PathBuf::from("data").join("layout.json.rescued");
        let line = rescue_line(
            Duration::from_millis(1500),
            &["tako-a".into(), "tako-b".into()],
            Ok(Some(&stash)),
            Ok(&rec),
        );
        assert!(line.contains("1.5 秒"), "{line}");
        assert!(line.contains("捨てた器 2: tako-a, tako-b"), "{line}");
        assert!(line.contains("layout.json.rescued へ退避"), "{line}");
        assert!(line.contains("次の起動は復元を見送る"), "{line}");
        assert!(!line.contains("data/"), "退避先はファイル名だけ: {line}");
        let orphan_only = rescue_line(Duration::ZERO, &[], Ok(None), Err("書けない"));
        assert!(orphan_only.contains("orphan 自動復帰だけ"), "{orphan_only}");
        assert!(
            orphan_only.contains("連続回数を記録できない"),
            "{orphan_only}"
        );
    }

    #[test]
    fn 入力に数えるのは送信とクリックと応答だけ() {
        use crate::protocol::Request;
        let send = Request::Send {
            pane: None,
            text: "exit".into(),
            newline: true,
            tmux_session: None,
            await_prompt: false,
            paste_guard: None,
        };
        assert!(counts_as_input(&send));
        // CLI / MCP の明示 close も数える（GUI の × と違って dispatch が器を片付ける経路）
        assert!(counts_as_input(&Request::Close {
            pane: Some(1),
            force: false,
            caller_role: None,
            caller_pane: None,
        }));
        assert!(counts_as_input(&Request::WindowClose { window: 1 }));
        // 読み取り・問い合わせ（master のポーリング）は数えない
        assert!(!counts_as_input(&Request::Persist { enabled: None }));
        assert!(!counts_as_input(&Request::Read {
            pane: None,
            lines: None,
            tmux_session: None,
        }));
    }
}
