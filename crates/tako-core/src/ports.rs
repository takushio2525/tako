//! ports — listen ポート検知（FR-2.4.2。Layer 3 パッシブ検知の素材）
//!
//! ペイン配下のプロセスが LISTEN している TCP ポートを列挙する。
//! macOS は libproc（`proc_listpids` / `proc_pidinfo` / `proc_pidfdinfo`）で、
//! 「ペイン配下」は**制御端末（tty）の一致**で判定する（PTY スレーブの rdev と
//! `proc_bsdinfo.e_tdev` の突き合わせ。プロセスツリー走査より単純で、ジョブ全体を拾える）。
//!
//! libc クレートに無い `socket_fdinfo` 系は SDK の `sys/proc_info.h` から転記した
//! `#[repr(C)]` 定義を使う（カーネル ABI のため変更されない前提。転記ミスは
//! 自プロセスで実際に listen して検知するユニットテストで捕まえる）。
//! Linux / Windows は未対応で空を返す（Windows は Phase 6 で GetExtendedTcpTable）。

use std::collections::HashMap;

/// 検知した listen ポート（提案チップ FR-2.4.3 と list 公開 FR-2.5.1 の素材）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenPort {
    pub port: u16,
    pub pid: i32,
    /// プロセス名（`proc_name`。取得できなければ空文字）
    pub process: String,
}

/// tty デバイス名（`/dev/ttysNNN`）→ rdev。tty とプロセスの突き合わせキーに使う
#[cfg(unix)]
pub fn tty_rdev(tty_name: &str) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(tty_name).ok().map(|m| m.rdev())
}

/// Windows に tty の概念は無い（ConPTY の対応付けは Phase 6 で別途設計する）
#[cfg(not(unix))]
pub fn tty_rdev(_tty_name: &str) -> Option<u64> {
    None
}

/// 指定した tty（rdev）群に属するプロセスの listen ポートを一括スキャンする。
/// 戻り値は rdev → ポート一覧（ポート番号で重複排除・昇順）。
/// 取得に失敗したプロセスは黙って飛ばす（権限・レース起因の失敗は正常系）
#[cfg(target_os = "macos")]
pub fn scan(ttys: &[u64]) -> HashMap<u64, Vec<ListenPort>> {
    let mut result: HashMap<u64, Vec<ListenPort>> = HashMap::new();
    if ttys.is_empty() {
        return result;
    }
    for pid in all_pids() {
        let Some(info) = bsd_info(pid) else { continue };
        let tdev = info.e_tdev as u64;
        if !ttys.contains(&tdev) {
            continue;
        }
        let ports = listening_ports_of_pid(pid);
        if !ports.is_empty() {
            result.entry(tdev).or_default().extend(ports);
        }
    }
    for ports in result.values_mut() {
        ports.sort_by_key(|p| p.port);
        ports.dedup_by_key(|p| p.port);
    }
    result
}

#[cfg(not(target_os = "macos"))]
pub fn scan(_ttys: &[u64]) -> HashMap<u64, Vec<ListenPort>> {
    HashMap::new()
}

/// 1 プロセスの LISTEN 中 TCP ポートを列挙する（IPv4 / IPv6。ポートで重複排除）
#[cfg(target_os = "macos")]
pub fn listening_ports_of_pid(pid: i32) -> Vec<ListenPort> {
    let mut ports: Vec<u16> = socket_fds(pid)
        .into_iter()
        .filter_map(|fd| listen_port_of_fd(pid, fd))
        .collect();
    ports.sort_unstable();
    ports.dedup();
    if ports.is_empty() {
        return Vec::new();
    }
    let name = process_name(pid);
    ports
        .into_iter()
        .map(|port| ListenPort {
            port,
            pid,
            process: name.clone(),
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn listening_ports_of_pid(_pid: i32) -> Vec<ListenPort> {
    Vec::new()
}

#[cfg(target_os = "macos")]
mod macos {
    //! `sys/proc_info.h` からの転記（libc クレートに無い socket_fdinfo 系のみ）。
    //! 取り出すフィールドは soi_kind / tcpsi_state / insi_lport だけだが、
    //! オフセットを正しく出すために手前のフィールドをすべて写している

    /// `PROC_PIDFDSOCKETINFO`（proc_pidfdinfo の flavor）
    pub const PROC_PIDFDSOCKETINFO: libc::c_int = 3;
    /// `SOCKINFO_TCP`（socket_info.soi_kind）
    pub const SOCKINFO_TCP: i32 = 2;
    /// `TSI_S_LISTEN`（tcp_sockinfo.tcpsi_state）
    pub const TSI_S_LISTEN: i32 = 1;

    #[repr(C)]
    pub struct VinfoStat {
        pub vst_dev: u32,
        pub vst_mode: u16,
        pub vst_nlink: u16,
        pub vst_ino: u64,
        pub vst_uid: u32,
        pub vst_gid: u32,
        pub vst_atime: i64,
        pub vst_atimensec: i64,
        pub vst_mtime: i64,
        pub vst_mtimensec: i64,
        pub vst_ctime: i64,
        pub vst_ctimensec: i64,
        pub vst_birthtime: i64,
        pub vst_birthtimensec: i64,
        pub vst_size: i64,
        pub vst_blocks: i64,
        pub vst_blksize: i32,
        pub vst_flags: u32,
        pub vst_gen: u32,
        pub vst_rdev: u32,
        pub vst_qspare: [i64; 2],
    }

    #[repr(C)]
    pub struct SockbufInfo {
        pub sbi_cc: u32,
        pub sbi_hiwat: u32,
        pub sbi_mbcnt: u32,
        pub sbi_mbmax: u32,
        pub sbi_lowat: u32,
        pub sbi_flags: i16,
        pub sbi_timeo: i16,
    }

    #[repr(C)]
    pub struct InSockinfo {
        pub insi_fport: i32,
        pub insi_lport: i32,
        pub insi_gencnt: u64,
        pub insi_flags: u32,
        pub insi_flow: u32,
        pub insi_vflag: u8,
        pub insi_ip_ttl: u8,
        pub rfu_1: u32,
        /// in4in6_addr / in6_addr の union（中身は使わないためバイト列で確保）
        pub insi_faddr: [u32; 4],
        pub insi_laddr: [u32; 4],
        pub insi_v4_tos: u8,
        pub insi_v6_hlim: u8,
        pub insi_v6_cksum: i32,
        pub insi_v6_ifindex: u16,
        pub insi_v6_hops: i16,
    }

    #[repr(C)]
    pub struct TcpSockinfo {
        pub tcpsi_ini: InSockinfo,
        pub tcpsi_state: i32,
        pub tcpsi_timer: [i32; 4],
        pub tcpsi_mss: i32,
        pub tcpsi_flags: u32,
        pub rfu_1: u32,
        pub tcpsi_tp: u64,
    }

    #[repr(C)]
    pub struct ProcFileinfo {
        pub fi_openflags: u32,
        pub fi_status: u32,
        pub fi_offset: i64,
        pub fi_type: i32,
        pub fi_guardflags: u32,
    }

    /// socket_info の先頭〜TCP 部分（union soi_proto は最大メンバではなく
    /// 読みたい pri_tcp で代表させる。バッファ自体は余裕を持って渡す）
    #[repr(C)]
    pub struct SocketInfoPrefix {
        pub soi_stat: VinfoStat,
        pub soi_so: u64,
        pub soi_pcb: u64,
        pub soi_type: i32,
        pub soi_protocol: i32,
        pub soi_family: i32,
        pub soi_options: i16,
        pub soi_linger: i16,
        pub soi_state: i16,
        pub soi_qlen: i16,
        pub soi_incqlen: i16,
        pub soi_qlimit: i16,
        pub soi_timeo: i16,
        pub soi_error: u16,
        pub soi_oobmark: u32,
        pub soi_rcv: SockbufInfo,
        pub soi_snd: SockbufInfo,
        pub soi_kind: i32,
        pub rfu_1: u32,
        pub pri_tcp: TcpSockinfo,
    }

    #[repr(C)]
    pub struct SocketFdinfoPrefix {
        pub pfi: ProcFileinfo,
        pub psi: SocketInfoPrefix,
    }
}

/// 全プロセスの pid 一覧（`proc_listpids`）。サイズ不足に備えて 1 回だけ拡張再試行する
#[cfg(target_os = "macos")]
fn all_pids() -> Vec<i32> {
    const PROC_ALL_PIDS: u32 = 1;
    let mut capacity = 4096usize;
    for _ in 0..2 {
        let mut pids = vec![0i32; capacity];
        let bytes = unsafe {
            libc::proc_listpids(
                PROC_ALL_PIDS,
                0,
                pids.as_mut_ptr().cast(),
                (pids.len() * size_of::<i32>()) as libc::c_int,
            )
        };
        if bytes <= 0 {
            return Vec::new();
        }
        let count = bytes as usize / size_of::<i32>();
        if count < pids.len() {
            pids.truncate(count);
            pids.retain(|&p| p > 0);
            return pids;
        }
        capacity *= 4; // バッファが埋まった = 取りこぼしの可能性 → 広げて取り直す
    }
    Vec::new()
}

/// プロセスの BSD 情報（制御端末 e_tdev の取得に使う）
#[cfg(target_os = "macos")]
fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (written == size).then_some(info)
}

/// プロセスが開いているソケット fd の一覧
#[cfg(target_os = "macos")]
fn socket_fds(pid: i32) -> Vec<i32> {
    let mut capacity = 256usize;
    for _ in 0..2 {
        let mut fds = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0,
            };
            capacity
        ];
        let bytes = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                (fds.len() * size_of::<libc::proc_fdinfo>()) as libc::c_int,
            )
        };
        if bytes <= 0 {
            return Vec::new();
        }
        let count = bytes as usize / size_of::<libc::proc_fdinfo>();
        if count < fds.len() {
            return fds[..count]
                .iter()
                .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
                .map(|fd| fd.proc_fd)
                .collect();
        }
        capacity *= 4;
    }
    Vec::new()
}

/// ソケット fd が LISTEN 中の TCP（IPv4 / IPv6）ならローカルポートを返す
#[cfg(target_os = "macos")]
fn listen_port_of_fd(pid: i32, fd: i32) -> Option<u16> {
    use macos::*;
    // カーネル側 socket_fdinfo の実サイズ（≈ 800 バイト）より十分大きいバッファに
    // 受け、先頭を転記済みプレフィクス構造体として解釈する
    let mut buffer = [0u8; 2048];
    let written = unsafe {
        libc::proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDSOCKETINFO,
            buffer.as_mut_ptr().cast(),
            buffer.len() as libc::c_int,
        )
    };
    if (written as usize) < size_of::<SocketFdinfoPrefix>() {
        return None;
    }
    // バッファは align 1 のため参照ではなく非アラインメント読みで取り出す（POD のみ）
    let info: SocketFdinfoPrefix = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast()) };
    if info.psi.soi_kind != SOCKINFO_TCP || info.psi.pri_tcp.tcpsi_state != TSI_S_LISTEN {
        return None;
    }
    // insi_lport はネットワークバイトオーダーの 16bit が int に入っている
    let port = u16::from_be((info.psi.pri_tcp.tcpsi_ini.insi_lport as u32 & 0xffff) as u16);
    (port != 0).then_some(port)
}

/// プロセス名（`proc_name`。最大 32 文字側の短い名前で十分）
#[cfg(target_os = "macos")]
fn process_name(pid: i32) -> String {
    let mut buf = [0u8; 64];
    let len = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..len as usize]).into_owned()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// 転記した socket_fdinfo レイアウトの e2e 検証: 実際に listen して自プロセスから検知する。
    /// オフセットのずれ・バイトオーダーの誤りはここで露見する
    #[test]
    fn 自プロセスのlistenポートを検知できる() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = std::process::id() as i32;
        let found = listening_ports_of_pid(me);
        assert!(
            found.iter().any(|p| p.port == port),
            "自分の listen ポート {port} が検知されること（検知結果: {found:?}）"
        );
        // 閉じると消える
        drop(listener);
        let found = listening_ports_of_pid(me);
        assert!(!found.iter().any(|p| p.port == port));
    }

    #[test]
    fn 接続済みソケットはlistenとして検知しない() {
        // LISTEN 状態のソケットだけが返ること: 接続ペアのクライアント側
        // エフェメラルポートは検知結果に現れない（他テストと並列でも壊れない判定）
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = std::process::id() as i32;
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (_server, _) = listener.accept().unwrap();
        let client_port = client.local_addr().unwrap().port();
        let found: Vec<u16> = listening_ports_of_pid(me).iter().map(|p| p.port).collect();
        assert!(found.contains(&port), "listen 側は検知される");
        assert!(
            !found.contains(&client_port),
            "接続済みクライアント側は検知されない"
        );
    }

    #[test]
    fn 存在しないpidや空のtty指定は空を返す() {
        assert!(listening_ports_of_pid(0x7fff_fff0).is_empty());
        assert!(scan(&[]).is_empty());
        assert!(tty_rdev("/dev/no-such-tty").is_none());
    }
}
