//! JSON-RPC のフレーミングと id 管理（#1678）
//!
//! LSP のベースプロトコル: `Content-Length: <len>\r\n\r\n<body>`。型は `lsp-types`、
//! 配線はここで自作する（Zed と同じ形。`async-lsp` は既定 features に tokio が入る）。
//!
//! `Read` / `Write` に対して書くので、テストは `Cursor` やパイプを渡して
//! 分割受信・1 回の read に複数メッセージ・不正な JSON を固定できる
//! （`.agent/plans/2026-09-lsp-s1.md` §13 (a)）。

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use serde_json::Value;

/// 1 メッセージの上限（壊れた / 悪意のある `Content-Length` でメモリを食い尽くさない）
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// 1 メッセージを書く（ヘッダ + 本文を 1 回の `write_all` にまとめる）
pub fn write_message(out: &mut impl Write, message: &Value) -> io::Result<()> {
    out.write_all(&frame(message))?;
    out.flush()
}

/// 1 メッセージぶんのバイト列（送信キューへ積む形）
pub fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// 受信の失敗
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("読み取りに失敗: {0}")]
    Io(#[from] io::Error),
    #[error("Content-Length が大きすぎる（{0} バイト）")]
    TooLarge(usize),
    /// 本文が JSON ではない。**ストリームは次のメッセージから読み続けられる**
    #[error("本文が JSON ではない: {0}")]
    InvalidJson(serde_json::Error),
}

/// 1 メッセージを読む。ストリームの終わりなら `Ok(None)`。
///
/// - ヘッダは空行まで読む。`Content-Length` 以外のヘッダ（`Content-Type`）と、
///   ヘッダに見えない行（ログインシェルの rc が stdout へ漏らした行など）は読み飛ばす
/// - 本文は `Content-Length` ぶんを `read_exact` する = 1 メッセージが複数回の read に
///   割れても、1 回の read に複数メッセージが乗っても同じ結果になる
/// - `Content-Length` の無いヘッダの塊は捨てて次を読む
pub fn read_message(input: &mut impl BufRead) -> Result<Option<Value>, ReadError> {
    loop {
        let mut length: Option<usize> = None;
        let mut saw_header = false;
        loop {
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if saw_header {
                    break;
                }
                continue;
            }
            saw_header = true;
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().ok();
                }
            }
        }
        let Some(length) = length else {
            continue;
        };
        if length > MAX_MESSAGE_BYTES {
            return Err(ReadError::TooLarge(length));
        }
        let mut body = vec![0; length];
        input.read_exact(&mut body)?;
        return serde_json::from_slice(&body)
            .map(Some)
            .map_err(ReadError::InvalidJson);
    }
}

/// 要求の id（LSP は整数か文字列）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

impl RequestId {
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) => n.as_i64().map(Self::Int),
            Value::String(s) => Some(Self::Str(s.clone())),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Int(n) => Value::from(*n),
            Self::Str(s) => Value::from(s.clone()),
        }
    }
}

/// 受け取ったメッセージの種類
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// こちらの要求への応答
    Response {
        id: RequestId,
        result: Result<Value, ResponseError>,
    },
    /// サーバからの要求（応答を返さないとサーバが待ち続ける）
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    /// サーバからの通知
    Notification { method: String, params: Value },
    /// どれにも当たらない形
    Invalid,
}

/// エラー応答
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

/// メッセージを種類に分ける
pub fn classify(message: Value) -> Incoming {
    let Value::Object(mut map) = message else {
        return Incoming::Invalid;
    };
    let id = map.get("id").and_then(RequestId::from_value);
    let method = map
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let params = map.remove("params").unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => Incoming::Request { id, method, params },
        (None, Some(method)) => Incoming::Notification { method, params },
        (Some(id), None) => {
            let result = match map.remove("error") {
                Some(error) => Err(ResponseError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }),
                None => Ok(map.remove("result").unwrap_or(Value::Null)),
            };
            Incoming::Response { id, result }
        }
        (None, None) => Incoming::Invalid,
    }
}

pub fn request_message(id: &RequestId, method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id.to_value(), "method": method, "params": params })
}

pub fn notification_message(method: &str, params: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

pub fn response_message(id: &RequestId, result: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id.to_value(), "result": result })
}

pub fn error_response_message(id: &RequestId, code: i64, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.to_value(),
        "error": { "code": code, "message": message },
    })
}

/// JSON-RPC の「そのメソッドは知らない」
pub const METHOD_NOT_FOUND: i64 = -32601;

/// 応答待ちの表。**待ちを解いたら（応答 / タイムアウト / 取り消し）必ずここから消す**
#[derive(Debug)]
pub struct PendingTable<T> {
    next_id: i64,
    entries: HashMap<RequestId, Pending<T>>,
}

#[derive(Debug)]
struct Pending<T> {
    method: String,
    deadline: Instant,
    waiter: T,
}

impl<T> Default for PendingTable<T> {
    fn default() -> Self {
        Self {
            next_id: 1,
            entries: HashMap::new(),
        }
    }
}

impl<T> PendingTable<T> {
    /// 新しい id を払い出して待ちを登録する
    pub fn register(
        &mut self,
        method: &str,
        timeout: Duration,
        now: Instant,
        waiter: T,
    ) -> RequestId {
        let id = RequestId::Int(self.next_id);
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.entries.insert(
            id.clone(),
            Pending {
                method: method.to_string(),
                deadline: now + timeout,
                waiter,
            },
        );
        id
    }

    /// 応答が来た待ちを取り出す（表から消える）。知らない id なら `None`
    pub fn resolve(&mut self, id: &RequestId) -> Option<(String, T)> {
        self.entries.remove(id).map(|p| (p.method, p.waiter))
    }

    /// 期限を過ぎた待ちを取り出す（表から消える）
    pub fn expire(&mut self, now: Instant) -> Vec<(RequestId, String, T)> {
        let expired: Vec<RequestId> = self
            .entries
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.entries.remove(&id).map(|p| (id, p.method, p.waiter)))
            .collect()
    }

    /// 全部取り出す（プロセスが死んだとき）
    pub fn drain(&mut self) -> Vec<(RequestId, String, T)> {
        self.entries
            .drain()
            .map(|(id, p)| (id, p.method, p.waiter))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufReader, Cursor, Read};

    #[test]
    fn ヘッダと本文を組む() {
        let bytes = frame(&json!({"a": 1}));
        assert_eq!(bytes, b"Content-Length: 7\r\n\r\n{\"a\":1}");
        let mut out = Vec::new();
        write_message(&mut out, &json!({"a": 1})).unwrap();
        assert_eq!(out, bytes);
    }

    #[test]
    fn 一回の_read_に複数メッセージが乗っても順に読める() {
        let mut bytes = frame(&json!({"n": 1}));
        bytes.extend(frame(&json!({"n": 2})));
        let mut input = Cursor::new(bytes);
        assert_eq!(read_message(&mut input).unwrap(), Some(json!({"n": 1})));
        assert_eq!(read_message(&mut input).unwrap(), Some(json!({"n": 2})));
        assert_eq!(read_message(&mut input).unwrap(), None);
    }

    /// 1 バイトずつしか返さない読み手（分割受信の最悪形）
    struct Trickle(Cursor<Vec<u8>>);

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(1);
            self.0.read(&mut buf[..n])
        }
    }

    #[test]
    fn 一バイトずつ届いても一メッセージとして読める() {
        let message = json!({"method": "日本語", "params": {"x": "😀"}});
        let mut input = BufReader::with_capacity(1, Trickle(Cursor::new(frame(&message))));
        assert_eq!(read_message(&mut input).unwrap(), Some(message));
    }

    #[test]
    fn 他のヘッダとヘッダに見えない行は読み飛ばす() {
        let body = r#"{"ok":true}"#;
        let raw = format!(
            "welcome from rc\r\n\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut input = Cursor::new(raw.into_bytes());
        assert_eq!(read_message(&mut input).unwrap(), Some(json!({"ok": true})));
    }

    #[test]
    fn 不正な_json_の後ろも読み続けられる() {
        let mut bytes = b"Content-Length: 5\r\n\r\n{nope".to_vec();
        bytes.extend(frame(&json!({"n": 2})));
        let mut input = Cursor::new(bytes);
        assert!(matches!(
            read_message(&mut input),
            Err(ReadError::InvalidJson(_))
        ));
        assert_eq!(read_message(&mut input).unwrap(), Some(json!({"n": 2})));
    }

    #[test]
    fn 大きすぎる_content_length_は拒否する() {
        let raw = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        let mut input = Cursor::new(raw.into_bytes());
        assert!(matches!(
            read_message(&mut input),
            Err(ReadError::TooLarge(_))
        ));
    }

    #[test]
    fn 本文の途中で切れたら読み取りの失敗() {
        let mut input = Cursor::new(b"Content-Length: 10\r\n\r\n{\"a\"".to_vec());
        assert!(matches!(read_message(&mut input), Err(ReadError::Io(_))));
    }

    #[test]
    fn 種類に分ける() {
        assert_eq!(
            classify(json!({"id": 3, "result": {"x": 1}})),
            Incoming::Response {
                id: RequestId::Int(3),
                result: Ok(json!({"x": 1})),
            }
        );
        assert_eq!(
            classify(json!({"id": "a", "error": {"code": -32800, "message": "cancelled"}})),
            Incoming::Response {
                id: RequestId::Str("a".into()),
                result: Err(ResponseError {
                    code: -32800,
                    message: "cancelled".into(),
                }),
            }
        );
        assert_eq!(
            classify(
                json!({"id": 9, "method": "workspace/configuration", "params": {"items": []}})
            ),
            Incoming::Request {
                id: RequestId::Int(9),
                method: "workspace/configuration".into(),
                params: json!({"items": []}),
            }
        );
        assert_eq!(
            classify(json!({"method": "window/logMessage"})),
            Incoming::Notification {
                method: "window/logMessage".into(),
                params: Value::Null,
            }
        );
        assert_eq!(classify(json!([1, 2])), Incoming::Invalid);
    }

    #[test]
    fn 応答とタイムアウトと全消去で待ちの表から消える() {
        let now = Instant::now();
        let mut table = PendingTable::default();
        let a = table.register("initialize", Duration::from_secs(10), now, "a");
        let b = table.register("shutdown", Duration::from_secs(1), now, "b");
        assert_ne!(a, b);
        assert_eq!(table.len(), 2);
        // 期限前は何も消えない
        assert!(table.expire(now).is_empty());
        let expired = table.expire(now + Duration::from_secs(2));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, b);
        assert_eq!(table.len(), 1, "タイムアウトした待ちは表から消える");
        assert_eq!(table.resolve(&a), Some(("initialize".into(), "a")));
        assert!(table.resolve(&a).is_none(), "2 度目は引けない");
        assert!(table.is_empty());
        table.register("x", Duration::from_secs(1), now, "c");
        assert_eq!(table.drain().len(), 1);
        assert!(table.is_empty());
    }
}
