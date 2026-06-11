//! mcp_host — MCP / IPC の実機検証用スタンドアロンホスト
//!
//! GUI（tako-app）を起動せずに IpcServer + McpServer + dispatch を立ち上げ、
//! `TAKO_*` 環境変数を注入した子プロセス（例: `claude -p`、検証シェル）を実行する。
//! Claude Code 実機検証（`scripts/verify-claude-mcp.sh`）がこの中で claude を走らせる。
//!
//! ターミナルセッションは持たない（attach は no-op）ため、send / read は
//! NoSession エラーになる。レイアウト操作（split / close / list / title / tab 系）の
//! 検証用と割り切る。
//!
//! 使い方: `cargo run -p tako-control --example mcp_host -- <command> [args...]`

use std::process::ExitCode;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

use tako_control::{ControlHost, IncomingRequest, IpcServer, McpServer};

/// セッションを持たないヘッドレスホスト（dispatch の検証用）
struct HeadlessHost {
    workspace: Workspace,
}

impl ControlHost for HeadlessHost {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(&mut self, _pane: PaneId) {}
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: mcp_host <command> [args...]");
        return ExitCode::from(2);
    }

    let (tx, mut rx) = unbounded::<IncomingRequest>();
    let token = tako_control::generate_token().expect("CSPRNG は使える前提の検証ツール");
    let ipc = IpcServer::start(tx.clone(), token.clone()).expect("IPC サーバーを起動できる");
    let mcp = McpServer::start(tx, token.clone()).expect("MCP サーバーを起動できる");

    let mut host = HeadlessHost {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    };
    let root_pane = host.workspace.active_tab().tree().focused();
    let tab_id = host.workspace.active_tab_id();

    // 受信リクエストを処理するディスパッチャ（UI イベントループの代役）
    std::thread::spawn(move || {
        while let Some(incoming) = futures::executor::block_on(rx.next()) {
            let result = tako_control::dispatch(&mut host, incoming.request, incoming.origin);
            let _ = incoming.reply.send(result);
        }
    });

    // 子プロセスへ tako 内のペインと同じ接続情報を注入する（FR-2.1.1 相当）
    let status = std::process::Command::new(&args[0])
        .args(&args[1..])
        .env("TAKO_SOCKET", ipc.endpoint())
        .env("TAKO_MCP_URL", mcp.url())
        .env("TAKO_TOKEN", &token)
        .env("TAKO_PANE_ID", root_pane.to_string())
        .env("TAKO_TAB_ID", tab_id.to_string())
        .status();
    match status {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("error: 子プロセスを起動できない（{}: {e}）", args[0]);
            ExitCode::FAILURE
        }
    }
}
