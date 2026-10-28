//! probe の打ち切りで、cmd.exe 経由で起きた孫まで止まることの実測（#2462）。
//! **実際に `cmd /C "… ping -n 600 …"` を起こして測る**。
//!
//! Windows の `TerminateProcess` は子孫を道連れにしないので、#2462 まで probe の打ち切り
//! （`probe::wait_with_timeout` の 1 実装 = `output_with_timeout` / `command_output_with_timeout` /
//! `claude agents --json` の走査が通る）は cmd.exe だけを止め、`ping` が残った（`probe::tests` の
//! Windows 版の `hang_script` と同じ形。`.cmd` の CLI = `claude.cmd` → cmd.exe → node.exe も同じ）。
//!
//! ## 測り方
//!
//! `probe::tests::hang_script` と同じ形の子（`cmd /C "echo partial& ping -n 600 127.0.0.1 > NUL"`）を
//! 起こし、起こした直後に待ちの 1 実装へ渡す（本番の `output_with_timeout` と同じ順番 = 待ちの入口で
//! 入れ物へ入る）。見張り役のスレッドがプロセスの一覧から cmd.exe の子の `ping` を見つけ、
//! **ハンドルを持ったまま**見張る（止まった後に pid が別のプロセスへ再利用されて「生きている」と読むのを
//! 防ぐ = #2284 の形）。上限（[`BUDGET`]）は見張り役が孫を見つけるまでの幅で、比較には使わない。
//!
//! - 既定: 打ち切りの後に孫が止まる
//! - `TAKO_2454_LEGACY=1`（Job を作らない = #2462 の前）: cmd.exe が止まった後も孫が生きている
//!   = この測り方に検出力がある（修正前の実装では既定のテストが落ちる）
//! - 普通に終わった子の残り（`start /b` の常駐）は止めない（#2462 の前と unix と同じ）
//! - `TAKO_2454_INJECT=assign_fail`（入れ物へ入れる段の失敗）: 打ち切りが返り panic しない
//!   （素の kill へ落ちる）
//!
//! env を立てる 2 本は `tako_core::run_alone!` の子で走る（#2400）。後始末で止めるのは、
//! このテストが起こした子と孫だけ（ハンドルで撃つので別のプロセスへは届かない）。
//!
//! Windows 以外ではコンパイルしない（unix の打ち切りは従来どおり。孫を作らない形は #2448 の `exec`）。

#![cfg(windows)]

use std::ffi::c_void;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tako_core::platform::job;
use tako_core::probe::{self, Outcome};

/// 打ち切りの上限。見張り役が孫を見つけるまでの幅（cmd.exe が `ping` を起こすのは実測で 1 秒未満。
/// 混んだ CI でも足りる幅の状態待ちの上限）
const BUDGET: Duration = Duration::from_secs(15);

/// 止まるのを待つ上限（状態待ちの上限。比較には使わない）
const GONE_LIMIT: Duration = Duration::from_secs(60);

/// 普通に終わった子の残りを止めていないことを見届ける時間（止める処理は返る前に済んでいる）
const KEPT_WATCH: Duration = Duration::from_secs(3);

/// 返ってこない子（`probe::tests::hang_script(Some("partial"))` の Windows の形と同じ）
const HANG: &str = "echo partial& ping -n 600 127.0.0.1 > NUL";

/// すぐ終わるが常駐（`start /b` の ping）を残す子
const LEAVE_DAEMON: &str = "start /b ping -n 600 127.0.0.1 > NUL & echo done";

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
/// （このテストが起こした子と孫にだけ使う）
struct Held {
    pid: u32,
    handle: Handle,
}

// SAFETY: カーネルオブジェクトのハンドルはどのスレッドからでも使える（見張り役から渡す）
unsafe impl Send for Held {}

impl Held {
    fn open(pid: u32) -> Option<Held> {
        // SAFETY: 戻りは null を検査し、drop で 1 回だけ閉じる
        let handle = unsafe {
            OpenProcess(
                SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        (!handle.is_null()).then_some(Held { pid, handle })
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
        // SAFETY: open が返した有効なハンドル。撃つのはこのテストが起こした子と孫だけ
        unsafe {
            if WaitForSingleObject(self.handle, 0) != WAIT_OBJECT_0 {
                TerminateProcess(self.handle, 1);
            }
            CloseHandle(self.handle);
        }
    }
}

/// `cmd /C <script>` を、本番の probe と同じ作法（コンソールを出さない・stdin を塞ぐ・出力はパイプ）で起こす
fn spawn_cmd(script: &str) -> Child {
    let mut command = Command::new("cmd");
    tako_core::platform::process::no_console_window(&mut command)
        .args(["/C", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().expect("cmd.exe を起こせる")
}

/// 見張り役の答え: cmd.exe（終わっていれば `None`）と、その子の `ping.exe`（見つからなければ `None`）
type Watched = (Option<Held>, Option<Held>);

/// 見張り役。**子を起こす前に立てておき**、起こした cmd.exe の pid を送ると、（`hold_parent` なら
/// それを開いてから）その子の `ping.exe` を一覧から見つけて開き、送り返す（`limit` まで）。
/// 起こしてから待ちの 1 実装へ渡すまでの間にスレッドを立てない = 入れ物へ入る前の窓を広げない。
/// すぐ終わる cmd.exe は開かない（終わった後に pid を取った別のプロセスを開いて drop で撃たない）
fn watcher(limit: Duration, hold_parent: bool) -> (mpsc::Sender<u32>, mpsc::Receiver<Watched>) {
    let (pid_tx, pid_rx) = mpsc::channel::<u32>();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok(parent) = pid_rx.recv() else {
            return;
        };
        let cmd = if hold_parent {
            Held::open(parent)
        } else {
            None
        };
        let deadline = Instant::now() + limit;
        let ping = loop {
            let ping = tako_core::platform::procinfo::snapshot()
                .into_iter()
                .find(|p| p.ppid == parent && p.name.eq_ignore_ascii_case("ping.exe"))
                .and_then(|p| Held::open(p.pid));
            if ping.is_some() || Instant::now() >= deadline {
                break ping;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = tx.send((cmd, ping));
    });
    (pid_tx, rx)
}

/// 返らない子を打ち切る。戻りは (cmd.exe, 孫の ping, 打ち切りの stdout)
fn time_out_hang() -> (Held, Held, String) {
    let (send_pid, answer) = watcher(BUDGET, true);
    let child = spawn_cmd(HANG);
    let cmd_pid = child.id();
    send_pid.send(cmd_pid).expect("見張り役へ pid を渡せる");
    let outcome = probe::wait_with_timeout(child, "cmd ping", BUDGET);
    let Outcome::TimedOut { stdout, .. } = outcome else {
        panic!("TimedOut を期待したが {outcome:?}");
    };
    let (cmd, ping) = answer.recv().expect("見張り役が返す");
    let cmd = cmd.unwrap_or_else(|| panic!("打ち切りの前に cmd.exe {cmd_pid} を開けなかった"));
    let ping = ping.unwrap_or_else(|| {
        panic!(
            "cmd.exe {cmd_pid} の孫の ping を打ち切り（{BUDGET:?}）までに見つけられない（測りたい形でない）"
        )
    });
    (cmd, ping, String::from_utf8_lossy(&stdout).into_owned())
}

/// **受け入れ条件 2 / 3**: probe の打ち切りで、cmd.exe 経由で起きた孫（`ping`）まで止まる
/// （`probe::tests` の Windows 版と同じ形の子が孫を残さない）
#[test]
fn issue2462_probeの打ち切りでcmd経由の孫まで止まる() {
    let (cmd, ping, stdout) = time_out_hang();
    assert!(
        stdout.contains("partial"),
        "打ち切る前に出ていたぶんは持ち帰る（実際は {stdout:?}）"
    );
    assert!(
        ping.wait_gone(GONE_LIMIT),
        "打ち切りの後も cmd.exe 経由の孫 {} が {GONE_LIMIT:?} 残った（#2462）",
        ping.pid
    );
    assert!(
        cmd.wait_gone(GONE_LIMIT),
        "直下の cmd.exe {} が残った",
        cmd.pid
    );
    println!(
        "TAKO2462_PROBE cmd={} grandchild={} gone=true",
        cmd.pid, ping.pid
    );
}

/// **A/B の旧アーム（= 修正前の実装）**: 打ち切りで cmd.exe が止まった後も、孫は生きている。
/// 既定のテスト（上）がこの回を「残った」として落とせる = 測り方に検出力がある
#[test]
fn issue2462_旧アームではprobeの打ち切りで孫が残る() {
    if tako_core::run_alone!(issue2462_旧アームではprobeの打ち切りで孫が残る) {
        return;
    }
    std::env::set_var(job::ENV_LEGACY, "1"); // 子の中だけ（他のテストは居ない）
    let (cmd, ping, _) = time_out_hang();
    assert!(
        cmd.wait_gone(GONE_LIMIT),
        "旧アームでも直下の cmd.exe {} は kill で止まるはず",
        cmd.pid
    );
    assert!(
        ping.alive(),
        "旧アームで孫 {} が止まった（この測り方では #2462 の差が出ない）",
        ping.pid
    );
    println!(
        "TAKO2462_PROBE_LEGACY cmd={} grandchild={} survived=true",
        cmd.pid, ping.pid
    );
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// **普通に終わった子の残りは止めない**（#2462 の前と unix と同じ。入れ物の drop は
/// `KILL_ON_JOB_CLOSE` を外してから閉じる）。`start /b` の常駐を残して cmd.exe はすぐ終わる
#[test]
fn issue2462_普通に終わった子が残した常駐は止めない() {
    let (send_pid, answer) = watcher(BUDGET, false);
    let child = spawn_cmd(LEAVE_DAEMON);
    let cmd_pid = child.id();
    send_pid.send(cmd_pid).expect("見張り役へ pid を渡せる");
    let outcome = probe::wait_with_timeout(child, "cmd start", BUDGET);
    let Outcome::Done { status, stdout, .. } = outcome else {
        panic!("Done を期待したが {outcome:?}");
    };
    assert!(status.success(), "cmd.exe の終了コード");
    assert!(String::from_utf8_lossy(&stdout).contains("done"));
    // cmd.exe は開かない（すぐ終わる）。常駐の ping は cmd.exe を親として一覧に残る
    let (_, ping) = answer.recv().expect("見張り役が返す");
    let ping =
        ping.expect("cmd.exe が `start /b` で起こした ping を見つけられない（測りたい形でない）");
    assert!(
        !ping.wait_gone(KEPT_WATCH),
        "普通に終わった子の常駐 {} を止めた（入れ物の drop が中身を止めている）",
        ping.pid
    );
    println!(
        "TAKO2462_PROBE_DONE cmd={cmd_pid} daemon={} kept=true",
        ping.pid
    );
    // 常駐は `Held` の drop が止める（このテストが起こしたもの）
}

/// **割り当てに失敗した回**: 入れ物が無いまま打ち切りが返り、panic しない（素の kill へ落ちる）
#[test]
fn issue2462_jobへ入れられなくてもprobeは打ち切れる() {
    if tako_core::run_alone!(issue2462_jobへ入れられなくてもprobeは打ち切れる) {
        return;
    }
    std::env::set_var(job::ENV_INJECT, "assign_fail"); // 子の中だけ（他のテストは居ない）
    let (cmd, ping, _) = time_out_hang();
    assert!(
        cmd.wait_gone(GONE_LIMIT),
        "入れ物が無い回も直下の cmd.exe {} は素の kill で止まる",
        cmd.pid
    );
    drop(ping);
}
