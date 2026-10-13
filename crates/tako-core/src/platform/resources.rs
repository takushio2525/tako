//! PC 全体と各プロセスの資源の計測（抽象境界 B29。FR-2.43 / Issue #1993。エピック #1992 の S1）
//!
//! ## 何を閉じ込めるか
//!
//! OS へ「機全体のメモリ・swap・メモリ圧・CPU・load」と「各プロセスの親・名前・
//! 物理フットプリント・CPU 時間・開始時刻」を問い合わせる部分。解釈（Activity Monitor と同じ
//! 内訳の式・差分・分類・帰属）は OS 分岐を持たない [`crate::resources`] の純関数が持つ。
//!
//! B25（`tako_control::platform::sysload`）は「診断専用・製品の判断に使わない」と決めてあるので
//! 流用せず、製品の判断（順番待ち = #1994）に使うこちらを別に置く（設計書 §2.1）。
//!
//! ## 子プロセスを起こさない
//!
//! `sysctl` / mach / libproc だけで採る（`ps` / `vm_stat` / `sysctl` コマンドは起こさない）。
//! 実測（2026-10-12・1,320 プロセス）で機全体 0.008 ms・全プロセスの走査 wall 11.0 ms / CPU 5.2 ms。
//! 待ちは生まれないが 11 ms は UI の 1 フレームを超えるので、**UI スレッドからは呼ばない**
//! （呼び手は `tako_control::resources` の背景スレッドと IPC の受け口だけ。番犬
//! `crates/tako-control/tests/issue1993_resources_ui_thread_watchdog.rs`）。
//!
//! ## 罠
//!
//! - `ri_user_time` / `ri_system_time` は Apple Silicon では mach tick。[`crate::resources::mach_to_ns`]
//!   で timebase を掛けてから返す（秒として読むと 41.7 分の 1）
//! - `mach_host_self()` は呼ぶたびに port の送信権を 1 つ増やすので、1 回だけ採って覚える
//! - `mach_host_self` / `mach_timebase_info` は libc 0.2.186 では非推奨の印が付いているので、
//!   必要な 2 つだけ自前で宣言する（`windows-sys` を入れない方針と同じ。大きさは assert で固定）
//!
//! ## Windows
//!
//! 未実装（#2000 = S8）。どの関数も `None` を返し、呼び手は「この OS では採れない」と表示する。

use std::collections::HashMap;

use crate::resources::{CpuTicks, VmPages};

/// 機全体の生の値（解釈前）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawMachine {
    /// 物理メモリの全体（バイト）
    pub total: u64,
    pub vm: Option<VmPages>,
    /// swap の (使用, 全体) バイト
    pub swap: Option<(u64, u64)>,
    /// `kern.memorystatus_vm_pressure_level`（1 / 2 / 4）
    pub pressure_level: Option<i32>,
    pub cpu: Option<CpuTicks>,
    pub load: Option<[f64; 3]>,
    pub cpus: u32,
}

/// プロセス 1 本の生の値
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawProc {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    /// 実行ファイル名（16 文字で切れる）
    pub name: String,
    /// 開始時刻（OS の単調時計の値。pid の再利用を見分けるためだけに使う）。読めなければ 0
    pub start: u64,
    /// 物理フットプリント（バイト）。別ユーザーのプロセスは読めないので `None`
    pub footprint: Option<u64>,
    /// 累積 CPU 時間（ナノ秒。timebase で換算済み）。読めなければ `None`
    pub cpu_ns: Option<u64>,
}

/// argv と env の指定キー
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub argv: Option<String>,
    pub env: HashMap<String, String>,
}

/// 機全体を採る。採れない OS では `None`
pub fn machine() -> Option<RawMachine> {
    imp::machine()
}

/// 全プロセスを走査する。採れない OS では `None`
pub fn processes() -> Option<Vec<RawProc>> {
    imp::processes()
}

/// この プロセスの実ユーザー ID（自分のプロセスかを見分ける）。採れない OS では `None`
pub fn own_uid() -> Option<u32> {
    imp::own_uid()
}

/// argv と env を読む口。読み取りバッファ（`kern.argmax` ぶん）を使い回すために持つ
pub struct DetailsReader {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    buf: Vec<u8>,
}

impl DetailsReader {
    /// 読めない OS では `None`
    pub fn new() -> Option<Self> {
        imp::details_buffer().map(|buf| Self { buf })
    }

    /// `pid` の argv と、env のうち `names` のキーだけを読む。読めなければ `None`
    pub fn read(&mut self, pid: u32, names: &[&str]) -> Option<Details> {
        imp::details(&mut self.buf, pid, names)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::OnceLock;

    use super::{Details, RawMachine, RawProc};
    use crate::resources::{mach_to_ns, CpuTicks, VmPages};

    /// `struct mach_timebase_info`（`mach/mach_time.h`）
    #[repr(C)]
    #[derive(Default)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    const _: () = assert!(size_of::<MachTimebaseInfo>() == 8);

    /// `host_cpu_load_info_data_t`（`mach/host_info.h`）: user / system / idle / nice の tick
    #[repr(C)]
    #[derive(Default)]
    struct HostCpuLoadInfo {
        cpu_ticks: [u32; 4],
    }

    const _: () = assert!(size_of::<HostCpuLoadInfo>() == 16);

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
        fn mach_host_self() -> u32;
    }

    fn host() -> u32 {
        static HOST: OnceLock<u32> = OnceLock::new();
        // SAFETY: 引数の無い関数。返る port は覚えて使い回す（呼ぶたびに送信権が増えるため）
        *HOST.get_or_init(|| unsafe { mach_host_self() })
    }

    /// (numer, denom)。採れなければ 1/1
    pub(super) fn timebase() -> (u32, u32) {
        static TB: OnceLock<(u32, u32)> = OnceLock::new();
        *TB.get_or_init(|| {
            let mut info = MachTimebaseInfo::default();
            // SAFETY: 呼び出し側が所有する 8 バイトへ書かせるだけ
            let ok = unsafe { mach_timebase_info(&mut info) };
            if ok == 0 && info.denom != 0 {
                (info.numer, info.denom)
            } else {
                (1, 1)
            }
        })
    }

    /// `sysctlbyname` で固定長の値を 1 つ読む
    fn sysctl_value<T: Copy>(name: &str) -> Option<T> {
        let cname = std::ffi::CString::new(name).ok()?;
        let mut value = std::mem::MaybeUninit::<T>::zeroed();
        let mut len = size_of::<T>();
        // SAFETY: 長さ `len` の領域へ書かせ、書かれた長さが型の大きさと一致したときだけ読む
        let ok = unsafe {
            libc::sysctlbyname(
                cname.as_ptr(),
                value.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        // SAFETY: 成功して長さが一致した = 値は初期化済み（全 0 初期化の上に書かれている）
        (ok == 0 && len == size_of::<T>()).then(|| unsafe { value.assume_init() })
    }

    fn vm_pages() -> Option<VmPages> {
        // SAFETY: POD の構造体なので全 0 は正しい値
        let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
        let mut count = libc::HOST_VM_INFO64_COUNT;
        // SAFETY: `count` 個の integer_t ぶんの領域（= 構造体）へ書かせる
        let kr = unsafe {
            libc::host_statistics64(
                host(),
                libc::HOST_VM_INFO64,
                (&mut stats as *mut libc::vm_statistics64).cast(),
                &mut count,
            )
        };
        if kr != 0 {
            return None;
        }
        // SAFETY: 引数の無い問い合わせ
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0 {
            return None;
        }
        Some(VmPages {
            page_size: page as u64,
            free: u64::from(stats.free_count),
            active: u64::from(stats.active_count),
            inactive: u64::from(stats.inactive_count),
            wired: u64::from(stats.wire_count),
            purgeable: u64::from(stats.purgeable_count),
            speculative: u64::from(stats.speculative_count),
            compressor: u64::from(stats.compressor_page_count),
            external: u64::from(stats.external_page_count),
            internal: u64::from(stats.internal_page_count),
        })
    }

    fn cpu_ticks() -> Option<CpuTicks> {
        let mut info = HostCpuLoadInfo::default();
        let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
        // SAFETY: 4 個の natural_t ぶんの領域へ書かせる
        let kr = unsafe {
            libc::host_statistics(
                host(),
                libc::HOST_CPU_LOAD_INFO,
                (&mut info as *mut HostCpuLoadInfo).cast(),
                &mut count,
            )
        };
        (kr == 0).then(|| CpuTicks {
            user: u64::from(info.cpu_ticks[libc::CPU_STATE_USER as usize]),
            system: u64::from(info.cpu_ticks[libc::CPU_STATE_SYSTEM as usize]),
            idle: u64::from(info.cpu_ticks[libc::CPU_STATE_IDLE as usize]),
            nice: u64::from(info.cpu_ticks[libc::CPU_STATE_NICE as usize]),
        })
    }

    pub(super) fn machine() -> Option<RawMachine> {
        let total = sysctl_value::<u64>("hw.memsize")?;
        let swap =
            sysctl_value::<libc::xsw_usage>("vm.swapusage").map(|s| (s.xsu_used, s.xsu_total));
        let pressure_level = sysctl_value::<libc::c_int>("kern.memorystatus_vm_pressure_level");
        let mut load = [0f64; 3];
        // SAFETY: 3 要素の配列へ書かせる
        let filled = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
        Some(RawMachine {
            total,
            vm: vm_pages(),
            swap,
            pressure_level,
            cpu: cpu_ticks(),
            load: (filled == 3).then_some(load),
            cpus: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(0),
        })
    }

    pub(super) fn processes() -> Option<Vec<RawProc>> {
        let pids = crate::ports::list_pids();
        if pids.is_empty() {
            return None;
        }
        let (numer, denom) = timebase();
        // SAFETY: 引数の無い問い合わせ
        let me = unsafe { libc::getuid() };
        let mut out = Vec::with_capacity(pids.len());
        for pid in pids {
            let Some((ppid, uid, name)) = crate::ports::short_info(pid) else {
                continue; // 列挙と問い合わせの間に終わった
            };
            // SAFETY: POD の構造体なので全 0 は正しい値
            let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
            // 別ユーザー（root 等）のプロセスの rusage は必ず断られるので問い合わせない
            // （実測で全体の約 1/4。数えるのは自分のプロセスだけ = 設計書 §1.3）
            let ok = uid == me
                // SAFETY: rusage_info_v4 ぶんの領域へ書かせる（flavor と構造体の版が一致）
                && unsafe {
                    libc::proc_pid_rusage(
                        pid,
                        libc::RUSAGE_INFO_V4,
                        (&mut ri as *mut libc::rusage_info_v4).cast(),
                    )
                } == 0;
            out.push(RawProc {
                pid: pid as u32,
                ppid,
                uid,
                name,
                start: if ok { ri.ri_proc_start_abstime } else { 0 },
                footprint: ok.then_some(ri.ri_phys_footprint),
                cpu_ns: ok.then(|| {
                    mach_to_ns(
                        ri.ri_user_time.saturating_add(ri.ri_system_time),
                        numer,
                        denom,
                    )
                }),
            });
        }
        Some(out)
    }

    pub(super) fn own_uid() -> Option<u32> {
        // SAFETY: 引数の無い問い合わせ
        Some(unsafe { libc::getuid() })
    }

    pub(super) fn details_buffer() -> Option<Vec<u8>> {
        crate::ports::procargs_buffer_len().map(|len| vec![0u8; len])
    }

    pub(super) fn details(buf: &mut [u8], pid: u32, names: &[&str]) -> Option<Details> {
        let (argv, env) = crate::ports::argv_and_env(pid, buf, names)?;
        Some(Details { argv, env })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Details, RawMachine, RawProc};

    pub(super) fn machine() -> Option<RawMachine> {
        None
    }

    pub(super) fn processes() -> Option<Vec<RawProc>> {
        None
    }

    pub(super) fn own_uid() -> Option<u32> {
        None
    }

    pub(super) fn details_buffer() -> Option<Vec<u8>> {
        None
    }

    pub(super) fn details(_buf: &mut [u8], _pid: u32, _names: &[&str]) -> Option<Details> {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// getrusage(RUSAGE_SELF) の user + system（ナノ秒）
    fn self_cpu_ns() -> u64 {
        // SAFETY: POD の構造体なので全 0 は正しい値
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: 呼び出し側が所有する構造体へ書かせる
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1_000;
        tv(ru.ru_utime) + tv(ru.ru_stime)
    }

    #[test]
    fn 機全体の値が採れて辻褄が合う() {
        let m = machine().expect("macOS では採れる");
        assert!(m.total > 0);
        let vm = m.vm.expect("host_statistics64");
        assert!(vm.page_size == 4096 || vm.page_size == 16384, "{vm:?}");
        let b = crate::resources::memory_breakdown(m.total, &vm);
        assert!(
            b.used <= m.total * 2,
            "使用が全体の 2 倍を超えるのは式の誤り: {b:?}"
        );
        assert!(
            matches!(m.pressure_level, Some(1 | 2 | 4)),
            "{:?}",
            m.pressure_level
        );
        assert!(m.cpu.is_some());
        assert!(m.cpus > 0);
    }

    #[test]
    fn 自分のcpu時間はtimebaseで換算されgetrusageと一致する() {
        // 換算を忘れると Apple Silicon では 41.7 分の 1 になる。差が見えるだけの CPU 時間を
        // 先に使う（1 コアで 0.1 秒ほど。負荷を作るためではなく、比べる量を作るため）
        // 壁時計ではなく CPU 時間で数える（混んだ機では 150 ms 回しても CPU 時間が溜まらない）
        let mut acc = 0u64;
        let base = self_cpu_ns();
        while self_cpu_ns() < base + 120_000_000 {
            for i in 0..10_000u64 {
                acc = acc.wrapping_mul(6364136223846793005).wrapping_add(i);
            }
        }
        std::hint::black_box(acc);
        let before = self_cpu_ns();
        let me = std::process::id();
        let mine = processes()
            .expect("走査できる")
            .into_iter()
            .find(|p| p.pid == me)
            .expect("自分が一覧にいる");
        let after = self_cpu_ns();
        let cpu = mine.cpu_ns.expect("自分の rusage は読める");
        assert!(before >= 100_000_000, "比べる量が足りない: {before}");
        let tol = 20_000_000 + after / 5;
        assert!(
            cpu + tol >= before && cpu <= after + tol,
            "proc_pid_rusage の CPU 時間 {cpu} ns が getrusage の {before}〜{after} ns と合わない\
             （timebase の換算漏れ？）"
        );
        assert!(mine.footprint.is_some_and(|f| f > 0));
        // SAFETY: 引数の無い問い合わせ
        assert_eq!(mine.ppid, unsafe { libc::getppid() } as u32);
        assert_eq!(Some(mine.uid), own_uid());
        assert!(mine.start > 0, "開始時刻（pid の再利用を見分ける鍵）");
    }

    #[test]
    fn 自分のargvとenvを読める() {
        let mut reader = DetailsReader::new().expect("macOS では読める");
        let d = reader
            .read(std::process::id(), &["PATH", "TAKO_NO_SUCH_VAR_1993"])
            .expect("自分のプロセスは読める");
        let argv = d.argv.expect("argv");
        let exe = std::env::args().next().unwrap();
        assert!(argv.starts_with(&exe), "{argv} は {exe} で始まる");
        assert!(!d.env.contains_key("TAKO_NO_SUCH_VAR_1993"));
    }
}
