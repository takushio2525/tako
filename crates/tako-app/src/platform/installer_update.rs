//! アプリ内更新の Windows の腕（抽象境界 B14。#2434）
//!
//! ## なぜ macOS と別の腕が要るか
//!
//! macOS は zip を展開して `.app` を差し替え、`open -n` で起動し直す（`ditto` /
//! `bundle_install` / `os_integration::open_new_instance`）。Windows の配布物は
//! Inno Setup のインストーラー（`tako-<tag>-windows-x86_64.exe`。#587）で、
//! **動いている exe は自分では上書きできない**。そこで差し替えはインストーラーに任せ、
//! tako は「検査して起動して抜ける」だけを受け持つ。
//!
//! ## 順序（起動 → 終了 → インストール → 起動し直し）
//!
//! 1. **取得**: GitHub Releases のインストーラーを一時 dir へ受ける（[`download`]）
//! 2. **検査**: 取得元の URL（[`check_origin`]）・サイズ・sha256（GitHub API の asset の
//!    `digest`）・PE ヘッダ（`MZ`）。1 つでも外れたら受けたファイルを消して断る。
//!    配布元がサイズや sha256 を示していなければ**検査できないので起動しない**
//! 3. **起動**: [`installer_args`] の無言の指定で起こし（[`launch`]）、tako は正規の終了経路
//!    （layout の保存を通る `cx.quit()`）で抜ける
//! 4. **インストール**: インストーラーは `/TAKOWAITPID` の pid（= 抜ける tako）が消えるのを
//!    待ってから上書きする（`installer/windows/tako.iss` の `InitializeSetup`）。
//!    残っていれば Restart Manager が閉じる（`CloseApplications=yes`。無言のときは常に閉じる）
//! 5. **起動し直し**: インストーラーは終わり際（`DeinitializeSetup`）に、成功なら新しい
//!    `{app}\tako-app.exe`、失敗・中断なら `/TAKORELAUNCH` に渡した今の exe を起こす
//!
//! 1〜3 のどこで失敗しても tako は動いたまま（何も上書きしていない）で、応答に理由が出る。
//! 4 で失敗したときは Inno Setup のロールバックで元の版に戻り、5 で起動し直す。
//! 何が起きたかは `/LOG` に渡したセットアップのログ（受けたインストーラーの隣）に残る。
//!
//! ## 無言の指定の根拠（Inno Setup の仕様）
//!
//! - `/SILENT`: ウィザードを出さず進捗の窓だけを出す（数秒 tako が消えるあいだ何が起きて
//!   いるかが見える）。`/VERYSILENT` は窓も出さないので採らない
//! - `/SUPPRESSMSGBOXES`: 問い合わせの窓を出さず既定の答えで進む（止まらない）
//! - `/NORESTART`: 無言のときは再起動が要ると問わずに OS ごと再起動しうるので止める
//! - `/CLOSEAPPLICATIONS`: 使用中のファイルを握るアプリを Restart Manager で閉じる
//!   （`tako.iss` の `CloseApplications=yes` と同じ向き。コマンドラインで明示する）
//! - 入れる先・タスク（PATH への追加・デスクトップのアイコン）は `UsePreviousAppDir` /
//!   `UsePreviousTasks`（既定 yes）で前回のものが使われる。設定（`%APPDATA%\tako`）は
//!   インストーラーの `[Files]` の外なので触られない
//!
//! ## プラットフォームの分岐
//!
//! 判断（取得元・検査・引数・置き場）はすべて純関数で、**macOS の `cargo test` からも
//! Windows の腕を検証できる**（#515 / #595 と同じ方針）。`cfg` は引数をコマンドラインへ
//! 載せる 1 か所（[`imp`]）だけ。

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sha2::Digest as _;

/// 接続の待ち（応答の頭が来るまで）
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 本体の受信の上限。インストーラーは 25 MB 前後なので、遅い回線でも桁で余る
const BODY_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// 一時 dir の下に置く、この経路専用のフォルダ名
pub const STAGING_DIR_NAME: &str = "tako-update-installer";

/// 配布元（GitHub Releases の asset）が示した、受け取るべきインストーラー
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub asset_name: &'a str,
    pub url: &'a str,
    /// asset の `size`
    pub size: Option<u64>,
    /// asset の `digest`（`sha256:<16 進 64 桁>`）
    pub digest: Option<&'a str>,
}

/// 検査を通ったインストーラー
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInstaller {
    pub path: PathBuf,
    /// セットアップのログの書き先（`/LOG`）。受けたインストーラーの隣
    pub log_path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

/// この経路の断り。どれも「tako は今の版のまま動いている」状態で返る
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallerError {
    /// 取得元が tako の GitHub Releases のインストーラーではない
    Origin(String),
    /// 配布元がサイズか sha256 を示していない（検査できないものは起動しない）
    Unverifiable,
    /// 接続・受信の失敗（途中で切れた場合を含む）
    Network(String),
    Http(u16),
    /// 受けたバイト数が配布元の示すサイズと合わない
    Size {
        got: u64,
        want: u64,
    },
    /// sha256 が配布元の示す値と合わない
    Digest,
    /// 中身が実行ファイル（PE）ではない
    NotExecutable,
    Io(String),
    /// インストーラーを起動できない
    Launch(String),
    /// 動いている tako がインストーラーで入れた場所に居ない（ポータブル版・開発ビルド）
    NotInstalled,
}

impl InstallerError {
    /// 表示言語を明示しての文言（#2365。(バリアント, 言語) の網羅 match。日本語は #2365 より前の
    /// 固定文とバイト一致）。更新の口の断り（`update_checker::UpdateRefusal`）が枠と同じ言語で引く
    pub fn text_in(&self, lang: tako_core::i18n::Lang) -> String {
        use tako_core::i18n::Lang::{En, Ja};
        match (self, lang) {
            (Self::Origin(url), Ja) => format!(
                "インストーラーの取得元が tako の GitHub Releases ではないため取りに行かない: {url}"
            ),
            (Self::Origin(url), En) => format!(
                "the installer does not come from tako's GitHub Releases, so it is not downloaded: {url}"
            ),
            (Self::Unverifiable, Ja) => {
                "配布元がインストーラーのサイズか sha256 を示していないため、検査できないので起動しない"
                    .into()
            }
            (Self::Unverifiable, En) => {
                "the release does not give the installer's size or sha256, so it cannot be checked and is not started"
                    .into()
            }
            (Self::Network(e), Ja) => format!("インストーラーのダウンロードに失敗: {e}"),
            (Self::Network(e), En) => format!("the installer download failed: {e}"),
            (Self::Http(status), Ja) => {
                format!("インストーラーのダウンロードに失敗（HTTP {status}）")
            }
            (Self::Http(status), En) => format!("the installer download failed (HTTP {status})"),
            (Self::Size { got, want }, Ja) => format!(
                "インストーラーの大きさが配布元の値と合わない（受信 {got} バイト / 配布元 {want} バイト。\
                 途中で切れた可能性がある）"
            ),
            (Self::Size { got, want }, En) => format!(
                "the installer size does not match the release (received {got} bytes / release {want} bytes; \
                 the download may have been cut off)"
            ),
            (Self::Digest, Ja) => {
                "インストーラーの sha256 が配布元の値と合わないので起動しない".into()
            }
            (Self::Digest, En) => {
                "the installer's sha256 does not match the release, so it is not started".into()
            }
            (Self::NotExecutable, Ja) => {
                "受けたファイルが Windows の実行ファイルではないので起動しない".into()
            }
            (Self::NotExecutable, En) => {
                "the downloaded file is not a Windows executable, so it is not started".into()
            }
            (Self::Io(e), Ja) => format!("インストーラーの保存に失敗: {e}"),
            (Self::Io(e), En) => format!("cannot save the installer: {e}"),
            (Self::Launch(e), Ja) => format!("インストーラーを起動できない: {e}"),
            (Self::Launch(e), En) => format!("cannot start the installer: {e}"),
            (Self::NotInstalled, Ja) => {
                "この tako はインストーラーで入れた場所から動いていない（ポータブル版・開発ビルド）ため、\
                 アプリ内では更新できない。リリースページから同じ形式の配布物を取り直してください"
                    .into()
            }
            (Self::NotInstalled, En) => {
                "this tako does not run from where the installer put it (a portable or development build), \
                 so it cannot update itself. Download the same kind of package again from the release page"
                    .into()
            }
        }
    }
}

impl std::fmt::Display for InstallerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

/// インストーラーへ渡す 1 つの指定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallerArg {
    Flag(&'static str),
    /// `/名前="値"`。値はパス
    Path(&'static str, PathBuf),
    /// `/名前=値`
    Number(&'static str, u32),
}

/// 起動し直す対象の exe を渡す指定（`tako.iss` の `{param:TAKORELAUNCH|}` が読む）
pub const RELAUNCH_PARAM: &str = "/TAKORELAUNCH";
/// 抜けるのを待つ tako の pid を渡す指定（`tako.iss` の `{param:TAKOWAITPID|0}` が読む）
pub const WAIT_PID_PARAM: &str = "/TAKOWAITPID";

/// 無言で上書きして起動し直すための指定（根拠はモジュール冒頭の節）
pub fn installer_args(log_path: &Path, wait_pid: u32, relaunch: &Path) -> Vec<InstallerArg> {
    vec![
        InstallerArg::Flag("/SILENT"),
        InstallerArg::Flag("/SUPPRESSMSGBOXES"),
        InstallerArg::Flag("/NORESTART"),
        InstallerArg::Flag("/CLOSEAPPLICATIONS"),
        InstallerArg::Flag("/SP-"),
        InstallerArg::Path("/LOG", log_path.to_path_buf()),
        InstallerArg::Number(WAIT_PID_PARAM, wait_pid),
        InstallerArg::Path(RELAUNCH_PARAM, relaunch.to_path_buf()),
    ]
}

/// Windows のコマンドラインの尾（Inno Setup の書式どおり `/名前="値"`）。
///
/// std の引数の引用（引数全体を `"` で囲む）に頼らず、Inno Setup の説明書の形
/// （`/LOG="filename"`）で組む。Windows のパスは `"` を含められないので値の引用はこれで閉じる。
/// 起動で使うのは Windows だけ（他の OS では単体テストが形を確かめる）
#[cfg_attr(not(windows), allow(dead_code))]
pub fn command_tail(args: &[InstallerArg]) -> String {
    args.iter()
        .map(|arg| match arg {
            InstallerArg::Flag(flag) => (*flag).to_string(),
            InstallerArg::Path(name, path) => format!("{name}=\"{}\"", path.display()),
            InstallerArg::Number(name, n) => format!("{name}={n}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 取得元の検査。`https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>` の形で、
/// 最後の要素が選んだ asset の名前と一致するときだけ通す
pub fn check_origin(url: &str, owner_repo: &str, asset_name: &str) -> Result<(), InstallerError> {
    let refuse = || InstallerError::Origin(url.to_string());
    if !is_installer_name(asset_name) {
        return Err(refuse());
    }
    let prefix = format!("https://github.com/{owner_repo}/releases/download/");
    let rest = url.strip_prefix(&prefix).ok_or_else(refuse)?;
    let (tag, name) = rest.split_once('/').ok_or_else(refuse)?;
    let tag_ok = !tag.is_empty()
        && tag != "."
        && tag != ".."
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if !tag_ok || name != asset_name {
        return Err(refuse());
    }
    Ok(())
}

/// インストーラーとして受け取ってよい名前か（区切り・親への参照を含まない `.exe`）
fn is_installer_name(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".exe")
        && !name.contains(['/', '\\', ':'])
        && !name.starts_with('.')
}

/// asset の `digest`（`sha256:<16 進 64 桁>`）から 16 進を取り出す（小文字へ揃える）
pub fn parse_sha256_digest(digest: &str) -> Option<String> {
    let hex = digest.trim().strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// 受けたバイト列の 16 進の sha256
fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 本体を `part` へ書きながら検査する。**失敗したら `part` を消す**。
///
/// サイズを超えた時点で打ち切る（配布元の示す値より大きいものは受けきらない）。
/// 読みの途中のエラー（接続が切れた）は `Network`、足りないまま終わったら `Size`
pub fn receive(
    mut reader: impl Read,
    part: &Path,
    want_size: u64,
    want_sha256: &str,
) -> Result<(u64, String), InstallerError> {
    let result = (|| {
        let mut file =
            std::fs::File::create(part).map_err(|e| InstallerError::Io(e.to_string()))?;
        let mut hasher = sha2::Sha256::new();
        let mut head = [0u8; 2];
        let mut got = 0u64;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(InstallerError::Network(e.to_string())),
            };
            if got < 2 {
                let at = got as usize;
                let take = (2 - at).min(n);
                head[at..at + take].copy_from_slice(&buf[..take]);
            }
            got += n as u64;
            if got > want_size {
                return Err(InstallerError::Size {
                    got,
                    want: want_size,
                });
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .map_err(|e| InstallerError::Io(e.to_string()))?;
        }
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|e| InstallerError::Io(e.to_string()))?;
        if got != want_size {
            return Err(InstallerError::Size {
                got,
                want: want_size,
            });
        }
        let sha256 = hex_of(&hasher.finalize());
        if !sha256.eq_ignore_ascii_case(want_sha256) {
            return Err(InstallerError::Digest);
        }
        if &head != b"MZ" {
            return Err(InstallerError::NotExecutable);
        }
        Ok((got, sha256))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
}

/// この経路の一時 dir の根（`%TEMP%\tako-update-installer`）
pub fn staging_root() -> PathBuf {
    std::env::temp_dir().join(STAGING_DIR_NAME)
}

/// 今回の取得の置き場を `root` の下に作り、前回までの置き場を片付ける。
///
/// 前回のインストーラーがまだ動いていれば消せないので、失敗は黙って残す
/// （次の更新のときにまた試す）。片付けるのは `root` の直下だけ
pub fn fresh_attempt_dir(root: &Path) -> Result<PathBuf, InstallerError> {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.parent() == Some(root) && path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dir = root.join(format!("{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| InstallerError::Io(e.to_string()))?;
    Ok(dir)
}

/// 配布元の示した値で検査できるか（大きさと sha256 が揃っているか）。
/// 揃っていなければ**取りに行く前に**断る。戻り値は `(大きさ, 16 進の sha256)`
pub fn checkable(expected: &Expected<'_>) -> Result<(u64, String), InstallerError> {
    if !is_installer_name(expected.asset_name) {
        return Err(InstallerError::Origin(expected.url.to_string()));
    }
    match (
        expected.size.filter(|n| *n > 0),
        expected.digest.and_then(parse_sha256_digest),
    ) {
        (Some(size), Some(sha256)) => Ok((size, sha256)),
        _ => Err(InstallerError::Unverifiable),
    }
}

/// インストーラーを `dir` へ受けて検査する（取得元の検査は呼び出し側が先に済ませる）
pub fn download(
    expected: &Expected<'_>,
    dir: &Path,
    user_agent: &str,
) -> Result<PreparedInstaller, InstallerError> {
    let (want_size, want_sha256) = checkable(expected)?;
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(CONNECT_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .user_agent(user_agent)
        .build()
        .new_agent();
    let response = agent
        .get(expected.url)
        .call()
        .map_err(|e| InstallerError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(InstallerError::Http(status));
    }
    let part = dir.join(format!("{}.part", expected.asset_name));
    let (size, sha256) = receive(
        response.into_body().into_reader(),
        &part,
        want_size,
        &want_sha256,
    )?;
    let path = dir.join(expected.asset_name);
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        InstallerError::Io(e.to_string())
    })?;
    Ok(PreparedInstaller {
        path,
        log_path: dir.join("setup.log"),
        size,
        sha256,
    })
}

/// 検査済みのインストーラーを起こす。tako はこの後すぐ抜ける前提
/// （インストーラーは `wait_pid` が消えるのを待ってから上書きする）
pub fn launch(
    prepared: &PreparedInstaller,
    wait_pid: u32,
    relaunch: &Path,
) -> Result<Child, InstallerError> {
    let args = installer_args(&prepared.log_path, wait_pid, relaunch);
    let mut cmd = Command::new(&prepared.path);
    imp::push_args(&mut cmd, &args);
    // インストーラーは GUI なので窓の有無は変わらないが、GUI から起こす子は境界を通す（#586）
    tako_core::platform::process::no_console_window(&mut cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| InstallerError::Launch(e.to_string()))
}

/// 動いている exe がインストーラーで入れた先に居るか（#2434）。
///
/// インストーラーは前回の入れた先へ上書きするので、別の場所の exe（ポータブル版・
/// 開発ビルド）を更新すると「別の場所に新しい版が入り、動いている方は古いまま」になる。
/// 比べ方は Windows のパスの流儀（大文字小文字を区別しない・`/` と `\` を同一視・
/// 末尾の区切りを無視・`\\?\` を剥がす）
pub fn runs_from_install_dir(install_location: Option<&Path>, exe: &Path) -> bool {
    let Some(install) = install_location else {
        return false;
    };
    // `Path::parent` は実行環境の区切りで切るので使わない（macOS の単体からも
    // Windows のパスを同じ規則で比べられるよう、文字列のまま Windows の流儀で切る）
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
        s.trim_end_matches('\\').to_lowercase()
    };
    let exe = norm(exe);
    let Some((exe_dir, _)) = exe.rsplit_once('\\') else {
        return false;
    };
    let install = norm(install);
    !install.is_empty() && install == exe_dir
}

/// 起動し直す exe（今動いている tako-app）
pub fn relaunch_target() -> Result<PathBuf, InstallerError> {
    std::env::current_exe().map_err(|e| InstallerError::Launch(e.to_string()))
}

#[cfg(windows)]
mod imp {
    use super::{command_tail, InstallerArg};
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// std の引用（引数全体を `"` で囲む）ではなく Inno Setup の書式でそのまま載せる
    pub(super) fn push_args(cmd: &mut Command, args: &[InstallerArg]) {
        cmd.raw_arg(command_tail(args));
    }
}

#[cfg(not(windows))]
mod imp {
    use super::InstallerArg;
    use std::process::Command;

    /// Windows 以外で起こすのはテストの偽のインストーラーだけ。引数は 1 つずつ渡す
    pub(super) fn push_args(cmd: &mut Command, args: &[InstallerArg]) {
        for arg in args {
            cmd.arg(super::plain_arg(arg));
        }
    }
}

/// 引数 1 つを `/名前=値` の素の形にする（Windows 以外の偽のインストーラー向け）
#[cfg_attr(windows, allow(dead_code))]
fn plain_arg(arg: &InstallerArg) -> OsString {
    match arg {
        InstallerArg::Flag(flag) => OsString::from(flag),
        InstallerArg::Path(name, path) => {
            let mut s = OsString::from(format!("{name}="));
            s.push(path.as_os_str());
            s
        }
        InstallerArg::Number(name, n) => OsString::from(format!("{name}={n}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const OWNER_REPO: &str = "takushio2525/tako";
    const ASSET: &str = "tako-v0.8.40-windows-x86_64.exe";

    /// テスト専用の一時 dir（消す前に一時 dir の中であることを確かめる）
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tako-test-2434-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            assert!(
                self.0.starts_with(std::env::temp_dir()),
                "一時 dir の外を消そうとしている: {}",
                self.0.display()
            );
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_installer(len: usize) -> Vec<u8> {
        let mut bytes = vec![0x41u8; len];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex_of(&sha2::Sha256::digest(bytes))
    }

    #[test]
    fn 取得元はtakoのリリースのインストーラーだけを通す() {
        let ok = format!("https://github.com/{OWNER_REPO}/releases/download/v0.8.40/{ASSET}");
        assert_eq!(check_origin(&ok, OWNER_REPO, ASSET), Ok(()));
        let refused = [
            // 平文
            format!("http://github.com/{OWNER_REPO}/releases/download/v0.8.40/{ASSET}"),
            // 別のリポジトリ
            format!("https://github.com/someone/tako/releases/download/v0.8.40/{ASSET}"),
            // 別のホスト
            format!("https://example.com/{OWNER_REPO}/releases/download/v0.8.40/{ASSET}"),
            // 選んだ asset と違う名前
            format!("https://github.com/{OWNER_REPO}/releases/download/v0.8.40/other.exe"),
            // 階層の差し込み・親への参照
            format!("https://github.com/{OWNER_REPO}/releases/download/v0.8.40/x/{ASSET}"),
            format!("https://github.com/{OWNER_REPO}/releases/download/../{ASSET}"),
            // 問い合わせ文字列の付け足し
            format!("https://github.com/{OWNER_REPO}/releases/download/v0.8.40/{ASSET}?x=1"),
            // タグが空
            format!("https://github.com/{OWNER_REPO}/releases/download//{ASSET}"),
        ];
        for url in &refused {
            assert!(
                matches!(
                    check_origin(url, OWNER_REPO, ASSET),
                    Err(InstallerError::Origin(_))
                ),
                "通してはいけない: {url}"
            );
        }
        // asset の名前そのものが exe でない・区切りを含む
        for name in [
            "tako-v0.8.40-windows-x86_64.zip",
            "..\\x.exe",
            "a/b.exe",
            ".exe",
        ] {
            let url = format!("https://github.com/{OWNER_REPO}/releases/download/v0.8.40/{name}");
            assert!(check_origin(&url, OWNER_REPO, name).is_err(), "{name}");
        }
    }

    #[test]
    fn digestはsha256の64桁だけを読む() {
        let hex = "94154135a2b1fd1a330732461d7b833a094cb8be2f2e7b195d6338369a27be88";
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{hex}")).as_deref(),
            Some(hex)
        );
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{}", hex.to_uppercase())).as_deref(),
            Some(hex)
        );
        for bad in [
            hex.to_string(),
            format!("sha512:{hex}"),
            // 切り出し安全: hex は 16 進の ASCII だけ
            format!("sha256:{}", &hex[1..]),
            // 切り出し安全: 同上
            format!("sha256:{}zz", &hex[2..]),
            String::new(),
        ] {
            assert_eq!(parse_sha256_digest(&bad), None, "{bad}");
        }
    }

    #[test]
    fn 受信は大きさとsha256とpeヘッダが揃ったときだけ通す() {
        let tmp = Scratch::new("receive");
        let part = tmp.0.join("x.exe.part");
        let body = fake_installer(200_000);
        let sha = sha256_hex(&body);

        let (got, digest) = receive(Cursor::new(body.clone()), &part, 200_000, &sha).unwrap();
        assert_eq!((got, digest.as_str()), (200_000, sha.as_str()));
        assert_eq!(std::fs::read(&part).unwrap(), body);

        // 途中で切れた（足りないまま終わった）
        // 切り出し安全: body はバイト列（Vec<u8>）で、文字列ではない
        let short: &[u8] = &body[..150_000];
        assert_eq!(
            receive(Cursor::new(short.to_vec()), &part, 200_000, &sha),
            Err(InstallerError::Size {
                got: 150_000,
                want: 200_000
            })
        );
        assert!(!part.exists(), "失敗したら受けかけのファイルを残さない");

        // 配布元の値より大きい
        let mut long = body.clone();
        long.extend_from_slice(&[0u8; 10]);
        assert!(matches!(
            receive(Cursor::new(long), &part, 200_000, &sha),
            Err(InstallerError::Size { want: 200_000, .. })
        ));
        assert!(!part.exists());

        // 中身の差し替え（同じ大きさ・違う sha256）
        let mut tampered = body.clone();
        tampered[100_000] ^= 0xff;
        assert_eq!(
            receive(Cursor::new(tampered), &part, 200_000, &sha),
            Err(InstallerError::Digest)
        );
        assert!(!part.exists());

        // sha256 は合うが実行ファイルではない（配布側の取り違え）
        let not_pe = vec![0u8; 1000];
        let not_pe_sha = sha256_hex(&not_pe);
        assert_eq!(
            receive(Cursor::new(not_pe), &part, 1000, &not_pe_sha),
            Err(InstallerError::NotExecutable)
        );
        assert!(!part.exists());
    }

    /// 読みの途中で接続が切れる偽の本体
    struct CutAfter {
        data: Vec<u8>,
        at: usize,
        pos: usize,
    }
    impl Read for CutAfter {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.at {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "connection reset by peer",
                ));
            }
            let n = buf.len().min(self.at - self.pos);
            // 切り出し安全: data はバイト列（Vec<u8>）で、文字列ではない
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn 受信の途中で切れたら消して接続の失敗を返す() {
        let tmp = Scratch::new("cut");
        let part = tmp.0.join("x.exe.part");
        let body = fake_installer(300_000);
        let sha = sha256_hex(&body);
        let reader = CutAfter {
            data: body,
            at: 70_000,
            pos: 0,
        };
        let err = receive(reader, &part, 300_000, &sha).unwrap_err();
        assert!(matches!(err, InstallerError::Network(_)), "{err:?}");
        assert!(!part.exists());
    }

    /// 偽の配布元（127.0.0.1 の 1 回だけ応える HTTP）。`cut` を渡すとその長さで切る
    fn serve_once(body: Vec<u8>, declared: usize, cut: Option<usize>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut req = [0u8; 4096];
            let _ = stream.read(&mut req);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\n\
                 Content-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            let end = cut.unwrap_or(body.len()).min(body.len());
            // 切り出し安全: body はバイト列（Vec<u8>）で、文字列ではない
            let _ = stream.write_all(&body[..end]);
            let _ = stream.flush();
            // 切るときはそのまま閉じる（Content-Length に足りないまま終わる）
        });
        format!("http://{addr}/releases/download/v0.8.40/{ASSET}")
    }

    #[test]
    fn 偽の配布元から受けて検査を通す() {
        let tmp = Scratch::new("serve-ok");
        let body = fake_installer(500_000);
        let digest = format!("sha256:{}", sha256_hex(&body));
        let url = serve_once(body.clone(), body.len(), None);
        let expected = Expected {
            asset_name: ASSET,
            url: &url,
            size: Some(body.len() as u64),
            digest: Some(&digest),
        };
        let prepared = download(&expected, &tmp.0, "tako-test").unwrap();
        assert_eq!(prepared.path, tmp.0.join(ASSET));
        assert_eq!(prepared.log_path, tmp.0.join("setup.log"));
        assert_eq!(prepared.size, 500_000);
        assert_eq!(std::fs::read(&prepared.path).unwrap(), body);
        assert!(!tmp.0.join(format!("{ASSET}.part")).exists());
    }

    #[test]
    fn 偽の配布元が途中で切れたら何も残さず断る() {
        let tmp = Scratch::new("serve-cut");
        let body = fake_installer(500_000);
        let digest = format!("sha256:{}", sha256_hex(&body));
        let url = serve_once(body.clone(), body.len(), Some(123_456));
        let expected = Expected {
            asset_name: ASSET,
            url: &url,
            size: Some(body.len() as u64),
            digest: Some(&digest),
        };
        let err = download(&expected, &tmp.0, "tako-test").unwrap_err();
        // ureq が「Content-Length に足りない」を読みのエラーで返すか、短いまま終えるかは
        // 実装次第なので、どちらでも断ること・何も残さないことを見る
        assert!(
            matches!(
                err,
                InstallerError::Network(_) | InstallerError::Size { got: 123_456, .. }
            ),
            "{err:?}"
        );
        assert!(!tmp.0.join(ASSET).exists());
        assert!(!tmp.0.join(format!("{ASSET}.part")).exists());
    }

    #[test]
    fn 配布元のsha256と合わなければ断る() {
        let tmp = Scratch::new("serve-digest");
        let body = fake_installer(50_000);
        let wrong = format!("sha256:{}", sha256_hex(b"something else"));
        let url = serve_once(body.clone(), body.len(), None);
        let expected = Expected {
            asset_name: ASSET,
            url: &url,
            size: Some(body.len() as u64),
            digest: Some(&wrong),
        };
        assert_eq!(
            download(&expected, &tmp.0, "tako-test"),
            Err(InstallerError::Digest)
        );
        assert!(!tmp.0.join(ASSET).exists());
    }

    #[test]
    fn 配布元が大きさかsha256を示さなければ取りに行かない() {
        let tmp = Scratch::new("unverifiable");
        // 接続しない URL（取りに行けば Network になるので、Unverifiable が先に返ることを見る）
        let url = format!("http://127.0.0.1:9/releases/download/v0.8.40/{ASSET}");
        let digest = format!("sha256:{}", "0".repeat(64));
        for (size, digest) in [
            (None, Some(digest.as_str())),
            (Some(10), None),
            (Some(0), Some(digest.as_str())),
            (Some(10), Some("md5:abc")),
        ] {
            let expected = Expected {
                asset_name: ASSET,
                url: &url,
                size,
                digest,
            };
            assert_eq!(
                download(&expected, &tmp.0, "tako-test"),
                Err(InstallerError::Unverifiable)
            );
        }
    }

    #[test]
    fn 無言で上書きして起動し直す指定を渡す() {
        let log =
            Path::new(r"C:\Users\testuser\AppData\Local\Temp\tako-update-installer\1-2\setup.log");
        let exe = Path::new(r"C:\Users\testuser\AppData\Local\Programs\tako\tako-app.exe");
        let args = installer_args(log, 4242, exe);
        assert_eq!(
            command_tail(&args),
            "/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /SP- \
             /LOG=\"C:\\Users\\testuser\\AppData\\Local\\Temp\\tako-update-installer\\1-2\\setup.log\" \
             /TAKOWAITPID=4242 \
             /TAKORELAUNCH=\"C:\\Users\\testuser\\AppData\\Local\\Programs\\tako\\tako-app.exe\""
        );
        // 空白入りのパス（ユーザー名やフォルダ名に空白がある）も値だけを引用して 1 つの指定に収める
        let spaced = command_tail(&[InstallerArg::Path(
            RELAUNCH_PARAM,
            PathBuf::from(r"C:\Program Files\tako test\tako-app.exe"),
        )]);
        assert_eq!(
            spaced,
            r#"/TAKORELAUNCH="C:\Program Files\tako test\tako-app.exe""#
        );
        // 再起動を OS に任せない・ウィザードを出さない、は欠かせない
        for must in ["/NORESTART", "/SUPPRESSMSGBOXES", "/SILENT"] {
            assert!(args.contains(&InstallerArg::Flag(must)), "{must}");
        }
    }

    #[test]
    fn インストーラーで入れた場所から動いているかを比べる() {
        let install = Path::new(r"C:\Users\testuser\AppData\Local\Programs\tako\");
        for exe in [
            r"C:\Users\testuser\AppData\Local\Programs\tako\tako-app.exe",
            r"c:\USERS\testuser\APPDATA\local\programs\TAKO\tako-app.exe",
            r"\\?\C:\Users\testuser\AppData\Local\Programs\tako\tako-app.exe",
            "C:/Users/testuser/AppData/Local/Programs/tako/tako-app.exe",
        ] {
            assert!(
                runs_from_install_dir(Some(install), Path::new(exe)),
                "{exe}"
            );
        }
        for exe in [
            // ポータブル zip を展開した先
            r"C:\Users\testuser\Downloads\tako-v0.8.39-windows-x86_64\tako-app.exe",
            // 開発ビルド
            r"C:\src\tako\target\debug\tako-app.exe",
            // 入れた先の下の別フォルダ
            r"C:\Users\testuser\AppData\Local\Programs\tako\sub\tako-app.exe",
        ] {
            assert!(
                !runs_from_install_dir(Some(install), Path::new(exe)),
                "{exe}"
            );
        }
        // 記録が無い = インストーラーで入れていない
        assert!(!runs_from_install_dir(
            None,
            Path::new(r"C:\x\tako-app.exe")
        ));
        assert!(!runs_from_install_dir(
            Some(Path::new("")),
            Path::new(r"C:\x\tako-app.exe")
        ));
    }

    #[test]
    fn 置き場は前回の分を片付けて毎回新しく作る() {
        let tmp = Scratch::new("staging");
        let root = tmp.0.join(STAGING_DIR_NAME);
        let first = fresh_attempt_dir(&root).unwrap();
        std::fs::write(first.join("old.exe"), b"MZ").unwrap();
        // 直下のファイル（フォルダでないもの）は片付けの対象にしない
        std::fs::write(root.join("keep.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = fresh_attempt_dir(&root).unwrap();
        assert_ne!(first, second);
        assert!(second.starts_with(&root));
        assert!(!first.exists(), "前回の置き場は消える");
        assert!(root.join("keep.txt").exists());
    }

    #[test]
    fn インストーラーを起動できなければ断る() {
        let tmp = Scratch::new("launch");
        let prepared = PreparedInstaller {
            path: tmp.0.join("missing.exe"),
            log_path: tmp.0.join("setup.log"),
            size: 0,
            sha256: String::new(),
        };
        let err = launch(&prepared, 1, Path::new("tako-app")).unwrap_err();
        assert!(matches!(err, InstallerError::Launch(_)), "{err:?}");
    }

    /// 起動した偽のインストーラーが受け取った引数を書き出す（Windows 以外）
    #[cfg(unix)]
    #[test]
    fn 起動したインストーラーへ指定が届く() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = Scratch::new("launch-args");
        let out = tmp.0.join("argv.txt");
        let fake = tmp.0.join("fake-setup.exe");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let prepared = PreparedInstaller {
            path: fake,
            log_path: tmp.0.join("setup.log"),
            size: 0,
            sha256: String::new(),
        };
        let mut child = launch(&prepared, 77, Path::new("/opt/tako/tako-app")).unwrap();
        assert!(child.wait().unwrap().success());
        let argv = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = argv.lines().collect();
        assert_eq!(lines[0], "/SILENT");
        assert!(lines.contains(&"/TAKOWAITPID=77"), "{argv}");
        assert!(
            lines.contains(&"/TAKORELAUNCH=/opt/tako/tako-app"),
            "{argv}"
        );
        assert!(
            lines.contains(&format!("/LOG={}", tmp.0.join("setup.log").display()).as_str()),
            "{argv}"
        );
    }
}
