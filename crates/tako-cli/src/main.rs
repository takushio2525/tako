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

mod setup;

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
    /// ペインへテキストを送信する（既定で末尾に改行を付与）。claude 等の全画面 TUI へは
    /// 送達確認ループ（貼り付け → 分離 Enter → 入力欄の空検証 + 再送）で配送する
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
    /// ファイルをプレビューペインで開く（コード = ハイライト表示、
    /// .md は既定でレンダリング表示。--mode code でソース表示へ切替）
    Open(OpenArgs),
    /// タブ操作（new / rename / select / move-pane）
    #[command(subcommand)]
    Tab(TabCommand),
    /// タブ・ペイン名の AI 自動リネームの ON/OFF・状態確認
    Autorename(ToggleArgs),
    /// listen ポート検知 + 提案チップの ON/OFF・状態確認
    Portdetect(ToggleArgs),
    /// セッション永続化（tmux バックエンド）の ON/OFF・状態確認。
    /// 有効時、tako を再起動してもタブ構成と実行中プロセスが復元される
    Persist(ToggleArgs),
    /// 右サイドバー情報パネル（tmux 一覧 / agents 集約センター）の表示・幅・ビュー切替。
    /// 引数なしで現在状態を表示する
    Panel(PanelArgs),
    /// サイドバー tmux ビューのタブ枠を折りたたむ / 展開する
    /// （配下のバックグラウンド行 + バックグラウンドを隠し、前面表示中の行は残す）
    Collapse(CollapseArgs),
    /// プレビューをピン留め / 解除する（バックグラウンドペイン / 閉じたタブグループの
    /// 実画面をアプリ内フローティングウィンドウとして常駐・ライブ更新させる）
    Pin(PinArgs),
    /// ペインをバックグラウンドへ送る（プロセスは生きたまま画面から外す）
    #[command(name = "background")]
    Background(BackgroundArgs),
    /// バックグラウンドのペインを画面に復帰させる
    #[command(name = "foreground")]
    Foreground(ForegroundArgs),
    /// バックグラウンドのペイン一覧を JSON で出力する
    #[command(name = "backgrounded")]
    BackgroundList,
    /// ファイル操作（パスコピー / Finder 表示 / cd / リネーム / 作成 / ゴミ箱）
    #[command(subcommand)]
    File(FileCommand),
    /// git リポジトリ情報の取得（コミット履歴 / diff）
    #[command(subcommand)]
    Git(GitCommand),
    /// tmux セッションの一覧・kill・取り込み（消し忘れ tmux の発見と片付け）
    #[command(subcommand)]
    Tmux(TmuxCommand),
    /// MCP 連携（serve = stdio ブリッジ。エージェントの MCP クライアントが起動する）
    #[command(subcommand)]
    Mcp(McpCommand),
    /// 対話式セットアップ。claude と対話しながら環境を最適化する。
    /// アプリ未起動でも実行できる
    Setup(SetupArgs),
    /// Claude Code の settings.json に tako MCP サーバーの接続設定を追加する。
    /// アプリ未起動でも実行できる（settings.json の書き換えのみ）
    SetupMcp(SetupMcpArgs),
    /// 動画操作（play / pause / seek。プレビューペインが動画モードの場合のみ有効）
    #[command(subcommand)]
    Video(VideoCommand),
    /// リモートアクセス API サーバーの操作（start / stop / status）
    #[command(subcommand)]
    Remote(RemoteCommand),
    /// マスターオーケストレーターを起動する。新タブで claude を master system prompt 付きで起動する。
    /// プロファイル名を指定して設定を切り替えられる（例: tako master -2 → "2" プロファイル）。
    /// 引数なしは default プロファイル。旧形式（tako master dev）も後方互換で動作する
    Master {
        /// プロファイル名（-2, -difficult 等）またはサフィックス（旧形式: dev 等）
        #[arg(allow_hyphen_values = true)]
        profile: Option<String>,
    },
    /// オーケストレーター操作（projects / spawn / status / watch）
    #[command(subcommand)]
    Orchestrator(OrchestratorCommand),
    /// URL を Chrome CDP ミラー方式で Web ビューペインとして開く（FR-3.8 PoC）
    #[command(subcommand)]
    Chrome(ChromeCommand),
    /// アプリ内更新の診断・チェック・実行（Issue #36）。
    /// 引数なしで配布系統・現在バージョン・重複 CLI を表示する
    #[command(subcommand)]
    Update(UpdateCommand),
}

#[derive(Subcommand)]
enum ChromeCommand {
    /// URL を Chrome Web ビューペインで開く
    Open {
        /// 開く URL
        url: String,
        /// 基準ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
        /// 右に分割
        #[arg(long)]
        right: bool,
        /// 下に分割
        #[arg(long)]
        down: bool,
        /// 左に分割
        #[arg(long)]
        left: bool,
        /// 上に分割
        #[arg(long)]
        up: bool,
    },
}

#[derive(Subcommand)]
enum UpdateCommand {
    /// 配布系統・現在バージョン・重複 CLI の診断情報を表示する
    Status,
    /// GitHub Releases から最新版の有無を確認する（更新は行わない）
    Check,
    /// 配布系統に応じた更新を実行する
    Apply,
    /// zip 経由で強制更新する（brew 失敗時のフォールバック）
    ApplyZip,
    /// broken-brew 状態の修復（brew install --cask --force で台帳を再締結）
    Repair,
}

#[derive(Subcommand)]
enum RemoteCommand {
    /// リモートアクセス API サーバーを起動し、QR コードを表示する
    Start {
        /// サーバーのポート番号（省略時は 7749）
        #[arg(long, default_value_t = 7749)]
        port: u16,
        /// cloudflared Quick Tunnel を起動しない（LAN のみモード）
        #[arg(long)]
        no_tunnel: bool,
    },
    /// リモートアクセス API サーバーを停止する
    Stop,
    /// リモートアクセス API サーバーの状態を表示する
    Status,
    /// エージェント一覧を表示する（claude agents --json + tmux ペイン対応付け）
    Agents,
    /// Claude Code の会話ログ（transcript）の末尾を正規化 JSON で表示する
    Messages {
        /// 対象セッション ID（claude の sessionId。`tako remote agents` で確認できる）
        session_id: String,
        /// 取得する末尾件数（省略時は 30）
        #[arg(long, default_value_t = 30)]
        tail: usize,
    },
    /// ペインのスクロールバック履歴をプレーンテキストで表示する
    Scrollback {
        /// 対象ペイン ID（session:window.pane）
        pane_id: String,
        /// 取得する履歴行数（省略時は 1000）
        #[arg(long, default_value_t = 1000)]
        lines: u32,
    },
    /// [内部用] HTTP サーバーをフォアグラウンドで起動する（start から自動呼び出し）
    Serve {
        /// サーバーのポート番号（省略時は 7749）
        #[arg(long, default_value_t = 7749)]
        port: u16,
        /// cloudflared Quick Tunnel を起動しない（LAN のみモード）
        #[arg(long)]
        no_tunnel: bool,
    },
}

#[derive(Subcommand)]
enum GitCommand {
    /// コミット履歴・ブランチ一覧・変更状態を JSON で出力する
    Log {
        /// 取得するコミット数上限（省略時 200）
        #[arg(long, default_value_t = 200)]
        max_count: usize,
        /// 対象ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
    },
    /// git diff をファイル・ハンク・行単位の JSON で出力する
    Diff {
        /// diff 種別: unstaged（既定）/ staged / コミットハッシュ
        #[arg(long)]
        target: Option<String>,
        /// 対象ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
    },
}

#[derive(Subcommand)]
enum TmuxCommand {
    /// 全 tmux セッションを JSON で一覧する（tako ペインとの対応付け込み）
    List {
        /// tmux サーバー名（`tmux -L` 相当。省略時は既定サーバー）
        #[arg(long)]
        socket: Option<String>,
    },
    /// 取り残された orphan tmux セッションを一括クリーンアップする（FR-2.16.11）。
    /// detached・非 grouped・未使用の `tako-` バックエンドセッションだけを kill する
    /// （使用中・ユーザーのセッションには触れない）。kill した名前を JSON で返す
    Cleanup {
        /// tmux サーバー名（`tmux -L` 相当。省略時は tako バックエンドサーバー）
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
    /// window を指定サイズへリサイズする（スマホリモートのビューポート連動用）。
    /// tmux の window-size が manual になるため、戻すときは --reset を使う
    Resize {
        /// 対象セッション名
        #[arg(long)]
        session: String,
        /// window index（省略時は 0）
        #[arg(long, default_value_t = 0)]
        window: u32,
        /// 幅（桁数）。--reset なしなら --rows と併せて必須
        #[arg(long)]
        cols: Option<u32>,
        /// 高さ（行数）。--reset なしなら --cols と併せて必須
        #[arg(long)]
        rows: Option<u32>,
        /// manual サイズを解除してサーバー既定へ戻す
        #[arg(long)]
        reset: bool,
        /// tmux サーバー名（`tmux -L` 相当）
        #[arg(long)]
        socket: Option<String>,
    },
    /// バックエンドセッションのアクティブ window を切り替える
    SelectWindow {
        /// 切り替え先の window index
        window: u32,
        /// 対象ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
    },
    /// セッションを現在のタブへ取り込んで表示する。
    /// 対象ペインを分割した新ペインで attach クライアントを起動する。
    /// 新ペインを閉じてもセッションは残る（kill ではない）
    Open {
        /// 対象セッション名
        session: String,
        /// tmux サーバー名（`tmux -L` 相当。`tako tmux list` の socket をそのまま渡す）
        #[arg(long)]
        socket: Option<String>,
        /// 分割の基準ペイン ID（省略時は呼び出し元）
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
    },
}

#[derive(Subcommand)]
enum FileCommand {
    /// ファイルの絶対パスを出力する（--relative でペイン cwd 基準の相対パス）
    CopyPath {
        path: String,
        #[arg(long)]
        relative: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// Finder でファイルの場所を表示する（macOS のみ）
    Reveal { path: String },
    /// 指定パスのディレクトリへペイン内で cd する
    OpenTerminal {
        path: String,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ファイル・フォルダの名前を変更する
    Rename { path: String, name: String },
    /// 新しいファイルを作成する（path 配下に name で作成）
    Create { path: String, name: String },
    /// 新しいフォルダを作成する（path 配下に name で作成）
    Mkdir { path: String, name: String },
    /// ファイル・フォルダをゴミ箱へ移動する
    Trash { path: String },
}

#[derive(Subcommand)]
enum VideoCommand {
    /// 動画の再生を開始する
    Play {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 動画の一時停止
    Pause {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 動画の再生/一時停止トグル
    Toggle {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 動画のシーク（秒単位）
    Seek {
        /// シーク先の秒数
        seconds: f64,
        #[arg(long)]
        pane: Option<u64>,
    },
}

#[derive(Subcommand)]
enum OrchestratorCommand {
    /// worker が完了（idle）または消滅（gone）するまでブロックし、結果を 1 行出力する。
    /// Monitor ツールから呼ばれる想定。出力形式: WORKER_IDLE / WORKER_GONE
    Watch {
        /// 監視対象ペイン ID（位置引数または --pane で指定）
        #[arg(long)]
        pane: Option<u64>,
        /// 監視対象ペイン ID（位置引数）
        #[arg(value_name = "PANE_ID")]
        pane_pos: Option<u64>,
        /// claude の session ID（あれば精度向上）
        #[arg(long)]
        session_id: Option<String>,
        /// tmux session 名（pane 消滅時のフォールバック追跡）
        #[arg(long)]
        tmux_session: Option<String>,
        /// タイムアウト秒数（省略時は無制限）
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// プロジェクト管理（一覧 / 追加 / 削除）
    #[command(subcommand)]
    Projects(ProjectsCommand),
    /// プロファイル管理（一覧 / 表示 / 設定）
    #[command(subcommand)]
    Profiles(ProfilesCommand),
    /// 子 worker を spawn する（split + claude 起動 + プロンプト送信）
    Spawn {
        /// プロジェクトキー（projects.yaml に登録済み）
        #[arg(long)]
        project: String,
        /// worker に渡す初期プロンプト
        #[arg(long)]
        prompt: String,
        /// ペインタイトルに付けるラベル
        #[arg(long)]
        label: Option<String>,
        /// claude のモデル（省略時は master のプロファイル設定 → 未設定なら claude 既定）
        #[arg(long)]
        model: Option<String>,
        /// thinking effort（省略時は master のプロファイル設定）
        #[arg(long)]
        effort: Option<String>,
        /// 分割元ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID。tab と両方指定時は pane を優先）
        #[arg(long)]
        pane: Option<u64>,
        /// 子を出すタブ ID（そのタブのフォーカスペインを分割元にする）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// worker の状態確認
    Status {
        /// ペイン ID
        #[arg(long)]
        pane: u64,
        /// claude の session ID
        #[arg(long)]
        session_id: Option<String>,
        /// tmux session 名（pane 消滅時のフォールバック追跡）
        #[arg(long)]
        tmux_session: Option<String>,
    },
    /// spawn + 完了待ち + 出力取得 + close を 1 回で行う
    Run {
        /// プロジェクトキー（projects.yaml に登録済み）
        #[arg(long)]
        project: String,
        /// worker に渡すプロンプト
        #[arg(long)]
        prompt: String,
        /// ペインタイトルに付けるラベル
        #[arg(long)]
        label: Option<String>,
        /// 分割元ペイン ID
        #[arg(long)]
        pane: Option<u64>,
        /// 子を出すタブ ID
        #[arg(long)]
        tab: Option<u64>,
        /// 完了待ちタイムアウト秒数（省略時 1800）
        #[arg(long, default_value = "1800")]
        timeout: u64,
        /// 完了後にペインを自動 close するか（省略時 true）
        #[arg(long, default_value = "true")]
        auto_close: bool,
        /// 返す出力の末尾行数（省略時 200）
        #[arg(long, default_value = "200")]
        output_lines: usize,
    },
}

#[derive(Subcommand)]
enum ProjectsCommand {
    /// 登録済みプロジェクトの一覧
    List,
    /// プロジェクトを追加する
    Add {
        /// プロジェクトキー
        #[arg(long)]
        key: String,
        /// 作業ディレクトリ（~ は $HOME に展開される）
        #[arg(long)]
        cwd: String,
        /// プロジェクトの説明
        #[arg(long)]
        description: Option<String>,
    },
    /// プロジェクトを削除する
    Remove {
        /// プロジェクトキー
        #[arg(long)]
        key: String,
    },
}

#[derive(Subcommand)]
enum ProfilesCommand {
    /// プロファイルの一覧（model が null のものは claude CLI の既定モデルで起動する）
    List,
    /// プロファイルの内容と解決結果を表示する
    Show {
        /// プロファイル名（省略時 default）
        name: Option<String>,
    },
    /// プロファイルを作成・更新する。[1m] 付きモデルは Max / API プラン限定なので注意
    Set {
        /// プロファイル名
        name: String,
        /// master のモデル（--clear-model と排他）
        #[arg(long, conflicts_with = "clear_model")]
        model: Option<String>,
        /// master のモデル指定を解除して claude 既定に戻す
        #[arg(long)]
        clear_model: bool,
        /// worker_model_policy=fixed 時の子 worker モデル（--clear-worker-model と排他）
        #[arg(long, conflicts_with = "clear_worker_model")]
        worker_model: Option<String>,
        /// 子 worker のモデル指定を解除する
        #[arg(long)]
        clear_worker_model: bool,
        /// master の thinking effort
        #[arg(long)]
        effort: Option<String>,
        /// 子 worker の thinking effort
        #[arg(long)]
        worker_effort: Option<String>,
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
    /// 対象ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID。--tab と排他）
    #[arg(long, conflicts_with = "tab")]
    pane: Option<u64>,
    /// 分割先タブ ID（そのタブのフォーカス中ペインの隣に分割。--pane と排他）
    #[arg(long)]
    tab: Option<u64>,
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
    /// 新ペインにフォーカスを移す（省略時は分割元を維持）
    #[arg(long)]
    focus: bool,
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
    /// tmux session 名（pane ID 解決不能時のフォールバック）
    #[arg(long)]
    tmux_session: Option<String>,
    /// claude TUI の起動（❯ プロンプト表示）を待ってから送信する（信頼ダイアログは自動承諾）
    #[arg(long)]
    await_prompt: bool,
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
    /// tmux session 名（pane ID 解決不能時のフォールバック）
    #[arg(long)]
    tmux_session: Option<String>,
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
    /// busy な worker でも強制的に close する
    #[arg(long)]
    force: bool,
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
struct OpenArgs {
    /// 開くファイルのパス（相対パスは対象ペインの cwd 基準で解決される）
    path: String,
    /// 基準ペイン ID（省略時は呼び出し元。プレビューの表示先解決に使う）
    #[arg(long)]
    pane: Option<u64>,
    /// 表示モード（省略時は拡張子から自動判定。md = markdown の別名）
    #[arg(long, value_parser = ["code", "markdown", "md", "image", "pdf", "video"])]
    mode: Option<String>,
    /// 既存プレビューを再利用せず右に分割して開く（FR-3.11 = D&D のドロップ位置相当）
    #[arg(long, conflicts_with_all = ["down", "up", "left"])]
    right: bool,
    /// 同・下に分割して開く
    #[arg(long, conflicts_with_all = ["right", "up", "left"])]
    down: bool,
    /// 同・上に分割して開く
    #[arg(long, conflicts_with_all = ["right", "down", "left"])]
    up: bool,
    /// 同・左に分割して開く
    #[arg(long, conflicts_with_all = ["right", "down", "up"])]
    left: bool,
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
struct BackgroundArgs {
    /// バックグラウンドへ送るペイン ID（省略時は呼び出し元。TAKO_PANE_ID から自動解決）
    #[arg(long)]
    pane: Option<u64>,
}

#[derive(Args)]
struct CollapseArgs {
    /// 対象タブ ID（省略時は呼び出し元ペインのタブ）
    #[arg(long)]
    tab: Option<u64>,
    /// on = 折りたたむ、off = 展開（省略時はトグル）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

#[derive(Args)]
struct PinArgs {
    /// ピン留めするペイン ID（省略時は呼び出し元。--group-tab と排他）
    #[arg(long)]
    pane: Option<u64>,
    /// 閉じたタブグループの由来タブ ID（--pane と排他）
    #[arg(long)]
    group_tab: Option<u64>,
    /// on = ピン留め、off = 解除（省略時はトグル）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

#[derive(Args)]
struct ForegroundArgs {
    /// 復帰させるペインの ID（tako backgrounded で確認）
    pane: u64,
    /// 挿入先ペインの ID（省略時は由来タブ。閉じていればアクティブタブ）
    #[arg(long)]
    target: Option<u64>,
    /// 分割方向（right / down / left / up。省略時は right）
    #[arg(long)]
    direction: Option<String>,
}

#[derive(Args)]
struct SetupArgs {
    /// 環境チェックだけ実行して終了する
    #[arg(long)]
    check: bool,
    /// セットアップ状態をリセットして初回扱いに戻す
    #[arg(long, conflicts_with = "check")]
    reset: bool,
}

#[derive(Args)]
struct SetupMcpArgs {
    /// ~/.claude/settings.json（ユーザーグローバル）に書き込む（既定）
    #[arg(long, conflicts_with = "project")]
    global: bool,
    /// カレントディレクトリの .claude/settings.json に書き込む
    #[arg(long)]
    project: bool,
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
    /// ペインを移動する: タブ ID 指定 = 別タブの末尾へ、--target 指定 = そのペインの
    /// 隣（--right 等の方向）へ挿し直す（FR-1.10 = タイトルバー D&D の同等操作）
    MovePane {
        /// 移送先タブ ID（--target と排他）
        tab: Option<u64>,
        /// 挿入先ペイン ID（このペインの隣に入る。同タブ内の並べ替えに使う）
        #[arg(long, conflicts_with = "tab")]
        target: Option<u64>,
        /// 対象ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
        /// --target の右に入る（既定）
        #[arg(long, conflicts_with_all = ["down", "up", "left"])]
        right: bool,
        /// --target の下に入る
        #[arg(long, conflicts_with_all = ["right", "up", "left"])]
        down: bool,
        /// --target の上に入る
        #[arg(long, conflicts_with_all = ["right", "down", "left"])]
        up: bool,
        /// --target の左に入る
        #[arg(long, conflicts_with_all = ["right", "down", "up"])]
        left: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Mcp(McpCommand::Serve) => mcp_serve(),
        Command::Setup(ref args) => {
            if args.check {
                setup::run_check()
            } else if args.reset {
                setup::run_reset().and_then(|()| setup::run_setup())
            } else {
                setup::run_setup()
            }
        }
        Command::SetupMcp(ref args) => setup_mcp_local(args),
        Command::Master { ref profile } => orchestrator_master(profile.as_deref()),
        Command::Orchestrator(OrchestratorCommand::Watch {
            pane,
            pane_pos,
            ref session_id,
            ref tmux_session,
            timeout,
        }) => {
            let resolved = pane.or(pane_pos).ok_or_else(|| {
                "ペイン ID を指定してください（tako orchestrator watch <PANE_ID> または --pane <N>）".to_string()
            });
            match resolved {
                Ok(p) => {
                    orchestrator_watch(p, session_id.as_deref(), tmux_session.as_deref(), timeout)
                }
                Err(e) => Err(e),
            }
        }
        Command::Orchestrator(OrchestratorCommand::Projects(ref sub)) => {
            orchestrator_projects_cli(sub)
        }
        Command::Orchestrator(OrchestratorCommand::Profiles(ref sub)) => {
            orchestrator_profiles_cli(sub)
        }
        Command::Orchestrator(OrchestratorCommand::Run {
            ref project,
            ref prompt,
            ref label,
            pane,
            tab,
            timeout,
            auto_close,
            output_lines,
        }) => orchestrator_run(
            project,
            prompt,
            label.as_deref(),
            pane,
            tab,
            timeout,
            auto_close,
            output_lines,
        ),
        // remote コマンドはローカル処理（IPC 不要）
        Command::Remote(RemoteCommand::Start { port, no_tunnel }) => remote_start(port, no_tunnel),
        Command::Remote(RemoteCommand::Stop) => remote_stop(),
        Command::Remote(RemoteCommand::Status) => remote_status(),
        Command::Remote(RemoteCommand::Serve { port, no_tunnel }) => remote_serve(port, no_tunnel),
        Command::Remote(RemoteCommand::Agents) => remote_agents(),
        Command::Remote(RemoteCommand::Messages { session_id, tail }) => {
            remote_messages(&session_id, tail)
        }
        Command::Remote(RemoteCommand::Scrollback { pane_id, lines }) => {
            remote_scrollback(&pane_id, lines)
        }
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

/// MCP セットアップ（アプリ未起動でも動作）。settings.json に tako MCP 設定を追加する
fn setup_mcp_local(args: &SetupMcpArgs) -> Result<(), String> {
    let tako_bin = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| tako_control::dispatch::resolve_tako_binary());
    let settings_dir = if args.project {
        std::env::current_dir()
            .map_err(|e| format!("カレントディレクトリの取得に失敗: {e}"))?
            .join(".claude")
    } else {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .ok_or("ホームディレクトリが取得できない（$HOME 未設定）")?
            .join(".claude")
    };
    let settings_path = settings_dir.join("settings.json");
    match tako_control::dispatch::setup_mcp_settings(&tako_bin, &settings_path) {
        Ok(result) => {
            if result.already_existed {
                eprintln!("既に設定されています: {}", settings_path.display());
            } else {
                eprintln!("設定を追加しました: {}", settings_path.display());
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `tako master [-profile]` — 新タブで claude をマスター system prompt 付きで起動する。
/// `-<名前>` でプロファイルを指定、引数なしは default、旧形式（suffix のみ）も後方互換で動作
fn orchestrator_master(arg: Option<&str>) -> Result<(), String> {
    use tako_control::orchestrator;

    orchestrator::ensure_defaults().map_err(|e| format!("セットアップに失敗: {e}"))?;

    // 旧バージョンが default.yaml に書き込んだ [1m] 既定値のマイグレーション（Issue #27）
    if let Some(notice) = orchestrator::migrate_legacy_default_profile() {
        eprintln!("ℹ {notice}");
        eprintln!();
    }

    // 引数をパース: "-<name>" → プロファイル名、それ以外 → 旧 suffix として扱う
    let (profile_name, suffix) = match arg {
        None => ("default", None),
        Some(s) if s.starts_with('-') => {
            let name = &s[1..];
            if name.is_empty() {
                return Err("プロファイル名が空です（例: tako master -2）".into());
            }
            (name, Some(name))
        }
        Some(s) => {
            // 旧形式の後方互換: `tako master dev` → suffix "dev"、プロファイル "default"
            ("default", Some(s))
        }
    };

    // プロファイルを読み込む（存在しなければデフォルト値で作成）
    let profile = match orchestrator::Profile::load(profile_name) {
        Ok(p) => p,
        Err(_) if profile_name == "default" => orchestrator::Profile::default(),
        Err(e) => return Err(e),
    };

    // プロファイルに明示された [1m] モデルは opt-in として尊重するが、
    // Pro プランでは起動不能になるため警告を出す（Issue #27）
    if let Some(warning) = profile
        .model
        .as_deref()
        .and_then(|m| orchestrator::one_m_model_warning(m, "master"))
    {
        eprintln!("{warning}");
    }
    if let Some(warning) = profile
        .resolve_worker_model()
        .filter(|m| Some(*m) != profile.model.as_deref())
        .and_then(|m| orchestrator::one_m_model_warning(m, "worker"))
    {
        eprintln!("{warning}");
    }

    // system prompt をプロファイル設定に基づいて合成し、一時ファイルに書き出す
    let prompt_content = profile.build_system_prompt(profile_name);
    let dir = orchestrator::config_dir().ok_or("ホームディレクトリが取得できない")?;
    let prompt_path = dir.join(format!("_system_prompt_{profile_name}.md"));
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| format!("system prompt の書き出しに失敗: {e}"))?;

    // タブ名
    let tab_title = match suffix {
        Some(s) => format!("master-{s}"),
        None => "master".into(),
    };

    // 新タブを作成
    let tab_result = send_request(Request::TabNew {
        title: Some(tab_title.clone()),
    })?;
    let pane_id = tab_result["pane"]
        .as_u64()
        .ok_or("タブ作成の応答に pane が含まれない")?;

    // master ペインに role を設定
    let role = match suffix {
        Some(s) => format!("orchestrator-master:{s}"),
        None => "orchestrator-master".into(),
    };
    send_request(Request::Title {
        pane: Some(pane_id),
        title: None,
        role: Some(role.clone()),
    })?;

    // TAKO_ORCHESTRATOR_ROLE 環境変数を設定
    let role_env = match suffix {
        Some(s) => format!("master:{s}"),
        None => "master".into(),
    };
    // model 未指定のプロファイルは --model を付けず claude CLI の既定に委ねる（Issue #27）
    let claude_cmd = orchestrator::build_master_claude_cmd(&role_env, &profile, &prompt_path);
    send_request(Request::Send {
        pane: Some(pane_id),
        text: claude_cmd,
        newline: true,
        tmux_session: None,
        await_prompt: false,
    })?;

    eprintln!("master を起動しました: タブ '{tab_title}'（ペイン {pane_id}）");
    eprintln!(
        "プロファイル: {profile_name}（モデル: {}、effort: {}）",
        profile.model_label(),
        profile.effort
    );
    let policy_desc = match profile.worker_model_policy {
        orchestrator::WorkerModelPolicy::Inherit => format!(
            "inherit（master と同じ {} / {}）",
            profile.model_label(),
            profile.effort
        ),
        orchestrator::WorkerModelPolicy::Fixed => format!(
            "fixed（{} / {}）",
            profile.worker_model_label(),
            profile.resolve_worker_effort()
        ),
        orchestrator::WorkerModelPolicy::Delegate => "delegate（master が判断）".into(),
    };
    eprintln!("worker モデルポリシー: {policy_desc}");
    eprintln!("system prompt: {}", prompt_path.display());
    Ok(())
}

/// `tako orchestrator watch --pane N [--session-id S] [--timeout T]` — worker の完了まで待機し 1 行出力する
fn orchestrator_watch(
    pane: u64,
    session_id: Option<&str>,
    tmux_session: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<(), String> {
    let interval = std::time::Duration::from_secs(5);
    let deadline =
        timeout_secs.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    // agents 一次（明示/自動解決）は streak 3、画面推定フォールバックは streak 8
    let mut idle_streak: u32 = 0;
    let mut gone_streak: u32 = 0;

    loop {
        if let Some(dl) = deadline {
            if std::time::Instant::now() >= dl {
                println!("WORKER_TIMEOUT: tako:{pane}");
                return Ok(());
            }
        }
        // ペインの存在確認（IPC 経由。tmux_session 指定時は pane 消滅後も tmux で追跡）
        let result = send_request(Request::OrchestratorWorkerStatus {
            pane_id: pane,
            session_id: session_id.map(|s| s.to_string()),
            tmux_session: tmux_session.map(|s| s.to_string()),
        });

        match result {
            Ok(val) => {
                let status = val["status"].as_str().unwrap_or("unknown");
                let recent = val["recent_output"].as_str().unwrap_or("");
                let source = val["status_source"].as_str().unwrap_or("screen");
                // agents 一次シグナル（明示 or 自動解決）は streak 3、画面推定は streak 8
                let need_streak: u32 = if source == "screen" { 8 } else { 3 };

                match status {
                    "gone" => {
                        // tmux session 経由でペインの実在を直接確認（tako 再起動時の誤検知防止）
                        if let Some(ts) = tmux_session {
                            if tmux_session_alive(ts) {
                                // tmux session が生きている = ペインは生存中（tako が再起動しただけ）
                                gone_streak = 0;
                                idle_streak = 0;
                                std::thread::sleep(interval);
                                continue;
                            }
                        }
                        gone_streak += 1;
                        if gone_streak >= 2 {
                            println!("WORKER_GONE: tako:{pane}");
                            return Ok(());
                        }
                    }
                    "idle" => {
                        gone_streak = 0;
                        // 画面内容で busy パターンがあれば idle を取り消す
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
                        gone_streak = 0;
                        // unknown: 画面内容から状態を推定（保守的 = busy 寄り）
                        if screen_looks_busy(recent) {
                            idle_streak = 0;
                        } else if screen_looks_idle(recent) {
                            idle_streak += 1;
                        } else {
                            // 判定不能は busy 扱い（誤 idle 防止）
                            idle_streak = 0;
                        }
                    }
                }

                if idle_streak >= need_streak {
                    let ctx = val["ctx_percent"].as_u64();
                    if let Some(pct) = ctx {
                        println!("WORKER_IDLE: tako:{pane} (ctx {pct}%)");
                    } else {
                        println!("WORKER_IDLE: tako:{pane}");
                    }
                    return Ok(());
                }
            }
            Err(_) => {
                // IPC エラー = tako が再起動中の可能性。tmux で実在確認
                if let Some(ts) = tmux_session {
                    if tmux_session_alive(ts) {
                        gone_streak = 0;
                        std::thread::sleep(interval);
                        continue;
                    }
                }
                gone_streak += 1;
                if gone_streak >= 2 {
                    println!("WORKER_GONE: tako:{pane}");
                    return Ok(());
                }
            }
        }

        std::thread::sleep(interval);
    }
}

/// tmux session が生きているか直接確認する（tako-core に依存せず CLI 単体で判定）
fn tmux_session_alive(session: &str) -> bool {
    let socket = std::env::var("TAKO_TMUX_SOCKET")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tako".into());
    std::process::Command::new("tmux")
        .args(["-L", &socket, "has-session", "-t", &format!("={session}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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

/// 画面内容が busy（作業中）を示すパターンを含むか（末尾 5 行に限定）
fn screen_looks_busy(output: &str) -> bool {
    let lines = tail_lines(output, 5);
    lines.iter().any(|l| {
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

/// 画面内容が idle（入力待ち）を示すパターンを含むか
/// Claude TUI は ❯ プロンプトの下にフッター（区切り線・モデル情報・ctx%等）が
/// 4〜6 行あるため、末尾 10 行の範囲でチェックする
fn screen_looks_idle(output: &str) -> bool {
    tail_lines(output, 10)
        .iter()
        .any(|l| l.trim_start().starts_with('❯'))
}

/// 末尾付近に ❯ プロンプトがあるか（dispatch 側の idle 補正と共用）
/// Claude TUI のフッター行を考慮して末尾 10 行をチェック
pub fn last_line_has_prompt(output: &str) -> bool {
    tail_lines(output, 10)
        .iter()
        .any(|l| l.trim_start().starts_with('❯'))
}

/// `tako orchestrator run` — spawn + 完了待ち + 出力取得 + close を 1 回で行う
#[allow(clippy::too_many_arguments)]
fn orchestrator_run(
    project: &str,
    prompt: &str,
    label: Option<&str>,
    pane: Option<u64>,
    tab: Option<u64>,
    timeout_secs: u64,
    auto_close: bool,
    output_lines: usize,
) -> Result<(), String> {
    let pane_resolved = if pane.is_some() {
        pane
    } else if tab.is_some() {
        None
    } else {
        caller_pane()
    };
    let tab_resolved = if pane.is_some() { None } else { tab };
    if pane_resolved.is_none() && tab_resolved.is_none() {
        return Err("--pane または --tab を指定してください".into());
    }
    // 1. Spawn
    let spawn_result = send_request(Request::OrchestratorSpawn {
        project: project.to_string(),
        prompt: prompt.to_string(),
        label: label.map(|s| s.to_string()),
        model: None,
        effort: None,
        pane: pane_resolved,
        tab: tab_resolved,
    })?;
    let pane_id = spawn_result["pane_id"].as_u64().unwrap_or(0);
    let spawned_by = spawn_result["spawned_by"].as_u64().unwrap_or(0);
    let tmux_session = spawn_result["tmux_session"].as_str().map(String::from);
    eprintln!(
        "spawned pane {pane_id} (tmux: {})",
        tmux_session.as_deref().unwrap_or("none")
    );

    // 2. 完了待ちポーリング（orchestrator_watch と同じ判定ロジック）
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let interval = std::time::Duration::from_secs(5);
    let mut idle_streak: u32 = 0;
    let mut gone_streak: u32 = 0;
    let mut final_status = "timeout".to_string();

    // claude 起動 + プロンプト送信を待つ
    std::thread::sleep(std::time::Duration::from_secs(20));

    loop {
        if start.elapsed() > timeout {
            break;
        }

        let result = send_request(Request::OrchestratorWorkerStatus {
            pane_id,
            session_id: None,
            tmux_session: tmux_session.clone(),
        });

        match result {
            Ok(val) => {
                let status = val["status"].as_str().unwrap_or("unknown");
                let recent = val["recent_output"].as_str().unwrap_or("");
                let source = val["status_source"].as_str().unwrap_or("screen");
                let need_streak: u32 = if source == "screen" { 8 } else { 3 };
                match status {
                    "gone" => {
                        if let Some(ref ts) = tmux_session {
                            if tmux_session_alive(ts) {
                                gone_streak = 0;
                                idle_streak = 0;
                                std::thread::sleep(interval);
                                continue;
                            }
                        }
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
                if let Some(ref ts) = tmux_session {
                    if tmux_session_alive(ts) {
                        gone_streak = 0;
                        std::thread::sleep(interval);
                        continue;
                    }
                }
                gone_streak += 1;
                if gone_streak >= 2 {
                    final_status = "error".to_string();
                    break;
                }
            }
        }

        std::thread::sleep(interval);
    }

    // 3. 出力取得
    let output = send_request(Request::Read {
        pane: Some(pane_id),
        lines: Some(output_lines),
        tmux_session: tmux_session.clone(),
    })
    .ok()
    .and_then(|v| v["content"].as_str().map(String::from))
    .unwrap_or_default();

    // 4. 自動 close（orchestrator run の完了後なので force: true）
    let closed = if auto_close {
        send_request(Request::Close {
            pane: Some(pane_id),
            force: true,
        })
        .is_ok()
    } else {
        false
    };

    let result = serde_json::json!({
        "pane_id": pane_id,
        "spawned_by": spawned_by,
        "status": final_status,
        "output": output,
        "duration_seconds": start.elapsed().as_secs(),
        "closed": closed,
    });
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako orchestrator projects` — CLI 版プロジェクト管理
fn orchestrator_projects_cli(sub: &ProjectsCommand) -> Result<(), String> {
    use tako_control::orchestrator;

    match sub {
        ProjectsCommand::List => {
            let config = orchestrator::ProjectsConfig::load()?;
            let projects = config.list_resolved();
            if projects.is_empty() {
                eprintln!("登録済みプロジェクトはありません。");
                eprintln!("追加: tako orchestrator projects add --key <名前> --cwd <パス>");
            } else {
                for p in &projects {
                    let desc = p.description.as_deref().unwrap_or("");
                    println!("{:<16} {}  {}", p.key, p.cwd, desc);
                }
            }
            Ok(())
        }
        ProjectsCommand::Add {
            key,
            cwd,
            description,
        } => {
            orchestrator::ensure_defaults()?;
            let mut config = orchestrator::ProjectsConfig::load()?;
            config.add(key.clone(), cwd.clone(), description.clone());
            config.save()?;
            eprintln!("追加しました: {key} → {cwd}");
            Ok(())
        }
        ProjectsCommand::Remove { key } => {
            let mut config = orchestrator::ProjectsConfig::load()?;
            if !config.remove(key) {
                return Err(format!("プロジェクト '{key}' が見つかりません"));
            }
            config.save()?;
            eprintln!("削除しました: {key}");
            Ok(())
        }
    }
}

/// `tako orchestrator profiles` — CLI 版プロファイル管理。
/// dispatch と同じ実装（ファイル直読み）を呼ぶため、tako アプリの起動は不要
fn orchestrator_profiles_cli(sub: &ProfilesCommand) -> Result<(), String> {
    use tako_control::dispatch::{dispatch_orchestrator_profiles, ProfilesParams};

    let params = match sub {
        ProfilesCommand::List => ProfilesParams {
            action: "list".into(),
            name: None,
            model: None,
            worker_model: None,
            effort: None,
            worker_effort: None,
            clear_model: false,
            clear_worker_model: false,
        },
        ProfilesCommand::Show { name } => ProfilesParams {
            action: "show".into(),
            name: name.clone(),
            model: None,
            worker_model: None,
            effort: None,
            worker_effort: None,
            clear_model: false,
            clear_worker_model: false,
        },
        ProfilesCommand::Set {
            name,
            model,
            clear_model,
            worker_model,
            clear_worker_model,
            effort,
            worker_effort,
        } => ProfilesParams {
            action: "set".into(),
            name: Some(name.clone()),
            model: model.clone(),
            worker_model: worker_model.clone(),
            effort: effort.clone(),
            worker_effort: worker_effort.clone(),
            clear_model: *clear_model,
            clear_worker_model: *clear_worker_model,
        },
    };
    let result = dispatch_orchestrator_profiles(params).map_err(|e| e.to_string())?;
    if let Some(warnings) = result["warnings"].as_array() {
        for w in warnings {
            if let Some(text) = w.as_str() {
                eprintln!("{text}");
            }
        }
    }
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote start` — デーモンをバックグラウンドで fork 起動し QR を表示する
fn remote_start(port: u16, no_tunnel: bool) -> Result<(), String> {
    let result = tako_control::remote::spawn_daemon(Some(port), no_tunnel)?;
    println!("{}", pretty_json(&result));
    if let Some(connect) = result["connect_url"].as_str() {
        match tako_control::remote::generate_qr_png(connect) {
            Ok(path) => {
                eprintln!("\nQR コードを生成しました: {}", path.display());
                // tako-app が起動していれば IPC 経由で OpenFile を送る（エラーは握りつぶす）
                let _ = send_request(Request::OpenFile {
                    pane: None,
                    path: path.display().to_string(),
                    mode: Some(tako_control::protocol::PreviewModeWire::Image),
                    direction: None,
                });
                eprintln!("スマホでスキャンしてください。");
            }
            Err(e) => eprintln!("\nQR コード画像の生成に失敗: {e}"),
        }
        eprintln!("URL: {connect}");
        if let Some(tunnel) = result["tunnel_url"].as_str() {
            eprintln!("Tunnel: {tunnel}");
        }
        if let Some(mid) = result["machine_id"].as_str() {
            eprintln!("Machine ID: {mid}");
        }
    }
    Ok(())
}

/// `tako remote stop` — デーモンを PID ファイルから kill する
fn remote_stop() -> Result<(), String> {
    let result = tako_control::remote::daemon_stop()?;
    println!("{}", pretty_json(&result));
    eprintln!("リモートサーバーを停止しました");
    Ok(())
}

/// `tako remote status` — デーモンの状態を表示する
fn remote_status() -> Result<(), String> {
    let status = tako_control::remote::daemon_status();
    println!("{}", pretty_json(&status));
    Ok(())
}

/// `tako remote serve` — HTTP サーバーをフォアグラウンドで起動する（内部用）
fn remote_serve(port: u16, no_tunnel: bool) -> Result<(), String> {
    tako_control::remote::run_daemon(Some(port), no_tunnel).map_err(|e| e.to_string())
}

/// `tako remote agents` — claude agents --json + tmux ペイン対応付けを表示する
fn remote_agents() -> Result<(), String> {
    let result = tako_control::agents::list_agents_with_panes(None)?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote messages` — transcript の末尾を正規化 JSON で表示する
fn remote_messages(session_id: &str, tail: usize) -> Result<(), String> {
    let result = tako_control::transcript::read_messages(session_id, tail)?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote scrollback` — ペインのスクロールバック履歴をプレーンテキストで表示する
fn remote_scrollback(pane_id: &str, lines: u32) -> Result<(), String> {
    let result = tako_control::remote::scrollback(pane_id, lines)?;
    for line in result {
        println!("{line}");
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
                // --tab 指定時は pane を使わない（タブのフォーカスペインを dispatch が解決）
                pane: if args.tab.is_some() {
                    None
                } else {
                    target_pane(args.pane)?
                },
                tab: args.tab,
                direction,
                ratio: args.ratio,
                command: (!args.command.is_empty()).then(|| args.command.clone()),
                cwd: args.cwd.clone(),
                focus: Some(args.focus),
            }
        }
        Command::Send(args) => Request::Send {
            pane: target_pane(args.pane)?,
            text: args.text.join(" "),
            newline: !args.no_newline,
            tmux_session: args.tmux_session.clone(),
            await_prompt: args.await_prompt,
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
            tmux_session: args.tmux_session.clone(),
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
            force: args.force,
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
        Command::Open(args) => {
            // 相対パスは CLI 実行時の cwd で絶対化する（--pane で別ペインを指定しても
            // 「いま居る場所」基準のまま意図どおりに解決される）
            let path = std::path::Path::new(&args.path);
            let path = if path.is_relative() {
                std::env::current_dir()
                    .map(|cwd| cwd.join(path).display().to_string())
                    .unwrap_or_else(|_| args.path.clone())
            } else {
                args.path.clone()
            };
            Request::OpenFile {
                pane: target_pane(args.pane)?,
                path,
                mode: match args.mode.as_deref() {
                    None => None,
                    Some("code") => Some(tako_control::protocol::PreviewModeWire::Code),
                    Some("image") => Some(tako_control::protocol::PreviewModeWire::Image),
                    Some("pdf") => Some(tako_control::protocol::PreviewModeWire::Pdf),
                    Some("video") => Some(tako_control::protocol::PreviewModeWire::Video),
                    Some(_) => Some(tako_control::protocol::PreviewModeWire::Markdown),
                },
                // 方向指定なし = 既存プレビュー再利用の従来セマンティクス
                direction: match (args.right, args.down, args.up, args.left) {
                    (true, _, _, _) => Some(Direction::Right),
                    (_, true, _, _) => Some(Direction::Down),
                    (_, _, true, _) => Some(Direction::Up),
                    (_, _, _, true) => Some(Direction::Left),
                    _ => None,
                },
            }
        }
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
        Command::Tab(TabCommand::MovePane {
            tab,
            target,
            pane,
            right,
            down,
            up,
            left,
        }) => {
            // 方向フラグは --target 指定時のみ有効（黙って無視せず明示エラーにする）
            if (*right || *down || *up || *left) && target.is_none() {
                return Err("--right/--down/--up/--left は --target と併用する".into());
            }
            Request::MovePane {
                pane: target_pane(*pane)?,
                tab: *tab,
                target: *target,
                direction: target.map(|_| match (down, up, left) {
                    (true, _, _) => Direction::Down,
                    (_, true, _) => Direction::Up,
                    (_, _, true) => Direction::Left,
                    _ => Direction::Right,
                }),
            }
        }
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
        Command::Git(GitCommand::Log { max_count, pane }) => Request::GitLog {
            pane: target_pane(*pane)?,
            max_count: Some(*max_count),
        },
        Command::Git(GitCommand::Diff { target, pane }) => Request::GitDiff {
            pane: target_pane(*pane)?,
            target: target.clone(),
        },
        Command::Collapse(args) => Request::CollapseTab {
            // tab 明示時はペイン不要。省略時は呼び出し元ペインのタブへ
            pane: if args.tab.is_some() {
                None
            } else {
                target_pane(None)?
            },
            tab: args.tab,
            collapsed: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Pin(args) => Request::Pin {
            // group-tab 指定時はペイン不要。pane / group-tab 省略時は呼び出し元ペイン
            pane: if args.group_tab.is_some() {
                None
            } else {
                target_pane(args.pane)?
            },
            group_tab: args.group_tab,
            pinned: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Background(args) => Request::Background {
            pane: target_pane(args.pane)?,
        },
        Command::Foreground(args) => Request::Foreground {
            pane: args.pane,
            target: args.target,
            direction: args.direction.as_deref().map(parse_direction).transpose()?,
        },
        Command::BackgroundList => Request::BackgroundList,
        Command::Tmux(TmuxCommand::List { socket }) => Request::TmuxList {
            socket: socket.clone(),
        },
        Command::Tmux(TmuxCommand::Cleanup { socket }) => Request::TmuxCleanup {
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
        Command::Tmux(TmuxCommand::Resize {
            session,
            window,
            cols,
            rows,
            reset,
            socket,
        }) => Request::TmuxResize {
            socket: socket.clone(),
            session: session.clone(),
            window: *window,
            cols: *cols,
            rows: *rows,
            reset: *reset,
        },
        Command::Tmux(TmuxCommand::SelectWindow { window, pane }) => Request::TmuxSelectWindow {
            pane: target_pane(*pane)?,
            window: *window,
        },
        Command::Tmux(TmuxCommand::Open {
            session,
            socket,
            pane,
            right: _,
            down,
            up,
            left,
        }) => Request::TmuxOpen {
            socket: socket.clone(),
            session: session.clone(),
            window: None,
            pane: target_pane(*pane)?,
            direction: match (down, up, left) {
                (true, _, _) => Some(Direction::Down),
                (_, true, _) => Some(Direction::Up),
                (_, _, true) => Some(Direction::Left),
                _ => Some(Direction::Right),
            },
        },
        Command::File(FileCommand::CopyPath {
            path,
            relative,
            pane,
        }) => {
            let abs = resolve_cli_path(path);
            if *relative {
                Request::FileOp {
                    op: tako_control::protocol::FileOpKind::CopyRelativePath,
                    path: abs,
                    name: None,
                    pane: target_pane(*pane)?,
                }
            } else {
                Request::FileOp {
                    op: tako_control::protocol::FileOpKind::CopyAbsolutePath,
                    path: abs,
                    name: None,
                    pane: None,
                }
            }
        }
        Command::File(FileCommand::Reveal { path }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::Reveal,
            path: resolve_cli_path(path),
            name: None,
            pane: None,
        },
        Command::File(FileCommand::OpenTerminal { path, pane }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenTerminal,
            path: resolve_cli_path(path),
            name: None,
            pane: target_pane(*pane)?,
        },
        Command::File(FileCommand::Rename { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::Rename,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
        },
        Command::File(FileCommand::Create { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::CreateFile,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
        },
        Command::File(FileCommand::Mkdir { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::CreateDir,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
        },
        Command::File(FileCommand::Trash { path }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::Trash,
            path: resolve_cli_path(path),
            name: None,
            pane: None,
        },
        Command::Video(VideoCommand::Play { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "play".into(),
        },
        Command::Video(VideoCommand::Pause { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "pause".into(),
        },
        Command::Video(VideoCommand::Toggle { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "toggle".into(),
        },
        Command::Video(VideoCommand::Seek { seconds, pane }) => Request::VideoSeek {
            pane: target_pane(*pane)?,
            seconds: *seconds,
        },
        Command::Orchestrator(OrchestratorCommand::Spawn {
            project,
            prompt,
            label,
            model,
            effort,
            pane,
            tab,
        }) => {
            let pane_resolved = if pane.is_some() {
                *pane
            } else if tab.is_some() {
                None
            } else {
                caller_pane()
            };
            let tab_resolved = if pane.is_some() { None } else { *tab };
            if pane_resolved.is_none() && tab_resolved.is_none() {
                return Err("--pane または --tab を指定してください".into());
            }
            Request::OrchestratorSpawn {
                project: project.clone(),
                prompt: prompt.clone(),
                label: label.clone(),
                model: model.clone(),
                effort: effort.clone(),
                pane: pane_resolved,
                tab: tab_resolved,
            }
        }
        Command::Orchestrator(OrchestratorCommand::Status {
            pane,
            session_id,
            tmux_session,
        }) => Request::OrchestratorWorkerStatus {
            pane_id: *pane,
            session_id: session_id.clone(),
            tmux_session: tmux_session.clone(),
        },
        // remote コマンドは main() でローカル処理済みのため到達不能
        Command::Remote(_) => unreachable!("remote は run() を通らない"),
        // main() で分岐済みのため論理的に到達不能
        Command::Mcp(_) => unreachable!("mcp serve は run() を通らない"),
        Command::Setup(_) => unreachable!("setup は run() を通らない"),
        Command::SetupMcp(_) => unreachable!("setup-mcp は run() を通らない"),
        Command::Master { .. } => {
            unreachable!("master は run() を通らない（直接 orchestrator_master() を呼ぶ）")
        }
        Command::Orchestrator(OrchestratorCommand::Watch { .. }) => {
            unreachable!("orchestrator watch は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Projects(_)) => {
            unreachable!("orchestrator projects は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Profiles(_)) => {
            unreachable!("orchestrator profiles は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Run { .. }) => {
            unreachable!("orchestrator run は run() を通らない")
        }
        Command::Chrome(ChromeCommand::Open {
            ref url,
            pane,
            right,
            down,
            left,
            up,
        }) => {
            let direction = match (down, left, up) {
                (true, _, _) => Some(Direction::Down),
                (_, true, _) => Some(Direction::Left),
                (_, _, true) => Some(Direction::Up),
                _ if *right => Some(Direction::Right),
                _ => None,
            };
            Request::ChromeOpen {
                url: url.clone(),
                pane: target_pane(*pane)?,
                direction,
            }
        }
        Command::Update(sub) => Request::Update {
            action: Some(match sub {
                UpdateCommand::Status => "status".to_string(),
                UpdateCommand::Check => "check".to_string(),
                UpdateCommand::Apply => "apply".to_string(),
                UpdateCommand::ApplyZip => "apply-zip".to_string(),
                UpdateCommand::Repair => "repair".to_string(),
            }),
        },
    })
}

fn parse_direction(s: &str) -> Result<Direction, String> {
    match s {
        "right" | "r" => Ok(Direction::Right),
        "down" | "d" => Ok(Direction::Down),
        "left" | "l" => Ok(Direction::Left),
        "up" | "u" => Ok(Direction::Up),
        _ => Err(format!("不正な方向: {s}（right / down / left / up）")),
    }
}

fn resolve_cli_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(p).display().to_string())
            .unwrap_or_else(|_| path.to_string())
    } else {
        path.to_string()
    }
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

fn pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
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
            println!("{}", pretty_json(result));
        }
        Command::Tab(TabCommand::New { .. }) => println!("{result}"),
        Command::Open(_) => println!("{result}"),
        Command::Autorename(_)
        | Command::Portdetect(_)
        | Command::Persist(_)
        | Command::Panel(_)
        | Command::Collapse(_)
        | Command::Pin(_) => {
            println!("{result}")
        }
        Command::Git(GitCommand::Log { .. }) | Command::Git(GitCommand::Diff { .. }) => {
            println!("{}", pretty_json(result));
        }
        Command::Tmux(TmuxCommand::List { .. }) | Command::Tmux(TmuxCommand::Cleanup { .. }) => {
            println!("{}", pretty_json(result));
        }
        Command::Tmux(TmuxCommand::Kill { .. })
        | Command::Tmux(TmuxCommand::Resize { .. })
        | Command::Tmux(TmuxCommand::Open { .. })
        | Command::Tmux(TmuxCommand::SelectWindow { .. }) => {
            println!("{result}")
        }
        Command::File(FileCommand::CopyPath { .. }) => {
            if let Some(p) = result["path"].as_str() {
                println!("{p}");
            }
        }
        Command::File(_) => println!("{result}"),
        Command::Video(_) => println!("{result}"),
        Command::Orchestrator(OrchestratorCommand::Spawn { .. }) => {
            println!("{}", pretty_json(result));
        }
        Command::Orchestrator(OrchestratorCommand::Status { .. }) => {
            println!("{}", pretty_json(result));
        }
        Command::BackgroundList => {
            println!("{}", pretty_json(result));
        }
        Command::Chrome(_) => println!("{result}"),
        Command::Update(_) => println!("{}", pretty_json(result)),
        // remote は run() → print_result を通らない
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
                tab: None,
                direction: Some(Direction::Down),
                ratio: Some(0.3),
                command: Some(vec!["npm".into(), "run".into(), "dev".into()]),
                cwd: None,
                focus: Some(false),
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
                tmux_session: None,
                await_prompt: false,
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
                tab: Some(4),
                target: None,
                direction: None,
            }
        );
        let command = parse(&["tako", "tab", "select", "2"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::TabSelect { tab: 2 }
        );
    }

    #[test]
    fn move_paneのtarget指定は方向つきで写す() {
        // FR-1.10: タイトルバー D&D の同等操作（同タブ内の挿し直し）
        let command = parse(&[
            "tako",
            "tab",
            "move-pane",
            "--target",
            "7",
            "--pane",
            "9",
            "--down",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::MovePane {
                pane: Some(9),
                tab: None,
                target: Some(7),
                direction: Some(Direction::Down),
            }
        );
        // 方向省略は右
        let command = parse(&["tako", "tab", "move-pane", "--target", "7", "--pane", "9"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::MovePane {
                pane: Some(9),
                tab: None,
                target: Some(7),
                direction: Some(Direction::Right),
            }
        );
        // tab と --target の併用は clap が拒否、--target なしの方向指定は build_request が拒否
        assert!(Cli::try_parse_from(["tako", "tab", "move-pane", "4", "--target", "7"]).is_err());
        let command = parse(&["tako", "tab", "move-pane", "4", "--pane", "9", "--down"]);
        assert!(build_request(&command).is_err());
    }

    #[test]
    fn openは絶対パスとモード別名を解釈する() {
        let command = parse(&["tako", "open", "/tmp/a.md", "--pane", "5", "--mode", "md"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::OpenFile {
                pane: Some(5),
                path: "/tmp/a.md".into(),
                mode: Some(tako_control::protocol::PreviewModeWire::Markdown),
                direction: None,
            }
        );
        // 相対パスは CLI の cwd で絶対化される
        let command = parse(&["tako", "open", "b.rs", "--pane", "5"]);
        let Request::OpenFile {
            path,
            mode,
            direction,
            ..
        } = build_request(&command).unwrap()
        else {
            panic!("OpenFile になる");
        };
        assert!(std::path::Path::new(&path).is_absolute());
        assert!(path.ends_with("b.rs"));
        assert_eq!(mode, None);
        assert_eq!(direction, None);
        // 方向指定（FR-3.11 = D&D のドロップ位置相当）
        let command = parse(&["tako", "open", "/tmp/a.md", "--pane", "5", "--down"]);
        let Request::OpenFile { direction, .. } = build_request(&command).unwrap() else {
            panic!("OpenFile になる");
        };
        assert_eq!(direction, Some(Direction::Down));
    }

    #[test]
    fn tmux_openは方向とソケットを解釈する() {
        let command = parse(&[
            "tako",
            "tmux",
            "open",
            "master-tako",
            "--socket",
            "work",
            "--pane",
            "3",
            "--down",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::TmuxOpen {
                socket: Some("work".into()),
                session: "master-tako".into(),
                window: None,
                pane: Some(3),
                direction: Some(Direction::Down),
            }
        );
        // 方向省略は右
        let command = parse(&["tako", "tmux", "open", "s1", "--pane", "3"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::TmuxOpen {
                socket: None,
                session: "s1".into(),
                window: None,
                pane: Some(3),
                direction: Some(Direction::Right),
            }
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
