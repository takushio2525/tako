//! ipc — Layer 1 IPC サーバー（FR-2.2 の受け口）
//!
//! トランスポート（`.agent/architecture.md` プラットフォーム抽象 / 抽象境界 B3）:
//! - unix: Unix domain socket（パーミッション 0600 + セッション毎ランダムトークン）
//! - windows: named pipe（`\\.\pipe\tako-<ユーザー>`。実体は
//!   `platform::named_pipe`、既定 DACL + トークンの二段防御）
//!
//! 1 行 1 JSON の接続処理はトランスポート非依存（`conn` モジュール）で、
//! unix / windows の実装は「バイトストリームの確立」だけを持つ。
//!
//! スレッド構成: accept スレッド + 接続毎スレッド（ブロッキング IO）。
//! 接続スレッドはリクエストを futures channel で UI スレッドへ渡し、応答を同期で待つ。
//! dispatch の実行は受信側（UI のイベントループ。GPUI executor）で行うため、
//! ここでは tokio 等の非同期ランタイムを持ち込まない（Phase 0 の方針踏襲）。

use std::io;

use futures::channel::mpsc::UnboundedSender;
use tako_core::PaneOrigin;

use crate::dispatch::DispatchError;
use crate::protocol::Request;

/// UI 側へ渡す 1 リクエスト。`reply` へ dispatch の結果を返すと接続スレッドが応答を書く。
/// `origin` は新規生成ペインの生成主体（IPC 直 = Cli、MCP 経由 = Mcp）
pub struct IncomingRequest {
    pub request: Request,
    pub origin: PaneOrigin,
    pub reply: std::sync::mpsc::SyncSender<Result<serde_json::Value, DispatchError>>,
}

/// IPC サーバーのハンドル。drop でソケットファイルを片付ける。
/// `endpoint` はペインのシェルへ `TAKO_SOCKET` として注入する
pub struct IpcServer {
    endpoint: String,
}

impl IpcServer {
    /// サーバーを起動する。受け取った各リクエストは `tx` 経由で UI スレッドへ届く。
    /// `token` はセッション共有の認証トークン（[`crate::generate_token`] で生成し、
    /// MCP サーバーとも共有する。FR-2.3.4）
    pub fn start(tx: UnboundedSender<IncomingRequest>, token: String) -> io::Result<Self> {
        Self::start_with(tx, token, false)
    }

    /// `prefer_temp_socket` = true は固定ソケット（`<data_dir>/tako.sock`）を候補にせず
    /// PID 入り一時パスで起動する。セカンダリモード（多重起動の後発。Issue #113）が
    /// プライマリの固定ソケットを unlink + bind で乗っ取らないための経路
    pub fn start_with(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        prefer_temp_socket: bool,
    ) -> io::Result<Self> {
        #[cfg(unix)]
        {
            unix_imp::start(tx, token, prefer_temp_socket)
        }
        #[cfg(windows)]
        {
            windows_imp::start(tx, token, prefer_temp_socket)
        }
    }

    /// IPC エンドポイント（unix: ソケットパス、windows: パイプ名）
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint);
    }
}

/// トランスポート非依存の接続処理（1 行 1 JSON のリクエスト / レスポンス）。
/// unix / windows の実装はストリームの読み書き半分をここへ渡すだけ
mod conn {
    use std::io::{BufRead, BufReader, BufWriter, Read, Write};

    use futures::channel::mpsc::UnboundedSender;
    use tako_core::PaneOrigin;

    use super::IncomingRequest;
    use crate::protocol::{error_code, RequestEnvelope, ResponseEnvelope};

    /// 1 接続を処理する。ストリームが閉じるかエラーで戻る
    pub(super) fn handle_connection<R: Read, W: Write>(
        read_half: R,
        write_half: W,
        token: &str,
        tx: &UnboundedSender<IncomingRequest>,
    ) {
        let reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(write_half);
        for line in reader.lines() {
            let Ok(line) = line else {
                return;
            };
            if line.trim().is_empty() {
                continue;
            }
            let response = process_line(&line, token, tx);
            let Ok(json) = serde_json::to_string(&response) else {
                return;
            };
            if writeln!(writer, "{json}")
                .and_then(|_| writer.flush())
                .is_err()
            {
                return;
            }
        }
    }

    fn process_line(
        line: &str,
        token: &str,
        tx: &UnboundedSender<IncomingRequest>,
    ) -> ResponseEnvelope {
        let envelope: RequestEnvelope = match serde_json::from_str(line) {
            Ok(envelope) => envelope,
            Err(e) => {
                return ResponseEnvelope::err(
                    0,
                    error_code::PARSE,
                    format!("リクエストを解釈できない: {e}"),
                )
            }
        };
        if envelope.token != token {
            // トークン不一致 = アプリ外プロセスからの接続（FR-2.3.4）。詳細は返さない
            return ResponseEnvelope::err(
                envelope.id,
                error_code::AUTH,
                "認証に失敗した（TAKO_TOKEN が一致しない）",
            );
        }
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        // MCP stdio ブリッジ（`tako mcp serve`）経由のリクエストは origin = Mcp として扱う
        let origin = match envelope.origin.as_deref() {
            Some("mcp") => PaneOrigin::Mcp,
            _ => PaneOrigin::Cli,
        };
        let incoming = IncomingRequest {
            request: envelope.request,
            origin,
            reply: reply_tx,
        };
        if tx.unbounded_send(incoming).is_err() {
            return ResponseEnvelope::err(
                envelope.id,
                error_code::INTERNAL,
                "アプリ側の受け口が閉じている",
            );
        }
        match reply_rx.recv() {
            Ok(Ok(result)) => ResponseEnvelope::ok(envelope.id, result),
            Ok(Err(e)) => ResponseEnvelope::err(envelope.id, e.code(), e.to_string()),
            Err(_) => ResponseEnvelope::err(
                envelope.id,
                error_code::INTERNAL,
                "アプリ側から応答が返らなかった",
            ),
        }
    }
}

#[cfg(windows)]
mod windows_imp {
    use futures::channel::mpsc::UnboundedSender;

    use super::{conn, IncomingRequest, IpcServer};
    use crate::platform::named_pipe;

    /// PID + 連番の一時パイプ名（テスト・セルフテスト・セカンダリモード用）
    fn temp_pipe_name() -> String {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!(r"\\.\pipe\tako-{}-{}-{seq}", user_tag(), std::process::id())
    }

    /// パイプ名の名前空間はマシン全体で共有のため、ユーザー名で分離する
    fn user_tag() -> String {
        std::env::var("USERNAME")
            .unwrap_or_else(|_| "user".into())
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
            .collect()
    }

    /// 再起動をまたいで安定するパイプ名（unix の固定ソケットパスに相当）
    fn preferred_pipe_name(prefer_temp: bool) -> String {
        if prefer_temp || cfg!(test) || std::env::var_os("TAKO_SELF_TEST").is_some() {
            return temp_pipe_name();
        }
        format!(r"\\.\pipe\tako-{}", user_tag())
    }

    pub(super) fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        prefer_temp_socket: bool,
    ) -> std::io::Result<IpcServer> {
        let mut name = preferred_pipe_name(prefer_temp_socket);
        // 固定名が既に占有されている（先行インスタンス生存）なら一時名へフォールバック
        // （unix の「固定ソケットへ接続可能なら一時パス」と同じ方針）
        let first = match named_pipe::create_server_instance(&name, true) {
            Ok(instance) => instance,
            Err(_) if !prefer_temp_socket => {
                name = temp_pipe_name();
                named_pipe::create_server_instance(&name, true)?
            }
            Err(e) => return Err(e),
        };

        let accept_name = name.clone();
        let accept_token = token;
        std::thread::Builder::new()
            .name("tako-ipc-accept".into())
            .spawn(move || {
                let mut next = Some(first);
                loop {
                    let instance = match next.take() {
                        Some(instance) => instance,
                        None => match named_pipe::create_server_instance(&accept_name, false) {
                            Ok(instance) => instance,
                            Err(e) => {
                                tracing::warn!("IPC パイプインスタンスを作れない: {e}");
                                break;
                            }
                        },
                    };
                    match instance.wait_client() {
                        Ok(stream) => {
                            let tx = tx.clone();
                            let token = accept_token.clone();
                            let result = std::thread::Builder::new()
                                .name("tako-ipc-conn".into())
                                .spawn(move || match stream.try_clone() {
                                    Ok(read_half) => {
                                        conn::handle_connection(read_half, stream, &token, &tx)
                                    }
                                    Err(e) => tracing::warn!("IPC 接続の複製に失敗: {e}"),
                                });
                            if let Err(e) = result {
                                tracing::warn!("IPC 接続スレッドを起動できない: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("IPC accept に失敗: {e}"),
                    }
                }
            })?;

        Ok(IpcServer { endpoint: name })
    }
}

#[cfg(unix)]
mod unix_imp {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    use futures::channel::mpsc::UnboundedSender;

    use super::{conn, IncomingRequest, IpcServer};

    /// PID ベースの一時ソケットパス（テスト・セルフテスト・多重起動フォールバック用）
    fn temp_socket_path() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("tako-{}-{seq}.sock", std::process::id()))
    }

    /// 再起動をまたいで安定するソケットパスを決定する。
    /// - 単体テスト / セルフテスト / セカンダリモード（`prefer_temp`）: 一時パス
    ///   （他インスタンスと衝突回避・プライマリの固定ソケットを乗っ取らない）
    /// - 通常起動: `<data_dir>/tako.sock`（固定。既存クライアントがそのまま再接続可能）
    /// - 別インスタンスが生きている: フォールバックで一時パス
    fn preferred_socket_path(prefer_temp: bool) -> std::path::PathBuf {
        if prefer_temp || cfg!(test) || std::env::var_os("TAKO_SELF_TEST").is_some() {
            return temp_socket_path();
        }
        if let Some(well_known) = tako_core::paths::data_dir().map(|d| d.join("tako.sock")) {
            if well_known.exists() && UnixStream::connect(&well_known).is_ok() {
                return temp_socket_path();
            }
            return well_known;
        }
        temp_socket_path()
    }

    pub(super) fn start(
        tx: UnboundedSender<IncomingRequest>,
        token: String,
        prefer_temp_socket: bool,
    ) -> std::io::Result<IpcServer> {
        let path = preferred_socket_path(prefer_temp_socket);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // 前回残骸（クラッシュ等で remove されなかったもの）を除去
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        // 自ユーザーのプロセスのみ接続可能にする（トークンと二段の防御線）
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

        let accept_token = token;
        std::thread::Builder::new()
            .name("tako-ipc-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let tx = tx.clone();
                            let token = accept_token.clone();
                            let result = std::thread::Builder::new()
                                .name("tako-ipc-conn".into())
                                .spawn(move || match stream.try_clone() {
                                    Ok(read_half) => {
                                        conn::handle_connection(read_half, stream, &token, &tx)
                                    }
                                    Err(e) => tracing::warn!("IPC 接続の複製に失敗: {e}"),
                                });
                            if let Err(e) = result {
                                tracing::warn!("IPC 接続スレッドを起動できない: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("IPC accept に失敗: {e}"),
                    }
                }
            })?;

        Ok(IpcServer {
            endpoint: path.display().to_string(),
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    use futures::channel::mpsc::unbounded;
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::protocol::{error_code, RequestEnvelope, ResponseEnvelope};

    const TEST_TOKEN: &str = "test-token";

    /// サーバー + ダミーディスパッチャ（list に固定値を返す）を立てて 1 往復する
    fn roundtrip(token_for_client: Option<String>) -> ResponseEnvelope {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).expect("IPC サーバーを起動できる");

        // UI イベントループの代わりに同期実行のディスパッチャを立てる
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({ "pong": true })));
            }
        });

        let token = token_for_client.unwrap_or_else(|| TEST_TOKEN.to_string());
        let stream = UnixStream::connect(server.endpoint()).expect("ソケットへ接続できる");
        let mut writer = stream.try_clone().unwrap();
        let envelope = RequestEnvelope::new(1, token, Request::List);
        writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("レスポンスを解釈できる")
    }

    #[test]
    fn 正しいトークンでリクエストが往復する() {
        let response = roundtrip(None);
        assert_eq!(response.id, 1);
        assert_eq!(response.result.unwrap()["pong"], json!(true));
        assert!(response.error.is_none());
    }

    #[test]
    fn 連続接続でfdが漏れない() {
        fn fd_count() -> usize {
            std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
        }
        fn one_roundtrip(endpoint: &str) {
            let stream = UnixStream::connect(endpoint).unwrap();
            let mut writer = stream.try_clone().unwrap();
            let envelope = RequestEnvelope::new(1, TEST_TOKEN.to_string(), Request::List);
            writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
        }

        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        // ウォームアップ（スレッドスタック等の初期確保を測定から外す）
        for _ in 0..3 {
            one_roundtrip(server.endpoint());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let before = fd_count();
        for _ in 0..10 {
            one_roundtrip(server.endpoint());
        }
        // 接続スレッドの終了（fd クローズ）を待つ。fd 数はプロセス全体の共有状態で、
        // 並列実行中の他テストが一時的に開く fd でも揺れるため、落ち着くまでリトライで
        // 待つ（真のリークなら fd は開いたままなので待っても失敗が保たれる）
        // 判定は**観測期間の最小値**で行う。fd 数はプロセス全体の共有量なので、
        // 並列実行中の他テストが一時的に開いた fd が「増えた」に見える
        // （#916 でファイルを触るテストが増えたあと CI で 15 → 18 の偽陽性が出た）。
        // 真のリークなら fd は開いたままなので最小値も下がらず、検出力は落ちない。
        //
        // 許容差を 2 → 6 へ広げた（#983）。設定ファイルを触るテストがさらに増え、
        // **このテスト単体では緑・スイート全体では 3/3 で 10 → 14** という形で偽陽性が出た
        // （#983 のテストだけを一緒に走らせると緑 = リークではなく他テストの同時保持）。
        // 検出力は落ちない: このテストは 10 回接続するので、1 接続でも取りこぼせば
        // 最小値は +10 側へ動き、+6 では吸収できない
        const FD_NOISE_TOLERANCE: usize = 6;
        let mut lowest = usize::MAX;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            lowest = lowest.min(fd_count());
            if lowest <= before + FD_NOISE_TOLERANCE {
                break;
            }
        }
        assert!(
            lowest <= before + FD_NOISE_TOLERANCE,
            "IPC 接続 10 回で fd が {before} → {lowest}（6 秒間の最小値）に増えた（リーク）"
        );
    }

    #[test]
    fn 不正なトークンは認証エラーで拒否される() {
        let response = roundtrip(Some("bogus-token".into()));
        let error = response.error.expect("エラーになる");
        assert_eq!(error.code, error_code::AUTH);
        assert!(response.result.is_none());
    }

    #[test]
    fn dropでソケットファイルが消える() {
        let (tx, _rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        let path = server.endpoint().to_string();
        assert!(std::fs::metadata(&path).is_ok());
        drop(server);
        assert!(std::fs::metadata(&path).is_err());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::io::{BufRead, BufReader, Write};

    use futures::channel::mpsc::unbounded;
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::platform::named_pipe;
    use crate::protocol::{error_code, Request, RequestEnvelope, ResponseEnvelope};

    const TEST_TOKEN: &str = "test-token";

    /// サーバー + ダミーディスパッチャを立てて 1 往復する（unix 側テストの対）
    fn roundtrip(token_for_client: Option<String>) -> ResponseEnvelope {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).expect("IPC サーバーを起動できる");

        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({ "pong": true })));
            }
        });

        let token = token_for_client.unwrap_or_else(|| TEST_TOKEN.to_string());
        let stream =
            named_pipe::connect_client(server.endpoint(), 3_000).expect("パイプへ接続できる");
        let mut writer = stream.try_clone().unwrap();
        let envelope = RequestEnvelope::new(1, token, Request::List);
        writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).expect("レスポンスを解釈できる")
    }

    #[test]
    fn 正しいトークンでリクエストが往復する() {
        let response = roundtrip(None);
        assert_eq!(response.id, 1);
        assert_eq!(response.result.unwrap()["pong"], json!(true));
        assert!(response.error.is_none());
    }

    #[test]
    fn 不正なトークンは認証エラーで拒否される() {
        let response = roundtrip(Some("bogus-token".into()));
        let error = response.error.expect("エラーになる");
        assert_eq!(error.code, error_code::AUTH);
        assert!(response.result.is_none());
    }

    #[test]
    fn 連続接続が全件処理される() {
        let (tx, mut rx) = unbounded::<IncomingRequest>();
        let server = IpcServer::start(tx, TEST_TOKEN.into()).unwrap();
        std::thread::spawn(move || {
            while let Some(incoming) = futures::executor::block_on(rx.next()) {
                let _ = incoming.reply.send(Ok(json!({})));
            }
        });
        for i in 0..10 {
            let stream = named_pipe::connect_client(server.endpoint(), 3_000)
                .unwrap_or_else(|e| panic!("{i} 回目の接続に失敗: {e}"));
            let mut writer = stream.try_clone().unwrap();
            let envelope = RequestEnvelope::new(1, TEST_TOKEN.to_string(), Request::List);
            writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            assert!(!line.is_empty(), "{i} 回目の応答が空");
        }
    }
}
