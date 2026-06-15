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
}

impl PreviewModeWire {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewModeWire::Code => "code",
            PreviewModeWire::Markdown => "markdown",
            PreviewModeWire::Image => "image",
            PreviewModeWire::Pdf => "pdf",
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
    },
    /// ペイン削除（FR-2.5.4。呼び出し元自身の削除 = 自己片付けを含む）
    Close { pane: Option<u64> },
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
    /// ペインへのテキスト送信（FR-2.2.2）。`newline` で末尾に改行（CR）を付与
    Send {
        pane: Option<u64>,
        text: String,
        #[serde(default = "default_true")]
        newline: bool,
    },
    /// ペインの画面内容取得（FR-2.2.5）。`lines` は末尾からの行数制限
    Read {
        pane: Option<u64>,
        lines: Option<usize>,
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
    /// タブ配下の**バックグラウンド項目（裏で実行中のペイン行 + 退避）を隠し、前面表示中の
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
    /// ペインをたまり場へ退避する（FR-2.15.1）。プロセスは生きたまま画面から外す
    Shelve { pane: Option<u64> },
    /// たまり場からペインを復帰させる（FR-2.15.3 / FR-2.15.4）。`target` を
    /// `direction`（省略時は右）へ分割した位置に挿し直す。`target` 省略時は
    /// アクティブタブのフォーカス中ペインの隣
    Unshelve {
        pane: u64,
        target: Option<u64>,
        direction: Option<Direction>,
    },
    /// たまり場の一覧取得。shelved ペインの ID / title / role / 状態を返す
    ShelvedList,
    /// たまり場のペインを kill する（FR-2.15.2）。確認は呼び出し側の責務
    ShelvedKill { pane: u64 },
    /// 環境の健全性診断。CLI の PATH / バージョン一致 / 外部ツールの有無等をチェックする
    CheckHealth,
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
                newline: true
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
}
