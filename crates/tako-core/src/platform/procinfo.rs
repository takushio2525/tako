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

/// 実行中プロセスの実行ファイルの絶対パス。取れなければ `None`
/// （プロセスが既に居ない / 権限が無い / 取得手段が無いプラットフォーム）。
///
/// **返すのは「いまのファイル名」**: 実行中に実行ファイルが改名されたら
/// 改名後のパスを返す（モジュールの解説を参照。#936 の stale 検知が依っている）
pub fn image_path(pid: u32) -> Option<std::path::PathBuf> {
    imp::image_path(pid)
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
    base.strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base)
        .to_ascii_lowercase()
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

/// エージェント CLI として動いているプロセスの件数（診断用。親子は問わない）
pub fn agent_process_count(procs: &[ProcEntry]) -> usize {
    procs
        .iter()
        .filter(|p| AGENT_NAMES.contains(&stem_of(&p.name).as_str()))
        .count()
}

#[cfg(windows)]
mod imp {
    use super::{ProcEntry, TcpListenEntry};
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
}

#[cfg(not(windows))]
mod imp {
    use super::{ProcEntry, TcpListenEntry};

    /// macOS の検査は `ports.rs` の libproc 実装が正（このモジュールは使わない）
    pub(super) fn snapshot() -> Vec<ProcEntry> {
        Vec::new()
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
}

#[cfg(test)]
mod tests {
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

    /// **Windows の名前でも同じ判定になる**（macOS 上から検査できるのが要点）
    #[test]
    fn windowsの実行ファイル名でも判定できる() {
        let procs = vec![
            proc(10, 4, "tako-app.exe"),
            proc(20, 10, "tmux.exe"),
            proc(30, 20, "pwsh.exe"),
            proc(40, 30, "TAKO.EXE"),
            proc(50, 40, "claude.exe"),
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
