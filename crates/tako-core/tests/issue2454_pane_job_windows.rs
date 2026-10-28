//! ペインを閉じたとき、シェルの孫まで止まることの実測（#2454）。**実際に ConPTY を起動して測る**。
//!
//! Windows の `TerminateProcess` は子孫を道連れにしない。ConPTY を閉じれば疑似コンソールに
//! 繋がった子は止まるが、`CREATE_NO_WINDOW` で起きた子（stdio をパイプにした MCP サーバ・
//! cmd.exe 経由の言語サーバと同じ形）はコンソールに繋がっていないので、#2454 まではペインを
//! 閉じても残った（FR-2.33.9 の「1 日 46 本」）。
//!
//! ## 測り方
//!
//! ペインの中の PowerShell が、孫（`ping -n 600`）を .NET の `Process.Start`
//! （`UseShellExecute = false` / `CreateNoWindow = true` = `CREATE_NO_WINDOW`）で起こし、その pid を
//! ファイルへ書いて居座る。テストは孫のプロセスを**ハンドルを持ったまま**見張り（閉じた後に
//! pid が別のプロセスへ再利用されて「生きている」と読むのを防ぐ = #2284 の形）、
//!
//! - 既定: 孫が Job に入っていること（起動直後の窓をすり抜けていない）を確かめてからペインを閉じ、
//!   孫が止まる
//! - `TAKO_2454_LEGACY=1`（A/B の旧アーム = #2454 前）: ペインを閉じてシェルが止まった後も孫が
//!   生きている = この測り方に検出力がある（修正前の実装では既定のテストが落ちる）
//! - `TAKO_2454_INJECT=assign_fail`（Job へ入れる段の失敗）: ペインは起動し、閉じても panic しない
//!   （素の止め方 = 旧アームと同じ回収へ落ちる）
//!
//! env を立てる 2 本は `tako_core::run_alone!` の子で走る（#2400。親のプロセスの env は書き換えない）。
//! 後始末で止めるのは、このテストが起こした孫だけ（ハンドルで撃つので別のプロセスへは届かない）。
//!
//! Windows 以外ではコンパイルしない（unix のペインは PTY の HUP に任せる = 振る舞いを変えない）。

#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tako_core::platform::job;
use tako_core::platform::process::pid_alive;
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

/// ペインの中の PowerShell へ孫の pid の書き出し先を渡す（ペインの env として渡す）
const OUT_ENV: &str = "TAKO_2454_OUT";

/// 起動を待つ上限（ConPTY + PowerShell の起動。混んだ CI でも足りる幅の状態待ちの上限）
const START_LIMIT: Duration = Duration::from_secs(120);

/// 止まるのを待つ上限（状態待ちの上限。比較には使わない）
const GONE_LIMIT: Duration = Duration::from_secs(60);

/// 孫を `CREATE_NO_WINDOW` で起こし、pid を書いて居座る（10 分。テストが閉じるまでの間）
const SCRIPT: &str = "$i = New-Object System.Diagnostics.ProcessStartInfo; \
$i.FileName = 'ping.exe'; $i.Arguments = '-n 600 127.0.0.1'; \
$i.UseShellExecute = $false; $i.CreateNoWindow = $true; \
$p = [System.Diagnostics.Process]::Start($i); \
[IO.File]::WriteAllText($env:TAKO_2454_OUT, [string]$p.Id); \
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

    /// 待たずに 1 回だけ問う
    fn alive(&self) -> bool {
        !self.wait_gone(Duration::ZERO)
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
        self.0.join("grandchild.pid")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 孫を起こして居座るペイン
struct Pane {
    session: TerminalSession,
    _rx: futures::channel::mpsc::UnboundedReceiver<tako_core::terminal::SessionEvent>,
    shell: u32,
    grandchild: Held,
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
            // 書き出し先はペインの env として渡す（このプロセスの env は書き換えない）
            env: vec![(OUT_ENV.to_string(), scratch.out().display().to_string())],
            scrollback_lines: None,
        },
    )
    .expect("ConPTY を起動できること");
    let shell = session.child_pid().expect("PTY 直下の子の pid");
    let deadline = Instant::now() + START_LIMIT;
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(scratch.out())
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "ペインの PowerShell が {START_LIMIT:?} で孫の pid を書き出さない（シェル {shell}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let grandchild = Held::open(pid);
    assert!(grandchild.alive(), "書き出された孫 {pid} が既に居ない");
    Pane {
        session,
        _rx: rx,
        shell,
        grandchild,
    }
}

/// **受け入れ条件 1**: ペインを閉じると、`CREATE_NO_WINDOW` で起きた孫まで止まる。
/// 閉じる前に、シェルと孫が Job に入っていること（起動直後の窓をすり抜けていない）を確かめる
#[test]
fn issue2454_ペインを閉じるとコンソールに繋がらない孫まで止まる() {
    let scratch = Scratch::new("close");
    let pane = spawn_pane(&scratch);
    let job = pane
        .session
        .descendant_job()
        .expect("器なしのペインは子孫を Job に入れる（#2454）");
    assert_eq!(
        job.contains(pane.shell),
        Some(true),
        "PTY 直下のシェル {} が Job に入っていない",
        pane.shell
    );
    assert_eq!(
        job.contains(pane.grandchild.pid),
        Some(true),
        "孫 {} が Job に入っていない（起動直後の窓をすり抜けた）",
        pane.grandchild.pid
    );
    let Pane {
        session,
        _rx,
        shell,
        grandchild,
    } = pane;
    drop(session);
    assert!(
        grandchild.wait_gone(GONE_LIMIT),
        "ペインを閉じたのに孫 {} が {GONE_LIMIT:?} 残った（#2454）",
        grandchild.pid
    );
    // シェルは alacritty がハンドルを持ち続ける（pid は再利用されない）ので境界の生存判定で見る
    assert!(
        tako_core::platform::process::wait_gone(shell, GONE_LIMIT),
        "ペインを閉じたのにシェル {shell} が残った"
    );
    println!(
        "TAKO2454_PANE_CLOSE shell={shell} grandchild={} gone=true",
        grandchild.pid
    );
}

/// **A/B の旧アーム（= 修正前の実装）**: ペインを閉じてシェルが止まった後も、孫は生きている。
/// 既定のテスト（上）がこの回を「残った」として落とせる = 測り方に検出力がある
#[test]
fn issue2454_旧アームではペインを閉じても孫が残る() {
    if tako_core::run_alone!(issue2454_旧アームではペインを閉じても孫が残る) {
        return;
    }
    std::env::set_var(job::ENV_LEGACY, "1"); // 子の中だけ（他のテストは居ない）
    let scratch = Scratch::new("legacy");
    let pane = spawn_pane(&scratch);
    assert!(
        pane.session.descendant_job().is_none(),
        "旧アームで Job を作った"
    );
    let Pane {
        session,
        _rx,
        shell,
        grandchild,
    } = pane;
    drop(session);
    // シェルが止まった（= ペインを閉じる処理が済んだ）のを待ってから孫を見る
    assert!(
        tako_core::platform::process::wait_gone(shell, GONE_LIMIT),
        "旧アームでもシェル {shell} は ConPTY を閉じれば止まるはず"
    );
    assert!(
        grandchild.alive(),
        "旧アームで孫 {} が止まった（この測り方では #2454 の差が出ない）",
        grandchild.pid
    );
    println!(
        "TAKO2454_PANE_LEGACY shell={shell} grandchild={} survived=true",
        grandchild.pid
    );
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// **割り当てに失敗した回**: Job が無いままペインは起動し、閉じても panic しない
/// （素の止め方 = 旧アームと同じ回収へ落ちる）
#[test]
fn issue2454_jobへ入れられなくてもペインは起動し閉じられる() {
    if tako_core::run_alone!(issue2454_jobへ入れられなくてもペインは起動し閉じられる)
    {
        return;
    }
    std::env::set_var(job::ENV_INJECT, "assign_fail"); // 子の中だけ（他のテストは居ない）
    let scratch = Scratch::new("assign-fail");
    let pane = spawn_pane(&scratch);
    assert!(
        pane.session.descendant_job().is_none(),
        "入れられなかったのに Job を持った"
    );
    let Pane {
        session,
        _rx,
        shell,
        grandchild,
    } = pane;
    drop(session);
    assert!(
        tako_core::platform::process::wait_gone(shell, GONE_LIMIT),
        "Job が無い回もシェル {shell} は ConPTY を閉じれば止まる"
    );
    drop(grandchild);
}

/// **境界の端**: 中身が先に死んだ Job を止める・同じ Job を 2 回止める（同じペインを 2 回閉じる形）・
/// 空になった Job をペインの閉じ方（GUI を残す）で手放す、のどれも成功し panic しない
#[test]
fn issue2454_中身が先に死んだjobを止めても2回止めても成功する() {
    let mut child = std::process::Command::new("ping");
    child
        .args(["-n", "600", "127.0.0.1"])
        .stdout(std::process::Stdio::null());
    let mut child = tako_core::platform::process::no_console_window(&mut child)
        .spawn()
        .expect("眠るだけの子を起こせる");
    let job = job::Job::contain(child.id()).expect("自分の子を Job に入れられる");
    assert_eq!(job.contains(child.id()), Some(true));
    child.kill().expect("止められる");
    child.wait().expect("看取れる");
    assert_eq!(job.terminate(), Ok(()), "中身が死んだ Job を止められない");
    assert_eq!(job.terminate(), Ok(()), "同じ Job の 2 回目を止められない");
    assert_eq!(
        job.release_keeping_gui(),
        job::Release::default(),
        "空の Job を手放すと何も止めず何も残さない"
    );
    assert!(!pid_alive(child.id()));
}

/// ペインの閉じ方（GUI を残す）でも、コンソールの子だけの Job は全部止まる（GUI が居ない速い道）
#[test]
fn issue2454_コンソールだけのjobはペインの閉じ方でも全部止まる() {
    let mut child = std::process::Command::new("ping");
    child
        .args(["-n", "600", "127.0.0.1"])
        .stdout(std::process::Stdio::null());
    let mut child = tako_core::platform::process::no_console_window(&mut child)
        .spawn()
        .expect("眠るだけの子を起こせる");
    let job = job::Job::contain(child.id()).expect("自分の子を Job に入れられる");
    let release = job.release_keeping_gui();
    // 止めた数は子とそのコンソールのホスト（conhost。Windows が付ける）で 1〜2 本
    assert!(
        release.kept == 0 && release.stopped >= 1,
        "コンソールの子を止めなかった・GUI と読んだ（{release:?}）"
    );
    // 自分の子なので看取れる（Job が止めた = 終了コード 1）
    let status = child.wait().expect("看取れる");
    assert_eq!(status.code(), Some(1), "Job が止めていない（{status:?}）");
}
