//! **#1441 の実測**: 深い `TAKO_DATA_DIR` でも IPC サーバーが立ち、CLI が繋げる。
//!
//! `crates/tako-control/src/ipc.rs` の `preferred_socket_path` は `cfg!(test)` なら
//! 一時パスへ倒すので、**lib の単体テストからは本番の置き場を通れない**。
//! 統合テスト（別バイナリ）から呼ぶと tako-control 側の `cfg(test)` は立たないので、
//! ここが起動経路そのものを踏める唯一の場所になる。
//!
//! env（`TAKO_DATA_DIR`）はプロセス全域なので、**このファイルのテストは 1 本**に
//! まとめてある（並列実行で他のテストと取り合わない）。
//!
//! Windows は名前付きパイプで `sun_path` の上限が無いので、このファイルは丸ごと unix 限定。
//! 置き場の決め方（純関数）の検査は `issue1441_ipc_socket_watchdog` が両 OS で持つ。

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use futures::channel::mpsc::unbounded;
use serde_json::json;
use tako_control::protocol::{Request, RequestEnvelope, ResponseEnvelope};
use tako_control::{IncomingRequest, IpcServer};

const TOKEN: &str = "issue1441-token";

/// `<base>/<深い名前>/data` を 150 バイト超で作る（中身は `d` だけ = 個人情報を含まない）
fn deep_data_dir(tag: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("tako1441-{tag}-{}", std::process::id()));
    // 前回が途中で落ちていたら残骸が居るので、作る前に消す（#1296 と同じ作法。
    // 末尾の掃除は panic で飛ばされるが、ここは必ず通る）
    std::fs::remove_dir_all(&base).ok();
    let mut dir = base.clone();
    while dir.as_os_str().as_encoded_bytes().len() < 150 {
        dir = dir.join("dddddddddddddddddddddddddddddddddddddddd");
    }
    std::fs::create_dir_all(&dir).expect("検証用 data dir");
    dir
}

/// 1 往復させて応答を返す
fn roundtrip(endpoint: &str) -> ResponseEnvelope {
    let stream = UnixStream::connect(endpoint).expect("ソケットへ接続できる");
    let mut writer = stream.try_clone().unwrap();
    let envelope = RequestEnvelope::new(1, TOKEN.to_string(), Request::List);
    writeln!(writer, "{}", serde_json::to_string(&envelope).unwrap()).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).expect("レスポンスを解釈できる")
}

#[test]
fn 深いdatadirでもipcが立ち往復して参照ファイルが残る() {
    let data = deep_data_dir("ipc");
    let bytes = data.as_os_str().as_encoded_bytes().len();
    assert!(bytes > 150, "検証用 data dir が浅い: {bytes}");
    assert!(
        bytes + 1 + tako_core::ipc_socket::WELL_KNOWN_NAME.len()
            > tako_core::ipc_socket::max_path_bytes(),
        "固定パスが上限を超えていない = この検査に意味が無い",
    );
    std::env::set_var("TAKO_DATA_DIR", &data);
    std::env::remove_var("TAKO_SELF_TEST");
    tako_core::ipc_socket::reset_status_for_test();

    // 前回の残骸ソケットが在っても立つ（同じ data dir は同じ短縮パスへ落ちる）
    let plan = tako_core::ipc_socket::plan().expect("置き場を決められる");
    assert_eq!(plan.kind, tako_core::ipc_socket::SocketPathKind::Shortened);
    let _ = std::fs::remove_file(&plan.path);
    std::fs::write(&plan.path, b"stale").expect("残骸ソケットを置く");

    let (tx, mut rx) = unbounded::<IncomingRequest>();
    let server = IpcServer::start(tx, TOKEN.into()).expect("深い data dir でも IPC が立つ");
    std::thread::spawn(move || {
        while let Some(incoming) = futures::executor::block_on(futures::StreamExt::next(&mut rx)) {
            let _ = incoming.reply.send(Ok(json!({ "pong": true })));
        }
    });

    let endpoint = server.endpoint().to_string();
    assert!(
        endpoint.len() <= tako_core::ipc_socket::max_path_bytes(),
        "受け口が上限を超えている: {} バイト",
        endpoint.len(),
    );
    let response = roundtrip(&endpoint);
    assert_eq!(response.result.unwrap()["pong"], json!(true));

    // data dir 側には実体への参照だけが残り、繋ぐ側は同じ 1 実装で引ける
    assert_eq!(
        tako_core::ipc_socket::read_pointer(&data).map(|p| p.display().to_string()),
        Some(endpoint.clone()),
        "参照ファイルが実体を指していない",
    );
    assert_eq!(
        tako_core::ipc_socket::resolve_with(
            &data,
            &std::env::temp_dir(),
            tako_core::ipc_socket::max_path_bytes()
        )
        .display()
        .to_string(),
        endpoint,
    );
    assert!(
        !data.join(tako_core::ipc_socket::WELL_KNOWN_NAME).exists(),
        "data dir 直下に実体を置いている（#1441 の形が戻っている）",
    );

    // 立ったことが機械で読める（`check_health` が読む記録）
    let status = tako_core::ipc_socket::status().expect("記録が残る");
    assert!(status.bound);
    assert_eq!(status.endpoint.as_deref(), Some(endpoint.as_str()));
    assert_eq!(
        status.kind,
        tako_core::ipc_socket::SocketPathKind::Shortened
    );
    assert!(!status.too_long());

    // エッジ: **生きた先客が居る短縮パスは奪わない**（短縮パスは data dir ごとに
    // 安定なので、同じ data dir を指す 2 個目が unlink + bind すると 1 個目の
    // 受け口が黙って死ぬ = #113 が固定ソケットで潰した穴と同じ形）
    let (tx2, _rx2) = unbounded::<IncomingRequest>();
    let second = IpcServer::start(tx2, TOKEN.into()).expect("2 個目も立つ");
    assert_ne!(
        second.endpoint(),
        endpoint,
        "生きた先客の受け口を奪っている（#1441）",
    );
    assert_eq!(
        roundtrip(&endpoint).result.unwrap()["pong"],
        json!(true),
        "先客の受け口が 2 個目の起動で死んだ",
    );
    drop(second);

    // drop で実体と参照の両方が片付く
    drop(server);
    assert!(
        !std::path::Path::new(&endpoint).exists(),
        "実体が残っている"
    );
    assert!(
        tako_core::ipc_socket::read_pointer(&data).is_none(),
        "死んだ実体を指す参照が残っている",
    );

    std::env::remove_var("TAKO_DATA_DIR");
    tako_core::ipc_socket::reset_status_for_test();
    let base = std::env::temp_dir().join(format!("tako1441-ipc-{}", std::process::id()));
    std::fs::remove_dir_all(base).ok();
}
