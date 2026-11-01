//! 言語サーバを止めたとき、cmd.exe 経由で起きた孫まで止まることの実測（#2454）。
//! **実際に `.cmd` → cmd.exe → 偽サーバの 3 段を起こして測る**。
//!
//! npm が配る言語サーバは `.cmd` で、std の `Command` は `.cmd` を cmd.exe 経由で起こす。
//! #2454 まで `ServerProcess::kill` は `Child::kill`（= cmd.exe への `TerminateProcess`）だけで、
//! cmd.exe の子（node.exe）は残った。
//!
//! ## 測り方
//!
//! 一時 dir に `fake-ls.cmd`（偽サーバ `tako-lsp-fake` を `linger` で起こすだけ）を置き、
//! `LspManager` から本番と同じ経路で起こして `stop`（= `tako lsp stop`）する。`linger` は `exit` も
//! stdin の EOF も無視して居座る = 止まるのは kill だけ（cmd.exe の孫として残ったら居座る形）。
//! 偽サーバは自分の pid を `--spawns` へ書くので、テストはそれを**ハンドルを持ったまま**見張る
//! （止まった後に pid が別のプロセスへ再利用されて「生きている」と読むのを防ぐ = #2284 の形）。
//!
//! - 既定: stop の後に孫（偽サーバ）が止まる
//! - `TAKO_2454_LEGACY=1`（A/B の旧アーム = #2454 前）: stop が済み cmd.exe が止まった後も孫が
//!   生きている = この測り方に検出力がある（修正前の実装では既定のテストが落ちる）
//! - `TAKO_2454_INJECT=assign_fail`（Job へ入れる段の失敗）: 起動・握手・stop が通り panic しない
//!   （素の kill へ落ちる）
//!
//! env を立てる 2 本は `tako_core::run_alone!` の子で走る（#2400）。後始末で止めるのは、
//! このテストが起こした偽サーバだけ（ハンドルで撃つので別のプロセスへは届かない）。
//!
//! Windows 以外ではコンパイルしない（unix の言語サーバは従来どおり kill と stdin の EOF に任せる）。

#![cfg(windows)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::platform::job;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 起動と握手を待つ上限（cmd.exe 経由の起動。混んだ CI でも足りる幅の状態待ちの上限）
const START_LIMIT: Duration = Duration::from_secs(60);

/// 止まるのを待つ上限（状態待ちの上限。比較には使わない）
const GONE_LIMIT: Duration = Duration::from_secs(60);

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
/// （このテストが起こした偽サーバにだけ使う）
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
            "偽サーバ {pid} を開けない（{}）",
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
        // SAFETY: open が返した有効なハンドル。撃つのは自分が起こした偽サーバだけ
        unsafe {
            if WaitForSingleObject(self.handle, 0) != WAIT_OBJECT_0 {
                TerminateProcess(self.handle, 1);
            }
            CloseHandle(self.handle);
        }
    }
}

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        // 初回 exec の検査を時計の外で済ませる（#2228）
        tako_core::fake_exec::warm_binary(FAKE);
        let dir = std::env::temp_dir().join(format!(
            "tako-2454-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let scratch = Self(dir);
        // npm の言語サーバと同じ形: `.cmd` が本体（node.exe の代わりに偽サーバ）を起こす
        std::fs::write(
            scratch.wrapper(),
            format!(
                "@echo off\r\n\"{FAKE}\" --scenario linger --log \"{}\" --spawns \"{}\"\r\n",
                scratch.0.join("received.jsonl").display(),
                scratch.spawns().display()
            ),
        )
        .unwrap();
        scratch
    }

    fn file(&self) -> PathBuf {
        self.0.join("src").join("main.rs")
    }

    fn wrapper(&self) -> PathBuf {
        self.0.join("fake-ls.cmd")
    }

    fn spawns(&self) -> PathBuf {
        self.0.join("spawns.txt")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(scratch: &Scratch) -> LspConfig {
    let wrapper = scratch.wrapper().display().to_string();
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: wrapper.clone(),
                args: Vec::new(),
            },
            program_path: wrapper.clone(),
        }),
        request_timeout: Duration::from_secs(30),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 0,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {limit:?} 以内に起きなかった"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn state(manager: &LspManager) -> String {
    manager.status(None)["servers"][0]["state"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// 動いている言語サーバ 1 つ（直下の子 = cmd.exe と、その子 = 偽サーバ）
struct Running {
    manager: LspManager,
    _link: DocLink,
    cmd: u32,
    server: Held,
}

fn start(scratch: &Scratch) -> Running {
    let manager = LspManager::new(config(scratch));
    let mut link = DocLink::default();
    manager.sync(&mut link, true, &scratch.file(), "fn main() {}\n", 0);
    wait_until("握手（running）", START_LIMIT, || {
        state(&manager) == "running"
    });
    let cmd = *manager
        .server_pids()
        .first()
        .expect("動いているサーバの pid");
    let pid = std::fs::read_to_string(scratch.spawns())
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.trim().parse::<u32>().ok())
        .expect("偽サーバが pid を書いている（握手の前に書く）");
    assert_ne!(
        pid, cmd,
        "偽サーバが直下の子として起きた（cmd.exe 経由の孫になっていない = 測りたい形でない）"
    );
    let server = Held::open(pid);
    assert!(server.alive(), "偽サーバ {pid} が既に居ない");
    Running {
        manager,
        _link: link,
        cmd,
        server,
    }
}

fn stop(running: &Running) {
    running.manager.stop(None);
    wait_until("止めた（stopped）", GONE_LIMIT, || {
        state(&running.manager) == "stopped"
    });
}

/// **受け入れ条件 2**: `tako lsp stop` の経路で止めると、cmd.exe 経由で起きた孫
/// （`exit` を無視して居座る偽サーバ）まで止まる
#[test]
fn issue2454_lsp_を止めるとcmd_exe経由の孫まで止まる() {
    let scratch = Scratch::new("lsp-stop");
    let running = start(&scratch);
    stop(&running);
    assert!(
        running.server.wait_gone(GONE_LIMIT),
        "stop の後も cmd.exe 経由の孫 {} が {GONE_LIMIT:?} 残った（#2454）",
        running.server.pid
    );
    assert!(
        tako_core::platform::process::wait_gone(running.cmd, GONE_LIMIT),
        "直下の cmd.exe {} が残った",
        running.cmd
    );
    println!(
        "TAKO2454_LSP_STOP cmd={} server={} gone=true",
        running.cmd, running.server.pid
    );
}

/// **A/B の旧アーム（= 修正前の実装）**: stop が済み cmd.exe が止まった後も、孫は生きている。
/// 既定のテスト（上）がこの回を「残った」として落とせる = 測り方に検出力がある
#[test]
fn issue2454_旧アームではlspを止めても孫が残る() {
    if tako_core::run_alone!(issue2454_旧アームではlspを止めても孫が残る) {
        return;
    }
    std::env::set_var(job::ENV_LEGACY, "1"); // 子の中だけ（他のテストは居ない）
    let scratch = Scratch::new("lsp-legacy");
    let running = start(&scratch);
    stop(&running);
    assert!(
        tako_core::platform::process::wait_gone(running.cmd, GONE_LIMIT),
        "旧アームでも直下の cmd.exe {} は kill で止まるはず",
        running.cmd
    );
    assert!(
        running.server.alive(),
        "旧アームで孫 {} が止まった（この測り方では #2454 の差が出ない）",
        running.server.pid
    );
    println!(
        "TAKO2454_LSP_LEGACY cmd={} server={} survived=true",
        running.cmd, running.server.pid
    );
    // 孫は `Held` の drop が止める（このテストが起こしたもの）
}

/// **割り当てに失敗した回**: Job が無いまま起動・握手・stop が通り、panic しない
/// （直下の cmd.exe は素の kill で止まる）
#[test]
fn issue2454_jobへ入れられなくてもlspは起動し止められる() {
    if tako_core::run_alone!(issue2454_jobへ入れられなくてもlspは起動し止められる)
    {
        return;
    }
    std::env::set_var(job::ENV_INJECT, "assign_fail"); // 子の中だけ（他のテストは居ない）
    let scratch = Scratch::new("lsp-assign-fail");
    let running = start(&scratch);
    stop(&running);
    assert!(
        tako_core::platform::process::wait_gone(running.cmd, GONE_LIMIT),
        "Job が無い回も直下の cmd.exe {} は素の kill で止まる",
        running.cmd
    );
}
