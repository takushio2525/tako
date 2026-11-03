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
//!
//! ## 起動直後の窓（#2697）
//!
//! CI のワークスペース全体の `cargo test` で queue の回が間欠に「孫が 60s 残った」で落ちた（子のテストが
//! 60.12s / 60.16s で終わった = 通る回の 2.11s と違い、孫を**子を起こした直後の 1 回目の心拍**で掴んでいた）。
//! 真因は製品の「起動直後の窓」（`tako_core::platform::job`）: 子を起こしてから入れ物へ入れ終える前に
//! cmd.exe が ping を起こすと、ping は入れ物の外に生まれて取り消しで止まらない（通る回の実測で、ping が
//! 生まれるのは入れ終えた +11 / +14ms 後 = 窓の余白は 12ms ほど）。修正は子を一時停止で起こし、入れて
//! から動かす（`job::KillScope::spawn`）。
//!
//! - 番犬: 入れる前に待たせる注入（`TAKO_2697_INJECT_CONTAIN_DELAY_MS`）を当てても孫まで止まる。
//!   `TAKO_2697_LEGACY=1`（起こしてから入れる）では 1 回目の心拍で掴んだ孫が残る = 番犬に検出力がある
//! - ppid だけで探すと、親の pid が再利用された古い ping（孤児）を掴みうるので、[`ping_child_of`] は
//!   親より後に生まれたものだけを子と読む
//! - 一時停止の口で 20 回起こし、毎回入れ物に入って動き終わる（動かせずに止まったまま残る子が無い）。
//!   起動 1 回の上乗せを `TAKO2697_SPAWN_COST` に出す
//!
//! 落ちたときは [`diagnose`] が孫と子の生成時刻・入れ物へ入れ終えた時刻・ping の一覧から、どちらの形かを名指す。

#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::queue_wrapper::{self, Options, SendError};
use tako_core::platform::job;
use tako_core::terminal::{SpawnCommand, SpawnOptions, TerminalSession};

/// 孫が起きるまでの上限（状態待ちの上限。比較には使わない）
const START_LIMIT: Duration = Duration::from_secs(120);

/// 止まるのを待つ上限（状態待ちの上限。比較には使わない）
const GONE_LIMIT: Duration = Duration::from_secs(60);

/// 止められていないことを見届ける窓（止められたプロセスは kill から間を置かずに終わる）
const CONFIRM_ALIVE: Duration = Duration::from_secs(3);

/// #2697 の注入で入れ物へ入れる前に待つ幅（ms）。cmd.exe が ping を起こすのは実測で 1 秒未満
/// （`crates/tako-core/tests/issue2462_probe_job_windows.rs` の上限の注記）
const CONTAIN_DELAY_MS: &str = "3000";

/// プロセスの生成時刻の刻み（ms）。カーネルの時計は既定で約 15.6ms ごとに進み、生成時刻はその
/// 最後の刻みの値（実際の生成より最大 1 刻み前）
const TICK_MS: u64 = 16;

/// [`classify`] の答え（[`diagnose`] の文に入る。repro のテストが名指しを確かめる）
const VERDICT_REUSE: &str =
    "孫は cmd.exe より前に生まれている = 親の pid が再利用された別の ping を ppid だけで掴んだ（テスト側）";
const VERDICT_WINDOW: &str =
    "孫は入れ物へ入れ終える前に生まれている = 起動直後の窓で入れ物の外に生まれた（製品側）";
const VERDICT_AFTER: &str =
    "孫は入れ物へ入れた後に生まれている = 入れ物に入っているはず（入れ物を作れなかった・止め方）";
const VERDICT_EDGE: &str = "孫の生成は入れ終えた時刻と刻みの幅の中で前後を決められない";
const VERDICT_UNKNOWN: &str = "生成時刻を読めない";

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

/// FILETIME（1601-01-01 からの 100ns）と unix 時刻の差（100ns）
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
    fn CloseHandle(object: Handle) -> i32;
    // FILETIME は 2 つの u32（下位・上位）= u64 の配置と同じ（リトルエンディアン）
    fn GetProcessTimes(
        process: Handle,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
}

/// 開いたハンドルから生成・終了の時刻（unix ms）。終わっていなければ終了は `None`
fn times_of(handle: Handle) -> Option<(u64, Option<u64>)> {
    let (mut creation, mut exit, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: 呼び手が有効なハンドルを渡す。出力は呼び出しの間生きている u64
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    let to_ms = |ft: u64| ft.checked_sub(FILETIME_UNIX_EPOCH).map(|t| t / 10_000);
    if ok == 0 {
        return None;
    }
    let born = to_ms(creation)?;
    Some((born, to_ms(exit).filter(|_| exit != 0)))
}

/// その pid の生成時刻（unix ms）。読むだけの権利で開いて閉じる（止めない。他人のプロセスにも使える）
fn born_ms(pid: u32) -> Option<u64> {
    // SAFETY: 戻りは null を検査し、この関数の中で 1 回だけ閉じる
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let born = times_of(handle).map(|(born, _)| born);
    // SAFETY: 上で開いた有効なハンドル
    unsafe { CloseHandle(handle) };
    born
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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

    /// 生成・終了の時刻（unix ms。ハンドルを持つ間は終わった後も読める）
    fn times(&self) -> Option<(u64, Option<u64>)> {
        times_of(self.handle)
    }

    /// 終了コード（生きていれば 259 = STILL_ACTIVE）
    fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: drop まで有効なハンドル。出力は呼び出しの間生きている
        (unsafe { GetExitCodeProcess(self.handle, &mut code) } != 0).then_some(code)
    }

    /// 止めずに閉じる（開いたものがこのテストの起こしたものでなかったとき）
    fn close_without_stopping(self) {
        // SAFETY: open が返した有効なハンドルを 1 回だけ閉じ、drop（止める）は走らせない
        unsafe { CloseHandle(self.handle) };
        std::mem::forget(self);
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

/// 親より後に生まれたか（`ping_child_of` の読み方。純粋関数）。どちらかが読めなければ子と読まない
fn born_after(child_born: Option<u64>, parent_born: Option<u64>) -> bool {
    matches!((child_born, parent_born), (Some(child), Some(parent)) if child >= parent)
}

/// `parent` の子の `ping.exe` を一覧から 1 回だけ探して開く。Windows は親が終わっても子の親の pid を
/// 書き換えないので、親より前に生まれたもの（親の pid を前に使っていたプロセスの子 = 孤児）は子と読まず、
/// 止めずに閉じる（#2697。開いてから生成時刻を確かめる = 確かめた後に pid が入れ替わらない。
/// 親は呼び手がハンドルを持っているので生成時刻を読める）
fn ping_child_of(parent: u32) -> Option<Held> {
    let parent_born = born_ms(parent);
    tako_core::platform::procinfo::snapshot()
        .into_iter()
        .filter(|p| p.ppid == parent && p.name.eq_ignore_ascii_case("ping.exe"))
        .filter_map(|p| Held::open(p.pid))
        .find_map(|held| {
            if born_after(held.times().map(|(born, _)| born), parent_born) {
                Some(held)
            } else {
                held.close_without_stopping();
                None
            }
        })
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

/// 取り消された回の結果
struct Cancelled {
    /// wrapper の終了コード
    code: i32,
    /// 子 = cmd.exe
    cmd: Held,
    /// 孫 = 見つけて握った ping
    ping: Held,
    /// 孫を見つけた心拍の番号（0 = 子を起こした直後の 1 回目。通る回は 1 = 2 秒後）
    beat: u32,
    /// 心拍に載った子の開始時刻（unix ms。wrapper が子を入れ物へ入れ終えた直後に記録する）
    started_at_ms: Option<u64>,
    /// 取り消しを返した時刻（unix ms と経過の起点）
    cancel_ms: u64,
    cancel_at: Instant,
}

/// `cmd /C ping` を本番の `queue_wrapper::run` で走らせ、孫を握ってから取り消す
fn run_and_cancel() -> Cancelled {
    let argv: Vec<String> = ["cmd", "/C", "ping -n 600 127.0.0.1 > NUL"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let deadline = Instant::now() + START_LIMIT;
    let mut cmd: Option<Held> = None;
    let mut ping: Option<Held> = None;
    let mut beats = 0u32;
    let mut found: Option<(u32, Option<u64>, u64, Instant)> = None;
    let code = {
        let mut send = |req: Request| -> Result<Value, SendError> {
            let Request::Queue { action, job, .. } = &req else {
                panic!("queue 以外の要求: {req:?}");
            };
            Ok(match action.as_deref() {
                Some("enqueue") => json!({ "state": "granted", "share": 0 }),
                Some("started") => {
                    let wire = job.as_ref().expect("心拍に受付の 1 件が載る");
                    let pid = wire.child_pid.expect("心拍に子の pid が載る");
                    if cmd.is_none() {
                        cmd = Held::open(pid);
                    }
                    if ping.is_none() {
                        ping = ping_child_of(pid);
                        if ping.is_some() {
                            found = Some((beats, wire.started_at_ms, now_ms(), Instant::now()));
                        }
                    }
                    beats += 1;
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
    let (beat, started_at_ms, cancel_ms, cancel_at) = found.expect("取り消しの前に孫を握った");
    Cancelled {
        code,
        cmd: cmd.expect("心拍の時点で cmd.exe を開けた"),
        ping: ping.expect("取り消しの前に孫を握った"),
        beat,
        started_at_ms,
        cancel_ms,
        cancel_at,
    }
}

/// 孫の生成時刻から、取り消しの後に残った理由の候補を名指す（#2697）。
///
/// 生成時刻は刻みの値（実際より最大 [`TICK_MS`] 前）、入れ終えた時刻は精密な時計。孫と子は同じ時計
/// なので「孫 < 子」は刻みをまたいだ = 確かに前。入れ終えた時刻とは、孫の刻みの値が 1 刻み以上前なら前、
/// 後ろなら後、その間は決めない
fn classify(ping_born: Option<u64>, cmd_born: Option<u64>, assigned: Option<u64>) -> &'static str {
    let (Some(ping), Some(cmd)) = (ping_born, cmd_born) else {
        return VERDICT_UNKNOWN;
    };
    if ping < cmd {
        return VERDICT_REUSE;
    }
    match assigned {
        Some(assigned) if ping + TICK_MS <= assigned => VERDICT_WINDOW,
        Some(assigned) if ping > assigned => VERDICT_AFTER,
        Some(_) => VERDICT_EDGE,
        None => VERDICT_UNKNOWN,
    }
}

/// 差（ms。`a - b`）を符号つきで書く。どちらかが読めなければ `?`
fn delta(a: Option<u64>, b: Option<u64>) -> String {
    match (a, b) {
        (Some(a), Some(b)) => format!("{:+}", a as i64 - b as i64),
        _ => "?".to_string(),
    }
}

/// 孫を見つけた時機と生成時刻の差の 1 行（通る回にも出す = CI の実測の余白）
fn timing(run: &Cancelled) -> String {
    let ping_born = run.ping.times().map(|(born, _)| born);
    let cmd_born = run.cmd.times().map(|(born, _)| born);
    format!(
        "beat={} ping_minus_cmd_ms={} ping_minus_assigned_ms={}",
        run.beat,
        delta(ping_born, cmd_born),
        delta(ping_born, run.started_at_ms),
    )
}

/// 取り消しの後に孫が残ったときの手掛かり（#2697）: 孫と子の状態・生成 / 終了の時刻・入れ物へ入れ終えた
/// 時刻・取り消しの時刻と経過・一覧の ping の全部（親と、その親の pid の今の持ち主の生成時刻）。
/// 入れ物の中身は wrapper が `TAKO_2697_DIAG=1` で止める直前に stderr へ出す（`[#2697 診断]` の行）
fn diagnose(run: &Cancelled) -> String {
    let (ping_born, ping_exit) = run.ping.times().map_or((None, None), |(b, e)| (Some(b), e));
    let (cmd_born, cmd_exit) = run.cmd.times().map_or((None, None), |(b, e)| (Some(b), e));
    let snapshot = tako_core::platform::procinfo::snapshot();
    let ping_now = snapshot.iter().find(|p| p.pid == run.ping.pid);
    let census: Vec<String> = snapshot
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case("ping.exe"))
        .map(|p| {
            format!(
                "{}(親 {}・生成 {:?}・親の pid の今の持ち主の生成 {:?})",
                p.pid,
                p.ppid,
                born_ms(p.pid),
                born_ms(p.ppid)
            )
        })
        .collect();
    format!(
        "[#2697 診断] 判定: {}\n\
         孫 {}: 生きている={} 終了コード={:?} 生成={ping_born:?} 終了={ping_exit:?} 今の親={:?}\n\
         子 cmd.exe {}: 生きている={} 終了コード={:?} 生成={cmd_born:?} 終了={cmd_exit:?}\n\
         入れ物へ入れ終えた時刻={:?} 孫を見つけた心拍={} 取り消し={} 取り消しからの経過={:?}\n\
         孫-子={}ms 孫-入れ終わり={}ms 子の終了-取り消し={}ms\n\
         一覧の ping: {}",
        classify(ping_born, cmd_born, run.started_at_ms),
        run.ping.pid,
        run.ping.alive(),
        run.ping.exit_code(),
        ping_now.map(|p| p.ppid),
        run.cmd.pid,
        run.cmd.alive(),
        run.cmd.exit_code(),
        run.started_at_ms,
        run.beat,
        run.cancel_ms,
        run.cancel_at.elapsed(),
        delta(ping_born, cmd_born),
        delta(ping_born, run.started_at_ms),
        delta(cmd_exit, Some(run.cancel_ms)),
        census.join(", "),
    )
}

/// **受け入れ条件 2**: `tako queue` の取り消しで、子（cmd.exe）の孫（ping）まで止まる。
/// queue の回は入れ子の印（`TAKO_QUEUE_JOB`）を外した子で走らせ直す（`tako queue cargo test` の
/// 中で回しても入れ子として素通りしない）
#[test]
fn issue2462_queueの取り消しで孫まで止まる() {
    if tako_core::test_child::rerun_alone(
        "issue2462_queueの取り消しで孫まで止まる",
        &[tako_control::queue::ENV_JOB],
    ) {
        return;
    }
    std::env::set_var(job::ENV_DIAG_2697, "1"); // 子の中だけ（他のテストは居ない）
    let run = run_and_cancel();
    assert!(
        run.ping.wait_gone(GONE_LIMIT),
        "取り消しの後も cmd.exe の孫 {} が {GONE_LIMIT:?} 残った（#2462）\n{}",
        run.ping.pid,
        diagnose(&run)
    );
    assert!(
        run.cmd.wait_gone(GONE_LIMIT),
        "取り消した cmd.exe {} が残った\n{}",
        run.cmd.pid,
        diagnose(&run)
    );
    assert_ne!(run.code, 0, "取り消した子が成功で終わった");
    println!(
        "TAKO2462_QUEUE cmd={} grandchild={} gone=true code={} {}",
        run.cmd.pid,
        run.ping.pid,
        run.code,
        timing(&run)
    );
}

/// **A/B の旧アーム（= 修正前の実装）**: 取り消しで cmd.exe が止まった後も、孫は生きている
#[test]
fn issue2462_旧アームではqueueの取り消しで孫が残る() {
    if tako_core::test_child::rerun_alone(
        "issue2462_旧アームではqueueの取り消しで孫が残る",
        &[tako_control::queue::ENV_JOB],
    ) {
        return;
    }
    std::env::set_var(job::ENV_LEGACY, "1"); // 子の中だけ（他のテストは居ない）
    let run = run_and_cancel();
    assert!(
        run.cmd.wait_gone(GONE_LIMIT),
        "旧アームでも直下の cmd.exe {} は kill で止まるはず",
        run.cmd.pid
    );
    assert!(
        run.ping.alive(),
        "旧アームで孫 {} が止まった（この測り方では #2462 の差が出ない）",
        run.ping.pid
    );
    println!(
        "TAKO2462_QUEUE_LEGACY cmd={} grandchild={} survived=true {}",
        run.cmd.pid,
        run.ping.pid,
        timing(&run)
    );
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// **#2697 の番犬**: 子を入れ物へ入れる前に [`CONTAIN_DELAY_MS`] 待たせる注入（起動直後の窓を広げる）を
/// 当てても、取り消しで孫まで止まる。子は一時停止の間に入れ物へ入るので、待っている間に孫は生まれない
/// （孫は入れ物を受け継いで生まれる）。修正前の形（`TAKO_2697_LEGACY=1`）では次のテストのとおり残る
#[test]
fn issue2697_入れ物へ入れる前に待たせても取り消しで孫まで止まる() {
    if tako_core::test_child::rerun_alone(
        "issue2697_入れ物へ入れる前に待たせても取り消しで孫まで止まる",
        &[tako_control::queue::ENV_JOB],
    ) {
        return;
    }
    // 子の中だけ（他のテストは居ない）
    std::env::set_var(job::ENV_INJECT_2697_CONTAIN_DELAY, CONTAIN_DELAY_MS);
    std::env::set_var(job::ENV_DIAG_2697, "1");
    let run = run_and_cancel();
    assert!(
        run.ping.wait_gone(GONE_LIMIT),
        "入れる前に待たせた回で、取り消しの後も cmd.exe の孫 {} が {GONE_LIMIT:?} 残った（#2697）\n{}",
        run.ping.pid,
        diagnose(&run)
    );
    assert!(
        run.cmd.wait_gone(GONE_LIMIT),
        "取り消した cmd.exe {} が残った\n{}",
        run.cmd.pid,
        diagnose(&run)
    );
    println!(
        "TAKO2697_WINDOW_CLOSED cmd={} grandchild={} gone=true {}",
        run.cmd.pid,
        run.ping.pid,
        timing(&run)
    );
}

/// **#2697 の A/B（修正前の形）**: `TAKO_2697_LEGACY=1`（動いている子を入れる）で同じ注入を当てると、
/// cmd.exe が入れ物の外に ping を起こし、孫は 1 回目の心拍で見つかり、取り消しで cmd.exe は止まるが
/// 孫は残る = CI で落ちた形（番犬に検出力がある）。診断はそれを「起動直後の窓」と名指す
#[test]
fn issue2697_旧アームでは入れ物へ入れる前に生まれた孫が取り消しの後も残る() {
    if tako_core::test_child::rerun_alone(
        "issue2697_旧アームでは入れ物へ入れる前に生まれた孫が取り消しの後も残る",
        &[tako_control::queue::ENV_JOB],
    ) {
        return;
    }
    // 子の中だけ（他のテストは居ない）
    std::env::set_var(job::ENV_LEGACY_2697, "1");
    std::env::set_var(job::ENV_INJECT_2697_CONTAIN_DELAY, CONTAIN_DELAY_MS);
    std::env::set_var(job::ENV_DIAG_2697, "1");
    let run = run_and_cancel();
    assert!(
        run.cmd.wait_gone(GONE_LIMIT),
        "取り消した cmd.exe {} が残った\n{}",
        run.cmd.pid,
        diagnose(&run)
    );
    assert!(
        !run.ping.wait_gone(CONFIRM_ALIVE),
        "旧アームで孫が止まった（この測り方では #2697 の差が出ない）\n{}",
        diagnose(&run)
    );
    let report = diagnose(&run);
    assert!(
        run.beat == 0 && report.contains(VERDICT_WINDOW),
        "1 回目の心拍で掴み「起動直後の窓」と名指すはず\n{report}"
    );
    println!("TAKO2697_WINDOW_LEGACY {}\n{report}", timing(&run));
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// 孫の生成時刻から残った理由を名指す判定（[`classify`]）と、子と読む条件（[`born_after`]）
#[test]
fn issue2697_孫の生成時刻から残った理由を名指す() {
    // 子より前に生まれた = 親の pid の再利用（テスト側）。入れ終えた時刻より前でも再利用が先
    assert_eq!(classify(Some(990), Some(1000), Some(1002)), VERDICT_REUSE);
    // 子の後・入れ終えた時刻より刻み 1 つ以上前 = 起動直後の窓（製品側）
    assert_eq!(classify(Some(1010), Some(1000), Some(4000)), VERDICT_WINDOW);
    assert_eq!(
        classify(Some(1010), Some(1000), Some(1010 + TICK_MS)),
        VERDICT_WINDOW
    );
    // 入れ終えた後 = 入れ物に入っているはず
    assert_eq!(classify(Some(1015), Some(1000), Some(1002)), VERDICT_AFTER);
    // 刻みの幅の中は決めない
    assert_eq!(classify(Some(1010), Some(1000), Some(1020)), VERDICT_EDGE);
    assert_eq!(classify(Some(1010), Some(1000), Some(1010)), VERDICT_EDGE);
    // 読めない
    assert_eq!(classify(None, Some(1000), Some(1002)), VERDICT_UNKNOWN);
    assert_eq!(classify(Some(1010), None, Some(1002)), VERDICT_UNKNOWN);
    assert_eq!(classify(Some(1010), Some(1000), None), VERDICT_UNKNOWN);
    // 子と読むのは親と同じ刻み以後に生まれたものだけ
    assert!(born_after(Some(1000), Some(1000)));
    assert!(born_after(Some(1013), Some(1000)));
    assert!(!born_after(Some(961), Some(1000)), "親より前 = 孤児");
    assert!(!born_after(None, Some(1000)));
    assert!(!born_after(Some(1013), None));
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

// ---- #2697: 一時停止の口の繰り返し ----

/// 一時停止の口の繰り返しの回数
const SPAWN_ROUNDS: usize = 20;
/// 1 回の子（`cmd /C exit 0`）が終わるまでの上限（動かせずに止まったままなら落とす）
const SPAWN_EXIT_LIMIT: Duration = Duration::from_secs(30);

/// すぐ終わる子
fn quick_cmd() -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command
        .args(["/C", "exit 0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// 子が上限までに終わった終了状態（終わらなければ止めて `None`）
fn exit_within(
    child: &mut std::process::Child,
    limit: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
}

/// 並びの p50 と最大
fn p50_max(mut micros: Vec<u128>) -> (u128, u128) {
    micros.sort_unstable();
    (
        micros.get(micros.len() / 2).copied().unwrap_or(0),
        micros.last().copied().unwrap_or(0),
    )
}

/// 一時停止の口（`Job::spawn_contained`）で起こした子が毎回 Job に入り、動き出して終わる（動かせずに
/// 止まったまま残る子が無い）。起動 1 回の上乗せを修正前の形（起こしてから入れる）と並べてログへ出す
/// （再開の手段 = `NtResumeProcess` を選んだ根拠の実測は `platform::job` の `resume` の doc）
#[test]
fn issue2697_一時停止の口で起こした子が毎回入れ物に入って動き終わる() {
    use std::os::windows::process::CommandExt;
    /// winbase.h の `CREATE_NO_WINDOW`（製品の口の `Console::NoWindow` と同じ作成フラグで比べる）
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut product = Vec::new();
    for round in 0..SPAWN_ROUNDS {
        let start = Instant::now();
        let (mut child, job) =
            job::Job::spawn_contained(&mut quick_cmd(), job::Console::NoWindow, |_| true)
                .expect("一時停止の口で起こせる");
        product.push(start.elapsed().as_micros());
        assert!(job.is_some(), "{round} 回目: Job に入っていない");
        let status = exit_within(&mut child, SPAWN_EXIT_LIMIT);
        assert!(
            status.is_some_and(|s| s.success()),
            "{round} 回目: 一時停止の口で起こした子が {SPAWN_EXIT_LIMIT:?} で動き終わらない（{status:?}）"
        );
    }
    let mut legacy = Vec::new();
    for _ in 0..SPAWN_ROUNDS {
        let start = Instant::now();
        let mut child = quick_cmd()
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("起こせる");
        let job = job::Job::contain(child.id());
        legacy.push(start.elapsed().as_micros());
        let _ = exit_within(&mut child, SPAWN_EXIT_LIMIT);
        drop(job);
    }
    let (product_p50, product_max) = p50_max(product);
    let (legacy_p50, legacy_max) = p50_max(legacy);
    println!(
        "TAKO2697_SPAWN_COST n={SPAWN_ROUNDS} suspended_p50_us={product_p50} suspended_max_us={product_max} \
         legacy_p50_us={legacy_p50} legacy_max_us={legacy_max}"
    );
}
