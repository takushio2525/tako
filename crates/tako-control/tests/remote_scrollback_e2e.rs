//! `tako remote scrollback` の実経路テスト（#972。**実バイナリの器を使う**）
//!
//! 数値のペイン ID は「稼働中の tako-app へ IPC で問い合わせて器のセッション名へ
//! 解決する」経路を通る。ここでは **偽の IPC サーバー**（`List` にだけ答える）を
//! 立てて、GUI を起動せずにその経路を丸ごと通す。
//!
//! - 器は実物（macOS = tmux / Windows = psmux）。**ソケットは隔離**する
//! - discovery も隔離する（`TAKO_DISCOVERY_DIR`）ので、**本番の tako-app には触らない**
//! - env（`TAKO_DISCOVERY_DIR` / `TAKO_TMUX_SOCKET`）はプロセス全域なので、
//!   このバイナリは**テスト 1 本**だけを持ち、器の解決より先に設定する
//!
//! #972 以前はこの経路が数値 ID を器のセッションと取り違えて
//! （`=1` / `no server running on '<socket>__1'`）必ず失敗していた。

use std::process::Command;

use tako_control::discovery::ControlInfo;
use tako_control::ipc::IpcServer;
use tako_control::protocol::Request;

/// このテストが使う器のセッション名とペイン ID
const SESSION: &str = "tako972e2e";
const PANE_ID: u64 = 4972;

/// 器の CLI（実物）。器が無ければ `None`
fn container_bin() -> Option<String> {
    match tako_core::backend::binary() {
        tako_core::backend::Binary::Tmux { bin } => Some(bin.clone()),
        tako_core::backend::Binary::Psmux { bin, .. } => Some(bin.clone()),
        tako_core::backend::Binary::Absent => None,
    }
}

/// 器のセッションを作り、既知の行を流し込む。
///
/// **器があるのに作れなかったら黙って skip しない**（skip は「測っていない」を
/// 「緑」に見せる。#796 の作法）。`-x` / `-y` は器によっては受けないので、
/// 落ちたらサイズ指定なしで作り直す
fn start_session(bin: &str, socket: &str) -> Result<(), String> {
    let sized = run(
        bin,
        &[
            "-L",
            socket,
            "new-session",
            "-d",
            "-x",
            "80",
            "-y",
            "24",
            "-s",
            SESSION,
        ],
    );
    if sized.is_err() {
        run(bin, &["-L", socket, "new-session", "-d", "-s", SESSION])
            .map_err(|e| format!("器のセッションを作れない（サイズ指定あり / なしとも）: {e}"))?;
    }
    run(
        bin,
        &[
            "-L",
            socket,
            "send-keys",
            "-t",
            SESSION,
            "echo TAKO972_MARKER",
            "Enter",
        ],
    )
    .map_err(|e| format!("器へ送れない: {e}"))?;
    Ok(())
}

/// 器の CLI を 1 回叩く。失敗は stderr つきで返す
fn run(bin: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| format!("{bin} を実行できない: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn kill_session(bin: &str, socket: &str) {
    // **必ず `-L` 付き**（psmux の kill-server は `-L` を落とすと全ソケットを殺す）
    let _ = Command::new(bin)
        .args(["-L", socket, "kill-server"])
        .output();
}

/// `List` にだけ答える偽 IPC サーバーを立て、discovery へ登録する。
/// 返り値を drop するとソケットが片付く
fn spawn_fake_app(session: &str) -> IpcServer {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let token = "tako972-test-token".to_string();
    let server =
        IpcServer::start_with(tx, token.clone(), true).expect("IPC サーバーを立てられない");

    let list = serde_json::json!({
        "tabs": [{
            "id": 1,
            "title": "t",
            "panes": [{ "id": PANE_ID, "tmux_session": session }],
        }]
    });
    std::thread::spawn(move || {
        use futures::StreamExt;
        futures::executor::block_on(async move {
            while let Some(incoming) = rx.next().await {
                let reply = match incoming.request {
                    Request::List => Ok(list.clone()),
                    other => Err(tako_control::dispatch::DispatchError::Operation(format!(
                        "この偽 app は List しか答えない: {other:?}"
                    ))),
                };
                let _ = incoming.reply.send(reply);
            }
        });
    });

    tako_control::discovery::write_instance_only(&ControlInfo {
        version: 1,
        pid: std::process::id(),
        socket: server.endpoint().to_string(),
        token,
        mcp_url: None,
    })
    .expect("discovery へ書けない");
    server
}

/// **#972 の受け入れ条件 1**: 数値のペイン ID とセッション名の両方で
/// scrollback が返ること
#[test]
fn ペインidとセッション名の両方でscrollbackが返る() {
    // env は器の解決（OnceLock）より先に置く
    let socket = format!("tako-972e2e-{}", std::process::id());
    let disc = std::env::temp_dir().join(format!("tako-972e2e-disc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&disc);
    std::env::set_var("TAKO_DISCOVERY_DIR", &disc);
    std::env::set_var("TAKO_TMUX_SOCKET", &socket);

    let Some(bin) = container_bin() else {
        eprintln!("SKIP: 器になれるバイナリが無い（tmux / psmux）");
        return;
    };
    if let Err(e) = start_session(&bin, &socket) {
        panic!("器（{bin}）があるのにセッションを用意できない: {e}");
    }
    let _guard = SessionGuard {
        bin: bin.clone(),
        socket: socket.clone(),
        disc: disc.clone(),
    };

    // 器の出力が落ち着くまで待つ（固定待ちにしない）
    let mut by_session = Vec::new();
    for _ in 0..100 {
        by_session = tako_control::remote::scrollback(SESSION, 500).unwrap_or_default();
        if by_session.iter().any(|l| l.contains("TAKO972_MARKER")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        by_session.iter().any(|l| l.contains("TAKO972_MARKER")),
        "セッション名で採れていない: {by_session:?}"
    );

    // 数値ペイン ID は偽 app への IPC 越しに器のセッションへ解決される
    let _server = spawn_fake_app(SESSION);
    let by_pane = tako_control::remote::scrollback(&PANE_ID.to_string(), 500)
        .expect("ペイン ID で採れていない（IPC 解決が効いていない）");
    assert!(
        by_pane.iter().any(|l| l.contains("TAKO972_MARKER")),
        "ペイン ID で採れていない: {by_pane:?}"
    );

    // 解決できないペイン ID は**理由つきで**断る（黙って空を返さない）
    let err = tako_control::remote::scrollback("999999", 10)
        .expect_err("解決できないペイン ID が成功してしまった");
    assert!(err.contains("999999"), "理由に対象が出ていない: {err}");
}

struct SessionGuard {
    bin: String,
    socket: String,
    disc: std::path::PathBuf,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        kill_session(&self.bin, &self.socket);
        let _ = std::fs::remove_dir_all(&self.disc);
    }
}
