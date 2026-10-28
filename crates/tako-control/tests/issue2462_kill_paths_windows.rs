//! `tako queue` の取り消しとエージェントの再起動で、孫まで止まることの実測（#2462）。
//! **実際に子と孫を起こして測る**。
//!
//! Windows の `TerminateProcess`・`Child::kill()` は子孫を道連れにしない。#2462 まで:
//!
//! - `tako queue` の取り消し（`queue_wrapper` の `tree::terminate`）は直下の子だけを止め、
//!   `tako queue cargo build` の rustc・`cmd /C` の孫が残った
//! - エージェントの再起動（session-restart の harness = `platform::process::terminate`）は
//!   エージェントの pid だけを止め、古いエージェントが起こした MCP サーバ・背景のシェルが
//!   ペインを閉じるまで残った
//!
//! ## 測り方
//!
//! - queue: `cmd /C "ping -n 600 127.0.0.1 > NUL"` を本番の `queue_wrapper::run` で走らせ、偽の調停役
//!   （`send` の閉包）が心拍のたびにプロセスの一覧から cmd.exe の子の `ping` を探し、見つけて
//!   **ハンドルを持ってから**取り消しを返す（止まった後の pid の再利用で偽の緑・赤を出さない = #2284）
//! - 再起動: ConPTY のペイン（器なし = #2454 の Job あり）の PowerShell が「エージェント」役の cmd.exe を
//!   起こし、cmd.exe がその子の `ping` を起こす。本番の再起動と同じ口
//!   （`platform::process::terminate_with_descendants` にペインの `descendant_job`）でエージェントを止め、
//!   孫が止まり**ペインのシェルは生きている**ことを見る
//!
//! どちらも `TAKO_2454_LEGACY=1`（Job を作らない = #2462 の前）の旧アームで孫が残る = 測り方に検出力がある。
//! env を立てる回と queue の回は `tako_core::test_child::rerun_alone` の子で走る（#2400。queue は入れ子の印
//! `TAKO_QUEUE_JOB` を外した子 = `tako queue cargo test` の中で回しても入れ子として素通りしない）。
//! 後始末で止めるのは、このテストが起こした子と孫だけ（ハンドルで撃つので別のプロセスへは届かない）。
//!
//! Windows 以外ではコンパイルしない（unix の取り消しは libproc の木への SIGTERM、再起動は SIGTERM を受けた
//! エージェントが自分の子を畳む = 振る舞いを変えない）。

#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::queue_wrapper::{self, Options, SendError};
use tako_core::platform::job;
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

/// 孫が起きるまでの上限（状態待ちの上限。比較には使わない）
const START_LIMIT: Duration = Duration::from_secs(120);

/// 止まるのを待つ上限（状態待ちの上限。比較には使わない）
const GONE_LIMIT: Duration = Duration::from_secs(60);

/// ペインの中の PowerShell へ「エージェント」の pid の書き出し先を渡す（ペインの env として渡す）
const OUT_ENV: &str = "TAKO_2462_OUT";

/// 「エージェント」役の cmd.exe を起こし（コンソールはペインと同じ）、pid を書いて居座る
const PANE_SCRIPT: &str = "$i = New-Object System.Diagnostics.ProcessStartInfo; \
$i.FileName = 'cmd.exe'; $i.Arguments = '/C ping -n 600 127.0.0.1 > NUL'; \
$i.UseShellExecute = $false; \
$p = [System.Diagnostics.Process]::Start($i); \
[IO.File]::WriteAllText($env:TAKO_2462_OUT, [string]$p.Id); \
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
/// （このテストが起こした子と孫にだけ使う）
struct Held {
    pid: u32,
    handle: Handle,
}

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

/// `parent` の子の `ping.exe` を一覧から 1 回だけ探して開く
fn ping_child_of(parent: u32) -> Option<Held> {
    tako_core::platform::procinfo::snapshot()
        .into_iter()
        .find(|p| p.ppid == parent && p.name.eq_ignore_ascii_case("ping.exe"))
        .and_then(|p| Held::open(p.pid))
}

/// `limit` まで `parent` の子の `ping.exe` を探す（状態待ち）
fn wait_ping_child_of(parent: u32, limit: Duration) -> Held {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(ping) = ping_child_of(parent) {
            return ping;
        }
        assert!(
            Instant::now() < deadline,
            "{parent} の子の ping が {limit:?} 以内に起きなかった（測りたい形でない）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---- tako queue の取り消し ----

/// 取り消された回の結果: (wrapper の終了コード, 子 = cmd.exe, 孫 = ping)
fn run_and_cancel() -> (i32, Held, Held) {
    let argv: Vec<String> = ["cmd", "/C", "ping -n 600 127.0.0.1 > NUL"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let deadline = Instant::now() + START_LIMIT;
    let mut cmd: Option<Held> = None;
    let mut ping: Option<Held> = None;
    let code = {
        let mut send = |req: Request| -> Result<Value, SendError> {
            let Request::Queue { action, job, .. } = &req else {
                panic!("queue 以外の要求: {req:?}");
            };
            Ok(match action.as_deref() {
                Some("enqueue") => json!({ "state": "granted", "share": 0 }),
                Some("started") => {
                    let pid = job
                        .as_ref()
                        .and_then(|j| j.child_pid)
                        .expect("心拍に子の pid が載る");
                    if cmd.is_none() {
                        cmd = Held::open(pid);
                    }
                    if ping.is_none() {
                        ping = ping_child_of(pid);
                    }
                    assert!(
                        ping.is_some() || Instant::now() < deadline,
                        "cmd.exe {pid} の子の ping が {START_LIMIT:?} 以内に起きなかった（測りたい形でない）"
                    );
                    // 孫を握ってから取り消す（握る前に止めると、孫が居たかを確かめられない）
                    json!({ "state": "running", "cancel": ping.is_some() })
                }
                _ => json!({ "state": "done" }),
            })
        };
        queue_wrapper::run(&argv, &Options::default(), &mut send)
    };
    let cmd = cmd.expect("心拍の時点で cmd.exe を開けた");
    let ping = ping.expect("取り消しの前に孫を握った");
    (code, cmd, ping)
}

/// queue の回は入れ子の印を外した子で走らせ直す（`tako queue cargo test` の中でも入れ子として素通りしない）
fn rerun_without_queue_job(test: &str) -> bool {
    tako_core::test_child::rerun_alone(test, &[tako_control::queue::ENV_JOB])
}

/// **受け入れ条件 2**: `tako queue` の取り消しで、子（cmd.exe）の孫（ping）まで止まる
#[test]
fn issue2462_queueの取り消しで孫まで止まる() {
    if rerun_without_queue_job("issue2462_queueの取り消しで孫まで止まる") {
        return;
    }
    let (code, cmd, ping) = run_and_cancel();
    assert!(
        ping.wait_gone(GONE_LIMIT),
        "取り消しの後も cmd.exe の孫 {} が {GONE_LIMIT:?} 残った（#2462）",
        ping.pid
    );
    assert!(
        cmd.wait_gone(GONE_LIMIT),
        "取り消した cmd.exe {} が残った",
        cmd.pid
    );
    assert_ne!(code, 0, "取り消した子が成功で終わった");
    println!(
        "TAKO2462_QUEUE cmd={} grandchild={} gone=true code={code}",
        cmd.pid, ping.pid
    );
}

/// **A/B の旧アーム（= 修正前の実装）**: 取り消しで cmd.exe が止まった後も、孫は生きている
#[test]
fn issue2462_旧アームではqueueの取り消しで孫が残る() {
    if rerun_without_queue_job("issue2462_旧アームではqueueの取り消しで孫が残る") {
        return;
    }
    std::env::set_var(job::ENV_LEGACY, "1"); // 子の中だけ（他のテストは居ない）
    let (_, cmd, ping) = run_and_cancel();
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
        "TAKO2462_QUEUE_LEGACY cmd={} grandchild={} survived=true",
        cmd.pid, ping.pid
    );
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

// ---- エージェントの再起動（session-restart の harness） ----

/// 使い捨ての置き場（テストごと・プロセスごとに別名）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-2462-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作れる");
        Scratch(dir)
    }

    fn out(&self) -> PathBuf {
        self.0.join("agent.pid")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 「エージェント」と孫を抱えたペイン
struct Pane {
    session: TerminalSession,
    _rx: futures::channel::mpsc::UnboundedReceiver<tako_core::terminal::SessionEvent>,
    shell: u32,
    agent: Held,
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
                    PANE_SCRIPT.into(),
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
    let agent = loop {
        if let Some(pid) = std::fs::read_to_string(scratch.out())
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "ペインの PowerShell が {START_LIMIT:?} で「エージェント」の pid を書き出さない（シェル {shell}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let agent = Held::open(agent).expect("書き出された「エージェント」を開ける");
    let grandchild = wait_ping_child_of(agent.pid, START_LIMIT);
    Pane {
        session,
        _rx: rx,
        shell,
        agent,
        grandchild,
    }
}

/// **受け入れ条件 2**: 再起動の口でエージェントを止めると、ペインの Job の中のその子孫（ping）まで
/// 止まり、ペインのシェルは生きている（ペインを閉じたことにならない）
#[test]
fn issue2462_再起動でエージェントの孫まで止まりシェルは残る() {
    let scratch = Scratch::new("restart");
    let pane = spawn_pane(&scratch);
    let job = pane
        .session
        .descendant_job()
        .expect("器なしのペインは子孫を Job に入れる（#2454）");
    assert_eq!(
        job.contains(pane.grandchild.pid),
        Some(true),
        "孫 {} がペインの Job に入っていない",
        pane.grandchild.pid
    );
    let release = tako_control::platform::process::terminate_with_descendants(
        pane.agent.pid,
        false,
        Some(job),
    )
    .expect("エージェントを止められる");
    assert!(
        pane.grandchild.wait_gone(GONE_LIMIT),
        "再起動の後もエージェントの孫 {} が {GONE_LIMIT:?} 残った（#2462）",
        pane.grandchild.pid
    );
    assert!(
        pane.agent.wait_gone(GONE_LIMIT),
        "エージェント {} が残った",
        pane.agent.pid
    );
    assert!(
        release.stopped >= 1 && release.kept == 0,
        "止めた数の読み違い（{release:?}）"
    );
    assert!(
        tako_core::platform::process::pid_alive(pane.shell),
        "ペインのシェル {} まで止めた（ペインを閉じたのと同じになった）",
        pane.shell
    );
    assert_eq!(
        job.contains(pane.shell),
        Some(true),
        "ペインのシェル {} が Job から居なくなった",
        pane.shell
    );
    println!(
        "TAKO2462_RESTART shell={} agent={} grandchild={} gone=true shell_alive=true stopped={}",
        pane.shell, pane.agent.pid, pane.grandchild.pid, release.stopped
    );
}

/// **A/B の旧アーム（= 修正前の実装）**: エージェントが止まった後も、その孫は生きている
#[test]
fn issue2462_旧アームでは再起動でエージェントの孫が残る() {
    if tako_core::run_alone!(issue2462_旧アームでは再起動でエージェントの孫が残る)
    {
        return;
    }
    std::env::set_var(job::ENV_LEGACY, "1"); // 子の中だけ（他のテストは居ない）
    let scratch = Scratch::new("restart-legacy");
    let pane = spawn_pane(&scratch);
    assert!(
        pane.session.descendant_job().is_none(),
        "旧アームで Job を作った"
    );
    let release = tako_control::platform::process::terminate_with_descendants(
        pane.agent.pid,
        false,
        pane.session.descendant_job(),
    )
    .expect("エージェントを止められる");
    assert_eq!(
        release,
        job::Release::default(),
        "Job が無いのに子孫を数えた"
    );
    assert!(
        pane.agent.wait_gone(GONE_LIMIT),
        "旧アームでもエージェント {} は止まるはず",
        pane.agent.pid
    );
    assert!(
        pane.grandchild.alive(),
        "旧アームで孫 {} が止まった（この測り方では #2462 の差が出ない）",
        pane.grandchild.pid
    );
    println!(
        "TAKO2462_RESTART_LEGACY agent={} grandchild={} survived=true",
        pane.agent.pid, pane.grandchild.pid
    );
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// **境界の端**: エージェントが先に死んでいた（再起動の前に落ちた）回も panic せず、親を失った孫
/// （親の pid は書き換わらない）を止め、ペインのシェルは撃たない。終わった相手への終了要求は
/// 果たされている扱い（#1599 の契約。テストがハンドルを持つ間は pid が再利用されない）
#[test]
fn issue2462_先に死んだエージェントの孫も止めシェルは撃たない() {
    let scratch = Scratch::new("restart-dead");
    let pane = spawn_pane(&scratch);
    let job = pane
        .session
        .descendant_job()
        .expect("器なしのペインは Job を持つ");
    // エージェントだけを先に止める（テストが握っているハンドルで）
    // SAFETY: このテストが起こした「エージェント」の有効なハンドル
    unsafe { TerminateProcess(pane.agent.handle, 1) };
    assert!(pane.agent.wait_gone(GONE_LIMIT));
    assert!(
        pane.grandchild.alive(),
        "孫が親と一緒に止まった（測りたい形でない）"
    );
    let release = tako_control::platform::process::terminate_with_descendants(
        pane.agent.pid,
        false,
        Some(job),
    )
    .expect("終わった相手への終了要求は果たされている扱い");
    assert!(
        pane.grandchild.wait_gone(GONE_LIMIT),
        "先に死んだエージェントの孫 {} が残った（{release:?}）",
        pane.grandchild.pid
    );
    assert!(
        tako_core::platform::process::pid_alive(pane.shell),
        "ペインのシェル {} まで止めた",
        pane.shell
    );
    println!(
        "TAKO2462_RESTART_DEAD_AGENT agent={} grandchild={} gone=true shell_alive=true",
        pane.agent.pid, pane.grandchild.pid
    );
}
