//! プロセス検査（抽象境界 B5 の検査側。#524）
//!
//! ## 何を閉じ込めるか
//!
//! 「あるペインの配下で何が動いているか」「その中で何が LISTEN しているか」を
//! OS へ問い合わせる部分。呼び出し側（`ports` / UI）は単一のコードパスを持ち、
//! `cfg(target_os)` はこのモジュールの内側だけに置く。
//!
//! macOS 側の実装（libproc）は歴史的に `ports.rs` にあり、そちらが正のまま。
//! ここには **Windows 実装と、両プラットフォームで共有できる純粋関数**を置く
//! （macOS 実装を移設すると挙動差ゼロの検証を Windows 機からは行えないため。
//! 移設は macOS 実機で検証できるタイミングに回す）。
//!
//! ## Windows で「ペイン配下」をどう決めるか
//!
//! macOS は**制御端末（tty）の一致**で判定している（`proc_bsdinfo.e_tdev`）。
//! Windows の ConPTY に制御端末の概念は無く、疑似コンソールに接続している
//! プロセスを列挙する公開 API も無い。そこで **PTY 直下の子プロセス
//! （`TerminalSession::child_pid`）の子孫**で判定する。
//!
//! どちらの方式も「永続化バックエンド（tmux / psmux）越しに起動されたプロセス」は
//! 拾えない（器のサーバープロセス配下に移るため）。この制限は macOS と同じで、
//! Windows 固有の縮退ではない。
//!
//! ## 実行中プロセスの実行ファイルパス（#936）
//!
//! [`image_path`] は**両プラットフォームぶんをここに置く**。この関数には
//! `ports.rs` のような macOS 側の先行実装が無く（`tako-control::stale_binary` が
//! `cfg` 付きで持っていた）、寄せ先を分けると呼び出し側に `cfg` が残るため。
//!
//! Windows は `QueryFullProcessImageNameW`。**フラグは 0（Win32 形式）で呼ぶ**:
//! `PROCESS_NAME_NATIVE`（1）は**リネームを反映しない**（実測 2026-09-04:
//! 実行中の exe を改名すると 0 は新しい名前、1 は古い名前を返す）。claude の
//! 自己更新は「旧 exe を `claude.exe.old.<ts>` へ改名 → 新 exe を同じ名前で設置」
//! なので、**リネームを反映する 0 でないと古いプロセスを見分けられない**
//! （#936 の stale 検知はこの差だけで成り立っている）。
//! .NET の `Process.Path` / `GetModuleFileNameEx` も改名前の名前を返すので
//! 「Get-Process で見えているパス」を根拠にしないこと。
//!
//! ## 依存クレートを足さない方針
//!
//! `windows-sys` は入れず必要な関数だけ宣言する（`platform::locale` の
//! `GetUserPreferredUILanguages`、`platform::ime` の IMM32 と同じ。
//! 構造体レイアウトの転記ミスは `const _: () = assert!(size_of…)` と
//! 実機テストで捕まえる）。

use std::collections::{HashMap, HashSet};

/// プロセス 1 件のスナップショット（親子関係と名前だけ持つ）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    /// 実行ファイル名（`node.exe`）。取得できなければ空
    pub name: String,
}

/// LISTEN 中の TCP エンドポイント（所有プロセス付き）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpListenEntry {
    pub port: u16,
    pub pid: u32,
}

/// 全プロセスのスナップショット。取得手段が無いプラットフォームでは空を返す
pub fn snapshot() -> Vec<ProcEntry> {
    imp::snapshot()
}

/// LISTEN 中の TCP エンドポイント全件（IPv4 + IPv6）。
/// 取得手段が無いプラットフォームでは空を返す
pub fn tcp_listeners() -> Vec<TcpListenEntry> {
    imp::tcp_listeners()
}

/// ループバック TCP 接続を**張った側**（クライアント側）のプロセス（#841）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackPeer {
    /// 接続元プロセスの pid
    pub pid: u32,
    /// 接続元ソケットの所有ユーザー。**Windows は `None`**
    /// （この OS ではソケットの所有者を引く手段が無い = 呼び出し側は
    /// 「所有者ゲートを強制できない」と扱う）
    pub uid: Option<u32>,
}

/// `127.0.0.1:peer_port -> 127.0.0.1:local_port` の接続を張った側のプロセスを引く（#841）。
///
/// remote デーモンは `tiny_http` 越しに接続元アドレス（= `peer_port`）しか受け取れない。
/// そこから**所有プロセス**まで辿れて初めて「この接続は tailscaled が張ったのか」を
/// 問える。取れなければ `None`（呼び出し側は**拒否側へ倒す**こと）。
///
/// **`lsof` / libproc の fd 走査では引けない**（実測 2026-09-12: 非 root から
/// root 所有 tailscaled の fd は 1 件も見えない）。macOS は `netstat -anv` と同じ
/// `net.inet.tcp.pcblist_n` sysctl を使う。これは所有者に関係なく全ソケットを返す。
/// Windows は `GetExtendedTcpTable`（[`tcp_listeners`] と同じ表の非 LISTEN 行）
pub fn loopback_tcp_peer(peer_port: u16, local_port: u16) -> Option<LoopbackPeer> {
    if peer_port == 0 || local_port == 0 || peer_port == local_port {
        return None;
    }
    imp::loopback_tcp_peer(peer_port, local_port)
}

/// 自プロセスの所有ユーザー。**Windows は `None`**（[`LoopbackPeer::uid`] と対）
pub fn current_uid() -> Option<u32> {
    imp::current_uid()
}

/// 実行中プロセスの実行ファイルの絶対パス。取れなければ `None`
/// （プロセスが既に居ない / 権限が無い / 取得手段が無いプラットフォーム）。
///
/// **返すのは「いまのファイル名」**: 実行中に実行ファイルが改名されたら
/// 改名後のパスを返す（モジュールの解説を参照。#936 の stale 検知が依っている）
pub fn image_path(pid: u32) -> Option<std::path::PathBuf> {
    imp::image_path(pid)
}

/// プロセス 1 件の**コマンドラインつき**スナップショット（#1282）。
///
/// [`ProcEntry`] は名前と親子だけを持つ。器（tmux / psmux）を名前付きパイプ越しに
/// 見分けるには **`-L <ソケット名>` が載っているコマンドライン**が要るので、
/// 必要になる場面だけこちらを使う（全プロセスぶん引くと重い）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcDetail {
    pub pid: u32,
    /// 実行ファイル名（`tmux.exe`）
    pub name: String,
    /// 起動時のコマンドライン全体。読めなければ `None`
    /// （権限が無い / 既に居ない / 取得手段が無いプラットフォーム）
    pub command_line: Option<String>,
    /// 起動時刻（UNIX 秒）。読めなければ `None`
    pub started_unix: Option<u64>,
}

/// 実行中プロセスのコマンドライン全体。取れなければ `None`。
///
/// Windows は `NtQueryInformationProcess(ProcessCommandLineInformation)`。
/// **`PROCESS_QUERY_LIMITED_INFORMATION` だけで読める**ので、PEB を
/// `ReadProcessMemory` で辿る古い手口（bit 幅を跨ぐと壊れる）は使わない。
/// Windows 8.1 以降で使える（tako の下限は 10.0.17763 = #965 の動作要件）。
///
/// unix には「他プロセスのコマンドラインを引く」用途がまだ無いので `None`
/// （器の列挙はソケットファイル走査で足りる = [`crate::tmux_cleanup`]）
pub fn command_line(pid: u32) -> Option<String> {
    imp::command_line(pid)
}

/// 実行中プロセスの起動時刻（UNIX 秒）。取れなければ `None`。
///
/// **pid の再利用を見分ける材料**（#1282: 所有者が死んだ後に別プロセスが
/// 同じ pid を取っていないかを、器の起動時刻と突き合わせて確かめる /
/// #1296: 使い捨て dir より後に始まったプロセスはその dir の持ち主ではない）。
///
/// Windows は `GetProcessTimes` の生成時刻、macOS は libproc
/// （`proc_pidinfo(PROC_PIDTBSDINFO)` の `pbi_start_tvsec`）。
/// **それ以外の unix は `None`**（材料が無いときは呼び出し側が見送る側へ倒れる）
pub fn start_time_unix(pid: u32) -> Option<u64> {
    imp::start_time_unix(pid)
}

/// 名前（拡張子と大文字小文字を無視）が `names` のいずれかに一致するプロセスの詳細。
///
/// 取得手段が無いプラットフォームでは空を返す
/// （呼び出し側は「見つからない」= 何もしない側へ倒すこと）
pub fn details_by_name(names: &[&str]) -> Vec<ProcDetail> {
    let wanted: HashSet<String> = names.iter().map(|n| stem_of(n)).collect();
    snapshot()
        .into_iter()
        .filter(|p| wanted.contains(&stem_of(&p.name)))
        .map(|p| ProcDetail {
            command_line: command_line(p.pid),
            started_unix: start_time_unix(p.pid),
            pid: p.pid,
            name: p.name,
        })
        .collect()
}

/// `root` とその子孫の pid 集合（`root` 自身を含む）。
///
/// 純粋関数なので **macOS 上でもテストできる**。Windows の ppid は
/// 「親が先に死んで pid が再利用された」場合に無関係なプロセスを指しうるため、
/// 訪問済み集合で循環を止める（無限ループにしない）。
///
/// `root = 0`（System Idle Process）は親を持たないシステムプロセス群の入口に
/// なってしまうので辿らない。ペインの子 pid が 0 になることは無い
pub fn descendants_of(procs: &[ProcEntry], root: u32) -> HashSet<u32> {
    if root == 0 {
        return HashSet::from([0]);
    }
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        // 自分が自分の親になっている行（壊れた ppid）は「自分の子」にしない
        if p.ppid != p.pid {
            children.entry(p.ppid).or_default().push(p.pid);
        }
    }
    let mut found: HashSet<u32> = HashSet::new();
    found.insert(root);
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        let Some(kids) = children.get(&pid) else {
            continue;
        };
        for &kid in kids {
            if found.insert(kid) {
                stack.push(kid);
            }
        }
    }
    found
}

/// 実行ファイル名から拡張子と大文字小文字を落とした比較用の語
fn stem_of(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    // Windows のファイル名は大文字小文字を区別しないので、**先に**畳んでから
    // 拡張子を落とす（`.Exe` のような混在も同じ語になる）
    let lower = base.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// tako 自身の実行ファイル名（GUI と CLI）
const TAKO_NAMES: [&str; 2] = ["tako", "tako-app"];
/// エージェント CLI の実行ファイル名
const AGENT_NAMES: [&str; 3] = ["claude", "codex", "agy"];

/// **tako 自身の直接の子として動いているエージェント CLI** を洗い出す（#1129）。
///
/// tako がエージェントを起こす設計上の経路は必ず**ペインのシェル**を通る
/// （`queue_command_flow` / `Request::Send`）ので、正しい親子は
/// `シェル → claude` になる。`tako → claude` は「tako が自分の子として起こして
/// 待っている」形で、`tako setup` の認証段が起こしていた
/// `claude auth login`（ブラウザ操作待ち = 自分では終わらない）と
/// setup エージェントがこれに当たる。
///
/// この形の子は **tako が死んでも回収されない**。Windows は子プロセスの終了要求
/// （#1067 の境界 B5）が未実装で、ペイン close も隔離インスタンスの終了も
/// 孫を回収しないため、実機では 1 日で 46 本まで積み上がり CPU が 100% に
/// 張り付いた（#1129 の採取）。
///
/// **直接の親だけ**を見る。祖先を辿ると、器を持たない構成（`TAKO_BACKEND=none`）で
/// ペインのシェルが `tako-app` の子になるため、シェルから正しく起動した
/// エージェントまで拾ってしまう。
///
/// 返すのは `(エージェントの pid, 親の tako の pid)`。純粋関数なので
/// **macOS 上から Windows の名前（`claude.exe` / `tako.exe`）も検査できる**
pub fn agent_children_of_tako(procs: &[ProcEntry]) -> Vec<(u32, u32)> {
    let takos: HashSet<u32> = procs
        .iter()
        .filter(|p| TAKO_NAMES.contains(&stem_of(&p.name).as_str()))
        .map(|p| p.pid)
        .collect();
    let mut found: Vec<(u32, u32)> = procs
        .iter()
        // 自分が自分の親になっている行（壊れた ppid）は親子と見なさない
        .filter(|p| p.ppid != p.pid)
        .filter(|p| AGENT_NAMES.contains(&stem_of(&p.name).as_str()))
        .filter(|p| takos.contains(&p.ppid))
        .map(|p| (p.pid, p.ppid))
        .collect();
    found.sort_unstable();
    found
}

/// [`agent_children_of_tako`] を **`roots` の子孫に限る**版（#1129）。
///
/// `tako → claude` は**本番でも正当に存在する**（#391 の setup 対話エージェントは
/// `tako setup` が自分の子として起こして待つ形）。だから機械全体で見ると、人が
/// `tako setup` を開いているだけで検査が落ちる（開発機で実測: 本番の
/// `tako(17296) → claude(20014)` が居た）。検査したいインスタンスのペイン配下へ絞る。
///
/// `roots` にはペインのプロセスを渡す。器つきならペインのシェルは器の子なので
/// 器のペイン一覧（`pane_pids_all`）から、器なしなら PTY の直接の子
/// （`TerminalSession::child_pid`）から採る（両方の構成を覆う）
pub fn agent_children_of_tako_under(procs: &[ProcEntry], roots: &[u32]) -> Vec<(u32, u32)> {
    let mut scope: HashSet<u32> = HashSet::new();
    for &root in roots {
        scope.extend(descendants_of(procs, root));
    }
    agent_children_of_tako(procs)
        .into_iter()
        .filter(|(_, parent)| scope.contains(parent))
        .collect()
}

/// **GUI（`tako-app`）として動いているプロセス**の pid（`self_pid` は除く）。
///
/// 除外が要るのは、この数えを使う `tako tmux cleanup --servers` が
/// **GUI プロセスの中で実行される**（CLI は IPC で GUI へ投げる）ため。
/// 自分を数えると「生きた tako-app が居る」が常に真になり、回収が永久に見送られる。
///
/// CLI（`tako`）も含めない: コマンドを投げた CLI 自身が数えられてしまう（#1282）
pub fn live_tako_app_pids(procs: &[ProcEntry], self_pid: u32) -> Vec<u32> {
    let mut out: Vec<u32> = procs
        .iter()
        .filter(|p| stem_of(&p.name) == "tako-app")
        .map(|p| p.pid)
        .filter(|&pid| pid != self_pid)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// エージェント CLI として動いているプロセスの件数（診断用。親子は問わない）
pub fn agent_process_count(procs: &[ProcEntry]) -> usize {
    procs
        .iter()
        .filter(|p| AGENT_NAMES.contains(&stem_of(&p.name).as_str()))
        .count()
}

#[cfg(windows)]
mod imp {
    use super::{LoopbackPeer, ProcEntry, TcpListenEntry};
    use std::ffi::c_void;

    type Handle = *mut c_void;

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const MAX_PATH: usize = 260;

    /// `PROCESSENTRY32W`（tlhelp32.h）。`tako-control::agents` の同名構造体と対。
    /// あちらは制御プレーン用（親子マップのみ）で、こちらは検査用に名前も使う
    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; MAX_PATH],
    }

    /// `PROCESS_QUERY_LIMITED_INFORMATION`。`PROCESS_QUERY_INFORMATION` より弱く、
    /// **保護されたプロセスにも開ける**（実行ファイルパスの取得はこれで足りる）
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x0000_1000;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
        fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(object: Handle) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            exe_name: *mut u16,
            size: *mut u32,
        ) -> i32;
    }

    /// `FILETIME`（100ns 単位・1601-01-01 起点）。`GetProcessTimes` の出力
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    /// `UNICODE_STRING`（ntdef.h）。`Buffer` は**呼び出し側が渡したバッファの内側**を
    /// 指す（`ProcessCommandLineInformation` は 1 つのバッファに構造体と文字列を詰める）
    #[repr(C)]
    struct UnicodeString {
        /// **バイト数**（文字数ではない）
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    // 転記ミスを型検査で捕まえる（x64: 2 + 2 + パディング 4 + ポインタ 8）
    const _: () = assert!(std::mem::size_of::<UnicodeString>() == 2 * size_of::<usize>());
    const _: () = assert!(std::mem::size_of::<FileTime>() == 8);

    /// `ProcessCommandLineInformation`（PROCESSINFOCLASS = 60）。Windows 8.1 以降
    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
    /// `STATUS_INFO_LENGTH_MISMATCH`（バッファが足りない）
    const STATUS_INFO_LENGTH_MISMATCH: i32 = -1_073_741_820; // 0xC0000004
    /// 1601-01-01 から 1970-01-01 までの 100ns 単位
    const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetProcessTimes(
            process: Handle,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process: Handle,
            info_class: u32,
            info: *mut std::ffi::c_void,
            info_len: u32,
            return_len: *mut u32,
        ) -> i32;
    }

    pub(super) fn command_line(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        // SAFETY: OpenProcess の戻りは null を検査し、復帰経路すべてで CloseHandle する。
        // バッファは **u64 の器**で確保して `UNICODE_STRING` のポインタ整列を満たす
        // （`Vec<u8>` の先頭は 1 バイト整列しか保証されない）。
        // NtQueryInformationProcess にはバッファの実長だけを渡し、書き戻された
        // `Buffer` / `Length` がそのバッファの内側に収まることを読み出し前に検査する
        // （API の返す値を信用しない）
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            // 構造体 + 文字列を 1 つのバッファに詰めて返す API。足りなければ
            // 必要長を教えてくれるので 1 度だけ取り直す
            let mut buf: Vec<u64> = vec![0; 4096 / 8];
            let mut need: u32 = 0;
            let mut status = NtQueryInformationProcess(
                handle,
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut need,
            );
            if status == STATUS_INFO_LENGTH_MISMATCH && need as usize > buf.len() * 8 {
                buf = vec![0; (need as usize).div_ceil(8)];
                status = NtQueryInformationProcess(
                    handle,
                    PROCESS_COMMAND_LINE_INFORMATION,
                    buf.as_mut_ptr().cast(),
                    (buf.len() * 8) as u32,
                    &mut need,
                );
            }
            CloseHandle(handle);
            let bytes = buf.len() * 8;
            if status < 0 || bytes < std::mem::size_of::<UnicodeString>() {
                return None;
            }
            let us = &*buf.as_ptr().cast::<UnicodeString>();
            let len = us.length as usize;
            if us.buffer.is_null() || len == 0 || len % 2 != 0 {
                return None;
            }
            // `Buffer` はこのバッファの内側を指しているはず。外を指していたら読まない
            let start = buf.as_ptr() as usize;
            let at = us.buffer as usize;
            if at < start || at.checked_add(len).is_none_or(|end| end > start + bytes) {
                return None;
            }
            let units = std::slice::from_raw_parts(us.buffer, len / 2);
            Some(String::from_utf16_lossy(units))
        }
    }

    pub(super) fn start_time_unix(pid: u32) -> Option<u64> {
        if pid == 0 {
            return None;
        }
        // SAFETY: OpenProcess の戻りは null を検査し、復帰経路すべてで CloseHandle する。
        // 4 つの出力はすべてローカル変数で、API はそれ以上書き込まない
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let (mut created, mut exited, mut kernel, mut user) = (
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
                FileTime::default(),
            );
            let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            let ticks = ((created.high as u64) << 32) | created.low as u64;
            ticks
                .checked_sub(FILETIME_UNIX_EPOCH)
                .map(|since_epoch| since_epoch / 10_000_000)
        }
    }

    pub(super) fn image_path(pid: u32) -> Option<std::path::PathBuf> {
        // SAFETY: OpenProcess の戻りは null を検査し、復帰経路すべてで CloseHandle する。
        // buf は size に渡した長さぶん確保済みで、API はそれ以上書き込まない
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            // 長いパス（verbatim 無しでも `MAX_PATH` を超えうる）に備えて広く取る。
            // 呼ばれるのは走査の変化時だけなので確保コストは問題にならない
            let mut buf = vec![0u16; 32 * 1024];
            let mut size = buf.len() as u32;
            // フラグ 0 = Win32 形式。1（`PROCESS_NAME_NATIVE`）は改名を反映しない
            let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
            CloseHandle(handle);
            if ok == 0 || size == 0 {
                return None;
            }
            let len = (size as usize).min(buf.len());
            Some(std::path::PathBuf::from(String::from_utf16_lossy(
                &buf[..len],
            )))
        }
    }

    fn invalid_handle() -> Handle {
        -1isize as Handle
    }

    pub(super) fn snapshot() -> Vec<ProcEntry> {
        let mut out = Vec::new();
        // SAFETY: スナップショットハンドルは取得直後に妥当性を検査し、
        // 復帰経路すべてで CloseHandle する。entry は毎回 dw_size を設定した
        // ローカル変数で、API はこのサイズ以上には書き込まない
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap == invalid_handle() {
                return out;
            }
            let mut entry: ProcessEntry32W = std::mem::zeroed();
            entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let len = entry
                        .sz_exe_file
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(MAX_PATH);
                    out.push(ProcEntry {
                        pid: entry.th32_process_id,
                        ppid: entry.th32_parent_process_id,
                        name: String::from_utf16_lossy(&entry.sz_exe_file[..len]),
                    });
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    // --- TCP テーブル（iphlpapi） ---

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    /// `TCP_TABLE_OWNER_PID_ALL`。LISTENER 専用クラス（3）ではなく全件を取り、
    /// 状態は自分で見る（macOS 実装が `TSI_S_LISTEN` を見るのと同じ判定にするため）
    const TCP_TABLE_OWNER_PID_ALL: u32 = 5;
    /// `MIB_TCP_STATE_LISTEN`
    const TCP_STATE_LISTEN: u32 = 2;
    const NO_ERROR: u32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    /// `MIB_TCPROW_OWNER_PID`（tcpmib.h）
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TcpRowOwnerPid {
        state: u32,
        local_addr: u32,
        /// ネットワークバイトオーダーの 16bit が下位に入っている
        local_port: u32,
        remote_addr: u32,
        remote_port: u32,
        owning_pid: u32,
    }
    const _: () = assert!(std::mem::size_of::<TcpRowOwnerPid>() == 24);

    /// `MIB_TCP6ROW_OWNER_PID`（tcpmib.h）。v4 と**フィールド順が違う**
    /// （state と owning_pid が末尾）ので転記を取り違えないこと
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Tcp6RowOwnerPid {
        local_addr: [u8; 16],
        local_scope_id: u32,
        local_port: u32,
        remote_addr: [u8; 16],
        remote_scope_id: u32,
        remote_port: u32,
        state: u32,
        owning_pid: u32,
    }
    const _: () = assert!(std::mem::size_of::<Tcp6RowOwnerPid>() == 56);

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            order: i32,
            af: u32,
            table_class: u32,
            reserved: u32,
        ) -> u32;
    }

    /// テーブルを取得して生バイト列（先頭 4 バイトが件数）を返す。
    /// 件数はスキャンの合間に増えうるので、サイズ不足なら数回まで取り直す
    fn fetch_table(af: u32) -> Option<Vec<u32>> {
        let mut size: u32 = 0;
        // SAFETY: 1 回目はバッファ null + size 0 で必要量を問い合わせる規定の呼び方
        let rc = unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                af,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if rc != ERROR_INSUFFICIENT_BUFFER && rc != NO_ERROR {
            // 長さ問い合わせを受け付けない環境に備えて、それらしい大きさから始める
            // （足りなければ下のループが API の返す必要量で取り直す）
            size = 64 * 1024;
        }
        for _ in 0..4 {
            if size == 0 {
                return None;
            }
            // u8 ではなく u32 で確保して 4 バイト境界を保証する（行は align 4）
            let mut buf = vec![0u32; (size as usize).div_ceil(4)];
            // SAFETY: buf は size バイト以上を確保済みで、size にその長さを渡している
            let rc = unsafe {
                GetExtendedTcpTable(
                    buf.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    af,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
            match rc {
                NO_ERROR => return Some(buf),
                // 取得の合間に接続が増えた → 新しい size で取り直す
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return None,
            }
        }
        None
    }

    /// 先頭の件数と行配列を読み出す。バッファ長を超える件数は信用しない
    fn rows<T: Copy>(buf: &[u32]) -> Vec<T> {
        if buf.is_empty() {
            return Vec::new();
        }
        let count = buf[0] as usize;
        let bytes = buf.len() * 4;
        let row_size = std::mem::size_of::<T>();
        let available = (bytes.saturating_sub(4)) / row_size;
        let count = count.min(available);
        let base = unsafe { buf.as_ptr().cast::<u8>().add(4) };
        (0..count)
            // SAFETY: base + i*row_size は上で available により範囲内に制限済み。
            // 行は POD で、バッファは u32 確保のため 4 バイト境界に載っている
            .map(|i| unsafe { std::ptr::read_unaligned(base.add(i * row_size).cast::<T>()) })
            .collect()
    }

    /// `dwLocalPort` はネットワークバイトオーダーの 16bit が DWORD の下位に入る
    fn port_of(raw: u32) -> u16 {
        u16::from_be((raw & 0xffff) as u16)
    }

    pub(super) fn tcp_listeners() -> Vec<TcpListenEntry> {
        let mut out = Vec::new();
        if let Some(buf) = fetch_table(AF_INET) {
            for r in rows::<TcpRowOwnerPid>(&buf) {
                if r.state == TCP_STATE_LISTEN {
                    out.push(TcpListenEntry {
                        port: port_of(r.local_port),
                        pid: r.owning_pid,
                    });
                }
            }
        }
        if let Some(buf) = fetch_table(AF_INET6) {
            for r in rows::<Tcp6RowOwnerPid>(&buf) {
                if r.state == TCP_STATE_LISTEN {
                    out.push(TcpListenEntry {
                        port: port_of(r.local_port),
                        pid: r.owning_pid,
                    });
                }
            }
        }
        out.retain(|e| e.port != 0);
        out
    }

    /// 127.0.0.1 の `peer_port -> local_port` を張った側の pid（#841）。
    ///
    /// [`tcp_listeners`] と**同じ表**（`TCP_TABLE_OWNER_PID_ALL`）の非 LISTEN 行を見る。
    /// 4 つ組（両端のアドレスとポート）まで照合するので、たまたま同じポート番号を
    /// 使っている別ホスト向けの接続を取り違えない。
    ///
    /// Windows には**ソケットの所有ユーザーを引く手段が無い**ので `uid` は `None`
    /// （所有者ゲートは呼び出し側で「強制できない」として扱う）
    pub(super) fn loopback_tcp_peer(peer_port: u16, local_port: u16) -> Option<LoopbackPeer> {
        // dwLocalAddr / dwRemoteAddr はネットワークバイトオーダーの 32bit がそのまま入る
        let loopback = u32::from_ne_bytes(std::net::Ipv4Addr::LOCALHOST.octets());
        let buf = fetch_table(AF_INET)?;
        let mut found: Option<u32> = None;
        for r in rows::<TcpRowOwnerPid>(&buf) {
            if r.state == TCP_STATE_LISTEN
                || r.local_addr != loopback
                || r.remote_addr != loopback
                || port_of(r.local_port) != peer_port
                || port_of(r.remote_port) != local_port
            {
                continue;
            }
            match found {
                // 同じ 4 つ組が 2 行あり、しかも所有者が食い違う = どちらか分からない。
                // 曖昧なまま「信頼できる pid」を返さない（呼び出し側は拒否へ倒す）
                Some(pid) if pid != r.owning_pid => return None,
                _ => found = Some(r.owning_pid),
            }
        }
        found.map(|pid| LoopbackPeer { pid, uid: None })
    }

    /// Windows にはソケット / プロセスの所有ユーザーという概念の直接の対応が無い
    /// （トークンの SID を引くのは別の権限が要る）。`None` = 所有者ゲートは効かせない
    pub(super) fn current_uid() -> Option<u32> {
        None
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{LoopbackPeer, ProcEntry, TcpListenEntry};

    /// macOS の検査は `ports.rs` の libproc 実装が正（このモジュールは使わない）
    pub(super) fn snapshot() -> Vec<ProcEntry> {
        Vec::new()
    }

    /// 他プロセスのコマンドラインを引く用途が unix にはまだ無い（#1282 の器の列挙は
    /// ソケットファイル走査で足りる）。読めない = 呼び出し側は見送る
    pub(super) fn command_line(_pid: u32) -> Option<String> {
        None
    }

    /// macOS は libproc の `proc_bsdinfo`（`ports.rs` の `bsd_info` と同じ呼び出しだが、
    /// あちらは制御端末の取得用に private なので、ここでは起動時刻だけを取る）。
    /// pid 再利用の判別（#1296）に使う
    #[cfg(target_os = "macos")]
    pub(super) fn start_time_unix(pid: u32) -> Option<u64> {
        let pid = i32::try_from(pid).ok()?;
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: 渡すバッファは size ぴったりのローカル変数で、
        // 書き込まれたバイト数が size と一致したときだけ中身を読む
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        (written == size).then_some(info.pbi_start_tvsec)
    }

    /// macOS 以外の unix は取得手段を持たない（`/proc/<pid>/stat` の
    /// starttime は boot 時刻との合成が要る。tako の対象 OS ではないので入れない）
    #[cfg(not(target_os = "macos"))]
    pub(super) fn start_time_unix(_pid: u32) -> Option<u64> {
        None
    }

    pub(super) fn tcp_listeners() -> Vec<TcpListenEntry> {
        Vec::new()
    }

    /// macOS は libproc の `proc_pidpath`（**実体のパス**を返すので symlink の
    /// ランチャ越しに起動しても `versions/<版>` 側が出る）。
    /// それ以外の unix は `/proc/<pid>/exe`
    #[cfg(target_os = "macos")]
    pub(super) fn image_path(pid: u32) -> Option<std::path::PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: buf.len() を長さとして渡しており、API はそれ以上書き込まない
        let ret =
            unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if ret <= 0 {
            return None;
        }
        let len = buf.iter().position(|&b| b == 0).unwrap_or(ret as usize);
        let text = std::str::from_utf8(&buf[..len]).ok()?;
        Some(std::path::PathBuf::from(text))
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn image_path(pid: u32) -> Option<std::path::PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    }

    /// unix はソケットの所有ユーザーを引けるので、自分の uid も返せる
    pub(super) fn current_uid() -> Option<u32> {
        // SAFETY: getuid は引数を取らず必ず成功する
        Some(unsafe { libc::getuid() })
    }

    /// macOS 以外の unix は TCP テーブルの読み口を持たない（tako の対象 OS ではない）。
    /// `None` = 呼び出し側は拒否へ倒す
    #[cfg(not(target_os = "macos"))]
    pub(super) fn loopback_tcp_peer(_peer_port: u16, _local_port: u16) -> Option<LoopbackPeer> {
        None
    }

    /// macOS の接続元プロセス解決（#841）。
    ///
    /// ## なぜ libproc の fd 走査ではないのか
    ///
    /// `ports.rs` が使っている libproc の fd 走査は**自分と同じユーザーのプロセスしか
    /// 見えない**（実測 2026-09-12: 非 root から root 所有の tailscaled は
    /// `lsof -p <pid>` も `PROC_PIDLISTFDS` も 0 件）。tailscaled は root で動くので、
    /// fd 走査では**正規の接続元すら特定できない**。`net.inet.tcp.pcblist_n` は
    /// 所有者に関係なく全 TCP ソケットを返す（`netstat -anv` の `process:pid` 列と同じ出所）。
    ///
    /// ## バイト列の読み方
    ///
    /// 先頭は `struct xinpgen`（自身の長さを持つ）。以降は `{u32 len, u32 kind}` で
    /// 始まる**自己記述レコード**の並びで、ソケット 1 本ぶんが
    /// XSO_INPCB → XSO_SOCKET → XSO_RCVBUF → XSO_SNDBUF → XSO_STATS → XSO_TCPCB の
    /// 順に連続する。次のレコードへは **8 バイト境界へ丸めて**進む（`xtcpcb_n` のように
    /// len が 8 の倍数でない種別があり、len だけで進むと途中でずれて走査が止まる）。
    ///
    /// `xinpcb_n` / `xsocket_n` は xnu の PRIVATE ヘッダにあり **SDK に入っていない**。
    /// しかも **4 バイトパック**（`u_int64_t so_pcb` がオフセット 28 に来る = 自然
    /// アライメントなら 32）なので、`#[repr(C)]` の転記は取り違えやすい。ここでは
    /// **必要なフィールドのオフセットだけ**を定数で持ち、レコード長で範囲を守る。
    /// オフセットの正しさは「自分で張った接続の pid / uid が自分と一致するか」を
    /// 見るユニットテストが実測で押さえる（ずれたら必ず落ちる）
    #[cfg(target_os = "macos")]
    pub(super) fn loopback_tcp_peer(peer_port: u16, local_port: u16) -> Option<LoopbackPeer> {
        /// `XSO_SOCKET`（socketvar.h）
        const XSO_SOCKET: u32 = 0x001;
        /// `XSO_INPCB`（socketvar.h）
        const XSO_INPCB: u32 = 0x010;
        /// `xinpcb_n.inp_fport`（u16・ネットワークバイトオーダー）
        const INP_FPORT: usize = 16;
        /// `xinpcb_n.inp_lport`
        const INP_LPORT: usize = 18;
        /// `xinpcb_n.inp_vflag`（u8）
        const INP_VFLAG: usize = 44;
        /// `xinpcb_n.inp_dependfaddr` の IPv4 部（`in_addr_4in6` の末尾 4 バイト）
        const INP_FADDR4: usize = 60;
        /// `xinpcb_n.inp_dependladdr` の IPv4 部
        const INP_LADDR4: usize = 76;
        /// ここまで読むので、これより短い XSO_INPCB は無視する
        const INPCB_MIN: usize = INP_LADDR4 + 4;
        /// `INP_IPV4`（in_pcb.h）
        const INP_IPV4: u8 = 0x1;
        /// `xsocket_n.so_uid`（u32）
        const SO_UID: usize = 64;
        /// `xsocket_n.so_last_pid`（pid_t）
        const SO_LAST_PID: usize = 68;
        /// ここまで読むので、これより短い XSO_SOCKET は無視する
        const SOCKET_MIN: usize = SO_LAST_PID + 4;

        fn u32_at(rec: &[u8], off: usize) -> u32 {
            u32::from_ne_bytes([rec[off], rec[off + 1], rec[off + 2], rec[off + 3]])
        }
        fn port_at(rec: &[u8], off: usize) -> u16 {
            u16::from_be_bytes([rec[off], rec[off + 1]])
        }
        /// 次のレコードは 8 バイト境界から始まる
        fn roundup8(n: usize) -> usize {
            n.div_ceil(8) * 8
        }

        let buf = pcblist_n()?;
        if buf.len() < 8 {
            return None;
        }
        // 先頭の xinpgen を読み飛ばす（自身の長さが先頭 u32）
        let mut pos = roundup8(u32_at(&buf, 0) as usize);
        let loopback = u32::from_ne_bytes(std::net::Ipv4Addr::LOCALHOST.octets());
        // 直前に見た XSO_INPCB が探している 4 つ組だったか
        let mut pending = false;
        let mut found: Option<LoopbackPeer> = None;
        while pos + 8 <= buf.len() {
            let rlen = u32_at(&buf, pos) as usize;
            let kind = u32_at(&buf, pos + 4);
            if rlen < 8 || pos + rlen > buf.len() {
                break;
            }
            let rec = &buf[pos..pos + rlen];
            if kind == XSO_INPCB {
                pending = rlen >= INPCB_MIN
                    && rec[INP_VFLAG] & INP_IPV4 != 0
                    && port_at(rec, INP_LPORT) == peer_port
                    && port_at(rec, INP_FPORT) == local_port
                    && u32_at(rec, INP_LADDR4) == loopback
                    && u32_at(rec, INP_FADDR4) == loopback;
            } else if kind == XSO_SOCKET && std::mem::take(&mut pending) && rlen >= SOCKET_MIN {
                let pid = u32_at(rec, SO_LAST_PID);
                let peer = LoopbackPeer {
                    pid,
                    uid: Some(u32_at(rec, SO_UID)),
                };
                if pid != 0 {
                    match found {
                        // 同じ 4 つ組が 2 本あって所有者が食い違う = どちらか分からない。
                        // 曖昧なまま「信頼できる pid」を名乗らせない（拒否へ倒す）
                        Some(prev) if prev != peer => return None,
                        _ => found = Some(peer),
                    }
                }
            }
            pos += roundup8(rlen);
        }
        found
    }

    /// `net.inet.tcp.pcblist_n` の生バイト列。
    /// 問い合わせと取得の間にソケットが増えるので、少し大きめに確保して数回試す
    #[cfg(target_os = "macos")]
    fn pcblist_n() -> Option<Vec<u8>> {
        const NAME: &[u8] = b"net.inet.tcp.pcblist_n\0";
        for _ in 0..4 {
            let mut len: libc::size_t = 0;
            // SAFETY: 1 回目はバッファ null + len 0 で必要量を問い合わせる規定の呼び方
            let rc = unsafe {
                libc::sysctlbyname(
                    NAME.as_ptr().cast(),
                    std::ptr::null_mut(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if rc != 0 || len == 0 {
                return None;
            }
            // 問い合わせ後に増えたぶんの余白（足りなければ ENOMEM で取り直す）
            let cap = len + len / 8 + 4096;
            let mut buf = vec![0u8; cap];
            let mut got: libc::size_t = cap;
            // SAFETY: buf は cap バイト確保済みで、got にその長さを渡している
            let rc = unsafe {
                libc::sysctlbyname(
                    NAME.as_ptr().cast(),
                    buf.as_mut_ptr().cast(),
                    &mut got,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if rc == 0 {
                buf.truncate(got.min(cap));
                return Some(buf);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    /// #841: 接続元プロセスの解決が**実際に当たる**か。
    ///
    /// macOS 側は SDK に無い PRIVATE 構造体のオフセットを定数で持っているので、
    /// ここがオフセットの唯一の実測検査になる（ずれたら pid / uid が別の値になって落ちる）。
    /// Windows 側は `GetExtendedTcpTable` の 4 つ組照合の検査。
    /// 自分で張った接続を自分で引くので権限の問題が起きず、両ランナーで走る
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn 自分で張ったループバック接続の所有プロセスを引ける() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let local_port = listener.local_addr().expect("listen addr").port();
        let mut client = std::net::TcpStream::connect(("127.0.0.1", local_port)).expect("connect");
        let peer_port = client.local_addr().expect("client addr").port();
        let _accepted = listener.accept().expect("accept");
        // 接続が確立したことを確かめてから表を引く（未確立だと行がまだ無い）
        client.write_all(b"x").expect("write");

        let peer = super::loopback_tcp_peer(peer_port, local_port)
            .expect("自分で張った接続の所有プロセスは引けるはず");
        assert_eq!(
            peer.pid,
            std::process::id(),
            "接続を張ったのはこのテストプロセス自身"
        );
        #[cfg(unix)]
        {
            assert_eq!(
                peer.uid,
                super::current_uid(),
                "ソケットの所有ユーザーは自分自身"
            );
            assert!(peer.uid.is_some(), "unix は uid を引ける");
        }
        #[cfg(windows)]
        assert_eq!(peer.uid, None, "Windows は所有ユーザーを引かない");

        // 向きを逆にすると別の接続（= 受け側のソケット）を指すので、
        // 「4 つ組をそのまま照合している」ことも確かめる
        let reversed = super::loopback_tcp_peer(local_port, peer_port);
        assert!(
            reversed.is_some_and(|p| p.pid == std::process::id()),
            "受け側も同じプロセスなので引けるが、別のソケットとして引ける"
        );
    }

    /// 引けないときは `None`（呼び出し側が拒否へ倒せる形）
    #[test]
    fn 使われていない組み合わせや不正なポートはnone() {
        // 0 と自己ループは問い合わせる前に弾く
        assert_eq!(super::loopback_tcp_peer(0, 1), None);
        assert_eq!(super::loopback_tcp_peer(1, 0), None);
        assert_eq!(super::loopback_tcp_peer(9, 9), None);
        // 特権ポート同士の接続は（非 root では）張れないので必ず見つからない
        assert_eq!(super::loopback_tcp_peer(1, 2), None);
    }

    /// #1282: コマンドライン / 起動時刻の FFI が**実際に読めるか**。
    ///
    /// 自分自身を対象にするので権限の問題が起きず、CI の Windows ランナーでも走る。
    /// 転記ミス（`UNICODE_STRING` のレイアウト・`FILETIME` の起点・情報クラス番号）は
    /// ここでしか捕まらない（純粋関数のテストは全部 macOS で通ってしまう）
    #[cfg(windows)]
    #[test]
    fn 自分のコマンドラインと起動時刻をffiで読める() {
        let pid = std::process::id();
        let cmd = super::command_line(pid).expect("自分のコマンドラインは読めるはず");
        assert!(!cmd.trim().is_empty(), "空のコマンドライン");
        assert!(
            !crate::tmux_cleanup::split_command_line(&cmd).is_empty(),
            "語に割れない: {cmd:?}"
        );
        let started = super::start_time_unix(pid).expect("自分の起動時刻は読めるはず");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(
            (1_600_000_000..=now + 60).contains(&started),
            "UNIX 秒に見えない（FILETIME の起点がずれている）: {started} / now={now}"
        );
    }

    /// #1282: 名前で引く経路（Toolhelp の列挙 → コマンドライン → 起動時刻）が
    /// 一続きで動くこと。器の列挙はこの形で `tmux.exe` / `psmux.exe` / `pmux.exe` を引く
    #[cfg(windows)]
    #[test]
    fn 自分のプロセスを名前で引ける() {
        let exe = std::env::current_exe().expect("自分の exe パス");
        let name = exe
            .file_name()
            .expect("ファイル名がある")
            .to_string_lossy()
            .to_string();
        let details = super::details_by_name(&[name.as_str()]);
        let me = details
            .iter()
            .find(|d| d.pid == std::process::id())
            .unwrap_or_else(|| panic!("自分（{name}）が名前で引けない: {details:?}"));
        assert!(me.command_line.as_deref().is_some_and(|c| !c.is_empty()));
        assert!(me.started_unix.is_some());
    }

    /// 取得手段が無いプラットフォームは `None` を返す（呼び出し側は見送る）。
    /// 起動時刻だけは macOS にも実装がある（#1296 の pid 再利用の判別に要る）ので別扱い
    #[cfg(not(windows))]
    #[test]
    fn コマンドラインを引けない環境では見送りへ倒れる() {
        assert_eq!(super::command_line(std::process::id()), None);
        assert!(super::details_by_name(&["tmux"]).is_empty());
    }

    /// #1296: macOS の起動時刻は libproc で読める（Windows は上の FFI テストが見る）。
    /// 転記ミス（情報クラス番号・フィールド位置）はここでしか捕まらない
    #[cfg(target_os = "macos")]
    #[test]
    fn 自分の起動時刻をlibprocで読める() {
        let started = super::start_time_unix(std::process::id()).expect("自分の起動時刻");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(
            (1_600_000_000..=now + 60).contains(&started),
            "UNIX 秒に見えない: {started} / now={now}"
        );
        assert_eq!(
            super::start_time_unix(0),
            None,
            "pid 0 は起動時刻を持たない（読めたら判定材料として誤り）"
        );
    }

    /// macOS 以外の unix には取得手段が無い（呼び出し側は見送る）
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn 起動時刻を引けない環境では見送りへ倒れる() {
        assert_eq!(super::start_time_unix(std::process::id()), None);
    }

    /// #1282: 回収の最終ゲートに使う「生きた GUI」の数え方。
    /// **CLI（`tako`）を数えると `tako tmux cleanup` 自身が引っかかる**ので入れない
    #[test]
    fn live_tako_app_pidsはguiだけを自分を除いて返す() {
        let procs = vec![
            super::ProcEntry {
                pid: 10,
                ppid: 1,
                name: "tako-app.exe".into(),
            },
            super::ProcEntry {
                pid: 11,
                ppid: 1,
                name: "TAKO-APP".into(),
            },
            super::ProcEntry {
                pid: 12,
                ppid: 1,
                name: "tako.exe".into(),
            },
            super::ProcEntry {
                pid: 13,
                ppid: 1,
                name: "pwsh.exe".into(),
            },
        ];
        assert_eq!(super::live_tako_app_pids(&procs, 11), vec![10]);
        assert_eq!(super::live_tako_app_pids(&procs, 0), vec![10, 11]);
    }

    use super::*;

    fn p(pid: u32, ppid: u32, name: &str) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            name: name.to_string(),
        }
    }

    #[test]
    fn 子孫は自分自身と全世代を含む() {
        let procs = vec![
            p(1, 0, "system"),
            p(100, 1, "tako-app.exe"),
            p(200, 100, "pwsh.exe"),
            p(300, 200, "node.exe"),
            p(400, 1, "explorer.exe"),
        ];
        let d = descendants_of(&procs, 100);
        assert_eq!(d, HashSet::from([100, 200, 300]));
        assert!(!d.contains(&400), "無関係なプロセスは入らない");
    }

    #[test]
    fn 子を持たないrootは自分だけ返す() {
        let procs = vec![p(1, 0, "system"), p(100, 1, "pwsh.exe")];
        assert_eq!(descendants_of(&procs, 100), HashSet::from([100]));
    }

    #[test]
    fn スナップショットに居ないpidでも自分自身は返る() {
        // 走査とプロセス終了のレースで root が消えていても呼び出し側は空集合を
        // 期待しない（ポートが 1 件も無いだけ）
        assert_eq!(descendants_of(&[], 4242), HashSet::from([4242]));
    }

    #[test]
    fn ppidの循環でも停止する() {
        // pid 再利用で「親が子」になった壊れたスナップショット
        let procs = vec![p(100, 200, "a.exe"), p(200, 100, "b.exe")];
        let d = descendants_of(&procs, 100);
        assert_eq!(d, HashSet::from([100, 200]));
    }

    #[test]
    fn 自己ループの行があっても子は正しく辿れる() {
        let procs = vec![p(100, 100, "self.exe"), p(200, 100, "child.exe")];
        assert_eq!(descendants_of(&procs, 100), HashSet::from([100, 200]));
    }

    #[test]
    fn pid0は辿らない() {
        // System Idle Process を root にすると親無しプロセス群を全部拾ってしまう
        let procs = vec![p(0, 0, "idle"), p(4, 0, "System"), p(100, 4, "smss.exe")];
        assert_eq!(descendants_of(&procs, 0), HashSet::from([0]));
    }

    /// 実機の OS へ問い合わせる（**両プラットフォームで走る**）。#936 の
    /// `stale_binary::pidpath` はこの 1 本に載っているので、Windows 未実装へ
    /// 戻すとここが落ちる
    #[test]
    fn 実行中プロセスの実行ファイルパスを解決できる() {
        let me = std::process::id();
        let path = image_path(me).expect("自プロセスの実行ファイルパスが取れない");
        assert!(path.is_absolute(), "絶対パスでない: {}", path.display());
        assert!(
            path.is_file(),
            "実ファイルを指していない: {}",
            path.display()
        );
        // 自分は cargo のテストバイナリなので、名前にクレート名の断片が入る
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        assert!(
            name.contains("tako") || name.contains("procinfo") || name.contains("test"),
            "自プロセスと無関係なパスを返している: {}",
            path.display()
        );
        // verbatim prefix（`\\?\`）を持ち回らない。#970 の比較相手
        // （`platform::path::canonicalize`）は剥がした形なので、付いていると
        // stale 判定が**常に true** になる
        assert!(
            !path.to_string_lossy().starts_with(r"\\?\"),
            "verbatim prefix が付いている: {}",
            path.display()
        );
    }

    #[test]
    fn 居ないpidは解決できない() {
        // 32bit の pid 空間の上端付近。実在しない値なので必ず None
        assert_eq!(image_path(0xFFFF_FFF0), None);
    }

    fn proc(pid: u32, ppid: u32, name: &str) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            name: name.to_string(),
        }
    }

    /// #1129 の実機の形。`tako setup` の認証段が `claude auth login` を
    /// 自分の子として起こして `.status()` で待っていた
    #[test]
    fn takoの直接の子のエージェントを名指しする() {
        let procs = vec![
            proc(1, 0, "launchd"),
            proc(10, 1, "tako-app"),
            proc(20, 10, "tmux"),
            proc(30, 20, "zsh"),
            proc(40, 30, "tako"),   // ペインのシェルが起こした `tako setup`
            proc(50, 40, "claude"), // ← その子（`claude auth login`）
        ];
        assert_eq!(agent_children_of_tako(&procs), vec![(50, 40)]);
    }

    /// **Windows の名前でも同じ判定になる**（macOS 上から検査できるのが要点）。
    /// 大文字小文字は区別しない（`TAKO.EXE` / `Claude.Exe` も同じ語）
    #[test]
    fn windowsの実行ファイル名でも判定できる() {
        let procs = vec![
            proc(10, 4, "tako-app.exe"),
            proc(20, 10, "tmux.exe"),
            proc(30, 20, "pwsh.exe"),
            proc(40, 30, "TAKO.EXE"),
            proc(50, 40, "Claude.Exe"),
        ];
        assert_eq!(agent_children_of_tako(&procs), vec![(50, 40)]);
    }

    /// 設計どおりの形（**ペインのシェルが起こした**エージェント）は拾わない。
    /// 器を持たない構成ではシェルが `tako-app` の子になるので、
    /// 祖先を辿る判定にすると正しい起動まで落としてしまう
    #[test]
    fn シェルが起こしたエージェントは拾わない() {
        let procs = vec![
            proc(10, 1, "tako-app"),
            // 器なし構成: ペインのシェルが tako-app の直接の子
            proc(30, 10, "zsh"),
            proc(50, 30, "claude"),
            // 器つき構成: シェルは器の子
            proc(20, 10, "tmux"),
            proc(31, 20, "pwsh.exe"),
            proc(51, 31, "claude.exe"),
        ];
        assert!(agent_children_of_tako(&procs).is_empty());
    }

    /// 親が先に死んだ孤児（実機で 46 本中 45 本がこの形）は
    /// tako の子ではないので拾わない = 検査は「いま起こしている側」だけを見る
    #[test]
    fn 孤児になったエージェントは拾わない() {
        let procs = vec![proc(50, 1, "claude"), proc(51, 51, "claude")];
        assert!(agent_children_of_tako(&procs).is_empty());
    }

    #[test]
    fn claude以外のエージェントも対象にする() {
        let procs = vec![
            proc(40, 1, "tako"),
            proc(50, 40, "codex"),
            proc(51, 40, "agy"),
            proc(52, 40, "node"),
        ];
        assert_eq!(agent_children_of_tako(&procs), vec![(50, 40), (51, 40)]);
    }

    #[test]
    fn エージェントの件数は親子を問わず数える() {
        let procs = vec![
            proc(40, 1, "tako"),
            proc(50, 40, "claude"),
            proc(51, 1, "claude.exe"),
            proc(52, 1, "zsh"),
        ];
        assert_eq!(agent_process_count(&procs), 2);
    }

    /// **本番の `tako setup` の対話エージェント（#391）を誤検出しない**。
    /// これは実際に開発機で観測した形（`tako(17296) → claude(20014)`）で、
    /// 機械全体を見る判定だと人が setup を開いているだけで落ちる
    #[test]
    fn 検査対象のインスタンス配下だけを見る() {
        let procs = vec![
            // 別インスタンス（本番の GUI）配下: 人が `tako setup` を開いている
            proc(100, 1, "tako-app"),
            proc(110, 100, "tmux"),
            proc(120, 110, "zsh"),
            proc(130, 120, "tako"),
            proc(140, 130, "claude"), // #391 の setup エージェント（正当）
            // 検査対象（セルフテストのインスタンス）配下: これが #1129 の形
            proc(200, 1, "tako-app"),
            proc(210, 200, "tmux"),
            proc(220, 210, "zsh"),
            proc(230, 220, "tako"),
            proc(240, 230, "claude"),
        ];
        // 機械全体で見ると両方拾ってしまう
        assert_eq!(agent_children_of_tako(&procs), vec![(140, 130), (240, 230)]);
        // ペイン（器の中のシェル）を根に渡すと自分の分だけ
        assert_eq!(
            agent_children_of_tako_under(&procs, &[220]),
            vec![(240, 230)]
        );
        // 器なし構成: PTY の直接の子（シェル）を根に渡しても同じ
        assert_eq!(
            agent_children_of_tako_under(&procs, &[210, 220]),
            vec![(240, 230)]
        );
        // 根が空なら何も拾わない（材料が採れなかったときに誤って落とさない）
        assert!(agent_children_of_tako_under(&procs, &[]).is_empty());
    }

    /// 実機の OS へ問い合わせる。**この環境の構成に依存しない検査だけ**を書く
    #[test]
    fn 実環境のプロセススナップショットを取得できる() {
        let procs = snapshot();
        if cfg!(windows) {
            assert!(!procs.is_empty(), "Windows でプロセス一覧を取得できない");
            let me = std::process::id();
            let mine = procs.iter().find(|p| p.pid == me);
            let mine = mine.expect("自プロセスがスナップショットに居ない");
            assert!(
                mine.name.to_ascii_lowercase().ends_with(".exe"),
                "実行ファイル名が取れていない: {mine:?}"
            );
            // 自分の子孫には必ず自分が含まれる
            assert!(descendants_of(&procs, me).contains(&me));
        }
    }
}
