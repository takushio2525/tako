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
mod owner;
/// psmux 実装。**ここだけ `pub`** なのは、器の適合（conf の受理・起動時プローブ）を
/// 実バイナリで確かめる統合テスト（`tests/psmux_backend.rs`）が自由関数を呼ぶため
pub mod psmux;
mod tmux;

pub use null::NullBackend;
pub use psmux::PsmuxBackend;
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

/// **UI のスクロール表示**をどちらの履歴で描くか。
///
/// `Backend` = 器の履歴をローカルへ写して描く（`scroll_mirror`。#159）。
/// `InProcess` = 外側 alacritty が持つ履歴をそのまま描く（直接ペイン経路）。
///
/// **「器が履歴を持っているか」ではない**点に注意。ミラー経路は器へ 2 つの前提を置く
/// （`#{mouse_any_flag}` で内側アプリのマウス要求を答えられる /
/// `send-keys -H` でホイール報告をバイト列として注入できる）。psmux は履歴を持つが
/// この 2 つを持たないため `InProcess` を申告する（#654）。
/// 器の履歴を**採取できるか**は [`BackendCapabilities::detached_capture`] が答える
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbackAuthority {
    Backend,
    InProcess,
}

/// 器のセッション作成時（`new-session -e K=V`）に値を確定させる**ペイン固有**の環境変数。
///
/// 器のサーバーのグローバル環境は最初のクライアントから継承され、後続セッションも
/// その stale な値を使う。ペインごとに違う値はここに載せて `-e` で渡さないと、
/// **別のペインの値が見える**（#210 で実害を踏んだ）。
///
/// 器の実装（tmux / psmux）が同じ表を引くので、キーを増やすときの編集は 1 箇所で済む
/// （片方だけ足すと「tmux では効くが psmux では効かない」という追いにくい差になる）
pub const PANE_SCOPED_ENV: &[&str] = &[
    "TAKO_PANE_ID",
    "TAKO_TAB_ID",
    // #766: 器が OSC を素通ししないときのシェル統合の書き先（`osc_sink::SINK_ENV`）
    crate::osc_sink::SINK_ENV,
];

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
    /// tako-app 不在 / ペイン消失時に**画面・履歴を採取**できるか（役割 B の読み側）。
    /// psmux はここだけ `true`（`capture-pane` は動くが送出系は信頼できない。#519）
    pub detached_capture: bool,
    /// tako-app 不在 / ペイン消失時に**入力を送出**できるか（役割 B の書き側）。
    /// `detached_capture` を含意する（送れる器は読めもする）
    pub detached_access: bool,
    /// スクロールバックの権威
    pub scrollback: ScrollbackAuthority,
    /// シェルが出す OSC（7 = cwd / 133 = コマンド状態）が**外側の tako まで届くか**
    /// （シェル統合 = FR-2.4.1 が成立する条件。#525）。
    ///
    /// tmux は `allow-passthrough on` + DCS で包めば通る。psmux 3.3.7 は
    /// **オプションはあるが素通しされない**（実測: 素の OSC / DCS の ESC 二重化あり /
    /// 二重化なし のいずれも外へ出ず、同時に流した平文だけが届いた）。
    /// 器なしはそもそも間に何も挟まらないので true
    pub osc_passthrough: bool,
    /// 器へ渡す**内側コマンドの第 1 語（プログラム）を引用符で括れるか**（#881）。
    ///
    /// tmux は内側コマンドを `sh -c` の意味論で解釈するので、空白入りのパスを
    /// `'…'` で括れば正しく起動する。**psmux は括れない**: 単語分割の過程で
    /// 引用符ごと落として `CreateProcess` へ渡すため、`'C:\Program Files\…'` が
    /// そのまま「そんなプログラムは無い」になり、器が既定シェルへ丸投げして死ぬ
    /// （実測 2026-08-21・psmux 3.3.7）。false の器へは
    /// `platform::program_path::single_token` で空白の無い表記へ落として渡す
    pub quotes_program: bool,
    /// **器の client の打鍵経路が ASCII しか運べないか**（#907）。
    ///
    /// tako は器つきペインへも「外側 PTY へ書く」= 器の client の打鍵として
    /// テキストを送っている。psmux はこの経路で **cp932 に無い文字を落とす**
    /// （実機実測: `テスト─❯` を送ると `テスト` だけが届き `─`（U+2500）と
    /// `❯`（U+276F）が消える。器なしの同じ経路はバイト等価）。
    /// 器自身の注入口（`send-keys -l` / `paste-buffer`）は UTF-8 をそのまま運ぶので、
    /// true の器へは打鍵ではなく [`inject_text`] で入れる。
    /// tmux は打鍵経路がバイト等価なので false（macOS の経路は据え置き）
    pub keystrokes_ascii_only: bool,
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
            "detached_capture": self.detached_capture,
            "detached_access": self.detached_access,
            "scrollback": match self.scrollback {
                ScrollbackAuthority::Backend => "backend",
                ScrollbackAuthority::InProcess => "in_process",
            },
            "osc_passthrough": self.osc_passthrough,
            "note": self.degraded_note(),
        })
    }
}

/// プロセス全体の backend の能力（`backend().capabilities()` の短縮形）。
///
/// **呼び出し側は「tmux があるか」ではなく「何ができるか」を尋ねる**。
/// この向きにしておくと、案 B-1（器あり・到達なし）が入ったときに
/// 呼び出し側を書き換えずに済む（設計 §3.6 の合格条件）
/// **打鍵ではなく器の注入口へ入れるべきテキストか**（#907。純粋関数）。
///
/// 器の client が ASCII しか運べない（`keystrokes_ascii_only`）のに
/// 非 ASCII を含むときだけ true。ASCII だけのテキストは従来どおり打鍵で送る
/// （経路を増やさないほうが挙動差が出ない。Enter・制御キーも同じ理由で打鍵のまま）
pub fn needs_text_injection(caps: &BackendCapabilities, text: &str) -> bool {
    caps.keystrokes_ascii_only && !text.is_ascii()
}

/// いまの器の注入口へテキストを入れる（#907）。器が無い / 対応していないなら `Err`
pub fn inject_text(session: &str, text: &str) -> Result<(), BackendError> {
    let session = SessionRef::new(session)?;
    backend().inject_text(&session, text)
}

pub fn capabilities() -> BackendCapabilities {
    backend().capabilities()
}

/// [`Holder::pid`] が何の PID なのか。**器によって観測できるものが違う**ため、
/// 「所有インスタンスが生きているか」を呼び出し側がどう確かめるべきかを型で言う
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderKind {
    /// 器のクライアントプロセスの PID（tmux の `#{client_pid}`）。
    /// クライアントは tako-app が spawn した PTY の子なので、
    /// **呼び出し側が祖先を辿って**所有インスタンスを特定する
    Client,
    /// 所有インスタンス（tako-app）そのものの PID。
    /// **生存は器の実装が確認済み**なので、呼び出し側は祖先辿りをしない
    /// （psmux はクライアント PID を観測できず、tako 側のオーナー記録で答えるため。#519 M2）
    Owner,
}

/// 器を握っている他インスタンス（#177 の復元強奪ガードの材料）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    /// PID。意味は [`Holder::kind`] で決まる
    pub pid: u32,
    pub session: SessionRef,
    pub kind: HolderKind,
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
    /// 履歴のバイト数。**器によっては観測できない**（psmux の `#{history_bytes}` は空）。
    /// 観測できない器は 0 を返すので、**変化の検知にだけ使い、絶対値を信じない**
    pub bytes: u64,
    /// 内側アプリが alt screen（TUI 実行中）か
    pub alternate: bool,
}

/// 器が持つ**表示位置**の観測結果（#687。CLI / MCP のスクロールが使う）。
///
/// 器の中でスクロールバックを遡っている状態を外から読む。tako-app の
/// `TerminalSession::display_offset()` は**外側** alacritty の位置なので、
/// 器がスクロールを持つペイン（psmux の attach クライアントは alt screen で
/// 外側に履歴が積まれない）では常に 0 になり、実態を答えられない
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ScrollProbe {
    /// 最下部からの遡り行数（0 = 最下部）。`TerminalSession::display_offset` と同じ向き
    pub position: usize,
    /// 器の履歴行数
    pub history: usize,
    /// 器がスクロールモード（tmux / psmux の copy mode）に居るか
    pub in_mode: bool,
    /// 内側アプリが alt screen（= スクロール位置は内側アプリが所有する）か
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

    /// **器の注入口へテキストを入れる**（#907）。既定は「無い」。
    ///
    /// 打鍵（外側 PTY への書き込み）ではなく器の CLI（`send-keys -l`）を通す経路。
    /// 引数は Windows のコマンドラインとして UTF-16 で渡るので、
    /// **cp932 に無い文字も落ちない**（実機実測: `send-keys -l` と
    /// `load-buffer` + `paste-buffer` はどちらもバイト等価だった）。
    ///
    /// 改行は含めない（Enter は「貼り付けと分離した単独キー」として送るのが
    /// tako の規約 = #95 / #32。ASCII なので打鍵経路でよい）
    fn inject_text(&self, _session: &SessionRef, _text: &str) -> Result<(), BackendError> {
        Err(BackendError::Operation(
            "この器はテキスト注入に対応していない".into(),
        ))
    }

    /// 器の中のペインの制御端末。listen ポート検知（FR-2.4.2）の突き合わせに使う
    fn pane_tty(&self, session: &SessionRef) -> Option<String>;

    /// 器の中で動いているプロセスの PID。
    ///
    /// **器の中のシェルは tako の PTY の子ではない**（器のサーバーが自前に作った
    /// 疑似コンソールの中に居る）ので、`TerminalSession::spawn` が握っている
    /// 「PTY 直下の子」からは辿れない。器の内側のプロセスへ何かを届ける経路
    /// （疑似コンソールのコードページ固定 = 境界 B19。#659）はここから pid を得る。
    /// 器を持たない実装・まだセッションが出来ていない場合は空を返す
    fn pane_pids(&self, _session: &SessionRef) -> Vec<u32> {
        Vec::new()
    }

    /// 器の中の**全**ペインを `(ターゲット ID, pane_pid)` で列挙する（#728）。
    ///
    /// ID は `session:window.pane`。remote API のペイン ID 形式と一致させてあるので、
    /// 呼び出し側は `:` の手前を切って器のセッション名としても使える。
    ///
    /// [`Self::pane_pids`] のセッション単位版に対して、こちらは**器の全体**を 1 回で返す。
    /// 用途は「実プロセス → どのペインか」の逆引き（`claude agents --json` の pid から
    /// 祖先を辿ってペインへ対応付ける経路。#592 / セッションカタログの #112）で、
    /// セッション数ぶんサブプロセスを起こすと 2 秒ポーリングの経路が破綻する。
    ///
    /// 既定実装は空。**器を持つ実装は必ず上書きすること**（`pane_pids` と違い、
    /// ここが空だと「器の中で動いている claude が 1 つも見えない」に化ける）
    fn pane_pids_all(&self) -> Vec<(String, u32)> {
        Vec::new()
    }

    /// 器がそのペインを copy mode（履歴閲覧）に置いているか。
    /// **答えられない器は `None`**（呼び出し側は「分からない」として扱い、
    /// 器へ副作用のある操作をしない）。#686
    fn pane_in_mode(&self, _session: &SessionRef) -> Option<bool> {
        None
    }

    /// copy mode から抜けるために **PTY へ前置する**バイト列（in-band 解除。#686）。
    ///
    /// ソケット経由（`send-keys -X cancel`）ではなく in-band にするのは**順序のため**。
    /// 打鍵は PTY へ書かれるので、解除も同じバイト列に混ぜれば器が必ず先に解除を見る。
    /// ソケット側へ撃つと「解除が届く前に打鍵が copy mode に食われる」競合が残り、
    /// 打鍵経路に器のサブプロセスを同期で挟めば #212 / #168 で排除した UI 停止が戻る。
    /// **解除の要否を知らないまま撃ってはいけない**（copy mode でなければ
    /// このバイト列はそのままシェルへ入力される）
    fn copy_mode_exit_bytes(&self) -> Option<&'static [u8]> {
        None
    }

    /// 器の中の現在の作業ディレクトリ。orphan 復帰（#191）が復元ペインの cwd に使う
    fn session_cwd(&self, session: &SessionRef) -> Option<String>;

    fn session_env(&self, session: &SessionRef, name: &str) -> Option<String>;

    fn set_session_env(&self, session: &SessionRef, name: &str, value: &str);

    /// 稼働中の器へ最新設定を再適用する（器が tako の再起動を生き残るため必要）
    fn sync_config(&self) {}

    /// 役割 B の**読み側**の入口。**持たない実装は `None`**。
    ///
    /// 送出まで持つ器は [`Self::detached`] を実装すればよく、こちらは既定実装が
    /// そこから引き上げる。psmux のように**採取だけできる器**はこちらだけを実装する
    fn detached_capture(&self) -> Option<&dyn DetachedCapture> {
        self.detached().map(|a| a as &dyn DetachedCapture)
    }

    /// 役割 B の**書き側まで**含む入口。**持たない実装は `None`**
    fn detached(&self) -> Option<&dyn DetachedAccess> {
        None
    }
}

/// 役割 B の読み側: アウトオブプロセス**採取**。
///
/// **これを持たない = tako-app が居ないと画面も履歴も読めない**（`NullBackend`）。
/// psmux はここまでは持つ（`capture-pane` / `display-message` は動く）が、
/// 送出系（[`DetachedAccess`]）は持たない。
///
/// **読みと書きを別の trait にしてある**のは、
/// 「採取はできるが送出はできない」を型で表せるようにするため。
/// 1 つの trait にして送出だけ `Unsupported` を返す形にすると、
/// 「`Detached` に解決できた = 送れる」という段取り ③ の不変条件が崩れ、
/// 失敗がランタイムまで落ちる（設計 §3.6 と同じ「能力が違うものは型で分ける」）。
pub trait DetachedCapture: Send + Sync {
    /// 可視画面の採取
    fn capture_screen(&self, session: &SessionRef) -> Result<Vec<String>, BackendError>;

    /// 履歴末尾 `lines` 行の平文採取。**折り返し行はそのまま**
    /// （`#{history_size}` の行数カウントと 1:1 で対応する。pane_log が使う）
    fn capture_history(&self, session: &SessionRef, lines: usize) -> Option<Vec<String>>;

    /// 履歴末尾 `lines` 行を 1 本のテキストで返す。
    /// 人間・エージェントが読む報告（`orchestrator report` 第 1 層）が使う。
    ///
    /// tmux は折り返し行を結合する（`-J`）。**psmux は `-J` を無視する**ので
    /// 折り返しは行のまま残る（中身は失われない）。折り返しの扱いが器で違う点を除けば
    /// [`Self::capture_history`] との差は「1 本のテキストか行の列か」
    fn capture_history_joined(&self, session: &SessionRef, lines: usize) -> Option<String>;

    /// 履歴末尾 `lines` 行 **+ 現画面**の平文採取（`tako remote scrollback` /
    /// `GET /api/panes/:id/scrollback` の「遡って読む」形）。
    ///
    /// [`Self::capture_history`] との違いは**現画面を含む**こと。履歴だけを
    /// `#{history_size}` の行数と 1:1 で数えたいペインログは向こうを使う。
    /// [`Self::capture_history_joined`] との違いは折り返しを結合しない
    /// （`-J` を付けない）ことと、行の列で返すこと。
    ///
    /// **`Option` ではなく `Result`** にしてあるのは、この経路の出口が
    /// 人間の端末（CLI）だからで、「読めなかった」だけでなく**器が何と言ったか**を
    /// そのまま見せる必要がある（#972 の `no server running` はまさにそれだった）
    fn capture_scrollback(
        &self,
        session: &SessionRef,
        lines: usize,
    ) -> Result<Vec<String>, BackendError>;

    fn history_probe(&self, session: &SessionRef) -> Option<HistoryProbe>;

    /// 全セッションの履歴観測を 1 コマンドで（#369 の probe 一括化）
    fn history_probe_batch(&self) -> Vec<(SessionRef, HistoryProbe)>;

    /// 器の中の表示位置（#687）。器がスクロールを持たない実装は `None`
    fn scroll_probe(&self, session: &SessionRef) -> Option<ScrollProbe>;
}

/// 役割 B の書き側まで: アウトオブプロセス**送出**（採取を含む）。
///
/// Windows 初期リリース（`NullBackend` / `PsmuxBackend`）はこれを持たない。
///
/// 現時点で載せているのは、tako-core の既存関数へ正直に委譲できるものだけ。
/// スクロールのホイール転送（`scroll_mirror::send_wheel`）はネスト tmux の
/// ターゲット解決（#181）という別の抽象を必要とし、ペイン PID 列挙・子プロセス判定は
/// tako-control 側にあるため、呼び出し側の移行（段取り ③）と同時に加える。
pub trait DetachedAccess: DetachedCapture {
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
/// 器へ渡す「内側コマンド 1 本」を組み立てる（#881）。
///
/// tmux と psmux で**第 1 語の書き方だけ**が違う。ここを 1 か所にしておかないと、
/// 器を差し替えたときに「引用符が消えて起動できない」形の行が静かに作られる
pub fn inner_command_line(command: &crate::terminal::SpawnCommand) -> String {
    compose_inner_command(command, capabilities().quotes_program)
}

/// [`inner_command_line`] の判断部（純粋関数。**macOS からも両分岐をテストできる**）
pub(crate) fn compose_inner_command(
    command: &crate::terminal::SpawnCommand,
    quotes_program: bool,
) -> String {
    if quotes_program {
        return crate::tmux_backend::shell_quoted(command);
    }
    psmux::inner_command(command)
}

pub fn wrap_spawn_for_pane(
    backend: &'static dyn SessionBackend,
    persist: bool,
    existing: Option<&str>,
    candidate: impl FnOnce() -> String,
    options: SpawnOptions,
) -> (SpawnOptions, Option<SessionRef>) {
    match reserve_for_pane(backend, persist, existing, candidate) {
        Some(session) => {
            let wrapped = backend.wrap_spawn(options, &session);
            // 器の中のシェルは tako の ConPTY の子ではないので、
            // `TerminalSession::spawn` のコードページ固定（B19）が届かない。
            // **器を配るこの 1 箇所**から器の内側にも同じ固定を届ける（#659）
            pin_container_encoding(backend, session.clone());
            (wrapped, Some(session))
        }
        None => (options, None),
    }
}

/// 器の中のペインの pid が見えるようになるまでの上限。
/// `new-session` の完了待ちで、実測（この Windows 機・psmux 3.3.7）は 1 秒未満
const CONTAINER_PID_TIMEOUT: Duration = Duration::from_secs(15);
/// 器への問い合わせ間隔。1 回がサブプロセス起動（実測 30〜50ms）なので細かくしない
const CONTAINER_PID_INTERVAL: Duration = Duration::from_millis(120);

/// 器の中のシェルの疑似コンソールを UTF-8 へ固定する（#659。Windows のみ実体を持つ）。
///
/// `backend=psmux` のとき、tako の ConPTY 直下の子は **psmux クライアント**であって
/// シェルではない。シェルは psmux サーバーが自前に作った別の疑似コンソールの中で動くので、
/// [`crate::platform::console`] の固定を PTY 直下の子へ当てても器の内側には届かない
/// （#655 の対処が psmux ペインに効かなかった理由そのもの）。
///
/// **呼び出しは即座に返る**。器へ pid を尋ねるのはサブプロセス起動なので、
/// 待ちも問い合わせも別スレッドへ逃がす（UI スレッドは止めない）。
/// 固定できなくてもペインは起動する（描画が化けるだけで、動かなくなるよりはよい）
fn pin_container_encoding(backend: &'static dyn SessionBackend, session: SessionRef) {
    // unix は `LC_CTYPE` 注入側で担保済み。ここで器へ問い合わせる意味が無いので
    // **スレッドもサブプロセスも作らない**
    if !crate::platform::console::pin_needed() {
        return;
    }
    std::thread::Builder::new()
        .name("tako-pin-container-cp".into())
        .spawn(move || {
            let deadline = std::time::Instant::now() + CONTAINER_PID_TIMEOUT;
            loop {
                // 新規作成なら器のセッションが出来るまで空。再 attach（復元）なら即座に返る
                // （既に走っているシェルでも固定は効く = 実測。#659）
                let pids = backend.pane_pids(&session);
                if !pids.is_empty() {
                    for pid in pids {
                        crate::platform::console::pin_pane_to_utf8_when_ready(pid);
                    }
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    tracing::debug!("器の中のペイン pid を取得できず: {session}");
                    return;
                }
                std::thread::sleep(CONTAINER_PID_INTERVAL);
            }
        })
        .ok();
}

// --- 実装の選択 -----------------------------------------------------------

/// どの backend を使うか。**プラットフォームではなく能力で決まる**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Tmux,
    /// psmux を器にする（Windows。#519 M2）
    Psmux,
    None,
}

/// `TAKO_BACKEND` の値（`auto` / `tmux` / `psmux` / `none`）。既定は `auto`。
///
/// **`none` は Windows の縮退経路を macOS 上で実行するための鍵**（設計 §8.2 の R0）。
/// `TAKO_PERSIST=0` は保存ごと止めてしまい（`main.rs` の `save_layout`）、
/// Windows の「保存する・復元は構造のみ」とは別物になるため、縮退の再現には使えない。
pub const ENV_BACKEND: &str = "TAKO_BACKEND";

/// psmux 実行ファイルの明示指定（未設定なら PATH 上の `psmux` → `tmux` の順に探す）
pub const ENV_PSMUX_BIN: &str = "TAKO_PSMUX_BIN";

/// 器そのものの実行ファイル名（拡張子なし・小文字）。
///
/// psmux は `psmux.exe` / `pmux.exe` / `tmux.exe` の 3 本を配り、**どの名前で
/// 起動されても内部で `<bin> server …` を子プロセスとして起動する**。
/// つまり器のプロセスは「ペインの PTY 直下の子」の子孫として必ず現れる。
///
/// これらは **tako の配管であってユーザーが動かしたプログラムではない**ので、
/// パッシブ検知（`ports`）が拾ったものは結果から落とす（#724）。
/// 一覧を [`Binary`] の検出結果から作らないのは、tako が `tmux.exe` を起動しても
/// 器が名乗る名前は `psmux.exe` でありうる（実測）ため。
pub const PLUMBING_PROCESS_NAMES: &[&str] = &["psmux", "pmux", "tmux"];

/// プロセス名が器そのものか（`ports` の除外判定。`psmux.exe` / `PSMUX` どちらも真）。
///
/// 比較は「拡張子を落として小文字化」で行う。Windows のプロセス名は
/// `PROCESSENTRY32W.szExeFile` = 実行ファイル名なので、これで一意に決まる
pub fn is_plumbing_process(name: &str) -> bool {
    let stem = name
        .rsplit_once('.')
        .map(|(base, _ext)| base)
        .unwrap_or(name);
    PLUMBING_PROCESS_NAMES
        .iter()
        .any(|known| stem.eq_ignore_ascii_case(known))
}

/// 見つかった「tmux を名乗るバイナリ」の正体。
///
/// **psmux は `psmux.exe` / `pmux.exe` / `tmux.exe` の 3 本を配る**ので、
/// PATH に `tmux` があることは「本物の tmux がある」ことを意味しない。
/// 判別せずに [`Choice::Tmux`] を選ぶと、器は作れるのに `kill-session -t =name` が
/// 効かない（= ペインを閉じるたびに器がリークし 5 秒固まる）**半端に壊れた永続化**になる
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binary {
    /// 本物の tmux
    Tmux { bin: String },
    /// psmux（tmux 互換 CLI の別実装）
    Psmux { bin: String, version: String },
    /// 器になれるバイナリが無い
    Absent,
}

/// 見つかったバイナリ（プロセス内で 1 回だけ解決してキャッシュする）
pub fn binary() -> &'static Binary {
    static BINARY: OnceLock<Binary> = OnceLock::new();
    BINARY.get_or_init(detect_binary)
}

fn detect_binary() -> Binary {
    // 明示指定が最優先（隔離検証・非 PATH 配置）
    if let Some(bin) = std::env::var(ENV_PSMUX_BIN).ok().filter(|s| !s.is_empty()) {
        if let Some(found) = probe_binary(&bin) {
            return found;
        }
    }
    // psmux は専用名でも配られる。tmux より先に見る（`tmux` が psmux の可能性があるため、
    // 先に確定させておくとバージョン取得が 1 回で済む）
    if let Some(found) = probe_binary("psmux") {
        return found;
    }
    // 従来どおりの tmux 解決（PATH → 既知の場所 → ログインシェル）。
    // ここで見つかったものが psmux であることもある（Windows で winget / scoop 導入時）
    probe_binary(crate::tmux::tmux_bin()).unwrap_or(Binary::Absent)
}

/// `<bin> -V` を実行して正体を判別する。実行できなければ `None`
fn probe_binary(bin: &str) -> Option<Binary> {
    let mut command = std::process::Command::new(bin);
    crate::platform::process::no_console_window(&mut command);
    let output = command.arg("-V").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    classify_version_output(&text, bin)
}

/// `-V` の出力から正体を判別する（純関数）。
///
/// psmux は 1 行目で `tmux 3.3.7` を**詐称**し、2 行目に自分の素性を書く:
///
/// ```text
/// tmux 3.3.7
/// psmux 3.3.7 (05cc5d4 2026-07-20)
/// ```
///
/// 本物の tmux は `tmux 3.6` の 1 行だけを返す
fn classify_version_output(output: &str, bin: &str) -> Option<Binary> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("psmux ") {
            return Some(Binary::Psmux {
                bin: bin.to_string(),
                version: rest.split_whitespace().next().unwrap_or("").to_string(),
            });
        }
    }
    output
        .lines()
        .any(|line| line.trim().starts_with("tmux "))
        .then(|| Binary::Tmux {
            bin: bin.to_string(),
        })
}

/// 選択の解決（プロセス内で 1 回だけ。`available()` の従来のキャッシュ挙動を引き継ぐ）
pub fn choice() -> Choice {
    static CHOICE: OnceLock<Choice> = OnceLock::new();
    *CHOICE.get_or_init(|| {
        let env = std::env::var(ENV_BACKEND).ok();
        let decided = decide(env.as_deref(), binary(), cfg!(windows));
        vet(decided, binary())
    })
}

/// 純粋関数として切り出した解決ロジック（**全分岐を macOS 上でもテストできる**ようにするため）。
/// 未知の値は `auto` と同じに倒す（環境変数のタイポでユーザーの永続化を壊さない）
fn decide(env: Option<&str>, binary: &Binary, windows: bool) -> Choice {
    match env.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("none" | "null" | "off") => Choice::None,
        // 明示指定でも、その器が無ければ嘘の能力を申告しない
        Some("tmux") => match binary {
            Binary::Tmux { .. } if !windows => Choice::Tmux,
            _ => Choice::None,
        },
        Some("psmux") => match binary {
            Binary::Psmux { .. } => Choice::Psmux,
            _ => Choice::None,
        },
        _ => match binary {
            Binary::Psmux { .. } => Choice::Psmux,
            // **Windows で本物の tmux を選ばない**: POSIX 版 tmux（MSYS2 / Cygwin）は
            // ネイティブの ConPTY シェルを抱える器にならず、`-f` に渡す Windows パスも
            // 解釈できない。器があるように見えて壊れているより、構成のみ永続化へ倒す
            Binary::Tmux { .. } if !windows => Choice::Tmux,
            _ => Choice::None,
        },
    }
}

/// 未検証バージョンの psmux を **実際に試してから**採用する（#519 M2 要件 6）。
///
/// psmux は直近 30 日で 100+ コミットという速度で動いている。バージョン一致だけを
/// 条件にすると patch が上がった翌日に全ユーザーの永続化が黙って落ち、
/// 無条件に信じると壊れた器を掴む。だから**測って決める**。
fn vet(choice: Choice, binary: &Binary) -> Choice {
    vet_with(choice, binary, psmux::behavior_probe)
}

/// プローブを差し替えられる形（**採否の分岐を実バイナリ無しでテストするため**）。
/// 検証済みバージョンでは `probe` を**呼ばない**（起動のたびに器を作る無駄を避ける）
fn vet_with(
    choice: Choice,
    binary: &Binary,
    probe: impl FnOnce(&str) -> Result<(), String>,
) -> Choice {
    let (Choice::Psmux, Binary::Psmux { bin, version }) = (choice, binary) else {
        return choice;
    };
    if psmux::version_support(version) == psmux::VersionSupport::Verified {
        return choice;
    }
    match probe(bin) {
        Ok(()) => {
            eprintln!(
                "warning: psmux {version} は tako の適合検証済みバージョン（{}）と異なります。\
                 起動時プローブは通ったので永続バックエンドとして使います",
                psmux::VERIFIED_VERSION
            );
            choice
        }
        Err(e) => {
            eprintln!(
                "warning: psmux {version} は永続バックエンドとして使えません（{e}）。\
                 タブ・ペイン構成と cwd のみ復元する縮退モードで動きます\
                 （適合検証済みは psmux {}）",
                psmux::VERIFIED_VERSION
            );
            Choice::None
        }
    }
}

/// プロセス全体で共有する backend。
pub fn backend() -> &'static dyn SessionBackend {
    static BACKEND: OnceLock<Box<dyn SessionBackend>> = OnceLock::new();
    BACKEND
        .get_or_init(|| match (choice(), binary()) {
            (Choice::Tmux, _) => Box::new(TmuxBackend::new()) as Box<dyn SessionBackend>,
            (Choice::Psmux, Binary::Psmux { bin, version }) => {
                Box::new(PsmuxBackend::new(bin.clone(), version.clone())) as Box<dyn SessionBackend>
            }
            _ => Box::new(NullBackend) as Box<dyn SessionBackend>,
        })
        .as_ref()
}

/// `#{scroll_position}\t#{history_size}\t#{pane_in_mode}\t#{alternate_on}` の
/// 出力を [`ScrollProbe`] にする（純関数。tmux / psmux が共有する）。
///
/// **欠けたフィールドで `None` を返さない**のが要点（#654 と同じ設計）。
/// tmux は copy mode の外で `#{scroll_position}` を空にし、psmux は
/// フォーマットによっては空文字へ展開する。答えられない器を
/// 「観測できなかった」ではなく「既定値（最下部・非スクロールモード）」として扱う。
/// ただし `history_size` すら読めない出力は器の応答として壊れているので `None`
pub(crate) fn parse_scroll_probe(output: &str) -> Option<ScrollProbe> {
    let line = output.lines().next()?;
    let mut f = line.split('\t');
    let position = f.next().unwrap_or("").trim().parse().unwrap_or(0);
    let history = f.next()?.trim().parse().ok()?;
    let in_mode = f.next().unwrap_or("").trim() == "1";
    let alternate = f.next().unwrap_or("").trim() == "1";
    Some(ScrollProbe {
        position,
        history,
        in_mode,
        alternate,
    })
}

/// `list-panes -a -F "<id> #{pane_pid}"` の出力をパースする（純関数。#728）。
///
/// **`rsplit_once(' ')` で切る**のが要点。左側のセッション名に空白は入らない
/// （[`SessionRef::new`] が拒否する）が、器が行頭へ警告を混ぜたときに
/// 前から切ると ID を取り違える。pid にならない行・pid 0 は捨てる
pub(crate) fn parse_pane_pids_all(output: &str) -> Vec<(String, u32)> {
    output
        .lines()
        .filter_map(|line| {
            let (id, pid) = line.trim_end().rsplit_once(' ')?;
            let pid: u32 = pid.trim().parse().ok()?;
            let id = id.trim();
            (pid != 0 && !id.is_empty()).then(|| (id.to_string(), pid))
        })
        .collect()
}

/// 現在時刻（unix epoch 秒）。器の実装が最終アクティビティの猶予判定に使う
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod pane_scoped_env_tests {
    use super::*;

    /// #766: 側路の書き先はペインごとに違うので、`-e` で渡す表に載っていないと
    /// **器のグローバル環境の stale な値**（別ペインの書き先）が見えてしまう
    #[test]
    fn 側路の書き先はペイン固有の環境変数として渡る() {
        assert!(PANE_SCOPED_ENV.contains(&crate::osc_sink::SINK_ENV));
        assert!(PANE_SCOPED_ENV.contains(&"TAKO_PANE_ID"));
        assert!(PANE_SCOPED_ENV.contains(&"TAKO_TAB_ID"));
    }

    /// 表を引く側が tmux / psmux の両方であること（片方だけ足すと
    /// 「tmux では効くが psmux では効かない」という追いにくい差になる）
    #[test]
    fn 器の実装はどちらもこの表を引いている() {
        for (name, src) in [
            ("tmux_backend.rs", include_str!("../tmux_backend.rs")),
            ("backend/psmux.rs", include_str!("psmux.rs")),
        ] {
            assert!(
                src.contains("PANE_SCOPED_ENV.contains("),
                "{name} が PANE_SCOPED_ENV を引いていない（キーの直書きへ戻っている）"
            );
            assert!(
                !src.contains(r#"key == "TAKO_PANE_ID""#),
                "{name} にキーの直書きが残っている（表と食い違う）"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    /// #907: 打鍵で運べない組み合わせだけ器の注入口へ迂回する
    #[test]
    fn 注入へ迂回するのは非asciiかつ打鍵がasciiのみの器のとき() {
        let lossy = BackendCapabilities {
            keystrokes_ascii_only: true,
            ..tmux::TmuxBackend::new().capabilities()
        };
        let clean = BackendCapabilities {
            keystrokes_ascii_only: false,
            ..lossy
        };
        // 非 ASCII × 落とす器 = 迂回
        assert!(needs_text_injection(&lossy, "テスト"));
        assert!(needs_text_injection(&lossy, "ascii と 日本語"));
        // ASCII だけなら経路を増やさない（挙動差を作らない）
        assert!(!needs_text_injection(&lossy, "echo hello"));
        assert!(!needs_text_injection(&lossy, ""));
        // バイト等価な器（tmux / 器なし）は常に打鍵のまま
        assert!(!needs_text_injection(&clean, "テスト"));
    }

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
    fn 器の全ペイン列挙は末尾のpidで切る() {
        // #728: tmux / psmux が実際に返す形（Windows 実機の psmux 出力から採った）
        let out = "tako-1f3c0f0d9f5f:0.0 3340\ntako-2159422b104f:0.0 21236\n";
        assert_eq!(
            parse_pane_pids_all(out),
            vec![
                ("tako-1f3c0f0d9f5f:0.0".to_string(), 3340),
                ("tako-2159422b104f:0.0".to_string(), 21236),
            ]
        );
        // 器が行頭へ警告を混ぜても、**末尾**の pid で切るので ID を取り違えない
        assert_eq!(
            parse_pane_pids_all("warning: something tako-a:0.0 42\n"),
            vec![("warning: something tako-a:0.0".to_string(), 42)]
        );
        // pid にならない行・pid 0・空行・ID 欠落は捨てる
        assert!(parse_pane_pids_all("no server running\n").is_empty());
        assert!(parse_pane_pids_all("tako-a:0.0 0\n").is_empty());
        assert!(parse_pane_pids_all("\n \n").is_empty());
        assert!(parse_pane_pids_all("").is_empty());
        // CRLF（Windows の器）でも pid が壊れない
        assert_eq!(
            parse_pane_pids_all("tako-a:0.0 7\r\n"),
            vec![("tako-a:0.0".to_string(), 7)]
        );
    }

    #[test]
    fn 器を持たない実装は全ペイン列挙も空を返す() {
        // 既定実装の契約: 器が無いなら「器の中のペイン」は 0 件
        assert!(NullBackend.pane_pids_all().is_empty());
    }

    fn tmux_bin() -> Binary {
        Binary::Tmux { bin: "tmux".into() }
    }

    /// 内側コマンドの第 1 語の書き方が器で変わる（#881）
    #[test]
    fn 内側コマンドの組み立ては器の引用能力で変わる() {
        let spaced = crate::terminal::SpawnCommand {
            program: "C:\\Program Files\\PowerShell\\7\\pwsh.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-Command".into(),
                "Write-Output ok".into(),
            ],
        };
        // tmux は `sh -c` の意味論なので引用符で括ってよい（macOS の従来出力そのもの）
        assert_eq!(
            compose_inner_command(&spaced, true),
            "'C:\\Program Files\\PowerShell\\7\\pwsh.exe' -NoLogo -Command 'Write-Output ok'"
        );
        // psmux は括れない。第 1 語は 1 語へ落ち、引数だけがクオートされる
        let psmux_line = compose_inner_command(&spaced, false);
        let first = psmux_line.split(' ').next().unwrap_or_default();
        assert!(
            crate::platform::program_path::is_single_token(first),
            "第 1 語が引用符付き・空白入りのまま: {psmux_line}"
        );
        assert!(
            psmux_line.ends_with(" -NoLogo -Command 'Write-Output ok'"),
            "{psmux_line}"
        );

        // 空白の無いプログラムはどちらの器でも同じ（macOS の既存出力を変えない）
        let plain = crate::terminal::SpawnCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
        };
        assert_eq!(compose_inner_command(&plain, true), "/bin/sh -c 'echo hi'");
        assert_eq!(compose_inner_command(&plain, false), "/bin/sh -c 'echo hi'");
    }

    /// 器へ渡す行の組み立てが 1 か所に集まっていること（番犬）。
    /// `tmux_backend::wrap_options` が `shell_quoted` を直接呼ぶ形へ戻ると、
    /// psmux では第 1 語が引用符付きのまま渡って**静かに起動できなくなる**（#881）
    #[test]
    fn wrap_optionsは内側コマンドの組み立てを境界へ委ねる() {
        let src = include_str!("../tmux_backend.rs");
        let body = src
            .split("pub fn wrap_options(")
            .nth(1)
            .expect("wrap_options の定義");
        let body = &body[..body.find("\npub ").unwrap_or(body.len())];
        assert!(
            body.contains("inner_command_line("),
            "wrap_options が内側コマンドの組み立てを境界へ委ねていない"
        );
        assert!(
            !body.contains("shell_quoted(inner)"),
            "wrap_options が shell_quoted を直接呼んでいる（psmux で第 1 語が壊れる。#881）"
        );
    }

    fn psmux_bin() -> Binary {
        Binary::Psmux {
            bin: "tmux".into(),
            version: "3.3.7".into(),
        }
    }

    #[test]
    fn 環境変数noneはバイナリの有無によらず器なしへ倒れる() {
        for env in ["none", "NONE", " none ", "null", "off"] {
            assert_eq!(decide(Some(env), &tmux_bin(), false), Choice::None);
            assert_eq!(decide(Some(env), &psmux_bin(), true), Choice::None);
        }
    }

    #[test]
    fn 未知の値と未設定はautoと同じ扱いになる() {
        // タイポで永続化が黙って落ちるのが最悪なので auto へ倒す
        for binary in [tmux_bin(), psmux_bin(), Binary::Absent] {
            let auto = decide(None, &binary, false);
            assert_eq!(decide(Some("tmuxx"), &binary, false), auto);
            assert_eq!(decide(Some(""), &binary, false), auto);
        }
    }

    /// **#519 M2 要件 8（tmux 誤判別ガード）**: psmux は `tmux.exe` を PATH に置くので、
    /// `tmux -V` が成功することは本物の tmux がある証拠にならない。
    /// 誤って [`Choice::Tmux`] を選ぶと「器は作れるが kill が効かない」
    /// （`kill-session -t =name` が 5.1 秒ブロックの末に失敗）永続化になる
    #[test]
    fn psmuxをtmuxと誤判別しない() {
        // `tmux` という名前で見つかっても、正体が psmux なら psmux backend を選ぶ
        assert_eq!(decide(None, &psmux_bin(), true), Choice::Psmux);
        assert_eq!(decide(None, &psmux_bin(), false), Choice::Psmux);
        // 明示 tmux 指定でも psmux を tmux として使わない
        assert_eq!(decide(Some("tmux"), &psmux_bin(), true), Choice::None);
        assert_eq!(decide(Some("tmux"), &psmux_bin(), false), Choice::None);
    }

    /// `-V` の出力から正体を判別する（psmux は 1 行目で tmux を詐称する）
    #[test]
    fn バージョン出力から正体を判別する() {
        assert_eq!(
            classify_version_output("tmux 3.3.7\npsmux 3.3.7 (05cc5d4 2026-07-20)\n", "tmux"),
            Some(Binary::Psmux {
                bin: "tmux".into(),
                version: "3.3.7".into()
            })
        );
        assert_eq!(
            classify_version_output("tmux 3.6\n", "tmux"),
            Some(Binary::Tmux { bin: "tmux".into() })
        );
        // tmux 系でない出力は器の候補にしない
        assert_eq!(
            classify_version_output("GNU screen 4.9.1\n", "screen"),
            None
        );
        assert_eq!(classify_version_output("", "tmux"), None);
    }

    /// Windows では本物の tmux（MSYS2 / Cygwin 版）を器に選ばない。
    /// ネイティブの ConPTY シェルを抱えられず、`-f` の Windows パスも解釈できないため
    #[test]
    fn windowsではposix版tmuxを器に選ばない() {
        assert_eq!(decide(None, &tmux_bin(), true), Choice::None);
        assert_eq!(decide(None, &tmux_bin(), false), Choice::Tmux);
        assert_eq!(decide(None, &Binary::Absent, true), Choice::None);
        assert_eq!(decide(None, &Binary::Absent, false), Choice::None);
    }

    /// **#519 M2 要件 6**: 未検証バージョンは「測って」から採る。
    ///
    /// バージョン一致だけを条件にすると psmux が patch を上げた翌日に全ユーザーの
    /// 永続化が黙って落ち、無条件に信じると壊れた器を掴む
    #[test]
    fn 未検証バージョンはプローブの結果で採否が決まる() {
        let untested = Binary::Psmux {
            bin: "psmux".into(),
            version: "9.9.9".into(),
        };
        // 検証済みバージョンではプローブを呼ばない（起動のたびに器を作らない）
        assert_eq!(
            vet_with(Choice::Psmux, &psmux_bin(), |_| panic!(
                "検証済みバージョンでプローブを走らせた"
            )),
            Choice::Psmux
        );
        // 未検証でもプローブが通れば使う
        assert_eq!(
            vet_with(Choice::Psmux, &untested, |_| Ok(())),
            Choice::Psmux
        );
        // 通らなければ器を配らない（構成のみ復元へ明示縮退）
        assert_eq!(
            vet_with(Choice::Psmux, &untested, |_| Err("器を壊せない".into())),
            Choice::None
        );
        // psmux 以外の選択にはプローブを挟まない
        assert_eq!(
            vet_with(Choice::Tmux, &tmux_bin(), |_| panic!(
                "tmux でプローブを走らせた"
            )),
            Choice::Tmux
        );
        assert_eq!(
            vet_with(Choice::None, &untested, |_| panic!(
                "器なしでプローブを走らせた"
            )),
            Choice::None
        );
    }

    /// psmux が無ければ psmux は選ばれない（明示指定でも器を捏造しない）
    #[test]
    fn psmuxが無ければ明示指定でも器なしへ倒れる() {
        assert_eq!(decide(Some("psmux"), &tmux_bin(), false), Choice::None);
        assert_eq!(decide(Some("psmux"), &Binary::Absent, true), Choice::None);
        assert_eq!(decide(Some("psmux"), &psmux_bin(), true), Choice::Psmux);
    }

    #[test]
    fn 器が無いときだけ縮退の説明が出る() {
        let with_container = BackendCapabilities {
            survives_app_exit: true,
            detached_capture: true,
            detached_access: true,
            scrollback: ScrollbackAuthority::Backend,
            osc_passthrough: true,
            quotes_program: true,
            keystrokes_ascii_only: false,
            label: "tmux",
        };
        assert!(with_container.degraded_note().is_none());
        assert!(with_container.full_restore());

        let without = BackendCapabilities {
            survives_app_exit: false,
            detached_capture: false,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            osc_passthrough: true,
            quotes_program: true,
            keystrokes_ascii_only: false,
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
            detached_capture: false,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            osc_passthrough: true,
            quotes_program: true,
            keystrokes_ascii_only: false,
            label: "none",
        };
        let v = caps.describe();
        assert_eq!(v["label"], "none");
        assert_eq!(v["survives_app_exit"], false);
        assert_eq!(v["detached_capture"], false);
        assert_eq!(v["detached_access"], false);
        assert_eq!(v["scrollback"], "in_process");
        assert!(v["note"].is_string(), "縮退時は note が入る");

        let tmux = BackendCapabilities {
            survives_app_exit: true,
            detached_capture: true,
            detached_access: true,
            scrollback: ScrollbackAuthority::Backend,
            osc_passthrough: true,
            quotes_program: true,
            keystrokes_ascii_only: false,
            label: "tmux",
        };
        assert_eq!(tmux.describe()["scrollback"], "backend");
        assert!(
            tmux.describe()["note"].is_null(),
            "縮退していなければ note は無い"
        );

        // psmux（採取だけできる器）は 2 つの bool が食い違う。
        // **`describe()` が両方を別々に出す**ので、AI / CLI は
        // 「読めるが送れない」をこの 1 箇所から読める
        let psmux = BackendCapabilities {
            survives_app_exit: true,
            detached_capture: true,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            osc_passthrough: false,
            quotes_program: false,
            keystrokes_ascii_only: true,
            label: "psmux",
        };
        let v = psmux.describe();
        assert_eq!(v["detached_capture"], true);
        assert_eq!(v["detached_access"], false);
    }

    /// 案 B-1（器だけの ConPTY セッションホスト）の形をした偽 backend。
    /// **tmux ではない器**がこの境界に嵌まることを検証するための最小実装で、
    /// 呼ばれた `wrap_spawn` のセッション名を記録する
    struct FakeSessionHost {
        wrapped: std::sync::Mutex<Vec<String>>,
        /// `pane_pids` が呼ばれたことを試験側へ伝える（#659 の配線検証）
        pid_asked: std::sync::mpsc::SyncSender<String>,
    }

    impl FakeSessionHost {
        /// `wrap_spawn_for_pane` は `&'static` を要る（器の内側へコードページ固定を
        /// 届けるバックグラウンド作業が backend を持ち越すため）。試験では意図的に leak する
        fn leaked() -> (&'static Self, std::sync::mpsc::Receiver<String>) {
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            let host = Box::leak(Box::new(Self {
                wrapped: std::sync::Mutex::new(Vec::new()),
                pid_asked: tx,
            }));
            (host, rx)
        }
    }

    impl SessionBackend for FakeSessionHost {
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities {
                survives_app_exit: true,
                detached_capture: false,
                detached_access: false,
                scrollback: ScrollbackAuthority::InProcess,
                osc_passthrough: true,
                quotes_program: true,
                keystrokes_ascii_only: false,
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
        fn pane_pids(&self, session: &SessionRef) -> Vec<u32> {
            let _ = self.pid_asked.try_send(session.as_str().to_string());
            // 器の中の pid が「まだ無い」状態を返す（配線だけを見たいので固定はさせない）
            Vec::new()
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
        let (host, _pids) = FakeSessionHost::leaked();
        let options = SpawnOptions {
            command: None,
            cwd: Some(std::path::PathBuf::from("/tmp/work")),
            env: vec![("TAKO_PANE_ID".into(), "7".into())],
        };
        let (wrapped, session) = wrap_spawn_for_pane(
            host,
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
        let (host, _pids) = FakeSessionHost::leaked();
        let (_, session) = wrap_spawn_for_pane(
            host,
            true,
            Some("tako-ffffffffffff"),
            || panic!("既存名があるのに候補名を払い出した"),
            SpawnOptions::default(),
        );
        assert_eq!(session.unwrap().as_str(), "tako-ffffffffffff");
    }

    /// **#659 の配線**: 器を配ったら、器の**内側**のプロセスへコードページ固定を
    /// 届ける経路が起動する。#655 の固定は tako の ConPTY 直下の子（= psmux では
    /// クライアント）にしか当たらず、器の中のシェルに届いていなかった。
    ///
    /// 固定が要るのは Windows だけなので、**unix では器へ問い合わせすらしない**
    /// （器への問い合わせはサブプロセス起動。無駄なスレッドも作らない）ことも同時に固定する
    #[test]
    fn 器を配ったら器の内側のpidを問い合わせる() {
        let (host, pids) = FakeSessionHost::leaked();
        let (_, session) = wrap_spawn_for_pane(
            host,
            true,
            None,
            || "tako-0123456789ab".to_string(),
            SpawnOptions::default(),
        );
        assert!(session.is_some());
        let pinning = crate::platform::console::pin_needed();
        // 起きないことの確認に 5 秒待たない（unix 側は短く打ち切る）
        let wait = if pinning {
            Duration::from_secs(5)
        } else {
            Duration::from_millis(300)
        };
        let asked = pids.recv_timeout(wait);
        if pinning {
            assert_eq!(
                asked.ok().as_deref(),
                Some("tako-0123456789ab"),
                "器を配ったのに器の内側の pid を尋ねていない（#659 の再発）"
            );
        } else {
            assert!(
                asked.is_err(),
                "固定が不要なプラットフォームで器へ問い合わせている（無駄なサブプロセス）"
            );
        }
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
        let (host, _pids) = FakeSessionHost::leaked();
        let (passthrough, session) = wrap_spawn_for_pane(
            host,
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
        let (host, _pids) = FakeSessionHost::leaked();
        let session = reserve_for_pane(host, true, Some("tako-abc:0.0"), || {
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
            detached_capture: false,
            detached_access: false,
            scrollback: ScrollbackAuthority::InProcess,
            osc_passthrough: true,
            quotes_program: true,
            keystrokes_ascii_only: false,
            label: "session-host",
        };
        assert!(b1.full_restore());
        assert!(!b1.detached_access);

        // #519: psmux で実際に現れた「器あり・採取あり・送出なし」。
        // 読みと書きを 1 つの bool にまとめていたら表現できなかった中間状態
        let psmux = BackendCapabilities {
            detached_capture: true,
            ..b1
        };
        assert!(psmux.detached_capture && !psmux.detached_access);
    }

    /// 採取だけを持つ器（psmux 形）。**`detached()` は `None` のまま
    /// `detached_capture()` が開く**ことを、実装名に依存せず境界の形だけで固定する
    struct FakeCaptureOnly {
        capture: FakeCapture,
    }

    struct FakeCapture;

    impl DetachedCapture for FakeCapture {
        fn capture_screen(&self, _s: &SessionRef) -> Result<Vec<String>, BackendError> {
            Ok(vec!["live".into()])
        }
        fn capture_history(&self, _s: &SessionRef, _lines: usize) -> Option<Vec<String>> {
            Some(vec!["old".into()])
        }
        fn capture_history_joined(&self, _s: &SessionRef, _lines: usize) -> Option<String> {
            Some("old".into())
        }
        fn capture_scrollback(
            &self,
            _s: &SessionRef,
            _lines: usize,
        ) -> Result<Vec<String>, BackendError> {
            Ok(vec!["old".into(), "live".into()])
        }
        fn history_probe(&self, _s: &SessionRef) -> Option<HistoryProbe> {
            Some(HistoryProbe {
                history: 3,
                limit: 10,
                bytes: 0,
                alternate: false,
            })
        }
        fn history_probe_batch(&self) -> Vec<(SessionRef, HistoryProbe)> {
            Vec::new()
        }
        fn scroll_probe(&self, _s: &SessionRef) -> Option<ScrollProbe> {
            Some(ScrollProbe {
                position: 2,
                history: 3,
                in_mode: true,
                alternate: false,
            })
        }
    }

    impl SessionBackend for FakeCaptureOnly {
        fn capabilities(&self) -> BackendCapabilities {
            BackendCapabilities {
                survives_app_exit: true,
                detached_capture: true,
                detached_access: false,
                scrollback: ScrollbackAuthority::InProcess,
                osc_passthrough: true,
                quotes_program: true,
                keystrokes_ascii_only: false,
                label: "capture-only",
            }
        }
        fn reserve(&self, candidate: &str) -> Option<SessionRef> {
            SessionRef::new(candidate).ok()
        }
        fn wrap_spawn(&self, options: SpawnOptions, _s: &SessionRef) -> SpawnOptions {
            options
        }
        fn exists(&self, _s: &SessionRef) -> bool {
            true
        }
        fn kill(&self, _s: &SessionRef) -> Result<(), BackendError> {
            Ok(())
        }
        fn list(&self) -> Vec<SessionInfo> {
            Vec::new()
        }
        fn foreign_holders(&self, _s: &[SessionRef]) -> Vec<Holder> {
            Vec::new()
        }
        fn orphans(&self, _p: &HashSet<SessionRef>) -> Vec<SessionRef> {
            Vec::new()
        }
        fn cleanup_orphans(
            &self,
            _p: &HashSet<SessionRef>,
            _min_idle: Option<Duration>,
        ) -> Vec<SessionRef> {
            Vec::new()
        }
        fn pane_tty(&self, _s: &SessionRef) -> Option<String> {
            None
        }
        fn session_cwd(&self, _s: &SessionRef) -> Option<String> {
            None
        }
        fn session_env(&self, _s: &SessionRef, _n: &str) -> Option<String> {
            None
        }
        fn set_session_env(&self, _s: &SessionRef, _n: &str, _v: &str) {}
        fn detached_capture(&self) -> Option<&dyn DetachedCapture> {
            Some(&self.capture)
        }
    }

    /// **読みと書きが型で分かれている**ことの検証。
    /// 採取だけの器では `detached()`（送出）が閉じたまま `detached_capture()` が開く
    #[test]
    fn 採取だけの器は読みの入口だけを開く() {
        let b = FakeCaptureOnly {
            capture: FakeCapture,
        };
        let s = SessionRef::new("tako-0123456789ab").unwrap();
        assert!(b.detached().is_none(), "送出の入口は閉じたまま");
        let capture = b.detached_capture().expect("採取の入口は開く");
        assert_eq!(
            capture.capture_screen(&s).unwrap(),
            vec!["live".to_string()]
        );
        assert_eq!(capture.scroll_probe(&s).unwrap().position, 2);
    }

    /// 逆向き: 送出まで持つ器は `detached_capture()` の既定実装で
    /// **何も書かずに**読みの入口も開く（tmux が該当）
    #[test]
    fn 送出できる器は既定実装で読みの入口も開く() {
        let b = TmuxBackend::new();
        assert!(b.detached().is_some());
        assert!(
            b.detached_capture().is_some(),
            "detached_capture の既定実装が detached から引き上げる"
        );
    }

    /// #724: 器そのもののプロセス名を、名乗り方によらず見分けられること
    #[test]
    fn 器のプロセス名は拡張子と大小文字を問わず見分けられる() {
        for name in [
            "psmux.exe",
            "PSMUX.EXE",
            "psmux",
            "pmux.exe",
            "tmux.exe",
            "tmux",
            "Tmux.Exe",
        ] {
            assert!(is_plumbing_process(name), "器として見分けられない: {name}");
        }
    }

    /// ユーザーのプログラムを器と誤認しないこと（誤認すると本物の
    /// dev サーバーのポートが黙って消える）
    #[test]
    fn ユーザーのプログラムは器と誤認しない() {
        for name in [
            "node.exe",
            "python.exe",
            "",
            "tmuxinator.exe",
            "my-tmux-wrapper.exe",
            "psmuxd.exe",
            "pwsh.exe",
        ] {
            assert!(!is_plumbing_process(name), "器と誤認した: {name}");
        }
    }

    /// 判定の材料（名前の一覧）は器の検出結果と独立に持つ（#724 の理由節）。
    /// 一覧が空になると除外そのものが効かなくなるので下限を固定する
    #[test]
    fn 器の名前一覧は3種を必ず含む() {
        for known in ["psmux", "pmux", "tmux"] {
            assert!(
                PLUMBING_PROCESS_NAMES.contains(&known),
                "{known} が一覧から落ちている"
            );
        }
    }
}
