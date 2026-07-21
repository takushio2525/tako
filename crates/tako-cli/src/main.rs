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
use tako_control::orchestrator::wait;
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
    /// PDF・画像プレビューのズーム・ページ・パン操作。引数なしで現在状態を表示する
    Preview(PreviewArgs),
    /// Markdown・PDF プレビューのアウトラインを表示し、項目へジャンプする
    #[command(name = "preview-outline")]
    PreviewOutline(PreviewOutlineArgs),
    /// PDF プレビュー内のリンク一覧を表示する
    #[command(name = "preview-link-list")]
    PreviewLinkList(PaneArg),
    /// PDF プレビュー内のリンクをフォローする（外部 URL はブラウザ、内部はページジャンプ）
    #[command(name = "preview-follow-link")]
    PreviewFollowLink(PreviewFollowLinkArgs),
    /// 表示中プレビューファイルのライブリロード ON/OFF・状態確認
    #[command(name = "preview-reload")]
    PreviewReload(ToggleArgs),
    /// デコード済みプレビュー画像キャッシュの上限（MiB）と利用状況
    #[command(name = "preview-cache")]
    PreviewCache(PreviewCacheArgs),
    /// プレビューのチェンジログビュー切替・diff 展開（Issue #338）
    #[command(name = "preview-changelog")]
    PreviewChangelog(PreviewChangelogArgs),
    /// コードプレビューの軽量編集（開始 / 全文適用 / 保存）
    #[command(subcommand)]
    Edit(EditCommand),
    /// タブ操作（new / rename / select / move-pane）
    #[command(subcommand)]
    Tab(TabCommand),
    /// 複数ウィンドウの操作（Issue #339。list / new / close / move-tab / focus）
    #[command(subcommand)]
    Window(WindowCommand),
    /// タブ・ペイン名の AI 自動リネームの ON/OFF・状態確認
    Autorename(ToggleArgs),
    /// listen ポート検知 + 提案チップの ON/OFF・状態確認
    Portdetect(ToggleArgs),
    /// セッション永続化（tmux バックエンド）の ON/OFF・状態確認。
    /// 有効時、tako を再起動してもタブ構成と実行中プロセスが復元される
    Persist(ToggleArgs),
    /// × ボタン close の確認ダイアログ ON/OFF・状態確認
    #[command(name = "confirm-close")]
    ConfirmClose(ToggleArgs),
    /// UI テーマ（ライト/ダーク）の確認・切替（Issue #217）。
    /// 引数なしで現在テーマを表示、dark / light で指定、toggle で反転
    Theme(ThemeArgs),
    /// UI 表示言語（日本語/英語）の確認・切替（Issue #435）。
    /// 引数なしで現在言語を表示、ja / en で指定、system で OS ロケール追従
    Lang(LangArgs),
    /// ステータスバーの利用制限表示サービスの確認・切替（Issue #321）。
    /// 引数なしで現在サービスを表示、claude / codex / agy で指定
    #[command(name = "limit-service")]
    LimitService(LimitServiceArgs),
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
    /// 質問ゼロの自動セットアップ。claude / codex / agy を検出して環境を最適化する。
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
    /// マスターオーケストレーターを起動する。profile の claude / codex を system prompt 付きで起動する。
    /// 既定は現在のペインでインライン起動（新タブを作らない）。--tab で従来の新タブ起動。
    /// プロファイル名を指定して設定を切り替えられる（例: tako master -2 → "2" プロファイル）。
    /// 引数なしは default プロファイル。旧形式（tako master dev）も後方互換で動作する
    Master {
        /// プロファイル名（-2, -difficult 等）またはサフィックス（旧形式: dev 等）
        #[arg(allow_hyphen_values = true)]
        profile: Option<String>,
        /// 新しいタブで起動する（既定はインライン = 現在のペインで起動）
        #[arg(long)]
        tab: bool,
    },
    /// ソロエージェントを起動する。既定は現在のペインでインライン起動（新タブを作らない）。
    /// --tab で従来の新タブ起動。
    /// オーケストレーション無しの 1 対 1 対話モード（worker spawn を禁止、作業は自分で行う）。
    /// エコ運用（既定 effort=high）で Pro プランでも使える。master と同じプロファイル引数パターン。
    /// プロファイル名を指定して設定を切り替えられる（例: tako solo -fast → "fast" プロファイル）。
    /// 引数なしは default プロファイル。旧形式（tako solo docs）も後方互換で動作する
    Solo {
        /// プロファイル名（-fast 等）またはサフィックス（旧形式: docs 等）。role は solo:<suffix>
        #[arg(allow_hyphen_values = true)]
        profile: Option<String>,
        /// 新しいタブで起動する（既定はインライン = 現在のペインで起動）
        #[arg(long)]
        tab: bool,
    },
    /// オーケストレーター操作（projects / spawn / status / watch）
    #[command(subcommand)]
    Orchestrator(OrchestratorCommand),
    /// ネイティブ Web ビューペインの操作（FR-3.8 / #155）。
    /// URL をペインで開く・dock への退避と呼び出し・ナビゲーション・JS 評価
    #[command(subcommand)]
    Web(WebCommand),
    /// アプリ内更新の診断・チェック・実行（Issue #36）。
    /// 引数なしで配布系統・現在バージョン・重複 CLI を表示する
    #[command(subcommand)]
    Update(UpdateCommand),
    /// フルディスクアクセス (FDA) の状態確認と設定画面の起動（Issue #118）。
    /// FDA を付与するとフォルダアクセス許可ダイアログが一括で出なくなる
    #[command(subcommand)]
    Fda(FdaCommand),
    /// スリープ防止機能の状態確認・設定変更（Issue #173）。
    /// macOS のアイドルスリープを IOKit 電源アサーションで防止する
    #[command(subcommand, name = "sleep-guard")]
    SleepGuard(SleepGuardCommand),
    /// エラーレポートの自動送信（テレメトリ）の状態確認・切替（Issue #333）
    #[command(subcommand)]
    Telemetry(TelemetryCommand),
    /// ファイルツリーへのフォルダの追加・削除・一覧（#134）。
    /// AI が作業対象プロジェクトのフォルダを明示追加する
    #[command(subcommand)]
    Tree(TreeCommand),
    /// エージェント共通ルールの同期（#136）。
    /// 正本ファイルの内容を各エージェントのグローバル指示ファイルにマーカーブロックで埋め込む
    #[command(subcommand, name = "agents")]
    Agents(AgentsCommand),
    /// セッションカタログの参照・復元（Issue #112。worker / master / solo の会話を発見して呼び戻す）
    #[command(subcommand)]
    Sessions(SessionsCommand),
    /// ペインの平文ターミナルログの参照・設定（Issue #112。ペインが死んでも出力を遡る）
    #[command(subcommand)]
    Logs(LogsCommand),
    /// レイアウトの世代バックアップからの復旧（#177）。
    /// 引数なしで現在の layout.json とバックアップ世代の一覧を表示する。
    /// タブ・ペインが大量消失したときは tako を終了してから
    /// `tako recover --apply <世代>` で直前の構成へ戻し、tako を再起動する
    Recover(RecoverArgs),
    /// ディレクトリ/リポジトリ/SSH ホストを開く（#20）。
    /// 新タブを作成してファイルツリーに追加し、フォーカスを移す
    #[command(subcommand, name = "open-in")]
    OpenIn(OpenInCommand),
    /// 最近開いた項目の一覧・クリア（#20）
    #[command(subcommand)]
    Recent(RecentCommand),
    /// SSH config の Host 一覧を表示する（#20）
    SshHosts,
    /// タスクチェックポイントの操作（Issue #242）。
    /// worker タスクの進行状態を永続化し、クラッシュや利用上限からの resume を可能にする
    #[command(subcommand)]
    Task(TaskCommand),
    /// ユーザー入力が必要なコマンドを可視ペインに委譲する（Issue #305）。
    /// split → タイトル設定 → コマンド投入をアトミックに実行し、pane_id を返す。
    /// --wait で完了まで待って exit code を返す
    #[command(name = "run-interactive")]
    RunInteractive(RunInteractiveArgs),
    /// run-interactive で起動したペインの完了状態を確認する。
    /// exit code マーカーを探し、見つかれば auto_close 方針に従い処理する
    #[command(name = "run-interactive-status")]
    RunInteractiveStatus(RunInteractiveStatusArgs),
    /// ファイルを実行する（Code Runner: FR-3.18, #453）。
    /// ファイル内の tako:run 宣言または拡張子既定コマンドで新ペインを分割して実行する
    #[command(name = "run")]
    Run(RunArgs),
    /// 拡張子ごとの実行コマンド既定を一覧/設定/削除する（FR-3.18, #453）
    #[command(name = "run-default")]
    RunDefault(RunDefaultArgs),
}

#[derive(Args)]
struct RunInteractiveArgs {
    /// 実行するコマンド文字列
    command: String,
    /// ユーザーへの入力案内（タイトルに表示。省略時はコマンド文字列）
    #[arg(long)]
    hint: Option<String>,
    /// 分割の基準ペイン ID（省略時は呼び出し元。--tab と排他）
    #[arg(long, conflicts_with = "tab")]
    pane: Option<u64>,
    /// 分割先タブ ID（--pane と排他）
    #[arg(long)]
    tab: Option<u64>,
    /// 下に分割
    #[arg(long)]
    down: bool,
    /// 新ペイン側の取り分（0.0–1.0、省略時は 0.3）
    #[arg(long)]
    ratio: Option<f32>,
    /// 完了後の自動 close 方針（success / always / never。省略時は success）
    #[arg(long, default_value = "success")]
    auto_close: String,
    /// 完了まで待って exit code を返す（ポーリング）
    #[arg(long)]
    wait: bool,
}

#[derive(Args)]
struct RunInteractiveStatusArgs {
    /// 対象ペイン ID
    pane: u64,
}

#[derive(Args)]
struct RunArgs {
    /// 実行対象のファイルパス
    file: String,
    /// 実行プロファイル名（省略時は既定プロファイル）
    #[arg(long)]
    profile: Option<String>,
    /// コマンド上書き（最優先）
    #[arg(long)]
    command: Option<String>,
    /// 分割の基準ペイン ID（省略時は呼び出し元）
    #[arg(long, conflicts_with = "tab")]
    pane: Option<u64>,
    /// 分割先タブ ID
    #[arg(long)]
    tab: Option<u64>,
    /// 右に分割（既定は下）
    #[arg(long)]
    right: bool,
    /// 新ペイン側の取り分（0.0–1.0、省略時は 0.3）
    #[arg(long)]
    ratio: Option<f32>,
    /// 完了後の自動 close 方針（success / always / never。既定 never）
    #[arg(long, default_value = "never")]
    auto_close: String,
    /// 新ペインにフォーカスを移す
    #[arg(long)]
    focus: bool,
    /// 完了まで待って exit code を返す（ポーリング）
    #[arg(long)]
    wait: bool,
    /// 実行せずプロファイル一覧を表示する（--dry-run / --list）
    #[arg(long, alias = "dry-run")]
    list: bool,
}

#[derive(Args)]
struct RunDefaultArgs {
    /// 拡張子（省略時は全一覧）
    ext: Option<String>,
    /// 設定するコマンドテンプレート
    command: Option<String>,
    /// 拡張子既定を削除（組み込みに戻す）
    #[arg(long)]
    remove: bool,
}

#[derive(Args)]
struct RecoverArgs {
    /// このバックアップ世代（1〜3、または good = 最後に復元へ成功した良品）を
    /// layout.json へ復元する。現在の layout.json は layout.json.pre-recover へ退避される
    #[arg(long, value_name = "世代")]
    apply: Option<String>,
    /// 稼働中チェックをスキップして強制実行する（プロセス走査は別データ
    /// ディレクトリで動く無関係な tako も検出するため、その場合の明示上書き用）
    #[arg(long)]
    force: bool,
}

#[derive(Subcommand)]
enum OpenInCommand {
    /// ディレクトリを新タブで開く（cwd として起動 + ファイルツリーに追加）
    Dir {
        /// 開くディレクトリの絶対パス
        path: String,
        /// フォーカスを新タブに移さない
        #[arg(long)]
        no_focus: bool,
    },
    /// git リポジトリを新タブで開く（git root を自動検出）
    Repo {
        /// リポジトリ内の任意のパス（git root を自動検出する）
        path: String,
        /// フォーカスを新タブに移さない
        #[arg(long)]
        no_focus: bool,
    },
    /// SSH ホストに接続する新タブを開く
    Remote {
        /// ~/.ssh/config の Host 名（未定義でも ssh コマンドとして実行）
        host: String,
        /// フォーカスを新タブに移さない
        #[arg(long)]
        no_focus: bool,
    },
}

#[derive(Subcommand)]
enum RecentCommand {
    /// 最近開いたディレクトリ/リポジトリ/SSH ホストの一覧
    List,
    /// 履歴をクリアする
    Clear,
}

#[derive(Subcommand)]
enum WebCommand {
    /// URL を新しい Web ビューペインで開く
    Open {
        /// 開く URL（スキーム省略時は https、localhost 系は http に正規化）
        url: String,
        /// 分割の基準ペイン ID（省略時は呼び出し元）
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
        /// 新ペインにフォーカスを移す（省略時は元ペインを維持）
        #[arg(long)]
        focus: bool,
    },
    /// Web ビューの一覧（表示中 + dock 退避中。id・URL・タイトル・ペイン）
    List,
    /// dock 退避中の Web ビューをペインへ呼び出す
    Show {
        /// 対象 Web ビュー ID（`tako web list` で確認）
        id: u64,
        /// 分割の基準ペイン ID（省略時は呼び出し元）
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
        /// 呼び出したペインにフォーカスを移す（省略時は元ペインを維持）
        #[arg(long)]
        focus: bool,
    },
    /// Web ビューをペインから外して dock へ退避する（ページは生きたまま）
    Hide {
        /// 対象 Web ビュー ID（省略時は表示中が 1 つならそれ）
        #[arg(long)]
        id: Option<u64>,
        /// 対象が表示中のペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// Web ビューを完全に破棄する（表示中ならペインも閉じる）
    Close {
        /// 対象 Web ビュー ID（省略時は表示中が 1 つならそれ）
        #[arg(long)]
        id: Option<u64>,
        /// 対象が表示中のペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ページ遷移（back / forward / reload / URL）
    Nav {
        /// 遷移先: back / forward / reload / URL
        to: String,
        /// 対象 Web ビュー ID（省略時は表示中が 1 つならそれ）
        #[arg(long)]
        id: Option<u64>,
        /// 対象が表示中のペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// JavaScript を非同期評価して token を返す（結果は eval-result で回収）
    Eval {
        /// 実行する JavaScript
        js: String,
        /// 対象 Web ビュー ID（省略時は表示中が 1 つならそれ）
        #[arg(long)]
        id: Option<u64>,
        /// 対象が表示中のペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// eval の評価結果を回収する（未完なら pending: true）
    EvalResult {
        /// eval が返した token
        token: u64,
        /// 対象 Web ビュー ID（省略時は表示中が 1 つならそれ）
        #[arg(long)]
        id: Option<u64>,
        /// 対象が表示中のペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// URL・タイトル・読み込み状態を取得する
    Read {
        /// 対象 Web ビュー ID（省略時は表示中が 1 つならそれ）
        #[arg(long)]
        id: Option<u64>,
        /// 対象が表示中のペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
}

#[derive(Subcommand)]
enum UpdateCommand {
    /// 配布系統・現在バージョン・チャンネル・重複 CLI の診断情報を表示する
    Status,
    /// GitHub Releases から最新版の有無を確認する（更新は行わない）
    Check {
        /// 対象チャンネル（stable / test。省略で全チャンネル同時チェック）
        #[arg(long)]
        channel: Option<String>,
    },
    /// 配布系統に応じた更新を実行する
    Apply {
        /// 対象チャンネル（stable / test。省略で stable）
        #[arg(long)]
        channel: Option<String>,
    },
    /// zip 経由で強制更新する（brew 失敗時のフォールバック）
    ApplyZip {
        /// 対象チャンネル（stable / test。省略で stable）
        #[arg(long)]
        channel: Option<String>,
    },
    /// broken-brew 状態の修復（brew install --cask --force で台帳を再締結）
    Repair,
}

#[derive(Subcommand)]
enum FdaCommand {
    /// FDA の付与状態を確認する
    Status,
    /// システム設定のフルディスクアクセスパネルを開く
    Open,
}

#[derive(Subcommand)]
enum TelemetryCommand {
    /// テレメトリの状態を確認する
    Status,
    /// テレメトリを有効にする
    On,
    /// テレメトリを無効にする
    Off,
}

#[derive(Subcommand)]
enum SleepGuardCommand {
    /// スリープ防止の状態を確認する
    Status,
    /// スリープ防止の設定を変更する
    Set {
        /// アイドルスリープ防止モード: off / on / while-agents-running
        #[arg(long)]
        mode: Option<String>,
        /// 電源条件: ac-only / always
        #[arg(long, name = "power")]
        power_condition: Option<String>,
        /// 蓋閉じ防止モード: off / while-agents-running（要 sudoers 登録）
        #[arg(long)]
        lid_sleep_mode: Option<String>,
    },
    /// 蓋閉じ防止の sudoers 登録（管理者パスワード必要、初回のみ）
    InstallLidSleep,
    /// 蓋閉じ防止の sudoers 登録を削除
    RemoveLidSleep,
}

#[derive(Subcommand)]
enum TreeCommand {
    /// フォルダをファイルツリーに追加する
    Add {
        /// 追加するフォルダの絶対パス
        path: String,
        /// 対象タブ ID（省略時は呼び出し元ペインのタブ）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// フォルダをファイルツリーから削除する
    Remove {
        /// 削除するフォルダの絶対パス
        path: String,
        /// 対象タブ ID（省略時は呼び出し元ペインのタブ）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// 追加済みフォルダの一覧を表示する
    List {
        /// 対象タブ ID（省略時は呼び出し元ペインのタブ）
        #[arg(long)]
        tab: Option<u64>,
    },
}

#[derive(Subcommand)]
enum SessionsCommand {
    /// カタログの一覧（last_seen の新しい順）
    List {
        /// 種別で絞り込む: master / worker / solo / pane
        #[arg(long)]
        role: Option<String>,
        /// プロジェクトで絞り込む
        #[arg(long)]
        project: Option<String>,
        /// 最大表示件数（既定 30）
        #[arg(long)]
        limit: Option<usize>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// セッションのメタ情報と会話冒頭を表示する
    Show {
        /// session_id（前方一致可）
        id: String,
    },
    /// 会話を新しいペインで復元する（記録された cwd で claude --resume を起動）
    Resume {
        /// session_id（前方一致可）
        id: String,
        /// 分割元ペイン ID（省略時は呼び出し元ペイン）
        #[arg(long)]
        pane: Option<u64>,
        /// 分割先タブ ID（そのタブのフォーカスペインの隣に開く）
        #[arg(long)]
        tab: Option<u64>,
        /// 分割方向: right / down / left / up（省略時 right）
        #[arg(long)]
        direction: Option<String>,
    },
}

#[derive(Subcommand)]
enum LogsCommand {
    /// ログファイルの一覧
    List,
    /// ログの末尾を表示する（クローズ済みペインも可）
    Show {
        /// ペイン ID
        pane: Option<u64>,
        /// セッション ID で引く（カタログ経由。前方一致可）
        #[arg(long)]
        session: Option<String>,
        /// 表示行数（既定 200）
        #[arg(long)]
        lines: Option<usize>,
    },
    /// ログ保存の状態（ON/OFF・上限・保存先）
    Status,
    /// ログ保存の設定を変更する（設定は永続化）
    Set {
        /// 保存の ON/OFF
        #[arg(long)]
        enabled: Option<bool>,
        /// ペインあたりの上限（MB）
        #[arg(long = "max-mb")]
        max_mb: Option<u64>,
        /// ログ全体の上限（MB）
        #[arg(long = "total-max-mb")]
        total_max_mb: Option<u64>,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// チェックポイントを記録・更新する
    Checkpoint {
        /// task_id（省略時は自動採番）
        #[arg(long)]
        task_id: Option<String>,
        /// 対象ペイン ID
        #[arg(long)]
        pane: Option<u64>,
        /// GitHub Issue 番号
        #[arg(long)]
        issue: Option<u32>,
        /// 作業ブランチ名
        #[arg(long)]
        branch: Option<String>,
        /// フェーズ: queued / running / verifying / done / failed / suspended
        #[arg(long)]
        phase: Option<String>,
        /// 直近の git commit SHA
        #[arg(long)]
        last_commit: Option<String>,
        /// エージェント種別: claude / codex / agy
        #[arg(long)]
        agent: Option<String>,
        /// モデル名
        #[arg(long)]
        model: Option<String>,
        /// コンテキスト復元用のプロンプト冒頭
        #[arg(long)]
        prompt_head: Option<String>,
        /// プロジェクト名（projects.yaml のキー）
        #[arg(long)]
        project: Option<String>,
        /// 作業ディレクトリ
        #[arg(long)]
        cwd: Option<String>,
    },
    /// チェックポイント一覧
    List {
        /// フェーズで絞り込む
        #[arg(long)]
        phase: Option<String>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// チェックポイントから worker を再開する
    Resume {
        /// 再開するチェックポイントの task_id
        task_id: String,
        /// モデルを変更して再開する
        #[arg(long)]
        model: Option<String>,
        /// 分割元ペイン ID
        #[arg(long)]
        pane: Option<u64>,
        /// 分割先タブ ID
        #[arg(long)]
        tab: Option<u64>,
    },
    /// チェックポイントのフェーズを手動で変更する
    Update {
        /// 対象の task_id
        task_id: String,
        /// 新しいフェーズ
        #[arg(long)]
        phase: String,
        /// 理由（suspended_reason に記録）
        #[arg(long)]
        reason: Option<String>,
    },
    /// 受け入れゲートの操作（述語の定義・検証・参照。#244）
    #[command(subcommand)]
    Gate(GateCommand),
}

#[derive(Subcommand)]
enum GateCommand {
    /// 受け入れ条件（述語）を定義する
    Set {
        /// 対象のタスク ID
        task_id: String,
        /// Command 述語を追加（シェルコマンド。exit 0 で Passed）
        #[arg(long = "command", value_name = "CMD")]
        commands: Vec<String>,
        /// PrMerged 述語を追加（PR 番号。マージ済みで Passed）
        #[arg(long = "pr-merged", value_name = "PR_NUMBER")]
        pr_merged: Vec<u32>,
        /// Custom 述語を追加（説明文。手動で判定する）
        #[arg(long = "custom", value_name = "DESCRIPTION")]
        customs: Vec<String>,
        /// Command 述語の実行ディレクトリ
        #[arg(long)]
        cwd: Option<String>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 述語を実行し結果を記録する（Command / PrMerged を自動判定）
    Check {
        /// 対象のタスク ID
        task_id: String,
        /// 全 Passed で checkpoint.phase を done に遷移させない（既定は遷移する）
        #[arg(long)]
        no_sync: bool,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 受け入れゲートの状態を表示する
    Show {
        /// 対象のタスク ID
        task_id: String,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AgentsCommand {
    /// 共通ルールを各エージェントのグローバル指示ファイルに同期する
    SyncRules {
        /// 正本ファイルの絶対パス（省略時は config.yaml の設定値）
        #[arg(long)]
        source: Option<String>,
        /// 対象エージェント（複数指定可。省略時は設定値 or 全対象）
        #[arg(long)]
        targets: Option<Vec<String>>,
        /// JSON で結果を出力する
        #[arg(long)]
        json: bool,
    },
    /// 同期の設定状態を確認する
    Status {
        /// JSON で結果を出力する
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RemoteCommand {
    /// リモートアクセス API サーバーを起動し、QR コードを表示する
    /// （transport は Tailscale Serve + UDS。未セットアップなら不足項目を案内して停止）
    Start,
    /// リモートアクセス API サーバーを停止する
    Stop {
        /// SIGTERM の代わりに SIGKILL で停止する（P0-4）
        #[arg(long)]
        force: bool,
    },
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
    /// ペアリング済み端末の管理（一覧・失効。承認は Mac 画面のダイアログでのみ行う）
    Devices {
        #[command(subcommand)]
        command: RemoteDevicesCommand,
    },
    /// Tailscale を使ったリモート接続のセットアップ（対話ウィザード）
    Setup {
        /// 全質問に自動で yes と回答する（brew install 等）
        #[arg(long)]
        yes: bool,
        /// 非対話パラメータを JSON で渡す（MCP / dispatch と同じ形式）
        #[arg(long)]
        answers: Option<String>,
    },
    /// [内部用] HTTP サーバーをフォアグラウンドで起動する（start から自動呼び出し）
    Serve,
}

#[derive(Subcommand)]
enum RemoteDevicesCommand {
    /// 登録済み端末と保留中のペアリング要求を一覧する
    List,
    /// 端末の登録を失効させる（接続中なら即時切断される）
    Revoke {
        /// 対象デバイス ID（`tako remote devices list` で確認できる）
        device_id: String,
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
    /// デフォルトアプ��で開く（macOS）
    Open { path: String },
    /// 指定アプリで開く（macOS）
    OpenWith { path: String, name: String },
}

#[derive(Subcommand)]
enum EditCommand {
    /// 編集モードを開始する
    Start {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 編集モードを終了する（未保存バッファは保持）
    Stop {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 編集状態（editing / dirty）を取得する
    Status {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 編集バッファの全文を置き換える（保存はしない）
    Apply {
        text: String,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 編集バッファをファイルへ保存する
    Save {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 直前の編集を取り消す（undo）
    Undo {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 取り消した編集をやり直す（redo）
    Redo {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// テキスト検索（query 省略時は現在の検索状態を返す）
    Search {
        /// 検索文字列
        query: Option<String>,
        /// 移動方向（next / prev）
        #[arg(long, default_value = "next")]
        direction: String,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// テキスト置換（1 件または全置換）
    Replace {
        /// 検索文字列
        query: String,
        /// 置換文字列
        replacement: String,
        /// 全置換
        #[arg(long)]
        all: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 自動保存の設定（enabled 省略時は状態取得）
    Autosave {
        /// true = ON、false = OFF（省略時は状態取得）
        enabled: Option<bool>,
        #[arg(long)]
        pane: Option<u64>,
    },
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
    /// ミュートのトグル
    Mute {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ミュート解除
    Unmute {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ループ再生のトグル
    Loop {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 音量の設定（0.0〜1.0）
    Volume {
        /// 音量（0.0〜1.0）
        volume: f64,
        #[arg(long)]
        pane: Option<u64>,
    },
}

#[derive(Subcommand)]
enum OrchestratorCommand {
    /// worker が完了（idle）・異常停止（error）・消滅（gone）するまでブロックし、結果を出力する。
    /// Monitor ツールから呼ばれる想定。出力形式: WORKER_IDLE / WORKER_ERROR / WORKER_GONE
    Watch {
        /// 監視対象ペイン ID（位置引数または --pane で指定）
        #[arg(long)]
        pane: Option<u64>,
        /// 監視対象ペイン ID（位置引数）
        #[arg(value_name = "PANE_ID")]
        pane_pos: Option<u64>,
        /// worker レジストリの ID（#390。pane が消えても追跡を継続する）
        #[arg(long)]
        worker: Option<String>,
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
    /// worker spawn のレイアウト設定（全オプション省略で現在値を表示）
    Layout {
        /// 配置ポリシー: master-reserved（master の取り分を維持。既定）/ legacy（従来の右等分割）
        #[arg(long)]
        policy: Option<String>,
        /// master 側へ残す取り分（0.1〜0.9。既定 0.5 = 画面半分）
        #[arg(long)]
        master_ratio: Option<f32>,
        /// worker 領域内の配置アルゴリズム: grid（十字四分割系。既定）/ spiral（縦横交互の半分割）
        #[arg(long)]
        algorithm: Option<String>,
    },
    /// 子 worker を spawn する（split + エージェント CLI 起動 + プロンプト送信）
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
        /// worker のエージェント CLI（claude / codex / agy。省略時はプロファイルの worker_agent → claude）
        #[arg(long)]
        agent: Option<String>,
        /// worker のモデル（agent のネイティブ表記。省略時は master のプロファイル設定）
        #[arg(long)]
        model: Option<String>,
        /// thinking / reasoning effort（claude・codex のみ。省略時は master のプロファイル設定）
        #[arg(long)]
        effort: Option<String>,
        /// 分割元ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID。tab と両方指定時は pane を優先）
        #[arg(long)]
        pane: Option<u64>,
        /// 子を出すタブ ID（そのタブのフォーカスペインを分割元にする）
        #[arg(long)]
        tab: Option<u64>,
        /// 委任台帳の task_type（省略時は investigation）
        #[arg(long)]
        task_type: Option<String>,
    },
    /// worker の状態確認（busy / idle / error / gone / unknown。error 時は
    /// error.kind（api_error / usage_limit / limit_dialog）と recommended_action を含む。#157）
    Status {
        /// ペイン ID（--worker と排他。どちらか必須）
        #[arg(long)]
        pane: Option<u64>,
        /// worker レジストリの ID（#390。pane が消えても状態を取得できる）
        #[arg(long)]
        worker: Option<String>,
        /// claude の session ID
        #[arg(long)]
        session_id: Option<String>,
        /// tmux session 名（pane 消滅時のフォールバック追跡）
        #[arg(long)]
        tmux_session: Option<String>,
    },
    /// master/solo が自身の pane・tab・ctx%・session_id を取得する（#123 / #193）
    #[command(name = "self")]
    SelfInfo {
        /// 自 pane ID（省略時は TAKO_PANE_ID / TAKO_ORCHESTRATOR_ROLE から自動解決）
        #[arg(long)]
        pane: Option<u64>,
    },
    /// master の引き継ぎを実行する（#193）。handoff ファイルを読み新 master を spawn
    Handoff {
        /// 呼び出し元ペイン ID（省略時は自動解決）
        #[arg(long)]
        pane: Option<u64>,
        /// 新 master を出すタブ ID（省略時は呼び出し元と同タブ）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// worker の permission ダイアログに応答する（#319）。
    /// ダイアログ不在時はエラー（誤爆防止）
    Respond {
        /// 対象ペイン ID
        #[arg(long)]
        pane: u64,
        /// 選択肢の番号（1-based）または "yes"/"no" エイリアス
        #[arg(long)]
        choice: String,
    },
    /// worker の報告内容を取得する（scrollback 主 + transcript 補強。#364）
    Report {
        /// 対象ペイン ID（--worker と排他。どちらか必須）
        #[arg(long)]
        pane: Option<u64>,
        /// worker レジストリの ID（#390。pane が消えても報告を取得できる）
        #[arg(long)]
        worker: Option<String>,
        /// スクロールバック取得行数（既定 2000）
        #[arg(long, default_value = "2000")]
        lines: usize,
        /// transcript から取得する直近 assistant メッセージ件数（既定 1。古い順で返す）
        #[arg(long)]
        messages: Option<usize>,
    },
    /// worker レジストリの一覧（#390）。spawn 済み worker をペインの生死と
    /// 無関係に列挙する（tako 再起動後も追跡できる）。既定は active のみ
    Workers {
        /// closed（明示 close 済み）の worker も含める
        #[arg(long)]
        all: bool,
    },
    /// worker 自動復旧 supervisor の操作（#401）
    Supervisor {
        /// status / set_mode / history
        action: String,
        /// set_mode 時のモード（auto / notify_only / off）
        #[arg(long)]
        mode: Option<String>,
        /// set_mode 時: WORKER_DEAD の自動 resume を有効にする
        #[arg(long)]
        auto_resume_dead: Option<bool>,
        /// set_mode 時: 同一 worker の最大リトライ回数（既定 3）
        #[arg(long)]
        max_retries: Option<u32>,
        /// 監査ログの返却行数
        #[arg(long)]
        lines: Option<usize>,
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
        /// worker のエージェント CLI（claude / codex / agy。省略時はプロファイルの worker_agent → claude）
        #[arg(long)]
        agent: Option<String>,
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
        /// 委任台帳の task_type（省略時は investigation）
        #[arg(long)]
        task_type: Option<String>,
    },
    /// 非同期 run の進捗照会（#121）。run_id 省略時は全 run の一覧
    #[command(name = "run-status")]
    RunStatus {
        /// 照会する run_id（省略時は全 run 一覧）
        run_id: Option<String>,
    },
    /// 完了した非同期 run の結果回収（#121）。未完了なら pending を返す
    #[command(name = "run-result")]
    RunResult {
        /// 回収する run_id
        run_id: String,
    },
    /// 委任台帳の操作（Issue #292）
    #[command(subcommand)]
    Ledger(LedgerCommand),
}

#[derive(Subcommand)]
enum LedgerCommand {
    /// 台帳エントリの一覧
    List {
        /// フィルタ: プロジェクト
        #[arg(long)]
        project: Option<String>,
        /// フィルタ: task_type
        #[arg(long)]
        task_type: Option<String>,
        /// 返す件数の上限（既定 50）
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// task_type x model の集計
    Stats,
    /// 検収結果の記録
    Record {
        /// エントリ ID（spawn 応答の ledger_id）
        id: String,
        /// 検収結果
        #[arg(long)]
        outcome: String,
        /// 差し戻し回数
        #[arg(long)]
        rounds: Option<u32>,
        /// メモ
        #[arg(long)]
        note: Option<String>,
    },
    /// 事後修正（検収 pass だが実使用で問題発覚）
    Amend {
        /// エントリ ID
        id: String,
        /// 修正メモ
        #[arg(long)]
        note: String,
    },
    /// project 前方一致でエントリを除去（selftest 混入等の掃除用）
    Prune {
        /// 除去対象の project プレフィックス（例: tako-selftest-）
        #[arg(long)]
        project_prefix: String,
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

// Set のオプション数で variant サイズ差 lint が出るが、CLI 引数のパースは
// プロセスで 1 回きりのため実害がなく許容する（clap は Box variant を扱えない）
#[allow(clippy::large_enum_variant)]
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
        /// master のエージェント種別（claude / codex。agy は master 非対応。--clear-master-agent と排他）
        #[arg(long, conflicts_with = "clear_master_agent")]
        master_agent: Option<String>,
        /// master_agent の指定を解除して claude 既定に戻す
        #[arg(long)]
        clear_master_agent: bool,
        /// master のモデル（master_agent のネイティブ表記。--clear-model と排他）
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
        /// worker の既定エージェント種別（claude / codex / agy。--clear-worker-agent と排他）
        #[arg(long, conflicts_with = "clear_worker_agent")]
        worker_agent: Option<String>,
        /// worker_agent の指定を解除して claude 既定に戻す
        #[arg(long)]
        clear_worker_agent: bool,
        /// --agent-* 系で編集する対象エージェント名（claude / codex / agy）
        #[arg(long)]
        agent: Option<String>,
        /// 対象エージェントの worker 既定モデル（CLI ネイティブ表記。--clear-agent-model と排他）
        #[arg(long, requires = "agent", conflicts_with = "clear_agent_model")]
        agent_model: Option<String>,
        /// 対象エージェントのモデル指定を解除する
        #[arg(long, requires = "agent")]
        clear_agent_model: bool,
        /// 対象エージェントの worker 既定 effort（agy は無視。--clear-agent-effort と排他）
        #[arg(long, requires = "agent", conflicts_with = "clear_agent_effort")]
        agent_effort: Option<String>,
        /// 対象エージェントの effort 指定を解除する
        #[arg(long, requires = "agent")]
        clear_agent_effort: bool,
        /// 対象エージェントの許可プロンプトスキップ（true / false。明示 opt-in）
        #[arg(long, requires = "agent")]
        agent_skip_permissions: Option<bool>,
        /// 対象エージェントの追加 CLI 引数（カンマ区切り。丸ごと置き換え。空文字でクリア）
        #[arg(long, requires = "agent", value_delimiter = ',')]
        agent_args: Option<Vec<String>>,
        /// worker のモデル選択ポリシー（inherit / delegate / fixed）
        #[arg(long)]
        worker_model_policy: Option<String>,
        /// タブ名の命名規則（master プロンプトに注入。空文字でクリア）
        #[arg(long)]
        tab_naming_convention: Option<String>,
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
    /// プレビューペインにフォーカスを移す（省略時は元ペインを維持）
    #[arg(long)]
    focus: bool,
}

#[derive(Args)]
struct PreviewArgs {
    /// 対象 PDF・画像プレビューペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 表示倍率（百分率。25〜400。例: 150 = 150%）
    #[arg(long, conflicts_with_all = ["zoom_in", "zoom_out", "reset"])]
    zoom: Option<f32>,
    /// 1 段階ズームイン
    #[arg(long, conflicts_with_all = ["zoom", "zoom_out", "reset"])]
    zoom_in: bool,
    /// 1 段階ズームアウト
    #[arg(long, conflicts_with_all = ["zoom", "zoom_in", "reset"])]
    zoom_out: bool,
    /// 幅フィット（100%）へ戻しパン位置をリセット
    #[arg(long, conflicts_with_all = ["zoom", "zoom_in", "zoom_out"])]
    reset: bool,
    /// PDF の表示ページ（1 始まり）
    #[arg(long)]
    page: Option<usize>,
    /// 現在位置から横へパンする量（logical px。正 = 右）
    #[arg(long, allow_hyphen_values = true)]
    pan_x: Option<f32>,
    /// 現在位置から縦へパンする量（logical px。正 = 下）
    #[arg(long, allow_hyphen_values = true)]
    pan_y: Option<f32>,
}

#[derive(Args)]
struct PreviewOutlineArgs {
    /// 対象 Markdown・PDF プレビューペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// ジャンプするアウトライン項目（表示順の 1 始まり。省略時は一覧取得のみ）
    #[arg(long)]
    item: Option<usize>,
}

#[derive(Args)]
struct PaneArg {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
}

#[derive(Args)]
struct PreviewFollowLinkArgs {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// フォローするリンクのインデックス（0 始まり。preview-link-list の結果で確認）
    index: usize,
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
    /// 表示するビュー（orch = オーケストレーター俯瞰。#217）
    #[arg(long, value_parser = ["tmux", "orch", "git"])]
    view: Option<String>,
    /// 左サイドバーのファイルツリー表示（FR-2.16.5。on = 表示、off = 非表示）
    #[arg(long, value_parser = ["on", "off"])]
    filetree: Option<String>,
    /// 左サイドバーの幅（px。Issue #307）
    #[arg(long)]
    sidebar_width: Option<f32>,
}

/// ON/OFF トグル系コマンド共通の引数（autorename / portdetect）
#[derive(Args)]
struct ToggleArgs {
    /// on = 有効化、off = 無効化（省略時は現在状態を表示）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

#[derive(Args)]
struct PreviewCacheArgs {
    /// キャッシュ上限（MiB、256〜8192。省略時は利用状況を表示）
    max_mb: Option<u64>,
}

/// チェンジログビューの引数（Issue #338）
#[derive(Args)]
struct PreviewChangelogArgs {
    /// 対象プレビューペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// on = チェンジログ表示、off = コードプレビューに戻す（省略時は状態取得）
    #[arg(value_parser = ["on", "off"])]
    mode: Option<String>,
    /// 取得するコミット数の上限（省略時は 50）
    #[arg(long)]
    max_count: Option<usize>,
    /// 指定コミットハッシュの diff を展開/折りたたみ
    #[arg(long)]
    expand: Option<String>,
}

/// UI テーマコマンドの引数（Issue #217）
#[derive(Args)]
struct ThemeArgs {
    /// dark / light = 指定テーマへ、toggle = 反転（省略時は現在テーマを表示）
    #[arg(value_parser = ["dark", "light", "toggle"])]
    mode: Option<String>,
}

/// UI 表示言語コマンドの引数（Issue #435）
#[derive(Args)]
struct LangArgs {
    /// ja / en = 指定言語へ、system = OS ロケール追従（省略時は現在言語を表示）
    #[arg(value_parser = ["ja", "en", "system"])]
    value: Option<String>,
}

/// 利用制限表示サービスの引数（Issue #321）
#[derive(Args)]
struct LimitServiceArgs {
    /// claude / codex / agy（省略時は現在サービスを表示）
    #[arg(value_parser = ["claude", "codex", "agy"])]
    service: Option<String>,
    /// 最新メトリクスを即時再取得する
    #[arg(long)]
    refresh: bool,
}

#[derive(Args)]
struct BackgroundArgs {
    /// バックグラウンドへ送るペイン ID（省略時は呼び出し元。--tab と排他）
    #[arg(long)]
    pane: Option<u64>,
    /// バックグラウンドへ送るタブ ID（タブ内全ペインを一括退避。--pane と排他）
    #[arg(long)]
    tab: Option<u64>,
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
    /// アップデート追従状況（前回セットアップ以降の setup 関連変更）を表示して終了する
    #[arg(long, conflicts_with_all = ["check", "reset"])]
    changes: bool,
    /// --changes の出力を JSON にする（MCP tako_setup_changes と同一ペイロード）
    #[arg(long, requires = "changes")]
    json: bool,
    /// 検出値・前回値・既定値を使い、標準入力を読まずにセットアップする
    #[arg(long, conflicts_with_all = ["check", "changes", "review"])]
    yes: bool,
    /// 全回答を JSON、@ファイル、または -（標準入力）で与える（指定時は非対話）
    #[arg(long, value_name = "JSON|@FILE|-", conflicts_with_all = ["check", "changes", "review"])]
    answers: Option<String>,
    /// 前回設定を setup agent と個別に見直す
    #[arg(long, conflicts_with_all = ["check", "changes", "yes", "answers"])]
    review: bool,
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
        /// 新タブをアクティブにする（省略時は現在のタブを維持）
        #[arg(long)]
        focus: bool,
    },
    /// タブの表示タイトルを変える（明示リネーム = 自動リネームより優先。空文字で解除）
    Rename {
        /// 対象タブ ID（省略時は呼び出し元ペインの属するタブ）
        #[arg(long)]
        tab: Option<u64>,
        /// manual（既定）= 手動リネーム。auto = 作業内容ベースの自動命名（手動リネーム済みタブは上書きしない）
        #[arg(long)]
        source: Option<String>,
        /// 新しいタイトル（複数引数はスペース連結。空文字で手動指定を解除）
        title: Vec<String>,
    },
    /// タブを切り替える
    Select { tab: u64 },
    /// タブの並び順を変更する（D&D 並べ替えと同等。#308）
    Reorder {
        /// 移動するタブ ID
        tab: u64,
        /// 移動先インデックス（0 始まり。範囲外は末尾にクランプ）
        #[arg(long)]
        index: usize,
    },
    /// ペインを移動する: タブ ID 指定 = 別タブの末尾へ、--target 指定 = そのペインの
    /// 隣（--right 等の方向）へ挿し直す（FR-1.10 = タイトルバー D&D の同等操作）
    MovePane {
        /// 移送先タブ ID（--target / --new と排他）
        #[arg(conflicts_with_all = ["target", "new"])]
        tab: Option<u64>,
        /// 挿入先ペイン ID（このペインの隣に入る。同タブ内の並べ替えに使う）
        #[arg(long, conflicts_with_all = ["tab", "new"])]
        target: Option<u64>,
        /// 新しいタブとして分離する（Issue #209）
        #[arg(long, conflicts_with_all = ["tab", "target"])]
        new: bool,
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
        /// 移動先のタブをアクティブにする（省略時は現在のタブを維持）
        #[arg(long)]
        focus: bool,
    },
}

#[derive(Subcommand)]
enum WindowCommand {
    /// ウィンドウ一覧を表示する
    List,
    /// 新しいウィンドウを開く。--tab で既存タブを分離、省略で新規タブ付き
    New {
        /// このタブを新しいウィンドウへ分離する（省略時は新規タブを作って開く）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// ウィンドウを閉じる（タブは残存ウィンドウへ合流。プロセスは殺さない）
    Close {
        /// 対象ウィンドウ ID
        window: u64,
    },
    /// タブを別ウィンドウへ移動する（移動先の表示タブになる）
    MoveTab {
        /// 移動するタブ ID
        #[arg(long)]
        tab: u64,
        /// 移動先ウィンドウ ID
        #[arg(long)]
        window: u64,
    },
    /// ウィンドウをアクティブにして前面化する
    Focus {
        /// 対象ウィンドウ ID
        window: u64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Mcp(McpCommand::Serve) => mcp_serve(),
        Command::Setup(ref args) => {
            if args.check {
                setup::run_check()
            } else if args.changes {
                setup::run_changes(args.json)
            } else if args.reset {
                setup::load_answers(args.answers.as_deref()).and_then(|answers| {
                    setup::run_reset().and_then(|()| {
                        setup::run_setup(args.yes || args.answers.is_some(), args.review, &answers)
                    })
                })
            } else {
                setup::load_answers(args.answers.as_deref()).and_then(|answers| {
                    setup::run_setup(args.yes || args.answers.is_some(), args.review, &answers)
                })
            }
        }
        Command::SetupMcp(ref args) => setup_mcp_local(args),
        Command::Master { ref profile, tab } => orchestrator_master(profile.as_deref(), tab),
        Command::Solo { ref profile, tab } => orchestrator_solo(profile.as_deref(), tab),
        Command::Orchestrator(OrchestratorCommand::Watch {
            pane,
            pane_pos,
            ref worker,
            ref session_id,
            ref tmux_session,
            timeout,
        }) => orchestrator_watch(
            pane.or(pane_pos),
            worker.as_deref(),
            session_id.as_deref(),
            tmux_session.as_deref(),
            timeout,
        ),
        Command::Orchestrator(OrchestratorCommand::Projects(ref sub)) => {
            orchestrator_projects_cli(sub)
        }
        Command::Orchestrator(OrchestratorCommand::Profiles(ref sub)) => {
            orchestrator_profiles_cli(sub)
        }
        Command::Orchestrator(OrchestratorCommand::SelfInfo { pane }) => {
            let pane = pane.or_else(caller_pane);
            let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
            send_request(Request::OrchestratorSelf {
                pane,
                caller_role,
                caller_pid: Some(std::process::id()),
            })
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Handoff { pane, tab }) => {
            let pane = pane.or_else(caller_pane);
            let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
            send_request(Request::OrchestratorHandoff {
                pane,
                caller_role,
                tab,
                caller_pid: Some(std::process::id()),
            })
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Layout {
            ref policy,
            master_ratio,
            ref algorithm,
        }) => {
            // config.yaml のみの操作のため IPC 不要。dispatch と同一関数を共用する
            // （MCP `tako_orchestrator_layout` と 1:1。二重実装を作らない）
            tako_control::dispatch_orchestrator_layout(
                policy.as_deref(),
                master_ratio,
                algorithm.as_deref(),
            )
            .map_err(|e| e.to_string())
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Respond { pane, ref choice }) => {
            let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
            send_request(Request::OrchestratorRespond {
                pane_id: pane,
                choice: choice.clone(),
                caller_role,
            })
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Report {
            pane,
            ref worker,
            lines,
            messages,
        }) => send_request(Request::OrchestratorReport {
            pane_id: pane,
            lines: Some(lines),
            messages,
            worker: worker.clone(),
        })
        .map(|result| println!("{}", pretty_json(&result))),
        Command::Orchestrator(OrchestratorCommand::Workers { all }) => {
            send_request(Request::OrchestratorWorkers {
                all: Some(all).filter(|a| *a),
            })
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Supervisor {
            ref action,
            ref mode,
            auto_resume_dead,
            max_retries,
            lines,
        }) => send_request(Request::OrchestratorSupervisor {
            action: action.clone(),
            mode: mode.clone(),
            auto_resume_dead,
            max_retries,
            lines,
        })
        .map(|result| println!("{}", pretty_json(&result))),
        Command::Orchestrator(OrchestratorCommand::Run {
            ref project,
            ref prompt,
            ref label,
            ref agent,
            pane,
            tab,
            timeout,
            auto_close,
            output_lines,
            ref task_type,
        }) => orchestrator_run(
            project,
            prompt,
            label.as_deref(),
            agent.as_deref(),
            pane,
            tab,
            timeout,
            auto_close,
            output_lines,
            task_type.as_deref(),
        ),
        Command::Orchestrator(OrchestratorCommand::RunStatus { ref run_id }) => {
            let request = Request::OrchestratorRunStatus {
                run_id: run_id.clone(),
            };
            send_request(request).map(|v| println!("{}", pretty_json(&v)))
        }
        Command::Orchestrator(OrchestratorCommand::RunResult { ref run_id }) => {
            let request = Request::OrchestratorRunResult {
                run_id: run_id.clone(),
            };
            send_request(request).map(|v| println!("{}", pretty_json(&v)))
        }
        Command::Orchestrator(OrchestratorCommand::Ledger(ref sub)) => ledger_cli(sub),
        // gate 操作は YAML I/O + コマンド実行のみのためローカル処理（#244）
        Command::Task(TaskCommand::Gate(ref gate_sub)) => gate_cli(gate_sub),
        // remote コマンドはローカル処理（IPC 不要）
        Command::Remote(RemoteCommand::Start) => remote_start(),
        Command::Remote(RemoteCommand::Stop { force }) => remote_stop(force),
        Command::Remote(RemoteCommand::Status) => remote_status(),
        Command::Remote(RemoteCommand::Serve) => remote_serve(),
        Command::Remote(RemoteCommand::Agents) => remote_agents(),
        Command::Remote(RemoteCommand::Messages { session_id, tail }) => {
            remote_messages(&session_id, tail)
        }
        Command::Remote(RemoteCommand::Scrollback { pane_id, lines }) => {
            remote_scrollback(&pane_id, lines)
        }
        Command::Remote(RemoteCommand::Devices { command }) => remote_devices(command),
        Command::Remote(RemoteCommand::Setup { yes, answers }) => {
            remote_setup_cli(yes, answers.as_deref())
        }
        // テレメトリもローカル処理（IPC 不要。設定ファイルの読み書きのみ）
        Command::Telemetry(ref sub) => telemetry_local(sub),
        // FDA チェックはローカル処理（IPC 不要。ファイルシステムのみ）
        Command::Fda(ref sub) => fda_local(sub),
        // スリープ防止もローカル処理（IPC 不要。設定ファイルの読み書きのみ）
        Command::SleepGuard(ref sub) => sleep_guard_local(sub),
        // エージェント共通ルール同期もローカル処理（IPC 不要）
        Command::Agents(ref sub) => agents_local(sub),
        // レイアウト復旧もローカル処理（GUI 死亡・縮退保存後の復旧手段のため IPC 不要が本質）
        Command::Recover(ref args) => recover_local(args),
        // run-interactive --wait は起動 + ポーリングの合成
        Command::RunInteractive(ref args) if args.wait => run_interactive_wait(&cli.command),
        // run --wait / --list は合成処理
        Command::Run(ref args) if args.wait => run_wait(&cli.command),
        Command::Run(ref args) if args.list => run_list(&cli.command),
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
    let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();

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
                    caller_role: caller_role.clone(),
                    connected,
                    exec: &mut exec,
                    ipc_tx: None,
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
    let tako_bin = tako_control::dispatch::resolve_tako_binary();
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
            if result.repaired {
                let old = result.old_command.as_deref().unwrap_or("(不明)");
                eprintln!(
                    "登録パスが消失していたため付け替えました: {}",
                    settings_path.display()
                );
                eprintln!("  旧: {old}");
                eprintln!("  新: {tako_bin}");
            } else if result.already_existed {
                eprintln!("既に設定されています: {}", settings_path.display());
            } else {
                eprintln!("設定を追加しました: {}", settings_path.display());
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// MCP 登録パスの存在を確認し、不在なら警告を出す（master/solo 起動前のガード）
fn check_mcp_health_warning() {
    let home = match std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
    {
        Some(h) => h,
        None => return,
    };
    let settings_path = home.join(".claude").join("settings.json");
    let content = match std::fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };
    let cmd = match settings
        .get("mcpServers")
        .and_then(|s| s.get("tako"))
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(c) => c,
        None => return, // 未登録は setup の管轄
    };
    if !std::path::Path::new(cmd).is_file() {
        eprintln!("[警告] MCP 登録パスが消失しています: {cmd}");
        eprintln!("        tako MCP なしで起動します。tako setup-mcp で修復してください。");
        eprintln!();
    }
}

/// `tako master [-profile]` — 新タブで claude をマスター system prompt 付きで起動する。
/// `-<名前>` でプロファイルを指定、引数なしは default、旧形式（suffix のみ）も後方互換で動作
fn orchestrator_master(arg: Option<&str>, use_tab: bool) -> Result<(), String> {
    use tako_control::orchestrator;

    orchestrator::ensure_defaults().map_err(|e| format!("セットアップに失敗: {e}"))?;

    check_mcp_health_warning();

    if let Some(notice) = orchestrator::migrate_legacy_default_profile() {
        eprintln!("ℹ {notice}");
        eprintln!();
    }

    let (profile_name, suffix) = match arg {
        None => ("default", None),
        Some(s) if s.starts_with('-') => {
            let name = &s[1..];
            if name.is_empty() {
                return Err("プロファイル名が空です（例: tako master -2）".into());
            }
            (name, Some(name))
        }
        Some(s) => ("default", Some(s)),
    };

    let profile = match orchestrator::Profile::load(profile_name) {
        Ok(p) => p,
        Err(_) if profile_name == "default" => orchestrator::Profile::default(),
        Err(e) => return Err(e),
    };

    let master_agent = profile.resolve_master_agent()?;

    if profile.master_agent_is_claude() {
        if let Some(warning) = profile
            .model
            .as_deref()
            .and_then(|m| orchestrator::one_m_model_warning(m, "master"))
        {
            eprintln!("{warning}");
        }
    }
    if let Some(warning) = profile
        .resolve_worker_model()
        .filter(|m| Some(*m) != profile.model.as_deref())
        .and_then(|m| orchestrator::one_m_model_warning(m, "worker"))
    {
        eprintln!("{warning}");
    }

    let prompt_content = profile.build_system_prompt(profile_name);
    let dir = orchestrator::config_dir().ok_or("ホームディレクトリが取得できない")?;
    let prompt_path = dir.join(format!("_system_prompt_{profile_name}.md"));
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| format!("system prompt の書き出しに失敗: {e}"))?;

    let tab_title = match suffix {
        Some(s) => format!("master-{s}"),
        None => "master".into(),
    };

    let role = match suffix {
        Some(s) => format!("orchestrator-master:{s}"),
        None => "orchestrator-master".into(),
    };
    let role_env = match suffix {
        Some(s) => format!("master:{s}"),
        None => "master".into(),
    };

    let tako_bin = tako_control::dispatch::resolve_tako_binary();
    let master_cmd = orchestrator::build_master_cmd(&role_env, &profile, &prompt_path, &tako_bin)?;

    // インライン起動（既定）: 現在のペインでコマンドを実行（新タブを作らない。#264）
    // --tab 指定時: 従来の新タブ起動
    let pane_id = if use_tab {
        let tab_result = send_request(Request::TabNew {
            title: Some(tab_title.clone()),
            focus: Some(true),
        })?;
        tab_result["pane"]
            .as_u64()
            .ok_or("タブ作成の応答に pane が含まれない")?
    } else {
        let cp = caller_pane().ok_or(
            "呼び出し元ペインが不明（tako 内から実行するか、--tab で新タブ起動してください）",
        )?;
        send_request(Request::TabRename {
            tab: None,
            pane: Some(cp),
            title: tab_title.clone(),
            source: None,
        })
        .ok();
        cp
    };

    send_request(Request::Title {
        pane: Some(pane_id),
        title: None,
        role: Some(role.clone()),
    })?;

    send_request(Request::Send {
        pane: Some(pane_id),
        text: master_cmd,
        newline: true,
        tmux_session: None,
        await_prompt: false,
    })?;

    let location = if use_tab {
        format!("タブ '{tab_title}'（ペイン {pane_id}）")
    } else {
        format!("ペイン {pane_id}（インライン）")
    };
    eprintln!("master を起動しました: {location}");
    eprintln!(
        "プロファイル: {profile_name}（エージェント: {}、モデル: {}、effort: {}）",
        master_agent.as_str(),
        profile.master_model_label(),
        profile.effort
    );
    let policy_desc = match profile.worker_model_policy {
        orchestrator::WorkerModelPolicy::Inherit if profile.master_agent_is_claude() => format!(
            "inherit（master と同じ {} / {}）",
            profile.model_label(),
            profile.effort
        ),
        orchestrator::WorkerModelPolicy::Inherit => format!(
            "inherit（master は {} のため claude worker へは非継承: {} / {}）",
            master_agent.as_str(),
            profile.worker_model_label(),
            profile.resolve_worker_effort()
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

/// `tako solo [-profile]` — solo system prompt 付きで claude / codex を起動する。
/// 既定はインライン（現在のペインで起動）、--tab で新タブ起動（#264）。
fn orchestrator_solo(arg: Option<&str>, use_tab: bool) -> Result<(), String> {
    use tako_control::orchestrator;

    orchestrator::ensure_solo_defaults().map_err(|e| format!("セットアップに失敗: {e}"))?;

    check_mcp_health_warning();

    let (profile_name, suffix) = match arg {
        None => ("default", None),
        Some(s) if s.starts_with('-') => {
            let name = &s[1..];
            if name.is_empty() {
                return Err("プロファイル名が空です（例: tako solo -fast）".into());
            }
            (name, Some(name))
        }
        Some(s) => ("default", Some(s)),
    };

    let profile = match orchestrator::load_solo_profile(profile_name) {
        Ok(p) => p,
        Err(_) if profile_name == "default" => orchestrator::solo_default_profile(),
        Err(e) => return Err(e),
    };

    let solo_agent = profile.resolve_master_agent()?;

    if profile.master_agent_is_claude() {
        if let Some(warning) = profile
            .model
            .as_deref()
            .and_then(|m| orchestrator::one_m_model_warning(m, "solo"))
        {
            eprintln!("{warning}");
        }
    }

    let prompt_content = profile.build_solo_system_prompt(profile_name);
    let dir = orchestrator::config_dir().ok_or("ホームディレクトリが取得できない")?;
    let prompt_path = dir.join(format!("_solo_system_prompt_{profile_name}.md"));
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| format!("system prompt の書き出しに失敗: {e}"))?;

    let tab_title = match suffix {
        Some(s) => format!("solo-{s}"),
        None => "solo".into(),
    };

    let role = match suffix {
        Some(s) => format!("solo:{s}"),
        None => "solo".into(),
    };

    let tako_bin = tako_control::dispatch::resolve_tako_binary();
    let solo_cmd = orchestrator::build_master_cmd(&role, &profile, &prompt_path, &tako_bin)?;

    let pane_id = if use_tab {
        let tab_result = send_request(Request::TabNew {
            title: Some(tab_title.clone()),
            focus: Some(true),
        })?;
        tab_result["pane"]
            .as_u64()
            .ok_or("タブ作成の応答に pane が含まれない")?
    } else {
        let cp = caller_pane().ok_or(
            "呼び出し元ペインが不明（tako 内から実行するか、--tab で新タブ起動してください）",
        )?;
        send_request(Request::TabRename {
            tab: None,
            pane: Some(cp),
            title: tab_title.clone(),
            source: None,
        })
        .ok();
        cp
    };

    send_request(Request::Title {
        pane: Some(pane_id),
        title: None,
        role: Some(role.clone()),
    })?;

    send_request(Request::Send {
        pane: Some(pane_id),
        text: solo_cmd,
        newline: true,
        tmux_session: None,
        await_prompt: false,
    })?;

    let location = if use_tab {
        format!("タブ '{tab_title}'（ペイン {pane_id}）")
    } else {
        format!("ペイン {pane_id}（インライン）")
    };
    eprintln!("solo を起動しました: {location}");
    eprintln!(
        "プロファイル: {profile_name}（エージェント: {}、モデル: {}、effort: {}）",
        solo_agent.as_str(),
        profile.master_model_label(),
        profile.effort
    );
    eprintln!("モード: solo（オーケストレーション無し・1 対 1 対話・worker spawn 禁止）");
    eprintln!("system prompt: {}", prompt_path.display());
    Ok(())
}

/// `tako orchestrator watch --pane N [--worker W] [--session-id S] [--timeout T]` —
/// worker の完了まで待機し 1 行出力する。
/// 判定は tako-control の完了待ちエンジン（`orchestrator::wait`。MCP の run と共通。#83）。
/// 異常停止（API エラー・usage limit 等）は WORKER_ERROR として区別する（#157）。
/// #390: `--worker`（レジストリ ID）指定で pane 省略可。pane 指定でも session_id /
/// tmux_session の欠けをレジストリで自動補完し、pane 消失後も追跡を継続する
fn orchestrator_watch(
    pane: Option<u64>,
    worker: Option<&str>,
    session_id: Option<&str>,
    tmux_session: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<(), String> {
    use tako_control::orchestrator::registry::WorkerRegistry;
    let mut session_id = session_id.map(str::to_string);
    let mut tmux_session = tmux_session.map(str::to_string);
    let pane = if let Some(worker_id) = worker {
        // レジストリからペイン・追跡キーを解決（watch ループは IPC 断でも回り続ける
        // 設計のため、レジストリ解決も CLI プロセス内で行い tako 本体に依存しない）
        let reg =
            WorkerRegistry::load().map_err(|e| format!("worker レジストリを読めない: {e}"))?;
        let (_, entry) = reg.resolve(worker_id)?;
        session_id = session_id.or_else(|| entry.session_id.clone());
        tmux_session = tmux_session.or_else(|| entry.tmux_session.clone());
        entry.pane
    } else {
        let Some(p) = pane else {
            return Err(
                "ペイン ID または --worker を指定してください（tako orchestrator watch <PANE_ID> / --worker <ID>）"
                    .to_string(),
            );
        };
        // pane 指定でも欠けた追跡キーはレジストリで補完（読めなければ従来動作）
        if session_id.is_none() || tmux_session.is_none() {
            if let Ok(reg) = WorkerRegistry::load() {
                if let Some((_, entry)) = reg.find_active_by_pane(p) {
                    session_id = session_id.or_else(|| entry.session_id.clone());
                    tmux_session = tmux_session.or_else(|| entry.tmux_session.clone());
                }
            }
        }
        p
    };
    let mut exec = |req: Request| send_request(req);
    let opts = wait::WatchOptions {
        pane_id: pane,
        session_id: session_id.clone(),
        tmux_session: tmux_session.clone(),
        timeout: timeout_secs.map(std::time::Duration::from_secs),
        initial_delay: std::time::Duration::ZERO,
        interval: std::time::Duration::from_secs(5),
    };
    let outcome = wait::wait_for_worker(&mut exec, &opts, None);

    // #243: Idle / Error 確定後に events を取得して補助行に出力する。
    // wait_for_worker の最終ポーリング結果から events を構築するため、
    // 完了後に worker_status を 1 回追加で取得する
    let print_events = |exec: &mut dyn FnMut(Request) -> Result<serde_json::Value, String>| {
        if let Ok(val) = exec(Request::OrchestratorWorkerStatus {
            pane_id: Some(pane),
            session_id: session_id.clone(),
            tmux_session: tmux_session.clone(),
            worker: None,
        }) {
            if let Some(events) = val["events"].as_array() {
                for ev in events {
                    if let Some(kind) = ev["kind"].as_str() {
                        let mut parts = vec![format!("  event: {kind}")];
                        if let Some(from) = ev["from"].as_str() {
                            parts.push(format!("from={from}"));
                        }
                        if let Some(to) = ev["to"].as_str() {
                            parts.push(format!("to={to}"));
                        }
                        if let Some(pct) = ev["percent"].as_u64() {
                            parts.push(format!("percent={pct}"));
                        }
                        println!("{}", parts.join(" "));
                    }
                }
            }
        }
    };

    match outcome {
        wait::WatchOutcome::Idle {
            ctx_percent: Some(pct),
        } => {
            println!("WORKER_IDLE: tako:{pane} (ctx {pct}%)");
            print_events(&mut exec);
        }
        wait::WatchOutcome::Idle { .. } => {
            println!("WORKER_IDLE: tako:{pane}");
            print_events(&mut exec);
        }
        wait::WatchOutcome::Question {
            ctx_percent: Some(pct),
        } => {
            println!("WORKER_QUESTION: tako:{pane} (ctx {pct}%)");
            print_events(&mut exec);
        }
        wait::WatchOutcome::Question { .. } => {
            println!("WORKER_QUESTION: tako:{pane}");
            print_events(&mut exec);
        }
        wait::WatchOutcome::Error { kind, detail } => {
            println!("WORKER_ERROR: tako:{pane} ({})", kind.as_str());
            if !detail.is_empty() {
                println!("  detail: {detail}");
            }
            println!("  action: {}", kind.recommended_action());
            print_events(&mut exec);
        }
        wait::WatchOutcome::Stalled { detail } => {
            println!("WORKER_STALLED: tako:{pane}");
            if !detail.is_empty() {
                println!("  detail: {detail}");
            }
            println!("  action: check_and_resume");
        }
        wait::WatchOutcome::PermissionWaiting { permission_dialog } => {
            println!("WORKER_PERMISSION: tako:{pane}");
            if let Some(cmd) = permission_dialog.get("command").and_then(|v| v.as_str()) {
                println!("  command: {cmd}");
            }
            if let Some(opts) = permission_dialog.get("options").and_then(|v| v.as_array()) {
                for (i, opt) in opts.iter().enumerate() {
                    if let Some(text) = opt.as_str() {
                        println!("  {}. {text}", i + 1);
                    }
                }
            }
            println!("  action: respond");
            print_events(&mut exec);
        }
        wait::WatchOutcome::AgentDead { resume_command } => {
            println!("WORKER_DEAD: tako:{pane}");
            println!(
                "  detail: エージェント CLI プロセスが終了している（SIGSEGV 等の突然死の疑い）"
            );
            if let Some(cmd) = resume_command {
                println!("  resume: {cmd}");
            } else {
                println!("  resume: (session ID 未記録のため resume コマンドを組み立てられない)");
            }
            println!("  action: resume_session");
        }
        wait::WatchOutcome::Gone => println!("WORKER_GONE: tako:{pane}"),
        wait::WatchOutcome::Timeout => {
            println!("WORKER_TIMEOUT: tako:{pane}");
            // #390: prompt 未達（welcome 画面のまま = idle 判定が積めず TIMEOUT に
            // なりやすい）等の検知イベントを補助行で出す
            print_events(&mut exec);
        }
    }
    Ok(())
}

/// `tako orchestrator run` — spawn + 完了待ち + 出力取得 + close を 1 回で行う。
/// 本体は tako-control の `wait::run_worker`（MCP `tako_orchestrator_run` と共通。#83）
#[allow(clippy::too_many_arguments)]
fn orchestrator_run(
    project: &str,
    prompt: &str,
    label: Option<&str>,
    agent: Option<&str>,
    pane: Option<u64>,
    tab: Option<u64>,
    timeout_secs: u64,
    auto_close: bool,
    output_lines: usize,
    task_type: Option<&str>,
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
    let opts = wait::RunOptions {
        project: project.to_string(),
        prompt: prompt.to_string(),
        label: label.map(|s| s.to_string()),
        model: None,
        effort: None,
        agent: agent.map(|s| s.to_string()),
        pane: pane_resolved,
        tab: tab_resolved,
        caller_role: std::env::var("TAKO_ORCHESTRATOR_ROLE").ok(),
        timeout: std::time::Duration::from_secs(timeout_secs),
        auto_close,
        output_lines,
        // claude 起動 + プロンプト送信を待つ
        initial_delay: std::time::Duration::from_secs(20),
        interval: std::time::Duration::from_secs(5),
        task_type: task_type.map(str::to_string),
    };
    let mut exec = |req: Request| send_request(req);
    let result = wait::run_worker(&mut exec, &opts, &mut |pane_id, tmux| {
        eprintln!("spawned pane {pane_id} (tmux: {})", tmux.unwrap_or("none"));
    })?;
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
            // ロック付き read-modify-write（#169: 並行 add で他エントリを消さない）
            orchestrator::ProjectsConfig::mutate(|config| {
                config.add(key.clone(), cwd.clone(), description.clone());
            })?;
            eprintln!("追加しました: {key} → {cwd}");
            Ok(())
        }
        ProjectsCommand::Remove { key } => {
            let removed = orchestrator::ProjectsConfig::mutate(|config| config.remove(key))?;
            if !removed {
                return Err(format!("プロジェクト '{key}' が見つかりません"));
            }
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
            ..Default::default()
        },
        ProfilesCommand::Show { name } => ProfilesParams {
            action: "show".into(),
            name: name.clone(),
            ..Default::default()
        },
        ProfilesCommand::Set {
            name,
            master_agent,
            clear_master_agent,
            model,
            clear_model,
            worker_model,
            clear_worker_model,
            effort,
            worker_effort,
            worker_agent,
            clear_worker_agent,
            agent,
            agent_model,
            clear_agent_model,
            agent_effort,
            clear_agent_effort,
            agent_skip_permissions,
            agent_args,
            worker_model_policy,
            tab_naming_convention,
        } => ProfilesParams {
            action: "set".into(),
            name: Some(name.clone()),
            master_agent: master_agent.clone(),
            clear_master_agent: *clear_master_agent,
            model: model.clone(),
            worker_model: worker_model.clone(),
            effort: effort.clone(),
            worker_effort: worker_effort.clone(),
            clear_model: *clear_model,
            clear_worker_model: *clear_worker_model,
            worker_agent: worker_agent.clone(),
            clear_worker_agent: *clear_worker_agent,
            agent: agent.clone(),
            agent_model: agent_model.clone(),
            clear_agent_model: *clear_agent_model,
            agent_effort: agent_effort.clone(),
            clear_agent_effort: *clear_agent_effort,
            agent_skip_permissions: *agent_skip_permissions,
            agent_args: agent_args.clone(),
            worker_model_policy: worker_model_policy.clone(),
            tab_naming_convention: tab_naming_convention.clone(),
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

/// `tako remote start` — デーモンをバックグラウンドで fork 起動し QR を表示する。
/// transport は Tailscale Serve のみ（tailnet 内限定・WireGuard E2E 暗号化）。
/// Tailscale 未セットアップ時は spawn_daemon が不足項目を列挙して起動を拒否する（#282）。
/// QR は恒久固定 URL のみ（#283: secret を含まない。初回接続時に Mac 側で
/// ペアリング承認ダイアログが表示される）
fn remote_start() -> Result<(), String> {
    let result = tako_control::remote::spawn_daemon()?;
    println!("{}", pretty_json(&result));
    if let Some(url) = result["url"].as_str() {
        match tako_control::remote::generate_qr_png(url) {
            Ok(path) => {
                eprintln!("\nQR コードを生成しました: {}", path.display());
                // tako-app が起動していれば IPC 経由で OpenFile を送る（エラーは握りつぶす）
                let _ = send_request(Request::OpenFile {
                    pane: None,
                    path: path.display().to_string(),
                    mode: Some(tako_control::protocol::PreviewModeWire::Image),
                    direction: None,
                    focus: Some(true),
                });
                eprintln!("スマホでスキャンしてください。");
            }
            Err(e) => eprintln!("\nQR コード画像の生成に失敗: {e}"),
        }
        eprintln!("URL: {url}");
        eprintln!(
            "この URL は恒久固定で secret を含みません（Tailscale MagicDNS 名。tailnet 内限定）。"
        );
        eprintln!("スマホ側にも Tailscale アプリを入れ、同じアカウントでログインしてください。");
        eprintln!("初回アクセス時は Mac の画面にペアリング承認ダイアログが表示されます。");
    }
    Ok(())
}

/// `tako remote stop` — デーモンを PID ファイルから kill する
fn remote_stop(force: bool) -> Result<(), String> {
    let result = if force {
        tako_control::remote::daemon_force_stop()?
    } else {
        tako_control::remote::daemon_stop()?
    };
    println!("{}", pretty_json(&result));
    eprintln!("リモートサーバーを停止しました");
    Ok(())
}

/// `tako remote status` — デーモンの状態を表示する。
/// 応答にトークンは含まれない（#283 で長寿命 bearer token を全廃）
fn remote_status() -> Result<(), String> {
    let status = tako_control::remote::daemon_status();
    println!("{}", pretty_json(&status));
    Ok(())
}

/// `tako remote devices` — ペアリング済み端末の一覧・失効。
/// ペアリングの承認・role 変更は Mac 画面の GUI ダイアログでのみ行う
/// （AI フルコントロール不変条件の例外。`.agent/requirements.md`）
fn remote_devices(command: RemoteDevicesCommand) -> Result<(), String> {
    let result = match command {
        RemoteDevicesCommand::List => tako_control::remote::devices_list()?,
        RemoteDevicesCommand::Revoke { device_id } => {
            tako_control::remote::devices_revoke(&device_id)?
        }
    };
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote setup` — Tailscale リモートセットアップウィザード
fn remote_setup_cli(yes: bool, answers_json: Option<&str>) -> Result<(), String> {
    if let Some(json_str) = answers_json {
        let mut answers: tako_control::remote_setup::RemoteSetupAnswers =
            serde_json::from_str(json_str).map_err(|e| format!("answers JSON が不正: {e}"))?;
        if yes {
            answers.yes = Some(true);
        }
        let result = tako_control::remote_setup::run_noninteractive(&answers)?;
        println!("{}", pretty_json(&result));
    } else {
        let mut stdout = std::io::stdout();
        tako_control::remote_setup::run_interactive(yes, &mut stdout)?;
    }
    Ok(())
}

/// `tako remote serve` — HTTP サーバーをフォアグラウンドで起動する（内部用）
fn remote_serve() -> Result<(), String> {
    tako_control::remote::run_daemon().map_err(|e| e.to_string())
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

fn telemetry_local(sub: &TelemetryCommand) -> Result<(), String> {
    let mut settings = tako_control::settings::load();
    match sub {
        TelemetryCommand::Status => {
            let recent = tako_control::telemetry::recent_count();
            let queued = tako_control::telemetry::queue_count();
            let log_path = tako_control::telemetry::log_file_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            if settings.telemetry {
                eprintln!("telemetry: ON");
            } else {
                eprintln!("telemetry: OFF");
            }
            eprintln!("  直近のレポート件数: {recent}");
            if queued > 0 {
                eprintln!("  未送信キュー: {queued}");
            }
            eprintln!("  ログ: {log_path}");
            let json = serde_json::json!({
                "telemetry": settings.telemetry,
                "recent_reports": recent,
                "queued_reports": queued,
                "log_path": log_path,
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
            Ok(())
        }
        TelemetryCommand::On => {
            settings.telemetry = true;
            tako_control::settings::save(&settings)
                .map_err(|e| format!("設定の保存に失敗: {e}"))?;
            tako_control::telemetry::set_enabled(true);
            eprintln!("telemetry: ON");
            Ok(())
        }
        TelemetryCommand::Off => {
            settings.telemetry = false;
            tako_control::settings::save(&settings)
                .map_err(|e| format!("設定の保存に失敗: {e}"))?;
            tako_control::telemetry::set_enabled(false);
            eprintln!("telemetry: OFF");
            Ok(())
        }
    }
}

fn fda_local(sub: &FdaCommand) -> Result<(), String> {
    match sub {
        FdaCommand::Status => {
            let status = tako_control::fda::status_info();
            if status.granted {
                eprintln!("✓ フルディスクアクセス: 付与済み");
            } else {
                eprintln!("△ フルディスクアクセス: 未付与");
                eprintln!(
                    "  フォルダアクセス時に macOS の許可ダイアログが表示されることがあります"
                );
                eprintln!("  付与方法: tako fda open → システム設定で tako を追加");
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&status.to_json()).unwrap()
            );
            Ok(())
        }
        FdaCommand::Open => {
            tako_control::fda::open_settings()?;
            eprintln!(
                "システム設定を開きました。tako を「フルディスクアクセス」に追加してください"
            );
            Ok(())
        }
    }
}

/// レイアウト世代バックアップからの復旧（#177。ローカル処理・IPC 不要）。
/// GUI 死亡・縮退 layout 保存後の復旧手段なので、GUI 内蔵の MCP からは提供できない
/// （GUI が生きていれば復旧は不要。開発不変条件の例外は requirements.md FR-5 参照）
fn recover_local(args: &RecoverArgs) -> Result<(), String> {
    let path = tako_control::layout::layout_path()
        .ok_or_else(|| "データディレクトリを解決できない（HOME 未設定等）".to_string())?;
    match args.apply.as_deref() {
        None => recover_list(&path),
        Some(generation) => recover_apply(&path, generation, args.force),
    }
}

/// layout.json とバックアップ世代の一覧（タブ数 / ペイン数 / 更新時刻）を表示する
fn recover_list(path: &std::path::Path) -> Result<(), String> {
    fn describe(path: &std::path::Path) -> String {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return "（無し）".into(),
        };
        let age = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| {
                let secs = d.as_secs();
                if secs < 60 {
                    format!("{secs} 秒前")
                } else if secs < 3600 {
                    format!("{} 分前", secs / 60)
                } else if secs < 86400 {
                    format!("{} 時間前", secs / 3600)
                } else {
                    format!("{} 日前", secs / 86400)
                }
            })
            .unwrap_or_else(|| "更新時刻不明".into());
        match tako_control::layout::load_file(path) {
            Ok(layout) => format!(
                "{} タブ / {} ペイン（{age} 更新）",
                layout.tabs.len(),
                layout.pane_count()
            ),
            Err(e) => format!("読めない: {e}（{age} 更新）"),
        }
    }
    println!("layout.json         : {}", describe(path));
    for generation in 1..=3u32 {
        let bak = tako_control::config_io::backup_path(path, generation);
        println!("layout.json.bak.{generation}   : {}", describe(&bak));
    }
    // 良品スナップショット（#381: 最後に復元へ実際に成功した構成）
    let good = path.with_extension("json.good");
    println!("layout.json.good    : {}", describe(&good));
    eprintln!();
    eprintln!("復元するには: tako を終了（Cmd-Q）してから `tako recover --apply <世代>` →");
    eprintln!("tako を再起動すると復元されたレイアウトで立ち上がります。");
    eprintln!("実体の tmux セッションが生きていれば、実行中プロセスごと画面に戻ります。");
    eprintln!("（good = 最後に復元へ成功した良品。`tako recover --apply good` で戻せます）");
    Ok(())
}

/// バックアップ世代（1〜3 / good）を layout.json へ復元する
/// （現行は layout.json.pre-recover へ退避）
fn recover_apply(path: &std::path::Path, generation: &str, force: bool) -> Result<(), String> {
    let bak = match generation {
        "1" | "2" | "3" => tako_control::config_io::backup_path(path, generation.parse().unwrap()),
        "good" => path.with_extension("json.good"),
        other => {
            return Err(format!(
                "世代は 1〜3 または good で指定してください（指定: {other}）"
            ))
        }
    };
    // 稼働中の tako があると、復元した layout.json を定期保存が即上書きしてしまう。
    // discovery（control.json）と全プロセス走査の両方で確認する（#177 の教訓:
    // control.json は消えている・別を指していることがある）
    if !force {
        if let Some(pid) = tako_control::discovery::live_primary_pid() {
            return Err(format!(
                "tako（pid {pid}）が稼働中です。終了（Cmd-Q）してから実行してください（--force で強制実行）"
            ));
        }
        if tako_core::ports::other_tako_running() {
            return Err(
                "tako が稼働中です（定期保存が復元結果を上書きします）。終了してから実行するか、\
                 別のデータディレクトリの tako だと確かなら --force を付けてください"
                    .to_string(),
            );
        }
    }
    let layout = tako_control::layout::load_file(&bak)
        .map_err(|e| format!("バックアップ {} を読めない: {e}", bak.display()))?;
    if path.is_file() {
        let stash = path.with_extension("json.pre-recover");
        std::fs::copy(path, &stash).map_err(|e| format!("現在の layout.json の退避に失敗: {e}"))?;
        eprintln!("現在の layout.json → {} へ退避", stash.display());
    }
    std::fs::copy(&bak, path).map_err(|e| format!("復元コピーに失敗: {e}"))?;
    eprintln!(
        "{}（{} タブ / {} ペイン）→ layout.json へ復元しました。",
        bak.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| bak.display().to_string()),
        layout.tabs.len(),
        layout.pane_count()
    );
    eprintln!("tako を起動すると、このレイアウトで復元されます。");
    Ok(())
}

fn sleep_guard_local(sub: &SleepGuardCommand) -> Result<(), String> {
    match sub {
        SleepGuardCommand::Status => {
            let settings = tako_control::settings::load();
            let state = tako_control::sleep_guard::status(
                settings.sleep_guard_mode,
                settings.sleep_guard_power,
                settings.lid_sleep_mode,
            );
            if state.assertion_held {
                eprintln!("  idle-sleep: アサーション保持中");
            } else {
                eprintln!("  idle-sleep: アサーション未保持");
            }
            eprintln!("  モード: {}", state.mode.as_str());
            eprintln!("  電源条件: {}", state.power_condition.as_str());
            eprintln!(
                "  AC 電源: {}",
                if state.on_ac_power {
                    "接続中"
                } else {
                    "未接続"
                }
            );
            eprintln!("  蓋: {}", if state.lid_closed { "閉" } else { "開" });
            eprintln!(
                "  蓋閉じ防止: {} (sudoers: {})",
                state.lid_sleep_mode.as_str(),
                if state.sudoers_installed {
                    "登録済み"
                } else {
                    "未登録"
                }
            );
            eprintln!(
                "  disablesleep: {}",
                if state.lid_sleep_disabled {
                    "有効"
                } else {
                    "無効"
                }
            );
            eprintln!("  thermal: {}", state.thermal_state.as_str());
            if state.display_sleep_forced {
                eprintln!("  ディスプレイ: 消灯済み（蓋閉じ中）");
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&state.to_json()).unwrap()
            );
            Ok(())
        }
        SleepGuardCommand::Set {
            mode,
            power_condition,
            lid_sleep_mode,
        } => {
            let mut settings = tako_control::settings::load();
            if let Some(m) = mode {
                settings.sleep_guard_mode =
                    tako_control::sleep_guard::SleepGuardMode::from_str_opt(m).ok_or_else(
                        || {
                            format!(
                                "不明な mode: {m:?}（off / on / while-agents-running のいずれか）"
                            )
                        },
                    )?;
            }
            if let Some(pc) = power_condition {
                settings.sleep_guard_power =
                    tako_control::sleep_guard::PowerCondition::from_str_opt(pc).ok_or_else(
                        || format!("不明な power: {pc:?}（ac-only / always のいずれか）"),
                    )?;
            }
            if let Some(lsm) = lid_sleep_mode {
                settings.lid_sleep_mode = tako_control::sleep_guard::LidSleepMode::from_str_opt(
                    lsm,
                )
                .ok_or_else(|| {
                    format!(
                        "不明な lid-sleep-mode: {lsm:?}（off / while-agents-running のいずれか）"
                    )
                })?;
            }
            tako_control::settings::save(&settings)
                .map_err(|e| format!("設定の保存に失敗: {e}"))?;
            eprintln!(
                "  設定を変更しました: mode={}, power={}, lid-sleep={}",
                settings.sleep_guard_mode.as_str(),
                settings.sleep_guard_power.as_str(),
                settings.lid_sleep_mode.as_str(),
            );
            let state = tako_control::sleep_guard::status(
                settings.sleep_guard_mode,
                settings.sleep_guard_power,
                settings.lid_sleep_mode,
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&state.to_json()).unwrap()
            );
            Ok(())
        }
        SleepGuardCommand::InstallLidSleep => {
            eprintln!("蓋閉じ防止の sudoers 登録を行います...");
            eprintln!("  登録内容: pmset -a disablesleep 0/1 のみ NOPASSWD");
            eprintln!("  管理者パスワードの入力ダイアログが表示されます。");
            let result = tako_control::sleep_guard::install_sudoers()?;
            eprintln!("  {result}");
            let mut settings = tako_control::settings::load();
            settings.lid_sleep_mode = tako_control::sleep_guard::LidSleepMode::WhileAgentsRunning;
            tako_control::settings::save(&settings)
                .map_err(|e| format!("設定の保存に失敗: {e}"))?;
            eprintln!("  lid-sleep-mode を while-agents-running に設定しました。");
            eprintln!("  解除: tako sleep-guard remove-lid-sleep");
            Ok(())
        }
        SleepGuardCommand::RemoveLidSleep => {
            let result = tako_control::sleep_guard::remove_sudoers()?;
            eprintln!("  {result}");
            let mut settings = tako_control::settings::load();
            settings.lid_sleep_mode = tako_control::sleep_guard::LidSleepMode::Off;
            tako_control::settings::save(&settings)
                .map_err(|e| format!("設定の保存に失敗: {e}"))?;
            eprintln!("  lid-sleep-mode を off に設定しました。");
            Ok(())
        }
    }
}

fn agents_local(sub: &AgentsCommand) -> Result<(), String> {
    match sub {
        AgentsCommand::SyncRules {
            source,
            targets,
            json,
        } => {
            let result =
                tako_control::agents_sync::run_sync(source.as_deref(), targets.as_deref())?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
            } else {
                if let Some(results) = result.get("results").and_then(|v| v.as_array()) {
                    for r in results {
                        let agent = r["agent"].as_str().unwrap_or("?");
                        let action = r["action"].as_str().unwrap_or("?");
                        let path = r["path"].as_str().unwrap_or("");
                        let mark = match action {
                            "updated" | "created" => "✓",
                            "unchanged" => "─",
                            "skipped" => "△",
                            _ => "✗",
                        };
                        eprintln!("  {mark} {agent}: {action} ({path})");
                        if let Some(bak) = r["backup"].as_str() {
                            eprintln!("      バックアップ: {bak}");
                        }
                        if let Some(err) = r["error"].as_str() {
                            eprintln!("      {err}");
                        }
                    }
                }
            }
            Ok(())
        }
        AgentsCommand::Status { json } => {
            let result = tako_control::agents_sync::status()?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                );
            } else {
                let status = result["status"].as_str().unwrap_or("unknown");
                match status {
                    "not_configured" => {
                        eprintln!("△ エージェント共通ルール同期: 未設定");
                        eprintln!("  tako setup で正本ファイルを設定できます");
                    }
                    "source_missing" => {
                        let path = result["source_path"].as_str().unwrap_or("?");
                        eprintln!("✗ 正本ファイルが見つかりません: {path}");
                    }
                    "up_to_date" => {
                        eprintln!("✓ エージェント共通ルール同期: 最新");
                    }
                    "outdated" => {
                        eprintln!("△ エージェント共通ルール同期: ずれあり");
                        eprintln!("  tako agents sync-rules で同期できます");
                    }
                    _ => {
                        eprintln!("? 状態: {status}");
                    }
                }
                if let Some(agents) = result["agents"].as_array() {
                    for a in agents {
                        let name = a["agent"].as_str().unwrap_or("?");
                        let st = a["status"].as_str().unwrap_or("?");
                        let mark = match st {
                            "up_to_date" => "✓",
                            "not_installed" => "─",
                            "outdated" => "△",
                            "not_synced" => "△",
                            _ => "✗",
                        };
                        eprintln!("    {mark} {name}: {st}");
                    }
                }
            }
            Ok(())
        }
    }
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
                focus: if args.focus { Some(true) } else { None },
            }
        }
        Command::Preview(args) => Request::PreviewView {
            pane: target_pane(args.pane)?,
            zoom: args.zoom,
            zoom_in: args.zoom_in,
            zoom_out: args.zoom_out,
            reset: args.reset,
            page: args.page,
            pan_x: args.pan_x,
            pan_y: args.pan_y,
        },
        Command::PreviewOutline(args) => Request::PreviewOutline {
            pane: target_pane(args.pane)?,
            item: args.item,
        },
        Command::PreviewLinkList(args) => Request::PreviewLinkList {
            pane: target_pane(args.pane)?,
        },
        Command::PreviewFollowLink(args) => Request::PreviewFollowLink {
            pane: target_pane(args.pane)?,
            index: args.index,
        },
        Command::PreviewReload(args) => Request::PreviewReload {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::PreviewCache(args) => Request::PreviewCache {
            max_mb: args.max_mb,
        },
        Command::PreviewChangelog(args) => Request::PreviewChangelog {
            pane: target_pane(args.pane)?,
            enabled: args.mode.as_deref().map(|s| s == "on"),
            max_count: args.max_count,
            expand: args.expand.clone(),
        },
        Command::Edit(command) => match command {
            EditCommand::Start { pane } => Request::PreviewEdit {
                pane: target_pane(*pane)?,
                enabled: Some(true),
            },
            EditCommand::Stop { pane } => Request::PreviewEdit {
                pane: target_pane(*pane)?,
                enabled: Some(false),
            },
            EditCommand::Status { pane } => Request::PreviewEdit {
                pane: target_pane(*pane)?,
                enabled: None,
            },
            EditCommand::Apply { text, pane } => Request::PreviewApply {
                pane: target_pane(*pane)?,
                text: text.clone(),
            },
            EditCommand::Save { pane } => Request::PreviewSave {
                pane: target_pane(*pane)?,
            },
            EditCommand::Undo { pane } => Request::PreviewUndo {
                pane: target_pane(*pane)?,
            },
            EditCommand::Redo { pane } => Request::PreviewRedo {
                pane: target_pane(*pane)?,
            },
            EditCommand::Search {
                query,
                direction,
                pane,
            } => Request::PreviewSearch {
                pane: target_pane(*pane)?,
                query: query.clone(),
                direction: Some(direction.clone()),
            },
            EditCommand::Replace {
                query,
                replacement,
                all,
                pane,
            } => Request::PreviewReplace {
                pane: target_pane(*pane)?,
                query: query.clone(),
                replacement: replacement.clone(),
                all: Some(*all),
            },
            EditCommand::Autosave { enabled, pane } => Request::PreviewAutosave {
                pane: target_pane(*pane)?,
                enabled: *enabled,
            },
        },
        Command::Tab(TabCommand::New { title, focus }) => Request::TabNew {
            title: title.clone(),
            focus: if *focus { Some(true) } else { None },
        },
        Command::Tab(TabCommand::Rename { tab, source, title }) => Request::TabRename {
            pane: if tab.is_none() {
                target_pane(None)?
            } else {
                None
            },
            tab: *tab,
            title: title.join(" "),
            source: source.clone(),
        },
        Command::Tab(TabCommand::Select { tab }) => Request::TabSelect { tab: *tab },
        Command::Window(WindowCommand::List) => Request::WindowList,
        Command::Window(WindowCommand::New { tab }) => Request::WindowNew { tab: *tab },
        Command::Window(WindowCommand::Close { window }) => {
            Request::WindowClose { window: *window }
        }
        Command::Window(WindowCommand::MoveTab { tab, window }) => Request::WindowMoveTab {
            tab: *tab,
            window: *window,
        },
        Command::Window(WindowCommand::Focus { window }) => {
            Request::WindowFocus { window: *window }
        }
        Command::Tab(TabCommand::Reorder { tab, index }) => Request::TabReorder {
            tab: *tab,
            index: *index,
        },
        Command::Tab(TabCommand::MovePane {
            tab,
            target,
            new,
            pane,
            right,
            down,
            up,
            left,
            focus,
        }) => {
            // 方向フラグは --target 指定時のみ有効（黙って無視せず明示エラーにする）
            if (*right || *down || *up || *left) && target.is_none() {
                return Err("--right/--down/--up/--left は --target と併用する".into());
            }
            if !new && tab.is_none() && target.is_none() {
                return Err("tab か --target か --new のいずれかを指定する".into());
            }
            Request::MovePane {
                pane: target_pane(*pane)?,
                tab: if *new { None } else { *tab },
                target: *target,
                direction: target.map(|_| match (down, up, left) {
                    (true, _, _) => Direction::Down,
                    (_, true, _) => Direction::Up,
                    (_, _, true) => Direction::Left,
                    _ => Direction::Right,
                }),
                focus: if *focus { Some(true) } else { None },
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
                "orch" => tako_control::protocol::PanelViewWire::Orch,
                _ => tako_control::protocol::PanelViewWire::Tmux,
            }),
            filetree: args.filetree.as_deref().map(|s| s == "on"),
            sidebar_width: args.sidebar_width,
        },
        Command::Portdetect(args) => Request::PortDetect {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::ConfirmClose(args) => Request::ConfirmClose {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Theme(args) => Request::Theme {
            action: args.mode.as_deref().map(|m| {
                if m == "toggle" {
                    "toggle".to_string()
                } else {
                    "set".to_string()
                }
            }),
            mode: args.mode.clone().filter(|m| m != "toggle"),
        },
        Command::Lang(args) => Request::Lang {
            action: args.value.as_deref().map(|_| "set".to_string()),
            value: args.value.clone(),
        },
        Command::LimitService(args) => Request::LimitService {
            action: if args.refresh {
                Some("refresh".to_string())
            } else {
                args.service.as_ref().map(|_| "set".to_string())
            },
            service: args.service.clone(),
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
            pane: if args.tab.is_some() {
                None
            } else {
                target_pane(args.pane)?
            },
            tab: args.tab,
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
        Command::File(FileCommand::Open { path }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenDefault,
            path: resolve_cli_path(path),
            name: None,
            pane: None,
        },
        Command::File(FileCommand::OpenWith { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenWith,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
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
        Command::Video(VideoCommand::Mute { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "toggle_mute".into(),
        },
        Command::Video(VideoCommand::Unmute { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "unmute".into(),
        },
        Command::Video(VideoCommand::Loop { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "toggle_loop".into(),
        },
        Command::Video(VideoCommand::Volume { volume, pane }) => Request::VideoVolume {
            pane: target_pane(*pane)?,
            volume: *volume,
        },
        Command::Orchestrator(OrchestratorCommand::Spawn {
            project,
            prompt,
            label,
            agent,
            model,
            effort,
            pane,
            tab,
            task_type,
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
                caller_role: std::env::var("TAKO_ORCHESTRATOR_ROLE").ok(),
                agent: agent.clone(),
                caller_pid: Some(std::process::id()),
                task_type: task_type.clone(),
            }
        }
        Command::Orchestrator(OrchestratorCommand::SelfInfo { .. }) => {
            unreachable!("orchestrator self は run() を通らない（main() でローカル処理済み）")
        }
        Command::Orchestrator(OrchestratorCommand::Handoff { .. }) => {
            unreachable!("orchestrator handoff は run() を通らない（main() でローカル処理済み）")
        }
        Command::Orchestrator(OrchestratorCommand::Status {
            pane,
            worker,
            session_id,
            tmux_session,
        }) => Request::OrchestratorWorkerStatus {
            pane_id: *pane,
            session_id: session_id.clone(),
            tmux_session: tmux_session.clone(),
            worker: worker.clone(),
        },
        Command::Orchestrator(OrchestratorCommand::Workers { .. }) => {
            unreachable!("orchestrator workers は run() を通らない（main() でローカル処理済み）")
        }
        Command::Orchestrator(OrchestratorCommand::Supervisor { .. }) => {
            unreachable!("orchestrator supervisor は run() を通らない（main() でローカル処理済み）")
        }
        // remote コマンドは main() でローカル処理済みのため到達不能
        Command::Remote(_) => unreachable!("remote は run() を通らない"),
        // main() で分岐済みのため論理的に到達不能
        Command::Mcp(_) => unreachable!("mcp serve は run() を通らない"),
        Command::Setup(_) => unreachable!("setup は run() を通らない"),
        Command::SetupMcp(_) => unreachable!("setup-mcp は run() を通らない"),
        Command::Master { .. } => {
            unreachable!("master は run() を通らない（直接 orchestrator_master() を呼ぶ）")
        }
        Command::Solo { .. } => {
            unreachable!("solo は run() を通らない（直接 orchestrator_solo() を呼ぶ）")
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
        Command::Orchestrator(OrchestratorCommand::RunStatus { .. }) => {
            unreachable!("orchestrator run-status は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::RunResult { .. }) => {
            unreachable!("orchestrator run-result は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Layout { .. }) => {
            unreachable!("orchestrator layout は run() を通らない（ローカルで config.yaml を操作）")
        }
        Command::Orchestrator(OrchestratorCommand::Report { .. }) => {
            unreachable!("orchestrator report は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Respond { .. }) => {
            unreachable!("orchestrator respond は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Ledger(_)) => {
            unreachable!("orchestrator ledger は run() を通らない（ローカル処理）")
        }
        Command::Web(sub) => {
            let dir = |right: bool, down: bool, left: bool, up: bool| match (down, left, up) {
                (true, _, _) => Some(Direction::Down),
                (_, true, _) => Some(Direction::Left),
                (_, _, true) => Some(Direction::Up),
                _ if right => Some(Direction::Right),
                _ => None,
            };
            // Request::Web は enum バリアントのため record update が使えない。
            // 全フィールドを引数で受けるビルダで各アームの重複を抑える
            #[allow(clippy::too_many_arguments)]
            fn web(
                action: &str,
                url: Option<String>,
                id: Option<u64>,
                pane: Option<u64>,
                direction: Option<Direction>,
                to: Option<String>,
                js: Option<String>,
                token: Option<u64>,
                focus: Option<bool>,
            ) -> Request {
                Request::Web {
                    action: action.to_string(),
                    url,
                    id,
                    pane,
                    direction,
                    to,
                    js,
                    token,
                    focus,
                }
            }
            match sub {
                WebCommand::Open {
                    url,
                    pane,
                    right,
                    down,
                    left,
                    up,
                    focus,
                } => {
                    // 基準ペインは任意: tako 外（別インスタンス操作・スクリプト）からは
                    // 省略のまま送り、アプリ側がフォーカスペインへ解決する（OpenFile と同じ）
                    let pane = pane.or_else(caller_pane);
                    let d = dir(*right, *down, *left, *up);
                    let f = if *focus { Some(true) } else { None };
                    web(
                        "open",
                        Some(url.clone()),
                        None,
                        pane,
                        d,
                        None,
                        None,
                        None,
                        f,
                    )
                }
                WebCommand::List => web("list", None, None, None, None, None, None, None, None),
                WebCommand::Show {
                    id,
                    pane,
                    right,
                    down,
                    left,
                    up,
                    focus,
                } => {
                    let pane = pane.or_else(caller_pane);
                    let d = dir(*right, *down, *left, *up);
                    let f = if *focus { Some(true) } else { None };
                    web("show", None, Some(*id), pane, d, None, None, None, f)
                }
                WebCommand::Hide { id, pane } => {
                    web("hide", None, *id, *pane, None, None, None, None, None)
                }
                WebCommand::Close { id, pane } => {
                    web("close", None, *id, *pane, None, None, None, None, None)
                }
                WebCommand::Nav { to, id, pane } => web(
                    "navigate",
                    None,
                    *id,
                    *pane,
                    None,
                    Some(to.clone()),
                    None,
                    None,
                    None,
                ),
                WebCommand::Eval { js, id, pane } => web(
                    "eval",
                    None,
                    *id,
                    *pane,
                    None,
                    None,
                    Some(js.clone()),
                    None,
                    None,
                ),
                WebCommand::EvalResult { token, id, pane } => web(
                    "eval_result",
                    None,
                    *id,
                    *pane,
                    None,
                    None,
                    None,
                    Some(*token),
                    None,
                ),
                WebCommand::Read { id, pane } => {
                    web("read", None, *id, *pane, None, None, None, None, None)
                }
            }
        }
        Command::Update(sub) => {
            let (action, channel) = match sub {
                UpdateCommand::Status => ("status", None),
                UpdateCommand::Check { channel } => ("check", channel.clone()),
                UpdateCommand::Apply { channel } => ("apply", channel.clone()),
                UpdateCommand::ApplyZip { channel } => ("apply-zip", channel.clone()),
                UpdateCommand::Repair => ("repair", None),
            };
            Request::Update {
                action: Some(action.to_string()),
                channel,
            }
        }
        Command::Telemetry(sub) => Request::Telemetry {
            action: Some(match sub {
                TelemetryCommand::Status => "status".to_string(),
                TelemetryCommand::On => "on".to_string(),
                TelemetryCommand::Off => "off".to_string(),
            }),
        },
        Command::Fda(sub) => Request::Fda {
            action: Some(match sub {
                FdaCommand::Status => "status".to_string(),
                FdaCommand::Open => "open".to_string(),
            }),
        },
        Command::SleepGuard(sub) => match sub {
            SleepGuardCommand::Status => Request::SleepGuard {
                action: Some("status".to_string()),
                mode: None,
                power_condition: None,
                lid_sleep_mode: None,
            },
            SleepGuardCommand::Set {
                mode,
                power_condition,
                lid_sleep_mode,
            } => Request::SleepGuard {
                action: Some("set".to_string()),
                mode: mode.clone(),
                power_condition: power_condition.clone(),
                lid_sleep_mode: lid_sleep_mode.clone(),
            },
            SleepGuardCommand::InstallLidSleep => Request::SleepGuard {
                action: Some("install-lid-sleep".to_string()),
                mode: None,
                power_condition: None,
                lid_sleep_mode: None,
            },
            SleepGuardCommand::RemoveLidSleep => Request::SleepGuard {
                action: Some("remove-lid-sleep".to_string()),
                mode: None,
                power_condition: None,
                lid_sleep_mode: None,
            },
        },
        Command::Tree(sub) => match sub {
            TreeCommand::Add { path, tab } => Request::TreeFolder {
                action: "add".to_string(),
                path: Some(resolve_cli_path(path)),
                tab: *tab,
                pane: caller_pane(),
            },
            TreeCommand::Remove { path, tab } => Request::TreeFolder {
                action: "remove".to_string(),
                path: Some(resolve_cli_path(path)),
                tab: *tab,
                pane: caller_pane(),
            },
            TreeCommand::List { tab } => Request::TreeFolder {
                action: "list".to_string(),
                path: None,
                tab: *tab,
                pane: caller_pane(),
            },
        },
        Command::Sessions(sub) => match sub {
            SessionsCommand::List {
                role,
                project,
                limit,
                ..
            } => Request::Sessions {
                action: "list".to_string(),
                id: None,
                role: role.clone(),
                project: project.clone(),
                limit: *limit,
                pane: None,
                tab: None,
                direction: None,
            },
            SessionsCommand::Show { id } => Request::Sessions {
                action: "show".to_string(),
                id: Some(id.clone()),
                role: None,
                project: None,
                limit: None,
                pane: None,
                tab: None,
                direction: None,
            },
            SessionsCommand::Resume {
                id,
                pane,
                tab,
                direction,
            } => Request::Sessions {
                action: "resume".to_string(),
                id: Some(id.clone()),
                role: None,
                project: None,
                limit: None,
                // 明示指定 → 呼び出し元ペイン（TAKO_PANE_ID）→ None。
                // None は dispatch がアクティブタブへフォールバックする
                // （tako 外の CLI からの消失復旧を想定）
                pane: if tab.is_some() {
                    None
                } else {
                    pane.or_else(caller_pane)
                },
                tab: *tab,
                direction: direction.as_deref().map(parse_direction).transpose()?,
            },
        },
        Command::Logs(sub) => match sub {
            LogsCommand::List => Request::Logs {
                action: "list".to_string(),
                pane: None,
                session_id: None,
                lines: None,
                enabled: None,
                max_mb: None,
                total_max_mb: None,
            },
            LogsCommand::Show {
                pane,
                session,
                lines,
            } => Request::Logs {
                action: "read".to_string(),
                // セッション指定が無ければペイン（省略時は呼び出し元）のログを引く
                pane: if session.is_some() {
                    *pane
                } else {
                    target_pane(*pane)?
                },
                session_id: session.clone(),
                lines: *lines,
                enabled: None,
                max_mb: None,
                total_max_mb: None,
            },
            LogsCommand::Status => Request::Logs {
                action: "status".to_string(),
                pane: None,
                session_id: None,
                lines: None,
                enabled: None,
                max_mb: None,
                total_max_mb: None,
            },
            LogsCommand::Set {
                enabled,
                max_mb,
                total_max_mb,
            } => Request::Logs {
                action: "set".to_string(),
                pane: None,
                session_id: None,
                lines: None,
                enabled: *enabled,
                max_mb: *max_mb,
                total_max_mb: *total_max_mb,
            },
        },
        Command::Agents(_) => unreachable!("agents は run() を通らない"),
        Command::Recover(_) => unreachable!("recover は run() を通らない（ローカル処理）"),
        Command::OpenIn(sub) => match sub {
            OpenInCommand::Dir { path, no_focus } => Request::OpenDir {
                path: resolve_cli_path(path),
                focus: Some(!no_focus),
            },
            OpenInCommand::Repo { path, no_focus } => {
                let resolved = resolve_cli_path(path);
                let dir = std::path::PathBuf::from(&resolved);
                let git_root = find_git_root_cli(&dir).unwrap_or(resolved);
                Request::OpenDir {
                    path: git_root,
                    focus: Some(!no_focus),
                }
            }
            OpenInCommand::Remote { host, no_focus } => Request::OpenRemote {
                host: host.clone(),
                focus: Some(!no_focus),
            },
        },
        Command::Recent(sub) => match sub {
            RecentCommand::List => Request::RecentItems {
                action: "list".into(),
            },
            RecentCommand::Clear => Request::RecentItems {
                action: "clear".into(),
            },
        },
        Command::SshHosts => Request::SshHosts,
        Command::Task(sub) => match sub {
            TaskCommand::Checkpoint {
                task_id,
                pane,
                issue,
                branch,
                phase,
                last_commit,
                agent,
                model,
                prompt_head,
                project,
                cwd,
            } => Request::TaskCheckpoint {
                action: "checkpoint".into(),
                task_id: task_id.clone(),
                pane: pane.or_else(caller_pane),
                issue: *issue,
                branch: branch.clone(),
                phase: phase.clone(),
                last_commit: last_commit.clone(),
                agent: agent.clone(),
                model: model.clone(),
                prompt_head: prompt_head.clone(),
                suspended_reason: None,
                project: project.clone(),
                cwd: cwd.clone(),
                resume_pane: None,
                tab: None,
                resume_model: None,
                caller_role: std::env::var("TAKO_ORCHESTRATOR_ROLE").ok(),
            },
            TaskCommand::List { phase, .. } => Request::TaskCheckpoint {
                action: "list".into(),
                task_id: None,
                pane: None,
                issue: None,
                branch: None,
                phase: phase.clone(),
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
            TaskCommand::Resume {
                task_id,
                model,
                pane,
                tab,
            } => Request::TaskCheckpoint {
                action: "resume".into(),
                task_id: Some(task_id.clone()),
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
                resume_pane: if tab.is_some() {
                    None
                } else {
                    pane.or_else(caller_pane)
                },
                tab: *tab,
                resume_model: model.clone(),
                caller_role: std::env::var("TAKO_ORCHESTRATOR_ROLE").ok(),
            },
            TaskCommand::Update {
                task_id,
                phase,
                reason,
            } => Request::TaskCheckpoint {
                action: "update".into(),
                task_id: Some(task_id.clone()),
                pane: None,
                issue: None,
                branch: None,
                phase: Some(phase.clone()),
                last_commit: None,
                agent: None,
                model: None,
                prompt_head: None,
                suspended_reason: reason.clone(),
                project: None,
                cwd: None,
                resume_pane: None,
                tab: None,
                resume_model: None,
                caller_role: None,
            },
            // gate は main() でローカル処理。ここには来ない
            TaskCommand::Gate(_) => unreachable!("gate は main() でローカル処理する"),
        },
        Command::RunInteractive(ref args) => {
            let direction = if args.down {
                Some(Direction::Down)
            } else {
                Some(Direction::Right)
            };
            Request::RunInteractive {
                pane: if args.tab.is_some() {
                    None
                } else {
                    target_pane(args.pane)?
                },
                tab: args.tab,
                command: args.command.clone(),
                input_hint: args.hint.clone(),
                direction,
                ratio: args.ratio,
                auto_close: Some(args.auto_close.clone()),
            }
        }
        Command::RunInteractiveStatus(ref args) => Request::RunInteractiveStatus {
            pane: args.pane,
            no_wait: false,
        },
        Command::Run(ref args) => {
            let direction = if args.right {
                Some(Direction::Right)
            } else {
                Some(Direction::Down)
            };
            Request::Run {
                path: args.file.clone(),
                pane: if args.tab.is_some() {
                    None
                } else {
                    target_pane(args.pane)?
                },
                tab: args.tab,
                profile: args.profile.clone(),
                command: args.command.clone(),
                direction,
                ratio: args.ratio,
                auto_close: Some(args.auto_close.clone()),
                focus: Some(args.focus),
            }
        }
        Command::RunDefault(ref args) => Request::RunnerDefaults {
            ext: args.ext.clone(),
            command: args.command.clone(),
            remove: args.remove,
        },
    })
}

/// 委任台帳のローカル処理（YAML I/O のみ。IPC 不要。#292）
fn ledger_cli(sub: &LedgerCommand) -> Result<(), String> {
    use tako_control::orchestrator::ledger;
    match sub {
        LedgerCommand::List {
            project,
            task_type,
            limit,
        } => {
            let l = ledger::Ledger::load()?;
            let mut entries: Vec<&ledger::LedgerEntry> = l.entries.iter().collect();
            if let Some(p) = project {
                entries.retain(|e| e.project == *p);
            }
            if let Some(t) = task_type {
                entries.retain(|e| e.task_type == *t);
            }
            if entries.len() > *limit {
                entries = entries[entries.len() - *limit..].to_vec();
            }
            let result = serde_json::json!({
                "entries": entries,
                "total": l.entries.len(),
                "unevaluated": l.unevaluated_count(),
            });
            println!("{}", pretty_json(&result));
            Ok(())
        }
        LedgerCommand::Stats => {
            let l = ledger::Ledger::load()?;
            let stats = l.stats();
            let result = serde_json::json!({
                "stats": stats,
                "total_entries": l.entries.len(),
                "unevaluated": l.unevaluated_count(),
            });
            println!("{}", pretty_json(&result));
            Ok(())
        }
        LedgerCommand::Record {
            id,
            outcome,
            rounds,
            note,
        } => {
            ledger::record_outcome(id, outcome, *rounds, note.as_deref())?;
            println!("recorded: {id} -> {outcome}");
            Ok(())
        }
        LedgerCommand::Amend { id, note } => {
            ledger::amend_entry(id, note)?;
            println!("amended: {id} (post_issue=true)");
            Ok(())
        }
        LedgerCommand::Prune { project_prefix } => {
            let removed = ledger::Ledger::mutate(|l| l.prune_by_project_prefix(project_prefix))?;
            println!("pruned: {removed} entries with project prefix '{project_prefix}'");
            Ok(())
        }
    }
}

/// run-interactive --wait: 起動 → ポーリングで完了待ち → exit code を返す
fn run_interactive_wait(command: &Command) -> Result<(), String> {
    let request = build_request(command)?;
    let result = send_request(request)?;
    let pane = result["pane"]
        .as_u64()
        .ok_or("run-interactive が pane ID を返さなかった")?;
    println!(
        "pane {pane} で対話コマンドを起動しました（status: {}）",
        result["status"].as_str().unwrap_or("?")
    );

    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let status = send_request(Request::RunInteractiveStatus {
            pane,
            no_wait: false,
        })?;
        if status["status"].as_str() == Some("exited") {
            println!("{}", pretty_json(&status));
            let code = status["exit_code"].as_i64().unwrap_or(1);
            if code != 0 {
                return Err(format!("コマンドが exit code {code} で終了"));
            }
            return Ok(());
        }
    }
}

/// run --wait: 起動 → ポーリングで完了待ち → exit code を返す
fn run_wait(command: &Command) -> Result<(), String> {
    let request = build_request(command)?;
    let result = send_request(request)?;
    let pane = result["pane"]
        .as_u64()
        .ok_or("run が pane ID を返さなかった")?;
    println!(
        "pane {pane} でコマンドを実行中（command: {}）",
        result["command"].as_str().unwrap_or("?")
    );

    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let status = send_request(Request::RunInteractiveStatus {
            pane,
            no_wait: false,
        })?;
        if status["status"].as_str() == Some("exited") {
            println!("{}", pretty_json(&status));
            let code = status["exit_code"].as_i64().unwrap_or(1);
            if code != 0 {
                return Err(format!("コマンドが exit code {code} で終了"));
            }
            return Ok(());
        }
    }
}

/// run --list: ファイルの実行プロファイル一覧を表示する（実行しない）
fn run_list(command: &Command) -> Result<(), String> {
    let Command::Run(args) = command else {
        return Err("内部エラー: run --list に非 Run コマンド".into());
    };
    let request = Request::RunResolve {
        path: args.file.clone(),
        pane: target_pane(args.pane)?,
    };
    let result = send_request(request)?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// gate 操作のローカル処理（YAML I/O + コマンド実行。IPC 不要。#244）
fn gate_cli(sub: &GateCommand) -> Result<(), String> {
    match sub {
        GateCommand::Set {
            task_id,
            commands,
            pr_merged,
            customs,
            cwd,
            json,
        } => {
            let criteria_json = build_criteria_json(commands, pr_merged, customs)?;
            let result = tako_control::acceptance_gates::set_gate_payload(
                task_id,
                &criteria_json,
                cwd.as_deref(),
            )?;
            if *json {
                println!("{}", pretty_json(&result));
            } else {
                print_gate_result(&result);
            }
            Ok(())
        }
        GateCommand::Check {
            task_id,
            no_sync,
            json,
        } => {
            let result = tako_control::acceptance_gates::execute_gate_check(task_id, !no_sync)?;
            if *json {
                println!("{}", pretty_json(&result));
            } else {
                print_gate_result(&result);
            }
            Ok(())
        }
        GateCommand::Show { task_id, json } => {
            let result = tako_control::acceptance_gates::show_gate_payload(task_id)?;
            if *json {
                println!("{}", pretty_json(&result));
            } else {
                print_gate_result(&result);
            }
            Ok(())
        }
    }
}

/// CLI の --command / --pr-merged / --custom フラグから criteria JSON を組み立てる
fn build_criteria_json(
    commands: &[String],
    pr_merged: &[u32],
    customs: &[String],
) -> Result<String, String> {
    if commands.is_empty() && pr_merged.is_empty() && customs.is_empty() {
        return Err("少なくとも 1 つの述語を指定する（--command / --pr-merged / --custom）".into());
    }
    let mut criteria = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        criteria.push(serde_json::json!({
            "id": format!("cmd_{}", i + 1),
            "kind": { "type": "command", "cmd": cmd },
        }));
    }
    for pr in pr_merged {
        criteria.push(serde_json::json!({
            "id": format!("pr_{pr}"),
            "kind": { "type": "pr_merged", "pr_number": pr },
        }));
    }
    for (i, desc) in customs.iter().enumerate() {
        criteria.push(serde_json::json!({
            "id": format!("custom_{}", i + 1),
            "kind": { "type": "custom", "description": desc },
        }));
    }
    serde_json::to_string(&criteria).map_err(|e| format!("JSON 変換に失敗: {e}"))
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

fn find_git_root_cli(dir: &std::path::Path) -> Option<String> {
    tako_core::git::repo_root(dir).map(|p| p.to_string_lossy().into_owned())
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

/// `tako sessions list` の人間向け表示（1 セッション 1 行 + pending 節）
fn print_sessions_list(result: &Value) {
    let sessions = result["sessions"].as_array().cloned().unwrap_or_default();
    if sessions.is_empty() {
        println!("カタログにセッションが無い（claude ペインの検出後に記録される）");
    }
    for s in &sessions {
        let issues = s["issues"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64())
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|t| !t.is_empty())
            .map(|t| format!(" [{t}]"))
            .unwrap_or_default();
        let name = match (s["project"].as_str(), s["label"].as_str()) {
            (Some(p), Some(l)) => format!("{p}: {l}"),
            (_, Some(l)) => l.to_string(),
            (Some(p), None) => p.to_string(),
            _ => "-".into(),
        };
        let resumable = if s["resumable"].as_bool() == Some(true) {
            ""
        } else {
            "（resume 不可）"
        };
        println!(
            "{}  {}  {:6}  {}{}{}",
            s["short_id"].as_str().unwrap_or("-"),
            s["last_seen_at"].as_str().unwrap_or("-"),
            s["kind"].as_str().unwrap_or("-"),
            name,
            issues,
            resumable,
        );
    }
    let pending = result["pending"].as_array().cloned().unwrap_or_default();
    if !pending.is_empty() {
        println!("--- session 未検出の spawn 記録（codex / agy・起動直後の claude）---");
        for p in &pending {
            println!(
                "{}  {}  {}  {}",
                p["recorded_at"].as_str().unwrap_or("-"),
                p["agent"].as_str().unwrap_or("-"),
                p["tmux_session"].as_str().unwrap_or("-"),
                p["label"].as_str().or(p["project"].as_str()).unwrap_or("-"),
            );
        }
    }
    eprintln!("(resume: tako sessions resume <id> / 詳細: tako sessions show <id>)");
}

fn print_task_list(result: &Value) {
    let checkpoints = result["checkpoints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if checkpoints.is_empty() {
        println!("チェックポイントが無い");
        return;
    }
    for cp in &checkpoints {
        let issue = cp["issue"]
            .as_u64()
            .map(|n| format!(" #{n}"))
            .unwrap_or_default();
        let branch = cp["branch"]
            .as_str()
            .map(|b| format!("  branch:{b}"))
            .unwrap_or_default();
        let reason = cp["suspended_reason"]
            .as_str()
            .map(|r| format!("  ({r})"))
            .unwrap_or_default();
        println!(
            "{:<12}  {:10}  pane:{}{}{}{}",
            cp["task_id"].as_str().unwrap_or("-"),
            cp["phase"].as_str().unwrap_or("-"),
            cp["pane_id"]
                .as_u64()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            issue,
            branch,
            reason,
        );
    }
    eprintln!(
        "(resume: tako task resume <task_id> / update: tako task update <task_id> --phase ...)"
    );
}

fn print_gate_result(result: &Value) {
    let task_id = result["task_id"].as_str().unwrap_or("-");
    let overall = result["overall"].as_str().unwrap_or("?");
    let overall_marker = match overall {
        "passed" => "[PASSED]",
        "failed" => "[FAILED]",
        _ => "[PENDING]",
    };
    println!("Gate: {task_id}  {overall_marker}");
    if let Some(criteria) = result["criteria"].as_array() {
        for c in criteria {
            let id = c["id"].as_str().unwrap_or("-");
            let status = c["status"].as_str().unwrap_or("?");
            let marker = match status {
                "passed" => "[PASSED]",
                "failed" => "[FAILED]",
                _ => "[      ]",
            };
            let kind_type = c["kind"]["type"].as_str().unwrap_or("?");
            let kind_detail = match kind_type {
                "command" => c["kind"]["cmd"].as_str().unwrap_or("").to_string(),
                "pr_merged" => format!("PR #{}", c["kind"]["pr_number"].as_u64().unwrap_or(0)),
                "custom" => c["kind"]["description"].as_str().unwrap_or("").to_string(),
                _ => String::new(),
            };
            println!("  {marker} {id}: {kind_detail}");
            if let Some(ev) = c["evidence"].as_str() {
                let ev_short = if ev.len() > 120 {
                    format!("{}...", &ev[..120])
                } else {
                    ev.to_string()
                };
                println!("         {ev_short}");
            }
        }
    }
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
            if let Some(status) = result.get("input_status").filter(|v| !v.is_null()) {
                eprintln!(
                    "[input_status] style={} text={:?}",
                    status["style"].as_str().unwrap_or("?"),
                    status["text"].as_str().unwrap_or(""),
                );
            }
        }
        Command::Scroll(_) => println!("{result}"),
        Command::List => {
            println!("{}", pretty_json(result));
        }
        Command::Tab(TabCommand::New { .. }) => println!("{result}"),
        Command::Window(WindowCommand::List) => println!("{}", pretty_json(result)),
        Command::Window(
            WindowCommand::New { .. } | WindowCommand::Close { .. } | WindowCommand::MoveTab { .. },
        ) => println!("{result}"),
        Command::Open(_) | Command::Preview(_) | Command::PreviewOutline(_) | Command::Edit(_) => {
            println!("{result}")
        }
        Command::PreviewLinkList(_) => println!("{}", pretty_json(result)),
        Command::PreviewFollowLink(_) => println!("{result}"),
        Command::PreviewReload(_) | Command::PreviewCache(_) | Command::PreviewChangelog(_) => {
            println!("{result}")
        }
        Command::Autorename(_)
        | Command::Portdetect(_)
        | Command::Persist(_)
        | Command::ConfirmClose(_)
        | Command::Theme(_)
        | Command::Lang(_)
        | Command::LimitService(_)
        | Command::Telemetry(_)
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
        Command::Web(_) => println!("{}", pretty_json(result)),
        Command::Update(_) => println!("{}", pretty_json(result)),
        Command::Tree(_) => println!("{}", pretty_json(result)),
        Command::Sessions(SessionsCommand::List { json, .. }) => {
            if *json {
                println!("{}", pretty_json(result));
            } else {
                print_sessions_list(result);
            }
        }
        Command::Sessions(SessionsCommand::Show { .. }) => {
            println!("{}", pretty_json(result));
        }
        Command::Sessions(SessionsCommand::Resume { .. }) => {
            if let (Some(pane), Some(sid)) =
                (result["pane"].as_u64(), result["session_id"].as_str())
            {
                eprintln!(
                    "復元しました: ペイン {pane}（session {}…, cwd {}）",
                    &sid[..sid.len().min(8)],
                    result["cwd"].as_str().unwrap_or("-"),
                );
            }
            println!("{result}");
        }
        Command::Logs(LogsCommand::Show { .. }) => {
            if let Some(content) = result["content"].as_str() {
                println!("{content}");
            }
            if let Some(path) = result["path"].as_str() {
                eprintln!("[log] {path}");
            }
        }
        Command::Logs(_) => println!("{}", pretty_json(result)),
        Command::OpenIn(_) => println!("{}", pretty_json(result)),
        Command::Recent(_) => println!("{}", pretty_json(result)),
        Command::SshHosts => println!("{}", pretty_json(result)),
        Command::Task(TaskCommand::List { json, .. }) => {
            if *json {
                println!("{}", pretty_json(result));
            } else {
                print_task_list(result);
            }
        }
        // gate は main() でローカル処理。ここには来ない
        Command::Task(_) => println!("{}", pretty_json(result)),
        Command::RunInteractive(_) => {
            println!("{}", pretty_json(result));
        }
        Command::RunInteractiveStatus(_) => {
            println!("{}", pretty_json(result));
        }
        Command::Run(_) => {
            println!("{}", pretty_json(result));
        }
        Command::RunDefault(_) => {
            println!("{}", pretty_json(result));
        }
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
                focus: None,
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
                focus: None,
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
                focus: None,
            }
        );
        // tab と --target の併用は clap が拒否、--target なしの方向指定は build_request が拒否
        assert!(Cli::try_parse_from(["tako", "tab", "move-pane", "4", "--target", "7"]).is_err());
        let command = parse(&["tako", "tab", "move-pane", "4", "--pane", "9", "--down"]);
        assert!(build_request(&command).is_err());
        // --new は新タブ化（Issue #209）
        let command = parse(&["tako", "tab", "move-pane", "--new", "--pane", "9"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::MovePane {
                pane: Some(9),
                tab: None,
                target: None,
                direction: None,
                focus: None,
            }
        );
        // --new は tab / --target と排他
        assert!(
            Cli::try_parse_from(["tako", "tab", "move-pane", "4", "--new", "--pane", "9"]).is_err()
        );
        assert!(Cli::try_parse_from([
            "tako",
            "tab",
            "move-pane",
            "--target",
            "7",
            "--new",
            "--pane",
            "9"
        ])
        .is_err());
        // tab / target / new すべて省略はエラー
        let command = parse(&["tako", "tab", "move-pane", "--pane", "9"]);
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
                focus: None,
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
    fn editサブコマンドを操作へ写す() {
        let command = parse(&["tako", "edit", "start", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewEdit {
                pane: Some(5),
                enabled: Some(true),
            }
        );
        let command = parse(&["tako", "edit", "apply", "日本語\n", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewApply {
                pane: Some(5),
                text: "日本語\n".into(),
            }
        );
        let command = parse(&["tako", "edit", "save", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewSave { pane: Some(5) }
        );
        let command = parse(&["tako", "edit", "undo", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewUndo { pane: Some(5) }
        );
        let command = parse(&["tako", "edit", "redo", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewRedo { pane: Some(5) }
        );
        let command = parse(&["tako", "edit", "search", "hello", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewSearch {
                pane: Some(5),
                query: Some("hello".into()),
                direction: Some("next".into()),
            }
        );
        let command = parse(&[
            "tako", "edit", "replace", "old", "new", "--all", "--pane", "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewReplace {
                pane: Some(5),
                query: "old".into(),
                replacement: "new".into(),
                all: Some(true),
            }
        );
        let command = parse(&["tako", "edit", "autosave", "true", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewAutosave {
                pane: Some(5),
                enabled: Some(true),
            }
        );
    }

    #[test]
    fn previewは倍率ページパンを操作へ写す() {
        let command = parse(&[
            "tako", "preview", "--pane", "5", "--zoom", "150", "--page", "3", "--pan-x", "24",
            "--pan-y", "48",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewView {
                pane: Some(5),
                zoom: Some(150.0),
                zoom_in: false,
                zoom_out: false,
                reset: false,
                page: Some(3),
                pan_x: Some(24.0),
                pan_y: Some(48.0),
            }
        );
        assert!(Cli::try_parse_from(["tako", "preview", "--zoom", "150", "--zoom-in"]).is_err());
    }

    #[test]
    fn preview_outlineは一覧取得と項目ジャンプを操作へ写す() {
        let list = parse(&["tako", "preview-outline", "--pane", "5"]);
        assert_eq!(
            build_request(&list).unwrap(),
            Request::PreviewOutline {
                pane: Some(5),
                item: None,
            }
        );
        let jump = parse(&["tako", "preview-outline", "--pane", "5", "--item", "3"]);
        assert_eq!(
            build_request(&jump).unwrap(),
            Request::PreviewOutline {
                pane: Some(5),
                item: Some(3),
            }
        );
    }

    #[test]
    fn preview_reloadは状態取得と切替を操作へ写す() {
        let status = parse(&["tako", "preview-reload"]);
        assert_eq!(
            build_request(&status).unwrap(),
            Request::PreviewReload { enabled: None }
        );
        let disable = parse(&["tako", "preview-reload", "off"]);
        assert_eq!(
            build_request(&disable).unwrap(),
            Request::PreviewReload {
                enabled: Some(false)
            }
        );
    }

    #[test]
    fn preview_cacheは状態取得と上限変更を操作へ写す() {
        let status = parse(&["tako", "preview-cache"]);
        assert_eq!(
            build_request(&status).unwrap(),
            Request::PreviewCache { max_mb: None }
        );
        let changed = parse(&["tako", "preview-cache", "768"]);
        assert_eq!(
            build_request(&changed).unwrap(),
            Request::PreviewCache { max_mb: Some(768) }
        );
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
                source: None,
            }
        );
        let command2 = parse(&[
            "tako",
            "tab",
            "rename",
            "--tab",
            "5",
            "--source",
            "auto",
            "開発中",
        ]);
        assert_eq!(
            build_request(&command2).unwrap(),
            Request::TabRename {
                pane: None,
                tab: Some(5),
                title: "開発中".into(),
                source: Some("auto".into()),
            }
        );
    }

    #[test]
    fn run_interactiveのパースと変換() {
        let command = parse(&[
            "tako",
            "run-interactive",
            "sudo systemctl start foo",
            "--hint",
            "sudo password",
            "--pane",
            "5",
            "--down",
            "--ratio",
            "0.4",
            "--auto-close",
            "always",
        ]);
        let request = build_request(&command).unwrap();
        assert_eq!(
            request,
            Request::RunInteractive {
                pane: Some(5),
                tab: None,
                command: "sudo systemctl start foo".into(),
                input_hint: Some("sudo password".into()),
                direction: Some(Direction::Down),
                ratio: Some(0.4),
                auto_close: Some("always".into()),
            }
        );
    }

    #[test]
    fn run_interactive_statusのパースと変換() {
        let command = parse(&["tako", "run-interactive-status", "42"]);
        let request = build_request(&command).unwrap();
        assert_eq!(
            request,
            Request::RunInteractiveStatus {
                pane: 42,
                no_wait: false,
            }
        );
    }
}
