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
    match send_request(request) {
        Ok(value) => Ok(value["pane"].as_u64().map(|pane| ResolvedCaller {
            pane,
            method: value["method"].as_str().map(str::to_string),
        })),
        Err(e) => match send_request(Request::List) {
            Ok(list) => Ok(requested
                .filter(|p| list_contains_pane(&list, *p))
                .map(|pane| ResolvedCaller { pane, method: None })),
            Err(_) => Err(e),
        },
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
    /// 届いたが操作が失敗した
    Operation(String),
}

/// 接続情報の解決とフォールバック（FR-2.2.9）。
/// ①環境変数（`TAKO_SOCKET` / `TAKO_TOKEN`）で試行し、接続不可・認証失敗
/// （= アプリ再起動で env が古い）なら ②発見ファイルの候補列（current →
/// 生きているインスタンス。`discovery::read_candidates`）を順に再試行する。
/// 一時インスタンス（セルフテスト・二重起動）が current を上書きして exit しても、
/// 生きているメインへ自動で届く（2026-06-12 バグ (8) の恒久対策）。
/// 操作エラーはフォールバックせずそのまま返す。どの情報源も無ければ「tako の外」
pub fn send_request_via(request: Request, origin: Option<&str>) -> Result<Value, String> {
    send_request_classified(request, origin).map_err(|e| match e {
        SendFailure::Unreachable(m) | SendFailure::Broken(m) | SendFailure::Operation(m) => m,
    })
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
        }
    }
}

pub mod transport {
    //! Layer 1 IPC のクライアント側。ワイヤ処理（1 行 1 JSON）はトランスポート
    //! 非依存（`roundtrip_on`）で、接続の確立だけがプラットフォームで異なる
    //! （unix: Unix domain socket / windows: named pipe = 抽象境界 B3）

    use std::io::{BufRead, BufReader, Read, Write};

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
        let (read_half, write_half) = connect(socket)?;
        roundtrip_on(read_half, write_half, token, request, origin)
    }

    #[cfg(unix)]
    fn connect(socket: &str) -> Result<(impl Read, impl Write), TransportError> {
        use std::os::unix::net::UnixStream;
        let stream = UnixStream::connect(socket).map_err(|e| {
            TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
        })?;
        let read_half = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        Ok((read_half, stream))
    }

    #[cfg(windows)]
    fn connect(socket: &str) -> Result<(impl Read, impl Write), TransportError> {
        let stream =
            tako_control::platform::named_pipe::connect_client(socket, 3_000).map_err(|e| {
                TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
            })?;
        let read_half = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        Ok((read_half, stream))
    }

    fn roundtrip_on<R: Read, W: Write>(
        read_half: R,
        mut write_half: W,
        token: &str,
        request: Request,
        origin: Option<&str>,
    ) -> Result<Value, TransportError> {
        let mut envelope = RequestEnvelope::new(1, token, request);
        envelope.origin = origin.map(Into::into);
        let json = serde_json::to_string(&envelope)
            .map_err(|e| TransportError::Other(format!("送信の構築に失敗: {e}")))?;
        writeln!(write_half, "{json}")
            .map_err(|e| TransportError::Broken(format!("送信に失敗: {e}")))?;

        let mut line = String::new();
        BufReader::new(read_half)
            .read_line(&mut line)
            .map_err(|e| TransportError::Broken(format!("応答の受信に失敗: {e}")))?;
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
}
