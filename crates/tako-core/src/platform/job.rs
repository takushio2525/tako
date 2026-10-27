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
//! 以後その子が起こすものは Job を受け継ぐ（Windows の仕様）。止めるときは
//! `TerminateJobObject` で中身を全部止める（[`Job::terminate`] / drop）。Job は
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 付きなので、tako が落ちてハンドルが閉じたときも中身は止まる
//! （隔離インスタンスの終了も同じ）。
//!
//! - `JOB_OBJECT_LIMIT_BREAKAWAY_OK` を付ける: `CREATE_BREAKAWAY_FROM_JOB` で明示的に外へ出る子
//!   （親の寿命から切り離すと宣言した常駐 = unix の `setsid` に当たる）は出られる。付けないと
//!   その子の `CreateProcess` が ERROR_ACCESS_DENIED で失敗する（tako のペインの中でだけ壊れる）
//! - `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` は付けない（付けると全部の子が黙って外へ出て、回収にならない）
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
//! unix は何もしない（[`Job::contain`] が `None`）: ペインは PTY の HUP、言語サーバは stdin の EOF と
//! kill に任せる（従来どおり）。A/B は `TAKO_2454_LEGACY=1`（Job を作らない = #2454 前）。

/// この OS で子孫を Job で回収するか（Windows だけ真）
pub const SUPPORTED: bool = cfg!(windows);

/// A/B の旧経路（`TAKO_2454_LEGACY=1`）。Job を作らない = #2454 前の回収（直下の子だけ）
pub const ENV_LEGACY: &str = "TAKO_2454_LEGACY";

/// 注入（`TAKO_2454_INJECT=assign_fail`）。**テストプロセスでだけ**、子を Job へ入れる段を
/// 失敗させる = 割り当てに失敗した回（素の kill へ落ちる）を作る。製品のバイナリでは無視する
pub const ENV_INJECT: &str = "TAKO_2454_INJECT";

fn legacy() -> bool {
    std::env::var(ENV_LEGACY).is_ok_and(|v| v == "1")
}

fn injected_assign_failure() -> bool {
    crate::paths::is_test_process() && std::env::var(ENV_INJECT).is_ok_and(|v| v == "assign_fail")
}

/// 子孫の入れ物。drop で中身を全部止める
pub struct Job {
    inner: imp::Job,
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
            Ok(inner) => Some(Job { inner }),
            Err(reason) => {
                tracing::warn!("子孫の回収の入れ物を作れない（直下の子だけを止める）: {reason}");
                None
            }
        }
    }

    /// 中身を全部止める（冪等。空の Job・止め終わった Job に撃ってもよい）
    pub fn terminate(&self) -> Result<(), String> {
        self.inner.terminate()
    }

    /// `pid` がこの Job に入っているか。確かめられない（開けない）ときは `None`。
    /// 製品の判断には使わない（テストと診断用）
    pub fn contains(&self, pid: u32) -> Option<bool> {
        self.inner.contains(pid)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // ハンドルを閉じれば KILL_ON_JOB_CLOSE でも止まるが、ハンドルが他所に残っていても
        // 止まるよう明示的に止めてから閉じる（閉じるのは inner の drop）
        if let Err(reason) = self.inner.terminate() {
            tracing::warn!("子孫の回収の入れ物を止められない: {reason}");
        }
    }
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
    /// `JOBOBJECTINFOCLASS` の `JobObjectExtendedLimitInformation`
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    /// 止めたプロセスの終了コード。`platform::process::terminate` と同じ 1
    /// （`STILL_ACTIVE` でなければ生存判定が終了済みと読む。#2275）
    const EXIT_CODE: u32 = 1;

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
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn IsProcessInJob(process: Handle, job: Handle, result: *mut i32) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn CloseHandle(object: Handle) -> i32;
    }

    fn last_error() -> std::io::Error {
        std::io::Error::last_os_error()
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
            let mut info = ExtendedLimitInformation::default();
            info.basic.limit_flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            // SAFETY: info は winnt.h と同じ配置（大きさはコンパイル時に検査）で、呼び出しの間は生きている
            let set = unsafe {
                SetInformationJobObject(
                    job.handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&mut info as *mut ExtendedLimitInformation).cast::<c_void>(),
                    std::mem::size_of::<ExtendedLimitInformation>() as u32,
                )
            };
            if set == 0 {
                return Err(format!("SetInformationJobObject: {}", last_error()));
            }
            // SAFETY: 戻りのハンドルは null を検査し、使い終えたらすぐ閉じる
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if process.is_null() {
                return Err(format!("OpenProcess({pid}): {}", last_error()));
            }
            // SAFETY: どちらも上で開いた有効なハンドル
            let assigned = unsafe { AssignProcessToJobObject(job.handle, process) };
            let error = last_error();
            // SAFETY: 上で開いたハンドルを 1 回だけ閉じる
            unsafe { CloseHandle(process) };
            if assigned == 0 {
                return Err(format!("AssignProcessToJobObject({pid}): {error}"));
            }
            Ok(job)
        }

        pub(super) fn terminate(&self) -> Result<(), String> {
            // SAFETY: drop まで有効なハンドル。空の Job へ撃っても中身が無いだけ
            if unsafe { TerminateJobObject(self.handle, EXIT_CODE) } == 0 {
                return Err(format!("TerminateJobObject: {}", last_error()));
            }
            Ok(())
        }

        pub(super) fn contains(&self, pid: u32) -> Option<bool> {
            // SAFETY: 戻りのハンドルは null を検査し、使い終えたらすぐ閉じる
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if process.is_null() {
                return None;
            }
            let mut result = 0i32;
            // SAFETY: どちらも有効なハンドルで、result は呼び出しの間生きている
            let ok = unsafe { IsProcessInJob(process, self.handle, &mut result) };
            // SAFETY: 上で開いたハンドルを 1 回だけ閉じる
            unsafe { CloseHandle(process) };
            (ok != 0).then_some(result != 0)
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
}
