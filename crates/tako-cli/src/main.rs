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

// `orchestrator profiles set` のオプション数で variant サイズ差 lint が出る
// （ProfilesCommand 側と同じ理由）。CLI 引数のパースはプロセスで 1 回きりなので
// 実害がなく、clap は Subcommand / Args を Box 化できないため許容する
#[allow(clippy::large_enum_variant)]
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
    /// Markdown・PDF プレビュー内のリンク一覧を表示する
    #[command(name = "preview-link-list")]
    PreviewLinkList(PaneArg),
    /// プレビュー内のリンクをフォローする（URL はブラウザ、PDF 内部リンクはページジャンプ）
    #[command(name = "preview-follow-link")]
    PreviewFollowLink(PreviewFollowLinkArgs),
    /// Markdown プレビューのコードブロック全文をクリップボードへコピーする
    #[command(name = "preview-copy-code")]
    PreviewCopyCode(PreviewCopyCodeArgs),
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
    /// 複数ウィンドウの操作（Issue #339。list / new / close / move-tab / focus /
    /// minimize / maximize / restore）
    #[command(subcommand)]
    Window(WindowCommand),
    /// メニューバーの操作（Issue #657。list / open / close / invoke）。
    /// Windows は自前描画のメニューバー行、macOS は OS のメニューバー
    #[command(subcommand)]
    Menu(MenuCommand),
    /// タブ・ペイン名の AI 自動リネームの ON/OFF・状態確認
    Autorename(ToggleArgs),
    /// listen ポート検知 + 提案チップの ON/OFF・状態確認
    Portdetect(ToggleArgs),
    /// tako 内 zsh の入力予測（履歴ベースのゴーストテキスト。右矢印か Tab で確定）の
    /// ON/OFF・状態確認。既定 ON。tako の外の zsh には影響しない。
    /// `hint` / `tab` で確定キーの案内と Tab 確定を個別に切り替える
    Autosuggest(AutosuggestArgs),
    /// 利用上限（5h / 週次）後の自動復帰の ON/OFF・状態確認（ペイン単位。既定 OFF）。
    /// 有効にしたペインは、上限で止まってもリセット時刻を過ぎたら tako が作業を再開させる
    #[command(name = "limit-resume")]
    LimitResume(LimitResumeArgs),
    /// セッション永続化（tmux バックエンド）の ON/OFF・状態確認。
    /// 有効時、tako を再起動してもタブ構成と実行中プロセスが復元される
    Persist(ToggleArgs),
    /// close 確認ダイアログの ON/OFF・状態確認（× ボタン / cmd+W。
    /// 確認が入るのはエージェント・実行中プロセスがあるペインのみ）
    #[command(name = "confirm-close")]
    ConfirmClose(ToggleArgs),
    /// UI テーマの確認・切替・色設定・プリセット・フォント（Issue #217/#459）
    Theme(ThemeArgs),
    /// 設定画面を開く（Issue #459）
    Settings(SettingsArgs),
    /// 設定・データファイルの自動マイグレーションの状態確認・手動発火（Issue #916）。
    /// 引数なしで全永続ファイルの形式を確認するだけ（何も書き換えない）
    Migrate(MigrateArgs),
    /// 初回起動のウェルカムバナーの状態確認・再表示・非表示（Issue #549）。
    /// 引数なしで現在の表示状態と案内すべきコマンドを表示する
    Welcome(WelcomeArgs),
    /// ユーザーに実行してほしいコマンドをコピー可能なカードとして提示する（Issue #666）。
    /// `tako show-command "コマンド"` で対象ペイン下部にカードが出る
    // 変異名が enum 名（Command）で終わるが、CLI 名 `show-command` は FR-2.7 の
    // show_file / show_diff / show_url と揃えた提示系の語彙。名前を崩さず allow する
    #[allow(clippy::enum_variant_names)]
    ShowCommand(ShowCommandArgs),
    /// プラットフォーム対応マトリクスの参照（Issue #515）。
    /// この環境でどの機能が使えるか・縮退しているか・未実装かを表示する
    Platform(PlatformArgs),
    /// シェル統合（OSC 7 / 133 = ペインの cwd 追従とコマンド実行状態）の
    /// 配置状態の確認と配置・解除（Issue #525）。引数なしで現在の状態を表示。
    /// unix は環境変数の注入だけで完結するので配置操作は不要
    ShellIntegration(ShellIntegrationArgs),
    /// AI 系設定（tako の宣言的設定 + claude のグローバル指示）を
    /// git リポジトリでデバイス間共有する（Issue #513）。
    /// 引数なしで現在の配線状態と差分を表示する
    Config(ConfigArgs),
    /// UI 表示言語（日本語/英語）の確認・切替（Issue #435）。
    /// 引数なしで現在言語を表示、ja / en で指定、system で OS ロケール追従
    Lang(LangArgs),
    /// UI 表示モード（GUI ライク表示 / ターミナル表示）の確認・切替（Issue #691）。
    /// 引数なしで現在モードを表示、gui / terminal で指定、toggle で反転。
    /// release / restore は指定ペインだけを一時的にターミナル表示へ（揮発）
    #[command(name = "ui-mode")]
    UiMode(UiModeArgs),
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
    /// ファイル操作（パスコピー / ファイルマネージャ表示 / cd / リネーム / 作成 / ゴミ箱）
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
    /// stale claude バイナリの検知と張り直し（Issue #498）。
    /// 長生きセッションが古い claude バイナリのまま動いている場合に検知・張り直し
    #[command(subcommand, name = "stale-binary")]
    StaleBinary(StaleBinaryCommand),
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
    /// GUI モードのチャットビュー本文のコピー（Issue #725。UI のコピーボタンと同じ経路）
    #[command(subcommand)]
    Chat(ChatCommand),
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
    /// リモート（SSH 先）のフォルダをワークスペースとして開く・閉じる・覗く（#919 / #65）。
    /// GUI の「リモートからフォルダを開く」と同じ操作
    #[command(subcommand, name = "remote-folder")]
    RemoteFolder(RemoteFolderCommand),
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
        /// 接続後に cd するリモートのパス（#919）
        #[arg(long)]
        remote_dir: Option<String>,
    },
}

/// リモートフォルダの操作（#919 / #65）。MCP `tako_remote_folder` と 1:1
#[derive(Subcommand)]
enum RemoteFolderCommand {
    /// SSH 先のフォルダをファイルツリーに開く（接続に失敗したら開かず理由を返す）
    Open {
        /// ~/.ssh/config の Host 名
        host: String,
        /// リモート側の絶対パス（省略時はリモートのホーム）
        path: Option<String>,
        /// 対象タブ ID（省略時はアクティブタブ）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// 開いているリモートフォルダを閉じる（既定は全タブ横断）
    Close {
        /// ~/.ssh/config の Host 名（--all のときは省略可）
        host: Option<String>,
        /// リモート側の絶対パス（省略時はそのホストの全部）
        path: Option<String>,
        /// ホスト指定なしに全部閉じる（既定は全タブ横断。--tab で 1 タブへ絞る）
        #[arg(long)]
        all: bool,
        /// 対象タブ ID（省略時は全タブ横断）
        #[arg(long)]
        tab: Option<u64>,
    },
    /// 開いているリモートフォルダの一覧（読み込み状態つき）
    List,
    /// リモートのディレクトリを一覧する（ツリーを開かずに覗く）
    Ls {
        /// ~/.ssh/config の Host 名
        host: String,
        /// リモート側の絶対パス（省略時はリモートのホーム）
        path: Option<String>,
    },
    /// リモートのファイルをプレビューで開く（読み取り専用）
    OpenFile {
        /// ~/.ssh/config の Host 名
        host: String,
        /// リモート側のファイルの絶対パス
        path: String,
        /// フォーカスを移さない
        #[arg(long)]
        no_focus: bool,
    },
    /// そのフォルダを cwd にした SSH ペインを開く
    SshPane {
        /// ~/.ssh/config の Host 名
        host: String,
        /// リモート側の絶対パス（省略時はログイン時の cwd）
        path: Option<String>,
        /// フォーカスを新タブに移さない
        #[arg(long)]
        no_focus: bool,
    },
    /// リモートへ押し出せていない保存の一覧（切断中の保存はここに残る。#966）
    Pending {
        /// ~/.ssh/config の Host 名（省略時は全ホスト）
        host: Option<String>,
        /// リモート側のファイルの絶対パス（省略時はそのホストの全部）
        path: Option<String>,
    },
    /// 押し出せていない保存を再試行する（#966）
    Push {
        /// ~/.ssh/config の Host 名（省略時は全件）
        host: Option<String>,
        /// リモート側のファイルの絶対パス（省略時はそのホストの全部）
        path: Option<String>,
        /// 競合（開いた時点からリモートが変わっている）を承知のうえ上書きする
        #[arg(long)]
        force: bool,
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
    /// アップデート専用画面（GUI）を開く
    Open,
    /// 上部通知カードの操作（引数なしで現在の状態）
    Card {
        /// dismiss = 閉じてこのバージョンは以後通知しない / show = 抑止を解除して出し直す
        #[arg(value_parser = ["dismiss", "show"])]
        action: Option<String>,
    },
}

#[derive(Subcommand)]
enum StaleBinaryCommand {
    /// 指定ペインの stale 判定（握っている版 / 最新版 / stale か）
    Status {
        /// 対象ペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// stale ペインを張り直す（worker は resume、master は handoff）
    Restart {
        /// 対象ペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
    /// バナーを閉じる
    Dismiss {
        /// 対象ペイン ID
        #[arg(long)]
        pane: Option<u64>,
    },
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
enum ChatCommand {
    /// 発話（またはその中のコードブロック）をクリップボードへコピーする。
    /// 既定は最後の assistant 発話を「画面と同じプレーンテキスト」で
    Copy {
        /// 対象ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
        /// 発話の表示順（0 始まり。省略時は最後の assistant 発話）
        #[arg(long)]
        message: Option<usize>,
        /// その発話の中のコードブロック出現順（0 始まり。省略時は本文全体）
        #[arg(long)]
        code: Option<usize>,
        /// md ソースをそのままコピーする（既定は画面と同じプレーンテキスト）
        #[arg(long)]
        markdown: bool,
        /// コピーせずに発話の一覧（添字・role・文字数・コードブロック数）だけ出す
        #[arg(long)]
        list: bool,
    },
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
    /// コミット詳細を JSON で出力する（#495）。メタ情報・変更ファイル一覧を返す。
    /// --file でそのファイルの diff も含める
    Show {
        /// コミットハッシュ（短縮可）
        hash: String,
        /// diff を取得するファイルパス（省略時はファイル一覧のみ）
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// git commit（#472）
    Commit {
        /// コミットメッセージ
        #[arg(short, long)]
        message: String,
        /// tracked ファイルを自動ステージ（-a 相当）
        #[arg(short, long)]
        all: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// git pull（#472）
    Pull {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// git push（#472）
    Push {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// git stage（#472）。パス指定なしで全変更をステージ
    Stage {
        /// ステージするファイルパス（省略で全変更）
        paths: Vec<String>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// git unstage（#472）。パス指定なしで全アンステージ
    Unstage {
        /// アンステージするファイルパス（省略で全変更）
        paths: Vec<String>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ブランチを切り替える（#496）。未コミット変更があるときは実行せず、
    /// 何が起きるか（持ち越し / 衝突するファイル）を出力する。--yes で実行する
    Checkout {
        /// 切替先ブランチ（`origin/foo` を指定すると同名のローカル追跡ブランチを作る）
        branch: String,
        /// 事前提示を承諾して実行する
        #[arg(short = 'y', long)]
        yes: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 新規ブランチを作成して切り替える（#496）
    Branch {
        /// 作成するブランチ名
        name: String,
        /// 基点（省略時は現在の HEAD）
        #[arg(long)]
        from: Option<String>,
        /// 作成するだけで切り替えない
        #[arg(long)]
        no_checkout: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 指定ブランチを現在のブランチへマージする（#496）。既定では実行せず、
    /// マージ種別・取り込みコミット数・予測されるコンフリクトを出力する。--yes で実行する
    Merge {
        /// マージ元ブランチ
        branch: String,
        /// 事前提示を承諾して実行する
        #[arg(short = 'y', long)]
        yes: bool,
        /// 早送りせずマージコミットを作る
        #[arg(long)]
        no_ff: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 進行中の merge / rebase / cherry-pick / revert を中止する（#496）
    Abort {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// コンフリクト状態を JSON で出力する（#496）
    Conflicts {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// コンフリクト解消エージェントを起動する（#496）。同じタブにペインを立て、
    /// リポジトリ・未解決ファイル・ブランチを含む解消用プロンプトを自動投入する
    Resolve {
        /// エージェント種別（claude / codex / agy。省略時はプロファイル既定）
        #[arg(long)]
        agent: Option<String>,
        /// 分割先タブ（省略時は呼び出し元ペインのタブ）
        #[arg(long)]
        tab: Option<u64>,
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
    /// ファイルマネージャ（Finder / エクスプローラー）でファイルの場所を表示する
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
    /// ファイル・フォルダをゴミ箱（Windows はごみ箱）へ移動する（完全削除ではない）
    Trash { path: String },
    /// デフォルトアプリで開く
    Open { path: String },
    /// 指定アプリで開く
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
    /// 動画プレイヤーの現在状態（再生位置・総尺・再生状態）。UI の表示と同じ値
    Status {
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
#[allow(clippy::large_enum_variant)]
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
    /// アカウント管理（accounts.yaml の一覧 / 表示 / 追加 / 削除。#504 / #548）
    #[command(subcommand)]
    Accounts(AccountsCommand),
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
        /// アカウント名（accounts.yaml のキー。この worker だけ該当アカウントで起動する。#504）
        #[arg(long)]
        account: Option<String>,
        /// この worker だけ利用上限後の自動復帰を明示指定する
        /// （省略時はプロファイルの limit_resume → 無効。#822）
        #[arg(long)]
        limit_resume: Option<bool>,
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
    /// master の引き継ぎを実行する（#193）。管轄プロジェクトの引き継ぎを読み新 master を spawn
    Handoff {
        /// 呼び出し元ペイン ID（省略時は自動解決）
        #[arg(long)]
        pane: Option<u64>,
        /// 新 master を出すタブ ID（省略時は呼び出し元と同タブ）
        #[arg(long)]
        tab: Option<u64>,
        /// 後任へ渡すプロジェクト（カンマ区切り。#915。省略時はプロファイルの担当 +
        /// 稼働 worker から推定する）
        #[arg(long, value_delimiter = ',')]
        projects: Option<Vec<String>>,
    },
    /// 引き継ぎファイルの管理（#915）。プロジェクト単位の一覧・読み・書きと旧形式の移行
    #[command(subcommand)]
    Handoffs(HandoffsCommand),
    /// worker の選択肢ダイアログに応答する（#319 → #748 で permission 以外も対象）。
    /// ダイアログ不在時はエラー（誤爆防止）。
    /// `--choice` を省略すると**送信せず**選択肢の構造だけを表示する（下見）
    Respond {
        /// 対象ペイン ID
        #[arg(long)]
        pane: u64,
        /// 選択肢の番号（画面の番号 or 1 始まりの順番）／ラベルの部分一致／
        /// "yes"/"no" エイリアス。省略すると構造だけ表示する
        #[arg(long)]
        choice: Option<String>,
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
    /// 無関係に列挙する（tako 再起動後も追跡できる）。既定は active のみ。
    /// 列挙のついでに、ペインも器も 5 分以上観測できない active エントリを
    /// closed（gone）へ倒す（#658。closed でも resume / report は引ける）
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
        /// アカウント名（accounts.yaml のキー。この worker だけ該当アカウントで起動する。#504）
        #[arg(long)]
        account: Option<String>,
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

/// `tako orchestrator handoffs` — 引き継ぎファイルの管理（#915）。
/// MCP `tako_orchestrator_handoffs` と同じ dispatch 関数を呼ぶ（二重実装を作らない）
#[derive(Subcommand)]
enum HandoffsCommand {
    /// プロジェクト単位の引き継ぎとプロファイル運用メモの一覧
    List,
    /// 1 件を読む（--project か --profile のどちらか）
    Show {
        /// プロジェクトキー
        #[arg(long)]
        project: Option<String>,
        /// プロファイル名（運用メモ側）
        #[arg(long, conflicts_with = "project")]
        profile: Option<String>,
    },
    /// 1 件を書く（--project か --profile のどちらか。内容は --content か標準入力）
    Write {
        /// プロジェクトキー
        #[arg(long)]
        project: Option<String>,
        /// プロファイル名（運用メモ側）
        #[arg(long, conflicts_with = "project")]
        profile: Option<String>,
        /// 書き込む内容（省略時は標準入力から読む）
        #[arg(long)]
        content: Option<String>,
    },
    /// 旧形式（プロファイル単位の混在ファイル）をプロジェクト単位へ移行する。
    /// 通常は setup 実行時と master が引き継ぎを読む経路で自動で走る（#916）
    Migrate {
        /// 対象プロファイル（省略時は全プロファイル）
        #[arg(long)]
        profile: Option<String>,
    },
}

/// `tako orchestrator accounts` — アカウントレジストリ（accounts.yaml）の CRUD。
/// MCP `tako_orchestrator_accounts` と同じ dispatch 関数を呼ぶ（表示・警告・検証を二重実装しない）
#[derive(Subcommand)]
enum AccountsCommand {
    /// 登録済みアカウントの一覧
    List,
    /// 1 件の詳細
    Show {
        /// アカウント名
        name: String,
    },
    /// アカウントの追加 / 更新
    Add {
        /// アカウント名
        name: String,
        /// CLAUDE_CONFIG_DIR に設定するパス（~ は $HOME に展開。--inherit と排他）
        #[arg(long)]
        config_dir: Option<String>,
        /// CLAUDE_CONFIG_DIR を設定しない（既定の資格情報をそのまま使う。#512）
        #[arg(long, conflicts_with = "config_dir")]
        inherit: bool,
        /// 説明
        #[arg(long)]
        description: Option<String>,
        /// このアカウントの既定モデル（spawn で model 未指定時のフォールバック）
        #[arg(long)]
        default_model: Option<String>,
        /// このアカウントの既定 effort
        #[arg(long)]
        default_effort: Option<String>,
    },
    /// アカウントの削除
    Remove {
        /// アカウント名
        name: String,
    },
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

/// プロファイル種別の指定（#721）。既定は master なので、master を使うときは
/// 何も付けない（「最も簡単なコマンドを提案する」原則。#322）
#[derive(Args, Clone, Copy)]
struct ProfileKindArgs {
    /// solo プロファイル（tako solo が読む solo-profiles/）を対象にする（省略時 master）
    #[arg(long)]
    solo: bool,
}

impl ProfileKindArgs {
    /// dispatch の kind パラメータ。master は None（既定）で送る
    fn kind(&self) -> Option<String> {
        self.solo.then(|| "solo".to_string())
    }
}

// Set のオプション数で variant サイズ差 lint が出るが、CLI 引数のパースは
// プロセスで 1 回きりのため実害がなく許容する（clap は Box variant を扱えない）
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum ProfilesCommand {
    /// プロファイルの一覧（model が null のものは claude CLI の既定モデルで起動する）
    List {
        #[command(flatten)]
        kind: ProfileKindArgs,
    },
    /// プロファイルの内容と解決結果を表示する
    Show {
        /// プロファイル名（省略時 default）
        name: Option<String>,
        #[command(flatten)]
        kind: ProfileKindArgs,
    },
    /// プロファイルを新規作成する（既存があればエラー。中身は set で埋める）
    Create {
        /// プロファイル名（英数字と - _ . のみ）
        name: String,
        #[command(flatten)]
        kind: ProfileKindArgs,
    },
    /// 既存プロファイルを複製する
    Copy {
        /// 複製元のプロファイル名
        from: String,
        /// 複製先のプロファイル名
        name: String,
        #[command(flatten)]
        kind: ProfileKindArgs,
    },
    /// プロファイルを削除する（default は削除できない）
    Delete {
        /// プロファイル名
        name: String,
        #[command(flatten)]
        kind: ProfileKindArgs,
    },
    /// プロファイルを作成・更新する。[1m] 付きモデルは Max / API プラン限定なので注意
    Set {
        /// プロファイル名
        name: String,
        #[command(flatten)]
        kind: ProfileKindArgs,
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
        /// 環境変数を設定する（KEY=VALUE 形式。複数指定可。Issue #500）
        #[arg(long = "env-set")]
        env_set: Option<Vec<String>>,
        /// 環境変数を削除する（キー名。複数指定可。Issue #500）
        #[arg(long = "env-unset")]
        env_unset: Option<Vec<String>>,
        /// master の既定アカウント名（accounts.yaml のキー。空文字でクリア。#504）
        #[arg(long)]
        master_account: Option<String>,
        /// master_account を解除する
        #[arg(long)]
        clear_master_account: bool,
        /// worker の既定アカウント名（空文字でクリア。#504）
        #[arg(long)]
        worker_account: Option<String>,
        /// worker_account を解除する
        #[arg(long)]
        clear_worker_account: bool,
        /// 割り当てるプロジェクトキー（カンマ区切り。丸ごと置き換え。#721）
        #[arg(long, value_delimiter = ',', conflicts_with = "clear_projects")]
        projects: Option<Vec<String>>,
        /// projects の割り当てを解除する（#721）
        #[arg(long)]
        clear_projects: bool,
        /// 引き継ぎを始める ctx 使用率の閾値（%。50〜60。#749）
        #[arg(long, conflicts_with = "clear_ctx_threshold")]
        ctx_threshold: Option<u32>,
        /// ctx_threshold を解除する（config.yaml → 既定 60 へ戻る。#749）
        #[arg(long)]
        clear_ctx_threshold: bool,
        /// 閾値超過時に tako が引き継ぎを促す自動通知（既定 true。#749）
        #[arg(long, conflicts_with = "clear_auto_handoff")]
        auto_handoff: Option<bool>,
        /// auto_handoff を解除して既定（有効）へ戻す（#749）
        #[arg(long)]
        clear_auto_handoff: bool,
        /// spawn した worker で利用上限後の自動復帰を既定 ON にする（既定 false。#822）
        #[arg(long, conflicts_with = "clear_limit_resume")]
        limit_resume: Option<bool>,
        /// limit_resume を解除して既定（無効）へ戻す（#822）
        #[arg(long)]
        clear_limit_resume: bool,
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
    /// 新しいタブ 1 枚をこのファイル専用のプレビューにして開く（タブ名はファイル名。
    /// Finder の「このアプリケーションで開く」と同じ表示。#835）
    #[arg(long = "new-tab", conflicts_with_all = ["right", "down", "up", "left"])]
    new_tab: bool,
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
struct PreviewCopyCodeArgs {
    /// 対象ペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// コードブロックの出現順（0 始まり。省略時は先頭）
    index: Option<usize>,
}

/// `--view` の受理値（#553）。正式値は GUI のタブ表示名と 1:1 で、
/// 旧称は後方互換のため受理を続ける（`--help` と invalid value エラーの
/// possible values に旧称も出し、どちらの語彙からでも辿れるようにする）
fn panel_view_parser() -> clap::builder::PossibleValuesParser {
    use clap::builder::PossibleValue;
    use tako_control::protocol::PanelViewWire;

    let mut values: Vec<PossibleValue> = PanelViewWire::VALUES
        .iter()
        .map(|v| PossibleValue::new(*v))
        .collect();
    values.extend(
        PanelViewWire::LEGACY_VALUES
            .iter()
            .map(|(old, new)| PossibleValue::new(*old).help(format!("{new} の旧称（後方互換）"))),
    );
    clap::builder::PossibleValuesParser::new(values)
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
    /// 表示するビュー（GUI のタブ名と同じ。fleet = ペイン / セッション俯瞰、orch = オーケストレーター俯瞰、git = git。tmux は fleet の旧称）
    #[arg(long, value_parser = panel_view_parser())]
    view: Option<String>,
    /// 左サイドバーのファイルツリー表示（FR-2.16.5。on = 表示、off = 非表示）
    #[arg(long, value_parser = ["on", "off"])]
    filetree: Option<String>,
    /// 左サイドバーの幅（px。下限 120 / 上限はウィンドウ幅の 50% にクランプされる。#307 / #789）
    #[arg(long)]
    sidebar_width: Option<f32>,
    /// ファイルツリーの隠しファイル（ドット始まり）表示（Issue #550。既定 off）
    #[arg(long, value_parser = ["on", "off"])]
    show_hidden: Option<String>,
}

/// ON/OFF トグル系コマンド共通の引数（autorename / portdetect）
#[derive(Args)]
struct ToggleArgs {
    /// on = 有効化、off = 無効化（省略時は現在状態を表示）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

/// 入力予測の引数（Issue #600 / #614）。
///
/// `tako autosuggest [on|off]` = 予測そのもの、
/// `tako autosuggest hint|tab [on|off]` = 確定キーの案内 / Tab 確定。
/// サブコマンドではなく位置引数にしているのは、素の `tako autosuggest` を
/// 状態表示のままにするため（`tako theme` と同じ形）
#[derive(Args)]
struct AutosuggestArgs {
    /// on / off（予測そのもの）、または hint / tab（切替対象）。省略時は現在状態を表示
    #[arg(value_parser = ["on", "off", "hint", "tab"])]
    target_or_state: Option<String>,
    /// hint / tab を指定したときの on / off（省略時はその項目の現在状態を表示）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

/// 利用上限後の自動復帰の引数（Issue #813）。
///
/// `tako limit-resume` = 呼び出し元ペインの現在値、`on` / `off` で切替、
/// `--all` で全ペインの一覧。素のコマンドが最短で済む形にしてある（#322）
#[derive(Args)]
struct LimitResumeArgs {
    /// on / off（省略時は現在状態を表示）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
    /// 対象ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID）
    #[arg(long)]
    pane: Option<u64>,
    /// 全ペインの状態を一覧する（state とは併用しない）
    #[arg(long)]
    all: bool,
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

/// UI テーマコマンドの引数（Issue #217/#459）
#[derive(Args)]
struct ThemeArgs {
    /// dark / light / toggle / colors / preset（省略時は現在テーマを表示）
    mode: Option<String>,
    /// set-color の色キー名 / preset save の名前
    name_or_key: Option<String>,
    /// set-color の #RRGGBB / preset delete
    value_or_action: Option<String>,
    /// 色操作の対象
    #[arg(long)]
    target: Option<String>,
    /// reset（reset-color 用）
    #[arg(long)]
    reset: bool,
    /// フォントサイズ
    #[arg(long)]
    size: Option<f32>,
}

/// 設定画面コマンドの引数（Issue #459）
#[derive(Args)]
struct SettingsArgs {
    /// 開くタブ指定（general / appearance / runner / profiles / setup / sleep / remote / advanced）
    #[arg(long)]
    tab: Option<String>,
}

/// 自動マイグレーションコマンドの引数（Issue #916）。
/// 既定は「見るだけ」（#322 の最簡形 = `tako migrate` で状態が分かる）
#[derive(Args)]
struct MigrateArgs {
    /// run（実際に当てる）。省略時は status = 見るだけ
    action: Option<String>,
    /// 対象のファイル種別（省略時は全種別）
    #[arg(long)]
    schema: Option<String>,
}

/// ウェルカムバナーコマンドの引数（Issue #549）
#[derive(Args)]
struct WelcomeArgs {
    /// show（再表示）/ dismiss（閉じて以後出さない）。省略時は状態表示
    action: Option<String>,
}

/// コマンド提案カードの引数（Issue #666）。
/// 標準の使い方は `tako show-command "コマンド"` の 1 形（#322 の最簡形）
#[derive(Args)]
struct ShowCommandArgs {
    /// 提示するコマンド（複数指定でそのぶんカードに並ぶ）
    commands: Vec<String>,
    /// 何のためのコマンドかの短い説明（カード見出しに出る）
    #[arg(long)]
    label: Option<String>,
    /// 対象ペイン（省略時は呼び出し元ペイン）
    #[arg(long)]
    pane: Option<u64>,
    /// 表示中のカードと保管されている論理文字列を一覧する
    #[arg(long, conflicts_with_all = ["label", "copy", "run", "dismiss"])]
    list: bool,
    /// カードのコマンドをクリップボードへコピーする（カードの「コピー」と同じ）
    #[arg(long, conflicts_with_all = ["label", "run", "dismiss"])]
    copy: bool,
    /// カードのコマンドを新しいペインで実行する（カードの「新規ペインで実行」と同じ）
    #[arg(long, conflicts_with_all = ["label", "dismiss"])]
    run: bool,
    /// カードを閉じる（--card 省略時はそのペインの全カード）
    #[arg(long)]
    dismiss: bool,
    /// 対象カード ID（copy / run / dismiss。省略時は最新カード）
    #[arg(long)]
    card: Option<u64>,
    /// 対象コマンド番号（copy / run。1 始まり。省略時は 1）
    #[arg(long)]
    index: Option<usize>,
    /// run で新しいペインへフォーカスを移す（既定は移さない）
    #[arg(long)]
    focus: bool,
}

impl ShowCommandArgs {
    /// フラグから dispatch の action を決める（既定は show）
    fn action(&self) -> &'static str {
        if self.list {
            "list"
        } else if self.copy {
            "copy"
        } else if self.run {
            "run"
        } else if self.dismiss {
            "dismiss"
        } else {
            "show"
        }
    }
}

/// プラットフォーム対応マトリクスの参照引数（Issue #515）
#[derive(Args)]
struct PlatformArgs {
    /// 対象プラットフォーム（省略時は実行中の環境）
    #[arg(long, value_parser = ["macos", "windows"])]
    platform: Option<String>,
    /// この状態のものだけに絞る（省略時は全件）
    #[arg(long, value_parser = ["supported", "degraded", "pending", "unsupported"])]
    status: Option<String>,
    /// リリースノート用の Known limitations 節（日英併記の markdown）だけを出力する（Issue #594。
    /// scripts/release.sh が使う。縮退が無ければ何も出力しない）
    #[arg(long)]
    known_limitations: bool,
    /// 生の JSON で出力する
    #[arg(long)]
    json: bool,
}

/// シェル統合の配置操作の引数（Issue #525）
#[derive(Args)]
struct ShellIntegrationArgs {
    /// 操作（省略時は status）
    #[arg(value_parser = ["status", "install", "uninstall"])]
    action: Option<String>,
    /// 生の JSON で出力する
    #[arg(long)]
    json: bool,
}

/// UI 表示言語コマンドの引数（Issue #435）
#[derive(Args)]
struct LangArgs {
    /// ja / en = 指定言語へ、system = OS ロケール追従（省略時は現在言語を表示）
    #[arg(value_parser = ["ja", "en", "system"])]
    value: Option<String>,
}

/// UI 表示モードの引数（Issue #691 / #694）
#[derive(Args)]
struct UiModeArgs {
    /// gui / terminal = そのモードへ、toggle = 反転、
    /// release / restore = 対象ペインだけターミナル表示へ / その解除を戻す
    /// （省略時は現在モードを表示）
    #[arg(value_parser = ["gui", "terminal", "toggle", "release", "restore"])]
    action: Option<String>,
    /// release / restore の対象ペイン ID（省略時は呼び出し元ペイン）
    #[arg(long)]
    pane: Option<u64>,
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
    #[command(subcommand)]
    command: Option<SetupCommand>,
}

/// `tako setup` のサブコマンド。**素の `tako setup` の意味は変えない**（#322）。
/// ゼロスタート導入（#868）を AI から段ごとに操作するための入口
#[derive(Subcommand)]
enum SetupCommand {
    /// エージェント CLI の導入状況を確認・実行する（#868）
    Bootstrap {
        /// status（既定・読み取り専用）/ install / path / undo-path
        action: Option<String>,
        /// install で実行せず「何をどこに入れるか」だけ出す
        #[arg(long)]
        dry_run: bool,
        /// 出力を JSON にする（MCP tako_setup_bootstrap と同一ペイロード）
        #[arg(long)]
        json: bool,
    },
}

/// `tako config`（Issue #513）。サブコマンド省略 = status
#[derive(Args)]
struct ConfigArgs {
    /// 出力を JSON にする（MCP tako_config_share と同一ペイロード）。
    /// サブコマンドの前後どちらに置いてもよい
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<ConfigCommand>,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// 配線状態と push / pull 待ちの差分を表示する（既定）
    Status,
    /// 共有リポジトリを新規に作って配線し、いまの設定を書き出す
    Init {
        /// リポジトリの配置先（省略時は ~/tako-config-sync）
        #[arg(long)]
        path: Option<String>,
        /// origin として登録するリモート URL（指定時は初回 push まで行う）
        #[arg(long)]
        remote: Option<String>,
    },
    /// 既存の共有リポジトリに配線する（ローカルパスまたは git URL）
    Link {
        /// リポジトリのパス、または clone 元の git URL
        target: String,
        /// URL を clone するときの配置先（省略時は ~/tako-config-sync）
        #[arg(long)]
        path: Option<String>,
    },
    /// 配線を外す（リポジトリ自体は消さない）
    Unlink,
    /// このデバイスの設定を書き出してコミットする（リモートがあれば push）
    Push {
        /// コミットメッセージ
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// リモートへ送らずコミットまでで止める
        #[arg(long)]
        no_push: bool,
    },
    /// 共有リポジトリの設定をこのデバイスへ取り込む（リモートがあれば pull）
    Pull,
    /// 何を共有し何を共有しないかの分類表を表示する
    List,
}

#[derive(Args)]
struct SetupMcpArgs {
    /// ユーザーグローバルに書き込む（既定）
    #[arg(long, conflicts_with = "project")]
    global: bool,
    /// カレントディレクトリの .mcp.json に書き込む
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
        /// 初期ペインのシェルを起動するフォルダ（省略時は継承。#835）
        #[arg(long)]
        cwd: Option<String>,
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
    /// いまのタブ名を固定する（自動リネームに上書きされなくなる。#552）
    Pin {
        /// 対象タブ ID（省略時は呼び出し元ペインの属するタブ）
        #[arg(long)]
        tab: Option<u64>,
        /// 固定を解除して自動リネームを再開する
        #[arg(long)]
        off: bool,
        /// 変更せず現在の固定状態だけを表示する
        #[arg(long, conflicts_with = "off")]
        status: bool,
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
    /// ウィンドウを最小化する（省略時はアクティブウィンドウ）
    Minimize {
        /// 対象ウィンドウ ID（省略時はアクティブウィンドウ）
        window: Option<u64>,
    },
    /// ウィンドウを最大化する（省略時はアクティブウィンドウ）
    Maximize {
        /// 対象ウィンドウ ID（省略時はアクティブウィンドウ）
        window: Option<u64>,
    },
    /// 最大化を解除して元のサイズへ戻す（省略時はアクティブウィンドウ）
    Restore {
        /// 対象ウィンドウ ID（省略時はアクティブウィンドウ）
        window: Option<u64>,
    },
}

/// メニューバーの操作（Issue #657）
#[derive(Subcommand, Debug)]
enum MenuCommand {
    /// メニュー構成と開閉状態を表示する
    List,
    /// メニューを開く（Windows のみ。macOS は OS がメニューを描くので開けない）
    Open {
        /// メニュー名（完全一致 → 前方一致 → 部分一致で解決。添字も可）
        menu: String,
    },
    /// 開いているメニューを閉じる（Windows のみ）
    Close,
    /// メニュー項目を実行する（macOS / Windows 共通）
    Invoke {
        /// 「メニュー名/項目名」または項目名のみ（例: `ファイル/新規タブ`、`新規タブ`、
        /// `表示/パネル/git ビュー`）
        path: String,
    },
}

fn main() -> ExitCode {
    // Windows のメインスレッドは既定 1MB スタックで、コマンド定義（clap の巨大ツリー）の
    // 構築だけで debug ビルドが溢れる（macOS / Linux は 8MB）。**main 由来の既存バグ**で、
    // `origin/main` の `tako.exe list` も実機で `has overflowed its stack` で落ちる
    // （スライス 3 の IPC 検証はユニットテストだったため踏まなかった）。
    // 本体を十分なスタックのワーカースレッドで実行する（プラットフォーム共通・挙動不変）
    std::thread::Builder::new()
        .name("tako-main".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(cli_main)
        .expect("メインスレッドを起動できない")
        .join()
        .expect("メインスレッドが異常終了した")
}

fn cli_main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Mcp(McpCommand::Serve) => mcp_serve(),
        Command::Setup(ref args) => {
            // setup も表示言語を settings.json から解決する（#435。移行の説明文など
            // Note ベースの文言がここを通るので、設定が ja なら日本語で出る）
            tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
            if let Some(SetupCommand::Bootstrap {
                ref action,
                dry_run,
                json,
            }) = args.command
            {
                setup::run_bootstrap(action.as_deref(), dry_run, json)
            } else if args.check {
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
        Command::Orchestrator(OrchestratorCommand::Handoff {
            pane,
            tab,
            ref projects,
        }) => {
            let pane = pane.or_else(caller_pane);
            let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
            send_request(Request::OrchestratorHandoff {
                pane,
                caller_role,
                tab,
                caller_pid: Some(std::process::id()),
                projects: projects.clone(),
            })
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Handoffs(ref sub)) => {
            // 引き継ぎファイルはローカルのファイル操作なので IPC 不要。
            // MCP `tako_orchestrator_handoffs` と同一関数を共用する（二重実装を作らない）
            let (action, project, profile, content) = match sub {
                HandoffsCommand::List => ("list", None, None, None),
                HandoffsCommand::Show { project, profile } => {
                    ("show", project.clone(), profile.clone(), None)
                }
                HandoffsCommand::Write {
                    project,
                    profile,
                    content,
                } => {
                    let body = match content {
                        Some(c) => c.clone(),
                        None => {
                            let mut buf = String::new();
                            match std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
                                Ok(_) => buf,
                                Err(e) => {
                                    eprintln!("エラー: 標準入力の読み取りに失敗: {e}");
                                    return ExitCode::FAILURE;
                                }
                            }
                        }
                    };
                    ("write", project.clone(), profile.clone(), Some(body))
                }
                HandoffsCommand::Migrate { profile } => ("migrate", None, profile.clone(), None),
            };
            tako_control::dispatch::dispatch_orchestrator_handoff_files(
                action,
                project.as_deref(),
                profile.as_deref(),
                content.as_deref(),
            )
            .map_err(|e| e.to_string())
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
        Command::Orchestrator(OrchestratorCommand::Accounts(ref sub)) => {
            // accounts.yaml のみの操作のため IPC 不要。dispatch と同一関数を共用する
            // （MCP `tako_orchestrator_accounts` と 1:1。二重実装を作らない。#548）
            let (action, name, config_dir, inherit, description, default_model, default_effort) =
                match sub {
                    AccountsCommand::List => ("list", None, None, None, None, None, None),
                    AccountsCommand::Show { name } => {
                        ("show", Some(name.as_str()), None, None, None, None, None)
                    }
                    AccountsCommand::Add {
                        name,
                        config_dir,
                        inherit,
                        description,
                        default_model,
                        default_effort,
                    } => (
                        "add",
                        Some(name.as_str()),
                        config_dir.as_deref(),
                        // 指定が無いときは None を渡す（dispatch 側の既定 = false）
                        inherit.then_some(true),
                        description.as_deref(),
                        default_model.as_deref(),
                        default_effort.as_deref(),
                    ),
                    AccountsCommand::Remove { name } => {
                        ("remove", Some(name.as_str()), None, None, None, None, None)
                    }
                };
            tako_control::dispatch_orchestrator_accounts(
                action,
                name,
                config_dir,
                inherit,
                description,
                default_model,
                default_effort,
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
            ref account,
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
            account.as_deref(),
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
        // 移行もローカル処理（IPC 不要）。**壊れた設定で GUI が起動しないときの
        // 復旧手段**なので、GUI に依存しないことが本質（recover と同じ理由）
        Command::Migrate(ref args) => migrate_local(args),
        // 対応マトリクスはバイナリに埋め込まれた静的な表なのでローカル処理。
        // GUI が動いていない環境（移植作業中の Windows がまさにそれ）でも引けることが本質
        Command::Platform(ref args) => platform_local(args),
        // GUI を必要としないローカル処理（platform と同じ扱い）。
        // 実体は dispatch と共通の tako_control::shell_integration::run
        Command::ShellIntegration(ref args) => shell_integration_local(args),
        Command::Config(ref args) => config_share_local(args),
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
    let scope = if args.project {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("カレントディレクトリの取得に失敗: {e}"))?;
        tako_control::dispatch::McpScope::Project(cwd)
    } else {
        tako_control::dispatch::McpScope::User
    };
    match tako_control::dispatch::setup_mcp(&tako_bin, &scope) {
        Ok(result) => {
            if result.repaired {
                let old = result.old_command.as_deref().unwrap_or("(不明)");
                eprintln!(
                    "登録パスが消失していたため付け替えました: {}",
                    result.target_path.display()
                );
                eprintln!("  旧: {old}");
                eprintln!("  新: {tako_bin}");
            } else if result.already_existed {
                eprintln!("既に設定されています: {}", result.target_path.display());
            } else {
                eprintln!("設定を追加しました: {}", result.target_path.display());
            }
            if result.legacy_cleaned {
                eprintln!("旧 ~/.claude/settings.json の無効な MCP 設定を除去しました");
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
    let claude_json = home.join(".claude.json");
    let content = match std::fs::read_to_string(&claude_json) {
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
        None => return,
    };
    if !std::path::Path::new(cmd).is_file() {
        eprintln!("[警告] MCP 登録パスが消失しています: {cmd}");
        eprintln!("        tako MCP なしで起動します。tako setup-mcp で修復してください。");
        eprintln!();
    }
}

/// master / solo 起動時の env とアカウントの可視化（Issue #500 / #547）。
/// 値はマスクし、キー名と解決後の config dir だけを出す。
/// アカウント解決の失敗は build_master_cmd が起動前に Err にするので、ここは表示だけ
fn print_master_env(profile: &tako_control::orchestrator::Profile) {
    use tako_control::orchestrator;
    if !profile.env.is_empty() {
        let keys: Vec<&str> = profile.env.keys().map(|k| k.as_str()).collect();
        eprintln!("env: {}", keys.join(", "));
    }
    match profile.resolve_master_account() {
        // #547: master_account があればそちらが CLAUDE_CONFIG_DIR の正
        Ok(Some(account)) => match account.config_dir.path() {
            Some(dir) => eprintln!("アカウント: {}（config dir: {dir}）", account.name),
            None => eprintln!(
                "アカウント: {}（既定の資格情報 / CLAUDE_CONFIG_DIR 未設定）",
                account.name
            ),
        },
        // アカウント未指定: プロファイル env の config dir を従来どおり出す
        Ok(None) => {
            if let Some(config_dir) = profile.env.get("CLAUDE_CONFIG_DIR") {
                eprintln!("config dir: {}", orchestrator::expand_tilde(config_dir));
            }
        }
        Err(e) => eprintln!("warning: {e}"),
    }
}

/// `tako master [-profile]` — 新タブで claude をマスター system prompt 付きで起動する。
/// `-<名前>` でプロファイルを指定、引数なしは default、旧形式（suffix のみ）も後方互換で動作
fn orchestrator_master(arg: Option<&str>, use_tab: bool) -> Result<(), String> {
    use tako_control::orchestrator;

    orchestrator::ensure_defaults().map_err(|e| format!("セットアップに失敗: {e}"))?;

    check_mcp_health_warning();

    // 旧形式の設定ファイルを master 起動のたびに検知して直す（#916）
    if let Some(notice) = tako_control::migrations::ensure_migrated() {
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

    // env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env()?;

    // Part 5: cwd 解決（存在しなければ診断つきエラー）
    let resolved_cwd = profile.resolve_cwd()?;

    // Part 7: projects の key が projects.yaml に存在するか検証（起動時エラー）
    profile.validate_projects()?;

    // Part 5: cwd が指定されていれば、そのディレクトリへ移動
    if let Some(ref cwd) = resolved_cwd {
        std::env::set_current_dir(cwd)
            .map_err(|e| format!("プロファイルの cwd に移動できない: {} ({e})", cwd.display()))?;
    }

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
    // --tab 指定時 / 呼び出し元ペインが解決できないとき（#567）: 新タブ起動
    let target = resolve_launch_target(&tab_title, use_tab, &master_cmd_hint(profile_name))?;
    let pane_id = target.pane;

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

    eprintln!(
        "master を起動しました: {}",
        launch_location(&tab_title, &target)
    );
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
    // Part 4: env の可視化（キー名のみ。Issue #500 / #547）
    print_master_env(&profile);
    if let Some(ref projects) = profile.projects {
        eprintln!("projects 制限: {}", projects.join(", "));
    }
    // Part 5: cwd 表示
    if let Some(ref cwd) = resolved_cwd {
        eprintln!("cwd: {}", cwd.display());
    }
    eprintln!("system prompt: {}", prompt_path.display());

    // Part 6: master 起動後にファイルツリーへ cwd と projects のフォルダを自動追加
    let mut tree_folders: Vec<String> = Vec::new();
    if let Some(ref cwd) = resolved_cwd {
        if let Ok(canonical) = cwd.canonicalize() {
            tree_folders.push(canonical.display().to_string());
        }
    }
    if let Some(ref project_keys) = profile.projects {
        if let Ok(config) = orchestrator::ProjectsConfig::load() {
            for key in project_keys {
                if let Ok(cwd) = config.resolve_cwd(key) {
                    let path = std::path::PathBuf::from(&cwd);
                    if let Ok(canonical) = path.canonicalize() {
                        let s = canonical.display().to_string();
                        if !tree_folders.contains(&s) {
                            tree_folders.push(s);
                        }
                    }
                }
            }
        }
    }
    for folder in &tree_folders {
        let _ = send_request(Request::TreeFolder {
            action: "add".into(),
            path: Some(folder.clone()),
            tab: None,
            pane: Some(pane_id),
        });
    }
    if !tree_folders.is_empty() {
        eprintln!(
            "ファイルツリーに追加: {}",
            tree_folders
                .iter()
                .map(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone())
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

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

    // env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env()?;

    // Part 5: cwd 解決
    let resolved_cwd = profile.resolve_cwd()?;
    if let Some(ref cwd) = resolved_cwd {
        std::env::set_current_dir(cwd)
            .map_err(|e| format!("プロファイルの cwd に移動できない: {} ({e})", cwd.display()))?;
    }

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

    // --tab 指定時 / 呼び出し元ペインが解決できないとき（#567）は新タブ起動
    let target = resolve_launch_target(&tab_title, use_tab, &solo_cmd_hint(profile_name))?;
    let pane_id = target.pane;

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

    eprintln!(
        "solo を起動しました: {}",
        launch_location(&tab_title, &target)
    );
    eprintln!(
        "プロファイル: {profile_name}（エージェント: {}、モデル: {}、effort: {}）",
        solo_agent.as_str(),
        profile.master_model_label(),
        profile.effort
    );
    eprintln!("モード: solo（オーケストレーション無し・1 対 1 対話・worker spawn 禁止）");
    // Part 4: env の可視化（キー名のみ。Issue #500 / #547）
    print_master_env(&profile);
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
                        // #572: 対処を併記しないと master が画面から推測するしかなく、
                        // 「Enter を代行すれば直る」のような誤読の温床になる
                        if let Some(action) = ev["recommended_action"].as_str() {
                            parts.push(format!("action={action}"));
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
        // #748: permission 以外の選択肢ダイアログ待ち。旧実装ではこの状態が
        // WORKER_IDLE / WORKER_QUESTION として出ていたため、master は
        // 「完了した」「本文で質問された」と読み違えていた
        wait::WatchOutcome::ChoiceWaiting { choice_dialog } => {
            let kind = choice_dialog
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("select");
            println!("WORKER_DIALOG: tako:{pane} ({kind})");
            if let Some(title) = choice_dialog.get("title").and_then(|v| v.as_str()) {
                if !title.is_empty() {
                    println!("  title: {title}");
                }
            }
            if let Some(opts) = choice_dialog.get("options").and_then(|v| v.as_array()) {
                for (i, opt) in opts.iter().enumerate() {
                    let number = opt
                        .get("number")
                        .and_then(|v| v.as_u64())
                        .unwrap_or((i + 1) as u64);
                    let label = opt.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let mark = if opt.get("highlighted").and_then(|v| v.as_bool()) == Some(true) {
                        " ← 現在の選択"
                    } else {
                        ""
                    };
                    println!("  {number}. {label}{mark}");
                }
            }
            if choice_dialog.get("numbered").and_then(|v| v.as_bool()) == Some(false) {
                println!("  note: 番号キーは無反応なダイアログ（tako が矢印移動で応答する）");
            }
            println!(
                "  action: {}",
                choice_dialog
                    .get("recommended_action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("respond")
            );
            println!("  respond: tako orchestrator respond --pane {pane} --choice <番号|ラベル>");
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
    account: Option<&str>,
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
        account: account.map(str::to_string),
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
        ProfilesCommand::List { kind } => ProfilesParams {
            action: "list".into(),
            kind: kind.kind(),
            ..Default::default()
        },
        ProfilesCommand::Show { name, kind } => ProfilesParams {
            action: "show".into(),
            name: name.clone(),
            kind: kind.kind(),
            ..Default::default()
        },
        ProfilesCommand::Create { name, kind } => ProfilesParams {
            action: "create".into(),
            name: Some(name.clone()),
            kind: kind.kind(),
            ..Default::default()
        },
        ProfilesCommand::Copy { from, name, kind } => ProfilesParams {
            action: "copy".into(),
            name: Some(name.clone()),
            from: Some(from.clone()),
            kind: kind.kind(),
            ..Default::default()
        },
        ProfilesCommand::Delete { name, kind } => ProfilesParams {
            action: "delete".into(),
            name: Some(name.clone()),
            kind: kind.kind(),
            ..Default::default()
        },
        ProfilesCommand::Set {
            name,
            kind,
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
            env_set,
            env_unset,
            master_account,
            clear_master_account,
            worker_account,
            clear_worker_account,
            projects,
            clear_projects,
            ctx_threshold,
            clear_ctx_threshold,
            auto_handoff,
            clear_auto_handoff,
            limit_resume,
            clear_limit_resume,
        } => ProfilesParams {
            action: "set".into(),
            name: Some(name.clone()),
            kind: kind.kind(),
            from: None,
            projects: projects.clone(),
            clear_projects: *clear_projects,
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
            env_set: env_set.clone(),
            env_unset: env_unset.clone(),
            master_account: master_account.clone(),
            clear_master_account: *clear_master_account,
            worker_account: worker_account.clone(),
            clear_worker_account: *clear_worker_account,
            ctx_threshold: *ctx_threshold,
            clear_ctx_threshold: *clear_ctx_threshold,
            auto_handoff: *auto_handoff,
            clear_auto_handoff: *clear_auto_handoff,
            limit_resume: *limit_resume,
            clear_limit_resume: *clear_limit_resume,
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
                    new_tab: false,
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

/// `tako config`（Issue #513）。GUI が動いていなくても使えるローカル処理。
/// 実体は dispatch と共通なので、MCP `tako_config_share` と結果が食い違わない
fn config_share_local(args: &ConfigArgs) -> Result<(), String> {
    // CLI 単独で走るのでここでも表示言語を解決する（platform_local と同じ理由。#435）
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    let (action, target, path, remote, message, no_push) = match &args.command {
        None | Some(ConfigCommand::Status) => ("status", None, None, None, None, false),
        Some(ConfigCommand::List) => ("list", None, None, None, None, false),
        Some(ConfigCommand::Init { path, remote }) => (
            "init",
            None,
            path.as_deref(),
            remote.as_deref(),
            None,
            false,
        ),
        Some(ConfigCommand::Link { target, path }) => (
            "link",
            Some(target.as_str()),
            path.as_deref(),
            None,
            None,
            false,
        ),
        Some(ConfigCommand::Unlink) => ("unlink", None, None, None, None, false),
        Some(ConfigCommand::Push { message, no_push }) => {
            ("push", None, None, None, message.as_deref(), *no_push)
        }
        Some(ConfigCommand::Pull) => ("pull", None, None, None, None, false),
    };
    let result = tako_control::dispatch::dispatch_config_share(
        action, target, path, remote, message, no_push,
    )
    .map_err(|e| e.to_string())?;
    if args.json {
        println!("{}", pretty_json(&result));
        return Ok(());
    }
    print_config_share(action, &result);
    Ok(())
}

/// `tako config` の人向け表示。JSON の全量は `--json` で出せるので、ここは要点だけ
fn print_config_share(action: &str, result: &serde_json::Value) {
    use tako_core::i18n::Lang;
    let ja = matches!(tako_core::i18n::lang(), Lang::Ja);
    let t = |j: &'static str, e: &'static str| if ja { j } else { e };

    if action == "list" {
        for entry in result["entries"].as_array().into_iter().flatten() {
            let class = entry["class"].as_str().unwrap_or("?");
            println!(
                "{class:<7} {}/{}",
                entry["root"].as_str().unwrap_or("?"),
                entry["path"].as_str().unwrap_or("?")
            );
            if let Some(note) = entry["note"].as_str() {
                println!("        {note}");
            }
            let fields = entry["local_fields"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if !fields.is_empty() {
                let names: Vec<&str> = fields.iter().filter_map(|f| f.as_str()).collect();
                println!(
                    "        {}: {}",
                    t("共有しないフィールド", "fields kept local"),
                    names.join(", ")
                );
            }
        }
        let counts = &result["counts"];
        println!();
        println!(
            "shared {} / local {} / secret {}   ({})",
            counts["shared"],
            counts["local"],
            counts["secret"],
            t(
                "未分類は共有されません",
                "unclassified files are never shared"
            )
        );
        return;
    }

    if result["linked"] == serde_json::Value::Bool(false) {
        println!("{}", t("設定共有: 未配線", "config share: not linked"));
        if let Some(hint) = result["hint"].as_str() {
            println!("{hint}");
        }
        return;
    }

    if let Some(repo) = result["repo"].as_str() {
        println!("repo:   {repo}");
    }
    if let Some(remote) = result["remote"].as_str() {
        println!("remote: {remote}");
    }
    if let Some(branch) = result["branch"].as_str() {
        println!("branch: {branch}");
    }

    match action {
        "status" => {
            let summary = &result["summary"];
            println!(
                "{}: same {} / differs {} / local_only {} / repo_only {}",
                t("差分", "diff"),
                summary["same"].as_u64().unwrap_or(0),
                summary["differs"].as_u64().unwrap_or(0),
                summary["local_only"].as_u64().unwrap_or(0),
                summary["repo_only"].as_u64().unwrap_or(0),
            );
            for file in result["files"].as_array().into_iter().flatten() {
                let state = file["state"].as_str().unwrap_or("?");
                if state == "same" {
                    continue;
                }
                println!("  {state:<11} {}", file["path"].as_str().unwrap_or("?"));
            }
            print_list_section(
                t("共有しない（未分類）", "not shared (unclassified)"),
                &result["unclassified"],
            );
            print_list_section(
                t(
                    "リポジトリ内の管理外ファイル",
                    "untracked files in repository",
                ),
                &result["untracked_in_repo"],
            );
            print_list_section(
                t(
                    "可搬でない絶対パス（別デバイスで解決できません）",
                    "non-portable absolute paths (unresolvable on other devices)",
                ),
                &result["non_portable_paths"],
            );
        }
        "push" | "init" => {
            let push = if action == "init" {
                &result["push"]
            } else {
                result
            };
            println!(
                "{}: {} files",
                t("書き出し", "exported"),
                push["written"].as_u64().unwrap_or(0)
            );
            if push["committed"] == serde_json::Value::Bool(true) {
                println!(
                    "{}: {}",
                    t("コミット", "committed"),
                    push["commit"].as_str().unwrap_or("-")
                );
            } else {
                println!("{}", t("変更なし（コミットなし）", "no changes to commit"));
            }
            if push["pushed"] == serde_json::Value::Bool(true) {
                println!("{}", t("リモートへ push しました", "pushed to remote"));
            } else if let Some(err) = push["push_error"].as_str() {
                println!("{}: {err}", t("push に失敗", "push failed"));
            }
            print_list_section(
                t(
                    "リポジトリ内の管理外ファイル",
                    "untracked files in repository",
                ),
                &push["untracked_in_repo"],
            );
        }
        "pull" => {
            let applied = result["applied"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let changed: Vec<&serde_json::Value> = applied
                .iter()
                .filter(|a| a["action"] != "unchanged")
                .collect();
            println!(
                "{}: {} / {}",
                t("取り込み", "applied"),
                changed.len(),
                applied.len()
            );
            for a in changed {
                println!(
                    "  {:<9} {}",
                    a["action"].as_str().unwrap_or("?"),
                    a["path"].as_str().unwrap_or("?")
                );
            }
            let needs = result["needs_local"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if !needs.is_empty() {
                println!();
                println!(
                    "{}",
                    t(
                        "このデバイスで設定が必要な値（共有されない項目）:",
                        "values that must be set on this device (never shared):"
                    )
                );
                for n in needs {
                    println!(
                        "  {} → {}",
                        n["path"].as_str().unwrap_or("?"),
                        n["field"].as_str().unwrap_or("?")
                    );
                }
            }
        }
        _ => {
            if let Some(hint) = result["hint"].as_str() {
                println!("{hint}");
            }
        }
    }
}

fn print_list_section(title: &str, value: &serde_json::Value) {
    let items = value.as_array().map(Vec::as_slice).unwrap_or(&[]);
    if items.is_empty() {
        return;
    }
    println!();
    println!("{title}: {}", items.len());
    for item in items.iter().take(20) {
        match item.as_str() {
            Some(s) => println!("  {s}"),
            None => println!("  {item}"),
        }
    }
    if items.len() > 20 {
        println!("  …");
    }
}

/// プラットフォーム対応マトリクスの表示（#515。ローカル処理・IPC 不要）。
///
/// 応答の組み立ては `tako_control::platform::report` を通す。MCP `tako_platform` と
/// **同じ 1 本**なので、CLI と AI で見える内容が食い違わない
fn shell_integration_local(args: &ShellIntegrationArgs) -> Result<(), String> {
    let out = tako_control::shell_integration::run(args.action.as_deref())?;
    if args.json {
        println!("{}", pretty_json(&out));
        return Ok(());
    }

    let delivery = out["delivery"].as_str().unwrap_or("?");
    println!("シェル統合: {}", out["shells"].as_str().unwrap_or("?"));
    println!(
        "  届け方   : {}",
        match delivery {
            "automatic" => "環境変数の注入（ユーザーのファイルは触らない）",
            "profile" => "PowerShell の $PROFILE へブロックを配置",
            other => other,
        }
    );
    if let Some(script) = out["script"].as_str() {
        println!("  スクリプト: {script}");
    }
    for t in out["targets"].as_array().into_iter().flatten() {
        let state = match (t["installed"].as_bool(), t["up_to_date"].as_bool()) {
            (Some(true), Some(true)) => "配置済み",
            (Some(true), _) => "配置済み（内容が古い。install で更新できます）",
            _ => "未配置",
        };
        println!(
            "  - {:<24} {state}\n    {}",
            t["label"].as_str().unwrap_or("?"),
            t["path"].as_str().unwrap_or("?")
        );
    }
    for c in out["changes"].as_array().into_iter().flatten() {
        println!(
            "  → {} {}",
            c["kind"].as_str().unwrap_or("?"),
            c["path"].as_str().unwrap_or("?")
        );
    }
    // 「配置できているのに効かない」を必ず言う（#525。psmux は OSC を通さない）
    if let Some(note) = out["blocked_by_backend"].as_str() {
        println!("  [警告] {note}");
    }
    println!(
        "  実効     : {}",
        if out["effective"].as_bool() == Some(true) {
            "有効"
        } else {
            "無効"
        }
    );
    Ok(())
}

/// `tako migrate`（Issue #916）。GUI 無しで動くローカル処理
fn migrate_local(args: &MigrateArgs) -> Result<(), String> {
    // CLI 単独で走るので表示言語を settings.json から解決する（platform と同じ。#435）
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    let action = args.action.as_deref().unwrap_or("status");
    let result = tako_control::migrations::report_json(action, args.schema.as_deref())?;
    println!("{}", pretty_json(&result));
    Ok(())
}

fn platform_local(args: &PlatformArgs) -> Result<(), String> {
    // 表示言語のグローバルは既定が英語。GUI は起動時に settings.json から解決するので、
    // CLI 単独で走るここでも同じ解決をしないと日本語設定なのに英語で出てしまう（#435）
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    let report = tako_control::platform::report(
        args.platform.as_deref(),
        args.status.as_deref(),
        args.known_limitations,
    )?;
    if args.json {
        println!("{}", pretty_json(&report));
        return Ok(());
    }
    // リリースノートへ差し込むための素の markdown 出力（#594）。
    // 他の表示を混ぜない = そのままリダイレクトできる
    if args.known_limitations {
        let md = report["known_limitations_markdown"].as_str().unwrap_or("");
        if !md.is_empty() {
            print!("{md}");
        }
        return Ok(());
    }

    let target = report["platform"].as_str().unwrap_or("?");
    let current = report["current"].as_str().unwrap_or("?");
    let total = report["total"].as_u64().unwrap_or(0);
    use tako_core::i18n::Lang;
    let here = if target == current {
        match tako_core::i18n::lang() {
            Lang::Ja => "（実行中の環境）",
            Lang::En => " (current environment)",
        }
    } else {
        ""
    };
    println!("platform: {target}{here}");
    if let Some(counts) = report["counts"].as_object() {
        let line: Vec<String> = ["supported", "degraded", "pending", "unsupported"]
            .iter()
            .filter_map(|k| {
                counts
                    .get(*k)
                    .and_then(|v| v.as_u64())
                    .map(|n| format!("{k} {n}"))
            })
            .collect();
        println!("counts:   {}", line.join(" / "));
    }
    println!();

    for f in report["features"].as_array().into_iter().flatten() {
        let key = f["key"].as_str().unwrap_or("?");
        let status = f["status"].as_str().unwrap_or("?");
        print!("{status:<12} {key}");
        if let Some(issue) = f["issue"].as_u64() {
            print!("  #{issue}");
        }
        println!();
        if let Some(note) = f["note"].as_str() {
            println!("{:<12} {note}", "");
        }
    }
    if total == 0 {
        println!(
            "{}",
            match tako_core::i18n::lang() {
                Lang::Ja => "（該当なし）",
                Lang::En => "(no matches)",
            }
        );
    }
    Ok(())
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
            // 蓋の開閉は macOS でしか観測できない。取れない OS で「開」と言い切ると
            // 嘘になるので不明と出す（#697）
            if tako_control::sleep_guard::lid_state_detectable() {
                eprintln!("  蓋: {}", if state.lid_closed { "閉" } else { "開" });
            } else {
                eprintln!("  蓋: 不明（この OS では開閉を検知できません）");
            }
            // sudoers は macOS 固有の手段。要らない OS では出さない（#697）
            if tako_control::sleep_guard::lid_requires_privileged_setup() {
                eprintln!(
                    "  蓋閉じ防止: {} (sudoers: {})",
                    state.lid_sleep_mode.as_str(),
                    if state.sudoers_installed {
                        "登録済み"
                    } else {
                        "未登録"
                    }
                );
            } else if tako_control::sleep_guard::lid_control_supported() {
                eprintln!("  蓋閉じ防止: {}", state.lid_sleep_mode.as_str());
            } else {
                eprintln!("  蓋閉じ防止: この OS では対応していません");
            }
            eprintln!(
                "  蓋閉じ継続の適用: {}",
                if state.lid_sleep_disabled {
                    "有効"
                } else {
                    "無効"
                }
            );
            // thermal は macOS でしか取れない。取れない OS で常に nominal と出しても情報がない
            if tako_control::sleep_guard::lid_requires_privileged_setup()
                || state.thermal_state != tako_control::sleep_guard::ThermalState::Nominal
            {
                eprintln!("  thermal: {}", state.thermal_state.as_str());
            }
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
            // 案内は「この OS で何が起きるか」で出し分ける。
            // 分岐は sleep_guard 側に閉じているのでここは単一経路（#697）
            if tako_control::sleep_guard::lid_setup_pending() {
                eprintln!("蓋閉じ防止の sudoers 登録を行います...");
                eprintln!("  登録内容: pmset -a disablesleep 0/1 のみ NOPASSWD");
                eprintln!("  管理者パスワードの入力ダイアログが表示されます。");
            } else {
                eprintln!("蓋閉じ防止を有効にします...");
            }
            let result = tako_control::sleep_guard::prepare_lid_control()?;
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
            let result = tako_control::sleep_guard::teardown_lid_control()?;
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

/// master / solo の起動先ペイン（Issue #567）
struct LaunchTarget {
    pane: u64,
    /// 新規タブを作ったか（表示文言と復旧案内の出し分けに使う）
    new_tab: bool,
}

/// 起動場所の表示文言（Issue #567。フォールバックで新タブになった場合もタブ表記になる）
fn launch_location(tab_title: &str, target: &LaunchTarget) -> String {
    if target.new_tab {
        format!("タブ '{tab_title}'（ペイン {}）", target.pane)
    } else {
        format!("ペイン {}（インライン）", target.pane)
    }
}

/// 案内文に出す最簡形のコマンド（#322。既定プロファイルなら引数を付けない）
fn master_cmd_hint(profile_name: &str) -> String {
    profile_cmd_hint("tako master", profile_name)
}

fn solo_cmd_hint(profile_name: &str) -> String {
    profile_cmd_hint("tako solo", profile_name)
}

fn profile_cmd_hint(base: &str, profile_name: &str) -> String {
    if profile_name == "default" {
        base.to_string()
    } else {
        format!("{base} -{profile_name}")
    }
}

/// master / solo の起動先ペインを決める（Issue #567）。
///
/// `TAKO_PANE_ID` はシェルの再利用やアプリ再起動をまたぐと古くなる。古い ID のまま
/// 起動しようとして「ペイン N が見つからない」で止まると、master 消失からの復旧という
/// 最も急いでいる場面で手が止まるため、次の順で**必ず起動先を確保する**:
///
/// 1. アプリに現世代のペインを問い合わせる（pid 祖先辿り → pane → stale map。#210 / #288）
/// 2. 解決できなければ「呼び出し元不明」として新規タブを作る
///
/// アプリへ届かないときだけエラーで止め、復旧手順を添える。
/// `cmd_hint` は案内文に出す最簡形のコマンド（例: `tako master -fable`。#322）
fn resolve_launch_target(
    tab_title: &str,
    use_tab: bool,
    cmd_hint: &str,
) -> Result<LaunchTarget, String> {
    let requested = caller_pane();
    if use_tab {
        return new_tab_target(tab_title, cmd_hint, requested);
    }
    let resolved = resolve_caller_pane_via_app(requested)
        .map_err(|e| launch_failure_message(&e, cmd_hint, requested))?;
    match resolved {
        Some(caller) => {
            let pane = caller.pane;
            if let Some(old) = requested.filter(|old| *old != pane) {
                if caller.method.as_deref() == Some("stale") {
                    eprintln!(
                        "ℹ TAKO_PANE_ID={old} は旧世代のペイン ID です（アプリ再起動をまたいだ値）"
                    );
                    eprintln!("  現世代のペイン {pane} へ読み替えて起動します");
                } else {
                    eprintln!(
                        "ℹ TAKO_PANE_ID={old} は呼び出し元ペインと一致しません（シェルが古い値を持っています）"
                    );
                    eprintln!("  実際の呼び出し元ペイン {pane} で起動します");
                }
                eprintln!("  このシェルを使い続けるなら: unset TAKO_PANE_ID");
            }
            send_request(Request::TabRename {
                tab: None,
                pane: Some(pane),
                title: tab_title.to_string(),
                source: None,
            })
            .ok();
            Ok(LaunchTarget {
                pane,
                new_tab: false,
            })
        }
        None => {
            match requested {
                Some(old) => eprintln!(
                    "ℹ 呼び出し元ペインを特定できません（TAKO_PANE_ID={old} は現在の tako に無い古い値）"
                ),
                None => {
                    eprintln!("ℹ 呼び出し元ペインを特定できません（TAKO_PANE_ID 未設定）")
                }
            }
            eprintln!("  新しいタブ '{tab_title}' を作ってそこで起動します");
            if requested.is_some() {
                eprintln!("  このシェルを使い続けるなら: unset TAKO_PANE_ID");
            }
            new_tab_target(tab_title, cmd_hint, requested)
        }
    }
}

/// 新規タブを作って起動先にする（Issue #567 のフォールバック / `--tab` 指定時）
fn new_tab_target(
    tab_title: &str,
    cmd_hint: &str,
    requested: Option<u64>,
) -> Result<LaunchTarget, String> {
    let tab_result = send_request(Request::TabNew {
        title: Some(tab_title.to_string()),
        focus: Some(true),
        cwd: None,
    })
    .map_err(|e| launch_failure_message(&e, cmd_hint, requested))?;
    let pane = tab_result["pane"]
        .as_u64()
        .ok_or("タブ作成の応答に pane が含まれない")?;
    Ok(LaunchTarget {
        pane,
        new_tab: true,
    })
}

/// 起動先を確保できないときのエラー文（Issue #567）。
/// フォールバック（新規タブ）まで届かない = アプリへ接続できていない状態なので、
/// 復旧手順を必ず添える。コマンドは最簡形で示す（#322）
fn launch_failure_message(cause: &str, cmd_hint: &str, requested: Option<u64>) -> String {
    let mut message = format!(
        "起動先ペインを確保できない: {cause}\n\
         復旧: tako アプリを起動し、その中のターミナルで `{cmd_hint}` を実行してください"
    );
    if let Some(old) = requested {
        message.push_str(&format!(
            "\n      このシェルの TAKO_PANE_ID={old} は古い可能性があります: \
             `unset TAKO_PANE_ID` してから再実行してください"
        ));
    }
    message
}

/// 呼び出し元ペインの解決結果（Issue #567）
struct ResolvedCaller {
    pane: u64,
    /// 解決手段（`pid` / `pane` / `stale`。縮退経路では `None`）。案内文の出し分けに使う
    method: Option<String>,
}

/// アプリへ現世代の呼び出し元ペインを問い合わせる（Issue #567）。
/// 解決できなければ `Ok(None)`（エラーではない）。アプリへ届かないときだけ `Err`。
///
/// 新旧バイナリ混在（`resolve_pane` を知らない古い GUI が動いている）でも止まらないよう、
/// 問い合わせに失敗したら `list` でのペイン実在確認へ縮退する（stale map / pid 解決は
/// 効かないが、少なくとも「古い ID のまま起動して失敗する」ことは無くなる）
fn resolve_caller_pane_via_app(requested: Option<u64>) -> Result<Option<ResolvedCaller>, String> {
    let request = Request::ResolvePane {
        pane: requested,
        caller_pid: Some(std::process::id()),
    };
    match send_request(request) {
        Ok(value) => Ok(value["pane"].as_u64().map(|pane| ResolvedCaller {
            pane,
            method: value["method"].as_str().map(str::to_string),
        })),
        Err(e) => match send_request(Request::List) {
            Ok(list) => Ok(requested
                .filter(|p| list_contains_pane(&list, *p))
                .map(|pane| ResolvedCaller { pane, method: None })),
            Err(_) => Err(e),
        },
    }
}

/// `list` 応答に該当ペインが居るか（Issue #567 の縮退経路）
fn list_contains_pane(list: &Value, pane: u64) -> bool {
    list["tabs"]
        .as_array()
        .is_some_and(|tabs| tabs.iter().any(|t| tab_contains_pane(t, pane)))
}

fn tab_contains_pane(tab: &Value, pane: u64) -> bool {
    tab["panes"]
        .as_array()
        .is_some_and(|panes| panes.iter().any(|p| p["id"].as_u64() == Some(pane)))
}

/// 相対パスを CLI 実行時の cwd で絶対化する。`--pane` で別ペインを指定しても
/// 「いま居る場所」基準のまま意図どおりに解決させるため（cwd を取れなければ
/// そのまま渡し、アプリ側のペイン cwd 解決に任せる）
fn absolutize(path: &str) -> String {
    let p = std::path::Path::new(path);
    if !p.is_relative() {
        return path.to_string();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(p).display().to_string())
        .unwrap_or_else(|_| path.to_string())
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
            // #566: エージェントのペインから `tako close` を叩いたとき
            // 「どの role が閉じたか」をペインログへ残す（監査情報。close の可否には影響しない）
            caller_role: std::env::var("TAKO_ORCHESTRATOR_ROLE")
                .ok()
                .filter(|r| !r.trim().is_empty()),
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
            Request::OpenFile {
                pane: target_pane(args.pane)?,
                path: absolutize(&args.path),
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
                new_tab: args.new_tab,
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
        Command::PreviewCopyCode(args) => Request::PreviewCopyCode {
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
        Command::Tab(TabCommand::New { title, focus, cwd }) => Request::TabNew {
            title: title.clone(),
            focus: if *focus { Some(true) } else { None },
            // 相対パスは CLI 実行時の cwd で絶対化する（`tako open` と同じ規則）
            cwd: cwd.as_ref().map(|c| absolutize(c)),
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
        Command::Tab(TabCommand::Pin { tab, off, status }) => Request::TabPinTitle {
            pane: if tab.is_none() {
                target_pane(None)?
            } else {
                None
            },
            tab: *tab,
            pinned: if *status { None } else { Some(!*off) },
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
        Command::Window(WindowCommand::Minimize { window }) => {
            Request::WindowMinimize { window: *window }
        }
        Command::Window(WindowCommand::Maximize { window }) => {
            Request::WindowMaximize { window: *window }
        }
        Command::Window(WindowCommand::Restore { window }) => {
            Request::WindowRestore { window: *window }
        }
        Command::Menu(MenuCommand::List) => Request::MenuList,
        Command::Menu(MenuCommand::Open { menu }) => Request::MenuOpen { menu: menu.clone() },
        Command::Menu(MenuCommand::Close) => Request::MenuClose,
        Command::Menu(MenuCommand::Invoke { path }) => Request::MenuInvoke { path: path.clone() },
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
            // value_parser で正式値・旧称ともに検証済み（#553）
            view: args
                .view
                .as_deref()
                .and_then(tako_control::protocol::PanelViewWire::parse),
            filetree: args.filetree.as_deref().map(|s| s == "on"),
            sidebar_width: args.sidebar_width,
            show_hidden: args.show_hidden.as_deref().map(|s| s == "on"),
        },
        Command::Portdetect(args) => Request::PortDetect {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::Autosuggest(args) => {
            let on = |v: &Option<String>| v.as_deref().map(|s| s == "on");
            match args.target_or_state.as_deref() {
                Some("hint") => Request::Autosuggest {
                    enabled: None,
                    hint: on(&args.state),
                    tab: None,
                },
                Some("tab") => Request::Autosuggest {
                    enabled: None,
                    hint: None,
                    tab: on(&args.state),
                },
                other => Request::Autosuggest {
                    enabled: other.map(|s| s == "on"),
                    hint: None,
                    tab: None,
                },
            }
        }
        Command::ConfirmClose(args) => Request::ConfirmClose {
            enabled: args.state.as_deref().map(|s| s == "on"),
        },
        Command::LimitResume(args) => Request::LimitResume {
            pane: args.pane.or_else(caller_pane),
            enabled: args.state.as_deref().map(|s| s == "on"),
            all: args.all.then_some(true),
        },
        Command::Theme(args) => {
            let m = args.mode.as_deref();
            match m {
                Some("colors") => Request::Theme {
                    action: Some("colors".into()),
                    mode: None,
                    target: args.target.clone(),
                    key: None,
                    value: None,
                    name: None,
                    font_family: None,
                    font_size: None,
                },
                Some("color") => {
                    let k = args.name_or_key.clone();
                    if args.reset {
                        Request::Theme {
                            action: Some("reset-color".into()),
                            mode: None,
                            target: args.target.clone(),
                            key: k,
                            value: None,
                            name: None,
                            font_family: None,
                            font_size: None,
                        }
                    } else {
                        Request::Theme {
                            action: Some("set-color".into()),
                            mode: None,
                            target: args.target.clone(),
                            key: k,
                            value: args.value_or_action.clone(),
                            name: None,
                            font_family: None,
                            font_size: None,
                        }
                    }
                }
                Some("reset-colors") => Request::Theme {
                    action: Some("reset-colors".into()),
                    mode: None,
                    target: args.target.clone(),
                    key: None,
                    value: None,
                    name: None,
                    font_family: None,
                    font_size: None,
                },
                Some("preset") => {
                    let sub = args.name_or_key.as_deref();
                    match sub {
                        Some("save") => Request::Theme {
                            action: Some("save-preset".into()),
                            mode: None,
                            target: None,
                            key: None,
                            value: None,
                            name: args.value_or_action.clone(),
                            font_family: None,
                            font_size: None,
                        },
                        Some("delete") => Request::Theme {
                            action: Some("delete-preset".into()),
                            mode: None,
                            target: None,
                            key: None,
                            value: None,
                            name: args.value_or_action.clone(),
                            font_family: None,
                            font_size: None,
                        },
                        _ => Request::Theme {
                            action: Some("status".into()),
                            mode: None,
                            target: None,
                            key: None,
                            value: None,
                            name: None,
                            font_family: None,
                            font_size: None,
                        },
                    }
                }
                Some("font") => Request::Theme {
                    action: Some("set-font".into()),
                    mode: None,
                    target: None,
                    key: None,
                    value: None,
                    name: None,
                    font_family: args.name_or_key.clone(),
                    font_size: args.size,
                },
                Some("toggle") => Request::Theme {
                    action: Some("toggle".into()),
                    mode: None,
                    target: None,
                    key: None,
                    value: None,
                    name: None,
                    font_family: None,
                    font_size: None,
                },
                Some(mode_val) => Request::Theme {
                    action: Some("set".into()),
                    mode: Some(mode_val.to_string()),
                    target: None,
                    key: None,
                    value: None,
                    name: None,
                    font_family: None,
                    font_size: None,
                },
                None => Request::Theme {
                    action: None,
                    mode: None,
                    target: None,
                    key: None,
                    value: None,
                    name: None,
                    font_family: None,
                    font_size: None,
                },
            }
        }
        Command::Settings(args) => Request::Settings {
            action: Some("open".into()),
            tab: args.tab.clone(),
        },
        Command::Welcome(args) => Request::Welcome {
            action: args.action.clone(),
        },
        Command::ShowCommand(args) => Request::ShowCommand {
            action: Some(args.action().to_string()),
            commands: args.commands.clone(),
            label: args.label.clone(),
            pane: args.pane,
            card: args.card,
            index: args.index,
            focus: Some(args.focus).filter(|f| *f),
        },
        Command::Lang(args) => Request::Lang {
            action: args.value.as_deref().map(|_| "set".to_string()),
            value: args.value.clone(),
        },
        // #694: `tako ui-mode` は `tako theme` と同型（引数なし = 現在値、値 = set、
        // toggle = 反転）。release / restore だけペイン単位の揮発操作
        Command::UiMode(args) => match args.action.as_deref() {
            Some(action @ ("release" | "restore")) => Request::UiMode {
                action: Some(action.to_string()),
                mode: None,
                pane: target_pane(args.pane)?,
            },
            Some("toggle") => Request::UiMode {
                action: Some("toggle".into()),
                mode: None,
                pane: None,
            },
            Some(mode) => Request::UiMode {
                action: Some("set".into()),
                mode: Some(mode.to_string()),
                pane: None,
            },
            None => Request::UiMode {
                action: None,
                mode: None,
                pane: None,
            },
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
        Command::Git(GitCommand::Show { hash, file, pane }) => Request::GitShow {
            pane: target_pane(*pane)?,
            hash: hash.clone(),
            file: file.clone(),
        },
        Command::Git(GitCommand::Commit { message, all, pane }) => Request::GitCommit {
            pane: target_pane(*pane)?,
            message: message.clone(),
            all: *all,
        },
        Command::Git(GitCommand::Pull { pane }) => Request::GitPull {
            pane: target_pane(*pane)?,
        },
        Command::Git(GitCommand::Push { pane }) => Request::GitPush {
            pane: target_pane(*pane)?,
        },
        Command::Git(GitCommand::Stage { paths, pane }) => Request::GitStage {
            pane: target_pane(*pane)?,
            paths: paths.clone(),
        },
        Command::Git(GitCommand::Unstage { paths, pane }) => Request::GitUnstage {
            pane: target_pane(*pane)?,
            paths: paths.clone(),
        },
        Command::Git(GitCommand::Checkout { branch, yes, pane }) => Request::GitCheckout {
            pane: target_pane(*pane)?,
            branch: branch.clone(),
            confirm: *yes,
        },
        Command::Git(GitCommand::Branch {
            name,
            from,
            no_checkout,
            pane,
        }) => Request::GitBranchCreate {
            pane: target_pane(*pane)?,
            name: name.clone(),
            start_point: from.clone(),
            checkout: Some(!*no_checkout),
        },
        Command::Git(GitCommand::Merge {
            branch,
            yes,
            no_ff,
            pane,
        }) => Request::GitMerge {
            pane: target_pane(*pane)?,
            branch: branch.clone(),
            confirm: *yes,
            no_ff: *no_ff,
        },
        Command::Git(GitCommand::Abort { pane }) => Request::GitMergeAbort {
            pane: target_pane(*pane)?,
        },
        Command::Git(GitCommand::Conflicts { pane }) => Request::GitConflicts {
            pane: target_pane(*pane)?,
        },
        Command::Git(GitCommand::Resolve { agent, tab, pane }) => Request::GitResolveAgent {
            pane: target_pane(*pane)?,
            agent: agent.clone(),
            tab: *tab,
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
        Command::Video(VideoCommand::Status { pane }) => Request::VideoPlayback {
            pane: target_pane(*pane)?,
            action: "status".into(),
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
            account,
            limit_resume,
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
                account: account.clone(),
                limit_resume: *limit_resume,
            }
        }
        Command::Orchestrator(OrchestratorCommand::SelfInfo { .. }) => {
            unreachable!("orchestrator self は run() を通らない（main() でローカル処理済み）")
        }
        Command::Orchestrator(OrchestratorCommand::Handoffs(_)) => {
            unreachable!("orchestrator handoffs は run() を通らない（main() でローカル処理済み）")
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
        Command::Orchestrator(OrchestratorCommand::Accounts(_)) => {
            unreachable!(
                "orchestrator accounts は run() を通らない（ローカルで accounts.yaml を操作）"
            )
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
        Command::StaleBinary(sub) => {
            let (action, pane) = match sub {
                StaleBinaryCommand::Status { pane } => ("status", *pane),
                StaleBinaryCommand::Restart { pane } => ("restart", *pane),
                StaleBinaryCommand::Dismiss { pane } => ("dismiss", *pane),
            };
            Request::StaleBinary {
                action: Some(action.to_string()),
                pane,
            }
        }
        Command::Update(sub) => {
            let (action, channel) = match sub {
                UpdateCommand::Status => ("status", None),
                UpdateCommand::Check { channel } => ("check", channel.clone()),
                UpdateCommand::Apply { channel } => ("apply", channel.clone()),
                UpdateCommand::ApplyZip { channel } => ("apply-zip", channel.clone()),
                UpdateCommand::Repair => ("repair", None),
                // #616: 専用画面 + 通知カード
                UpdateCommand::Open => ("open", None),
                UpdateCommand::Card { action } => (
                    match action.as_deref() {
                        Some("dismiss") => "card-dismiss",
                        Some("show") => "card-show",
                        _ => "card",
                    },
                    None,
                ),
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
        Command::Chat(sub) => match sub {
            ChatCommand::Copy {
                pane,
                message,
                code,
                markdown,
                list,
            } => Request::ChatCopy {
                pane: pane.or_else(caller_pane),
                list: *list,
                message: *message,
                code: *code,
                markdown: *markdown,
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
        Command::Platform(_) => unreachable!("platform は run() を通らない（ローカル処理）"),
        Command::Migrate(_) => unreachable!("migrate は run() を通らない（ローカル処理）"),
        Command::ShellIntegration(_) => {
            unreachable!("shell-integration は run() を通らない（ローカル処理）")
        }
        Command::Config(_) => unreachable!("config は run() を通らない（ローカル処理）"),
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
            OpenInCommand::Remote {
                host,
                no_focus,
                remote_dir,
            } => Request::OpenRemote {
                host: host.clone(),
                focus: Some(!no_focus),
                remote_dir: remote_dir.clone(),
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
        Command::RemoteFolder(sub) => match sub {
            RemoteFolderCommand::Open { host, path, tab } => Request::RemoteFolder {
                action: "open".into(),
                host: Some(host.clone()),
                path: path.clone(),
                tab: *tab,
                focus: None,
                all: false,
                force: false,
            },
            RemoteFolderCommand::Close {
                host,
                path,
                all,
                tab,
            } => Request::RemoteFolder {
                action: "close".into(),
                host: host.clone(),
                path: path.clone(),
                tab: *tab,
                focus: None,
                all: *all,
                force: false,
            },
            RemoteFolderCommand::List => Request::RemoteFolder {
                action: "list".into(),
                host: None,
                path: None,
                tab: None,
                focus: None,
                all: false,
                force: false,
            },
            RemoteFolderCommand::Ls { host, path } => Request::RemoteFolder {
                action: "ls".into(),
                host: Some(host.clone()),
                path: path.clone(),
                tab: None,
                focus: None,
                all: false,
                force: false,
            },
            RemoteFolderCommand::OpenFile {
                host,
                path,
                no_focus,
            } => Request::RemoteFolder {
                action: "open-file".into(),
                host: Some(host.clone()),
                path: Some(path.clone()),
                tab: None,
                focus: Some(!no_focus),
                all: false,
                force: false,
            },
            RemoteFolderCommand::SshPane {
                host,
                path,
                no_focus,
            } => Request::RemoteFolder {
                action: "ssh-pane".into(),
                host: Some(host.clone()),
                path: path.clone(),
                tab: None,
                focus: Some(!no_focus),
                all: false,
                force: false,
            },
            RemoteFolderCommand::Pending { host, path } => Request::RemoteFolder {
                action: "pending".into(),
                host: host.clone(),
                path: path.clone(),
                tab: None,
                focus: None,
                all: false,
                force: false,
            },
            RemoteFolderCommand::Push { host, path, force } => Request::RemoteFolder {
                action: "push".into(),
                host: host.clone(),
                path: path.clone(),
                tab: None,
                focus: None,
                all: false,
                force: *force,
            },
        },
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
            // 器がある構成はセッション名、無い構成はペイン ID がキー（#728）
            let key = p["tmux_session"]
                .as_str()
                .map(str::to_string)
                .or_else(|| p["pane"].as_u64().map(|n| format!("pane {n}")))
                .unwrap_or_else(|| "-".into());
            println!(
                "{}  {}  {}  {}",
                p["recorded_at"].as_str().unwrap_or("-"),
                p["agent"].as_str().unwrap_or("-"),
                key,
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
        Command::Tab(TabCommand::New { .. }) | Command::Tab(TabCommand::Pin { .. }) => {
            println!("{result}")
        }
        Command::Window(WindowCommand::List) => println!("{}", pretty_json(result)),
        // #657: list は構成そのままの JSON、open / invoke は解決結果（1 行）
        Command::Menu(MenuCommand::List) => println!("{}", pretty_json(result)),
        Command::Menu(
            MenuCommand::Open { .. } | MenuCommand::Close | MenuCommand::Invoke { .. },
        ) => println!("{result}"),
        Command::Window(
            WindowCommand::New { .. }
            | WindowCommand::Close { .. }
            | WindowCommand::MoveTab { .. }
            | WindowCommand::Minimize { .. }
            | WindowCommand::Maximize { .. }
            | WindowCommand::Restore { .. },
        ) => println!("{result}"),
        Command::Open(_) | Command::Preview(_) | Command::PreviewOutline(_) | Command::Edit(_) => {
            println!("{result}")
        }
        Command::PreviewLinkList(_) => println!("{}", pretty_json(result)),
        Command::PreviewFollowLink(_) => println!("{result}"),
        // #680: コピーしたコード全文は改行込みで読みたいので整形して出す
        Command::PreviewCopyCode(_) => println!("{}", pretty_json(result)),
        Command::PreviewReload(_) | Command::PreviewCache(_) | Command::PreviewChangelog(_) => {
            println!("{result}")
        }
        // #666: カードの内容（論理文字列）は改行込みで読みたいので整形して出す
        Command::ShowCommand(_) => println!("{}", pretty_json(result)),
        Command::Autorename(_)
        | Command::Portdetect(_)
        | Command::Autosuggest(_)
        | Command::Persist(_)
        | Command::ConfirmClose(_)
        | Command::Theme(_)
        | Command::Settings(_)
        | Command::Welcome(_)
        | Command::Lang(_)
        | Command::UiMode(_)
        | Command::LimitService(_)
        | Command::Telemetry(_)
        | Command::Panel(_)
        | Command::Collapse(_)
        | Command::Pin(_) => {
            println!("{result}")
        }
        // #813: 状態（stop / resume_at / attempts）が入れ子なので整形して出す
        Command::LimitResume(_) => println!("{}", pretty_json(result)),
        Command::Git(GitCommand::Log { .. })
        | Command::Git(GitCommand::Diff { .. })
        | Command::Git(GitCommand::Show { .. })
        // #496: 事前提示（preview）を含む構造化応答なので整形して出す。
        // ここへの登録漏れは #495 で「空応答」として実機に出た経路
        | Command::Git(GitCommand::Checkout { .. })
        | Command::Git(GitCommand::Branch { .. })
        | Command::Git(GitCommand::Merge { .. })
        | Command::Git(GitCommand::Abort { .. })
        | Command::Git(GitCommand::Conflicts { .. })
        | Command::Git(GitCommand::Resolve { .. }) => {
            println!("{}", pretty_json(result));
        }
        Command::Git(GitCommand::Commit { .. })
        | Command::Git(GitCommand::Pull { .. })
        | Command::Git(GitCommand::Push { .. })
        | Command::Git(GitCommand::Stage { .. })
        | Command::Git(GitCommand::Unstage { .. }) => {
            println!("{result}")
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
        Command::StaleBinary(_) => println!("{}", pretty_json(result)),
        Command::Update(_) => println!("{}", pretty_json(result)),
        Command::Chat(_) => println!("{}", pretty_json(result)),
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
        Command::RemoteFolder(_) => println!("{}", pretty_json(result)),
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

mod transport {
    //! Layer 1 IPC のクライアント側。ワイヤ処理（1 行 1 JSON）はトランスポート
    //! 非依存（`roundtrip_on`）で、接続の確立だけがプラットフォームで異なる
    //! （unix: Unix domain socket / windows: named pipe = 抽象境界 B3）

    use std::io::{BufRead, BufReader, Read, Write};

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
        let (read_half, write_half) = connect(socket)?;
        roundtrip_on(read_half, write_half, token, request, origin)
    }

    #[cfg(unix)]
    fn connect(socket: &str) -> Result<(impl Read, impl Write), TransportError> {
        use std::os::unix::net::UnixStream;
        let stream = UnixStream::connect(socket).map_err(|e| {
            TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
        })?;
        let read_half = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        Ok((read_half, stream))
    }

    #[cfg(windows)]
    fn connect(socket: &str) -> Result<(impl Read, impl Write), TransportError> {
        let stream =
            tako_control::platform::named_pipe::connect_client(socket, 3_000).map_err(|e| {
                TransportError::Connect(format!("tako アプリへ接続できない（{socket}: {e}）"))
            })?;
        let read_half = stream
            .try_clone()
            .map_err(|e| TransportError::Other(format!("接続の複製に失敗: {e}")))?;
        Ok((read_half, stream))
    }

    fn roundtrip_on<R: Read, W: Write>(
        read_half: R,
        mut write_half: W,
        token: &str,
        request: Request,
        origin: Option<&str>,
    ) -> Result<Value, TransportError> {
        let mut envelope = RequestEnvelope::new(1, token, request);
        envelope.origin = origin.map(Into::into);
        let json = serde_json::to_string(&envelope)
            .map_err(|e| TransportError::Other(format!("送信の構築に失敗: {e}")))?;
        writeln!(write_half, "{json}")
            .map_err(|e| TransportError::Other(format!("送信に失敗: {e}")))?;

        let mut line = String::new();
        BufReader::new(read_half)
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

    /// #511: dispatch にある account が CLI からも指定できる（1:1 不変条件）
    #[test]
    fn spawnのaccountがrequestへ渡る() {
        let command = parse(&[
            "tako",
            "orchestrator",
            "spawn",
            "--project",
            "tako",
            "--prompt",
            "テスト",
            "--pane",
            "3",
            "--account",
            "personal",
        ]);
        let request = build_request(&command).unwrap();
        match request {
            Request::OrchestratorSpawn { account, .. } => {
                assert_eq!(account.as_deref(), Some("personal"))
            }
            other => panic!("想定外の Request: {other:?}"),
        }
        // 省略時は None（従来挙動 = プロファイルの worker_account 解決に委ねる）
        let command = parse(&[
            "tako",
            "orchestrator",
            "spawn",
            "--project",
            "tako",
            "--prompt",
            "テスト",
            "--pane",
            "3",
        ]);
        match build_request(&command).unwrap() {
            Request::OrchestratorSpawn { account, .. } => assert_eq!(account, None),
            other => panic!("想定外の Request: {other:?}"),
        }
    }

    /// #511: run 側も同じく account を受け取る（spawn と対称）
    #[test]
    fn runのaccountフラグがパースできる() {
        let command = parse(&[
            "tako",
            "orchestrator",
            "run",
            "--project",
            "tako",
            "--prompt",
            "テスト",
            "--pane",
            "3",
            "--account",
            "univ",
        ]);
        match command {
            Command::Orchestrator(OrchestratorCommand::Run { account, .. }) => {
                assert_eq!(account.as_deref(), Some("univ"))
            }
            _ => panic!("想定外の Command（run 以外にパースされた）"),
        }
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
        // 絶対パスの表記はプラットフォーム依存（Windows はドライブレターが要る。
        // `/tmp/a.md` は Windows では相対パス扱いになり cwd のドライブで絶対化される）。
        // ここで見たいのは「絶対パスはそのまま渡る」ことなので実行環境の表記に合わせる
        let abs_md = if cfg!(windows) {
            r"C:\tmp\a.md"
        } else {
            "/tmp/a.md"
        };
        let command = parse(&["tako", "open", abs_md, "--pane", "5", "--mode", "md"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::OpenFile {
                pane: Some(5),
                path: abs_md.into(),
                mode: Some(tako_control::protocol::PreviewModeWire::Markdown),
                direction: None,
                focus: None,
                new_tab: false,
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
        let command = parse(&["tako", "open", abs_md, "--pane", "5", "--down"]);
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

    /// #813: 素の `tako limit-resume` で状態確認、on / off で切替、`--all` で一覧。
    /// `--pane` 省略時は呼び出し元（TAKO_PANE_ID）を埋める
    #[test]
    fn limit_resumeは状態取得と切替と一覧を操作へ写す() {
        let status = parse(&["tako", "limit-resume"]);
        assert_eq!(
            build_request(&status).unwrap(),
            Request::LimitResume {
                pane: caller_pane(),
                enabled: None,
                all: None
            }
        );
        let enable = parse(&["tako", "limit-resume", "on", "--pane", "12"]);
        assert_eq!(
            build_request(&enable).unwrap(),
            Request::LimitResume {
                pane: Some(12),
                enabled: Some(true),
                all: None
            }
        );
        let disable = parse(&["tako", "limit-resume", "off", "--pane", "12"]);
        assert_eq!(
            build_request(&disable).unwrap(),
            Request::LimitResume {
                pane: Some(12),
                enabled: Some(false),
                all: None
            }
        );
        // 一覧は呼び出し元ペインが分からなくても引ける（dispatch 側が pane を見ない）
        let all = parse(&["tako", "limit-resume", "--all"]);
        assert_eq!(
            build_request(&all).unwrap(),
            Request::LimitResume {
                pane: caller_pane(),
                enabled: None,
                all: Some(true)
            }
        );
        // on / off 以外は clap が弾く（誤った語で黙って状態取得にならない）
        assert!(Cli::try_parse_from(["tako", "limit-resume", "yes"]).is_err());
    }

    /// #600: 入力予測は素の `tako autosuggest` で状態確認、on / off で切替
    #[test]
    fn autosuggestは状態取得と切替を操作へ写す() {
        let status = parse(&["tako", "autosuggest"]);
        assert_eq!(
            build_request(&status).unwrap(),
            Request::Autosuggest {
                enabled: None,
                hint: None,
                tab: None
            }
        );
        let disable = parse(&["tako", "autosuggest", "off"]);
        assert_eq!(
            build_request(&disable).unwrap(),
            Request::Autosuggest {
                enabled: Some(false),
                hint: None,
                tab: None
            }
        );
        let enable = parse(&["tako", "autosuggest", "on"]);
        assert_eq!(
            build_request(&enable).unwrap(),
            Request::Autosuggest {
                enabled: Some(true),
                hint: None,
                tab: None
            }
        );
    }

    /// #614: `hint` / `tab` は本体を巻き込まずにその項目だけを触る。
    /// 引数なしなら現在状態の取得（`tako autosuggest hint` = 表示のみ）
    #[test]
    fn autosuggestのヒントとtab確定を個別に切り替えられる() {
        for (argv, expect) in [
            (
                vec!["tako", "autosuggest", "hint"],
                Request::Autosuggest {
                    enabled: None,
                    hint: None,
                    tab: None,
                },
            ),
            (
                vec!["tako", "autosuggest", "hint", "off"],
                Request::Autosuggest {
                    enabled: None,
                    hint: Some(false),
                    tab: None,
                },
            ),
            (
                vec!["tako", "autosuggest", "hint", "on"],
                Request::Autosuggest {
                    enabled: None,
                    hint: Some(true),
                    tab: None,
                },
            ),
            (
                vec!["tako", "autosuggest", "tab", "off"],
                Request::Autosuggest {
                    enabled: None,
                    hint: None,
                    tab: Some(false),
                },
            ),
        ] {
            let parsed = parse(&argv);
            assert_eq!(build_request(&parsed).unwrap(), expect, "{argv:?}");
        }
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

    /// Issue #553: GUI に見えている fleet をそのまま指定できる。
    /// 旧称 tmux も同じビューへ解決され、既存スクリプトが動き続ける
    #[test]
    fn panel_viewはfleetと旧称tmuxの両方を受理する() {
        use tako_control::protocol::PanelViewWire;

        let view_of = |value: &str| match build_request(&parse(&[
            "tako", "panel", "--show", "--view", value,
        ]))
        .unwrap()
        {
            Request::Panel { view, .. } => view,
            other => panic!("Panel 以外になった: {other:?}"),
        };
        assert_eq!(view_of("fleet"), Some(PanelViewWire::Fleet));
        assert_eq!(view_of("tmux"), Some(PanelViewWire::Fleet));
        assert_eq!(view_of("orch"), Some(PanelViewWire::Orch));
        assert_eq!(view_of("git"), Some(PanelViewWire::Git));
    }

    /// invalid value のエラーに GUI 表示名 fleet と旧称の両方が載る（#553 案 2）
    #[test]
    fn panel_viewの不正値エラーにfleetが載る() {
        let err = match Cli::try_parse_from(["tako", "panel", "--view", "fleets"]) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("不正値がエラーにならなかった"),
        };
        assert!(err.contains("fleet"), "エラーに fleet が無い: {err}");
        assert!(err.contains("tmux"), "エラーに旧称 tmux が無い: {err}");
    }

    // ---- Issue #567: stale な TAKO_PANE_ID からの master / solo 起動 ----

    fn list_with_panes(ids: &[u64]) -> Value {
        serde_json::json!({
            "tabs": [{
                "id": 1,
                "panes": ids.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>(),
            }],
        })
    }

    #[test]
    fn list_contains_paneは現存判定に使える() {
        let list = list_with_panes(&[780, 781]);
        assert!(list_contains_pane(&list, 780));
        assert!(
            !list_contains_pane(&list, 305),
            "旧世代の ID は現存しないと判定される（#567 の実事象）"
        );
        // 応答が壊れていても panic せず「居ない」に倒す
        assert!(!list_contains_pane(&serde_json::json!({}), 780));
    }

    #[test]
    fn launch_locationはフォールバック時にタブ表記になる() {
        let inline = LaunchTarget {
            pane: 780,
            new_tab: false,
        };
        assert_eq!(
            launch_location("master", &inline),
            "ペイン 780（インライン）"
        );
        let fallback = LaunchTarget {
            pane: 900,
            new_tab: true,
        };
        assert_eq!(
            launch_location("master-fable", &fallback),
            "タブ 'master-fable'（ペイン 900）"
        );
    }

    /// 復旧案内は最簡形で出す（既定プロファイルに余計な引数を付けない。#322）
    #[test]
    fn 復旧案内のコマンドは最簡形() {
        assert_eq!(master_cmd_hint("default"), "tako master");
        assert_eq!(master_cmd_hint("fable"), "tako master -fable");
        assert_eq!(solo_cmd_hint("default"), "tako solo");
        assert_eq!(solo_cmd_hint("fast"), "tako solo -fast");
    }

    /// フォールバック不能（GUI 不在）のエラーには復旧手順が載る（#567 受け入れ条件 2）
    #[test]
    fn 起動失敗のエラーに復旧手順が載る() {
        let with_stale = launch_failure_message(OUTSIDE_TAKO, "tako master -fable", Some(305));
        assert!(with_stale.contains("tako アプリを起動"), "{with_stale}");
        assert!(
            with_stale.contains("unset TAKO_PANE_ID"),
            "古い ID を持つシェルには unset を案内する: {with_stale}"
        );
        assert!(with_stale.contains("tako master -fable"), "{with_stale}");
        assert!(
            with_stale.contains(OUTSIDE_TAKO),
            "原因も残す: {with_stale}"
        );

        let without = launch_failure_message(OUTSIDE_TAKO, "tako master", None);
        assert!(
            !without.contains("unset TAKO_PANE_ID"),
            "そもそも設定されていないなら unset は案内しない: {without}"
        );
    }
}

/// T3 CLI 表: 全 CLI サブコマンドがマトリクスのキー（= MCP ツール）へ写像できること。
///
/// **狙い**: 「CLI にだけ機能を足して MCP に足さない」を検出する。
/// tako の開発不変条件「UI でできることはすべて AI からもできる」の機械的な担保でもある。
/// 新しい CLI コマンドを足すと、規則でも表でも解決できずここが落ちる。
#[cfg(test)]
mod platform_matrix_parity {
    use super::*;
    use clap::CommandFactory as _;
    use tako_core::platform::support::MATRIX;

    /// 規則（`tako_` + コマンドパス）で解けない対応を明示する表。
    /// 前方一致で最長のものが勝つ
    const CLI_KEY_OVERRIDES: &[(&str, &str)] = &[
        ("agents", "tako_agents_sync_rules"),
        ("autorename", "tako_auto_rename"),
        ("backgrounded", "tako_background_list"),
        ("background", "tako_background_pane"),
        ("close", "tako_close_pane"),
        ("collapse", "tako_collapse_tab"),
        // #513: CLI は `tako config <操作>`、MCP は action 引数を持つ 1 ツール
        ("config", "tako_config_share"),
        ("edit apply", "tako_preview_apply"),
        ("edit autosave", "tako_preview_autosave"),
        ("edit redo", "tako_preview_redo"),
        ("edit replace", "tako_preview_replace"),
        ("edit save", "tako_preview_save"),
        ("edit search", "tako_preview_search"),
        ("edit undo", "tako_preview_undo"),
        ("edit start", "tako_preview_edit"),
        ("edit status", "tako_preview_edit"),
        ("edit stop", "tako_preview_edit"),
        ("equalize", "tako_equalize_layout"),
        ("file", "tako_file_op"),
        ("focus", "tako_focus_pane"),
        ("foreground", "tako_foreground_pane"),
        // #496: CLI は git の語彙（branch / abort / resolve）、MCP は操作名で命名しているぶんのズレ
        ("git abort", "tako_git_merge_abort"),
        ("git branch", "tako_git_branch_create"),
        ("git resolve", "tako_git_resolve_agent"),
        ("list", "tako_list_panes"),
        ("open-in dir", "tako_open_dir"),
        ("open-in remote", "tako_open_remote"),
        // #919: CLI は `tako remote-folder <操作>`、MCP は action 引数を持つ 1 ツール
        ("remote-folder", "tako_remote_folder"),
        ("open-in repo", "tako_open_dir"),
        ("open", "tako_open_file"),
        ("orchestrator status", "tako_orchestrator_worker_status"),
        ("orchestrator watch", "tako_orchestrator_worker_status"),
        ("pin", "tako_pin_preview"),
        ("portdetect", "tako_port_detect"),
        ("preview", "tako_preview_view"),
        ("read", "tako_read_pane"),
        ("resize", "tako_resize_pane"),
        ("run-default", "tako_run_defaults"),
        ("scroll", "tako_scroll_pane"),
        ("send", "tako_send_input"),
        ("split", "tako_split_pane"),
        ("tab move-pane", "tako_move_pane_to_tab"),
        ("tab new", "tako_create_tab"),
        ("tab pin", "tako_pin_tab_title"),
        ("tab rename", "tako_rename_tab"),
        ("tab reorder", "tako_reorder_tab"),
        ("tab select", "tako_select_tab"),
        ("task update", "tako_task_checkpoint"),
        ("title", "tako_set_title"),
        ("tree", "tako_tree_folder"),
        ("video", "tako_video_playback"),
    ];

    /// MCP ツールを持たないことが意図的な CLI 専用コマンド。
    /// いずれも「GUI / MCP が使えない状況のための入口」なので MCP からは提供できない
    /// （`master` / `solo` はエージェント CLI の起動そのもの、`mcp serve` は MCP ブリッジ自身、
    /// `recover` は GUI 死亡時の復旧、`remote serve` はデーモン本体）
    const CLI_ONLY: &[&str] = &["master", "solo", "mcp serve", "recover", "remote serve"];

    fn leaf_commands() -> Vec<String> {
        fn walk(c: &clap::Command, prefix: &str, out: &mut Vec<String>) {
            for sub in c.get_subcommands() {
                let name = if prefix.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{prefix} {}", sub.get_name())
                };
                if sub.get_subcommands().next().is_some() {
                    walk(sub, &name, out);
                } else {
                    out.push(name);
                }
            }
        }
        let mut out = Vec::new();
        walk(&Cli::command(), "", &mut out);
        out
    }

    /// CLI コマンドパス → マトリクスキー。`Ok(None)` は意図的な CLI 専用
    fn resolve(path: &str) -> Result<Option<&'static str>, ()> {
        let matches = |pref: &str| path == pref || path.starts_with(&format!("{pref} "));
        if CLI_ONLY.iter().any(|p| matches(p)) {
            return Ok(None);
        }
        let best = CLI_KEY_OVERRIDES
            .iter()
            .filter(|(pref, _)| matches(pref))
            .max_by_key(|(pref, _)| pref.len());
        if let Some((_, key)) = best {
            return Ok(Some(key));
        }
        // 規則: `tako_` + コマンドパス（後ろの語から順に落として探す）
        let parts: Vec<&str> = path.split(' ').collect();
        for i in (1..=parts.len()).rev() {
            let key = format!("tako_{}", parts[..i].join("_").replace('-', "_"));
            if let Some(f) = MATRIX.iter().find(|f| f.key == key) {
                return Ok(Some(f.key));
            }
        }
        Err(())
    }

    #[test]
    fn t3_全cliコマンドがマトリクスのキーへ写像できる() {
        let mut unresolved = Vec::new();
        for cmd in leaf_commands() {
            match resolve(&cmd) {
                Ok(Some(key)) => assert!(
                    MATRIX.iter().any(|f| f.key == key),
                    "{cmd} の写像先 {key} が MATRIX に無い"
                ),
                Ok(None) => {}
                Err(()) => unresolved.push(cmd),
            }
        }
        assert!(
            unresolved.is_empty(),
            "対応する MCP ツールを解決できない CLI コマンドがある: {unresolved:?}\n\
             → MCP ツールを追加する（開発不変条件）か、対応表 CLI_KEY_OVERRIDES に写像を書くか、\n\
             または意図的に CLI 専用なら CLI_ONLY に理由つきで登録してください"
        );
    }

    /// #548: accounts の 4 コマンドが MCP の 1 ツールへ写ること。
    /// 規則（後ろの語を落として探す）で解けるので CLI_KEY_OVERRIDES への登録は要らないが、
    /// MATRIX 側のキー名が変わったら気づけるように明示しておく
    #[test]
    fn t3_accountsコマンドはmcpツールへ写る() {
        for cmd in [
            "orchestrator accounts list",
            "orchestrator accounts show",
            "orchestrator accounts add",
            "orchestrator accounts remove",
        ] {
            assert_eq!(
                resolve(cmd),
                Ok(Some("tako_orchestrator_accounts")),
                "{cmd} の写像先"
            );
        }
    }

    /// 表そのものが腐らないようにする（消えたコマンド・キーを残さない）
    #[test]
    fn t3_対応表に死んだエントリが無い() {
        let cmds = leaf_commands();
        let used = |pref: &str| {
            cmds.iter()
                .any(|c| c == pref || c.starts_with(&format!("{pref} ")))
        };
        for (pref, key) in CLI_KEY_OVERRIDES {
            assert!(
                used(pref),
                "CLI_KEY_OVERRIDES の {pref} に該当するコマンドが無い"
            );
            assert!(
                MATRIX.iter().any(|f| f.key == *key),
                "CLI_KEY_OVERRIDES の写像先 {key} が MATRIX に無い"
            );
        }
        for pref in CLI_ONLY {
            assert!(used(pref), "CLI_ONLY の {pref} に該当するコマンドが無い");
        }
    }
}
