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
//!
//! Windows（#524）は tty の概念が無いため、**PTY 直下の子プロセスの子孫**で
//! 「ペイン配下」を判定する。ポートの列挙は `GetExtendedTcpTable`、プロセスの
//! 親子関係と名前は Toolhelp32 スナップショット（いずれも `platform::procinfo`）。
//! Linux は未対応で空を返す。
//!
//! **ペインを指すキーはプラットフォームで実体が違う**（macOS = 制御端末の rdev、
//! Windows = 子 pid）。呼び出し側に `cfg` を書かせないため、キーの作成は
//! `pane_key()` に閉じ込め、以後は不透明な `u64` として扱う。

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

/// Windows に tty の概念は無い（ペインの特定は `pane_key` が子 pid で行う）
#[cfg(not(unix))]
pub fn tty_rdev(_tty_name: &str) -> Option<u64> {
    None
}

/// ペイン配下を指すスキャンキーを作る（#524）。
///
/// 中身の意味はプラットフォームで違い、**呼び出し側は解釈してはいけない**:
/// macOS は制御端末の rdev、Windows は PTY 直下の子プロセスの pid。
/// `scan()` へ渡すキーはここでしか作らない。
///
/// どちらの材料も取れなければ `None`（そのペインはスキャン対象から外れるだけ）
pub fn pane_key(tty_name: Option<&str>, child_pid: Option<u32>) -> Option<u64> {
    // 実装選択は `let` への cfg で行う。`return` で書くと、有効な分岐が末尾に
    // 来ないぶん clippy の `needless_return` に当たる（macOS の `-D warnings` が落ちる）
    #[cfg(target_os = "macos")]
    let key = {
        let _ = child_pid;
        tty_name.and_then(tty_rdev)
    };
    #[cfg(windows)]
    let key = {
        let _ = tty_name;
        // pid 0 は System Idle Process。ペインの子として返ることは無いが、
        // 万一 0 が来たら全プロセスを配下と誤認しかねないので弾く
        child_pid.filter(|&pid| pid != 0).map(u64::from)
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let key = {
        let _ = (tty_name, child_pid);
        None
    };
    key
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

/// Windows 版（#524）。キーは PTY 直下の子 pid で、その**子孫**が LISTEN している
/// TCP ポートを集める。プロセス一覧と TCP テーブルはキーの数によらず 1 回ずつしか
/// 取らない（3 秒毎に走るため、ペイン数に比例して重くしない）
#[cfg(windows)]
pub fn scan(keys: &[u64]) -> HashMap<u64, Vec<ListenPort>> {
    if keys.is_empty() {
        return HashMap::new();
    }
    let procs = crate::platform::procinfo::snapshot();
    let listeners = crate::platform::procinfo::tcp_listeners();
    group_by_root(&procs, &listeners, keys)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn scan(_keys: &[u64]) -> HashMap<u64, Vec<ListenPort>> {
    HashMap::new()
}

/// スキャン結果の組み立て（純粋関数）。プロセス一覧・LISTEN 一覧・キー群から
/// キー → ポート一覧を作る。**Windows 実機が無くてもテストできる**ように
/// `cfg` の外に出してある（`platform::locale` の parse 関数と同じ方針）
#[cfg_attr(not(windows), allow(dead_code))]
fn group_by_root(
    procs: &[crate::platform::procinfo::ProcEntry],
    listeners: &[crate::platform::procinfo::TcpListenEntry],
    roots: &[u64],
) -> HashMap<u64, Vec<ListenPort>> {
    let names: HashMap<u32, &str> = procs.iter().map(|p| (p.pid, p.name.as_str())).collect();
    let mut result: HashMap<u64, Vec<ListenPort>> = HashMap::new();
    for &root in roots {
        let Ok(root_pid) = u32::try_from(root) else {
            continue;
        };
        let family = crate::platform::procinfo::descendants_of(procs, root_pid);
        let mut ports: Vec<ListenPort> = listeners
            .iter()
            .filter(|e| family.contains(&e.pid))
            .map(|e| ListenPort {
                port: e.port,
                // 実在の Windows pid は i32 に収まる（4 の倍数で 2^31 未満）
                pid: e.pid as i32,
                process: names.get(&e.pid).copied().unwrap_or_default().to_string(),
            })
            .collect();
        if ports.is_empty() {
            continue;
        }
        // macOS 実装と同じ整形: ポート昇順・同一ポートは 1 件（IPv4 / IPv6 の重複を畳む）
        ports.sort_by_key(|p| p.port);
        ports.dedup_by_key(|p| p.port);
        result.insert(root, ports);
    }
    result
}

/// 1 プロセスの LISTEN 中 TCP ポートを列挙する（IPv4 / IPv6。ポートで重複排除）。
///
/// 抽象境界 B5 の検査 API。`scan` は複数ペインを一括で見るためこれを通らない
/// 経路（Windows）もあるが、単一プロセスを調べる入口としては両プラットフォームで
/// 同じ意味を持つ
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

/// Windows 版（#524）。`pid` **自身**が LISTEN している TCP ポートを列挙する
/// （子孫は含めない。macOS 版と同じ粒度）
#[cfg(windows)]
pub fn listening_ports_of_pid(pid: i32) -> Vec<ListenPort> {
    let Ok(pid) = u32::try_from(pid) else {
        return Vec::new();
    };
    let listeners = crate::platform::procinfo::tcp_listeners();
    let mut ports: Vec<u16> = listeners
        .iter()
        .filter(|e| e.pid == pid)
        .map(|e| e.port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    if ports.is_empty() {
        return Vec::new();
    }
    let name = crate::platform::procinfo::snapshot()
        .into_iter()
        .find(|p| p.pid == pid)
        .map(|p| p.name)
        .unwrap_or_default();
    ports
        .into_iter()
        .map(|port| ListenPort {
            port,
            pid: pid as i32,
            process: name.clone(),
        })
        .collect()
}

/// 検査手段を持たないプラットフォーム（Linux）
#[cfg(not(any(target_os = "macos", windows)))]
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

/// 自分以外の `tako-app` プロセスが生きているか（二重起動ガード用）
#[cfg(target_os = "macos")]
pub fn other_tako_running() -> bool {
    let my_pid = std::process::id() as i32;
    for pid in all_pids() {
        if pid == my_pid {
            continue;
        }
        if process_name(pid) == "tako-app" {
            return true;
        }
    }
    false
}

#[cfg(not(target_os = "macos"))]
pub fn other_tako_running() -> bool {
    false
}

/// `pid` が生きている `tako-app` プロセスか（多重起動ガード用。Issue #113）。
/// 死んだ pid は `proc_name` が失敗して空文字になるため生存確認を兼ねる。
/// pid 再利用（別プロセスに割り当て直し）はプロセス名の不一致で除外される
#[cfg(target_os = "macos")]
pub fn is_live_tako_app(pid: u32) -> bool {
    pid <= i32::MAX as u32 && process_name(pid as i32) == "tako-app"
}

/// 非 macOS はプロセス名の取得手段が未整備のため常に false
/// （多重起動ガードは効かない = 従来挙動。Phase 6 の Windows 対応で実装する）
#[cfg(not(target_os = "macos"))]
pub fn is_live_tako_app(_pid: u32) -> bool {
    false
}

/// プラットフォームを問わず走るテスト（純粋関数 + 実機の検知 e2e）
#[cfg(test)]
mod portable_tests {
    use super::*;
    use crate::platform::procinfo::{ProcEntry, TcpListenEntry};

    fn proc(pid: u32, ppid: u32, name: &str) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            name: name.to_string(),
        }
    }

    fn listener(pid: u32, port: u16) -> TcpListenEntry {
        TcpListenEntry { port, pid }
    }

    #[test]
    fn 子孫のlistenだけがそのペインのポートになる() {
        let procs = vec![
            proc(100, 1, "pwsh.exe"),
            proc(200, 100, "node.exe"),
            proc(300, 1, "other.exe"),
        ];
        let listeners = vec![listener(200, 5173), listener(300, 8080)];
        let grouped = group_by_root(&procs, &listeners, &[100]);
        let ports = grouped.get(&100).expect("ペイン 100 のポートが無い");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 5173);
        assert_eq!(ports[0].pid, 200);
        assert_eq!(ports[0].process, "node.exe");
        assert!(
            !grouped.contains_key(&300),
            "キーに渡していないプロセスは結果に出ない"
        );
    }

    #[test]
    fn 同一ポートのipv4とipv6は1件に畳まれポート昇順に並ぶ() {
        let procs = vec![proc(100, 1, "pwsh.exe"), proc(200, 100, "node.exe")];
        // IPv4 / IPv6 の両待ち受け（同じポートが 2 行で返る）+ 別ポート
        let listeners = vec![
            listener(200, 8080),
            listener(200, 3000),
            listener(200, 8080),
        ];
        let ports = group_by_root(&procs, &listeners, &[100]);
        let ports = &ports[&100];
        assert_eq!(
            ports.iter().map(|p| p.port).collect::<Vec<_>>(),
            vec![3000, 8080]
        );
    }

    #[test]
    fn listenが無いペインは結果に現れない() {
        let procs = vec![proc(100, 1, "pwsh.exe")];
        assert!(group_by_root(&procs, &[], &[100]).is_empty());
    }

    #[test]
    fn 既に終了したpidをキーにしても壊れない() {
        // スキャンとペイン終了のレース。空の結果になるだけで panic しない
        let procs = vec![proc(100, 1, "pwsh.exe")];
        let listeners = vec![listener(100, 5173)];
        assert!(group_by_root(&procs, &listeners, &[999_999]).is_empty());
    }

    #[test]
    fn u32に収まらないキーは無視する() {
        // macOS の rdev をそのまま渡されても panic しない（キーは不透明 u64）
        let procs = vec![proc(100, 1, "pwsh.exe")];
        let listeners = vec![listener(100, 5173)];
        assert!(group_by_root(&procs, &listeners, &[u64::MAX]).is_empty());
    }

    #[test]
    fn プロセス名が取れなくてもポートは返す() {
        // 権限不足などで名前だけ取れない場合（macOS 実装も空文字にする）
        let listeners = vec![listener(200, 5173)];
        let procs = vec![proc(100, 1, "pwsh.exe"), proc(200, 100, "")];
        let ports = group_by_root(&procs, &listeners, &[100]);
        assert_eq!(ports[&100][0].process, "");
    }

    #[test]
    fn キーが空ならスキャンしない() {
        assert!(scan(&[]).is_empty());
    }

    #[test]
    fn pane_keyは材料が無ければnone() {
        assert!(pane_key(None, None).is_none());
        // 子 pid 0（System Idle Process）は配下判定の材料にしない
        assert!(pane_key(None, Some(0)).is_none());
    }

    /// 実機の検知 e2e。**自分で listen して自分で検知する**ので、
    /// 構造体レイアウトの転記ミス・バイトオーダーの誤りはここで露見する
    #[cfg(windows)]
    #[test]
    fn 自プロセスのlistenポートを検知できる() {
        let l4 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p4 = l4.local_addr().unwrap().port();
        let l6 = std::net::TcpListener::bind("[::1]:0").unwrap();
        let p6 = l6.local_addr().unwrap().port();
        let me = std::process::id() as i32;

        let found = listening_ports_of_pid(me);
        assert!(
            found.iter().any(|p| p.port == p4),
            "IPv4 の listen ポート {p4} が検知されない（検知結果: {found:?}）"
        );
        assert!(
            found.iter().any(|p| p.port == p6),
            "IPv6 の listen ポート {p6} が検知されない（検知結果: {found:?}）"
        );
        assert!(found
            .iter()
            .all(|p| p.process.to_ascii_lowercase().ends_with(".exe")));

        drop(l4);
        drop(l6);
        let found = listening_ports_of_pid(me);
        assert!(
            !found.iter().any(|p| p.port == p4 || p.port == p6),
            "閉じたポートが残っている: {found:?}"
        );
    }

    /// 接続済み（ESTABLISHED）は LISTEN ではないので出てこない
    #[cfg(windows)]
    #[test]
    fn 接続済みソケットはlistenとして検知しない() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (_server, _) = l.accept().unwrap();
        let client_port = client.local_addr().unwrap().port();
        let me = std::process::id() as i32;
        let found: Vec<u16> = listening_ports_of_pid(me).iter().map(|p| p.port).collect();
        assert!(found.contains(&port), "listen 側は検知される");
        assert!(
            !found.contains(&client_port),
            "接続済みクライアント側は検知されない"
        );
    }

    /// `scan` の実機経路（子孫の判定 + テーブル取得）を自プロセスで通す
    #[cfg(windows)]
    #[test]
    fn scanは自プロセスをrootにすると自分のlistenを拾う() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let key = pane_key(None, Some(std::process::id())).expect("自 pid がキーになる");
        let scanned = scan(&[key]);
        let ports = scanned.get(&key).expect("自プロセスのポートが取れない");
        let mine = ports
            .iter()
            .find(|p| p.port == port)
            .unwrap_or_else(|| panic!("listen 中の {port} が scan で拾えない: {ports:?}"));
        // キーの持ち主に正しく紐付いていること（別プロセスのポートを混ぜていない）。
        // 同じテストバイナリの子プロセスが listen していれば結果に混ざりうるので、
        // 「全件が自分」ではなく「自分のポートが自分の pid で返る」を検査する
        assert_eq!(mine.pid, std::process::id() as i32);
    }

    /// 存在しない pid・権限の無いシステムプロセスを対象にしても panic しない
    #[cfg(windows)]
    #[test]
    fn 存在しないpidやシステムプロセスでも壊れない() {
        assert!(listening_ports_of_pid(0x7fff_fff0).is_empty());
        assert!(listening_ports_of_pid(-1).is_empty());
        // pid 4 = System。LSASS 等の LISTEN はこの pid の配下ではないので空、
        // かつ OpenProcess を使わないので権限エラーにもならない
        let _ = listening_ports_of_pid(4);
        assert!(scan(&[0x7fff_fff0]).is_empty());
    }
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

    /// Issue #113 多重起動ガードの生存判定: 死んだ pid と tako-app でないプロセス
    /// （このテストランナー自身）はどちらも false（誤ってセカンダリ化させない）
    #[test]
    fn is_live_tako_appは死んだpidや別名プロセスをfalseにする() {
        assert!(!is_live_tako_app(0x7fff_fff0), "存在しない pid は false");
        assert!(
            !is_live_tako_app(std::process::id()),
            "テストランナー（プロセス名が tako-app でない）は false"
        );
    }
}
