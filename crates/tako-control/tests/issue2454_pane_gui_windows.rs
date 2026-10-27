//! ペインを閉じたとき、ペインから起こした GUI アプリは残り、同じペインのコンソールの孫は止まる
//! ことの実測（#2454 の差し戻し）。**実際に ConPTY を起動して測る**。
//!
//! 寿命の決まり（`tako_core::platform::job`）: ペインを閉じるとコンソールのプロセスは止めるが、
//! GUI のアプリとその子孫は残す（`code .`・`notepad` をペインと一緒に消さない = Windows Terminal と
//! macOS の `open` と同じ）。言語サーバと tako が落ちたときは全部止める。
//!
//! ## 測り方
//!
//! ペインの中の PowerShell が、GUI サブシステムの孫（テスト専用の `tako-gui-sleeper`）と、
//! コンソールの孫（`ping -n 600` を `CREATE_NO_WINDOW`）を .NET の `Process.Start` で起こし、
//! 2 つの pid をファイルへ書いて居座る。テストは孫を**ハンドルを持ったまま**見張る
//! （止まった後に pid が別のプロセスへ再利用されて読み違えるのを防ぐ = #2284 の形）。
//!
//! - 既定: ペインを閉じると、コンソールの孫とシェルは止まり、GUI の孫は生きている
//! - `TAKO_2454_KILL_GUI=1`（A/B = 差し戻し前の #2454 = ペインを閉じると全部止める）: GUI の孫も止まる
//!   = この測り方に検出力がある
//!
//! 「生きている」は、閉じる処理（`TerminalSession` の drop の中で同期で済む）が終わり、
//! コンソールの孫が止まったのを見てから、止められていないことを [`CONFIRM_ALIVE`] の間見届ける
//! （止められていれば kill から間を置かずに終わる）。env を立てる旧アームは `tako_core::run_alone!` の
//! 子で走る（#2400）。後始末で止めるのは、このテストが起こした孫だけ（ハンドルで撃つ）。
//!
//! Windows 以外ではコンパイルしない。

#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tako_core::platform::job;
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

const GUI: &str = env!("CARGO_BIN_EXE_tako-gui-sleeper");

/// ペインの中の PowerShell へ渡す env（ペインの env として渡す）
const OUT_ENV: &str = "TAKO_2454_OUT";
const GUI_ENV: &str = "TAKO_2454_GUI";

/// 起動を待つ上限（ConPTY + PowerShell の起動。状態待ちの上限）
const START_LIMIT: Duration = Duration::from_secs(120);

/// 止まるのを待つ上限（状態待ちの上限。比較には使わない）
const GONE_LIMIT: Duration = Duration::from_secs(60);

/// 止められていないことを見届ける窓（止められたプロセスは kill から間を置かずに終わる）
const CONFIRM_ALIVE: Duration = Duration::from_secs(3);

/// GUI の孫とコンソールの孫を起こし、「GUI の pid コンソールの pid」を書いて居座る
const SCRIPT: &str = "$g = New-Object System.Diagnostics.ProcessStartInfo; \
$g.FileName = $env:TAKO_2454_GUI; $g.UseShellExecute = $false; \
$gp = [System.Diagnostics.Process]::Start($g); \
$c = New-Object System.Diagnostics.ProcessStartInfo; \
$c.FileName = 'ping.exe'; $c.Arguments = '-n 600 127.0.0.1'; \
$c.UseShellExecute = $false; $c.CreateNoWindow = $true; \
$cp = [System.Diagnostics.Process]::Start($c); \
[IO.File]::WriteAllText($env:TAKO_2454_OUT, [string]$gp.Id + ' ' + [string]$cp.Id); \
Start-Sleep -Seconds 600";

type Handle = *mut c_void;

/// `SYNCHRONIZE` / `PROCESS_TERMINATE` / `PROCESS_QUERY_LIMITED_INFORMATION`（winnt.h）
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
    fn CloseHandle(object: Handle) -> i32;
}

/// 見張るプロセス（ハンドルを持つ間は pid が再利用されない）。drop で生きていれば止める
/// （このテストが起こした孫にだけ使う）
struct Held {
    pid: u32,
    handle: Handle,
}

impl Held {
    fn open(pid: u32) -> Held {
        // SAFETY: 戻りは null を検査し、drop で 1 回だけ閉じる
        let handle = unsafe {
            OpenProcess(
                SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        assert!(
            !handle.is_null(),
            "孫 {pid} を開けない（{}）",
            std::io::Error::last_os_error()
        );
        Held { pid, handle }
    }

    /// 終わるまで最大 `limit` 待つ。終わったら true（状態待ち）
    fn wait_gone(&self, limit: Duration) -> bool {
        let ms = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: drop まで有効なハンドル
        unsafe { WaitForSingleObject(self.handle, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: open が返した有効なハンドル。撃つのは自分が起こした孫だけ（ハンドルで撃つので
        // pid の再利用で別のプロセスへ届くことはない）
        unsafe {
            if WaitForSingleObject(self.handle, 0) != WAIT_OBJECT_0 {
                TerminateProcess(self.handle, 1);
            }
            CloseHandle(self.handle);
        }
    }
}

/// 使い捨ての置き場（テストごと・プロセスごとに別名）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-2454-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作れる");
        Scratch(dir)
    }

    fn out(&self) -> PathBuf {
        self.0.join("grandchildren.pid")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// GUI の孫とコンソールの孫を起こして居座るペイン
struct Pane {
    session: TerminalSession,
    _rx: futures::channel::mpsc::UnboundedReceiver<tako_core::terminal::SessionEvent>,
    shell: u32,
    gui: Held,
    console: Held,
}

fn powershell() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    PathBuf::from(root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

fn spawn_pane(scratch: &Scratch) -> Pane {
    let (session, rx) = TerminalSession::spawn(
        120,
        40,
        SpawnOptions {
            command: Some(SpawnCommand {
                program: powershell().display().to_string(),
                args: vec![
                    "-NoLogo".into(),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    SCRIPT.into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            // 書き出し先と GUI の実行ファイルはペインの env として渡す（このプロセスの env は書き換えない）
            env: vec![
                (OUT_ENV.to_string(), scratch.out().display().to_string()),
                (GUI_ENV.to_string(), GUI.to_string()),
            ],
            scrollback_lines: None,
        },
    )
    .expect("ConPTY を起動できること");
    let shell = session.child_pid().expect("PTY 直下の子の pid");
    let deadline = Instant::now() + START_LIMIT;
    let (gui, console) = loop {
        let pids: Vec<u32> = std::fs::read_to_string(scratch.out())
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if let [gui, console] = pids[..] {
            break (gui, console);
        }
        assert!(
            Instant::now() < deadline,
            "ペインの PowerShell が {START_LIMIT:?} で孫の pid を書き出さない（シェル {shell}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let gui = Held::open(gui);
    let console = Held::open(console);
    assert!(
        !gui.wait_gone(Duration::ZERO),
        "GUI の孫 {} が既に居ない",
        gui.pid
    );
    assert!(
        !console.wait_gone(Duration::ZERO),
        "コンソールの孫 {} が既に居ない",
        console.pid
    );
    let job = session
        .descendant_job()
        .expect("器なしのペインは子孫を Job に入れる（#2454）");
    for (what, pid) in [("GUI の孫", gui.pid), ("コンソールの孫", console.pid)] {
        assert_eq!(
            job.contains(pid),
            Some(true),
            "{what} {pid} が Job に入っていない（測りたい形でない）"
        );
    }
    Pane {
        session,
        _rx: rx,
        shell,
        gui,
        console,
    }
}

/// **差し戻しの受け入れ条件**: ペインを閉じると、同じペインのコンソールの孫は止まり、
/// GUI サブシステムの孫は生きている
#[test]
fn issue2454_ペインを閉じるとguiの孫は残りコンソールの孫は止まる() {
    let scratch = Scratch::new("pane-gui");
    let Pane {
        session,
        _rx,
        shell,
        gui,
        console,
    } = spawn_pane(&scratch);
    drop(session);
    assert!(
        console.wait_gone(GONE_LIMIT),
        "ペインを閉じたのにコンソールの孫 {} が {GONE_LIMIT:?} 残った（#2454）",
        console.pid
    );
    assert!(
        tako_core::platform::process::wait_gone(shell, GONE_LIMIT),
        "ペインを閉じたのにシェル {shell} が残った"
    );
    assert!(
        !gui.wait_gone(CONFIRM_ALIVE),
        "ペインを閉じたら GUI の孫 {} まで止まった（GUI のアプリは残す）",
        gui.pid
    );
    println!(
        "TAKO2454_PANE_GUI shell={shell} gui={} gui_alive=true console={} console_gone=true",
        gui.pid, console.pid
    );
    // GUI の孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// **A/B（差し戻し前の #2454 = ペインを閉じると全部止める）**: GUI の孫も止まる。
/// 既定のテスト（上）がこの回を「GUI まで止まった」として落とせる = 測り方に検出力がある
#[test]
fn issue2454_全部止める旧アームではguiの孫も止まる() {
    if tako_core::run_alone!(issue2454_全部止める旧アームではguiの孫も止まる) {
        return;
    }
    std::env::set_var(job::ENV_KILL_GUI, "1"); // 子の中だけ（他のテストは居ない）
    let scratch = Scratch::new("pane-gui-killall");
    let Pane {
        session,
        _rx,
        shell,
        gui,
        console,
    } = spawn_pane(&scratch);
    drop(session);
    assert!(
        console.wait_gone(GONE_LIMIT),
        "旧アームでコンソールの孫 {} が残った",
        console.pid
    );
    assert!(
        gui.wait_gone(GONE_LIMIT),
        "旧アームで GUI の孫 {} が止まらない（この測り方では差が出ない）",
        gui.pid
    );
    println!(
        "TAKO2454_PANE_GUI_KILLALL shell={shell} gui={} gui_gone=true console={} console_gone=true",
        gui.pid, console.pid
    );
}
