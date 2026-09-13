//! ipc_socket — Layer 1 IPC の Unix ドメインソケットの置き場（#1441）
//!
//! ## 直した症状
//!
//! ソケットの実体を `<data_dir>/tako.sock` に置いていたので、**パス長が data dir に
//! 比例していた**。`TAKO_ISOLATED=1 TAKO_DATA_DIR=<深いパス>` の隔離起動（worker の
//! scratchpad はセッション ID 込みで深くなる）では `sun_path` の上限を超えて bind が
//! 失敗し、`warning: IPC サーバーを起動できない` の 1 行だけで GUI は普通に立つ。
//! CLI / MCP からは一切操作できないのに見た目は正常なので、原因に気づくまで時間を失う
//! （#782 の計測中に実測: data dir 150 バイト = ソケット 160 バイト > 上限 103）。
//!
//! ## 決め方（この 1 実装が正）
//!
//! 1. `<data_dir>/tako.sock` が上限に収まるならそこ。**浅いパスの既存挙動は変えない**
//!    （再起動をまたいで同じパス = 既存クライアントがそのまま繋ぎ直せる、が固定パスの
//!    存在理由）
//! 2. 収まらないなら `<temp_dir>/tako-<data dir の 16 桁ハッシュ>.sock` へ逃がす。
//!    **pid ではなくハッシュ**なのは 1 の性質（再起動をまたいだ安定）を保つため
//! 3. `$TMPDIR` 自体が深い機のために [`FALLBACK_TEMP`] も候補に持つ
//! 4. どれも収まらなければ [`SocketPathKind::Overflow`]。bind は失敗するが、
//!    **黙って縮退させない**ために理由（バイト長と上限）を申告する口を持つ
//!
//! data dir 側には参照ファイル [`POINTER_NAME`]（実体パスを 1 行）だけを残す。
//! **実体を symlink にしても解決にならない**: `sun_path` の上限は繋ぐ側が
//! `connect()` へ渡すパスに掛かるので、長いパスの symlink を置いても
//! クライアントは同じ上限で弾かれる。だから参照は「読んで短いパスへ繋ぎ直す」
//! ためのテキストにしてある。
//!
//! ## 繋ぐ側との関係
//!
//! 通常の CLI / MCP は discovery（`control.json`）に書かれた**実体パス**を読むので、
//! ここの決め方を知らなくても繋がる。知る必要があるのは discovery ごと失われたとき
//! （= まさに bind が失敗した隔離起動）の診断で、そこは [`resolve_with`] が
//! bind 側と同じ 1 実装を通る。

use std::path::{Path, PathBuf};

/// 固定ソケットのファイル名（`<data_dir>/tako.sock`）
pub const WELL_KNOWN_NAME: &str = "tako.sock";

/// data dir 側に残す参照ファイル名（中身は実体パス 1 行）
pub const POINTER_NAME: &str = "tako.sock.path";

/// `$TMPDIR` 自体が上限に収まらない機のための最後の候補
pub const FALLBACK_TEMP: &str = "/tmp";

/// `sockaddr_un.sun_path` の要素数（**終端 NUL を含む**）。macOS / BSD = 104
pub const SUN_PATH_BSD: usize = 104;
/// 同上。Linux = 108
pub const SUN_PATH_LINUX: usize = 108;

/// この OS でパスに使える最大バイト数（終端 NUL を除く）。
///
/// 実測（macOS 25.4 / 2026-09-13）: 103 バイトまで bind 成功・104 バイトで
/// `AF_UNIX path too long`。std も `bytes.len() >= sun_path.len()` で弾く。
///
/// 純関数側（[`plan_with`] / [`resolve_with`]）が上限を**引数で受ける**ので、
/// macOS から Linux の上限も検査できる（#515 / #905 と同じ作法）
pub const fn max_path_bytes() -> usize {
    if cfg!(target_os = "linux") {
        SUN_PATH_LINUX - 1
    } else {
        SUN_PATH_BSD - 1
    }
}

/// ソケットパスの決まり方（診断・`check_health` に出す識別子）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketPathKind {
    /// `<data_dir>/tako.sock`（従来どおり。浅い data dir はここ）
    WellKnown,
    /// data dir が深すぎるので短いパスへ逃がした
    Shortened,
    /// 逃がし先も上限に収まらない（bind は失敗する）
    Overflow,
    /// pid 入りの一時パス（単体テスト / セルフテスト / セカンダリモード）
    Temp,
    /// Windows の名前付きパイプ（`sun_path` の上限は無い）
    NamedPipe,
}

impl SocketPathKind {
    /// JSON・診断へ出す ASCII 名（`NoticeArea::tag` と同じ作法）
    pub fn as_str(self) -> &'static str {
        match self {
            SocketPathKind::WellKnown => "well_known",
            SocketPathKind::Shortened => "shortened",
            SocketPathKind::Overflow => "overflow",
            SocketPathKind::Temp => "temp",
            SocketPathKind::NamedPipe => "named_pipe",
        }
    }
}

/// 「どこへ bind するか」の決定（[`plan_with`] の結果）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketPlan {
    /// 実際に bind / connect するパス
    pub path: PathBuf,
    pub kind: SocketPathKind,
    /// data dir 側に残す参照ファイル（[`SocketPathKind::WellKnown`] 以外で `Some`）
    pub pointer: Option<PathBuf>,
    /// この判定に使った上限（バイト）
    pub limit: usize,
    /// `<data_dir>/tako.sock` のバイト長（短縮した理由がこれ）
    pub well_known_bytes: usize,
}

impl SocketPlan {
    /// 実体パスのバイト長
    pub fn path_bytes(&self) -> usize {
        path_bytes(&self.path)
    }

    /// 上限を超えている理由を 1 行で。**パスそのものは出さない**
    /// （診断ログへ載せるのは長さと上限だけ = `log_ui_failure` と同じ作法）
    pub fn length_note(&self) -> String {
        format!(
            "kind={} path_bytes={} well_known_bytes={} limit={}",
            self.kind.as_str(),
            self.path_bytes(),
            self.well_known_bytes,
            self.limit,
        )
    }
}

/// パスのバイト長（`sun_path` へ入るのはこのバイト列）
pub fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

/// data dir ごとに安定した短いファイル名（`tako-<16 桁>.sock`）。
///
/// `/a/b` と `/a/b/` が別のソケットにならないよう、区切りの揺れは
/// `Path::components` で正規化してからハッシュする
pub fn short_name(data_dir: &Path) -> String {
    let normalized: PathBuf = data_dir.components().collect();
    let key = normalized.as_os_str().as_encoded_bytes();
    format!("tako-{:016x}.sock", crate::fnv::fnv1a64(key))
}

/// 置き場を決める（**純関数**。env も実ファイルも見ないので、
/// macOS から Linux の上限・深い `$TMPDIR` の機も検査できる）
pub fn plan_with(data_dir: &Path, temp_dir: &Path, limit: usize) -> SocketPlan {
    let well_known = data_dir.join(WELL_KNOWN_NAME);
    let well_known_bytes = path_bytes(&well_known);
    if well_known_bytes <= limit {
        return SocketPlan {
            path: well_known,
            kind: SocketPathKind::WellKnown,
            pointer: None,
            limit,
            well_known_bytes,
        };
    }
    let name = short_name(data_dir);
    let pointer = Some(data_dir.join(POINTER_NAME));
    let mut shortest: Option<PathBuf> = None;
    for base in [temp_dir, Path::new(FALLBACK_TEMP)] {
        let candidate = base.join(&name);
        if path_bytes(&candidate) <= limit {
            return SocketPlan {
                path: candidate,
                kind: SocketPathKind::Shortened,
                pointer,
                limit,
                well_known_bytes,
            };
        }
        if shortest
            .as_ref()
            .is_none_or(|s| path_bytes(&candidate) < path_bytes(s))
        {
            shortest = Some(candidate);
        }
    }
    SocketPlan {
        path: shortest.unwrap_or(well_known),
        kind: SocketPathKind::Overflow,
        pointer,
        limit,
        well_known_bytes,
    }
}

/// #1441 の A/B。`TAKO_1441_LEGACY=1` で**同一バイナリのまま**旧挙動へ戻す
/// （置き場を `<data_dir>/tako.sock` 直置きに固定する = 深い data dir では bind が
/// `sun_path` で落ち、「警告 1 行で黙って縮退する」が再現する）。
///
/// 画面へ出す側の宣言は `tako-app` の `TakoApp::legacy_1441`（番犬がアームごとの
/// 宣言をそちらに求めるため）。**同じ env を読んでいること**は #1441 の番犬が照合する
pub fn legacy_1441() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1441_LEGACY").map(|v| v == "1") == Ok(true))
}

/// env を読む入口（`TAKO_DATA_DIR` → [`crate::paths::data_dir`]、`std::env::temp_dir()`）
pub fn plan() -> Option<SocketPlan> {
    let data_dir = crate::paths::data_dir()?;
    if legacy_1441() {
        // 旧アーム: 深さに関わらず data dir 直下へ置く（bind は上限で落ちる）
        let path = data_dir.join(WELL_KNOWN_NAME);
        let well_known_bytes = path_bytes(&path);
        return Some(SocketPlan {
            path,
            kind: SocketPathKind::WellKnown,
            pointer: None,
            limit: max_path_bytes(),
            well_known_bytes,
        });
    }
    Some(plan_with(
        &data_dir,
        &std::env::temp_dir(),
        max_path_bytes(),
    ))
}

/// data dir 側の参照ファイルを書く（[`SocketPathKind::WellKnown`] では呼ばない）。
/// 中身は実体パス 1 行。読めなくても本体の動作は変わらない（診断のための保険）
pub fn write_pointer(plan: &SocketPlan) -> std::io::Result<()> {
    let Some(pointer) = plan.pointer.as_ref() else {
        return Ok(());
    };
    if let Some(dir) = pointer.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(pointer, format!("{}\n", plan.path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(pointer, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// data dir 側の参照ファイルを読む（無ければ `None`）
pub fn read_pointer(data_dir: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(data_dir.join(POINTER_NAME)).ok()?;
    let line = raw.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// data dir から「いま繋ぐべきソケットパス」を引く。
///
/// **参照ファイルがあればそれが正**（実際に bind した側が書いた値）。無ければ
/// [`plan_with`] と同じ決め方で導く = 繋ぐ側と bind 側が 1 実装を通る
pub fn resolve_with(data_dir: &Path, temp_dir: &Path, limit: usize) -> PathBuf {
    read_pointer(data_dir).unwrap_or_else(|| plan_with(data_dir, temp_dir, limit).path)
}

/// 起動時に 1 回だけ記録する「IPC の受け口が立ったか」の実測（#1441）。
///
/// GUI を立てずに検証できるよう、`check_health` の文と重さは
/// [`IpcStatus::health_issue`] が持つ（#1160 の `Placement` と同じ作法）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcStatus {
    /// 立った受け口（unix = ソケットパス / windows = パイプ名）。失敗なら `None`
    pub endpoint: Option<String>,
    pub kind: SocketPathKind,
    pub bound: bool,
    /// 試したパスのバイト長（named pipe は 0）
    pub path_bytes: usize,
    /// この OS の上限（named pipe は 0）
    pub limit: usize,
    /// `<data_dir>/tako.sock` のバイト長（短縮した理由。named pipe は 0）
    pub well_known_bytes: usize,
    /// 失敗の理由（`io::Error` の表示）
    pub error: Option<String>,
}

impl IpcStatus {
    /// 上限超過で立たなかったか（案内の出し分けに使う）
    pub fn too_long(&self) -> bool {
        !self.bound && self.limit > 0 && self.path_bytes > self.limit
    }

    /// 診断ログ 1 行ぶん。**パスは載せない**（長さと上限だけ）
    pub fn length_note(&self) -> String {
        format!(
            "kind={} bound={} path_bytes={} well_known_bytes={} limit={}",
            self.kind.as_str(),
            self.bound,
            self.path_bytes,
            self.well_known_bytes,
            self.limit,
        )
    }
}

/// プロセス内の記録。`check_health` が読む
static STATUS: std::sync::Mutex<Option<IpcStatus>> = std::sync::Mutex::new(None);

/// IPC の受け口の実測を記録する（サーバー起動の成否どちらでも呼ぶ）。
///
/// **最初に記録できた成功を保つ**。複数ウィンドウの後発は一時パスで立つので、
/// 上書きさせるとプライマリの受け口が見えなくなる。まだ成功が無ければ上書きする
/// （= 失敗は次の成功まで見え続ける）
pub fn record(status: IpcStatus) {
    let Ok(mut slot) = STATUS.lock() else {
        return;
    };
    if slot.as_ref().is_some_and(|s| s.bound) {
        return;
    }
    *slot = Some(status);
}

/// 記録された IPC の受け口（記録が無ければ `None` = GUI 以外のホスト）
pub fn status() -> Option<IpcStatus> {
    STATUS.lock().ok().and_then(|s| s.clone())
}

/// テスト用に記録を消す（本番経路からは呼ばない）
#[doc(hidden)]
pub fn reset_status_for_test() {
    if let Ok(mut slot) = STATUS.lock() {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: usize = SUN_PATH_BSD - 1;

    fn deep(bytes: usize) -> PathBuf {
        // `/tmp/` + `d` の並びでちょうど `bytes` バイトの data dir を作る
        let head = "/tmp/";
        PathBuf::from(format!("{head}{}", "d".repeat(bytes - head.len())))
    }

    #[test]
    fn 浅いdatadirは固定パスのまま() {
        let plan = plan_with(Path::new("/tmp/tako"), Path::new("/var/tmp"), MAC);
        assert_eq!(plan.kind, SocketPathKind::WellKnown);
        assert_eq!(plan.path, PathBuf::from("/tmp/tako/tako.sock"));
        assert_eq!(plan.pointer, None);
    }

    #[test]
    fn 上限ちょうどは固定パスのまま() {
        // `<data_dir>/tako.sock` がちょうど上限 = 収まる（境界の内側）
        let data = deep(MAC - "/tako.sock".len());
        let plan = plan_with(&data, Path::new("/var/tmp"), MAC);
        assert_eq!(plan.path_bytes(), MAC);
        assert_eq!(plan.kind, SocketPathKind::WellKnown);
    }

    #[test]
    fn 上限を1バイト超えたら短縮する() {
        let data = deep(MAC - "/tako.sock".len() + 1);
        let plan = plan_with(&data, Path::new("/var/tmp"), MAC);
        assert_eq!(plan.kind, SocketPathKind::Shortened);
        assert!(plan.path.starts_with("/var/tmp"), "{:?}", plan.path);
        assert!(plan.path_bytes() <= MAC);
        assert_eq!(plan.pointer, Some(data.join(POINTER_NAME)));
        assert_eq!(plan.well_known_bytes, MAC + 1);
    }

    #[test]
    fn 深いtmpdirはフォールバックへ逃げる() {
        let data = deep(MAC);
        let temp = deep(MAC - 4); // `<temp>/tako-<16 桁>.sock` は必ず上限を超える
        let plan = plan_with(&data, &temp, MAC);
        assert_eq!(plan.kind, SocketPathKind::Shortened);
        assert!(plan.path.starts_with(FALLBACK_TEMP), "{:?}", plan.path);
        assert!(plan.path_bytes() <= MAC);
    }

    #[test]
    fn どこにも収まらなければoverflowを申告する() {
        // 上限を極端に小さくすると `/tmp/tako-<16 桁>.sock` も収まらない
        let plan = plan_with(Path::new("/tmp/x"), Path::new("/var/tmp"), 10);
        assert_eq!(plan.kind, SocketPathKind::Overflow);
        assert!(plan.path_bytes() > plan.limit);
        // 一番短い候補を返す（`/tmp/...` のほう）
        assert!(plan.path.starts_with(FALLBACK_TEMP), "{:?}", plan.path);
    }

    #[test]
    fn 同じdatadirは同じソケット名になる() {
        let a = short_name(Path::new("/tmp/tako-iso-data-42"));
        let b = short_name(Path::new("/tmp/tako-iso-data-42/"));
        assert_eq!(a, b, "末尾の区切りで別のソケットになってはいけない");
        assert_ne!(a, short_name(Path::new("/tmp/tako-iso-data-43")));
        assert_eq!(a.len(), "tako-0123456789abcdef.sock".len());
    }

    #[test]
    fn linuxの上限も検査できる() {
        // macOS で 1 バイト超過する長さでも Linux（107）なら収まる
        let data = deep(SUN_PATH_BSD - "/tako.sock".len());
        assert_eq!(
            plan_with(&data, Path::new("/var/tmp"), MAC).kind,
            SocketPathKind::Shortened
        );
        assert_eq!(
            plan_with(&data, Path::new("/var/tmp"), SUN_PATH_LINUX - 1).kind,
            SocketPathKind::WellKnown
        );
    }

    #[test]
    fn 参照ファイルは実体パスを指す() {
        let dir = std::env::temp_dir().join(format!("tako1441-ptr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = SocketPlan {
            path: PathBuf::from("/tmp/tako-abc.sock"),
            kind: SocketPathKind::Shortened,
            pointer: Some(dir.join(POINTER_NAME)),
            limit: MAC,
            well_known_bytes: 200,
        };
        write_pointer(&plan).unwrap();
        assert_eq!(
            read_pointer(&dir),
            Some(PathBuf::from("/tmp/tako-abc.sock"))
        );
        // 参照が正（bind した側が書いた値を、決め方より優先する）
        assert_eq!(
            resolve_with(&dir, Path::new("/var/tmp"), MAC),
            PathBuf::from("/tmp/tako-abc.sock")
        );
        std::fs::remove_file(dir.join(POINTER_NAME)).unwrap();
        // 参照が無ければ決め方で導く（浅いので固定パス）
        assert_eq!(
            resolve_with(&dir, Path::new("/var/tmp"), MAC),
            dir.join(WELL_KNOWN_NAME)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn 記録は最初の成功を保つ() {
        reset_status_for_test();
        let failed = IpcStatus {
            endpoint: None,
            kind: SocketPathKind::Overflow,
            bound: false,
            path_bytes: 160,
            limit: MAC,
            well_known_bytes: 160,
            error: Some("path must be shorter than SUN_LEN".into()),
        };
        record(failed.clone());
        assert_eq!(status(), Some(failed.clone()));
        assert!(failed.too_long());
        let ok = IpcStatus {
            endpoint: Some("/tmp/tako-abc.sock".into()),
            kind: SocketPathKind::Shortened,
            bound: true,
            path_bytes: 18,
            limit: MAC,
            well_known_bytes: 160,
            error: None,
        };
        record(ok.clone());
        assert_eq!(status(), Some(ok.clone()), "失敗は成功で置き換わる");
        record(failed);
        assert_eq!(
            status(),
            Some(ok),
            "後発ウィンドウの一時パスで成功を上書きしない"
        );
        reset_status_for_test();
    }
}
