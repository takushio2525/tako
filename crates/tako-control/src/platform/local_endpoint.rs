//! remote デーモンのローカルエンドポイント（抽象境界 B4）
//!
//! `tako remote serve` のデーモンは「同一マシンからのみ到達できるローカル HTTP
//! エンドポイント」を張り、`tailscale serve` がそれを tailnet へプロキシする。
//! この「ローカルエンドポイント」の実体だけがプラットフォームで異なる…
//! **はずだったが、実測では同じ macOS の中でも tailscaled の導入形態で変わる**（#1038）。
//!
//! - ループバック TCP（`127.0.0.1:<エフェメラルポート>`）= **既定**。
//!   どの tailscaled 変種でも dial できる唯一の形（#1038 / #971）
//! - Unix domain socket（0600）= unix のみの互換経路。`TAKO_REMOTE_ENDPOINT=unix` で選ぶ。
//!   別 OS ユーザーは接続自体が不能になるので分離は強いが、**サンドボックス版の
//!   tailscaled（macOS の GUI 版 Tailscale.app のシステム拡張）は unix socket を
//!   一切 dial できない**ため、その環境では全リクエストが 502 になる
//!
//! ## ループバック TCP のトレードオフ（#841）
//!
//! TCP は「同一マシンの別ユーザー / 別プロセスも接続できる」。UDS がカーネルで
//! 強制していた同一ユーザー限定がここでは効かないため、`X-Forwarded-For` を
//! identity の根拠として信じる層①（`remote_auth`）には偽装の余地が残る。
//! 接続元プロセスが tailscaled であることの検証は #841 で追う。
//! 緩和として ①バインドは 127.0.0.1 限定（LAN へは出さない）②ポートは毎回
//! エフェメラル ③管理 API は `X-Forwarded-For` が付いていると常に拒否、を維持する。
//!
//! 呼び出し側（`remote.rs`）はこのモジュールだけを見る。`cfg` はこのファイルの内側に閉じる。

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// ループバック TCP の接続先は常に 127.0.0.1（LAN へは出さない）
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
/// 生存確認の接続タイムアウト。ローカルなので短くてよい
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// 待ち受けの形（bind する前の指定）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointSpec {
    /// Unix domain socket（unix のみ）
    Unix(PathBuf),
    /// 127.0.0.1 のエフェメラルポート
    Loopback,
}

/// 待ち受け中のエンドポイント（bind して確定した実体）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Unix(PathBuf),
    Loopback(u16),
}

impl Endpoint {
    /// 人間向けの表示（status / ログ用）。`tailscale serve` の target 表現とは別物
    pub fn describe(&self) -> String {
        match self {
            Self::Unix(p) => format!("unix:{}", p.display()),
            Self::Loopback(port) => format!("127.0.0.1:{port}"),
        }
    }

    /// 種別の短い識別子（JSON 応答用）
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Unix(_) => "unix",
            Self::Loopback(_) => "loopback-tcp",
        }
    }

    /// ループバック TCP のポート（UDS なら None）
    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Loopback(p) => Some(*p),
            Self::Unix(_) => None,
        }
    }

    /// UDS のパス（TCP なら None）
    pub fn socket_path(&self) -> Option<&Path> {
        match self {
            Self::Unix(p) => Some(p.as_path()),
            Self::Loopback(_) => None,
        }
    }
}

/// エンドポイントで既に別のデーモンが待ち受けているか。
/// 「接続できる = 生きている」「接続できない = stale」の判定に使う
pub fn probe_alive(endpoint: &Endpoint) -> bool {
    match endpoint {
        Endpoint::Loopback(port) => {
            TcpStream::connect_timeout(&SocketAddr::from((LOOPBACK, *port)), PROBE_TIMEOUT).is_ok()
        }
        Endpoint::Unix(path) => imp::probe_alive_unix(path),
    }
}

/// デーモン側の待ち受けを開始する。UDS は同一ユーザーのみ到達できるよう保護する。
/// 返り値の `Endpoint` が確定した実体（ループバックはここでポートが決まる）
pub fn bind(spec: &EndpointSpec) -> io::Result<(tiny_http::Server, Endpoint)> {
    match spec {
        EndpointSpec::Loopback => {
            // ポート 0 = OS にエフェメラルポートを選ばせる。バインドは 127.0.0.1 限定
            let server = tiny_http::Server::http(SocketAddr::from((LOOPBACK, 0)))
                .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
            let port = server
                .server_addr()
                .to_ip()
                .map(|a| a.port())
                .ok_or_else(|| io::Error::other("待ち受けポートを取得できない"))?;
            Ok((server, Endpoint::Loopback(port)))
        }
        EndpointSpec::Unix(path) => {
            let server = imp::bind_unix(path)?;
            Ok((server, Endpoint::Unix(path.clone())))
        }
    }
}

/// 組み立て済みの生 HTTP リクエストを送り、応答全体を文字列で返す（クライアント側）。
/// `read_timeout` が `None` のときは待ち続ける
pub fn request_raw(
    endpoint: &Endpoint,
    request: &str,
    read_timeout: Option<Duration>,
) -> io::Result<String> {
    match endpoint {
        Endpoint::Loopback(port) => {
            let mut stream =
                TcpStream::connect_timeout(&SocketAddr::from((LOOPBACK, *port)), PROBE_TIMEOUT)?;
            if let Some(t) = read_timeout {
                stream.set_read_timeout(Some(t)).ok();
            }
            roundtrip(&mut stream, request)
        }
        Endpoint::Unix(path) => imp::request_raw_unix(path, request, read_timeout),
    }
}

/// 接続済みストリームへ書いて全部読む（TCP / UDS 共通の HTTP/1.1 往復）
fn roundtrip<S: io::Read + io::Write>(stream: &mut S, request: &str) -> io::Result<String> {
    stream.write_all(request.as_bytes())?;
    stream.flush()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

/// エンドポイントのパス長上限（unix の `sun_path` 制約に由来）。
/// 制約が無いプラットフォームでは `None`
pub fn path_byte_limit() -> Option<usize> {
    imp::PATH_BYTE_LIMIT
}

/// この環境で Unix domain socket を待ち受けに使えるか
pub fn unix_supported() -> bool {
    cfg!(unix)
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// macOS の `sun_path` は 104 バイト。余裕を見て 100 で弾く
    pub const PATH_BYTE_LIMIT: Option<usize> = Some(100);

    pub fn probe_alive_unix(endpoint: &Path) -> bool {
        UnixStream::connect(endpoint).is_ok()
    }

    pub fn bind_unix(endpoint: &Path) -> io::Result<tiny_http::Server> {
        let server = tiny_http::Server::http_unix(endpoint)
            .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
        // socket を 0600 に制限（別 OS ユーザーの接続を遮断）
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| io::Error::other(format!("socket パーミッションの設定に失敗: {e}")))?;
        Ok(server)
    }

    pub fn request_raw_unix(
        endpoint: &Path,
        request: &str,
        read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        let mut stream = UnixStream::connect(endpoint)?;
        if let Some(t) = read_timeout {
            stream.set_read_timeout(Some(t)).ok();
        }
        super::roundtrip(&mut stream, request)
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub const PATH_BYTE_LIMIT: Option<usize> = None;

    /// Windows の tailscale には unix socket の serve target が無く（#971）、
    /// AF_UNIX にも `SO_PEERCRED` 相当が無いので UDS 経路は用意しない。
    /// ループバック TCP が唯一の形（縮退ではなくその OS での正しい形）
    pub fn probe_alive_unix(_endpoint: &Path) -> bool {
        false
    }

    pub fn bind_unix(_endpoint: &Path) -> io::Result<tiny_http::Server> {
        Err(io::Error::other(UNSUPPORTED))
    }

    pub fn request_raw_unix(
        _endpoint: &Path,
        _request: &str,
        _read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        Err(io::Error::other(UNSUPPORTED))
    }

    const UNSUPPORTED: &str = "この環境では Unix domain socket を待ち受けに使えません。\
        ループバック TCP（既定）をお使いください";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ループバックはバインドでポートが確定し表示できる() {
        let (server, ep) = bind(&EndpointSpec::Loopback).expect("bind できる");
        let port = ep.port().expect("ポートがある");
        assert!(port > 0, "エフェメラルポートが割り当たる");
        assert_eq!(ep, Endpoint::Loopback(port));
        assert_eq!(ep.describe(), format!("127.0.0.1:{port}"));
        assert_eq!(ep.kind_str(), "loopback-tcp");
        assert!(ep.socket_path().is_none());
        // バインド先は 127.0.0.1 限定（LAN へ露出しない）
        let addr = server.server_addr().to_ip().expect("ip アドレス");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        drop(server);
    }

    #[test]
    fn 待ち受けていないループバックポートはprobeでfalse() {
        // バインドして即 drop したポートは（通常）閉じている
        let port = {
            let (server, ep) = bind(&EndpointSpec::Loopback).expect("bind できる");
            let p = ep.port().unwrap();
            drop(server);
            p
        };
        assert!(!probe_alive(&Endpoint::Loopback(port)));
    }

    #[test]
    fn 待ち受け中のループバックはprobeでtrue() {
        let (server, ep) = bind(&EndpointSpec::Loopback).expect("bind できる");
        assert!(probe_alive(&ep));
        drop(server);
    }

    #[test]
    fn udsの表示は接頭辞unixを持つ() {
        let ep = Endpoint::Unix(PathBuf::from("/tmp/x.sock"));
        assert_eq!(ep.describe(), "unix:/tmp/x.sock");
        assert_eq!(ep.kind_str(), "unix");
        assert!(ep.port().is_none());
        assert_eq!(ep.socket_path(), Some(Path::new("/tmp/x.sock")));
    }
}
