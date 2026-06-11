//! discovery — CLI / MCP 接続情報の永続化（FR-2.2.9）
//!
//! トークンとソケットパスはアプリ起動毎に変わる（トークンはメモリ生成、ソケットは
//! PID 入りパス）ため、環境変数しか手段が無いとアプリ再起動後に外部の長寿命プロセス
//! （tmux 内のエージェント等）が繋ぎ直せない。アプリは起動時に接続情報を
//! **ユーザー専用パーミッション（ファイル 0600 / ディレクトリ 0700）**で書き出し、
//! CLI は環境変数が無い・古い（接続不可 / 認証失敗）場合にこのファイルへ
//! フォールバックする。
//!
//! 方針メモ:
//! - ソケットパスは PID 入りのまま（複数インスタンスの衝突回避）。発見はこのファイル
//!   経由で行う（安定パス方式は複数インスタンスで取り合いになるため不採用）
//! - 複数インスタンスは「最後に起動したものが上書き = 最新優先」。旧インスタンスへは
//!   従来どおり環境変数（ペイン内）で届く
//! - アプリ終了時の削除はしない（GPUI 終了経路で Drop が保証されないため）。
//!   残骸ファイルは CLI 側の接続失敗として顕在化し、誤接続はトークン認証で防がれる

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 接続情報ファイルの中身。トークンを含むためログに出さないこと（`conventions.md`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    /// フォーマット版数（後方互換の判断用）
    pub version: u32,
    /// 書き出したアプリの PID（診断用。生存確認には使わない）
    pub pid: u32,
    /// IPC ソケットパス（`TAKO_SOCKET` 相当）
    pub socket: String,
    /// 認証トークン（`TAKO_TOKEN` 相当）
    pub token: String,
    /// MCP エンドポイント（`TAKO_MCP_URL` 相当。MCP サーバーが立たなければ None）
    pub mcp_url: Option<String>,
}

/// 接続情報ファイルのパス（`<data_dir>/control.json`）
pub fn info_path() -> Option<PathBuf> {
    tako_core::paths::data_dir().map(|d| d.join("control.json"))
}

/// 接続情報を書き出す（アプリ起動時に呼ぶ）。tmp へ書いて rename する（読み手と競合しない）
pub fn write(info: &ControlInfo) -> io::Result<PathBuf> {
    let path = info_path().ok_or_else(|| {
        io::Error::new(io::ErrorKind::Unsupported, "データディレクトリを解決できない")
    })?;
    write_to(&path, info)?;
    Ok(path)
}

fn write_to(path: &std::path::Path, info: &ControlInfo) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリが無い"))?;
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(info)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // トークンを含むためユーザー専用にする（ソケットの 0600 と同じ防御線）
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

/// 接続情報を読む（CLI のフォールバック用）。
/// ファイルが無い・読めない・版数不明なら None（呼び出し側は環境変数のみで判断する）
pub fn read() -> Option<ControlInfo> {
    read_from(&info_path()?)
}

fn read_from(path: &std::path::Path) -> Option<ControlInfo> {
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

    #[test]
    fn 書き出しと読み戻しが往復しパーミッションは0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_path("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("control.json");
        let info = ControlInfo {
            version: 1,
            pid: 42,
            socket: "/tmp/tako-42-0.sock".into(),
            token: "secret".into(),
            mcp_url: Some("http://127.0.0.1:1234/mcp".into()),
        };
        write_to(&path, &info).unwrap();
        assert_eq!(read_from(&path), Some(info.clone()));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "ファイルはユーザー専用");
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700, "ディレクトリはユーザー専用");
        // 上書き = 最新優先
        let newer = ControlInfo {
            pid: 43,
            ..info.clone()
        };
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
        let info = ControlInfo {
            version: 99,
            pid: 1,
            socket: "s".into(),
            token: "t".into(),
            mcp_url: None,
        };
        write_to(&path, &info).unwrap();
        assert_eq!(read_from(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
