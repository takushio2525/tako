//! remote デーモンのローカルエンドポイント（抽象境界 B4）
//!
//! `tako remote serve` のデーモンは「同一マシン・同一ユーザーからのみ到達できる
//! ローカル HTTP エンドポイント」を張り、`tailscale serve` がそれを tailnet へ
//! プロキシする。この「ローカルエンドポイント」の実体だけがプラットフォームで異なる。
//!
//! - unix: Unix domain socket（0600）。別 OS ユーザーは接続自体が不能
//! - Windows: loopback TCP + トークンへ置き換える予定（未実装）。
//!   Windows の AF_UNIX には `SO_PEERCRED` 相当が無く UDS では同一ユーザー検証が
//!   成立しないため、UDS をそのまま移植することはしない
//!   （設計は `.agent/plans/2026-07-windows-port-architecture.md` の B4）
//!
//! 呼び出し側（`remote.rs`）はこのモジュールだけを見る。`cfg` はこのファイルの内側に閉じる。

use std::io;
use std::path::Path;
use std::time::Duration;

/// エンドポイントで既に別のデーモンが待ち受けているか。
/// 「接続できる = 生きている」「接続できない = stale」の判定に使う
pub fn probe_alive(endpoint: &Path) -> bool {
    imp::probe_alive(endpoint)
}

/// デーモン側の待ち受けを開始する。同一ユーザーのみ到達できるよう保護した状態で返す
pub fn bind(endpoint: &Path) -> io::Result<tiny_http::Server> {
    imp::bind(endpoint)
}

/// 組み立て済みの生 HTTP リクエストを送り、応答全体を文字列で返す（クライアント側）。
/// `read_timeout` が `None` のときは待ち続ける
pub fn request_raw(
    endpoint: &Path,
    request: &str,
    read_timeout: Option<Duration>,
) -> io::Result<String> {
    imp::request_raw(endpoint, request, read_timeout)
}

/// エンドポイントのパス長上限（unix の `sun_path` 制約に由来）。
/// 制約が無いプラットフォームでは `None`
pub fn path_byte_limit() -> Option<usize> {
    imp::PATH_BYTE_LIMIT
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream;

    /// macOS の `sun_path` は 104 バイト。余裕を見て 100 で弾く
    pub const PATH_BYTE_LIMIT: Option<usize> = Some(100);

    pub fn probe_alive(endpoint: &Path) -> bool {
        UnixStream::connect(endpoint).is_ok()
    }

    pub fn bind(endpoint: &Path) -> io::Result<tiny_http::Server> {
        let server = tiny_http::Server::http_unix(endpoint)
            .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
        // socket を 0600 に制限（別 OS ユーザーの接続を遮断）
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| io::Error::other(format!("socket パーミッションの設定に失敗: {e}")))?;
        Ok(server)
    }

    pub fn request_raw(
        endpoint: &Path,
        request: &str,
        read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        let mut stream = UnixStream::connect(endpoint)?;
        if let Some(t) = read_timeout {
            stream.set_read_timeout(Some(t)).ok();
        }
        stream.write_all(request.as_bytes())?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        Ok(String::from_utf8_lossy(&response).into_owned())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    pub const PATH_BYTE_LIMIT: Option<usize> = None;

    /// Windows 実装は loopback TCP + トークンで作り直す（B4 の Windows 実装タスク）。
    /// それまでは「稼働中のデーモンは存在しない」として扱う
    pub fn probe_alive(_endpoint: &Path) -> bool {
        false
    }

    pub fn bind(_endpoint: &Path) -> io::Result<tiny_http::Server> {
        Err(io::Error::other(UNSUPPORTED))
    }

    pub fn request_raw(
        _endpoint: &Path,
        _request: &str,
        _read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        Err(io::Error::other(UNSUPPORTED))
    }

    const UNSUPPORTED: &str = "remote デーモンは Windows では未対応です。\
        Unix domain socket に代わる loopback TCP + トークン方式への置き換えが必要です";
}
