//! cargo の全体並列の上限: tako のペインへ注入する共有 jobserver の札の番人
//! （FR-2.43.13 / Issue #1998。エピック #1992 の S6）
//!
//! 設計の正は `.agent/plans/2026-10-resource-manager.md`（§9.4・§9.7）。解釈は `tako_core::jobserver` の
//! 純関数、OS への口は境界 B31（`tako_core::platform::jobserver`）。
//!
//! ## 形
//!
//! - **番人はプロセスに 1 つ**（[`start`]）。本番・隔離の GUI はそれぞれ自分の data dir の fifo を持つ
//!   （[`Mode::Keeper`]）。同じ data dir の先行の tako が居るセカンダリは fifo を持たず、同じパスを
//!   ペインへ渡すだけ（[`Mode::Follower`]）
//! - 背景スレッド（`tako-jobserver`）が [`TICK`] ごとに 1 回: 設定の変化を拾い、fifo が置き場から
//!   消えていたら作り直し、札の過不足を直す（[`Keeper::step`] → [`Keeper::settle`]）。
//!   **札を足すのは札を持ちうるプロセスが 0 本と新しい走査で確かめたときだけ**（GUI の再起動で
//!   二重にしない・持ち逃げを戻す）。多すぎる分はいつでも抜く
//! - 走査（約 20 ms）は**番人のロックの外**で行う（[`crate::resources::jobserver_holders`]）。
//!   UI スレッドがペインを起こすときに読む [`pane_env`] は覚えを写すだけ
//! - **札待ちで止まる経路は作らない**: 作れない / 開けない / 置き場が無い / 切っているときは
//!   注入しない（cargo は従来どおり自前の並列で走る）。注入していれば cargo は暗黙の 1 枚で必ず進む
//! - 終了（[`shutdown`]）: 札を持ちうるプロセスが 0 本なら fifo を消す（GUI が居ない間の cargo は
//!   パスが無いので従来どおり走る）。居れば残す（再起動した GUI が同じ fifo を開き直して続ける）
//!
//! 番犬: `crates/tako-control/tests/issue1998_jobserver_watchdog.rs`

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use tako_core::jobserver::{self as core_js, Plan};
use tako_core::platform::jobserver::{Fifo, SUPPORTED};

/// 番人の 1 周の間隔
pub const TICK: Duration = Duration::from_secs(2);
/// 設定ファイルの mtime を見直す間隔
const SETTINGS_RECHECK: Duration = Duration::from_secs(2);

/// 番人の役
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// fifo を作って開いたまま持ち、札を数え直す
    Keeper,
    /// 同じ data dir の先行の tako が持つ fifo のパスをペインへ渡すだけ（セカンダリ）
    Follower,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Keeper => "keeper",
            Mode::Follower => "follower",
        }
    }
}

/// 札を配らない理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Off {
    /// `settings.json` の `resources.jobserver` が false
    Setting,
    /// GUI のプロセスの env の `TAKO_1992_NO_JOBSERVER=1`（A/B）
    Legacy,
    /// この OS では持てない（Windows = #2000）
    Unsupported,
    /// 置き場が無い（data dir が分からない / どの候補もパスに空白が入る）
    NoPlace,
    /// fifo を作れない・開けない・数えられない（理由の文）
    Error(String),
}

impl Off {
    pub fn code(&self) -> &'static str {
        match self {
            Off::Setting => "setting",
            Off::Legacy => "legacy",
            Off::Unsupported => "unsupported",
            Off::NoPlace => "no_place",
            Off::Error(_) => "error",
        }
    }

    pub fn text(&self) -> String {
        match self {
            Off::Setting => {
                "設定 resources.jobserver が off（`tako queue limits --jobserver on` で戻す）"
                    .into()
            }
            Off::Legacy => format!("{}=1（A/B）", core_js::AB_OFF_ENV),
            Off::Unsupported => "この OS ではまだ持てない（Windows は #2000）".into(),
            Off::NoPlace => {
                "fifo の置き場が無い（data dir が分からないか、候補のパスがどれも空白を含む）"
                    .into()
            }
            Off::Error(e) => format!("fifo を使えない: {e}"),
        }
    }
}

/// 数え直しの記録（status の `last_recount`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recount {
    pub at_ms: u64,
    pub free_before: usize,
    pub added: usize,
    pub removed: usize,
    /// なぜ（`idle_short` = 誰も持っていないのに足りない / `over` = 多すぎる）
    pub why: &'static str,
}

/// [`Keeper::step`] の答え
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 何もしなくてよい（N 枚ちょうど / 抜いた / 配っていない）
    Done,
    /// 足りない。札を持ちうるプロセスの本数を確かめてから [`Keeper::settle`] へ（`free` は今の空き）
    NeedHolders { free: usize },
}

/// 札の番人の本体（本番はプロセスに 1 つ = [`start`]。テストは自分で作って回す）
#[derive(Debug)]
pub struct Keeper {
    mode: Mode,
    path: Option<PathBuf>,
    tokens: u32,
    legacy: bool,
    setting_on: bool,
    fifo: Option<Fifo>,
    error: Option<String>,
    holders: Option<(usize, Instant)>,
    last_recount: Option<Recount>,
    recounts: u64,
    recreated: u64,
    stopped: bool,
}

fn unix_ms() -> u64 {
    crate::queue::unix_ms()
}

impl Keeper {
    pub fn new(
        mode: Mode,
        path: Option<PathBuf>,
        tokens: u32,
        legacy: bool,
        setting_on: bool,
    ) -> Self {
        Self {
            mode,
            path,
            tokens: tokens.max(1),
            legacy,
            setting_on,
            fifo: None,
            error: None,
            holders: None,
            last_recount: None,
            recounts: 0,
            recreated: 0,
            stopped: false,
        }
    }

    /// 配らない理由（配っていれば `None`）
    pub fn off(&self) -> Option<Off> {
        if !SUPPORTED {
            return Some(Off::Unsupported);
        }
        if self.legacy {
            return Some(Off::Legacy);
        }
        if !self.setting_on {
            return Some(Off::Setting);
        }
        if self.path.is_none() {
            return Some(Off::NoPlace);
        }
        if let Some(e) = &self.error {
            return Some(Off::Error(e.clone()));
        }
        if self.mode == Mode::Keeper && self.fifo.is_none() {
            return Some(Off::Error("fifo を開いていない".into()));
        }
        None
    }

    fn wants_fifo(&self) -> bool {
        SUPPORTED
            && !self.stopped
            && !self.legacy
            && self.setting_on
            && self.path.is_some()
            && self.mode == Mode::Keeper
    }

    /// ペインへ足す env（配っていなければ `None`）
    pub fn pane_env(&self) -> Option<(&'static str, String)> {
        if self.stopped || self.off().is_some() {
            return None;
        }
        let path = self.path.as_deref()?;
        Some((core_js::ENV, core_js::makeflags(self.tokens, path)))
    }

    /// 設定の on / off を反映する（off への切り替えは fifo を閉じて置き場から消す = 前に作った
    /// ペインの cargo も次からは従来どおり。on へ戻すと作り直し、札は誰も持っていなくなってから足す）
    pub fn set_setting(&mut self, on: bool) {
        if self.setting_on == on {
            return;
        }
        self.setting_on = on;
        self.error = None;
        self.reconcile();
    }

    /// 状態を設定へ合わせる（開く / 閉じる）。開いても札は足さない（[`Keeper::step`] が足す）
    pub fn reconcile(&mut self) {
        if !self.wants_fifo() {
            if let (Some(fifo), Some(path)) = (self.fifo.take(), self.path.as_deref()) {
                if fifo.unlink_if_at(path) {
                    crate::diag::persist_log(&format!(
                        "jobserver: fifo を消した（{}）",
                        self.off().map(|o| o.text()).unwrap_or_default()
                    ));
                }
            }
            return;
        }
        if self.fifo.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        match Fifo::open_or_create(&path) {
            Ok((fifo, created)) => {
                crate::diag::persist_log(&format!(
                    "jobserver: fifo を{}（札 {} 枚・{}）",
                    if created {
                        "作った"
                    } else {
                        "開き直した"
                    },
                    self.tokens,
                    path.display()
                ));
                self.fifo = Some(fifo);
                self.error = None;
            }
            Err(e) => {
                crate::diag::persist_log(&format!("jobserver: fifo を使えない（注入しない）: {e}"));
                self.error = Some(e.to_string());
            }
        }
    }

    /// 1 周の前半（走査を要らない分）: 置き場から消えていたら作り直し、多すぎる札を抜く
    pub fn step(&mut self) -> Step {
        self.reconcile();
        let (Some(path), true) = (self.path.clone(), self.wants_fifo()) else {
            return Step::Done;
        };
        // 置き場から消された（一時 dir の掃除・手で消した）→ 作り直す。古い fifo を開いている cargo は
        // そのまま古い札で続き、新しい fifo は誰も持っていなくなってから札を足す
        if self.fifo.as_ref().is_some_and(|f| !f.is_at(&path)) {
            self.fifo = None;
            self.recreated += 1;
            crate::diag::persist_log("jobserver: fifo が置き場から消えていたので作り直す");
            self.reconcile();
        }
        let Some(fifo) = &self.fifo else {
            return Step::Done;
        };
        let free = match fifo.available() {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(format!("札を数えられない: {e}"));
                self.fifo = None;
                return Step::Done;
            }
        };
        match core_js::plan(free, self.tokens as usize, false) {
            Plan::Keep => Step::Done,
            Plan::Remove(n) => {
                let removed = fifo.take(n).unwrap_or(0);
                self.record(free, 0, removed, "over");
                Step::Done
            }
            Plan::Wait | Plan::Add(_) => Step::NeedHolders { free },
        }
    }

    /// 1 周の後半: 札を持ちうるプロセスの本数（`None` = 数えられない）を受けて足すか待つ。
    ///
    /// 確かめている間に空きが変わった（誰かが札を取った / 返した）なら、その回は足さない
    /// （0 本と読んだ走査の後に始まった cargo の札を、持ち逃げと取り違えない）
    pub fn settle(&mut self, free_at_step: usize, holders: Option<usize>) {
        if let Some(h) = holders {
            self.holders = Some((h, Instant::now()));
        }
        let Some(fifo) = &self.fifo else {
            return;
        };
        let Ok(free) = fifo.available() else {
            return;
        };
        let idle = holders == Some(0) && free == free_at_step;
        match core_js::plan(free, self.tokens as usize, idle) {
            Plan::Add(n) => {
                let added = fifo.put(n).unwrap_or(0);
                self.record(free, added, 0, "idle_short");
            }
            Plan::Remove(n) => {
                let removed = fifo.take(n).unwrap_or(0);
                self.record(free, 0, removed, "over");
            }
            Plan::Keep | Plan::Wait => {}
        }
    }

    /// テストと終了処理用: 1 周を通しで（`holders` は必要なときだけ呼ばれる）
    pub fn tick(&mut self, holders: impl FnOnce() -> Option<usize>) {
        if let Step::NeedHolders { free } = self.step() {
            let h = holders();
            self.settle(free, h);
        }
    }

    fn record(&mut self, free_before: usize, added: usize, removed: usize, why: &'static str) {
        if added == 0 && removed == 0 {
            return;
        }
        self.recounts += 1;
        self.last_recount = Some(Recount {
            at_ms: unix_ms(),
            free_before,
            added,
            removed,
            why,
        });
        crate::diag::persist_log(&format!(
            "jobserver: 札を数え直した（空き {free_before} → +{added} / -{removed}・総数 {}・{why}）",
            self.tokens
        ));
    }

    /// 終了: 札を持ちうるプロセスが 0 本なら fifo を消す（居なければ GUI の居ない間も cargo は従来どおり）。
    /// 居れば残す（再起動した GUI が開き直す）。何をしたかの 1 語を返す
    pub fn shutdown(&mut self, holders: impl FnOnce() -> Option<usize>) -> &'static str {
        self.stopped = true;
        let Some(fifo) = self.fifo.take() else {
            return "none";
        };
        let Some(path) = self.path.clone() else {
            return "none";
        };
        match holders() {
            Some(0) => {
                if fifo.unlink_if_at(&path) {
                    "removed"
                } else {
                    "gone"
                }
            }
            _ => "kept",
        }
    }

    /// 今の空き（配っていなければ `None`）
    pub fn free(&self) -> Option<usize> {
        self.fifo.as_ref().and_then(|f| f.available().ok())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn tokens(&self) -> u32 {
        self.tokens
    }

    pub fn last_recount(&self) -> Option<&Recount> {
        self.last_recount.as_ref()
    }

    /// status の `jobserver` 節（札の空きは今読む = `FIONREAD` 1 回。採らない）
    pub fn status(&self) -> Value {
        let mut v = json!({
            "env": core_js::ENV,
            "mode": self.mode.as_str(),
            "tokens": self.tokens,
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "recounts": self.recounts,
            "recreated": self.recreated,
            "last_recount": self.last_recount.as_ref().map(|r| json!({
                "at_ms": r.at_ms,
                "free_before": r.free_before,
                "added": r.added,
                "removed": r.removed,
                "why": r.why,
            })),
        });
        match self.off() {
            Some(off) => {
                v["state"] = json!("off");
                v["code"] = json!(off.code());
                v["reason"] = json!(off.text());
            }
            None => {
                v["state"] = json!(if self.mode == Mode::Keeper {
                    "active"
                } else {
                    "follower"
                });
                if let Some((_, value)) = self.pane_env() {
                    v["value"] = json!(value);
                }
            }
        }
        if let Some(free) = self.free() {
            v["free"] = json!(free);
            v["in_use"] = json!((self.tokens as usize).saturating_sub(free));
        }
        if let Some((h, at)) = self.holders {
            v["holders"] = json!(h);
            v["holders_age_s"] = json!(at.elapsed().as_secs());
        }
        v
    }
}

// ===== プロセスに 1 つ =====

struct Global {
    keeper: Keeper,
    settings_seen: Option<(Instant, Option<SystemTime>)>,
}

fn global() -> &'static Mutex<Option<Global>> {
    static G: OnceLock<Mutex<Option<Global>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(None))
}

fn lock() -> std::sync::MutexGuard<'static, Option<Global>> {
    global()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn settings_mtime() -> Option<SystemTime> {
    crate::settings::settings_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// この起動の fifo の置き場（data dir と一時 dir から。`tako_core::jobserver::fifo_path`）
pub fn planned_path() -> Option<PathBuf> {
    let data_dir = tako_core::paths::data_dir()?;
    core_js::fifo_path(&data_dir, &std::env::temp_dir())
}

/// 札の枚数（論理 CPU 数。隔離 / テストのプロセスだけが注入 `TAKO_1998_INJECT_TOKENS` を読む）
pub fn planned_tokens() -> u32 {
    let inject = tako_core::paths::is_verification_process()
        .then(|| std::env::var(core_js::INJECT_TOKENS_ENV).ok())
        .flatten();
    core_js::tokens(
        std::thread::available_parallelism().ok().map(|n| n.get()),
        inject.as_deref(),
    )
}

/// A/B（`TAKO_1992_NO_JOBSERVER=1`）が立っているか
pub fn legacy() -> bool {
    core_js::ab_off(std::env::var(core_js::AB_OFF_ENV).ok().as_deref())
}

/// 番人を立てる（GUI の起動時に 1 回。2 回目以降は何もしない）。`secondary` = 同じ data dir の
/// 先行の tako が居る（fifo を持たずパスだけ渡す）
pub fn start(secondary: bool) {
    {
        let mut g = lock();
        if g.is_some() {
            return;
        }
        let mode = if secondary {
            Mode::Follower
        } else {
            Mode::Keeper
        };
        let setting_on = crate::settings::load().resources.jobserver_on();
        let mut keeper = Keeper::new(mode, planned_path(), planned_tokens(), legacy(), setting_on);
        // fifo はここで開く（mkfifo + open だけ = 子プロセスも走査も無い）。最初に起こすペインから
        // 注入できるように。札は背景スレッドの最初の 1 周が、誰も持っていないと確かめてから足す
        keeper.reconcile();
        if let Some(off) = keeper.off() {
            crate::diag::persist_log(&format!("jobserver: 札を配らない（{}）", off.text()));
        }
        *g = Some(Global {
            keeper,
            settings_seen: Some((Instant::now(), settings_mtime())),
        });
    }
    // この OS では札を配らない（Windows = #2000）ので、設定を見張るスレッドも要らない
    if !SUPPORTED {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("tako-jobserver".into())
        .spawn(|| loop {
            if !tick_global() {
                return;
            }
            std::thread::sleep(TICK);
        });
    if let Err(e) = spawned {
        crate::diag::persist_log(&format!("jobserver: 番人のスレッドを立てられない: {e}"));
    }
}

/// 背景スレッドの 1 周（終わったら false）
fn tick_global() -> bool {
    let need = {
        let mut g = lock();
        let Some(global) = g.as_mut() else {
            return false;
        };
        if global.keeper.stopped {
            return false;
        }
        reload_settings_if_changed(global);
        let step = global.keeper.step();
        (step, global.keeper.path.clone())
    };
    if let (Step::NeedHolders { free }, Some(path)) = need {
        // 走査はロックの外（UI スレッドの pane_env を待たせない）
        let holders = crate::resources::jobserver_holders(&path, false);
        if let Some(global) = lock().as_mut() {
            global.keeper.settle(free, holders);
        }
    }
    true
}

fn reload_settings_if_changed(global: &mut Global) {
    let now = Instant::now();
    if global
        .settings_seen
        .is_some_and(|(at, _)| now.duration_since(at) < SETTINGS_RECHECK)
    {
        return;
    }
    let mtime = settings_mtime();
    let changed = global.settings_seen.is_none_or(|(_, seen)| seen != mtime);
    global.settings_seen = Some((now, mtime));
    if changed {
        global
            .keeper
            .set_setting(crate::settings::load().resources.jobserver_on());
    }
}

/// 設定を書いた直後に反映する（`tako queue limits` の受け口。mtime を待たない）
pub fn apply_setting(on: bool) {
    if let Some(global) = lock().as_mut() {
        global.keeper.set_setting(on);
        global.settings_seen = Some((Instant::now(), settings_mtime()));
    }
}

/// ペインへ足す env（**UI スレッドから呼ぶ**。覚えを写すだけで fifo も走査も触らない）
pub fn pane_env() -> Option<(&'static str, String)> {
    lock().as_ref().and_then(|g| g.keeper.pane_env())
}

/// status の `jobserver` 節（番人が居なければ `started: false`）
pub fn status_section() -> Value {
    match lock().as_ref() {
        Some(g) => g.keeper.status(),
        None => json!({
            "state": "off",
            "code": "not_started",
            "reason": "この tako は札を配っていない（GUI でない / まだ起動中）",
            "env": core_js::ENV,
        }),
    }
}

/// 終了処理（GUI の終了・セルフテストの失敗の出口）。札を持ちうるプロセスを新しく数えて、0 本なら
/// fifo を消す。何をしたかの 1 語（`removed` / `kept` / `gone` / `none`）。何度呼んでも壊れない
pub fn shutdown() -> &'static str {
    let mut g = lock();
    let Some(global) = g.as_mut() else {
        return "none";
    };
    if global.keeper.fifo.is_none() {
        global.keeper.stopped = true;
        return "none";
    }
    // 終了のときだけはロックを持ったまま数える（この後ペインは起きない。走査は番人のロックを取らない）
    let path = global.keeper.path.clone();
    global.keeper.shutdown(|| {
        path.as_deref()
            .and_then(|p| crate::resources::jobserver_holders(p, true))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> tako_core::test_residue::ScratchDir {
        tako_core::test_residue::ScratchDir::new(tag)
    }

    fn keeper_at(dir: &Path, tokens: u32) -> Keeper {
        let mut k = Keeper::new(
            Mode::Keeper,
            Some(dir.join("resources/jobserver.fifo")),
            tokens,
            false,
            true,
        );
        k.reconcile();
        k
    }

    #[test]
    fn 作りたては誰も持っていないと確かめてから札をn枚にする() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-fresh");
        let mut k = keeper_at(dir.path(), 4);
        assert_eq!(k.free(), Some(0));
        // 札を持ちうるプロセスが居る間は足さない（GUI の再起動で二重にしない）
        k.tick(|| Some(2));
        assert_eq!(k.free(), Some(0));
        // 数えられない（走査できない）ときも足さない
        k.tick(|| None);
        assert_eq!(k.free(), Some(0));
        k.tick(|| Some(0));
        assert_eq!(k.free(), Some(4));
        assert_eq!(k.last_recount().map(|r| r.added), Some(4));
        // ちょうど N 枚なら走査を求めない
        k.tick(|| panic!("N 枚ちょうどなら数えない"));
    }

    #[test]
    fn 多すぎる札は持っている人が居ても抜く() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-over");
        let mut k = keeper_at(dir.path(), 3);
        k.tick(|| Some(0));
        k.fifo.as_ref().unwrap().put(5).unwrap();
        assert_eq!(k.free(), Some(8));
        k.tick(|| panic!("多すぎる分は数えずに抜く"));
        assert_eq!(k.free(), Some(3));
        assert_eq!(
            k.last_recount().map(|r| (r.removed, r.why)),
            Some((5, "over"))
        );
    }

    #[test]
    fn 持ち逃げは誰も持っていなくなってから戻す() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-leak");
        let mut k = keeper_at(dir.path(), 4);
        k.tick(|| Some(0));
        // cargo が 3 枚取って kill された（返らない）
        assert_eq!(k.fifo.as_ref().unwrap().take(3).unwrap(), 3);
        k.tick(|| Some(1)); // まだ子が残っている
        assert_eq!(k.free(), Some(1));
        k.tick(|| Some(0));
        assert_eq!(k.free(), Some(4));
        assert_eq!(
            k.last_recount().map(|r| (r.free_before, r.added)),
            Some((1, 3))
        );
    }

    #[test]
    fn 確かめている間に札が動いたらその回は足さない() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-race");
        let mut k = keeper_at(dir.path(), 4);
        k.tick(|| Some(0));
        k.fifo.as_ref().unwrap().take(2).unwrap();
        let Step::NeedHolders { free } = k.step() else {
            panic!("足りないので数える");
        };
        // 0 本と読んだ走査の後に始まった cargo が 1 枚取った
        k.fifo.as_ref().unwrap().take(1).unwrap();
        k.settle(free, Some(0));
        assert_eq!(k.free(), Some(1), "取り違えて足さない");
        k.tick(|| Some(0));
        assert_eq!(k.free(), Some(4));
    }

    #[test]
    fn 再起動しても札はnを超えない() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-restart");
        let mut a = keeper_at(dir.path(), 4);
        a.tick(|| Some(0));
        // cargo が 2 枚持ったまま（fifo を開いたまま）GUI が終わる
        let cargo = Fifo::open_or_create(a.path().unwrap()).unwrap().0;
        cargo.take(2).unwrap();
        assert_eq!(
            a.shutdown(|| Some(1)),
            "kept",
            "持っている cargo が居れば残す"
        );
        assert!(a.path().unwrap().exists());
        // 新しい GUI は同じ fifo を開き直し、cargo が居る間は足さない
        let mut b = keeper_at(dir.path(), 4);
        assert_eq!(b.free(), Some(2));
        b.tick(|| Some(1));
        assert_eq!(b.free(), Some(2), "足すと 2 + 4 = 6 > N");
        // cargo が返して終わる
        cargo.put(2).unwrap();
        drop(cargo);
        b.tick(|| Some(0));
        assert_eq!(b.free(), Some(4));
        assert!(
            b.last_recount().is_none(),
            "返った分で揃い、数え直しは要らない"
        );
    }

    #[test]
    fn 終了は誰も持っていなければfifoを消す() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-quit");
        let mut k = keeper_at(dir.path(), 2);
        k.tick(|| Some(0));
        let path = k.path().unwrap().to_path_buf();
        assert_eq!(k.shutdown(|| Some(0)), "removed");
        assert!(!path.exists(), "GUI の居ない間の cargo は従来どおり走る");
        assert!(k.pane_env().is_none());
        assert_eq!(k.shutdown(|| Some(0)), "none", "2 回目は何もしない");
    }

    #[test]
    fn 設定offは注入をやめfifoを消しonで作り直す() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-setting");
        let mut k = keeper_at(dir.path(), 2);
        k.tick(|| Some(0));
        let path = k.path().unwrap().to_path_buf();
        assert!(k.pane_env().is_some());
        k.set_setting(false);
        assert!(k.pane_env().is_none(), "切った直後に作るペインには渡さない");
        assert!(
            !path.exists(),
            "前に作ったペインの cargo も次からは従来どおり"
        );
        assert_eq!(k.off(), Some(Off::Setting));
        assert_eq!(k.status()["state"], "off");
        assert_eq!(k.status()["code"], "setting");
        k.set_setting(true);
        assert!(path.exists());
        k.tick(|| Some(0));
        assert_eq!(k.free(), Some(2));
        let (key, value) = k.pane_env().unwrap();
        assert_eq!(key, "CARGO_MAKEFLAGS");
        assert_eq!(value, core_js::makeflags(2, &path));
    }

    #[test]
    fn 置き場から消されたら作り直す() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-heal");
        let mut k = keeper_at(dir.path(), 3);
        k.tick(|| Some(0));
        std::fs::remove_file(k.path().unwrap()).unwrap();
        k.tick(|| Some(0));
        assert!(k.path().unwrap().exists());
        assert_eq!(k.free(), Some(3));
        assert_eq!(k.status()["recreated"], 1);
    }

    #[test]
    fn 置き場にfifo以外があれば注入しない() {
        if !SUPPORTED {
            return;
        }
        let dir = scratch("js-blocked");
        let path = dir.path().join("resources/jobserver.fifo");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
        let mut k = keeper_at(dir.path(), 3);
        k.tick(|| Some(0));
        assert!(k.pane_env().is_none(), "札待ちで止まる経路を作らない");
        assert_eq!(k.status()["code"], "error");
        assert_eq!(std::fs::read(&path).unwrap(), b"x");
    }

    #[test]
    fn ab_offとセカンダリと未対応() {
        let dir = scratch("js-modes");
        let path = dir.path().join("resources/jobserver.fifo");
        let mut legacy = Keeper::new(Mode::Keeper, Some(path.clone()), 4, true, true);
        legacy.tick(|| Some(0));
        assert!(legacy.pane_env().is_none());
        assert!(!path.exists(), "A/B は作らない");
        let follower = Keeper::new(Mode::Follower, Some(path.clone()), 4, false, true);
        if SUPPORTED {
            assert_eq!(
                follower.pane_env().map(|(_, v)| v),
                Some(core_js::makeflags(4, &path)),
                "セカンダリは先行の tako の fifo を渡すだけ"
            );
            assert_eq!(follower.status()["state"], "follower");
        } else {
            assert_eq!(follower.off(), Some(Off::Unsupported));
        }
        let none = Keeper::new(Mode::Keeper, None, 4, false, true);
        if SUPPORTED {
            assert_eq!(none.off(), Some(Off::NoPlace));
        }
    }
}
