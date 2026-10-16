//! Layer 1 IPC のクライアント（`tako` CLI と MCP の stdio ブリッジ `tako-mcp` が共有する。#2021）。
//!
//! 接続情報の解決とフォールバック（FR-2.2.9）・呼び出し元ペインの特定（FR-2.1.1 / #567）・
//! ワイヤ処理（1 行 1 JSON）を持つ。#2021 までは `main.rs` の中にあったが、ブリッジ専用の
//! 小さな実行ファイル（`src/bin/tako-mcp.rs`）からも同じ 1 実装を使うためにここへ出した
//! （中身は 1 文字も変えていない）

use serde_json::Value;
use tako_control::protocol::Request;

/// tako の外で実行されたときのエラー（FR-2.2.8）。
/// 接続情報は環境変数 → 発見ファイル（FR-2.2.9）の順で解決した上での不在を意味する
pub const OUTSIDE_TAKO: &str = "tako アプリへの接続情報が無い（TAKO_SOCKET / TAKO_TOKEN 未設定・\
    接続情報ファイルも無し）。tako アプリを起動するか、tako 内のターミナルで実行してください";

/// `TAKO_PANE_ID`（呼び出し元ペイン）。tako 内のシェルなら必ず入っている（FR-2.1.1）
pub fn caller_pane() -> Option<u64> {
    std::env::var("TAKO_PANE_ID").ok()?.parse().ok()
}

/// 呼び出し元ペインの解決結果（Issue #567）
pub struct ResolvedCaller {
    pub pane: u64,
    /// 解決手段（`pid` / `pane` / `stale`。縮退経路では `None`）。案内文の出し分けに使う
    pub method: Option<String>,
}

/// アプリへ現世代の呼び出し元ペインを問い合わせる（Issue #567）。
/// 解決できなければ `Ok(None)`（エラーではない）。アプリへ届かないときだけ `Err`。
///
/// 新旧バイナリ混在（`resolve_pane` を知らない古い GUI が動いている）でも止まらないよう、
/// 問い合わせに失敗したら `list` でのペイン実在確認へ縮退する（stale map / pid 解決は
/// 効かないが、少なくとも「古い ID のまま起動して失敗する」ことは無くなる）
pub fn resolve_caller_pane_via_app(
    requested: Option<u64>,
) -> Result<Option<ResolvedCaller>, String> {
    let request = Request::ResolvePane {
        pane: requested,
        caller_pid: Some(std::process::id()),
    };
    match send_request_classified(request, None) {
        Ok(value) => Ok(value["pane"].as_u64().map(|pane| ResolvedCaller {
            pane,
            method: value["method"].as_str().map(str::to_string),
        })),
        // 応答しない GUI へ list を撃ち直しても同じだけ待つので縮退しない（#2123）
        Err(e @ SendFailure::Timeout(_)) => Err(e.message()),
        Err(e) => {
            let e = e.message();
            match send_request(Request::List) {
                Ok(list) => Ok(requested
                    .filter(|p| list_contains_pane(&list, *p))
                    .map(|pane| ResolvedCaller { pane, method: None })),
                Err(_) => Err(e),
            }
        }
    }
}

/// `list` 応答に該当ペインが居るか（Issue #567 の縮退経路）
pub fn list_contains_pane(list: &Value, pane: u64) -> bool {
    list["tabs"]
        .as_array()
        .is_some_and(|tabs| tabs.iter().any(|t| tab_contains_pane(t, pane)))
}

fn tab_contains_pane(tab: &Value, pane: u64) -> bool {
    tab["panes"]
        .as_array()
        .is_some_and(|panes| panes.iter().any(|p| p["id"].as_u64() == Some(pane)))
}

/// 環境変数から接続情報を読み、1 リクエストを往復させる
pub fn send_request(request: Request) -> Result<Value, String> {
    send_request_via(request, None)
}

/// 送れなかった理由（#1944: GUI が居ないときだけ CLI のプロセスで代わりに走らせる口のため）
pub enum SendFailure {
    /// tako アプリへ届かない（接続情報が無い・どの候補にも接続できない / 認証できない）
    Unreachable(String),
    /// 繋がったが応答の途中で切れた（送れない・応答が返らない・応答を読めない）。
    /// 順番待ち（#1994）だけが「届かない」と同じに扱い、他の呼び手は操作の失敗と同じに扱う
    Broken(String),
    /// 繋がったが上限まで応答が来ない（GUI が固まっている。#2123）。値は待った上限。
    /// 順番待ちは「届かない」と同じく待たずに走らせ、他の呼び手は操作の失敗と同じに扱う
    Timeout(std::time::Duration),
    /// 届いたが操作が失敗した
    Operation(String),
}

impl SendFailure {
    /// 利用者へ出す 1 行（時間切れは CLI・MCP のブリッジで同じ文言。#2123）
    pub fn message(self) -> String {
        match self {
            SendFailure::Unreachable(m) | SendFailure::Broken(m) | SendFailure::Operation(m) => m,
            SendFailure::Timeout(limit) => tako_control::protocol::reply_timeout_message(limit),
        }
    }
}

/// 接続情報の解決とフォールバック（FR-2.2.9）。
/// ①環境変数（`TAKO_SOCKET` / `TAKO_TOKEN`）で試行し、接続不可・認証失敗
/// （= アプリ再起動で env が古い）なら ②発見ファイルの候補列（current →
/// 生きているインスタンス。`discovery::read_candidates`）を順に再試行する。
/// 一時インスタンス（セルフテスト・二重起動）が current を上書きして exit しても、
/// 生きているメインへ自動で届く（2026-06-12 バグ (8) の恒久対策）。
/// 操作エラーはフォールバックせずそのまま返す。どの情報源も無ければ「tako の外」
pub fn send_request_via(request: Request, origin: Option<&str>) -> Result<Value, String> {
    send_request_classified(request, origin).map_err(SendFailure::message)
}

/// [`send_request_via`] の本体。届かなかったのか、届いて操作が失敗したのかを分けて返す
pub fn send_request_classified(
    request: Request,
    origin: Option<&str>,
) -> Result<Value, SendFailure> {
    let env_pair = match (std::env::var("TAKO_SOCKET"), std::env::var("TAKO_TOKEN")) {
        (Ok(socket), Ok(token)) if !socket.is_empty() && !token.is_empty() => Some((socket, token)),
        _ => None,
    };
    let mut last_failure = None;
    if let Some((socket, token)) = &env_pair {
        match transport::roundtrip(socket, token, request.clone(), origin) {
            Ok(value) => return Ok(value),
            Err(TransportError::Other(message)) => return Err(SendFailure::Operation(message)),
            Err(TransportError::Broken(message)) => return Err(SendFailure::Broken(message)),
            // 固まった GUI の残りの候補へ回ると待ちが候補の数だけ伸びるので、ここで返す
            Err(TransportError::Timeout(limit)) => return Err(SendFailure::Timeout(limit)),
            Err(stale) => last_failure = Some(stale),
        }
    }
    // 試行済みと同一内容の候補へ再試行しても無意味なので除外する。
    // 除外キーは (socket, token) ペア（socket だけで除外すると「正しいソケット +
    // 古いトークン」の認証失敗から正トークンで再試行できなくなる）
    let mut tried: Vec<(String, String)> = env_pair.iter().cloned().collect();
    for info in tako_control::discovery::read_candidates() {
        let key = (info.socket.clone(), info.token.clone());
        if tried.contains(&key) {
            continue;
        }
        tried.push(key);
        match transport::roundtrip(&info.socket, &info.token, request.clone(), origin) {
            Ok(value) => return Ok(value),
            Err(TransportError::Other(message)) => return Err(SendFailure::Operation(message)),
            Err(TransportError::Broken(message)) => return Err(SendFailure::Broken(message)),
            Err(TransportError::Timeout(limit)) => return Err(SendFailure::Timeout(limit)),
            // 死んだ残骸・別インスタンスのトークン → 次の候補へ
            Err(stale) => last_failure = Some(stale),
        }
    }
    Err(SendFailure::Unreachable(match last_failure {
        Some(stale) => stale.message(),
        None => OUTSIDE_TAKO.to_string(),
    }))
}

/// 接続試行の失敗種別。Connect / Auth は「環境変数が古い」可能性があり、
/// 発見ファイルへのフォールバック対象になる（FR-2.2.9）
pub enum TransportError {
    /// 接続できない（ソケット不在・アプリ停止）
    Connect(String),
    /// 認証失敗（トークンが古い = 別インスタンスのもの）
    Auth(String),
    /// 繋がった後の送受信の失敗（応答の途中で相手が落ちた等。フォールバックしない。#1994）
    Broken(String),
    /// 繋がったが上限まで応答が来ない（値は待った上限。フォールバックしない。#2123）
    Timeout(std::time::Duration),
    /// その他（操作エラー・プロトコルエラー。フォールバックしない）
    Other(String),
}

impl TransportError {
    pub fn message(self) -> String {
        match self {
            TransportError::Connect(m)
            | TransportError::Auth(m)
            | TransportError::Broken(m)
            | TransportError::Other(m) => m,
            TransportError::Timeout(limit) => tako_control::protocol::reply_timeout_message(limit),
        }
    }
}

pub mod transport {
    //! Layer 1 IPC のクライアント側。ワイヤ処理（1 行 1 JSON）はトランスポート
    //! 非依存（`roundtrip_on`）で、接続の確立だけがプラットフォームで異なる
    //! （unix: Unix domain socket / windows: named pipe = 抽象境界 B3）。
    //!
    //! 応答を待つ上限（#2123）は要求の種類から決まる（`protocol::reply_timeout` の 1 実装）。
    //! unix はソケットの読み書きの時間切れ（往復の全体に掛かる締め切り）、windows は
    //! 同期ハンドルの named pipe に読み取りの時間切れが無いので往復を別スレッドで回して待つ

    use std::io::{BufRead, BufReader, Read, Write};
    use std::time::Duration;

    use serde_json::Value;
    use tako_control::protocol::{error_code, Request, RequestEnvelope, ResponseEnvelope};

    use super::TransportError;

    /// `origin` は生成主体の自己申告（MCP ブリッジは `Some("mcp")`、CLI 直は `None`）
    pub fn roundtrip(
        socket: &str,
        token: &str,
        request: Request,
        origin: Option<&str>,
    ) -> Result<Value, TransportError> {
        let limit = tako_control::protocol::reply_timeout(&request);
        roundtrip_within(socket, token, request, origin, limit)
    }

    /// [`roundtrip`] の本体（上限を外から渡す = テストが env に依らない）。`None` は上限なし
    pub fn roundtrip_within(
        socket: &str,
        token: &str,
        request: Request,
        origin: Option<&str>,
        limit: Option<Duration>,
    ) -> Result<Value, TransportError> {
        let (read_half, write_half) = connect(socket, limit)?;
        exchange(read_half, write_half, token, request, origin, limit)
    }

    #[cfg(unix)]
    fn connect(
        socket: &str,
        limit: Option<Duration>,
    ) -> Result<(deadline::Stream, deadline::Stream), TransportError> {
        use std::os::unix::net::UnixStream;
        let stream = UnixStream::connect(socket).map_err(|e| {
            TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
        })?;
        let read_half = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        // 読み書きの両方に「繋がってから上限まで」の同じ締め切りを掛ける
        let at = limit.map(|l| std::time::Instant::now() + l);
        Ok((
            deadline::Stream::new(read_half, at),
            deadline::Stream::new(stream, at),
        ))
    }

    #[cfg(windows)]
    fn connect(
        socket: &str,
        _limit: Option<Duration>,
    ) -> Result<(std::fs::File, std::fs::File), TransportError> {
        let stream =
            tako_control::platform::named_pipe::connect_client(socket, 3_000).map_err(|e| {
                TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
            })?;
        let read_half = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        Ok((read_half, stream))
    }

    #[cfg(unix)]
    fn exchange(
        read_half: deadline::Stream,
        write_half: deadline::Stream,
        token: &str,
        request: Request,
        origin: Option<&str>,
        limit: Option<Duration>,
    ) -> Result<Value, TransportError> {
        roundtrip_on(read_half, write_half, token, request, origin, limit)
    }

    /// 上限を過ぎたら往復のスレッドは置いていく（GUI が返事をするかパイプが閉じれば抜ける）。
    /// CLI はこの直後に終わる。常駐する MCP のブリッジでは固まっている間の時間切れ 1 回につき
    /// 1 本残るが、GUI が戻って溜まった要求に答えるか GUI が終わればすべて抜ける
    #[cfg(windows)]
    fn exchange(
        read_half: std::fs::File,
        write_half: std::fs::File,
        token: &str,
        request: Request,
        origin: Option<&str>,
        limit: Option<Duration>,
    ) -> Result<Value, TransportError> {
        let Some(limit) = limit else {
            return roundtrip_on(read_half, write_half, token, request, origin, None);
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let token = token.to_string();
        let origin = origin.map(str::to_string);
        std::thread::spawn(move || {
            let result = roundtrip_on(
                read_half,
                write_half,
                &token,
                request,
                origin.as_deref(),
                Some(limit),
            );
            let _ = tx.send(result);
        });
        match rx.recv_timeout(limit) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(TransportError::Timeout(limit)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(TransportError::Broken(
                "応答を待つスレッドが途中で終わった".into(),
            )),
        }
    }

    fn roundtrip_on<R: Read, W: Write>(
        read_half: R,
        mut write_half: W,
        token: &str,
        request: Request,
        origin: Option<&str>,
        limit: Option<Duration>,
    ) -> Result<Value, TransportError> {
        // 締め切りを過ぎた読み書き（unix は EAGAIN = WouldBlock）は時間切れ、それ以外は途中で切れた
        let broken = |what: &str, e: std::io::Error| match limit {
            Some(limit)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                TransportError::Timeout(limit)
            }
            _ => TransportError::Broken(format!("{what}に失敗: {e}")),
        };
        let mut envelope = RequestEnvelope::new(1, token, request);
        envelope.origin = origin.map(Into::into);
        let json = serde_json::to_string(&envelope)
            .map_err(|e| TransportError::Other(format!("送信の構築に失敗: {e}")))?;
        writeln!(write_half, "{json}").map_err(|e| broken("送信", e))?;

        let mut line = String::new();
        BufReader::new(read_half)
            .read_line(&mut line)
            .map_err(|e| broken("応答の受信", e))?;
        if line.is_empty() {
            return Err(TransportError::Broken(
                "tako アプリから応答が返らなかった".into(),
            ));
        }
        let response: ResponseEnvelope = serde_json::from_str(&line)
            .map_err(|e| TransportError::Broken(format!("応答を解釈できない: {e}")))?;
        if let Some(error) = response.error {
            return Err(if error.code == error_code::AUTH {
                TransportError::Auth(error.message)
            } else {
                TransportError::Other(error.message)
            });
        }
        Ok(response.result.unwrap_or(Value::Null))
    }

    /// 締め切りつきの Unix domain socket（#2123）。読み書きのたびに残りの時間を入れ直すので、
    /// 上限は 1 回の read ではなく往復の全体に掛かる（少しずつ届く応答で延びない）
    #[cfg(unix)]
    mod deadline {
        use std::io::{self, Read, Write};
        use std::os::unix::net::UnixStream;
        use std::time::{Duration, Instant};

        pub struct Stream {
            stream: UnixStream,
            at: Option<Instant>,
        }

        impl Stream {
            pub fn new(stream: UnixStream, at: Option<Instant>) -> Self {
                Self { stream, at }
            }

            /// 残りの時間（締め切りが無ければ `None`、過ぎていれば `TimedOut`）
            fn left(&self) -> io::Result<Option<Duration>> {
                let Some(at) = self.at else {
                    return Ok(None);
                };
                let left = at.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                Ok(Some(left))
            }
        }

        impl Read for Stream {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if let Some(left) = self.left()? {
                    self.stream.set_read_timeout(Some(left))?;
                }
                self.stream.read(buf)
            }
        }

        impl Write for Stream {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if let Some(left) = self.left()? {
                    self.stream.set_write_timeout(Some(left))?;
                }
                self.stream.write(buf)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.stream.flush()
            }
        }

        // ---- Issue #2169: 相手が応答を書いてすぐ閉じても、届いた応答を読む ----
        #[cfg(test)]
        mod tests {
            use std::io::{BufRead, Read, Write};
            use std::os::unix::net::{UnixListener, UnixStream};
            use std::time::{Duration, Instant};

            use tako_control::protocol::{Request, ResponseEnvelope};

            use super::Stream;

            fn deadline_in(limit: Duration) -> Option<Instant> {
                Some(Instant::now() + limit)
            }

            #[test]
            fn 相手が書いてすぐ閉じても届いた分は読める() {
                let (ours, mut theirs) = UnixStream::pair().expect("対を作れる");
                theirs.write_all(b"response\n").expect("書ける");
                drop(theirs);
                let mut stream = Stream::new(ours, deadline_in(Duration::from_secs(10)));
                let mut got = Vec::new();
                let started = Instant::now();
                stream
                    .read_to_end(&mut got)
                    .expect("届いた応答を読めない（#2169 の EINVAL）");
                assert_eq!(got, b"response\n");
                assert!(started.elapsed() < Duration::from_secs(2), "待った");
            }

            #[test]
            fn 何も書かずに閉じた相手は待たずに終わりを返す() {
                let (ours, theirs) = UnixStream::pair().expect("対を作れる");
                drop(theirs);
                let mut stream = Stream::new(ours, deadline_in(Duration::from_secs(10)));
                let mut buf = [0u8; 16];
                let started = Instant::now();
                assert_eq!(stream.read(&mut buf).expect("EOF を返す"), 0);
                assert!(started.elapsed() < Duration::from_secs(2), "待った");
            }

            /// 要求の 1 行を書いた後に止まる書き口（相手が応答を書いて閉じるのを先に起こす）
            struct PauseAfterLine<W> {
                inner: W,
                pause: Duration,
            }

            impl<W: Write> Write for PauseAfterLine<W> {
                fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                    let n = self.inner.write(buf)?;
                    if buf[..n].contains(&b'\n') {
                        std::thread::sleep(self.pause);
                    }
                    Ok(n)
                }

                fn flush(&mut self) -> std::io::Result<()> {
                    self.inner.flush()
                }
            }

            #[test]
            fn 応答を書いてすぐ閉じる受け口でも往復は応答を返す() {
                let path = std::env::temp_dir()
                    .join(format!("tk2169-{}-close.sock", std::process::id()));
                let _ = std::fs::remove_file(&path);
                let listener = UnixListener::bind(&path).expect("受け口を作れる");
                let server = std::thread::spawn(move || {
                    let (stream, _) = listener.accept().expect("受けられる");
                    let mut line = String::new();
                    std::io::BufReader::new(stream.try_clone().expect("複製できる"))
                        .read_line(&mut line)
                        .expect("要求を読める");
                    let reply = ResponseEnvelope::ok(1, serde_json::json!({ "ok": true }));
                    let json = serde_json::to_string(&reply).expect("応答を組める");
                    let mut stream = stream;
                    writeln!(stream, "{json}").expect("応答を書ける");
                    // 応答を書いた直後に閉じる（GUI が応答を書いて終わった回）
                });
                let stream = UnixStream::connect(&path).expect("繋がる");
                let at = deadline_in(Duration::from_secs(10));
                let read_half = Stream::new(stream.try_clone().expect("複製できる"), at);
                let write_half = PauseAfterLine {
                    inner: Stream::new(stream, at),
                    pause: Duration::from_millis(300),
                };
                let got = super::super::roundtrip_on(
                    read_half,
                    write_half,
                    "t",
                    Request::List,
                    None,
                    Some(Duration::from_secs(10)),
                );
                server.join().expect("受け口が落ちない");
                let _ = std::fs::remove_file(&path);
                match got {
                    Ok(v) => assert_eq!(v["ok"], true),
                    Err(e) => panic!("届いた応答を読まずに失敗した: {}", e.message()),
                }
            }

            /// 読むたびに間を置く読み口（何回かに分けて読む応答の途中で相手が閉じるのを先に起こす）
            struct PauseBeforeRead<R> {
                inner: R,
                pause: Duration,
            }

            impl<R: Read> Read for PauseBeforeRead<R> {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    std::thread::sleep(self.pause);
                    self.inner.read(buf)
                }
            }

            /// 何回かに分けて読む大きさの応答を書き、書き終えたらすぐ閉じる受け口で往復する。
            /// 最後の数回の読みは相手が閉じた後になる（修正前はそこで EINVAL）
            #[test]
            fn 大きな応答を書いてすぐ閉じる受け口からも受け取れる() {
                const PAD: usize = 64 * 1024;
                let path =
                    std::env::temp_dir().join(format!("tk2169-{}-big.sock", std::process::id()));
                let _ = std::fs::remove_file(&path);
                let listener = UnixListener::bind(&path).expect("受け口を作れる");
                let server = std::thread::spawn(move || {
                    let (stream, _) = listener.accept().expect("受けられる");
                    let mut line = String::new();
                    std::io::BufReader::new(stream.try_clone().expect("複製できる"))
                        .read_line(&mut line)
                        .expect("要求を読める");
                    let pad = "x".repeat(PAD);
                    let reply = ResponseEnvelope::ok(1, serde_json::json!({ "pad": pad }));
                    let json = serde_json::to_string(&reply).expect("応答を組める");
                    let mut stream = stream;
                    // 読み手が途中で諦めたら書けなくなるだけ（判定は読み手の側でする）
                    let _ = writeln!(stream, "{json}");
                });
                let stream = UnixStream::connect(&path).expect("繋がる");
                let at = deadline_in(Duration::from_secs(10));
                let read_half = PauseBeforeRead {
                    inner: Stream::new(stream.try_clone().expect("複製できる"), at),
                    pause: Duration::from_millis(50),
                };
                let got = super::super::roundtrip_on(
                    read_half,
                    Stream::new(stream, at),
                    "t",
                    Request::List,
                    None,
                    Some(Duration::from_secs(10)),
                );
                server.join().expect("受け口が落ちない");
                let _ = std::fs::remove_file(&path);
                match got {
                    Ok(v) => assert_eq!(v["pad"].as_str().map(str::len), Some(PAD)),
                    Err(e) => panic!("届いた応答を読まずに失敗した: {}", e.message()),
                }
            }

            #[test]
            fn 締め切りを過ぎた読みは閉じた相手でも時間切れ() {
                let (ours, mut theirs) = UnixStream::pair().expect("対を作れる");
                theirs.write_all(b"late\n").expect("書ける");
                drop(theirs);
                let mut stream = Stream::new(ours, Some(Instant::now()));
                let mut buf = [0u8; 16];
                let e = stream.read(&mut buf).expect_err("締め切りを過ぎている");
                assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Issue #567: stale な TAKO_PANE_ID からの master / solo 起動 ----

    fn list_with_panes(ids: &[u64]) -> Value {
        serde_json::json!({
            "tabs": [{
                "id": 1,
                "panes": ids.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>(),
            }],
        })
    }

    #[test]
    fn list_contains_paneは現存判定に使える() {
        let list = list_with_panes(&[780, 781]);
        assert!(list_contains_pane(&list, 780));
        assert!(
            !list_contains_pane(&list, 305),
            "旧世代の ID は現存しないと判定される（#567 の実事象）"
        );
        // 応答が壊れていても panic せず「居ない」に倒す
        assert!(!list_contains_pane(&serde_json::json!({}), 780));
    }

    // ---- Issue #2123: 接続を受けたまま返事をしない受け口で永久に待たない ----

    mod reply_timeout {
        use std::time::{Duration, Instant};

        use tako_control::protocol::{Request, ResponseEnvelope};

        use crate::ipc::transport::roundtrip_within;
        use crate::ipc::TransportError;

        /// 偽の受け口の振る舞い
        #[derive(Clone, Copy)]
        enum Fake {
            /// 接続を受けて要求も読まず、返事をしないまま持ち続ける（固まった GUI）
            Silent,
            /// 要求を 1 行読み、`delay` 置いてから応答を返す
            ReplyAfter(Duration),
        }

        /// 受け口を 1 本立てて、1 接続だけ `fake` の通りに振る舞う。戻り値は接続先
        #[cfg(unix)]
        fn serve(tag: &str, fake: Fake) -> String {
            use std::os::unix::net::UnixListener;
            let path =
                std::env::temp_dir().join(format!("tk2123-{}-{tag}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect("受け口を作れる");
            std::thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    behave(stream.try_clone().expect("複製できる"), stream, fake);
                }
            });
            path.to_string_lossy().into_owned()
        }

        #[cfg(windows)]
        fn serve(tag: &str, fake: Fake) -> String {
            use tako_control::platform::named_pipe;
            let name = format!(r"\\.\pipe\tako-test-2123-{}-{tag}", std::process::id());
            let instance = named_pipe::create_server_instance(&name, true).expect("パイプを作れる");
            std::thread::spawn(move || {
                if let Ok(stream) = instance.wait_client() {
                    behave(stream.try_clone().expect("複製できる"), stream, fake);
                }
            });
            name
        }

        fn behave<R: std::io::Read, W: std::io::Write>(
            read_half: R,
            mut write_half: W,
            fake: Fake,
        ) {
            use std::io::BufRead;
            match fake {
                Fake::Silent => {
                    // 返事をしないまま持ち続ける（テストの上限より十分長く）
                    std::thread::sleep(Duration::from_secs(20));
                    drop((read_half, write_half));
                }
                Fake::ReplyAfter(delay) => {
                    let mut line = String::new();
                    let _ = std::io::BufReader::new(read_half).read_line(&mut line);
                    std::thread::sleep(delay);
                    let reply = ResponseEnvelope::ok(1, serde_json::json!({ "ok": true }));
                    let json = serde_json::to_string(&reply).expect("応答を組める");
                    let _ = writeln!(write_half, "{json}");
                    let _ = write_half.flush();
                }
            }
        }

        #[test]
        fn 返事をしない受け口は上限で時間切れを返す() {
            let endpoint = serve("silent", Fake::Silent);
            let limit = Duration::from_millis(400);
            let started = Instant::now();
            let result = roundtrip_within(&endpoint, "t", Request::List, None, Some(limit));
            let waited = started.elapsed();
            match result {
                Err(TransportError::Timeout(got)) => assert_eq!(got, limit),
                Err(e) => panic!("時間切れではない: {}", e.message()),
                Ok(v) => panic!("返事をしない受け口から応答が来た: {v}"),
            }
            assert!(waited >= limit, "上限より早く諦めた: {waited:?}");
            assert!(
                waited < Duration::from_secs(5),
                "上限を過ぎても待ち続けた: {waited:?}"
            );
        }

        #[test]
        fn 上限の内に返る応答は受け取れる() {
            let endpoint = serve("slow", Fake::ReplyAfter(Duration::from_millis(300)));
            let got = roundtrip_within(
                &endpoint,
                "t",
                Request::List,
                None,
                Some(Duration::from_secs(10)),
            );
            match got {
                Ok(v) => assert_eq!(v["ok"], true),
                Err(e) => panic!("上限の内の応答を取りこぼした: {}", e.message()),
            }
        }

        #[test]
        fn 上限なしは応答まで待つ() {
            let endpoint = serve("nolimit", Fake::ReplyAfter(Duration::from_millis(600)));
            match roundtrip_within(&endpoint, "t", Request::List, None, None) {
                Ok(v) => assert_eq!(v["ok"], true),
                Err(e) => panic!("上限なしで応答を取りこぼした: {}", e.message()),
            }
        }

        #[test]
        fn 時間切れの文言は上限の秒数を出す() {
            let m = TransportError::Timeout(Duration::from_secs(30)).message();
            assert!(m.starts_with("tako アプリが 30 秒応答しない"), "{m}");
        }
    }
}
