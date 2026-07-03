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

use futures::channel::mpsc::UnboundedSender;
use serde_json::{json, Value};
use tako_core::PaneOrigin;

use crate::ipc::IncomingRequest;
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
    /// false のとき tools/list は空を返す（tako の外で起動された stdio ブリッジ用。
    /// 登録済みでも tako 外の Claude Code セッションを邪魔しない）
    pub connected: bool,
    /// 操作の実行係（HTTP: dispatch チャネル往復、stdio: IPC 往復）。
    /// Err は「ツール実行エラー」として isError 付き結果になる
    pub exec: &'a mut dyn FnMut(Request) -> Result<Value, String>,
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
                応答は queued: true が即座に返り、実際の送達確認はバックグラウンドで行われる）。",
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
                tmux_session を指定するとペインが見つからない場合でも tmux session 経由で読める。",
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
            "description": "動画プレビューペインの再生/一時停止/再生速度を操作する。\
                対象ペインが動画プレビュー（tako open で .mp4/.mov 等を開いた状態）の場合のみ有効。\
                action に play / pause / toggle / rate:N（N は 0.1〜4.0 の速度倍率、例: rate:2.0）を指定する。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
                    "action": {
                        "type": "string",
                        "description": "再生操作（play=再生、pause=一時停止、toggle=切替、rate:N=速度変更）",
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
                いまのタブと無関係な作業系列を始めるときに使う（1 グループ = 1 タブ）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "タブのタイトル（省略時は連番）" },
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
                ペインタイトルバーの D&D と同じ操作。タブまたぎも可）。どちらか一方を指定する。\
                レイアウトを整えてユーザーに見せる導線に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tab": { "type": "integer", "minimum": 0, "description": "移送先タブ ID（target と排他）" },
                    "target": { "type": "integer", "minimum": 0, "description": "挿入先ペイン ID（このペインの隣に入る）" },
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "target のどちら側に入るか（省略時は right。target 指定時のみ有効）",
                    },
                    "pane": pane_schema("対象ペイン ID（省略時は呼び出し元）"),
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
                    "view": { "type": "string", "enum": ["tmux", "git"], "description": "表示するビュー" },
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
                },
                "required": ["path"],
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
            "description": "ペインをバックグラウンドへ送る。プロセスは生きたまま\
                画面から外す。邪魔なペインを画面外へ送るのに使う。バックグラウンドのペインは\
                tako_background_list で確認でき、tako_foreground_pane で画面に戻せる。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pane": { "type": "integer", "description": "バックグラウンドへ送るペインの ID（省略時は呼び出し元）" },
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
                プロファイルは profiles/<name>.yaml に保存され、master のモデル・effort と\
                子 worker のモデル決定に使われる。model が null / 未指定のプロファイルは\
                claude CLI の既定モデルで起動する（プラン非依存・推奨）。\
                1M コンテキスト版（[1m] サフィックス）は Max / API プラン限定のため、\
                set で明示指定した場合のみ使われる（Pro プランでは起動不能になる点に注意）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "show", "set"],
                        "description": "操作種別（省略時は list）",
                    },
                    "name": { "type": "string", "description": "プロファイル名（set 時に必須。show 省略時は default）" },
                    "model": { "type": "string", "description": "master のモデル（set 時。省略で現状維持）" },
                    "clear_model": { "type": "boolean", "description": "master のモデル指定を解除して claude 既定に戻す（set 時）" },
                    "worker_model": { "type": "string", "description": "worker_model_policy=fixed 時の子 worker モデル（set 時）" },
                    "clear_worker_model": { "type": "boolean", "description": "子 worker のモデル指定を解除する（set 時）" },
                    "effort": { "type": "string", "description": "master の thinking effort（set 時。省略で現状維持）" },
                    "worker_effort": { "type": "string", "description": "子 worker の thinking effort（set 時）" },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_orchestrator_spawn",
            "description": "プロジェクトの作業ディレクトリで子 claude worker を spawn する。\
                呼び出し元ペインを右に分割して新ペインを作り、claude を起動してプロンプトを送信する。\
                worker の pane_id・tmux_session・spawned_by（spawn 元ペイン ID）を返す。\
                tmux_session は pane ID が解決できない場合\
                （BG タブ移動・tako 再起動後）のフォールバックとして tako_read_pane / tako_send_input に渡せる。\
                worker_status / watch は pane_id だけで session を自動解決するため session_id は不要。\
                起動からプロンプト送信まで 15〜20 秒かかる（これは想定内）。\
                pane または tab のいずれかを必ず指定すること。省略すると呼び出し元タブに出るため、\
                master が別タブにいる場合に意図しないタブに子が生える。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml に登録済みであること）" },
                    "prompt": { "type": "string", "description": "worker に渡す初期プロンプト" },
                    "label": { "type": "string", "description": "ペインタイトルに付けるラベル（省略時は '<project>-worker'）" },
                    "model": { "type": "string", "description": "claude のモデル（省略時はマスターのプロファイル設定に従う）" },
                    "effort": { "type": "string", "description": "thinking effort（省略時はマスターのプロファイル設定に従う）" },
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
                gone（ペイン消滅かつ tmux session も消滅）/ unknown（agents 不可）。\
                session_id を省略しても pane→session の自動解決（pid 祖先辿り）で claude agents --json の \
                正確な status を取得する（status_source が agents-auto になる）。自動解決失敗時のみ \
                画面パターン推定にフォールバック（status_source が screen）。\
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
            "name": "tako_orchestrator_run",
            "description": "子 worker を spawn し、完了まで待って結果を返す。spawn + 完了待ち + \
                出力取得 + close を 1 回で行うアトミック操作。Monitor や手動ポーリングは不要。\
                完了判定は OrchestratorWorkerStatus と同じロジック（pane→session 自動解決 + \
                claude agents --json の status 一次シグナル、フォールバックで端末出力パターン）を \
                内部で繰り返し呼ぶ。\
                タイムアウト（既定 1800 秒）に達した場合は status=timeout で途中結果を返す。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "プロジェクトキー（projects.yaml に登録済み）" },
                    "prompt": { "type": "string", "description": "worker に渡すプロンプト" },
                    "label": { "type": "string", "description": "ペインタイトルのラベル（省略時は '<project>-worker'）" },
                    "model": { "type": "string", "description": "claude のモデル（省略時はマスターのプロファイル設定に従う）" },
                    "effort": { "type": "string", "description": "thinking effort（省略時はマスターのプロファイル設定に従う）" },
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
                },
                "required": ["project", "prompt"],
                "additionalProperties": false,
            },
        }),
        // --- リモートアクセス MCP ツール ---
        json!({
            "name": "tako_remote_start",
            "description": "リモートアクセス API サーバーを起動する。スマホからブラウザ経由で\
                ペインを操作するための HTTP API サーバーが指定ポート（既定 7749）で開始される。\
                cloudflared Quick Tunnel を自動起動して外部からのアクセスも可能にする。\
                起動後は /api/panes 等のエンドポイントで curl やブラウザから操作できる。\
                ターミナルに接続用の QR コードが表示される。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "port": {
                        "type": "integer", "minimum": 1, "maximum": 65535,
                        "description": "サーバーのポート番号（省略時は 7749）",
                    },
                    "no_tunnel": {
                        "type": "boolean",
                        "description": "true にすると cloudflared を起動しない（LAN のみモード）",
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
                起動中なら running=true とポート番号・トークンを返す。",
            "inputSchema": {
                "type": "object",
                "properties": {},
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
            "name": "tako_chrome_open",
            "description": "URL を Chrome CDP ミラー方式で Web ビューペインとして開く（FR-3.8 PoC）。\
                Chrome を --remote-debugging-port 付きで起動し、ページのスクリーンショットを \
                ペインにミラー表示する。ペイン内のクリックは Chrome に中継される。\
                dev サーバーのプレビュー表示やドキュメント参照に使う。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "開く URL（必須）" },
                    "pane": pane_schema("基準ペイン ID（省略時は呼び出し元。この隣に Web ビューペインを分割する）"),
                    "direction": {
                        "type": "string",
                        "enum": ["right", "down", "left", "up"],
                        "description": "分割方向（省略時は右）",
                    },
                },
                "required": ["url"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "tako_update",
            "description": "アプリ内更新の診断・チェック・実行（Issue #36）。\
                action=status で配布系統（homebrew / zip）・現在バージョン・PATH 上の重複 CLI を返す。\
                action=check で GitHub Releases から最新版の有無を確認する（更新は行わない）。\
                action=apply で配布系統に応じた更新を実行する \
                （homebrew → brew upgrade --cask、zip → GitHub Releases から zip を DL して .app を差し替え）。\
                zip 系統に brew を被せない（管理台帳と実体のズレを防ぐため）。\
                apply 成功後の再起動は UI 側で行う（CLI / MCP からは apply 結果の確認まで）。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "check", "apply"],
                        "description": "操作種別（省略時は status）",
                    },
                },
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

    // orchestrator_run はポーリングループを伴うため MCP ハンドラスレッドで合成する
    // （dispatch は同期・UI スレッド実行のため長時間ブロック不可）
    if name == "tako_orchestrator_run" {
        return orchestrator_run(&args, session);
    }

    let request = build_request(name, &args, session.caller_pane).map_err(|e| (-32602, e))?;
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

/// `tako_orchestrator_run` — spawn + 完了待ち + 出力取得 + close の合成操作。
/// ポーリングループは MCP ハンドラスレッドで実行される（UI スレッドはブロックしない）。
/// 各ステップは exec 経由で dispatch を呼ぶため、UI スレッドは短時間しか占有しない
fn orchestrator_run(args: &Value, session: &mut McpSession) -> Result<Value, (i64, String)> {
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

    // --- 1. Spawn ---
    let spawn_req = Request::OrchestratorSpawn {
        project: project.clone(),
        prompt: prompt.clone(),
        label: label.clone(),
        model,
        effort,
        pane,
        tab,
    };
    let spawn_result = (session.exec)(spawn_req).map_err(|e| (-32602, e))?;
    let pane_id = spawn_result["pane_id"].as_u64().unwrap_or(0);
    let spawned_by = spawn_result["spawned_by"].as_u64().unwrap_or(0);
    let tmux_session = spawn_result["tmux_session"].as_str().map(String::from);

    // --- 2. 完了待ちポーリング ---
    // orchestrator_watch と同じ判定ロジック: OrchestratorWorkerStatus を繰り返し呼び、
    // status + 端末出力パターンで idle/gone を判定する。
    // session_id は spawn 直後には不明だが、dispatch 側で pane→session 自動解決する
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let interval = std::time::Duration::from_secs(5);
    let mut idle_streak: u32 = 0;
    let mut gone_streak: u32 = 0;
    let mut final_status = "timeout".to_string();

    // claude 起動 + プロンプト送信を待つ（prompt_flow は 15〜20 秒かかる）
    std::thread::sleep(std::time::Duration::from_secs(20));

    loop {
        if start.elapsed() > timeout {
            break;
        }

        let status_req = Request::OrchestratorWorkerStatus {
            pane_id,
            session_id: None,
            tmux_session: tmux_session.clone(),
        };

        match (session.exec)(status_req) {
            Ok(val) => {
                let status = val["status"].as_str().unwrap_or("unknown");
                let recent = val["recent_output"].as_str().unwrap_or("");
                let source = val["status_source"].as_str().unwrap_or("screen");
                let need_streak: u32 = if source == "screen" { 8 } else { 3 };

                match status {
                    "gone" => {
                        gone_streak += 1;
                        if gone_streak >= 2 {
                            final_status = "error".to_string();
                            break;
                        }
                    }
                    "idle" => {
                        gone_streak = 0;
                        if screen_looks_busy(recent) {
                            idle_streak = 0;
                        } else {
                            idle_streak += 1;
                        }
                    }
                    "busy" => {
                        gone_streak = 0;
                        idle_streak = 0;
                    }
                    _ => {
                        // unknown: 端末出力から推定
                        gone_streak = 0;
                        if screen_looks_busy(recent) {
                            idle_streak = 0;
                        } else if screen_looks_idle(recent) {
                            idle_streak += 1;
                        } else {
                            idle_streak = 0;
                        }
                    }
                }

                if idle_streak >= need_streak {
                    final_status = "completed".to_string();
                    break;
                }
            }
            Err(_) => {
                gone_streak += 1;
                if gone_streak >= 2 {
                    final_status = "error".to_string();
                    break;
                }
            }
        }

        std::thread::sleep(interval);
    }

    // --- 3. 出力取得 ---
    let output = {
        let read_req = Request::Read {
            pane: Some(pane_id),
            lines: Some(output_lines),
            tmux_session: tmux_session.clone(),
        };
        match (session.exec)(read_req) {
            Ok(result) => result["content"].as_str().unwrap_or("").to_string(),
            Err(_) => String::new(),
        }
    };

    // --- 4. 自動 close（orchestrator run の完了後なので force: true）---
    let closed = if auto_close {
        let close_req = Request::Close {
            pane: Some(pane_id),
            force: true,
        };
        (session.exec)(close_req).is_ok()
    } else {
        false
    };

    let result = json!({
        "pane_id": pane_id,
        "spawned_by": spawned_by,
        "status": final_status,
        "output": output,
        "duration_seconds": start.elapsed().as_secs(),
        "closed": closed,
    });
    Ok(json!({
        "content": [{ "type": "text", "text": result.to_string() }],
        "isError": false,
    }))
}

/// 端末出力が busy を示すパターンを含むか（末尾 5 行に限定）
fn screen_looks_busy(output: &str) -> bool {
    tail_lines(output, 5).iter().any(|l| {
        l.contains("esc to interrupt")
            || l.contains("ing… (")
            || l.contains("Thinking")
            || l.contains("Reading")
            || l.contains("Editing")
            || l.contains("Running")
            || l.contains("Writing")
            || l.contains("Searching")
    })
}

/// 端末出力が idle（❯ プロンプト）を示すか（末尾 10 行でチェック）
fn screen_looks_idle(output: &str) -> bool {
    tail_lines(output, 10)
        .iter()
        .any(|l| l.trim_start().starts_with('❯'))
}

/// 空行を除いた末尾 N 行を返す
fn tail_lines(output: &str, n: usize) -> Vec<&str> {
    output
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .collect()
}

/// ツール呼び出しを操作プロトコル（[`Request`]）へ写す。エラーは引数バリデーション失敗
fn build_request(name: &str, args: &Value, caller: Option<u64>) -> Result<Request, String> {
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
        },
        "tako_select_tab" => Request::TabSelect {
            tab: required_u64(args, "tab")?,
        },
        "tako_move_pane_to_tab" => Request::MovePane {
            pane: Some(target_pane(args, caller)?),
            tab: u64_arg(args, "tab")?,
            target: u64_arg(args, "target")?,
            direction: direction_arg(args)?,
        },
        "tako_auto_rename" => Request::AutoRename {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_port_detect" => Request::PortDetect {
            enabled: bool_arg(args, "enabled")?,
        },
        "tako_persist" => Request::Persist {
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
        "tako_background_pane" => Request::Background {
            pane: Some(target_pane(args, caller)?),
        },
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
                Some("git") => Some(crate::protocol::PanelViewWire::Git),
                Some(other) => return Err(format!("view が不正: {other}（tmux | git）")),
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
            worker_model: str_arg(args, "worker_model")?,
            effort: str_arg(args, "effort")?,
            worker_effort: str_arg(args, "worker_effort")?,
            clear_model: bool_arg(args, "clear_model")?.unwrap_or(false),
            clear_worker_model: bool_arg(args, "clear_worker_model")?.unwrap_or(false),
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
            }
        }
        "tako_orchestrator_worker_status" => Request::OrchestratorWorkerStatus {
            pane_id: required_u64(args, "pane_id")?,
            session_id: str_arg(args, "session_id")?,
            tmux_session: str_arg(args, "tmux_session")?,
        },
        "tako_remote_start" => Request::RemoteStart {
            port: u64_arg(args, "port")?.map(|v| v as u16),
            no_tunnel: bool_arg(args, "no_tunnel")?.unwrap_or(false),
        },
        "tako_remote_stop" => Request::RemoteStop,
        "tako_remote_status" => Request::RemoteStatus,
        "tako_remote_agents" => Request::RemoteAgents,
        "tako_remote_messages" => Request::RemoteMessages {
            session_id: str_arg(args, "session_id")?.ok_or("session_id を指定する")?,
            tail: u64_arg(args, "tail")?.map(|n| n as usize),
        },
        "tako_chrome_open" => Request::ChromeOpen {
            url: str_arg(args, "url")?.ok_or("url は必須")?.to_string(),
            pane: u64_arg(args, "pane")?.or(caller),
            direction: direction_arg(args)?,
        },
        "tako_update" => Request::Update {
            action: str_arg(args, "action")?.map(|s| s.to_string()),
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
        std::thread::Builder::new()
            .name("tako-mcp-http".into())
            .spawn(move || {
                for request in server.incoming_requests() {
                    handle_http(request, &token, &tx);
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
    let mut session = McpSession {
        caller_pane,
        connected: true,
        exec: &mut exec,
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
            connected,
            exec: &mut exec,
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
    fn ツールカタログは操作セットを網羅する() {
        let tools = tools();
        assert_eq!(tools.len(), 50);
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
            connected: true,
            exec: &mut exec,
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
    fn chrome_openはurl必須でdirectionを解釈する() {
        let (_, requests) = run(
            call(
                "tako_chrome_open",
                json!({ "url": "http://localhost:3000" }),
            ),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::ChromeOpen {
                url: "http://localhost:3000".into(),
                pane: Some(5),
                direction: None,
            }]
        );
        // direction 指定
        let (_, requests) = run(
            call(
                "tako_chrome_open",
                json!({ "url": "http://localhost:3000", "direction": "down" }),
            ),
            Some(5),
            true,
        );
        assert_eq!(
            requests,
            vec![Request::ChromeOpen {
                url: "http://localhost:3000".into(),
                pane: Some(5),
                direction: Some(Direction::Down),
            }]
        );
        // url 欠落はエラー
        let (response, requests) = run(call("tako_chrome_open", json!({})), Some(5), true);
        assert!(requests.is_empty());
        assert!(response.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("url"));
        // 不正な direction はエラー
        let (response, requests) = run(
            call(
                "tako_chrome_open",
                json!({ "url": "http://localhost:3000", "direction": "diagonal" }),
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
    }
}
