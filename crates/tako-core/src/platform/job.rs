//! 子孫をまとめて回収する入れ物（抽象境界 B30。#2454）
//!
//! ## なぜ要るか
//!
//! Windows の `TerminateProcess` は子孫を道連れにしない（unix のプロセスグループや PTY の
//! HUP に当たる仕組みが無い）。ペインを閉じても・言語サーバを止めても、シェル / agent CLI /
//! サーバが起こした孫が残る（FR-2.33.9 に、実機で `claude auth login` が 1 日 46 本たまって
//! CPU 100% に張り付いた記録がある）。
//!
//! - ペイン: ConPTY を閉じれば疑似コンソールに繋がった子は止まるが、`CREATE_NO_WINDOW` で
//!   起きた子（stdio をパイプにした MCP サーバ・言語サーバなど）はコンソールに繋がっていないので残る
//! - 言語サーバ: npm の `.cmd` は cmd.exe 経由で起きるので、`Child::kill()` で止まるのは
//!   cmd.exe だけで node.exe が残る
//!
//! ## 作り
//!
//! 子 1 つ（ペインの PTY 直下の子 / 言語サーバ 1 つ）ごとに Job を 1 つ作り、起こした直後に入れる。
//! 以後その子が起こすものは Job を受け継ぐ（Windows の仕様）。Job は
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 付き。
//!
//! - `JOB_OBJECT_LIMIT_BREAKAWAY_OK` を付ける: `CREATE_BREAKAWAY_FROM_JOB` で明示的に外へ出る子
//!   （親の寿命から切り離すと宣言した常駐 = unix の `setsid` に当たる）は出られる。付けないと
//!   その子の `CreateProcess` が ERROR_ACCESS_DENIED で失敗する（tako のペインの中でだけ壊れる）
//! - `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` は付けない（付けると全部の子が黙って外へ出て、回収にならない）
//!
//! ## 寿命の決まり
//!
//! | いつ | コンソールのプロセス | GUI のアプリとその子孫 |
//! |---|---|---|
//! | ペインを閉じる（[`Job::close_for_pane`]） | 止める | **残す** |
//! | tako を普通に終了する（`on_app_quit` → `terminal::release_descendants_on_quit`） | 止める | **残す** |
//! | 言語サーバを止める（[`Job::terminate`] / drop） | 止める | 止める |
//! | 打ち切り・取り消し（probe の上限・`tako queue` の取り消し・AI 命名の上限 = [`KillScope::stop`]。#2462） | 止める | **残す** |
//! | 上の子が普通に終わる（[`KillScope`] の drop。#2462） | **残す** | **残す** |
//! | エージェントの再起動（session-restart の harness = [`Job::stop_descendants_of`]。#2462） | そのエージェントの子孫だけ止める | **残す** |
//! | tako が落ちる（OS がハンドルを閉じる = `KILL_ON_JOB_CLOSE`） | 止める | 止める |
//!
//! 普通に終わった子の残り（常駐。unix で親を失っても残るもの）を止めないのは、#2462 の前と unix の
//! 振る舞いを変えないため。止めるのは tako が止めると決めたとき（打ち切り・取り消し・再起動）だけ。
//!
//! ペインから起こした GUI アプリ（`code .` の初回・`notepad` など）をペインと一緒に消さない
//! （Windows Terminal も macOS の `open` も残す。消すと「ペインを閉じたらエディタごと消えた」になる）。
//! 残すのは GUI サブシステムのプロセス（実行ファイルの PE ヘッダ = [`pe_subsystem`]）と、親を Job の
//! 中でたどって GUI に当たるもの（VS Code の統合ターミナルのシェルや言語サーバはコンソールだが VS Code の
//! 子孫 = 残す）。コンソールのホスト（[`is_console_host`]）は GUI と数えない。判断は [`split_for_release`] の 1 実装。止めたあと `KILL_ON_JOB_CLOSE` を外してから
//! ハンドルを閉じる（外さないと閉じた瞬間に残したものも止まる）。
//!
//! ## 起動直後の窓
//!
//! 子は動き出してから Job へ入る（`CreateProcess` の時点では入れられない）:
//!
//! - ペインの ConPTY は alacritty_terminal が `CreateProcessW` を呼び、属性リストを中で 1 枠
//!   （疑似コンソール）だけ作るので、`PROC_THREAD_ATTRIBUTE_JOB_LIST` を足す口が無い。
//!   作成フラグも固定で `CREATE_SUSPENDED` を渡せない
//! - std の `Command` に属性を足す `CommandExt::raw_attribute` は unstable（toolchain 1.95 で確認）
//!
//! 入れるのは起こした直後（同じスレッドで次に呼ぶ）なので、窓はシェル / cmd.exe が初期化を終えて
//! 子を起こすまでの時間より十分短い。Windows CI の実プロセスのテストは、孫が Job に入っていること
//! （[`Job::contains`]）を確かめてから止める。
//!
//! ## 器（psmux）は入れない
//!
//! 器のペインの PTY 直下の子は器のクライアントで、器のサーバー（`new-session -A` が無ければ起こす）は
//! その子として起きる。入れるとサーバーまで Job を受け継ぎ、ペインを閉じたとき・GUI を終えたときに
//! 器ごと消える（再起動をまたいで残す約束が壊れる）。器のクライアントかどうかは呼び手が判断する
//! （`TerminalSession::spawn_in` が `tmux_backend::is_container_client` で見る）。
//!
//! 打ち切り・取り消しの子（[`KillScope`]）も同じ理由で器の CLI は入れない（probe の
//! `wait_with_timeout` が実行ファイル名で見る。器の 1 回の問い合わせがサーバーを起こすことがある）。
//!
//! unix は何もしない（[`Job::contain`] が `None`）: ペインは PTY の HUP、言語サーバは stdin の EOF と
//! kill に任せる（従来どおり）。A/B は `TAKO_2454_LEGACY=1`（Job を作らない = #2454 前）と
//! `TAKO_2454_KILL_GUI=1`（ペインを閉じるときも GUI ごと全部止める）。

use std::collections::{HashMap, HashSet};

/// この OS で子孫を Job で回収するか（Windows だけ真）
pub const SUPPORTED: bool = cfg!(windows);

/// A/B の旧経路（`TAKO_2454_LEGACY=1`）。Job を作らない = #2454 前の回収（直下の子だけ）
pub const ENV_LEGACY: &str = "TAKO_2454_LEGACY";

/// A/B（`TAKO_2454_KILL_GUI=1`）。ペインを閉じるときも GUI のアプリごと全部止める
/// （[`Job::close_for_pane`] が [`Job::terminate`] と同じになる = GUI を残す前の #2454）
pub const ENV_KILL_GUI: &str = "TAKO_2454_KILL_GUI";

/// 注入（`TAKO_2454_INJECT=assign_fail`）。**テストプロセスでだけ**、子を Job へ入れる段を
/// 失敗させる = 割り当てに失敗した回（素の kill へ落ちる）を作る。製品のバイナリでは無視する
pub const ENV_INJECT: &str = "TAKO_2454_INJECT";

/// 実行ファイルの PE ヘッダのサブシステムの値（winnt.h の `IMAGE_SUBSYSTEM_WINDOWS_GUI`）
pub const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;

/// 閉じるときに「列挙 → 止める」を繰り返す上限。止めている間に起きた子を拾う（ふつうは 1 回で済む）
const RELEASE_ROUNDS: usize = 3;

fn legacy() -> bool {
    std::env::var(ENV_LEGACY).is_ok_and(|v| v == "1")
}

fn kill_gui() -> bool {
    std::env::var(ENV_KILL_GUI).is_ok_and(|v| v == "1")
}

fn injected_assign_failure() -> bool {
    crate::paths::is_test_process() && std::env::var(ENV_INJECT).is_ok_and(|v| v == "assign_fail")
}

/// 子孫の入れ物。drop で中身を全部止める（ペインは [`Job::close_for_pane`] で閉じる）
pub struct Job {
    /// drop と [`Job::release_keeping_gui`] が 1 回だけ取り出す（それまで常に `Some`）
    inner: Option<imp::Job>,
}

/// [`Job::release_keeping_gui`] の結果（テストと診断用）
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Release {
    /// 止めたプロセスの数
    pub stopped: usize,
    /// 残した（GUI のアプリとその子孫の）プロセスの数
    pub kept: usize,
}

/// 閉じるときの振り分け（[`split_for_release`] の答え）
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReleasePlan {
    /// 止める pid（コンソールのプロセスで、GUI の子孫でないもの）
    pub stop: Vec<u32>,
    /// 残す pid（GUI のプロセスと、親を Job の中でたどって GUI に当たるもの）
    pub keep: Vec<u32>,
}

impl Job {
    /// `pid` の子を入れた Job を作る。
    ///
    /// この OS では作らない（unix）・A/B の旧経路・作れない / 入れられないときは `None`
    /// （呼び手は従来の止め方 = 直下の子だけへ落ちる。panic しない）。
    ///
    /// `pid` は呼び手がその子のハンドル（`Child` / ConPTY の子）を持っている間に渡すこと
    /// （持っている間は pid が別のプロセスへ再利用されない）
    pub fn contain(pid: u32) -> Option<Job> {
        if !SUPPORTED || legacy() {
            return None;
        }
        let made = if injected_assign_failure() {
            Err("注入（TAKO_2454_INJECT=assign_fail）".to_string())
        } else {
            imp::Job::contain(pid)
        };
        match made {
            Ok(inner) => Some(Job { inner: Some(inner) }),
            Err(reason) => {
                tracing::warn!("子孫の回収の入れ物を作れない（直下の子だけを止める）: {reason}");
                None
            }
        }
    }

    /// 中身を全部止める（冪等。空の Job・止め終わった Job に撃ってもよい）
    pub fn terminate(&self) -> Result<(), String> {
        self.inner.as_ref().map_or(Ok(()), imp::Job::terminate)
    }

    /// `pid` がこの Job に入っているか。確かめられない（開けない）ときは `None`。
    /// 製品の判断には使わない（テストと診断用）
    pub fn contains(&self, pid: u32) -> Option<bool> {
        self.inner.as_ref().and_then(|job| job.contains(pid))
    }

    /// ペインを閉じるときの止め方: コンソールのプロセスは止め、GUI のアプリとその子孫は残す
    /// （[`Job::release_keeping_gui`]）。A/B の `TAKO_2454_KILL_GUI=1` では全部止める
    pub fn close_for_pane(self) {
        if kill_gui() {
            // drop が中身を全部止める
            return;
        }
        let release = self.release_keeping_gui();
        tracing::debug!(
            "ペインの子孫: {} 本を止め {} 本（GUI）を残した",
            release.stopped,
            release.kept
        );
    }

    /// コンソールのプロセスを止め、GUI のアプリとその子孫を残して入れ物を手放す。
    ///
    /// 中身を列挙して [`split_for_release`] で振り分け、止める側を 1 本ずつ止める（止める前に
    /// その pid がまだこの Job に居ることを開いたハンドルで確かめる = 列挙の後に pid が別の
    /// プロセスへ再利用されても撃たない）。GUI が 1 本も居なければ Job ごと止める。最後に
    /// `KILL_ON_JOB_CLOSE` を外してからハンドルを閉じる。列挙できないときは判断できないので
    /// 全部止める（従来の止め方）
    pub fn release_keeping_gui(mut self) -> Release {
        let mut release = Release::default();
        let Some(job) = self.inner.take() else {
            return release;
        };
        for _ in 0..RELEASE_ROUNDS {
            let members = match job.members() {
                Ok(members) => members,
                Err(reason) => {
                    tracing::warn!("子孫を列挙できないので全部止める: {reason}");
                    let _ = job.terminate();
                    return release;
                }
            };
            if members.is_empty() {
                break;
            }
            let gui: HashSet<u32> = members.iter().copied().filter(|pid| is_gui(*pid)).collect();
            if gui.is_empty() {
                // 残すものが無い = Job ごと止める（列挙の後に起きた子も 1 回で止まる）
                if let Err(reason) = job.terminate() {
                    tracing::warn!("子孫の回収の入れ物を止められない: {reason}");
                }
                release.stopped += members.len();
                return release;
            }
            let parents: HashMap<u32, u32> = crate::platform::procinfo::snapshot_checked()
                .unwrap_or_default()
                .into_iter()
                .map(|entry| (entry.pid, entry.ppid))
                .collect();
            let plan = split_for_release(
                &members,
                |pid| gui.contains(&pid),
                |pid| parents.get(&pid).copied(),
                crate::platform::procinfo::start_time_unix,
            );
            release.kept = plan.keep.len();
            if plan.stop.is_empty() {
                break;
            }
            for pid in plan.stop {
                if job.stop_member(pid) {
                    release.stopped += 1;
                }
            }
        }
        if let Err(reason) = job.clear_kill_on_close() {
            tracing::warn!("KILL_ON_JOB_CLOSE を外せない（閉じると残した GUI も止まる）: {reason}");
        }
        release
        // job の drop がハンドルを閉じる
    }

    /// `root` の子孫を、この Job の中で止める（#2462。エージェントの再起動 = session-restart の harness）。
    ///
    /// ペインの Job はシェルごと入っているので Job ごとは止められない（シェルまで止まる）。中身を
    /// 列挙して親を Job の中でたどり（[`descendants_in`]）、`root` の子孫だけを選ぶ。寿命はペインを
    /// 閉じるのと同じで、GUI のアプリとその子孫は残す（[`split_for_release`]）。`root` 自身は止めない
    /// （呼び手が先に止める。止めた後に呼んでよい = Windows は親が終わっても子の親の pid を書き換えない）。
    /// 止める前にその pid がまだこの Job に居ることを開いたハンドルで確かめる。
    /// 列挙できない・`root` の子孫が Job に居ないときは何もしない（ペインのシェルを撃たない）
    pub fn stop_descendants_of(&self, root: u32) -> Release {
        let mut release = Release::default();
        let Some(job) = self.inner.as_ref() else {
            return release;
        };
        // 前の回で止めた中継ぎ（もう列挙に出ない）の親も覚えておく = その子もたどれる
        let mut parents: HashMap<u32, u32> = HashMap::new();
        let mut stopped: HashSet<u32> = HashSet::new();
        for _ in 0..RELEASE_ROUNDS {
            let members = match job.members() {
                Ok(members) => members,
                Err(reason) => {
                    tracing::warn!("子孫を列挙できないので止めない: {reason}");
                    return release;
                }
            };
            parents.extend(
                crate::platform::procinfo::snapshot_checked()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|entry| (entry.pid, entry.ppid)),
            );
            let parent = |pid: u32| parents.get(&pid).copied();
            let targets = descendants_in(
                &members,
                root,
                &stopped,
                parent,
                crate::platform::procinfo::start_time_unix,
            );
            if targets.is_empty() {
                break;
            }
            let gui: HashSet<u32> = targets.iter().copied().filter(|pid| is_gui(*pid)).collect();
            let plan = split_for_release(
                &targets,
                |pid| gui.contains(&pid),
                parent,
                crate::platform::procinfo::start_time_unix,
            );
            release.kept = plan.keep.len();
            if plan.stop.is_empty() {
                break;
            }
            for pid in plan.stop {
                if job.stop_member(pid) {
                    release.stopped += 1;
                }
                stopped.insert(pid);
            }
        }
        release
    }

    /// 中身を止めずに手放す（#2462。[`KillScope`] の drop = 子が普通に終わった）。
    /// `KILL_ON_JOB_CLOSE` を外してから閉じる。外せないときは閉じると中身が止まる（警告を残す）
    fn detach(mut self) {
        if let Some(job) = self.inner.take() {
            if let Err(reason) = job.clear_kill_on_close() {
                tracing::warn!(
                    "KILL_ON_JOB_CLOSE を外せない（閉じると残ったものも止まる）: {reason}"
                );
            }
            // job の drop がハンドルを閉じる
        }
    }
}

/// 打ち切り・取り消しの kill を子孫まで届かせる入れ物（#2462。境界 B30 の上の薄い型）。
///
/// probe の上限（`probe::wait_with_timeout`）・`tako queue` の取り消し・AI 命名の上限が使う。
/// 起こした直後に [`KillScope::contain`] で入れ、止めると決めたときだけ [`KillScope::stop`]
/// （コンソールのプロセスは止め、GUI のアプリとその子孫は残す = ペインを閉じるのと同じ）。
/// **drop では何も止めない**: 子が普通に終わった後の残り（常駐）は #2462 の前も unix も残るので、
/// `KILL_ON_JOB_CLOSE` を外してから閉じる。呼び手のプロセスが落ちたときは OS がハンドルを閉じて
/// 全部止まる（寿命の決まりの最後の行）。
///
/// unix・A/B の legacy（`TAKO_2454_LEGACY=1`）・入れられなかった回は空で、`stop` は何もしない
/// （呼び手は従来どおり直下の子を kill する = 孫は残る）
pub struct KillScope {
    /// `stop` と drop が 1 回だけ取り出す
    job: Option<Job>,
}

impl KillScope {
    /// `pid` の子を子孫ごと入れる（[`Job::contain`]。`pid` の扱いの約束も同じ）
    pub fn contain(pid: u32) -> KillScope {
        KillScope {
            job: Job::contain(pid),
        }
    }

    /// 何も入れない（入れない子 = 器の CLI・既に終わった子）
    pub fn empty() -> KillScope {
        KillScope { job: None }
    }

    /// 入れられたか（テストと診断用）
    pub fn is_contained(&self) -> bool {
        self.job.is_some()
    }

    /// `pid` が入っているか（[`Job::contains`]。テストと診断用）
    pub fn contains(&self, pid: u32) -> Option<bool> {
        self.job.as_ref().and_then(|job| job.contains(pid))
    }

    /// 打ち切る・取り消す: コンソールのプロセスを止め、GUI のアプリとその子孫は残して手放す
    /// （[`Job::release_keeping_gui`]）。2 回目以降と空の入れ物は何もしない
    pub fn stop(&mut self) -> Release {
        self.job
            .take()
            .map(Job::release_keeping_gui)
            .unwrap_or_default()
    }
}

impl Drop for KillScope {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            job.detach();
        }
    }
}

/// `root` の子孫（`root` 自身は含まない）を Job の中の pid から選ぶ（純粋関数。macOS からも検査できる）。
///
/// `members` の pid の親（`parent`）をたどって `root` に着くものを返す。間でたどってよいのは
/// `members` と `via`（同じ呼び出しの前の回で止めた = もう列挙に出ない中継ぎ）だけ。`root` は
/// `members` に居なくてよい（止めた後）。親子の読み違いは子孫の側へ倒さない（[`split_for_release`] と同じ）:
///
/// - 親が分からない・Job の外（`root` を除く）ならそこで止まる（= 子孫でない）
/// - 親の起動時刻（`started`）が子より後なら、その親は pid が再利用された別のプロセスなのでたどらない
///   （`root` が止まった後に別のプロセスがその pid を取った回も同じ）。時刻が分からなければ親子を信じる
/// - 親子の読みが輪になってもたどる回数は中の数までで打ち切る
pub fn descendants_in(
    members: &[u32],
    root: u32,
    via: &HashSet<u32>,
    parent: impl Fn(u32) -> Option<u32>,
    started: impl Fn(u32) -> Option<u64>,
) -> Vec<u32> {
    let inside: HashSet<u32> = members.iter().chain(via.iter()).copied().collect();
    let reaches_root = |pid: u32| {
        let mut current = pid;
        for _ in 0..=inside.len() {
            let Some(up) = parent(current).filter(|up| *up != current) else {
                return false;
            };
            if let (Some(up_started), Some(started)) = (started(up), started(current)) {
                if up_started > started {
                    return false;
                }
            }
            if up == root {
                return true;
            }
            if !inside.contains(&up) {
                return false;
            }
            current = up;
        }
        false
    };
    members
        .iter()
        .copied()
        .filter(|pid| *pid != root && reaches_root(*pid))
        .collect()
}

impl Drop for Job {
    fn drop(&mut self) {
        // ハンドルを閉じれば KILL_ON_JOB_CLOSE でも止まるが、ハンドルが他所に残っていても
        // 止まるよう明示的に止めてから閉じる（閉じるのは inner の drop）
        if let Some(job) = self.inner.take() {
            if let Err(reason) = job.terminate() {
                tracing::warn!("子孫の回収の入れ物を止められない: {reason}");
            }
        }
    }
}

/// PE ヘッダを探す範囲（`e_lfanew` はふつう数百バイト以内）
const PE_HEADER_BYTES: u64 = 4096;

/// その pid の実行ファイルが GUI サブシステムか。パスは境界 B5（`procinfo::image_path`）で引き、
/// ヘッダは [`pe_subsystem`] で読む。引けない・読めない・コンソールのホストは false
/// （コンソールとして扱う = 止める側。#2454 の目的は残骸を残さないこと）
fn is_gui(pid: u32) -> bool {
    use std::io::Read;
    let Some(path) = crate::platform::procinfo::image_path(pid) else {
        return false;
    };
    if is_console_host(&path.to_string_lossy()) {
        return false;
    }
    let mut header = Vec::new();
    let read = std::fs::File::open(&path)
        .and_then(|file| file.take(PE_HEADER_BYTES).read_to_end(&mut header));
    read.is_ok() && pe_subsystem(&header) == Some(IMAGE_SUBSYSTEM_WINDOWS_GUI)
}

/// コンソールのホスト（`conhost.exe` / `OpenConsole.exe`）か（純粋関数。macOS からも検査できる）。
///
/// コンソールのプロセスには Windows がホストを子として付ける（`CREATE_NO_WINDOW` でも付く）。
/// ホストの実行ファイルのサブシステムが何であれ、コンソールの側の部品なので GUI のアプリとは数えない
/// （残すかどうかは親 = コンソールのプロセスに従う。ホストは繋がっている相手が居なくなれば自分で終わる）
pub fn is_console_host(image_path: &str) -> bool {
    let name = image_path.rsplit(['/', '\\']).next().unwrap_or(image_path);
    name.eq_ignore_ascii_case("conhost.exe") || name.eq_ignore_ascii_case("OpenConsole.exe")
}

/// 実行ファイルの先頭のバイト列から PE のサブシステムを読む（純粋関数。macOS からも検査できる）。
///
/// `MZ` → `e_lfanew`（0x3C）→ `PE\0\0` → COFF ヘッダ（20 バイト）→ オプションヘッダの
/// `Magic`（PE32 = 0x10B / PE32+ = 0x20B）を確かめ、オプションヘッダの先頭から 68 バイト目の
/// `Subsystem`（どちらの形でも同じ位置）を返す。PE でない・足りない・壊れているときは `None`
pub fn pe_subsystem(image: &[u8]) -> Option<u16> {
    if image.get(0..2)? != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(image.get(0x3C..0x40)?.try_into().ok()?) as usize;
    if image.get(e_lfanew..e_lfanew.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let optional = e_lfanew.checked_add(4 + 20)?;
    let magic = image.get(optional..optional.checked_add(2)?)?;
    if magic != [0x0B, 0x01] && magic != [0x0B, 0x02] {
        return None;
    }
    let at = optional.checked_add(68)?;
    let subsystem = image.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([subsystem[0], subsystem[1]]))
}

/// 閉じるときの振り分け（純粋関数。macOS からも検査できる）。
///
/// `members` は Job の中の pid。`is_gui` が真のものと、親（`parent`）を **Job の中で**たどって
/// `is_gui` に当たるものを残し、残りを止める。親子の読み違いは残す側へ倒さない:
///
/// - 親が Job の外・親が分からない（列挙の失敗を含む）ならそこで止まる（= 止める側）
/// - 親の起動時刻（`started`）が子より後なら、その親は pid が再利用された別のプロセス
///   （本当の親は子より先に起きている）なのでたどらない。時刻が分からなければ親子を信じる
/// - 親子の読みが輪になってもたどる回数は Job の中の数までで打ち切る
pub fn split_for_release(
    members: &[u32],
    is_gui: impl Fn(u32) -> bool,
    parent: impl Fn(u32) -> Option<u32>,
    started: impl Fn(u32) -> Option<u64>,
) -> ReleasePlan {
    let inside: HashSet<u32> = members.iter().copied().collect();
    let gui_rooted = |pid: u32| {
        let mut current = pid;
        for _ in 0..=inside.len() {
            if is_gui(current) {
                return true;
            }
            let Some(up) = parent(current).filter(|up| *up != current && inside.contains(up))
            else {
                return false;
            };
            if let (Some(up_started), Some(started)) = (started(up), started(current)) {
                if up_started > started {
                    return false;
                }
            }
            current = up;
        }
        false
    };
    let mut plan = ReleasePlan::default();
    for &pid in members {
        if gui_rooted(pid) {
            plan.keep.push(pid);
        } else {
            plan.stop.push(pid);
        }
    }
    plan
}

#[cfg(not(windows))]
mod imp {
    /// unix では作らない（値が存在しない型）
    pub(super) enum Job {}

    impl Job {
        pub(super) fn contain(_pid: u32) -> Result<Job, String> {
            Err("この OS では使わない".to_string())
        }

        pub(super) fn terminate(&self) -> Result<(), String> {
            match *self {}
        }

        pub(super) fn contains(&self, _pid: u32) -> Option<bool> {
            match *self {}
        }

        pub(super) fn members(&self) -> Result<Vec<u32>, String> {
            match *self {}
        }

        pub(super) fn stop_member(&self, _pid: u32) -> bool {
            match *self {}
        }

        pub(super) fn clear_kill_on_close(&self) -> Result<(), String> {
            match *self {}
        }
    }
}

/// Windows の腕。`CreateJobObjectW` + `SetInformationJobObject` + `AssignProcessToJobObject`
#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    /// `PROCESS_TERMINATE` / `PROCESS_SET_QUOTA`（winnt.h）。`AssignProcessToJobObject` に要る権利
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    /// `PROCESS_QUERY_LIMITED_INFORMATION`（winnt.h）。`IsProcessInJob` に要る権利
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    /// winnt.h の `JOB_OBJECT_LIMIT_*`
    const JOB_OBJECT_LIMIT_BREAKAWAY_OK: u32 = 0x0000_0800;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    /// `JOBOBJECTINFOCLASS` の `JobObjectBasicProcessIdList` / `JobObjectExtendedLimitInformation`
    const JOB_OBJECT_BASIC_PROCESS_ID_LIST: i32 = 3;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    /// winerror.h の `ERROR_MORE_DATA`（列挙の入れ物が足りない）
    const ERROR_MORE_DATA: i32 = 234;
    /// 止めたプロセスの終了コード。`platform::process::terminate` と同じ 1
    /// （`STILL_ACTIVE` でなければ生存判定が終了済みと読む。#2275）
    const EXIT_CODE: u32 = 1;
    /// 列挙の入れ物の上限（pid の数）
    const MAX_MEMBERS: usize = 65_536;

    /// `JOBOBJECT_BASIC_LIMIT_INFORMATION`（winnt.h）
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    /// `IO_COUNTERS`（winnt.h）
    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`（winnt.h）
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    // winnt.h の 64 bit の大きさ（144 バイト）と一致すること。ずれると SetInformationJobObject が
    // ERROR_BAD_LENGTH で断る（= 回収されないまま黙って素の kill へ落ちる）のでコンパイル時に潰す
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(std::mem::size_of::<ExtendedLimitInformation>() == 144);

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *mut c_void, length: u32) -> i32;
        fn QueryInformationJobObject(
            job: Handle,
            class: i32,
            info: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn IsProcessInJob(process: Handle, job: Handle, result: *mut i32) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    fn last_error() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    /// 開いたプロセスのハンドル（drop で閉じる）
    struct Process(Handle);

    impl Process {
        fn open(access: u32, pid: u32) -> Option<Process> {
            // SAFETY: 戻りは null を検査し、drop で 1 回だけ閉じる
            let handle = unsafe { OpenProcess(access, 0, pid) };
            (!handle.is_null()).then_some(Process(handle))
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: open が返した有効なハンドルを 1 回だけ閉じる
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Job のハンドル（閉じるのは drop の 1 か所）
    pub(super) struct Job {
        handle: Handle,
    }

    // SAFETY: カーネルオブジェクトのハンドルはどのスレッドからでも使える。
    // 書き換える状態を持たない（止める・問う・閉じるだけ）
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub(super) fn contain(pid: u32) -> Result<Job, String> {
            if pid == 0 {
                return Err("pid 0 は入れられない".to_string());
            }
            // SAFETY: 戻りのハンドルは null を検査し、以後は `Job` の drop が閉じる。
            // 属性は null = 子へ継承されないハンドル
            let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if handle.is_null() {
                return Err(format!("CreateJobObjectW: {}", last_error()));
            }
            // 以降の失敗の戻りでも drop がハンドルを閉じる（空の Job なので閉じても誰も止まらない）
            let job = Job { handle };
            job.set_limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK)?;
            let process = Process::open(PROCESS_SET_QUOTA | PROCESS_TERMINATE, pid)
                .ok_or_else(|| format!("OpenProcess({pid}): {}", last_error()))?;
            // SAFETY: どちらも有効なハンドル
            if unsafe { AssignProcessToJobObject(job.handle, process.0) } == 0 {
                return Err(format!("AssignProcessToJobObject({pid}): {}", last_error()));
            }
            Ok(job)
        }

        fn set_limits(&self, flags: u32) -> Result<(), String> {
            let mut info = ExtendedLimitInformation::default();
            info.basic.limit_flags = flags;
            // SAFETY: info は winnt.h と同じ配置（大きさはコンパイル時に検査）で、呼び出しの間は生きている
            let set = unsafe {
                SetInformationJobObject(
                    self.handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&mut info as *mut ExtendedLimitInformation).cast::<c_void>(),
                    std::mem::size_of::<ExtendedLimitInformation>() as u32,
                )
            };
            if set == 0 {
                return Err(format!("SetInformationJobObject: {}", last_error()));
            }
            Ok(())
        }

        pub(super) fn terminate(&self) -> Result<(), String> {
            // SAFETY: drop まで有効なハンドル。空の Job へ撃っても中身が無いだけ
            if unsafe { TerminateJobObject(self.handle, EXIT_CODE) } == 0 {
                return Err(format!("TerminateJobObject: {}", last_error()));
            }
            Ok(())
        }

        pub(super) fn contains(&self, pid: u32) -> Option<bool> {
            let process = Process::open(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
            self.holds(&process)
        }

        /// 開いたプロセスがこの Job に入っているか（問い合わせに失敗したら `None`）
        fn holds(&self, process: &Process) -> Option<bool> {
            let mut result = 0i32;
            // SAFETY: どちらも有効なハンドルで、result は呼び出しの間生きている
            let ok = unsafe { IsProcessInJob(process.0, self.handle, &mut result) };
            (ok != 0).then_some(result != 0)
        }

        /// 中の pid の列挙（`JOBOBJECT_BASIC_PROCESS_ID_LIST`）。入れ物が足りなければ広げて取り直す
        pub(super) fn members(&self) -> Result<Vec<u32>, String> {
            // 先頭は 2 つの u32（割り当て数・列挙できた数）で、その後ろに ULONG_PTR の並び
            let header = 8 / std::mem::size_of::<usize>();
            let mut capacity = 256usize;
            loop {
                let mut buffer = vec![0usize; header + capacity];
                let bytes = (buffer.len() * std::mem::size_of::<usize>()) as u32;
                // SAFETY: buffer は bytes の大きさで呼び出しの間生きている。戻りの長さは使わない
                let ok = unsafe {
                    QueryInformationJobObject(
                        self.handle,
                        JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                        buffer.as_mut_ptr().cast::<c_void>(),
                        bytes,
                        std::ptr::null_mut(),
                    )
                };
                let error = last_error();
                // SAFETY: 先頭 8 バイトは usize の並びの中（header 個ぶん）にあり、u32 の並びとして読める
                let (assigned, listed) = unsafe {
                    let counts = buffer.as_ptr().cast::<u32>();
                    (*counts as usize, *counts.add(1) as usize)
                };
                let short = (ok == 0 && error.raw_os_error() == Some(ERROR_MORE_DATA))
                    || (ok != 0 && listed < assigned);
                if short && capacity < MAX_MEMBERS {
                    capacity = (assigned + 16).max(capacity * 2).min(MAX_MEMBERS);
                    continue;
                }
                if ok == 0 {
                    return Err(format!("QueryInformationJobObject: {error}"));
                }
                let listed = listed.min(capacity);
                return Ok(buffer[header..header + listed]
                    .iter()
                    .map(|pid| *pid as u32)
                    .collect());
            }
        }

        /// 中の 1 本を止める。開いたハンドルでまだこの Job に居ることを確かめてから撃つ
        /// （列挙の後に pid が別のプロセスへ再利用されていても撃たない）。止めたら true
        pub(super) fn stop_member(&self, pid: u32) -> bool {
            let Some(process) =
                Process::open(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, pid)
            else {
                return false;
            };
            if self.holds(&process) != Some(true) {
                return false;
            }
            // SAFETY: 有効なハンドル。この Job の中のプロセスだけを撃つ
            unsafe { TerminateProcess(process.0, EXIT_CODE) != 0 }
        }

        /// `KILL_ON_JOB_CLOSE` を外す（閉じても中身を止めない）。明示的な外出の許可はそのまま
        pub(super) fn clear_kill_on_close(&self) -> Result<(), String> {
            self.set_limits(JOB_OBJECT_LIMIT_BREAKAWAY_OK)
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: CreateJobObjectW が返した有効なハンドルを 1 回だけ閉じる
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// unix は Job を作らない（ペインは PTY の HUP、言語サーバは EOF と kill のまま = 振る舞いを変えない）
    #[test]
    fn windows以外ではjobを作らない() {
        if SUPPORTED {
            return;
        }
        assert!(Job::contain(std::process::id()).is_none());
    }

    /// 入れられない pid（0 = System Idle Process / 居ない pid）は `None` で返り、panic しない
    #[test]
    fn 入れられないpidはnoneで返る() {
        assert!(Job::contain(0).is_none());
        assert!(Job::contain(u32::MAX).is_none());
    }

    /// PE ヘッダを組む（`e_lfanew` = 0x80。`magic` は PE32 = 0x10B / PE32+ = 0x20B）
    fn pe_image(magic: u16, subsystem: u16) -> Vec<u8> {
        let mut image = vec![0u8; 0x200];
        image[0..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x80..0x84].copy_from_slice(b"PE\0\0");
        let optional = 0x80 + 4 + 20;
        image[optional..optional + 2].copy_from_slice(&magic.to_le_bytes());
        image[optional + 68..optional + 70].copy_from_slice(&subsystem.to_le_bytes());
        image
    }

    #[test]
    fn issue2454_peのサブシステムを読む() {
        assert_eq!(pe_subsystem(&pe_image(0x20B, 2)), Some(2), "PE32+ の GUI");
        assert_eq!(
            pe_subsystem(&pe_image(0x20B, 3)),
            Some(3),
            "PE32+ のコンソール"
        );
        assert_eq!(pe_subsystem(&pe_image(0x10B, 2)), Some(2), "PE32 の GUI");
        assert_eq!(
            pe_subsystem(&pe_image(0x10B, 3)),
            Some(3),
            "PE32 のコンソール"
        );
        assert_eq!(IMAGE_SUBSYSTEM_WINDOWS_GUI, 2, "winnt.h の値");
    }

    /// コンソールのホストは GUI のアプリと数えない（大小・区切りの違いも同じ）
    #[test]
    fn issue2454_コンソールのホストを見分ける() {
        for host in [
            r"C:\Windows\System32\conhost.exe",
            r"C:\WINDOWS\system32\CONHOST.EXE",
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal\OpenConsole.exe",
            "conhost.exe",
        ] {
            assert!(is_console_host(host), "{host} をホストと読まない");
        }
        for other in [
            r"C:\Windows\System32\notepad.exe",
            r"C:\Users\winuser\AppData\Local\Programs\Microsoft VS Code\Code.exe",
            r"C:\Tools\conhost-helper.exe",
            r"C:\Windows\System32\cmd.exe",
        ] {
            assert!(!is_console_host(other), "{other} をホストと読んだ");
        }
    }

    #[test]
    fn issue2454_peでない_足りない_壊れているヘッダはnone() {
        assert_eq!(pe_subsystem(b""), None);
        assert_eq!(pe_subsystem(b"#!/bin/sh\n"), None, "スクリプト");
        let mut not_pe = pe_image(0x20B, 2);
        not_pe[0x80..0x84].copy_from_slice(b"NE\0\0");
        assert_eq!(pe_subsystem(&not_pe), None, "PE の署名が無い");
        let mut bad_magic = pe_image(0x20B, 2);
        bad_magic[0x80 + 24] = 0x07;
        assert_eq!(
            pe_subsystem(&bad_magic),
            None,
            "オプションヘッダの Magic が違う"
        );
        let mut far = pe_image(0x20B, 2);
        far[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(pe_subsystem(&far), None, "e_lfanew が範囲外（溢れない）");
        let cut = pe_image(0x20B, 2);
        assert_eq!(
            pe_subsystem(&cut[..0x80 + 24 + 68]),
            None,
            "途中で切れている"
        );
    }

    /// 木を `(pid, 親, 起動時刻, GUI か)` で書いて振り分ける
    fn split(tree: &[(u32, u32, u64, bool)]) -> ReleasePlan {
        let members: Vec<u32> = tree.iter().map(|t| t.0).collect();
        let find = |pid: u32| tree.iter().find(|t| t.0 == pid);
        split_for_release(
            &members,
            |pid| find(pid).is_some_and(|t| t.3),
            |pid| find(pid).map(|t| t.1),
            |pid| find(pid).map(|t| t.2),
        )
    }

    /// **#2454 の差し戻しの規則**: シェル（コンソール）から起こした GUI アプリとその子孫は残し、
    /// シェルとコンソールの孫（MCP サーバ・cmd.exe 経由の node.exe）は止める
    #[test]
    fn issue2454_guiのアプリとその子孫は残しコンソールは止める() {
        // 1 = シェル（PTY 直下）/ 2 = claude（node）/ 3 = MCP サーバ /
        // 4 = VS Code（GUI）/ 5 = VS Code の統合ターミナルのシェル / 6 = その言語サーバ / 7 = notepad
        let plan = split(&[
            (1, 100, 10, false),
            (2, 1, 11, false),
            (3, 2, 12, false),
            (4, 1, 13, true),
            (5, 4, 14, false),
            (6, 5, 15, false),
            (7, 1, 16, true),
        ]);
        assert_eq!(plan.stop, vec![1, 2, 3]);
        assert_eq!(plan.keep, vec![4, 5, 6, 7]);
    }

    #[test]
    fn issue2454_guiが居なければ全部止める() {
        let plan = split(&[(1, 100, 10, false), (2, 1, 11, false), (3, 2, 12, false)]);
        assert_eq!(plan.stop, vec![1, 2, 3]);
        assert!(plan.keep.is_empty());
    }

    /// 親が終わって pid が別の GUI のプロセスに再利用された（起動時刻が子より後）なら、
    /// その GUI を親と読まない = コンソールの孤児を残す側へ倒さない
    #[test]
    fn issue2454_再利用されたpidのguiを親と読まない() {
        let plan = split(&[(5, 4, 20, false), (4, 1, 30, true)]);
        assert_eq!(plan.stop, vec![5], "後から起きた GUI を親と読んだ");
        assert_eq!(plan.keep, vec![4]);
        // 同じ秒に起きた本当の親子は信じる
        let plan = split(&[(5, 4, 20, false), (4, 1, 20, true)]);
        assert_eq!(plan.keep, vec![5, 4]);
    }

    /// 親が分からない（列挙の失敗）・Job の外・輪になった読みは、たどらずに止める側
    #[test]
    fn issue2454_親が分からない_外_輪は止める側() {
        let members = [1, 2, 3];
        let plan = split_for_release(&members, |pid| pid == 9, |_| None, |_| None);
        assert_eq!(plan.stop, vec![1, 2, 3], "親が分からない");
        let plan = split_for_release(&members, |pid| pid == 9, |_| Some(9), |_| None);
        assert_eq!(plan.stop, vec![1, 2, 3], "GUI の親が Job の外");
        let plan = split_for_release(
            &members,
            |_| false,
            |pid| Some(if pid == 3 { 1 } else { pid + 1 }),
            |_| None,
        );
        assert_eq!(plan.stop, vec![1, 2, 3], "輪になった親子の読み");
        let plan = split_for_release(&members, |_| false, Some, |_| None);
        assert_eq!(plan.stop, vec![1, 2, 3], "自分が親");
    }

    /// 木を `(pid, 親, 起動時刻)` で書いて `root` の子孫を選ぶ
    fn descendants(tree: &[(u32, u32, u64)], root: u32, via: &[u32]) -> Vec<u32> {
        let members: Vec<u32> = tree.iter().map(|t| t.0).collect();
        let find = |pid: u32| tree.iter().find(|t| t.0 == pid);
        descendants_in(
            &members,
            root,
            &via.iter().copied().collect(),
            |pid| find(pid).map(|t| t.1),
            |pid| find(pid).map(|t| t.2),
        )
    }

    /// **#2462 の再起動の規則**: ペインの Job の中から、止めたエージェントの子孫だけを選ぶ
    /// （シェルとシェルの別の子は選ばない）。GUI を残すのは続く [`split_for_release`] の役目
    #[test]
    fn issue2462_エージェントの子孫だけを選びシェルとほかの子は選ばない() {
        // 1 = シェル / 2 = エージェント / 3 = MCP の cmd.exe / 4 = その node.exe /
        // 5 = エージェントが起こした GUI / 6 = その子 / 7 = シェルの別の子
        let tree = [
            (1, 100, 10),
            (2, 1, 11),
            (3, 2, 12),
            (4, 3, 13),
            (5, 2, 14),
            (6, 5, 15),
            (7, 1, 16),
        ];
        assert_eq!(descendants(&tree, 2, &[]), vec![3, 4, 5, 6]);
        // 続く振り分けで GUI（5）とその子孫（6）は残り、コンソール（3 / 4）は止まる
        let targets = descendants(&tree, 2, &[]);
        let plan = split_for_release(
            &targets,
            |pid| pid == 5,
            |pid| tree.iter().find(|t| t.0 == pid).map(|t| t.1),
            |pid| tree.iter().find(|t| t.0 == pid).map(|t| t.2),
        );
        assert_eq!(plan.stop, vec![3, 4]);
        assert_eq!(plan.keep, vec![5, 6]);
    }

    /// エージェントを先に止めた後（Job の列挙に居ない）でも、その子孫は親の pid でたどれる
    #[test]
    fn issue2462_止めた後のエージェントの子孫もたどれる() {
        let tree = [(1, 100, 10), (3, 2, 12), (4, 3, 13), (7, 1, 16)];
        assert_eq!(descendants(&tree, 2, &[]), vec![3, 4]);
    }

    /// 前の回で止めた中継ぎ（列挙に出ない）を通る子孫も選ぶ。通してよいのは止めたものだけ
    #[test]
    fn issue2462_前の回で止めた中継ぎを通る子孫も選ぶ() {
        // 3 は前の回で止めた（列挙に居ない）。4 はその後に 3 が起こした子
        let members = [1, 4];
        let parent = |pid: u32| match pid {
            1 => Some(100),
            4 => Some(3),
            3 => Some(2),
            _ => None,
        };
        let via: HashSet<u32> = [3].into_iter().collect();
        assert_eq!(descendants_in(&members, 2, &via, parent, |_| None), vec![4]);
        assert!(
            descendants_in(&members, 2, &HashSet::new(), parent, |_| None).is_empty(),
            "止めていない Job の外の中継ぎを通った"
        );
    }

    /// 止めたエージェントの pid を後から起きた別のプロセスが取った（起動時刻が子より後）なら、
    /// その子をエージェントの子孫と読まない
    #[test]
    fn issue2462_再利用されたpidを親と読まない() {
        let tree = [(1, 100, 10), (2, 1, 50), (3, 2, 12)];
        assert!(
            descendants(&tree, 2, &[]).is_empty(),
            "後から起きた 2 を親と読んだ"
        );
        // 同じ秒に起きた本当の親子は信じる
        let tree = [(1, 100, 10), (2, 1, 12), (3, 2, 12)];
        assert_eq!(descendants(&tree, 2, &[]), vec![3]);
    }

    /// 子孫が居ない・親が分からない・輪・root 自身は選ばない（ペインのシェルを撃たない）
    #[test]
    fn issue2462_子孫が居ない_親が分からない_輪_root自身は選ばない() {
        assert!(descendants(&[(1, 100, 10), (7, 1, 16)], 2, &[]).is_empty());
        let members = [1, 2, 3];
        assert!(descendants_in(&members, 2, &HashSet::new(), |_| None, |_| None).is_empty());
        let cycle = |pid: u32| Some(if pid == 3 { 1 } else { pid + 1 });
        assert!(
            descendants_in(&members, 9, &HashSet::new(), cycle, |_| None).is_empty(),
            "輪になった親子の読みで止まらない"
        );
        assert_eq!(
            descendants(&[(2, 1, 11), (3, 2, 12)], 2, &[]),
            vec![3],
            "root 自身を選んだ"
        );
    }

    /// unix・入れられない pid は空の入れ物で、止めても何もしない（直下の子を kill する従来の形）
    #[test]
    fn issue2462_空の入れ物は止めても何もしない() {
        let mut empty = KillScope::empty();
        assert!(!empty.is_contained());
        assert_eq!(empty.stop(), Release::default());
        assert_eq!(empty.stop(), Release::default(), "2 回目");
        let mut bad = KillScope::contain(0);
        assert!(!bad.is_contained());
        assert_eq!(bad.stop(), Release::default());
        if !SUPPORTED {
            assert!(!KillScope::contain(std::process::id()).is_contained());
        }
    }
}
