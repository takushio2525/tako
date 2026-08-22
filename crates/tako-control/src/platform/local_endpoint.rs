//! remote デーモンのローカルエンドポイント（抽象境界 B4）
//!
//! `tako remote serve` のデーモンは「同一マシン・同一ユーザーからのみ到達できる
//! ローカル HTTP エンドポイント」を張り、`tailscale serve` がそれを tailnet へ
//! プロキシする。この「ローカルエンドポイント」の実体だけがプラットフォームで異なる。
//!
//! - unix: Unix domain socket（0600）。別 OS ユーザーは接続自体が不能
//! - Windows: loopback TCP（`127.0.0.1:0` = OS が選ぶ空きポート）。実ポート番号は
//!   endpoint ファイルへ書き出し、`status` / `stop` / serve 設定がそこから読む。
//!   Windows の AF_UNIX には `SO_PEERCRED` 相当が無く UDS では同一ユーザー検証が
//!   成立しないため、UDS をそのまま移植することはしない
//!   （設計は `.agent/plans/2026-07-windows-port-architecture.md` の B4）
//!
//! ## Windows の loopback TCP に残る差（#528 の既知の制限）
//!
//! UDS の 0600 は「同一ユーザー以外は接続できない」を**カーネルに強制させる**が、
//! loopback TCP にはそれが無く、同一マシンの別ユーザーのプロセスでも
//! `127.0.0.1:<port>` へ接続できる。daemon は `tailscale serve` が付ける
//! `X-Forwarded-For` を identity の根拠にしているので、ローカルの攻撃者が
//! ペアリング済み端末の tailnet IP を騙る余地が生まれる（#287 P1-2 が UDS 化で
//! 構造的に消した経路の一部が Windows では戻る）。
//!
//! 塞ぐには接続元ソケットの所有プロセスを `GetExtendedTcpTable` で引き当て、
//! それが tailscaled かを検証する必要がある（#524 の `platform::procinfo` に
//! 同型の実装がある）。**MVP では未実装で、追跡は別 Issue**。
//! ポート番号は毎回ランダムだが、これは緩和ではあっても防御ではない。
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

/// エンドポイントの実体の種別。`tako remote status`（CLI / MCP）が
/// 「実際に何で待ち受けているか」を申告するために使う（#528）。
/// unix: `unix-domain-socket` / Windows: `loopback-tcp`
pub fn kind() -> &'static str {
    imp::KIND
}

/// Windows の loopback TCP で待ち受けているポート番号。
/// unix（UDS）では常に `None`。status 表示・serve のプロキシ先組み立てに使う
pub fn loopback_port(endpoint: &Path) -> Option<u16> {
    imp::loopback_port(endpoint)
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream;

    /// macOS の `sun_path` は 104 バイト。余裕を見て 100 で弾く
    pub const PATH_BYTE_LIMIT: Option<usize> = Some(100);

    pub const KIND: &str = "unix-domain-socket";

    /// UDS には「ポート」が無い
    pub fn loopback_port(_endpoint: &Path) -> Option<u16> {
        None
    }

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
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

    pub const PATH_BYTE_LIMIT: Option<usize> = None;

    pub const KIND: &str = "loopback-tcp";

    /// endpoint ファイル（ポート番号を書いたテキスト）から待ち受けポートを読む
    pub fn loopback_port(endpoint: &Path) -> Option<u16> {
        read_port(endpoint)
    }

    /// endpoint ファイルに書かれたポート番号へ TCP 接続できるか。
    ///
    /// 注意: 異常終了でポートファイルが残ると、そのポートを**別の無関係なプロセス**が
    /// 取っている場合に「生きている」と誤判定しうる（UDS のパスでは起こらない）。
    /// 呼び出し側（`remote.rs`）は PID ファイルを一次情報として先に見るので、
    /// ここが効くのは PID ファイルを失った縮退経路だけ
    pub fn probe_alive(endpoint: &Path) -> bool {
        let Some(port) = read_port(endpoint) else {
            return false;
        };
        TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_secs(2),
        )
        .is_ok()
    }

    /// loopback の空きポートで待ち受け、実ポート番号を endpoint ファイルへ書き出す。
    ///
    /// ポート 0 を **tiny_http に直接渡して** OS に選ばせ、`server_addr()` で実ポートを
    /// 読み戻す。「自分で bind して port を調べ、drop してから tiny_http で張り直す」形は
    /// その隙に別プロセスが同じポートを取れてしまうので採らない
    pub fn bind(endpoint: &Path) -> io::Result<tiny_http::Server> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| io::Error::other(format!("remote API サーバーを起動できない: {e}")))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| io::Error::other("待ち受けポートを取得できない"))?;

        // ポート番号を endpoint ファイルに記録（status / stop / serve 設定が参照する）
        if let Some(dir) = endpoint.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(endpoint, port.to_string())?;
        Ok(server)
    }

    /// endpoint ファイルからポートを読み、loopback TCP で HTTP リクエストを送る
    pub fn request_raw(
        endpoint: &Path,
        request: &str,
        read_timeout: Option<Duration>,
    ) -> io::Result<String> {
        let port = read_port(endpoint)
            .ok_or_else(|| io::Error::other("daemon のポートファイルを読めない"))?;
        let mut stream = TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_secs(5),
        )?;
        if let Some(t) = read_timeout {
            stream.set_read_timeout(Some(t)).ok();
        }
        stream.write_all(request.as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        Ok(String::from_utf8_lossy(&response).into_owned())
    }

    fn read_port(endpoint: &Path) -> Option<u16> {
        std::fs::read_to_string(endpoint).ok()?.trim().parse().ok()
    }
}
