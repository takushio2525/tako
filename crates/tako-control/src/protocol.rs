//! protocol — Layer 1 IPC / Layer 2 MCP 共通の操作プロトコル定義（FR-2.2 / FR-2.5）
//!
//! ワイヤ形式は 1 行 1 JSON の JSON-RPC 2.0 サブセット + `token` フィールド拡張:
//!
//! ```json
//! {"jsonrpc":"2.0","id":1,"token":"...","method":"split","params":{"pane":3,"direction":"down"}}
//! {"jsonrpc":"2.0","id":1,"result":{"pane":7}}
//! {"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"..."}}
//! ```
//!
//! 操作セットは FR-2.5（AI レイアウト操作セット）と 1:1。Phase 3 の MCP ツールも
//! この [`Request`] を共有し、`dispatch` 経由で同じセマンティクスを呼ぶ。

use serde::{Deserialize, Serialize};
use tako_core::{SplitAxis, SplitDirection};

pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC エラーコード
pub mod error_code {
    /// JSON として解釈できない
    pub const PARSE: i64 = -32700;
    /// パラメータ不正
    pub const INVALID_PARAMS: i64 = -32602;
    /// サーバー内部エラー（受け口の消失等）
    pub const INTERNAL: i64 = -32603;
    /// ドメイン操作の失敗（ペインが無い等）
    pub const OPERATION: i64 = -32000;
    /// トークン認証失敗（FR-2.3.4）
    pub const AUTH: i64 = -32001;
}

/// 分割・フォーカス移動の方向（`tako_core::SplitDirection` のワイヤ表現）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Right,
    Down,
    Left,
    Up,
}

impl Direction {
    pub fn to_core(self) -> SplitDirection {
        match self {
            Direction::Right => SplitDirection::Right,
            Direction::Down => SplitDirection::Down,
            Direction::Left => SplitDirection::Left,
            Direction::Up => SplitDirection::Up,
        }
    }
}

/// リサイズの軸（`tako_core::SplitAxis` のワイヤ表現）。x = 横幅、y = 縦幅
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    pub fn to_core(self) -> SplitAxis {
        match self {
            Axis::X => SplitAxis::Horizontal,
            Axis::Y => SplitAxis::Vertical,
        }
    }
}

fn default_true() -> bool {
    true
}

/// 右サイドバー情報パネルの内部ビュー（固定タブ 0 個方針。FR-2.16.6 で agents は
/// tmux ビューへ統合済み。git は git graph（FR-3.6）実装までプレースホルダ表示）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelViewWire {
    Tmux,
    Git,
}

impl PanelViewWire {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelViewWire::Tmux => "tmux",
            PanelViewWire::Git => "git",
        }
    }
}

/// プレビューペインの表示モード（FR-3.2 / FR-3.3 / FR-3.10 / FR-3.4）。
/// Markdown ファイルは目アイコンのトグル（UI）または CLI / MCP の mode 指定で
/// 「コードとして表示」⇔「md レンダリング表示」を切り替えられる
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewModeWire {
    Code,
    Markdown,
    Image,
    Pdf,
    Video,
}

impl PreviewModeWire {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewModeWire::Code => "code",
            PreviewModeWire::Markdown => "markdown",
            PreviewModeWire::Image => "image",
            PreviewModeWire::Pdf => "pdf",
            PreviewModeWire::Video => "video",
        }
    }
}

/// 操作リクエスト。`pane` 省略時は呼び出し元ペイン（クライアント側で `TAKO_PANE_ID` から
/// 解決して詰める。FR-2.2.7）。各操作のセマンティクスは tako-core の API と 1:1
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// ペイン分割（FR-2.2.1 / FR-2.5.3）。`command` 指定時はシェルの代わりに実行する。
    /// `tab` 指定時はそのタブのフォーカス中ペインの隣に分割する（`pane` より優先）
    Split {
        pane: Option<u64>,
        /// タブ ID 指定（そのタブのフォーカス中ペインの隣に分割する。pane と排他）
        tab: Option<u64>,
        direction: Option<Direction>,
        /// 新ペイン側の取り分（0.0–1.0、省略時は等分）
        ratio: Option<f32>,
        command: Option<Vec<String>>,
        cwd: Option<String>,
        /// 新ペインにフォーカスを移すか（省略時は false = 分割元を維持）
        #[serde(default)]
        focus: Option<bool>,
    },
    /// ペイン削除（FR-2.5.4。呼び出し元自身の削除 = 自己片付けを含む）
    Close {
        pane: Option<u64>,
        /// true にすると busy な worker でも強制 close（省略時 false）
        #[serde(default)]
        force: bool,
    },
    /// フォーカス移動（FR-2.5.5）。`direction` 指定時はアクティブタブ内の方向移動
    Focus {
        pane: Option<u64>,
        direction: Option<Direction>,
    },
    /// サイズ調整（FR-2.5.6）。`delta` は相対増減、`share` は取り分の絶対指定
    Resize {
        pane: Option<u64>,
        axis: Axis,
        delta: Option<f32>,
        share: Option<f32>,
    },
    /// レイアウト均等化（FR-2.5.7）。`tab` 省略時は `pane` の属するタブ
    Equalize { pane: Option<u64>, tab: Option<u64> },
    /// タブ / ペインのツリー構造・ジオメトリ・状態の取得（FR-2.2.4 / FR-2.5.1〜2）
    List,
    /// ペインへのテキスト送信（FR-2.2.2）。`newline` で末尾に改行（CR）を付与。
    /// `tmux_session` 指定時はペインが見つからなくても tmux session 経由で送信する。
    /// `await_prompt` が true の場合、claude TUI の ❯ プロンプト表示を待ってから送信する。
    /// 全画面 TUI（claude 等）への newline つき送信は送達確認ループ（貼り付け →
    /// 分離 Enter → 入力欄の空検証 + 再送）で配送される（Issue #32。応答は queued）
    Send {
        pane: Option<u64>,
        text: String,
        #[serde(default = "default_true")]
        newline: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_session: Option<String>,
        #[serde(default)]
        await_prompt: bool,
    },
    /// ペインの画面内容取得（FR-2.2.5）。`lines` は末尾からの行数制限。
    /// `tmux_session` 指定時はペインが見つからなくても tmux session 経由で読む
    Read {
        pane: Option<u64>,
        lines: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_session: Option<String>,
    },
    /// スクロールバック表示の操作（FR-2.5.13）。`to` は絶対位置（0 = 最下部）、
    /// `delta` は相対行数（正 = 過去方向）。両方省略はエラー
    Scroll {
        pane: Option<u64>,
        to: Option<u64>,
        delta: Option<i32>,
    },
    /// タイトル・役割ラベルの設定（FR-2.2.6 / FR-2.1.3）。空文字でクリア
    Title {
        pane: Option<u64>,
        title: Option<String>,
        role: Option<String>,
    },
    /// tmux セッション一覧 + tako ペインとの対応付け（FR-2.13）。
    /// `socket` は `tmux -L` のサーバー名（通常は None = 既定サーバー）
    TmuxList { socket: Option<String> },
    /// tmux の kill（FR-2.13.3）。`window` 指定で kill-window、なければ kill-session。
    /// 誤爆防止の確認は呼び出し側（UI / AI）の責務
    TmuxKill {
        socket: Option<String>,
        session: String,
        window: Option<u32>,
    },
    /// tmux window のリサイズ（スマホリモートのビューポート連動用。Issue #23）。
    /// `cols` / `rows` 指定で `resize-window -x -y`（window-size が manual になる）、
    /// `reset` = true で manual を解除しサーバー既定へ戻す
    TmuxResize {
        socket: Option<String>,
        session: String,
        window: u32,
        cols: Option<u32>,
        rows: Option<u32>,
        #[serde(default)]
        reset: bool,
    },
    /// tmux セッションをタブ内へ取り込む（FR-2.16.10。統合 tmux ビューの D&D と同等操作）。
    /// `pane` を `direction`（省略時は右）へ分割した新ペインで attach クライアント
    /// （`TMUX= tmux [-L socket] attach-session -t =session`）を起動する。
    /// 新ペインを閉じてもセッションは残る（kill しない）
    TmuxOpen {
        socket: Option<String>,
        session: String,
        /// 特定 window のみ attach（省略時はセッション全体）
        window: Option<u32>,
        pane: Option<u64>,
        direction: Option<Direction>,
    },
    /// バックエンドセッションのアクティブ window を切り替える。`pane` のバックエンド
    /// セッション内で `window` 番号の window に切り替える。`pane` 省略時は呼び出し元ペイン
    TmuxSelectWindow { pane: Option<u64>, window: u32 },
    /// orphan tmux セッションの一括クリーンアップ（FR-2.16.11）。`socket` 省略時は
    /// tako バックエンドサーバー。detached・非 grouped・未使用の `tako-` セッションのみ
    /// kill する（使用中・ユーザーセッションには触れない）。kill した名前を返す
    TmuxCleanup { socket: Option<String> },
    /// タブのリネーム（FR-2.12.1）。`tab` 省略時は `pane`（呼び出し元）の属するタブ。
    /// 明示リネームとして自動リネーム（FR-2.12）より優先され、空文字で手動指定を解除する
    TabRename {
        pane: Option<u64>,
        tab: Option<u64>,
        title: String,
    },
    /// タブ作成（FR-2.5.10）
    TabNew { title: Option<String> },
    /// タブ切替（FR-2.5.10）
    TabSelect { tab: u64 },
    /// ペインの移動（FR-2.5.10 / FR-1.10）。`tab` 指定 = 別タブの末尾へ移送（従来動作）、
    /// `target` 指定 = そのペインを `direction`（省略時は右）へ分割した位置に挿し直す
    /// （同タブ内の並べ替え = タイトルバー D&D と同等。タブまたぎも可）。
    /// `tab` と `target` は排他で、どちらか一方が必須
    MovePane {
        pane: Option<u64>,
        tab: Option<u64>,
        target: Option<u64>,
        direction: Option<Direction>,
    },
    /// タブ・ペイン名の AI 自動リネーム（FR-2.12.4）の ON/OFF。
    /// `enabled` 省略時は現在状態の取得のみ。設定は永続化される
    AutoRename { enabled: Option<bool> },
    /// listen ポート検知 + 提案チップ（FR-2.4.4）の ON/OFF。
    /// `enabled` 省略時は現在状態の取得のみ。設定は永続化される
    PortDetect { enabled: Option<bool> },
    /// tmux バックエンドによるセッション永続化（Phase 5.5 / FR-5）の ON/OFF。
    /// `enabled` 省略時は現在状態の取得のみ。切替は**以後生成されるペイン**に効く
    /// （既存ペインのバックエンドは変わらない）。設定は永続化される
    Persist { enabled: Option<bool> },
    /// タブ/ペインの × ボタン close 時の確認ダイアログ ON/OFF（Issue #172）。
    /// `enabled` 省略時は現在状態の取得のみ。設定は config.yaml に永続化される
    ConfirmClose { enabled: Option<bool> },
    /// 右サイドバー情報パネル（統合 tmux ビュー / git）の表示・幅・ビュー切替と、
    /// 左サイドバーのファイルツリー表示切替（FR-2.16.5。下部ステータスバーのトグルと
    /// 同じ経路）。すべて省略 = 現在状態の取得のみ（AI が成果や状況をユーザーへ見せる
    /// 導線。設計原則 5: UI でできる操作はすべてここから可能）
    Panel {
        visible: Option<bool>,
        /// パネル幅（px）
        width: Option<f32>,
        view: Option<PanelViewWire>,
        /// 左サイドバーのファイルツリー（FR-3.1）の表示・非表示
        filetree: Option<bool>,
    },
    /// サイドバー tmux ビューのタブ枠の折りたたみ（FR-2.16.14）。折りたたむと、その
    /// タブ配下の**バックグラウンド項目（裏で実行中のペイン行 + バックグラウンド）を隠し、前面表示中の
    /// 行は残す**。`tab` 省略時は `pane`（呼び出し元）の属するタブ。`collapsed` 省略時は
    /// トグル（現在状態の反転）。設定は永続化される
    CollapseTab {
        pane: Option<u64>,
        tab: Option<u64>,
        collapsed: Option<bool>,
    },
    /// プレビューのピン留め / 解除（FR-2.16.15）。サイドバー tmux ビューのバックグラウンド
    /// ペイン（`pane`）または閉じたタブグループ（`group_tab` = 由来タブ ID）の実画面サムネイルを
    /// アプリ内フローティングウィンドウとして常駐させ、ライブ更新し続ける。`pane` / `group_tab` は
    /// 排他で、どちらも省略時は呼び出し元ペイン。`pinned` 省略時はトグル
    Pin {
        pane: Option<u64>,
        group_tab: Option<u64>,
        pinned: Option<bool>,
    },
    /// ファイルをプレビューペインで開く（FR-3.2 / FR-2.5.11。「探して開いて見せて」のコア操作）。
    /// `pane` がプレビューペインならそのまま差し替え、ターミナルペインなら同タブの既存
    /// プレビューペインを再利用し、無ければ `pane` を分割して生やす（ターミナルは起動しない）。
    /// 相対パスは `pane` の cwd（OSC 7）基準で解決する。
    /// `mode` 省略時は拡張子から自動判定（.md / .markdown → markdown、それ以外 code）。
    /// `direction` 指定時（FR-3.11 = ファイル D&D の同等操作）は既存プレビューを
    /// 再利用せず、必ず `pane` をその方向へ分割して新しいプレビューペインを生やす
    OpenFile {
        pane: Option<u64>,
        path: String,
        mode: Option<PreviewModeWire>,
        direction: Option<Direction>,
    },
    /// コードプレビューの編集モードを切り替える（FR-3.5）。`enabled` 省略時は状態取得。
    PreviewEdit {
        pane: Option<u64>,
        enabled: Option<bool>,
    },
    /// 編集バッファの全文を置き換える（GUI の個々のキー入力ではなく、CLI / MCP から
    /// 編集内容を自然に適用するための操作）。保存は PreviewSave で明示する。
    PreviewApply { pane: Option<u64>, text: String },
    /// 編集バッファをファイルへ保存する。外部変更を検知した場合は上書きしない。
    PreviewSave { pane: Option<u64> },
    /// undo（#195）
    PreviewUndo { pane: Option<u64> },
    /// redo（#195）
    PreviewRedo { pane: Option<u64> },
    /// 検索（#195）。query を指定してヒット一覧を返す。direction で次/前を移動
    PreviewSearch {
        pane: Option<u64>,
        query: Option<String>,
        direction: Option<String>,
    },
    /// 置換（#195）。query に一致する箇所を replacement で置換。all=true で全置換
    PreviewReplace {
        pane: Option<u64>,
        query: String,
        replacement: String,
        all: Option<bool>,
    },
    /// 自動保存設定の取得・変更（#195）。enabled 省略時は状態取得のみ
    PreviewAutosave {
        pane: Option<u64>,
        enabled: Option<bool>,
    },
    /// ファイルシステム操作（FR-3.12）
    FileOp {
        op: FileOpKind,
        path: String,
        name: Option<String>,
        pane: Option<u64>,
    },
    /// git ログ取得（FR-3.6 git graph）。`pane` の cwd のリポジトリのコミット一覧・
    /// ブランチ・status を返す。`max_count` は取得上限（省略時 200）
    GitLog {
        pane: Option<u64>,
        max_count: Option<usize>,
    },
    /// git diff 取得（FR-3.9 diff ビューア）。`target` で diff の種別を指定:
    /// `"unstaged"` / `"staged"` / コミットハッシュ（省略時は unstaged）
    GitDiff {
        pane: Option<u64>,
        target: Option<String>,
    },
    /// ペインをバックグラウンドへ送る（FR-2.15.1）。プロセスは生きたまま画面から外す
    Background { pane: Option<u64> },
    /// バックグラウンドからペインを復帰させる（FR-2.15.3 / FR-2.15.4）。`target` を
    /// `direction`（省略時は右）へ分割した位置に挿し直す。`target` 省略時は
    /// アクティブタブのフォーカス中ペインの隣
    Foreground {
        pane: u64,
        target: Option<u64>,
        direction: Option<Direction>,
    },
    /// バックグラウンドペインの一覧取得。ID / title / role / 状態を返す
    BackgroundList,
    /// バックグラウンドのペインを kill する（FR-2.15.2）。確認は呼び出し側の責務
    BackgroundKill { pane: u64 },
    /// 環境の健全性診断。CLI の PATH / バージョン一致 / 外部ツールの有無等をチェックする
    CheckHealth,
    /// Claude Code の settings.json に tako MCP サーバーの接続設定を追加する（FR-2.14）。
    /// `scope` = "global"（`~/.claude/settings.json`、既定）/ "project"（ペインの cwd
    /// 配下 `.claude/settings.json`）
    SetupMcp {
        scope: Option<String>,
        pane: Option<u64>,
    },
    /// 動画の再生/一時停止（プレビューペインが Video モードの場合のみ有効）。
    /// `action` = "play" / "pause" / "toggle"。`pane` 省略時は呼び出し元ペイン
    VideoPlayback { pane: Option<u64>, action: String },
    /// 動画のシーク。`seconds` は絶対位置（秒）。`pane` 省略時は呼び出し元ペイン
    VideoSeek { pane: Option<u64>, seconds: f64 },
    /// オーケストレーター: プロジェクト管理（list / add / remove）
    OrchestratorProjects {
        action: String,
        key: Option<String>,
        cwd: Option<String>,
        description: Option<String>,
    },
    /// オーケストレーター: プロファイル管理（list / show / set）。
    /// model 未指定のプロファイルは claude CLI の既定モデルで起動する（Issue #27）。
    /// set は model / worker_model / effort / worker_effort の更新と、
    /// clear_model / clear_worker_model による解除（claude 既定へ戻す）に対応。
    /// worker_agent（既定エージェント種別）と agent_* 系（`worker_agents.<agent>` の
    /// エージェント別 worker 設定）は Issue #120、master_agent は Issue #127 で追加
    OrchestratorProfiles {
        action: String,
        name: Option<String>,
        /// master のエージェント種別（claude / codex。agy は master 非対応）を設定する
        #[serde(default, skip_serializing_if = "Option::is_none")]
        master_agent: Option<String>,
        /// master_agent の指定を解除して claude 既定へ戻す
        #[serde(default)]
        clear_master_agent: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_effort: Option<String>,
        #[serde(default)]
        clear_model: bool,
        #[serde(default)]
        clear_worker_model: bool,
        /// worker の既定エージェント種別（claude / codex / agy）を設定する
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_agent: Option<String>,
        /// worker_agent の指定を解除して claude 既定へ戻す
        #[serde(default)]
        clear_worker_agent: bool,
        /// `worker_agents.<agent>` を編集する対象エージェント名（agent_* 系の指定に必須）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        /// 対象エージェントの worker 既定モデル（CLI ネイティブ表記）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_model: Option<String>,
        #[serde(default)]
        clear_agent_model: bool,
        /// 対象エージェントの worker 既定 effort（agy は無視される）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_effort: Option<String>,
        #[serde(default)]
        clear_agent_effort: bool,
        /// 対象エージェントの許可プロンプトスキップ（明示 opt-in）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_skip_permissions: Option<bool>,
        /// 対象エージェントの追加 CLI 引数（丸ごと置き換え。空配列でクリア）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_args: Option<Vec<String>>,
        /// worker_model_policy（inherit / delegate / fixed）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_model_policy: Option<String>,
    },
    /// オーケストレーター: worker spawn のレイアウト設定の取得・変更（Issue #165）。
    /// 全パラメータ省略で現在値の取得、いずれか指定でその項目を更新して結果を返す。
    /// policy: "master-reserved"（master の取り分を維持し worker は右側の worker 領域内へ。既定）
    /// / "legacy"（従来の右等分割）。master_ratio: master 側へ残す取り分（0.1〜0.9。既定 0.5）。
    /// algorithm: worker 領域内の配置（"grid" = 十字四分割系 / "spiral" = 縦横交互の半分割）
    OrchestratorLayout {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        master_ratio: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        algorithm: Option<String>,
    },
    /// オーケストレーター: worker の spawn（split + エージェント CLI 起動 + プロンプト送信）
    OrchestratorSpawn {
        project: String,
        prompt: String,
        label: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        pane: Option<u64>,
        tab: Option<u64>,
        /// 呼び出し元の TAKO_ORCHESTRATOR_ROLE。複数 master 並行時に pane が stale でも
        /// 正しい master を特定するフォールバック（#109）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_role: Option<String>,
        /// worker のエージェント種別（claude / codex / agy。省略時はプロファイル既定。#120）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// オーケストレーター: master が自身の pane/tab/ctx% を取得する（#123 / #193）。
    /// caller_pane（MCP: X-Tako-Pane / stdio: TAKO_PANE_ID）と caller_role から
    /// master のペインを特定し、claude agents --json で ctx% を取得する。
    /// pane/tab 指定は省略可（省略時は caller 情報から自動解決）
    OrchestratorSelf {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_role: Option<String>,
    },
    /// オーケストレーター: master の引き継ぎ（#193）。
    /// handoff ファイル（`<config_dir>/handoff/<profile>.md`）を確認し、
    /// 同プロファイルの新 master を spawn して引き継ぎプロンプトを注入する
    OrchestratorHandoff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_role: Option<String>,
        /// 引き継ぎ先のタブ ID（省略時は呼び出し元と同タブ）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<u64>,
    },
    /// オーケストレーター: worker の状態確認。`tmux_session` 指定時は pane が gone でも
    /// tmux session 経由で recent_output を取得する
    OrchestratorWorkerStatus {
        pane_id: u64,
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tmux_session: Option<String>,
    },
    /// オーケストレーター: 非同期 run の進捗照会（#121）。
    /// run_id が不明なら Err。run_id 省略時は全 run の一覧を返す
    OrchestratorRunStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    /// オーケストレーター: 完了した非同期 run の結果回収（#121）。
    /// 未完了なら `phase: "running"` を返す。完了済みなら出力取得 + auto_close +
    /// レジストリから除去
    OrchestratorRunResult { run_id: String },
    /// リモートアクセス API サーバーの起動。`port` 省略時は 7749。
    /// 既定では暗号化トンネル（cloudflared）経由でのみホストし、トンネルを張れなければ
    /// 起動を拒否する。`insecure` = true のときだけ平文 HTTP の LAN 直モードを許可する
    /// （明示 opt-in・非推奨。同一 LAN 上の盗聴リスクあり。#104）
    RemoteStart {
        port: Option<u16>,
        #[serde(default)]
        insecure: bool,
    },
    /// リモートアクセス API サーバーの停止
    RemoteStop,
    /// リモートアクセス API サーバーの状態取得。
    /// `show_token` = true のときだけ応答にトークンを平文で含める（既定はマスク。
    /// スクリーンショット・画面共有経由でのトークン漏えいを防ぐため）
    RemoteStatus {
        #[serde(default)]
        show_token: bool,
    },
    /// エージェント一覧（`claude agents --json` プロキシ + tmux ペイン対応付け。Issue #23）
    RemoteAgents,
    /// Claude Code の会話ログ（transcript）の末尾 `tail` 件を正規化して取得（Issue #23）
    RemoteMessages {
        session_id: String,
        tail: Option<usize>,
    },
    /// ペインのスクロールバック履歴をプレーンテキストで取得（Issue #42 履歴レイヤー用）
    RemoteScrollback { pane_id: String, lines: Option<u32> },
    /// Web ビューペインの操作（FR-3.8、Issue #155）。ネイティブ webview
    /// （macOS = WKWebView）をペインとして表示・管理する。`action`:
    /// - "open": `url` を新しい Web ビューペインで開く（`pane` を `direction` 方向に分割）
    /// - "list": 全 Web ビュー（表示中 + dock 退避中）の一覧
    /// - "show": dock 退避中の `id` をペインへ呼び出す（`pane` を `direction` 方向に分割）
    /// - "hide": 対象をペインから外して dock へ退避する（ページは生きたまま）
    /// - "close": 対象を完全に破棄する（表示中ならペインも閉じる）
    /// - "navigate": `to`（"back" / "forward" / "reload" / URL）でページ遷移
    /// - "eval": `js` を非同期評価し `token` を返す（結果は "eval_result" で回収）
    /// - "eval_result": `token` の評価結果を回収する（未完なら pending: true）
    /// - "read": URL・タイトル・読み込み状態を返す
    ///
    /// 対象解決（hide / close / navigate / eval / eval_result / read）: `id` 優先、
    /// 次に `pane`（そのペインに表示中の Web ビュー）、どちらも省略時は
    /// 表示中の Web ビューが 1 つだけならそれ
    Web {
        action: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        id: Option<u64>,
        #[serde(default)]
        pane: Option<u64>,
        #[serde(default)]
        direction: Option<Direction>,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        js: Option<String>,
        #[serde(default)]
        token: Option<u64>,
    },
    /// アプリ内更新の診断・実行（Issue #36 + #50）。
    /// `action` 省略 or `"status"` → 配布系統・現在バージョン・重複 CLI の診断情報。
    /// `"check"` → GitHub Releases を問い合わせて最新版の有無を返す（更新しない）。
    /// `"apply"` → 配布系統に応じた更新を実行する（再起動は UI 側の責務）。
    /// `"apply-zip"` → 配布系統を問わず zip 経由で強制更新する（brew 失敗時のフォールバック）。
    /// `"repair"` → broken-brew 状態を修復する（brew install --cask --force で台帳を再締結）
    Update {
        #[serde(default)]
        action: Option<String>,
    },
    /// フルディスクアクセス (FDA) の状態確認と設定画面を開く操作（Issue #118）。
    /// `action` = "status"（既定）/ "open"（システム設定を開く）
    Fda {
        #[serde(default)]
        action: Option<String>,
    },
    /// setup のアップデート追従状況の照会（Issue #94）。
    /// 適用済みリビジョン・現在リビジョン・未適用の setup 関連変更の一覧を返す。
    /// 適用自体は `tako setup`（対話）が行い、これは読み取り専用
    SetupChanges,
    /// エージェント共通ルールの同期（Issue #136）。
    /// `action` = "sync"（同期実行）/ "status"（状態確認）
    AgentsSyncRules {
        #[serde(default)]
        action: Option<String>,
        /// 正本パスの一時上書き（config.yaml の設定より優先。省略時は設定値）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        /// 対象エージェントの絞り込み（省略時は設定値、設定値も無ければ全対象）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        targets: Option<Vec<String>>,
    },
    /// スリープ防止機能の状態確認・設定変更（Issue #173）。
    /// `action` = "status"（既定）/ "set"（モード・電源条件の変更）
    SleepGuard {
        #[serde(default)]
        action: Option<String>,
        /// スリープ防止モード: "off" / "on" / "while-agents-running"
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        /// 電源条件: "ac-only" / "always"
        #[serde(default, skip_serializing_if = "Option::is_none")]
        power_condition: Option<String>,
    },
    /// ファイルツリーへのフォルダ追加・削除・一覧（#134）。
    /// AI が作業対象プロジェクトのフォルダをファイルツリーに明示追加する。タブ単位スコープ
    TreeFolder {
        /// "add" / "remove" / "list"
        action: String,
        /// 追加・削除するフォルダの絶対パス（list 時は省略可）
        path: Option<String>,
        /// 対象タブ ID（省略時は pane の属するタブ）
        tab: Option<u64>,
        /// 呼び出し元ペイン（タブ解決用）
        pane: Option<u64>,
    },
    /// セッションカタログの参照と復元（Issue #112 A）。
    /// `action`:
    /// - "list": カタログ一覧（`role` / `project` で絞り込み、`limit` 件まで）
    /// - "show": `id`（session_id の前方一致可）のメタ + 会話冒頭の抜粋
    /// - "resume": `id` の会話を新しいペインで復元する（該当 cwd でシェルを起動し
    ///   `claude --resume <session_id>` を注入。claude セッションのみ対応）。
    ///   配置は `pane`（省略時は呼び出し元）を `direction`（省略時は右）へ分割、
    ///   `tab` 指定時はそのタブのフォーカスペインの隣
    Sessions {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Direction>,
    },
    /// ペインの平文ターミナルログの参照・設定（Issue #112 B）。
    /// `action`:
    /// - "list": ログファイル一覧（ペイン ID・サイズ・更新時刻）
    /// - "read": ログの末尾 `lines` 行（既定 200）。対象は `pane`（クローズ済み可）
    ///   または `session_id`（カタログ経由でそのセッションの端末ログを引く）
    /// - "status": 有効/無効・上限・保存先の取得
    /// - "set": `enabled` / `max_mb` / `total_max_mb` の変更（設定は永続化）
    Logs {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_mb: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_max_mb: Option<u64>,
    },
}

impl Request {
    /// ログ・計測用の種別名（Issue #113 の dispatch 遅延計測）。
    /// Debug 表記の先頭トークン = variant 名だけを返し、ペイロード（送信テキスト・
    /// パス等）は含めない（conventions: ペイン内容・送信テキストをログに出さない）
    pub fn kind_name(&self) -> String {
        let dbg = format!("{self:?}");
        dbg.split([' ', '{', '(']).next().unwrap_or("?").to_string()
    }
}

/// リクエストエンベロープ。`token` はセッション毎のランダム値（FR-2.3.4）。
/// `origin` は生成主体の自己申告（`"mcp"` = MCP 経由。省略時は CLI）。
/// トークンを持つプロセスは信頼済みのため、これは UI 表示・ポリシー用のラベルであって
/// セキュリティ境界ではない
#[derive(Debug, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub jsonrpc: String,
    pub id: u64,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(flatten)]
    pub request: Request,
}

impl RequestEnvelope {
    pub fn new(id: u64, token: impl Into<String>, request: Request) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            token: token.into(),
            origin: None,
            request,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// レスポンスエンベロープ。`result` / `error` のどちらか一方を持つ
#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl ResponseEnvelope {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// ファイルシステム操作の種別（FR-3.12）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOpKind {
    CopyAbsolutePath,
    CopyRelativePath,
    Reveal,
    OpenTerminal,
    Rename,
    CreateFile,
    CreateDir,
    Trash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn リクエストエンベロープが往復できる() {
        let envelope = RequestEnvelope::new(
            7,
            "secret",
            Request::Split {
                pane: Some(3),
                tab: None,
                direction: Some(Direction::Down),
                ratio: None,
                command: Some(vec!["npm".into(), "run".into(), "dev".into()]),
                cwd: None,
                focus: None,
            },
        );
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains(r#""method":"split""#));
        assert!(json.contains(r#""token":"secret""#));
        let back: RequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.request, envelope.request);
    }

    #[test]
    fn paramsなしのlistとデフォルト値が解釈できる() {
        let back: RequestEnvelope =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"token":"t","method":"list"}"#)
                .unwrap();
        assert_eq!(back.request, Request::List);

        // newline はデフォルト true、pane は省略可
        let back: RequestEnvelope = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":2,"token":"t","method":"send","params":{"text":"ls"}}"#,
        )
        .unwrap();
        assert_eq!(
            back.request,
            Request::Send {
                pane: None,
                text: "ls".into(),
                newline: true,
                tmux_session: None,
                await_prompt: false,
            }
        );
    }

    #[test]
    fn レスポンスはresultとerrorを排他で持つ() {
        let ok = serde_json::to_string(&ResponseEnvelope::ok(1, serde_json::json!({"pane": 5})))
            .unwrap();
        assert!(ok.contains(r#""result""#) && !ok.contains(r#""error""#));
        let err =
            serde_json::to_string(&ResponseEnvelope::err(1, error_code::AUTH, "認証失敗")).unwrap();
        assert!(err.contains(r#""error""#) && !err.contains(r#""result""#));
    }

    /// Issue #113: dispatch 遅延計測のログには種別名のみが載り、ペイロード
    /// （送信テキスト・パス等）を含まない（conventions: 内容をログに出さない）
    #[test]
    fn kind_nameは種別名のみでペイロードを含まない() {
        let send = Request::Send {
            pane: Some(1),
            text: "secret-prompt-text".into(),
            newline: true,
            tmux_session: None,
            await_prompt: false,
        };
        assert_eq!(send.kind_name(), "Send");
        // フィールド無し variant / struct variant の両形
        assert_eq!(Request::List.kind_name(), "List");
        assert_eq!(Request::SetupChanges.kind_name(), "SetupChanges");
        assert_eq!(Request::TmuxList { socket: None }.kind_name(), "TmuxList");
    }
}
