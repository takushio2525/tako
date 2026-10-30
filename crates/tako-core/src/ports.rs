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
///
/// ## 器（psmux）自身の LISTEN を落とす（#724）
///
/// psmux はクライアント↔サーバー間の IPC に **TCP ループバック**を使い、
/// サーバーを**クライアントの子プロセス**として 1 セッションに 1 個起動する。
/// クライアント = ペインの PTY 直下の子（`pane_key` の材料）なので、
/// 器つきのペインは**例外なく** 1 個の偽 listen ポートを持つことになる。
///
/// そこで所有プロセスが器そのものなら結果から落とす。**子孫の走査自体は止めない**
/// （器の下で動いている本物の dev サーバーは引き続き拾いたいため）
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
            // 名前が取れないものは器と断定できないので落とさない（従来どおり報告する）
            .filter(|e| {
                !names
                    .get(&e.pid)
                    .is_some_and(|name| crate::backend::is_plumbing_process(name))
            })
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

/// 自分以外の生きた `tako-app` プロセスの pid（二重起動ガード + #1187 の所有者判定）。
/// **見送りの理由に pid を出す**ため、bool ではなく一覧を返すのが正
#[cfg(target_os = "macos")]
pub fn other_tako_pids() -> Vec<u32> {
    let my_pid = std::process::id() as i32;
    all_pids()
        .into_iter()
        .filter(|&pid| pid != my_pid && pid > 0 && process_name(pid) == "tako-app")
        .map(|pid| pid as u32)
        .collect()
}

/// 非 macOS はプロセス名の取得手段が未整備のため空（多重起動ガードは効かない = 従来挙動）
#[cfg(not(target_os = "macos"))]
pub fn other_tako_pids() -> Vec<u32> {
    Vec::new()
}

/// 自分以外の `tako-app` プロセスが生きているか（二重起動ガード用）
pub fn other_tako_running() -> bool {
    !other_tako_pids().is_empty()
}

/// プロセスが生きているか（#1192 の所有者判定）。`kill(pid, 0)` はシグナルを送らず
/// 存在と権限だけを見る（EPERM = 別ユーザーの生存プロセス = 生きている）。
///
/// **「生きていない」と言い切れるときだけ false**（回収の可否を分ける材料なので、
/// 迷ったら生きている側に倒す）
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        // 0 はプロセスグループ指定になるため所有者判定には使わない（= 触らない）
        return true;
    }
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Windows には tmux のソケットレイアウトが無く、回収の対象も存在しない。
/// 「生きている」を返して**何も回収しない**側へ倒す
#[cfg(not(unix))]
pub fn process_alive(_pid: u32) -> bool {
    true
}

/// 別プロセスの**初期環境変数**のうち `names` に挙げたものを読む（#1187）。
///
/// 用途は「相手の tako-app がどの tmux ソケットを使っているか」の特定だけなので、
/// 環境全体は返さず**要求されたキーだけ**を返す（`TAKO_TOKEN` 等を不用意に持ち回らない。
/// `conventions.md`「ログに書かない」の趣旨）。
///
/// macOS は `sysctl(KERN_PROCARGS2)`。これは **exec 時点**の環境なので、相手が
/// 起動後に `setenv` した値は見えない（tako-app の一括隔離は exec 後に行うため、
/// 呼び出し側は [`crate::tmux_cleanup::resolve_socket_name`] で同じ導出をやり直す）。
/// 読めなければ `None`（= 呼び出し側は安全側に倒す）
#[cfg(target_os = "macos")]
pub fn process_env_vars(pid: u32, names: &[&str]) -> Option<HashMap<String, String>> {
    let raw = procargs2(pid)?;
    Some(pick_env(&parse_procargs2(&raw), names))
}

#[cfg(not(target_os = "macos"))]
pub fn process_env_vars(_pid: u32, _names: &[&str]) -> Option<HashMap<String, String>> {
    None
}

/// `KERN_PROCARGS2` の生バイト列（argc + exec path + argv + env）
#[cfg(target_os = "macos")]
fn procargs2(pid: u32) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; kern_argmax()?];
    let len = procargs2_into(pid, &mut buf)?;
    buf.truncate(len);
    Some(buf)
}

/// `kern.argmax`（`KERN_PROCARGS2` のバッファ長。環境ごとに違う。既定 1 MB 前後）
#[cfg(target_os = "macos")]
fn kern_argmax() -> Option<usize> {
    let mut argmax: libc::c_int = 0;
    let mut size = size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            (&mut argmax as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok == 0 && argmax > 0).then_some(argmax as usize)
}

/// `KERN_PROCARGS2` を `buf`（長さ = `kern.argmax`）へ読み、書かれたバイト数を返す。
/// 全プロセスぶん引く [`process_table`] が 1 枚のバッファを使い回すための形
#[cfg(target_os = "macos")]
fn procargs2_into(pid: u32, buf: &mut [u8]) -> Option<usize> {
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    let mut len = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    // 別ユーザーのプロセス・既に死んだ pid は EPERM / EINVAL で失敗する（= 不明）
    (ok == 0 && len >= size_of::<u32>()).then_some(len)
}

/// 全プロセスの親子関係（pid → ppid）とコマンド行（pid → argv を空白で繋いだもの）を
/// **子プロセスを起こさずに**引く（#1979。`ps -axo pid=,ppid=,command=` の置き換え）。
///
/// ## なぜ `ps` をやめるか
///
/// 照会のたびに `ps` を起こしていた（worker の状態照会・stale binary 検知・ssh の宛先の
/// 自動検知が 2 秒 tick と監視ごとに）。さらに `Command::output()` は子が終わらないと
/// **永久に返らない**。2026-10-10 の本番ハングは、メインスレッドがこの `ps` の
/// `output()` の `poll` で 10 分止まったもの（sample のスタックで特定）。libproc なら
/// 子プロセスもパイプも無いので、その形の待ちがそもそも生まれない。
///
/// ## `ps` との違い（実測で揃えた）
///
/// - コマンド行は `KERN_PROCARGS2` の argv を空白で繋ぐ（`ps` の `command` 列と同じ材料）
/// - 印字できない文字（改行・タブ・制御文字・ゼロ幅文字）は**そのまま**返す（不正な UTF-8 は
///   U+FFFD）。`ps` は表示用に `\012` / `^A` などへ表記し直すので、複数行の `sh -c` の本文の
///   ような argv は表記だけが違う（実測 2026-10-16・#2193）。読み手が見る第 1 語（codex / agy /
///   stale binary）と ssh 本体の引数には出ない
/// - ゾンビは載らない（`PROC_PIDT_SHORTBSDINFO` が 0 バイトで返る。`ps` は `<defunct>` で出す）
/// - 読めないプロセスは `ps` の流儀で `(名前)` を入れる。**別ユーザー（root）のプロセスの
///   コマンド行は読めない**: `/bin/ps` は Apple の特権付きバイナリなので読めるが、tako からの
///   `KERN_PROCARGS2` は拒まれる（実測 2026-10-10: 自分のプロセスは 791 件中 789 件が `ps` と
///   一致し、ずれた 2 件は採取の間に argv が変わったもの。root の約 320 件は `(名前)`）。
///   読み手（ssh の宛先・agent の判定・stale binary）が見るのはペインの子孫 = 自分の
///   プロセスなので困らない。`sudo ssh` のように**ペインの中で root になった子**の宛先だけは
///   読めなくなる
/// - 親子は `proc_pidinfo(PROC_PIDT_SHORTBSDINFO)` の `pbsi_ppid`。**`PROC_PIDTBSDINFO` では
///   ない**: あちらは別ユーザー（root）のプロセスで失敗し、実測で 1,111 件中 787 件しか
///   取れなかった（`sudo` 配下の子が「子なし」に化ける）。短いほうは 1,107 件取れる
///
/// 列挙できなければ `None`（呼び手は上限つきの `ps` へ落ちる）
#[cfg(target_os = "macos")]
pub fn process_table() -> Option<(HashMap<u32, u32>, HashMap<u32, String>)> {
    let pids = all_pids();
    if pids.is_empty() {
        return None;
    }
    let mut buf = vec![0u8; kern_argmax()?];
    let mut parents = HashMap::with_capacity(pids.len());
    let mut argv = HashMap::with_capacity(pids.len());
    for pid in pids {
        let Some(info) = bsd_short_info(pid) else {
            continue; // 列挙と問い合わせの間に終わった
        };
        let pid = pid as u32;
        parents.insert(pid, info.pbsi_ppid);
        let line = procargs2_into(pid, &mut buf)
            .and_then(|len| procargs2_argv(&buf[..len]))
            .unwrap_or_else(|| format!("({})", comm_of(&info.pbsi_comm)));
        argv.insert(pid, line);
    }
    Some((parents, argv))
}

#[cfg(not(target_os = "macos"))]
pub fn process_table() -> Option<(HashMap<u32, u32>, HashMap<u32, String>)> {
    None
}

/// 全プロセスの pid（境界 B29 `platform::resources` の走査が使う。#1993）
#[cfg(target_os = "macos")]
pub(crate) fn list_pids() -> Vec<i32> {
    all_pids()
}

/// 親 pid・uid・短い名前（`PROC_PIDT_SHORTBSDINFO`。別ユーザーのプロセスでも取れる。#1993）
#[cfg(target_os = "macos")]
pub(crate) fn short_info(pid: i32) -> Option<(u32, u32, String)> {
    let info = bsd_short_info(pid)?;
    Some((info.pbsi_ppid, info.pbsi_uid, comm_of(&info.pbsi_comm)))
}

/// `kern.argmax`（`KERN_PROCARGS2` の読み取りバッファの長さ。#1993）
#[cfg(target_os = "macos")]
pub(crate) fn procargs_buffer_len() -> Option<usize> {
    kern_argmax()
}

/// 1 回の `KERN_PROCARGS2` で argv（空白で繋いだもの）と env の指定キーを読む（#1993）。
/// `buf` は [`procargs_buffer_len`] の長さで使い回す。読めなければ `None`
/// （別ユーザーのプロセス・既に終わった pid）
#[cfg(target_os = "macos")]
pub(crate) fn argv_and_env(
    pid: u32,
    buf: &mut [u8],
    names: &[&str],
) -> Option<(Option<String>, HashMap<String, String>)> {
    let len = procargs2_into(pid, buf)?;
    let raw = &buf[..len];
    Some((procargs2_argv(raw), pick_env(&parse_procargs2(raw), names)))
}

/// `struct proc_bsdshortinfo`（`sys/proc_info.h`）。tako が使う libc 0.2.186 には無いので
/// 転記する（新しい libc には同じ形で入っている。ずれは下の大きさの assert と
/// 「ps と同じ答え」の単体テストで捕まえる）
#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcBsdShortInfo {
    pbsi_pid: u32,
    pbsi_ppid: u32,
    pbsi_pgid: u32,
    pbsi_status: u32,
    pbsi_comm: [libc::c_char; 16],
    pbsi_flags: u32,
    pbsi_uid: libc::uid_t,
    pbsi_gid: libc::gid_t,
    pbsi_ruid: libc::uid_t,
    pbsi_rgid: libc::gid_t,
    pbsi_svuid: libc::uid_t,
    pbsi_svgid: libc::gid_t,
    pbsi_rfu: u32,
}

#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<ProcBsdShortInfo>() == 64);

/// `PROC_PIDT_SHORTBSDINFO`（`sys/proc_info.h`）
#[cfg(target_os = "macos")]
const PROC_PIDT_SHORTBSDINFO: libc::c_int = 13;

/// 親 pid と短い名前（`PROC_PIDT_SHORTBSDINFO`）。**別ユーザーのプロセスでも取れる**
/// （[`bsd_info`] の `PROC_PIDTBSDINFO` は root のプロセスで失敗する。#1979 の実測）
#[cfg(target_os = "macos")]
fn bsd_short_info(pid: i32) -> Option<ProcBsdShortInfo> {
    // SAFETY: POD（整数と固定長配列だけ）なので全 0 は正しい値
    let mut info: ProcBsdShortInfo = unsafe { std::mem::zeroed() };
    let size = size_of::<ProcBsdShortInfo>() as libc::c_int;
    // SAFETY: 渡すバッファは size ぴったりのローカル変数で、
    // 書き込まれたバイト数が size と一致したときだけ中身を読む
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDT_SHORTBSDINFO,
            0,
            (&mut info as *mut ProcBsdShortInfo).cast(),
            size,
        )
    };
    (written == size).then_some(info)
}

/// 短い名前（`pbsi_comm`。NUL まで）
#[cfg(target_os = "macos")]
fn comm_of(comm: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = comm
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `KERN_PROCARGS2` から argv を取り出し、空白で繋ぐ（純粋関数。`ps` の `command` 列と同じ形）。
/// argv が 1 つも無い（argc = 0）ときは `None`
#[cfg(target_os = "macos")]
fn procargs2_argv(buf: &[u8]) -> Option<String> {
    if buf.len() < 4 {
        return None;
    }
    let argc = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let body = &buf[4..];
    // exec path（最初の NUL まで）と詰めの NUL を飛ばす
    let mut pos = body.iter().position(|&b| b == 0)? + 1;
    while body.get(pos) == Some(&0) {
        pos += 1;
    }
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        let rest = body.get(pos..)?;
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        args.push(String::from_utf8_lossy(&rest[..end]).into_owned());
        pos += end + 1;
        if pos > body.len() {
            break;
        }
    }
    (!args.is_empty()).then(|| args.join(" "))
}

/// argv[0] の名前部分（#2533。パスと引数は捨てる）。読めなければ `None`
/// （別ユーザー・root のプロセス・既に死んだ pid）。
///
/// 祖先の記録で、**起動したときの名前**を出すために使う。comm（`pbsi_comm`）は実体の
/// ファイル名なので、`~/.local/bin/claude` → `versions/2.1.301` のように版の数字で
/// 置かれた実行ファイルが `2.1.301` に見える（`ps` が `claude` と出すのは argv[0] を見るから）
#[cfg(target_os = "macos")]
pub(crate) fn argv0_name(pid: u32) -> Option<String> {
    procargs2_argv0_name(&procargs2(pid)?)
}

/// `KERN_PROCARGS2` から argv[0] の名前部分を取り出す（純粋関数）
#[cfg(target_os = "macos")]
fn procargs2_argv0_name(buf: &[u8]) -> Option<String> {
    if buf.len() < 4 || u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) == 0 {
        return None;
    }
    let body = &buf[4..];
    // exec path（最初の NUL まで）と詰めの NUL を飛ばす
    let mut pos = body.iter().position(|&b| b == 0)? + 1;
    while body.get(pos) == Some(&0) {
        pos += 1;
    }
    let rest = body.get(pos..)?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    let argv0 = String::from_utf8_lossy(&rest[..end]);
    let name = argv0.rsplit('/').next().unwrap_or_default().trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// `KERN_PROCARGS2` のレイアウトを解く（純粋関数）。
/// `[argc: u32][exec path\0][詰めの \0…][argv × argc][env…][\0]`
#[cfg(target_os = "macos")]
fn parse_procargs2(buf: &[u8]) -> Vec<(String, String)> {
    if buf.len() < 4 {
        return Vec::new();
    }
    let argc = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let body = &buf[4..];
    let mut pos = 0usize;
    // exec path（最初の NUL まで）
    let Some(end) = body.iter().position(|&b| b == 0) else {
        return Vec::new();
    };
    pos = pos.max(end + 1);
    // 詰めの NUL を飛ばす
    while body.get(pos) == Some(&0) {
        pos += 1;
    }
    // argv を argc 個読み飛ばす
    for _ in 0..argc {
        let Some(end) = body[pos..].iter().position(|&b| b == 0) else {
            return Vec::new();
        };
        pos += end + 1;
    }
    // 以降が env。空文字列（連続 NUL）か末尾で終わり
    let mut env = Vec::new();
    while pos < body.len() {
        let end = body[pos..]
            .iter()
            .position(|&b| b == 0)
            .map(|e| pos + e)
            .unwrap_or(body.len());
        if end == pos {
            break; // 空エントリ = env の終端
        }
        let entry = String::from_utf8_lossy(&body[pos..end]);
        if let Some((k, v)) = entry.split_once('=') {
            env.push((k.to_string(), v.to_string()));
        }
        pos = end + 1;
    }
    env
}

/// 読んだ env から要求されたキーだけを取り出す（純粋関数）
#[cfg(target_os = "macos")]
fn pick_env(env: &[(String, String)], names: &[&str]) -> HashMap<String, String> {
    env.iter()
        .filter(|(k, _)| names.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
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
    fn 器そのもののlistenは落とすが器の下のdevサーバーは拾う() {
        // psmux は「クライアントの子」としてサーバーを起こし、その IPC に
        // TCP ループバックを使う（#724）。ペインの PTY 直下の子 = クライアントなので、
        // 器つきの全ペインが必ず 1 個の偽 listen を持つ形になる
        let procs = vec![
            proc(100, 1, "psmux.exe"),   // ペイン直下の子（= キー）
            proc(200, 100, "psmux.exe"), // 器のサーバー。落としたい
            proc(300, 200, "node.exe"),  // 器の下で動く本物の dev サーバー
        ];
        let listeners = vec![listener(200, 63836), listener(300, 5173)];
        let ports = group_by_root(&procs, &listeners, &[100]);
        let ports = &ports[&100];
        assert_eq!(
            ports.iter().map(|p| p.port).collect::<Vec<_>>(),
            vec![5173],
            "器の内部ポートだけが落ち、子孫の走査は止まっていない"
        );
    }

    #[test]
    fn 器しかlistenしていないペインは結果に現れない() {
        // 症状①（全ペインにチップが出る）の直接の再現。落とした結果が空なら
        // そのペインはキーごと結果から消える（= チップが立たない）
        let procs = vec![proc(100, 1, "pwsh.exe"), proc(200, 100, "tmux.exe")];
        let listeners = vec![listener(200, 59000)];
        assert!(group_by_root(&procs, &listeners, &[100]).is_empty());
    }

    #[test]
    fn 名前が取れないlistenは器と断定できないので落とさない() {
        // 権限・レースで名前だけ取れないことがある。そこで落とすと本物を見落とす
        let procs = vec![proc(100, 1, "pwsh.exe")];
        let listeners = vec![listener(200, 5173)]; // pid 200 は procs に居ない
        let ports = group_by_root(&procs, &listeners, &[100]);
        assert!(ports.is_empty(), "そもそも子孫として見えないので対象外");
        let procs = vec![proc(100, 1, "pwsh.exe"), proc(200, 100, "")];
        let ports = group_by_root(&procs, &listeners, &[100]);
        assert_eq!(ports[&100].len(), 1, "名前が空でも報告する");
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

    /// `KERN_PROCARGS2` のレイアウト（argc → exec path → 詰め → argv → env）を
    /// 合成バッファで解けること。実バッファはプロセスごとに違うのでここは純粋関数で拘束する
    #[test]
    fn procargs2のレイアウトを解いてenvだけ取り出す() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2u32.to_ne_bytes()); // argc = 2
        buf.extend_from_slice(b"/path/to/tako-app\0");
        buf.extend_from_slice(b"\0\0"); // 詰め
        buf.extend_from_slice(b"tako-app\0"); // argv[0]
        buf.extend_from_slice(b"--flag\0"); // argv[1]
        buf.extend_from_slice(b"TAKO_TMUX_SOCKET=tako-iso-42\0");
        buf.extend_from_slice(b"PATH=/usr/bin:/bin\0");
        buf.extend_from_slice(b"TAKO_ISOLATED=1\0");
        buf.extend_from_slice(b"\0"); // env 終端
        let env = parse_procargs2(&buf);
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == "TAKO_TMUX_SOCKET")
                .map(|(_, v)| v.as_str()),
            Some("tako-iso-42")
        );
        // 要求したキーだけが返る（TAKO_TOKEN 等を不用意に持ち回らない）
        let picked = pick_env(&env, &["TAKO_ISOLATED"]);
        assert_eq!(picked.get("TAKO_ISOLATED").map(String::as_str), Some("1"));
        assert_eq!(picked.len(), 1);
        // argv を env と取り違えていない（`--flag` に = が無いので混ざれば消えるだけ。
        // 取り違えると argv 由来の値が env に載るので、件数で拘束する）
        assert_eq!(env.len(), 3);
    }

    /// 実プロセス（自分自身）の初期環境を読めること。読めなければ呼び出し側は
    /// 「不明 = 見送り」に倒すので、ここが動くことが #1187 のガード緩和の前提になる
    #[test]
    fn 自分の初期環境をsysctlで読める() {
        let env = process_env_vars(std::process::id(), &["PATH"])
            .expect("自分のプロセスの環境は読めるはず");
        assert!(
            env.get("PATH").is_some_and(|p| !p.is_empty()),
            "PATH が読めない: {env:?}"
        );
        // 死んだ pid は None（不明）
        assert!(process_env_vars(0x7fff_fff0, &["PATH"]).is_none());
        assert!(process_env_vars(0, &["PATH"]).is_none());
    }

    /// #1187: 見送りの理由に pid を出せること（bool では pid が出せない）
    #[test]
    fn other_tako_pidsは自分を含まない() {
        let pids = other_tako_pids();
        assert!(!pids.contains(&std::process::id()));
        assert_eq!(other_tako_running(), !pids.is_empty());
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

    /// #1979: libproc の親子表が `ps -axo pid=,ppid=,command=` と**同じ答え**を返す。
    ///
    /// 置き換えの前後で ssh の宛先検知（#976）・agent の判定（argv）・stale binary 検知が
    /// 黙って変わらないことの実測。親子は**全ユーザーぶん**、コマンド行は**自分の
    /// プロセス**を比べる（別ユーザーのものは特権付きの `/bin/ps` にしか読めない）。
    ///
    /// 比べ方は [`compare_with_ps`]（#2193）。libproc → `ps` → libproc の順に取り、
    /// 2 回の libproc が同じ答えのものは `ps` と**全件一致**を求める（許容の % は置かない）
    #[test]
    fn libprocの親子表はpsと同じ答えを返す() {
        let me = std::process::id();
        let mut before = process_table().expect("macOS は列挙できる");
        let mut ps = std::process::Command::new("/bin/ps");
        let out = crate::platform::process::no_console_window(&mut ps)
            .args(["-axo", "pid=,uid=,ppid=,command="])
            .output()
            .expect("ps");
        let mut after = process_table().expect("macOS は列挙できる");
        assert!(
            before.0.contains_key(&me) && after.0.contains_key(&me),
            "自分が載っていない"
        );
        let rows = parse_ps_rows(&String::from_utf8_lossy(&out.stdout));
        // SAFETY: getuid は引数を取らず必ず成功する
        let my_uid = unsafe { libc::getuid() };
        if let Ok(spec) = std::env::var(INJECT_ENV) {
            inject_2193(&spec, &mut before, &mut after, &rows, my_uid);
        }
        let cmp = compare_with_ps(&before, &rows, &after, my_uid);
        eprintln!(
            "共通 {} 件（うち採取の間に変わった {}）・自分のプロセスのコマンド行 {} 件\
             （うち ps が表記し直す文字を含み手前だけ比べた {}）・ずれ {} 件",
            cmp.common,
            cmp.moved,
            cmp.mine,
            cmp.rewritten,
            cmp.mismatches.len()
        );
        assert!(cmp.common > 10, "比べる材料が無い（ps が空?）");
        assert!(cmp.mine > 0, "自分のプロセスが 1 つも比べられていない");
        assert!(
            cmp.mismatches.is_empty(),
            "libproc の答えが ps とずれている（{} 件。先頭 10 件: {:?}）",
            cmp.mismatches.len(),
            // 切り出し安全: mismatches はずれの行の Vec
            &cmp.mismatches[..cmp.mismatches.len().min(10)]
        );
        // 2 回の libproc の間（数十 ms）に変わるのは exec した・親が死んだものだけで、
        // 実測は 1 回に 0〜1 件（`ps` の前へ 3 秒の遅れを入れても 0 件。2026-10-16）。
        // 1 割も揺れるなら、時刻ではなく libproc の答えが揺れている（比べずに見逃さない）
        assert!(
            cmp.moved * 10 < cmp.common,
            "採取の間に変わったものが多すぎる（{} / {} 件。libproc の答えが呼ぶたびに違う?）",
            cmp.moved,
            cmp.common
        );
    }

    /// #2193 の注入（検証専用）。`churn[:N]` は 1 回目の libproc の表だけを書き換え、
    /// 共通の pid の N 件に 1 件を「採取の間に変わった」形（exec 前・前の親が死ぬ前）にし、
    /// 別の N 件に 1 件ずつを「1 回目の後に生まれた」「`ps` の後に死んだ」にする。
    /// `regress[:N]` は 2 回の表の**両方**を同じに書き換える（親子表が本当にずれる回帰）。
    /// N の既定は 20（= 5 %。修正前の比べ方の許容 1 % を超える）
    const INJECT_ENV: &str = "TAKO_2193_INJECT";

    /// 採取の間に死んだ前の親の代役（実在しない pid）
    const GONE_PARENT: u32 = 0x7fff_fff0;

    fn inject_2193(spec: &str, before: &mut Table, after: &mut Table, rows: &[PsRow], my_uid: u32) {
        let (kind, every) = spec
            .split_once(':')
            .map_or((spec, 20), |(k, n)| (k, n.parse().unwrap_or(0)));
        assert!(every > 0, "{INJECT_ENV} の N は正の整数（{spec}）");
        let regress = match kind {
            "churn" => false,
            "regress" => true,
            _ => panic!("{INJECT_ENV} は churn[:N] か regress[:N]"),
        };
        let common: Vec<&PsRow> = rows
            .iter()
            .filter(|r| before.0.contains_key(&r.pid) && after.0.contains_key(&r.pid))
            .collect();
        for (i, row) in common.into_iter().enumerate() {
            match (i % every, regress) {
                (0, _) => {
                    // churn: 1 回目だけ古い（exec 前 / 付け替え前）。regress: 2 回とも同じに違う
                    let wrong_argv = |argv: &str| match argv.rsplit_once(' ') {
                        Some((head, _)) if regress => head.to_string(),
                        _ if regress => String::new(),
                        _ => "(exec 前)".to_string(),
                    };
                    let mut tables = vec![&mut *before];
                    if regress {
                        tables.push(&mut *after);
                    }
                    for table in tables {
                        table.0.insert(row.pid, GONE_PARENT);
                        if row.uid == my_uid {
                            let argv = table.1.get(&row.pid).cloned().unwrap_or_default();
                            table.1.insert(row.pid, wrong_argv(&argv));
                        }
                    }
                }
                (1, false) => {
                    before.0.remove(&row.pid); // 1 回目の後に生まれた
                    before.1.remove(&row.pid);
                }
                (2, false) => {
                    after.0.remove(&row.pid); // `ps` の後に死んだ
                    after.1.remove(&row.pid);
                }
                _ => {}
            }
        }
    }

    /// 親子表とコマンド行（[`process_table`] の戻りと同じ形）
    type Table = (HashMap<u32, u32>, HashMap<u32, String>);

    /// `ps -axo pid=,uid=,ppid=,command=` の 1 行
    struct PsRow {
        pid: u32,
        uid: u32,
        ppid: u32,
        /// 空白の連続は 1 つに畳んである（`ps` の列は空白で区切られて届く）
        command: String,
    }

    fn parse_ps_rows(text: &str) -> Vec<PsRow> {
        text.lines()
            .filter_map(|line| {
                let mut it = line.split_whitespace();
                let pid = it.next()?.parse().ok()?;
                let uid = it.next()?.parse().ok()?;
                let ppid = it.next()?.parse().ok()?;
                let command = it.collect::<Vec<_>>().join(" ");
                Some(PsRow {
                    pid,
                    uid,
                    ppid,
                    command,
                })
            })
            .collect()
    }

    /// [`compare_with_ps`] の結果
    #[derive(Debug, Default)]
    struct PsComparison {
        /// 2 回の libproc と `ps` のすべてに居た pid（採取の間に生まれた / 死んだものは入らない）
        common: usize,
        /// そのうち 2 回の libproc の間に親かコマンド行が変わったもの
        moved: usize,
        /// コマンド行を比べた自分のプロセス
        mine: usize,
        /// そのうち `ps` が表記し直す文字を含み、手前の ASCII だけを比べたもの
        rewritten: usize,
        /// 本当のずれ（1 件でもあれば落とす）。コマンド行の中身は出さない（長さと位置だけ）
        mismatches: Vec<String>,
    }

    /// libproc の 2 回の採取（`before` / `after`）で `ps` の 1 回（`rows`）を挟んで比べる（#2193）。
    ///
    /// - **2 回の libproc が同じ答え**なら、その間に走った `ps` も同じものを見ているはず
    ///   （変わって元に戻る、は数十 ms では起きない）なので**全件一致**を求める
    /// - 2 回の間に変わったもの（exec した・前の親が死んで launchd へ付け替わった）は、
    ///   `ps` がどちらを見たかが時刻で決まるので比べない。ただし親の付け替えは 1 回しか
    ///   起きないので、`ps` の親は 2 回のどちらかと同じでなければずれ
    /// - コマンド行は `ps` が印字できない文字を表記し直す（[`ps_prints_verbatim`]）ので、
    ///   そういう文字を含む argv は手前の ASCII だけを比べる
    fn compare_with_ps(before: &Table, rows: &[PsRow], after: &Table, my_uid: u32) -> PsComparison {
        let mut cmp = PsComparison::default();
        for row in rows {
            let pid = row.pid;
            let (Some(&p1), Some(&p2)) = (before.0.get(&pid), after.0.get(&pid)) else {
                continue; // 採取の間に生まれた / 死んだ（ゾンビも libproc には載らない）
            };
            cmp.common += 1;
            let mut moved = p1 != p2;
            if row.ppid != p1 && row.ppid != p2 {
                cmp.mismatches.push(if moved {
                    format!("pid {pid}: 親が libproc {p1} → {p2} / ps {}", row.ppid)
                } else {
                    format!("pid {pid}: 親が libproc {p1} / ps {}", row.ppid)
                });
            }
            if row.uid == my_uid {
                let a1 = before.1.get(&pid).map(String::as_str).unwrap_or("");
                let a2 = after.1.get(&pid).map(String::as_str).unwrap_or("");
                if a1 != a2 {
                    moved = true;
                } else {
                    cmp.mine += 1;
                    let rewritten = !a1.chars().all(ps_prints_verbatim);
                    cmp.rewritten += usize::from(rewritten);
                    let expected = if rewritten {
                        let end = a1.find(|c: char| !matches!(c, ' '..='~'));
                        squeeze(&a1[..end.unwrap_or(a1.len())])
                    } else {
                        squeeze(a1)
                    };
                    let same = if rewritten {
                        row.command.starts_with(&expected)
                    } else {
                        row.command == expected
                    };
                    if !same {
                        let at = expected
                            .chars()
                            .zip(row.command.chars())
                            .take_while(|(a, b)| a == b)
                            .count();
                        cmp.mismatches.push(format!(
                            "pid {pid}: コマンド行が {at} 文字目から違う（libproc {} 文字 / ps {} 文字{}）",
                            expected.chars().count(),
                            row.command.chars().count(),
                            if rewritten { "・手前だけ比較" } else { "" }
                        ));
                    }
                }
            }
            cmp.moved += usize::from(moved);
        }
        cmp
    }

    /// `ps` がそのまま出す文字（#2193 の実測 2026-10-16）。
    ///
    /// `ps` は印字できない文字を表記し直す: 改行 → `\012`・タブ → `\011`・0x01 → `^A`・
    /// DEL → `^?`・C1 制御文字 / ゼロ幅空白 / BOM / 不正な UTF-8 → `M^E` / `\040^K` /
    /// `M-~M^?` など。日本語・全角空白・NBSP・U+2028 はそのまま。libproc は生のまま返すので、
    /// これらを含む argv（複数行の `sh -c` の本文・Bash ツールのラッパー等）は**時刻と関係なく**
    /// 表記が食い違う。文字でも空白でもないもの（絵文字・記号など。実測ではそのまま出た）は
    /// 一律に「表記し直す側」へ倒す（手前だけ比べるので、確かめていない文字で偽のずれを出さない）
    fn ps_prints_verbatim(c: char) -> bool {
        matches!(c, ' '..='~') || (!c.is_control() && (c.is_alphanumeric() || c.is_whitespace()))
    }

    /// 空白の連続を 1 つに畳む（`ps` の列の組み立てと同じ扱い）
    fn squeeze(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn table(entries: &[(u32, u32, &str)]) -> Table {
        let parents = entries.iter().map(|&(pid, ppid, _)| (pid, ppid)).collect();
        let argv = entries
            .iter()
            .map(|&(pid, _, argv)| (pid, argv.to_string()))
            .collect();
        (parents, argv)
    }

    /// 比べる側の自分の uid（合成の表で使う）
    const ME: u32 = 501;

    #[test]
    fn ps照合は採取の間に変わったものを比べない_2193() {
        let before = table(&[
            (10, 1, "tako"),
            (11, 10, "tako"), // 1 回目の後に exec（fork 直後は親のコマンド行のまま）
            (12, 10, "zsh -c make"), // 前の親が死んで launchd へ付け替わる
            (13, 10, "sleep 1"), // `ps` の後に死ぬ
            (14, 10, "cargo build"),
        ]);
        let rows = parse_ps_rows(
            "10 501 1 tako\n11 501 10 git status\n12 501 1 zsh -c make\n\
             13 501 10 sleep 1\n14 501 10 cargo build\n15 501 10 rustc -vV\n",
        );
        let after = table(&[
            (10, 1, "tako"),
            (11, 10, "git status"),
            (12, 1, "zsh -c make"),
            (14, 10, "cargo build"),
            (15, 10, "rustc -vV"), // 1 回目の後に生まれた
        ]);
        let cmp = compare_with_ps(&before, &rows, &after, ME);
        assert!(cmp.mismatches.is_empty(), "{:?}", cmp.mismatches);
        // 共通は 10 / 11 / 12 / 14。変わったのは 11（exec）と 12（付け替え）。
        // コマンド行を比べたのは変わっていない 10 / 12 / 14
        assert_eq!((cmp.common, cmp.moved, cmp.mine), (4, 2, 3));
        // `ps` が変わる前を見た（exec が `ps` の後）でも同じ
        let late = parse_ps_rows("11 501 10 tako\n12 501 10 zsh -c make\n");
        assert!(compare_with_ps(&before, &late, &after, ME)
            .mismatches
            .is_empty());
    }

    #[test]
    fn ps照合はlibprocの本当のずれを見逃さない_2193() {
        // 2 回の libproc が同じ答えなのに `ps` と違う = 採取の時刻では説明できない
        let both = table(&[(10, 7, "cargo build"), (11, 1, "sleep 600")]);
        let rows = parse_ps_rows("10 501 8 cargo build\n11 501 1 sleep 600 --x\n");
        let cmp = compare_with_ps(&both, &rows, &both, ME);
        assert_eq!(cmp.mismatches.len(), 2, "{:?}", cmp.mismatches);
        // 親が変わった最中でも、`ps` が 2 回のどちらとも違う親を見ていればずれ
        let (before, after) = (table(&[(12, 5, "a")]), table(&[(12, 1, "a")]));
        let cmp = compare_with_ps(&before, &parse_ps_rows("12 501 9 a\n"), &after, ME);
        assert_eq!(cmp.mismatches.len(), 1, "{:?}", cmp.mismatches);
        // 1,000 件中 1 件のずれでも落とす（修正前の比べ方は 1 % まで見逃した）
        let many: Vec<(u32, u32, String)> = (100..1100).map(|p| (p, 1, format!("w {p}"))).collect();
        let many_table = table(
            &many
                .iter()
                .map(|(p, pp, a)| (*p, *pp, a.as_str()))
                .collect::<Vec<_>>(),
        );
        let text: String = (100..1100)
            .map(|p| format!("{p} 501 {} w {p}\n", if p == 500 { 2 } else { 1 }))
            .collect();
        let cmp = compare_with_ps(&many_table, &parse_ps_rows(&text), &many_table, ME);
        assert_eq!(cmp.mismatches, ["pid 500: 親が libproc 1 / ps 2"]);
        // 別ユーザーのコマンド行は比べない（特権付きの `ps` だけが読める）
        let root = table(&[(13, 1, "(sshd)")]);
        let cmp = compare_with_ps(
            &root,
            &parse_ps_rows("13 0 1 /usr/sbin/sshd -D\n"),
            &root,
            ME,
        );
        assert!(cmp.mismatches.is_empty() && cmp.mine == 0);
    }

    #[test]
    fn ps照合はpsの表記し直しを同じとみなす_2193() {
        // 実測（2026-10-16）で `ps` がそのまま出した文字と、表記し直した文字
        for c in [
            'a', ' ', '~', 'é', '日', 'ー', '\u{3000}', '\u{a0}', '\u{2028}',
        ] {
            assert!(ps_prints_verbatim(c), "{c:?} はそのまま出る");
        }
        for c in [
            '\n', '\t', '\u{1}', '\u{7f}', '\u{85}', '\u{200b}', '\u{feff}', '\u{fffd}',
        ] {
            assert!(!ps_prints_verbatim(c), "{c:?} は表記し直される");
        }
        let t = table(&[
            (20, 1, "/bin/sh -c sleep 900\n: x"),
            (21, 1, "printf a\tb\u{1}"),
            (22, 1, "git commit -m 日本語\u{3000}です"),
            (23, 1, "sh -c echo 日本語\nx"),
        ]);
        let rows = parse_ps_rows(
            "20 501 1 /bin/sh -c sleep 900\\012: x\n21 501 1 printf a\\011b^A\n\
             22 501 1 git commit -m 日本語\u{3000}です\n23 501 1 sh -c echo 日本語\\012x\n",
        );
        let cmp = compare_with_ps(&t, &rows, &t, ME);
        assert!(cmp.mismatches.is_empty(), "{:?}", cmp.mismatches);
        assert_eq!((cmp.mine, cmp.rewritten), (4, 3));
        // 表記し直しのある argv でも、手前が違えばずれ
        let bad = table(&[(24, 1, "/bin/sh -c slep 900\n: x")]);
        let cmp = compare_with_ps(
            &bad,
            &parse_ps_rows("24 501 1 /bin/sh -c sleep 900\\012: x\n"),
            &bad,
            ME,
        );
        assert_eq!(
            cmp.mismatches,
            ["pid 24: コマンド行が 14 文字目から違う（libproc 19 文字 / ps 27 文字・手前だけ比較）"]
        );
    }

    #[test]
    fn procargs2のargvは空白で繋がる() {
        let mut buf = 2u32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/sleep\0\0\0sleep\x00600\0HOME=/x\0\0");
        assert_eq!(procargs2_argv(&buf).as_deref(), Some("sleep 600"));
        // argc = 0 は「argv が無い」
        let mut empty = 0u32.to_ne_bytes().to_vec();
        empty.extend_from_slice(b"/bin/x\0\0");
        assert_eq!(procargs2_argv(&empty), None);
        // 途中で切れていても読めたぶんだけ返す（壊れた入力で panic しない）
        let mut cut = 3u32.to_ne_bytes().to_vec();
        cut.extend_from_slice(b"/bin/x\0x\0y");
        assert_eq!(procargs2_argv(&cut).as_deref(), Some("x y"));
        assert_eq!(procargs2_argv(&[1, 0]), None);
    }

    #[test]
    fn procargs2のargv0は名前だけを取る() {
        // 版の数字の実体を symlink 越しに起動した claude（#2533）: 名前は argv[0] 側
        let mut buf = 3u32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/x/versions/2.1.301\0\0\0/x/bin/claude\0--model\0m\0HOME=/x\0\0");
        assert_eq!(procargs2_argv0_name(&buf).as_deref(), Some("claude"));
        // パスの無い argv[0]・ログインシェルの `-zsh` はそのまま
        let mut login = 1u32.to_ne_bytes().to_vec();
        login.extend_from_slice(b"/bin/zsh\0-zsh\0\0");
        assert_eq!(procargs2_argv0_name(&login).as_deref(), Some("-zsh"));
        // argc = 0・空の argv[0]・壊れた入力は None（panic しない）
        let mut empty = 0u32.to_ne_bytes().to_vec();
        empty.extend_from_slice(b"/bin/x\0\0");
        assert_eq!(procargs2_argv0_name(&empty), None);
        let mut blank = 1u32.to_ne_bytes().to_vec();
        blank.extend_from_slice(b"/bin/x\0\0/\0");
        assert_eq!(procargs2_argv0_name(&blank), None);
        assert_eq!(procargs2_argv0_name(&[1, 0]), None);
    }
}
