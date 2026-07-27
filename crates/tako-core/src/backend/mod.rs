//! backend — バックグラウンド永続バックエンドの抽象境界（B2。Issue #518 / #519）
//!
//! 設計の正は `.agent/plans/2026-07-windows-persistence-backend.md`。要旨:
//!
//! tmux は tako にとって**独立した 2 つの役割**を担っている。
//!
//! - **役割 A（生存の器）**: シェルの PTY を tako プロセスの外に置き、tako が死んでも
//!   実行中プロセスと画面内容を保つ。実体は `wrap_spawn`（`SpawnOptions` の書き換え）
//!   ただ 1 つで、PTY を所有するのは相変わらず in-process の `TerminalSession` である
//! - **役割 B（アウトオブプロセス到達）**: tako-app が動いていない / ペインが消えた状態で、
//!   CLI・daemon・MCP から画面を読み、キーを送り、履歴を採る手段
//!
//! この 2 つを 1 つの trait に混ぜると、呼び出し側が「in-process の主経路」と
//! 「役割 B のフォールバック」を型で区別できない。だから
//! [`SessionBackend`]（役割 A）と [`DetachedAccess`]（役割 B）に分け、
//! 後者は [`SessionBackend::detached`] が `Option` で返す。
//! **`detached()` が `None` を返す = Windows 初期リリースの縮退状態**である。
//!
//! ## なぜ `platform/` ではなく `backend/` なのか
//!
//! 選択はプラットフォームではなく**能力**で決まる。macOS でも tmux が無ければ
//! [`NullBackend`] になり、それは #30 で実装・検証済みの既存経路（Homebrew 配布先の
//! 本番実績）そのものである。`platform/` に置くと `cfg` で選ぶ誤解を生む。
//!
//! ## 実装の段取り（#519）
//!
//! 本モジュールは段取り ①②（骨格 + `TmuxBackend` + `TAKO_BACKEND` + `NullBackend`）の成果物。
//! 呼び出し側（`dispatch` / UI / CLI / orchestrator）の移行は ③④⑤ で行う。
//! それまで既存の `crate::tmux_backend` / `crate::tmux` の自由関数は現役のまま残り、
//! [`TmuxBackend`] はそれらへ委譲する薄いアダプタとして振る舞う（**挙動不変**）。

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use crate::terminal::SpawnOptions;

mod null;
mod tmux;

pub use null::NullBackend;
pub use tmux::TmuxBackend;

/// バックエンドセッション名の接頭辞。
///
/// **命名ポリシーは呼び出し側の責務**（`reserve` の候補名を作るのは呼び出し側。
/// 現行の払い出しは `tako-control::generate_token` の CSPRNG に依存しており、
/// tako-core へ持ち込むと依存が増える）。ただし接頭辞そのものは
/// 「tako が作った器か」の目印として器の実装側（orphan 判定・シェル統合スクリプト）も
/// 見るため、**器の実装ではなく境界が持つ**。tmux 固有の値ではない。
pub const SESSION_PREFIX: &str = "tako-";

/// バックエンドセッションの参照。
///
/// **文字列の直渡しを禁止するための newtype**。#428 は tmux の**ターゲット式**
/// （`session:0.0`）をセッション**名**を期待する経路へ渡し、`=session:0.0:` という
/// 解決不能なターゲットになって無音で失敗した実機バグである。
/// [`SessionRef::new`] がターゲット式を拒否するので、同種の取り違えは構造的に起きない。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionRef(String);

impl SessionRef {
    /// セッション**名**から作る。ターゲット式・空文字・制御文字は拒否する。
    ///
    /// `:` を拒む理由は #428 そのもの（tmux もセッション名に `:` を許さない。
    /// ターゲット式 `session:window.pane` の区切りだから）。
    pub fn new(name: impl Into<String>) -> Result<Self, BackendError> {
        let name = name.into();
        if name.is_empty() {
            return Err(BackendError::InvalidSession {
                name,
                reason: "セッション名が空",
            });
        }
        if name.contains(':') {
            return Err(BackendError::InvalidSession {
                name,
                reason: "tmux のターゲット式（session:window.pane）はセッション名として使えない",
            });
        }
        if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(BackendError::InvalidSession {
                name,
                reason: "セッション名に空白・制御文字は使えない",
            });
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for SessionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// スクロールバックの権威がどちらにあるか。
/// `Backend` なら履歴は器の側にあり、ローカルミラー（`scroll_mirror`）で読む。
/// `InProcess` なら alacritty が履歴を持ち、直接ペイン経路（#159）がそのまま効く
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbackAuthority {
    Backend,
    InProcess,
}

/// バックエンドの能力。
///
/// **bool の集合であって `enum Backend { Tmux, None }` ではない**のが重要。
/// 実装名で分岐すると、将来の中間実装（案 B-1 = 器だけ持つ ConPTY セッションホスト。
/// `survives_app_exit = true` / `detached_access = false`）を足したときに
/// 全呼び出し側の変更になる。設計の合格条件は
/// 「B-1 を足したとき呼び出し側の変更が 0 行で済むこと」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// tako 終了後もセッション内のプロセスが生き残るか（役割 A）
    pub survives_app_exit: bool,
    /// tako-app 不在 / ペイン消失時に画面採取・入力送出ができるか（役割 B）
    pub detached_access: bool,
    /// スクロールバックの権威
    pub scrollback: ScrollbackAuthority,
    /// UI・診断・system prompt に出す名前
    pub label: &'static str,
}

impl BackendCapabilities {
    /// 完全復元ができるか（persist の UI 文言・マトリクスの分類に使う）。
    ///
    /// **器があること = 実行中プロセスと画面内容が tako の再起動を生き延びること**。
    /// 器が無くても構成のみ永続化は動く（#30 で実装・検証済みの経路）ので、
    /// 「保存するか」の判断にこれを使ってはいけない（保存のゲートは persist 設定だけ）
    pub fn full_restore(&self) -> bool {
        self.survives_app_exit
    }

    /// 縮退の説明。UI・診断・エラー・system prompt がこの 1 文を共有する
    pub fn degraded_note(&self) -> Option<String> {
        if self.survives_app_exit {
            return None;
        }
        Some(format!(
            "永続バックエンド（{}）に器が無いため、タブ・ペイン構成と cwd は復元するが、\
             実行中プロセスと画面内容は tako の終了時に失われる",
            self.label
        ))
    }

    /// 診断・API 応答用の構造化表現（`tako persist` / MCP が返す）
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "label": self.label,
            "survives_app_exit": self.survives_app_exit,
            "detached_access": self.detached_access,
            "scrollback": match self.scrollback {
                ScrollbackAuthority::Backend => "backend",
                ScrollbackAuthority::InProcess => "in_process",
            },
            "note": self.degraded_note(),
        })
    }
}

/// プロセス全体の backend の能力（`backend().capabilities()` の短縮形）。
///
/// **呼び出し側は「tmux があるか」ではなく「何ができるか」を尋ねる**。
/// この向きにしておくと、案 B-1（器あり・到達なし）が入ったときに
/// 呼び出し側を書き換えずに済む（設計 §3.6 の合格条件）
pub fn capabilities() -> BackendCapabilities {
    backend().capabilities()
}

/// 器を握っている外部クライアント（#177 の復元強奪ガードの材料）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    /// クライアントプロセスの PID。所有インスタンスの特定は呼び出し側が祖先辿りで行う
    pub pid: u32,
    pub session: SessionRef,
}

/// 器の一覧に載る 1 セッション
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub session: SessionRef,
    pub attached: bool,
    /// 最終アクティビティ（unix epoch 秒）。cleanup の猶予判定（#113）に使う
    pub last_activity: i64,
}

/// 履歴の観測結果（pane_log / スクロールミラーが使う）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryProbe {
    pub history: usize,
    pub limit: usize,
    pub bytes: u64,
    /// 内側アプリが alt screen（TUI 実行中）か
    pub alternate: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// この backend には該当の能力が無い。`note` はサポートマトリクス由来の文字列を入れる
    /// （UI・エラー・system prompt で二重管理しない。architecture.md T5「診断一致」）
    #[error("{note}")]
    Unsupported { note: String },
    #[error("セッション名として不正（{name}）: {reason}")]
    InvalidSession { name: String, reason: &'static str },
    #[error("{0}")]
    Operation(String),
}

/// 役割 A: 生存の器
pub trait SessionBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    /// ペインへ器を割り当てるか決める。器を持たない実装は `None` を返す。
    ///
    /// `candidate` は呼び出し側が払い出した候補名。**命名ポリシーを backend へ移さない**のは、
    /// 現行の命名が CSPRNG（`tako-control::generate_token`）に依存しており、
    /// tako-core へ持ち込むと依存が増えるため（#177 の「乱数ベースなので多重起動・
    /// PID 再利用でも過去の残骸と衝突しない」性質はそのまま維持する）。
    fn reserve(&self, candidate: &str) -> Option<SessionRef>;

    /// spawn を器の中で起動する形へ書き換える。器が無い実装は**恒等変換**。
    /// PTY を所有するのは呼び出し側の `TerminalSession::spawn` のままである点に注意
    fn wrap_spawn(&self, options: SpawnOptions, session: &SessionRef) -> SpawnOptions;

    fn exists(&self, session: &SessionRef) -> bool;

    fn kill(&self, session: &SessionRef) -> Result<(), BackendError>;

    fn list(&self) -> Vec<SessionInfo>;

    /// 指定セッション群を握っている外部クライアント（#177 復元強奪ガード）
    fn foreign_holders(&self, sessions: &[SessionRef]) -> Vec<Holder>;

    /// `protected` に無い残骸セッション（#191 の起動時 orphan 復帰が使う。kill はしない）
    fn orphans(&self, protected: &HashSet<SessionRef>) -> Vec<SessionRef>;

    /// 残骸セッションを kill して、kill した分を返す（FR-2.16.11）。
    /// `min_idle` を渡すとそれより新しいアクティビティのセッションは残す（#113 の猶予）
    fn cleanup_orphans(
        &self,
        protected: &HashSet<SessionRef>,
        min_idle: Option<Duration>,
    ) -> Vec<SessionRef>;

    /// 器の中のペインの制御端末。listen ポート検知（FR-2.4.2）の突き合わせに使う
    fn pane_tty(&self, session: &SessionRef) -> Option<String>;

    /// 器の中の現在の作業ディレクトリ。orphan 復帰（#191）が復元ペインの cwd に使う
    fn session_cwd(&self, session: &SessionRef) -> Option<String>;

    fn session_env(&self, session: &SessionRef, name: &str) -> Option<String>;

    fn set_session_env(&self, session: &SessionRef, name: &str, value: &str);

    /// 稼働中の器へ最新設定を再適用する（器が tako の再起動を生き残るため必要）
    fn sync_config(&self) {}

    /// 役割 B の入口。**持たない実装は `None`**
    fn detached(&self) -> Option<&dyn DetachedAccess> {
        None
    }
}

/// 役割 B: アウトオブプロセス到達。
///
/// **これを持たない = tako-app が居ないと何も読めない・送れない**。
/// Windows 初期リリース（`NullBackend`）はこれを持たない。
///
/// 現時点で載せているのは、tako-core の既存関数へ正直に委譲できるものだけ。
/// スクロールのホイール転送（`scroll_mirror::send_wheel`）はネスト tmux の
/// ターゲット解決（#181）という別の抽象を必要とし、ペイン PID 列挙・子プロセス判定は
/// tako-control 側にあるため、呼び出し側の移行（段取り ③）と同時に加える。
pub trait DetachedAccess: Send + Sync {
    /// 可視画面の採取
    fn capture_screen(&self, session: &SessionRef) -> Result<Vec<String>, BackendError>;

    /// 履歴末尾 `lines` 行の平文採取。**折り返し行はそのまま**
    /// （`#{history_size}` の行数カウントと 1:1 で対応する。pane_log が使う）
    fn capture_history(&self, session: &SessionRef, lines: usize) -> Option<Vec<String>>;

    /// 履歴末尾 `lines` 行を**折り返し結合して** 1 本のテキストで返す。
    /// 人間・エージェントが読む報告（`orchestrator report` 第 1 層）が使う。
    /// [`capture_history`] とは折り返しの扱いが違うので混同しないこと
    fn capture_history_joined(&self, session: &SessionRef, lines: usize) -> Option<String>;

    fn history_probe(&self, session: &SessionRef) -> Option<HistoryProbe>;

    /// 全セッションの履歴観測を 1 コマンドで（#369 の probe 一括化）
    fn history_probe_batch(&self) -> Vec<(SessionRef, HistoryProbe)>;

    /// テキスト送出（改行は呼び出し側で正規化済みの前提）
    fn send_text(&self, session: &SessionRef, text: &str) -> Result<(), BackendError>;

    /// キー名（`Enter` / `Down` 等）での送出
    fn send_key(&self, session: &SessionRef, key: &str) -> Result<(), BackendError>;

    /// bracketed paste での貼り付け
    fn paste(&self, session: &SessionRef, text: &str) -> Result<(), BackendError>;

    /// 器の window を明示サイズへ固定する（`cols` / `rows` は既存 API に合わせて u32）
    fn resize_window(
        &self,
        session: &SessionRef,
        window: u32,
        cols: u32,
        rows: u32,
    ) -> Result<(), BackendError>;
}

// --- 本番 spawn 経路の配線（#519 M1） --------------------------------------

/// ペインへ器を割り当てる。**本番の spawn 経路と、dispatch の事前予約が共有する 1 箇所**。
///
/// 問いを「tmux があるか」でも「`survives_app_exit` か」でもなく
/// **「この backend はこのペインに器を配るか」**にしてある。器の有無は実装が
/// [`SessionBackend::reserve`] で答えるので、案 B-1（器あり・到達なし）を足したときに
/// ここも呼び出し側も変更が要らない（設計 §3.6 の合格条件）。
///
/// - `persist`: 永続設定（ユーザーが OFF にしていれば器は配らない）
/// - `existing`: 既に割り当て済みの名前（復元・再 spawn）。**あればそれを使う**
/// - `candidate`: 新規払い出しの候補名。`existing` があるときは呼ばない
pub fn reserve_for_pane(
    backend: &dyn SessionBackend,
    persist: bool,
    existing: Option<&str>,
    candidate: impl FnOnce() -> String,
) -> Option<SessionRef> {
    if !persist {
        return None;
    }
    match existing {
        Some(name) => backend.reserve(name),
        None => backend.reserve(&candidate()),
    }
}

/// ペインの spawn を器の中で起動する形へ書き換える（[`reserve_for_pane`] + [`SessionBackend::wrap_spawn`]）。
///
/// 返り値の `Option<SessionRef>` が `None` = 器なし = 呼び出し側は
/// そのペインを直接ペインとして扱う（`SpawnOptions` は素通し）。
/// **PTY を所有するのは呼び出し側の `TerminalSession::spawn` のまま**である点に注意
pub fn wrap_spawn_for_pane(
    backend: &dyn SessionBackend,
    persist: bool,
    existing: Option<&str>,
    candidate: impl FnOnce() -> String,
    options: SpawnOptions,
) -> (SpawnOptions, Option<SessionRef>) {
    match reserve_for_pane(backend, persist, existing, candidate) {
        Some(session) => {
            let wrapped = backend.wrap_spawn(options, &session);
            (wrapped, Some(session))
        }
        None => (options, None),
    }
}

// --- 実装の選択 -----------------------------------------------------------

/// どの backend を使うか。**プラットフォームではなく能力で決まる**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Tmux,
    None,
}

/// `TAKO_BACKEND` の値（`auto` / `tmux` / `none`）。既定は `auto`。
///
/// **`none` は Windows の縮退経路を macOS 上で実行するための鍵**（設計 §8.2 の R0）。
/// `TAKO_PERSIST=0` は保存ごと止めてしまい（`main.rs` の `save_layout`）、
/// Windows の「保存する・復元は構造のみ」とは別物になるため、縮退の再現には使えない。
pub const ENV_BACKEND: &str = "TAKO_BACKEND";

/// 選択の解決（プロセス内で 1 回だけ。`available()` の従来のキャッシュ挙動を引き継ぐ）
pub fn choice() -> Choice {
    static CHOICE: OnceLock<Choice> = OnceLock::new();
    *CHOICE.get_or_init(|| resolve_choice(std::env::var(ENV_BACKEND).ok().as_deref()))
}

/// 純粋関数として切り出した解決ロジック（macOS 上で全分岐をテストできるようにするため）。
/// 未知の値は `auto` と同じに倒す（環境変数のタイポでユーザーの永続化を壊さない）
fn resolve_choice(env: Option<&str>) -> Choice {
    match env.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("none" | "null" | "off") => Choice::None,
        // 明示 tmux でも、tmux が無ければ器は作れない（嘘の能力を申告しない）
        Some("tmux") => tmux_or_none(),
        _ => tmux_or_none(),
    }
}

fn tmux_or_none() -> Choice {
    if crate::tmux_backend::tmux_binary_present() {
        Choice::Tmux
    } else {
        Choice::None
    }
}

/// プロセス全体で共有する backend。
pub fn backend() -> &'static dyn SessionBackend {
    static BACKEND: OnceLock<Box<dyn SessionBackend>> = OnceLock::new();
    BACKEND
        .get_or_init(|| match choice() {
            Choice::Tmux => Box::new(TmuxBackend::new()) as Box<dyn SessionBackend>,
            Choice::None => Box::new(NullBackend) as Box<dyn SessionBackend>,
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn セッション参照はターゲット式を拒否する() {
        // #428 の回帰: "session:0.0" をセッション名として渡すと
        // `=session:0.0:` になり can't find pane で無音失敗していた
        let err = SessionRef::new("tako-abc:0.0").unwrap_err();
        assert!(matches!(err, BackendError::InvalidSession { .. }));
        assert!(err.to_string().contains("ターゲット式"));

        assert!(SessionRef::new("").is_err());
        assert!(SessionRef::new("tako abc").is_err());
        assert!(SessionRef::new("tako-\u{7}abc").is_err());

        let ok = SessionRef::new("tako-0123456789ab").unwrap();
        assert_eq!(ok.as_str(), "tako-0123456789ab");
        assert_eq!(ok.to_string(), "tako-0123456789ab");
    }

    #[test]
    fn 環境変数noneはtmuxの有無によらず器なしへ倒れる() {
        assert_eq!(resolve_choice(Some("none")), Choice::None);
        assert_eq!(resolve_choice(Some("NONE")), Choice::None);
        assert_eq!(resolve_choice(Some(" none ")), Choice::None);
        assert_eq!(resolve_choice(Some("null")), Choice::None);
        assert_eq!(resolve_choice(Some("off")), Choice::None);
    }

    #[test]
    fn 未知の値と未設定はautoと同じ扱いになる() {
        // タイポで永続化が黙って落ちるのが最悪なので auto へ倒す
        let auto = resolve_choice(None);
        assert_eq!(resolve_choice(Some("tmuxx")), auto);
        assert_eq!(resolve_choice(Some("")), auto);
        // 明示 tmux も「tmux が無ければ None」= auto と同じ結果になる
        assert_eq!(resolve_choice(Some("tmux")), auto);
    }

    #[test]
    fn 器が無いときだけ縮退の説明が出る() {
        let with_container = BackendCapabilities {
            survives_app_exit: true,
            detached_access: true,
            scrollback: ScrollbackAuthority::Backend,
            label: "tmux",
        };
        assert!(with_container.degraded_note().is_none());
        assert!(with_container.full_restore());

        let without = BackendCapabilities {
            survives_app_exit: false,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            label: "none",
        };
        let note = without.degraded_note().expect("縮退の説明が要る");
        assert!(note.contains("none"), "note={note}");
        // 「構成は戻る / 画面は失われる」の両方を言う（片方だけだと誤解を生む）
        assert!(note.contains("構成"), "note={note}");
        assert!(note.contains("失われる"), "note={note}");
    }

    #[test]
    fn describeは能力をそのまま構造化して返す() {
        let caps = BackendCapabilities {
            survives_app_exit: false,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            label: "none",
        };
        let v = caps.describe();
        assert_eq!(v["label"], "none");
        assert_eq!(v["survives_app_exit"], false);
        assert_eq!(v["detached_access"], false);
        assert_eq!(v["scrollback"], "in_process");
        assert!(v["note"].is_string(), "縮退時は note が入る");

        let tmux = BackendCapabilities {
            survives_app_exit: true,
            detached_access: true,
            scrollback: ScrollbackAuthority::Backend,
            label: "tmux",
        };
        assert_eq!(tmux.describe()["scrollback"], "backend");
        assert!(
            tmux.describe()["note"].is_null(),
            "縮退していなければ note は無い"
        );
    }

    /// 案 B-1（器だけの ConPTY セッションホスト）の形をした偽 backend。
    /// **tmux ではない器**がこの境界に嵌まることを検証するための最小実装で、
    /// 呼ばれた `wrap_spawn` のセッション名を記録する
    struct FakeSessionHost {
        wrapped: std::sync::Mutex<Vec<String>>,
    }

    impl FakeSessionHost {
        fn new() -> Self {
            Self {
                wrapped: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl SessionBackend for FakeSessionHost {
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities {
                survives_app_exit: true,
                detached_access: false,
                scrollback: ScrollbackAuthority::InProcess,
                label: "session-host",
            }
        }
        fn reserve(&self, candidate: &str) -> Option<SessionRef> {
            SessionRef::new(candidate).ok()
        }
        fn wrap_spawn(&self, mut options: SpawnOptions, session: &SessionRef) -> SpawnOptions {
            self.wrapped
                .lock()
                .unwrap()
                .push(session.as_str().to_string());
            // 器の中で起動する形（B-1 なら session-host クライアントの起動）へ書き換える
            options.command = Some(crate::terminal::SpawnCommand {
                program: "tako-session-host".into(),
                args: vec!["--attach".into(), session.as_str().to_string()],
            });
            options
        }
        fn exists(&self, _session: &SessionRef) -> bool {
            true
        }
        fn kill(&self, _session: &SessionRef) -> Result<(), BackendError> {
            Ok(())
        }
        fn list(&self) -> Vec<SessionInfo> {
            Vec::new()
        }
        fn foreign_holders(&self, _sessions: &[SessionRef]) -> Vec<Holder> {
            Vec::new()
        }
        fn orphans(&self, _protected: &HashSet<SessionRef>) -> Vec<SessionRef> {
            Vec::new()
        }
        fn cleanup_orphans(
            &self,
            _protected: &HashSet<SessionRef>,
            _min_idle: Option<Duration>,
        ) -> Vec<SessionRef> {
            Vec::new()
        }
        fn pane_tty(&self, _session: &SessionRef) -> Option<String> {
            None
        }
        fn session_cwd(&self, _session: &SessionRef) -> Option<String> {
            None
        }
        fn session_env(&self, _session: &SessionRef, _name: &str) -> Option<String> {
            None
        }
        fn set_session_env(&self, _session: &SessionRef, _name: &str, _value: &str) {}
    }

    /// **M1 の合格条件（設計 §3.6）**: 本番の spawn 経路が実装名ではなく境界を見ていること。
    ///
    /// tmux ではない器（B-1 形）を渡すと、その `wrap_spawn` が実際に呼ばれ、
    /// 器の中で起動する形へ書き換えられる。**この関数を本番の `spawn_session` が呼ぶ**ので、
    /// B-1 を足したときの呼び出し側の変更は 0 行になる
    #[test]
    fn spawn配線はtmux以外の器でも器の中で起動する() {
        let host = FakeSessionHost::new();
        let options = SpawnOptions {
            command: None,
            cwd: Some(std::path::PathBuf::from("/tmp/work")),
            env: vec![("TAKO_PANE_ID".into(), "7".into())],
        };
        let (wrapped, session) = wrap_spawn_for_pane(
            &host,
            true,
            None,
            || "tako-0123456789ab".to_string(),
            options.clone(),
        );
        let session = session.expect("器を持つ backend は器を配る");
        assert_eq!(session.as_str(), "tako-0123456789ab");
        let cmd = wrapped.command.expect("器の中で起動する形へ書き換わる");
        assert_eq!(cmd.program, "tako-session-host");
        assert_eq!(*host.wrapped.lock().unwrap(), vec!["tako-0123456789ab"]);
        // env / cwd は素通しで in-process の PTY へ渡る（orchestrator の env 注入が縮退しない）
        assert_eq!(wrapped.env, options.env);
        assert_eq!(wrapped.cwd, options.cwd);
    }

    #[test]
    fn 既存の器がある再spawnは同じセッションを使い候補名を払い出さない() {
        // 復元・再 spawn。ここで新しい名前を払い出すと、生きている器を取り残して
        // 別の器を作る（= 実行中プロセスの置き去り）ことになる
        let host = FakeSessionHost::new();
        let (_, session) = wrap_spawn_for_pane(
            &host,
            true,
            Some("tako-ffffffffffff"),
            || panic!("既存名があるのに候補名を払い出した"),
            SpawnOptions::default(),
        );
        assert_eq!(session.unwrap().as_str(), "tako-ffffffffffff");
    }

    #[test]
    fn persist_offと器なしはどちらも直接ペインになる() {
        let opts = SpawnOptions {
            command: Some(crate::terminal::SpawnCommand {
                program: "/bin/zsh".into(),
                args: vec!["-l".into()],
            }),
            cwd: None,
            env: vec![],
        };

        // persist OFF: 器を持つ backend でも器は配らない（ユーザー設定が最優先）
        let host = FakeSessionHost::new();
        let (passthrough, session) = wrap_spawn_for_pane(
            &host,
            false,
            None,
            || "tako-0123456789ab".to_string(),
            opts.clone(),
        );
        assert!(session.is_none());
        assert_eq!(passthrough.command.unwrap().program, "/bin/zsh");
        assert!(host.wrapped.lock().unwrap().is_empty());

        // 器なし backend: persist ON でも直接ペイン（#30 の「構造のみ永続化」経路）
        let (passthrough, session) = wrap_spawn_for_pane(
            &NullBackend,
            true,
            None,
            || "tako-0123456789ab".to_string(),
            opts.clone(),
        );
        assert!(session.is_none());
        assert_eq!(passthrough.command.unwrap().program, "/bin/zsh");
    }

    #[test]
    fn 壊れた既存名は器として採用しない() {
        // layout.json 由来の名前がターゲット式に化けていても（#428）、
        // reserve が弾いて直接ペインへ倒れる（無音で別セッションを掴まない）
        let host = FakeSessionHost::new();
        let session = reserve_for_pane(&host, true, Some("tako-abc:0.0"), || {
            "tako-0123456789ab".to_string()
        });
        assert!(session.is_none());
    }

    #[test]
    fn 能力は実装名ではなくbool集合で表現される() {
        // 案 B-1（器あり・到達なし）が中間状態として表現できることの確認。
        // これが表現できない trait は切り方を間違えている（設計 §3.6）
        let b1 = BackendCapabilities {
            survives_app_exit: true,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            label: "session-host",
        };
        assert!(b1.full_restore());
        assert!(!b1.detached_access);
    }
}
