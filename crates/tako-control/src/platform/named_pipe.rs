//! Layer 1 IPC の Windows トランスポート実体（抽象境界 B3）
//!
//! named pipe（`\\.\pipe\tako-*`）のサーバーインスタンス生成・クライアント接続・
//! 生存プローブを提供する。ワイヤ形式（1 行 1 JSON）はトランスポート非依存で
//! `ipc.rs`（サーバー）と tako-cli（クライアント）にあり、ここは
//! **バイトストリームの確立だけ**を担う。
//!
//! アクセス制御: named pipe の既定 DACL（作成ユーザーのフルアクセス。Everyone は
//! read のみでリクエストを書き込めない）+ 全リクエストのトークン検証の二段
//! （unix の 0600 + トークンに相当）。`GetNamedPipeClientProcessId` による
//! PeerIdentity 検証は B3 の後続タスク（`.agent/plans/2026-07-windows-port-architecture.md`）。
//!
//! 依存クレートを増やさないため、必要な Win32 API のみ最小の FFI 宣言で呼ぶ。

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};

type Handle = *mut c_void;

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_TYPE_BYTE: u32 = 0;
const PIPE_READMODE_BYTE: u32 = 0;
const PIPE_WAIT: u32 = 0;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const ERROR_PIPE_CONNECTED: i32 = 535;
const ERROR_PIPE_BUSY: i32 = 231;
const ERROR_SEM_TIMEOUT: i32 = 121;

#[link(name = "kernel32")]
extern "system" {
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer_size: u32,
        in_buffer_size: u32,
        default_timeout: u32,
        security_attributes: *mut c_void,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
    fn CreateFileW(
        name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: Handle,
    ) -> Handle;
    fn WaitNamedPipeW(name: *const u16, timeout_ms: u32) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn is_invalid(handle: Handle) -> bool {
    handle as isize == -1
}

/// endpoint 文字列が named pipe 名か（`TAKO_SOCKET` の判別に使う）
pub fn is_pipe_name(endpoint: &str) -> bool {
    endpoint.starts_with(r"\\.\pipe\")
}

/// サーバー側の 1 インスタンス。`wait_client` でクライアント接続を待つ
pub struct ServerInstance {
    handle: Handle,
}

// HANDLE は accept スレッドへ move する（named pipe ハンドルはスレッド間で有効）
unsafe impl Send for ServerInstance {}

impl Drop for ServerInstance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

/// パイプインスタンスを作る。`first` = 名前の初回占有
/// （同名で別プロセスが待ち受け中なら AccessDenied で失敗し、呼び出し側が
/// 一時名へフォールバックする。unix の「固定ソケット生存 → 一時パス」に相当）
pub fn create_server_instance(name: &str, first: bool) -> io::Result<ServerInstance> {
    let mode = PIPE_ACCESS_DUPLEX
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    let handle = unsafe {
        CreateNamedPipeW(
            wide(name).as_ptr(),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null_mut(),
        )
    };
    if is_invalid(handle) {
        return Err(io::Error::last_os_error());
    }
    Ok(ServerInstance { handle })
}

impl ServerInstance {
    /// クライアント接続をブロッキングで待ち、接続済みのバイトストリームを返す。
    /// 返した `File` の drop がハンドルを閉じる（クライアント側は EOF を観測する）
    pub fn wait_client(self) -> io::Result<File> {
        let ok = unsafe { ConnectNamedPipe(self.handle, std::ptr::null_mut()) };
        if ok == 0 {
            let err = io::Error::last_os_error();
            // 接続待ちに入る前にクライアントが繋いでいたケースは成功扱い
            if err.raw_os_error() != Some(ERROR_PIPE_CONNECTED) {
                return Err(err);
            }
        }
        let handle = self.handle;
        std::mem::forget(self); // 所有権を File へ移す（二重 CloseHandle を防ぐ）
        Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
    }
}

/// クライアント側: パイプへ接続する。全インスタンスが使用中の間は
/// `timeout_ms` を上限に待って再試行する
pub fn connect_client(name: &str, timeout_ms: u32) -> io::Result<File> {
    let wname = wide(name);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.into());
    loop {
        let handle = unsafe {
            CreateFileW(
                wname.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if !is_invalid(handle) {
            return Ok(unsafe { File::from_raw_handle(handle as RawHandle) });
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_PIPE_BUSY) || std::time::Instant::now() >= deadline {
            return Err(err);
        }
        unsafe { WaitNamedPipeW(wname.as_ptr(), 50) };
    }
}

/// 生存プローブ: この名前で待ち受け中のサーバーが居るか（discovery の掃除用）。
/// 「インスタンスが全部使用中」も生存として扱う
pub fn probe_alive(name: &str) -> bool {
    if !is_pipe_name(name) {
        return false;
    }
    let wname = wide(name);
    if unsafe { WaitNamedPipeW(wname.as_ptr(), 1) } != 0 {
        return true;
    }
    matches!(
        io::Error::last_os_error().raw_os_error(),
        Some(ERROR_PIPE_BUSY | ERROR_SEM_TIMEOUT)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    fn temp_name(tag: &str) -> String {
        format!(r"\\.\pipe\tako-test-{}-{tag}", std::process::id())
    }

    #[test]
    fn サーバーとクライアントがバイト列を往復できる() {
        let name = temp_name("roundtrip");
        let instance = create_server_instance(&name, true).expect("パイプを作れる");
        let server = std::thread::spawn(move || {
            let mut stream = instance.wait_client().expect("接続を受けられる");
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).unwrap();
            stream.write_all(b"pong\n").unwrap();
            buf
        });
        let mut client = connect_client(&name, 3_000).expect("接続できる");
        client.write_all(b"ping\n").unwrap();
        let mut response = [0u8; 5];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&server.join().unwrap(), b"ping\n");
        assert_eq!(&response, b"pong\n");
    }

    #[test]
    fn 生存プローブは待ち受けの有無を判定する() {
        let name = temp_name("probe");
        assert!(!probe_alive(&name), "サーバー不在なら false");
        let _instance = create_server_instance(&name, true).unwrap();
        assert!(probe_alive(&name), "待ち受け中なら true");
        assert!(!probe_alive("/tmp/not-a-pipe.sock"), "パイプ名以外は false");
    }

    #[test]
    fn 同名の初回占有は二重に成功しない() {
        let name = temp_name("first");
        let _first = create_server_instance(&name, true).unwrap();
        assert!(
            create_server_instance(&name, true).is_err(),
            "FIRST_PIPE_INSTANCE の二重占有は失敗する"
        );
    }
}
