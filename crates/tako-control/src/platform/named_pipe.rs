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
//!
//! 読み取りに上限が要る相手（remote daemon → GUI。#2774）は [`connect_client_stream`] の
//! [`PipeStream`] を使う。同期の `File` は `ReadFile` が戻るまで待つしかなく、unix の
//! `set_read_timeout` に当たるものが無い（GUI が固まると daemon の HTTP ワーカーが戻らない）。
//! [`PipeStream`] は overlapped I/O で読み、上限を過ぎたら `CancelIoEx` で取り消して
//! **取り消しの完了まで待ってから**返す（返った後に OS がバッファと OVERLAPPED を触らない）

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::time::Duration;

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
const ERROR_NO_SYSTEM_RESOURCES: i32 = 1450;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const ERROR_BROKEN_PIPE: i32 = 109;
const ERROR_OPERATION_ABORTED: i32 = 995;
const ERROR_IO_PENDING: i32 = 997;
/// 打ち切りを返すときの OS エラー（`io::ErrorKind::TimedOut` に解釈される）
const ERROR_TIMEOUT: i32 = 1460;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 0x0000_0102;
const INFINITE: u32 = 0xFFFF_FFFF;
const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;

/// Win32 の `OVERLAPPED`（`Offset` / `OffsetHigh` と `Pointer` の共用体は前者の形で持つ。
/// パイプは位置を使わないので 0 のまま）
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

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
    fn ReadFile(
        file: Handle,
        buffer: *mut u8,
        to_read: u32,
        read: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn WriteFile(
        file: Handle,
        buffer: *const u8,
        to_write: u32,
        written: *mut u32,
        overlapped: *mut Overlapped,
    ) -> i32;
    fn GetOverlappedResult(
        file: Handle,
        overlapped: *mut Overlapped,
        transferred: *mut u32,
        wait: i32,
    ) -> i32;
    fn CancelIoEx(file: Handle, overlapped: *mut Overlapped) -> i32;
    fn CreateEventW(
        security_attributes: *mut c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> Handle;
    fn WaitForSingleObject(handle: Handle, timeout_ms: u32) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        desired_access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
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
    if take_injected_failure(name) {
        return Err(io::Error::from_raw_os_error(ERROR_NO_SYSTEM_RESOURCES));
    }
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

/// インスタンスの作成へ注入した失敗（#2361。名前ごと）
struct InjectedFailures {
    name: String,
    /// これから失敗させる回数
    remaining: u32,
    /// 作成が試みられて失敗させた回数
    taken: u32,
}

static INJECTED: std::sync::Mutex<Vec<InjectedFailures>> = std::sync::Mutex::new(Vec::new());

/// この名前のインスタンスの作成を次の `count` 回失敗させる（#2361 の検証）。
///
/// 本番と同じ [`create_server_instance`] の入口で効く（呼び出し側の accept ループはそのまま
/// 通る = テスト専用の別実装にしない）。失敗は資源不足（`ERROR_NO_SYSTEM_RESOURCES`）の形で返す
pub fn inject_create_failures(name: &str, count: u32) {
    let Ok(mut injected) = INJECTED.lock() else {
        return;
    };
    match injected.iter_mut().find(|i| i.name == name) {
        Some(entry) => entry.remaining = count,
        None => injected.push(InjectedFailures {
            name: name.to_string(),
            remaining: count,
            taken: 0,
        }),
    }
}

/// 注入した失敗のうち、作成が試みられて実際に失敗させた回数（#2361）
pub fn injected_failures_taken(name: &str) -> u32 {
    INJECTED
        .lock()
        .ok()
        .and_then(|injected| injected.iter().find(|i| i.name == name).map(|i| i.taken))
        .unwrap_or(0)
}

/// 注入した失敗が残っていれば 1 回ぶん使う
fn take_injected_failure(name: &str) -> bool {
    let Ok(mut injected) = INJECTED.lock() else {
        return false;
    };
    match injected
        .iter_mut()
        .find(|i| i.name == name && i.remaining > 0)
    {
        Some(entry) => {
            entry.remaining -= 1;
            entry.taken += 1;
            true
        }
        None => false,
    }
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
    let handle = open_client(name, timeout_ms, 0)?;
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

/// クライアント側: 読み取りに上限を付けられるストリームで接続する（#2774）。
/// 接続の待ち方は [`connect_client`] と同じ（`timeout_ms` = 全インスタンスが使用中の間の
/// 再試行の上限）。上限は [`PipeStream::set_read_timeout`] で付ける
pub fn connect_client_stream(name: &str, timeout_ms: u32) -> io::Result<PipeStream> {
    let handle = open_client(name, timeout_ms, FILE_FLAG_OVERLAPPED)?;
    PipeStream::from_handle(handle)
}

/// 接続の 1 実装。`flags` は `CreateFileW` の属性（overlapped で開くかどうか）
fn open_client(name: &str, timeout_ms: u32, flags: u32) -> io::Result<Handle> {
    let wname = wide(name);
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.into());
    loop {
        let handle = unsafe {
            CreateFileW(
                wname.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        };
        if !is_invalid(handle) {
            return Ok(handle);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_PIPE_BUSY) || std::time::Instant::now() >= deadline {
            return Err(err);
        }
        unsafe { WaitNamedPipeW(wname.as_ptr(), 50) };
    }
}

/// 読み取りに上限を付けられる名前付きパイプのストリーム（#2774。unix の `UnixStream` の
/// `set_read_timeout` に当たる）。
///
/// ハンドルは overlapped で開いてあるので `std::fs::File` として読み書きしない（std の `File` は
/// 同期の完了を前提にしていて、保留になった I/O を扱えない）。読み書きはこの型の
/// `Read` / `Write` だけが行い、どちらも**完了（打ち切りなら取り消しの完了）まで待ってから**
/// 返す。なので drop の時点で保留中の I/O は無く、ハンドルとイベントを閉じるだけでよい
pub struct PipeStream {
    handle: Handle,
    /// この値の I/O の完了を知らせる手動リセットのイベント（値ごとに 1 つ。複製と共有すると
    /// 片方の完了で他方の待ちが起きる）
    event: Handle,
    /// 読み取り 1 回の上限（`None` = 上限なし）
    read_timeout: Option<Duration>,
}

// HANDLE は読み書きの半分ずつを別スレッドへ渡せる（名前付きパイプのハンドルはスレッド間で有効）
unsafe impl Send for PipeStream {}

impl Drop for PipeStream {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.event);
            CloseHandle(self.handle);
        }
    }
}

impl PipeStream {
    /// 開いたハンドルを引き取る（イベントを作れなければハンドルを閉じて失敗を返す）
    fn from_handle(handle: Handle) -> io::Result<Self> {
        let event = unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            let err = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(err);
        }
        Ok(Self {
            handle,
            event,
            read_timeout: None,
        })
    }

    /// 読み取り 1 回の上限を付ける（`None` で外す）。上限を過ぎた読み取りは
    /// `io::ErrorKind::TimedOut` で返る。`Some(0)` は std の `set_read_timeout` と同じく
    /// `InvalidInput`。
    ///
    /// unix の `SO_RCVTIMEO` はソケットに付くので複製にも効くが、ここは値ごとに持つ
    /// （[`Self::try_clone`] はその時点の値を写す）
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        if timeout == Some(Duration::ZERO) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot set a 0 duration timeout",
            ));
        }
        self.read_timeout = timeout;
        Ok(())
    }

    /// いま付いている読み取りの上限
    pub fn read_timeout(&self) -> Option<Duration> {
        self.read_timeout
    }

    /// 同じ接続を指す別の値を作る（読みと書きを分けて持つため。`UnixStream::try_clone` と同じ役）
    pub fn try_clone(&self) -> io::Result<Self> {
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate: Handle = std::ptr::null_mut();
        let ok = unsafe {
            DuplicateHandle(
                process,
                self.handle,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut clone = Self::from_handle(duplicate)?;
        clone.read_timeout = self.read_timeout;
        Ok(clone)
    }

    /// overlapped の I/O を 1 回行い、転送したバイト数を返す。`start` は I/O を始める呼び出し
    /// （`ReadFile` / `WriteFile`）。`limit` を過ぎたら取り消し、**取り消しの完了を待ってから**
    /// `TimedOut` を返す（取り消しより先に完了していたら、その結果を返して届いたものを捨てない）
    fn overlapped_io(
        &self,
        limit: Option<Duration>,
        start: impl FnOnce(*mut Overlapped) -> i32,
    ) -> io::Result<usize> {
        let mut ov = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: self.event,
        };
        // ReadFile / WriteFile は始める時にイベントを非シグナルへ戻す
        if start(&mut ov) == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_IO_PENDING) {
                // 始まらなかった（保留中の I/O は無い）
                return Err(err);
            }
            let waited = unsafe { WaitForSingleObject(self.event, wait_ms(limit)) };
            if waited != WAIT_OBJECT_0 {
                // 上限切れ（か待ちそのものの失敗）。OS がまだ `ov` とバッファを持っているので、
                // 取り消して完了を待つまで返さない
                let wait_err = io::Error::last_os_error();
                unsafe { CancelIoEx(self.handle, &mut ov) };
                let mut moved = 0u32;
                if unsafe { GetOverlappedResult(self.handle, &mut ov, &mut moved, 1) } != 0 {
                    return Ok(moved as usize);
                }
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(ERROR_OPERATION_ABORTED) {
                    return Err(err);
                }
                return Err(if waited == WAIT_TIMEOUT {
                    io::Error::from_raw_os_error(ERROR_TIMEOUT)
                } else {
                    wait_err
                });
            }
        }
        let mut moved = 0u32;
        if unsafe { GetOverlappedResult(self.handle, &mut ov, &mut moved, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(moved as usize)
    }
}

/// 待ちの上限をミリ秒へ（`None` = 無限。端数は切り上げ、0 にはしない = 0 は「待たない」になる）
fn wait_ms(limit: Option<Duration>) -> u32 {
    match limit {
        None => INFINITE,
        Some(limit) => {
            let ms = limit.as_nanos().div_ceil(1_000_000);
            ms.clamp(1, u128::from(INFINITE - 1)) as u32
        }
    }
}

impl io::Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let handle = self.handle;
        let ptr = buf.as_mut_ptr();
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        match self.overlapped_io(self.read_timeout, |ov| unsafe {
            ReadFile(handle, ptr, len, std::ptr::null_mut(), ov)
        }) {
            // 相手が閉じた = EOF（std の `File` の読み取りと同じ解釈）
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE) => Ok(0),
            other => other,
        }
    }
}

impl io::Write for PipeStream {
    /// 書き込みには上限を付けない（unix の daemon も読み取りだけに上限を付けている。#2774）
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let handle = self.handle;
        let ptr = buf.as_ptr();
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        self.overlapped_io(None, |ov| unsafe {
            WriteFile(handle, ptr, len, std::ptr::null_mut(), ov)
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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

    // --- #2774: 読み取りの上限（overlapped + CancelIoEx）---
    //
    // 修正前の daemon は同期の `File` で読んでいたので、答えない相手の前で `read` が戻らなかった。
    // 下のテストは読み取りを別スレッドで走らせ、テスト側の待ち（`within_2774`）を過ぎたら
    // **名指しで落とす**（上限が効いていない実装でも CI をハングで止めない）。判定は時間ではなく
    // 「どの結果で戻ったか」（TimedOut / 届いたバイト / EOF）で書く（時間の番犬の規則）

    /// テスト側の待ち。読み取りの上限（下の各テストの `limit`）より桁で長くとる
    const GUARD_2774: Duration = Duration::from_secs(20);

    /// `work` を別スレッドで走らせ、[`GUARD_2774`] までに戻らなければ `what` を名指しで落とす
    fn within_2774<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        match rx.recv_timeout(GUARD_2774) {
            Ok(value) => value,
            Err(_) => panic!(
                "#2774: {what} が {GUARD_2774:?} 待っても戻らない（名前付きパイプの読み取りに上限が効いていない）"
            ),
        }
    }

    #[test]
    fn issue2774_答えない相手の読み取りは上限で打ち切り後から届いた応答も読める() {
        let name = temp_name("2774-silent");
        let limit = Duration::from_millis(200);
        let instance = create_server_instance(&name, true).expect("パイプを作れる");
        let (release, released) = std::sync::mpsc::channel::<()>();
        // 要求を受け取ったまま答えない相手（GUI が固まった形）。合図で遅れて答え、
        // その後はクライアントが閉じるまで読む（複製のハンドルが漏れていれば EOF が来ない）
        let server = std::thread::spawn(move || -> io::Result<Vec<u8>> {
            let mut stream = instance.wait_client()?;
            let mut request = [0u8; 5];
            stream.read_exact(&mut request)?;
            let _ = released.recv();
            stream.write_all(b"late\n")?;
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest)?;
            Ok(rest)
        });

        let mut client = connect_client_stream(&name, 3_000).expect("接続できる");
        client
            .set_read_timeout(Some(limit))
            .expect("上限を付けられる");
        let mut read_half = client.try_clone().expect("複製できる");
        assert_eq!(read_half.read_timeout(), Some(limit), "複製は上限を写す");
        client.write_all(b"ping\n").expect("書ける");

        let (read_half, first) = within_2774("答えない相手の読み取り", move || {
            let started = std::time::Instant::now();
            let mut buf = [0u8; 16];
            let got = read_half.read(&mut buf);
            println!(
                "TAKO2774_SILENT_READ result={:?} took={:?}",
                got.as_ref().map_err(|e| (e.kind(), e.raw_os_error())),
                started.elapsed()
            );
            (read_half, got.map_err(|e| (e.kind(), e.raw_os_error())))
        });
        assert_eq!(
            first,
            Err((io::ErrorKind::TimedOut, Some(ERROR_TIMEOUT))),
            "答えない相手の読み取りは上限で TimedOut を返す"
        );

        // 取り消しの後も同じ接続で読める（取り消しが接続を壊さない・遅れた応答を失わない）
        release.send(()).expect("合図");
        let mut read_half = read_half;
        read_half
            .set_read_timeout(Some(GUARD_2774))
            .expect("上限を付け直せる");
        let (read_half, late) = within_2774("遅れて届いた応答の読み取り", move || {
            let mut buf = [0u8; 5];
            let got = read_half.read_exact(&mut buf).map(|()| buf);
            (read_half, got.map_err(|e| e.kind()))
        });
        assert_eq!(late, Ok(*b"late\n"), "打ち切りの後に届いた応答を読める");

        // 両方の値を閉じたら相手は EOF を見る（ハンドルを漏らしていない）
        drop(read_half);
        drop(client);
        let rest = within_2774("閉じた後の相手の EOF", move || server.join());
        assert!(
            matches!(rest, Ok(Ok(ref rest)) if rest.is_empty()),
            "クライアントを閉じたら相手の読み取りは EOF で終わる: {rest:?}"
        );
    }

    #[test]
    fn issue2774_途中で切った相手は上限を待たずに届いたぶんとeofを返す() {
        let name = temp_name("2774-hangup");
        // 上限はテスト側の待ちより長い = 上限まで待つ実装なら within_2774 が名指しで落ちる
        let limit = GUARD_2774 * 3;
        let instance = create_server_instance(&name, true).expect("パイプを作れる");
        let server = std::thread::spawn(move || -> io::Result<()> {
            let mut stream = instance.wait_client()?;
            let mut request = [0u8; 5];
            stream.read_exact(&mut request)?;
            // 改行の前で切る（応答の途中で GUI が落ちた形）
            stream.write_all(b"par")
        });
        let mut client = connect_client_stream(&name, 3_000).expect("接続できる");
        client
            .set_read_timeout(Some(limit))
            .expect("上限を付けられる");
        client.write_all(b"ping\n").expect("書ける");
        let got = within_2774("途中で切った相手の読み取り", move || {
            let mut all = Vec::new();
            let got = client.read_to_end(&mut all).map(|_| all);
            got.map_err(|e| (e.kind(), e.raw_os_error()))
        });
        assert_eq!(
            got,
            Ok(b"par".to_vec()),
            "届いたぶんを読み、相手が閉じたら EOF（BrokenPipe のエラーにしない）"
        );
        server.join().expect("相手").expect("相手の書き込み");
    }

    #[test]
    fn issue2774_上限より前に届いた応答は打ち切らずに読む() {
        let name = temp_name("2774-reply");
        let limit = Duration::from_secs(10);
        let instance = create_server_instance(&name, true).expect("パイプを作れる");
        let server = std::thread::spawn(move || -> io::Result<[u8; 5]> {
            let mut stream = instance.wait_client()?;
            let mut request = [0u8; 5];
            stream.read_exact(&mut request)?;
            // 答えを少し遅らせて、クライアントの読み取りを保留（IO_PENDING）の経路に乗せる
            std::thread::sleep(Duration::from_millis(100));
            stream.write_all(b"pong\n")?;
            Ok(request)
        });
        let mut client = connect_client_stream(&name, 3_000).expect("接続できる");
        client
            .set_read_timeout(Some(limit))
            .expect("上限を付けられる");
        let mut read_half = client.try_clone().expect("複製できる");
        client.write_all(b"ping\n").expect("書ける");
        let reply = within_2774("上限より前に届く応答の読み取り", move || {
            let mut buf = [0u8; 5];
            read_half
                .read_exact(&mut buf)
                .map(|()| buf)
                .map_err(|e| e.kind())
        });
        assert_eq!(
            reply,
            Ok(*b"pong\n"),
            "上限より前に届いた応答をそのまま読む"
        );
        assert_eq!(
            server.join().expect("相手").expect("相手の読み書き"),
            *b"ping\n",
            "書いた要求が相手に届く（複製の側から書いても同じ接続）"
        );
    }

    #[test]
    fn issue2774_読み取りの上限の既定と0の扱いはstdと同じ() {
        let name = temp_name("2774-limit");
        let _instance = create_server_instance(&name, true).expect("パイプを作れる");
        let mut client = connect_client_stream(&name, 3_000).expect("接続できる");
        assert_eq!(client.read_timeout(), None, "既定は上限なし（std と同じ）");
        assert_eq!(
            client
                .set_read_timeout(Some(Duration::ZERO))
                .map_err(|e| e.kind()),
            Err(io::ErrorKind::InvalidInput),
            "0 は std の set_read_timeout と同じく InvalidInput"
        );
        assert_eq!(client.read_timeout(), None, "断った値は付けない");
        assert_eq!(wait_ms(Some(Duration::from_micros(1))), 1, "端数は切り上げ");
        assert_eq!(wait_ms(None), INFINITE, "上限なしは無限に待つ");
    }
}
