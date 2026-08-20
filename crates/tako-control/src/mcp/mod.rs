//! mcp — Layer 2 内蔵 MCP サーバー（FR-2.3 / FR-2.5。最大の差別化点）
//!
//! Model Context Protocol の JSON-RPC 処理（initialize / tools/list / tools/call）と
//! ツールカタログをトランスポート非依存のエンジン（[`handle_message`]）として実装する。
//! 操作の実行は [`crate::dispatch`] へ委ねるため、ツールのセマンティクスは
//! CLI（Layer 1）と完全に一致する（設計原則 5「AI フルコントロール」）。
//!
//! トランスポートは 2 系統（採用理由と検証結果は `.agent/architecture.md`「Layer 2」節）:
//!
//! - **Streamable HTTP**（[`McpServer`]）: localhost バインド + Bearer トークン認証。
//!   接続先 URL を `TAKO_MCP_URL` として各ペインへ注入する。呼び出し元ペインは
//!   `X-Tako-Pane` ヘッダで申告する（FR-2.3.3）
//! - **stdio ブリッジ**（`tako mcp serve`、tako-cli 側）: Claude Code 等の stdio
//!   クライアント向け。このエンジンを共有し、実行だけ IPC へ中継する
//!
//! ツール説明文と initialize の `instructions` には FR-2.7.5 の行動規範
//! （レビューを求めるときは見せろ / 読んでほしければ開け / 方針相談は例を作って並べろ /
//! 終わったら片付けろ）を埋め込む。エージェントの振る舞いをプロンプトで誘導するのも
//! プロダクトの一部である。

mod catalog;
mod http;
mod request;

pub use catalog::tools;
pub use http::McpServer;

use futures::channel::mpsc::UnboundedSender;
use serde_json::{json, Value};
use tako_core::PaneOrigin;

use crate::ipc::IncomingRequest;
use crate::orchestrator::wait;
use crate::protocol::Request;
use request::{bool_arg, build_request, str_arg, u64_arg, validate_known_params};

#[cfg(test)]
use crate::protocol::Direction;

/// サーバーが既定で名乗る MCP プロトコルバージョン
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// 応答できるバージョン（クライアント申告がここにあればそのまま受ける）
const KNOWN_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// 1 接続分の文脈。トランスポート層（HTTP / stdio ブリッジ）が組み立てる
pub struct McpSession<'a> {
    /// 呼び出し元ペイン（stdio: `TAKO_PANE_ID`、HTTP: `X-Tako-Pane` ヘッダ。FR-2.3.3）。
    /// pane 引数が省略されたツール呼び出しのデフォルト対象になる
    pub caller_pane: Option<u64>,
    /// 呼び出し元のオーケストレーター role（stdio: `TAKO_ORCHESTRATOR_ROLE`）。
    /// 複数 master 並行時に caller_pane が stale でも正しい master を特定する（#109）
    pub caller_role: Option<String>,
    /// false のとき tools/list は空を返す（tako の外で起動された stdio ブリッジ用。
    /// 登録済みでも tako 外の Claude Code セッションを邪魔しない）
    pub connected: bool,
    /// 操作の実行係（HTTP: dispatch チャネル往復、stdio: IPC 往復）。
    /// Err は「ツール実行エラー」として isError 付き結果になる
    pub exec: &'a mut dyn FnMut(Request) -> Result<Value, String>,
    /// 非同期 run のポーリングスレッド用 IPC チャネル（#121）。
    /// HTTP 経路では tx.clone() でスレッドに渡す。stdio ブリッジでは None（sync のみ）
    pub ipc_tx: Option<UnboundedSender<IncomingRequest>>,
}

/// MCP メッセージを 1 件処理する。応答すべき JSON-RPC レスポンスを返す
/// （notification と response メッセージには `None`）
pub fn handle_message(message: &Value, session: &mut McpSession) -> Option<Value> {
    // method が無いものはクライアントからの response（ping への返事等）→ 無視
    let method = message.get("method")?.as_str()?.to_string();
    let id = match message.get("id") {
        // id 無し = notification（notifications/initialized 等）。応答しない
        None | Some(Value::Null) => return None,
        Some(id) => id.clone(),
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method.as_str() {
        "initialize" => Ok(initialize_result(&params, session.connected)),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools = if session.connected {
                tools()
            } else {
                Vec::new()
            };
            Ok(json!({ "tools": tools }))
        }
        "tools/call" => call_tool(&params, session),
        _ => Err((-32601, format!("メソッド {method} は未対応"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }),
    })
}

fn initialize_result(params: &Value, connected: bool) -> Value {
    // バージョン交渉: クライアント申告が既知ならそれを受け、未知なら最新を名乗る
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = match requested {
        Some(v) if KNOWN_VERSIONS.contains(&v) => v,
        _ => PROTOCOL_VERSION,
    };
    let instructions = if connected {
        INSTRUCTIONS
    } else {
        "tako アプリの外で起動されたため、ペイン操作ツールは提供されない。\
         tako 内のターミナルからエージェントを起動すると使えるようになる。"
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "tako",
            "title": "tako terminal",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": instructions,
    })
}

/// initialize で配るサーバー指示
const INSTRUCTIONS: &str = "\
あなたは今 tako ターミナル内で動いている。tako は AI エージェントが GUI ターミナルの画面\
（タブ / ペイン）をプログラマブルに操作できる環境であり、以下のツール群を通じて\
ペインの分割・コマンド実行・画面の読み取り・ファイルプレビュー・レイアウト管理ができる。\
通常のターミナルでは手作業が必要な画面操作を、AI が自律的に行えるのが最大の特徴。\n\
\n\
重要な概念:\n\
- タブ = 作業グループ（1 つのタスクや文脈ごとに 1 タブ）\n\
- ペイン = タブ内の個別のターミナル画面（分割して並べられる）\n\
- 各ペインには固有の ID があり、全操作はこの ID で対象を指定する\n\
- ペインを分割して作業ペインを増やし、不要になったら閉じるのが基本フロー\n\
\n\
行動規範（ユーザー体験の一部。意識的に従うこと）:\n\
- レビューを求めるときは見せろ: 作業結果を確認してもらうときは、口頭説明だけでなく\
成果物（diff・ファイル・実行結果）を tako_split_pane で新しいペインに開いて提示する\
（例: command=[\"git\",\"diff\",\"HEAD\"] や tako_open_file で差分やコードを見せる）\n\
- 読んでほしければ開け: ユーザーに読んでほしいドキュメントは、実際にペインで開いて見せる\n\
- 方針相談は例を作って並べろ: 複数案があるときは案ごとにペインを並べて同時に見せ、\
ユーザーが見比べて選べるようにする（tako_equalize_layout で整える）\n\
- 終わったら片付けろ: 役目を終えた作業ペインは tako_close_pane で閉じ、\
レイアウトが乱れたら tako_equalize_layout で整える\n\
- 操作の前に tako_list_panes で現状のレイアウトとペイン ID を把握する\n\
- 実行可能ファイルを新規作成したら先頭コメントに tako:run 宣言を書く: \
ユーザーが再生ボタン一発で実行できるようになる（書式は tako_run の説明を参照）";

enum SpecialTool {
    TaskGateCheck,
    OrchestratorRun,
}

fn special_tool(name: &str) -> Option<SpecialTool> {
    match name {
        "tako_task_gate_check" => Some(SpecialTool::TaskGateCheck),
        "tako_orchestrator_run" => Some(SpecialTool::OrchestratorRun),
        _ => None,
    }
}

fn call_tool(params: &Value, session: &mut McpSession) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "ツール名（name）が無い".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // 未知パラメータの検出（#227: タイポが黙って無視される事故を防ぐ）
    validate_known_params(name, &args)?;

    // gate check はコマンド実行を伴うため MCP ハンドラスレッドで直接実行する
    // （dispatch は UI スレッドで実行されるため長時間ブロック不可。#244）
    if matches!(special_tool(name), Some(SpecialTool::TaskGateCheck)) {
        let task_id = args
            .get("task_id")
            .and_then(Value::as_str)
            .ok_or((-32602, "task_id を指定する".to_string()))?;
        let sync = args
            .get("sync_checkpoint")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        return match crate::acceptance_gates::execute_gate_check(task_id, sync) {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                Ok(json!({ "content": [{ "type": "text", "text": text }] }))
            }
            Err(e) => Ok(json!({
                "content": [{ "type": "text", "text": e }],
                "isError": true,
            })),
        };
    }

    // orchestrator_run はポーリングループを伴うため MCP ハンドラスレッドで合成する
    // （dispatch は同期・UI スレッド実行のため長時間ブロック不可）
    if matches!(special_tool(name), Some(SpecialTool::OrchestratorRun)) {
        let ipc_tx = session.ipc_tx.as_ref().cloned();
        return orchestrator_run(&args, session, ipc_tx.as_ref());
    }

    let request = build_request(
        name,
        &args,
        session.caller_pane,
        session.caller_role.as_deref(),
    )
    .map_err(|e| (-32602, e))?;

    // list_panes の応答に caller_pane_id / caller_tab_id を付加する（#123）。
    // master が「自分がどこにいるか」を list で確認できる導線
    if name == "tako_list_panes" {
        return list_panes_with_caller(request, session);
    }

    // #283: remote の応答にトークンは存在しない（長寿命 bearer token を全廃。
    // 接続時の認証は機器ペアリング二層認証が行う）ため、除去処理は不要になった

    exec_and_wrap(request, session)
}

fn exec_and_wrap(request: Request, session: &mut McpSession) -> Result<Value, (i64, String)> {
    // 実行失敗は「ツール実行エラー」としてエージェントへ返す（MCP の isError。
    // エージェントが読んで自己修正できるよう、JSON-RPC エラーにはしない）
    Ok(match (session.exec)(request) {
        Ok(value) => {
            let text = match value {
                Value::Null => "ok".to_string(),
                value => value.to_string(),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": false })
        }
        Err(message) => {
            json!({ "content": [{ "type": "text", "text": message }], "isError": true })
        }
    })
}

/// list_panes の応答に caller_pane_id / caller_tab_id を後付けする（#123）
fn list_panes_with_caller(
    request: Request,
    session: &mut McpSession,
) -> Result<Value, (i64, String)> {
    match (session.exec)(request) {
        Ok(mut value) => {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("caller_pane_id".to_string(), json!(session.caller_pane));
                // caller_tab_id: caller_pane が属するタブを探す
                let caller_tab = session.caller_pane.and_then(|cpane| {
                    obj.get("tabs")?.as_array()?.iter().find_map(|tab| {
                        let panes = tab.get("panes")?.as_array()?;
                        if panes.iter().any(|p| p["id"].as_u64() == Some(cpane)) {
                            tab.get("id")?.as_u64()
                        } else {
                            None
                        }
                    })
                });
                obj.insert("caller_tab_id".to_string(), json!(caller_tab));
                if let Some(role) = &session.caller_role {
                    obj.insert("caller_role".to_string(), json!(role));
                }
            }
            let text = value.to_string();
            Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
        }
        Err(message) => {
            Ok(json!({ "content": [{ "type": "text", "text": message }], "isError": true }))
        }
    }
}

/// `tako_orchestrator_run` — spawn + 完了待ち + 出力取得 + close の合成操作（#121 で非同期化）。
/// 既定（sync=false）は spawn 後に即座に `{run_id, pane_id, ...}` を返す非同期モード。
/// sync=true は旧挙動（完了までブロッキング）を維持する後方互換モード。
/// `ipc_tx` は非同期モードのポーリングスレッド用 IPC チャネル。None のとき
/// 非同期モードは「IPC チャネルが渡されていない」エラーを返す（stdio ブリッジ等）
fn orchestrator_run(
    args: &Value,
    session: &mut McpSession,
    ipc_tx: Option<&UnboundedSender<IncomingRequest>>,
) -> Result<Value, (i64, String)> {
    let map_err = |e: String| (-32602i64, e);

    // --- パラメータ解析 ---
    let project = str_arg(args, "project")
        .map_err(map_err)?
        .ok_or((-32602, "project を指定する".to_string()))?;
    let prompt = str_arg(args, "prompt")
        .map_err(map_err)?
        .ok_or((-32602, "prompt を指定する".to_string()))?;
    let label = str_arg(args, "label").map_err(map_err)?;
    let pane_raw = u64_arg(args, "pane").map_err(map_err)?;
    let tab = u64_arg(args, "tab").map_err(map_err)?;
    let pane = if pane_raw.is_some() {
        pane_raw
    } else if tab.is_some() {
        None
    } else {
        session.caller_pane
    };
    let tab = if pane_raw.is_some() { None } else { tab };
    if pane.is_none() && tab.is_none() {
        return Err((-32602, "pane または tab を指定してください".into()));
    }
    let timeout_secs = u64_arg(args, "timeout_seconds")
        .map_err(map_err)?
        .unwrap_or(1800);
    let auto_close = bool_arg(args, "auto_close")
        .map_err(map_err)?
        .unwrap_or(true);
    let output_lines = u64_arg(args, "output_lines")
        .map_err(map_err)?
        .unwrap_or(200) as usize;
    let model = str_arg(args, "model").map_err(map_err)?;
    let effort = str_arg(args, "effort").map_err(map_err)?;
    let agent = str_arg(args, "agent").map_err(map_err)?;
    let sync_mode = bool_arg(args, "sync").map_err(map_err)?.unwrap_or(false);

    let task_type = str_arg(args, "task_type").map_err(map_err)?;
    let account = str_arg(args, "account").map_err(map_err)?;
    let opts = wait::RunOptions {
        project,
        prompt,
        label,
        model,
        effort,
        agent,
        pane,
        tab,
        caller_role: session.caller_role.clone(),
        timeout: std::time::Duration::from_secs(timeout_secs),
        auto_close,
        output_lines,
        initial_delay: std::time::Duration::from_secs(20),
        interval: std::time::Duration::from_secs(5),
        task_type,
        account,
    };

    if sync_mode {
        // 後方互換: 完了までブロッキング
        let result =
            wait::run_worker(&mut *session.exec, &opts, &mut |_, _| {}).map_err(|e| (-32602, e))?;
        return Ok(json!({
            "content": [{ "type": "text", "text": result.to_string() }],
            "isError": false,
        }));
    }

    // 非同期モード（#121）
    let tx = ipc_tx
        .ok_or((
            -32602,
            "非同期 run は HTTP MCP 経由でのみ利用可能（stdio は sync=true を指定してください）"
                .to_string(),
        ))?
        .clone();
    let result = wait::run_start(&mut *session.exec, &opts, move || {
        let tx = tx;
        Box::new(move |req: Request| -> Result<Value, String> {
            let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
            tx.unbounded_send(IncomingRequest {
                request: req,
                origin: PaneOrigin::Mcp,
                reply: reply_tx,
            })
            .map_err(|_| "アプリ側の受け口が閉じている".to_string())?;
            match reply_rx.recv() {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("アプリ側から応答が返らなかった".into()),
            }
        })
    })
    .map_err(|e| (-32602, e))?;
    Ok(json!({
        "content": [{ "type": "text", "text": result.to_string() }],
        "isError": false,
    }))
}

include!("tests.rs");
