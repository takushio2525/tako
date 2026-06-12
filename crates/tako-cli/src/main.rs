//! tako — Layer 1 CLI（FR-2.2）
//!
//! `TAKO_SOCKET` + `TAKO_TOKEN` を読んで IPC サーバーへ JSON-RPC で接続する。
//! `--pane` 省略時は `TAKO_PANE_ID`（呼び出し元ペイン）を対象にする（FR-2.2.7）。
//! tako の外で実行された場合は明確なエラーを返す（FR-2.2.8）。
//!
//! 操作セットは `tako_control::protocol::Request`（FR-2.5）と 1:1。
//! `tako mcp serve` は Layer 2 の MCP stdio ブリッジ（FR-2.3）として動き、
//! エージェントの MCP クライアントから起動される（mcp_serve のコメント参照）。
//! シェルスクリプトから使う例:
//!
//! ```sh
//! worker=$(tako split --down -- claude -p "テストを直して")
//! tako title --pane "$worker" --role worker-1 修復係
//! tako read --pane "$worker" --lines 20
//! ```

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::Value;
use tako_control::protocol::{Axis, Direction, Request};

/// tako の外で実行されたときのエラー（FR-2.2.8）。
/// 接続情報は環境変数 → 発見ファイル（FR-2.2.9）の順で解決した上での不在を意味する
const OUTSIDE_TAKO: &str = "tako アプリへの接続情報が無い（TAKO_SOCKET / TAKO_TOKEN 未設定・\
    接続情報ファイルも無し）。tako アプリを起動するか、tako 内のターミナルで実行してください";

#[derive(Parser)]
#[command(
    name = "tako",
    about = "tako アプリのペイン・タブを外から操作する CLI（Layer 1）",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 対象ペインの隣に新ペインを生やす（既定は右）。新ペイン ID を出力する
    Split(SplitArgs),
    /// ペインへテキストを送信する（既定で末尾に改行を付与）
    Send(SendArgs),
    /// ペインへフォーカスを移す（ID 指定または --left 等の方向指定）
    Focus(FocusArgs),
    /// タブ / ペインのツリー構造・ジオメトリ・状態を JSON で出力する
    List,
    /// ペインの画面内容をテキストで出力する
    Read(ReadArgs),
    /// スクロールバック表示を動かす（--to 0 で最下部へ）
    Scroll(ScrollArgs),
    /// ペインを閉じる（タブ最後の 1 ペインならタブごと閉じる）
    Close(CloseArgs),
    /// ペインのタイトル・役割ラベルを設定する（空文字でクリア）
    Title(TitleArgs),
    /// ペインの取り分を調整する（--dx/--dy は相対、--share-x/--share-y は絶対指定）
    Resize(ResizeArgs),
    /// タブ内の全ペインのサイズを均等化する
    Equalize(EqualizeArgs),
    /// タブ操作（new / rename / select / move-pane）
    #[command(subcommand)]
    Tab(TabCommand),
    /// タブ・ペイン名の AI 自動リネーム（FR-2.12）の ON/OFF・状態確認
    Autorename(ToggleArgs),
    /// listen ポート検知 + 提案チップ（FR-2.4.2〜2.4.4）の ON/OFF・状態確認
    Portdetect(ToggleArgs),
    /// セッション永続化 = tmux バックエンド（FR-5）の ON/OFF・状態確認。
    /// 有効時、tako を再起動してもタブ構成と実行中プロセスが復元される
    Persist(ToggleArgs),
    /// 右サイドバー情報パネル（tmux 一覧 / agents 集約センター）の表示・幅・ビュー切替。
    /// 引数なしで現在状態を表示する
    Panel(PanelArgs),
    /// tmux セッションの一覧・kill（FR-2.13。消し忘れ tmux の発見と片付け）
    #[command(subcommand)]
    Tmux(TmuxCommand),
    /// MCP 連携（serve = stdio ブリッジ。エージェントの MCP クライアントが起動する）
    #[command(subcommand)]
    Mcp(McpCommand),
}

#[derive(Subcommand)]
enum TmuxCommand {
    /// 全 tmux セッションを JSON で一覧する（tako ペインとの対応付け込み）
    List {
        /// tmux サーバー名（`tmux -L` 相当。省略時は既定サーバー）
        #[arg(long)]
        socket: Option<String>,
    },
    /// セッション（--window 指定時はその window）を kill する。確認なしで即実行されるため
    /// 対象は `tako tmux list` で確認してから指定すること
    Kill {
        /// 対象セッション名
        #[arg(long)]
        session: String,
        /// window index（指定時は kill-window、省略時は kill-session）
        #[arg(long)]
        window: Option<u32>,
        /// tmux サーバー名（`tmux -L` 相当）
        #[arg(long)]
        socket: Option<String>,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    /// stdio で MCP サーバーとして動き、操作を tako アプリへ中継する。
    /// Claude Code には 1 回だけ `claude mcp add --scope user tako -- tako mcp serve` で登録すると、
    /// 以後 tako 内のどのペインからでも設定なしでペイン操作ツールが使える
    /// （接続情報は起動毎に TAKO_SOCKET / TAKO_TOKEN / TAKO_PANE_ID から読む）。
    /// tako の外ではツールを公開しない（無害に 0 ツールで応答する）
    Serve,
}

#[derive(Args)]
struct SplitArgs {
    /// 対象ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID）
    #[arg(long)]
    pane: Option<u64>,
    /// 右に分割（既定）
    #[arg(long, conflicts_with_all = ["down", "up", "left"])]
    right: bool,
    /// 下に分割
    #[arg(long, conflicts_with_all = ["right", "up", "left"])]
    down: bool,
    /// 上に分割
    #[arg(long, conflicts_with_all = ["right", "down", "left"])]
    up: bool,
    /// 左に分割
    #[arg(long, conflicts_with_all = ["right", "down", "up"])]
    left: bool,
    /// 新ペイン側の取り分（0.0–1.0、省略時は等分）
    #[arg(long)]
    ratio: Option<f32>,
    /// 新ペインの作業ディレクトリ
    #[arg(long)]
    cwd: Option<String>,
    /// シェルの代わりに実行するコマンド（`--` の後に指定）
    #[arg(last = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct SendArgs {
    /// 送信先ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 末尾に改行を付けない（プロンプトへの部分入力などに使う）
    #[arg(long)]
    no_newline: bool,
    /// 送信するテキスト（複数引数はスペース連結）
    #[arg(required = true)]
    text: Vec<String>,
}

#[derive(Args)]
struct FocusArgs {
    /// フォーカス先ペイン ID
    pane: Option<u64>,
    /// 左の隣接ペインへ
    #[arg(long, conflicts_with_all = ["right", "up", "down"])]
    left: bool,
    /// 右の隣接ペインへ
    #[arg(long, conflicts_with_all = ["left", "up", "down"])]
    right: bool,
    /// 上の隣接ペインへ
    #[arg(long, conflicts_with_all = ["left", "right", "down"])]
    up: bool,
    /// 下の隣接ペインへ
    #[arg(long, conflicts_with_all = ["left", "right", "up"])]
    down: bool,
}

#[derive(Args)]
struct ReadArgs {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 末尾からの行数制限
    #[arg(long)]
    lines: Option<usize>,
}

#[derive(Args)]
struct ScrollArgs {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 絶対位置（0 = 最下部、大きいほど過去）
    #[arg(long, conflicts_with = "delta")]
    to: Option<u64>,
    /// 相対行数（正 = 過去方向）
    #[arg(long, allow_hyphen_values = true)]
    delta: Option<i32>,
}

#[derive(Args)]
struct CloseArgs {
    /// 対象ペイン ID（省略時は呼び出し元 = 自己片付け）
    #[arg(long)]
    pane: Option<u64>,
}

#[derive(Args)]
struct TitleArgs {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 役割ラベル（例: worker-1, dev-server）
    #[arg(long)]
    role: Option<String>,
    /// 表示タイトル
    title: Option<String>,
}

#[derive(Args)]
struct ResizeArgs {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 横の取り分を相対変更（例: 0.1 / -0.1）
    #[arg(long, allow_hyphen_values = true)]
    dx: Option<f32>,
    /// 縦の取り分を相対変更
    #[arg(long, allow_hyphen_values = true)]
    dy: Option<f32>,
    /// 横の取り分を絶対指定（0.0–1.0）
    #[arg(long)]
    share_x: Option<f32>,
    /// 縦の取り分を絶対指定（0.0–1.0）
    #[arg(long)]
    share_y: Option<f32>,
}

#[derive(Args)]
struct PanelArgs {
    /// パネルを表示する
    #[arg(long, conflicts_with = "hide")]
    show: bool,
    /// パネルを隠す
    #[arg(long)]
    hide: bool,
    /// パネル幅（px）
    #[arg(long)]
    width: Option<f32>,
    /// 表示するビュー
    #[arg(long, value_parser = ["tmux", "git"])]
    view: Option<String>,
    /// 左サイドバーのファイルツリー表示（FR-2.16.5。on = 表示、off = 非表示）
    #[arg(long, value_parser = ["on", "off"])]
    filetree: Option<String>,
}

/// ON/OFF トグル系コマンド共通の引数（autorename / portdetect）
#[derive(Args)]
struct ToggleArgs {
    /// on = 有効化、off = 無効化（省略時は現在状態を表示）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

#[derive(Args)]
struct EqualizeArgs {
    /// 対象タブ ID（省略時は呼び出し元ペインの属するタブ）
    #[arg(long)]
    tab: Option<u64>,
}

#[derive(Subcommand)]
enum TabCommand {
    /// 新しいタブを作る。{"tab":N,"pane":M} を出力する
    New {
        /// タブのタイトル（省略時は連番）
        #[arg(long)]
        title: Option<String>,
    },
    /// タブの表示タイトルを変える（明示リネーム = 自動リネームより優先。空文字で解除）
    Rename {
        /// 対象タブ ID（省略時は呼び出し元ペインの属するタブ）
        #[arg(long)]
        tab: Option<u64>,
        /// 新しいタイトル（複数引数はスペース連結。空文字で手動指定を解除）
        title: Vec<String>,
    },
    /// タブを切り替える
    Select { tab: u64 },
    /// ペインを別タブへ移送する
    MovePane {
        /// 移送先タブ ID
        tab: u64,
        /// 対象ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Mcp(McpCommand::Serve) => mcp_serve(),
        command => run(command),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// MCP stdio ブリッジ（FR-2.3.2 のゼロコンフィグ接続を成立させる実体）。
/// 1 行 1 JSON の MCP メッセージを stdin から読み、プロトコル処理は
/// `tako_control::mcp`（HTTP トランスポートと共有）に任せ、操作の実行だけ
/// IPC へ origin="mcp" で中継する。呼び出し元ペインは環境変数から特定する
fn mcp_serve() -> Result<(), String> {
    use std::io::{BufRead, Write};

    // ツール公開の判定は**環境変数のみ**で行う（発見ファイルは見ない）。
    // tako の外で起動された Claude セッションへツールを公開しない方針（FR-2.3.2 の
    // 「tako 外で 0 ツール」）を保つため。tako 内で起動された長寿命ブリッジが
    // アプリ再起動で stale になった場合のみ、exec 時にファイルへフォールバックする
    let connected = matches!(
        (std::env::var("TAKO_SOCKET"), std::env::var("TAKO_TOKEN")),
        (Ok(s), Ok(t)) if !s.is_empty() && !t.is_empty()
    );
    let caller = caller_pane();

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin の読み取りに失敗: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(message) => {
                let mut exec = |request: Request| -> Result<Value, String> {
                    if connected {
                        send_request_via(request, Some("mcp"))
                    } else {
                        Err(OUTSIDE_TAKO.into())
                    }
                };
                let mut session = tako_control::mcp::McpSession {
                    caller_pane: caller,
                    connected,
                    exec: &mut exec,
                };
                tako_control::mcp::handle_message(&message, &mut session)
            }
            Err(e) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": format!("JSON として解釈できない: {e}") },
            })),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}")
                .map_err(|e| format!("stdout への書き込みに失敗: {e}"))?;
            stdout
                .flush()
                .map_err(|e| format!("stdout の flush に失敗: {e}"))?;
        }
    }
    Ok(())
}

fn run(command: Command) -> Result<(), String> {
    let request = build_request(&command)?;
    let result = send_request(request)?;
    print_result(&command, &result);
    Ok(())
}

/// `TAKO_PANE_ID`（呼び出し元ペイン）。tako 内のシェルなら必ず入っている（FR-2.1.1）
fn caller_pane() -> Option<u64> {
    std::env::var("TAKO_PANE_ID").ok()?.parse().ok()
}

/// `--pane` 指定が無ければ呼び出し元へフォールバックする（FR-2.2.7）
fn target_pane(explicit: Option<u64>) -> Result<Option<u64>, String> {
    explicit.or_else(caller_pane).map(Some).ok_or_else(|| {
        "対象ペインを特定できない（--pane を指定するか、tako アプリ内のターミナルで実行する）"
            .into()
    })
}

fn build_request(command: &Command) -> Result<Request, String> {
    Ok(match command {
        Command::Split(args) => {
            let direction = match (args.down, args.up, args.left) {
                (true, _, _) => Some(Direction::Down),
                (_, true, _) => Some(Direction::Up),
                (_, _, true) => Some(Direction::Left),
                _ => Some(Direction::Right),
            };
            Request::Split {
                pane: target_pane(args.pane)?,
                direction,
                ratio: args.ratio,
                command: (!args.command.is_empty()).then(|| args.command.clone()),
                cwd: args.cwd.clone(),
            }
        }
        Command::Send(args) => Request::Send {
            pane: target_pane(args.pane)?,
            text: args.text.join(" "),
            newline: !args.no_newline,
        },
        Command::Focus(args) => {
            let direction = match (args.left, args.right, args.up, args.down) {
                (true, _, _, _) => Some(Direction::Left),
                (_, true, _, _) => Some(Direction::Right),
                (_, _, true, _) => Some(Direction::Up),
                (_, _, _, true) => Some(Direction::Down),
                _ => None,
            };
            if direction.is_none() && args.pane.is_none() {
                return Err("フォーカス先のペイン ID か方向（--left 等）を指定する".into());
            }
            Request::Focus {
                pane: args.pane,
                direction,
            }
        }
        Command::List => Request::List,
        Command::Read(args) => Request::Read {
            pane: target_pane(args.pane)?,
            lines: args.lines,
        },
        Command::Scroll(args) => {
            if args.to.is_none() && args.delta.is_none() {
                return Err("--to（絶対位置。0 = 最下部）か --delta（相対行数）を指定する".into());
            }
            Request::Scroll {
                pane: target_pane(args.pane)?,
                to: args.to,
                delta: args.delta,
            }
        }
        Command::Close(args) => Request::Close {
            pane: target_pane(args.pane)?,
        },
        Command::Title(args) => Request::Title {
            pane: target_pane(args.pane)?,
            title: args.title.clone(),
            role: args.role.clone(),
        },
        Command::Resize(args) => {
            let (axis, delta, share) = match (args.dx, args.dy, args.share_x, args.share_y) {
                (Some(d), None, None, None) => (Axis::X, Some(d), None),
                (None, Some(d), None, None) => (Axis::Y, Some(d), None),
                (None, None, Some(s), None) => (Axis::X, None, Some(s)),
                (None, None, None, Some(s)) => (Axis::Y, None, Some(s)),
                _ => {
                    return Err(
                        "--dx / --dy / --share-x / --share-y のどれか 1 つを指定する".into(),
                    )
                }
            };
            Request::Resize {
                pane: target_pane(args.pane)?,
                axis,
                delta,
                share,
            }
        }
        Command::Equalize(args) => Request::Equalize {
            // --tab 指定があればそれを、無ければ呼び出し元ペインからタブを解決する
            pane: if args.tab.is_none() {
                target_pane(None)?
            } else {
                None
            },
            tab: args.tab,
        },
        Command::Tab(TabCommand::New { title }) => Request::TabNew {
            title: title.clone(),
        },
        Command::Tab(TabCommand::Rename { tab, title }) => Request::TabRename {
            // --tab 指定があればそれを、無ければ呼び出し元ペインからタブを解決する
            pane: if tab.is_none() {
                target_pane(None)?
            } else {
                None
            },
            tab: *tab,
            title: title.join(" "),
        },
        Command::Tab(TabCommand::Select { tab }) => Request::TabSelect { tab: *tab },
        Command::Tab(TabCommand::MovePane { tab, pane }) => Request::MovePane {
            pane: target_pane(*pane)?,
            tab: *tab,
        },
        Command::Autorename(args) => Request::AutoRename {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Persist(args) => Request::Persist {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Panel(args) => Request::Panel {
            visible: match (args.show, args.hide) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            },
            width: args.width,
            view: args.view.as_deref().map(|v| match v {
                "git" => tako_control::protocol::PanelViewWire::Git,
                _ => tako_control::protocol::PanelViewWire::Tmux,
            }),
            filetree: args.filetree.as_deref().map(|s| s == "on"),
        },
        Command::Portdetect(args) => Request::PortDetect {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Tmux(TmuxCommand::List { socket }) => Request::TmuxList {
            socket: socket.clone(),
        },
        Command::Tmux(TmuxCommand::Kill {
            session,
            window,
            socket,
        }) => Request::TmuxKill {
            socket: socket.clone(),
            session: session.clone(),
            window: *window,
        },
        // main() で mcp_serve() へ分岐済みのため論理的に到達不能
        Command::Mcp(_) => unreachable!("mcp serve は run() を通らない"),
    })
}

/// 環境変数から接続情報を読み、1 リクエストを往復させる
fn send_request(request: Request) -> Result<Value, String> {
    send_request_via(request, None)
}

/// 接続情報の解決とフォールバック（FR-2.2.9）。
/// ①環境変数（`TAKO_SOCKET` / `TAKO_TOKEN`）で試行し、接続不可・認証失敗
/// （= アプリ再起動で env が古い）なら ②発見ファイルの候補列（current →
/// 生きているインスタンス。`discovery::read_candidates`）を順に再試行する。
/// 一時インスタンス（セルフテスト・二重起動）が current を上書きして exit しても、
/// 生きているメインへ自動で届く（2026-06-12 バグ (8) の恒久対策）。
/// 操作エラーはフォールバックせずそのまま返す。どの情報源も無ければ「tako の外」
fn send_request_via(request: Request, origin: Option<&str>) -> Result<Value, String> {
    let env_pair = match (std::env::var("TAKO_SOCKET"), std::env::var("TAKO_TOKEN")) {
        (Ok(socket), Ok(token)) if !socket.is_empty() && !token.is_empty() => Some((socket, token)),
        _ => None,
    };
    let mut last_failure = None;
    if let Some((socket, token)) = &env_pair {
        match transport::roundtrip(socket, token, request.clone(), origin) {
            Ok(value) => return Ok(value),
            Err(TransportError::Other(message)) => return Err(message),
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
            Err(TransportError::Other(message)) => return Err(message),
            // 死んだ残骸・別インスタンスのトークン → 次の候補へ
            Err(stale) => last_failure = Some(stale),
        }
    }
    Err(match last_failure {
        Some(stale) => stale.message(),
        None => OUTSIDE_TAKO.to_string(),
    })
}

fn print_result(command: &Command, result: &Value) {
    match command {
        // 新ペイン ID をそのままスクリプトで使えるよう数値のみ出力する
        Command::Split(_) => {
            if let Some(pane) = result["pane"].as_u64() {
                println!("{pane}");
            }
        }
        Command::Read(_) => {
            if let Some(text) = result["text"].as_str() {
                println!("{text}");
            }
        }
        Command::Scroll(_) => println!("{result}"),
        Command::List => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).unwrap_or_default()
            );
        }
        Command::Tab(TabCommand::New { .. }) => println!("{result}"),
        Command::Autorename(_)
        | Command::Portdetect(_)
        | Command::Persist(_)
        | Command::Panel(_) => {
            println!("{result}")
        }
        Command::Tmux(TmuxCommand::List { .. }) => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).unwrap_or_default()
            );
        }
        Command::Tmux(TmuxCommand::Kill { .. }) => println!("{result}"),
        _ => {}
    }
}

/// 接続試行の失敗種別。Connect / Auth は「環境変数が古い」可能性があり、
/// 発見ファイルへのフォールバック対象になる（FR-2.2.9）
enum TransportError {
    /// 接続できない（ソケット不在・アプリ停止）
    Connect(String),
    /// 認証失敗（トークンが古い = 別インスタンスのもの）
    Auth(String),
    /// その他（操作エラー・プロトコルエラー。フォールバックしない）
    Other(String),
}

impl TransportError {
    fn message(self) -> String {
        match self {
            TransportError::Connect(m) | TransportError::Auth(m) | TransportError::Other(m) => m,
        }
    }
}

#[cfg(unix)]
mod transport {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

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
        let stream = UnixStream::connect(socket).map_err(|e| {
            TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
        })?;
        let mut writer = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        let mut envelope = RequestEnvelope::new(1, token, request);
        envelope.origin = origin.map(Into::into);
        let json = serde_json::to_string(&envelope)
            .map_err(|e| TransportError::Other(format!("送信の構築に失敗: {e}")))?;
        writeln!(writer, "{json}")
            .map_err(|e| TransportError::Other(format!("送信に失敗: {e}")))?;

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|e| TransportError::Other(format!("応答の受信に失敗: {e}")))?;
        if line.is_empty() {
            return Err(TransportError::Other(
                "tako アプリから応答が返らなかった".into(),
            ));
        }
        let response: ResponseEnvelope = serde_json::from_str(&line)
            .map_err(|e| TransportError::Other(format!("応答を解釈できない: {e}")))?;
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

#[cfg(windows)]
mod transport {
    //! TODO(Phase 6): named pipe での実装（`.agent/architecture.md`「IPC トランスポート」節）

    use serde_json::Value;
    use tako_control::protocol::Request;

    use super::TransportError;

    pub fn roundtrip(
        _socket: &str,
        _token: &str,
        _request: Request,
        _origin: Option<&str>,
    ) -> Result<Value, TransportError> {
        Err(TransportError::Other(
            "Windows の IPC（named pipe）は未実装（Phase 6 で対応予定）".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI 引数 → Request の対応（接続せずに検証できる範囲）
    fn parse(args: &[&str]) -> Command {
        Cli::try_parse_from(args)
            .expect("引数をパースできる")
            .command
    }

    #[test]
    fn 引数定義が壊れていない() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn splitの方向と末尾コマンド() {
        let command = parse(&[
            "tako", "split", "--down", "--pane", "3", "--ratio", "0.3", "--", "npm", "run", "dev",
        ]);
        let request = build_request(&command).unwrap();
        assert_eq!(
            request,
            Request::Split {
                pane: Some(3),
                direction: Some(Direction::Down),
                ratio: Some(0.3),
                command: Some(vec!["npm".into(), "run".into(), "dev".into()]),
                cwd: None,
            }
        );
    }

    #[test]
    fn sendはテキストを連結し改行は既定で付く() {
        let command = parse(&["tako", "send", "--pane", "2", "echo", "hello"]);
        let request = build_request(&command).unwrap();
        assert_eq!(
            request,
            Request::Send {
                pane: Some(2),
                text: "echo hello".into(),
                newline: true,
            }
        );
    }

    #[test]
    fn resizeは排他指定() {
        let command = parse(&["tako", "resize", "--pane", "2", "--dx", "-0.1"]);
        let request = build_request(&command).unwrap();
        assert_eq!(
            request,
            Request::Resize {
                pane: Some(2),
                axis: Axis::X,
                delta: Some(-0.1),
                share: None,
            }
        );
        let command = parse(&[
            "tako",
            "resize",
            "--pane",
            "2",
            "--dx",
            "0.1",
            "--share-y",
            "0.5",
        ]);
        assert!(build_request(&command).is_err());
    }

    #[test]
    fn focusは方向かidが必須() {
        let command = parse(&["tako", "focus", "--right"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::Focus {
                pane: None,
                direction: Some(Direction::Right),
            }
        );
        let command = parse(&["tako", "focus"]);
        assert!(build_request(&command).is_err());
    }

    #[test]
    fn tabサブコマンド() {
        let command = parse(&["tako", "tab", "move-pane", "4", "--pane", "9"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::MovePane {
                pane: Some(9),
                tab: 4,
            }
        );
        let command = parse(&["tako", "tab", "select", "2"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::TabSelect { tab: 2 }
        );
    }

    #[test]
    fn tab_renameはタイトルを連結しタブ指定を解釈する() {
        let command = parse(&["tako", "tab", "rename", "--tab", "3", "実験", "用"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::TabRename {
                pane: None,
                tab: Some(3),
                title: "実験 用".into(),
            }
        );
    }
}
