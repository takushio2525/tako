//! 閉じたポート = 繋ぐと拒否される番号（Issue #2373。テスト専用）。「オフラインの配布元」に使う
//!
//! ## なぜ要るか（注入で再現して確かめた）
//!
//! 以前は一時 listener を bind して drop した番号を使っていた。macOS の std は `socket()` の後で
//! 別の呼び出しで CLOEXEC を立てるので、その間に並列のテストが子（偽の言語サーバ等）を spawn すると
//! listener が子に残り、drop しても待ち受けが閉じない。取得は拒否されずに繋がって応答を待つ。
//! listener を 40 秒生きる子へ持ち出す注入で、`issue1956_lsp_servers_wave2` は応答の上限（30 秒）を
//! 待ちの天井（30 秒）の中で待ちきれずに落ち、`issue1944_lsp_fetch` は同じ番号へ配布元を立て直す
//! ところで `AddrInUse` で落ちた
//!
//! ## 作り（#2369 の `net_stop` のテストと同じ形）
//!
//! 一時 listener へ connect したクライアントの**手元側の番号**を、両方を握ったまま使う。一度も
//! listen していない番号なので、fd が子に残っても受け口にならず、握っている間は connect の送信元にも
//! 配られない。作った時点で、その番号への connect が拒否（`ConnectionRefused`）で落ちることを確かめる
//! （繋がってしまう番号では、取得は拒否ではなく応答の上限の時間切れで落ち、検査が待ちの天井との
//! 競争になる）。
//!
//! 同じ番号へ待ち受けを立て直すとき（#1944 の「ネットワークが戻った」）は、先に [`ClosedPort`] を
//! drop する。macOS の実測では、握っていた fd が子に残っていても、drop の後に同じ番号へ bind できる

use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub struct ClosedPort {
    pub port: u16,
    _held: (TcpListener, TcpStream),
}

pub fn closed_port() -> ClosedPort {
    let anchor = TcpListener::bind("127.0.0.1:0").expect("一時 listener を立てる");
    let near = TcpStream::connect(anchor.local_addr().unwrap()).expect("一時 listener へ繋ぐ");
    let port = near.local_addr().unwrap().port();
    // 10 秒は待ちではなく天井（拒否は macOS ではすぐ返り、Windows は RST の後に SYN を送り直してから返る）
    let refused =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(10));
    assert_eq!(
        refused.map(|_| ()).map_err(|e| e.kind()),
        Err(std::io::ErrorKind::ConnectionRefused),
        "閉じたポート {port} への connect は拒否で落ちる"
    );
    ClosedPort {
        port,
        _held: (anchor, near),
    }
}
