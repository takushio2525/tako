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

// 出力マクロ（`println!` 等）を std の版から差し替える。**他の `mod` より先に置く**
// （マクロは宣言より後ろにしか効かない。理由は `stdio.rs` の先頭 = #1758）
#[macro_use]
mod stdio;

mod setup;

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::Value;
// IPC クライアントと MCP の stdio ブリッジは `tako-mcp`（#2021）と共有する 1 実装
use tako_cli::ipc::{
    resolve_caller_pane_via_app, send_request, send_request_classified, send_request_machine_wide,
    transport, SendFailure, TransportError,
};
use tako_cli::mcp_bridge;
use tako_control::claude_mod::text::ModText;
use tako_control::orchestrator::wait;
use tako_control::protocol::{Axis, Direction, Request};
use tako_control::{HealthText, Reason, ReplyNote};

// 呼び出し元ペイン（`TAKO_PANE_ID`。FR-2.1.1）。テストのビルドだけは env を読まない差し替えへ
// 向ける（#1716。手元のシェルが継承した env でテストが通り CI で落ちる「偽の緑」を作らない）
#[cfg(not(test))]
use tako_cli::ipc::caller_pane;
#[cfg(test)]
use test_caller::caller_pane;

/// ヘルプへ埋める「修飾 + クリック」の表記（実行プラットフォーム。Issue #763）。
///
/// macOS は `⌘+クリック` / Windows は `Ctrl+クリック`。**直書きしない**:
/// Windows のユーザーへ `cmd+クリック` と案内すると、Win キーは OS が奪うので
/// 書いてあるとおりに押しても開かない（#1203 と同じ事故）。表記の正本は
/// `tako_core::platform::keys::link_modifier` で、GUI の判定と同じ表を見る
fn link_click() -> String {
    tako_core::platform::keys::link_click(tako_core::platform::support::Platform::current())
}

/// ジャンプ履歴の戻る / 進むの打鍵（#1677。実行中の OS の表記）
fn jump_keys() -> (&'static str, &'static str) {
    let platform = tako_core::platform::support::Platform::current();
    (
        tako_core::platform::keys::jump_back(platform),
        tako_core::platform::keys::jump_forward(platform),
    )
}

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
    /// 環境の健全性を診断する（MCP `tako_check_health` と 1:1）。
    /// アプリへ届かないときは IPC の受け口だけをローカルで診断する（Issue #1441）
    #[command(name = "check-health")]
    CheckHealth(CheckHealthArgs),
    /// PC のメモリ・CPU と重い処理（ビルド・テスト等）の状況を出す（MCP `tako_queue` と 1:1。Issue #1993）。
    /// コマンドを付けると重い処理を順番待ちで走らせる（`tako queue cargo build`。Issue #1994）。
    /// アプリへ届かないときは状況は機の状態だけをこのプロセスで採り、コマンドは待たずに走らせる
    Queue(QueueArgs),
    /// ペインの画面内容をテキストで出力する
    Read(ReadArgs),
    /// ターミナル画面のリンク（修飾 + クリックで開けるもの）を列挙する（Issue #1283）
    #[command(about = format!("ターミナル画面のリンク（{}で開けるもの）を列挙する（Issue #1283）", link_click()))]
    Links(LinksArgs),
    /// ワークスペース横断検索（Issue #1694。GUI の右パネルの search ビューと同じ）
    #[command(about = format!("ワークスペース横断検索（Issue #1694。GUI の {} と同じ）", find_in_files_key()))]
    Search(SearchArgs),
    /// 言語サーバ（LSP）の状態と起動・停止（Issue #1678。編集モードで自動的に起きる）
    #[command(subcommand)]
    Lsp(LspCommand),
    /// スクロールバック表示を動かす（--to 0 で最下部へ）
    Scroll(ScrollArgs),
    /// 直接ペインのスクロールバック保持上限（行）の確認・変更（Issue #818）
    Scrollback(ScrollbackArgs),
    /// フォーカスの無いペインの出力による再描画の上限（fps）の確認・変更（Issue #1979）
    RedrawLimit(RedrawLimitArgs),
    /// マウス報告を要求する TUI（全画面の claude 等）へのクリックの転送の確認・切替（Issue #1961）
    Mouse(MouseArgs),
    /// ペインの表示セルを左クリックする（内側アプリがマウス報告を要求しているときだけ。Issue #1961）
    Click(ClickArgs),
    /// 貼り付けに API キー・パスワードらしきものがあれば送る前に確かめる機能の確認・切替・判定（Issue #2046）
    PasteGuard(PasteGuardArgs),
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
    /// ジャンプ履歴（行を指定して開いた位置）を戻る・進む・一覧する（Issue #1677）
    #[command(subcommand)]
    Jump(JumpCommand),
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
    /// 有効時、tako を再起動してもタブ構成と実行中プロセスが復元される。
    /// `reattach --pane N` は tmux のセッションへ再接続できなくなったペインを attach し直す
    Persist(PersistArgs),
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
    /// agent 能力マトリクスを表示する（どの CLI でどこまで使えるか。Issue #982）
    AgentSupport(AgentSupportArgs),
    /// プラットフォーム対応マトリクスの参照（Issue #515）。
    /// この環境でどの機能が使えるか・縮退しているか・未実装かを表示する
    Platform(PlatformArgs),
    /// 起動時ロードの予算の確認と自動修正（Issue #1139）。
    /// AI が起動した瞬間に強制ロードされるもの（グローバル指示・AGENTS.md と
    /// @import チェーン・system prompt・引き継ぎ）を棚卸しし、上限と突き合わせる。
    /// `fix` で作業ログの archive 移送だけを自動で直す
    #[command(name = "context-budget")]
    ContextBudget(ContextBudgetArgs),
    /// テスト・検証プロセスが一時ディレクトリへ残した使い捨て dir を掃除する（Issue #1296 / #1312）。
    /// 対象は `<TMPDIR>` 直下の `tako-test-data-<pid>`（cargo test の data dir）/
    /// `tako-agent-config-<pid>`（検証プロセスのエージェント設定）/
    /// `tako-test-scratch-<pid>`（テスト本体の使い捨て作業ディレクトリ）/
    /// `tako-test-orchestrator-<pid>` / `tako-test-supervisor-<pid>`（オーケストレーターの
    /// テスト隔離先）で、**所有プロセスが生きていないものだけ**。
    /// **既定は dry-run**（1 つも消さない）
    #[command(name = "test-residue")]
    TestResidue(TestResidueArgs),
    /// シェル統合（OSC 7 / 133 = ペインの cwd 追従とコマンド実行状態）の
    /// 配置状態の確認と配置・解除（Issue #525）。引数なしで現在の状態を表示。
    /// unix は環境変数の注入だけで完結するので配置操作は不要
    ShellIntegration(ShellIntegrationArgs),
    /// tako mod（Claude Code の mod 連携。FR-2.42 / Issue #1879）。引数なしで状態
    /// （claude の版・注入の有無・ペインごとの報告 = ctx / 使用制限 / turn）を表示。
    /// on / off は次に作るペインから効く
    Mod(ModArgs),
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
    /// 右サイドバー情報パネルの表示・幅・ビュー切替と、左サイドバーのファイルツリーの表示操作。
    /// 引数なしで現在状態を表示する
    Panel(PanelArgs),
    /// サイドバー tmux ビューのタブ枠を折りたたむ / 展開する
    /// （配下のバックグラウンド行 + バックグラウンドを隠し、前面表示中の行は残す）
    Collapse(CollapseArgs),
    /// プレビューをピン留め / 解除する（バックグラウンドペイン / 閉じたタブグループの
    /// 実画面をアプリ内フローティングウィンドウとして常駐・ライブ更新させる）
    Pin(PinArgs),
    /// ペインまたはタブをバックグラウンドへ送る（プロセスは生きたまま画面から外す。
    /// --tab はタブ 1 単位で退避し、分割ツリー・比率・並び位置を保つ）
    #[command(name = "background")]
    Background(BackgroundArgs),
    /// バックグラウンドのペイン / 退避タブを画面に復帰させる（--tab はタブ単位）
    #[command(name = "foreground")]
    Foreground(ForegroundArgs),
    /// バックグラウンドのペイン / 退避タブ一覧を JSON で出力する
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
    /// エージェント CLI に tako MCP サーバーの接続設定を追加する（アプリ未起動でも実行できる）
    ///
    /// 引数なしで claude + この環境に導入済みの codex / agy へまとめて登録する。
    /// claude の書き先は ~/.claude.json（--project のときは <cwd>/.mcp.json）、
    /// codex / agy はそれぞれの CLI の mcp add に書かせる。
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
    /// エージェントペインを会話ごと引き継いで再起動する（Issue #1067）。
    ///
    /// 引数なしで**下見**（このペインで何ができるか + できない理由）だけを返す。
    /// `--mode harness` は CLI プロセスだけを建て直して `--resume` で同じ会話を続ける
    /// （claude の自動更新に追いつく手段）。`--mode handoff` は引き継ぎを書かせてから
    /// 新しいセッションへ交代する（master ペインのみ）
    #[command(name = "session-restart")]
    SessionRestart(SessionRestartArgs),
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
    /// 利用制限（5h / 7d）の観測と使用量の内訳（usage 分析 #2192 の S1 = #2200 / S2 = #2201）。
    /// ログインごとの最新の % と、期間の中のトークン・API 換算・会話数をプロジェクト・Issue・会話などの段で
    /// 出す。GUI が動いていなくても読める（会話記録の索引を追いつかせてから答える）
    Usage(UsageArgs),
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
    /// ユーザー向けタスク（人がやること）の操作（Issue #1450）。
    /// 承認待ち・レビュー・権限の確認・宣伝投稿など、AI ではなくユーザーの手が要るものを
    /// 一覧で溜めて片付ける。AI 自身のタスクは `tako task`（別物）
    #[command(subcommand)]
    Todo(TodoCommand),
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
    /// ファイル / プロジェクトの実行設定（実行環境・プロファイル・引数・環境変数・作業ディレクトリ・
    /// 実行前コマンド・実行先）を見る・選ぶ（FR-3.18.5 / FR-3.18.6 / FR-3.18.9, #1731 / #1732 / #1734）。
    /// 項目を渡さなければ実効値と実行環境の候補を出す
    #[command(name = "run-config")]
    RunConfig(RunConfigArgs),
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
    /// 同じファイルの実行ペインを使い回さず、新しく分割する（#1657。保存した実行先
    /// `tako run-config --target` より勝つ = #1734。既定は同じファイル・プロファイルの
    /// 実行ペインをその位置で差し替える）
    #[arg(long)]
    new_pane: bool,
    /// 実行せずプロファイル一覧と実行環境の候補を表示する（--dry-run / --list）
    #[arg(long, alias = "dry-run")]
    list: bool,
    /// --list で、実行環境の問い合わせ（道具の場所・版）を覚えた答えを捨てて聞き直す
    #[arg(long, requires = "list")]
    refresh: bool,
    /// --list で、.ino の基板を名前か FQBN で探す（語を省くと入っているコアの基板を全部。#1952）
    #[arg(long, requires = "list", value_name = "語", num_args = 0..=1, default_missing_value = "")]
    boards: Option<String>,
}

#[derive(Args)]
struct RunConfigArgs {
    /// 対象ファイル
    #[arg(value_name = "FILE")]
    path: String,
    /// 実行環境を選ぶ（候補の ID・短縮（.venv / uv / 版名）・パス。既定でプロジェクトに保存）
    #[arg(long)]
    runtime: Option<String>,
    /// プロファイルを選ぶ（既定でこのファイルに保存）
    #[arg(long)]
    profile: Option<String>,
    /// 引数（シェルへそのまま渡る 1 本の文字列。`${args}` が無ければコマンドの末尾へ足す。既定でこのファイルに保存）
    #[arg(long, allow_hyphen_values = true)]
    args: Option<String>,
    /// 環境変数を設定する（K=V。繰り返せる。既定でこのファイルに保存）
    #[arg(long = "env", value_name = "K=V")]
    env: Vec<String>,
    /// 環境変数を消す（繰り返せる。--project / --file を省くと両方から）
    #[arg(long = "unset-env", value_name = "K")]
    unset_env: Vec<String>,
    /// 作業ディレクトリ（相対はファイルのディレクトリ基準。既定でこのファイルに保存）
    #[arg(long)]
    cwd: Option<String>,
    /// 実行前コマンド（失敗したら本体を走らせない。既定でこのファイルに保存）
    #[arg(long, allow_hyphen_values = true)]
    before: Option<String>,
    /// .ino の基板（FQBN。例 arduino:avr:uno）を選ぶ。`arduino-cli board attach` で sketch.yaml へ書く（#1952）
    #[arg(long, value_name = "FQBN")]
    board: Option<String>,
    /// .ino の書き込み先のポートを選ぶ（sketch.yaml の default_port。#1952）
    #[arg(long, allow_hyphen_values = true)]
    port: Option<String>,
    /// 実行先（reuse = 同じペインで再実行 / new = 毎回新しいペイン。既定でこのファイルに保存。
    /// `tako run --new-pane` が保存値より勝つ。#1734）
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(
        tako_core::runner_config::RunTarget::VALUES
    ))]
    target: Option<String>,
    /// 自動・既定へ戻す（項目名か all。値を省くと全項目。--project / --file を省くと両方から消す）
    #[arg(long, num_args = 0.., value_name = "項目", value_parser = clap::builder::PossibleValuesParser::new(
        tako_core::runner_config::RunConfigField::reset_values()
    ))]
    reset: Option<Vec<String>>,
    /// プロジェクトに保存する
    #[arg(long, conflicts_with = "scope_file")]
    project: bool,
    /// このファイルだけに保存する
    #[arg(long = "file", id = "scope_file")]
    scope_file: bool,
    /// 相対パスの基準ペイン（省略時は呼び出し元。CLI は自分の cwd で絶対パスにしてから送る）
    #[arg(long)]
    pane: Option<u64>,
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
    /// このバックアップ世代（1〜3、good = 最後に復元へ成功した良品、または
    /// rescued = 復元の直後に全ペインが終わったので退避した構成。#641）を
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
    /// SSH ホストに接続する（既定 = いま開いているタブへ新ペイン。#1006）
    Remote {
        /// ~/.ssh/config の Host 名（未定義でも ssh コマンドとして実行）
        host: String,
        /// フォーカスを接続したペインに移さない
        #[arg(long)]
        no_focus: bool,
        /// 接続後に cd するリモートのパス（#919）
        #[arg(long)]
        remote_dir: Option<String>,
        /// 開き先（#1006。split = いまのタブへ新ペイン（既定）/ tab = 新しいタブ /
        /// pane = 既存ペインをそのまま SSH 化）
        #[arg(long, value_parser = ["split", "tab", "pane"])]
        target: Option<String>,
        /// 対象ペイン ID（--target pane は SSH 化するペイン / split は分割元）
        #[arg(long)]
        pane: Option<u64>,
        /// 対象タブ ID（--target split のとき。省略時はアクティブタブ）
        #[arg(long)]
        tab: Option<u64>,
        /// 分割方向（--target split のとき。省略時 right）
        #[arg(long, value_parser = ["right", "down", "left", "up"])]
        direction: Option<String>,
    },
}

/// リモートフォルダの操作（#919 / #65）。MCP `tako_remote_folder` と 1:1
#[derive(Subcommand)]
enum RemoteFolderCommand {
    /// SSH 先のフォルダをファイルツリーに開く（接続に失敗したら開かず理由を返す）。
    /// 既定でそのホストへ SSH 済み + そのフォルダへ cd 済みのターミナルも用意する（#1041）
    Open {
        /// ~/.ssh/config の Host 名
        host: String,
        /// リモート側の絶対パス（省略時はリモートのホーム）
        path: Option<String>,
        /// 対象タブ ID（省略時はアクティブタブ）
        #[arg(long)]
        tab: Option<u64>,
        /// ターミナルは繋がず、ファイルツリーへ開くだけにする（#1041）
        #[arg(long)]
        no_terminal: bool,
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
    /// ペインの ssh 検知による自動追加の状態・切替（#976）
    Auto {
        /// on / off（省略で現在値と検知状態の照会だけ）
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
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

/// セッション再起動の引数（Issue #1067）。
///
/// 素のコマンドが最短で済む形（#322）: `tako session-restart` = 下見、
/// `--mode` を付けたときだけ実行する
#[derive(Args)]
struct SessionRestartArgs {
    /// 種別（省略時は下見だけを返す）
    #[arg(long, value_parser = ["harness", "handoff"])]
    mode: Option<String>,
    /// 対象ペイン ID（**省略時は呼び出し元** = TAKO_PANE_ID。
    /// 他のペインを建て直すときは必ず指定する）
    #[arg(long)]
    pane: Option<u64>,
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
        /// 蓋閉じ継続の電源条件: ac-only（既定）/ always（バッテリーでも継続。#1473）
        #[arg(long)]
        lid_power_condition: Option<String>,
        /// 蓋閉じ継続をバッテリーで続けるときの残量下限（%。既定 20。#1473）
        #[arg(long)]
        lid_battery_floor: Option<i64>,
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

/// `tako jump`（#1677）。MCP は `action` 引数を持つ 1 ツール `tako_jump`
#[derive(Subcommand)]
enum JumpCommand {
    /// 1 つ戻る（行を指定して開く前にいた場所へ）
    #[command(about = format!("1 つ戻る（行を指定して開く前にいた場所へ。GUI は {}）", jump_keys().0))]
    Back(JumpMoveArgs),
    /// 1 つ進む
    #[command(about = format!("1 つ進む（GUI は {}）", jump_keys().1))]
    Forward(JumpMoveArgs),
    /// 履歴の全項目と現在位置（行を指定して開くと積まれる）
    List {
        /// JSON のまま出す
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct JumpMoveArgs {
    /// 閉じたペインの項目を開き直すときの基準ペイン（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 着地したペインへフォーカスを移す（省略時は元ペインを維持）
    #[arg(long)]
    focus: bool,
    /// JSON のまま出す（MCP `tako_jump` と同じ応答）
    #[arg(long)]
    json: bool,
}

impl JumpCommand {
    /// dispatch の action（綴りの正本は `dispatch::JUMP_ACTIONS`）
    fn action(&self) -> &'static str {
        match self {
            Self::Back(_) => "back",
            Self::Forward(_) => "forward",
            Self::List { .. } => "list",
        }
    }
}

/// `tako lsp`（#1678）。MCP は `action` 引数を持つ 1 ツール `tako_lsp_server`
#[derive(Subcommand)]
enum LspCommand {
    /// 状態・能力・診断件数（未導入なら理由と導入コマンド）
    Status {
        /// サーバの ID（省略で全部）
        #[arg(long)]
        name: Option<String>,
        /// JSON のまま出す
        #[arg(long)]
        json: bool,
    },
    /// 検出表と、導入済みかどうか（解決は 2 回目からキャッシュ。引き直しは restart）
    Servers {
        #[arg(long)]
        json: bool,
    },
    /// 止めて起こし直す（未導入・諦めた・止めたも対象）
    Restart {
        #[arg(long)]
        name: Option<String>,
    },
    /// 止める（restart まで自動では起こさない）
    Stop {
        #[arg(long)]
        name: Option<String>,
    },
    /// サーバが stderr へ出した直近の行
    Logs {
        #[arg(long)]
        name: Option<String>,
    },
    /// 言語サーバの初期化オプション（#2035）。既定・上書き・実効値と、動いているサーバへ送った値を出す。
    /// rust-analyzer の既定は cargo.targetDir = true（初回の check を target/rust-analyzer で走らせ、
    /// 自分の cargo build と target のロックを取り合わない）。`--set KEY=VALUE` で上書きを
    /// settings.json へ書く（値は JSON か文字列・null で tako の既定へ戻す。`--name` が要る）。
    /// 次に起こしたときから効く。tako が起動していなくても読み書きできる。MCP は `tako_lsp_server` の action=options
    Options {
        /// サーバの ID（省略で既定か上書きを持つもの全部）
        #[arg(long)]
        name: Option<String>,
        /// 上書き（`cargo.targetDir=false` の形。繰り返せる）
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// 未導入の言語サーバを tako の data dir へ取る（グローバルには入れない。#1944）。
    /// 省略で取れるもの全部。グローバルに入っていれば取らない。tako が起動していなくても取れる
    Install {
        /// サーバの ID（省略で取れるもの全部）
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 編集中のコードの診断（エラー・警告）。位置は `tako edit replace-range` と同じ
    /// （行 1 始まり・桁 0 始まりの UTF-8 バイト）。MCP は `tako_lsp` の action=diagnostics（#1679）
    Diagnostics {
        /// プレビューペイン ID（省略で言語サーバにつながった文書すべて）
        #[arg(long)]
        pane: Option<u64>,
        /// この重大度以上に絞る（warning = エラーと警告）
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(
            tako_core::lsp::diagnostic::Severity::NAMES
        ))]
        severity: Option<String>,
        /// JSON のまま出す
        #[arg(long)]
        json: bool,
    },
    /// 識別子の定義へ飛ぶ（GUI の修飾クリック = macOS は ⌘・Windows は Ctrl と同じ。#1680）。
    /// 同じファイルは同じペイン、同じタブで開いているファイルはそのペイン、それ以外は新しいペイン。
    /// MCP は `tako_lsp` の action=definition
    Definition(LspGotoArgs),
    /// 宣言へ飛ぶ（引数は definition と同じ）
    Declaration(LspGotoArgs),
    /// 型の定義へ飛ぶ（引数は definition と同じ）
    TypeDefinition(LspGotoArgs),
    /// 実装へ飛ぶ（引数は definition と同じ）
    Implementation(LspGotoArgs),
    /// 編集中のコードを整形する（GUI の編集メニューの「コードを整形」と同じ。#1683）。
    /// 編集バッファへ当てて undo 1 回で戻せる（保存はしない）。`--range` で範囲の整形
    /// （範囲の外は変えない）。MCP は `tako_lsp` の action=format
    Format {
        /// コードプレビューのペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
        /// 範囲（`行:桁-行:桁`。行は 1 始まり・桁は 0 始まりの UTF-8 バイト =
        /// `tako edit replace-range` と同じ。桁は省略可 = 0）
        #[arg(long)]
        range: Option<String>,
        /// JSON のまま出す
        #[arg(long)]
        json: bool,
    },
    /// 保存時整形の設定（既定 off）。on でも明示的な保存だけで整形し、自動保存では整形しない。
    /// 省略で今の値。MCP は `tako_lsp` の action=format-on-save
    FormatOnSave {
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
    },
    /// その位置の補完候補（GUI の打鍵中の一覧と同じ絞り込みと並び。#1682）。`--choice N` で
    /// N 番目の候補で確定する（undo 1 回で戻る）。MCP は `tako_lsp` の action=completion
    Completion(LspCompletionArgs),
    /// その位置の右クリックメニューの LSP 項目（GUI のコードプレビューの本文を右クリックしたときと
    /// 同じ出し分け = サーバの能力に無いものは出ない・識別子でない位置は 0 件。#1684）。
    /// 項目ごとに押したときと同じコマンドを出す。MCP は `tako_lsp` の action=menu
    Menu(LspMenuArgs),
    /// その位置の型・doc（GUI のマウスのホバーのカードと同じ問い合わせ。#1681）。本文は
    /// Markdown / 平文のまま出す（既定は 16,000 字まで。`--full` で全文・`--limit N` で N 字まで）。
    /// `--show` で GUI のその位置にカードも出す。MCP は `tako_lsp` の action=hover（`limit`。0 = 全文）
    Hover(LspHoverArgs),
    /// その位置の識別子の参照の一覧（GUI の右クリックの「参照を検索」と同じ。#1685）。
    /// 結果はそのペインの結果として保持し（`tako lsp results` で読み直す）、行の `id` を
    /// `tako lsp open-result --item N` へ渡すとその位置を開く。MCP は `tako_lsp` の action=references
    References(LspXrefArgs),
    /// その位置の関数の呼び出し元のツリー（根を 1 段開いた形。引数は references と同じ）
    IncomingCalls(LspXrefArgs),
    /// その位置の関数の呼び出し先のツリー（引数は references と同じ）
    OutgoingCalls(LspXrefArgs),
    /// その位置の型のスーパータイプのツリー（引数は references と同じ）
    Supertypes(LspXrefArgs),
    /// その位置の型のサブタイプのツリー（引数は references と同じ）
    Subtypes(LspXrefArgs),
    /// 保持している参照・階層の結果を読む（右パネルの references ビューと同じ中身。`retained` =
    /// 保持件数）。`--pane` は結果を問い合わせたペイン（省略で右パネルの結果）
    Results(LspResultsArgs),
    /// ツリーのノードを開く（子を読んでいなければ言語サーバへ問い合わせる）
    Expand(LspResultItemArgs),
    /// ツリーのノードを閉じる
    Collapse(LspResultItemArgs),
    /// 一覧の行 / ツリーのノードの位置を開く（右パネルの行のクリックと同じ。着地は定義ジャンプと同じ）
    OpenResult(LspOpenResultArgs),
    /// 結果を消す（右パネルの見出しの × と同じ。`--pane` 省略で右パネルの結果）
    ClearResults {
        /// 結果を問い合わせたペイン ID（省略で右パネルの結果）
        #[arg(long)]
        pane: Option<u64>,
    },
    /// その位置の識別子の名前を変える（GUI の右クリックの「名前の変更…」と同じ。#1686）。
    /// 既定は書き換えずに「N ファイル・M 箇所」と場所を出す。`--yes` で頼んだペインの編集バッファへ
    /// 当てる（undo 1 回で戻る・保存はしない）。別のファイルに及ぶときは当てない（段階 1）。
    /// MCP は `tako_lsp` の action=rename（`confirm` = `--yes`）
    Rename(LspRenameArgs),
}

/// `tako lsp rename` の引数（#1686）。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの行内 UTF-8 バイト）
#[derive(clap::Args)]
struct LspRenameArgs {
    /// コードプレビューのペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 行（1 始まり）
    #[arg(long)]
    line: usize,
    /// 桁（0 始まりの行内 UTF-8 バイト。文字の途中は拒否）
    #[arg(long)]
    column: usize,
    /// 新しい名前
    #[arg(long)]
    new_name: String,
    /// 書き換える（省略時は何も書き換えずに件数と場所だけ出す）
    #[arg(long)]
    yes: bool,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp references` 等の引数（#1685）。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの行内 UTF-8 バイト。応答の位置もこの形）
#[derive(clap::Args)]
struct LspXrefArgs {
    /// コードプレビューのペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 行（1 始まり）
    #[arg(long)]
    line: usize,
    /// 桁（0 始まりの行内 UTF-8 バイト。文字の途中は拒否）
    #[arg(long)]
    column: usize,
    /// 右パネルの references ビューにも出す
    #[arg(long)]
    show: bool,
    /// 出す行の数（既定 100。0 は保持しているもの全部）
    #[arg(long)]
    limit: Option<usize>,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp results` の引数（#1685）
#[derive(clap::Args)]
struct LspResultsArgs {
    /// 結果を問い合わせたペイン ID（省略で右パネルの結果）
    #[arg(long)]
    pane: Option<u64>,
    /// 出す行の数（既定 100。0 は全部）
    #[arg(long)]
    limit: Option<usize>,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp expand` / `collapse` の引数（#1685）
#[derive(clap::Args)]
struct LspResultItemArgs {
    /// ノードの番号（応答の id）
    #[arg(long)]
    item: usize,
    /// 結果を問い合わせたペイン ID（省略で右パネルの結果）
    #[arg(long)]
    pane: Option<u64>,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp open-result` の引数（#1685）
#[derive(clap::Args)]
struct LspOpenResultArgs {
    /// 一覧の行 / ツリーのノードの番号（応答の id）
    #[arg(long)]
    item: usize,
    /// 結果を問い合わせたペイン ID（省略で右パネルの結果）
    #[arg(long)]
    pane: Option<u64>,
    /// 別のファイルを開く新しいペインの置き場所（省略で right。none は開かずに場所だけ返す）
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(
        tako_core::lsp::goto::Placement::NAMES
    ))]
    open: Option<String>,
    /// 着地したペインへフォーカスを移す
    #[arg(long)]
    focus: bool,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp menu` の引数（#1684）。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの行内 UTF-8 バイト）
#[derive(clap::Args)]
struct LspMenuArgs {
    /// コードプレビューのペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 行（1 始まり）
    #[arg(long)]
    line: usize,
    /// 桁（0 始まりの行内 UTF-8 バイト。文字の途中は拒否）
    #[arg(long)]
    column: usize,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp format --range` の `行:桁-行:桁` を解く（#1683。行:桁は [`parse_position`]）
fn parse_line_col_range(text: &str) -> Result<tako_control::protocol::LineColRange, String> {
    let (start, end) = text
        .split_once('-')
        .ok_or_else(|| front_err(tako_control::Reason::RangeShape(text.to_string())))?;
    let (start_line, start_col) = parse_position(start)?;
    let (end_line, end_col) = parse_position(end)?;
    Ok(tako_control::protocol::LineColRange {
        start_line,
        start_col,
        end_line,
        end_col,
    })
}

/// `tako lsp completion` の引数（#1682）。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの行内 UTF-8 バイト。応答の範囲もこの形）
#[derive(clap::Args)]
struct LspCompletionArgs {
    /// コードプレビューのペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 行（1 始まり）
    #[arg(long)]
    line: usize,
    /// 桁（0 始まりの行内 UTF-8 バイト。文字の途中は拒否）
    #[arg(long)]
    column: usize,
    /// 返す件数（既定 50。切った件数は応答の truncated）
    #[arg(long)]
    limit: Option<usize>,
    /// N 番目（1 始まり。件数の上限に関わらず全体の並びの番号）の候補で確定する
    #[arg(long)]
    choice: Option<usize>,
    /// 返す候補の説明を言語サーバに補ってもらう（completionItem/resolve）
    #[arg(long)]
    resolve: bool,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

/// `tako lsp hover` の引数（#1681）。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの行内 UTF-8 バイト。応答の範囲もこの形）
#[derive(clap::Args)]
struct LspHoverArgs {
    /// コードプレビューのペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 行（1 始まり）
    #[arg(long)]
    line: usize,
    /// 桁（0 始まりの行内 UTF-8 バイト。文字の途中は拒否）
    #[arg(long)]
    column: usize,
    /// GUI のその位置にカードも出す
    #[arg(long)]
    show: bool,
    /// 本文を切らずに全文を出す（既定は 16,000 字まで。#1893）
    #[arg(long, conflicts_with = "limit")]
    full: bool,
    /// 本文の字数の上限（既定 16,000。0 は全文 = --full。#1893）
    #[arg(long, value_parser = clap::value_parser!(usize))]
    limit: Option<usize>,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

impl LspHoverArgs {
    /// `limit` の wire 形（`--full` = 0 = 全文。どちらも無ければ省略 = 16,000 字。#1893）
    fn wire_limit(&self) -> Option<usize> {
        if self.full {
            Some(0)
        } else {
            self.limit
        }
    }
}

/// `tako lsp definition` 等の引数（#1680）。位置は `tako edit replace-range` と同じ
/// （行 1 始まり・桁 0 始まりの行内 UTF-8 バイト。`tako lsp` の応答の位置もこの形）
#[derive(clap::Args)]
struct LspGotoArgs {
    /// コードプレビューのペイン ID（省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 行（1 始まり）
    #[arg(long)]
    line: usize,
    /// 桁（0 始まりの行内 UTF-8 バイト。文字の途中は拒否）
    #[arg(long)]
    column: usize,
    /// 別のファイルを開く新しいペインの置き場所（省略で right。none は開かずに場所だけ返す）
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(
        tako_core::lsp::goto::Placement::NAMES
    ))]
    open: Option<String>,
    /// 候補が複数のときに選ぶ番号（1 始まり）
    #[arg(long)]
    choice: Option<usize>,
    /// 着地したペインへフォーカスを移す
    #[arg(long)]
    focus: bool,
    /// JSON のまま出す
    #[arg(long)]
    json: bool,
}

impl LspCommand {
    /// dispatch の要求（action の綴りの正本は `dispatch::LSP_ACTIONS` /
    /// 言語機能は `dispatch::LSP_FEATURE_ACTIONS`）
    fn request(&self) -> Result<Request, String> {
        let server = |action: &str, name: &Option<String>| Request::LspServer {
            action: action.to_string(),
            name: name.clone(),
            set: None,
        };
        Ok(match self {
            Self::Status { name, .. } => server("status", name),
            Self::Servers { .. } => server("list", &None),
            Self::Restart { name } => server("restart", name),
            Self::Stop { name } => server("stop", name),
            Self::Logs { name } => server("logs", name),
            Self::Install { name, .. } => server("install", name),
            Self::Options { name, set, .. } => {
                let mut map = serde_json::Map::new();
                for item in set {
                    // 断り（`OptionRefusal`）は表示言語で文にする（#2365。GUI へ送る前の手元の読み取り）
                    let (key, value) = tako_core::lsp::init_options::parse_assignment(item)
                        .map_err(|e| {
                            e.text_in(tako_control::settings::peek_lang_setting().resolve())
                        })?;
                    map.insert(key, value);
                }
                Request::LspServer {
                    action: "options".into(),
                    name: name.clone(),
                    set: (!map.is_empty()).then_some(map),
                }
            }
            Self::Diagnostics { pane, severity, .. } => Request::LspDiagnostics {
                pane: *pane,
                severity: severity.clone(),
            },
            Self::Definition(args)
            | Self::Declaration(args)
            | Self::TypeDefinition(args)
            | Self::Implementation(args) => Request::LspGoto {
                action: self
                    .goto_kind()
                    .map(|k| k.slug())
                    .unwrap_or_default()
                    .to_string(),
                pane: target_pane(args.pane)?,
                line: args.line,
                column: args.column,
                open: args.open.clone(),
                choice: args.choice,
                focus: args.focus.then_some(true),
            },
            Self::Format { pane, range, .. } => Request::LspFormat {
                pane: target_pane(*pane)?,
                range: range.as_deref().map(parse_line_col_range).transpose()?,
            },
            Self::FormatOnSave { state } => Request::LspFormatOnSave {
                enabled: state.as_deref().map(|s| s == "on"),
            },
            Self::Completion(args) => Request::LspCompletion {
                pane: target_pane(args.pane)?,
                line: args.line,
                column: args.column,
                limit: args.limit,
                choice: args.choice,
                resolve: args.resolve.then_some(true),
            },
            Self::Menu(args) => Request::LspMenu {
                pane: target_pane(args.pane)?,
                line: args.line,
                column: args.column,
            },
            Self::Hover(args) => Request::LspHover {
                pane: target_pane(args.pane)?,
                line: args.line,
                column: args.column,
                show: args.show.then_some(true),
                limit: args.wire_limit(),
            },
            Self::References(args)
            | Self::IncomingCalls(args)
            | Self::OutgoingCalls(args)
            | Self::Supertypes(args)
            | Self::Subtypes(args) => Request::LspXref {
                action: self
                    .xref_kind()
                    .map(|k| k.slug())
                    .unwrap_or_default()
                    .to_string(),
                pane: target_pane(args.pane)?,
                line: args.line,
                column: args.column,
                show: args.show.then_some(true),
                limit: args.limit,
            },
            Self::Results(args) => results(
                tako_control::dispatch::LSP_RESULTS_ACTION,
                args.pane,
                None,
                args.limit,
            ),
            Self::Expand(args) => results(
                tako_control::dispatch::LSP_EXPAND_ACTION,
                args.pane,
                Some(args.item),
                None,
            ),
            Self::Collapse(args) => results(
                tako_control::dispatch::LSP_COLLAPSE_ACTION,
                args.pane,
                Some(args.item),
                None,
            ),
            Self::OpenResult(args) => Request::LspResults {
                action: tako_control::dispatch::LSP_OPEN_RESULT_ACTION.to_string(),
                pane: args.pane,
                item: Some(args.item),
                open: args.open.clone(),
                focus: args.focus.then_some(true),
                limit: None,
            },
            Self::ClearResults { pane } => results(
                tako_control::dispatch::LSP_CLEAR_RESULTS_ACTION,
                *pane,
                None,
                None,
            ),
            Self::Rename(args) => Request::LspRename {
                pane: target_pane(args.pane)?,
                line: args.line,
                column: args.column,
                new_name: args.new_name.clone(),
                confirm: args.yes,
            },
        })
    }

    /// 参照・階層なら種類（#1685）
    fn xref_kind(&self) -> Option<tako_core::lsp::xref::XrefKind> {
        use tako_core::lsp::xref::XrefKind;
        Some(match self {
            Self::References(_) => XrefKind::References,
            Self::IncomingCalls(_) => XrefKind::IncomingCalls,
            Self::OutgoingCalls(_) => XrefKind::OutgoingCalls,
            Self::Supertypes(_) => XrefKind::Supertypes,
            Self::Subtypes(_) => XrefKind::Subtypes,
            _ => return None,
        })
    }

    /// 定義ジャンプ系なら種類
    fn goto_kind(&self) -> Option<tako_core::lsp::goto::GotoKind> {
        use tako_core::lsp::goto::GotoKind;
        Some(match self {
            Self::Definition(_) => GotoKind::Definition,
            Self::Declaration(_) => GotoKind::Declaration,
            Self::TypeDefinition(_) => GotoKind::TypeDefinition,
            Self::Implementation(_) => GotoKind::Implementation,
            _ => return None,
        })
    }

    fn json(&self) -> bool {
        match self {
            Self::Status { json, .. }
            | Self::Servers { json }
            | Self::Install { json, .. }
            | Self::Options { json, .. }
            | Self::Diagnostics { json, .. }
            | Self::Format { json, .. } => *json,
            Self::Definition(args)
            | Self::Declaration(args)
            | Self::TypeDefinition(args)
            | Self::Implementation(args) => args.json,
            Self::Completion(args) => args.json,
            Self::Menu(args) => args.json,
            Self::Hover(args) => args.json,
            Self::References(args)
            | Self::IncomingCalls(args)
            | Self::OutgoingCalls(args)
            | Self::Supertypes(args)
            | Self::Subtypes(args) => args.json,
            Self::Results(args) => args.json,
            Self::Expand(args) | Self::Collapse(args) => args.json,
            Self::OpenResult(args) => args.json,
            Self::Rename(args) => args.json,
            _ => false,
        }
    }
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
    /// ファイルツリーに出ている git の状態を取得する（#1009）。
    /// サイドバーの色・バッジと同じ分類（modified / added / deleted / renamed /
    /// untracked / conflicted / ignored）を、ステージ済み（staged）と
    /// 未ステージ（unstaged）に分けて返す。
    /// propagated=true の行はディレクトリで、changed が配下の変更ファイル数
    #[command(name = "git-status")]
    GitStatus {
        /// 対象フォルダ（省略時はタブのワークスペースフォルダ全部 = 画面に出ている範囲）
        path: Option<String>,
        /// 対象タブ ID（省略時は呼び出し元ペインのタブ）
        #[arg(long)]
        tab: Option<u64>,
        /// 返すエントリ数の上限（既定 500）
        #[arg(long)]
        limit: Option<usize>,
    },
    /// ファイルツリーで選んでいる行を読む・選ぶ・キーと同じに動かす（#1908）。
    /// 引数無し = いまの選択を表示 / <path> = その行だけを選ぶ（開かない）/
    /// --key = 選んでいる行の上でそのキーを押したのと同じ（画面のキーと同じ dispatch を通る）
    Selection {
        /// 選ぶ行（ツリーに見えている行。CLI の cwd 基準で絶対化する）
        path: Option<String>,
        /// up / down / left / right / enter / extend_up / extend_down / extend_top / extend_bottom
        #[arg(long, conflicts_with = "path")]
        key: Option<String>,
        /// アクティブタブの ID（照合だけ。ツリーはアクティブタブのものだけを出す）
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
    /// Claude 公式 Remote Control の session URL を表示する（#1069）
    Link {
        /// 対象ペイン ID（省略時は呼び出し元ペイン。--id と排他）
        #[arg(long, conflicts_with = "id")]
        pane: Option<u64>,
        /// session_id で引く（前方一致可。--pane と排他）
        #[arg(long)]
        id: Option<String>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
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
enum TodoCommand {
    /// タスクを起票する
    Add {
        /// 一覧に出る 1 行
        title: String,
        /// 本文（markdown）
        #[arg(long)]
        body: Option<String>,
        /// 種類: review / confirm / permission / post / other
        #[arg(long)]
        kind: Option<String>,
        /// プロジェクトキー
        #[arg(long)]
        project: Option<String>,
        /// 添付の絶対パス（複数指定可）
        #[arg(long = "attach", value_name = "PATH")]
        attachments: Vec<String>,
        /// コピー用テキスト（`ラベル=本文`。複数指定可）
        #[arg(long = "copy-text", value_name = "LABEL=TEXT")]
        copy_texts: Vec<String>,
        /// 関連 URL（複数指定可）
        #[arg(long = "link", value_name = "URL")]
        links: Vec<String>,
        /// 期限（YYYY-MM-DD）
        #[arg(long)]
        due: Option<String>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 一覧（既定は未完了のみ）
    List {
        /// 状態で絞り込む: open / done / dismissed
        #[arg(long)]
        status: Option<String>,
        /// 種類で絞り込む
        #[arg(long)]
        kind: Option<String>,
        /// プロジェクトで絞り込む
        #[arg(long)]
        project: Option<String>,
        /// 状態の絞り込みを外す（完了・却下も出す）
        #[arg(long)]
        all: bool,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 1 件の詳細（本文・添付・返答・配送状態）
    Show {
        /// 対象のタスク id（u-N）
        id: String,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 内容を更新する（指定した項目だけ置き換える）
    Update {
        /// 対象のタスク id
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        project: Option<String>,
        /// 添付を置き換える（複数指定可）
        #[arg(long = "attach", value_name = "PATH")]
        attachments: Vec<String>,
        /// コピー用テキストを置き換える（複数指定可）
        #[arg(long = "copy-text", value_name = "LABEL=TEXT")]
        copy_texts: Vec<String>,
        /// 関連 URL を置き換える（複数指定可）
        #[arg(long = "link", value_name = "URL")]
        links: Vec<String>,
        #[arg(long)]
        due: Option<String>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 片付いたことにする
    Done {
        /// 対象のタスク id
        id: String,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// やらないことにする
    Dismiss {
        /// 対象のタスク id
        id: String,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 右パネルの tasks ビューでその 1 件をその場に展開して見せる（#1479）。
    /// パネルが閉じていれば開いて tasks ビューへ切り替える
    Expand {
        /// 対象のタスク id
        id: String,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 展開を畳む（#1479）
    Collapse {
        /// JSON で出力する
        #[arg(long)]
        json: bool,
    },
    /// 返答する（起票した master へ届く）
    Respond {
        /// 対象のタスク id
        id: String,
        /// 判断: approve / reject / needs_change / answered
        #[arg(long)]
        decision: String,
        /// 自由記述（スレッドとして積まれる）
        #[arg(long)]
        comment: Option<String>,
        /// どこから返したか: pc / pwa / cli / mcp（省略時 cli）
        #[arg(long)]
        via: Option<String>,
        /// JSON で出力する
        #[arg(long)]
        json: bool,
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
    /// （transport は Tailscale Serve + ループバック TCP。未セットアップなら不足項目を案内して停止）
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
        /// 対象（tako のペイン ID / 器のセッション名 / `session:window.pane`）
        pane_id: String,
        /// 取得する履歴行数（省略時は 1000）
        #[arg(long, default_value_t = 1000)]
        lines: u32,
    },
    /// ペアリング済み端末の管理（一覧・権限を弱める・失効。
    /// 権限を上げるのは Mac 画面 = 承認ダイアログ / 設定 → リモートだけ）
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
        /// 使う Tailscale 系統。auto / gui = CLI の既定探索に任せる（GUI 版アプリが
        /// 入っていればそれ）、standalone = 自前の tailscaled を --socket で名指しする。
        /// 省略時は検出結果から決める（2 系統が同居しているときだけ選択を聞く）
        #[arg(long, value_name = "auto|gui|standalone")]
        tailscale: Option<String>,
    },
    /// スマホのファイル閲覧で使うショートカット（お気に入り）を管理する（#1451）。
    /// 引数なしで一覧、`add <path>` で追加、`remove <path|id>` で削除
    Shortcuts {
        #[command(subcommand)]
        command: Option<RemoteShortcutsCommand>,
    },
    /// [内部用] HTTP サーバーをフォアグラウンドで起動する（start から自動呼び出し）
    Serve,
}

#[derive(Subcommand)]
enum RemoteShortcutsCommand {
    /// 既定（ホーム / デスクトップ / ダウンロード）+ 登録分を一覧する
    List,
    /// ショートカットを追加する（同じパスを 2 回足しても 1 件のまま）
    Add {
        /// 追加するフォルダの絶対パス
        path: String,
        /// 表示名（省略時は末尾のフォルダ名）
        #[arg(long)]
        name: Option<String>,
    },
    /// ショートカットを削除する（既定のショートカットは消せない）
    Remove {
        /// 消す対象（`tako remote shortcuts` の id でも、登録したパスでもよい）
        path: String,
    },
}

#[derive(Subcommand)]
enum RemoteDevicesCommand {
    /// 登録済み端末と保留中の要求（ペアリング・権限の更新）を一覧する
    List,
    /// 端末の権限を**弱める**（上げるのは Mac 画面からだけ = #1452）
    Role {
        /// 対象デバイス ID（`tako remote devices list` で確認できる）
        device_id: String,
        /// 行き先の権限（observe / interact / manage / admin）。
        /// 現在より強いものを渡すと、画面での操作を案内して断る
        role: String,
    },
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
    /// （使用中・ユーザーのセッションには触れない）。
    ///
    /// 応答は `{socket, killed, skipped, detail}`。掃除しなかったときは `skipped` に
    /// 理由コード（`peer_shares_socket` = 同じソケットを使う別の tako-app が生きている 等）が
    /// 入るので、「対象が無かった」と「見送った」を区別できる
    Cleanup {
        /// tmux サーバー名（`tmux -L` 相当。省略時は tako バックエンドサーバー）
        #[arg(long)]
        socket: Option<String>,
        /// セッションではなく**サーバー（ソケット）単位**で回収する（#1192 / #1282）。
        /// 隔離起動やテストが残した tmux サーバーと残骸ソケットが対象。
        /// 器の見つけ方は unix = ソケットファイルの走査、Windows（psmux）=
        /// 器のプロセスのコマンドライン（`-L <名前>`）。
        /// **既定は dry-run**（一覧と判定理由を返すだけ）
        #[arg(long)]
        servers: bool,
        /// `--servers` の判定どおりに実際に kill / ソケット削除を行う。
        /// 所有プロセスが生きているサーバーには付けても触らない
        /// （Windows は器の pid を名指しして落とす。名前一致の一括 kill はしない）
        #[arg(long)]
        apply: bool,
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
        /// tmux サーバー名（`tmux -L` 相当）。
        /// 省略時は `tako tmux list` と同じく既定サーバーと tako バックエンドの両方から探す
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
        /// manual サイズを解除して自動サイズへ戻す
        #[arg(long)]
        reset: bool,
        /// tmux サーバー名（`tmux -L` 相当）。
        /// 省略時は `tako tmux list` と同じく既定サーバーと tako バックエンドの両方から探す
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
        /// tmux サーバー名（`tmux -L` 相当。`tako tmux list` の socket をそのまま渡す）。
        /// 省略時は既定サーバーと tako バックエンドの両方から探す
        #[arg(long)]
        socket: Option<String>,
        /// 取り込み直後に表示する window index（省略時はセッションの現在 window）
        #[arg(long)]
        window: Option<u32>,
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

/// `tako file clipboard` の操作（#1860 / 複数は #1867）
#[derive(Subcommand)]
enum FileClipboardCommand {
    /// コピーする（貼ると複製。OS のクリップボードにも書くのでファイルマネージャへも貼れる）。
    /// 複数を渡すとまとめて置く（ファイルツリーの複数選択と同じ）
    Copy {
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
    },
    /// 切り取る（貼ると移動）。複数を渡すとまとめて置く
    Cut {
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
    },
    /// 中身と、DIR へ貼ったときの貼り付け先を表示する（省略時はカレントディレクトリ）
    Show { dir: Option<String> },
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
    /// ファイル・フォルダを別のフォルダへ移す（ファイルツリーのドラッグ＆ドロップと同じ。
    /// 同名・自分の配下・別のボリュームは断る。開いているペインは新しいパスへ付け替わる）。
    /// 最後の引数が移動先のフォルダ。複数を渡すとまとめて移す（1 件でも断られたら終了コード 1）
    Move {
        /// 移すもの（1 つ以上）と、最後に移動先のフォルダ
        #[arg(required = true, num_args = 2.., value_name = "SRC... DEST")]
        paths: Vec<String>,
    },
    /// ファイル・フォルダを別のフォルダへ複製する（ファイルツリーのコピー → 貼り付けと同じ。
    /// フォルダは中身ごと。同名は上書きせず「名前 のコピー」等の別名で置く。自分の配下へは断る）。
    /// 最後の引数が貼り付け先のフォルダ。例: `tako file copy a.txt docs dst`
    Copy {
        /// コピー元（1 つ以上）と、最後に貼り付け先のフォルダ
        #[arg(required = true, num_args = 2.., value_name = "SRC... DEST")]
        paths: Vec<String>,
    },
    /// ファイルツリーのクリップボード（コピー・切り取り・中身の確認）。引数なしは中身の確認
    Clipboard {
        #[command(subcommand)]
        action: Option<FileClipboardCommand>,
    },
    /// クリップボードの中身を貼る（ファイルツリーの貼り付けと同じ。コピーは複製・切り取りは移動。
    /// ファイルマネージャでコピーしたものも貼れる）
    Paste {
        /// 貼り付け先（フォルダならその中・ファイルならそのフォルダ。省略時はカレントディレクトリ）
        dest: Option<String>,
        /// 移動として貼る（コピーしたものも移す = ファイルツリーの ⌥⌘V / Windows は Ctrl+Alt+V）
        #[arg(long = "move")]
        as_move: bool,
    },
    /// 走っているコピーの進み具合（件数・バイト）を表示する
    Progress,
    /// 走っているコピーを取り消す（作りかけは消す。ID は `tako file progress` の id、省略で全部）
    Cancel { id: Option<u64> },
    /// 新しいファイルを作成する（path 配下に name で作成）
    Create { path: String, name: String },
    /// 新しいフォルダを作成する（path 配下に name で作成）
    Mkdir { path: String, name: String },
    /// ファイル・フォルダをゴミ箱（Windows はごみ箱）へ移動する（完全削除ではない）。
    /// 複数を渡すとまとめて移す（1 件でも断られたら終了コード 1）
    Trash {
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
    },
    /// デフォルトアプリで開く
    Open { path: String },
    /// 指定アプリで開く
    OpenWith { path: String, name: String },
    /// tako の中で開く（ファイル = プレビューペイン / ディレクトリ = そのディレクトリの
    /// シェル。ターミナル内のパスリンクの修飾 + クリック・パスメニューと同じ動作。#1182）
    #[command(about = format!(
        "tako の中で開く（ファイル = プレビューペイン / ディレクトリ = そのディレクトリの\
         シェル。ターミナル内のパスリンクの{}・パスメニューと同じ動作。#1182）",
        link_click()
    ))]
    OpenInTako {
        path: String,
        #[arg(long)]
        pane: Option<u64>,
    },
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
    /// 行・桁で指定した範囲だけを置き換える（保存はしない）
    ///
    /// 位置は `行:桁`（行は 1 始まり・桁は 0 始まりの UTF-8 バイト。桁は省略可 = 0）。
    /// 例: `tako edit replace-range 12:0 12:34 "新しい行"`
    ReplaceRange {
        /// 置き換える範囲の始まり（`行:桁`）
        start: String,
        /// 置き換える範囲の終わり（`行:桁`）
        end: String,
        /// 範囲に入れる本文
        text: String,
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// カーソルを移動する（`--select-to` でそこまで選択）
    Cursor {
        /// カーソルの位置（`行:桁`）
        position: String,
        /// ここまで選択する（`行:桁`）。カーソルはこちら側へ来る
        #[arg(long)]
        select_to: Option<String>,
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// カーソルを単語・行・ページ・文書端の単位で動かす（⌥← / Home / Page Down と同じ）
    ///
    /// 単位: left / right / up / down / word-left / word-right / line-start /
    /// smart-home / line-end / doc-start / doc-end / page-up / page-down。
    /// 選択中の left / right は選択の端へ畳む（←→ の打鍵と同じ）。
    /// 例: `tako edit move word-left --select`
    Move {
        /// 動かす単位
        movement: String,
        /// 選択を伸ばす（⇧ 付きの打鍵と同じ）
        #[arg(long)]
        select: bool,
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 単語・行単位で消す（⌥⌫ / ⌘⌫ と同じ。選択があれば選択を消す）
    ///
    /// 単位: char-backward / char-forward / word-backward / word-forward /
    /// to-line-start / to-line-end。例: `tako edit delete word-backward`
    Delete {
        /// 消す単位
        motion: String,
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 選択行を 1 段深くする（Tab と同じ。選択が無ければカーソル位置へ 1 段ぶん挿す）
    ///
    /// インデントの単位（タブ / スペース幅）は既存行から推定する
    Indent {
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 選択行（無ければカーソルの行）を 1 段浅くする（Shift+Tab と同じ）
    Outdent {
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// インデントを引き継いで改行する（Enter と同じ。開き括弧の直後は 1 段深く・
    /// Python の return / pass などの後ろは 1 段浅く・Markdown のリストは印を引き継ぐ）
    Newline {
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 文字を 1 つずつ打鍵として入れる（キーボードで打つのと同じ。#1759）
    ///
    /// 開き括弧は閉じ括弧も入れ、自分で入れた閉じ括弧の手前の閉じ括弧は飛び越す
    /// （`tako edit auto-close off` で 1 文字ずつ入れるだけ）。1 回 1000 文字まで。
    /// 例: `tako edit type "f(x)"`
    Type {
        /// 打つ文字
        text: String,
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 括弧の自動閉じの設定（既定 on。Markdown・プレーンテキストは on でも閉じない。#1759）。
    /// 省略で今の値。MCP は `tako_preview_edit` の auto_close
    AutoClose {
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
    },
    /// 選択した行（無ければカーソルの行）を行単位で操作する（#1692）
    ///
    /// 操作: duplicate-up / duplicate-down（行の複製）/ move-up / move-down（行の移動。
    /// 文書の端では何もしない）/ indent / outdent（行ごと 1 段深く / 浅く）/ comment /
    /// uncomment / toggle-comment（行コメントの付け外し。記号はファイルの形式で決まる）。
    /// undo 1 回で戻る。例: `tako edit line move-up`
    Line {
        /// 操作
        operation: String,
        /// 文書の版。指定すると版が違うときは何もせず失敗する
        #[arg(long)]
        expect_version: Option<u64>,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// 編集バッファをファイルへ保存する（外で変わっていたら書かずに断る）
    Save {
        /// 外で変わっていても自分の変更で上書きする（消されていれば作り直す。#1659）。
        /// `--overwrite` は同じ意味の別名（MCP `tako_preview_save` の `action=overwrite` と同じ綴り。#1833）
        #[arg(long, visible_alias = "overwrite")]
        force: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ディスクから読み直す（編集中の変更は捨てる。undo 1 回で戻せる。#1659）
    Reload {
        #[arg(long)]
        pane: Option<u64>,
    },
    /// ディスクと編集中の差分（ディスク → 編集中の unified diff。何も変えない。#1659）
    Diff {
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
    /// テキスト検索（query 省略時は現在の検索状態を返す）。
    /// 既定は大文字小文字を区別する。query を省略したときは今の検索の条件を引き継ぐ。
    /// `--open` / `--close` は画面の検索欄を開く / 閉じる（GUI の ⌘F・Escape と同じ）
    Search {
        /// 検索文字列
        query: Option<String>,
        /// 移動方向（next / prev。省略時は next。`--open` / `--close` だけのときは動かない）
        #[arg(long)]
        direction: Option<String>,
        /// 大文字小文字を区別しない（既定は区別する）
        #[arg(short = 'i', long)]
        ignore_case: bool,
        /// 単語単位で探す（前後が英数字・かな漢字・_ に続く一致を外す）
        #[arg(short = 'w', long)]
        whole_word: bool,
        /// 画面の検索欄を開く（開いた欄で探すとヒットをエディタの行の上に描く）
        #[arg(long, conflicts_with = "close")]
        open: bool,
        /// 画面の検索欄を閉じる（閲覧中の Markdown は描画へ戻る）
        #[arg(long)]
        close: bool,
        #[arg(long)]
        pane: Option<u64>,
    },
    /// テキスト置換（1 件または全置換）。既定は大文字小文字を区別する
    Replace {
        /// 検索文字列
        query: String,
        /// 置換文字列
        replacement: String,
        /// 全置換
        #[arg(long)]
        all: bool,
        /// 大文字小文字を区別しない（既定は区別する）
        #[arg(short = 'i', long)]
        ignore_case: bool,
        /// 単語単位で置き換える
        #[arg(short = 'w', long)]
        whole_word: bool,
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
    /// 走っている master を専用プロファイルへその場で寄せる（#1453。立て直さない）。
    /// 名前はプロファイル名、またはまだプロファイルの無い projects.yaml のキー
    /// （後者はその場で default から継承して作る）。default を渡すと汎用へ戻る
    Adopt {
        /// 採用するプロファイル名 / プロジェクトキー
        #[arg(value_name = "NAME")]
        name: String,
        /// 対象の master ペイン ID（省略時は呼び出し元）
        #[arg(long)]
        pane: Option<u64>,
    },
    /// プロファイル管理（一覧 / 表示 / 設定）
    #[command(subcommand)]
    Profiles(ProfilesCommand),
    /// アカウント管理（accounts.yaml の一覧 / 表示 / 追加 / 削除。#504 / #548）
    #[command(subcommand)]
    Accounts(AccountsCommand),
    /// master の手順書を引く（#1154。topic 省略で一覧）
    Guide {
        /// 引く topic（省略時は一覧）
        #[arg(value_name = "TOPIC")]
        topic: Option<String>,
        /// プレースホルダを解決するプロファイル（省略時は呼び出し元の role → default）
        #[arg(long)]
        profile: Option<String>,
        /// 生の JSON で出力する
        #[arg(long)]
        json: bool,
    },
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
        /// worker ペイン 1 枚に保証する最小の桁数（既定 60。0 = 保証しない / 20〜400）。
        /// 割っても別タブへは出さず、worker ペインのフォントを縮めて確保する（#1132 / #1439）
        #[arg(long)]
        min_worker_cols: Option<u16>,
        /// 下限桁数を割るとき worker ペインのフォントを自動で縮めるか（既定 true。
        /// false でも別タブへは出さず、狭いまま置く）（#1439）
        #[arg(long)]
        auto_shrink_font: Option<bool>,
        /// 自動縮小の床（既定フォントサイズに対する比率。0.4〜1.0。既定 0.6）。
        /// 床まで縮めても下限に届かなければ床で置き、応答に cols_short が立つ（#1439）
        #[arg(long)]
        min_worker_font_scale: Option<f32>,
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
    /// error.kind（api_error / usage_limit / limit_dialog / launch_failed /
    /// entitlement_blocked）と recommended_action を含む。#157 / #983 / #1106）
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
    /// 突然死した worker の会話を、spawn と同じ起動（プロファイルの許可モード・env・MCP の絞り込み・
    /// model / effort・アカウント）でこのシェルに戻す（#390 / #2470）。worker_status / watch / workers の
    /// `resume_command` がこの 1 行。死んだペインのシェルで打つ。GUI が無くても動く
    Resume {
        /// worker ID（`tako orchestrator workers --all` の worker_id）
        worker_id: String,
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

/// `tako orchestrator projects mcp <key> <op>` の操作（#2412）
#[derive(Clone, Copy, clap::ValueEnum)]
enum ProjectMcpOp {
    /// 持ち込む MCP を足す
    Add,
    /// 持ち込む MCP を外す
    Remove,
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
    /// claude worker へ持ち込む user scope の MCP を見る・足す・外す
    /// （既定は tako の MCP だけ。.mcp.json のものは書かなくても立つ。#2412）
    Mcp {
        /// プロジェクトキー
        key: String,
        /// add / remove（省略は今の構成を見る）
        op: Option<ProjectMcpOp>,
        /// MCP サーバー名（* = user scope を全部持ち込む）
        servers: Vec<String>,
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
        /// 対象エージェントの worker 既定 effort（claude / codex / agy 共通。--clear-agent-effort と排他）
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
        /// master / solo をこのフォルダで起動する（絶対パスか ~/ 始まり。空文字でクリア。#500 / #1056）
        #[arg(long, conflicts_with = "clear_cwd")]
        cwd: Option<String>,
        /// cwd の指定を解除する（#1056）
        #[arg(long)]
        clear_cwd: bool,
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
        /// master / solo 本人のペイン（引き継ぎの後任を含む。#1140）と spawn した
        /// worker（#822）で利用上限後の自動復帰を既定 ON にする（既定 false）
        #[arg(long, conflicts_with = "clear_limit_resume")]
        limit_resume: Option<bool>,
        /// limit_resume を解除して既定（無効）へ戻す（#822 / #1140）
        #[arg(long)]
        clear_limit_resume: bool,
        /// codex（master / worker）のサンドボックスと承認を丸ごと外して起動することを
        /// 許可する（既定 false。#981。true にすると codex は確認なしでコマンドを実行し、
        /// 書き込み先もネットワークも制限されなくなる）
        #[arg(long)]
        bypass_sandbox: Option<bool>,
        /// この会話を Claude 公式の Remote Control へ繋ぐ（既定 false。#1068。
        /// true にすると claude.ai と Claude モバイルアプリからこの会話を操作できるが、
        /// transcript は Anthropic のサーバーにも保存され、tako の機器ペアリングと
        /// role はその会話には効かなくなる）
        #[arg(long)]
        remote_control: Option<bool>,
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
    /// 開いた直後にこの行へ飛ぶ（1 始まり。行数を超えたら末尾行へ丸める）。
    /// 指定すると表示は原文の行がそのまま残る code になる（#1676）
    #[arg(long, allow_negative_numbers = true)]
    line: Option<i64>,
    /// 着地する桁（1 始まりの文字単位。--line と一緒に指定する）
    #[arg(long, requires = "line", allow_negative_numbers = true)]
    column: Option<i64>,
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
/// possible values に旧称も出し、どちらの語彙からでも辿れるようにする）。
///
/// 各値の意味は MCP `tako_panel` の説明と同じ正本（[`PanelViewWire::summary`]）から引く。
/// `--view` の説明文に値を手書きで並べると、ビューが増えたときに追従が漏れる
/// （#2036: fleet / orch / git の 3 つのまま tasks 以降が抜けていた）
fn panel_view_parser() -> clap::builder::PossibleValuesParser {
    use clap::builder::PossibleValue;
    use tako_control::protocol::PanelViewWire;

    let mut values: Vec<PossibleValue> = PanelViewWire::VALUES
        .iter()
        .map(|v| {
            let view = PanelViewWire::parse(v).expect("VALUES の値は parse を通る");
            PossibleValue::new(*v).help(view.summary())
        })
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
    /// 表示するビュー（値は GUI の右パネルのタブ名と同じ。各値の意味は `--help` の一覧）
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

/// `tako persist` の引数。on / off は ToggleArgs と同じで、`reattach` だけペインを取る（#1857）
#[derive(Args)]
struct PersistArgs {
    /// on = 有効化、off = 無効化、reattach = ペインを tmux のセッションへ attach し直す
    /// （省略時は現在状態を表示）
    #[arg(value_parser = ["on", "off", "reattach"])]
    state: Option<String>,
    /// reattach の対象ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID）
    #[arg(long)]
    pane: Option<u64>,
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

/// 利用上限後の自動復帰の引数（Issue #813 / #1945）とネット復帰の即時再開（Issue #2112）。
///
/// `tako limit-resume` = 呼び出し元ペインの現在値、`on` / `off` で切替、
/// `--all` で全ペインの一覧、`on --all` / `off --all` で一括（ステータスバーのボタンと同じ）。
/// `--network` = ネット復帰の再開の設定と状態、`off --network` / `on --network` で切替。
/// 素のコマンドが最短で済む形にしてある（#322）
#[derive(Args)]
struct LimitResumeArgs {
    /// on / off（省略時は現在状態を表示）
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
    /// 対象ペイン ID（省略時は呼び出し元 = TAKO_PANE_ID。退避中のペインも指定できる）
    #[arg(long)]
    pane: Option<u64>,
    /// 全ペインを対象にする。単独なら一覧、on / off と併用すると退避中を含む全エージェントの
    /// ペインを一括で切り替え、以後に立つペインの既定も同じ値にする（#1945）
    #[arg(long, conflicts_with = "pane")]
    all: bool,
    /// ネット復帰の即時再開（全体の設定。既定 on）。回線が戻ったら、接続エラーで止まった
    /// 全エージェントのセッションへ続きを送る。単独なら設定と状態、on / off で切替（#2112）
    #[arg(long, conflicts_with_all = ["pane", "all"])]
    network: bool,
}

#[derive(Args)]
struct PreviewCacheArgs {
    /// キャッシュ上限（MiB、256〜8192。天井。実効上限は見えている量から決まる。
    /// 省略時は利用状況とペインごとの持ち分を表示）
    max_mb: Option<u64>,
}

/// リンク列挙の引数（Issue #1283）。
/// 最簡形（#322）: `tako links` で呼び出し元ペインの画面を見る
#[derive(Args)]
struct LinksArgs {
    /// 対象ペイン ID（省略時は呼び出し元ペイン）
    #[arg(long)]
    pane: Option<u64>,
    /// ペインの画面の代わりに検査する画面テキスト（改行区切り。`-` で標準入力）
    #[arg(long)]
    text: Option<String>,
    /// --text の想定桁数（省略時は 120）
    #[arg(long)]
    cols: Option<usize>,
    /// 相対パスの解決基準（省略時はペインの cwd）
    #[arg(long)]
    cwd: Option<String>,
}

/// ワークスペース横断検索の引数（Issue #1694）。
/// 最簡形（#322）: `tako search <検索語>` でタブのファイルツリーのルートから探す
#[derive(Args)]
struct SearchArgs {
    /// 検索語（--results / --open / --clear では要らない）
    query: Option<String>,
    /// 探すフォルダ（省略時はタブのファイルツリーのルート）
    #[arg(long)]
    path: Option<String>,
    /// 検索語を正規表現として読む
    #[arg(long)]
    regex: bool,
    /// 大文字小文字を区別する
    #[arg(long)]
    case_sensitive: bool,
    /// 右パネルの search ビューにも出す
    #[arg(long)]
    show: bool,
    /// 表示する行の数（既定 100。0 で全部）
    #[arg(long)]
    limit: Option<usize>,
    /// 右パネルの結果を読む
    #[arg(long, conflicts_with_all = ["query", "open", "clear", "show"])]
    results: bool,
    /// 右パネルの結果の N 番目を開く（行を押すのと同じ）
    #[arg(long, value_name = "N", conflicts_with_all = ["query", "clear", "show"])]
    open: Option<usize>,
    /// 開いたペインへフォーカスを移す（--open と併用）
    #[arg(long, requires = "open")]
    focus: bool,
    /// 右パネルの結果を閉じる（走っている検索も取り消す）
    #[arg(long, conflicts_with_all = ["query", "show"])]
    clear: bool,
    /// 対象ペイン（探す範囲のタブ・--open の起点。省略時は呼び出し元）
    #[arg(long)]
    pane: Option<u64>,
    /// 応答を JSON のまま出す
    #[arg(long)]
    json: bool,
}

impl SearchArgs {
    fn action(&self) -> &'static str {
        if self.results {
            "results"
        } else if self.open.is_some() {
            "open"
        } else if self.clear {
            "clear"
        } else {
            "search"
        }
    }
}

/// 横断検索のキーの表記（help に出す。macOS = ⇧⌘F / Windows = Ctrl+Shift+G）
fn find_in_files_key() -> &'static str {
    tako_core::platform::keys::find_in_files(tako_core::platform::support::Platform::current())
}

/// スクロールバック保持上限の引数（Issue #818）。
/// 最簡形（#322）: `tako scrollback` で現在値、`tako scrollback 2000` で変更
#[derive(Args)]
struct ScrollbackArgs {
    /// 保持行数（100〜100000。省略時は現在値と適用中のペイン数を表示）
    lines: Option<usize>,
}

/// フォーカスの無いペインの再描画の上限の引数（Issue #1979）。
/// 最簡形（#322）: `tako redraw-limit` で現在値、`tako redraw-limit 20` で変更
#[derive(Args)]
struct RedrawLimitArgs {
    /// 上限（1〜60 fps。既定 30。フォーカス中のペインは常に 60。省略時は現在値と
    /// 起動からの再描画回数を表示）
    fps: Option<u32>,
}

/// クリックの転送の引数（Issue #1961）。
/// 最簡形（#322）: `tako mouse` で現在状態、`tako mouse off` で #1961 前（常に tako の選択）へ
#[derive(Args)]
struct MouseArgs {
    /// on = 渡す（既定）/ off = 渡さない。省略時は現在状態と累計を表示
    #[arg(value_parser = ["on", "off"])]
    state: Option<String>,
}

/// 貼り付けの確認の引数（Issue #2046 / #2050）。
/// 最簡形（#322）: `tako paste-guard` で現在状態、`tako paste-guard off` で切る、
/// `tako paste-guard warn` で止めずに警告だけ、`pbpaste | tako paste-guard check` で判定だけ試す、
/// `pbpaste | tako paste-guard allow` でその値を許可リストへ（本文は標準入力から読む）
#[derive(Args)]
struct PasteGuardArgs {
    /// on = 確かめる（既定）/ warn = 止めずに警告だけ / off = 切る、check = 判定だけ試す、
    /// send / cancel = 確認待ちの貼り付けへ答える、allow / unallow = 許可リストへ足す / 外す、
    /// rule = 規則ごとの有効 / 無効。省略時は現在状態と累計（確認待ちがあれば伏せ字の要約）を表示
    #[arg(value_parser = [
        "on", "off", "warn", "check", "send", "cancel", "allow", "unallow", "rule",
    ])]
    action: Option<String>,
    /// check / allow の文字（省略か `-` で標準入力から読む。鍵をシェルの履歴へ残さないよう
    /// 標準入力を勧める）・unallow の ID（`tako paste-guard` の allow[].id）か all・rule の規則名
    text: Option<String>,
    /// rule の on / off
    #[arg(value_parser = ["on", "off"])]
    value: Option<String>,
    /// check をそのペインの送信時の確認（tako mod の 2 段目）として判定する
    #[arg(long)]
    pane: Option<u64>,
}

/// `tako paste-guard check` の本文（引数が無いか `-` なら標準入力。読む上限は 8 MiB）
fn paste_guard_check_text(arg: Option<&str>) -> Result<String, String> {
    use std::io::Read;
    match arg {
        Some(text) if text != "-" => Ok(text.to_string()),
        _ => {
            let mut buf = String::new();
            std::io::stdin()
                .take(8 << 20)
                .read_to_string(&mut buf)
                .map_err(|e| front_err(tako_control::Reason::StdinReadFailed(e.to_string())))?;
            Ok(buf)
        }
    }
}

/// クリックの引数（Issue #1961）。座標は `tako read` の行・桁（0 始まり）
#[derive(Args)]
struct ClickArgs {
    /// 対象ペイン ID（誤ったペインを押さないよう必須）
    #[arg(long)]
    pane: u64,
    /// 行（0 始まり。画面の上端が 0）
    #[arg(long)]
    row: usize,
    /// 桁（0 始まり）
    #[arg(long)]
    col: usize,
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
    /// 色操作・colors の対象（dark / light / プリセット名。省略 = 現在のテーマ）
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

/// ウェルカムバナーと初回起動の導線の引数（Issue #549 / #1510）
#[derive(Args)]
struct WelcomeArgs {
    /// next（次にやることを進める）/ auto（自動の初期設定をもう一度）/ show（再表示）/
    /// hide（いまだけ閉じる）/ dismiss（閉じて以後出さない）。省略時は状態表示
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

/// 利用制限の観測と使用量の内訳の引数（Issue #2200 / #2201。どれも省略できる）
#[derive(Args)]
struct UsageArgs {
    /// 束ねる段: project（既定）/ issue / task-type / session / model / account / day / role / login
    #[arg(long)]
    by: Option<String>,
    /// 期間: 7d（既定 = 今の 7d の窓）/ 5h / day / week / month / all
    #[arg(long)]
    window: Option<String>,
    /// tako のアカウント名で絞る
    #[arg(long)]
    account: Option<String>,
    /// プロジェクト（master はプロファイル名）で絞る。段の行を 1 つだけ選ぶときは
    /// project:<キー> / profile:<名前> / none（未分類）
    #[arg(long)]
    project: Option<String>,
    /// 主の Issue で絞る（番号 / none = Issue なし）
    #[arg(long)]
    issue: Option<String>,
    /// 段の上位の件数（既定 10）
    #[arg(long)]
    limit: Option<usize>,
    /// 同じ段・期間・絞り込みで右パネルの usage ビューも開く（tako が動いていること）
    #[arg(long)]
    show: bool,
    /// 生の JSON で出力する（MCP `tako_sessions` の `action: "usage"` と同じ形）
    #[arg(long)]
    json: bool,
}

/// agent 能力マトリクスの参照引数（Issue #982）
#[derive(Args)]
struct AgentSupportArgs {
    /// 対象の系統（省略時は全系統ぶんの表）
    #[arg(long, value_parser = ["claude", "codex", "agy", "local"])]
    agent: Option<String>,
    /// この状態のものだけに絞る（省略時は全件）
    #[arg(long, value_parser = ["supported", "degraded", "pending", "unsupported"])]
    status: Option<String>,
    /// 生の JSON で出力する
    #[arg(long)]
    json: bool,
}

/// 環境診断の引数（Issue #1441）
#[derive(Args)]
struct CheckHealthArgs {
    /// 生の JSON で出力する（既定は人が読む要約）
    #[arg(long)]
    json: bool,
}

/// PC リソースの状況と順番待ちの引数（Issue #1993 / #1994）
#[derive(Args)]
struct QueueArgs {
    /// 生の JSON で出力する（既定は人が読む要約）
    #[arg(long)]
    json: bool,
    /// 順番待ちの優先度（top / high / normal / low。普段は付けない。env `TAKO_QUEUE_PRIORITY` でも渡せる）
    #[arg(long)]
    priority: Option<String>,
    /// 待ちの上限（例: 90s / 10m / 1h。単位なしは分。既定は設定値 = 30 分。env `TAKO_QUEUE_MAX_WAIT` でも
    /// 渡せる）。切れたら終了コード 75
    #[arg(long)]
    max_wait: Option<String>,
    /// 状況に出す表示名（既定はコマンド行を短くしたもの）
    #[arg(long)]
    label: Option<String>,
    #[command(subcommand)]
    command: Option<QueueCommand>,
}

/// `tako queue` の操作（Issue #1994）。どれにも当たらない語は「順番待ちで走らせるコマンド」
#[derive(Subcommand)]
enum QueueCommand {
    /// 受付番号の処理を取り消す（待ち = 外れて 125 で終わる / 実行中 = 子を止める）
    Cancel {
        /// 受付番号（`tako queue` の一覧の番号 = 待っている `tako queue` の pid）
        id: u64,
        #[arg(long)]
        json: bool,
    },
    /// メモリの逼迫で起動を待っている agent を今すぐ起こす（#2413。番号は `tako queue` の起動待ちのペイン）
    Start {
        /// 起動を待っているペインの番号（spawn / run の応答の `pane_id`）
        id: u64,
        #[arg(long)]
        json: bool,
    },
    /// 上限を読む / 変える（引数なしで今の値。`settings.json` の `resources` 節）
    Limits {
        /// 同時に走らせる重い処理の数（既定 = RAM 12 GB あたり 1・1〜4）
        #[arg(long)]
        max_jobs: Option<u32>,
        /// 空けておくメモリ（GB。既定 = max(2, RAM の 10%)）
        #[arg(long)]
        reserve_gb: Option<f64>,
        /// swap の使用がこれ以上なら 1 件ずつに絞る（GB。既定 2）
        #[arg(long)]
        swap_guard_gb: Option<f64>,
        /// 待ちの上限（分。既定 30）
        #[arg(long)]
        max_wait_min: Option<u32>,
        /// tako のペインの cargo が共有する札（jobserver。既定 on = 論理 CPU 数ぶん。#1998）
        #[arg(long, value_parser = ["on", "off"])]
        jobserver: Option<String>,
        /// すべて既定へ戻す
        #[arg(long)]
        reset: bool,
        #[arg(long)]
        json: bool,
    },
    /// 隔離の調停役を GUI 無しで立てる（検証用。`TAKO_QUEUE_SOCKET` と `TAKO_TOKEN` が要る）
    #[command(hide = true)]
    Serve,
    /// 順番待ちで走らせるコマンド（例: `tako queue cargo build --release`）
    #[command(external_subcommand)]
    Run(Vec<String>),
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

/// 起動時ロードの予算の引数（Issue #1139）
#[derive(Args)]
struct ContextBudgetArgs {
    /// 操作（省略時は check = 何も書き換えない）
    #[arg(value_parser = ["check", "fix"])]
    action: Option<String>,
    /// 対象フォルダ（省略時は現在のフォルダ）
    #[arg(long)]
    cwd: Option<String>,
    /// system prompt / 引き継ぎを見るプロファイル（省略時は default）
    #[arg(long)]
    profile: Option<String>,
    /// fix のとき、書き込まずに移送予定だけを表示する
    #[arg(long)]
    dry_run: bool,
    /// 生の JSON で出力する
    #[arg(long)]
    json: bool,
}

/// テスト・検証プロセスの残骸掃除の引数（Issue #1296）
#[derive(Args)]
struct TestResidueArgs {
    /// 判定どおりに**実際に削除する**（既定は dry-run = 1 つも消さない）。
    /// 所有プロセスが生きているものには付けても触らない
    #[arg(long)]
    apply: bool,
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

/// tako mod の引数（Issue #1879 / #1881 / #1959）。
///
/// `report` は mod が stdin の JSON で状態を送る口で、人が打つものではないので
/// `--help` の候補に出さない（受け付けはする）
#[derive(Args)]
struct ModArgs {
    /// on / off（省略時は状態を表示）。band = Claude Code の画面の帯（band on / band off）。
    /// ui = 画面の UI 設定（ボタン・バー・帯の区切り・色。引数なしで今の値と選べる値）。
    /// install / uninstall = Claude Code の設定 dir の skills/tako へ写しを置く / 外す
    #[arg(value_parser = clap::builder::PossibleValuesParser::new([
        clap::builder::PossibleValue::new("status"),
        clap::builder::PossibleValue::new("on"),
        clap::builder::PossibleValue::new("off"),
        clap::builder::PossibleValue::new("band"),
        clap::builder::PossibleValue::new("ui"),
        clap::builder::PossibleValue::new("install"),
        clap::builder::PossibleValue::new("uninstall"),
        clap::builder::PossibleValue::new("report").hide(true),
    ]))]
    action: Option<String>,
    /// band のとき: on = 帯を出す / off = 隠す（動いている claude へ次の報告で届く）。
    /// ui のとき: set <キー> <値> / button add [<kind>] <中身> / button remove <id> /
    /// button move <id> <先> / preset <名前> / reset
    rest: Vec<String>,
    /// ui の button add: ボタンの表示名（16 桁まで。省くと中身から決める）
    #[arg(long)]
    label: Option<String>,
    /// ui の button add: 押すキー（数字か英小文字 1 字。省くと空いている英字）
    #[arg(long)]
    hotkey: Option<String>,
    /// ui の button add: ボタンの id（省くと中身から決める）
    #[arg(long)]
    id: Option<String>,
    /// install / uninstall のとき: どの設定 dir に何をするかだけを出して書かない
    #[arg(long)]
    dry_run: bool,
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
    /// 復帰させるペインの ID（tako backgrounded で確認。--tab と排他）
    pane: Option<u64>,
    /// 復帰させる退避タブの ID（分割ツリーごと元の位置へ戻す。--pane と排他）
    #[arg(long, conflicts_with_all = ["pane", "target", "direction"])]
    tab: Option<u64>,
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
    /// 各エージェント CLI が使えるモデル一覧を表示する（#1002）
    Models {
        /// 対象の系統（claude / codex / agy。省略で全系統）
        #[arg(long)]
        agent: Option<String>,
        /// 出力を JSON にする（MCP tako_setup_models と同一ペイロード）
        #[arg(long)]
        json: bool,
    },
    /// エージェント CLI の導入状況を確認・実行する（#868 / #1057 / #989）
    Bootstrap {
        /// status（既定・読み取り専用）/ status-all / install / path / undo-path / handoff
        action: Option<String>,
        /// 対象の系統（claude / codex / agy。省略で claude）
        #[arg(long)]
        agent: Option<String>,
        /// install で実行せず「何をどこに入れるか」だけ出す
        #[arg(long)]
        dry_run: bool,
        /// 出力を JSON にする（MCP tako_setup_bootstrap と同一ペイロード）
        #[arg(long)]
        json: bool,
    },
    /// 任意依存ツール（tmux / git / tailscale）の状態確認とその場導入（#88 / #1057）
    Deps {
        /// status（既定・読み取り専用）/ install
        action: Option<String>,
        /// install の対象を 1 件に絞る（省略で未導入のものすべて）
        #[arg(long)]
        dep: Option<String>,
        /// install で実行せず何を入れるかだけ出す
        #[arg(long)]
        dry_run: bool,
        /// 出力を JSON にする（MCP tako_setup_deps と同一ペイロード）
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
    /// カレントディレクトリの .mcp.json に書き込む（claude のみ）
    #[arg(long)]
    project: bool,
    /// 登録対象のエージェント（省略時は claude + 導入済みの codex / agy すべて）
    #[arg(long, value_parser = ["claude", "codex", "agy"])]
    agent: Option<String>,
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
    /// ウィンドウを動かす（#1442。座標は置き先ディスプレイの左上が原点）
    Move {
        /// 左端（ディスプレイ内の座標）
        x: f32,
        /// 上端（ディスプレイ内の座標）
        y: f32,
        /// 対象ウィンドウ ID（省略時はアクティブウィンドウ）
        #[arg(long)]
        window: Option<u64>,
    },
    /// ウィンドウの寸法を変える（#1442。位置は動かさない）
    Resize {
        /// 幅
        width: f32,
        /// 高さ
        height: f32,
        /// 対象ウィンドウ ID（省略時はアクティブウィンドウ）
        #[arg(long)]
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
    // #1811: 製品の入口を通ったと宣言する（**必ず 1 文目**。`cli_main` のスレッドより前）。
    // これが無いプロセスは置き場に関係なくテスト扱い = 本番の data dir / ホーム配下へ書かない
    tako_core::paths::mark_product_process();
    // #2367: 器（tmux）の中のペインの子を起動時の fd 上限へ戻す段（製品が段に使うのは GUI 自身。
    // CLI でも同じ段が動くのは、実 tmux のテストが段を実物の実行ファイルで通すため）
    tako_core::platform::fd_limit::exec_container_child_if_requested();
    // #2582: remote daemon（`tako remote serve`）はペインの中の CLI から起こすと、そのペインの env
    // （TAKO_PANE_ID・接続・claude のセッションの印・GIT_EDITOR=true ほか）を持ったまま常駐する。GUI（#2570）と
    // 同じ正本で外し、unix は exec し直す（スレッドを起こす前 = env の書き換えが別スレッドの読みと競らない）。
    // 普段の CLI は呼んだペインの env を使うので外さない
    if tako_control::remote::is_serve_invocation(std::env::args_os().skip(1)) {
        tako_control::remote::strip_inherited_env_at_startup();
    }
    // #1758 / #1805: 読み手が先に閉じたパイプへの出力で panic しない（標準出力 → 静かに 0 /
    // 標準エラー → 行を捨てて続ける）。tako-core / tako-control の出力も同じ口を通るので、
    // CLI の経路で走るライブラリ側の `eprintln!` にも効く（`cli_main` のスレッドより前）
    tako_core::stdio::stop_quietly_on_closed_stdout();
    tako_core::stdio::drop_lines_on_closed_stderr();
    // #2151: git の場所の解決（`tako open-in repo` 等が CLI のプロセスで git を起こす）が
    // ログインシェル等を締め切りで打ち切ったら、理由を persist.log へ
    tako_core::bin_resolve::set_notice_sink(tako_control::diag::persist_log);
    // #2021: MCP の stdio ブリッジ（claude 1 本に 1 本常駐）は clap を通さない。
    // コマンド定義の木を組むだけで malloc が約 2 MB の山を作り、解放しても OS へ返らず
    // 常駐の間ずっと footprint に残るため。`TAKO_2021_LEGACY=1` は clap を通る旧経路
    let bridge =
        !mcp_bridge::legacy() && mcp_bridge::is_mcp_serve_invocation(std::env::args_os().skip(1));
    if bridge {
        // macOS の .app なら専用の小さな `tako-mcp` へ exec で引き渡す（スレッドを起こす前）。
        // 戻ってきたら引き渡さなかったので、このプロセスでブリッジを続ける
        mcp_bridge::handoff_to_small_bridge();
    }
    let body: fn() -> ExitCode = if bridge { mcp_bridge::run } else { cli_main };
    // Windows のメインスレッドは既定 1MB スタックで、コマンド定義（clap の巨大ツリー）の
    // 構築だけで debug ビルドが溢れる（macOS / Linux は 8MB）。**main 由来の既存バグ**で、
    // `origin/main` の `tako.exe list` も実機で `has overflowed its stack` で落ちる
    // （スライス 3 の IPC 検証はユニットテストだったため踏まなかった）。
    // 本体を十分なスタックのワーカースレッドで実行する（プラットフォーム共通・挙動不変）
    let joined = std::thread::Builder::new()
        .name("tako-main".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(body)
        .expect("メインスレッドを起動できない")
        .join();
    // 標準出力の読み手が先に閉じた（`tako … | head -1`）のは失敗ではないので静かに 0（#1758）
    if matches!(&joined, Err(payload) if payload.is::<tako_core::stdio::StdoutClosed>()) {
        return ExitCode::SUCCESS;
    }
    joined.expect("メインスレッドが異常終了した")
}

fn cli_main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        // ここへ来るのは `TAKO_2021_LEGACY=1`（#2021 の前の形 = カタログを常駐プロセスで組む）だけ。
        // 普段の `tako mcp serve` は main の早い分岐で `mcp_bridge::run` へ入る
        Command::Mcp(McpCommand::Serve) => mcp_bridge::serve(mcp_bridge::Catalog::InProcess),
        Command::Setup(ref args) => {
            // setup も表示言語を settings.json から解決する（#435。移行の説明文など
            // Note ベースの文言がここを通るので、設定が ja なら日本語で出る）。
            // ファイルに触らない読みにする: 読めない settings.json を移行の表示より先に
            // 退避へ移すと、画面の移行の行から消える（#2238）
            tako_core::i18n::set_lang(tako_control::settings::peek_lang_setting().resolve());
            if let Some(SetupCommand::Models { ref agent, json }) = args.command {
                setup::run_models(agent.as_deref(), json)
            } else if let Some(SetupCommand::Bootstrap {
                ref action,
                ref agent,
                dry_run,
                json,
            }) = args.command
            {
                setup::run_bootstrap(action.as_deref(), agent.as_deref(), dry_run, json)
            } else if let Some(SetupCommand::Deps {
                ref action,
                ref dep,
                dry_run,
                json,
            }) = args.command
            {
                setup::run_deps(action.as_deref(), dep.as_deref(), dry_run, json)
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
        // #2470: レジストリ・プロファイル・会話の記録を読んでこのシェルで起動する（GUI を通さない）
        Command::Orchestrator(OrchestratorCommand::Resume { ref worker_id }) => {
            orchestrator_resume(worker_id)
        }
        Command::Orchestrator(OrchestratorCommand::Projects(ref sub)) => {
            orchestrator_projects_cli(sub)
        }
        Command::Orchestrator(OrchestratorCommand::Adopt { ref name, pane }) => {
            // ペインの role ラベルを書き換える = GUI の状態を触るので IPC 経由
            let pane = pane.or_else(caller_pane);
            let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
            send_request(Request::OrchestratorAdopt {
                name: name.clone(),
                pane,
                caller_role,
                caller_pid: Some(std::process::id()),
            })
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Profiles(ref sub)) => {
            // #1068: 応答に理由文（`Note`）が載るのでここで表示言語を決める。
            // profiles はローカル処理（dispatch を直呼び）なので、GUI 側の初期化に
            // 相乗りできない = 呼ぶ側が明示しないと静的既定の英語で凍る
            tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
            orchestrator_profiles_cli(sub)
        }
        Command::Orchestrator(OrchestratorCommand::SelfInfo { pane }) => {
            // #1516: `--pane N` は「そのペインは何か」という名指しなので、呼び出し元の
            // 手掛かり（env pane / role / pid）を混ぜない。判断は正本 1 本に閉じる
            let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
            send_request(Request::orchestrator_self(
                pane,
                caller_pane(),
                caller_role,
                Some(std::process::id()),
            ))
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
                                    let lang = cli_lang();
                                    eprintln!(
                                        "{}: {}",
                                        ReplyNote::ErrorLabel.text_in(lang),
                                        Reason::StdinReadFailed(e.to_string()).text_in(lang)
                                    );
                                    return ExitCode::FAILURE;
                                }
                            }
                        }
                    };
                    ("write", project.clone(), profile.clone(), Some(body))
                }
                HandoffsCommand::Migrate { profile } => ("migrate", None, profile.clone(), None),
            };
            // 手元で dispatch を直呼びする。理由の文（#2365）を前置きと同じ表示言語で組む
            init_local_lang();
            tako_control::dispatch::dispatch_orchestrator_handoff_files(
                action,
                project.as_deref(),
                profile.as_deref(),
                content.as_deref(),
            )
            .map_err(local_dispatch_err)
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Layout {
            ref policy,
            master_ratio,
            ref algorithm,
            min_worker_cols,
            auto_shrink_font,
            min_worker_font_scale,
        }) => {
            // config.yaml のみの操作のため IPC 不要。dispatch と同一関数を共用する
            // （MCP `tako_orchestrator_layout` と 1:1。二重実装を作らない）
            init_local_lang();
            tako_control::dispatch_orchestrator_layout(
                policy.as_deref(),
                master_ratio,
                algorithm.as_deref(),
                min_worker_cols,
                auto_shrink_font,
                min_worker_font_scale,
            )
            .map_err(local_dispatch_err)
            .map(|result| println!("{}", pretty_json(&result)))
        }
        Command::Orchestrator(OrchestratorCommand::Accounts(ref sub)) => {
            // accounts.yaml のみの操作のため IPC 不要。dispatch と同一関数を共用する
            // （MCP `tako_orchestrator_accounts` と 1:1。二重実装を作らない。#548）
            init_local_lang();
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
            .map_err(local_dispatch_err)
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
        Command::Remote(RemoteCommand::Shortcuts { command }) => remote_shortcuts(command),
        Command::Remote(RemoteCommand::Setup {
            yes,
            answers,
            tailscale,
        }) => remote_setup_cli(yes, answers.as_deref(), tailscale.as_deref()),
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
        Command::ContextBudget(ref args) => context_budget_local(args),
        // #1441: **IPC が立っていないときこそ答えが要る**診断なので、届かなければ
        // ローカルの受け口診断へ落ちる（run() を通すと「接続情報が無い」で終わる）
        Command::CheckHealth(ref args) => check_health_cli(args.json),
        // #1994: コマンド付きは順番待ちで走らせる（終了コードは子と同じ）
        Command::Queue(ref args) if matches!(args.command, Some(QueueCommand::Run(_))) => {
            return queue_run_cli(args);
        }
        Command::Queue(ref args) => queue_cli(args),
        // 残骸の掃除もローカル処理（IPC 不要）。**GUI が動いていなくても掃ける**ことが
        // 本質（掃除の対象は一時ディレクトリで、GUI の状態とは無関係）
        Command::TestResidue(ref args) => test_residue_local(args),
        // 手順書（#1154）はバイナリ埋め込みの静的な本文 + プロファイル読みだけなので
        // ローカル処理。**GUI が動いていなくても引ける**ことが本質（platform と同じ扱い）
        Command::Orchestrator(OrchestratorCommand::Guide {
            ref topic,
            ref profile,
            json,
        }) => orchestrator_guide_local(topic.as_deref(), profile.as_deref(), json),
        Command::AgentSupport(ref args) => agent_support_local(args),
        // #2200 / #2201: 観測の要約と使用量の内訳は data dir と会話記録を読むだけなのでローカル処理。
        // **GUI が動いていなくても読める**（索引を追いつかせてから答える）。実体は dispatch
        // （MCP `tako_sessions` の action=usage）と共通の tako_control::usage::usage_payload
        Command::Usage(ref args) => usage_local(args),
        // GUI を必要としないローカル処理（platform と同じ扱い）。
        // 実体は dispatch と共通の tako_control::shell_integration::run
        Command::ShellIntegration(ref args) => shell_integration_local(args),
        // #1960: 画面の UI 設定（ui.json）は tako の data dir のファイルなのでローカル処理。
        // **GUI が動いていなくても変えられる**（setup の途中・GUI を起動する前）。実体は
        // dispatch（MCP `tako_mod` の action=ui）と共通の tako_control::claude_mod_ui::run
        Command::Mod(ref args) if args.action.as_deref() == Some("ui") => mod_ui_local(args),
        // #1959: 設定 dir の skills/tako への出し入れ。**GUI が動いていなくても外せる**ことが
        // 本質（tako を消す前の後片付け）。実体は dispatch（MCP）と共通の claude_mod_install
        Command::Mod(ref args)
            if matches!(args.action.as_deref(), Some("install" | "uninstall")) =>
        {
            mod_install_local(args)
        }
        Command::Config(ref args) => config_share_local(args),
        // run-interactive --wait は起動 + ポーリングの合成。終了コードは結末から決まる
        // （上限で打ち切ったら 124 = `timeout(1)` と同じ。#1778）
        Command::RunInteractive(ref args) if args.wait => {
            return wait_exit_code(run_interactive_wait(&cli.command));
        }
        // run --wait / --list は合成処理
        Command::Run(ref args) if args.wait => return wait_exit_code(run_wait(&cli.command)),
        Command::Run(ref args) if args.list => run_list(&cli.command),
        // #1860 / #1895: 複数ならまとめた 1 要求（1 つのジョブ）。1 件でも断られたら終了コード 1
        Command::File(FileCommand::Copy { ref paths }) => file_copy_cli(paths),
        // #1867: まとめた操作は 1 件でも断られたら終了コード 1（結果は出してから = mv / rm と同じ）
        Command::File(FileCommand::Move { .. } | FileCommand::Trash { .. }) => {
            file_many_cli(&cli.command)
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

/// MCP セットアップ（アプリ未起動でも動作）。書き先の正本は `dispatch::mcp_target_path`
/// （claude = `~/.claude.json` / `--project` = `<cwd>/.mcp.json`。codex / agy は各 CLI の `mcp add`）
fn setup_mcp_local(args: &SetupMcpArgs) -> Result<(), String> {
    // dispatch の `setup_mcp_agents` は理由の文を組んだ時点の表示言語で文字列にする（#2365）。
    // 呼ぶ前に決めておかないと、日本語表示の人へ理由の文だけ英語（静的既定）で出る
    init_local_lang();
    let (scope, scope_label) = if args.project {
        let cwd = std::env::current_dir()
            .map_err(|e| front_err(Reason::CurrentDirFailed(e.to_string())))?;
        (tako_control::dispatch::McpScope::Project(cwd), "project")
    } else {
        (tako_control::dispatch::McpScope::User, "global")
    };
    // GUI が動いていなくても登録できる必要があるのでローカル処理（IPC を張らない）。
    // 対象の選び方・分類済みエラーは dispatch 側の 1 実装（MCP / CLI で同一）
    let resp = tako_control::dispatch::setup_mcp_agents(args.agent.as_deref(), &scope, scope_label)
        .map_err(local_dispatch_err)?;
    print_setup_mcp_report(&resp);
    println!("{}", pretty_json(&resp));
    Ok(())
}

/// `setup-mcp` の人間向けサマリを stderr へ出す（機械可読な JSON は stdout）
fn print_setup_mcp_report(resp: &serde_json::Value) {
    let Some(agents) = resp.get("agents").and_then(|a| a.as_array()) else {
        return;
    };
    for entry in agents {
        let name = entry.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
        if entry.get("skipped").and_then(|s| s.as_bool()) == Some(true) {
            let msg = entry.get("message").and_then(|m| m.as_str()).unwrap_or("");
            eprintln!("  [skip] {name}: {msg}");
            continue;
        }
        let target = entry
            .get("target_path")
            .and_then(|t| t.as_str())
            .unwrap_or("(不明)");
        if entry.get("repaired").and_then(|r| r.as_bool()) == Some(true) {
            let old = entry
                .get("old_command")
                .and_then(|o| o.as_str())
                .unwrap_or("(不明)");
            let new = entry.get("command").and_then(|c| c.as_str()).unwrap_or("");
            eprintln!("  [修復] {name}: 登録パスが消失していたため付け替えました（{target}）");
            eprintln!("         旧: {old}");
            eprintln!("         新: {new}");
        } else if entry.get("configured").and_then(|c| c.as_bool()) == Some(true) {
            eprintln!("  [設定] {name}: 追加しました（{target}）");
        } else {
            eprintln!("  [OK] {name}: 既に設定されています（{target}）");
        }
        if entry.get("legacy_cleaned").and_then(|l| l.as_bool()) == Some(true) {
            eprintln!("         旧 ~/.claude/settings.json の無効な MCP 設定を除去しました");
        }
    }
}

/// MCP 登録パスの存在を確認し、不在なら警告を出す（master/solo 起動前のガード）
fn check_mcp_health_warning(lang: tako_core::i18n::Lang) {
    let home = match tako_core::paths::home_dir() {
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
        eprintln!(
            "{}",
            ReplyNote::McpPathMissing(cmd.to_string()).text_in(lang)
        );
        eprintln!();
    }
}

/// master / solo 起動時の env とアカウントの可視化（Issue #500 / #547）。
/// 値はマスクし、キー名と解決後の config dir だけを出す。
/// アカウント解決の失敗は build_master_cmd が起動前に Err にするので、ここは表示だけ
fn print_master_env(
    profile: &tako_control::orchestrator::Profile,
    profile_name: &str,
    lang: tako_core::i18n::Lang,
) {
    use tako_control::orchestrator;
    if !profile.env.is_empty() {
        let keys: Vec<&str> = profile.env.keys().map(|k| k.as_str()).collect();
        eprintln!("env: {}", keys.join(", "));
    }
    match profile.resolve_master_account_typed() {
        // #547: master_account があればそちらが CLAUDE_CONFIG_DIR の正
        Ok(Some(account)) => {
            let line = match account.config_dir.path() {
                Some(dir) => ReplyNote::MasterAccount {
                    name: account.name.clone(),
                    config_dir: dir.to_string(),
                },
                None => ReplyNote::MasterAccountDefault(account.name.clone()),
            };
            eprintln!("{}", line.text_in(lang));
        }
        // アカウント未指定: プロファイル env の config dir を従来どおり出す
        Ok(None) => {
            if let Some(config_dir) = profile.env.get("CLAUDE_CONFIG_DIR") {
                eprintln!("config dir: {}", orchestrator::expand_tilde(config_dir));
            }
        }
        Err(e) => eprintln!("warning: {}", e.text_in(lang)),
    }
    print_sandbox_state(profile, profile_name, lang);
    print_remote_control_state(profile, profile_name, lang);
}

/// Remote Control の状態を 1 行で出す（Issue #1068。#981 の
/// `print_sandbox_state` と同じ思想 = **有効でも無効でも出す**）。
///
/// 無効のときに黙っていると「スマホから見えない」の切り分けができない。
/// 逆に opt-in しているのに環境が不適格なときは、**なぜ付かなかったか**を出す
/// （フラグを付けないことで claude の即死は避けているが、理由が要る）
fn print_remote_control_state(
    profile: &tako_control::orchestrator::Profile,
    profile_name: &str,
    lang: tako_core::i18n::Lang,
) {
    use tako_control::{claude_remote, orchestrator};
    // claude 以外には相当する仕組みが無い（マトリクスの宣言が正）。
    // opt-in していない限り黙る = codex master の画面に無関係な行を増やさない
    let is_claude = matches!(
        profile.resolve_master_agent(),
        Ok(orchestrator::WorkerAgent::Claude)
    );
    if !is_claude && !profile.remote_control_enabled() {
        return;
    }
    let decision = match orchestrator::master_remote_control_decision(profile) {
        Ok(d) => d,
        // アカウント / master_agent の解決失敗は print_master_env が既に出している
        Err(_) => return,
    };
    eprintln!("{}", claude_remote::status_line(&decision));
    if decision.flag.is_none() && decision.blocked.is_none() && is_claude {
        let hint =
            ReplyNote::RemoteControlEnableHint(claude_remote::enable_hint_command(profile_name));
        eprintln!("{}", hint.text_in(lang));
    }
}

/// codex 系 master / solo のサンドボックス状態を 1 行で出す（Issue #981）。
/// **外れているときも外れていないときも出す**（承認プロンプトで止まったとき、
/// 画面に理由が残っていなければ「黙って動かない」に見えるため）
fn print_sandbox_state(
    profile: &tako_control::orchestrator::Profile,
    profile_name: &str,
    lang: tako_core::i18n::Lang,
) {
    use tako_control::orchestrator;
    // codex 以外はサンドボックスの概念が無い（claude / agy にこのフラグは存在しない）
    if !matches!(
        profile.resolve_master_agent(),
        Ok(orchestrator::WorkerAgent::Codex)
    ) {
        return;
    }
    let enabled = profile.bypass_sandbox || orchestrator::legacy_unconditional_bypass();
    eprintln!(
        "{}",
        orchestrator::sandbox_bypass_line(enabled).text_in(lang)
    );
    if !enabled {
        let hint =
            ReplyNote::SandboxBypassHint(orchestrator::sandbox_bypass_hint_command(profile_name));
        eprintln!("{}", hint.text_in(lang));
    }
}

/// `tako master [-profile]` — 新タブで claude をマスター system prompt 付きで起動する。
/// `-<名前>` でプロファイルを指定、引数なしは default、旧形式（suffix のみ）も後方互換で動作
/// 突然死した worker をこのシェルで戻す（#390 / #2470）。
///
/// 起動コマンドは spawn と同じ組み立て（`resume_launch::registry_resume_launch` = GUI の復元・
/// `tako sessions resume` と同じ 1 実装）で、**打った時点で**組む。worker_status 等の照会は案内の
/// 1 行（`tako orchestrator resume <worker_id>`）を返すだけでファイルを読まない。
/// 案内は死んだペインのシェルへ送られる（supervisor の自動復旧も同じ）ので、新しいペインを
/// 作らず**このプロセスを起動コマンドに置き換える**（ペイン番号・レジストリの追跡がそのまま続く）。
/// A/B は `TAKO_2470_LEGACY=1`（修正前の案内 `cd <cwd> && claude … --resume <id>` をそのまま走らせる）
fn orchestrator_resume(worker_id: &str) -> Result<(), String> {
    use tako_control::orchestrator::registry;

    // 断り・案内・型の口の文（移行の知らせ・レジストリ・会話の再開）を表示言語で組む（#2365）
    init_local_lang();
    let lang = tako_core::i18n::lang();
    // 旧形式の設定ファイルを検知して直す（#916。プロファイルを読む前に）
    if let Some(notice) = tako_control::migrations::ensure_migrated() {
        eprintln!("{} {notice}", ReplyNote::InfoLabel.text_in(lang));
    }
    let reg = registry::WorkerRegistry::load_typed().map_err(|e| e.text_in(lang))?;
    let entry = reg
        .workers
        .get(worker_id)
        .ok_or_else(|| Reason::WorkerNotInRegistry(worker_id.to_string()).text_in(lang))?;
    let sid: String = entry
        .session_id
        .as_deref()
        .unwrap_or("-")
        .chars()
        .take(8)
        .collect();
    let (command, cwd) = if registry::legacy_2470() {
        // 修正前の案内（cd を含む 1 行）。比較のために同じ走らせ方を通す
        let cmd = registry::legacy_resume_command(entry)
            .ok_or_else(|| Reason::LegacyResumeUnbuildable.text_in(lang))?;
        (cmd, None)
    } else {
        let launch = tako_control::resume_launch::registry_resume_launch(entry)
            .map_err(|e| e.text_in(lang))?;
        for w in &launch.warnings {
            eprintln!("{}: {w}", ReplyNote::WarningLabel.text_in(lang));
        }
        // MCP の絞り込みの設定ファイルを書く（spawn と同じ。下見では書かない作法）
        launch.materialize_typed().map_err(|e| e.text_in(lang))?;
        let resuming = ReplyNote::ResumingWorker {
            worker: worker_id.to_string(),
            session: sid,
            profile: launch.profile.as_deref().unwrap_or("-").to_string(),
        };
        eprintln!("{}", resuming.text_in(lang));
        (launch.command, entry.cwd.clone().filter(|c| !c.is_empty()))
    };
    if let Some(cwd) = cwd {
        std::env::set_current_dir(&cwd).map_err(|e| {
            Reason::WorkerCwdUnreachable {
                cwd: cwd.clone(),
                error: e.to_string(),
            }
            .text_in(lang)
        })?;
    }
    run_launch_line(&command)
}

/// 組み立てた起動コマンドの 1 行（ペインのシェルへ打ち込むのと同じ文字列）をこの端末で走らせる（#2470）。
///
/// 方言は組み立てと同じ `launch_cmd::launch_dialect`。POSIX は `/bin/sh -c` で**このプロセスを置き換え**
/// （終了コード・シグナル・端末はそのままエージェントへ渡る）、PowerShell は子として走らせて終了コードを返す
fn run_launch_line(command: &str) -> Result<(), String> {
    use tako_control::launch_cmd::{launch_dialect, ShellDialect};
    let mut cmd = match launch_dialect() {
        ShellDialect::Posix => {
            let sh = if cfg!(unix) { "/bin/sh" } else { "sh" };
            let mut c = std::process::Command::new(sh);
            c.arg("-c").arg(command);
            c
        }
        ShellDialect::PowerShell => {
            let program = tako_core::platform::shell::default_shell()
                .map(|s| s.program)
                .unwrap_or_else(|| "powershell".into());
            let mut c = std::process::Command::new(program);
            c.args(["-NoLogo", "-NoProfile", "-Command", command]);
            c
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(front_err(Reason::LaunchLineFailed(err.to_string())))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|e| front_err(Reason::LaunchLineFailed(e.to_string())))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn orchestrator_master(arg: Option<&str>, use_tab: bool) -> Result<(), String> {
    use tako_control::orchestrator;

    // 起動の流れの断り・行・型の口の文と、移行の知らせ・`Note` 由来の文言（サンドボックス状態 #981 /
    // CLI 不在の案内 #983）を表示言語で組む。**移行より前に**決める（#2365。以前は cwd 移動の後に
    // 決めていたので、移行の知らせと断りが設定と無関係に英語 / 日本語で固定されていた）
    init_local_lang();
    let lang = tako_core::i18n::lang();

    orchestrator::ensure_defaults_typed()
        .map_err(|e| Reason::LaunchSetupFailed(e.text_in(lang)).text_in(lang))?;

    check_mcp_health_warning(lang);

    // 旧形式の設定ファイルを master 起動のたびに検知して直す（#916）
    if let Some(notice) = tako_control::migrations::ensure_migrated() {
        eprintln!("{} {notice}", ReplyNote::InfoLabel.text_in(lang));
        eprintln!();
    }

    let (profile_name, suffix) = match arg {
        None => ("default", None),
        Some(s) if s.starts_with('-') => {
            // 切り出し安全: 直前の starts_with('-') で先頭 1 バイトは ASCII の `-`
            let name = &s[1..];
            if name.is_empty() {
                return Err(Reason::EmptyProfileName("tako master -2").text_in(lang));
            }
            (name, Some(name))
        }
        Some(s) => ("default", Some(s)),
    };

    // 読み込みはスマホ・返答の配送と同じ 1 実装（#2227）。読めない default は組み込み既定で
    // 立てるが黙らない（`-<key>` が #1825 で作られていなければ理由と直し方で止まる）
    let orchestrator::master_launch::LaunchProfile {
        profile,
        warning: load_warning,
    } = orchestrator::master_launch::load_profile_in(profile_name, lang)?;
    // 理由は表示言語で（`警告:` 行と本人の system prompt の両方。persist.log は日本語で書き済み。#2365）
    let load_warning = load_warning.map(|w| w.text_in(lang));
    if let Some(w) = &load_warning {
        eprintln!("{}: {w}", ReplyNote::WarningLabel.text_in(lang));
    }

    // env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env_typed().map_err(|e| e.text_in(lang))?;

    // Part 5: cwd 解決（存在しなければ診断つきエラー）
    let resolved_cwd = profile.resolve_cwd_typed().map_err(|e| e.text_in(lang))?;

    // Part 7: projects の key が projects.yaml に存在するか検証（起動時エラー）
    profile
        .validate_projects_typed()
        .map_err(|e| e.text_in(lang))?;

    // Part 5: cwd が指定されていれば、そのディレクトリへ移動
    if let Some(ref cwd) = resolved_cwd {
        std::env::set_current_dir(cwd).map_err(|e| {
            Reason::ProfileCwdUnreachable {
                cwd: cwd.display().to_string(),
                error: e.to_string(),
            }
            .text_in(lang)
        })?;
    }

    let master_agent = profile
        .resolve_master_agent_typed()
        .map_err(|e| e.text_in(lang))?;
    // #983: CLI が無ければタブを作る前に理由 + 次の一手で落ちる
    // （組み立てたコマンドをそのまま流すと、新しいタブに command not found が出るだけ）
    orchestrator::agent_cli::preflight(master_agent).map_err(|e| e.message_in(lang))?;

    if profile.master_agent_is_claude() {
        if let Some(warning) = profile
            .model
            .as_deref()
            .and_then(|m| orchestrator::one_m_model_warning_in(m, "master", lang))
        {
            eprintln!("{warning}");
        }
    }
    if let Some(warning) = profile
        .resolve_worker_model()
        .filter(|m| Some(*m) != profile.model.as_deref())
        .and_then(|m| orchestrator::one_m_model_warning_in(m, "worker", lang))
    {
        eprintln!("{warning}");
    }

    // #1139: 起動時ロードが予算を超えていたら 1 行だけ出す（超過が無ければ黙る）。
    // ここは cwd 移動（Part 5）より後なので、これから master が読むフォルダで測れる
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(line) = tako_control::context_budget::startup_line(&cwd, Some(profile_name)) {
            eprintln!("{line}");
        }
    }

    // #2227: 上の `警告:` 行は claude の全画面 TUI に隠れうるので、master 本人にも渡す
    let prompt_content = orchestrator::master_launch::with_startup_warning(
        profile.build_system_prompt(profile_name),
        load_warning.as_deref(),
    );
    let dir = orchestrator::config_dir().ok_or_else(|| Reason::HomeDirUnknown.text_in(lang))?;
    let prompt_path = dir.join(format!("_system_prompt_{profile_name}.md"));
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| Reason::SystemPromptWriteFailed(e.to_string()).text_in(lang))?;

    // #2120: タブ名は管轄プロジェクトの名前を**自動名で**付ける（汎用 master は触らない）。
    // 役割名（`master` / `master-<名前>`）を手動名で付けると、master 自身の
    // `tako_rename_tab(source: "auto")` が全部弾かれてタブ名が固定される
    let tab_naming = orchestrator::master_launch::tab_naming(suffix.unwrap_or("default"), &profile);

    let role = match suffix {
        Some(s) => format!("orchestrator-master:{s}"),
        None => "orchestrator-master".into(),
    };
    let role_env = match suffix {
        Some(s) => format!("master:{s}"),
        None => "master".into(),
    };

    let tako_bin = tako_control::dispatch::resolve_tako_binary();
    let master_cmd =
        orchestrator::build_master_cmd_typed(&role_env, &profile, &prompt_path, &tako_bin)
            .map_err(|e| e.text_in(lang))?;

    // インライン起動（既定）: 現在のペインでコマンドを実行（新タブを作らない。#264）
    // --tab 指定時 / 呼び出し元ペインが解決できないとき（#567）: 新タブ起動
    let target = resolve_launch_target(
        tab_naming.as_ref(),
        None,
        use_tab,
        &master_cmd_hint(profile_name),
        lang,
    )?;
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
        paste_guard: None,
    })?;

    let launched = ReplyNote::MasterLaunched(launch_location(&target, lang));
    eprintln!("{}", launched.text_in(lang));
    eprintln!(
        "{}",
        launch_profile_line(&profile, profile_name, master_agent, lang)
    );
    // #1140: 本人のペインへ配った利用上限後の自動復帰。適用は role を貼る
    // dispatch 側（`Request::Title`）が行うので、ここは何が起きたかを言うだけ
    if orchestrator::resolve_master_limit_resume(&profile) {
        eprintln!("{}", ReplyNote::LimitResumeOn.text_in(lang));
    }
    let policy_line = match profile.worker_model_policy {
        orchestrator::WorkerModelPolicy::Inherit if profile.master_agent_is_claude() => {
            ReplyNote::WorkerPolicyInherit {
                model: profile.model_label().to_string(),
                effort: profile.effort.clone(),
            }
        }
        orchestrator::WorkerModelPolicy::Inherit => ReplyNote::WorkerPolicyInheritNonClaude {
            agent: master_agent.as_str().to_string(),
            model: profile.worker_model_label().to_string(),
            effort: profile.resolve_worker_effort().to_string(),
        },
        orchestrator::WorkerModelPolicy::Fixed => ReplyNote::WorkerPolicyFixed {
            model: profile.worker_model_label().to_string(),
            effort: profile.resolve_worker_effort().to_string(),
        },
        orchestrator::WorkerModelPolicy::Delegate => ReplyNote::WorkerPolicyDelegate,
    };
    eprintln!("{}", policy_line.text_in(lang));
    // Part 4: env の可視化（キー名のみ。Issue #500 / #547）
    print_master_env(&profile, profile_name, lang);
    if let Some(ref projects) = profile.projects {
        let line = ReplyNote::ProjectsRestriction(projects.join(", "));
        eprintln!("{}", line.text_in(lang));
    }
    // Part 5: cwd 表示
    if let Some(ref cwd) = resolved_cwd {
        eprintln!("cwd: {}", cwd.display());
    }
    eprintln!("system prompt: {}", prompt_path.display());

    // Part 6: master 起動後にファイルツリーへ cwd と projects のフォルダを自動追加
    let mut tree_folders: Vec<String> = Vec::new();
    if let Some(ref cwd) = resolved_cwd {
        // 解決は境界（B26）を通す（TreeFolder へ渡って保存される値。#970）
        if let Ok(canonical) = tako_core::platform::path::canonicalize(cwd) {
            tree_folders.push(canonical.display().to_string());
        }
    }
    if let Some(ref project_keys) = profile.projects {
        if let Ok(config) = orchestrator::ProjectsConfig::load() {
            for key in project_keys {
                if let Ok(cwd) = config.resolve_cwd(key) {
                    let path = std::path::PathBuf::from(&cwd);
                    if let Ok(canonical) = tako_core::platform::path::canonicalize(&path) {
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
            limit: None,
        });
    }
    if !tree_folders.is_empty() {
        let names = tree_folders
            .iter()
            .map(|p| {
                std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("{}", ReplyNote::TreeFoldersAdded(names).text_in(lang));
    }

    Ok(())
}

/// 起動した master / solo の `プロファイル: …（エージェント・モデル・effort）` の行（#2365）
fn launch_profile_line(
    profile: &tako_control::orchestrator::Profile,
    profile_name: &str,
    agent: tako_control::orchestrator::WorkerAgent,
    lang: tako_core::i18n::Lang,
) -> String {
    ReplyNote::LaunchProfile {
        profile: profile_name.to_string(),
        agent: agent.as_str().to_string(),
        model: profile.master_model_label(),
        effort: profile.effort.clone(),
    }
    .text_in(lang)
}

/// `tako solo [-profile]` — solo system prompt 付きで claude / codex を起動する。
/// 既定はインライン（現在のペインで起動）、--tab で新タブ起動（#264）。
fn orchestrator_solo(arg: Option<&str>, use_tab: bool) -> Result<(), String> {
    use tako_control::orchestrator;

    // 起動の流れの断り・行・型の口の文と `Note` 由来の文言を表示言語で組む（master と同じ。#2365）
    init_local_lang();
    let lang = tako_core::i18n::lang();

    orchestrator::ensure_solo_defaults_typed()
        .map_err(|e| Reason::LaunchSetupFailed(e.text_in(lang)).text_in(lang))?;

    check_mcp_health_warning(lang);

    let (profile_name, suffix) = match arg {
        None => ("default", None),
        Some(s) if s.starts_with('-') => {
            // 切り出し安全: 直前の starts_with('-') で先頭 1 バイトは ASCII の `-`
            let name = &s[1..];
            if name.is_empty() {
                return Err(Reason::EmptyProfileName("tako solo -fast").text_in(lang));
            }
            (name, Some(name))
        }
        Some(s) => ("default", Some(s)),
    };

    // 読み込みは master と同じ形（#2233）。読めない default は solo の組み込み既定で立てるが
    // 黙らない（`警告:` 行・persist.log・solo 本人の system prompt。中身は退避）
    let orchestrator::master_launch::LaunchProfile {
        profile,
        warning: load_warning,
    } = orchestrator::master_launch::load_solo_in(profile_name, lang)?;
    // 理由は表示言語で（`警告:` 行と本人の system prompt の両方。persist.log は日本語で書き済み。#2365）
    let load_warning = load_warning.map(|w| w.text_in(lang));
    if let Some(w) = &load_warning {
        eprintln!("{}: {w}", ReplyNote::WarningLabel.text_in(lang));
    }

    // env 検証（内部変数の上書き拒否。Issue #500）
    profile.validate_env_typed().map_err(|e| e.text_in(lang))?;

    // Part 5: cwd 解決
    let resolved_cwd = profile.resolve_cwd_typed().map_err(|e| e.text_in(lang))?;
    if let Some(ref cwd) = resolved_cwd {
        std::env::set_current_dir(cwd).map_err(|e| {
            Reason::ProfileCwdUnreachable {
                cwd: cwd.display().to_string(),
                error: e.to_string(),
            }
            .text_in(lang)
        })?;
    }

    let solo_agent = profile
        .resolve_master_agent_typed()
        .map_err(|e| e.text_in(lang))?;
    // #983: master と同じ理由で、起動前に CLI の実在を確かめる
    orchestrator::agent_cli::preflight(solo_agent).map_err(|e| e.message_in(lang))?;

    if profile.master_agent_is_claude() {
        if let Some(warning) = profile
            .model
            .as_deref()
            .and_then(|m| orchestrator::one_m_model_warning_in(m, "solo", lang))
        {
            eprintln!("{warning}");
        }
    }

    // #2233: 上の `警告:` 行は claude の全画面 TUI に隠れうるので、solo 本人にも渡す
    let prompt_content = orchestrator::master_launch::with_startup_warning(
        profile.build_solo_system_prompt(profile_name),
        load_warning.as_deref(),
    );
    let dir = orchestrator::config_dir().ok_or_else(|| Reason::HomeDirUnknown.text_in(lang))?;
    let prompt_path = dir.join(format!("_solo_system_prompt_{profile_name}.md"));
    std::fs::write(&prompt_path, &prompt_content)
        .map_err(|e| Reason::SystemPromptWriteFailed(e.to_string()).text_in(lang))?;

    // #2127: タブ名は solo が作業するフォルダ（Part 5 の cwd 移動の後）のリポジトリ名を
    // **自動名で**付ける（HOME で立てたら触らない）。役割名（`solo` / `solo-<名前>`）を
    // 手動名で付けると、solo 自身の `tako_rename_tab(source: "auto")` が全部弾かれる
    let solo_dir = std::env::current_dir().ok();
    let tab_naming = orchestrator::master_launch::solo_tab_naming(suffix, solo_dir.as_deref());
    // 新しいタブで立てるときはそのフォルダで開く（開かないとアプリの起動フォルダで立ち、
    // タブ名と solo の作業場所が食い違う）。A/B の旧挙動では従来どおりアプリの既定
    let new_tab_cwd = solo_dir
        .as_deref()
        .filter(|_| !tako_core::master_tab::legacy());

    let role = match suffix {
        Some(s) => format!("solo:{s}"),
        None => "solo".into(),
    };

    let tako_bin = tako_control::dispatch::resolve_tako_binary();
    let solo_cmd = orchestrator::build_master_cmd_typed(&role, &profile, &prompt_path, &tako_bin)
        .map_err(|e| e.text_in(lang))?;

    // --tab 指定時 / 呼び出し元ペインが解決できないとき（#567）は新タブ起動
    let target = resolve_launch_target(
        tab_naming.as_ref(),
        new_tab_cwd,
        use_tab,
        &solo_cmd_hint(profile_name),
        lang,
    )?;
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
        paste_guard: None,
    })?;

    let launched = ReplyNote::SoloLaunched(launch_location(&target, lang));
    eprintln!("{}", launched.text_in(lang));
    eprintln!(
        "{}",
        launch_profile_line(&profile, profile_name, solo_agent, lang)
    );
    eprintln!("{}", ReplyNote::SoloMode.text_in(lang));
    // #1140: solo は worker を spawn しないので、この設定が効く先は本人のペインだけ
    if orchestrator::resolve_master_limit_resume(&profile) {
        eprintln!("{}", ReplyNote::LimitResumeOn.text_in(lang));
    }
    // Part 4: env の可視化（キー名のみ。Issue #500 / #547）
    print_master_env(&profile, profile_name, lang);
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
        // 設計のため、レジストリ解決も CLI プロセス内で行い tako 本体に依存しない）。
        // 断り（`RegistryError`）は表示言語で文にするので、呼ぶ前に設定から決める（#2365）
        init_local_lang();
        let reg = WorkerRegistry::load_typed().map_err(|e| {
            front_err(tako_control::Reason::WorkerRegistryUnreadable(
                e.to_string(),
            ))
        })?;
        let (_, entry) = reg.resolve_typed(worker_id).map_err(|e| e.to_string())?;
        session_id = session_id.or_else(|| entry.session_id.clone());
        tmux_session = tmux_session.or_else(|| entry.tmux_session.clone());
        entry.pane
    } else {
        let Some(p) = pane else {
            return Err(front_err(Reason::WatchTargetMissing));
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
        wait::WatchOutcome::Error {
            kind,
            detail,
            account,
        } => {
            println!("WORKER_ERROR: tako:{pane} ({})", kind.as_str());
            if !detail.is_empty() {
                println!("  detail: {detail}");
            }
            println!("  action: {}", kind.recommended_action());
            // #757: ログイン失効は「どのアカウントを再ログインするか」まで出す
            // （MCP の `tako_orchestrator_worker_status` の `error.config_dir` /
            // `error.account` と同じ内容・同じ逆引き = CLI / MCP 1:1）
            if let Some(account) = account.as_ref() {
                println!("  {}", wait::account_suffix(account));
            }
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
                    // #1143: ラベルが画面上で打ち切られている（元の文字列は残っていない）
                    let cut = if opt.get("label_truncated").and_then(|v| v.as_bool()) == Some(true)
                    {
                        " [切り詰め]"
                    } else {
                        ""
                    };
                    println!("  {number}. {label}{cut}{mark}");
                }
            }
            if choice_dialog.get("numbered").and_then(|v| v.as_bool()) == Some(false) {
                println!("  note: 番号キーは無反応なダイアログ（tako が矢印移動で応答する）");
            }
            // #1143: 狭いペインではカーソルもラベルの全文も画面に残らない
            if choice_dialog
                .get("cursor_visible")
                .and_then(|v| v.as_bool())
                == Some(false)
            {
                println!(
                    "  note: ダイアログがペインより高く選択カーソルが画面外（番号で応答する）"
                );
            }
            if choice_dialog
                .get("labels_truncated")
                .and_then(|v| v.as_bool())
                == Some(true)
            {
                println!("  note: ラベルが切り詰められている（respond はラベルではなく番号で）");
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
        return Err(front_err(tako_control::Reason::EitherOf("--pane", "--tab")));
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
    init_local_lang();
    match sub {
        // list も読み取りだが MCP と同じ 1 本を通す。#1544 まで CLI 側だけ
        // 設定ファイルの直読みが残っていた。出力は一致していたが、**同じ関数に
        // 「直したもの」と「残したもの」が並ぶ**のが再発の温床で、add / remove は
        // #1453 でまさにこの写しから壊れた。番犬は issue1544_projects_dispatch_watchdog
        ProjectsCommand::List => {
            let res =
                tako_control::dispatch::dispatch_orchestrator_projects("list", None, None, None)
                    .map_err(local_dispatch_err)?;
            let projects = res["projects"].as_array().cloned().unwrap_or_default();
            if projects.is_empty() {
                eprintln!("登録済みプロジェクトはありません。");
                eprintln!("追加: tako orchestrator projects add --key <名前> --cwd <パス>");
            } else {
                for p in &projects {
                    println!(
                        "{:<16} {}  {}",
                        p["key"].as_str().unwrap_or(""),
                        p["cwd"].as_str().unwrap_or(""),
                        p["description"].as_str().unwrap_or("")
                    );
                    // #2412: worker へ持ち込む user scope の MCP（既定の tako だけなら出さない）
                    let mcp: Vec<&str> = p["mcp_servers"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
                        .unwrap_or_default();
                    if !mcp.is_empty() {
                        println!("{:<16} worker の MCP: tako + {}", "", mcp.join(", "));
                    }
                }
            }
            Ok(())
        }
        // add / remove は**書き込み**なので同じ 1 本を通す。#1453 まで CLI 側に写しがあり、
        // 専用プロファイルの自動生成が MCP からだけ効いて CLI からは効かなかった（実測で踏んだ）
        ProjectsCommand::Add {
            key,
            cwd,
            description,
        } => {
            let res = tako_control::dispatch::dispatch_orchestrator_projects(
                "add",
                Some(key.clone()),
                Some(cwd.clone()),
                description.clone(),
            )
            .map_err(local_dispatch_err)?;
            eprintln!("追加しました: {key} → {cwd}");
            // #1825: 黙らせてはいけない理由（default を継承できなかった等）。
            // 同じ文が profile_note にも載るので、そちらは重ねて出さない
            let warnings: Vec<&str> = res["warnings"]
                .as_array()
                .map(|a| a.iter().filter_map(|w| w.as_str()).collect())
                .unwrap_or_default();
            let note = res["profile_note"].as_str().unwrap_or("理由不明");
            // 専用プロファイルの生成結果も見せる（黙って作らない / 黙って作らないでもない）
            match res["profile_generation"].as_str() {
                Some("created") => eprintln!(
                    "専用プロファイルを作りました: {}（default から継承）",
                    res["profile_path"].as_str().unwrap_or("")
                ),
                Some("skipped") if !warnings.contains(&note) => {
                    eprintln!("専用プロファイルは作りませんでした: {note}")
                }
                _ => {}
            }
            for w in &warnings {
                eprintln!("{}: {w}", ReplyNote::WarningLabel.text_in(cli_lang()));
            }
            Ok(())
        }
        ProjectsCommand::Remove { key } => {
            tako_control::dispatch::dispatch_orchestrator_projects(
                "remove",
                Some(key.clone()),
                None,
                None,
            )
            .map_err(local_dispatch_err)?;
            eprintln!("削除しました: {key}");
            Ok(())
        }
        // #2412: MCP と同じ 1 本（dispatch_orchestrator_projects_mcp）を通す
        ProjectsCommand::Mcp { key, op, servers } => {
            let action = match op {
                Some(ProjectMcpOp::Add) => "mcp_add",
                Some(ProjectMcpOp::Remove) => "mcp_remove",
                None => "mcp",
            };
            let res = tako_control::dispatch::dispatch_orchestrator_projects_mcp(
                action,
                Some(key.clone()),
                servers.clone(),
            )
            .map_err(local_dispatch_err)?;
            print_project_mcp(key, &res);
            Ok(())
        }
    }
}

/// `tako orchestrator projects mcp` の表示（設定・今 spawn したときの構成・警告）
fn print_project_mcp(key: &str, res: &serde_json::Value) {
    let list = |v: &serde_json::Value| -> String {
        let names: Vec<&str> = v
            .as_array()
            .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
            .unwrap_or_default();
        if names.is_empty() {
            "（なし）".to_string()
        } else {
            names.join(", ")
        }
    };
    if let Some(added) = res.get("added") {
        eprintln!("足しました: {}", list(added));
    }
    if let Some(removed) = res.get("removed") {
        eprintln!("外しました: {}", list(removed));
    }
    let plan = &res["plan"];
    println!(
        "{key} の worker へ持ち込む user scope の MCP: {}",
        list(&res["mcp_servers"])
    );
    println!("  今 spawn すると持ち込む: tako + {}", list(&plan["kept"]));
    println!("  止める（user scope）: {}", list(&plan["denied"]));
    if plan["shadowed"].as_array().is_some_and(|a| !a.is_empty()) {
        println!(
            "  プロジェクトの .mcp.json と同名なので止めない: {}",
            list(&plan["shadowed"])
        );
    }
    // #2418: codex の worker（codex の設定がある機だけ応答に載る）
    let codex = &res["codex_plan"];
    if codex.is_object() {
        println!(
            "  codex の worker で止める（config.toml）: {}",
            list(&codex["denied"])
        );
        if codex["shadowed"].as_array().is_some_and(|a| !a.is_empty()) {
            println!(
                "  codex: プロジェクトの .codex/config.toml と同名なので止めない: {}",
                list(&codex["shadowed"])
            );
        }
    }
    for w in res["warnings"].as_array().into_iter().flatten() {
        if let Some(w) = w.as_str() {
            eprintln!("警告: {w}");
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
            cwd,
            clear_cwd,
            ctx_threshold,
            clear_ctx_threshold,
            auto_handoff,
            clear_auto_handoff,
            limit_resume,
            clear_limit_resume,
            bypass_sandbox,
            remote_control,
        } => ProfilesParams {
            action: "set".into(),
            name: Some(name.clone()),
            kind: kind.kind(),
            from: None,
            projects: projects.clone(),
            clear_projects: *clear_projects,
            cwd: cwd.clone(),
            clear_cwd: *clear_cwd,
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
            bypass_sandbox: *bypass_sandbox,
            remote_control: *remote_control,
        },
    };
    init_local_lang();
    let result = dispatch_orchestrator_profiles(params).map_err(local_dispatch_err)?;
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
    use tako_control::remote::{with_origin, RequestOrigin};
    // 起動の途中の断り（`RemoteRefusal::DaemonStarting`。#2613）は表示言語で文にするので、呼ぶ前に決める
    init_local_lang();
    // #2533: audit.log に「CLI から頼んだ」と残す
    let result = with_origin(RequestOrigin::Cli, tako_control::remote::spawn_daemon)?;
    println!("{}", pretty_json(&result));
    // #2518: 旧版のまま動いていた daemon を入れ替えたなら、スマホが一瞬切れた理由を言う
    if let Some(pid) = result["renewed"]["previous_pid"].as_u64() {
        eprintln!(
            "旧版のまま動いていたリモート daemon（PID {pid}）を止めて、新版で起動し直しました。"
        );
    }
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
                    line: None,
                    column: None,
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
    // #1485: 「一度起動すれば動き続ける」と読むのが自然なので、実際にそうなることを言う
    eprintln!(
        "tako や Mac を再起動しても、この状態は自動で戻ります（`tako remote stop` で解除）。"
    );
    Ok(())
}

/// `tako remote stop` — デーモンを PID ファイルから kill する
fn remote_stop(force: bool) -> Result<(), String> {
    use tako_control::remote::{with_origin, RequestOrigin};
    // 起動の途中の断り（`RemoteRefusal::DaemonStartingStop`。#2613）は表示言語で文にするので、呼ぶ前に決める
    init_local_lang();
    // #2533: audit.log に「CLI から頼んだ」と残す
    let result = if force {
        with_origin(RequestOrigin::Cli, tako_control::remote::daemon_force_stop)?
    } else {
        with_origin(RequestOrigin::Cli, tako_control::remote::daemon_stop)?
    };
    println!("{}", pretty_json(&result));
    eprintln!("リモートサーバーを停止しました");
    Ok(())
}

/// `tako remote status` — デーモンの状態を表示する。
/// 応答にトークンは含まれない（#283 で長寿命 bearer token を全廃）
fn remote_status() -> Result<(), String> {
    let status = tako_control::remote::daemon_status();
    // 自動復帰の失敗も JSON に埋もれさせない（#1485: 「起動していたのに戻っていない」を
    // 一目で分かる形に）。**判断は再計算せず状態を読むだけ**（#372 / #1473 と同じ）
    if let Some(last) = status.get("last_autostart") {
        if last.get("result").and_then(|v| v.as_str())
            == Some(tako_control::remote_autostart::RESULT_FAILED)
        {
            eprintln!(
                "前回の起動状態へ自動で戻せませんでした（{} 回試行）: {}",
                last.get("attempts").and_then(|v| v.as_u64()).unwrap_or(0),
                last.get("detail").and_then(|v| v.as_str()).unwrap_or(""),
            );
            eprintln!("  次の一手: `tako remote start` で手動起動できます");
        }
    }
    // 劣化は JSON の中に埋もれさせない（#1049: 「running なのに見れない」を一目で分かる形に）
    if let Some(d) = status.get("degraded") {
        eprintln!(
            "リモート公開が機能していません: {}",
            d.get("reason").and_then(|v| v.as_str()).unwrap_or("")
        );
        if let Some(next) = d.get("next_step").and_then(|v| v.as_str()) {
            if !next.is_empty() {
                eprintln!("  次の一手: {next}");
            }
        }
    }
    // 旧版のまま動いている daemon も埋もれさせない（#2518。判定は status の欄を読むだけ）
    if let Some(detail) = status["serve_generation"]["detail"].as_str() {
        eprintln!("{detail}");
    }
    // 起こしている途中（自動復帰の入れ替えほか）は止めずに待つことを言う（#2608。欄を読むだけ）
    if let Some(detail) = status["starting"]["detail"].as_str() {
        eprintln!("{detail}");
    }
    println!("{}", pretty_json(&status));
    Ok(())
}

/// `tako remote devices` — ペアリング済み端末の一覧・降格・失効。
///
/// **権限を上げる経路はここに無い**（#1452）。承認は Mac 画面の承認ダイアログ、
/// 昇格は設定 → リモートだけが持つ（AI フルコントロール不変条件の例外。
/// `.agent/requirements.md` FR-6.5 / FR-6.21）。`role` に強いものを渡すと断られるので、
/// 「上げられない」ことを CLI からそのまま観測できる
fn remote_devices(command: RemoteDevicesCommand) -> Result<(), String> {
    // 断り（`RemoteRefusal`）は表示言語で文にするので、呼ぶ前に設定から決める（#2365）
    init_local_lang();
    let result = match command {
        RemoteDevicesCommand::List => tako_control::remote::devices_list(),
        RemoteDevicesCommand::Role { device_id, role } => {
            tako_control::remote::devices_set_role(&device_id, &role)
        }
        RemoteDevicesCommand::Revoke { device_id } => {
            tako_control::remote::devices_revoke(&device_id)
        }
    }
    .map_err(|e| e.to_string())?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote shortcuts` — スマホのファイル閲覧のショートカット（#1451）。
///
/// **サブコマンド省略で一覧**（`tako remote shortcuts list` と打たせない = #322 の最簡形）。
/// 正本は `tako_core::remote_shortcuts` で、PWA・MCP と同じ 1 実装を通る
fn remote_shortcuts(command: Option<RemoteShortcutsCommand>) -> Result<(), String> {
    let (action, path, name) = match command {
        None | Some(RemoteShortcutsCommand::List) => ("list", None, None),
        Some(RemoteShortcutsCommand::Add { path, name }) => ("add", Some(path), name),
        Some(RemoteShortcutsCommand::Remove { path }) => ("remove", Some(path), None),
    };
    // dispatch と**同一関数**を共用する（IPC 不要 = GUI が動いていなくても効く）。
    // 理由の文（#2365）を前置きと同じ表示言語で組む
    init_local_lang();
    let result =
        tako_control::dispatch::dispatch_remote_shortcuts(action, path.as_deref(), name.as_deref())
            .map_err(local_dispatch_err)?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote setup` — Tailscale リモートセットアップウィザード
fn remote_setup_cli(
    yes: bool,
    answers_json: Option<&str>,
    tailscale: Option<&str>,
) -> Result<(), String> {
    if let Some(json_str) = answers_json {
        // 非対話の経路の断り（`RemoteRefusal`）は表示言語で文にするので、呼ぶ前に設定から決める（#2365。
        // 対話のウィザードは日本語の表示のまま）
        init_local_lang();
        let mut answers: tako_control::remote_setup::RemoteSetupAnswers =
            serde_json::from_str(json_str).map_err(|e| {
                tako_control::RemoteRefusal::SetupAnswersJsonInvalid(e.to_string()).to_string()
            })?;
        if yes {
            answers.yes = Some(true);
        }
        if let Some(v) = tailscale {
            answers.tailscale = Some(v.to_string());
        }
        let result =
            tako_control::remote_setup::run_noninteractive(&answers).map_err(|e| e.to_string())?;
        println!("{}", pretty_json(&result));
    } else {
        let mut stdout = std::io::stdout();
        tako_control::remote_setup::run_interactive(yes, tailscale, &mut stdout)?;
    }
    Ok(())
}

/// `tako remote serve` — HTTP サーバーをフォアグラウンドで起動する（内部用）
fn remote_serve() -> Result<(), String> {
    tako_control::remote::run_daemon().map_err(|e| e.to_string())
}

/// `tako remote agents` — claude agents --json + tmux ペイン対応付けを表示する
fn remote_agents() -> Result<(), String> {
    // 断り（`AgentScanError`）は表示言語で文にするので、呼ぶ前に設定から決める（#2365。
    // 日本語の口 `list_agents_with_panes` は daemon と監視の記録用）
    init_local_lang();
    let mut result =
        tako_control::agents::list_agents_with_panes_typed(None).map_err(|e| e.to_string())?;
    // #1069: 公式リンク（claude.ai/code）。HTTP の /api/agents と dispatch の
    // RemoteAgents と**同じ 1 実装**を通す（CLI はローカル処理なので自分で呼ぶ）
    tako_control::claude_remote_link::attach_to_agents(&mut result);
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote messages` — transcript の末尾を正規化 JSON で表示する
fn remote_messages(session_id: &str, tail: usize) -> Result<(), String> {
    // 断りは表示言語で文にする（#2365。日本語の口 `read_messages` は daemon が PWA へ返す本文用）
    init_local_lang();
    let result = tako_control::transcript::read_messages_typed(session_id, tail)
        .map_err(|e| e.to_string())?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako remote scrollback` — ペインのスクロールバック履歴をプレーンテキストで表示する
fn remote_scrollback(pane_id: &str, lines: u32) -> Result<(), String> {
    // 断り（`RemoteRefusal`）は表示言語で文にするので、呼ぶ前に設定から決める（#2365）
    init_local_lang();
    let result = tako_control::remote::scrollback(pane_id, lines).map_err(|e| e.to_string())?;
    for line in result {
        println!("{line}");
    }
    Ok(())
}

fn telemetry_local(sub: &TelemetryCommand) -> Result<(), String> {
    let mut settings = tako_control::settings::load();
    // 状態の行と保存の断りは表示言語（#2365）
    let lang = cli_lang();
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
            eprintln!("{}", ReplyNote::TelemetryRecent(recent).text_in(lang));
            if queued > 0 {
                eprintln!("{}", ReplyNote::TelemetryQueued(queued).text_in(lang));
            }
            eprintln!(
                "{}",
                ReplyNote::TelemetryLog(log_path.clone()).text_in(lang)
            );
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
                .map_err(|e| Reason::settings_save_failed(&e).text_in(lang))?;
            tako_control::telemetry::set_enabled(true);
            eprintln!("telemetry: ON");
            Ok(())
        }
        TelemetryCommand::Off => {
            settings.telemetry = false;
            tako_control::settings::save(&settings)
                .map_err(|e| Reason::settings_save_failed(&e).text_in(lang))?;
            tako_control::telemetry::set_enabled(false);
            eprintln!("telemetry: OFF");
            Ok(())
        }
    }
}

fn fda_local(sub: &FdaCommand) -> Result<(), String> {
    // 状態の行・JSON の description・断りは表示言語（#2365。`to_json` は表示言語のグローバルを引く）
    init_local_lang();
    let lang = tako_core::i18n::lang();
    match sub {
        FdaCommand::Status => {
            let status = tako_control::fda::status_info();
            if status.granted {
                eprintln!("{}", ReplyNote::FdaGranted.text_in(lang));
            } else {
                eprintln!("{}", ReplyNote::FdaNotGranted.text_in(lang));
                eprintln!("{}", ReplyNote::FdaDialogMayAppear.text_in(lang));
                eprintln!("{}", ReplyNote::FdaHowToGrant.text_in(lang));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&status.to_json()).unwrap()
            );
            Ok(())
        }
        FdaCommand::Open => {
            tako_control::fda::open_settings_typed().map_err(|e| e.text_in(lang))?;
            eprintln!("{}", ReplyNote::FdaSettingsOpened.text_in(lang));
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
    .map_err(local_dispatch_err)?;
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

/// `tako mod install|uninstall [--dry-run]`（#1959。ローカル処理・IPC 不要）。
///
/// claude の版は上限つきの判定（`bounded_claude_version`）で、置く先は accounts.yaml・既定・
/// このシェルの `CLAUDE_CONFIG_DIR`。mod の報告で見えた dir は GUI しか知らないので、GUI が
/// 動いていれば起動時の差分検出がそこへ置く（MCP `tako_mod` は GUI の材料ごと見る）
fn mod_install_local(args: &ModArgs) -> Result<(), String> {
    let action = args.action.as_deref().unwrap_or("install");
    // 位置引数は #1960 で `rest`（band の on / off・ui の操作）、ui の button add は
    // --label / --hotkey / --id になった。install / uninstall はどれも取らない
    if !args.rest.is_empty() || args.label.is_some() || args.hotkey.is_some() || args.id.is_some() {
        return Err(front_err(tako_control::Reason::ModTakesNoArgs(
            action.to_string(),
        )));
    }
    let ctx = tako_control::claude_mod_install::Context::from_env(
        tako_control::stale_binary::bounded_claude_version(),
    );
    // 応答の文（applies_to・理由）は表示言語で組まれるので、呼ぶ前に決める（#2365）
    init_local_lang();
    let lang = tako_core::i18n::lang();
    let out = tako_control::claude_mod_install::run_action(action, args.dry_run, &[], &ctx)?;
    // 置けない（外せない）設定 dir が 1 つでもあれば終了コード 1（JSON でも同じ）
    let failed = out["skills"]["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|t| t["outcome"] == "error");
    let result = if failed {
        Err(ModText::InstallTargetsFailed.text_in(lang))
    } else {
        Ok(())
    };
    if args.json {
        println!("{}", pretty_json(&out));
        return result;
    }
    let heading = ModText::InstallHeading {
        install: action == "install",
        dry_run: args.dry_run,
        claude: out["claude_version"].as_str().map(str::to_string),
        min: out["min_claude_version"]
            .as_str()
            .unwrap_or("-")
            .to_string(),
    };
    println!("{}", heading.text_in(lang));
    print_mod_skills(&out["skills"], lang);
    if let Some(note) = out["applies_to"].as_str() {
        println!("{note}");
    }
    result
}

/// 設定 dir ごとの skills/tako の行（`tako mod` と `tako mod install|uninstall` と `tako check-health` で
/// 共通。組み立ては tako-control の 1 本）。`lang` は行の枠の言語（どれも表示言語。#2365）
fn print_mod_skills(skills: &Value, lang: tako_core::i18n::Lang) {
    for line in tako_control::claude_mod::text::skills_lines(skills, lang) {
        println!("{line}");
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
            "launch" => "tako が開く PowerShell へ起動時に読ませる（$PROFILE は触らない）",
            other => other,
        }
    );
    if let Some(script) = out["script"].as_str() {
        println!("  スクリプト: {script}");
    }
    if let Some(shell) = out["launch_shell"].as_str() {
        println!("  シェル   : {shell}");
    }
    // #2432: 以前の版が置いた `$PROFILE` のブロックが残っている（移行が取り除けなかった）
    let legacy = out["legacy_blocks"].as_array().cloned().unwrap_or_default();
    if !legacy.is_empty() {
        println!(
            "  [注意] 以前の版が $PROFILE に置いた管理ブロックが残っています（取り除くには tako shell-integration uninstall）"
        );
        for b in &legacy {
            println!(
                "  - {}\n    {}",
                b["label"].as_str().unwrap_or("?"),
                b["path"].as_str().unwrap_or("?")
            );
        }
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
    // CLI 単独で走るので表示言語を settings.json から解決する（platform と同じ。#435）。
    // ファイルに触らない読みにする: 見るだけの status が読めない settings.json を退避へ移すと、
    // 報告に出るはずの settings.json が「不在」になる（#2238）
    tako_core::i18n::set_lang(tako_control::settings::peek_lang_setting().resolve());
    let action = args.action.as_deref().unwrap_or("status");
    let result = tako_control::migrations::report_json(action, args.schema.as_deref())?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// `tako usage`（#2200 / #2201。ローカル処理 = GUI 不要）。MCP `tako_sessions` の `action: "usage"` と
/// 同じ `tako_control::usage::usage_payload` を通る（観測は GUI が `<data_dir>/usage/` へ書き、
/// 使用量の索引は GUI の背景とこのコマンドが同じ関数で読み進める）
fn usage_local(args: &UsageArgs) -> Result<(), String> {
    let query = tako_control::usage::UsageQuery {
        by: args.by.clone(),
        window: args.window.clone(),
        account: args.account.clone(),
        project: args.project.clone(),
        issue: args.issue.clone(),
        limit: args.limit,
    };
    // #2203: `--show` は画面を動かすので tako へ送る（答えは同じ `usage_payload`。索引は GUI の
    // 背景と同じく上限つきで読み進める）。届かなければ開けない理由を言って止まる
    let payload = if args.show {
        // 引数の断り（`Reason` の Display）は表示言語で組むので、確かめる前に決める（#2365）
        init_local_lang();
        query.parsed()?;
        let request = Request::Sessions {
            action: "usage".into(),
            id: None,
            role: None,
            project: query.project.clone(),
            limit: query.limit,
            pane: None,
            tab: None,
            direction: None,
            by: query.by.clone(),
            window: query.window.clone(),
            account: query.account.clone(),
            issue: query.issue.clone(),
            show: Some(true),
        };
        match send_request_classified(request, None) {
            Ok(value) => value,
            Err(SendFailure::Unreachable(_)) => return Err(front_err(Reason::UsageShowNeedsGui)),
            Err(e) => return Err(e.message()),
        }
    } else {
        // 置き場が決まらない断り（`paths::NoDataDir`）は表示言語で組まれるので、呼ぶ前に決める（#2365）
        init_local_lang();
        tako_control::usage::usage_payload(&query, tako_control::usage::Refresh::Full)?
    };
    let update = &payload["index"]["update"];
    if update["bytes_read"].as_u64().unwrap_or(0) >= 256 * 1024 * 1024 {
        eprintln!(
            "使用量の索引を作った（会話記録 {} 本・{} MB を {:.1} 秒。次からは追記された分だけを読む）",
            update["read_files"].as_u64().unwrap_or(0),
            update["bytes_read"].as_u64().unwrap_or(0) / (1024 * 1024),
            update["elapsed_ms"].as_u64().unwrap_or(0) as f64 / 1000.0,
        );
    }
    if args.json {
        println!("{}", pretty_json(&payload));
        return Ok(());
    }
    let now = tako_core::user_task::unix_now();
    let offset = tako_core::platform::clock::local_utc_offset();
    print!("{}", usage_text(&payload, now, offset));
    Ok(())
}

/// トークン数の表記（万・億。右パネルの usage ビューと同じ 1 実装 = #2203）
fn usage_tokens(n: u64) -> String {
    tako_core::usage::display::tokens(n)
}

/// 量の 1 行（API 換算・トークン・応答・会話）
fn usage_amount(t: &Value) -> String {
    let mut out = format!(
        "API 換算 {}・トークン {}（出力 {}）・応答 {}・会話 {}",
        tako_core::usage::display::api_usd(t["api_usd"].as_f64().unwrap_or(0.0)),
        usage_tokens(t["total_tokens"].as_u64().unwrap_or(0)),
        usage_tokens(t["tokens"]["output"].as_u64().unwrap_or(0)),
        t["responses"].as_u64().unwrap_or(0),
        t["sessions"].as_u64().unwrap_or(0),
    );
    if let Some(n) = t["unpriced_tokens"].as_u64().filter(|n| *n > 0) {
        out.push_str(&format!("・単価不明 {}", usage_tokens(n)));
    }
    out
}

/// 期間の表記（右パネルの usage ビューと同じ 1 実装 = #2203）
fn usage_range(window: &str, r: &Value, now: i64, utc_offset: i32) -> String {
    tako_core::usage::display::range_label(window, r, now, utc_offset)
}

/// 利用制限の % の列に出す枠（主の枠 = `--window 5h` なら 5h、それ以外は 7d を左に）
fn usage_share_kinds(share: &Value, window: &str) -> Vec<Value> {
    let mut kinds = share.as_array().cloned().unwrap_or_default();
    let main = if window == "5h" {
        "five_hour"
    } else {
        "seven_day"
    };
    kinds.sort_by_key(|k| k["kind"].as_str() != Some(main));
    kinds
}

fn usage_kind_label(k: &Value) -> &'static str {
    match k["kind"].as_str() {
        Some("five_hour") => "5h",
        _ => "7d",
    }
}

/// % の 1 マス（12 桁: `30.9%` を右寄せ 6 桁 + 推定 `+0.4` を 6 桁。観測の無い枠は `-`）
fn usage_share_cell(available: bool, share: &Value) -> String {
    if !available {
        return format!("{:>6}{:6}", "-", "");
    }
    let percent = share["percent"].as_f64().unwrap_or(0.0);
    let estimated = share["estimated"].as_f64().unwrap_or(0.0);
    let est = tako_core::usage::display::estimated(estimated)
        .map(|e| format!(" {e:<5}"))
        .unwrap_or_default();
    format!(
        "{:>6}{est:6}",
        tako_core::usage::display::share_percent(percent)
    )
}

fn usage_share_cells(kinds: &[Value], share: &Value) -> String {
    kinds
        .iter()
        .map(|k| {
            let kind = k["kind"].as_str().unwrap_or("-");
            usage_share_cell(k["available"] == Value::Bool(true), &share[kind])
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// 段の後ろの行: tako の外・推定の合計と、枠ごとの按分の要約（観測 = 段 + tako の外・係数とその幅）
fn usage_share_footer(kinds: &[Value]) -> String {
    let mut out = String::new();
    let f = |k: &Value, key: &str| k[key].as_f64().unwrap_or(0.0);
    let available = |k: &Value| k["available"] == Value::Bool(true);
    let row = |label: &str, key: &str| {
        let cells: Vec<String> = kinds
            .iter()
            .map(|k| usage_share_cell(available(k), &serde_json::json!({ "percent": f(k, key) })))
            .collect();
        format!("      {}  {label}\n", cells.join("  "))
    };
    out.push_str(&row(
        "tako の外（区間にローカルの使用量が無いのに増えた分）",
        "outside",
    ));
    if kinds.iter().any(|k| f(k, "estimated") >= 0.05) {
        let cells: Vec<String> = kinds
            .iter()
            .map(|k| {
                if available(k) {
                    format!("{:>6}{:6}", format!("+{:.1}", f(k, "estimated")), "")
                } else {
                    format!("{:>6}{:6}", "-", "")
                }
            })
            .collect();
        out.push_str(&format!(
            "      {}  推定の合計（観測の区間の外 = 最後の観測より後・観測の無い窓）\n",
            cells.join("  ")
        ));
    }
    for k in kinds {
        let label = usage_kind_label(k);
        if !available(k) {
            out.push_str(&format!(
                "    {label} の按分: このログインの観測が無いので % を出していない\n"
            ));
            continue;
        }
        let head = match (k["scope"].as_str(), k["latest_percent"].as_f64()) {
            (Some("period"), _) => format!("期間の延べ {:.1}%", f(k, "observed")),
            (_, Some(latest)) => format!("今の窓 {latest:.1}%"),
            _ => "観測した今の窓が無い".to_string(),
        };
        let mut line = format!(
            "    {label} の按分: {head} = 段 {:.1} + tako の外 {:.1}",
            f(k, "allocated"),
            f(k, "outside")
        );
        if f(k, "estimated") >= 0.05 {
            line.push_str(&format!("・推定 +{:.1}", f(k, "estimated")));
        }
        if let Some(per) = k["coefficient"]["usd_per_percent"].as_f64() {
            line.push_str(&format!(
                "・1% あたり API 換算 ${per:.2}（区間 {} 本",
                k["coefficient"]["intervals"].as_u64().unwrap_or(0)
            ));
            let fit = k["windows"]
                .as_array()
                .and_then(|w| w.last())
                .map(|w| &w["usd_per_percent"]);
            match fit.filter(|v| v.is_object()) {
                Some(v) => line.push_str(&format!(
                    "・直近の窓の四分位 ${:.2}〜${:.2}）",
                    v["p25"].as_f64().unwrap_or(0.0),
                    v["p75"].as_f64().unwrap_or(0.0)
                )),
                None => line.push('）'),
            }
        }
        // codex など単価表に無いモデルだけのログインは 1% あたりの生トークン（S5 #2204）
        if k["coefficient"]["usd_per_percent"].is_null() {
            if let Some(per) = k["coefficient"]["tokens_per_percent"].as_f64() {
                line.push_str(&format!(
                    "・1% あたり トークン {}（区間 {} 本）",
                    usage_tokens(per.max(0.0) as u64),
                    k["coefficient"]["intervals"].as_u64().unwrap_or(0)
                ));
            }
        }
        if let Some(usd) = k["unestimated_usd"].as_f64().filter(|v| *v > 0.0) {
            line.push_str(&format!("・% へ直せない API 換算 ${usd:.2}"));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `tako usage` の既定出力（機械検証できるよう印字と分けてある）。時刻の表記は
/// `tako todo show` と同じ 1 実装（`user_task::response_stamp`）
fn usage_text(p: &Value, now: i64, utc_offset: i32) -> String {
    let stamp_ms = |ms: &Value| {
        ms.as_u64()
            .map(|ms| tako_core::user_task::response_stamp((ms / 1000) as i64, now, utc_offset))
            .unwrap_or_else(|| "-".into())
    };
    let pct = |v: &Value| {
        let f = v.as_f64().unwrap_or(0.0);
        if f.fract() == 0.0 {
            format!("{f:.0}%")
        } else {
            format!("{f:.1}%")
        }
    };
    let mut out = String::from(
        "利用制限の観測（tako mod の報告と codex の rollout。% か解除時刻が変わったときだけ記録）と使用量の内訳\n",
    );
    if p["recording"] == Value::Bool(false) {
        out.push_str(&format!(
            "  記録していない（{} が立っている）\n",
            p["legacy_env"].as_str().unwrap_or("A/B の env")
        ));
    }
    let logins = p["logins"].as_array().cloned().unwrap_or_default();
    if logins.is_empty() {
        if let Some(hint) = p["hint"].as_str() {
            out.push_str(&format!("  {hint}\n"));
        }
        return out;
    }
    let window = p["window"].as_str().unwrap_or("7d");
    let by = p["by"].as_str().unwrap_or("project");
    for l in &logins {
        let from = match l["key_from"].as_str() {
            Some("config_dir") => " = 資格情報を読めず設定 dir で束ねた",
            _ => "",
        };
        // 内訳があるときの会話数は内訳の行に出す（ここの会話は mod の報告で対応づけた数で、意味が違う）
        let sessions = if l["usage"].is_object() {
            String::new()
        } else {
            format!("・会話 {} 本", l["sessions"].as_u64().unwrap_or(0))
        };
        // S5 #2204: codex のログインは系統を添える（claude は既定なので書かない）
        let agent = match l["agent"].as_str() {
            Some("codex") => "codex・",
            _ => "",
        };
        out.push_str(&format!(
            "  ログイン {}（{agent}鍵 {}{from}）  観測 {} 点{sessions}\n",
            l["label"].as_str().unwrap_or("-"),
            l["login"].as_str().unwrap_or("-"),
            l["observations"].as_u64().unwrap_or(0),
        ));
        for w in l["windows"].as_array().cloned().unwrap_or_default() {
            let kind = match w["kind"].as_str() {
                Some("five_hour") => "5h".to_string(),
                Some("seven_day") => "7d".to_string(),
                other => other.unwrap_or("-").to_string(),
            };
            let resets = w["resets_at"]
                .as_i64()
                .map(|r| tako_core::user_task::response_stamp(r, now, utc_offset))
                .unwrap_or_else(|| "-".into());
            let expired = if w["expired"] == Value::Bool(true) {
                "（過ぎた = 今の窓の値ではない）"
            } else {
                ""
            };
            out.push_str(&format!(
                "    {kind:<3} {:>5}  解除 {resets}{expired}  最新の観測 {}（{} 点）\n",
                pct(&w["used_percent"]),
                stamp_ms(&w["observed_at"]),
                w["observations"].as_u64().unwrap_or(0),
            ));
        }
        let usage = &l["usage"];
        if usage.is_object() {
            out.push_str(&format!(
                "    {}  {}\n",
                usage_range(window, &usage["range"], now, utc_offset),
                usage_amount(&usage["total"])
            ));
            // #2202: 利用制限の % の列（観測のある枠だけ。主の枠を左に）
            let kinds = usage_share_kinds(&usage["limit_share"], window);
            let shares = kinds.iter().any(|k| k["available"] == Value::Bool(true));
            if shares {
                let scope = match kinds.iter().find_map(|k| k["scope"].as_str()) {
                    Some("period") => "期間に重なる窓の延べ",
                    _ => "今の窓のうち各段が使った分",
                };
                let heads: Vec<String> = kinds
                    .iter()
                    .map(|k| format!("{:>6}{:6}", usage_kind_label(k), ""))
                    .collect();
                out.push_str(&format!(
                    "      {}（% は{scope}・+ は推定）\n",
                    heads.join("  ")
                ));
            }
            for g in usage["groups"].as_array().cloned().unwrap_or_default() {
                let cells = if shares {
                    format!("{}  ", usage_share_cells(&kinds, &g["limit_share"]))
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "      {cells}{}  {}\n",
                    g["name"].as_str().unwrap_or("-"),
                    usage_amount(&g)
                ));
            }
            if let Some(n) = usage["more_groups"].as_u64() {
                out.push_str(&format!("      ほか {n} 件（--limit で増やせる）\n"));
            }
            if shares {
                out.push_str(&usage_share_footer(&kinds));
            }
        } else if p["index"]["enabled"] == Value::Bool(true) {
            out.push_str("    この期間の使用量は無い\n");
        }
    }
    let observations = p["observations"].as_u64().unwrap_or(0);
    if observations > 0 {
        out.push_str(&format!(
            "  記録 {observations} 点（{} 〜 {}）\n",
            stamp_ms(&p["first_observed_at"]),
            stamp_ms(&p["last_observed_at"]),
        ));
    } else if let Some(hint) = p["hint"].as_str() {
        out.push_str(&format!("  {hint}\n"));
    }
    if let Some(n) = p["unreadable_lines"].as_u64() {
        out.push_str(&format!(
            "  読めない行 {n} 行を飛ばした（書きかけ・壊れた行）\n"
        ));
    }
    if p["index"]["enabled"] == Value::Bool(true) && by != "project" {
        out.push_str(&format!("  段: {by}（既定は project）\n"));
    }
    for note in p["notes"].as_array().cloned().unwrap_or_default() {
        if let Some(note) = note.as_str() {
            out.push_str(&format!("  注: {note}\n"));
        }
    }
    out
}

/// agent 能力マトリクスの表示（#982。ローカル処理・IPC 不要）。
///
/// **GUI が無くても引ける**ことが要件。「codex worker を立てたが動かない」を
/// 調べるときに tako 本体が生きているとは限らない（`platform` と同じ判断）。
/// MCP からは `tako_agent_support` が同じ `agent_support::report` を通る
fn agent_support_local(args: &AgentSupportArgs) -> Result<(), String> {
    // 表示言語のグローバルは既定が英語。CLI 単独で走るここでも settings.json から
    // 解決しないと、日本語設定なのに英語で出てしまう（#435。platform_local と同じ）
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    let report =
        tako_control::agent_support::report(args.agent.as_deref(), args.status.as_deref())?;
    if args.json {
        println!("{}", pretty_json(&report));
        return Ok(());
    }

    use tako_core::i18n::Lang;
    let ja = matches!(tako_core::i18n::lang(), Lang::Ja);
    let agents: Vec<(String, String)> = report["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some((
                a["key"].as_str()?.to_string(),
                a["label"].as_str()?.to_string(),
            ))
        })
        .collect();

    // 見出し: 系統ごとの内訳
    for (key, label) in &agents {
        let c = &report["counts"][key];
        let line: Vec<String> = ["supported", "degraded", "pending", "unsupported"]
            .iter()
            .filter_map(|s| c[*s].as_u64().map(|n| format!("{s} {n}")))
            .collect();
        println!("{label:<18} {}", line.join(" / "));
    }
    println!();

    // 単独指定なら 1 列、全系統なら記号の表（○ を使わず status を短縮する）
    let short = |st: &str| match st {
        "supported" => "ok",
        "degraded" => "part",
        "pending" => "todo",
        "unsupported" => "n/a",
        _ => "?",
    };
    if agents.len() > 1 {
        let head: Vec<String> = agents.iter().map(|(k, _)| format!("{k:>7}")).collect();
        println!("{:<30} {}", "", head.join(" "));
    }
    let mut shown = 0;
    for f in report["features"].as_array().into_iter().flatten() {
        let key = f["key"].as_str().unwrap_or("?");
        shown += 1;
        if agents.len() > 1 {
            let cells: Vec<String> = agents
                .iter()
                .map(|(k, _)| {
                    format!(
                        "{:>7}",
                        short(f["agents"][k]["status"].as_str().unwrap_or("?"))
                    )
                })
                .collect();
            println!("{key:<30} {}", cells.join(" "));
        } else {
            let cell = &f["agents"][&agents[0].0];
            print!("{:<6} {key}", short(cell["status"].as_str().unwrap_or("?")));
            if let Some(issue) = cell["issue"].as_u64() {
                print!("  #{issue}");
            }
            println!();
            if let Some(note) = cell["note"].as_str() {
                println!("{:<6} {note}", "");
            }
        }
    }
    if shown == 0 {
        println!(
            "{}",
            if ja {
                "（該当なし）"
            } else {
                "(no matches)"
            }
        );
    } else if agents.len() > 1 {
        println!();
        println!(
            "{}",
            if ja {
                "ok = claude 同等 / part = 縮退 / todo = 未実装か未調査 / n/a = 上流に手段が無い"
            } else {
                "ok = same as claude / part = degraded / todo = unimplemented or uninvestigated / n/a = no upstream mechanism"
            }
        );
        println!(
            "{}",
            if ja {
                "理由と根拠は --agent <系統> か --json で読める"
            } else {
                "Run with --agent <name> or --json to read the reasons and evidence"
            }
        );
    }
    Ok(())
}

/// master の手順書（#1154）。CLI・MCP とも `orchestrator::guide::json` の 1 本を通る。
///
/// プレースホルダを解決するプロファイルは、明示指定 → 呼び出し元の
/// `TAKO_ORCHESTRATOR_ROLE` → `default` の順（master 自身が引くときは env で決まる）
fn orchestrator_guide_local(
    topic: Option<&str>,
    profile: Option<&str>,
    as_json: bool,
) -> Result<(), String> {
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    let profile_name = profile.map(str::to_string).unwrap_or_else(|| {
        std::env::var("TAKO_ORCHESTRATOR_ROLE")
            .ok()
            .as_deref()
            .and_then(tako_core::handoff::master_profile_of_any_role)
            .unwrap_or(tako_core::handoff::DEFAULT_PROFILE)
            .to_string()
    });
    let out = tako_control::orchestrator::guide::json(topic, &profile_name)?;
    // #2256: 読めないプロファイルで既定値から組み立てたことは画面にも出す（本文は stdout のまま）
    for w in out["warnings"].as_array().into_iter().flatten() {
        if let Some(w) = w.as_str() {
            eprintln!("{}: {w}", ReplyNote::WarningLabel.text_in(cli_lang()));
        }
    }
    if as_json {
        println!("{}", pretty_json(&out));
        return Ok(());
    }
    match out["text"].as_str() {
        // 全文はそのまま出す（移動前のブロックと diff が取れる形にしておく）
        Some(text) => print!("{text}"),
        None => {
            let empty = vec![];
            println!("引ける手順書: {} 件", out["count"].as_u64().unwrap_or(0));
            for g in out["topics"].as_array().unwrap_or(&empty) {
                println!(
                    "  {:<16} {:>6} bytes / 約 {:>5} tok  {}",
                    g["topic"].as_str().unwrap_or("?"),
                    g["bytes"].as_u64().unwrap_or(0),
                    g["est_tokens"].as_u64().unwrap_or(0),
                    g["title"].as_str().unwrap_or("")
                );
            }
            if let Some(hint) = out["hint"].as_str() {
                println!("\n{hint}");
            }
        }
    }
    Ok(())
}

/// 起動時ロードの予算（#1139）。GUI も IPC も要らないローカル処理。
/// **壊れた設定で GUI が起動しないときにも引ける**ことが本質なので IPC 非依存にする
/// テスト・検証プロセスの残骸の掃除（Issue #1296）。
///
/// **既定は dry-run**。判定は `tako_core::test_residue` の 1 実装で、
/// 起動時の自動掃除（テストプロセス）とまったく同じ規則を使う
/// （「pid が生きていない」だけが削除の条件で、pid 再利用は起動時刻で見分けて見送る）
fn test_residue_local(args: &TestResidueArgs) -> Result<(), String> {
    use tako_core::test_residue as residue;

    let (temp, items, outcome) = residue::sweep_temp(args.apply);
    if args.apply {
        tako_control::diag::persist_log(&outcome.log_line());
    }
    if args.json {
        println!(
            "{}",
            pretty_json(&residue::report_json(&temp, &items, &outcome))
        );
        return Ok(());
    }

    println!("一時ディレクトリ: {}", temp.display());
    if items.is_empty() {
        println!("残骸はありません");
        return Ok(());
    }
    let total_bytes: u64 = items.iter().map(|r| r.bytes).sum();
    println!("残骸: {} 件 / {}", items.len(), human_bytes(total_bytes));
    for kind in residue::KINDS {
        let mine: Vec<&residue::Residue> =
            items.iter().filter(|r| r.prefix == kind.prefix).collect();
        if mine.is_empty() {
            continue;
        }
        let bytes: u64 = mine.iter().map(|r| r.bytes).sum();
        let removable = mine.iter().filter(|r| r.verdict.removable()).count();
        // 桁は種別名の実長に合わせる（#1312 で `tako-test-orchestrator-` が増え、
        // 固定 20 桁では列がずれた）
        let width = residue::KINDS
            .iter()
            .map(|k| k.prefix.len())
            .max()
            .unwrap_or(20);
        println!(
            "  {:<width$} {:>5} 件 / {:>9}（消せる {removable}）  {}",
            kind.prefix,
            mine.len(),
            human_bytes(bytes),
            kind.note
        );
    }
    // 見送った理由は**黙って飲み込まない**（「対象が無かった」と区別できるように）
    for (code, count) in &outcome.skipped {
        let detail = items
            .iter()
            .find(|r| r.verdict.code() == code)
            .map(|r| r.verdict.detail())
            .unwrap_or("消す直前の再判定で対象から外れた");
        println!("  見送り {count} 件: {detail}");
    }
    let removable_bytes: u64 = items
        .iter()
        .filter(|r| r.verdict.removable())
        .map(|r| r.bytes)
        .sum();
    if args.apply {
        println!(
            "\n消した: {} 件 / {}（失敗 {}{}）",
            outcome.removed,
            human_bytes(outcome.removed_bytes),
            outcome.failed,
            outcome
                .last_error
                .as_deref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        );
    } else {
        println!(
            "\n消せる: {} 件 / {}（まだ 1 つも消していない）",
            items.iter().filter(|r| r.verdict.removable()).count(),
            human_bytes(removable_bytes)
        );
        // 既定値で済む引数は付けない（#322 の最簡形）
        println!("いま消す: tako test-residue --apply");
    }
    Ok(())
}

/// バイト数を人が読める形へ（1 桁小数・1024 進）
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn context_budget_local(args: &ContextBudgetArgs) -> Result<(), String> {
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    let cwd = match args.cwd {
        Some(ref c) => std::path::PathBuf::from(tako_control::orchestrator::expand_tilde(c)),
        None => {
            std::env::current_dir().map_err(|e| front_err(Reason::CwdNotObtained(e.to_string())))?
        }
    };
    let fixing = args.action.as_deref() == Some("fix");
    let out = if fixing {
        tako_control::context_budget::fix(&cwd, args.profile.as_deref(), args.dry_run)?
    } else {
        tako_control::context_budget::report(&cwd, args.profile.as_deref())?
    };
    if args.json {
        println!("{}", pretty_json(&out));
        return Ok(());
    }
    if fixing {
        print_context_budget_fix(&out, args.dry_run);
    } else {
        print_context_budget_check(&out);
    }
    Ok(())
}

fn print_context_budget_check(out: &serde_json::Value) {
    let empty = vec![];
    let items = out["items"].as_array().unwrap_or(&empty);
    println!(
        "起動時ロード: {} 項目 / 概算 {} トークン",
        items.len(),
        out["totals"]["est_tokens"].as_u64().unwrap_or(0)
    );
    for it in items {
        let v = it["violations"].as_array();
        let mark = if v.is_some() { "!" } else { " " };
        println!(
            "{mark} {:<40} {:>7} bytes / 約 {:>6} tok  [{}]",
            it["path"].as_str().unwrap_or("?"),
            it["bytes"].as_u64().unwrap_or(0),
            it["est_tokens"].as_u64().unwrap_or(0),
            it["kind"].as_str().unwrap_or("?")
        );
        for x in v.into_iter().flatten() {
            println!(
                "      超過 {}: {} > {}（{}）",
                x["metric"].as_str().unwrap_or("?"),
                x["actual"].as_u64().unwrap_or(0),
                x["limit"].as_u64().unwrap_or(0),
                x["note"].as_str().unwrap_or("")
            );
            // #1675: `fix` で直る復活は提案に載らないので、どこに戻ったかをここで名指しする
            // （直らないものは提案の側が同じ名指しを出す）
            if x["fixable"] == true {
                if let Some(detail) = x["detail"].as_str() {
                    println!("        {detail}");
                }
            }
        }
    }
    let violations = out["violations"].as_u64().unwrap_or(0);
    if violations == 0 {
        println!("\n予算内");
        return;
    }
    // `tako master` の起動前 1 行・MCP の応答と同じ 1 行（#1715。予算超過と
    // 作業ログの復活・重複を言い分け、自動で直せるときだけ最簡形の `fix` を案内する = #322）
    if let Some(line) = out["startup_line"].as_str() {
        println!("\n{line}");
    }
    print_context_budget_proposals(out);
}

/// 自動で直せなかったぶんの案内。**system prompt は内訳（大きい順）まで出す**（#1154）:
/// 「大きい」だけ言われても、どの block を分ければ効くのか分からない
fn print_context_budget_proposals(out: &serde_json::Value) {
    let empty = vec![];
    for p in out["proposals"].as_array().unwrap_or(&empty) {
        println!(
            "  提案 {}: {}",
            p["path"].as_str().unwrap_or("?"),
            p["next_step"].as_str().unwrap_or("")
        );
        for piece in p["pieces"].as_array().unwrap_or(&empty).iter().take(5) {
            println!(
                "        {:<34} {:>7} bytes",
                piece["name"].as_str().unwrap_or("?"),
                piece["bytes"].as_u64().unwrap_or(0)
            );
        }
        // #1477: 追記を持つプロファイルには「区切りを何行上げるか」まで出す
        if let Some(advice) = p["advice"].as_str() {
            println!("        → {advice}");
        }
        // #1645: 復活・重複は相手側の行まで名指しする（どこを見ればいいかが分かる）
        if let Some(detail) = p["detail"].as_str() {
            println!("        {detail}");
        }
    }
}

fn print_context_budget_fix(out: &serde_json::Value, dry_run: bool) {
    let empty = vec![];
    if dry_run {
        println!("（dry-run: 書き込まない）");
    }
    for c in out["changes"].as_array().unwrap_or(&empty) {
        let path = c["path"].as_str().unwrap_or("?");
        if !c["changed"].as_bool().unwrap_or(false) {
            println!("{path}: 変更なし（予算内）");
            continue;
        }
        println!(
            "{path}: {} 件 → 残 {} 件 / archive へ {} 件（総数保存: {}）",
            c["entries_before"].as_u64().unwrap_or(0),
            c["entries_kept"].as_u64().unwrap_or(0),
            c["entries_archived"].as_u64().unwrap_or(0),
            if c["entries_preserved"].as_bool().unwrap_or(false) {
                "はい"
            } else {
                "いいえ"
            }
        );
        println!(
            "  {} → {} bytes（約 {} → {} トークン）",
            c["bytes_before"].as_u64().unwrap_or(0),
            c["bytes_after"].as_u64().unwrap_or(0),
            c["est_tokens_before"].as_u64().unwrap_or(0),
            c["est_tokens_after"].as_u64().unwrap_or(0)
        );
        let aged = c["archive_lines_removed_by_age"].as_u64().unwrap_or(0);
        if aged > 0 {
            println!("  保持期間を過ぎて archive から削除: {aged} 行");
        }
        // #1675: アーカイブに既に在るので作業ログ側を落としたエントリ（rebase で戻ったもの）。
        // 無い回は何も出さない（それまでの表示とバイト一致）
        let dropped = c["entries_dropped_revived"].as_u64().unwrap_or(0);
        if dropped > 0 {
            println!(
                "  {}",
                tako_control::context_budget::dropped_line_in(dropped, tako_core::i18n::lang())
            );
            for l in c["dropped_revived"].as_array().unwrap_or(&empty) {
                println!("    {}", l.as_str().unwrap_or(""));
            }
        }
        for l in c["archived_preview"].as_array().unwrap_or(&empty) {
            println!("    移送予定 {}", l.as_str().unwrap_or(""));
        }
    }
    println!("変更したファイル: {}", out["changed"].as_u64().unwrap_or(0));
    print_context_budget_proposals(out);
}

/// `tako check-health`（MCP `tako_check_health` と 1:1。#1441）。
///
/// アプリへ届けば dispatch の結果をそのまま出す（`ipc` 節つき）。**届かないときこそ
/// 答えが要る**ので、IPC の受け口だけをローカルで診断して出す。#1441 の症状
/// （深い `TAKO_DATA_DIR` でソケットが `sun_path` の上限を超え、警告 1 行で縮退する）は
/// アプリ側の記録も discovery も残らないので、ここが唯一の機械的な手がかりになる。
///
/// 表示言語は設定に従う（#2365）。届いたときの行は GUI が同じ設定から組み、CLI が並べる行は
/// [`cli_lang`]、届かないときの手元の診断は表示言語を決めてから組む
fn check_health_cli(json: bool) -> Result<(), String> {
    let lang = cli_lang();
    match send_request(Request::CheckHealth) {
        Ok(result) => {
            if json {
                println!("{}", pretty_json(&result));
            } else {
                print_check_health(&result, lang);
            }
            Ok(())
        }
        Err(reason) => {
            let local = local_ipc_report(lang);
            // #1505: 届かなくても**環境診断は答えられる**（項目の正本は tako-control 側で、
            // GUI を要らない）。ここで黙ると「tako が起動していないと何も分からない」に戻る。
            // 正本は表示言語のグローバルを読むので、組む前に決める（#2365）
            init_local_lang();
            let diagnostics = tako_control::diagnostics::collect();
            // #1891: tako mod の項目は GUI のメモリにしか無いので、理由つきの不明（A/B では載せない）
            let claude_mod = (!tako_core::claude_mod::no_report_legacy()).then(|| {
                tako_control::claude_mod::health_unavailable(
                    &HealthText::AppUnreachable(reason.clone()).text_in(lang),
                )
            });
            if json {
                println!(
                    "{}",
                    pretty_json(&serde_json::json!({
                        "connected": false,
                        "reason": reason,
                        "ipc": local,
                        "diagnostics": diagnostics.to_json(),
                        "claude_mod": claude_mod,
                    }))
                );
            } else {
                println!(
                    "{}",
                    HealthText::ConnectionUnreachable(reason).text_in(lang)
                );
                print_ipc_section(&local, lang);
                for line in diagnostics.lines() {
                    println!("{line}");
                }
                if let Some(section) = &claude_mod {
                    print_check_health_mod(section, lang);
                }
            }
            Err(HealthText::AppUnreachableRefusal.text_in(lang))
        }
    }
}

/// GUI に届かないときに添える 1 行（#1993。順番待ち #1994 は GUI の中の調停役が持つ）
const QUEUE_OFFLINE_NOTE: &str = "順番待ちは無効（tako が起動していない）";

/// GUI が繋がったまま応答しないときに添える 1 行（#2123）
fn queue_unresponsive_note(limit: std::time::Duration) -> String {
    format!(
        "順番待ちの状況は読めない（tako が {} 秒応答しない = GUI が固まっている可能性。\
         tako queue <コマンド> は待たずに走る）",
        limit.as_secs()
    )
}

/// 機全体の CPU% を出すために、GUI が居ないとき 2 回採る間隔
const QUEUE_OFFLINE_CPU_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

/// 順番待ちの要求を調停役へ送る（#1994）。`TAKO_QUEUE_SOCKET` を明示していればそこだけ
/// （隔離の調停役。本番へ漏れない）、無ければいつもの解決（env → 発見ファイル）。
/// 発見ファイルは `TAKO_ISOLATED=1` でも既定の置き場から引く（#2531。調停役はマシン全体で 1 つ =
/// 隔離した検証のビルドも本番の順番待ちへ並ぶ。ほかの要求は隔離の印で既定の置き場を読まない）。
/// 接続できない・応答の途中で切れた = 届かない、繋がったが上限まで応答しない = 応答しない（#2123）、
/// 届いて断られた = 断られた
fn queue_send(request: Request) -> Result<Value, tako_control::queue_wrapper::SendError> {
    use tako_control::queue_wrapper::SendError;
    if let Some(socket) = tako_control::queue::explicit_socket() {
        let token = std::env::var("TAKO_TOKEN").unwrap_or_default();
        return match transport::roundtrip(&socket, &token, request, None) {
            Ok(v) => Ok(v),
            Err(TransportError::Other(m)) => Err(SendError::Rejected(m)),
            Err(TransportError::Timeout(limit)) => Err(SendError::Timeout(limit)),
            Err(e) => Err(SendError::Unreachable(e.message())),
        };
    }
    match send_request_machine_wide(request) {
        Ok(v) => Ok(v),
        Err(SendFailure::Unreachable(m) | SendFailure::Broken(m)) => Err(SendError::Unreachable(m)),
        Err(SendFailure::Timeout(limit)) => Err(SendError::Timeout(limit)),
        Err(SendFailure::Operation(m)) => Err(SendError::Rejected(m)),
    }
}

fn queue_request(action: &str) -> Request {
    Request::Queue {
        action: Some(action.into()),
        job: None,
        id: None,
        limits: None,
    }
}

/// `tako queue <コマンド>`: 順番待ちで走らせる（#1994）。終了コードは子と同じ
fn queue_run_cli(args: &QueueArgs) -> ExitCode {
    use tako_control::queue_wrapper as w;
    let Some(QueueCommand::Run(argv)) = &args.command else {
        return ExitCode::FAILURE;
    };
    let priority_text = args.priority.clone().or_else(|| {
        std::env::var("TAKO_QUEUE_PRIORITY")
            .ok()
            .filter(|v| !v.is_empty())
    });
    let priority = match priority_text.as_deref() {
        None => tako_core::queue::Priority::default(),
        Some(p) => match tako_core::queue::Priority::parse(p) {
            Some(p) => p,
            None => {
                eprintln!(
                    "error: {}",
                    front_err(Reason::QueuePriorityUnknown(p.to_string()))
                );
                return ExitCode::from(2);
            }
        },
    };
    let max_wait_text = args.max_wait.clone().or_else(|| {
        std::env::var("TAKO_QUEUE_MAX_WAIT")
            .ok()
            .filter(|v| !v.is_empty())
    });
    let max_wait = match max_wait_text.as_deref().map(w::parse_duration) {
        None => None,
        Some(Ok(d)) => Some(d),
        Some(Err(e)) => {
            eprintln!("error: {}", front_err(e));
            return ExitCode::from(2);
        }
    };
    let opts = w::Options {
        priority,
        max_wait,
        label: args.label.clone(),
    };
    let code = w::run(argv, &opts, &mut queue_send);
    ExitCode::from(u8::try_from(code.clamp(0, 255)).unwrap_or(1))
}

/// `tako queue`（状況）/ `cancel` / `limits` / `serve`（#1993 / #1994）
fn queue_cli(args: &QueueArgs) -> Result<(), String> {
    use tako_control::queue_wrapper::SendError;
    match &args.command {
        None => queue_status_cli(args.json),
        Some(QueueCommand::Cancel { id, json }) => {
            let request = Request::Queue {
                action: Some("cancel".into()),
                job: None,
                id: Some(*id),
                limits: None,
            };
            let result = queue_send(request).map_err(|e| match e {
                SendError::Unreachable(_) => QUEUE_OFFLINE_NOTE.to_string(),
                SendError::Timeout(limit) => tako_control::protocol::reply_timeout_message(limit),
                SendError::Rejected(m) => m,
            })?;
            if *json {
                println!("{}", pretty_json(&result));
            } else {
                let what = if result["phase"] == "waiting" {
                    "待ちから外した（その tako queue は 125 で終わる）"
                } else {
                    "子を止める（SIGTERM → 10 秒で SIGKILL）"
                };
                println!(
                    "取り消した: {} {} — {what}",
                    result["id"],
                    result["label"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
        Some(QueueCommand::Start { id, json }) => {
            let request = Request::Queue {
                action: Some("start".into()),
                job: None,
                id: Some(*id),
                limits: None,
            };
            let result = queue_send(request).map_err(|e| match e {
                SendError::Unreachable(_) => QUEUE_OFFLINE_NOTE.to_string(),
                SendError::Timeout(limit) => tako_control::protocol::reply_timeout_message(limit),
                SendError::Rejected(m) => m,
            })?;
            if *json {
                println!("{}", pretty_json(&result));
            } else if result["state"] == "already_started" {
                println!("ペイン {} の agent はもう起こしている", result["pane"]);
            } else {
                println!(
                    "ペイン {} の agent を今すぐ起こす（待ち {}{}）",
                    result["pane"],
                    tako_control::queue_wrapper::human_secs(
                        result["waited_s"].as_u64().unwrap_or(0)
                    ),
                    result["was"]["reason"]
                        .as_str()
                        .map(|r| format!("・待っていた理由: {r}"))
                        .unwrap_or_default()
                );
            }
            Ok(())
        }
        Some(QueueCommand::Limits {
            max_jobs,
            reserve_gb,
            swap_guard_gb,
            max_wait_min,
            jobserver,
            reset,
            json,
        }) => {
            let patch = tako_core::queue::LimitsPatch {
                max_jobs: *max_jobs,
                reserve_gb: *reserve_gb,
                swap_guard_gb: *swap_guard_gb,
                max_wait_min: *max_wait_min,
                jobserver: jobserver.as_deref().map(|v| v == "on"),
                reset: *reset,
            };
            let request = Request::Queue {
                action: Some("limits".into()),
                job: None,
                id: None,
                limits: Some(patch.clone()),
            };
            // GUI が居なくても・応答しなくても設定ファイルは変えられる（調停役は次に mtime を見て読み直す）
            let result = match queue_send(request) {
                Ok(v) => v,
                Err(SendError::Unreachable(_) | SendError::Timeout(_)) => {
                    tako_control::queue::limits_offline(patch).map_err(local_dispatch_err)?
                }
                Err(SendError::Rejected(m)) => return Err(m),
            };
            if *json {
                println!("{}", pretty_json(&result));
            } else {
                print_queue_limits(&result);
            }
            Ok(())
        }
        Some(QueueCommand::Serve) => {
            let socket = tako_control::queue::explicit_socket()
                .ok_or_else(|| front_err(Reason::QueueSocketRequired))?;
            let token = std::env::var("TAKO_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| front_err(Reason::QueueTokenRequired))?;
            tako_control::queue::serve_headless(&socket, token).map_err(|e| e.to_string())
        }
        Some(QueueCommand::Run(_)) => unreachable!("cli_main が queue_run_cli へ回す"),
    }
}

/// `tako queue`（状況）: PC リソースと順番待ち（#1993 / #1994）。届かなければ機の状態だけをこのプロセスで採る
fn queue_status_cli(json: bool) -> Result<(), String> {
    use tako_control::queue_wrapper::SendError;
    let result = match queue_send(queue_request("status")) {
        Ok(result) => result,
        Err(SendError::Rejected(message)) => return Err(message),
        Err(SendError::Unreachable(reason)) => {
            let mut local = tako_control::resources::machine_only(QUEUE_OFFLINE_CPU_WINDOW);
            local["gui"] = serde_json::json!(false);
            local["note"] = serde_json::json!(QUEUE_OFFLINE_NOTE);
            local["reason"] = serde_json::json!(reason);
            local
        }
        // #2123: 繋がったまま応答しない GUI。機の状態はこのプロセスで採って返す
        Err(SendError::Timeout(limit)) => {
            let mut local = tako_control::resources::machine_only(QUEUE_OFFLINE_CPU_WINDOW);
            local["gui"] = serde_json::json!(false);
            local["note"] = serde_json::json!(queue_unresponsive_note(limit));
            local["reason"] =
                serde_json::json!(tako_control::protocol::reply_timeout_message(limit));
            local
        }
    };
    if json {
        println!("{}", pretty_json(&result));
    } else {
        print_queue_status(&result);
    }
    Ok(())
}

/// 上限の 1 行
fn print_queue_limits(v: &Value) {
    let configured = &v["configured"];
    let set: Vec<String> = [
        "max_jobs",
        "reserve_gb",
        "swap_guard_gb",
        "max_wait_min",
        "jobserver",
    ]
    .iter()
    .filter(|k| !configured[**k].is_null())
    .map(|k| format!("{k}={}", configured[*k]))
    .collect();
    println!(
        "同時 {} 件・予約 {} GB・swap の警戒線 {} GB・待ちの上限 {} 分・cargo の札 {}（{}）",
        v["max_jobs"],
        v["reserve_gb"],
        v["swap_guard_gb"],
        v["max_wait_min"],
        // 旧い GUI は欄を返さない（そのときは何も言わない）
        match v["jobserver"].as_bool() {
            Some(true) => "on",
            Some(false) => "off",
            None => "-",
        },
        if set.is_empty() {
            "すべて機に合わせた既定".to_string()
        } else {
            format!("設定: {}", set.join(" "))
        }
    );
}

/// 順番待ちの節の表示（#1994）
fn print_queue_section(q: &Value) {
    if q.is_null() {
        return;
    }
    if q["arbiter"] != true {
        println!(
            "順番待ち: {}",
            q["reason"].as_str().unwrap_or("この tako は調停役ではない")
        );
        return;
    }
    let running = q["running"].as_array().cloned().unwrap_or_default();
    let waiting = q["waiting"].as_array().cloned().unwrap_or_default();
    let limits = &q["limits"];
    let mut head = format!(
        "順番待ち: 実行 {} / 上限 {}・待ち {}（予約 {} GB）",
        running.len(),
        limits["max_jobs"],
        waiting.len(),
        limits["reserve_gb"]
    );
    if q["unmanaged"].as_u64().unwrap_or(0) > 0 {
        head.push_str(&format!(
            "・順番待ちを通っていない重い処理 {}",
            q["unmanaged"]
        ));
    }
    println!("{head}");
    let pane = |r: &Value| {
        r["pane"]
            .as_u64()
            .map(|p| format!("  ペイン {p}"))
            .unwrap_or_default()
    };
    for r in &running {
        let mem = match (r["memory_mb"].as_u64(), r["peak_mb"].as_u64()) {
            (Some(m), Some(p)) => format!("  {}（最大 {}）", human_mb(m), human_mb(p)),
            (Some(m), None) => format!("  {}", human_mb(m)),
            _ => String::new(),
        };
        // 後ろの空きメモリの判定で押さえている分（ピーク前の「まだ使っていない見込み」。#2254）
        let pending = r["pending_mb"]
            .as_u64()
            .filter(|p| *p > 0)
            .map(|p| {
                format!(
                    "  まだ使っていない見込み {}（見込み {}）",
                    human_mb(p),
                    human_mb(r["estimate_mb"].as_u64().unwrap_or(0))
                )
            })
            .unwrap_or_default();
        let mut flags = Vec::new();
        if r["stalled"] == true {
            flags.push("停滞（枠を返した）");
        }
        if r["cancel"] == true {
            flags.push("取り消し中");
        }
        println!(
            "  実行  {}  {}{}  {}{}{}  取り分 {}{}",
            r["id"],
            r["label"].as_str().unwrap_or("?"),
            pane(r),
            tako_control::queue_wrapper::human_secs(r["ran_s"].as_u64().unwrap_or(0)),
            mem,
            pending,
            r["share"],
            if flags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", flags.join("・"))
            }
        );
    }
    for w in &waiting {
        println!(
            "  待ち {}  {}  {}{}  {}  待ち {}{}",
            w["position"],
            w["id"],
            w["label"].as_str().unwrap_or("?"),
            pane(w),
            w["priority"].as_str().unwrap_or("normal"),
            tako_control::queue_wrapper::human_secs(w["waited_s"].as_u64().unwrap_or(0)),
            // 待ちごとの理由（#2243。target dir 待ちの先頭は追い越せるので、先頭の理由だけでは
            // 後ろが何を待っているか分からない）
            w["text"]
                .as_str()
                .map(|t| format!("  — {t}"))
                .unwrap_or_default()
        );
    }
    // 待ちごとの理由を返さない旧い GUI のときだけ、先頭の理由を 1 行で
    let per_job = waiting.iter().any(|w| w["text"].is_string());
    if let Some(text) = q["blocked"]["text"].as_str().filter(|_| !per_job) {
        println!("  先頭が待つ理由: {text}");
    }
    let recent = q["recent"].as_array().cloned().unwrap_or_default();
    // 直近に終わった件（諦めた待ちは下の「直近の諦め」で出す）
    if let Some(r) = recent.iter().find(|r| r["gave_up"] != true) {
        let end = if r["lost"] == true {
            "wrapper が消えた".to_string()
        } else {
            format!("終了 {}", r["exit_code"])
        };
        println!(
            "  直近: {}  {}  {}（{}{}）",
            r["id"],
            r["label"].as_str().unwrap_or("?"),
            end,
            tako_control::queue_wrapper::human_secs(r["ran_s"].as_u64().unwrap_or(0)),
            r["peak_mb"]
                .as_u64()
                .map(|p| format!("・最大 {}", human_mb(p)))
                .unwrap_or_default()
        );
    }
    // 待ちの上限で諦めた直近の件と、そのとき押さえていた処理（#2254）
    if let Some(r) = recent.iter().find(|r| r["gave_up"] == true) {
        println!(
            "  直近の諦め: {}  {}  待ち {}{}",
            r["id"],
            r["label"].as_str().unwrap_or("?"),
            tako_control::queue_wrapper::human_secs(r["waited_s"].as_u64().unwrap_or(0)),
            r["text"]
                .as_str()
                .map(|t| format!("  — {t}"))
                .unwrap_or_default()
        );
        let lines = tako_control::queue_wrapper::gave_up_lines(&serde_json::json!({
            "holders": r["holders"],
        }));
        for line in lines {
            println!("    {line}");
        }
    }
}

// MiB → 人が読む GB / MB（wrapper の諦めたときの表示と同じ 1 実装。#2254）
use tako_control::queue_wrapper::human_mb;

/// 誰の処理か（ペインなら番号・タブ・役割、外なら起点のアプリ）
fn queue_owner_text(v: &Value) -> String {
    if let Some(origin) = v["external"].as_str() {
        return if origin.is_empty() {
            "tako の外".to_string()
        } else {
            format!("tako の外（{origin}）")
        };
    }
    let mut parts = vec![format!("ペイン {}", v["pane"].as_u64().unwrap_or(0))];
    for key in ["tab", "role"] {
        if let Some(t) = v[key].as_str().filter(|t| !t.is_empty()) {
            parts.push(t.to_string());
        }
    }
    parts.join(" · ")
}

/// `tako queue` の人間向け表示
fn print_queue_status(v: &Value) {
    if let Some(note) = v["note"].as_str() {
        println!("{note}");
    }
    let m = &v["machine"];
    if m.is_null() {
        println!(
            "機の状態: {}",
            v["machine_unavailable"].as_str().unwrap_or("採れない")
        );
    } else {
        let mem = &m["memory_mb"];
        let n = |key: &str| mem[key].as_u64().unwrap_or(0);
        let mut line = format!(
            "メモリ {} / {}（app {}・wired {}・圧縮 {}・空き {}）",
            human_mb(n("used")),
            human_mb(n("total")),
            human_mb(n("app")),
            human_mb(n("wired")),
            human_mb(n("compressed")),
            human_mb(n("available")),
        );
        if let Some(swap) = m["swap_mb"].as_object() {
            line.push_str(&format!(
                "  swap {} / {}",
                human_mb(swap["used"].as_u64().unwrap_or(0)),
                human_mb(swap["total"].as_u64().unwrap_or(0))
            ));
        }
        if let Some(p) = m["pressure"].as_str() {
            line.push_str(&format!("  メモリ圧 {p}"));
        }
        println!("{line}");
        let mut cpu = format!(
            "CPU {}",
            m["cpu_percent"]
                .as_f64()
                .map(|c| format!("{c:.0}%"))
                .unwrap_or_else(|| "?".into())
        );
        cpu.push_str(&format!("（{} コア）", m["cpus"].as_u64().unwrap_or(0)));
        if let Some(load) = m["load"].as_array() {
            let l: Vec<String> = load
                .iter()
                .filter_map(|x| x.as_f64())
                .map(|x| format!("{x:.1}"))
                .collect();
            cpu.push_str(&format!("  load {}", l.join(" / ")));
        }
        println!("{cpu}");
    }
    if v["gui"] == false {
        return;
    }
    let heavy = v["heavy"].as_array().cloned().unwrap_or_default();
    if heavy.is_empty() {
        println!("重い処理: なし");
    } else {
        println!("重い処理 {} 件:", heavy.len());
        for job in &heavy {
            println!(
                "  {}  {}  {}  {}",
                job["label"].as_str().unwrap_or("?"),
                human_mb(job["memory_mb"].as_u64().unwrap_or(0)),
                job["cpu_percent"]
                    .as_f64()
                    .map(|c| format!("{c:.0}%"))
                    .unwrap_or_else(|| "-".into()),
                queue_owner_text(job)
            );
        }
    }
    let panes = v["panes"].as_array().cloned().unwrap_or_default();
    if !panes.is_empty() {
        println!("ペインの上位（メモリの多い順）:");
        for p in &panes {
            let agent = p["agent"]
                .as_str()
                .map(|a| format!("  {a}"))
                .unwrap_or_default();
            println!(
                "  {}（agent {}・重い処理 {}）  {}{}",
                human_mb(p["memory_mb"].as_u64().unwrap_or(0)),
                human_mb(p["agent_mb"].as_u64().unwrap_or(0)),
                human_mb(p["heavy_mb"].as_u64().unwrap_or(0)),
                queue_owner_text(p),
                agent
            );
        }
    }
    let ext = &v["external"];
    if ext.is_object() {
        println!(
            "tako の外: {}・{} 本（重い処理 {} 件）",
            human_mb(ext["memory_mb"].as_u64().unwrap_or(0)),
            ext["processes"].as_u64().unwrap_or(0),
            ext["heavy"].as_u64().unwrap_or(0)
        );
    }
    let scan = &v["scan"];
    if scan.is_object() {
        println!(
            "走査: {} 本（読めた {}）・{:.1} 秒前",
            scan["processes"].as_u64().unwrap_or(0),
            scan["readable"].as_u64().unwrap_or(0),
            scan["age_ms"].as_u64().unwrap_or(0) as f64 / 1000.0
        );
    }
    print_queue_section(&v["queue"]);
    print_launches_section(&v["launches"]);
    print_jobserver_section(&v["jobserver"]);
}

/// cargo の札の節の表示（#1998）。旧い GUI（節が無い）では何も出さない
fn print_jobserver_section(j: &Value) {
    if !j.is_object() {
        return;
    }
    match j["state"].as_str() {
        Some("active") => {
            let mut line = format!(
                "cargo の札: 空き {} / {}",
                j["free"].as_u64().map_or("?".into(), |f| f.to_string()),
                j["tokens"]
            );
            if let Some(h) = j["holders"].as_u64() {
                line.push_str(&format!("（札を持ちうる cargo 系 {h} 本）"));
            }
            println!("{line}");
            if let Some(r) = j["last_recount"].as_object() {
                let added = r["added"].as_u64().unwrap_or(0);
                let removed = r["removed"].as_u64().unwrap_or(0);
                println!(
                    "  直近の数え直し: 空き {} → {}{}（{}）",
                    r["free_before"],
                    if added > 0 { "+" } else { "-" },
                    added.max(removed),
                    if r["why"] == "over" {
                        "多すぎた分を抜いた"
                    } else {
                        "誰も持っていないのに足りない分を足した"
                    }
                );
            }
        }
        Some("follower") => println!("cargo の札: 先行の tako の札を使う（総数 {}）", j["tokens"]),
        _ => println!(
            "cargo の札: 配っていない — {}",
            j["reason"].as_str().unwrap_or("?")
        ),
    }
}

/// agent の起動待ちの節の表示（#2413）。待ちも立ち上がり中も無ければ何も出さない
fn print_launches_section(l: &Value) {
    let waiting = l["waiting"].as_array().cloned().unwrap_or_default();
    let starting = l["starting"].as_array().cloned().unwrap_or_default();
    if waiting.is_empty() && starting.is_empty() {
        return;
    }
    println!(
        "agent の起動待ち: 待ち {}・立ち上がり中 {}（逼迫が解けると起きる。今すぐ起こすには tako queue start <ペイン>）",
        waiting.len(),
        starting.len()
    );
    for w in &waiting {
        println!(
            "  待ち {}  ペイン {}  {}  {}  {}  待ち {}  — {}",
            w["position"],
            w["pane"],
            w["kind"].as_str().unwrap_or("?"),
            w["label"].as_str().unwrap_or(""),
            w["priority"].as_str().unwrap_or("normal"),
            tako_control::queue_wrapper::human_secs(w["waited_s"].as_u64().unwrap_or(0)),
            w["reason"].as_str().unwrap_or("?")
        );
    }
    for r in &starting {
        println!(
            "  立ち上がり中  ペイン {}  {}  {}  {}秒{}",
            r["pane"],
            r["kind"].as_str().unwrap_or("?"),
            r["label"].as_str().unwrap_or(""),
            r["since_s"].as_u64().unwrap_or(0),
            r["memory_mb"]
                .as_u64()
                .map(|m| format!("  {}", human_mb(m)))
                .unwrap_or_default()
        );
    }
}

/// `ipc.pipe` の 1 行（#2361。節が無ければ `None` = windows 以外・旧版の応答）
fn pipe_line(pipe: &Value, lang: tako_core::i18n::Lang) -> Option<String> {
    if !pipe.is_object() {
        return None;
    }
    let max_secs = pipe["retry_max_ms"].as_u64().unwrap_or(0) / 1000;
    if pipe["listening"].as_bool() == Some(false) {
        return Some(
            HealthText::PipeDownLine {
                streak: pipe["streak"].as_u64().unwrap_or(0),
                error: pipe["error"].as_str().map(str::to_string),
                retry_ms: pipe["retry_ms"].as_u64().unwrap_or(0),
                max_secs,
            }
            .text_in(lang),
        );
    }
    let outages = pipe["outages"].as_u64().unwrap_or(0);
    if outages == 0 {
        return Some(HealthText::PipeOk(max_secs).text_in(lang));
    }
    let mut line = HealthText::PipeOkWithOutages {
        outages,
        failures: pipe["failures"].as_u64().unwrap_or(0),
    }
    .text_in(lang);
    if let Some(last) = pipe["last_recovery"].as_object() {
        line.push_str(
            &HealthText::PipeLastRecovery {
                secs_ago: last.get("secs_ago").and_then(Value::as_u64).unwrap_or(0),
                failures: last.get("failures").and_then(Value::as_u64).unwrap_or(0),
                error: last
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            }
            .text_in(lang),
        );
    }
    Some(line)
}

/// 繋ぐ側からの受け口の診断（#1441）。置き場の決め方は
/// `tako_core::ipc_socket` の 1 実装を通す（bind 側と同じ規則で引く）
fn local_ipc_report(lang: tako_core::i18n::Lang) -> Value {
    let Some(data_dir) = tako_core::paths::data_dir() else {
        return serde_json::json!({ "resolved": false });
    };
    let temp_dir = std::env::temp_dir();
    let limit = tako_core::ipc_socket::max_path_bytes();
    // `kind` は**決め方**（この data dir ならどこへ置くはずか）、`socket` は
    // **繋ぎ先**（参照ファイルがあればそちらが正）。両者がズレていたら
    // 「前の世代が別の場所へ置いた」ことが読める
    let plan = tako_core::ipc_socket::plan_with(&data_dir, &temp_dir, limit);
    let socket = tako_core::ipc_socket::resolve_with(&data_dir, &temp_dir, limit);
    let exists = socket.exists();
    serde_json::json!({
        "resolved": true,
        "data_dir": data_dir.display().to_string(),
        "socket": socket.display().to_string(),
        "kind": plan.kind.as_str(),
        "path_bytes": tako_core::ipc_socket::path_bytes(&socket),
        "limit": limit,
        "well_known_bytes": plan.well_known_bytes,
        "socket_exists": exists,
        "connect_ok": exists && tako_control::discovery::socket_alive(&socket.to_string_lossy()),
        "pointer": tako_core::ipc_socket::read_pointer(&data_dir)
            .map(|p| p.display().to_string()),
        // #2002: 届かないのは GUI が居ないからか、居るのに受け口が応答しないからか
        "gui": (!tako_core::ipc_socket::legacy_2002()).then(|| local_gui_report(lang)).flatten(),
    })
}

/// 接続情報ファイルの GUI が生きているか・その受け口へ繋がるか（#2002）。
/// 接続情報ファイルが無ければ `None`（GUI を起動したことが無い・片付けられた）
fn local_gui_report(lang: tako_core::i18n::Lang) -> Option<Value> {
    let info = tako_control::discovery::read()?;
    let alive = if cfg!(target_os = "macos") {
        tako_core::ports::is_live_tako_app(info.pid)
    } else {
        tako_core::platform::process::pid_alive(info.pid)
    };
    let (connect, error) = probe_socket(&info.socket);
    let state = match (alive, connect) {
        (false, _) => "not_running",
        (true, "ok") => "reachable",
        (true, _) => "alive_unreachable",
    };
    let next = match state {
        "not_running" => Some(HealthText::StartTako.text_in(lang)),
        "alive_unreachable" => Some(alive_unreachable_next(cfg!(windows), lang)),
        _ => None,
    };
    Some(serde_json::json!({
        "pid": info.pid,
        "alive": alive,
        "socket": info.socket,
        "connect": connect,
        "connect_error": error,
        "state": state,
        "next": next,
    }))
}

/// GUI は動いているのに受け口が応答しないときの次の一手（#2002 / #2361）。
/// 立て直し方が OS で違う（unix = パスの見張りと張り直し・windows = パイプの待ち受けの作り直し）
fn alive_unreachable_next(windows: bool, lang: tako_core::i18n::Lang) -> String {
    let secs = if windows {
        tako_core::ipc_socket::PIPE_RETRY_MAX_MS / 1000
    } else {
        tako_core::ipc_socket::WATCH_INTERVAL_MS_DEFAULT / 1000
    };
    HealthText::AliveUnreachableNext { windows, secs }.text_in(lang)
}

/// 受け口へ 1 本繋いでみる（認証はしない）。`ok` / `refused`（ファイルはあるが誰も受けない）/
/// `missing`（ファイルが無い）/ `error`
fn probe_socket(socket: &str) -> (&'static str, Option<String>) {
    #[cfg(unix)]
    {
        match std::os::unix::net::UnixStream::connect(socket) {
            Ok(_) => ("ok", None),
            Err(e) => (
                match e.kind() {
                    std::io::ErrorKind::ConnectionRefused => "refused",
                    std::io::ErrorKind::NotFound => "missing",
                    _ => "error",
                },
                Some(e.to_string()),
            ),
        }
    }
    #[cfg(not(unix))]
    {
        if tako_control::discovery::socket_alive(socket) {
            ("ok", None)
        } else {
            ("error", None)
        }
    }
}

/// 人が読む要約（`--json` 無しのとき）。表示言語は CLI の設定（#2365）。
/// 診断と tako mod の行は GUI が同じ設定の言語で組んだものをそのまま出す
fn print_check_health(result: &Value, lang: tako_core::i18n::Lang) {
    println!(
        "{}",
        HealthText::Healthy(result["healthy"].as_bool() == Some(true)).text_in(lang)
    );
    println!(
        "{}",
        HealthText::Versions {
            app: result["app_version"].as_str().unwrap_or("?").to_string(),
            cli: result["cli_version"].as_str().map(str::to_string),
        }
        .text_in(lang)
    );
    if let Some(ipc) = result.get("ipc").filter(|v| !v.is_null()) {
        print_ipc_section(ipc, lang);
    }
    if let Some(line) = result
        .get("fd_limit")
        .and_then(|fd| fd_limit_line(fd, lang))
    {
        println!("{line}");
    }
    if let Some(line) = result
        .get("fd_limit")
        .and_then(|fd| fd_container_line(fd, lang))
    {
        println!("{line}");
    }
    // #2350: 端末の問い合わせ（GUI が組んだ 1 行をそのまま出す）
    if let Some(line) = result["terminal_queries"]["summary"].as_str() {
        println!("{line}");
    }
    // #1505: 環境診断の項目。**`tako setup --check` と同じ行**（正本が組んだ文字列を
    // そのまま出すので、2 つのコマンドの答えが字面まで一致する）
    for line in result["diagnostics"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| item["lines"].as_array().into_iter().flatten())
        .filter_map(|line| line.as_str())
    {
        println!("{line}");
    }
    // #1891: tako mod の項目（`tako mod` と同じ関数で同じ字面。A/B では節が無い）
    if let Some(section) = result.get("claude_mod").filter(|v| v.is_object()) {
        print_check_health_mod(section, lang);
    }
    for issue in result["issues"].as_array().into_iter().flatten() {
        println!(
            "[{}] {}: {}",
            issue["level"].as_str().unwrap_or("?"),
            issue["check"].as_str().unwrap_or("?"),
            issue["message"].as_str().unwrap_or(""),
        );
    }
}

/// `tako check-health` の tako mod の項目（#1891）。行はすべて `tako mod` が出す行と同じ字面
/// （1 行目・設定 dir ごとの skills/tako・報告が届いていないペインの理由）。check-health の枠と
/// 同じ表示言語で引く（節の理由の文も GUI が同じ言語で組む。#2365）
fn print_check_health_mod(section: &Value, lang: tako_core::i18n::Lang) {
    println!(
        "{}",
        tako_control::claude_mod::text::injection_line(section, lang)
    );
    if section["skills"]["targets"].is_array() {
        println!("{}", ModText::SkillsHeading.text_in(lang));
        print_mod_skills(&section["skills"], lang);
    }
    for row in section["panes"].as_array().into_iter().flatten() {
        if let Some(line) = tako_control::claude_mod::text::reason_line(row) {
            println!("{line}");
        }
    }
}

/// `check-health` の fd の 1 行（#2359）。使用数 / 上限と起動時の引き上げ。
/// 上限の無い OS（`applicable=false`）・節の無い旧版の応答は None（何も出さない）
fn fd_limit_line(fd: &Value, lang: tako_core::i18n::Lang) -> Option<String> {
    if fd["applicable"].as_bool() != Some(true) {
        return None;
    }
    let num = |v: &Value| {
        v.as_u64()
            .map_or_else(|| "?".to_string(), |n| n.to_string())
    };
    let mut line = HealthText::FdUsage {
        open: num(&fd["open"]),
        soft: num(&fd["soft"]),
        percent: num(&fd["percent"]),
    }
    .text_in(lang);
    let startup = &fd["startup"];
    let launch = num(&startup["soft_at_launch"]);
    let note = match startup["outcome"].as_str() {
        Some("raised") => Some(HealthText::FdRaised {
            launch,
            soft: num(&fd["soft"]),
        }),
        Some("already_enough") => Some(HealthText::FdAlreadyEnough(launch)),
        Some("hard_capped") => Some(HealthText::FdHardCapped(num(&fd["hard"]))),
        Some("failed") => Some(HealthText::FdFailed(
            startup["error"].as_str().map(str::to_string),
        )),
        Some("legacy") => Some(HealthText::FdLegacy),
        _ => None,
    };
    if let Some(note) = note {
        line.push_str(&HealthText::NoteSeparator.text_in(lang));
        line.push_str(&note.text_in(lang));
    }
    Some(line)
}

/// `check-health` の器の tmux サーバーの 1 行（#2367）。使用数 / 上限と上限がどう決まったか。
/// 器が tmux でない（`container=null`）・節の無い旧版の応答は None（何も出さない）
fn fd_container_line(fd: &Value, lang: tako_core::i18n::Lang) -> Option<String> {
    let c = fd.get("container").filter(|c| c.is_object())?;
    let num = |v: &Value| {
        v.as_u64()
            .map_or_else(|| "?".to_string(), |n| n.to_string())
    };
    let mut line = match c["state"].as_str() {
        Some("not_running") => return Some(HealthText::ContainerNotRunning.text_in(lang)),
        Some("unreachable") => {
            HealthText::ContainerUnreachable(c["reason"].as_str().map(str::to_string))
        }
        _ => HealthText::ContainerUsage {
            open: num(&c["open"]),
            soft: num(&c["soft"]),
            percent: num(&c["percent"]),
        },
    }
    .text_in(lang);
    let note = match c["limit"].as_str() {
        Some("raised") => Some(HealthText::ContainerRaised(num(&c["children_soft"]))),
        Some("predates") => Some(HealthText::ContainerPredates(num(&c["pane_capacity"]))),
        Some("legacy") => Some(HealthText::ContainerLegacy),
        Some("launch_value") => Some(HealthText::ContainerLaunchValue),
        _ => None,
    };
    if let Some(note) = note {
        line.push_str(&HealthText::NoteSeparator.text_in(lang));
        line.push_str(&note.text_in(lang));
    }
    Some(line)
}

/// IPC の受け口 1 行（アプリ経由・ローカル診断のどちらも同じ形で出す）
fn print_ipc_section(ipc: &Value, lang: tako_core::i18n::Lang) {
    println!(
        "IPC: bound={} kind={} path_bytes={} limit={} well_known_bytes={}",
        ipc["bound"].as_bool().unwrap_or(false) || ipc["connect_ok"].as_bool().unwrap_or(false),
        ipc["kind"].as_str().unwrap_or("?"),
        ipc["path_bytes"].as_u64().unwrap_or(0),
        ipc["limit"].as_u64().unwrap_or(0),
        ipc["well_known_bytes"].as_u64().unwrap_or(0),
    );
    if let Some(endpoint) = ipc["endpoint"].as_str().or(ipc["socket"].as_str()) {
        println!(
            "{}",
            HealthText::IpcEndpoint(endpoint.to_string()).text_in(lang)
        );
    }
    if let Some(error) = ipc["error"].as_str() {
        println!(
            "{}",
            HealthText::IpcErrorLine(error.to_string()).text_in(lang)
        );
    }
    for line in ipc_recovery_lines(ipc, lang) {
        println!("{line}");
    }
}

/// `ipc` 節の #2002 の行（GUI 側の見張り・届かないときの GUI の生死と次の一手）。
/// 行を組むだけにしてある（単体テストが字面を固定する）
fn ipc_recovery_lines(ipc: &Value, lang: tako_core::i18n::Lang) -> Vec<String> {
    let mut lines = Vec::new();
    // GUI 側の見張り（届いたときだけある）
    let watch = &ipc["watch"];
    if watch.is_object() {
        let mut line = HealthText::WatchCounts {
            secs: watch["interval_ms"].as_u64().unwrap_or(0) / 1000,
            checks: watch["checks"].as_u64().unwrap_or(0),
            rebinds: watch["rebinds"].as_u64().unwrap_or(0),
        }
        .text_in(lang);
        if let Some(last) = watch["last_rebind"].as_object() {
            line.push_str(
                &HealthText::WatchLastRebind {
                    secs_ago: last.get("secs_ago").and_then(Value::as_u64).unwrap_or(0),
                    reason: last
                        .get("reason_text")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                }
                .text_in(lang),
            );
        }
        lines.push(line);
        if let Some(error) = watch["error"].as_str() {
            lines.push(HealthText::WatchRebindError(error.to_string()).text_in(lang));
        }
    }
    // #2361: Windows の名前付きパイプの待ち受け（作れない間は間を置いて作り直す）
    if let Some(line) = pipe_line(&ipc["pipe"], lang) {
        lines.push(line);
    }
    // #2360: 起動時に固定パスで見た先客（奪わずに一時パスで立てた・残骸を作り直した）
    let fixed = &ipc["fixed_path"];
    if fixed.is_object() {
        let reason = fixed["reason_text"].as_str().unwrap_or("?").to_string();
        lines.push(
            if fixed["held"].as_bool().unwrap_or(false) {
                HealthText::FixedPathHeld(reason)
            } else {
                HealthText::FixedPathReclaimed(reason)
            }
            .text_in(lang),
        );
    }
    // 届かないときの GUI の生死（届かない原因と次の一手を分ける）
    let gui = &ipc["gui"];
    if gui.is_object() {
        let pid = gui["pid"].as_u64().unwrap_or(0);
        let state_line = match gui["state"].as_str() {
            Some("alive_unreachable") => Some(HealthText::GuiAliveUnreachable {
                pid,
                why: match gui["connect"].as_str() {
                    Some("refused") => HealthText::SocketRefused.text_in(lang),
                    Some("missing") => HealthText::SocketMissing.text_in(lang),
                    _ => gui["connect_error"]
                        .as_str()
                        .map_or_else(|| HealthText::NotConnected.text_in(lang), str::to_string),
                },
            }),
            Some("not_running") => Some(HealthText::GuiNotRunning(pid)),
            Some("reachable") => Some(HealthText::GuiReachable(pid)),
            _ => None,
        };
        lines.extend(state_line.map(|text| text.text_in(lang)));
        if let Some(next) = gui["next"].as_str() {
            lines.push(HealthText::NextStep(next.to_string()).text_in(lang));
        }
    }
    lines
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
    // 復元の直後の全滅で救済・見送りしたときに退避した構成（#641）
    let rescued = path.with_extension(tako_control::restore_guard::RESCUED_EXT);
    println!("layout.json.rescued : {}", describe(&rescued));
    if let Some(record) = tako_control::restore_guard::load() {
        println!(
            "復元の直後の全滅    : 連続 {} 回（{} 回続くと次の起動は復元を見送る）",
            record.count,
            tako_control::restore_guard::SKIP_AFTER
        );
    }
    eprintln!();
    // 打鍵の表記は正本から引く（Windows は Ctrl+Shift+Q。#1203）
    let quit_key =
        tako_core::platform::keys::quit(tako_core::platform::support::Platform::current());
    eprintln!("復元するには: tako を終了（{quit_key}）してから `tako recover --apply <世代>` →");
    eprintln!("tako を再起動すると復元されたレイアウトで立ち上がります。");
    eprintln!("実体の tmux セッションが生きていれば、実行中プロセスごと画面に戻ります。");
    eprintln!("（good = 最後に復元へ成功した良品。`tako recover --apply good` で戻せます）");
    if rescued.is_file() {
        eprintln!(
            "（rescued = 復元の直後に全ペインが終わったので退避した構成。`tako recover --apply rescued` で戻せます）"
        );
    }
    Ok(())
}

/// バックアップ世代（1〜3 / good / rescued）を layout.json へ復元する
/// （現行は layout.json.pre-recover へ退避）
fn recover_apply(path: &std::path::Path, generation: &str, force: bool) -> Result<(), String> {
    let bak = match generation {
        "1" | "2" | "3" => tako_control::config_io::backup_path(path, generation.parse().unwrap()),
        "good" => path.with_extension("json.good"),
        // #641: 復元の直後の全滅で救済・見送りしたときの退避
        "rescued" => path.with_extension(tako_control::restore_guard::RESCUED_EXT),
        other => {
            return Err(format!(
                "世代は 1〜3・good・rescued のどれかで指定してください（指定: {other}）"
            ))
        }
    };
    // 稼働中の tako があると、復元した layout.json を定期保存が即上書きしてしまう。
    // discovery（control.json）と全プロセス走査の両方で確認する（#177 の教訓:
    // control.json は消えている・別を指していることがある）
    if !force {
        if let Some(pid) = tako_control::discovery::live_primary_pid() {
            let quit_key =
                tako_core::platform::keys::quit(tako_core::platform::support::Platform::current());
            return Err(format!(
                "tako（pid {pid}）が稼働中です。終了（{quit_key}）してから実行してください（--force で強制実行）"
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
    // #1858: 世代をそのまま写すと、バックアップより後に作られたタブ（復旧をしている
    // master 自身のタブを含む）が消える。現在の layout.json にあって世代に無いタブを、
    // 起動時に刈り込まれるだけのタブ（器の消えた master / worker のペインしか無い）以外は
    // 合流させる（判断の正本は `layout_reconcile::merge_after_backup`。
    // A/B は `TAKO_1858_LEGACY=1`）
    let merge = if tako_control::layout_reconcile::legacy() || !path.is_file() {
        None
    } else {
        match tako_control::layout::load_file(path) {
            Ok(current) => Some(tako_control::layout_reconcile::merge_after_backup(
                &layout,
                &current,
                tako_core::tmux::live_session_names(Some(&tako_core::tmux_backend::socket_name())),
            )),
            Err(e) => {
                eprintln!(
                    "現在の layout.json を読めないので、バックアップより後のタブは合流しない: {e}"
                );
                None
            }
        }
    };
    if path.is_file() {
        let stash = path.with_extension("json.pre-recover");
        std::fs::copy(path, &stash).map_err(|e| format!("現在の layout.json の退避に失敗: {e}"))?;
        eprintln!("現在の layout.json → {} へ退避", stash.display());
    }
    match &merge {
        // 合流があるときだけ書き直す（無ければ世代とバイト一致のコピーのまま）
        Some((merged, report)) if !report.merged.is_empty() => {
            tako_control::layout::write_file(path, merged)
                .map_err(|e| format!("復元の書き込みに失敗: {e}"))?
        }
        _ => {
            std::fs::copy(&bak, path).map_err(|e| format!("復元コピーに失敗: {e}"))?;
        }
    }
    eprintln!(
        "{}（{} タブ / {} ペイン）→ layout.json へ復元しました。",
        bak.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| bak.display().to_string()),
        layout.tabs.len(),
        layout.pane_count()
    );
    if let Some((merged, report)) = &merge {
        print_recover_merge(merged, report);
        if let Some(line) = report.log_line(generation) {
            tako_control::diag::persist_log(&line);
        }
    }
    eprintln!("tako を起動すると、このレイアウトで復元されます。");
    Ok(())
}

/// `recover --apply` の合流の結果（#1858）。タブ名はここ（利用者へ見せる出力）にだけ出す
fn print_recover_merge(
    merged: &tako_control::layout::LayoutFile,
    report: &tako_control::layout_reconcile::MergeReport,
) {
    if !report.merged.is_empty() {
        eprintln!(
            "バックアップより後に作られたタブを {} 枚合流しました:",
            report.merged.len()
        );
        for t in &report.merged {
            let mut notes = vec![format!("tmux セッションあり {}", t.alive)];
            if t.dead > 0 {
                notes.push(format!(
                    "終わっている {}（master / worker のペインは起動時に外し、ほかは開き直す）",
                    t.dead
                ));
            }
            if t.duplicates > 0 {
                notes.push(format!(
                    "バックアップ側にあるペイン {} を除いた",
                    t.duplicates
                ));
            }
            eprintln!(
                "  タブ {}「{}」: {} ペイン（{}）",
                t.id,
                t.title,
                t.panes,
                notes.join("・")
            );
        }
    }
    if !report.skipped.is_empty() {
        eprintln!("合流しなかったタブ {} 枚:", report.skipped.len());
        for t in &report.skipped {
            eprintln!("  タブ {}「{}」: {}", t.id, t.title, t.reason.label());
        }
    }
    if let Some(untrusted) = report.untrusted {
        if !report.merged.is_empty() {
            eprintln!(
                "（{}ので、tmux セッションの生死は確かめずに合流しました）",
                untrusted.label()
            );
        }
    }
    if !report.merged.is_empty() {
        eprintln!(
            "合流後: {} タブ / {} ペイン",
            merged.tabs.len(),
            merged.pane_count()
        );
    }
}

/// `sleep-guard status` の状態を採る（#372）。
///
/// **実行時の状態はアプリのプロセスが持つ**（アサーション ID と `busy_agents` は
/// どちらもプロセスローカルな static）ので、この CLI プロセスで計算すると
/// アプリが保持していても常に「未保持 / busy 0」になる。アプリが起動していれば
/// IPC（= MCP の `tako_sleep_guard` と同じ dispatch）で取りに行き、
/// 未起動ならこのプロセスで計算する。返り値の第 2 要素は取得元の表示用ラベル
fn sleep_guard_status_state() -> (tako_control::sleep_guard::SleepGuardState, &'static str) {
    let from_app = send_request(Request::SleepGuard {
        action: Some("status".to_string()),
        mode: None,
        power_condition: None,
        lid_sleep_mode: None,
        lid_power_condition: None,
        lid_battery_floor: None,
    })
    .ok()
    .and_then(|value| tako_control::sleep_guard::SleepGuardState::from_json(&value));
    if let Some(state) = from_app {
        return (state, "tako アプリ（IPC）");
    }
    let settings = tako_control::settings::load();
    (
        tako_control::sleep_guard::status(settings.sleep_guard_config()),
        "この CLI プロセス（tako アプリは未起動）",
    )
}

fn sleep_guard_local(sub: &SleepGuardCommand) -> Result<(), String> {
    match sub {
        SleepGuardCommand::Status => {
            let (state, source) = sleep_guard_status_state();
            eprintln!("  取得元: {source}");
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
            // #1473: 蓋閉じ継続の電源条件と、効いていないなら**その理由**
            if tako_control::sleep_guard::lid_control_supported() {
                eprintln!(
                    "  蓋閉じ継続の電源条件: {}",
                    state.lid_power_condition.as_str()
                );
                match state.battery_percent {
                    Some(percent) => eprintln!(
                        "  バッテリー残量: {percent}%（下限 {}%）",
                        state.lid_battery_floor
                    ),
                    None => eprintln!("  バッテリー残量: 不明（この機械では取得できません）"),
                }
                if !state.lid_sleep_disabled {
                    if let Some(reason) = state.lid_skip_reason {
                        eprintln!("  蓋閉じ継続が効かない理由: {}", reason.describe());
                    }
                }
            }
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
            lid_power_condition,
            lid_battery_floor,
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
            // #1473: 蓋閉じ継続の電源条件と残量下限（アイドルスリープ側とは別の軸）
            if let Some(pc) = lid_power_condition {
                settings.lid_sleep_power = tako_control::sleep_guard::PowerCondition::from_str_opt(
                    pc,
                )
                .ok_or_else(|| {
                    format!(
                        "不明な lid-power-condition: {pc:?}（{}）",
                        tako_control::sleep_guard::PowerCondition::values_hint()
                    )
                })?;
            }
            if let Some(floor) = lid_battery_floor {
                settings.lid_battery_floor =
                    tako_control::sleep_guard::parse_battery_floor(*floor)?;
            }
            tako_control::settings::save(&settings)
                .map_err(|e| format!("設定の保存に失敗: {e}"))?;
            eprintln!(
                "  設定を変更しました: mode={}, power={}, lid-sleep={}, lid-power={}, lid-battery-floor={}%",
                settings.sleep_guard_mode.as_str(),
                settings.sleep_guard_power.as_str(),
                settings.lid_sleep_mode.as_str(),
                settings.lid_sleep_power.as_str(),
                settings.lid_battery_floor,
            );
            let state = tako_control::sleep_guard::status(settings.sleep_guard_config());
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
                            "updated" | "created" | "unchanged" => "[OK]",
                            "skipped" => "[情報]",
                            _ => "[失敗]",
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
                        eprintln!("[情報] エージェント共通ルール同期: 未設定");
                        eprintln!("  tako setup で正本ファイルを設定できます");
                    }
                    "source_missing" => {
                        let path = result["source_path"].as_str().unwrap_or("?");
                        eprintln!("[不足] 正本ファイルが見つかりません: {path}");
                    }
                    "up_to_date" => {
                        eprintln!("[OK] エージェント共通ルール同期: 最新");
                    }
                    "outdated" => {
                        eprintln!("[情報] エージェント共通ルール同期: ずれあり");
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
                            "up_to_date" => "[OK]",
                            "not_installed" | "outdated" | "not_synced" => "[情報]",
                            _ => "[失敗]",
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
    // #1944: 言語サーバの取得は tako が起動していなくても取れる（同じ実装を CLI のプロセスで走らせる）。
    // #2035: 初期化オプションの読み書きも同じ（settings.json を読み書きするだけ）
    if let Command::Lsp(LspCommand::Install { .. } | LspCommand::Options { .. }) = &command {
        let local = request.clone();
        let result = match send_request_classified(request, None) {
            Ok(value) => value,
            Err(SendFailure::Unreachable(_)) => lsp_server_local(&local)?,
            Err(SendFailure::Broken(message) | SendFailure::Operation(message)) => {
                return Err(message)
            }
            // 応答しない GUI が取得を続けているかもしれないので、こちらで二重には取らない（#2123）
            Err(SendFailure::Timeout(limit)) => {
                return Err(tako_control::protocol::reply_timeout_message(limit))
            }
        };
        print_result(&command, &result);
        return Ok(());
    }
    let result = send_request(request)?;
    print_result(&command, &result);
    Ok(())
}

/// `tako lsp install`（#1944）/ `tako lsp options`（#2035）を GUI 抜きで行う。GUI の manager と同じ
/// `LspManager::install` / `options` を通り、置き場（`<data_dir>/lsp-servers`・settings.json）も同じ。
/// 名前・キーの検査も dispatch と同じ 1 実装
fn lsp_server_local(request: &Request) -> Result<Value, String> {
    let Request::LspServer { action, name, set } = request else {
        return Err("言語サーバの要求ではない".into());
    };
    tako_control::dispatch::lsp_server_action(
        Some(&tako_control::lsp::LspManager::from_env()),
        action,
        name.as_deref(),
        set.as_ref(),
    )
    .map(|mut value| {
        value["local"] = Value::Bool(true);
        value
    })
    .map_err(local_dispatch_err)
}

/// `tako mod report` の送り主の祖先（#2559。自分自身から上へ近い順の pid）。GUI はこれで、ペインの外で
/// 起こした claude（自動命名の `claude -p` が GUI の env の `TAKO_PANE_ID` を受け継いだ等）の報告を
/// そのペインに記録しない。**unix だけ**: Windows は器（psmux）のサーバーと GUI の親子を実機で
/// 確かめていないので材料を送らない（= GUI は従来どおり受け付ける。自動命名の子の env は送る側で外す）
fn report_ancestry() -> Option<Vec<u32>> {
    if !cfg!(unix) {
        return None;
    }
    let me = std::process::id();
    let mut chain = vec![me];
    chain.extend(tako_core::platform::procinfo::ancestor_pids(
        me,
        tako_core::claude_mod::REPORT_ANCESTRY_DEPTH,
    ));
    Some(chain)
}

/// `tako mod report` の stdin（mod が組んだ報告の JSON。#1879）
fn read_mod_report() -> Result<Value, String> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin()
        .take(1 << 20)
        .read_to_string(&mut input)
        .map_err(|e| front_err(tako_control::Reason::ModReportUnreadable(e.to_string())))?;
    serde_json::from_str(&input)
        .map_err(|e| front_err(tako_control::Reason::ModReportNotJson(e.to_string())))
}

/// `tako mod ui …`（#1960。ローカル処理 = GUI 不要）。引数の読み方も操作も
/// `tako_control::claude_mod_ui` の 1 実装（MCP `tako_mod` の action=ui と同じ応答）。
/// 断ったら理由と使える値を出して終了コード 1（ui.json は書いていない）
fn mod_ui_local(args: &ModArgs) -> Result<(), String> {
    tako_core::i18n::set_lang(tako_control::settings::load().lang_setting().resolve());
    // 断りの文はどれも表示言語に従う（#2365。上で決めた言語）
    let request = tako_control::claude_mod_ui::cli_request(
        &args.rest,
        args.label.as_deref(),
        args.hotkey.as_deref(),
        args.id.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    let path = tako_control::claude_mod_ui::default_path().map_err(|e| e.to_string())?;
    let out = tako_control::claude_mod_ui::run(&path, &request).map_err(|e| e.message())?;
    if args.json {
        println!("{}", pretty_json(&out));
    } else {
        print_mod_ui(&out);
    }
    Ok(())
}

/// `tako mod ui` の表示（今の値・ボタン・選べる値。変更の後は結果と今の値だけ）。
/// 枠の文は `tako_core::claude_mod_ui::UiText`（表示言語に従う。#2365）
fn print_mod_ui(out: &Value) {
    use tako_core::claude_mod_ui::UiText;
    let path = tako_core::paths::shorten_home(out["path"].as_str().unwrap_or("-"));
    let state = match out["state"].as_str() {
        Some("absent") => UiText::StateAbsent.to_string(),
        Some("repaired") => UiText::StateRepaired {
            quarantine: tako_core::paths::shorten_home(out["quarantine"].as_str().unwrap_or("-")),
        }
        .to_string(),
        Some("future") => UiText::StateFuture.to_string(),
        Some("read_error") => UiText::StateReadError.to_string(),
        _ => String::new(),
    };
    match out["changed"].as_bool() {
        Some(true) => println!("{}", UiText::Changed { path }),
        Some(false) => println!("{}", UiText::Unchanged { path }),
        None => println!("{}{state}", UiText::Heading { path }),
    }
    let choices = &out["choices"];
    let word_list = |v: &Value| {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|w| w.as_str().map_or_else(|| w.to_string(), str::to_string))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // 並びは語彙の順（JSON のオブジェクトはキーの辞書順に並ぶので、そのまま回さない）
    use tako_core::claude_mod_ui::Vocab;
    for key in tako_core::claude_mod_ui::SetKey::words() {
        let value = &out["values"][key];
        if value.is_null() {
            continue;
        }
        let shown = match value {
            Value::Array(_) => word_list(value),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let allowed = &choices["set"][key];
        if allowed.is_object() {
            let allowed = UiText::Allowed {
                values: word_list(&allowed["values"]).replace(", ", " / "),
                list: allowed["type"] == "list",
            };
            println!("  {key:<18} {shown:<36} {allowed}");
        } else {
            println!("  {key:<18} {shown}");
        }
    }
    let buttons = out["ui"]["buttons"].as_array().cloned().unwrap_or_default();
    println!(
        "  {}",
        UiText::ButtonsHeading {
            count: buttons.len(),
            max: tako_core::claude_mod_ui::MAX_BUTTONS,
        }
    );
    for (i, b) in buttons.iter().enumerate() {
        let action: Result<tako_core::claude_mod_ui::ButtonAction, _> =
            serde_json::from_value(b["action"].clone());
        println!(
            "    {}. {}  [{}]  {}  (id {})",
            i + 1,
            b["label"].as_str().unwrap_or("-"),
            b["hotkey"].as_str().unwrap_or("-"),
            action.map(|a| a.summary()).unwrap_or_default(),
            b["id"].as_str().unwrap_or("-"),
        );
    }
    if choices.is_object() {
        let kinds = &choices["button"]["kinds"];
        let values = |kind: &str| {
            kinds[kind]["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v["value"].as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        };
        println!("  {}", UiText::ButtonActionsHeading);
        println!("    slash  {}", values("slash"));
        println!("    tako   {}", values("tako"));
        println!("    {}", UiText::ShellPromptLine);
        println!("  {}", UiText::ExamplesHeading);
        for example in choices["examples"].as_array().into_iter().flatten() {
            println!("    {}", example.as_str().unwrap_or_default());
        }
    } else if let Some(note) = out["applies_to"].as_str() {
        println!("{note}");
    }
}

/// `tako mod` の表示（#1879）。`report` は mod が読むので 1 行の JSON のまま
fn print_mod(args: &ModArgs, result: &Value) {
    if args.json || args.action.as_deref() == Some("report") {
        println!("{result}");
        return;
    }
    // 行の組み立ては tako-control の 1 本（`tako check-health` と同じ関数の行を含む。#1891）。
    // 枠の言語は表示言語（応答の理由の文も GUI が表示言語で組む。#2365）
    let lang = tako_control::settings::peek_lang_setting().resolve();
    for line in tako_control::claude_mod::text::status_lines(result, lang) {
        println!("{line}");
    }
}

/// master / solo の起動先ペイン（Issue #567）
struct LaunchTarget {
    pane: u64,
    /// 新規タブを作ったか（表示文言と復旧案内の出し分けに使う）
    new_tab: bool,
    /// 起動で付けたタブ名（付けなかった = 汎用 master なら `None`。#2120）
    tab_title: Option<String>,
}

/// 起動場所の表示文言（Issue #567。フォールバックで新タブになった場合もタブ表記になる）
fn launch_location(target: &LaunchTarget, lang: tako_core::i18n::Lang) -> String {
    let note = match (target.new_tab, target.tab_title.as_deref()) {
        (true, Some(title)) => ReplyNote::LaunchTabNamed {
            title: title.to_string(),
            pane: target.pane,
        },
        (true, None) => ReplyNote::LaunchNewTab(target.pane),
        (false, _) => ReplyNote::LaunchInline(target.pane),
    };
    note.text_in(lang)
}

/// 案内文に出す最簡形のコマンド（#322。既定プロファイルなら引数を付けない）
fn master_cmd_hint(profile_name: &str) -> String {
    profile_cmd_hint("tako master", profile_name)
}

fn solo_cmd_hint(profile_name: &str) -> String {
    profile_cmd_hint("tako solo", profile_name)
}

fn profile_cmd_hint(base: &str, profile_name: &str) -> String {
    // 組み立ては tako-core の 1 実装（#1453 で `adopt` の応答も同じ文字列を返す）
    tako_core::handoff::profile_launch_command(base, profile_name)
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
/// `cmd_hint` は案内文に出す最簡形のコマンド（例: `tako master -fable`。#322）。
/// 案内の行・断りは `lang`（呼び手が決めた表示言語。#2365）で組む
///
/// タブ名は `naming` のとおりに付ける（`None` = 触らない）。master は管轄プロジェクトの
/// 自動名（#2120）、solo は作業フォルダのリポジトリの自動名（#2127）。
/// 新しいタブを作るときは `new_tab_cwd` で開く（`None` = アプリの既定。solo はタブ名を
/// 付けたフォルダで開かないと、名前と作業場所が食い違う）
fn resolve_launch_target(
    naming: Option<&tako_control::orchestrator::master_launch::TabNaming>,
    new_tab_cwd: Option<&std::path::Path>,
    use_tab: bool,
    cmd_hint: &str,
    lang: tako_core::i18n::Lang,
) -> Result<LaunchTarget, String> {
    let requested = caller_pane();
    if use_tab {
        return new_tab_target(naming, new_tab_cwd, cmd_hint, requested, lang);
    }
    let resolved = resolve_caller_pane_via_app(requested)
        .map_err(|e| launch_failure_message(&e, cmd_hint, requested))?;
    match resolved {
        Some(caller) => {
            let pane = caller.pane;
            if let Some(old) = requested.filter(|old| *old != pane) {
                let info = if caller.method.as_deref() == Some("stale") {
                    ReplyNote::StalePaneIdRemapped { old, pane }
                } else {
                    ReplyNote::PaneIdMismatch { old, pane }
                };
                eprintln!("{}", info.text_in(lang));
                eprintln!("{}", ReplyNote::UnsetPaneIdHint.text_in(lang));
            }
            let tab_title = naming.and_then(|n| rename_launch_tab(n, None, Some(pane)));
            Ok(LaunchTarget {
                pane,
                new_tab: false,
                tab_title,
            })
        }
        None => {
            let info = match requested {
                Some(old) => ReplyNote::CallerPaneUnknownStale(old),
                None => ReplyNote::CallerPaneUnknownUnset,
            };
            eprintln!("{}", info.text_in(lang));
            eprintln!("{}", ReplyNote::LaunchingInNewTab.text_in(lang));
            if requested.is_some() {
                eprintln!("{}", ReplyNote::UnsetPaneIdHint.text_in(lang));
            }
            new_tab_target(naming, new_tab_cwd, cmd_hint, requested, lang)
        }
    }
}

/// 新規タブを作って起動先にする（Issue #567 のフォールバック / `--tab` 指定時）
fn new_tab_target(
    naming: Option<&tako_control::orchestrator::master_launch::TabNaming>,
    cwd: Option<&std::path::Path>,
    cmd_hint: &str,
    requested: Option<u64>,
    lang: tako_core::i18n::Lang,
) -> Result<LaunchTarget, String> {
    // 明示タイトル付きの作成は手動名になるので、渡すのは手動名（#2120 / #2127 前の A/B）の
    // ときだけ。自動名は作ってから `source: auto` で付ける
    let manual_title = naming.filter(|n| n.is_manual()).map(|n| n.title.clone());
    let tab_result = send_request(Request::TabNew {
        title: manual_title.clone(),
        focus: Some(true),
        cwd: cwd.map(|p| p.display().to_string()),
    })
    .map_err(|e| launch_failure_message(&e, cmd_hint, requested))?;
    let pane = tab_result["pane"]
        .as_u64()
        .ok_or_else(|| Reason::TabNewNoPane.text_in(lang))?;
    let tab_title = match naming {
        Some(n) if !n.is_manual() => tab_result["tab"]
            .as_u64()
            .and_then(|tab| rename_launch_tab(n, Some(tab), None)),
        _ => manual_title,
    };
    Ok(LaunchTarget {
        pane,
        new_tab: true,
        tab_title,
    })
}

/// 起動先のタブへ名前を付け、付いた名前を返す（#2120）。
///
/// 自動名はユーザーが手で付けた名前を上書きしないので、返るのは**実際のタブ名**
/// （手動名が残ったならその名前）。名前付けの失敗は起動を止めない
fn rename_launch_tab(
    naming: &tako_control::orchestrator::master_launch::TabNaming,
    tab: Option<u64>,
    pane: Option<u64>,
) -> Option<String> {
    send_request(Request::TabRename {
        tab,
        pane,
        title: naming.title.clone(),
        source: naming.source.clone(),
    })
    .ok()
    .and_then(|v| v["title"].as_str().map(str::to_string))
}

/// 起動先を確保できないときのエラー文（Issue #567）。
/// フォールバック（新規タブ）まで届かない = アプリへ接続できていない状態なので、
/// 復旧手順を必ず添える。コマンドは最簡形で示す（#322）。
/// 原因（`cause`）は接続の断り（`ipc.rs`）なので、包みも同じ解決の表示言語で組む（#2365）
fn launch_failure_message(cause: &str, cmd_hint: &str, requested: Option<u64>) -> String {
    tako_control::Reason::LaunchPaneUnavailable {
        cause: cause.to_string(),
        cmd_hint: cmd_hint.to_string(),
        stale_pane: requested,
    }
    .text_in(tako_control::settings::client_lang())
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

/// `--pane` 指定が無ければ呼び出し元へフォールバックする（FR-2.2.7）。
///
/// 断りは GUI へ送る前に手元で出すので、表示言語は `local_dispatch_err` と同じく設定から
/// 解決する（#2358。表示言語を初期化していない枝で英語に倒れないように）
fn target_pane(explicit: Option<u64>) -> Result<Option<u64>, String> {
    explicit.or_else(caller_pane).map(Some).ok_or_else(|| {
        tako_control::NoTargetPaneAt::Cli
            .text_in(tako_control::settings::peek_lang_setting().resolve())
            .to_string()
    })
}

/// 検索条件のスイッチを wire の値へ写す（#1653）。
///
/// 付けたときだけ `Some(value)` を送り、付けなければ `None`（= dispatch が既定か
/// 今の検索の条件で埋める）。`-i` は「区別する」を `false` にするスイッチなので
/// `value = false`、`-w` は `true`
fn search_flag(set: bool, value: bool) -> Option<bool> {
    set.then_some(value)
}

/// `tako todo` の各サブコマンドを [`Request::UserTask`] へ畳む（#1450）。
///
/// **CLI 側で store を直接触らない**（`tako task gate` のようなローカル処理にしない）のは、
/// 起票の通知を画面へ出すこと・返答を master のペインへ配ることが GUI の中でしか
/// できないため。操作の実体は dispatch の 1 実装を通る（設計原則 5）
fn todo_request(sub: &TodoCommand) -> Request {
    let caller_role = std::env::var("TAKO_ORCHESTRATOR_ROLE").ok();
    let base = |action: &str| Request::UserTask {
        action: action.to_string(),
        id: None,
        title: None,
        body: None,
        kind: None,
        status: None,
        all: None,
        project: None,
        attachments: None,
        copy_texts: None,
        links: None,
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: caller_pane(),
        caller_role: caller_role.clone(),
    };
    /// 空の `Vec` は「指定なし」（update で既存を消さない）
    fn some_if_any(list: &[String]) -> Option<Vec<String>> {
        (!list.is_empty()).then(|| list.to_vec())
    }
    match sub {
        TodoCommand::Add {
            title,
            body,
            kind,
            project,
            attachments,
            copy_texts,
            links,
            due,
            ..
        } => {
            let mut req = base("add");
            if let Request::UserTask {
                title: t,
                body: b,
                kind: k,
                project: p,
                attachments: a,
                copy_texts: c,
                links: l,
                due: d,
                ..
            } = &mut req
            {
                *t = Some(title.clone());
                *b = body.clone();
                *k = kind.clone();
                *p = project.clone();
                // 添付は相対パスで打たれることが多い。**呼び出し元の cwd で絶対化**する
                // （dispatch は GUI のプロセスで走るので、相対のままでは別の場所を指す）
                *a = some_if_any(attachments)
                    .map(|list| list.iter().map(|p| resolve_cli_path(p)).collect());
                *c = some_if_any(copy_texts);
                *l = some_if_any(links);
                *d = due.clone();
            }
            req
        }
        TodoCommand::List {
            status,
            kind,
            project,
            all,
            ..
        } => {
            let mut req = base("list");
            if let Request::UserTask {
                status: s,
                kind: k,
                project: p,
                all: a,
                ..
            } = &mut req
            {
                *s = status.clone();
                *k = kind.clone();
                *p = project.clone();
                *a = all.then_some(true);
            }
            req
        }
        TodoCommand::Show { id, .. } => with_id(base("show"), id),
        TodoCommand::Update {
            id,
            title,
            body,
            kind,
            project,
            attachments,
            copy_texts,
            links,
            due,
            ..
        } => {
            let mut req = with_id(base("update"), id);
            if let Request::UserTask {
                title: t,
                body: b,
                kind: k,
                project: p,
                attachments: a,
                copy_texts: c,
                links: l,
                due: d,
                ..
            } = &mut req
            {
                *t = title.clone();
                *b = body.clone();
                *k = kind.clone();
                *p = project.clone();
                *a = some_if_any(attachments)
                    .map(|list| list.iter().map(|p| resolve_cli_path(p)).collect());
                *c = some_if_any(copy_texts);
                *l = some_if_any(links);
                *d = due.clone();
            }
            req
        }
        TodoCommand::Done { id, .. } => with_id(base("done"), id),
        TodoCommand::Dismiss { id, .. } => with_id(base("dismiss"), id),
        TodoCommand::Expand { id, .. } => with_id(base("expand"), id),
        TodoCommand::Collapse { .. } => base("collapse"),
        TodoCommand::Respond {
            id,
            decision,
            comment,
            via,
            ..
        } => {
            let mut req = with_id(base("respond"), id);
            if let Request::UserTask {
                decision: d,
                comment: c,
                via: v,
                ..
            } = &mut req
            {
                *d = Some(decision.clone());
                *c = comment.clone();
                *v = Some(via.clone().unwrap_or_else(|| "cli".to_string()));
            }
            req
        }
    }
}

/// `Request::UserTask` へ対象 id を入れる
fn with_id(mut req: Request, value: &str) -> Request {
    if let Request::UserTask { id, .. } = &mut req {
        *id = Some(value.to_string());
    }
    req
}

/// `行:桁` を解く（#1658）。桁を省いたら 0（行頭）。
///
/// **この綴りを解くのは CLI の入口だけ**。protocol / MCP は数値をそのまま運ぶので、
/// 「`3:0` をどう読むか」が経路ごとに割れない
fn parse_position(text: &str) -> Result<(usize, usize), String> {
    let (line, col) = match text.split_once(':') {
        Some((line, col)) => (line, Some(col)),
        None => (text, None),
    };
    let line: usize = line
        .trim()
        .parse()
        .map_err(|_| front_err(tako_control::Reason::PositionLine(text.to_string())))?;
    let col: usize = match col {
        Some(col) => col
            .trim()
            .parse()
            .map_err(|_| front_err(tako_control::Reason::PositionColumn(text.to_string())))?,
        None => 0,
    };
    Ok((line, col))
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
            paste_guard: None,
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
                return Err(front_err(tako_control::Reason::FocusTarget));
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
                return Err(front_err(tako_control::Reason::ScrollTarget));
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
            // 「どの role が閉じたか」をペインログへ残す監査情報。#2330 からは master のペインを
            // 閉じる close の守り（最後の master・受領済みの後任）の判断にも使う
            caller_role: std::env::var("TAKO_ORCHESTRATOR_ROLE")
                .ok()
                .filter(|r| !r.trim().is_empty()),
            // #2330: master が自分を閉じるのか別の master を閉じるのかを見分ける
            caller_pane: caller_pane(),
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
                _ => return Err(front_err(tako_control::Reason::ResizeOneOf)),
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
            // #1676: 負値・0 はここで弾く（文言の正本は open_plan。dispatch の
            // `usize` は負値を持てないので、CLI 側だけがこの境界を見られる）。
            // 表示言語は前段の断り（`front_err`）と同じ解決（#2365）
            use tako_core::open_plan::PlanRefusal;
            let one_based =
                |v: Option<i64>, refusal: PlanRefusal| -> Result<Option<usize>, String> {
                    match v {
                        Some(v) if v <= 0 => Err(format!(
                            "{}: {v}",
                            refusal.text_in(tako_control::settings::peek_lang_setting().resolve())
                        )),
                        Some(v) => Ok(Some(v as usize)),
                        None => Ok(None),
                    }
                };
            let line = one_based(args.line, PlanRefusal::LineNotOneBased)?;
            let column = one_based(args.column, PlanRefusal::ColumnNotOneBased)?;
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
                line,
                column,
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
        Command::Scrollback(args) => Request::Scrollback {
            lines: args.lines,
            unfocused_fps: None,
            mouse_forward: None,
        },
        // #1979: 再描画の上限は MCP `tako_scrollback` の `unfocused_fps` と同じ操作
        Command::RedrawLimit(args) => Request::Scrollback {
            lines: None,
            unfocused_fps: args.fps,
            mouse_forward: None,
        },
        // #1961: クリックの転送は MCP `tako_scrollback` の `mouse_forward` と同じ操作
        Command::Mouse(args) => Request::Scrollback {
            lines: None,
            unfocused_fps: None,
            mouse_forward: args.state.as_deref().map(|s| s == "on"),
        },
        // #1961: MCP は `tako_send_input` の `click`
        Command::Click(args) => Request::PaneClick {
            pane: Some(args.pane),
            row: args.row,
            col: args.col,
        },
        // #2046 / #2050: MCP は `tako_settings` の action=paste_guard（enabled / text / respond /
        // mode / allow / unallow / rules / pane）
        Command::PasteGuard(args) => {
            let action = args.action.as_deref();
            if args.text.is_some()
                && !matches!(action, Some("check" | "allow" | "unallow" | "rule"))
            {
                return Err(front_err(tako_control::Reason::PasteGuardTextUsage));
            }
            if args.value.is_some() != (action == Some("rule")) {
                return Err(front_err(tako_control::Reason::PasteGuardRuleUsage));
            }
            if args.pane.is_some() && action != Some("check") {
                return Err(front_err(tako_control::Reason::PasteGuardPaneOnlyCheck));
            }
            let rules = match (action, args.text.as_deref(), args.value.as_deref()) {
                (Some("rule"), Some(name), Some(on)) => Some(std::collections::BTreeMap::from([(
                    name.to_string(),
                    on == "on",
                )])),
                (Some("rule"), ..) => {
                    return Err(front_err(tako_control::Reason::PasteGuardRuleUsage));
                }
                _ => None,
            };
            Request::PasteGuard {
                enabled: match action {
                    Some("on" | "warn") => Some(true),
                    Some("off") => Some(false),
                    _ => None,
                },
                check: match action {
                    Some("check") => Some(paste_guard_check_text(args.text.as_deref())?),
                    _ => None,
                },
                respond: action
                    .filter(|a| matches!(*a, "send" | "cancel"))
                    .map(str::to_string),
                // on = 確かめる段・warn = 警告だけの段（off は段を変えない）
                mode: match action {
                    Some("on") => Some("confirm".into()),
                    Some("warn") => Some("warn".into()),
                    _ => None,
                },
                allow: match action {
                    Some("allow") => Some(paste_guard_check_text(args.text.as_deref())?),
                    _ => None,
                },
                unallow: match action {
                    Some("unallow") => Some(
                        args.text
                            .clone()
                            .ok_or_else(|| front_err(tako_control::Reason::PasteGuardUnallowId))?,
                    ),
                    _ => None,
                },
                rules,
                pane: args.pane,
            }
        }
        // #1677: 基準ペインは閉じたペインの項目を開き直すときだけ使う。tako の外から
        // 叩いて呼び出し元が分からなくても失敗させない（dispatch がアクティブタブへ倒す）
        Command::Jump(sub) => {
            let (pane, focus) = match sub {
                JumpCommand::Back(args) | JumpCommand::Forward(args) => {
                    (args.pane.or_else(caller_pane), args.focus.then_some(true))
                }
                JumpCommand::List { .. } => (None, None),
            };
            Request::Jump {
                action: sub.action().to_string(),
                pane,
                focus,
            }
        }
        Command::Lsp(sub) => sub.request()?,
        Command::Search(args) => {
            let action = args.action();
            if action == "search" && args.query.is_none() {
                return Err(front_err(tako_control::Reason::SearchQuery));
            }
            Request::WorkspaceSearch {
                action: action.into(),
                query: args.query.clone(),
                // 相対パスは自分の cwd 基準（GUI のペインの cwd ではない）
                path: args.path.as_deref().map(absolutize),
                pane: args.pane.or_else(caller_pane),
                regex: args.regex.then_some(true),
                case_sensitive: args.case_sensitive.then_some(true),
                show: args.show.then_some(true),
                limit: args.limit,
                item: args.open,
                focus: args.focus.then_some(true),
            }
        }
        Command::Links(args) => {
            // `--text -` は標準入力から読む（画面の写しをパイプで流せる形）
            let text = match args.text.as_deref() {
                Some("-") => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).map_err(
                        |e| front_err(tako_control::Reason::StdinReadFailed(e.to_string())),
                    )?;
                    Some(buf)
                }
                other => other.map(str::to_string),
            };
            Request::Links {
                // `--text` を渡したときはペインを見ないので呼び出し元の解決も要らない
                pane: if text.is_some() {
                    args.pane
                } else {
                    target_pane(args.pane)?
                },
                text,
                cols: args.cols,
                cwd: args.cwd.clone(),
            }
        }
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
            EditCommand::ReplaceRange {
                start,
                end,
                text,
                expect_version,
                pane,
            } => {
                let (start_line, start_col) = parse_position(start)?;
                let (end_line, end_col) = parse_position(end)?;
                Request::PreviewEditRange {
                    pane: target_pane(*pane)?,
                    start_line,
                    start_col,
                    end_line,
                    end_col,
                    text: text.clone(),
                    expected_version: *expect_version,
                }
            }
            EditCommand::Cursor {
                position,
                select_to,
                expect_version,
                pane,
            } => {
                let (line, col) = parse_position(position)?;
                let select_to = select_to.as_deref().map(parse_position).transpose()?;
                Request::PreviewCursor {
                    pane: target_pane(*pane)?,
                    line,
                    col,
                    select_to_line: select_to.map(|(line, _)| line),
                    select_to_col: select_to.map(|(_, col)| col),
                    expected_version: *expect_version,
                }
            }
            EditCommand::Move {
                movement,
                select,
                expect_version,
                pane,
            } => {
                // 綴りは実行前に弾く（サーバーまで持って行かない）。表は tako-core の 1 か所
                if tako_core::text_edit::CursorMovement::from_name(movement).is_none() {
                    return Err(front_err(tako_control::Reason::BadMovement(
                        movement.clone(),
                    )));
                }
                Request::PreviewMove {
                    pane: target_pane(*pane)?,
                    movement: movement.clone(),
                    select: *select,
                    expected_version: *expect_version,
                }
            }
            EditCommand::Delete {
                motion,
                expect_version,
                pane,
            } => {
                if tako_core::text_edit::DeleteMotion::from_name(motion).is_none() {
                    return Err(front_err(tako_control::Reason::BadMotion(motion.clone())));
                }
                Request::PreviewDelete {
                    pane: target_pane(*pane)?,
                    motion: motion.clone(),
                    expected_version: *expect_version,
                }
            }
            EditCommand::Indent {
                expect_version,
                pane,
            } => Request::PreviewEditCommand {
                pane: target_pane(*pane)?,
                command: "indent".into(),
                text: None,
                expected_version: *expect_version,
            },
            EditCommand::Outdent {
                expect_version,
                pane,
            } => Request::PreviewEditCommand {
                pane: target_pane(*pane)?,
                command: "outdent".into(),
                text: None,
                expected_version: *expect_version,
            },
            EditCommand::Newline {
                expect_version,
                pane,
            } => Request::PreviewEditCommand {
                pane: target_pane(*pane)?,
                command: "newline".into(),
                text: None,
                expected_version: *expect_version,
            },
            // #1759: 打鍵。綴りは tako-core の名前表（MCP の command と同じ）
            EditCommand::Type {
                text,
                expect_version,
                pane,
            } => Request::PreviewEditCommand {
                pane: target_pane(*pane)?,
                command: tako_core::platform::editor_keys::TYPE_COMMAND.into(),
                text: Some(text.clone()),
                expected_version: *expect_version,
            },
            EditCommand::AutoClose { state } => Request::PreviewAutoClose {
                enabled: state.as_deref().map(|s| s == "on"),
            },
            EditCommand::Line {
                operation,
                expect_version,
                pane,
            } => {
                // 綴りは実行前に弾く。表は tako-core の 1 か所で、MCP の command
                // （`line-<操作>`）へは editor_keys が頭を付ける（綴りを CLI で組まない）
                use tako_core::text_edit::LineOperation;
                let Some(op) = LineOperation::from_name(operation) else {
                    return Err(front_err(tako_control::Reason::BadLineOp(
                        operation.clone(),
                    )));
                };
                Request::PreviewEditCommand {
                    pane: target_pane(*pane)?,
                    command: tako_core::platform::editor_keys::EditorCommand::line_edit_name(op),
                    text: None,
                    expected_version: *expect_version,
                }
            }
            EditCommand::Save { force, pane } => Request::PreviewSave {
                pane: target_pane(*pane)?,
                force: *force,
            },
            EditCommand::Reload { pane } => Request::PreviewRevert {
                pane: target_pane(*pane)?,
            },
            EditCommand::Diff { pane } => Request::PreviewDiff {
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
                ignore_case,
                whole_word,
                open,
                close,
                pane,
            } => {
                // #1873: 検索欄の開閉（付けたときだけ送る = 付けなければ従来の JSON とバイト一致）
                let visible = match (*open, *close) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                };
                Request::PreviewSearch {
                    pane: target_pane(*pane)?,
                    query: query.clone(),
                    // 開閉だけのときは移動しない（GUI の ⌘F・Escape はヒットへ飛ばない）
                    direction: direction
                        .clone()
                        .or_else(|| visible.is_none().then(|| "next".to_string())),
                    case_sensitive: search_flag(*ignore_case, false),
                    whole_word: search_flag(*whole_word, true),
                    visible,
                }
            }
            EditCommand::Replace {
                query,
                replacement,
                all,
                ignore_case,
                whole_word,
                pane,
            } => Request::PreviewReplace {
                pane: target_pane(*pane)?,
                query: query.clone(),
                replacement: replacement.clone(),
                all: Some(*all),
                case_sensitive: search_flag(*ignore_case, false),
                whole_word: search_flag(*whole_word, true),
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
        Command::Window(WindowCommand::Move { x, y, window }) => Request::WindowMove {
            window: *window,
            x: *x,
            y: *y,
        },
        Command::Window(WindowCommand::Resize {
            width,
            height,
            window,
        }) => Request::WindowResize {
            window: *window,
            width: *width,
            height: *height,
        },
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
                return Err(front_err(tako_control::Reason::DirectionNeedsTarget));
            }
            if !new && tab.is_none() && target.is_none() {
                return Err(front_err(tako_control::Reason::MoveDestination));
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
        // #1879: mod からの報告。stdin の JSON をそのまま渡し、送り主は TAKO_PANE_ID。
        // #2559: 自分の祖先も添える（GUI がペインの外で起こした claude の報告を見分ける）
        Command::Mod(args) if args.action.as_deref() == Some("report") => Request::Mod {
            action: Some("report".into()),
            report: Some(read_mod_report()?),
            pane: caller_pane(),
            ancestry: report_ancestry(),
            ui: None,
            dry_run: false,
        },
        Command::Mod(args) if args.dry_run => {
            return Err(front_err(tako_control::Reason::ModDryRunOnly))
        }
        // #1881: `tako mod band on|off` は dispatch の action `band-on` / `band-off`（MCP と同じ語）
        Command::Mod(args) if args.action.as_deref() == Some("band") => Request::Mod {
            action: Some(
                match args.rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    ["on"] => "band-on".into(),
                    ["off"] => "band-off".into(),
                    _ => return Err(front_err(tako_control::Reason::ModBandOnOff)),
                },
            ),
            report: None,
            pane: None,
            ancestry: None,
            ui: None,
            dry_run: false,
        },
        Command::Mod(args)
            if !args.rest.is_empty()
                || args.label.is_some()
                || args.hotkey.is_some()
                || args.id.is_some() =>
        {
            return Err(front_err(tako_control::Reason::ModOnOffAfterBand))
        }
        Command::Mod(args) if matches!(args.action.as_deref(), Some("install" | "uninstall")) => {
            unreachable!("mod install / uninstall は run() を通らない（ローカル処理）")
        }
        Command::Mod(args) => Request::Mod {
            action: args.action.clone(),
            report: None,
            pane: None,
            ancestry: None,
            ui: None,
            dry_run: false,
        },
        // #1857: 再 attach は切替ではなくペイン 1 枚の操作（MCP の `reattach` と同じ要求）
        Command::Persist(args) if args.state.as_deref() == Some("reattach") => {
            Request::BackendReattach {
                pane: args
                    .pane
                    .or_else(caller_pane)
                    .ok_or_else(|| front_err(tako_control::Reason::ReattachNeedsPane))?,
            }
        }
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
        Command::SessionRestart(args) => Request::SessionRestart {
            pane: args.pane.or_else(caller_pane),
            mode: args.mode.clone(),
        },
        // #2112: `--network on|off` は全体の設定（ペインを解かない）。`--network` 単独は
        // 一覧を引いて `network` の節だけを出す（読むための専用の要求を作らない）
        Command::LimitResume(args) if args.network => {
            let enabled = args.state.as_deref().map(|s| s == "on");
            Request::LimitResume {
                pane: None,
                enabled: None,
                all: enabled.is_none().then_some(true),
                network: enabled,
            }
        }
        Command::LimitResume(args) => Request::LimitResume {
            pane: args.pane.or_else(caller_pane),
            enabled: args.state.as_deref().map(|s| s == "on"),
            all: args.all.then_some(true),
            network: None,
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
            // #1958: --pane 省略時は呼び出し元（MCP の tako_show_command と同じ規則）。
            // --card 指定時は dispatch がカードの所在ペインを引くので埋めない
            pane: match args.action() {
                "show" => target_pane(args.pane)?,
                _ if args.card.is_none() => args.pane.or_else(caller_pane),
                _ => args.pane,
            },
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
        Command::Foreground(args) => {
            if args.pane.is_none() && args.tab.is_none() {
                return Err(front_err(tako_control::Reason::ForegroundTarget));
            }
            Request::Foreground {
                pane: args.pane,
                tab: args.tab,
                target: args.target,
                direction: args.direction.as_deref().map(parse_direction).transpose()?,
            }
        }
        Command::BackgroundList => Request::BackgroundList,
        Command::Tmux(TmuxCommand::List { socket }) => Request::TmuxList {
            socket: socket.clone(),
        },
        Command::Tmux(TmuxCommand::Cleanup {
            socket,
            servers,
            apply,
        }) => Request::TmuxCleanup {
            socket: socket.clone(),
            servers: *servers,
            apply: *apply,
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
            window,
            pane,
            right: _,
            down,
            up,
            left,
        }) => Request::TmuxOpen {
            socket: socket.clone(),
            session: session.clone(),
            window: *window,
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
                    dest: None,
                }
            } else {
                Request::FileOp {
                    op: tako_control::protocol::FileOpKind::CopyAbsolutePath,
                    path: abs,
                    name: None,
                    pane: None,
                    dest: None,
                }
            }
        }
        Command::File(FileCommand::Reveal { path }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::Reveal,
            path: resolve_cli_path(path),
            name: None,
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::OpenTerminal { path, pane }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenTerminal,
            path: resolve_cli_path(path),
            name: None,
            pane: target_pane(*pane)?,
            dest: None,
        },
        Command::File(FileCommand::Rename { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::Rename,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
            dest: None,
        },
        // #1834: 移す元・移動先ともに CLI の cwd 基準で絶対パスへ（GUI の cwd で読ませない）。
        // #1867: 移す元が複数ならまとめた要求（ツリーの複数選択の D&D と同じ 1 要求）
        Command::File(FileCommand::Move { paths }) => {
            let Some((dest, sources)) = paths.split_last().filter(|(_, s)| !s.is_empty()) else {
                return Err(front_err(tako_control::Reason::FileMoveArgs));
            };
            file_op_for(
                tako_control::protocol::FileOpKind::Move,
                sources,
                Some(resolve_cli_path(dest)),
            )
        }
        // #1860 / #1895: 複数でもまとめた 1 要求（A/B の `TAKO_1895_LEGACY=1` だけ main の
        // `file_copy_cli` が 1 件ずつ送る）
        Command::File(FileCommand::Copy { paths }) => {
            let mut requests = file_copy_requests(paths)?;
            if requests.len() != 1 {
                return Err(front_err(tako_control::Reason::CopyOneAtATime));
            }
            requests.remove(0)
        }
        Command::File(FileCommand::Clipboard { action }) => {
            let (op, path) = match action {
                // #1867: 複数ならまとめた要求（ツリーの複数選択の ⌘C / ⌘X と同じ 1 要求）
                Some(FileClipboardCommand::Copy { paths }) => {
                    return Ok(file_op_for(
                        tako_control::protocol::FileOpKind::ClipboardCopy,
                        paths,
                        None,
                    ))
                }
                Some(FileClipboardCommand::Cut { paths }) => {
                    return Ok(file_op_for(
                        tako_control::protocol::FileOpKind::ClipboardCut,
                        paths,
                        None,
                    ))
                }
                Some(FileClipboardCommand::Show { dir }) => (
                    tako_control::protocol::FileOpKind::Clipboard,
                    dir.as_deref().unwrap_or("."),
                ),
                None => (tako_control::protocol::FileOpKind::Clipboard, "."),
            };
            Request::FileOp {
                op,
                path: resolve_cli_path(path),
                name: None,
                pane: None,
                dest: None,
            }
        }
        // 貼り付け先は CLI の cwd 基準（GUI の cwd で読ませない = move と同じ）
        Command::File(FileCommand::Paste { dest, as_move }) => Request::FileOp {
            // #1867: `--move` = ツリーの ⌥⌘V（コピーしたものも移す）
            op: if *as_move {
                tako_control::protocol::FileOpKind::PasteMove
            } else {
                tako_control::protocol::FileOpKind::Paste
            },
            path: resolve_cli_path(dest.as_deref().unwrap_or(".")),
            name: None,
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::Progress) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::CopyProgress,
            path: String::new(),
            name: None,
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::Cancel { id }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::CopyCancel,
            path: String::new(),
            name: id.map(|id| id.to_string()),
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::Create { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::CreateFile,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::Mkdir { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::CreateDir,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::Trash { paths }) => {
            file_op_for(tako_control::protocol::FileOpKind::Trash, paths, None)
        }
        Command::File(FileCommand::Open { path }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenDefault,
            path: resolve_cli_path(path),
            name: None,
            pane: None,
            dest: None,
        },
        Command::File(FileCommand::OpenWith { path, name }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenWith,
            path: resolve_cli_path(path),
            name: Some(name.clone()),
            pane: None,
            dest: None,
        },
        // #1182: 開き先（プレビュー / シェル）は分割元のペインを基準に決まるので pane を渡す
        Command::File(FileCommand::OpenInTako { path, pane }) => Request::FileOp {
            op: tako_control::protocol::FileOpKind::OpenInTako,
            path: resolve_cli_path(path),
            name: None,
            pane: target_pane(*pane)?,
            dest: None,
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
                return Err(front_err(tako_control::Reason::EitherOf("--pane", "--tab")));
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
        Command::Orchestrator(OrchestratorCommand::Resume { .. }) => {
            unreachable!("orchestrator resume は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Projects(_)) => {
            unreachable!("orchestrator projects は run() を通らない")
        }
        Command::Orchestrator(OrchestratorCommand::Adopt { .. }) => {
            unreachable!("orchestrator adopt は run() を通らない（run() で send_request 済み）")
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
                lid_power_condition: None,
                lid_battery_floor: None,
            },
            SleepGuardCommand::Set {
                mode,
                power_condition,
                lid_sleep_mode,
                lid_power_condition,
                lid_battery_floor,
            } => Request::SleepGuard {
                action: Some("set".to_string()),
                mode: mode.clone(),
                power_condition: power_condition.clone(),
                lid_sleep_mode: lid_sleep_mode.clone(),
                lid_power_condition: lid_power_condition.clone(),
                lid_battery_floor: *lid_battery_floor,
            },
            SleepGuardCommand::InstallLidSleep => Request::SleepGuard {
                action: Some("install-lid-sleep".to_string()),
                mode: None,
                power_condition: None,
                lid_sleep_mode: None,
                lid_power_condition: None,
                lid_battery_floor: None,
            },
            SleepGuardCommand::RemoveLidSleep => Request::SleepGuard {
                action: Some("remove-lid-sleep".to_string()),
                mode: None,
                power_condition: None,
                lid_sleep_mode: None,
                lid_power_condition: None,
                lid_battery_floor: None,
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
                limit: None,
            },
            TreeCommand::Remove { path, tab } => Request::TreeFolder {
                action: "remove".to_string(),
                path: Some(resolve_cli_path(path)),
                tab: *tab,
                pane: caller_pane(),
                limit: None,
            },
            TreeCommand::List { tab } => Request::TreeFolder {
                action: "list".to_string(),
                path: None,
                tab: *tab,
                pane: caller_pane(),
                limit: None,
            },
            TreeCommand::GitStatus { path, tab, limit } => Request::TreeFolder {
                action: "git-status".to_string(),
                path: path.as_deref().map(resolve_cli_path),
                tab: *tab,
                pane: caller_pane(),
                limit: *limit,
            },
            // #1908: MCP `tako_tree_folder` の `action=selection` と同じ要求
            TreeCommand::Selection { path, key, tab } => Request::TreeSelection {
                path: path.as_deref().map(resolve_cli_path),
                key: key.clone(),
                tab: *tab,
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
                by: None,
                window: None,
                account: None,
                issue: None,
                show: None,
            },
            SessionsCommand::Link { pane, id, .. } => Request::Sessions {
                action: "link".into(),
                id: id.clone(),
                role: None,
                project: None,
                limit: None,
                // #1958: 明示指定 → 呼び出し元ペイン（TAKO_PANE_ID）→ None（resume と同じ。
                // None は dispatch がアクティブタブのフォーカスペインへ倒す = tako 外の CLI 用）
                pane: if id.is_some() {
                    *pane
                } else {
                    pane.or_else(caller_pane)
                },
                tab: None,
                direction: None,
                by: None,
                window: None,
                account: None,
                issue: None,
                show: None,
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
                by: None,
                window: None,
                account: None,
                issue: None,
                show: None,
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
                by: None,
                window: None,
                account: None,
                issue: None,
                show: None,
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
        Command::CheckHealth(_) => {
            unreachable!("check-health は run() を通らない（ローカル処理）")
        }
        Command::Queue(_) => {
            unreachable!("queue は run() を通らない（届かないときのローカル処理を持つ）")
        }
        Command::ContextBudget(_) => {
            unreachable!("context-budget は run() を通らない（ローカル処理）")
        }
        Command::TestResidue(_) => {
            unreachable!("test-residue は run() を通らない（ローカル処理）")
        }
        Command::Orchestrator(OrchestratorCommand::Guide { .. }) => {
            unreachable!("orchestrator guide は run() を通らない（ローカル処理）")
        }
        Command::AgentSupport(_) => {
            unreachable!("agent-support は run() を通らない（ローカル処理）")
        }
        Command::Usage(_) => unreachable!("usage は run() を通らない（ローカル処理）"),
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
                target,
                pane,
                tab,
                direction,
            } => Request::OpenRemote {
                host: host.clone(),
                focus: Some(!no_focus),
                remote_dir: remote_dir.clone(),
                // #1006: 語彙の正本は `tako_core::remote_open`（MCP / GUI と同じ表）。
                // clap の value_parser も同じ 3 値なので、ここへ来る文字列は必ず解釈できる
                target: target.as_deref().map(|t| {
                    tako_core::remote_open::RemoteOpenTarget::parse(t)
                        .expect("value_parser が VALUES と同じ 3 値に絞っている")
                }),
                pane: *pane,
                tab: *tab,
                direction: direction
                    .as_deref()
                    .map(|d| parse_direction(d).expect("value_parser が 4 方向に絞っている")),
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
            RemoteFolderCommand::Open {
                host,
                path,
                tab,
                no_terminal,
            } => Request::RemoteFolder {
                action: "open".into(),
                host: Some(host.clone()),
                path: path.clone(),
                tab: *tab,
                focus: None,
                all: false,
                force: false,
                enabled: None,
                terminal: Some(!no_terminal),
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
                enabled: None,
                terminal: None,
            },
            RemoteFolderCommand::List => Request::RemoteFolder {
                action: "list".into(),
                host: None,
                path: None,
                tab: None,
                focus: None,
                all: false,
                force: false,
                enabled: None,
                terminal: None,
            },
            RemoteFolderCommand::Ls { host, path } => Request::RemoteFolder {
                action: "ls".into(),
                host: Some(host.clone()),
                path: path.clone(),
                tab: None,
                focus: None,
                all: false,
                force: false,
                enabled: None,
                terminal: None,
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
                enabled: None,
                terminal: None,
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
                enabled: None,
                terminal: None,
            },
            RemoteFolderCommand::Pending { host, path } => Request::RemoteFolder {
                action: "pending".into(),
                host: host.clone(),
                path: path.clone(),
                tab: None,
                focus: None,
                all: false,
                force: false,
                enabled: None,
                terminal: None,
            },
            RemoteFolderCommand::Auto { state } => Request::RemoteFolder {
                action: "auto".into(),
                host: None,
                path: None,
                tab: None,
                focus: None,
                all: false,
                force: false,
                enabled: state.as_deref().map(|s| s == "on"),
                terminal: None,
            },
            RemoteFolderCommand::Push { host, path, force } => Request::RemoteFolder {
                action: "push".into(),
                host: host.clone(),
                path: path.clone(),
                tab: None,
                focus: None,
                all: false,
                force: *force,
                enabled: None,
                terminal: None,
            },
        },
        Command::Todo(sub) => {
            // 引数の並べ替えは 1 箇所（`todo_request`）に閉じる。CLI と MCP が
            // **同じ `Request::UserTask`** を組むので、経路ごとの差が生まれない
            todo_request(sub)
        }
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
                new_pane: args.new_pane.then_some(true),
            }
        }
        Command::RunDefault(ref args) => {
            Request::runner_ext_defaults(args.ext.clone(), args.command.clone(), args.remove)
        }
        Command::RunConfig(ref args) => {
            use tako_core::runner_config::{RunConfigField, RunScope};
            // 相対パスは CLI を打ったシェルの cwd 基準（外部のターミナルからでも同じ意味になる）
            let path = std::path::Path::new(&args.path);
            let path = if path.is_relative() {
                std::env::current_dir()
                    .map_err(|e| front_err(tako_control::Reason::CwdUnreadable(e.to_string())))?
                    .join(path)
            } else {
                path.to_path_buf()
            };
            let scope = if args.project {
                Some(RunScope::Project)
            } else if args.scope_file {
                Some(RunScope::File)
            } else {
                None
            };
            Request::RunnerDefaults {
                ext: None,
                command: None,
                remove: false,
                path: Some(path.display().to_string()),
                pane: args.pane.or_else(caller_pane),
                scope: scope.map(|s| s.as_str().to_string()),
                profile: args.profile.clone(),
                runtime: args.runtime.clone(),
                args: args.args.clone(),
                env: run_config_env(&args.env, &args.unset_env)?,
                cwd: args.cwd.clone(),
                before: args.before.clone(),
                board: args.board.clone(),
                port: args.port.clone(),
                target: args.target.clone(),
                // 値なしの `--reset` = 全項目
                reset: args.reset.as_ref().map(|r| {
                    if r.is_empty() {
                        vec![RunConfigField::RESET_ALL.to_string()]
                    } else {
                        r.clone()
                    }
                }),
            }
        }
    })
}

/// `tako run-config` の `--env K=V` / `--unset-env K` → `RunnerDefaults.env`（値 `None` = 消す。#1732）。
/// 同じ名前を設定と削除の両方に渡したら、どちらのつもりか決められないので拒否する
fn run_config_env(
    set: &[String],
    unset: &[String],
) -> Result<Option<std::collections::BTreeMap<String, Option<String>>>, String> {
    if set.is_empty() && unset.is_empty() {
        return Ok(None);
    }
    let mut map = std::collections::BTreeMap::new();
    for kv in set {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| front_err(tako_control::Reason::EnvShape(kv.clone())))?;
        map.insert(k.to_string(), Some(v.to_string()));
    }
    for k in unset {
        if map.contains_key(k) {
            return Err(front_err(tako_control::Reason::EnvAndUnsetEnv(
                k.to_string(),
            )));
        }
        map.insert(k.clone(), None);
    }
    Ok(Some(map))
}

/// 委任台帳のローカル処理（YAML I/O のみ。IPC 不要。#292）
fn ledger_cli(sub: &LedgerCommand) -> Result<(), String> {
    use tako_control::orchestrator::ledger;
    // 台帳の断り（`ConfigError`）を出口の枠（`error:`）と同じ表示言語で組む（#2365。`gate_cli` と同じ）
    init_local_lang();
    let shown = |e: tako_control::orchestrator::ConfigError| e.to_string();
    match sub {
        LedgerCommand::List {
            project,
            task_type,
            limit,
        } => {
            let l = ledger::Ledger::load_typed().map_err(shown)?;
            let mut entries: Vec<&ledger::LedgerEntry> = l.entries.iter().collect();
            if let Some(p) = project {
                entries.retain(|e| e.project == *p);
            }
            if let Some(t) = task_type {
                entries.retain(|e| e.task_type == *t);
            }
            if entries.len() > *limit {
                // 切り出し安全: Vec のスライス（文字列ではない）。直前で len > limit を確かめた
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
            let l = ledger::Ledger::load_typed().map_err(shown)?;
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
            ledger::record_outcome_typed(id, outcome, *rounds, note.as_deref()).map_err(shown)?;
            println!("recorded: {id} -> {outcome}");
            Ok(())
        }
        LedgerCommand::Amend { id, note } => {
            ledger::amend_entry_typed(id, note).map_err(shown)?;
            println!("amended: {id} (post_issue=true)");
            Ok(())
        }
        LedgerCommand::Prune { project_prefix } => {
            let removed =
                ledger::Ledger::mutate_typed(|l| l.prune_by_project_prefix(project_prefix))
                    .map_err(shown)?;
            println!("pruned: {removed} entries with project prefix '{project_prefix}'");
            Ok(())
        }
    }
}

/// run-interactive --wait: 起動 → ポーリングで完了待ち → exit code を返す
fn run_interactive_wait(command: &Command) -> Result<WaitEnd, String> {
    let request = build_request(command)?;
    let result = send_request(request)?;
    let lang = cli_lang();
    let pane = result["pane"]
        .as_u64()
        .ok_or_else(|| Reason::RunNoPaneId("run-interactive").text_in(lang))?;
    let started = ReplyNote::RunInteractiveStarted {
        pane,
        status: result["status"].as_str().unwrap_or("?").to_string(),
    };
    println!("{}", started.text_in(lang));
    wait_run_pane(pane)
}

/// run --wait: 起動 → ポーリングで完了待ち → exit code を返す
fn run_wait(command: &Command) -> Result<WaitEnd, String> {
    let request = build_request(command)?;
    let result = send_request(request)?;
    let lang = cli_lang();
    let pane = result["pane"]
        .as_u64()
        .ok_or_else(|| Reason::RunNoPaneId("run").text_in(lang))?;
    let started = ReplyNote::RunStarted {
        pane,
        command: result["command"].as_str().unwrap_or("?").to_string(),
    };
    println!("{}", started.text_in(lang));
    // #1662: 走らせる前に保存した未保存の編集（無ければ出さない）
    if let Some(saved) = result["saved_panes"]
        .as_array()
        .filter(|panes| !panes.is_empty())
    {
        let panes: Vec<String> = saved.iter().map(Value::to_string).collect();
        println!(
            "{}",
            ReplyNote::RunSavedBefore(panes.join(", ")).text_in(lang)
        );
    }
    wait_run_pane(pane)
}

/// `--wait` の聞き直しの間隔（#1657 以前からの 2 秒）
const RUN_WAIT_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// `--wait` が上限で打ち切ったときの CLI の終了コード（#1778）。
///
/// `timeout(1)` と同じ 124。#1778 以前は 1 で、**コマンド自身の失敗（非 0 はすべて 1）と
/// 見分けられなかった**。コマンドが自分で 124 で終わっても CLI は 1 を返す（非 0 は 1 に
/// 畳む）ので、124 は打ち切りだけを指す。応答 JSON の `timed_out: true` と対になる
const RUN_WAIT_TIMED_OUT_EXIT: u8 = 124;

/// `--wait` の結末（#1778。CLI の終了コードは [`WaitEnd::exit_code`] の 1 か所で決まる）
#[derive(Debug, Clone, PartialEq, Eq)]
enum WaitEnd {
    /// コマンドが終わった（終了コード）
    Exited(i64),
    /// 上限まで待っても終わらなかった（実行は止めていない）。値は stderr へ出す 1 行
    TimedOut(String),
}

impl WaitEnd {
    /// CLI の終了コード: 0 = 成功 / 1 = コマンドが非 0 で終わった / 124 = 上限で打ち切った
    fn exit_code(&self) -> u8 {
        match self {
            WaitEnd::Exited(0) => 0,
            WaitEnd::Exited(_) => 1,
            WaitEnd::TimedOut(_) => RUN_WAIT_TIMED_OUT_EXIT,
        }
    }

    /// stderr へ出す 1 行（成功なら無し）
    fn message(&self) -> Option<String> {
        match self {
            WaitEnd::Exited(0) => None,
            WaitEnd::Exited(code) => Some(front_err(Reason::RunExitedWithCode(*code))),
            WaitEnd::TimedOut(message) => Some(message.clone()),
        }
    }
}

/// `--wait` の結末を CLI の終了コードへ写す（エラーの出し方は他のコマンドと同じ `error: …`）
fn wait_exit_code(result: Result<WaitEnd, String>) -> ExitCode {
    match result {
        Ok(end) => {
            if let Some(message) = end.message() {
                eprintln!("error: {message}");
            }
            ExitCode::from(end.exit_code())
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// run / run-interactive の `--wait` の本体（#1662）: 実行ペインの終わりを**上限つきで**待つ。
///
/// #1662 以前は 2 秒刻みの無限ループで、サーバーや入力待ちを `--wait` で走らせると
/// CLI が永久に返らなかった（`.agent/conventions.md`「外部コマンドを待つときは上限を持つ」）。
/// 上限は `tako_core::probe::run_wait_timeout`（env `TAKO_RUN_WAIT_TIMEOUT_SECS`。
/// 0 / 不正は既定）、聞き直しは `probe::poll_with_timeout` の 1 実装。
/// 超えたら「まだ実行中」を出して [`WaitEnd::TimedOut`]（= 終了コード 124。#1778）で返す
/// （**実行は止めない**）
fn wait_run_pane(pane: u64) -> Result<WaitEnd, String> {
    use tako_core::probe::{poll_with_timeout, run_wait_timeout, Polled};
    let budget = run_wait_timeout();
    println!(
        "{}",
        ReplyNote::RunWaitBudget(budget.as_secs()).text_in(cli_lang())
    );
    let polled = poll_with_timeout(budget, RUN_WAIT_POLL, || {
        let status = send_request(Request::RunInteractiveStatus {
            pane,
            no_wait: false,
        })?;
        Ok::<_, String>((status["status"].as_str() == Some("exited")).then_some(status))
    })?;
    match polled {
        Polled::Done(status) => {
            println!("{}", pretty_json(&status));
            Ok(WaitEnd::Exited(status["exit_code"].as_i64().unwrap_or(1)))
        }
        Polled::TimedOut { waited } => {
            println!("{}", pretty_json(&run_wait_timed_out(pane, waited, budget)));
            Ok(WaitEnd::TimedOut(run_wait_timed_out_message(pane, budget)))
        }
    }
}

/// `--wait` が上限まで待っても終わらなかったときの応答（#1662。JSON の形はここだけ）
fn run_wait_timed_out(
    pane: u64,
    waited: std::time::Duration,
    budget: std::time::Duration,
) -> Value {
    serde_json::json!({
        "pane": pane,
        "status": "running",
        "timed_out": true,
        "waited_secs": waited.as_secs(),
        "limit_secs": budget.as_secs(),
    })
}

/// 上限で打ち切ったときの 1 行（続きの聞き方は最簡形 = #322）。表示言語で組む（#2365）
fn run_wait_timed_out_message(pane: u64, budget: std::time::Duration) -> String {
    front_err(run_wait_timed_out_reason(pane, budget))
}

/// [`run_wait_timed_out_message`] の文の型（表示言語を明示して引ける）
fn run_wait_timed_out_reason(pane: u64, budget: std::time::Duration) -> Reason {
    Reason::RunWaitTimedOut {
        pane,
        secs: budget.as_secs(),
        env: tako_core::probe::RUN_WAIT_TIMEOUT_ENV,
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
        refresh: args.refresh,
        boards: args.boards.clone(),
    };
    let result = send_request(request)?;
    println!("{}", pretty_json(&result));
    Ok(())
}

/// gate 操作のローカル処理（YAML I/O + コマンド実行。IPC 不要。#244）
fn gate_cli(sub: &GateCommand) -> Result<(), String> {
    // 置き場の断り（`acceptance_gates` が組む `Reason`）の表示言語を設定から決める（#2365）
    init_local_lang();
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
        return Err(front_err(tako_control::Reason::GatePredicate));
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
    serde_json::to_string(&criteria).map_err(|e| {
        front_err(tako_control::Reason::JsonEncodeFailed {
            key: None,
            error: e.to_string(),
        })
    })
}

fn parse_direction(s: &str) -> Result<Direction, String> {
    match s {
        "right" | "r" => Ok(Direction::Right),
        "down" | "d" => Ok(Direction::Down),
        "left" | "l" => Ok(Direction::Left),
        "up" | "u" => Ok(Direction::Up),
        _ => Err(front_err(tako_control::Reason::CliInvalidDirection(
            s.to_string(),
        ))),
    }
}

/// `tako file copy SRC... DEST` の要求（#1860 / #1895）。コピー元が複数でも**まとめた 1 要求**
/// （`FileOpMany` = 1 つのジョブ。進み具合も取り消しも 1 つ = MCP の `paths` と同じ要求・応答）。
/// どれも CLI の cwd 基準で絶対化する（GUI の cwd で読ませない = move と同じ）
fn file_copy_requests(paths: &[String]) -> Result<Vec<Request>, String> {
    let Some((dest, sources)) = paths.split_last().filter(|(_, s)| !s.is_empty()) else {
        return Err(front_err(tako_control::Reason::FileCopyArgs));
    };
    let dest = resolve_cli_path(dest);
    if !tako_core::file_copy::legacy_1895() {
        return Ok(vec![file_op_for(
            tako_control::protocol::FileOpKind::Copy,
            sources,
            Some(dest),
        )]);
    }
    // A/B（`TAKO_1895_LEGACY=1`）: #1895 の前 = コピー元 1 つにつき 1 件（別のジョブ）
    Ok(sources
        .iter()
        .map(|src| Request::FileOp {
            op: tako_control::protocol::FileOpKind::Copy,
            path: resolve_cli_path(src),
            name: None,
            pane: None,
            dest: Some(dest.clone()),
        })
        .collect())
}

/// `tako file copy`（#1860 / #1895）。1 往復で結果を出し、まとめた要求で断られたものがあれば
/// 終了コード 1（`cp` と同じ。写せたものは写してある）。A/B の 1 件ずつの要求は 1 行ずつ出す
fn file_copy_cli(paths: &[String]) -> Result<(), String> {
    let requests = file_copy_requests(paths)?;
    let total = requests.len();
    if total == 1 {
        let request = requests.into_iter().next().expect("1 件");
        let many = matches!(request, Request::FileOpMany { .. });
        let result = send_request(request)?;
        println!("{result}");
        let failed = result["failed"].as_array().map_or(0, Vec::len);
        if many && failed > 0 {
            let done = result["done"].as_array().map_or(0, Vec::len);
            return Err(front_err(tako_control::Reason::CopyPartlyFailed {
                total: done + failed,
                failed,
            }));
        }
        return Ok(());
    }
    let mut failed = 0usize;
    for request in requests {
        match send_request(request) {
            Ok(result) => println!("{result}"),
            Err(e) => {
                failed += 1;
                eprintln!("error: {e}");
            }
        }
    }
    if failed > 0 {
        return Err(front_err(tako_control::Reason::CopyPartlyFailed {
            total,
            failed,
        }));
    }
    Ok(())
}

/// 1 件なら単数の `FileOp`、複数ならまとめた `FileOpMany`（#1867。MCP の `path` / `paths` と
/// 同じ振り分け = 同じ入力なら同じ要求・同じ応答）。パスは CLI の cwd 基準で絶対化する
fn file_op_for(
    op: tako_control::protocol::FileOpKind,
    paths: &[String],
    dest: Option<String>,
) -> Request {
    match paths {
        [one] => Request::FileOp {
            op,
            path: resolve_cli_path(one),
            name: None,
            pane: None,
            dest,
        },
        many => Request::FileOpMany {
            op,
            paths: many.iter().map(|p| resolve_cli_path(p)).collect(),
            dest,
        },
    }
}

/// `tako file move` / `trash`（#1867）。結果を出してから、まとめた要求で断られたものがあれば
/// 終了コード 1（`mv` / `rm` と同じ。続きは dispatch が続けている）
fn file_many_cli(command: &Command) -> Result<(), String> {
    let request = build_request(command)?;
    let many = matches!(request, Request::FileOpMany { .. });
    let result = send_request(request)?;
    print_result(command, &result);
    let failed = result["failed"].as_array().map_or(0, Vec::len);
    if many && failed > 0 {
        let done = result["done"].as_array().map_or(0, Vec::len);
        return Err(front_err(tako_control::Reason::ManyPartlyFailed {
            total: done + failed,
            failed,
        }));
    }
    Ok(())
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

fn pretty_json(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// GUI を経由せず手元で dispatch を直呼びした枝の断りを、表示言語で文字列にする（#1762）。
///
/// GUI 経由の断りは GUI のプロセスが表示言語で文字列にして返す。手元の枝は表示言語を
/// 初期化していないことがあり（静的既定は英語）、そのまま `to_string()` すると、日本語表示の
/// 人へ前置き（「無効なパラメータ: 」）だけ英語で出る。前置きの言語はここで設定から解決する
/// （解決は他の枝と同じ `LangSetting::resolve`。settings.json には触らない = #2238。
/// 理由の文は作られた時点のまま）
fn local_dispatch_err(e: tako_control::DispatchError) -> String {
    e.text_in(tako_control::settings::peek_lang_setting().resolve())
}

/// GUI を経由せず手元で dispatch を直呼びする枝の、理由の文の表示言語を決める（#2365）。
///
/// dispatch は理由の文（`tako_control::Reason` / `tako_core::LayoutParamError`）を
/// 組んだ時点の表示言語で文字列にする。前置き（[`local_dispatch_err`]）と同じ解決で
/// **呼ぶ前に**決めておかないと、日本語表示の人へ理由の文だけ英語（静的既定）で出る
fn init_local_lang() {
    tako_core::i18n::set_lang(tako_control::settings::peek_lang_setting().resolve());
}

/// dispatch へ送る前に CLI が手元で断る文（#2365）。表示言語は [`local_dispatch_err`] と同じ解決
fn front_err(reason: tako_control::Reason) -> String {
    reason.text_in(cli_lang())
}

/// CLI が手元で組む断り・行の表示言語（#2365）。前置き（[`local_dispatch_err`]）と同じ解決で、
/// 表示言語のグローバルには触らない（呼ぶたびに settings.json を読む = 手元の処理の 1 回分）
fn cli_lang() -> tako_core::i18n::Lang {
    tako_control::settings::peek_lang_setting().resolve()
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

/// `tako todo list` の既定出力（#1450）。**未完了の件数を最後に出す**
/// （画面のバッジと同じ数字を CLI でも見られるようにする）
fn print_todo_list(result: &Value) {
    let tasks = result["tasks"].as_array().cloned().unwrap_or_default();
    if tasks.is_empty() {
        println!("ユーザータスクが無い");
    }
    for task in &tasks {
        let responses = task["responses"].as_array().map(Vec::len).unwrap_or(0);
        let reply = if responses == 0 {
            String::new()
        } else {
            format!("  返答:{responses}")
        };
        let due = task["due"]
            .as_str()
            .map(|d| format!("  期限:{d}"))
            .unwrap_or_default();
        println!(
            "{:<6}  {:10}  {:9}  {}{}{}",
            task["id"].as_str().unwrap_or("-"),
            task["kind"].as_str().unwrap_or("-"),
            task["status"].as_str().unwrap_or("-"),
            task["title"].as_str().unwrap_or("-"),
            due,
            reply,
        );
    }
    println!("未完了 {} 件", result["open_count"].as_u64().unwrap_or(0));
}

/// 1 件の詳細（`show` / `add` / `respond` の既定出力）
/// `tako todo expand` / `collapse` の既定出力（#1479）。
/// **画面が無い（GUI が動いていない）ときに「開いた」と言わない**
fn print_todo_expanded(result: &Value) {
    match result.get("expanded").and_then(Value::as_str) {
        Some(id) => println!("expanded: {id}"),
        None => println!("expanded: (なし)"),
    }
    let visible = result
        .get("panel_visible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let view = result
        .get("panel_view")
        .and_then(Value::as_str)
        .unwrap_or("-");
    println!(
        "panel: {} / view: {view}",
        if visible { "表示" } else { "非表示" }
    );
}

fn print_todo_detail(result: &Value) {
    println!(
        "{} [{}/{}] {}",
        result["id"].as_str().unwrap_or("-"),
        result["kind"].as_str().unwrap_or("-"),
        result["status"].as_str().unwrap_or("-"),
        result["title"].as_str().unwrap_or("-"),
    );
    if let Some(body) = result["body"].as_str().filter(|b| !b.trim().is_empty()) {
        println!("{body}");
    }
    for a in result["attachments"].as_array().unwrap_or(&Vec::new()) {
        let mark = if a["exists"].as_bool() == Some(true) {
            ""
        } else {
            "（見つからない）"
        };
        println!("  添付: {}{mark}", a["path"].as_str().unwrap_or("-"));
    }
    for c in result["copy_texts"].as_array().unwrap_or(&Vec::new()) {
        let label = c["label"].as_str().unwrap_or("");
        let head: String = c["text"].as_str().unwrap_or("").chars().take(40).collect();
        println!("  コピー用[{label}]: {head}");
    }
    for l in result["links"].as_array().unwrap_or(&Vec::new()) {
        println!("  リンク: {}", l.as_str().unwrap_or("-"));
    }
    // #2016: やりとりは古い順（積んだ順）に、時刻つきで出す。時刻の表記は右パネルと同じ
    // 1 実装（`user_task::response_stamp`）。複数行の本文は 2 行目以降を字下げして続ける
    let now = tako_core::user_task::unix_now();
    let offset = tako_core::platform::clock::local_utc_offset();
    for r in result["responses"].as_array().unwrap_or(&Vec::new()) {
        print!("{}", todo_response_text(r, now, offset));
    }
    if let Some(d) = result["delivery"].as_object() {
        let reason = d
            .get("reason")
            .and_then(Value::as_str)
            .map(|r| format!("（{r}）"))
            .unwrap_or_default();
        println!(
            "  配送: {}{reason} pane:{}",
            d.get("state").and_then(Value::as_str).unwrap_or("-"),
            d.get("pane")
                .and_then(Value::as_u64)
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
        );
    }
}

/// やりとり 1 件の行（`tako todo show` の既定出力。#2016。機械検証できるよう印字と分けてある）。
/// `  返答 11:02 [pc] needs_change: 本文` の形で、本文の 2 行目以降は字下げして続ける
fn todo_response_text(r: &Value, now: i64, utc_offset: i32) -> String {
    let stamp = r["at"]
        .as_i64()
        .map(|at| tako_core::user_task::response_stamp(at, now, utc_offset))
        .unwrap_or_else(|| "-".to_string());
    let comment = r["comment"].as_str().unwrap_or("");
    let mut lines = comment.lines();
    let mut out = format!(
        "  返答 {stamp} [{}] {}: {}\n",
        r["via"].as_str().unwrap_or("-"),
        r["decision"].as_str().unwrap_or("-"),
        lines.next().unwrap_or(""),
    );
    for rest in lines {
        out.push_str(&format!("      {rest}\n"));
    }
    out
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
    print!("{}", gate_result_text(result));
}

/// 証拠の行頭の字下げ（2 行目以降も同じ幅で揃える = #1758）
const GATE_EVIDENCE_INDENT: &str = "         ";

/// `tako task gate set / check / show` の表示本文（機械検証できるよう印字と分けてある）
fn gate_result_text(result: &Value) -> String {
    let mut out = String::new();
    let task_id = result["task_id"].as_str().unwrap_or("-");
    let overall = result["overall"].as_str().unwrap_or("?");
    let overall_marker = match overall {
        "passed" => "[PASSED]",
        "failed" => "[FAILED]",
        _ => "[PENDING]",
    };
    out.push_str(&format!("Gate: {task_id}  {overall_marker}\n"));
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
            out.push_str(&format!("  {marker} {id}: {kind_detail}\n"));
            if let Some(ev) = c["evidence"].as_str() {
                // 証拠は複数行で来る（`format_evidence` の stdout の末尾 5 行など）。
                // 1 行目にだけ字下げを付けると 2 行目以降が 0 桁目へ落ちる（#1758）
                for line in gate_evidence_preview(ev).split('\n') {
                    let line = line.strip_suffix('\r').unwrap_or(line);
                    out.push_str(&format!("{GATE_EVIDENCE_INDENT}{line}\n"));
                }
            }
        }
    }
    out
}

/// 証拠の見出しの上限（文字数。`…` を含む。全文は `--json` で見られる）
const GATE_EVIDENCE_PREVIEW_CHARS: usize = 120;

/// `tako task gate set / check / show` の 1 基準ぶんの証拠の見出し。
///
/// 証拠はコマンドの出力そのもの（`acceptance_gates::format_evidence`）なので日本語が来る。
/// バイト位置の `&ev[..120]` で切っていたときは、120 バイト目が文字の途中に当たると
/// CLI ごと panic していた（#1746。`check` は結果を保存したあとで落ちるので、
/// ゲートは記録されるのに終了コードが 101 になる）
fn gate_evidence_preview(ev: &str) -> String {
    tako_core::text::truncate_chars(ev, GATE_EVIDENCE_PREVIEW_CHARS)
}

/// 送達フローの顛末を 1 行で出す（Issue #1259。MCP の `delivery` と同じ材料）。
///
/// `tako_send_input` / `tako send --await-prompt` は `queued: true` を即返すので、
/// **その後どうなったか**を問える口がここ。理由コードは `persist.log` の
/// 「送達フロー:」行と同じ語彙（`tako_core::prompt_delivery`）
fn print_delivery(result: &Value) {
    if let Some(line) = delivery_line(result) {
        eprintln!("{line}");
    }
    if let Some(line) = paste_guard_line(result) {
        eprintln!("{line}");
    }
}

/// `tako send` の鍵らしき文字の警告（#2050）の 1 行。載せるのは規則名と伏せ字だけ（応答と同じ）
fn paste_guard_line(result: &Value) -> Option<String> {
    let g = result.get("paste_guard").filter(|v| !v.is_null())?;
    let found: Vec<String> = g["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    format!(
                        "{} {}",
                        f["rule"].as_str().unwrap_or("?"),
                        f["masked"].as_str().unwrap_or("")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Some(format!(
        "[paste-guard] verdict={} {}（鍵らしき文字を含むまま送った。設定は tako paste-guard）",
        g["verdict"].as_str().unwrap_or("?"),
        found.join(", ")
    ))
}

/// `[delivery] …` の 1 行を組む（機械検証できるよう印字と分けてある）
fn delivery_line(result: &Value) -> Option<String> {
    let d = result.get("delivery").filter(|v| !v.is_null())?;
    let mut line = format!("[delivery] state={}", d["state"].as_str().unwrap_or("?"));
    if let Some(reason) = d["reason"].as_str() {
        line.push_str(&format!(" reason={reason}"));
    }
    if let Some(outcome) = d["outcome"].as_str() {
        line.push_str(&format!(" outcome={outcome}"));
    }
    if let Some(transport) = d["transport"].as_str() {
        line.push_str(&format!(" transport={transport}"));
    }
    if let Some(secs) = d["elapsed_secs"].as_u64() {
        line.push_str(&format!(" elapsed={secs}s"));
    }
    if let Some(note) = d["note"].as_str() {
        line.push_str(&format!("（{note}）"));
    }
    Some(line)
}

/// `tako jump` の表示（#1677）。中身の正本は dispatch の応答で、ここは体裁だけ
/// `tako search` の出力（#1694）。結果は grep と同じ `パス:行:桁: 抜粋`（パスは自分の cwd からの
/// 相対 = ターミナルのリンクとして開ける）で、最後に件数と打ち切りの理由を 1 行
fn print_search(args: &SearchArgs, result: &Value) {
    if args.json {
        println!("{}", pretty_json(result));
        return;
    }
    match args.action() {
        "open" => {
            println!(
                "開いた: {}:{}:{}（{}）",
                result["path"].as_str().unwrap_or(""),
                result["line"],
                result["column"],
                result["landing"].as_str().unwrap_or("")
            );
            return;
        }
        "clear" => {
            if result["cleared"] == Value::Bool(true) {
                println!("右パネルの検索結果を閉じた");
            } else {
                println!("閉じる検索結果は無かった");
            }
            return;
        }
        _ => {}
    }
    match result["status"].as_str() {
        Some("empty") => {
            println!("右パネルに検索結果は無い（tako search <検索語> --show で出す）");
            return;
        }
        Some("searching") => {
            println!(
                "検索中: {}",
                result["running"]["query"].as_str().unwrap_or("")
            );
            return;
        }
        Some("error") => {
            println!(
                "検索できなかった: {}",
                result["reason"].as_str().unwrap_or("")
            );
            return;
        }
        _ => {}
    }
    let cwd = std::env::current_dir().ok();
    let items = result["items"].as_array().cloned().unwrap_or_default();
    for item in &items {
        let path = std::path::PathBuf::from(item["path"].as_str().unwrap_or(""));
        let shown = cwd
            .as_deref()
            .and_then(|c| path.strip_prefix(c).ok())
            .unwrap_or(&path);
        println!(
            "{}:{}:{}: {}",
            shown.display(),
            item["line"],
            item["column"],
            item["excerpt"].as_str().unwrap_or("")
        );
    }
    let total = result["total"].as_u64().unwrap_or(0);
    let mut tail = vec![if total == 0 {
        format!(
            "見つからなかった（{} ファイルを探した）",
            result["files_searched"]
        )
    } else {
        format!(
            "{total} 件（{} ファイル・{} ms）",
            result["files_matched"], result["elapsed_ms"]
        )
    }];
    match result["stopped"].as_str() {
        Some("limit") => tail.push(format!(
            "上限 {} 件で打ち切り（--path で範囲を狭めるか検索語を絞る）",
            tako_core::workspace_search::MAX_RESULTS
        )),
        Some("time") => tail.push(format!(
            "所要の上限（{} 秒）で打ち切り",
            tako_core::workspace_search::DEFAULT_TIME_LIMIT.as_secs()
        )),
        Some("cancelled") => tail.push("取り消された（次の検索が始まった / 閉じられた）".into()),
        _ => {}
    }
    let more = result["truncated"].as_u64().unwrap_or(0);
    if more > 0 {
        tail.push(format!("ほか {more} 件（--limit 0 で全部）"));
    }
    let skipped = &result["skipped"];
    let skip = [
        ("バイナリ", &skipped["binary"]),
        ("大きすぎる", &skipped["too_large"]),
        ("読めない", &skipped["unreadable"]),
    ]
    .iter()
    .filter_map(|(label, n)| {
        n.as_u64()
            .filter(|n| *n > 0)
            .map(|n| format!("{label} {n}"))
    })
    .collect::<Vec<_>>();
    if !skip.is_empty() {
        tail.push(format!("探していない: {}", skip.join("・")));
    }
    println!("{}", tail.join(" / "));
}

fn print_jump(sub: &JumpCommand, result: &Value) {
    let place = |v: &Value| -> String {
        let path = v["path"].as_str().unwrap_or("");
        match v["line"].as_u64() {
            Some(line) => format!("{path}:{line}"),
            None => path.to_string(),
        }
    };
    match sub {
        JumpCommand::List { json: true } => println!("{}", pretty_json(result)),
        JumpCommand::Back(args) | JumpCommand::Forward(args) if args.json => {
            println!("{}", pretty_json(result))
        }
        JumpCommand::List { json: false } => {
            let entries = result["entries"].as_array().cloned().unwrap_or_default();
            if entries.is_empty() {
                println!(
                    "ジャンプ履歴は空（行を指定して開くと積まれる: tako open <file> --line <行>）"
                );
                return;
            }
            let current = result["history"]["current"].as_u64();
            for (i, entry) in entries.iter().enumerate() {
                let mark = if current == Some(i as u64) { ">" } else { " " };
                let closed = if entry["pane_alive"] == Value::Bool(false) {
                    "（ペインは閉じた。戻ると開き直す）"
                } else {
                    ""
                };
                println!(
                    "{mark} {i:>3}  {}  pane {}{closed}",
                    place(entry),
                    entry["pane"]
                );
            }
        }
        JumpCommand::Back(_) | JumpCommand::Forward(_) => {
            for gone in result["dropped"].as_array().into_iter().flatten() {
                println!("読み飛ばした（ファイルが無い）: {}", place(gone));
            }
            if result["moved"] == Value::Bool(true) {
                let reopened = if result["reopened"] == Value::Bool(true) {
                    "（閉じたペインの項目なので開き直した）"
                } else {
                    ""
                };
                println!(
                    "{}  pane {}{reopened}",
                    place(&result["location"]),
                    result["open"]["pane"]
                );
            } else {
                println!(
                    "{}",
                    result["reason"].as_str().unwrap_or("移れる履歴が無い")
                );
            }
        }
    }
}

/// `tako lsp` の表示（#1678）。中身の正本は dispatch の応答で、ここは体裁だけ
/// `tako lsp results` 等の要求（#1685。`pane` は結果を問い合わせたペインで、省略は右パネルの結果 =
/// 呼び出し元では埋めない）
fn results(action: &str, pane: Option<u64>, item: Option<usize>, limit: Option<usize>) -> Request {
    Request::LspResults {
        action: action.to_string(),
        pane,
        item,
        open: None,
        focus: None,
        limit,
    }
}

/// 参照・階層の応答を人向けの行へ（#1685）。1 行目は status と件数、続いて一覧の行
/// （`id  path:行:桁  抜粋`）かツリーの見えている行（字下げ + 開閉の印 `-` / `+` / `.`）。
/// 印は ASCII（開いている `-` / 閉じていて子がある or 未読み込み `+` / 子が 0 件 `.`）
fn lsp_xref_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let num = |v: &Value| v.as_u64().map_or("-".to_string(), |n| n.to_string());
    let status = result["status"].as_str().unwrap_or("-");
    let mut head = vec![status.to_string()];
    for key in ["kind", "server"] {
        if let Some(v) = result[key].as_str() {
            head.push(format!("{key}={v}"));
        }
    }
    if let Some(symbol) = result["from"]["symbol"].as_str().filter(|s| !s.is_empty()) {
        head.push(format!("symbol={symbol}"));
    }
    for key in ["total", "node_count", "shown", "truncated", "retained"] {
        if !result[key].is_null() {
            head.push(format!("{key}={}", num(&result[key])));
        }
    }
    if let Some(landing) = result["landing"].as_str() {
        head.push(format!("landing={landing}"));
    }
    if let Some(cleared) = result["cleared"].as_bool() {
        head.push(format!("cleared={cleared}"));
    }
    let mut lines = vec![head.join(" ")];
    let place = |row: &Value| {
        format!(
            "{}:{}:{}",
            text(&row["path"]),
            num(&row["line"]),
            num(&row["column"])
        )
    };
    for row in result["items"].as_array().into_iter().flatten() {
        lines.push(format!(
            "{:>4}  {}  {}",
            num(&row["id"]),
            place(row),
            text(&row["text"])
        ));
    }
    for node in result["nodes"].as_array().into_iter().flatten() {
        let depth = node["depth"].as_u64().unwrap_or(0) as usize;
        let mark = match (node["expanded"].as_bool(), node["children"].as_u64()) {
            (Some(true), _) => "-",
            (_, Some(0)) => ".",
            _ => "+",
        };
        lines.push(format!(
            "{:>4}  {}{} {}  {}  {}",
            num(&node["id"]),
            "  ".repeat(depth),
            mark,
            text(&node["name"]),
            text(&node["symbol_kind"]),
            place(node)
        ));
    }
    if let Some(location) = result.get("location").filter(|l| l.is_object()) {
        lines.push(format!(
            "  {}  {}",
            place(location),
            text(&location["text"])
        ));
    }
    for key in ["reason", "next_step"] {
        if let Some(v) = result[key].as_str() {
            lines.push(format!("  {v}"));
        }
    }
    lines
}

fn print_lsp(sub: &LspCommand, result: &Value) {
    if !sub.json() && sub.goto_kind().is_some() {
        for line in lsp_goto_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Format { .. }) {
        for line in lsp_format_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Rename(_)) {
        for line in lsp_rename_lines(result) {
            println!("{line}");
        }
        return;
    }
    if let LspCommand::FormatOnSave { .. } = sub {
        let state = if result["enabled"] == Value::Bool(true) {
            "on"
        } else {
            "off"
        };
        println!("format-on-save: {state}");
        if let Some(note) = result["note"].as_str() {
            println!("  {note}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Completion(_)) {
        for line in lsp_completion_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Menu(_)) {
        for line in lsp_menu_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Hover(_)) {
        for line in lsp_hover_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json()
        && (sub.xref_kind().is_some()
            || matches!(
                sub,
                LspCommand::Results(_)
                    | LspCommand::Expand(_)
                    | LspCommand::Collapse(_)
                    | LspCommand::OpenResult(_)
                    | LspCommand::ClearResults { .. }
            ))
    {
        for line in lsp_xref_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Install { .. }) {
        for line in lsp_install_lines(result) {
            println!("{line}");
        }
        return;
    }
    if !sub.json() && matches!(sub, LspCommand::Options { .. }) {
        for line in lsp_options_lines(result) {
            println!("{line}");
        }
        return;
    }
    if sub.json()
        || !matches!(
            sub,
            LspCommand::Status { .. } | LspCommand::Servers { .. } | LspCommand::Diagnostics { .. }
        )
    {
        println!("{}", pretty_json(result));
        return;
    }
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    if result["enabled"] == Value::Bool(false) {
        println!("{}", text(&result["reason"]));
        return;
    }
    if matches!(sub, LspCommand::Diagnostics { .. }) {
        for line in lsp_diagnostics_lines(result) {
            println!("{line}");
        }
        return;
    }
    let servers = result["servers"].as_array().cloned().unwrap_or_default();
    if servers.is_empty() {
        if let Some(note) = result["note"].as_str() {
            println!("{note}");
        }
        return;
    }
    for server in &servers {
        if matches!(sub, LspCommand::Servers { .. }) {
            let installed = server["installed"].as_bool().unwrap_or(false);
            let where_ = if installed {
                text(&server["path"])
            } else {
                "-".into()
            };
            // #1944: 出どころ（path / override / managed = tako が取った版）
            let source = server["source"].as_str().unwrap_or("-");
            println!(
                "{:<28} {:<14} {:<8} {}",
                text(&server["id"]),
                if installed {
                    "installed"
                } else {
                    "not_installed"
                },
                source,
                where_
            );
        } else {
            println!(
                "{:<28} {:<14} pid={} docs={} diagnostics={} {}",
                text(&server["id"]),
                text(&server["state"]),
                server["pid"],
                server["documents"],
                server["diagnostics"],
                text(&server["root"])
            );
        }
        for key in ["reason", "next_step"] {
            if let Some(line) = server[key].as_str() {
                println!("  {line}");
            }
        }
    }
    if matches!(sub, LspCommand::Status { .. }) {
        for line in lsp_status_tail_lines(result) {
            println!("{line}");
        }
    }
}

/// `tako lsp options` の人向けの体裁（#2035）。サーバごとにキー = 値（出どころ）と、動いている
/// サーバへ送った値がいまの実効値と同じか（違えば restart で効く）
fn lsp_options_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    if result["enabled"] == Value::Bool(false) {
        return vec![text(&result["reason"])];
    }
    let mut out = Vec::new();
    if result["legacy"] == Value::Bool(true) {
        out.push("TAKO_2035_LEGACY=1: initializationOptions を送らない".to_string());
    }
    for server in result["servers"].as_array().cloned().unwrap_or_default() {
        out.push(text(&server["id"]));
        let options = server["options"].as_array().cloned().unwrap_or_default();
        if options.is_empty() {
            out.push("  (なし)".to_string());
        }
        for option in options {
            let source = text(&option["source"]);
            let origin = match (source.as_str(), &option["default"]) {
                ("user", Value::Null) => "user".to_string(),
                ("user", default) => format!("user, default {default}"),
                _ => source,
            };
            out.push(format!(
                "  {} = {}  ({origin})",
                text(&option["key"]),
                option["value"]
            ));
        }
        for running in server["running"].as_array().cloned().unwrap_or_default() {
            let mark = if running["current"] == Value::Bool(true) {
                "current"
            } else {
                "stale (tako lsp restart)"
            };
            out.push(format!(
                "  running {}  sent={}  {mark}",
                text(&running["root"]),
                running["sent"]
            ));
        }
    }
    if let Some(note) = result["note"].as_str() {
        out.push(note.to_string());
    }
    out
}

/// `tako lsp install` の人向けの体裁（#1944）。1 サーバ 1 行（ID・結果・置き場か理由）+ 次の一手
fn lsp_install_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    if result["enabled"] == Value::Bool(false) {
        return vec![text(&result["reason"])];
    }
    let mut out = Vec::new();
    for server in result["servers"].as_array().cloned().unwrap_or_default() {
        let status = text(&server["status"]);
        let detail = match status.as_str() {
            "installed" | "present" => {
                let mb = server["disk_bytes"].as_u64().unwrap_or(0) as f64 / 1_000_000.0;
                let secs = server["elapsed_ms"].as_u64().unwrap_or(0) as f64 / 1000.0;
                format!("{} ({mb:.1} MB, {secs:.1}s)", text(&server["path"]))
            }
            "global" => text(&server["path"]),
            _ => text(&server["reason"]),
        };
        out.push(format!(
            "{:<28} {:<11} {detail}",
            text(&server["id"]),
            status
        ));
        if let Some(next) = server["next_step"].as_str() {
            out.push(format!("  {next}"));
        }
        // #1977: 入れた後に前の版の段を片付けた / 残した
        out.extend(lsp_cleanup_lines(&server["cleanup"], "  "));
    }
    out
}

/// 古い版の片付けの結果の人向けの行（#1977。消した・残したがあるときだけ。中身の正本は
/// `LspManager` の `cleanup` = `fetch::Sweep::to_json`）
fn lsp_cleanup_lines(cleanup: &Value, indent: &str) -> Vec<String> {
    use tako_control::lsp::text;
    let entries = |key: &str| -> Vec<String> {
        cleanup[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                let name = format!(
                    "{}/{}",
                    d["id"].as_str().unwrap_or("-"),
                    d["version"].as_str().unwrap_or("-")
                );
                match d["reason"].as_str() {
                    Some(reason) => format!("{name} {reason}"),
                    None => name,
                }
            })
            .collect()
    };
    let mut out = Vec::new();
    let removed = entries("removed");
    if !removed.is_empty() {
        let size =
            tako_control::lsp::fetch::format_mb(cleanup["freed_bytes"].as_u64().unwrap_or(0));
        out.push(format!(
            "{indent}{}",
            text::fill(
                text::CLEANUP_REMOVED_NOTE,
                &[("items", &removed.join(", ")), ("size", &size)]
            )
        ));
    }
    let kept = entries("kept");
    if !kept.is_empty() {
        out.push(format!(
            "{indent}{}",
            text::fill(text::CLEANUP_KEPT_NOTE, &[("items", &kept.join(", "))])
        ));
    }
    out
}

/// `tako lsp status` の末尾（#1977）: 直前の古い版の片付けと、未導入のサーバの引き直し
fn lsp_status_tail_lines(result: &Value) -> Vec<String> {
    use tako_control::lsp::text;
    let mut out = lsp_cleanup_lines(&result["fetch"]["cleanup"], "");
    let recheck = &result["recheck"];
    let targets: Vec<&str> = recheck["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !targets.is_empty() {
        let cost = recheck["last_cost_ms"]
            .as_u64()
            .map_or_else(|| "-".to_string(), |ms| ms.to_string());
        out.push(text::fill(
            text::RECHECK_NOTE,
            &[
                ("targets", &targets.join(", ")),
                ("interval", &recheck["interval_secs"].to_string()),
                ("cost", &cost),
            ],
        ));
    }
    // #2034: tako の外で変わったファイルの監視（動いていれば根、監視しない根があれば理由）
    let watch = &result["file_watch"];
    let paths = |key: &str, with_reason: bool| -> Vec<String> {
        watch[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let path = item["path"].as_str()?;
                Some(match (with_reason, item["reason"].as_str()) {
                    (true, Some(reason)) => format!("{path} {reason}"),
                    _ => path.to_string(),
                })
            })
            .collect()
    };
    if watch["active"] == Value::Bool(true) {
        out.push(text::fill(
            text::WATCH_NOTE,
            &[
                ("roots", &paths("roots", false).join(", ")),
                ("batches", &watch["batches"].to_string()),
            ],
        ));
    }
    let skipped = paths("skipped", true);
    if !skipped.is_empty() {
        out.push(text::fill(
            text::WATCH_SKIPPED_NOTE,
            &[("items", &skipped.join(", "))],
        ));
    }
    out
}

/// `tako lsp menu` の人向けの体裁（#1684）。中身の正本は dispatch の応答。
/// 1 行目は `status`・サーバ・識別子、続けて 1 項目 1 行（id と、押したときと同じコマンド）か
/// 理由と次の一手
fn lsp_menu_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let mut head = vec![text(&result["status"])];
    if let Some(server) = result["server"].as_str() {
        head.push(server.to_string());
    }
    if let Some(symbol) = result["symbol"].as_object() {
        head.push(format!(
            "{}（{}）",
            symbol.get("text").map(text).unwrap_or_default(),
            symbol.get("kind").map(text).unwrap_or_default()
        ));
    }
    let mut out = vec![head.join(" ")];
    let items = result["items"].as_array().cloned().unwrap_or_default();
    for item in &items {
        out.push(format!(
            "  {:<22} {}",
            text(&item["id"]),
            lsp_menu_command(&item["args"])
        ));
    }
    if items.is_empty() {
        for key in ["reason", "next_step"] {
            if let Some(line) = result[key].as_str() {
                out.push(format!("  {line}"));
            }
        }
    }
    out
}

/// 項目の `args`（MCP `tako_lsp` の引数）を同じ操作の CLI へ写す（#1684。表示だけ）
fn lsp_menu_command(args: &Value) -> String {
    let n = |key: &str| args[key].as_u64();
    let mut cmd = format!("tako lsp {}", args["action"].as_str().unwrap_or_default());
    if let Some(pane) = n("pane") {
        cmd.push_str(&format!(" --pane {pane}"));
    }
    match (n("line"), n("column"), n("end_line"), n("end_column")) {
        (Some(l), Some(c), Some(el), Some(ec)) => {
            cmd.push_str(&format!(" --range {l}:{c}-{el}:{ec}"))
        }
        (Some(l), Some(c), None, None) => cmd.push_str(&format!(" --line {l} --column {c}")),
        _ => {}
    }
    // #1686: 名前の変更は今の名前が入る（書き換えて打つ）。識別子の字でなければ引用する
    if let Some(name) = args["new_name"].as_str() {
        let plain = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
        if plain {
            cmd.push_str(&format!(" --new-name {name}"));
        } else {
            cmd.push_str(&format!(" --new-name '{}'", name.replace('\'', "'\\''")));
        }
    }
    cmd
}

/// `tako lsp diagnostics` の人向けの体裁（#1679）。
///
/// 文書ごとに見出し（パス・ペイン・重大度ごとの数）→ 1 件 1 行。位置は
/// `tako edit replace-range` へそのまま渡せる `行:桁-行:桁`（桁は 0 始まりの UTF-8 バイト）
fn lsp_diagnostics_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut out = Vec::new();
    let documents = result["documents"].as_array().cloned().unwrap_or_default();
    if documents.is_empty() {
        if let Some(note) = result["note"].as_str() {
            out.push(note.to_string());
        }
        return out;
    }
    for doc in &documents {
        let counts = tako_core::lsp::diagnostic::Severity::NAMES
            .iter()
            .map(|name| format!("{name}={}", doc["counts"][name].as_u64().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(" ");
        out.push(format!(
            "{} (pane {}) {counts}",
            text(&doc["path"]),
            doc["pane"]
        ));
        for key in ["reason", "next_step"] {
            if let Some(line) = doc[key].as_str() {
                out.push(format!("  {line}"));
            }
        }
        for d in doc["diagnostics"].as_array().into_iter().flatten() {
            let at = |p: &Value| format!("{}:{}", p["line"], p["column"]);
            let origin = [d["source"].as_str(), d["code"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let message = text(&d["message"]);
            let first = message.lines().next().unwrap_or("");
            let mut line = format!(
                "  {}-{}  {:<7}  {first}",
                at(&d["range"]["start"]),
                at(&d["range"]["end"]),
                text(&d["severity"]),
            );
            if !origin.is_empty() {
                line.push_str(&format!("  ({origin})"));
            }
            out.push(line);
        }
    }
    out
}

/// `tako lsp format` の人向けの体裁（#1683）。中身の正本は dispatch の応答。
/// 1 行目は `status`、続けて注記（何か所整形したか・変える所が無い）か理由と次の一手
fn lsp_format_lines(result: &Value) -> Vec<String> {
    let text = |key: &str| result[key].as_str().unwrap_or_default().to_string();
    let mut lines = vec![format!("{} {}", text("status"), text("path"))
        .trim_end()
        .to_string()];
    for key in [
        "note",
        "dropped_note",
        "reason",
        "next_step",
        "install_command",
    ] {
        if let Some(line) = result[key].as_str() {
            lines.push(format!("  {line}"));
        }
    }
    lines
}

/// `tako lsp rename` の人向けの体裁（#1686）。中身の正本は dispatch の応答。
///
/// 1 行目は `status`（dry-run / renamed / not-applied / rejected …）と今の名前 → 新しい名前。
/// ファイルごとに `パス  N 箇所`（頼んだペインのファイルに `*`）と、その下に `行:桁  前 → 後`。
/// 要約・理由・次の一手は字下げして続ける
fn lsp_rename_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut out = vec![format!(
        "{} {} -> {}",
        text(&result["status"]),
        result["placeholder"].as_str().unwrap_or("?"),
        text(&result["new_name"])
    )];
    for file in result["files"].as_array().into_iter().flatten() {
        let mark = if file["source"] == Value::Bool(true) {
            "*"
        } else {
            " "
        };
        let path = file["path"]
            .as_str()
            .unwrap_or_else(|| file["uri"].as_str().unwrap_or(""));
        let mut line = format!("{mark} {path}  {}", file["edits"]);
        if let Some(panes) = file["open_in"].as_array() {
            let panes: Vec<String> = panes.iter().map(Value::to_string).collect();
            line.push_str(&format!("  (pane {})", panes.join(", ")));
        }
        out.push(line);
        for place in file["places"].as_array().into_iter().flatten() {
            out.push(format!(
                "    {}:{}  {} -> {}",
                place["line"],
                place["column"],
                text(&place["old"]),
                text(&place["new"])
            ));
        }
    }
    for key in ["note", "reason", "next_step", "install_command"] {
        if let Some(line) = result[key].as_str() {
            out.push(format!("  {line}"));
        }
    }
    out
}

/// `tako lsp definition` 等の人向けの体裁（#1680）。中身の正本は dispatch の応答。
///
/// 1 行目は `status`（found / choose / not-found / timeout …）。飛び先は `パス:行:桁  抜粋`
/// （桁は 0 始まりの UTF-8 バイト）を番号つきで並べ、着地したら `landing` とペインを添える。
/// 理由と次の一手があれば字下げして続ける
fn lsp_goto_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut out = vec![text(&result["status"])];
    let locations = result["locations"].as_array().cloned().unwrap_or_default();
    let chosen = result["chosen"].as_u64();
    for (index, location) in locations.iter().enumerate() {
        let number = index as u64 + 1;
        let mark = if chosen == Some(number) { "*" } else { " " };
        out.push(format!(
            "{mark}{number:>3}  {}:{}:{}  {}",
            text(&location["path"]),
            location["line"],
            location["column"],
            text(&location["text"])
        ));
    }
    if let Some(landing) = result["landing"].as_str() {
        out.push(format!("  {landing} (pane {})", result["open"]["pane"]));
    }
    for key in ["reason", "next_step", "install_command"] {
        if let Some(line) = result[key].as_str() {
            out.push(format!("  {line}"));
        }
    }
    out
}

/// `tako lsp completion` の人向けの体裁（#1682）。中身の正本は dispatch の応答。
///
/// 1 行目は `status`（found / applied / none / not-installed …）と件数。候補は
/// `番号  label  種類  detail` を 1 行ずつ。確定したら入れた本文。理由と次の一手は字下げして続ける
fn lsp_completion_lines(result: &Value) -> Vec<String> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let mut head = text(&result["status"]);
    if let Some(total) = result["total"].as_u64() {
        head.push_str(&format!(" total={total}"));
        if let Some(shown) = result["shown"].as_u64() {
            head.push_str(&format!(
                " shown={shown} truncated={}",
                result["truncated"].as_u64().unwrap_or(0)
            ));
        }
    }
    let mut out = vec![head];
    for item in result["items"].as_array().into_iter().flatten() {
        let mut line = format!(
            "{:>4}  {}  {}",
            item["number"].as_u64().unwrap_or(0),
            text(&item["label"]),
            item["kind"].as_str().unwrap_or("-")
        );
        if let Some(detail) = item["detail"].as_str() {
            line.push_str(&format!("  {}", detail.lines().next().unwrap_or("")));
        }
        out.push(line);
    }
    if let Some(inserted) = result["edit"]["inserted"].as_str() {
        out.push(format!(
            "  {} (pane {}) +{} edits",
            inserted, result["from"]["pane"], result["edit"]["additional_edits"]
        ));
    }
    for key in ["reason", "next_step", "install_command"] {
        if let Some(line) = result[key].as_str() {
            out.push(format!("  {line}"));
        }
    }
    out
}

/// `tako lsp hover` の人向けの体裁（#1681）。中身の正本は dispatch の応答。
///
/// 1 行目は `status`（found / none / not-installed …）と種類・範囲。見つかれば本文を
/// **そのまま**（Markdown / 平文を解釈しない）続ける。理由と次の一手・注記は字下げして続ける
fn lsp_hover_lines(result: &Value) -> Vec<String> {
    let mut head = result["status"].as_str().unwrap_or("").to_string();
    if let Some(kind) = result["kind"].as_str() {
        head.push_str(&format!(" kind={kind}"));
    }
    let range = &result["range"];
    if range.is_object() {
        head.push_str(&format!(
            " range={}:{}-{}:{}",
            range["start"]["line"],
            range["start"]["column"],
            range["end"]["line"],
            range["end"]["column"]
        ));
    }
    if let Some(shown) = result["shown"].as_bool() {
        head.push_str(&format!(" shown={shown}"));
    }
    let mut out = vec![head];
    if let Some(contents) = result["contents"].as_str() {
        out.extend(contents.lines().map(str::to_string));
    }
    for key in ["note", "reason", "next_step", "install_command"] {
        if let Some(line) = result[key].as_str() {
            out.push(format!("  {line}"));
        }
    }
    out
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
            print_delivery(result);
        }
        // #1259: 送達は queued が即返るだけなので、顛末の入口を必ず 1 行出す
        // （MCP の応答に delivery を足したので CLI にも同じものを出す = 1:1）
        Command::Send(_) => print_delivery(result),
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
            | WindowCommand::Restore { .. }
            | WindowCommand::Move { .. }
            | WindowCommand::Resize { .. },
        ) => println!("{result}"),
        Command::Open(_) | Command::Preview(_) | Command::PreviewOutline(_) | Command::Edit(_) => {
            println!("{result}")
        }
        Command::PreviewLinkList(_) => println!("{}", pretty_json(result)),
        // #1283: 検出結果はスパンまで読みたいので整形して出す
        Command::Links(_) => println!("{}", pretty_json(result)),
        Command::Search(args) => print_search(args, result),
        Command::Lsp(sub) => print_lsp(sub, result),
        Command::Jump(sub) => print_jump(sub, result),
        Command::PreviewFollowLink(_) => println!("{result}"),
        // #680: コピーしたコード全文は改行込みで読みたいので整形して出す
        Command::PreviewCopyCode(_) => println!("{}", pretty_json(result)),
        Command::PreviewReload(_)
        | Command::PreviewCache(_)
        | Command::PreviewChangelog(_)
        | Command::Scrollback(_) => {
            println!("{result}")
        }
        // #1979: 再描画の上限の部分だけを出す（スクロールバックは `tako scrollback`）
        Command::RedrawLimit(_) => println!("{}", pretty_json(&result["unfocused_redraw"])),
        // #1961: クリックの転送の部分だけを出す
        Command::Mouse(_) => println!("{}", pretty_json(&result["mouse_forward"])),
        Command::Click(_) => println!("{result}"),
        // #2046: check は判定の結果だけ、それ以外は状態の全体（どちらも伏せ字だけ）
        Command::PasteGuard(args) if args.action.as_deref() == Some("check") => {
            println!("{}", pretty_json(&result["check"]))
        }
        Command::PasteGuard(_) => println!("{}", pretty_json(result)),
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
        // #2112: `--network` は全体の設定と実行状態の節だけ（一覧の panes は出さない）
        Command::LimitResume(args) if args.network => {
            println!("{}", pretty_json(&result["network"]))
        }
        // #813: 状態（stop / resume_at / attempts）が入れ子なので整形して出す
        Command::LimitResume(_) => println!("{}", pretty_json(result)),
        Command::Mod(args) => print_mod(args, result),
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
        Command::SessionRestart(_) => println!("{}", pretty_json(result)),
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
        // #1069: 既定は「URL 1 行」（スマホへ渡すのが用途なのでコピーしやすい形）。
        // 繋がっていなければ理由を出す（URL を捏造しない）
        Command::Sessions(SessionsCommand::Link { json, .. }) => {
            if *json {
                println!("{}", pretty_json(result));
            } else {
                let link = &result["remote_link"];
                match link["url"].as_str() {
                    Some(url) => {
                        println!("{url}");
                        if let Some(account) = link["account_label"].as_str() {
                            eprintln!("アカウント: {account}（このアカウントでログインした端末からだけ開けます）");
                        }
                    }
                    None => {
                        eprintln!(
                            "公式リンクはありません（state: {}）",
                            link["state"].as_str().unwrap_or("unknown")
                        );
                        eprintln!(
                            "  有効にする場合: tako orchestrator profiles set <名前> --remote-control true → 起動し直す"
                        );
                    }
                }
            }
        }
        Command::Sessions(SessionsCommand::Resume { .. }) => {
            if let (Some(pane), Some(sid)) =
                (result["pane"].as_u64(), result["session_id"].as_str())
            {
                // session_id は型の上で ASCII の保証が無いので文字単位で先頭を取る（#1746）
                let head: String = sid.chars().take(8).collect();
                eprintln!(
                    "復元しました: ペイン {pane}（session {head}…, cwd {}）",
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
        Command::Todo(TodoCommand::List { json, .. }) => {
            if *json {
                println!("{}", pretty_json(result));
            } else {
                print_todo_list(result);
            }
        }
        Command::Todo(
            TodoCommand::Add { json, .. }
            | TodoCommand::Show { json, .. }
            | TodoCommand::Update { json, .. }
            | TodoCommand::Done { json, .. }
            | TodoCommand::Dismiss { json, .. }
            | TodoCommand::Respond { json, .. },
        ) => {
            if *json {
                println!("{}", pretty_json(result));
            } else {
                print_todo_detail(result);
            }
        }
        // #1479: 画面の状態だけを返す（タスクの中身は出さない）
        Command::Todo(TodoCommand::Expand { json, .. } | TodoCommand::Collapse { json, .. }) => {
            if *json {
                println!("{}", pretty_json(result));
            } else {
                print_todo_expanded(result);
            }
        }
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
        Command::RunDefault(_) | Command::RunConfig(_) => {
            println!("{}", pretty_json(result));
        }
        // remote は run() → print_result を通らない
        _ => {}
    }
}

/// テストのビルドの呼び出し元ペイン（#1716）。
///
/// 製品の `caller_pane` は `TAKO_PANE_ID` を読むので、`--pane` を省略したテストは tako の中
/// （シェルが env を継承している）で走らせたときだけ通り、CI（env 無し）で落ちていた
/// （PR #1706）。テストのビルドでは env を読まず、既定は「呼び出し元なし」（CI と同じ）にする。
/// 呼び出し元ありの腕を見たいテストは [`with_caller`](test_caller::with_caller) で名指しする。
/// 迂回する経路（env の直読み・`ipc::caller_pane` の名指し）は
/// `tests/issue1716_caller_pane_env_watchdog.rs` が `file:line` で落とす
#[cfg(test)]
mod test_caller {
    use std::cell::Cell;

    thread_local! {
        static CALLER: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// 名指しした呼び出し元（既定は無し）。env は読まない
    pub fn caller_pane() -> Option<u64> {
        CALLER.with(Cell::get)
    }

    /// `f` の間だけ呼び出し元を `pane` にする（抜けるとき・落ちたときに元へ戻す）。
    /// 値はスレッドごと（libtest はテストを 1 本ずつ別のスレッドで走らせる）なので、
    /// 並走する他のテストへは漏れない
    pub fn with_caller<R>(pane: Option<u64>, f: impl FnOnce() -> R) -> R {
        struct Restore(Option<u64>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CALLER.with(|c| c.set(self.0));
            }
        }
        let _restore = Restore(CALLER.with(|c| c.replace(pane)));
        f()
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

    /// #2002: 届かないとき、GUI が生きているのに受け口が応答しないなら、そう言って次の一手を出す
    #[test]
    fn issue2002_生きたguiの受け口が応答しないと区別して次の一手を出す() {
        let ipc = serde_json::json!({
            "gui": {
                "pid": 42,
                "alive": true,
                "connect": "refused",
                "state": "alive_unreachable",
                "next": "tako は受け口を 10 秒ごとに確かめて張り直す。少し待って tako check-health をもう一度。戻らなければ tako を再起動する（#2002）",
            }
        });
        let lines = ipc_recovery_lines(&ipc, tako_core::i18n::Lang::Ja);
        assert_eq!(
            lines,
            [
                "  GUI: 動いている（pid 42）のに受け口が応答しない（ソケットファイルはあるが誰も受けていない）",
                "  次の一手: tako は受け口を 10 秒ごとに確かめて張り直す。少し待って tako check-health をもう一度。戻らなければ tako を再起動する（#2002）",
            ]
        );
        // GUI が居ないなら起動を案内する（区別できている）
        let down = serde_json::json!({
            "gui": { "pid": 42, "alive": false, "connect": "refused", "state": "not_running", "next": "tako を起動する" }
        });
        assert_eq!(
            ipc_recovery_lines(&down, tako_core::i18n::Lang::Ja),
            [
                "  GUI: 動いていない（接続情報の pid 42 は居ない）",
                "  次の一手: tako を起動する",
            ]
        );
        // A/B（`gui` 節が無い = 修正前）では何も足さない
        assert!(ipc_recovery_lines(
            &serde_json::json!({ "gui": null }),
            tako_core::i18n::Lang::Ja
        )
        .is_empty());
    }

    /// #2359: fd の 1 行は使用数 / 上限と起動時の引き上げを出す。上限の無い OS・旧版は出さない
    #[test]
    fn check_healthのfdの行は使用数と起動時の引き上げを出す() {
        let section = |outcome: &str, open: u64, soft: u64, hard: Value| {
            serde_json::json!({
                "applicable": true,
                "open": open,
                "soft": soft,
                "hard": hard,
                "percent": open * 100 / soft,
                "startup": { "outcome": outcome, "soft_at_launch": 256, "error": "setrlimit(10240): Invalid argument (os error 22)" },
            })
        };
        assert_eq!(
            fd_limit_line(
                &section("raised", 121, 10240, Value::Null),
                tako_core::i18n::Lang::Ja
            )
            .as_deref(),
            Some(
                "fd: 121 / 10240 本（1%）・起動時 256 → 10240 に引き上げ（ペインのシェルには 256）"
            )
        );
        assert_eq!(
            fd_limit_line(
                &section("hard_capped", 170, 200, serde_json::json!(200)),
                tako_core::i18n::Lang::Ja
            )
            .as_deref(),
            Some("fd: 170 / 200 本（85%）・hard が 200 で引き上げられない（起動時のまま）")
        );
        assert_eq!(
            fd_limit_line(&section("failed", 121, 256, Value::Null), tako_core::i18n::Lang::Ja).as_deref(),
            Some("fd: 121 / 256 本（47%）・引き上げに失敗（setrlimit(10240): Invalid argument (os error 22)）")
        );
        assert_eq!(
            fd_limit_line(
                &section("legacy", 121, 256, Value::Null),
                tako_core::i18n::Lang::Ja
            )
            .as_deref(),
            Some("fd: 121 / 256 本（47%）・引き上げない（TAKO_2359_LEGACY）")
        );
        // GUI 以外のホスト（起動時の記録が無い）は使用数だけ
        let mut bare = section("raised", 11, 10240, Value::Null);
        bare["startup"] = Value::Null;
        assert_eq!(
            fd_limit_line(&bare, tako_core::i18n::Lang::Ja).as_deref(),
            Some("fd: 11 / 10240 本（0%）")
        );
        assert_eq!(
            fd_limit_line(
                &serde_json::json!({ "applicable": false, "reason": "x" }),
                tako_core::i18n::Lang::Ja
            ),
            None,
            "Windows は出さない"
        );
        assert_eq!(
            fd_limit_line(&Value::Null, tako_core::i18n::Lang::Ja),
            None,
            "節の無い旧版の応答"
        );
    }

    /// #2367: 器の tmux サーバーの 1 行は使用数 / 上限と決まり方を出す。器が tmux でない・旧版は出さない
    #[test]
    fn check_healthの器のtmuxサーバーの行は使用数と決まり方を出す() {
        let section = |limit: &str, open: u64, soft: u64| {
            serde_json::json!({
                "applicable": true,
                "container": {
                    "state": "running",
                    "open": open,
                    "soft": soft,
                    "percent": open * 100 / soft,
                    "limit": limit,
                    "children_soft": 256,
                    "pane_capacity": (soft - 6) / 3,
                },
            })
        };
        assert_eq!(
            fd_container_line(&section("raised", 90, 10240), tako_core::i18n::Lang::Ja).as_deref(),
            Some("器の tmux サーバー: 90 / 10240 本（0%）・引き上げた値を受け継いだ（器のペインのシェルには 256）")
        );
        assert_eq!(
            fd_container_line(&section("predates", 210, 256), tako_core::i18n::Lang::Ja).as_deref(),
            Some("器の tmux サーバー: 210 / 256 本（82%）・引き上げより前に起きたので起動時のまま（約 83 ペインで詰まる。次にサーバーが起きたとき上がる）")
        );
        assert_eq!(
            fd_container_line(&section("legacy", 90, 256), tako_core::i18n::Lang::Ja).as_deref(),
            Some("器の tmux サーバー: 90 / 256 本（35%）・引き上げない（TAKO_2367_LEGACY）")
        );
        assert_eq!(
            fd_container_line(&section("launch_value", 90, 256), tako_core::i18n::Lang::Ja)
                .as_deref(),
            Some("器の tmux サーバー: 90 / 256 本（35%）・tako が引き上げていないので起動時のまま")
        );
        assert_eq!(
            fd_container_line(
                &serde_json::json!({ "container": { "state": "not_running" } }),
                tako_core::i18n::Lang::Ja
            )
            .as_deref(),
            Some("器の tmux サーバー: 起きていない")
        );
        assert_eq!(
            fd_container_line(&serde_json::json!({ "container": {
                "state": "unreachable", "reason": "server exited unexpectedly", "limit": "predates", "pane_capacity": 83,
            } }), tako_core::i18n::Lang::Ja)
            .as_deref(),
            Some("器の tmux サーバー: 問い合わせが届かない（server exited unexpectedly）・引き上げより前に起きたので起動時のまま（約 83 ペインで詰まる。次にサーバーが起きたとき上がる）")
        );
        assert_eq!(
            fd_container_line(
                &serde_json::json!({ "applicable": true, "container": null }),
                tako_core::i18n::Lang::Ja
            ),
            None,
            "器が tmux でない"
        );
        assert_eq!(
            fd_container_line(
                &serde_json::json!({ "applicable": true }),
                tako_core::i18n::Lang::Ja
            ),
            None,
            "節の無い旧版の応答"
        );
    }

    /// #2002: 届いたときは GUI 側の見張りと最後の張り直しを 1 行で出す
    #[test]
    fn issue2002_見張りの記録を1行で出す() {
        let ipc = serde_json::json!({
            "watch": {
                "interval_ms": 10_000,
                "checks": 7,
                "rebinds": 1,
                "last_rebind": { "secs_ago": 3, "reason_text": "ソケットファイルが消えていた" },
                "error": null,
            }
        });
        assert_eq!(
            ipc_recovery_lines(&ipc, tako_core::i18n::Lang::Ja),
            ["  見張り: 10 秒ごと・確認 7 回・張り直し 1 回（最後: 3 秒前・ソケットファイルが消えていた）"]
        );
    }

    /// #2360: 起動時に固定パスで見た先客を 1 行で出す（奪わなかった・残骸を作り直した）
    #[test]
    fn issue2360_起動時の固定パスの先客を1行で出す() {
        let held = serde_json::json!({
            "fixed_path": { "state": "busy", "held": true, "pid": 42, "reason_text": "繋がらないが、接続情報の持ち主（pid 42）が生きている" }
        });
        assert_eq!(
            ipc_recovery_lines(&held, tako_core::i18n::Lang::Ja),
            ["  固定パス: 生きた受け口が居たので奪わずに一時パスで立てた（繋がらないが、接続情報の持ち主（pid 42）が生きている）。同じデータディレクトリの別の tako が動いている = このウィンドウはセカンダリ（#2360）"]
        );
        let dead = serde_json::json!({
            "fixed_path": { "state": "dead", "held": false, "pid": null, "reason_text": "4 回繋いでも繋がらず、生きた持ち主の記録も無い（残骸）" }
        });
        assert_eq!(
            ipc_recovery_lines(&dead, tako_core::i18n::Lang::Ja),
            ["  固定パス: 起動時に残骸を消して張り直した（4 回繋いでも繋がらず、生きた持ち主の記録も無い（残骸））（#2360）"]
        );
        // 節が無い（確かめていない・旧版の応答）なら何も足さない
        assert!(ipc_recovery_lines(
            &serde_json::json!({ "fixed_path": null }),
            tako_core::i18n::Lang::Ja
        )
        .is_empty());
    }

    /// #2361: Windows の名前付きパイプの待ち受けを 1 行で出す（作れていない・正常・途切れの後に戻った）
    #[test]
    fn issue2361_パイプの待ち受けを1行で出す() {
        let down = serde_json::json!({
            "pipe": { "listening": false, "streak": 3, "error": "Insufficient system resources exist (os error 1450)", "retry_ms": 400, "retry_max_ms": 5000 }
        });
        assert_eq!(
            ipc_recovery_lines(&down, tako_core::i18n::Lang::Ja),
            ["  パイプの待ち受け: 作れていない（3 回続けて失敗・Insufficient system resources exist (os error 1450)）。400 ms 後に作り直す（最長 5 秒ごと）。この間は tako CLI / MCP の stdio が届かない（#2361）"]
        );
        let quiet = serde_json::json!({
            "pipe": { "listening": true, "outages": 0, "failures": 0, "retry_max_ms": 5000, "last_recovery": null }
        });
        assert_eq!(
            ipc_recovery_lines(&quiet, tako_core::i18n::Lang::Ja),
            ["  パイプの待ち受け: 正常（作れなければ最長 5 秒ごとに作り直す）"]
        );
        let recovered = serde_json::json!({
            "pipe": {
                "listening": true, "outages": 2, "failures": 7, "retry_max_ms": 5000,
                "last_recovery": { "secs_ago": 12, "failures": 5, "error": "os error 1450" }
            }
        });
        assert_eq!(
            ipc_recovery_lines(&recovered, tako_core::i18n::Lang::Ja),
            ["  パイプの待ち受け: 正常・これまでに途切れ 2 回（作れなかった 7 回）（最後: 12 秒前に 5 回続けて作れなかった後に戻った・os error 1450）"]
        );
        // 節が無い（windows 以外・旧版の応答）なら何も足さない
        assert!(ipc_recovery_lines(
            &serde_json::json!({ "pipe": null }),
            tako_core::i18n::Lang::Ja
        )
        .is_empty());
    }

    /// #2361: アプリへ届かないときの次の一手は OS の立て直し方に合わせる（unix の文は #2002 のまま）
    #[test]
    fn issue2361_届かないときの次の一手はosの立て直し方に合わせる() {
        assert_eq!(
            alive_unreachable_next(false, tako_core::i18n::Lang::Ja),
            "tako は受け口を 10 秒ごとに確かめて張り直す。少し待って tako check-health をもう一度。戻らなければ tako を再起動する（#2002）"
        );
        assert_eq!(
            alive_unreachable_next(true, tako_core::i18n::Lang::Ja),
            "tako はパイプの待ち受けを作れなければ最長 5 秒ごとに作り直す。少し待って tako check-health をもう一度。戻らなければ tako を再起動する（#2361）"
        );
    }

    /// #1962: mod の帯のボタン（tako の操作 9 つと shell）が叩く引数を、この CLI が要求へ組める
    /// （引数は tako-core が組んで報告の応答で mod へ渡す = ここで受け付けなければボタンは押しても失敗する）
    #[test]
    fn issue1962_ボタンの引数はcliが受け付ける() {
        use tako_core::claude_mod_ui::{shell_cli_args, TakoOp, Vocab};
        let mut all: Vec<Vec<String>> = TakoOp::ALL
            .iter()
            .map(|op| op.cli_args(7, Some("/w")).expect("cwd があれば組める"))
            .collect();
        all.push(shell_cli_args(7, "-n echo 'a b'"));
        for args in all {
            let argv: Vec<&str> = std::iter::once("tako")
                .chain(args.iter().map(String::as_str))
                .collect();
            let command = Cli::try_parse_from(&argv)
                .unwrap_or_else(|e| panic!("{argv:?} を CLI が受け付けない: {e}"))
                .command;
            let request = build_request(&command)
                .unwrap_or_else(|e| panic!("{argv:?} を要求へ組めない: {e}"));
            let debug = format!("{request:?}");
            // ペインを名指しする操作は 7 番を指す（open-cwd はパスだけ）
            if args.iter().any(|a| a == "--pane") {
                assert!(debug.contains("pane: Some(7)"), "{argv:?} → {debug}");
            }
            if args[0] == "run-interactive" {
                assert!(debug.contains("-n echo 'a b'"), "{debug}");
                assert!(debug.contains("auto_close: Some(\"never\")"), "{debug}");
            }
        }
    }

    /// #1895: `tako file copy a b dst` はまとめた 1 要求（= 1 つのジョブ。MCP の `paths` と同じ要求）、
    /// 1 件なら単数の要求。どれも CLI の cwd 基準で絶対化する
    #[test]
    fn file_copyの複数指定はまとめた1要求になる() {
        let cwd = std::env::current_dir().unwrap();
        let abs = |p: &str| cwd.join(p).display().to_string();
        // 相対パスで渡す（`/w/b` は Windows ではドライブの無い相対扱いになり、OS で形が割れる）
        let command = parse(&["tako", "file", "copy", "a.txt", "sub/b.txt", "dst"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::FileOpMany {
                op: tako_control::protocol::FileOpKind::Copy,
                paths: vec![abs("a.txt"), abs("sub/b.txt")],
                dest: Some(abs("dst")),
            }
        );
        let command = parse(&["tako", "file", "copy", "a.txt", "dst"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::FileOp {
                op: tako_control::protocol::FileOpKind::Copy,
                path: abs("a.txt"),
                name: None,
                pane: None,
                dest: Some(abs("dst")),
            }
        );
        assert!(
            file_copy_requests(&["dst".into()]).is_err(),
            "コピー元が無い"
        );
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

    /// #1259: MCP の `delivery` と同じ材料を CLI も 1 行で出す（1:1）。
    /// 応答に `delivery` が無い（GUI が居ない host 等）ときは何も出さない
    #[test]
    fn issue1259_deliveryの1行が理由と経過を出す() {
        let status = tako_core::prompt_delivery::Status::waiting(
            tako_core::prompt_delivery::Stall::NoInputBox,
            42,
        );
        let result =
            serde_json::json!({ "queued": true, "pane": 1627, "delivery": status.to_json() });
        let line = delivery_line(&result).expect("delivery があれば 1 行出る");
        assert!(line.starts_with("[delivery] state=waiting"), "{line}");
        assert!(line.contains("reason=no_input_box"), "{line}");
        assert!(line.contains("elapsed=42s"), "{line}");
        assert!(
            delivery_line(&serde_json::json!({ "queued": true })).is_none(),
            "delivery が無ければ出さない"
        );
        assert!(
            delivery_line(&serde_json::json!({ "delivery": serde_json::Value::Null })).is_none(),
            "null なら出さない"
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
                paste_guard: None,
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
                line: None,
                column: None,
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

    /// #1676: `--line` / `--column` は 1 始まり。**負値は CLI にしか見えない**
    /// （dispatch の `usize` は負値を持てない）ので、境界はここで見る。
    ///
    /// `--pane` は必ず名指しする（省略すると呼び出し元が無いので落ちる。PR #1706 では
    /// `target_pane` が `TAKO_PANE_ID` を読み、tako の**中**で走らせたときだけ通って CI の
    /// macOS / Windows 両方で落ちた。#1716 からテストのビルドは env を読まない）。
    /// 隣の `openは絶対パスとモード別名を解釈する` も同じ形
    #[test]
    fn openの行と桁は1始まりで負値と0を弾く() {
        let abs = if cfg!(windows) {
            r"C:\tmp\a.rs"
        } else {
            "/tmp/a.rs"
        };
        let command = parse(&[
            "tako", "open", abs, "--pane", "5", "--line", "42", "--column", "7",
        ]);
        let Request::OpenFile { line, column, .. } = build_request(&command).unwrap() else {
            panic!("OpenFile になる");
        };
        assert_eq!((line, column), (Some(42), Some(7)));

        // 省略時は従来どおり（行指定なし）
        let command = parse(&["tako", "open", abs, "--pane", "5"]);
        let Request::OpenFile { line, column, .. } = build_request(&command).unwrap() else {
            panic!("OpenFile になる");
        };
        assert_eq!((line, column), (None, None));

        // 0 と負値はエラー（文言の正本は open_plan の `PlanRefusal`。表示言語は手元の設定で
        // 決まる = 日英どちらの形とも突き合わせる。#2365）
        use tako_core::open_plan::PlanRefusal;
        let says = |err: &str, refusal: PlanRefusal| {
            [tako_core::i18n::Lang::Ja, tako_core::i18n::Lang::En]
                .into_iter()
                .any(|lang| err.contains(&refusal.text_in(lang)))
        };
        for value in ["0", "-1"] {
            let command = parse(&["tako", "open", abs, "--pane", "5", "--line", value]);
            let err = build_request(&command).unwrap_err();
            assert!(
                says(&err, PlanRefusal::LineNotOneBased) && err.contains(value),
                "--line {value}: {err}"
            );
            let command = parse(&[
                "tako", "open", abs, "--pane", "5", "--line", "1", "--column", value,
            ]);
            let err = build_request(&command).unwrap_err();
            assert!(
                says(&err, PlanRefusal::ColumnNotOneBased) && err.contains(value),
                "--column {value}: {err}"
            );
        }

        // --column は --line とセット（clap が引数解析の時点で弾く）
        assert!(Cli::try_parse_from(["tako", "open", abs, "--column", "3"]).is_err());
    }

    /// #1977: `tako lsp install` / `status` の人向けの表示に、古い版の片付けと未導入の引き直しを
    /// 出す（中身があるときだけ。文の正本は `lsp::text`）
    #[test]
    fn lsp_の片付けと引き直しを人向けに出す() {
        use tako_control::lsp::text;
        let cleanup = serde_json::json!({
            "removed": [{"id": "pyright", "version": "pyright-1.1.400", "bytes": 19_457_431}],
            "kept": [{"id": "node", "version": "22.0.0", "reason": "in_use", "detail": null}],
            "freed_bytes": 19_457_431,
        });
        let lines = lsp_install_lines(&serde_json::json!({
            "enabled": true,
            "servers": [{"id": "pyright", "status": "installed", "path": "/d/x.js",
                         "disk_bytes": 1_000_000, "elapsed_ms": 1500, "cleanup": cleanup}],
        }));
        assert_eq!(
            lines[1..],
            [
                format!(
                    "  {}",
                    text::fill(
                        text::CLEANUP_REMOVED_NOTE,
                        &[("items", "pyright/pyright-1.1.400"), ("size", "19.5 MB")]
                    )
                ),
                format!(
                    "  {}",
                    text::fill(text::CLEANUP_KEPT_NOTE, &[("items", "node/22.0.0 in_use")])
                ),
            ]
        );
        // 何も消さず残さなければ行を足さない
        let quiet = serde_json::json!({"removed": [], "kept": [], "freed_bytes": 0});
        assert!(lsp_cleanup_lines(&quiet, "  ").is_empty());
        assert!(lsp_cleanup_lines(&Value::Null, "  ").is_empty());
        let tail = lsp_status_tail_lines(&serde_json::json!({
            "fetch": {"cleanup": quiet},
            "recheck": {"targets": ["clangd"], "interval_secs": 100, "last_cost_ms": 2000},
        }));
        assert_eq!(
            tail,
            [text::fill(
                text::RECHECK_NOTE,
                &[("targets", "clangd"), ("interval", "100"), ("cost", "2000")]
            )]
        );
        let none = lsp_status_tail_lines(&serde_json::json!({
            "fetch": {"cleanup": null},
            "recheck": {"targets": [], "interval_secs": 15, "last_cost_ms": null},
            "file_watch": {"active": false, "roots": [], "skipped": [], "batches": 0},
        }));
        assert!(none.is_empty(), "{none:?}");
        // #2034: 監視が動いていれば根を、監視しない根があれば理由を 1 行ずつ
        let watching = lsp_status_tail_lines(&serde_json::json!({
            "fetch": {"cleanup": null},
            "recheck": {"targets": []},
            "file_watch": {
                "active": true,
                "roots": [{"path": "/w/p", "recursive": true}, {"path": "/w/q", "recursive": false}],
                "skipped": [{"path": "/h", "reason": "broad"}],
                "batches": 4,
            },
        }));
        assert_eq!(
            watching,
            [
                text::fill(
                    text::WATCH_NOTE,
                    &[("roots", "/w/p, /w/q"), ("batches", "4")]
                ),
                text::fill(text::WATCH_SKIPPED_NOTE, &[("items", "/h broad")]),
            ]
        );
    }

    /// #1684: `tako lsp menu` は MCP の action=menu と同じ要求になり、人向けの表示は 1 項目 1 行で
    /// 押したときと同じコマンドを出す（範囲の整形は `--range 行:桁-行:桁`）
    #[test]
    fn lsp_menuを操作へ写し押したときのコマンドを出す() {
        let command = parse(&[
            "tako", "lsp", "menu", "--pane", "7", "--line", "3", "--column", "4",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::LspMenu {
                pane: Some(7),
                line: 3,
                column: 4,
            }
        );
        assert!(
            Cli::try_parse_from(["tako", "lsp", "menu", "--pane", "7", "--line", "3"]).is_err()
        );
        let lines = lsp_menu_lines(&serde_json::json!({
            "status": "ok", "server": "rust-analyzer",
            "symbol": {"text": "helper", "kind": "identifier", "column": 4, "end_column": 10},
            "items": [
                {"id": "lsp-definition", "action": "definition",
                 "args": {"action": "definition", "pane": 7, "line": 3, "column": 4}},
                {"id": "lsp-format", "action": "format", "args": {"action": "format", "pane": 7}},
                {"id": "lsp-format-selection", "action": "format",
                 "args": {"action": "format", "pane": 7, "line": 2, "column": 0, "end_line": 5, "end_column": 1}},
            ],
        }));
        assert_eq!(
            lines,
            vec![
                "ok rust-analyzer helper（identifier）",
                "  lsp-definition         tako lsp definition --pane 7 --line 3 --column 4",
                "  lsp-format             tako lsp format --pane 7",
                "  lsp-format-selection   tako lsp format --pane 7 --range 2:0-5:1",
            ]
        );
        // 項目が無ければ理由と次の一手
        let lines = lsp_menu_lines(&serde_json::json!({
            "status": "not-symbol", "symbol": null, "items": [],
            "reason": "識別子ではない", "next_step": "識別子を指す",
        }));
        assert_eq!(
            lines,
            vec!["not-symbol", "  識別子ではない", "  識別子を指す"]
        );
    }

    /// #1686: `tako lsp rename` は MCP の action=rename と同じ要求になる（`--yes` = `confirm`。省略は
    /// dry-run）。人向けの表示はファイルごとの箇所数と場所、要約・理由・次の一手
    #[test]
    fn lsp_renameを操作へ写しファイルと場所を出す() {
        let command = parse(&[
            "tako",
            "lsp",
            "rename",
            "--pane",
            "7",
            "--line",
            "3",
            "--column",
            "4",
            "--new-name",
            "item",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::LspRename {
                pane: Some(7),
                line: 3,
                column: 4,
                new_name: "item".into(),
                confirm: false,
            }
        );
        let command = parse(&[
            "tako",
            "lsp",
            "rename",
            "--pane",
            "7",
            "--line",
            "3",
            "--column",
            "4",
            "--new-name",
            "item",
            "--yes",
        ]);
        assert!(matches!(
            build_request(&command).unwrap(),
            Request::LspRename { confirm: true, .. }
        ));
        // 新しい名前は必須
        assert!(Cli::try_parse_from([
            "tako", "lsp", "rename", "--pane", "7", "--line", "3", "--column", "4"
        ])
        .is_err());
        let lines = lsp_rename_lines(&serde_json::json!({
            "status": "dry-run", "placeholder": "value", "new_name": "item",
            "files": [
                {"path": "/w/a.rs", "source": true, "edits": 2, "places": [
                    {"line": 1, "column": 4, "old": "value", "new": "item"},
                    {"line": 3, "column": 8, "old": "value", "new": "item"},
                ]},
                {"path": "/w/b.rs", "source": false, "edits": 1, "open_in": [9], "places": [
                    {"line": 5, "column": 0, "old": "value", "new": "item"},
                ]},
            ],
            "note": "2 ファイル・3 箇所", "reason": "別のファイル", "next_step": "files を見る",
        }));
        assert_eq!(
            lines,
            vec![
                "dry-run value -> item",
                "* /w/a.rs  2",
                "    1:4  value -> item",
                "    3:8  value -> item",
                "  /w/b.rs  1  (pane 9)",
                "    5:0  value -> item",
                "  2 ファイル・3 箇所",
                "  別のファイル",
                "  files を見る",
            ]
        );
        // メニューの項目の args（今の名前入り）は同じ操作の CLI へ写る
        assert_eq!(
            lsp_menu_command(&serde_json::json!({
                "action": "rename", "pane": 7, "line": 3, "column": 4, "new_name": "value"
            })),
            "tako lsp rename --pane 7 --line 3 --column 4 --new-name value"
        );
        assert_eq!(
            lsp_menu_command(&serde_json::json!({
                "action": "rename", "pane": 7, "line": 3, "column": 4, "new_name": "a b"
            })),
            "tako lsp rename --pane 7 --line 3 --column 4 --new-name 'a b'"
        );
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
            Request::PreviewSave {
                pane: Some(5),
                force: false
            }
        );
        // #1659: 外部変更の後の逃げ道（上書き / 読み直し / 差分）
        let command = parse(&["tako", "edit", "save", "--force", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewSave {
                pane: Some(5),
                force: true
            }
        );
        // #1833: MCP の `action=overwrite` と同じ綴りの別名
        let command = parse(&["tako", "edit", "save", "--overwrite", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewSave {
                pane: Some(5),
                force: true
            }
        );
        let command = parse(&["tako", "edit", "reload", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewRevert { pane: Some(5) }
        );
        let command = parse(&["tako", "edit", "diff", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewDiff { pane: Some(5) }
        );
        // #1658: 位置は `行:桁`（桁は省略可 = 0）
        let command = parse(&[
            "tako",
            "edit",
            "replace-range",
            "12:0",
            "12:34",
            "新しい行",
            "--expect-version",
            "7",
            "--pane",
            "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewEditRange {
                pane: Some(5),
                start_line: 12,
                start_col: 0,
                end_line: 12,
                end_col: 34,
                text: "新しい行".into(),
                expected_version: Some(7),
            }
        );
        let command = parse(&["tako", "edit", "replace-range", "3", "4", "", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewEditRange {
                pane: Some(5),
                start_line: 3,
                start_col: 0,
                end_line: 4,
                end_col: 0,
                text: String::new(),
                expected_version: None,
            }
        );
        let command = parse(&[
            "tako",
            "edit",
            "cursor",
            "3:5",
            "--select-to",
            "8:2",
            "--pane",
            "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewCursor {
                pane: Some(5),
                line: 3,
                col: 5,
                select_to_line: Some(8),
                select_to_col: Some(2),
                expected_version: None,
            }
        );
        // 数値でない位置は実行前に弾く（サーバーまで持って行かない）
        let command = parse(&["tako", "edit", "cursor", "abc", "--pane", "5"]);
        assert!(build_request(&command).is_err());
        // #1652: 単位で動かす / 消す
        let command = parse(&[
            "tako",
            "edit",
            "move",
            "word-left",
            "--select",
            "--pane",
            "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewMove {
                pane: Some(5),
                movement: "word-left".into(),
                select: true,
                expected_version: None,
            }
        );
        let command = parse(&[
            "tako",
            "edit",
            "delete",
            "word-backward",
            "--expect-version",
            "7",
            "--pane",
            "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewDelete {
                pane: Some(5),
                motion: "word-backward".into(),
                expected_version: Some(7),
            }
        );
        // #1654: Tab / Shift+Tab / Enter と同じ編集
        for (sub, name) in [
            ("indent", "indent"),
            ("outdent", "outdent"),
            ("newline", "newline"),
        ] {
            let command = parse(&["tako", "edit", sub, "--pane", "5"]);
            assert_eq!(
                build_request(&command).unwrap(),
                Request::PreviewEditCommand {
                    pane: Some(5),
                    command: name.into(),
                    text: None,
                    expected_version: None,
                }
            );
            // 綴りは tako-core の名前表にある
            assert!(
                tako_core::platform::editor_keys::EditorCommand::from_edit_name(name).is_some()
            );
        }
        // #1692: `tako edit line <操作>` は全操作が MCP の command（`line-<操作>`）へ写り、
        // その綴りは dispatch が解く名前表にある
        for op in tako_core::text_edit::LineOperation::ALL {
            let command = parse(&[
                "tako",
                "edit",
                "line",
                op.name(),
                "--expect-version",
                "3",
                "--pane",
                "5",
            ]);
            let request = build_request(&command).unwrap();
            let Request::PreviewEditCommand { command, .. } = &request else {
                panic!("{op:?} が PreviewEditCommand にならない: {request:?}");
            };
            assert_eq!(
                tako_core::platform::editor_keys::EditorCommand::from_edit_name(command),
                Some(tako_core::platform::editor_keys::EditorCommand::Line(op))
            );
            assert_eq!(
                request,
                Request::PreviewEditCommand {
                    pane: Some(5),
                    command: format!("line-{}", op.name()),
                    text: None,
                    expected_version: Some(3),
                }
            );
        }
        // #1759: `tako edit type <文字>` は MCP の command=type + text と同じ Request
        assert_eq!(
            build_request(&parse(&[
                "tako",
                "edit",
                "type",
                "f(x)",
                "--expect-version",
                "4",
                "--pane",
                "5"
            ]))
            .unwrap(),
            Request::PreviewEditCommand {
                pane: Some(5),
                command: "type".into(),
                text: Some("f(x)".into()),
                expected_version: Some(4),
            }
        );
        assert_eq!(
            tako_core::platform::editor_keys::TYPE_COMMAND,
            "type",
            "CLI の綴りと MCP の enum は tako-core の名前表 1 か所"
        );
        assert_eq!(
            build_request(&parse(&["tako", "edit", "auto-close"])).unwrap(),
            Request::PreviewAutoClose { enabled: None }
        );
        assert_eq!(
            build_request(&parse(&["tako", "edit", "auto-close", "off"])).unwrap(),
            Request::PreviewAutoClose {
                enabled: Some(false)
            }
        );
        assert!(Cli::try_parse_from(["tako", "edit", "auto-close", "no"]).is_err());
        let command = parse(&["tako", "edit", "line", "move_up", "--pane", "5"]);
        assert!(build_request(&command).is_err());
        // 知らない単位は実行前に弾く
        let command = parse(&["tako", "edit", "move", "word_left", "--pane", "5"]);
        assert!(build_request(&command).is_err());
        let command = parse(&["tako", "edit", "delete", "word", "--pane", "5"]);
        assert!(build_request(&command).is_err());
        // #1678: `tako lsp <操作>` は action の綴りを dispatch の正本と共有する
        for (argv, action, name) in [
            (vec!["tako", "lsp", "status"], "status", None),
            (vec!["tako", "lsp", "servers"], "list", None),
            (
                vec!["tako", "lsp", "restart", "--name", "x"],
                "restart",
                Some("x"),
            ),
            (vec!["tako", "lsp", "stop"], "stop", None),
            (vec!["tako", "lsp", "logs"], "logs", None),
            // #1944: 取得も同じ 1 ツール（MCP `tako_lsp_server` の action=install）
            (vec!["tako", "lsp", "install"], "install", None),
            (
                vec!["tako", "lsp", "install", "--name", "x"],
                "install",
                Some("x"),
            ),
            // #2035: 初期化オプションも同じ 1 ツール（MCP `tako_lsp_server` の action=options）
            (vec!["tako", "lsp", "options"], "options", None),
        ] {
            assert!(tako_control::dispatch::LSP_ACTIONS.contains(&action));
            assert_eq!(
                build_request(&parse(&argv)).unwrap(),
                Request::LspServer {
                    action: action.into(),
                    name: name.map(str::to_string),
                    set: None,
                }
            );
        }
        // #2035: `--set KEY=VALUE` は JSON として読める値はその値・読めなければ文字列・null は消す印
        let Request::LspServer { action, name, set } = build_request(&parse(&[
            "tako",
            "lsp",
            "options",
            "--name",
            "rust-analyzer",
            "--set",
            "cargo.targetDir=false",
            "--set",
            "check.command=clippy",
            "--set",
            "cargo.features=null",
        ]))
        .unwrap() else {
            panic!("LspServer にならない");
        };
        assert_eq!(
            (action.as_str(), name.as_deref()),
            ("options", Some("rust-analyzer"))
        );
        assert_eq!(
            Value::Object(set.unwrap()),
            serde_json::json!({
                "cargo.targetDir": false,
                "check.command": "clippy",
                "cargo.features": null,
            })
        );
        // 形の崩れた `--set` は送る前に断る
        for bad in ["cargo.targetDir", "bad key=1", "=1"] {
            assert!(
                build_request(&parse(&[
                    "tako", "lsp", "options", "--name", "x", "--set", bad
                ]))
                .is_err(),
                "{bad:?} を通した"
            );
        }
        // #1677: `tako jump <操作>` は action の綴りを dispatch の正本と共有する。
        // list は基準ペインもフォーカスも持たない（読むだけ）
        for (argv, action, pane, focus) in [
            (vec!["tako", "jump", "list"], "list", None, None),
            (
                vec!["tako", "jump", "back", "--pane", "5"],
                "back",
                Some(5),
                None,
            ),
            (
                vec!["tako", "jump", "forward", "--pane", "5", "--focus"],
                "forward",
                Some(5),
                Some(true),
            ),
        ] {
            assert!(tako_control::dispatch::JUMP_ACTIONS.contains(&action));
            assert_eq!(
                build_request(&parse(&argv)).unwrap(),
                Request::Jump {
                    action: action.into(),
                    pane,
                    focus,
                }
            );
        }
        // #1679: 診断は言語機能の要求（MCP `tako_lsp` と同じ要求になる）
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "diagnostics"])).unwrap(),
            Request::LspDiagnostics {
                pane: None,
                severity: None
            }
        );
        assert_eq!(
            build_request(&parse(&[
                "tako",
                "lsp",
                "diagnostics",
                "--pane",
                "7",
                "--severity",
                "warning"
            ]))
            .unwrap(),
            Request::LspDiagnostics {
                pane: Some(7),
                severity: Some("warning".into())
            }
        );
        // 綴り違いは clap が弾く（`0 件` に見せない）
        assert!(Cli::try_parse_from(["tako", "lsp", "diagnostics", "--severity", "warn"]).is_err());
        // #1682: 補完は言語機能の要求（MCP `tako_lsp` の action=completion と同じ要求になる）
        assert_eq!(
            build_request(&parse(&[
                "tako",
                "lsp",
                "completion",
                "--pane",
                "7",
                "--line",
                "3",
                "--column",
                "4",
                "--limit",
                "20"
            ]))
            .unwrap(),
            Request::LspCompletion {
                pane: Some(7),
                line: 3,
                column: 4,
                limit: Some(20),
                choice: None,
                resolve: None,
            }
        );
        assert_eq!(
            build_request(&parse(&[
                "tako",
                "lsp",
                "completion",
                "--pane",
                "7",
                "--line",
                "1",
                "--column",
                "0",
                "--choice",
                "2",
                "--resolve"
            ]))
            .unwrap(),
            Request::LspCompletion {
                pane: Some(7),
                line: 1,
                column: 0,
                limit: None,
                choice: Some(2),
                resolve: Some(true),
            }
        );
        assert!(Cli::try_parse_from(["tako", "lsp", "completion", "--pane", "7"]).is_err());
        let lines = lsp_completion_lines(&serde_json::json!({
            "status": "found", "total": 980, "shown": 2, "truncated": 978,
            "items": [
                {"number": 1, "label": "HashMap", "kind": "struct", "detail": "struct HashMap<K, V>\nmore"},
                {"number": 2, "label": "hash", "kind": null},
            ],
            "next_step": "続き",
        }));
        assert_eq!(
            lines,
            vec![
                "found total=980 shown=2 truncated=978",
                "   1  HashMap  struct  struct HashMap<K, V>",
                "   2  hash  -",
                "  続き",
            ]
        );
        // #1681: ホバーは言語機能の要求（MCP `tako_lsp` の action=hover と同じ要求になる）
        assert_eq!(
            build_request(&parse(&[
                "tako", "lsp", "hover", "--pane", "7", "--line", "3", "--column", "4"
            ]))
            .unwrap(),
            Request::LspHover {
                pane: Some(7),
                line: 3,
                column: 4,
                show: None,
                limit: None,
            }
        );
        assert_eq!(
            build_request(&parse(&[
                "tako", "lsp", "hover", "--pane", "7", "--line", "1", "--column", "0", "--show"
            ]))
            .unwrap(),
            Request::LspHover {
                pane: Some(7),
                line: 1,
                column: 0,
                show: Some(true),
                limit: None,
            }
        );
        assert!(Cli::try_parse_from(["tako", "lsp", "hover", "--pane", "7"]).is_err());
        // #1893: --full = limit 0（全文）/ --limit N = N 字。MCP の `limit` と同じ wire。同時は拒否
        for (flags, limit) in [
            (&["--full"][..], Some(0)),
            (&["--limit", "500"][..], Some(500)),
        ] {
            let mut argv = vec![
                "tako", "lsp", "hover", "--pane", "7", "--line", "3", "--column", "4",
            ];
            argv.extend_from_slice(flags);
            assert_eq!(
                build_request(&parse(&argv)).unwrap(),
                Request::LspHover {
                    pane: Some(7),
                    line: 3,
                    column: 4,
                    show: None,
                    limit,
                },
                "{flags:?}"
            );
        }
        assert!(Cli::try_parse_from([
            "tako", "lsp", "hover", "--pane", "7", "--line", "3", "--column", "4", "--full",
            "--limit", "9"
        ])
        .is_err());
        // #1685: 参照・階層は位置つきの要求（MCP `tako_lsp` の同名の action と同じ要求になる）
        for (sub, action) in [
            ("references", "references"),
            ("incoming-calls", "incoming-calls"),
            ("outgoing-calls", "outgoing-calls"),
            ("supertypes", "supertypes"),
            ("subtypes", "subtypes"),
        ] {
            assert_eq!(
                build_request(&parse(&[
                    "tako", "lsp", sub, "--pane", "7", "--line", "3", "--column", "4", "--show",
                    "--limit", "0"
                ]))
                .unwrap(),
                Request::LspXref {
                    action: action.into(),
                    pane: Some(7),
                    line: 3,
                    column: 4,
                    show: Some(true),
                    limit: Some(0),
                },
                "{sub}"
            );
            assert!(Cli::try_parse_from(["tako", "lsp", sub, "--pane", "7"]).is_err());
        }
        // 結果の読み書きは pane を呼び出し元で埋めない（省略 = 右パネルの結果）
        let results = |action: &str, pane: Option<u64>, item: Option<usize>| Request::LspResults {
            action: action.into(),
            pane,
            item,
            open: None,
            focus: None,
            limit: None,
        };
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "results"])).unwrap(),
            results("results", None, None)
        );
        assert_eq!(
            build_request(&parse(&[
                "tako", "lsp", "expand", "--item", "2", "--pane", "5"
            ]))
            .unwrap(),
            results("expand", Some(5), Some(2))
        );
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "collapse", "--item", "2"])).unwrap(),
            results("collapse", None, Some(2))
        );
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "clear-results"])).unwrap(),
            results("clear-results", None, None)
        );
        assert_eq!(
            build_request(&parse(&[
                "tako",
                "lsp",
                "open-result",
                "--item",
                "3",
                "--open",
                "down",
                "--focus"
            ]))
            .unwrap(),
            Request::LspResults {
                action: "open-result".into(),
                pane: None,
                item: Some(3),
                open: Some("down".into()),
                focus: Some(true),
                limit: None,
            }
        );
        assert!(Cli::try_parse_from(["tako", "lsp", "expand"]).is_err());
        // 人向けの表示: 一覧は `id  path:行:桁  抜粋`、ツリーは字下げ + 開閉の印（- / + / .）
        let lines = lsp_xref_lines(&serde_json::json!({
            "status": "found", "kind": "references", "server": "srv",
            "from": {"symbol": "helper"}, "total": 2, "shown": 2, "truncated": 0, "retained": 1,
            "items": [
                {"id": 1, "path": "/w/a.rs", "line": 1, "column": 7, "text": "pub fn helper() {}"},
                {"id": 2, "path": "/w/b.rs", "line": 9, "column": 4, "text": "helper();"},
            ],
        }));
        assert_eq!(
            lines,
            vec![
                "found kind=references server=srv symbol=helper total=2 shown=2 truncated=0 retained=1",
                "   1  /w/a.rs:1:7  pub fn helper() {}",
                "   2  /w/b.rs:9:4  helper();",
            ]
        );
        let lines = lsp_xref_lines(&serde_json::json!({
            "status": "found", "kind": "incoming-calls", "node_count": 3,
            "nodes": [
                {"id": 1, "depth": 0, "name": "helper", "symbol_kind": "function", "path": "/w/a.rs", "line": 1, "column": 7, "expanded": true, "children": 2},
                {"id": 2, "depth": 1, "name": "main", "symbol_kind": "function", "path": "/w/m.rs", "line": 4, "column": 4, "expanded": false, "children": null},
                {"id": 3, "depth": 1, "name": "leaf", "symbol_kind": "method", "path": "/w/l.rs", "line": 8, "column": 0, "expanded": false, "children": 0},
            ],
        }));
        assert_eq!(
            lines,
            vec![
                "found kind=incoming-calls node_count=3",
                "   1  - helper  function  /w/a.rs:1:7",
                "   2    + main  function  /w/m.rs:4:4",
                "   3    . leaf  method  /w/l.rs:8:0",
            ]
        );
        let lines = lsp_xref_lines(&serde_json::json!({
            "status": "opened", "item": 2, "landing": "new-pane",
            "location": {"path": "/w/b.rs", "line": 9, "column": 4, "text": "helper();"},
        }));
        assert_eq!(
            lines,
            vec!["opened landing=new-pane", "  /w/b.rs:9:4  helper();"]
        );
        // 人向けの表示: 見出し（status・種類・範囲）→ 本文をそのまま（Markdown を解釈しない）
        let lines = lsp_hover_lines(&serde_json::json!({
            "status": "found", "kind": "markdown",
            "contents": "```rust\nfn len(&self) -> usize\n```\n\n*doc*",
            "range": {"start": {"line": 3, "column": 4}, "end": {"line": 3, "column": 7}},
            "shown": true,
        }));
        assert_eq!(
            lines,
            vec![
                "found kind=markdown range=3:4-3:7 shown=true",
                "```rust",
                "fn len(&self) -> usize",
                "```",
                "",
                "*doc*",
            ]
        );
        let lines = lsp_hover_lines(&serde_json::json!({
            "status": "none", "reason": "無い", "next_step": "次",
        }));
        assert_eq!(lines, vec!["none", "  無い", "  次"]);
        // 人向けの表示: 見出し（パス・ペイン・重大度ごとの数）→ 1 件 1 行（位置は replace-range の形）
        let lines = lsp_diagnostics_lines(&serde_json::json!({
            "documents": [{
                "pane": 7, "path": "/w/src/main.rs",
                "counts": {"error": 1, "warning": 0, "info": 0, "hint": 1},
                "diagnostics": [
                    {"severity": "error", "message": "mismatched types\nexpected i32",
                     "range": {"start": {"line": 3, "column": 17}, "end": {"line": 3, "column": 20}},
                     "source": "rustc", "code": "E0308"},
                    {"severity": "hint", "message": "h",
                     "range": {"start": {"line": 1, "column": 0}, "end": {"line": 1, "column": 2}}},
                ],
            }],
        }));
        assert_eq!(
            lines,
            vec![
                "/w/src/main.rs (pane 7) error=1 warning=0 info=0 hint=1".to_string(),
                "  3:17-3:20  error    mismatched types  (rustc E0308)".to_string(),
                "  1:0-1:2  hint     h".to_string(),
            ]
        );
        // つながった文書が無ければ案内だけ
        let note = lsp_diagnostics_lines(&serde_json::json!({"documents": [], "note": "n"}));
        assert_eq!(note, vec!["n".to_string()]);
        // #1683: 整形は MCP `tako_lsp` の action=format と同じ要求（範囲は replace-range と同じ綴り）
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "format", "--pane", "7"])).unwrap(),
            Request::LspFormat {
                pane: Some(7),
                range: None
            }
        );
        assert_eq!(
            build_request(&parse(&[
                "tako", "lsp", "format", "--pane", "7", "--range", "3:4-9"
            ]))
            .unwrap(),
            Request::LspFormat {
                pane: Some(7),
                range: Some(tako_control::protocol::LineColRange {
                    start_line: 3,
                    start_col: 4,
                    end_line: 9,
                    end_col: 0
                })
            }
        );
        assert!(build_request(&parse(&[
            "tako", "lsp", "format", "--pane", "7", "--range", "3:4"
        ]))
        .is_err());
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "format-on-save"])).unwrap(),
            Request::LspFormatOnSave { enabled: None }
        );
        assert_eq!(
            build_request(&parse(&["tako", "lsp", "format-on-save", "on"])).unwrap(),
            Request::LspFormatOnSave {
                enabled: Some(true)
            }
        );
        assert!(Cli::try_parse_from(["tako", "lsp", "format-on-save", "yes"]).is_err());
        assert_eq!(
            lsp_format_lines(&serde_json::json!({
                "status": "formatted", "path": "/w/a.rs", "note": "n",
            })),
            vec!["formatted /w/a.rs".to_string(), "  n".to_string()]
        );
        assert_eq!(
            lsp_format_lines(&serde_json::json!({
                "status": "timeout", "reason": "r", "next_step": "s",
            })),
            vec!["timeout".to_string(), "  r".to_string(), "  s".to_string()]
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
                case_sensitive: None,
                whole_word: None,
                visible: None,
            }
        );
        // #1653: -i / -w を付けたときだけ条件を送る（付けなければ既定 = 区別する）
        let command = parse(&["tako", "edit", "search", "hello", "-i", "-w", "--pane", "5"]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewSearch {
                pane: Some(5),
                query: Some("hello".into()),
                direction: Some("next".into()),
                case_sensitive: Some(false),
                whole_word: Some(true),
                visible: None,
            }
        );
        // #1873: --open / --close は検索欄の開閉。開閉だけのときは移動しない（direction を送らない）
        for (flag, visible) in [("--open", true), ("--close", false)] {
            let command = parse(&["tako", "edit", "search", flag, "--pane", "5"]);
            assert_eq!(
                build_request(&command).unwrap(),
                Request::PreviewSearch {
                    pane: Some(5),
                    query: None,
                    direction: None,
                    case_sensitive: None,
                    whole_word: None,
                    visible: Some(visible),
                },
                "{flag}"
            );
        }
        let command = parse(&[
            "tako",
            "edit",
            "search",
            "hello",
            "--open",
            "--direction",
            "prev",
            "--pane",
            "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewSearch {
                pane: Some(5),
                query: Some("hello".into()),
                direction: Some("prev".into()),
                case_sensitive: None,
                whole_word: None,
                visible: Some(true),
            }
        );
        assert!(
            Cli::try_parse_from(["tako", "edit", "search", "--open", "--close"]).is_err(),
            "--open と --close は同時に渡せない"
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
                case_sensitive: None,
                whole_word: None,
            }
        );
        let command = parse(&[
            "tako",
            "edit",
            "replace",
            "old",
            "new",
            "--ignore-case",
            "--whole-word",
            "--pane",
            "5",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::PreviewReplace {
                pane: Some(5),
                query: "old".into(),
                replacement: "new".into(),
                all: Some(false),
                case_sensitive: Some(false),
                whole_word: Some(true),
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
                all: None,
                network: None
            }
        );
        let enable = parse(&["tako", "limit-resume", "on", "--pane", "12"]);
        assert_eq!(
            build_request(&enable).unwrap(),
            Request::LimitResume {
                pane: Some(12),
                enabled: Some(true),
                all: None,
                network: None
            }
        );
        let disable = parse(&["tako", "limit-resume", "off", "--pane", "12"]);
        assert_eq!(
            build_request(&disable).unwrap(),
            Request::LimitResume {
                pane: Some(12),
                enabled: Some(false),
                all: None,
                network: None
            }
        );
        // 一覧は呼び出し元ペインが分からなくても引ける（dispatch 側が pane を見ない）
        let all = parse(&["tako", "limit-resume", "--all"]);
        assert_eq!(
            build_request(&all).unwrap(),
            Request::LimitResume {
                pane: caller_pane(),
                enabled: None,
                all: Some(true),
                network: None
            }
        );
        // #1945: on / off + --all は一括（dispatch が all + enabled を一括として扱う）
        let bulk = parse(&["tako", "limit-resume", "on", "--all"]);
        assert_eq!(
            build_request(&bulk).unwrap(),
            Request::LimitResume {
                pane: caller_pane(),
                enabled: Some(true),
                all: Some(true),
                network: None
            }
        );
        // 一括とペイン指定は同時に言えない（どちらの意味か取り違えない）
        assert!(
            Cli::try_parse_from(["tako", "limit-resume", "off", "--all", "--pane", "3"]).is_err()
        );
        // #2112: --network は全体の設定。単独は一覧を引いて network の節を読む
        let net = parse(&["tako", "limit-resume", "--network"]);
        assert_eq!(
            build_request(&net).unwrap(),
            Request::LimitResume {
                pane: None,
                enabled: None,
                all: Some(true),
                network: None,
            }
        );
        let net_off = parse(&["tako", "limit-resume", "off", "--network"]);
        assert_eq!(
            build_request(&net_off).unwrap(),
            Request::LimitResume {
                pane: None,
                enabled: None,
                all: None,
                network: Some(false),
            }
        );
        // ペイン指定・一覧とは同時に言えない
        assert!(Cli::try_parse_from(["tako", "limit-resume", "--network", "--pane", "3"]).is_err());
        assert!(Cli::try_parse_from(["tako", "limit-resume", "--network", "--all"]).is_err());
        // on / off 以外は clap が弾く（誤った語で黙って状態取得にならない）
        assert!(Cli::try_parse_from(["tako", "limit-resume", "yes"]).is_err());
    }

    /// #1958: 呼び出し元の**有無の両方の腕**で同じ本体を回す。
    ///
    /// 期待値を `caller_pane()` で書いたテストは、呼び出し元が無い腕（CI と同じ）だけだと
    /// 「埋めない」逆戻りを見分けられない（期待値も逆戻りも `None`）。#1716 からテストのビルドは
    /// env を読まないので、子プロセスの `TAKO_PANE_ID` ではなく
    /// [`test_caller::with_caller`] で呼び出し元を名指しして回す
    fn issue1958_both_caller_arms(body: impl Fn()) {
        for caller in [Some(4242), None] {
            let arm = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                test_caller::with_caller(caller, &body)
            }));
            if let Err(panic) = arm {
                eprintln!("呼び出し元 {caller:?} の腕で落ちた");
                std::panic::resume_unwind(panic);
            }
        }
    }

    /// #1716: テストのビルドは呼び出し元ペインの env（`TAKO_PANE_ID`）を読まない。
    ///
    /// `--pane` を省略したテストは、tako の中（シェルが env を継承している）でだけ通り CI（env 無し）で
    /// 落ちていた（PR #1706）。env は起動時にしか変えられないので、tako の中のシェルと同じ env を
    /// 持たせた 1 本だけの子（`rerun_alone`。本番の GUI の接続情報は外す）で、呼び出し元なし = CI と
    /// 同じ結果になることを見る。呼び出し元ありの腕は `with_caller` で名指ししたときだけ
    #[test]
    fn issue1716_テストのビルドは呼び出し元ペインのenvを読まない() {
        if tako_core::test_child::rerun_alone(
            "tests::issue1716_テストのビルドは呼び出し元ペインのenvを読まない",
            &["TAKO_SOCKET", "TAKO_TOKEN"],
        ) {
            return;
        }
        // 子の中だけ（他のテストは居ない。#2400）: tako の中のシェルと同じ形の env
        std::env::set_var("TAKO_PANE_ID", "4242");
        std::env::set_var("TAKO_TAB_ID", "7");
        // tako:caller-env-ok 対照（子に env が本当に立っていること。立っていなければ下の None は空振り）
        let product = tako_cli::ipc::caller_pane();
        assert_eq!(product, Some(4242), "製品の読みは env を見る");
        assert_eq!(caller_pane(), None, "テストのビルドは env を読まない");

        let abs = if cfg!(windows) {
            r"C:\tmp\a.rs"
        } else {
            "/tmp/a.rs"
        };
        let omitted = parse(&["tako", "open", abs, "--line", "42"]);
        let err = build_request(&omitted).unwrap_err();
        // 断りの文は設定の表示言語に従う（#2358）。どちらの言語でも同じ断りであればよい
        let said = [tako_core::i18n::Lang::Ja, tako_core::i18n::Lang::En]
            .into_iter()
            .any(|lang| err == tako_control::NoTargetPaneAt::Cli.text_in(lang));
        assert!(said, "{err}");
        // 名指ししたときだけ呼び出し元が入る（抜けたら元の「無し」へ戻る）
        test_caller::with_caller(Some(5), || {
            let Request::OpenFile { pane, .. } = build_request(&omitted).unwrap() else {
                panic!("OpenFile になる");
            };
            assert_eq!(pane, Some(5));
        });
        assert_eq!(caller_pane(), None);
    }

    /// #1958: `tako show-command` の `--pane` 省略は呼び出し元（TAKO_PANE_ID）。修正前は
    /// `args.pane` をそのまま送り、最簡形 `tako show-command "…"` が「対象ペインが未指定」で
    /// 断られていた。`--card` 指定は dispatch がカードの所在ペインを引くので埋めない。
    /// 期待値は `caller_pane()` で書き、呼び出し元の有無の両方を [`issue1958_both_caller_arms`] で回す
    #[test]
    fn issue1958_show_commandのpane省略は呼び出し元() {
        issue1958_both_caller_arms(issue1958_show_command_arm);
    }

    fn issue1958_show_command_arm() {
        let pane_of = |args: &[&str]| match build_request(&parse(args)) {
            Ok(Request::ShowCommand { pane, .. }) => Ok(pane),
            Ok(other) => panic!("ShowCommand になるはず: {other:?}"),
            Err(e) => Err(e),
        };
        match caller_pane() {
            Some(caller) => assert_eq!(
                pane_of(&["tako", "show-command", "echo a"]),
                Ok(Some(caller))
            ),
            // tako の外（CI）: 送らずに何を渡せばよいかを言う
            None => {
                let err = pane_of(&["tako", "show-command", "echo a"]).unwrap_err();
                // 断りの文は設定の表示言語に従う（#2358）。どちらの言語でも同じ断りであればよい
                let said = [tako_core::i18n::Lang::Ja, tako_core::i18n::Lang::En]
                    .into_iter()
                    .any(|lang| err == tako_control::NoTargetPaneAt::Cli.text_in(lang));
                assert!(said, "{err}");
            }
        }
        assert_eq!(
            pane_of(&["tako", "show-command", "--pane", "3", "echo a"]),
            Ok(Some(3))
        );
        for flag in ["--list", "--copy", "--run", "--dismiss"] {
            assert_eq!(
                pane_of(&["tako", "show-command", flag]),
                Ok(caller_pane()),
                "{flag}"
            );
            // --card 指定は今のまま（呼び出し元を混ぜない）
            assert_eq!(
                pane_of(&["tako", "show-command", flag, "--card", "5"]),
                Ok(None),
                "{flag}"
            );
            assert_eq!(
                pane_of(&["tako", "show-command", flag, "--card", "5", "--pane", "3"]),
                Ok(Some(3)),
                "{flag}"
            );
        }
    }

    /// #1958: `tako sessions link` の `--pane` 省略は呼び出し元（ヘルプの説明どおり。resume と同じ）
    #[test]
    fn issue1958_sessions_linkのpane省略は呼び出し元() {
        issue1958_both_caller_arms(issue1958_sessions_link_arm);
    }

    fn issue1958_sessions_link_arm() {
        let pane_of = |args: &[&str]| match build_request(&parse(args)).unwrap() {
            Request::Sessions { pane, .. } => pane,
            other => panic!("Sessions になるはず: {other:?}"),
        };
        assert_eq!(pane_of(&["tako", "sessions", "link"]), caller_pane());
        assert_eq!(
            pane_of(&["tako", "sessions", "link", "--pane", "3"]),
            Some(3)
        );
        assert_eq!(pane_of(&["tako", "sessions", "link", "--id", "abc"]), None);
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

    /// #1283: `tako links` が最簡形（引数なし = 呼び出し元ペイン）で通り、
    /// `--text` を渡したときはペインの解決を要求しない
    #[test]
    fn linksは画面テキストとペインのどちらでも操作へ写す() {
        let by_text = parse(&[
            "tako",
            "links",
            "--text",
            "見て `/tmp/x.md`。",
            "--cols",
            "80",
        ]);
        assert_eq!(
            build_request(&by_text).unwrap(),
            Request::Links {
                pane: None,
                text: Some("見て `/tmp/x.md`。".into()),
                cols: Some(80),
                cwd: None,
            }
        );

        let by_pane = parse(&["tako", "links", "--pane", "7", "--cwd", "/tmp"]);
        assert_eq!(
            build_request(&by_pane).unwrap(),
            Request::Links {
                pane: Some(7),
                text: None,
                cols: None,
                cwd: Some("/tmp".into()),
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

    /// #818: 最簡形（引数なし = 現在値 / 数値 1 個 = 変更）が操作へ写る
    #[test]
    fn scrollbackは状態取得と上限変更を操作へ写す() {
        let status = parse(&["tako", "scrollback"]);
        assert_eq!(
            build_request(&status).unwrap(),
            Request::Scrollback {
                lines: None,
                unfocused_fps: None,
                mouse_forward: None
            }
        );
        let changed = parse(&["tako", "scrollback", "2000"]);
        assert_eq!(
            build_request(&changed).unwrap(),
            Request::Scrollback {
                lines: Some(2_000),
                unfocused_fps: None,
                mouse_forward: None
            }
        );
    }

    /// #1979: 最簡形（引数なし = 現在値 / 数値 1 個 = 変更）が MCP `tako_scrollback` の
    /// `unfocused_fps` と同じ操作へ写る（スクロールバックの値には触らない）
    #[test]
    fn redraw_limitは状態取得と上限変更を操作へ写す() {
        let status = parse(&["tako", "redraw-limit"]);
        assert_eq!(
            build_request(&status).unwrap(),
            Request::Scrollback {
                lines: None,
                unfocused_fps: None,
                mouse_forward: None
            }
        );
        let changed = parse(&["tako", "redraw-limit", "20"]);
        assert_eq!(
            build_request(&changed).unwrap(),
            Request::Scrollback {
                lines: None,
                unfocused_fps: Some(20),
                mouse_forward: None
            }
        );
    }

    /// #2046: 最簡形（引数なし = 現在状態 / on・off = 切替 / check = 判定 / send・cancel = 答え）が
    /// `Request::PasteGuard` の 1 つへ写り、check 以外に文字を渡したら送らずに断る
    #[test]
    fn paste_guardは状態と切替と判定と答えを1つの操作へ写す() {
        let pg = |enabled, check: Option<&str>, respond: Option<&str>| Request::PasteGuard {
            enabled,
            check: check.map(str::to_string),
            respond: respond.map(str::to_string),
            mode: None,
            allow: None,
            unallow: None,
            rules: None,
            pane: None,
        };
        let with_mode = |req: Request, m: &str| match req {
            Request::PasteGuard {
                enabled,
                check,
                respond,
                ..
            } => Request::PasteGuard {
                enabled,
                check,
                respond,
                mode: Some(m.to_string()),
                allow: None,
                unallow: None,
                rules: None,
                pane: None,
            },
            other => other,
        };
        for (args, want) in [
            (vec!["tako", "paste-guard"], pg(None, None, None)),
            (
                vec!["tako", "paste-guard", "off"],
                pg(Some(false), None, None),
            ),
            (
                vec!["tako", "paste-guard", "on"],
                with_mode(pg(Some(true), None, None), "confirm"),
            ),
            // #2050: 警告だけの段
            (
                vec!["tako", "paste-guard", "warn"],
                with_mode(pg(Some(true), None, None), "warn"),
            ),
            (
                vec!["tako", "paste-guard", "check", "普通の文章"],
                pg(None, Some("普通の文章"), None),
            ),
            (
                vec!["tako", "paste-guard", "send"],
                pg(None, None, Some("send")),
            ),
            (
                vec!["tako", "paste-guard", "cancel"],
                pg(None, None, Some("cancel")),
            ),
        ] {
            assert_eq!(build_request(&parse(&args)).unwrap(), want, "{args:?}");
        }
        assert!(build_request(&parse(&["tako", "paste-guard", "off", "x"])).is_err());
        assert!(Cli::try_parse_from(["tako", "paste-guard", "maybe"]).is_err());
    }

    /// #2050: `tako send` の応答の鍵らしき文字の警告は規則名と伏せ字の 1 行（無ければ出さない）
    #[test]
    fn sendの鍵らしき文字の警告は伏せ字の1行() {
        let warned = serde_json::json!({
            "queued": true,
            "paste_guard": {
                "verdict": "warn",
                "rules": ["github_token"],
                "findings": [{ "rule": "github_token", "line": 1, "masked": "ghp_…qrst" }],
            },
        });
        let line = paste_guard_line(&warned).unwrap();
        assert!(
            line.starts_with("[paste-guard] verdict=warn github_token ghp_…qrst"),
            "{line}"
        );
        assert!(paste_guard_line(&serde_json::json!({ "queued": true })).is_none());
        assert!(paste_guard_line(&Value::Null).is_none());
    }

    /// #2050: 許可リスト・規則・送信時の確認（--pane）も `Request::PasteGuard` の 1 つへ写り、
    /// 組み合わせの誤りは送らずに断る
    #[test]
    fn paste_guardの許可と規則と送信時の確認を写す() {
        let req = |args: &[&str]| build_request(&parse(args));
        match req(&["tako", "paste-guard", "allow", "AIzaEXAMPLE"]).unwrap() {
            Request::PasteGuard { allow, check, .. } => {
                assert_eq!(allow.as_deref(), Some("AIzaEXAMPLE"));
                assert!(check.is_none());
            }
            other => panic!("{other:?}"),
        }
        match req(&["tako", "paste-guard", "unallow", "0a1b2c3d4e5f"]).unwrap() {
            Request::PasteGuard { unallow, .. } => {
                assert_eq!(unallow.as_deref(), Some("0a1b2c3d4e5f"))
            }
            other => panic!("{other:?}"),
        }
        match req(&["tako", "paste-guard", "rule", "high_entropy", "off"]).unwrap() {
            Request::PasteGuard { rules, .. } => assert_eq!(
                rules,
                Some(std::collections::BTreeMap::from([(
                    "high_entropy".to_string(),
                    false
                )]))
            ),
            other => panic!("{other:?}"),
        }
        match req(&["tako", "paste-guard", "check", "x", "--pane", "7"]).unwrap() {
            Request::PasteGuard { check, pane, .. } => {
                assert_eq!(check.as_deref(), Some("x"));
                assert_eq!(pane, Some(7));
            }
            other => panic!("{other:?}"),
        }
        for bad in [
            &["tako", "paste-guard", "unallow"][..],
            &["tako", "paste-guard", "rule", "high_entropy"],
            &["tako", "paste-guard", "rule"],
            &["tako", "paste-guard", "on", "--pane", "3"],
            &["tako", "paste-guard", "warn", "x"],
        ] {
            assert!(req(bad).is_err(), "{bad:?}");
        }
        assert!(Cli::try_parse_from(["tako", "paste-guard", "rule", "jwt", "maybe"]).is_err());
    }

    /// #1961: 最簡形（引数なし = 現在状態 / on・off = 切替）が MCP `tako_scrollback` の
    /// `mouse_forward` と同じ操作へ写り、`tako click` は `PaneClick` へ写る（ペイン必須）
    #[test]
    fn mouseとclickは転送の切替とクリックを操作へ写す() {
        for (args, want) in [
            (vec!["tako", "mouse"], None),
            (vec!["tako", "mouse", "off"], Some(false)),
            (vec!["tako", "mouse", "on"], Some(true)),
        ] {
            assert_eq!(
                build_request(&parse(&args)).unwrap(),
                Request::Scrollback {
                    lines: None,
                    unfocused_fps: None,
                    mouse_forward: want
                },
                "{args:?}"
            );
        }
        assert_eq!(
            build_request(&parse(&[
                "tako", "click", "--pane", "3", "--row", "5", "--col", "10"
            ]))
            .unwrap(),
            Request::PaneClick {
                pane: Some(3),
                row: 5,
                col: 10
            }
        );
        assert!(
            Cli::try_parse_from(["tako", "click", "--row", "5", "--col", "10"]).is_err(),
            "誤ったペインを押さないよう --pane は必須"
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

    /// #1185: protocol にある `window`（特定 window のみ attach）が CLI から到達できる
    #[test]
    fn tmux_openはwindow指定を解釈する() {
        let command = parse(&[
            "tako", "tmux", "open", "mywork", "--socket", "usersock", "--window", "2", "--pane",
            "9",
        ]);
        assert_eq!(
            build_request(&command).unwrap(),
            Request::TmuxOpen {
                socket: Some("usersock".into()),
                session: "mywork".into(),
                window: Some(2),
                pane: Some(9),
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

    /// #1662: `--wait` が上限で打ち切ったときは「まだ実行中」を機械可読で出し、
    /// 続きの聞き方は最簡形（#322）で案内する
    #[test]
    fn waitの打ち切りはまだ実行中を返し続きの聞き方を案内する() {
        let budget = std::time::Duration::from_secs(600);
        let v = run_wait_timed_out(7, std::time::Duration::from_millis(600_400), budget);
        assert_eq!(v["pane"], 7);
        assert_eq!(v["status"], "running", "{v}");
        assert_eq!(v["timed_out"], true, "{v}");
        assert_eq!(v["waited_secs"], 600, "{v}");
        assert_eq!(v["limit_secs"], 600, "{v}");
        // 文は表示言語で組む（#2365）。表示言語は設定次第なので、日本語を明示して見る
        let msg = run_wait_timed_out_reason(7, budget).text_in(tako_core::i18n::Lang::Ja);
        assert!(msg.contains("まだ実行中"), "{msg}");
        assert!(msg.contains("`tako run-interactive-status 7`"), "{msg}");
        assert!(msg.contains("TAKO_RUN_WAIT_TIMEOUT_SECS"), "{msg}");
        assert!(
            msg.contains("止めていません"),
            "実行を止めたと誤読させない: {msg}"
        );
    }

    /// #1778: `--wait` の終了コードは結末から決まる。上限での打ち切りは 124
    /// （`timeout(1)` と同じ）で、コマンド自身の失敗（1）と見分けられる
    #[test]
    fn waitの終了コードは打ち切りを124でコマンドの失敗と分ける() {
        assert_eq!(WaitEnd::Exited(0).exit_code(), 0);
        assert_eq!(WaitEnd::Exited(0).message(), None);
        assert_eq!(WaitEnd::Exited(1).exit_code(), 1);
        // 非 0 は 1 に畳む = コマンドが自分で 124 を返しても打ち切りと混ざらない
        assert_eq!(WaitEnd::Exited(7).exit_code(), 1);
        assert_eq!(WaitEnd::Exited(124).exit_code(), 1);
        assert_eq!(WaitEnd::Exited(-1).exit_code(), 1);
        assert!(WaitEnd::Exited(7)
            .message()
            .is_some_and(|m| m.contains("exit code 7")));
        let budget = std::time::Duration::from_secs(3);
        let timed_out = WaitEnd::TimedOut(
            run_wait_timed_out_reason(7, budget).text_in(tako_core::i18n::Lang::Ja),
        );
        assert_eq!(timed_out.exit_code(), 124);
        assert_eq!(RUN_WAIT_TIMED_OUT_EXIT, 124);
        assert!(timed_out
            .message()
            .is_some_and(|m| m.contains("まだ実行中")));
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
    // （`list_contains_pane` の検査は 1 実装と一緒に `ipc.rs` へ移った = #2021）

    #[test]
    fn launch_locationはフォールバック時にタブ表記になる() {
        let inline = LaunchTarget {
            pane: 780,
            new_tab: false,
            tab_title: Some("tako".into()),
        };
        assert_eq!(
            launch_location(&inline, tako_core::i18n::Lang::Ja),
            "ペイン 780（インライン）"
        );
        assert_eq!(
            launch_location(&inline, tako_core::i18n::Lang::En),
            "pane 780 (inline)"
        );
        let fallback = LaunchTarget {
            pane: 900,
            new_tab: true,
            tab_title: Some("tako".into()),
        };
        assert_eq!(
            launch_location(&fallback, tako_core::i18n::Lang::Ja),
            "タブ 'tako'（ペイン 900）"
        );
        assert_eq!(
            launch_location(&fallback, tako_core::i18n::Lang::En),
            "tab 'tako' (pane 900)"
        );
        // #2120: 汎用 master はタブ名に触らないので、名前の無い表記になる
        let generic = LaunchTarget {
            pane: 901,
            new_tab: true,
            tab_title: None,
        };
        assert_eq!(
            launch_location(&generic, tako_core::i18n::Lang::Ja),
            "新しいタブ（ペイン 901）"
        );
        assert_eq!(
            launch_location(&generic, tako_core::i18n::Lang::En),
            "a new tab (pane 901)"
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
        // 包みと原因の文は表示言語（GUI の外の解決 = 実行環境）で変わるので、日本語の対を固定して見る
        // （#2365。`launch_failure_message` は同じ対を実行環境の言語で引くだけ）
        use tako_core::i18n::Lang;
        let outside = tako_control::Reason::OutsideTako.text_in(Lang::Ja);
        let in_lang = |lang: Lang, cause: &str, cmd_hint: &str, requested: Option<u64>| {
            tako_control::Reason::LaunchPaneUnavailable {
                cause: cause.to_string(),
                cmd_hint: cmd_hint.to_string(),
                stale_pane: requested,
            }
            .text_in(lang)
        };
        assert_eq!(
            launch_failure_message(&outside, "tako master", Some(305)),
            in_lang(
                tako_control::settings::client_lang(),
                &outside,
                "tako master",
                Some(305)
            )
        );
        let with_stale = in_lang(Lang::Ja, &outside, "tako master -fable", Some(305));
        assert!(with_stale.contains("tako アプリを起動"), "{with_stale}");
        assert!(
            with_stale.contains("unset TAKO_PANE_ID"),
            "古い ID を持つシェルには unset を案内する: {with_stale}"
        );
        assert!(with_stale.contains("tako master -fable"), "{with_stale}");
        assert!(with_stale.contains(&outside), "原因も残す: {with_stale}");

        let without = in_lang(Lang::Ja, &outside, "tako master", None);
        assert!(
            !without.contains("unset TAKO_PANE_ID"),
            "そもそも設定されていないなら unset は案内しない: {without}"
        );
    }

    /// 証拠の見出しが「文字境界で終わり、上限以内で、切ったなら `…` で終わる」こと（#1746）
    fn assert_gate_preview(ev: &str, out: &str) {
        assert!(
            ev.starts_with(out.trim_end_matches('…')),
            "証拠の先頭になっていない: {out:?}"
        );
        assert!(
            out.chars().count() <= GATE_EVIDENCE_PREVIEW_CHARS,
            "{GATE_EVIDENCE_PREVIEW_CHARS} 文字を超えた（{} 文字）: {out:?}",
            out.chars().count()
        );
        if out != ev {
            assert!(out.ends_with('…'), "切ったのに `…` が付いていない: {out:?}");
        }
    }

    /// #1746 の実物: `exit 0; stdout: `（16 バイト）+ 3 バイト文字で 120 バイト目が文字の途中。
    /// 修正前はここで `end byte index 120 is not a char boundary` の panic だった
    #[test]
    fn gateの証拠は120バイト目が文字の途中でも落ちない() {
        let ev = format!("exit 0; stdout: {}", "あ".repeat(50));
        assert!(!ev.is_char_boundary(120), "場面が #1746 の形になっていない");
        // 66 文字しかないので切らずに全文を出す
        assert_eq!(gate_evidence_preview(&ev), ev);

        let long = format!("exit 1; stderr: {}", "テストが失敗しました".repeat(20));
        assert!(!long.is_char_boundary(120));
        let out = gate_evidence_preview(&long);
        assert_gate_preview(&long, &out);
        assert_eq!(out.chars().count(), GATE_EVIDENCE_PREVIEW_CHARS);
    }

    #[test]
    fn gateの証拠は上限ちょうどなら切らず_1文字あふれたら切る() {
        let exact = format!("exit 0; stdout: {}", "あ".repeat(104));
        assert_eq!(exact.chars().count(), GATE_EVIDENCE_PREVIEW_CHARS);
        assert_eq!(gate_evidence_preview(&exact), exact);

        let over = format!("{exact}い");
        let out = gate_evidence_preview(&over);
        assert_gate_preview(&over, &out);
        assert_eq!(out, format!("exit 0; stdout: {}…", "あ".repeat(103)));
    }

    #[test]
    fn gateの証拠は空とasciiだけでも上限内() {
        assert_eq!(gate_evidence_preview(""), "");
        assert_eq!(gate_evidence_preview("exit 0"), "exit 0");
        let ascii = format!("exit 0; stdout: {}", "x".repeat(200));
        let out = gate_evidence_preview(&ascii);
        assert_gate_preview(&ascii, &out);
        assert_eq!(out, format!("exit 0; stdout: {}…", "x".repeat(103)));
    }

    #[test]
    fn gateの証拠は4バイト文字と結合文字と改行でも落ちない() {
        // 120 バイト目が絵文字（4 バイト）の途中（前置き 17 バイト + 4 バイト × n）
        let emoji = format!("exit 0; stdout: a{}", "🎉".repeat(120));
        assert!(!emoji.is_char_boundary(120));
        // 濁点を合成文字で書いた「が」（macOS のファイル名は NFD で届く）
        let nfd = format!("exit 0; stdout: {}", "か\u{3099}".repeat(70));
        // 末尾 5 行の抜粋（`format_evidence`）は改行を含む。改行は 1 文字として数え、そのまま出す
        let lines = format!(
            "exit 0; stdout (last 5/7 lines): {}",
            ["日本語の長いテスト出力の行です番号付き"; 5].join("\n")
        );
        for ev in [&emoji, &nfd, &lines] {
            let out = gate_evidence_preview(ev);
            assert_gate_preview(ev, &out);
            assert!(out.ends_with('…'), "3 つとも上限を超える長さ: {out:?}");
        }
        assert!(gate_evidence_preview(&lines).contains('\n'));
    }

    fn gate_with_evidence(evidence: &str) -> Value {
        serde_json::json!({
            "task_id": "t1758",
            "overall": "pending",
            "criteria": [
                {
                    "id": "cmd_1",
                    "status": "passed",
                    "kind": { "type": "command", "cmd": "printf 'line-a\\nline-b\\n'" },
                    "evidence": evidence,
                },
                { "id": "custom_1", "status": "pending", "kind": { "type": "custom", "description": "手動確認" } },
            ],
        })
    }

    /// #1758 の実物: 修正前は 2 行目以降が 0 桁目へ落ち、次の基準の行と見分けが付かなかった
    #[test]
    fn gateの証拠は複数行でも全行が同じ字下げで出る() {
        let text = gate_result_text(&gate_with_evidence(
            "exit 0; stdout: line-a\nline-b\nline-c",
        ));
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "Gate: t1758  [PENDING]",
                "  [PASSED] cmd_1: printf 'line-a\\nline-b\\n'",
                "         exit 0; stdout: line-a",
                "         line-b",
                "         line-c",
                "  [      ] custom_1: 手動確認",
            ],
            "{text}"
        );
    }

    /// #2016: `tako todo show` のやりとりは時刻つきで、複数行の本文は字下げして続く
    #[test]
    fn todo_showのやりとりは時刻つきで複数行を字下げする() {
        // 2026-10-13 02:02:00 UTC
        let at = 1_791_856_920;
        let r = serde_json::json!({
            "decision": "needs_change", "via": "pc", "at": at,
            "comment": "サムネを大きく\n文字も太く",
        });
        assert_eq!(
            todo_response_text(&r, at + 60, 9 * 3_600),
            "  返答 11:02 [pc] needs_change: サムネを大きく\n      文字も太く\n"
        );
        // 本文が空でも 1 行出る（判断だけの返答）
        let bare =
            serde_json::json!({"decision": "approve", "via": "pwa", "at": at, "comment": ""});
        assert_eq!(
            todo_response_text(&bare, at + 86_400, 0),
            "  返答 10/13 02:02 [pwa] approve: \n"
        );
        // `at` が読めない応答でも落ちない
        let broken = serde_json::json!({"decision": "answered"});
        assert_eq!(
            todo_response_text(&broken, at, 0),
            "  返答 - [-] answered: \n"
        );
    }

    /// #2200: `tako usage` の既定出力（ログインごとの最新の % と点数・解除時刻・過ぎた窓・空のとき）
    #[test]
    fn usageの既定出力() {
        // 2026-10-13 02:02:00 UTC
        let at = 1_791_856_920;
        let payload = serde_json::json!({
            "recording": true,
            "observations": 3,
            "first_observed_at": at * 1000,
            "last_observed_at": (at + 600) * 1000,
            "logins": [{
                "label": "A", "login": "0123456789ab", "key_from": "account",
                "observations": 3, "sessions": 2,
                "windows": [
                    {"kind": "five_hour", "used_percent": 12.0, "resets_at": at + 3_600,
                     "observed_at": (at + 600) * 1000, "expired": false, "observations": 2},
                    {"kind": "seven_day", "used_percent": 40.5, "resets_at": at - 60,
                     "observed_at": at * 1000, "expired": true, "observations": 1},
                ],
            }, {
                "label": "B", "login": "ba9876543210", "key_from": "config_dir",
                "observations": 0, "sessions": 1, "windows": [],
            }],
        });
        let text = usage_text(&payload, at + 600, 9 * 3_600);
        assert!(
            text.contains("  ログイン A（鍵 0123456789ab）  観測 3 点・会話 2 本\n"),
            "{text}"
        );
        assert!(
            !text.contains("この期間の使用量"),
            "索引が無い（S1 だけ）の応答: {text}"
        );
        assert!(
            text.contains("    5h    12%  解除 12:02  最新の観測 11:12（2 点）\n"),
            "{text}"
        );
        assert!(
            text.contains("    7d  40.5%  解除 11:01（過ぎた = 今の窓の値ではない）"),
            "{text}"
        );
        assert!(
            text.contains("ログイン B（鍵 ba9876543210 = 資格情報を読めず設定 dir で束ねた）"),
            "{text}"
        );
        assert!(text.contains("  記録 3 点（11:02 〜 11:12）\n"), "{text}");
        // #2201: 使用量の内訳（期間・合計・段・残りの件数・注記）
        let mut with_usage = payload.clone();
        with_usage["window"] = serde_json::json!("7d");
        with_usage["by"] = serde_json::json!("issue");
        with_usage["index"] = serde_json::json!({"enabled": true});
        with_usage["notes"] = serde_json::json!(["api_usd は API 換算"]);
        let amount = serde_json::json!({
            "api_usd": 12.345, "total_tokens": 123_456_789u64, "tokens": {"output": 45_678},
            "responses": 10, "sessions": 2, "unpriced_tokens": 5,
        });
        let mut group = amount.clone();
        group["name"] = serde_json::json!("tako #2201");
        with_usage["logins"][0]["usage"] = serde_json::json!({
            "range": {"basis": "observed", "from": at - 3_600, "to": at + 600, "resets_at": at + 600},
            "total": amount,
            "groups": [group],
            "more_groups": 3,
        });
        let text = usage_text(&with_usage, at + 600, 9 * 3_600);
        assert!(
            text.contains(
                "    今の 7d の窓（10:02 〜・解除 11:12）  API 換算 $12.35・トークン 1.23 億（出力 4.6 万）・応答 10・会話 2・単価不明 5\n"
            ),
            "{text}"
        );
        assert!(text.contains("      tako #2201  API 換算 $12.35"), "{text}");
        assert!(
            text.contains("  ログイン A（鍵 0123456789ab）  観測 3 点\n"),
            "内訳があるときは mod の会話数を出さない: {text}"
        );
        assert!(
            text.contains("      ほか 3 件（--limit で増やせる）\n"),
            "{text}"
        );
        assert!(
            text.contains("    この期間の使用量は無い\n"),
            "ログイン B: {text}"
        );
        assert!(text.contains("  段: issue（既定は project）\n"), "{text}");
        assert!(text.ends_with("  注: api_usd は API 換算\n"), "{text}");
        // 観測が無いときは案内だけ・A/B では記録していないと出す
        let empty = serde_json::json!({
            "recording": false, "legacy_env": "TAKO_2192_S1_LEGACY",
            "observations": 0, "logins": [], "hint": "まだ観測が無い",
        });
        let text = usage_text(&empty, at, 0);
        assert!(
            text.contains("記録していない（TAKO_2192_S1_LEGACY が立っている）"),
            "{text}"
        );
        assert!(text.ends_with("  まだ観測が無い\n"), "{text}");
    }

    /// #2204: codex のログインは系統を添え、単価表に無いモデルの按分は 1% あたりのトークンで要約する
    #[test]
    fn usageの既定出力はcodexのログインを名指す() {
        let at = 1_791_856_920;
        let amount = serde_json::json!({
            "api_usd": 0.0, "total_tokens": 2_000_000u64, "tokens": {"output": 30_000},
            "responses": 4, "sessions": 1, "unpriced_tokens": 2_000_000u64,
        });
        let mut group = amount.clone();
        group["name"] = serde_json::json!("alpha");
        group["limit_share"] = serde_json::json!({"seven_day": {"percent": 12.0}});
        let payload = serde_json::json!({
            "recording": true, "window": "7d", "by": "project",
            "index": {"enabled": true},
            "observations": 2,
            "logins": [{
                "label": "C", "login": "c0dexc0dex12", "key_from": "account", "agent": "codex",
                "observations": 2, "sessions": 0, "windows": [],
                "usage": {
                    "range": {"basis": "observed", "from": at - 3_600, "to": at, "resets_at": at + 600},
                    "total": amount,
                    "groups": [group],
                    "limit_share": [
                        {"kind": "five_hour", "available": false},
                        {"kind": "seven_day", "available": true, "scope": "current_window",
                         "latest_percent": 12.0, "observed": 12.0, "allocated": 12.0,
                         "outside": 0.0, "estimated": 0.0,
                         "coefficient": {"tokens_per_percent": 166_667.0, "intervals": 3, "recent": true}},
                    ],
                },
            }],
        });
        let text = usage_text(&payload, at, 9 * 3_600);
        assert!(
            text.contains("  ログイン C（codex・鍵 c0dexc0dex12）  観測 2 点\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "    7d の按分: 今の窓 12.0% = 段 12.0 + tako の外 0.0・1% あたり トークン 16.7 万（区間 3 本）\n"
            ),
            "{text}"
        );
        assert!(!text.contains("1% あたり API 換算"), "{text}");
    }

    /// Windows の出力（CRLF）・末尾 5 行の抜粋・120 文字での切り詰めを通っても字下げが揃う
    #[test]
    fn gateの証拠はcrlfと切り詰めを通っても全行が同じ字下げ() {
        let crlf = "exit 1; stderr: 1 件目の失敗\r\n2 件目の失敗\r\n3 件目の失敗";
        let long = format!(
            "exit 0; stdout (last 5/7 lines): {}",
            ["日本語の長いテスト出力の行です番号付き"; 5].join("\n")
        );
        for ev in [crlf, long.as_str()] {
            let text = gate_result_text(&gate_with_evidence(ev));
            // `lines()` は行末の `\r` を剥がすので、CR が残ったかを見るには `\n` で割る
            let evidence: Vec<&str> = text
                .split('\n')
                .skip(2)
                .take_while(|l| !l.starts_with("  ["))
                .collect();
            let preview = gate_evidence_preview(ev);
            assert_eq!(
                evidence.len(),
                preview.split('\n').count(),
                "証拠の行が欠けた: {text}"
            );
            for line in &evidence {
                assert!(
                    line.starts_with(GATE_EVIDENCE_INDENT),
                    "字下げが無い行: {line:?}\n{text}"
                );
                assert!(!line.ends_with('\r'), "CR が残った: {line:?}");
            }
        }
        // 1 行だけ・空の証拠は修正前と同じ形（字下げ + 本文の 1 行）
        let one = gate_result_text(&gate_with_evidence("exit 0"));
        assert!(one.contains("\n         exit 0\n"), "{one}");
        let empty = gate_result_text(&gate_with_evidence(""));
        assert!(
            empty.contains(&format!("\n{GATE_EVIDENCE_INDENT}\n")),
            "{empty}"
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
        ("edit cursor", "tako_preview_cursor"),
        ("edit move", "tako_preview_move"),
        ("edit delete", "tako_preview_delete"),
        // #1654: MCP は `tako_preview_edit` の `command` 引数（ツールを増やさない）
        ("edit indent", "tako_preview_edit"),
        ("edit outdent", "tako_preview_edit"),
        ("edit newline", "tako_preview_edit"),
        // #1692: 行操作も `tako_preview_edit` の `command`（`line-<操作>`）
        ("edit line", "tako_preview_edit"),
        // #1759: 打鍵は `command: "type"` + `text`、括弧の自動閉じの設定は `auto_close`
        ("edit type", "tako_preview_edit"),
        ("edit auto-close", "tako_preview_edit"),
        ("edit replace-range", "tako_preview_edit_range"),
        ("edit redo", "tako_preview_redo"),
        ("edit replace", "tako_preview_replace"),
        ("edit save", "tako_preview_save"),
        // #1659: MCP は `tako_preview_save` の `action`（reload / diff。ツールを増やさない）。
        // 上書きは CLI `edit save --force`（別名 `--overwrite`。#1833）= MCP の `action=overwrite`
        ("edit reload", "tako_preview_save"),
        ("edit diff", "tako_preview_save"),
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
        // #1678: CLI は `tako lsp <操作>`、MCP は action 引数を持つ 1 ツール。
        // ライフサイクルは `tako_lsp_server`、言語機能（#1679 の診断・#1680 の定義ジャンプ以降）は `tako_lsp`
        ("lsp status", "tako_lsp_server"),
        ("lsp servers", "tako_lsp_server"),
        ("lsp restart", "tako_lsp_server"),
        ("lsp stop", "tako_lsp_server"),
        ("lsp logs", "tako_lsp_server"),
        ("lsp install", "tako_lsp_server"),
        ("lsp options", "tako_lsp_server"),
        ("lsp", "tako_lsp"),
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
        // #1979: MCP は `tako_scrollback` の `unfocused_fps` 引数（ツールを増やさない）
        ("redraw-limit", "tako_scrollback"),
        // #1961: MCP は `tako_scrollback` の `mouse_forward` / `tako_send_input` の `click`
        ("mouse", "tako_scrollback"),
        // #2046: 貼り付けの確認は `tako_settings` の action=paste_guard（ツールを増やさない）
        ("paste-guard", "tako_settings"),
        ("click", "tako_send_input"),
        ("resize", "tako_resize_pane"),
        ("run-default", "tako_run_defaults"),
        // #1731: 実行設定は `tako_run_defaults` の path 側（ツールを増やさない）
        ("run-config", "tako_run_defaults"),
        // #2200: 利用制限の観測の要約は `tako_sessions` の action=usage（ツールを増やさない）
        ("usage", "tako_sessions"),
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
    /// `recover` は GUI 死亡時の復旧、`remote serve` はデーモン本体、`orchestrator resume` は
    /// 打ったシェルのプロセスを起動コマンドに置き換える突然死からの復旧 = #2470。MCP からは
    /// worker_status の `resume_command`（この 1 行）を `tako_send_input` で死んだペインへ送る）
    const CLI_ONLY: &[&str] = &[
        "master",
        "solo",
        "mcp serve",
        "recover",
        "remote serve",
        "orchestrator resume",
    ];

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

/// `--help` の文言が実装の正本からずれていないことを機械検査する（Issue #1248）。
///
/// **狙い**: 実装が動いたのに help だけ古いまま残る形を落とす。#1248 の実例は 2 つで、
/// どちらも「実装は正しく、説明文だけが数か月前の姿を語っていた」:
///
/// 1. `setup-mcp` の書き先が `~/.claude/settings.json` から `~/.claude.json` /
///    `<cwd>/.mcp.json` へ移り（旧経路は `clean_legacy_settings_json` が掃除する側になった）、
///    さらに #979 で codex / agy へもまとめて登録するようになったのに、help は
///    「Claude Code の settings.json に…」のままだった
/// 2. `--agent-effort` は #1002 で agy にも `--effort` を渡すようになったのに、
///    help は「agy は無視」のままだった
///
/// なので検査も**説明文どうしを比べない**。書き先は [`mcp_target_path`] が組む実パス、
/// 能力の有無は [`agent_support`] のマトリクスという、**挙動を決めている側**と突き合わせる。
#[cfg(test)]
mod help_text_parity {
    use super::*;
    use clap::CommandFactory as _;
    use tako_control::dispatch::{
        mcp_target_path, McpScope, MCP_TARGET_FILE_PROJECT, MCP_TARGET_FILE_USER,
    };
    use tako_core::agent_support::{self, keys, Agent};

    /// サブコマンドを名前でたどる（`tako orchestrator profiles set` のような多段用）
    fn subcommand(path: &[&str]) -> clap::Command {
        let mut cmd = Cli::command();
        for name in path {
            cmd = cmd
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("サブコマンド {name} が無い（{path:?}）"))
                .clone();
        }
        cmd
    }

    /// `tako setup-mcp --help` が**実際に書く場所**を述べている（#1248 の①）
    #[test]
    fn setup_mcpのhelpが書き先の正本と一致する() {
        let help = subcommand(&["setup-mcp"]).render_long_help().to_string();

        // 正本が組む実パスの末尾要素 = help に出るべきファイル名
        let user = mcp_target_path(&McpScope::User);
        let project = mcp_target_path(&McpScope::Project(std::path::PathBuf::from("/tmp/x")));
        assert!(
            user.ends_with(MCP_TARGET_FILE_USER) && project.ends_with(MCP_TARGET_FILE_PROJECT),
            "正本の定数と mcp_target_path がずれている: {user:?} / {project:?}"
        );
        for name in [MCP_TARGET_FILE_USER, MCP_TARGET_FILE_PROJECT] {
            assert!(
                help.contains(name),
                "setup-mcp の --help が書き先 {name} を述べていない:\n{help}"
            );
        }

        // 旧経路。掃除される側なので「書き先」として案内してはいけない
        assert!(
            !help.contains("settings.json"),
            "setup-mcp の --help が旧経路 settings.json を書き先として案内している:\n{help}"
        );

        // #979: 引数なしで claude 以外にも登録する
        for agent in ["claude", "codex", "agy"] {
            assert!(
                help.contains(agent),
                "setup-mcp の --help が {agent} に触れていない（#979 でまとめて登録する）:\n{help}"
            );
        }
    }

    /// 「その系統では効かない」と書いてよいのは、マトリクスがそう宣言しているときだけ
    fn assert_no_stale_unsupported_claim(help: &str, key: &str, where_: &str) {
        for agent in Agent::ALL {
            if !agent_support::supports(agent, key) {
                continue;
            }
            let name = agent.as_str();
            for phrase in [
                format!("{name} は無視"),
                format!("{name} は指定手段が無"),
                format!("{name} は非対応"),
                format!("{name} は対応していない"),
                format!("{name}: 無視"),
                format!("{name} 無視"),
            ] {
                assert!(
                    !help.contains(&phrase),
                    "{where_} が「{phrase}」と書いているが、マトリクスは {name} の {key} を \
                     supported と宣言している（どちらかが古い）:\n{help}"
                );
            }
        }
    }

    /// `profiles set --agent-effort` の説明が effort_control の宣言と一致する（#1248 の②）
    #[test]
    fn agent_effortのhelpが能力マトリクスと一致する() {
        let cmd = subcommand(&["orchestrator", "profiles", "set"]);
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id() == "agent_effort")
            .expect("--agent-effort が無い");
        let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

        assert!(
            agent_support::supports(Agent::Agy, keys::EFFORT_CONTROL),
            "この検査は agy = supported を前提にしている（#1002）"
        );
        assert_no_stale_unsupported_claim(&help, keys::EFFORT_CONTROL, "--agent-effort の help");
    }

    /// `text` が `word` を英数字の語として含むか（`git` が `github` の一部で当たらないように）
    fn names_word(text: &str, word: &str) -> bool {
        let is_word = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        text.match_indices(word).any(|(i, _)| {
            let before = text[..i].chars().next_back().is_none_or(|c| !is_word(c));
            let after = text[i + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_word(c));
            before && after
        })
    }

    /// Issue #2036: `tako panel --help` が候補値の一覧と食い違わない。
    ///
    /// 各値の説明は MCP `tako_panel` と同じ正本（`PanelViewWire::summary`）から生成する。
    /// 手書きの説明（`--view` の説明文・サブコマンドの見出し）は値を挙げないか、
    /// 挙げるなら全件を挙げる（#2036 の症状 = fleet / orch / git の 3 つで止まっていた）
    #[test]
    fn panel_viewのhelpは候補値の全件を説明つきで載せる() {
        use tako_control::protocol::PanelViewWire;

        let panel = subcommand(&["panel"]);
        let help = panel.clone().render_long_help().to_string();
        for v in PanelViewWire::VALUES {
            let summary = PanelViewWire::parse(v)
                .expect("VALUES は parse を通る")
                .summary();
            assert!(
                help.lines().any(|l| {
                    let l = l.trim_start();
                    l.starts_with(&format!("- {v}:")) && l.contains(summary)
                }),
                "panel --help の一覧に {v} の説明（{summary}）が無い:\n{help}"
            );
        }

        let view = panel
            .get_arguments()
            .find(|a| a.get_id() == "view")
            .expect("panel に --view が在る");
        let handwritten = [
            ("--view の説明文", view.get_help()),
            ("--view の説明文（長）", view.get_long_help()),
            ("サブコマンドの見出し", panel.get_about()),
            ("サブコマンドの見出し（長）", panel.get_long_about()),
        ];
        for (place, text) in handwritten {
            let Some(text) = text.map(|t| t.to_string()) else {
                continue;
            };
            let named: Vec<&str> = PanelViewWire::VALUES
                .iter()
                .copied()
                .filter(|v| names_word(&text, v))
                .collect();
            assert!(
                named.is_empty() || named.len() == PanelViewWire::VALUES.len(),
                "{place}が候補値の一部（{named:?}）だけを挙げている。\
                 値の一覧は possible values の生成に任せて手書きで並べない: {text}"
            );
        }
    }

    /// `names_word` の検出力: #2036 の修正前の説明文は 3 値だけを挙げていたと判定できる
    #[test]
    fn names_wordは語の一部を値として拾わない() {
        let before = "表示するビュー（GUI のタブ名と同じ。fleet = ペイン / セッション俯瞰、\
                      orch = オーケストレーター俯瞰、git = git。tmux は fleet の旧称）";
        let named: Vec<&str> = tako_control::protocol::PanelViewWire::VALUES
            .iter()
            .copied()
            .filter(|v| names_word(before, v))
            .collect();
        assert_eq!(named, ["fleet", "orch", "git"]);
        assert!(!names_word("github の search-index", "git"));
        assert!(!names_word("github の search-index", "search"));
        assert!(names_word("view=search は", "search"));
    }
}

/// #1732: `tako run-config` の `--env K=V` / `--unset-env K` → `RunnerDefaults.env`
#[cfg(test)]
mod tests_1732 {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 環境変数の設定と削除を1つの表へ畳む() {
        assert_eq!(run_config_env(&[], &[]), Ok(None));
        let got = run_config_env(&v(&["A=1", "B=x=y", "C="]), &v(&["D"]))
            .unwrap()
            .unwrap();
        assert_eq!(got.get("A"), Some(&Some("1".to_string())));
        // 値の中の `=` はそのまま（最初の `=` で割る）・空の値も値
        assert_eq!(got.get("B"), Some(&Some("x=y".to_string())));
        assert_eq!(got.get("C"), Some(&Some(String::new())));
        assert_eq!(got.get("D"), Some(&None));
    }

    #[test]
    fn k_eq_vでない指定と設定と削除の重なりは拒否する() {
        let e = run_config_env(&v(&["NOVALUE"]), &[]).unwrap_err();
        assert!(e.contains("K=V"), "{e}");
        let e = run_config_env(&v(&["A=1"]), &v(&["A"])).unwrap_err();
        assert!(e.contains("--unset-env"), "{e}");
    }

    /// `--args` / `--before` はハイフンで始まる値も受ける（`--args --verbose`）
    #[test]
    fn ハイフンで始まる引数を値として受ける() {
        let cli = Cli::try_parse_from([
            "tako",
            "run-config",
            "a.py",
            "--args",
            "--verbose -n 1",
            "--before",
            "-x",
            "--env",
            "A=1",
            "--unset-env",
            "B",
            "--cwd",
            "..",
        ])
        .unwrap();
        let Command::RunConfig(a) = cli.command else {
            panic!("run-config に解釈されない");
        };
        assert_eq!(a.args.as_deref(), Some("--verbose -n 1"));
        assert_eq!(a.before.as_deref(), Some("-x"));
        assert_eq!(a.env, v(&["A=1"]));
        assert_eq!(a.unset_env, v(&["B"]));
        assert_eq!(a.cwd.as_deref(), Some(".."));
    }

    /// #1734: `--target` は正本（`RunTarget::VALUES`）の語だけを受け、`RunnerDefaults.target` へ渡る
    #[test]
    fn 実行先は正本の語だけを受けて実行設定の口へ渡る() {
        for t in tako_core::runner_config::RunTarget::VALUES {
            let cli =
                Cli::try_parse_from(["tako", "run-config", "/p/a.py", "--target", t]).unwrap();
            match build_request(&cli.command).unwrap() {
                // パスは Windows では `/p/a.py` が相対扱い（cwd と結ぶ）なので見ない
                Request::RunnerDefaults { path, target, .. } => {
                    assert!(path.is_some());
                    assert_eq!(target.as_deref(), Some(t));
                }
                other => panic!("RunnerDefaults ではない: {other:?}"),
            }
        }
        for bad in ["New", "tab"] {
            assert!(
                Cli::try_parse_from(["tako", "run-config", "a.py", "--target", bad]).is_err(),
                "{bad}"
            );
        }
        let cli = Cli::try_parse_from(["tako", "run-config", "/p/a.py"]).unwrap();
        match build_request(&cli.command).unwrap() {
            Request::RunnerDefaults { target, .. } => assert_eq!(target, None),
            other => panic!("RunnerDefaults ではない: {other:?}"),
        }
    }
}
