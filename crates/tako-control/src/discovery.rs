//! discovery — CLI / MCP 接続情報の永続化（FR-2.2.9）
//!
//! トークンとソケットパスはアプリ起動毎に変わる（トークンはメモリ生成、ソケットは
//! PID 入りパス）ため、環境変数しか手段が無いとアプリ再起動後に外部の長寿命プロセス
//! （tmux 内のエージェント等）が繋ぎ直せない。アプリは起動時に接続情報を
//! **ユーザー専用パーミッション（ファイル 0600 / ディレクトリ 0700。Windows は継承を切った
//! 本人 + SYSTEM の DACL = 境界 `platform::private_file`・#2581）**で書き出し、
//! CLI は環境変数が無い・古い（接続不可 / 認証失敗）場合にこのファイルへ
//! フォールバックする。
//!
//! ## 複数インスタンス（2026-06-12 バグ (8) の恒久対策）
//!
//! 「最新優先の単純上書き」だと、セルフテストの一時インスタンスや二重起動が
//! `control.json` を上書きして exit した時点で、メインインスタンスへの接続が壊れる
//! （AI フルコントロールの全断）。そのため:
//!
//! - 各インスタンスは `instances/control-<pid>.json` に自分の接続情報を書き、
//!   `control.json`（current ポインタ = 最新起動）も従来どおり更新する
//! - CLI は current が死んでいたら instances/ を新しい順に走査し、
//!   **生きているインスタンス**（接続試行で判定）へ自動フォールバックする
//! - 書き出し時に死んだインスタンスのファイルを掃除し、アプリの明示終了時は
//!   自分のファイルを削除する（クラッシュ残骸は次回の掃除と接続プローブが吸収）
//! - セルフテストは `TAKO_DISCOVERY_DIR` で専用の一時ディレクトリへ隔離し、
//!   メインの control.json には一切触らない
//!
//! ## 隔離の印（#2531）
//!
//! `TAKO_ISOLATED=1` の CLI / remote daemon が置き場（`TAKO_DISCOVERY_DIR` / `TAKO_DATA_DIR`）を
//! 明示していなければ、繋ぎ先の候補（[`read_candidates`]）は既定（本番）の置き場を読まずに
//! [`IsolatedWithoutDir`] を返す。tako-app は同じ印で起動すると両方を一時の側へ差し替える（#177）ので、
//! 「隔離の印 = 本番の GUI に触らない」を CLI 側でもそろえる。例外は順番待ち（#1994）の調停役だけ
//! （[`read_candidates_machine_wide`]。マシン全体で 1 つ = 隔離した検証のビルドも本番の順番待ちへ並ぶ）

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tako_core::i18n::Lang;

/// 接続情報ファイルの中身。トークンを含むためログに出さないこと（`conventions.md`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    /// フォーマット版数（後方互換の判断用）
    pub version: u32,
    /// 書き出したアプリの PID（インスタンスファイル名と掃除に使う）
    pub pid: u32,
    /// IPC ソケットパス（`TAKO_SOCKET` 相当）
    pub socket: String,
    /// 認証トークン（`TAKO_TOKEN` 相当）
    pub token: String,
    /// MCP エンドポイント（`TAKO_MCP_URL` 相当。MCP サーバーが立たなければ None）
    pub mcp_url: Option<String>,
}

/// 接続情報の置き場。`TAKO_DISCOVERY_DIR` で差し替え可能（セルフテストの隔離用）
fn base_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TAKO_DISCOVERY_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    tako_core::paths::data_dir()
}

/// current ポインタ（`<base>/control.json` = 最新起動インスタンス）
pub fn info_path() -> Option<PathBuf> {
    base_dir().map(|d| d.join("control.json"))
}

/// インスタンスごとの接続情報ディレクトリ（`<base>/instances/`）
fn instances_dir() -> Option<PathBuf> {
    base_dir().map(|d| d.join("instances"))
}

fn instance_path(pid: u32) -> Option<PathBuf> {
    instances_dir().map(|d| d.join(format!("control-{pid}.json")))
}

/// 接続情報を書き出す（アプリ起動時に呼ぶ）。
/// 自分のインスタンスファイル + current ポインタの両方を更新し、
/// 死んだインスタンスの残骸ファイルを掃除する
pub fn write(info: &ControlInfo) -> io::Result<PathBuf> {
    let path = info_path().ok_or_else(|| crate::config_io::DataDirUnresolved.io_error())?;
    if let Some(instance) = instance_path(info.pid) {
        write_to(&instance, info)?;
    }
    write_to(&path, info)?;
    prune_dead_instances(info.pid);
    prune_unparsable_instances(info.pid);
    Ok(path)
}

/// 接続情報を instances/ のみへ書く（current ポインタ = `control.json` は更新しない）。
/// セカンダリモード（多重起動の後発。Issue #113）用: 生きているプライマリへの
/// CLI / MCP 接続（control.json 経由）を奪わずに、自分もフォールバック候補列には載る
pub fn write_instance_only(info: &ControlInfo) -> io::Result<PathBuf> {
    let instance =
        instance_path(info.pid).ok_or_else(|| crate::config_io::DataDirUnresolved.io_error())?;
    write_to(&instance, info)?;
    Ok(instance)
}

/// この data_dir を最後に所有したインスタンス（control.json の主）がまだ生きていれば
/// その pid を返す（多重起動ガード。Issue #113）。判定は「pid が自分以外」かつ
/// 「その pid が生きた tako-app プロセス」。SIGKILL 済みの残骸 control.json は
/// pid 死亡（または pid 再利用でプロセス名不一致）で None になり通常起動を妨げない
pub fn live_primary_pid() -> Option<u32> {
    let info = read()?;
    (info.pid != std::process::id() && tako_core::ports::is_live_tako_app(info.pid))
        .then_some(info.pid)
}

/// このソケットのパスの持ち主として接続情報（control.json と instances/）に載った、
/// 自分以外の生きた tako-app の pid（#2360）。
///
/// 起動時の生存確認と #2002 の見張りが「繋がらない = 残骸」と読む前に引く。
/// **control.json だけでは足りない**: control.json は最後に起動したプライマリが上書きするので、
/// 消えた・壊れた・別のインスタンスに上書きされたときも instances/ に元の持ち主が残っている。
/// 生死は [`live_primary_pid`] と同じ「生きた tako-app か」（pid の再利用はプロセス名で外す）
pub fn live_owner_of(socket: &str) -> Option<u32> {
    let mut infos: Vec<ControlInfo> = read().into_iter().collect();
    if let Some(dir) = instances_dir() {
        infos.extend(list_instances(&dir));
    }
    owner_among(
        &infos,
        socket,
        std::process::id(),
        tako_core::ports::is_live_tako_app,
    )
}

/// [`live_owner_of`] の判定（IO 抜き）。同じソケットを指す記録のうち、自分以外で `alive` な最初の pid
fn owner_among(
    infos: &[ControlInfo],
    socket: &str,
    me: u32,
    alive: impl Fn(u32) -> bool,
) -> Option<u32> {
    infos
        .iter()
        .filter(|info| info.socket == socket && info.pid != me)
        .map(|info| info.pid)
        .find(|&pid| alive(pid))
}

/// 明示終了時のクリーンアップ（アプリの Quit 経路から呼ぶ）。
/// 自分のインスタンスファイルを消し、current が自分を指していたらそれも消す
/// （死んだ接続先を新規 CLI に掴ませない）。クラッシュ時は呼ばれないが、
/// 残骸は次回起動の掃除と CLI 側の接続プローブで無害化される
pub fn cleanup(pid: u32) {
    if let Some(instance) = instance_path(pid) {
        let _ = std::fs::remove_file(instance);
    }
    if let Some(path) = info_path() {
        if read_from(&path).is_some_and(|info| info.pid == pid) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 死んだインスタンスの残骸ファイルを消す。`keep_pid`（自分）は確認せず残す。
///
/// **判断はソケットではなく pid の生存**（#916）。ソケットパスは
/// `<data_dir>/tako.sock` に固定された（2026-06-23）ので、どの世代の
/// インスタンスファイルも同じパスを指す = 生きている 1 個がいれば
/// `socket_alive` は全件 true を返し、残骸が一切消えなくなっていた
/// （実測: 本番に 160 件・最古は固定パス化と同じ 6/23）
fn prune_dead_instances(keep_pid: u32) {
    let Some(dir) = instances_dir() else {
        return;
    };
    for info in list_instances(&dir) {
        if info.pid == keep_pid {
            continue;
        }
        if !tako_core::platform::process::pid_alive(info.pid) {
            if let Some(path) = instance_path(info.pid) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// 解釈できないインスタンスファイルの残骸を消す。
///
/// `list_instances` はパースできないファイルを**列挙ごと落とす**ので、
/// 形式が変わった世代のファイルは掃除の対象にすらならず永久に残る。
/// 名前から pid が読めれば生存で判断できるので、そこだけは救う（#916）
fn prune_unparsable_instances(keep_pid: u32) {
    let Some(dir) = instances_dir() else {
        return;
    };
    let Ok(reader) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        let Some(pid) = pid_from_instance_name(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        if pid == keep_pid || read_from(&path).is_some() {
            continue;
        }
        if !tako_core::platform::process::pid_alive(pid) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// `control-<pid>.json` から pid を読む（読めなければ None = 触らない）
fn pid_from_instance_name(name: &str) -> Option<u32> {
    name.strip_prefix("control-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// ソケットが接続を受け付けるか（生存プローブ。認証はしない）
#[cfg(unix)]
pub fn socket_alive(socket: &str) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
pub fn socket_alive(_socket: &str) -> bool {
    // Windows named pipe は Phase 6。それまで存在確認しない（候補に残して接続試行に任せる）
    true
}

/// 接続情報を 1 つ書く（tmp に書いて rename）。`remote::tests::issue2581_*` も直に呼ぶ
pub(crate) fn write_to(path: &Path, info: &ControlInfo) -> io::Result<()> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリが無い"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(info)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    // トークンを含むためユーザー専用にする（ソケットの 0600 と同じ防御線。unix は 0700 / 0600、
    // Windows は継承を切った本人 + SYSTEM の DACL = 境界 `platform::private_file`・#2581）。
    // 中身を書くのは置き場とファイルを絞った後
    tako_core::platform::private_file::restrict_dir(dir)?;
    tako_core::platform::private_file::create(&tmp)?.write_all(json.as_bytes())?;
    // 前の回の残骸（作るときの mode は既に在るファイルへ効かない）も同じ形へ揃える
    tako_core::platform::private_file::restrict(&tmp)?;
    std::fs::rename(&tmp, path)
}

/// current（最新起動インスタンス）の接続情報を読む。
/// ファイルが無い・読めない・版数不明なら None（呼び出し側は環境変数のみで判断する）
pub fn read() -> Option<ControlInfo> {
    read_from(&info_path()?)
}

/// `TAKO_ISOLATED=1` なのに発見の置き場（`TAKO_DISCOVERY_DIR` / `TAKO_DATA_DIR`）を明示していないので、
/// 既定（本番）の置き場の接続情報を読まなかった（#2531）。
///
/// `Display` は表示言語のグローバルを引く。CLI は GUI へ送るだけの枝でグローバルを決めないので、
/// 利用者へ返す断りは [`Self::text_in`] に `settings::client_lang` を渡して組む
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsolatedWithoutDir;

impl IsolatedWithoutDir {
    /// 表示言語を明示しての文言
    pub fn text_in(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self, Lang::Ja) => {
                "TAKO_ISOLATED=1 なので既定（本番）の置き場の接続情報は読まない（本番の tako へは\
                 繋がない）。隔離した tako へ繋ぐには、その tako と同じ TAKO_DISCOVERY_DIR か \
                 TAKO_DATA_DIR を指定するか、TAKO_SOCKET と TAKO_TOKEN を渡してください"
            }
            (Self, Lang::En) => {
                "TAKO_ISOLATED=1 is set, so the connection info in the default (production) \
                 location is not read (this never connects to the production tako). To reach an \
                 isolated tako, set the same TAKO_DISCOVERY_DIR or TAKO_DATA_DIR as that tako, \
                 or pass TAKO_SOCKET and TAKO_TOKEN"
            }
        }
    }

    /// 先に試した接続の失敗（死んだ `TAKO_SOCKET` 等。`cause` は同じ表示言語で組んだ文）の後ろへ
    /// この断りを続けた 1 文
    pub fn after(self, cause: &str, lang: Lang) -> String {
        match lang {
            Lang::Ja => format!("{cause}。{}", self.text_in(lang)),
            Lang::En => format!("{cause}. {}", self.text_in(lang)),
        }
    }
}

impl std::fmt::Display for IsolatedWithoutDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for IsolatedWithoutDir {}

/// 既定の置き場を読まない組み合わせか（#2531。純関数 = テストが env を書き換えない）。
/// 隔離の印の読み方は tako-app の一括隔離（#177）と同じ `1` / `true` / `on`、置き場の空の値は
/// 未設定と同じ（[`base_dir`] / `paths::data_dir` と同じ読み方）
pub fn refuses_default_place(
    isolated: Option<&str>,
    discovery_dir: Option<&OsStr>,
    data_dir: Option<&OsStr>,
) -> bool {
    tako_core::tmux_cleanup::is_isolated_value(isolated)
        && discovery_dir.is_none_or(OsStr::is_empty)
        && data_dir.is_none_or(OsStr::is_empty)
}

/// このプロセスの env で [`refuses_default_place`] を引く。A/B は `TAKO_2531_LEGACY=1`
/// （修正前 = 隔離の印でも既定の置き場から繋ぐ）
fn isolation_gate() -> Result<(), IsolatedWithoutDir> {
    if std::env::var_os("TAKO_2531_LEGACY").is_some_and(|v| v == "1") {
        return Ok(());
    }
    let refused = refuses_default_place(
        std::env::var("TAKO_ISOLATED").ok().as_deref(),
        std::env::var_os("TAKO_DISCOVERY_DIR").as_deref(),
        std::env::var_os("TAKO_DATA_DIR").as_deref(),
    );
    if refused {
        Err(IsolatedWithoutDir)
    } else {
        Ok(())
    }
}

/// 繋ぎ先のフォールバック候補列（CLI の `send_request_classified` と remote daemon の
/// `AppIpcClient::connect` が引く）。`TAKO_ISOLATED=1` で置き場を明示していなければ既定（本番）の
/// 置き場を読まずに断る（#2531）。中身は [`read_candidates_machine_wide`] と同じ
pub fn read_candidates() -> Result<Vec<ControlInfo>, IsolatedWithoutDir> {
    isolation_gate()?;
    Ok(candidates_in_base())
}

/// 順番待ち（#1994）の調停役を探す候補列。**隔離の印を見ない**: 調停役はマシン全体で 1 つ
/// （本番の GUI の中）で、隔離した検証のビルド（`run_heavy` / `tako queue`）もそこへ並ぶのが仕様。
/// 呼び手は CLI の順番待ちの口（`queue_send`）だけ（番犬 `issue2531_isolated_discovery` が固定する）
pub fn read_candidates_machine_wide() -> Vec<ControlInfo> {
    candidates_in_base()
}

/// フォールバック候補列: current → 各インスタンス（更新の新しい順）。
/// 重複（同一ソケット）は除く。CLI は先頭から順に接続を試す（バグ (8) の恒久対策:
/// current が一時インスタンスの残骸でも、生きているメインへ自動で届く）
fn candidates_in_base() -> Vec<ControlInfo> {
    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(info) = read() {
        seen.insert(info.socket.clone());
        candidates.push(info);
    }
    if let Some(dir) = instances_dir() {
        let mut instances: Vec<(std::time::SystemTime, ControlInfo)> = list_instances(&dir)
            .into_iter()
            .filter_map(|info| {
                let mtime = instance_path(info.pid)
                    .and_then(|p| std::fs::metadata(p).ok())
                    .and_then(|m| m.modified().ok())?;
                Some((mtime, info))
            })
            .collect();
        instances.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        for (_, info) in instances {
            if seen.insert(info.socket.clone()) {
                candidates.push(info);
            }
        }
    }
    candidates
}

fn list_instances(dir: &Path) -> Vec<ControlInfo> {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    reader
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("control-")
                .is_some_and(|rest| rest.ends_with(".json"))
        })
        .filter_map(|e| read_from(&e.path()))
        .collect()
}

fn read_from(path: &Path) -> Option<ControlInfo> {
    let json = std::fs::read_to_string(path).ok()?;
    let info: ControlInfo = serde_json::from_str(&json).ok()?;
    (info.version == 1).then_some(info)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tako-discovery-test-{}-{name}", std::process::id()))
    }

    fn info(pid: u32, socket: &str) -> ControlInfo {
        ControlInfo {
            version: 1,
            pid,
            socket: socket.into(),
            token: format!("token-{pid}"),
            mcp_url: None,
        }
    }

    #[test]
    fn issue2360_持ち主は同じソケットを指す自分以外の生きたpid() {
        let fixed = "/tmp/tk2360/d/tako.sock";
        let alive = |pid: u32| matches!(pid, 10 | 20 | 99);
        // control.json が別のパスを指していても、instances/ の持ち主を引く
        let infos = vec![
            info(20, "/tmp/tako-20-0.sock"),
            info(30, fixed),
            info(10, fixed),
        ];
        assert_eq!(owner_among(&infos, fixed, 99, alive), Some(10));
        // 自分は持ち主に数えない（#2002 の見張りが自分の記録で奪い返しを止めない）
        assert_eq!(owner_among(&[info(99, fixed)], fixed, 99, alive), None);
        // 死んだ pid・再利用された pid（生きた tako-app ではない）は持ち主ではない
        assert_eq!(owner_among(&[info(30, fixed)], fixed, 99, alive), None);
        // 記録が無い・別のパスだけ
        assert_eq!(owner_among(&[], fixed, 99, alive), None);
        assert_eq!(
            owner_among(&[info(20, "/tmp/other.sock")], fixed, 99, alive),
            None
        );
    }

    #[test]
    fn 書き出しと読み戻しが往復しパーミッションは0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_path("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("control.json");
        let payload = info(42, "/tmp/tako-42-0.sock");
        write_to(&path, &payload).unwrap();
        assert_eq!(read_from(&path), Some(payload.clone()));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "ファイルはユーザー専用");
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700, "ディレクトリはユーザー専用");
        // 上書き = 最新優先
        let newer = info(43, "/tmp/tako-42-0.sock");
        write_to(&path, &newer).unwrap();
        assert_eq!(read_from(&path), Some(newer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 不在や未知の版数はnone() {
        assert_eq!(read_from(&temp_path("missing").join("control.json")), None);
        let dir = temp_path("badver");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("control.json");
        let payload = ControlInfo {
            version: 99,
            ..info(1, "s")
        };
        write_to(&path, &payload).unwrap();
        assert_eq!(read_from(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// バグ (8) の回帰: 一時インスタンスが current を上書きして exit しても、
    /// 候補列に生きているインスタンスが残り、死んだ候補は掃除される。
    ///
    /// #916: 掃除の判断は**ソケットではなく pid の生存**。ソケットパスが
    /// `<data_dir>/tako.sock` に固定されて以降、死んだインスタンスも
    /// 「生きているソケット」を指すので socket 判定では一切消えなくなっていた
    #[test]
    fn 候補列はcurrentと生存インスタンスを返し死骸は掃除される() {
        // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
        if tako_core::run_alone!(候補列はcurrentと生存インスタンスを返し死骸は掃除される)
        {
            return;
        }
        // このテストは env（TAKO_DISCOVERY_DIR）でモジュール全体の置き場を差し替える
        // （子のプロセスの中だけ。親で並走する他のテストの置き場は変わらない）
        let dir = temp_path("candidates");
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("TAKO_DISCOVERY_DIR", &dir);

        std::fs::create_dir_all(&dir).unwrap();
        // 生きているインスタンス = 生きた pid（自分）。死んだ方は実在しない pid。
        // **ソケットパスは両者で同じ**にしてある（固定パス化後の実態。#916 の再現条件）
        let shared_socket = dir.join("tako.sock");
        // **実際に受け付けるソケット**を張る（これが無いと socket 判定でも死骸が
        // 消えてしまい、旧実装との差が出ない = 回帰テストにならない）
        let _listener =
            std::os::unix::net::UnixListener::bind(&shared_socket).expect("テスト用ソケット");
        assert!(
            socket_alive(shared_socket.to_str().unwrap()),
            "前提: ソケットは生きて見える"
        );
        let alive_pid = std::process::id();
        let dead_pid = u32::MAX;
        let alive = info(alive_pid, shared_socket.to_str().unwrap());
        let dead = info(dead_pid, shared_socket.to_str().unwrap());

        // メイン（生存）→ 一時インスタンス（死亡。current を上書きして exit した状況）
        write(&alive).unwrap();
        write(&dead).unwrap();
        // current は死んだ一時インスタンスを指している
        assert_eq!(read().map(|i| i.pid), Some(dead_pid));
        let candidates = read_candidates().expect("置き場を明示している");
        assert_eq!(candidates[0].pid, dead_pid, "先頭は current");
        // 死んだインスタンスのファイルは次の write で掃除される
        // （ソケットが生きて見えても pid が死んでいれば消える）
        write(&alive).unwrap();
        let candidates = read_candidates().expect("置き場を明示している");
        assert!(
            candidates.iter().all(|c| c.pid != dead_pid),
            "死骸が掃除される: {candidates:?}"
        );
        assert!(
            candidates.iter().any(|c| c.pid == alive_pid),
            "生きているメインが候補に残る: {candidates:?}"
        );
        // セカンダリモードの書き込み（Issue #113）: current ポインタを奪わず
        // instances/ のみに載る（プライマリへの既存接続を壊さない）
        let secondary = info(alive_pid + 1, dir.join("secondary.sock").to_str().unwrap());
        write_instance_only(&secondary).unwrap();
        assert_eq!(
            read().map(|i| i.pid),
            Some(alive_pid),
            "current はプライマリのまま"
        );
        assert!(
            read_candidates()
                .unwrap()
                .iter()
                .any(|c| c.pid == alive_pid + 1),
            "セカンダリはフォールバック候補列に載る"
        );
        cleanup(alive_pid + 1);
        // 明示終了のクリーンアップ: 自分のファイルと current が消える
        cleanup(alive_pid);
        assert_eq!(read(), None);
        assert!(read_candidates().unwrap().is_empty());

        // 解釈できない残骸も、名前の pid が死んでいれば掃除される（#916）。
        // 旧実装は list_instances がパース失敗で列挙ごと落とすため永久に残った
        let junk = dir.join("instances").join("control-4294967294.json");
        std::fs::create_dir_all(junk.parent().unwrap()).unwrap();
        std::fs::write(&junk, "{ 旧世代のこわれた形式").unwrap();
        let mine = dir
            .join("instances")
            .join(format!("control-{}.json", std::process::id()));
        std::fs::write(&mine, "{ 自分のは触らない").unwrap();
        write(&info(std::process::id(), shared_socket.to_str().unwrap())).unwrap();
        assert!(!junk.exists(), "死んだ pid の解釈不能な残骸は消える");
        cleanup(std::process::id());

        std::env::remove_var("TAKO_DISCOVERY_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod issue2531_tests {
    use super::*;

    /// 落ちたときに名指す判定の正本
    fn gate_site() -> String {
        let text = include_str!("discovery.rs");
        let line = text
            .lines()
            .position(|l| l.contains("fn read_candidates("))
            .map_or(0, |i| i + 1);
        format!("crates/tako-control/src/discovery.rs:{line}（fn read_candidates）")
    }

    #[test]
    fn 隔離の印で置き場の指定が無いときだけ既定の置き場を読まない() {
        let set = Some(OsStr::new("/tmp/tk2531"));
        let empty = Some(OsStr::new(""));
        // (TAKO_ISOLATED, TAKO_DISCOVERY_DIR, TAKO_DATA_DIR, 既定の置き場を読まないか)
        let cases = [
            (Some("1"), None, None, true),
            (Some("true"), None, None, true),
            (Some("on"), None, None, true),
            (Some("1"), empty, empty, true),
            (Some("1"), set, None, false),
            (Some("1"), None, set, false),
            (Some("1"), empty, set, false),
            (Some("0"), None, None, false),
            (Some(""), None, None, false),
            (Some("yes"), None, None, false),
            (None, None, None, false),
        ];
        for (isolated, discovery, data, want) in cases {
            assert_eq!(
                refuses_default_place(isolated, discovery, data),
                want,
                "TAKO_ISOLATED={isolated:?} TAKO_DISCOVERY_DIR={discovery:?} TAKO_DATA_DIR={data:?}"
            );
        }
    }

    #[test]
    fn 断りは日英で英語に日本語が混ざらない() {
        let ja = IsolatedWithoutDir.text_in(Lang::Ja);
        let en = IsolatedWithoutDir.text_in(Lang::En);
        for text in [ja, en] {
            for name in [
                "TAKO_ISOLATED=1",
                "TAKO_DISCOVERY_DIR",
                "TAKO_DATA_DIR",
                "TAKO_SOCKET",
            ] {
                assert!(text.contains(name), "何を指定すれば繋がるかが無い: {text}");
            }
        }
        assert!(en.is_ascii(), "英語表示に日本語が混ざる: {en}");
        assert_eq!(
            IsolatedWithoutDir.after("cannot connect to the tako app (x: y)", Lang::En),
            format!("cannot connect to the tako app (x: y). {en}")
        );
        assert_eq!(
            IsolatedWithoutDir.after("tako アプリへ接続できない（x: y）", Lang::Ja),
            format!("tako アプリへ接続できない（x: y）。{ja}")
        );
    }

    #[test]
    fn 候補列の門は隔離の印で断り順番待ちの口は既定の置き場を読む() {
        // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
        if tako_core::run_alone!(候補列の門は隔離の印で断り順番待ちの口は既定の置き場を読む)
        {
            return;
        }
        for key in ["TAKO_DISCOVERY_DIR", "TAKO_DATA_DIR", "TAKO_2531_LEGACY"] {
            std::env::remove_var(key);
        }
        // テストプロセスの既定の置き場（`paths::test_data_dir`。本番の置き場ではない）に接続情報を置く
        let path = info_path().expect("テストの置き場は決まる");
        let me = ControlInfo {
            version: 1,
            pid: std::process::id(),
            socket: "/tmp/tk2531-unit.sock".into(),
            token: "tk2531".into(),
            mcp_url: None,
        };
        write_to(&path, &me).expect("接続情報を書ける");

        std::env::set_var("TAKO_ISOLATED", "1");
        assert_eq!(
            read_candidates(),
            Err(IsolatedWithoutDir),
            "隔離の印・置き場の指定なしで既定の置き場を読んだ（判定の正本: {}）",
            gate_site()
        );
        assert_eq!(
            read_candidates_machine_wide(),
            vec![me.clone()],
            "順番待ちの口は隔離の印でも既定の置き場の調停役を引く"
        );
        // A/B の腕: 修正前は隔離の印でも既定の置き場から引いた
        std::env::set_var("TAKO_2531_LEGACY", "1");
        assert_eq!(read_candidates(), Ok(vec![me.clone()]));
        std::env::remove_var("TAKO_2531_LEGACY");
        // 印が無ければ従来どおり
        std::env::set_var("TAKO_ISOLATED", "0");
        assert_eq!(read_candidates(), Ok(vec![me]));
        let _ = std::fs::remove_file(&path);
    }
}
