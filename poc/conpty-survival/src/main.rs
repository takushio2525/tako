//! ConPTY 生存セマンティクス実測スパイク（M0）
//!
//! 目的は 2 点だけ。Issue #518 の案 **B-1（自前 ConPTY セッションホスト）** が成立するかを、
//! 実機の挙動で確定させる。
//!
//! 1. ConPTY を所有するプロセスが死んだとき、中のシェル（pwsh）は道連れに死ぬのか
//! 2. 別プロセス（常駐 host）が ConPTY を所有すれば、tako 終了後もシェルが生き残り、
//!    再 attach できるのか
//!
//! ## 役割の対応（設計書 `.agent/plans/2026-07-windows-persistence-backend.md`）
//!
//! - `host` … B-1 の `tako session-host`。ConPTY を所有し、named pipe で attach を受ける常駐プロセス
//! - `client` … B-1 の `session-client`。tako の PTY の中で pipe 中継するだけの薄いプロセス。
//!   **これを kill する = tako が死ぬ**、のシミュレーション
//! - `launch` … 子プロセスの生成フラグ（DETACHED_PROCESS の有無）を比較するための補助
//!
//! 使い捨ての検証コードなので、エラー処理は「失敗したら即 abort」で割り切っている。

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, OPEN_EXISTING};
use windows_sys::Win32::System::Console::{ClosePseudoConsole, CreatePseudoConsole, COORD, HPCON};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, CreatePipe, DisconnectNamedPipe, PeekNamedPipe,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW,
};

// --- Win32 定数（feature / パスの揺れを避けるためローカル定義）-------------------

const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// ProcThreadAttributeValue(22 /* PseudoConsole */, FALSE, TRUE, FALSE) = 22 | 0x20000
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const ERROR_PIPE_CONNECTED: u32 = 535;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// 再 attach 時にリプレイする画面バイト数の上限
const RING_CAPACITY: usize = 256 * 1024;

// --- 小道具 -------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn die(what: &str) -> ! {
    let err = unsafe { GetLastError() };
    eprintln!("[poc] FATAL {what} (GetLastError={err})");
    std::process::exit(90);
}

/// 進行状況ログ。DETACHED_PROCESS 起動だと stdout / stderr が無効なので、
/// 診断はすべてファイルへ落とす。呼び出しごとに open するが検証用なので気にしない。
fn log_to(path: &str, msg: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[poc {}] {msg}", std::process::id());
        let _ = f.flush();
    }
}

/// `\r` `\n` `\t` `\\` だけを解釈する。PowerShell 側からの引数渡しで実バイトを埋めるのが面倒なため。
fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match it.next() {
            Some('r') => out.push(b'\r'),
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some(other) => {
                out.push(b'\\');
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

fn arg_of(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn args_of(args: &[String], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == name {
            if let Some(v) = args.get(i + 1) {
                out.push(v.clone());
            }
        }
    }
    out
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

// --- host: ConPTY を所有する常駐プロセス（B-1 の session-host 相当）-------------

/// reader スレッドと pipe accept ループで共有する状態。
/// `HANDLE`（`*mut c_void`）は Send でないので `isize` で持ち回す（0 = 未接続）。
struct Shared {
    ring: Vec<u8>,
    client: isize,
}

// ConPTY のセットアップ全体を 1 つの unsafe ブロックに入れているため、
// 内側の FFI 呼び出しに付けた unsafe が「冗長」と判定される。
// どの呼び出しが FFI かを読み手に示す方を優先して、警告だけ抑える。
#[allow(unused_unsafe)]
fn cmd_host(args: &[String]) -> ! {
    let pipe = arg_of(args, "--pipe").unwrap_or_else(|| die("--pipe が必要"));
    let status_path = arg_of(args, "--status").unwrap_or_else(|| die("--status が必要"));
    let log_path = arg_of(args, "--log").unwrap_or_else(|| die("--log が必要"));
    let marker = arg_of(args, "--marker").unwrap_or_else(|| "nomarker".to_string());
    let shell = arg_of(args, "--shell")
        .unwrap_or_else(|| r"C:\Program Files\PowerShell\7\pwsh.exe".to_string());

    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .unwrap_or_else(|e| die(&format!("open log {log_path}: {e}")));

    let host_pid = std::process::id();
    let _ = writeln!(log, "\n[[HOST pid={host_pid} marker={marker} start]]");

    let shell_pid = unsafe {
        // 1. ConPTY の入出力に使う無名パイプ 2 組
        let mut in_read: HANDLE = ptr::null_mut();
        let mut in_write: HANDLE = ptr::null_mut();
        let mut out_read: HANDLE = ptr::null_mut();
        let mut out_write: HANDLE = ptr::null_mut();
        if CreatePipe(&mut in_read, &mut in_write, ptr::null(), 0) == 0 {
            die("CreatePipe(in)");
        }
        if CreatePipe(&mut out_read, &mut out_write, ptr::null(), 0) == 0 {
            die("CreatePipe(out)");
        }

        // 2. 疑似コンソールを作る。**このプロセスが HPCON の所有者になる**（本スパイクの主題）
        let mut hpc: HPCON = 0;
        let hr = CreatePseudoConsole(COORD { X: 120, Y: 30 }, in_read, out_write, 0, &mut hpc);
        if hr != 0 {
            eprintln!("[poc] FATAL CreatePseudoConsole hr=0x{hr:08x}");
            std::process::exit(90);
        }
        // ConPTY 側が複製済み。こちらの端は閉じる（閉じないと out_read が EOF を返さない）
        CloseHandle(in_read);
        CloseHandle(out_write);

        // 3. PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE 付きでシェルを起動
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        let mut attr_size: usize = 0;
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        si.lpAttributeList = attr_buf.as_mut_ptr() as *mut c_void;
        if InitializeProcThreadAttributeList(si.lpAttributeList, 1, 0, &mut attr_size) == 0 {
            die("InitializeProcThreadAttributeList");
        }
        // 注意: lpValue には **HPCON の値そのもの**を渡す（`&hpc` ではない）。
        // 属性リストは lpValue を「ポインタ値」としてそのまま保持し、カーネルは
        // PSEUDOCONSOLE 属性についてはそれを HPCON として解釈する（MS のサンプルも `hPC` を直接渡す）。
        // ここを `&hpc` にすると子はデタラメなコンソールに紐付き、
        // 「pwsh は生きているのに ConPTY 出力が 1 バイトも来ない」という無音の失敗になる。
        if UpdateProcThreadAttribute(
            si.lpAttributeList,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            hpc as usize as *const c_void,
            std::mem::size_of::<HPCON>(),
            ptr::null_mut(),
            ptr::null(),
        ) == 0
        {
            die("UpdateProcThreadAttribute(PSEUDOCONSOLE)");
        }

        // marker を環境変数として焼き込む。Win32_Process.CommandLine から
        // 「自分が起動した pwsh」だけを一意に特定できるようにするため（既存ペインを絶対に触らない）
        let cmdline_s = arg_of(args, "--cmdline").unwrap_or_else(|| {
            format!(
                "\"{shell}\" -NoLogo -NoProfile -NoExit -Command \"$env:TAKO_POC_MARKER='{marker}'\""
            )
        });
        log_to(&log_path, &format!("spawn cmdline: {cmdline_s}"));
        let mut cmdline = wide(&cmdline_s);
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessW(
            ptr::null(),
            cmdline.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0, // bInheritHandles = FALSE
            EXTENDED_STARTUPINFO_PRESENT,
            ptr::null(),
            ptr::null(),
            &si.StartupInfo,
            &mut pi,
        );
        if ok == 0 {
            die("CreateProcessW(shell)");
        }
        DeleteProcThreadAttributeList(si.lpAttributeList);
        CloseHandle(pi.hThread);

        let shell_pid = pi.dwProcessId;

        // 4. ConPTY 出力を常時吸い出すスレッド。
        //    **client が居なくても吸い続ける**のが要点（詰まるとシェルが write でブロックする）
        let shared = Arc::new(Mutex::new(Shared {
            ring: Vec::new(),
            client: 0,
        }));
        let shared_r = Arc::clone(&shared);
        let out_read_i = out_read as isize;
        let mut tee = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap_or_else(|e| die(&format!("open tee {log_path}: {e}")));
        let log_r = log_path.clone();
        thread::spawn(move || {
            let h = out_read_i as HANDLE;
            let mut buf = [0u8; 8192];
            let mut total: u64 = 0;
            loop {
                let mut n: u32 = 0;
                let ok = unsafe {
                    ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, ptr::null_mut())
                };
                if ok == 0 || n == 0 {
                    let err = unsafe { GetLastError() };
                    log_to(
                        &log_r,
                        &format!("conpty reader: EOF ok={ok} n={n} err={err} total={total}"),
                    );
                    break;
                }
                if total == 0 {
                    log_to(&log_r, &format!("conpty reader: first {n} bytes"));
                }
                total += n as u64;
                let data = &buf[..n as usize];
                let _ = tee.write_all(data);
                let _ = tee.flush();

                // ロックを握ったまま WriteFile しない（client が読まないと全体が固まる）
                let client = {
                    let mut g = shared_r.lock().unwrap();
                    g.ring.extend_from_slice(data);
                    let excess = g.ring.len().saturating_sub(RING_CAPACITY);
                    if excess > 0 {
                        g.ring.drain(..excess);
                    }
                    g.client
                };
                if client != 0 {
                    let mut w: u32 = 0;
                    let ok2 = unsafe {
                        WriteFile(
                            client as HANDLE,
                            data.as_ptr(),
                            data.len() as u32,
                            &mut w,
                            ptr::null_mut(),
                        )
                    };
                    if ok2 == 0 {
                        let mut g = shared_r.lock().unwrap();
                        if g.client == client {
                            g.client = 0;
                        }
                    }
                }
            }
            // EOF = シェルが終了した。ホストも役目を終える（後片付けを自動化するため）
            let _ = writeln!(tee, "\n[[HOST pid={host_pid} shell exited -> host exit]]");
            let _ = tee.flush();
            unsafe { ClosePseudoConsole(hpc) };
            std::process::exit(0);
        });

        // 5. named pipe で attach を受け付ける（1 client ずつ）
        // **一方向パイプを 2 本**使う。1 本の duplex パイプを同期ハンドルで読み書き
        // 兼用すると、常時 pending の ReadFile の後ろで WriteFile が詰まって
        // 相互にデッドロックする（同期ハンドルの I/O はカーネルが直列化するため）。
        let in_write_i = in_write as isize;
        let name_out = format!(r"\\.\pipe\{pipe}-out");
        let name_in = format!(r"\\.\pipe\{pipe}-in");
        let shared_a = Arc::clone(&shared);
        let log_o = log_path.clone();
        let log_i = log_path.clone();
        thread::spawn(move || pipe_out_server(&name_out, shared_a, &log_o));
        thread::spawn(move || pipe_in_server(&name_in, in_write_i, &log_i));

        shell_pid
    };

    // 6. 状態ファイル。後続の PowerShell 計測がここから PID を読む
    let status = format!(
        "{{\"host_pid\":{host_pid},\"shell_pid\":{shell_pid},\"pipe\":\"{pipe}\",\"marker\":\"{marker}\"}}\n"
    );
    let mut sf = File::create(&status_path).unwrap_or_else(|e| die(&format!("create status: {e}")));
    let _ = sf.write_all(status.as_bytes());
    let _ = sf.flush();
    drop(sf);
    let _ = writeln!(log, "[[HOST pid={host_pid} shell_pid={shell_pid} ready]]");
    let _ = log.flush();
    // DETACHED_PROCESS 起動だと stdout が無効なので、失敗は握りつぶす
    let _ = writeln!(std::io::stdout(), "{}", status.trim_end());

    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// 名前付きパイプの 1 インスタンスを作って client の接続を待つ。
fn accept_pipe(name_w: &[u16]) -> Option<HANDLE> {
    let h = unsafe {
        CreateNamedPipeW(
            name_w.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            65536,
            65536,
            0,
            ptr::null(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        die("CreateNamedPipeW");
    }
    let connected = unsafe { ConnectNamedPipe(h, ptr::null_mut()) } != 0
        || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
    if connected {
        Some(h)
    } else {
        unsafe { CloseHandle(h) };
        None
    }
}

/// host -> client 方向。attach 時に直近の画面をリプレイし、以後は
/// ConPTY reader スレッドがこのハンドルへ書く（= 再 attach で出力が戻る、の実体）。
fn pipe_out_server(pipe_name: &str, shared: Arc<Mutex<Shared>>, log: &str) {
    let name_w = wide(pipe_name);
    log_to(log, &format!("pipe(out): listening on {pipe_name}"));
    loop {
        let Some(h) = accept_pipe(&name_w) else {
            continue;
        };
        let ring = {
            let g = shared.lock().unwrap();
            g.ring.clone()
        };
        log_to(
            log,
            &format!("pipe(out): client attached, replay {} bytes", ring.len()),
        );
        if !ring.is_empty() {
            let mut w: u32 = 0;
            unsafe { WriteFile(h, ring.as_ptr(), ring.len() as u32, &mut w, ptr::null_mut()) };
        }
        shared.lock().unwrap().client = h as isize;

        // client が居なくなるまで待つ。判定は 2 系統:
        //  a) reader スレッドが書き込みに失敗して client を 0 に戻した
        //  b) PeekNamedPipe が失敗した（= 相手が閉じた）。
        //     シェルが無出力だと a) は永久に発火しないので b) が必須
        loop {
            thread::sleep(Duration::from_millis(200));
            if shared.lock().unwrap().client != h as isize {
                break;
            }
            let mut avail: u32 = 0;
            let alive = unsafe {
                PeekNamedPipe(
                    h,
                    ptr::null_mut(),
                    0,
                    ptr::null_mut(),
                    &mut avail,
                    ptr::null_mut(),
                )
            };
            if alive == 0 {
                let mut g = shared.lock().unwrap();
                if g.client == h as isize {
                    g.client = 0;
                }
                break;
            }
        }
        log_to(log, "pipe(out): client detached");
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }
}

/// client -> host 方向。受け取ったバイトをそのまま ConPTY の入力へ流す。
fn pipe_in_server(pipe_name: &str, conpty_in_write: isize, log: &str) {
    let name_w = wide(pipe_name);
    log_to(log, &format!("pipe(in): listening on {pipe_name}"));
    loop {
        let Some(h) = accept_pipe(&name_w) else {
            continue;
        };
        log_to(log, "pipe(in): client attached");
        let mut buf = [0u8; 4096];
        loop {
            let mut n: u32 = 0;
            let ok =
                unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, ptr::null_mut()) };
            if ok == 0 || n == 0 {
                break;
            }
            let mut w: u32 = 0;
            unsafe {
                WriteFile(
                    conpty_in_write as HANDLE,
                    buf.as_ptr(),
                    n,
                    &mut w,
                    ptr::null_mut(),
                )
            };
            log_to(log, &format!("pipe(in): relayed {n} bytes to ConPTY"));
        }
        log_to(log, "pipe(in): client detached");
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }
}

// --- client: tako の PTY の中で中継するだけの薄いプロセス ------------------------

fn cmd_client(args: &[String]) -> ! {
    let pipe = arg_of(args, "--pipe").unwrap_or_else(|| die("--pipe が必要"));
    let out_path = arg_of(args, "--out").unwrap_or_else(|| die("--out が必要"));
    let sends = args_of(args, "--send");
    let send_delay: u64 = arg_of(args, "--send-delay-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(800);
    let exit_after: Option<u64> = arg_of(args, "--exit-after-ms").and_then(|v| v.parse().ok());
    let log = arg_of(args, "--log").unwrap_or_else(|| format!("{out_path}.clientlog"));

    // host 側と同じ理由で読み用・書き用にパイプを分ける（同期ハンドルの I/O 直列化対策）
    let open = |suffix: &str| -> HANDLE {
        let w = wide(&format!(r"\\.\pipe\{pipe}{suffix}"));
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            let err = unsafe { GetLastError() };
            eprintln!("[poc] client: attach 失敗 pipe={pipe}{suffix} GetLastError={err}");
            std::process::exit(3);
        }
        h
    };
    let h_out = open("-out"); // host -> client（読む側）
    let h = open("-in"); // client -> host（書く側）
    let pid = std::process::id();
    eprintln!("[poc] client pid={pid} attached to {pipe}");
    log_to(&log, &format!("client attached to {pipe}"));

    // pipe -> ファイル（attach 直後のリプレイもここに落ちる）
    let h_i = h_out as isize;
    let out_path_r = out_path.clone();
    let log_r = log.clone();
    thread::spawn(move || {
        let mut f = File::create(&out_path_r).unwrap_or_else(|e| die(&format!("create out: {e}")));
        let hh = h_i as HANDLE;
        let mut buf = [0u8; 8192];
        let mut total: u64 = 0;
        loop {
            let mut n: u32 = 0;
            let ok = unsafe {
                ReadFile(hh, buf.as_mut_ptr(), buf.len() as u32, &mut n, ptr::null_mut())
            };
            if ok == 0 || n == 0 {
                log_to(&log_r, &format!("client reader: EOF total={total}"));
                break;
            }
            let _ = f.write_all(&buf[..n as usize]);
            let _ = f.flush();
            total += n as u64;
        }
    });

    if !sends.is_empty() {
        thread::sleep(Duration::from_millis(send_delay));
        for s in &sends {
            let bytes = unescape(s);
            let mut w: u32 = 0;
            log_to(&log, &format!("client: sending {} bytes", bytes.len()));
            let ok =
                unsafe { WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut w, ptr::null_mut()) };
            log_to(&log, &format!("client: sent ok={ok} written={w}"));
            thread::sleep(Duration::from_millis(300));
        }
    }

    match exit_after {
        Some(ms) => {
            thread::sleep(Duration::from_millis(ms));
            log_to(&log, "client: exit-after reached, exiting");
            std::process::exit(0);
        }
        // kill されるまで生き続ける（= tako が動いている状態）
        None => loop {
            thread::sleep(Duration::from_secs(3600));
        },
    }
}

// --- launch: 生成フラグを変えて子プロセスを起こす補助 ---------------------------

fn cmd_launch(args: &[String]) -> ! {
    let cmdline_s = arg_of(args, "--cmd").unwrap_or_else(|| die("--cmd が必要"));
    let detached = has_flag(args, "--detached");
    let mut flags = 0u32;
    if detached {
        flags |= DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    }

    let mut cmdline = wide(&cmdline_s);
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            ptr::null(),
            cmdline.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            flags,
            ptr::null(),
            ptr::null(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        die("CreateProcessW(launch)");
    }
    println!(
        "{{\"launched_pid\":{},\"detached\":{}}}",
        pi.dwProcessId, detached
    );
    unsafe {
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
    }
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("host") => cmd_host(&args[2..]),
        Some("client") => cmd_client(&args[2..]),
        Some("launch") => cmd_launch(&args[2..]),
        _ => {
            eprintln!(
                "usage:\n  \
                 poc-conpty host   --pipe <name> --status <file> --log <file> [--marker <s>] [--shell <exe>]\n  \
                 poc-conpty client --pipe <name> --out <file> [--send <text>]... [--send-delay-ms N] [--exit-after-ms N]\n  \
                 poc-conpty launch --cmd \"<command line>\" [--detached]"
            );
            std::process::exit(64);
        }
    }
}
