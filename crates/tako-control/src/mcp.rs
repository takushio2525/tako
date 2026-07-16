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

use std::io;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use serde_json::{json, Value};
use tako_core::PaneOrigin;

use crate::ipc::IncomingRequest;
use crate::orchestrator::wait;
use crate::protocol::{Axis, Direction, Request};

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
- 操作の前に tako_list_panes で現状のレイアウトとペイン ID を把握する";

/// ペイン ID 引数のスキーマ（省略時は呼び出し元）
fn pane_schema(description: &str) -> Value {
    json!({ "type": "integer", "minimum": 0, "description": description })
}

/// 公開ツールカタログ（FR-2.5 と 1:1。CLI のサブコマンドと同じ操作セット）
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "tako_list_panes",
            "description": "タブとペインのツリー構造・ジオメトリ（位置・サイズ・分割比率）・\
                状態（タイトル・role・origin・フォーカス・cwd・state・listen_ports・surface）を JSON で返す。\
                shelved_panes（バックグラウンドに退避されたペイン）も含む。\
                state はシェル統合（OSC 133）由来で idle / running / failed（exit_code 付き）\
                / unknown（統合なし）。surface はそのペインが前面表示中か裏で実行中かの分類で\
                foreground（アクティブタブ所属＝画面に出ている）/ background（非アクティブタブ＝裏で実行中）。\
                listen_ports はペイン配下プロセスが listen 中の\
                TCP ポート（dev サーバーの起動検知に使える）。エージェントや dev サーバーの\
                実行状況の把握に使える。\
                ペインを操作する前にまずこれを呼び、現状のレイアウトとペイン ID を把握すること。",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        }),
        json!({
            "name": "tako_split_pane",
            "description": "ペインを分割して新しいターミナルペインを作り、新ペイン ID を返す。\
                command を指定するとシェルの代わりにそのコマンドを実行する\
                （dev サーバーの起動、`git diff` やファイルビューアの表示に使う）。\
                ユーザーに成果物を見せるとき・レビューを求めるときは、このツールで結果を\
                開いて提示すること（見せたいものは口頭で説明せず実際に開く）。\
                対象の指定方法: pane（特定ペインの隣に生やす）または tab（そのタブの\
                フォーカス中ペインの隣に生やす。ユーザーがどのタブを見ていても正確に\
                対象タブ内に分割できる）。どちらも省略すると呼び出し元ペインの隣に生える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("分割の基準ペイン ID（省略時は呼び出し元ペインの隣に生える。tab と排他）"),
                    "tab": {
                        "type": "integer", "minimum": 0,
                        "description": "分割先タブ ID（そのタブのフォーカス中ペインの隣に生える。pane と排他。\
                            特定タブ内に確実に分割したいときに使う）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "新ペインが生える方向（省略時は right）",
                    },
                    "ratio": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "exclusiveMaximum": 1.0,
                        "description": "新ペイン側の取り分（省略時は等分）",
                    },
                    "command": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "シェルの代わりに実行するコマンドと引数（例: [\"npm\",\"run\",\"dev\"]）。\
                            終了するとペインも閉じる。省略時は対話シェルが起動する",
                    },
                    "cwd": { "type": "string", "description": "新ペインの作業ディレクトリ" },
                    "focus": {
                        "type": "boolean",
                        "description": "新ペインにフォーカスを移すか（省略時は false = 分割元を維持。\
                            ユーザーの入力中にフォーカスを奪わない）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_send_input",
            "description": "指定ペインの端末へテキストを書き込む（既定で末尾に改行を付けて実行する）。\
                対象の誤指定はそのまま誤実行になるため、必ず tako_list_panes で確認した\
                ペイン ID を渡すこと。tmux_session を指定するとペインが見つからない場合でも \
                tmux session 経由で送信できる。await_prompt を true にすると、claude TUI の\
                プロンプト（❯）が表示されるまで待ってからテキストを送信する。\
                claude 等の全画面 TUI への改行つき送信は送達確認ループで配送される: \
                信頼ダイアログの自動承諾 → bracketed paste 貼り付け → 分離 Enter → \
                入力欄が空になったことの検証 + Enter 単独再送（マルチラインもそのまま送れる。\
                応答は queued: true が即座に返り、実際の送達確認はバックグラウンドで行われる）。\
                text を空にして newline: true にすると Enter 単独送信になる: 入力欄に残った\
                テキストの送信代行に使え、入力欄が空へ戻るまで Enter を自動再送する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "送信先ペイン ID（必須）" },
                    "text": { "type": "string", "description": "送信するテキスト" },
                    "newline": {
                        "type": "boolean",
                        "description": "末尾に改行を付けるか（省略時 true。プロンプトへの部分入力は false）",
                    },
                    "tmux_session": {
                        "type": "string",
                        "description": "tmux session 名（pane ID 解決不能時のフォールバック。tako_orchestrator_spawn の返り値に含まれる）",
                    },
                    "await_prompt": {
                        "type": "boolean",
                        "description": "true にすると claude TUI の ❯ プロンプト表示を待ってから送信する（省略時 false）。\
                            子の Claude Code にメッセージを送るときに使う。送信はバックグラウンドで行われ、\
                            応答は即座に返る（queued: true）",
                    },
                },
                "required": ["pane", "text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_read_pane",
            "description": "指定ペインの画面内容（表示中のテキスト）を返す。\
                別ペインで実行したコマンドの結果確認や、エージェント・dev サーバーの出力監視に使う。\
                tmux_session を指定するとペインが見つからない場合でも tmux session 経由で読める。\
                応答の input_status は Claude Code TUI の入力行（❯）のテキスト属性を示す: \
                style が ghost なら自動提案（ゴーストテキスト）、user なら手動入力、\
                mixed なら混在、none なら入力テキストなし。❯ 行が見つからなければ null。\
                重要: ghost の場合はユーザーの意図した入力ではないため、送信してはならない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "対象ペイン ID（必須）" },
                    "lines": { "type": "integer", "minimum": 1, "description": "末尾からの行数制限" },
                    "tmux_session": {
                        "type": "string",
                        "description": "tmux session 名（pane ID 解決不能時のフォールバック。tako_orchestrator_spawn の返り値に含まれる）",
                    },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_scroll_pane",
            "description": "ペインのスクロールバック表示を動かす。\
                to は絶対位置（0 = 最下部、大きいほど過去）、delta は相対行数（正 = 過去方向）。\
                どちらか一方を指定する。応答に現在の offset と history（保持行数）を返す。\
                過去の出力を確認するときは tako_read_pane と組み合わせる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "対象ペイン ID（省略時は呼び出し元）" },
                    "to": { "type": "integer", "minimum": 0, "description": "絶対位置（0 = 最下部）" },
                    "delta": { "type": "integer", "description": "相対行数（正 = 過去方向）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_list",
            "description": "実行中の全 tmux セッションを一覧する。各セッションの\
                window 一覧・作成日時・attach 状態に加え、attach クライアントが tako の\
                どのタブ・ペインに表示されているか（pane / tab が null なら tako 外の\
                ターミナル由来）を返す。消し忘れて裏で動き続ける tmux の発見に使う。\
                backend = true のセッションは tako 自身のペイン永続化用: kill すると\
                対応ペイン（backend_pane）の中身が消えるため、通常は対象にしないこと。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当。省略時は既定サーバー）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_kill",
            "description": "tmux セッション（window 指定時はその window）を kill する。\
                **破壊的操作**: 中で動いているプロセスごと終了する。必ず tako_tmux_list で\
                対象を確認し、ユーザーの同意を得てから実行すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "対象セッション名（必須）" },
                    "window": { "type": "integer", "minimum": 0, "description": "window index（指定時は kill-window、省略時は kill-session）" },
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当）" },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_resize",
            "description": "tmux window を指定サイズ（cols × rows）へリサイズする。\
                スマホリモート（Issue #23）のビューポート連動用で、tmux の window-size が \
                manual に切り替わる。PC 側の表示に合わせ直すときは reset=true で解除する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "対象セッション名（必須）" },
                    "window": { "type": "integer", "minimum": 0, "default": 0, "description": "window index（省略時は 0）" },
                    "cols": { "type": "integer", "minimum": 1, "description": "幅（桁数）。reset なしなら rows と併せて必須" },
                    "rows": { "type": "integer", "minimum": 1, "description": "高さ（行数）。reset なしなら cols と併せて必須" },
                    "reset": { "type": "boolean", "description": "true で manual サイズを解除しサーバー既定へ戻す" },
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当）" },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_open",
            "description": "tmux セッションを現在のタブへ取り込んで表示する。\
                pane を direction（省略時は右）へ分割した新ペインで attach クライアントを\
                起動する。管理外・kill 漏れセッション（tako_tmux_list で発見したもの）の\
                中身をユーザーに見せる・自分で確認するときに使う。\
                新ペインを閉じてもセッション側は終了しない（kill ではない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "対象セッション名（必須。tako_tmux_list の name）" },
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当。tako_tmux_list の socket をそのまま渡す）" },
                    "pane": pane_schema("分割の基準ペイン ID（省略時は呼び出し元の隣に生える）"),
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "新ペインが生える方向（省略時は right）",
                    },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_cleanup",
            "description": "取り残された orphan tmux セッションを一括クリーンアップする。\
                tako バックエンドサーバー上の detached・非 grouped・未使用の tako- セッション\
                （前回クラッシュ等で残った裸のバックエンドセッション）だけを kill し、kill した\
                名前を返す。**使用中（attached）・表示中ビュー・ユーザーの実セッションには\
                一切触れない**ため tako_tmux_kill より安全。消し忘れ掃除の定型操作に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "socket": { "type": "string", "description": "tmux サーバー名（tmux -L 相当。省略時は tako バックエンドサーバー）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tmux_select_window",
            "description": "バックエンドセッション内の tmux window を切り替える。\
                pane のバックエンドセッション内で指定した window index をアクティブにする。\
                tako tmux list でペインの backend セッションの windows を確認してから使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "対象ペイン ID（省略時は呼び出し元）" },
                    "window": { "type": "integer", "minimum": 0, "description": "切り替え先 window index（必須）" },
                },
                "required": ["window"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_video_playback",
            "description": "動画プレビューペインの再生/一時停止/音量/ループを操作する。\
                対象ペインが動画プレビュー（tako open で .mp4/.mov 等を開いた状態）の場合のみ有効。\
                action: play / pause / toggle / rate:N（N は 0.1〜4.0 の速度倍率、例: rate:2.0）/ \
                mute / unmute / toggle_mute / loop_on / loop_off / toggle_loop。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "action": {
                        "type": "string",
                        "description": "再生操作（play / pause / toggle / rate:N / mute / unmute / toggle_mute / loop_on / loop_off / toggle_loop）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_video_seek",
            "description": "動画プレビューペインのシーク位置を指定する（秒単位の絶対位置）。\
                対象ペインが動画プレビューの場合のみ有効。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "seconds": { "type": "number", "minimum": 0, "description": "シーク先の秒数（絶対位置）" },
                },
                "required": ["seconds"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_video_volume",
            "description": "動画プレビューペインの音量を設定する（0.0〜1.0）。\
                対象ペインが動画プレビューの場合のみ有効。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "volume": { "type": "number", "minimum": 0, "maximum": 1, "description": "音量（0.0〜1.0）" },
                },
                "required": ["volume"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_focus_pane",
            "description": "ペインへフォーカスを移す。pane（ID 指定。別タブならタブも切り替わる）か\
                direction（アクティブタブ内の隣接移動）のどちらか一方を指定する。\
                ユーザーに見てほしいペインへ注意を向ける用途にも使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "minimum": 0, "description": "フォーカス先ペイン ID" },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "隣接ペインへの方向移動",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_close_pane",
            "description": "ペインを閉じる。pane 省略時は呼び出し元自身（自分のペイン）を閉じる。\
                役目を終えた作業ペインはこのツールで片付けること。\
                タブ最後の 1 ペインならタブごと閉じる（最後のタブの最後のペインは閉じられない）。\
                orchestrator-worker role のペインは busy 時に close が拒否される。\
                強制 close するには force: true を指定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元 = 自己片付け）"),
                    "force": {
                        "type": "boolean",
                        "description": "true にすると busy な worker でも強制的に close する（省略時 false）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_resize_pane",
            "description": "ペインの取り分（サイズ比率）を変える。delta は相対変更（正で拡大）、\
                share は 0–1 の絶対指定で、どちらか一方だけを渡す。\
                ユーザーに見せたいペインを広げる用途にも使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "axis": {
                        "type": "string",
                        "enum": ["x", "y"],
                        "description": "x = 横幅、y = 縦幅",
                    },
                    "delta": { "type": "number", "description": "取り分の相対変更量（例: 0.1 / -0.1）" },
                    "share": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "exclusiveMaximum": 1.0,
                        "description": "取り分の絶対指定",
                    },
                },
                "required": ["axis"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_equalize_layout",
            "description": "タブ内の全ペインのサイズを均等化する。作業後にレイアウトが乱れたら\
                これで整えること。複数案をペインで並べて見せるときの仕上げにも使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_set_title",
            "description": "ペインの表示タイトルと役割ラベル（role。例: worker-1, dev-server）を設定する。\
                ペインを作ったら役割が分かる名前を付け、ユーザーが監視しやすくすること。\
                空文字を渡すとクリアする。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "title": { "type": "string", "description": "表示タイトル" },
                    "role": { "type": "string", "description": "役割ラベル" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_rename_tab",
            "description": "タブの表示タイトルを変更する。明示リネームとして\
                自動リネームより優先される。空文字を渡すと手動指定を解除し、\
                自動リネーム（有効時）が再びタブ名を更新するようになる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）" },
                    "title": { "type": "string", "description": "新しいタブタイトル（空文字で手動指定を解除）" },
                },
                "required": ["title"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_create_tab",
            "description": "新しいタブ（= エージェントグループ）を作り、タブ ID と初期ペイン ID を返す。\
                いまのタブと無関係な作業系列を始めるときに使う（1 グループ = 1 タブ）。\
                既定ではアクティブタブは変わらない（ユーザーの入力を奪わない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "タブのタイトル（省略時は連番）" },
                    "focus": { "type": "boolean", "description": "true にすると新タブをアクティブにする（省略時は false = 現在のタブを維持）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_select_tab",
            "description": "表示するタブを切り替える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "アクティブにするタブ ID" },
                },
                "required": ["tab"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_move_pane_to_tab",
            "description": "ペインを移動する。tab 指定 = 別タブの末尾へ移送（グループ分け）、\
                target 指定 = そのペインの隣（direction 側）へ挿し直す（同タブ内の並べ替え = \
                ペインタイトルバーの D&D と同じ操作。タブまたぎも可）、new_tab = true で新タブとして分離。\
                tab / target / new_tab は排他。既定ではアクティブタブは変わらない（ユーザーの入力を奪わない）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "移送先タブ ID（target / new_tab と排他）" },
                    "target": { "type": "integer", "minimum": 0, "description": "挿入先ペイン ID（このペインの隣に入る）" },
                    "new_tab": { "type": "boolean", "description": "true = 新しいタブとして分離する（tab / target と排他）" },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "target のどちら側に入るか（省略時は right。target 指定時のみ有効）",
                    },
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "focus": { "type": "boolean", "description": "true にすると移動先タブをアクティブにする（省略時は false = 現在のタブを維持）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_port_detect",
            "description": "listen ポート検知 + 提案チップの ON/OFF を\
                切り替える（enabled 省略時は現在状態の取得のみ）。設定は永続化される。\
                有効時、各ペインの listen 中 TCP ポートは tako_list_panes の listen_ports で読める。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_auto_rename",
            "description": "タブ・ペイン名の AI 自動リネームの ON/OFF を切り替える\
                （enabled 省略時は現在状態の取得のみ）。設定は永続化される。\
                手動で付けた名前（tako_set_title / tako_rename_tab）は自動より常に優先される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_panel",
            "description": "右サイドバー情報パネルの表示・非表示・幅・ビュー切替と、\
                左サイドバーのファイルツリーの表示・非表示を操作する（全省略で現在状態の取得）。\
                view=tmux はタブごとの全ペイン一覧 + 管理外 / kill 漏れ tmux セッションの統合ビュー、\
                view=git は git graph（実装まではプレースホルダ）。ユーザーに tmux や\
                エージェントの状況を見せたいとき表示し、邪魔なら隠す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "visible": { "type": "boolean", "description": "true = 表示、false = 非表示" },
                    "width": { "type": "number", "exclusiveMinimum": 0, "description": "パネル幅（px）" },
                    "view": { "type": "string", "enum": ["tmux", "orch", "git"], "description": "表示するビュー（orch = オーケストレーター俯瞰。#217）" },
                    "filetree": { "type": "boolean", "description": "左サイドバーのファイルツリーの表示・非表示" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_collapse_tab",
            "description": "サイドバー tmux ビューのタブ枠を折りたたむ / 展開する。\
                折りたたむと、そのタブ配下のバックグラウンド項目（裏で実行中のペイン行 + バックグラウンド）を\
                隠し、前面表示中の行は残す。雑然とした一覧を畳んで注目すべきタブだけ見せたいときに使う。\
                collapsed 省略でトグル、tab 省略で呼び出し元のタブ。現在状態は tako_list_panes の\
                各タブ collapsed でも取得できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "description": "対象タブの ID（省略時は呼び出し元ペインのタブ）" },
                    "pane": pane_schema("タブ解決に使う基準ペイン ID（tab 省略時。省略時は呼び出し元）"),
                    "collapsed": { "type": "boolean", "description": "true = 折りたたむ、false = 展開（省略時はトグル）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_pin_preview",
            "description": "サイドバー tmux ビューのバックグラウンドペイン、または閉じたタブグループの\
                実画面サムネイルを、アプリ内のフローティングウィンドウとして常駐させる（ライブ更新し続ける）。\
                裏で動いているペインを画面に出さず見張りたいときに使う。pane = 対象ペイン、\
                group_tab = 閉じたタブグループの由来タブ ID（排他、どちらも省略で呼び出し元ペイン）。\
                pinned=false で解除、省略でトグル。現在のピンは tako_list_panes の pinned で確認できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("ピン留めするペイン ID（省略時は呼び出し元）"),
                    "group_tab": { "type": "integer", "description": "閉じたタブグループの由来タブ ID（pane と排他）" },
                    "pinned": { "type": "boolean", "description": "true = ピン留め、false = 解除（省略時はトグル）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_open_file",
            "description": "ファイルをプレビューペインで開いてユーザーに見せる。\
                コードはシンタックスハイライト付き、Markdown は既定でレンダリング表示\
                （mode=code でソース表示へ切替可能 = プレビューの目アイコントグルと同じ操作）。\
                ペインは再利用される: 対象がプレビューペインなら差し替え、同タブに既存の\
                プレビューペインがあればそこへ、無ければ pane を分割して生やす（ターミナルは\
                起動しない）。direction を指定すると再利用せず必ずその方向へ分割して開く\
                （表示位置を制御したいとき）。「このファイルを見て」「成果物を確認して」の\
                提示に使うこと。相対パスは pane の cwd 基準で解決する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("基準ペイン ID（省略時は呼び出し元。プレビューの表示先解決に使う）"),
                    "path": { "type": "string", "description": "開くファイルのパス（必須。相対パスは pane の cwd 基準）" },
                    "mode": {
                        "type": "string",
                        "enum": ["code", "markdown"],
                        "description": "表示モード（省略時は拡張子から自動判定。.md / .markdown → markdown）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "指定時は既存プレビューを再利用せず pane をこの方向へ分割して開く",
                    },
                    "focus": { "type": "boolean", "description": "true にするとプレビューペインにフォーカスを移す（省略時は false = 元ペインを維持）" },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_view",
            "description": "PDF・画像プレビューのズーム・ページ・パンを操作する。全操作を省略すると現在状態を返す。\
                zoom は百分率（150 = 150%）、page は 1 始まり。zoom と page を同時指定できるため、\
                『3 ページ目を 150% で見せて』を 1 回で実行できる。zoom_in / zoom_out は 1 段階、\
                reset は幅フィット（100%）+ パン位置リセット。pan_x / pan_y は現在位置へ加える logical px。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 PDF・画像プレビューペイン ID（省略時は呼び出し元）"),
                    "zoom": { "type": "number", "minimum": 25, "maximum": 400, "description": "表示倍率（百分率）" },
                    "zoom_in": { "type": "boolean", "description": "true = 1 段階ズームイン" },
                    "zoom_out": { "type": "boolean", "description": "true = 1 段階ズームアウト" },
                    "reset": { "type": "boolean", "description": "true = 100% + パン位置リセット" },
                    "page": { "type": "integer", "minimum": 1, "description": "PDF の表示ページ（1 始まり）" },
                    "pan_x": { "type": "number", "description": "横パン差分（logical px。正 = 右）" },
                    "pan_y": { "type": "number", "description": "縦パン差分（logical px。正 = 下）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_outline",
            "description": "Markdown 見出しまたは PDF 目次のアウトラインを取得し、項目へジャンプする。\
                item は返却順の 1 始まり。item を省略すると一覧取得だけを行う。Markdown の重複見出しも\
                別項目として保持され、PDF 項目は PDFKit のリンク先ページへ移動する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 Markdown・PDF プレビューペイン ID（省略時は呼び出し元）"),
                    "item": { "type": "integer", "minimum": 1, "description": "ジャンプするアウトライン項目（表示順の 1 始まり）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_link_list",
            "description": "PDF プレビュー内のリンク（外部 URL・内部ページ参照）を一覧する。\
                ページインデックスは 0 始まり、リンクの index は follow-link で使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 PDF プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_follow_link",
            "description": "PDF プレビュー内のリンクをフォローする。外部 URL はブラウザで開き、\
                内部リンクは該当ページへジャンプする。index は link-list の結果で得られる 0 始まりインデックス。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象 PDF プレビューペイン ID（省略時は呼び出し元）"),
                    "index": { "type": "integer", "minimum": 0, "description": "フォローするリンクのインデックス（0 始まり）" },
                },
                "required": ["index"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_reload",
            "description": "表示中プレビューファイルのライブリロードを設定する。enabled 省略時は現在状態を返す。\
                有効時は外部変更をイベント駆動で検知し、デバウンス後に background で再構築する。\
                編集中の外部変更は表示内容を上書きせず競合として通知する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = ライブリロード ON（既定）、false = OFF" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_cache",
            "description": "PDF・画像・動画サムネのデコード済み画像キャッシュをバイト予算つき LRU で管理する。\
                max_mb 省略時は現在の上限・使用 bytes・entry 数を返す。変更値は settings.json に永続化する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "max_mb": {
                        "type": "integer",
                        "minimum": 256,
                        "maximum": 8192,
                        "description": "キャッシュ上限（MiB、既定 512）"
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_edit",
            "description": "コードプレビューの編集モードを開始・終了する。enabled 省略時は状態取得。\
                PDF・画像・動画・末尾省略された巨大ファイルは編集できない。状態は editing / dirty で返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "enabled": { "type": "boolean", "description": "true = 編集開始、false = 編集終了（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_apply",
            "description": "コードプレビューの編集バッファ全文を text で置き換える。編集モード未開始なら開始する。\
                ファイルへはまだ書き込まず dirty になるため、続けて tako_preview_save を呼ぶ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "text": { "type": "string", "description": "適用するファイル全文（UTF-8）" },
                },
                "required": ["text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_save",
            "description": "コードプレビューの未保存編集をファイルへ書き戻す。読み込み後に外部変更があれば競合として拒否し、上書きしない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_undo",
            "description": "コードプレビュー編集の undo。直前の編集操作を取り消す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_redo",
            "description": "コードプレビュー編集の redo。取り消した操作をやり直す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_search",
            "description": "コードプレビューのテキスト検索。query でインクリメンタル検索し、direction で移動（next/prev）。\
                編集モードでなくても使える。query 省略時は現在の検索状態を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "query": { "type": "string", "description": "検索文字列（大文字小文字区別なし）" },
                    "direction": { "type": "string", "enum": ["next", "prev"], "description": "移動方向（省略時は next）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_replace",
            "description": "コードプレビューのテキスト置換。query に一致する箇所を replacement で置換する。all=true で全置換。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "query": { "type": "string", "description": "検索文字列" },
                    "replacement": { "type": "string", "description": "置換文字列" },
                    "all": { "type": "boolean", "description": "true = 全置換、false = 1 件（既定 false）" },
                },
                "required": ["query", "replacement"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_preview_autosave",
            "description": "コードプレビュー編集の自動保存設定。enabled 省略時は状態取得のみ。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象プレビューペイン ID（省略時は呼び出し元）"),
                    "enabled": { "type": "boolean", "description": "true = 自動保存 ON（既定）、false = 手動保存" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_file_op",
            "description": "ファイル操作を実行する。op で種別を指定:\n\
                copy_absolute_path = 絶対パスを取得 / copy_relative_path = ペイン cwd 基準の相対パスを取得 /\n\
                reveal = Finder でファイルの場所を表示（macOS）/\n\
                open_terminal = 指定パスのディレクトリへペイン内で cd /\n\
                rename = name でファイル名を変更 / create_file = path 配下に name でファイル作成 /\n\
                create_dir = path 配下に name でフォルダ作成 / trash = ゴミ箱へ移動。\n\
                rename / create_file / create_dir は name パラメータが必須。\
                open_terminal / copy_relative_path は pane パラメータでペインを指定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["copy_absolute_path","copy_relative_path","reveal","open_terminal","rename","create_file","create_dir","trash"],
                        "description": "操作種別",
                    },
                    "path": { "type": "string", "description": "対象のファイル・フォルダパス（必須）" },
                    "name": { "type": "string", "description": "新しい名前（rename / create_file / create_dir で必須）" },
                    "pane": pane_schema("対象ペイン ID（open_terminal の cd 先 / copy_relative_path の基準。省略時は呼び出し元）"),
                },
                "required": ["op", "path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_persist",
            "description": "セッション永続化の ON/OFF を切り替える（enabled 省略時は現在\
                状態の取得のみ）。有効時、タブ / ペイン構成は tmux の有無に関わらず保存・\
                復元される。tmux があれば各ペインは tako 専用 tmux サーバーのセッションと\
                して保持され、実行中プロセスごと復元される。available = false は tmux 不在で\
                構成のみ永続化（復元時は保存 cwd の新シェル）に劣化していることを示す。\
                切替は以後生成されるペインに効く。設定は永続化される。応答の layout_path /\
                layout_exists / last_restore / log_path で保存先と直近の復元結果を診断できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_confirm_close",
            "description": "タブ / ペインの × ボタンで閉じる際の確認ダイアログの ON/OFF を\
                切り替える（enabled 省略時は現在状態の取得のみ）。有効時、× クリックで\
                「失われるもの」を要約した確認ダイアログを表示し、⌘クリックでスキップできる。\
                設定は config.yaml に永続化される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean", "description": "true = 有効化、false = 無効化（省略時は状態取得）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_log",
            "description": "git リポジトリのコミット履歴・ブランチ一覧・変更状態を取得する。\
                対象ペインの cwd から git リポジトリを解決する。\
                コミットグラフ描画・ブランチ操作の判断材料として使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "max_count": { "type": "integer", "description": "取得するコミット数上限（省略時 200）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_git_diff",
            "description": "git diff を取得する。対象ペインの cwd の\
                リポジトリの diff をファイル・ハンク・行単位で返す。target で種別を指定: \
                \"unstaged\"（ワーキングツリー変更。既定）/ \"staged\"（ステージ済み）/ \
                コミットハッシュ（そのコミットの差分）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン"),
                    "target": { "type": "string", "description": "diff 種別: unstaged / staged / コミットハッシュ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_background_pane",
            "description": "ペインまたはタブをバックグラウンドへ送る。プロセスは生きたまま\
                画面から外す。邪魔なペインやタブを画面外へ送るのに使う。バックグラウンドのペインは\
                tako_background_list で確認でき、tako_foreground_pane で画面に戻せる。\
                tab 指定時はタブ内全ペインを一括退避する（pane と tab は排他）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "バックグラウンドへ送るペインの ID（省略時は呼び出し元。tab と排他）" },
                    "tab": { "type": "integer", "description": "バックグラウンドへ送るタブの ID（タブ内全ペインを一括退避。pane と排他）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_foreground_pane",
            "description": "バックグラウンドのペインを画面に復帰させる。target ペインの\
                direction 側を分割して表示する。target 省略時は由来タブへ戻す\
                （由来タブが閉じていればアクティブタブ）。バックグラウンドで動かしていたペインを取り出すのに使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "復帰させるペインの ID（background list から取得）" },
                    "target": { "type": "integer", "description": "挿入先ペインの ID（省略時はフォーカス中ペイン）" },
                    "direction": { "type": "string", "enum": ["right","down","left","up"], "description": "分割方向（省略時は right）" },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_background_list",
            "description": "バックグラウンドのペイン一覧を取得する。各ペインの\
                ID / title / role / state / cwd に加え、由来タブ（origin_tab / origin_tab_title）と\
                surface（常に background = 裏で実行中）を返す。バックグラウンドペインはこの由来タブで\
                グループ分けして表示され、tako_foreground_pane で由来タブへ戻せる。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_background_kill",
            "description": "バックグラウンドのペインを kill する。プロセスとバックエンド\
                セッションも終了する。復帰不要なペインの片付けに使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "kill するペインの ID" },
                },
                "required": ["pane"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_check_health",
            "description": "tako 環境の健全性を診断する。接続直後に呼んで環境に問題がないか確認すること。\
                チェック項目: tako CLI が PATH に通っているか / CLI とアプリのバージョンが一致するか / \
                外部ツール（tmux 等）の有無 / セッション永続化の状態。\
                問題がある場合は issue 配列に修正方法の提案を含めて返す。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup_mcp",
            "description": "Claude Code の settings.json に tako MCP サーバーの接続設定を\
                自動追加する。初回セットアップ時に呼ぶ。既に設定済みなら何もしない。\
                scope=global（既定）は ~/.claude/settings.json、scope=project は\
                呼び出し元ペインの cwd 配下 .claude/settings.json に書き込む。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["global", "project"],
                        "description": "設定の書き込み先スコープ（省略時は global = ~/.claude/settings.json）",
                    },
                    "pane": pane_schema("対象ペイン ID（scope=project 時の cwd 解決に使う。省略時は呼び出し元）"),
                },
                "additionalProperties": false,
            },
        }),
        // --- オーケストレーター MCP ツール ---
        json!({
            "name": "tako_orchestrator_projects",
            "description": "オーケストレーターのプロジェクトを管理する。\
                action=list で登録済みプロジェクト一覧、add で新規追加、remove で削除。\
                プロジェクトは projects.yaml に保存され、tako_orchestrator_spawn の\
                対象として使える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "add", "remove"],
                        "description": "操作種別（省略時は list）",
                    },
                    "key": { "type": "string", "description": "プロジェクトキー（add / remove 時に必須）" },
                    "cwd": { "type": "string", "description": "作業ディレクトリ（add 時に必須。~ は $HOME に展開される）" },
                    "description": { "type": "string", "description": "プロジェクトの説明（add 時に任意）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_profiles",
            "description": "オーケストレーターのプロファイル（tako master の起動設定）を管理する。\
                action=list で一覧、show で単一表示、set で作成・更新。\
                プロファイルは profiles/<name>.yaml に保存され、master のエージェント種別・\
                モデル・effort と子 worker のモデル決定に使われる。model が null / 未指定の\
                プロファイルはその CLI の既定モデルで起動する（プラン非依存・推奨）。\
                1M コンテキスト版（[1m] サフィックス）は Max / API プラン限定のため、\
                set で明示指定した場合のみ使われる（Pro プランでは起動不能になる点に注意）。\
                master のエージェント種別は master_agent（claude / codex。agy は master 非対応）で\
                指定し、model / effort はその CLI のネイティブ表記で書く\
                （codex 例: model=gpt-5.6-sol / effort=xhigh）。master_agent が claude 以外のとき\
                master の model / effort は claude worker へ継承されない。\
                worker のエージェント種別（claude / codex / agy）は worker_agent（既定種別）と\
                agent_* 系（worker_agents.<agent> のエージェント別 worker 設定: モデル・effort・\
                許可スキップ・追加引数）で指定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "show", "set"],
                        "description": "操作種別（省略時は list）",
                    },
                    "name": { "type": "string", "description": "プロファイル名（set 時に必須。show 省略時は default）" },
                    "master_agent": {
                        "type": "string",
                        "enum": ["claude", "codex"],
                        "description": "master のエージェント種別（set 時。tako master / solo がこの CLI で起動する。agy は master 非対応）",
                    },
                    "clear_master_agent": { "type": "boolean", "description": "master_agent の指定を解除して claude 既定に戻す（set 時）" },
                    "model": { "type": "string", "description": "master のモデル（master_agent のネイティブ表記。set 時。省略で現状維持）" },
                    "clear_model": { "type": "boolean", "description": "master のモデル指定を解除して claude 既定に戻す（set 時）" },
                    "worker_model": { "type": "string", "description": "worker_model_policy=fixed 時の子 worker モデル（set 時）" },
                    "clear_worker_model": { "type": "boolean", "description": "子 worker のモデル指定を解除する（set 時）" },
                    "effort": { "type": "string", "description": "master の thinking effort（set 時。省略で現状維持）" },
                    "worker_effort": { "type": "string", "description": "子 worker の thinking effort（set 時）" },
                    "worker_agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "worker の既定エージェント種別（set 時。省略時の spawn はこの種別で起動する）",
                    },
                    "clear_worker_agent": { "type": "boolean", "description": "worker_agent の指定を解除して claude 既定に戻す（set 時）" },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "agent_* 系で編集する対象エージェント名（set 時。agent_* 指定に必須）",
                    },
                    "agent_model": { "type": "string", "description": "対象エージェントの worker 既定モデル（CLI ネイティブ表記。codex: gpt-5.6-terra 等 / agy: 'Gemini 3.5 Flash (High)' 等）" },
                    "clear_agent_model": { "type": "boolean", "description": "対象エージェントのモデル指定を解除する" },
                    "agent_effort": { "type": "string", "description": "対象エージェントの worker 既定 effort（claude: --effort / codex: model_reasoning_effort。agy は無視される）" },
                    "clear_agent_effort": { "type": "boolean", "description": "対象エージェントの effort 指定を解除する" },
                    "agent_skip_permissions": { "type": "boolean", "description": "対象エージェントの許可プロンプトをスキップして起動する（明示 opt-in。agy は既定でコマンド毎に許可が出るため自律 worker 運用ではほぼ必須）" },
                    "agent_args": {
                        "type": "array", "items": { "type": "string" },
                        "description": "対象エージェントの追加 CLI 引数（丸ごと置き換え。空配列でクリア）",
                    },
                    "worker_model_policy": { "type": "string", "enum": ["inherit", "delegate", "fixed"], "description": "worker のモデル選択ポリシー（inherit: master と同じ / delegate: master が都度選ぶ / fixed: worker_model 固定）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_layout",
            "description": "worker spawn のレイアウト設定を取得・変更する（config.yaml の spawn_layout）。\
                全パラメータ省略で現在値の取得、いずれか指定でその項目を更新して結果を返す。\
                policy=master-reserved（既定）は spawn 元（master）の取り分を維持し、\
                worker を右側の worker 領域内に配置する。legacy は従来の右等分割\
                （worker が増えるほど全ペインが横に圧縮される）。\
                master_ratio は master 側へ残す取り分（0.1〜0.9。既定 0.5 = 画面半分）。\
                algorithm は worker 領域内の配置: grid（1 体=全面 → 2 体=上下 → 3〜4 体=十字四分割）/ \
                spiral（縦横交互に半分ずつの渦巻き分割）。\
                worker close 時は領域内だけがリフローされ、master とユーザーが自分で開いた\
                ペインの矩形は変わらない。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "policy": {
                        "type": "string",
                        "enum": ["master-reserved", "legacy"],
                        "description": "配置ポリシー（省略で現状維持）",
                    },
                    "master_ratio": {
                        "type": "number",
                        "description": "master 側へ残す取り分 0.1〜0.9（省略で現状維持）",
                    },
                    "algorithm": {
                        "type": "string",
                        "enum": ["grid", "spiral"],
                        "description": "worker 領域内の配置アルゴリズム（省略で現状維持）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_spawn",
            "description": "プロジェクトの作業ディレクトリで子 worker を spawn する。\
                worker のエージェント CLI は claude（既定）/ codex / agy から選べる（agent パラメータ）。\
                呼び出し元ペインを右に分割して新ペインを作り、エージェントを起動してプロンプトを送信する。\
                worker の pane_id・tmux_session・spawned_by（spawn 元ペイン ID）・agent を返す。\
                tmux_session は pane ID が解決できない場合\
                （BG タブ移動・tako 再起動後）のフォールバックとして tako_read_pane / tako_send_input に渡せる。\
                worker_status / watch は pane_id だけで session を自動解決するため session_id は不要\
                （codex / agy は画面推定で判定される）。\
                起動からプロンプト送信まで 15〜20 秒かかる（これは想定内）。\
                pane または tab のいずれかを必ず指定すること。省略すると呼び出し元タブに出るため、\
                master が別タブにいる場合に意図しないタブに子が生える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml に登録済みであること）" },
                    "prompt": { "type": "string", "description": "worker に渡す初期プロンプト" },
                    "label": { "type": "string", "description": "ペインタイトルに付けるラベル（省略時は '<project>-worker'）" },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "worker のエージェント CLI（省略時はプロファイルの worker_agent → claude）",
                    },
                    "model": { "type": "string", "description": "worker のモデル（agent のネイティブ表記。省略時はプロファイル設定に従う）" },
                    "effort": { "type": "string", "description": "thinking / reasoning effort（claude・codex のみ。agy はモデル名に組込みのため無視。省略時はプロファイル設定に従う）" },
                    "pane": pane_schema("分割元ペイン ID（省略時は呼び出し元。このペインの右に子が生える）。\
                        pane と tab の両方を指定した場合は pane を優先する"),
                    "tab": { "type": "integer", "minimum": 0, "description": "子を出すタブ ID。\
                        指定するとそのタブのフォーカスペインを分割元にする。\
                        複数マスター運用時は tab で出力先タブを明示指定することを推奨" },
                },
                "required": ["project", "prompt"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_worker_status",
            "description": "子 worker の状態を確認する。status は busy（作業中）/ idle（入力待ち・完了）/ \
                error（API エラー・usage limit 等の異常で停止。#157）/ \
                gone（ペイン消滅かつ tmux session も消滅）/ unknown（agents 不可）。\
                error 時は応答の error オブジェクトに kind（api_error = 続行指示で復帰可 / \
                usage_limit = 解除時刻まで待つ / limit_dialog = モデル切替等のダイアログに応答）と \
                detail（検知した画面上の行）、recommended_action（resume / wait_reset / respond_dialog）が入る。\
                events 配列に直近の検知イベントが入る（#243）: \
                question = worker が質問中（idle 時のみ。画面末尾に ? 終端行・選択肢・Should I 等のパターン）/ \
                model_switched = 自動モデル切替が発生（from/to つき。limit reached, now using ... の検知）/ \
                context_high = ctx 使用率が 60% 超（percent つき。handoff やセーフティコミットの判断材料）。\
                session_id を省略しても pane→session の自動解決（pid 祖先辿り）で claude agents --json の \
                正確な status を取得する（status_source が agents-auto になる）。自動解決失敗時のみ \
                画面パターン推定にフォールバック（status_source が screen）。\
                codex / agy worker は agents API が無いため常に画面推定で判定される（claude / codex / agy \
                すべての入力欄・busy パターンに対応済み）。\
                tmux_session を渡すとペインが消えても tmux session が生きている限り \
                recent_output を取得でき、gone にならない。\
                退避（shelved）されたペインも追跡可能。recent_output はペインの最近 30 行の出力。\
                resolved_session_id に自動解決された session_id が入る。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer", "minimum": 0, "description": "worker のペイン ID（必須）" },
                    "session_id": { "type": "string", "description": "claude の session ID（あれば精度向上）" },
                    "tmux_session": {
                        "type": "string",
                        "description": "tmux session 名（pane 消滅時のフォールバック追跡。tako_orchestrator_spawn の返り値に含まれる）",
                    },
                },
                "required": ["pane_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_self",
            "description": "master / solo が自分自身の pane・tab・ctx%・session_id を取得する。\
                master は MCP 経由では自分のペイン ID を知る手段がなかったが（#123）、\
                このツールで自己特定できる。ctx_percent はコンテキスト使用率（0〜100）、\
                ctx_threshold は引き継ぎ閾値（config.yaml の ctx_threshold、既定 60）、\
                ctx_over_threshold は閾値超えフラグ。\
                handoff_exists は引き継ぎファイル（handoff/<profile>.md）の有無。\
                pane を省略すると caller の環境変数（TAKO_PANE_ID / TAKO_ORCHESTRATOR_ROLE）\
                から自動解決する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("自 pane ID（省略時は caller から自動解決）"),
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_handoff",
            "description": "master の引き継ぎを実行する。handoff ファイル（handoff/<profile>.md）を読み、\
                同プロファイルの新 master を spawn して引き継ぎプロンプトを注入する。\
                旧 master のペインは閉じない（ユーザー判断）。\
                handoff ファイルが無ければエラーを返す（master は事前にファイルを更新する必要がある）。\
                tab を省略すると呼び出し元と同タブに新 master を spawn する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("呼び出し元ペイン ID（省略時は caller から自動解決）"),
                    "tab": { "type": "integer", "minimum": 0, "description": "新 master を出すタブ ID（省略時は呼び出し元と同タブ）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_run",
            "description": "子 worker を spawn し、即座に run_id を返す（非同期。#121）。\
                MCP 呼び出しが中断されても worker は孤児化せず、run_id で追跡できる。\
                進捗確認は tako_orchestrator_run_status、結果回収は tako_orchestrator_run_result を使う。\
                worker のエージェント CLI は claude（既定）/ codex / agy から選べる（agent パラメータ）。\
                完了判定はバックグラウンドで OrchestratorWorkerStatus と同じロジックを繰り返す。\
                タイムアウト（既定 1800 秒）に達した場合は run_status が status=timeout を返す。\
                worker が API エラー等で停止した場合は status=worker_error + error オブジェクト。\
                sync=true を指定すると旧挙動（完了までブロッキング）に戻る（後方互換）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml に登録済み）" },
                    "prompt": { "type": "string", "description": "worker に渡すプロンプト" },
                    "label": { "type": "string", "description": "ペインタイトルのラベル（省略時は '<project>-worker'）" },
                    "agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "worker のエージェント CLI（省略時はプロファイルの worker_agent → claude）",
                    },
                    "model": { "type": "string", "description": "worker のモデル（agent のネイティブ表記。省略時はマスターのプロファイル設定に従う）" },
                    "effort": { "type": "string", "description": "thinking / reasoning effort（claude・codex のみ。省略時はマスターのプロファイル設定に従う）" },
                    "pane": pane_schema("分割元ペイン ID（省略時は呼び出し元）"),
                    "tab": { "type": "integer", "minimum": 0, "description": "子を出すタブ ID" },
                    "timeout_seconds": {
                        "type": "integer", "minimum": 10, "default": 1800,
                        "description": "完了待ちタイムアウト秒数（省略時 1800 = 30 分）",
                    },
                    "auto_close": {
                        "type": "boolean", "default": true,
                        "description": "完了後にペインを自動 close するか（省略時 true）",
                    },
                    "output_lines": {
                        "type": "integer", "minimum": 1, "default": 200,
                        "description": "返す出力の末尾行数（省略時 200）",
                    },
                    "sync": {
                        "type": "boolean", "default": false,
                        "description": "true にすると完了までブロッキングする旧挙動（後方互換。既定 false = 非同期）",
                    },
                },
                "required": ["project", "prompt"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_run_status",
            "description": "非同期 run の進捗を照会する（#121）。run_id を指定すると \
                {run_id, pane_id, status, phase, elapsed_seconds} を返す。\
                phase は 'running'（進行中）または 'finished'（完了済み）。\
                status は busy / idle / error / gone / starting / completed / worker_error / timeout。\
                run_id を省略すると全 run の一覧を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "照会する run_id（省略時は全 run 一覧）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_run_result",
            "description": "完了した非同期 run の結果を回収する（#121）。\
                未完了なら phase='running' を返す（エラーにはならない）。\
                完了済みなら出力取得 + auto_close を行い、レジストリから除去して \
                {run_id, pane_id, status, output, duration_seconds, closed} を返す。\
                run ごとに 1 回だけ呼べる（2 回目は run_id が見つからないエラー）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string", "description": "回収する run_id" },
                },
                "required": ["run_id"],
                "additionalProperties": false,
            },
        }),
        // --- リモートアクセス MCP ツール ---
        json!({
            "name": "tako_remote_start",
            "description": "リモートアクセス API サーバーを起動する。スマホからブラウザ経由で\
                ペインを操作するための HTTP API サーバーが指定ポート（既定 7749）で開始される。\
                既定では cloudflared による暗号化トンネル経由でのみ公開し、トンネルを張れない\
                場合（cloudflared 不在等）は安全に提供できないため起動を拒否する。\
                起動後は接続用の QR コードが表示される。\
                注意: 接続したリモートはターミナルへ任意コマンドを送信できる（実質シェルアクセス）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "port": {
                        "type": "integer", "minimum": 1, "maximum": 65535,
                        "description": "サーバーのポート番号（省略時は 7749）",
                    },
                    "insecure": {
                        "type": "boolean",
                        "description": "true にすると暗号化トンネルを使わず平文 HTTP の LAN 直モードで\
                            起動する（明示 opt-in・非推奨。同一 LAN 上の第三者に通信を盗聴されうる）。\
                            既定 false = 暗号化トンネル必須",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_stop",
            "description": "リモートアクセス API サーバーを停止する。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_status",
            "description": "リモートアクセス API サーバーの状態を取得する。\
                起動中なら running=true とポート番号・接続 URL を返す。\
                トークンは既定でマスク（***）される。生値が必要なら show_token=true を指定する\
                （スクリーンショット・画面共有経由の漏えい防止のため既定はマスク）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "show_token": {
                        "type": "boolean",
                        "description": "true でトークンをマスクせず生値で返す（既定 false = マスク）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_agents",
            "description": "動作中の Claude Code エージェント一覧を取得する\
                （claude agents --json のプロキシ）。各エージェントの session_id / status / \
                ctx_percent / model / name / cwd に加え、tmux バックエンドのどのペインで\
                動いているか（pane）をプロセス祖先の突き合わせで対応付けて返す。\
                スマホリモートのエージェント監視やセッション ID の特定に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_messages",
            "description": "Claude Code セッションの会話ログ（transcript）の末尾を\
                正規化 JSON で取得する。user / assistant メッセージ・ツール使用サマリ・\
                thinking（折りたたみ用に分離）を返す。session_id は tako_remote_agents で\
                確認できる。エージェントの進捗確認や会話の振り返りに使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "対象セッション ID（必須。claude の sessionId）" },
                    "tail": { "type": "integer", "minimum": 1, "default": 30, "description": "取得する末尾件数（省略時は 30）" },
                },
                "required": ["session_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_remote_scrollback",
            "description": "ペインのスクロールバック履歴をプレーンテキストで取得する。\
                tmux capture-pane で指定行数の履歴を取得し、ANSI なしのテキストとして返す。\
                リモートからの画面履歴確認やログ検索に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane_id": { "type": "string", "description": "対象ペイン ID（必須。session:window.pane）" },
                    "lines": { "type": "integer", "minimum": 1, "default": 1000, "description": "取得する履歴行数（省略時は 1000）" },
                },
                "required": ["pane_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_web",
            "description": "ネイティブ Web ビューペインの操作（FR-3.8）。macOS の WKWebView を \
                ペインとして表示し、ユーザーはクリック・スクロール・文字入力を直接行える。\
                dev サーバーのプレビュー表示・ドキュメント提示・成果物の URL 提示に使う。\
                ペインから外しても（hide）ページは dock に生きたまま維持され、show で呼び戻せる。\
                action: open = url を新規ペインで開く / list = 一覧（id・URL・タイトル・表示中ペイン）/ \
                show = dock から id をペインへ呼び出す / hide = ペインから外して dock へ退避 / \
                close = 完全破棄 / navigate = to（back・forward・reload・URL）でページ遷移 / \
                eval = js を非同期評価して token を返す / eval_result = token の結果回収 \
                （eval 発行後 200ms 程度おいて呼ぶ。pending: true なら再試行）/ \
                read = URL・タイトル・読み込み状態の取得。\
                ページ内の操作（クリック・入力・スクロール・テキスト取得）は eval の JS で行う \
                （例: document.querySelector('button').click()）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["open", "list", "show", "hide", "close", "navigate", "eval", "eval_result", "read"],
                        "description": "実行する操作（必須）",
                    },
                    "url": { "type": "string", "description": "open: 開く URL（必須）" },
                    "id": { "type": "integer", "description": "対象 Web ビュー ID（list で確認。show では必須）" },
                    "pane": pane_schema("open / show: 分割の基準ペイン ID（省略時は呼び出し元）。その他: 対象 Web ビューが表示中のペイン ID"),
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "open / show: 分割方向（省略時は右）",
                    },
                    "to": { "type": "string", "description": "navigate: back / forward / reload / URL（必須）" },
                    "js": { "type": "string", "description": "eval: 実行する JavaScript（必須）" },
                    "token": { "type": "integer", "description": "eval_result: eval が返した token（必須）" },
                    "focus": { "type": "boolean", "description": "open / show: true にすると新ペインにフォーカスを移す（省略時は false = 元ペインを維持）" },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_update",
            "description": "アプリ内更新の診断・チェック・実行（Issue #36 + #50）。\
                action=status で配布系統（homebrew / zip / broken-brew）・現在バージョン・\
                PATH 上の重複 CLI を返す。broken-brew 検知時は診断情報と修復コマンドも含む。\
                action=check で GitHub Releases から最新版の有無を確認する（更新は行わない）。\
                action=apply で配布系統に応じた更新を実行する \
                （homebrew → brew upgrade --cask、zip/broken-brew → zip DL で .app 差し替え）。\
                action=apply-zip で配布系統を問わず zip 経由で強制更新する \
                （brew upgrade 失敗時のフォールバック）。\
                action=repair で broken-brew 状態を修復する \
                （brew install --cask --force で cask 台帳を再締結）。\
                apply 成功後の再起動は UI 側で行う（CLI / MCP からは apply 結果の確認まで）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "check", "apply", "apply-zip", "repair"],
                        "description": "操作種別（省略時は status）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_fda",
            "description": "macOS のフルディスクアクセス (FDA) の状態確認と設定画面の起動（Issue #118）。\
                フォルダアクセス許可ダイアログが頻発する場合、FDA を付与すれば一括で消せる。\
                action=status で FDA の付与状態を返す（granted: true/false）。\
                action=open でシステム設定のフルディスクアクセスパネルを開く。\
                ユーザーが「フォルダの許可が何度も出る」と言った場合は、\
                まず status で確認し、未付与なら open で設定画面を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "open"],
                        "description": "操作種別（省略時は status）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_sleep_guard",
            "description": "スリープ防止機能の状態確認・設定変更（Issue #173 + #218 蓋閉じ対応）。\
                macOS のアイドルスリープを IOKit 電源アサーションで防止する。\
                蓋閉じ防止は pmset disablesleep で制御（sudoers 登録が必要）。\
                action=status（既定）: モード・電源条件・アサーション状態・蓋の開閉・thermal 状態を返す。\
                action=set: mode / power_condition / lid_sleep_mode を設定する。\
                action=install-lid-sleep: sudoers.d に pmset NOPASSWD を登録（管理者パスワード必要、初回のみ）。\
                action=remove-lid-sleep: sudoers.d から削除 + disablesleep 解除。\
                action=open-battery-settings: System Settings の Battery を開く（フォールバック）。\
                ユーザーが「PC がスリープして作業が止まった」「蓋を閉じても続けたい」と言った場合は、\
                まず status で確認し、蓋閉じ防止なら install-lid-sleep で登録を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set", "install-lid-sleep", "remove-lid-sleep", "open-battery-settings"],
                        "description": "操作種別（省略時は status）",
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["off", "on", "while-agents-running"],
                        "description": "アイドルスリープ防止モード（set 時のみ有効）",
                    },
                    "power_condition": {
                        "type": "string",
                        "enum": ["ac-only", "always"],
                        "description": "電源条件（set 時のみ有効）",
                    },
                    "lid_sleep_mode": {
                        "type": "string",
                        "enum": ["off", "while-agents-running"],
                        "description": "蓋閉じ防止モード（set 時のみ有効。要 sudoers 登録）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_theme",
            "description": "UI テーマ（ライト/ダーク）の状態確認・切替（Issue #217）。\
                action=status（既定）: 現在のテーマ（dark / light）を返す。\
                action=set: mode で指定したテーマへ切り替える。\
                action=toggle: 現在のテーマを反転する。\
                変更は settings.json に永続化され、GUI に即時反映される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "set", "toggle"],
                        "description": "操作種別（省略時は status）",
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["dark", "light"],
                        "description": "テーマモード（set 時に必須）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup",
            "description": "tako setup を非対話で実行する（Issue #262）。ユーザーが日本語で伝えた好みを answers に変換して代行する。省略項目は detected → previous → default の順で自動解決され、標準ケースは質問ゼロで完走する。instructions / profile / projects / orchestrator / sleep_guard は明示指定時だけ既存値を更新する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selected_agent": {
                        "type": "string",
                        "enum": ["claude", "codex", "agy"],
                        "description": "setup の既定 agent。省略時は検出・前回値から自動決定",
                    },
                    "provider_plans": {
                        "type": "object",
                        "description": "プロバイダ別プラン。キーは claude / gpt / google",
                        "additionalProperties": {"type": "string"},
                    },
                    "instruction_content": {
                        "type": "string",
                        "description": "選択 agent のグローバル指示ファイルへ書く完全な Markdown。省略時は既存維持",
                    },
                    "profile": {
                        "type": "object",
                        "description": "profiles/default.yaml の完全な設定。省略時は既存維持または推奨生成",
                        "additionalProperties": true,
                    },
                    "projects": {
                        "type": "object",
                        "description": "projects.yaml の全登録。明示時だけ既存一覧を置換",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "cwd": {"type": "string"},
                                "description": {"type": "string"},
                            },
                            "required": ["cwd"],
                            "additionalProperties": false,
                        },
                    },
                    "orchestrator": {
                        "type": "object",
                        "properties": {
                            "auto_close": {"type": "boolean"},
                            "auto_push": {"type": "boolean"},
                        },
                        "additionalProperties": false,
                    },
                    "sleep_guard": {
                        "type": "object",
                        "properties": {
                            "mode": {"type": "string", "enum": ["off", "on", "while-agents-running"]},
                            "power": {"type": "string", "enum": ["ac-only", "always"]},
                        },
                        "additionalProperties": false,
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_setup_changes",
            "description": "tako setup のアップデート追従状況を照会する（Issue #94）。\
                前回 `tako setup` 完了時に適用したリビジョン（applied_revision）と\
                バイナリ同梱の setup changelog の現在リビジョンを突き合わせ、\
                未適用の setup 関連変更（セットアップ項目・設定フォーマット・\
                master 用システムプロンプト等の変更）の一覧を返す。読み取り専用。\
                pending の各エントリの kind が auto なら `tako setup` の再実行だけで追従が\
                完了する。guided ならユーザー所有ファイル（CLAUDE.md・profiles 等）に関わる\
                ため、`tako setup --review` で個別確認する。自動追従は `tako setup` を案内すること。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_agents_sync_rules",
            "description": "エージェント共通ルールの同期（#136）。\
                正本ファイルの内容を各エージェント（claude / codex / agy）のグローバル指示ファイルに\
                マーカーブロックで埋め込む。ブロック外の既存内容は一切変更しない。\
                action=sync（既定）: 同期を実行し結果を返す。書き換え前にバックアップ(.bak)を生成する。\
                action=status: 設定と現在の同期状態を返す（読み取り専用）。\
                正本パスは tako setup で設定済みの値を使うが、source で一時的に上書きできる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["sync", "status"],
                        "description": "操作種別（省略時は sync）",
                    },
                    "source": {
                        "type": "string",
                        "description": "正本ファイルの絶対パス（省略時は config.yaml の設定値）",
                    },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["claude", "codex", "agy"] },
                        "description": "同期対象エージェント（省略時は設定値 or 全対象）",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_tree_folder",
            "description": "ファイルツリーへのフォルダの追加・削除・一覧（#134）。\
                AI が作業対象プロジェクトのフォルダをファイルツリーに明示追加する。\
                追加されたフォルダは cwd 由来のエントリと並んでツリーに表示される。\
                プロジェクトの指示を受けたらそのルートフォルダを追加し、\
                作業対象外になったら削除する。タブ単位スコープ（永続化される）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "remove", "list"],
                        "description": "add: フォルダを追加, remove: フォルダを削除, list: 追加済み一覧"
                    },
                    "path": {
                        "type": "string",
                        "description": "追加・削除するフォルダの絶対パス（list 時は省略可）"
                    },
                    "tab": {
                        "type": "integer",
                        "description": "対象タブ ID（省略時は呼び出し元ペインのタブ）"
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_sessions",
            "description": "セッションカタログの参照と会話の復元（Issue #112）。\
                tako が起動した master / worker / solo / 手動 claude の会話セッションを、\
                ラベル・ロール・プロジェクト・Issue 番号つきで発見できるインデックス。\
                会話本文は claude の transcript（~/.claude/projects/）への参照のみ持つ。\
                action=list: 一覧（role / project で絞り込み、last_seen の新しい順に limit 件）。\
                action=show: id（前方一致可）のメタ情報 + 会話冒頭の抜粋。\
                action=resume: ペイン / タブ / tmux が全滅していても、記録された cwd で\
                新しいペインを分割起動し `claude --resume <session_id>` で会話文脈ごと復元する。\
                「昨日の #159 の子を呼び戻して」のような依頼は list で特定 → resume で復元する。\
                制限: resume は claude セッションのみ（codex / agy は list に載るが復元不可）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "show", "resume"],
                        "description": "操作種別",
                    },
                    "id": {
                        "type": "string",
                        "description": "session_id（前方一致可。show / resume で必須）",
                    },
                    "role": {
                        "type": "string",
                        "enum": ["master", "worker", "solo", "pane"],
                        "description": "list の種別絞り込み",
                    },
                    "project": {
                        "type": "string",
                        "description": "list のプロジェクト絞り込み",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "list の最大件数（既定 30）",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "resume の分割元ペイン ID（省略時は呼び出し元）",
                    },
                    "tab": {
                        "type": "integer",
                        "description": "resume の分割先タブ ID（そのタブのフォーカスペインの隣）",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "resume の分割方向（省略時 right）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_logs",
            "description": "ペインの平文ターミナルログの参照・設定（Issue #112）。\
                全ペインのスクロールバック確定行を平文でローテーション保存しており、\
                ペイン / タブ / アプリが死んだ後でもビルド・テスト出力を遡れる。\
                TUI（claude 等）の描画は保存されない（「TUI 実行中」マーカーのみ。\
                会話の復元は tako_sessions を使う）。\
                action=list: ログファイル一覧。action=read: 末尾 lines 行（既定 200）を返す。\
                対象は pane（クローズ済み可）か session_id（カタログ経由）。\
                action=status: 有効/無効・上限・保存先。action=set: enabled / max_mb / \
                total_max_mb の変更（永続化）。ログはユーザーローカル保存で、\
                トークン等が写り込み得るため内容を外部へ送らないこと。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "read", "status", "set"],
                        "description": "操作種別",
                    },
                    "pane": {
                        "type": "integer",
                        "description": "read 対象のペイン ID（クローズ済みでも可）",
                    },
                    "session_id": {
                        "type": "string",
                        "description": "read 対象のセッション ID（カタログ経由で端末ログを引く）",
                    },
                    "lines": {
                        "type": "integer",
                        "description": "read の表示行数（既定 200）",
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "set: ログ保存の ON/OFF",
                    },
                    "max_mb": {
                        "type": "integer",
                        "description": "set: ペインあたりの上限（MB）",
                    },
                    "total_max_mb": {
                        "type": "integer",
                        "description": "set: ログ全体の上限（MB）",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_open_dir",
            "description": "ディレクトリを新タブで開く（#20）。cwd を設定してシェルを起動し、\
                ファイルツリーにフォルダを自動追加する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "開くディレクトリの絶対パス",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "新タブにフォーカスを移すか（省略時 true）",
                    },
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_open_remote",
            "description": "SSH ホストに接続する新タブを開く（#20）。~/.ssh/config の Host 名を\
                指定すると、HostName / User / Port 等の設定を尊重して ssh コマンドを実行する。\
                未定義ホストでも ssh <host> として実行できる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": {
                        "type": "string",
                        "description": "SSH ホスト名（~/.ssh/config の Host、または直接 hostname）",
                    },
                    "focus": {
                        "type": "boolean",
                        "description": "新タブにフォーカスを移すか（省略時 true）",
                    },
                },
                "required": ["host"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_ssh_hosts",
            "description": "~/.ssh/config の Host 一覧を返す（#20）。ワイルドカード（*）を含む\
                エントリは除外される。各ホストの name / hostname / user / port を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_recent",
            "description": "最近開いたディレクトリ/リポジトリ/SSH ホストの一覧・クリア（#20）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "clear"],
                        "description": "操作種別",
                    },
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_checkpoint",
            "description": "タスクチェックポイントの記録・更新（Issue #242）。\
                worker タスクの進行状態（Issue 番号・ブランチ・フェーズ・直近コミット等）を \
                永続化し、クラッシュ・利用上限・API 切断からの resume を可能にする。\
                task_id を省略すると自動採番される。既存の task_id を指定すると上書き更新する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "タスク ID（省略時は自動採番 task-N）" },
                    "pane": { "type": "integer", "description": "ペイン ID" },
                    "issue": { "type": "integer", "description": "GitHub Issue 番号" },
                    "branch": { "type": "string", "description": "作業ブランチ名" },
                    "phase": { "type": "string", "enum": ["queued", "running", "verifying", "done", "failed", "suspended"], "description": "フェーズ（省略時 running）" },
                    "last_commit": { "type": "string", "description": "直近の git commit SHA" },
                    "agent": { "type": "string", "description": "エージェント種別（claude / codex / agy）" },
                    "model": { "type": "string", "description": "モデル名" },
                    "prompt_head": { "type": "string", "description": "コンテキスト復元用のプロンプト冒頭" },
                    "suspended_reason": { "type": "string", "description": "一時停止の理由（usage_limit / api_error / crash 等）" },
                    "project": { "type": "string", "description": "プロジェクト名（projects.yaml のキー）" },
                    "cwd": { "type": "string", "description": "作業ディレクトリ" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_list",
            "description": "タスクチェックポイントの一覧（Issue #242）。\
                永続化された全チェックポイントを updated_at の新しい順に返す。\
                phase で絞り込み可能（例: suspended で中断中のタスクだけ表示）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "phase": { "type": "string", "enum": ["queued", "running", "verifying", "done", "failed", "suspended"], "description": "フェーズで絞り込む（省略時は全件）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_resume",
            "description": "チェックポイントから worker を再開する（Issue #242）。\
                指定した task_id のチェックポイントを読み、元の branch / cwd / issue コンテキストを \
                resume プロンプトに含めて新しいペインに worker を spawn する。\
                モデルを変更して再開することも可能（usage_limit 後に別モデルへ切り替え等）。\
                再開後、チェックポイントの phase は running に遷移し、pane_id が新ペインに更新される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "再開するチェックポイントの task_id" },
                    "model": { "type": "string", "description": "モデルを変更して再開する（省略時はチェックポイントのモデル）" },
                    "pane": { "type": "integer", "description": "分割元ペイン ID（省略時は呼び出し元）" },
                    "tab": { "type": "integer", "description": "分割先タブ ID" },
                },
                "required": ["task_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_gate",
            "description": "受け入れゲートの定義（Issue #244）。\
                タスクに機械検証可能な受け入れ条件（述語）を設定する。\
                Command 述語はシェルコマンドの exit code、PrMerged は PR のマージ状態、\
                Custom は人間判断で判定する。\
                設定後は tako_task_gate_check で述語を実行し、結果を記録する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "対象のタスク ID（checkpoint の task_id と同じ）" },
                    "criteria": {
                        "type": "array",
                        "description": "受け入れ条件の配列",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "条件 ID（例: tests_green, pr_merged）" },
                                "kind": {
                                    "type": "object",
                                    "description": "条件の種別。type=command: {cmd, expect_exit_0?}、type=pr_merged: {pr_number, repo?}、type=custom: {description}",
                                },
                            },
                            "required": ["id", "kind"],
                        },
                    },
                    "cwd": { "type": "string", "description": "Command 述語の実行ディレクトリ（省略時は worker の cwd）" },
                },
                "required": ["task_id", "criteria"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_gate_check",
            "description": "受け入れゲートの述語を実行し、結果を記録する（Issue #244）。\
                Command 述語はシェルコマンドを実行し exit code で判定、\
                PrMerged 述語は gh pr view で PR のマージ状態を判定する。\
                Custom 述語はスキップされる（手動で tako_task_gate の record_results で設定）。\
                sync_checkpoint=true のとき、全 Passed で checkpoint.phase が done に遷移する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "対象のタスク ID" },
                    "sync_checkpoint": { "type": "boolean", "description": "true のとき、全 Passed で checkpoint.phase を done に遷移させる（既定 true）" },
                },
                "required": ["task_id"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_task_gate_show",
            "description": "受け入れゲートの状態を表示する（Issue #244）。\
                各 criterion の id / kind / status / evidence / checked_at と、\
                overall（pending / passed / failed）を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "対象のタスク ID" },
                },
                "required": ["task_id"],
                "additionalProperties": false,
            },
        }),
    ]
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
    if name == "tako_task_gate_check" {
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
    if name == "tako_orchestrator_run" {
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

/// ツール呼び出しを操作プロトコル（[`Request`]）へ写す。エラーは引数バリデーション失敗
fn build_request(
    name: &str,
    args: &Value,
    caller: Option<u64>,
    caller_role: Option<&str>,
) -> Result<Request, String> {
    Ok(match name {
        "tako_list_panes" => Request::List,
        "tako_split_pane" => {
            let tab = u64_arg(args, "tab")?;
            Request::Split {
                // tab 指定時は pane を使わない（タブのフォーカスペインを dispatch が解決）
                pane: if tab.is_some() {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                tab,
                direction: direction_arg(args)?,
                ratio: f32_arg(args, "ratio")?,
                command: str_vec_arg(args, "command")?.filter(|c| !c.is_empty()),
                cwd: str_arg(args, "cwd")?,
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_send_input" => Request::Send {
            pane: Some(required_u64(args, "pane")?),
            text: str_arg(args, "text")?.ok_or("text を指定する")?,
            newline: bool_arg(args, "newline")?.unwrap_or(true),
            tmux_session: str_arg(args, "tmux_session")?,
            await_prompt: bool_arg(args, "await_prompt")?.unwrap_or(false),
        },
        "tako_read_pane" => Request::Read {
            pane: Some(required_u64(args, "pane")?),
            lines: u64_arg(args, "lines")?.map(|n| n as usize),
            tmux_session: str_arg(args, "tmux_session")?,
        },
        "tako_tmux_list" => Request::TmuxList {
            socket: str_arg(args, "socket")?,
        },
        "tako_tmux_cleanup" => Request::TmuxCleanup {
            socket: str_arg(args, "socket")?,
        },
        "tako_tmux_kill" => Request::TmuxKill {
            socket: str_arg(args, "socket")?,
            session: str_arg(args, "session")?.ok_or("session を指定する")?,
            window: u64_arg(args, "window")?.map(|n| n as u32),
        },
        "tako_tmux_resize" => Request::TmuxResize {
            socket: str_arg(args, "socket")?,
            session: str_arg(args, "session")?.ok_or("session を指定する")?,
            window: u64_arg(args, "window")?.map(|n| n as u32).unwrap_or(0),
            cols: u64_arg(args, "cols")?.map(|n| n as u32),
            rows: u64_arg(args, "rows")?.map(|n| n as u32),
            reset: bool_arg(args, "reset")?.unwrap_or(false),
        },
        "tako_tmux_select_window" => Request::TmuxSelectWindow {
            pane: Some(target_pane(args, caller)?),
            window: u64_arg(args, "window")?.ok_or("window を指定する")? as u32,
        },
        "tako_tmux_open" => Request::TmuxOpen {
            socket: str_arg(args, "socket")?,
            session: str_arg(args, "session")?.ok_or("session を指定する")?,
            window: u64_arg(args, "window")?.map(|n| n as u32),
            pane: Some(target_pane(args, caller)?),
            direction: direction_arg(args)?,
        },
        "tako_scroll_pane" => Request::Scroll {
            pane: Some(target_pane(args, caller)?),
            to: u64_arg(args, "to")?,
            delta: i64_arg(args, "delta")?.map(|n| n as i32),
        },
        "tako_focus_pane" => {
            let pane = u64_arg(args, "pane")?;
            let direction = direction_arg(args)?;
            if pane.is_none() && direction.is_none() {
                return Err("pane か direction のどちらか一方を指定する".into());
            }
            Request::Focus { pane, direction }
        }
        "tako_close_pane" => Request::Close {
            pane: Some(target_pane(args, caller)?),
            force: bool_arg(args, "force")?.unwrap_or(false),
        },
        "tako_resize_pane" => Request::Resize {
            pane: Some(target_pane(args, caller)?),
            axis: match str_arg(args, "axis")?.as_deref() {
                Some("x") => Axis::X,
                Some("y") => Axis::Y,
                _ => return Err("axis は \"x\" か \"y\" を指定する".into()),
            },
            delta: f32_arg(args, "delta")?,
            share: f32_arg(args, "share")?,
        },
        "tako_equalize_layout" => {
            let tab = u64_arg(args, "tab")?;
            Request::Equalize {
                // tab 省略時は呼び出し元ペインからタブを解決する
                pane: if tab.is_none() {
                    Some(target_pane(args, caller)?)
                } else {
                    None
                },
                tab,
            }
        }
        "tako_set_title" => Request::Title {
            pane: Some(target_pane(args, caller)?),
            title: str_arg(args, "title")?,
            role: str_arg(args, "role")?,
        },
        "tako_rename_tab" => {
            let tab = u64_arg(args, "tab")?;
            Request::TabRename {
                // tab 省略時は呼び出し元ペインからタブを解決する（Equalize と同パターン）
                pane: if tab.is_none() {
                    Some(target_pane(args, caller)?)
                } else {
                    None
                },
                tab,
                title: str_arg(args, "title")?.ok_or("title を指定する")?,
            }
        }
        "tako_create_tab" => Request::TabNew {
            title: str_arg(args, "title")?,
            focus: bool_arg(args, "focus")?,
        },
        "tako_select_tab" => Request::TabSelect {
            tab: required_u64(args, "tab")?,
        },
        "tako_move_pane_to_tab" => {
            let new_tab = bool_arg(args, "new_tab")?.unwrap_or(false);
            Request::MovePane {
                pane: Some(target_pane(args, caller)?),
                tab: if new_tab { None } else { u64_arg(args, "tab")? },
                target: if new_tab {
                    None
                } else {
                    u64_arg(args, "target")?
                },
                direction: if new_tab { None } else { direction_arg(args)? },
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_auto_rename" => Request::AutoRename {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_port_detect" => Request::PortDetect {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_persist" => Request::Persist {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_confirm_close" => Request::ConfirmClose {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_open_file" => Request::OpenFile {
            pane: Some(target_pane(args, caller)?),
            path: str_arg(args, "path")?.ok_or("path を指定する")?,
            mode: match str_arg(args, "mode")?.as_deref() {
                None => None,
                Some("code") => Some(crate::protocol::PreviewModeWire::Code),
                Some("markdown") => Some(crate::protocol::PreviewModeWire::Markdown),
                Some(other) => return Err(format!("mode が不正: {other}（code | markdown）")),
            },
            direction: direction_arg(args)?,
            focus: bool_arg(args, "focus")?,
        },
        "tako_preview_view" => Request::PreviewView {
            pane: Some(target_pane(args, caller)?),
            zoom: f32_arg(args, "zoom")?,
            zoom_in: bool_arg(args, "zoom_in")?.unwrap_or(false),
            zoom_out: bool_arg(args, "zoom_out")?.unwrap_or(false),
            reset: bool_arg(args, "reset")?.unwrap_or(false),
            page: u64_arg(args, "page")?.map(|page| page as usize),
            pan_x: f32_arg(args, "pan_x")?,
            pan_y: f32_arg(args, "pan_y")?,
        },
        "tako_preview_outline" => Request::PreviewOutline {
            pane: Some(target_pane(args, caller)?),
            item: u64_arg(args, "item")?.map(|item| item as usize),
        },
        "tako_preview_link_list" => Request::PreviewLinkList {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_follow_link" => Request::PreviewFollowLink {
            pane: Some(target_pane(args, caller)?),
            index: u64_arg(args, "index")?.ok_or("index を指定する")? as usize,
        },
        "tako_preview_reload" => Request::PreviewReload {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_preview_cache" => Request::PreviewCache {
            max_mb: u64_arg(args, "max_mb")?,
        },
        "tako_preview_edit" => Request::PreviewEdit {
            pane: Some(target_pane(args, caller)?),
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_preview_apply" => Request::PreviewApply {
            pane: Some(target_pane(args, caller)?),
            text: str_arg(args, "text")?.ok_or("text を指定する")?,
        },
        "tako_preview_save" => Request::PreviewSave {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_undo" => Request::PreviewUndo {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_redo" => Request::PreviewRedo {
            pane: Some(target_pane(args, caller)?),
        },
        "tako_preview_search" => Request::PreviewSearch {
            pane: Some(target_pane(args, caller)?),
            query: str_arg(args, "query")?,
            direction: str_arg(args, "direction")?,
        },
        "tako_preview_replace" => Request::PreviewReplace {
            pane: Some(target_pane(args, caller)?),
            query: str_arg(args, "query")?.ok_or("query を指定する")?,
            replacement: str_arg(args, "replacement")?.ok_or("replacement を指定する")?,
            all: bool_arg(args, "all")?,
        },
        "tako_preview_autosave" => Request::PreviewAutosave {
            pane: Some(target_pane(args, caller)?),
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_file_op" => {
            let op_str = str_arg(args, "op")?.ok_or("op を指定する")?;
            let op = match op_str.as_str() {
                "copy_absolute_path" => crate::protocol::FileOpKind::CopyAbsolutePath,
                "copy_relative_path" => crate::protocol::FileOpKind::CopyRelativePath,
                "reveal" => crate::protocol::FileOpKind::Reveal,
                "open_terminal" => crate::protocol::FileOpKind::OpenTerminal,
                "rename" => crate::protocol::FileOpKind::Rename,
                "create_file" => crate::protocol::FileOpKind::CreateFile,
                "create_dir" => crate::protocol::FileOpKind::CreateDir,
                "trash" => crate::protocol::FileOpKind::Trash,
                other => return Err(format!("op が不正: {other}")),
            };
            Request::FileOp {
                op,
                path: str_arg(args, "path")?.ok_or("path を指定する")?,
                name: str_arg(args, "name")?,
                pane: match op {
                    crate::protocol::FileOpKind::OpenTerminal
                    | crate::protocol::FileOpKind::CopyRelativePath => {
                        Some(target_pane(args, caller)?)
                    }
                    _ => None,
                },
            }
        }
        "tako_git_log" => Request::GitLog {
            pane: Some(target_pane(args, caller)?),
            max_count: u64_arg(args, "max_count")?.map(|n| n as usize),
        },
        "tako_git_diff" => Request::GitDiff {
            pane: Some(target_pane(args, caller)?),
            target: str_arg(args, "target")?,
        },
        "tako_background_pane" => {
            let tab = u64_arg(args, "tab")?;
            Request::Background {
                pane: if tab.is_some() {
                    None
                } else {
                    Some(target_pane(args, caller)?)
                },
                tab,
            }
        }
        "tako_foreground_pane" => Request::Foreground {
            pane: required_u64(args, "pane")?,
            target: u64_arg(args, "target")?,
            direction: direction_arg(args)?,
        },
        "tako_background_list" => Request::BackgroundList,
        "tako_background_kill" => Request::BackgroundKill {
            pane: required_u64(args, "pane")?,
        },
        "tako_panel" => Request::Panel {
            visible: bool_arg(args, "visible")?,
            width: f32_arg(args, "width")?,
            view: match str_arg(args, "view")?.as_deref() {
                None => None,
                Some("tmux") => Some(crate::protocol::PanelViewWire::Tmux),
                Some("orch") => Some(crate::protocol::PanelViewWire::Orch),
                Some("git") => Some(crate::protocol::PanelViewWire::Git),
                Some(other) => return Err(format!("view が不正: {other}（tmux | orch | git）")),
            },
            filetree: bool_arg(args, "filetree")?,
        },
        "tako_collapse_tab" => Request::CollapseTab {
            pane: u64_arg(args, "pane")?.or(caller),
            tab: u64_arg(args, "tab")?,
            collapsed: bool_arg(args, "collapsed")?,
        },
        "tako_pin_preview" => {
            let group_tab = u64_arg(args, "group_tab")?;
            Request::Pin {
                // group_tab 指定時は pane を補完しない（排他）
                pane: if group_tab.is_some() {
                    None
                } else {
                    u64_arg(args, "pane")?.or(caller)
                },
                group_tab,
                pinned: bool_arg(args, "pinned")?,
            }
        }
        "tako_check_health" => Request::CheckHealth,
        "tako_setup_mcp" => Request::SetupMcp {
            scope: str_arg(args, "scope")?,
            pane: u64_arg(args, "pane")?.or(caller),
        },
        "tako_video_playback" => Request::VideoPlayback {
            pane: Some(target_pane(args, caller)?),
            action: str_arg(args, "action")?.ok_or("action を指定する")?,
        },
        "tako_video_seek" => Request::VideoSeek {
            pane: Some(target_pane(args, caller)?),
            seconds: f64_arg(args, "seconds")?.ok_or("seconds を指定する")?,
        },
        "tako_video_volume" => Request::VideoVolume {
            pane: Some(target_pane(args, caller)?),
            volume: f64_arg(args, "volume")?.ok_or("volume を指定する")?,
        },
        "tako_orchestrator_projects" => Request::OrchestratorProjects {
            action: str_arg(args, "action")?.unwrap_or_else(|| "list".into()),
            key: str_arg(args, "key")?,
            cwd: str_arg(args, "cwd")?,
            description: str_arg(args, "description")?,
        },
        "tako_orchestrator_profiles" => Request::OrchestratorProfiles {
            action: str_arg(args, "action")?.unwrap_or_else(|| "list".into()),
            name: str_arg(args, "name")?,
            model: str_arg(args, "model")?,
            master_agent: str_arg(args, "master_agent")?,
            clear_master_agent: bool_arg(args, "clear_master_agent")?.unwrap_or(false),
            worker_model: str_arg(args, "worker_model")?,
            effort: str_arg(args, "effort")?,
            worker_effort: str_arg(args, "worker_effort")?,
            clear_model: bool_arg(args, "clear_model")?.unwrap_or(false),
            clear_worker_model: bool_arg(args, "clear_worker_model")?.unwrap_or(false),
            worker_agent: str_arg(args, "worker_agent")?,
            clear_worker_agent: bool_arg(args, "clear_worker_agent")?.unwrap_or(false),
            agent: str_arg(args, "agent")?,
            agent_model: str_arg(args, "agent_model")?,
            clear_agent_model: bool_arg(args, "clear_agent_model")?.unwrap_or(false),
            agent_effort: str_arg(args, "agent_effort")?,
            clear_agent_effort: bool_arg(args, "clear_agent_effort")?.unwrap_or(false),
            agent_skip_permissions: bool_arg(args, "agent_skip_permissions")?,
            agent_args: str_vec_arg(args, "agent_args")?,
            worker_model_policy: str_arg(args, "worker_model_policy")?,
        },
        "tako_orchestrator_layout" => Request::OrchestratorLayout {
            policy: str_arg(args, "policy")?,
            master_ratio: f64_arg(args, "master_ratio")?.map(|v| v as f32),
            algorithm: str_arg(args, "algorithm")?,
        },
        "tako_orchestrator_self" => Request::OrchestratorSelf {
            pane: u64_arg(args, "pane")?.or(caller),
            caller_role: caller_role.map(str::to_string),
            caller_pid: u64_arg(args, "caller_pid")?.map(|v| v as u32),
        },
        "tako_orchestrator_handoff" => Request::OrchestratorHandoff {
            pane: u64_arg(args, "pane")?.or(caller),
            caller_role: caller_role.map(str::to_string),
            tab: u64_arg(args, "tab")?,
            caller_pid: u64_arg(args, "caller_pid")?.map(|v| v as u32),
        },
        "tako_orchestrator_spawn" => {
            let pane = u64_arg(args, "pane")?;
            let tab = u64_arg(args, "tab")?;
            let resolved_pane = if pane.is_some() {
                pane
            } else if tab.is_some() {
                None
            } else {
                caller
            };
            let resolved_tab = if pane.is_some() { None } else { tab };
            if resolved_pane.is_none() && resolved_tab.is_none() {
                return Err("pane または tab を指定してください".into());
            }
            Request::OrchestratorSpawn {
                project: str_arg(args, "project")?.ok_or("project を指定する")?,
                prompt: str_arg(args, "prompt")?.ok_or("prompt を指定する")?,
                label: str_arg(args, "label")?,
                model: str_arg(args, "model")?,
                effort: str_arg(args, "effort")?,
                pane: resolved_pane,
                tab: resolved_tab,
                caller_role: caller_role.map(str::to_string),
                agent: str_arg(args, "agent")?,
                caller_pid: u64_arg(args, "caller_pid")?.map(|v| v as u32),
            }
        }
        "tako_orchestrator_worker_status" => Request::OrchestratorWorkerStatus {
            pane_id: required_u64(args, "pane_id")?,
            session_id: str_arg(args, "session_id")?,
            tmux_session: str_arg(args, "tmux_session")?,
        },
        "tako_orchestrator_run_status" => Request::OrchestratorRunStatus {
            run_id: str_arg(args, "run_id")?,
        },
        "tako_orchestrator_run_result" => Request::OrchestratorRunResult {
            run_id: str_arg(args, "run_id")?.ok_or("run_id を指定する")?,
        },
        "tako_remote_start" => Request::RemoteStart {
            port: u64_arg(args, "port")?.map(|v| v as u16),
            insecure: bool_arg(args, "insecure")?.unwrap_or(false),
        },
        "tako_remote_stop" => Request::RemoteStop,
        "tako_remote_status" => Request::RemoteStatus {
            show_token: bool_arg(args, "show_token")?.unwrap_or(false),
        },
        "tako_remote_agents" => Request::RemoteAgents,
        "tako_remote_messages" => Request::RemoteMessages {
            session_id: str_arg(args, "session_id")?.ok_or("session_id を指定する")?,
            tail: u64_arg(args, "tail")?.map(|n| n as usize),
        },
        "tako_remote_scrollback" => Request::RemoteScrollback {
            pane_id: str_arg(args, "pane_id")?
                .ok_or("pane_id を指定する")?
                .to_string(),
            lines: u64_arg(args, "lines")?.map(|n| n as u32),
        },
        "tako_web" => {
            let action = str_arg(args, "action")?.ok_or("action は必須")?.to_string();
            // 分割系（open / show）だけ、基準ペイン省略時に呼び出し元を分割元とする。
            // 対象指定系で caller を埋めると「AI 自身のペイン」を対象と誤解するため埋めない
            let pane = match action.as_str() {
                "open" | "show" => u64_arg(args, "pane")?.or(caller),
                _ => u64_arg(args, "pane")?,
            };
            Request::Web {
                action,
                url: str_arg(args, "url")?.map(|s| s.to_string()),
                id: u64_arg(args, "id")?,
                pane,
                direction: direction_arg(args)?,
                to: str_arg(args, "to")?.map(|s| s.to_string()),
                js: str_arg(args, "js")?.map(|s| s.to_string()),
                token: u64_arg(args, "token")?,
                focus: bool_arg(args, "focus")?,
            }
        }
        "tako_update" => Request::Update {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
        },
        "tako_fda" => Request::Fda {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
        },
        "tako_sleep_guard" => Request::SleepGuard {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            mode: str_arg(args, "mode")?.map(|s| s.to_string()),
            power_condition: str_arg(args, "power_condition")?.map(|s| s.to_string()),
            lid_sleep_mode: str_arg(args, "lid_sleep_mode")?.map(|s| s.to_string()),
        },
        "tako_theme" => Request::Theme {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            mode: str_arg(args, "mode")?.map(|s| s.to_string()),
        },
        "tako_setup_changes" => Request::SetupChanges,
        "tako_setup" => Request::SetupRun {
            answers: Some(args.clone()),
        },
        "tako_agents_sync_rules" => Request::AgentsSyncRules {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
            source: str_arg(args, "source")?.map(|s| s.to_string()),
            targets: {
                let arr = args.get("targets").and_then(Value::as_array);
                arr.map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
            },
        },
        "tako_tree_folder" => Request::TreeFolder {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
            path: str_arg(args, "path")?.map(|s| s.to_string()),
            tab: u64_arg(args, "tab")?,
            pane: caller,
        },
        "tako_sessions" => {
            let action = str_arg(args, "action")?.ok_or("action を指定する")?;
            Request::Sessions {
                // resume はペイン省略時に呼び出し元（master 自身の隣）へ分割する
                pane: if action == "resume" && u64_arg(args, "tab")?.is_none() {
                    u64_arg(args, "pane")?.or(caller)
                } else {
                    u64_arg(args, "pane")?
                },
                action,
                id: str_arg(args, "id")?,
                role: str_arg(args, "role")?,
                project: str_arg(args, "project")?,
                limit: u64_arg(args, "limit")?.map(|v| v as usize),
                tab: u64_arg(args, "tab")?,
                direction: direction_arg(args)?,
            }
        }
        "tako_logs" => Request::Logs {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
            // read はペイン・セッション未指定なら呼び出し元ペインのログを引く
            pane: match (u64_arg(args, "pane")?, str_arg(args, "session_id")?) {
                (Some(p), _) => Some(p),
                (None, None) => caller,
                (None, Some(_)) => None,
            },
            session_id: str_arg(args, "session_id")?,
            lines: u64_arg(args, "lines")?.map(|v| v as usize),
            enabled: bool_arg(args, "enabled")?,
            max_mb: u64_arg(args, "max_mb")?,
            total_max_mb: u64_arg(args, "total_max_mb")?,
        },
        "tako_open_dir" => Request::OpenDir {
            path: str_arg(args, "path")?.ok_or("path を指定する")?.to_string(),
            focus: bool_arg(args, "focus")?,
        },
        "tako_open_remote" => Request::OpenRemote {
            host: str_arg(args, "host")?.ok_or("host を指定する")?.to_string(),
            focus: bool_arg(args, "focus")?,
        },
        "tako_ssh_hosts" => Request::SshHosts,
        "tako_recent" => Request::RecentItems {
            action: str_arg(args, "action")?
                .ok_or("action を指定する")?
                .to_string(),
        },
        "tako_task_checkpoint" => Request::TaskCheckpoint {
            action: "checkpoint".into(),
            task_id: str_arg(args, "task_id")?,
            pane: u64_arg(args, "pane")?.or(caller),
            issue: u64_arg(args, "issue")?.map(|v| v as u32),
            branch: str_arg(args, "branch")?,
            phase: str_arg(args, "phase")?,
            last_commit: str_arg(args, "last_commit")?,
            agent: str_arg(args, "agent")?,
            model: str_arg(args, "model")?,
            prompt_head: str_arg(args, "prompt_head")?,
            suspended_reason: str_arg(args, "suspended_reason")?,
            project: str_arg(args, "project")?,
            cwd: str_arg(args, "cwd")?,
            resume_pane: None,
            tab: None,
            resume_model: None,
            caller_role: caller_role.map(String::from),
        },
        "tako_task_list" => Request::TaskCheckpoint {
            action: "list".into(),
            task_id: None,
            pane: None,
            issue: None,
            branch: None,
            phase: str_arg(args, "phase")?,
            last_commit: None,
            agent: None,
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            resume_pane: None,
            tab: None,
            resume_model: None,
            caller_role: None,
        },
        "tako_task_resume" => Request::TaskCheckpoint {
            action: "resume".into(),
            task_id: str_arg(args, "task_id")?,
            pane: None,
            issue: None,
            branch: None,
            phase: None,
            last_commit: None,
            agent: None,
            model: None,
            prompt_head: None,
            suspended_reason: None,
            project: None,
            cwd: None,
            resume_pane: if u64_arg(args, "tab")?.is_some() {
                None
            } else {
                u64_arg(args, "pane")?.or(caller)
            },
            tab: u64_arg(args, "tab")?,
            resume_model: str_arg(args, "model")?,
            caller_role: caller_role.map(String::from),
        },
        "tako_task_gate" => {
            let criteria_val = args.get("criteria").ok_or("criteria を指定する")?;
            let criteria_json = serde_json::to_string(criteria_val)
                .map_err(|e| format!("criteria の JSON 変換に失敗: {e}"))?;
            Request::TaskGate {
                action: "set".into(),
                task_id: str_arg(args, "task_id")?,
                criteria_json: Some(criteria_json),
                results_json: None,
                cwd: str_arg(args, "cwd")?,
                sync_checkpoint: None,
            }
        }
        // tako_task_gate_check は call_tool で特殊処理（dispatch を経由しない）
        "tako_task_gate_show" => Request::TaskGate {
            action: "show".into(),
            task_id: str_arg(args, "task_id")?,
            criteria_json: None,
            results_json: None,
            cwd: None,
            sync_checkpoint: None,
        },
        _ => return Err(format!("不明なツール: {name}")),
    })
}

/// `pane` 引数（省略時は呼び出し元へフォールバック。FR-2.3.3 のデフォルトスコープ）
fn target_pane(args: &Value, caller: Option<u64>) -> Result<u64, String> {
    u64_arg(args, "pane")?.or(caller).ok_or_else(|| {
        "対象ペインを特定できない（pane を指定する。\
         呼び出し元ペインの自動特定には TAKO_PANE_ID / X-Tako-Pane が必要）"
            .into()
    })
}

/// ツール名 → 許可パラメータ名セットのキャッシュ（#227）。
/// `tools()` のスキーマから `inputSchema.properties` のキーを抽出して構築する。
/// 全ツールの `additionalProperties: false` を実行時に強制する
fn allowed_params_map(
) -> &'static std::collections::HashMap<String, std::collections::HashSet<String>> {
    use std::collections::{HashMap, HashSet};
    use std::sync::OnceLock;
    static MAP: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for tool in tools() {
            if let (Some(name), Some(schema)) = (
                tool.get("name").and_then(Value::as_str),
                tool.get("inputSchema"),
            ) {
                let keys: HashSet<String> = schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .map(|props| props.keys().cloned().collect())
                    .unwrap_or_default();
                map.insert(name.to_string(), keys);
            }
        }
        map
    })
}

/// 引数の全キーがツールスキーマの `properties` に含まれるか検証する。
/// 未知キーがあれば JSON-RPC InvalidParams エラーを返す
fn validate_known_params(tool_name: &str, args: &Value) -> Result<(), (i64, String)> {
    let map = allowed_params_map();
    let Some(allowed) = map.get(tool_name) else {
        return Ok(());
    };
    if let Some(obj) = args.as_object() {
        let unknown: Vec<&String> = obj.keys().filter(|k| !allowed.contains(*k)).collect();
        if !unknown.is_empty() {
            return Err((
                -32602,
                format!(
                    "未知のパラメータ: {}（{tool_name} が受け付けるのは {} のみ）",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if allowed.is_empty() {
                        "引数なし".to_string()
                    } else {
                        let mut sorted: Vec<&str> = allowed.iter().map(String::as_str).collect();
                        sorted.sort_unstable();
                        sorted.join(", ")
                    },
                ),
            ));
        }
    }
    Ok(())
}

fn required_u64(args: &Value, key: &str) -> Result<u64, String> {
    u64_arg(args, key)?.ok_or_else(|| format!("{key} を指定する"))
}

fn u64_arg(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} は非負整数で指定する")),
    }
}

fn i64_arg(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{key} は整数で指定する")),
    }
}

fn f32_arg(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| format!("{key} は数値で指定する")),
    }
}

fn f64_arg(args: &Value, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{key} は数値で指定する")),
    }
}

fn str_arg(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| format!("{key} は文字列で指定する")),
    }
}

fn bool_arg(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{key} は真偽値で指定する")),
    }
}

fn direction_arg(args: &Value) -> Result<Option<Direction>, String> {
    match str_arg(args, "direction")?.as_deref() {
        None => Ok(None),
        Some("right") => Ok(Some(Direction::Right)),
        Some("down") => Ok(Some(Direction::Down)),
        Some("left") => Ok(Some(Direction::Left)),
        Some("up") => Ok(Some(Direction::Up)),
        Some(other) => Err(format!(
            "direction が不正: {other}（right / down / left / up のいずれか）"
        )),
    }
}

fn str_vec_arg(args: &Value, key: &str) -> Result<Option<Vec<String>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("{key} は文字列の配列で指定する"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{key} は文字列の配列で指定する")),
    }
}

// --- Streamable HTTP トランスポート ---

/// リクエストボディの上限（暴走・誤接続対策）
const MAX_BODY_BYTES: u64 = 1 << 20;

/// 内蔵 MCP サーバーのハンドル。`url` をペインのシェルへ `TAKO_MCP_URL` として注入する。
/// ポートはプロセス終了時に OS が解放するため明示シャットダウンは持たない
pub struct McpServer {
    url: String,
}

impl McpServer {
    /// 127.0.0.1 の空きポートで Streamable HTTP サーバーを起動する。
    /// 受け取った各操作は IPC と同じ `tx` 経由で UI スレッドへ届く（dispatch 共有）
    pub fn start(tx: UnboundedSender<IncomingRequest>, token: String) -> io::Result<Self> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| io::Error::other(format!("MCP HTTP サーバーを起動できない: {e}")))?;
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("MCP サーバーのポートを特定できない"))?
            .port();
        let url = format!("http://127.0.0.1:{port}/mcp");
        let token = Arc::new(token);
        std::thread::Builder::new()
            .name("tako-mcp-http".into())
            .spawn(move || {
                for request in server.incoming_requests() {
                    let tx = tx.clone();
                    let token = Arc::clone(&token);
                    std::thread::Builder::new()
                        .name("tako-mcp-req".into())
                        .spawn(move || {
                            handle_http(request, &token, &tx);
                        })
                        .ok();
                }
            })?;
        Ok(Self { url })
    }

    /// 接続先 URL（`TAKO_MCP_URL` として注入する）
    pub fn url(&self) -> &str {
        &self.url
    }
}

fn header_value(request: &tiny_http::Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn respond(request: tiny_http::Request, status: u16, body: Option<String>) {
    let result = match body {
        Some(body) => {
            let header =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("固定値のヘッダ構築は失敗しない");
            // 応答サイズは既知なので常に Content-Length で送る。tiny_http の既定は
            // 32KB 超で chunked に切り替わり、チャンク境界がマルチバイト文字の途中に
            // 落ちると素朴なクライアントの read_to_string が壊れる（ツールカタログが
            // 32KB を超えた際にセルフテストで顕在化）
            request.respond(
                tiny_http::Response::from_string(body)
                    .with_chunked_threshold(usize::MAX)
                    .with_header(header)
                    .with_status_code(status),
            )
        }
        None => request.respond(tiny_http::Response::empty(status)),
    };
    if let Err(e) = result {
        tracing::debug!("MCP 応答の送信に失敗: {e}");
    }
}

fn handle_http(
    mut request: tiny_http::Request,
    token: &str,
    tx: &UnboundedSender<IncomingRequest>,
) {
    // Origin 検証: ブラウザからの DNS リバインディング対策（MCP 仕様の要請）。
    // 非ブラウザクライアントは通常 Origin を送らない
    if let Some(origin) = header_value(&request, "origin") {
        let local = [
            "http://127.0.0.1",
            "http://localhost",
            "https://127.0.0.1",
            "https://localhost",
        ]
        .iter()
        .any(|prefix| origin.starts_with(prefix));
        if !local {
            return respond(request, 403, None);
        }
    }
    // Bearer トークン認証（FR-2.3.4。アプリ外プロセスの拒否）
    let authorized =
        header_value(&request, "authorization").is_some_and(|v| v == format!("Bearer {token}"));
    if !authorized {
        return respond(request, 401, None);
    }
    // Streamable HTTP の必須経路は POST のみ実装（GET の SSE ストリームは任意機能のため
    // 405 を返す。サーバー発のリクエスト・通知を持たないため不要）
    if *request.method() != tiny_http::Method::Post {
        return respond(request, 405, None);
    }
    let caller_pane = header_value(&request, "x-tako-pane").and_then(|v| v.parse().ok());
    let mut body = String::new();
    {
        use std::io::Read as _;
        if request
            .as_reader()
            .take(MAX_BODY_BYTES)
            .read_to_string(&mut body)
            .is_err()
        {
            return respond(request, 400, None);
        }
    }
    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        let error = json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": { "code": -32700, "message": "リクエストを JSON として解釈できない" },
        });
        return respond(request, 400, Some(error.to_string()));
    };
    let mut exec = |req: Request| -> Result<Value, String> {
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
    };
    let caller_role = header_value(&request, "x-tako-role").map(|v| v.to_string());
    let mut session = McpSession {
        caller_pane,
        caller_role,
        connected: true,
        exec: &mut exec,
        ipc_tx: Some(tx.clone()),
    };
    match handle_message(&message, &mut session) {
        Some(response) => respond(request, 200, Some(response.to_string())),
        // notification（initialized 等）には 202 Accepted を返す（Streamable HTTP 仕様）
        None => respond(request, 202, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 受けた Request を記録して固定値を返す exec
    fn run(message: Value, caller: Option<u64>, connected: bool) -> (Option<Value>, Vec<Request>) {
        let mut seen = Vec::new();
        let mut exec = |request: Request| -> Result<Value, String> {
            seen.push(request);
            Ok(json!({ "pane": 7 }))
        };
        let mut session = McpSession {
            caller_pane: caller,
            caller_role: None,
            connected,
            exec: &mut exec,
            ipc_tx: None,
        };
        let response = handle_message(&message, &mut session);
        (response, seen)
    }

    fn call(name: &str, args: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        })
    }

    #[test]
    fn initializeはバージョン交渉とinstructionsを返す() {
        let message = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-03-26" },
        });
        let (response, _) = run(message, None, true);
        let result = &response.unwrap()["result"];
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["name"], "tako");
        // 行動規範（FR-2.7.5）が埋め込まれている
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("レビューを求めるときは見せろ"));
        assert!(instructions.contains("片付け"));

        // 未知バージョンは最新を名乗る
        let message = json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "9999-01-01" },
        });
        let (response, _) = run(message, None, true);
        assert_eq!(
            response.unwrap()["result"]["protocolVersion"],
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn notificationとresponseには応答しない() {
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            None,
            true,
        );
        assert!(response.is_none());
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "id": 5, "result": {} }),
            None,
            true,
        );
        assert!(response.is_none());
    }

    #[test]
    fn open_fileはモードを解釈し呼び出し元へフォールバックする() {
        let (response, requests) = run(
            call(
                "tako_open_file",
                json!({ "path": "/tmp/x.md", "mode": "code" }),
            ),
            Some(7),
            true,
        );
        assert!(response.is_some());
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "/tmp/x.md".into(),
                mode: Some(crate::protocol::PreviewModeWire::Code),
                direction: None,
                focus: None,
            }]
        );
        // mode 省略は拡張子の自動判定に委ねる（None で渡る）。direction も省略可
        let (_, requests) = run(
            call("tako_open_file", json!({ "path": "a.rs" })),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "a.rs".into(),
                mode: None,
                direction: None,
                focus: None,
            }]
        );
        // direction 指定（FR-3.11 = D&D のドロップ位置の同等操作）
        let (_, requests) = run(
            call(
                "tako_open_file",
                json!({ "path": "a.rs", "direction": "down" }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OpenFile {
                pane: Some(7),
                path: "a.rs".into(),
                mode: None,
                direction: Some(Direction::Down),
                focus: None,
            }]
        );
        // 不正な mode と path 欠落は引数エラー
        let (response, requests) = run(
            call("tako_open_file", json!({ "path": "a.rs", "mode": "html" })),
            Some(7),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mode"));
        let (response, _) = run(call("tako_open_file", json!({})), Some(7), true);
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("path"));
    }

    #[test]
    fn preview編集3操作をrequestへ写す() {
        let (_, requests) = run(
            call("tako_preview_edit", json!({ "enabled": true })),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewEdit {
                pane: Some(7),
                enabled: Some(true),
            }]
        );
        let (_, requests) = run(
            call(
                "tako_preview_apply",
                json!({ "pane": 9, "text": "日本語\n" }),
            ),
            Some(7),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewApply {
                pane: Some(9),
                text: "日本語\n".into(),
            }]
        );
        let (_, requests) = run(call("tako_preview_save", json!({})), Some(7), true);
        assert_eq!(requests, vec![Request::PreviewSave { pane: Some(7) }]);
    }

    #[test]
    fn preview_viewは倍率ページパンをrequestへ写す() {
        let (_, requests) = run(
            call(
                "tako_preview_view",
                json!({
                    "pane": 7,
                    "zoom": 150.0,
                    "page": 3,
                    "pan_x": 24.0,
                    "pan_y": 48.0
                }),
            ),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewView {
                pane: Some(7),
                zoom: Some(150.0),
                zoom_in: false,
                zoom_out: false,
                reset: false,
                page: Some(3),
                pan_x: Some(24.0),
                pan_y: Some(48.0),
            }]
        );
    }

    #[test]
    fn preview_outlineは一覧取得と項目ジャンプをrequestへ写す() {
        let (_, requests) = run(
            call("tako_preview_outline", json!({ "pane": 7 })),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewOutline {
                pane: Some(7),
                item: None,
            }]
        );
        let (_, requests) = run(
            call("tako_preview_outline", json!({ "item": 2 })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewOutline {
                pane: Some(5),
                item: Some(2),
            }]
        );
    }

    #[test]
    fn tmux_openはセッション必須でドロップ位置相当を写す() {
        let (_, requests) = run(
            call(
                "tako_tmux_open",
                json!({ "session": "master-tako", "socket": "work", "direction": "down" }),
            ),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::TmuxOpen {
                socket: Some("work".into()),
                session: "master-tako".into(),
                window: None,
                pane: Some(3),
                direction: Some(Direction::Down),
            }]
        );
        // session 欠落は引数エラー
        let (response, requests) = run(call("tako_tmux_open", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("session"));
    }

    #[test]
    fn tako_setup_changesはsetup_changesリクエストに変換される() {
        let (response, requests) = run(call("tako_setup_changes", json!({})), None, true);
        assert_eq!(requests, vec![Request::SetupChanges]);
        assert_eq!(response.unwrap()["result"]["isError"], false);
    }

    #[test]
    fn tako_setupは全回答をsetup_runリクエストに変換する() {
        let answers = json!({
            "selected_agent": "codex",
            "provider_plans": {"gpt": "plus"},
            "instruction_content": "# Rules",
            "profile": {"master_agent": "codex", "effort": "high"},
            "projects": {"app": {"cwd": "~/src/app"}},
            "orchestrator": {"auto_close": false, "auto_push": false},
            "sleep_guard": {"mode": "while-agents-running", "power": "ac-only"}
        });
        let (_, requests) = run(call("tako_setup", answers.clone()), None, true);
        assert_eq!(
            requests,
            vec![Request::SetupRun {
                answers: Some(answers)
            }]
        );
    }

    #[test]
    fn tako_orchestrator_layoutはリクエストに変換される() {
        // 全省略 = 取得
        let (response, requests) = run(call("tako_orchestrator_layout", json!({})), None, true);
        assert_eq!(
            requests,
            vec![Request::OrchestratorLayout {
                policy: None,
                master_ratio: None,
                algorithm: None,
            }]
        );
        assert_eq!(response.unwrap()["result"]["isError"], false);

        // 指定あり = 設定
        let (_, requests) = run(
            call(
                "tako_orchestrator_layout",
                json!({ "policy": "legacy", "master_ratio": 0.6, "algorithm": "spiral" }),
            ),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::OrchestratorLayout {
                policy: Some("legacy".into()),
                master_ratio: Some(0.6),
                algorithm: Some("spiral".into()),
            }]
        );
    }

    #[test]
    fn preview_reloadは状態取得と切替をrequestへ写す() {
        let (_, requests) = run(call("tako_preview_reload", json!({})), None, true);
        assert_eq!(requests, vec![Request::PreviewReload { enabled: None }]);

        let (_, requests) = run(
            call("tako_preview_reload", json!({ "enabled": false })),
            None,
            true,
        );
        assert_eq!(
            requests,
            vec![Request::PreviewReload {
                enabled: Some(false)
            }]
        );
    }

    #[test]
    fn preview_cacheは状態取得と上限変更をrequestへ写す() {
        let (_, requests) = run(call("tako_preview_cache", json!({})), None, true);
        assert_eq!(requests, vec![Request::PreviewCache { max_mb: None }]);

        let (_, requests) = run(
            call("tako_preview_cache", json!({ "max_mb": 768 })),
            None,
            true,
        );
        assert_eq!(requests, vec![Request::PreviewCache { max_mb: Some(768) }]);
    }

    #[test]
    fn ツールカタログは操作セットを網羅する() {
        let tools = tools();
        assert_eq!(tools.len(), 91);
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            assert!(name.starts_with("tako_"), "{name} は tako_ 接頭辞");
            assert!(!tool["description"].as_str().unwrap().is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
        // 行動規範が説明文側にも埋め込まれている（FR-2.7.5）
        let split = tools
            .iter()
            .find(|t| t["name"] == "tako_split_pane")
            .unwrap();
        assert!(split["description"].as_str().unwrap().contains("レビュー"));
    }

    #[test]
    fn 未接続ではツールを公開しない() {
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            None,
            false,
        );
        assert_eq!(response.unwrap()["result"]["tools"], json!([]));
    }

    #[test]
    fn splitは呼び出し元ペインへフォールバックする() {
        let (response, seen) = run(
            call("tako_split_pane", json!({ "direction": "down" })),
            Some(3),
            true,
        );
        assert_eq!(
            seen,
            vec![Request::Split {
                pane: Some(3),
                tab: None,
                direction: Some(Direction::Down),
                ratio: None,
                command: None,
                cwd: None,
                focus: None,
            }]
        );
        let result = &response.unwrap()["result"];
        assert_eq!(result["isError"], false);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("7"));
    }

    #[test]
    fn 呼び出し元不明でpane省略はエラー() {
        let (response, seen) = run(call("tako_close_pane", json!({})), None, true);
        assert!(seen.is_empty());
        let error = &response.unwrap()["error"];
        assert_eq!(error["code"], -32602);
        assert!(error["message"].as_str().unwrap().contains("pane"));
    }

    #[test]
    fn sendとreadはpane必須() {
        let (response, seen) = run(
            call("tako_send_input", json!({ "text": "ls" })),
            Some(3), // 呼び出し元があってもフォールバックしない（誤送信防止）
            true,
        );
        assert!(seen.is_empty());
        assert_eq!(response.unwrap()["error"]["code"], -32602);

        let (_, seen) = run(
            call("tako_read_pane", json!({ "pane": 4, "lines": 10 })),
            None,
            true,
        );
        assert_eq!(
            seen,
            vec![Request::Read {
                pane: Some(4),
                lines: Some(10),
                tmux_session: None,
            }]
        );
    }

    #[test]
    fn 実行エラーはエラーフラグ付き結果になる() {
        let mut exec = |_: Request| -> Result<Value, String> {
            Err("ペイン 9 が見つからない".into())
        };
        let mut session = McpSession {
            caller_pane: None,
            caller_role: None,
            connected: true,
            exec: &mut exec,
            ipc_tx: None,
        };
        let response = handle_message(&call("tako_list_panes", json!({})), &mut session).unwrap();
        let result = &response["result"];
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("見つからない"));
    }

    #[test]
    fn 不明なツールと未対応メソッドはエラー() {
        let (response, _) = run(call("tako_explode", json!({})), None, true);
        assert_eq!(response.unwrap()["error"]["code"], -32602);
        let (response, _) = run(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }),
            None,
            true,
        );
        assert_eq!(response.unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn pin_previewはペインまたはグループタブをトグルする() {
        // pane 指定（呼び出し元フォールバック）
        let (_, requests) = run(
            call("tako_pin_preview", json!({ "pinned": true })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Pin {
                pane: Some(5),
                group_tab: None,
                pinned: Some(true),
            }]
        );
        // group_tab 指定時は pane を補完しない（排他）
        let (_, requests) = run(
            call("tako_pin_preview", json!({ "group_tab": 2 })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Pin {
                pane: None,
                group_tab: Some(2),
                pinned: None,
            }]
        );
        // 両方省略 = 呼び出し元ペインでトグル
        let (_, requests) = run(call("tako_pin_preview", json!({})), Some(5), true);
        assert_eq!(
            requests,
            vec![Request::Pin {
                pane: Some(5),
                group_tab: None,
                pinned: None,
            }]
        );
        // pinned に不正な型を渡すとエラー
        let (response, requests) = run(
            call("tako_pin_preview", json!({ "pinned": "yes" })),
            Some(5),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pinned"));
    }

    #[test]
    fn video_playbackはaction必須でペインへフォールバックする() {
        let (_, requests) = run(
            call("tako_video_playback", json!({ "action": "toggle" })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoPlayback {
                pane: Some(3),
                action: "toggle".into(),
            }]
        );
        // pane 明示指定
        let (_, requests) = run(
            call(
                "tako_video_playback",
                json!({ "pane": 10, "action": "play" }),
            ),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoPlayback {
                pane: Some(10),
                action: "play".into(),
            }]
        );
        // action 欠落はエラー
        let (response, requests) = run(call("tako_video_playback", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("action"));
        // 呼び出し元なし + pane 省略もエラー
        let (response, requests) = run(
            call("tako_video_playback", json!({ "action": "pause" })),
            None,
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pane"));
    }

    #[test]
    fn video_seekはseconds必須でペインへフォールバックする() {
        let (_, requests) = run(
            call("tako_video_seek", json!({ "seconds": 42.5 })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoSeek {
                pane: Some(3),
                seconds: 42.5,
            }]
        );
        // seconds 欠落はエラー
        let (response, requests) = run(call("tako_video_seek", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("seconds"));
        // seconds に負値（スキーマでは minimum: 0 だが、f64_arg は型のみ検証。
        // ここではパース層が通ることを確認。意味検証は dispatch 側の責務）
        let (_, requests) = run(
            call("tako_video_seek", json!({ "seconds": 0.0 })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoSeek {
                pane: Some(3),
                seconds: 0.0,
            }]
        );
        // seconds に文字列を渡すとエラー
        let (response, requests) = run(
            call("tako_video_seek", json!({ "seconds": "ten" })),
            Some(3),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("seconds"));
    }

    #[test]
    fn video_volumeはvolume必須でペインへフォールバックする() {
        let (_, requests) = run(
            call("tako_video_volume", json!({ "volume": 0.5 })),
            Some(3),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::VideoVolume {
                pane: Some(3),
                volume: 0.5,
            }]
        );
        let (response, requests) = run(call("tako_video_volume", json!({})), Some(3), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("volume"));
    }

    #[test]
    fn video_playbackのmute_loop操作がパースできる() {
        for action in &[
            "mute",
            "unmute",
            "toggle_mute",
            "loop_on",
            "loop_off",
            "toggle_loop",
        ] {
            let (_, requests) = run(
                call("tako_video_playback", json!({ "action": action })),
                Some(3),
                true,
            );
            assert_eq!(
                requests,
                vec![Request::VideoPlayback {
                    pane: Some(3),
                    action: action.to_string(),
                }]
            );
        }
    }

    #[test]
    fn webはactionごとにcaller既定を使い分ける() {
        // open: pane 省略 → caller が分割元になる
        let (_, requests) = run(
            call(
                "tako_web",
                json!({ "action": "open", "url": "http://localhost:3000" }),
            ),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Web {
                action: "open".into(),
                url: Some("http://localhost:3000".into()),
                id: None,
                pane: Some(5),
                direction: None,
                to: None,
                js: None,
                token: None,
                focus: None,
            }]
        );
        // navigate: pane 省略でも caller を埋めない（対象は表示中 Web ビューの自動解決）
        let (_, requests) = run(
            call("tako_web", json!({ "action": "navigate", "to": "reload" })),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::Web {
                action: "navigate".into(),
                url: None,
                id: None,
                pane: None,
                direction: None,
                to: Some("reload".into()),
                js: None,
                token: None,
                focus: None,
            }]
        );
        // action 欠落はエラー
        let (response, requests) = run(call("tako_web", json!({})), Some(5), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("action"));
        // 不正な direction はエラー
        let (response, requests) = run(
            call(
                "tako_web",
                json!({ "action": "open", "url": "http://localhost:3000", "direction": "diagonal" }),
            ),
            Some(5),
            true,
        );
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("direction"));
    }

    #[test]
    fn orchestrator_spawnのpaneとtab優先順位() {
        // pane のみ → pane が使われ tab は None
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi", "pane": 5 }),
            ),
            Some(99),
            true,
        );
        assert_eq!(requests.len(), 1);
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, Some(5));
                assert_eq!(*tab, None);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // tab のみ → tab が使われ pane は None（caller もフォールバックしない）
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi", "tab": 2 }),
            ),
            Some(99),
            true,
        );
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, None);
                assert_eq!(*tab, Some(2));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // pane と tab 両方 → pane 優先、tab は None
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi", "pane": 5, "tab": 2 }),
            ),
            Some(99),
            true,
        );
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, Some(5), "pane が tab より優先される");
                assert_eq!(*tab, None, "pane 指定時は tab を無視する");
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 両方省略、caller あり → caller がフォールバック
        let (_, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi" }),
            ),
            Some(42),
            true,
        );
        match &requests[0] {
            Request::OrchestratorSpawn { pane, tab, .. } => {
                assert_eq!(*pane, Some(42), "caller へフォールバック");
                assert_eq!(*tab, None);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // 両方省略、caller なし → エラー
        let (response, requests) = run(
            call(
                "tako_orchestrator_spawn",
                json!({ "project": "p", "prompt": "hi" }),
            ),
            None,
            true,
        );
        assert!(requests.is_empty());
        let error = &response.unwrap()["error"];
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("pane または tab"),
            "pane も tab も無い場合はエラー"
        );
    }

    // --- HTTP トランスポート（実ポートで往復） ---

    mod http {
        use super::*;
        use futures::channel::mpsc::unbounded;
        use futures::StreamExt;
        use std::io::{Read, Write};

        const TOKEN: &str = "http-test-token";

        /// サーバー + ダミーディスパッチャ（list に固定値を返す）を立てる
        fn start_server() -> McpServer {
            let (tx, mut rx) = unbounded::<IncomingRequest>();
            let server = McpServer::start(tx, TOKEN.into()).expect("MCP サーバーを起動できる");
            std::thread::spawn(move || {
                while let Some(incoming) = futures::executor::block_on(rx.next()) {
                    assert_eq!(incoming.origin, PaneOrigin::Mcp);
                    let _ = incoming.reply.send(Ok(json!({ "tabs": [] })));
                }
            });
            server
        }

        fn post(
            url: &str,
            auth: Option<&str>,
            extra_headers: &[(&str, &str)],
            body: &str,
        ) -> (u16, String) {
            let rest = url.strip_prefix("http://").expect("テスト URL は http");
            let (hostport, path) = rest.split_once('/').expect("URL にパスがある");
            let mut stream = std::net::TcpStream::connect(hostport).expect("接続できる");
            let mut request = format!(
                "POST /{path} HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\n\
                 Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            if let Some(token) = auth {
                request.push_str(&format!("Authorization: Bearer {token}\r\n"));
            }
            for (name, value) in extra_headers {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
            request.push_str("\r\n");
            request.push_str(body);
            stream.write_all(request.as_bytes()).expect("送信できる");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("受信できる");
            let status = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .expect("ステータス行がある");
            let body = response
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_string())
                .unwrap_or_default();
            (status, body)
        }

        #[test]
        fn 認証付きでツール呼び出しが往復する() {
            let server = start_server();
            let body = call("tako_list_panes", json!({})).to_string();
            let (status, response) = post(server.url(), Some(TOKEN), &[], &body);
            assert_eq!(status, 200);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["result"]["isError"], false);
            assert!(response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("tabs"));
        }

        #[test]
        fn 不正トークンと不正オリジンは拒否される() {
            let server = start_server();
            let body = call("tako_list_panes", json!({})).to_string();
            let (status, _) = post(server.url(), Some("bogus"), &[], &body);
            assert_eq!(status, 401);
            let (status, _) = post(server.url(), None, &[], &body);
            assert_eq!(status, 401);
            let (status, _) = post(
                server.url(),
                Some(TOKEN),
                &[("Origin", "http://evil.example")],
                &body,
            );
            assert_eq!(status, 403);
        }

        #[test]
        fn tools_listはhttp経由で全カタログを返す() {
            // 50 ツール（日本語説明文込みで数十 KB）の大きな応答が HTTP 層で
            // 欠けずに返ることを検証する（セルフテスト項目 32 のユニット版）
            let server = start_server();
            let body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
            let (status, response) = post(server.url(), Some(TOKEN), &[], body);
            assert_eq!(status, 200);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(
                response["result"]["tools"].as_array().unwrap().len(),
                tools().len()
            );
        }

        #[test]
        fn notificationは202になる() {
            let server = start_server();
            let body = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
            let (status, _) = post(server.url(), Some(TOKEN), &[], &body.to_string());
            assert_eq!(status, 202);
        }

        #[test]
        fn 呼び出し元ペインはヘッダで申告できる() {
            let (tx, mut rx) = unbounded::<IncomingRequest>();
            let server = McpServer::start(tx, TOKEN.into()).unwrap();
            std::thread::spawn(move || {
                while let Some(incoming) = futures::executor::block_on(rx.next()) {
                    // X-Tako-Pane がデフォルト対象として解決されている（FR-2.3.3）
                    assert_eq!(
                        incoming.request,
                        Request::Close {
                            pane: Some(42),
                            force: false
                        },
                        "X-Tako-Pane が呼び出し元として使われる"
                    );
                    let _ = incoming.reply.send(Ok(json!({ "closed": 42 })));
                }
            });
            let body = call("tako_close_pane", json!({})).to_string();
            let (status, response) =
                post(server.url(), Some(TOKEN), &[("X-Tako-Pane", "42")], &body);
            assert_eq!(status, 200);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["result"]["isError"], false);
        }

        #[test]
        fn 遅いdispatch中も並行リクエストがブロックされない() {
            let (tx, mut rx) = unbounded::<IncomingRequest>();
            let server = McpServer::start(tx, TOKEN.into()).unwrap();
            // dispatch ハンドラ: 重い dispatch は別スレッドへ offload（実 app の
            // OffloadJob と同じパターン。UI スレッドは即座に次のリクエストへ進む）
            std::thread::spawn(move || {
                while let Some(incoming) = futures::executor::block_on(rx.next()) {
                    match &incoming.request {
                        Request::Read { .. } => {
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(500));
                                let _ = incoming.reply.send(Ok(json!({ "slow": true })));
                            });
                        }
                        _ => {
                            let _ = incoming.reply.send(Ok(json!({ "tabs": [] })));
                        }
                    }
                }
            });
            let url = server.url().to_string();
            // 遅い read_pane を先に投げる
            let url_slow = url.clone();
            let slow = std::thread::spawn(move || {
                let body = call("tako_read_pane", json!({"pane": 1})).to_string();
                let start = std::time::Instant::now();
                let (status, _) = post(&url_slow, Some(TOKEN), &[], &body);
                (status, start.elapsed())
            });
            // 少し待ってから高速な list_panes を投げる
            std::thread::sleep(std::time::Duration::from_millis(50));
            let url_fast = url.clone();
            let fast = std::thread::spawn(move || {
                let body = call("tako_list_panes", json!({})).to_string();
                let start = std::time::Instant::now();
                let (status, _) = post(&url_fast, Some(TOKEN), &[], &body);
                (status, start.elapsed())
            });
            let (slow_status, slow_elapsed) = slow.join().unwrap();
            let (fast_status, fast_elapsed) = fast.join().unwrap();
            assert_eq!(slow_status, 200);
            assert_eq!(fast_status, 200);
            // 並行化されていれば fast は slow を待たず 200ms 以内に返る
            // （直列なら slow の 500ms 完了後にしか処理されない）
            assert!(
                fast_elapsed < std::time::Duration::from_millis(200),
                "list_panes が read_pane の完了を待ってしまった（{:?}、並行化されていない）",
                fast_elapsed,
            );
            assert!(slow_elapsed >= std::time::Duration::from_millis(400));
        }
    }

    #[test]
    fn 未知パラメータはエラーになる_spawn() {
        let msg = call(
            "tako_orchestrator_spawn",
            json!({ "project": "p", "prompt": "hi", "agentt": "codex" }),
        );
        let (resp, _) = run(msg, Some(0), true);
        let err = &resp.unwrap()["error"];
        assert_eq!(err["code"], -32602);
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("agentt"), "エラーに未知キー名を含む: {msg}");
        assert!(
            msg.contains("tako_orchestrator_spawn"),
            "エラーにツール名を含む: {msg}"
        );
    }

    #[test]
    fn 未知パラメータはエラーになる_list_panes() {
        let msg = call("tako_list_panes", json!({ "foo": "bar" }));
        let (resp, _) = run(msg, Some(0), true);
        let err = &resp.unwrap()["error"];
        assert_eq!(err["code"], -32602);
        let msg = err["message"].as_str().unwrap();
        assert!(msg.contains("foo"), "エラーに未知キー名を含む: {msg}");
    }

    #[test]
    fn 正規パラメータはエラーにならない_spawn() {
        let msg = call(
            "tako_orchestrator_spawn",
            json!({ "project": "p", "prompt": "hi", "agent": "codex", "pane": 0 }),
        );
        let (resp, _) = run(msg, Some(0), true);
        assert!(
            resp.as_ref().unwrap().get("error").is_none(),
            "正規パラメータでエラー: {:?}",
            resp
        );
    }
}
