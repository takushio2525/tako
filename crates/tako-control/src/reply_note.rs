//! 応答の値に載せる注記と、CLI が断りと同じ出力に並べる行の文（#2365）。
//!
//! 断り（`Err`）の文は [`crate::Reason`] / [`crate::HostRefusal`] ほかが日英を持つ。ここは**断りではない文**
//! = dispatch が `Ok` の応答の値（`note` / `warnings` / `placement_reason` / `applies_to` ほか）へ載せる説明と、
//! CLI が手元の処理で断りと同じ出力に並べる行（`tako orchestrator resume` の案内・`tako telemetry` /
//! `tako fda` の状態・`tako run --wait` の進み具合）。以前はどれも日本語の固定文だったので、英語表示でも
//! 応答の理由や CLI の出力の一部だけ日本語で返った。
//!
//! 形は [`crate::Reason`] と同じ: (バリアント, 言語) の網羅 match なので、バリアントを足すと日英の両方を
//! 書くまでコンパイルが通らない。表示（`Display` / `String` への変換）は組んだその場の表示言語
//! （GUI は `tako_core::i18n::lang()`。GUI を通さない CLI の直呼びは呼ぶ前に表示言語を決める）。
//! 日本語は #2365 より前の固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が旧文の表と
//! 突き合わせる）。
//!
//! 中身に他モジュールの文を載せる注記は、枠だけ移すと 1 つの注記に日英が混ざるので、中身と一緒に移す
//! （Issue #2365 の表）。プロファイルの参照（`orchestrator::profile_warnings`）・採用の警告
//! （`decide_adopt`）・ctx の窓（`claude_ctx::window_warning`）・会話の再開の食い違い（`resume_launch` の
//! `warnings`）・worker の MCP（`worker_mcp` の `warnings`）は、組む側が文字列ではなくここのバリアントを
//! 返し、中身（`ConfigError` / `SessionRefusal`）も同じ言語で組む。persist.log へ載せる呼び手は
//! `text_in(Lang::Ja)`、応答・CLI の行は表示言語で組む。
//!
//! orchestrator の応答の `warnings` に並ぶ別モジュールの注記のうち、引き継ぎファイルの自動移行
//! （`handoff_store::ensure_migrated` の `MigrationOutcome::warnings`。応答の JSON へは表示言語の文で
//! 書き出し、`tako migrate` の記録は日本語）とモデルの食い違い（`registry::model_warning` /
//! `ModelObservation::warning`。旧 `tako_core::model_check::mismatch_warning`）も同じ形で持つ。
//! 引き継ぎの応答のほかの文（移行の 1 行サマリ `MigrationOutcome::summary`・運用メモの肥大
//! `handoff_store::memo_warning`。旧 `tako_core::handoff::profile_memo_warning`）と `workers` 一覧の
//! `resend_command`（`registry::resend_command`。プレースホルダだけ表示言語）も組む側は型を返す

use tako_core::handoff::{HANDOFF_PROJECTS_DIR, PROFILE_MEMO_SOFT_LIMIT_LINES};
use tako_core::i18n::Lang;
use tako_core::platform::support::Note;

use crate::orchestrator::{ConfigError, ProfileKind};
use crate::worker_mcp::CodexConfigError;
use crate::SessionRefusal;

/// 応答の値・CLI の行に載せる注記の文（#2365）。値（ペイン番号・パス・桁数・env 名）は表記のまま埋める
#[derive(Debug, Clone, PartialEq)]
pub enum ReplyNote {
    // ---- プレビュー・編集・ファイルツリー ----
    /// `git 管理外のファイル`（変更履歴を開いたファイルがリポジトリの外）
    ChangelogUntracked,
    /// `<path>（ディスク）`（ディスクとの差分の見出し）
    DiskDiffDisk(String),
    /// `<path>（編集中）`（ディスクとの差分の見出し）
    DiskDiffEditing(String),
    /// `リモート（SSH）の行はキーで動かさない`
    TreeRemoteRowStays,

    // ---- 設定・OS ----
    /// autosuggest の `applies_to`
    AutosuggestAppliesTo,
    /// autosuggest の `note`（二重注入ガードと Tab 確定の条件）
    AutosuggestNote,
    /// `tako setup models` の `apply_command`（プレースホルダは表示言語）
    ModelsApplyCommand,
    /// `System Settings の Battery を開きました`
    BatterySettingsOpened,
    /// 子の `tako setup` を上限で打ち切った
    SetupTimedOut,

    // ---- orchestrator のプロファイル・アカウント・レイアウト ----
    /// solo のプロファイルで limit_resume が効く先
    SoloLimitResumeScope,
    /// codex の承認スキップとサンドボックス解除が同じフラグ
    CodexBypassSandboxOff,
    /// プロファイルの remote_control が有効にできない（`detail` は env 名・設定ファイル名・系統名）
    ProfileRemoteControlBlocked {
        detail: String,
        reason: Note,
        next_step: Note,
    },
    /// accounts に claude の既定パスを明示した（#512）
    AccountsDefaultConfigDir(String),
    /// 有効なレイアウト方針: 桁数の保証なし（min_worker_cols=0）
    PolicySameTabNoGuarantee,
    /// 有効なレイアウト方針: 下限を割っても自動縮小しない
    PolicySameTabNoShrink(u16),
    /// 有効なレイアウト方針: 下限を割るときはフォントを縮める（`floor_pct` は床の %）
    PolicySameTabShrink { min_cols: u16, floor_pct: i32 },

    // ---- orchestrator self / handoff ----
    /// 名指しされたペインが master / solo でなく profile が既定へ落ちた（`solo_pane` = solo のペイン）
    SelfProfileDefaulted {
        pane: u64,
        role: String,
        profile: String,
        solo_pane: bool,
    },
    /// ctx_threshold の生値が値域外で丸めた
    CtxThresholdClamped {
        raw: u32,
        value: u32,
        min: u32,
        max: u32,
    },
    /// 旧 master は後任が閉じる
    PreviousMasterClosedBySuccessor,
    /// 旧 master を特定できず閉じない
    PreviousMasterKept,

    // ---- worker の配置（spawn の `placement_reason`） ----
    /// 床まで縮めても下限に届かない（`size` は `11.0pt` か `-`）
    FontFitShort {
        pct: i32,
        size: String,
        cols: u16,
        min_cols: u16,
    },
    /// フォントを縮めて桁数を確保した
    FontFitShrunk {
        pct: i32,
        size: String,
        cols: u16,
        min_cols: u16,
    },
    /// ウィンドウの全幅でも下限に届かない（#1132 の腕）
    PlacementWindowTooNarrow { full_cols: u16, min_cols: u16 },
    /// この master の worker が居るタブへ置いた（#1132 の腕）
    PlacementOverflowTab {
        predicted: u16,
        min_cols: u16,
        tab: u64,
        cols: u16,
    },
    /// 新しいタブへ置いた（#1132 の腕）
    PlacementNewTab { predicted: u16, min_cols: u16 },
    /// spawn した worker の Remote Control が有効にならなかった
    SpawnRemoteControlBlocked {
        detail: String,
        reason: Note,
        next_step: Note,
    },

    // ---- worker の状態の補正 ----
    /// mod は応答待ちを報告しているが画面に選択肢ダイアログが無い（`turn` は mod の状態名）
    ModTurnWithoutDialog(String),
    /// 画面に権限ダイアログがあるが mod は報告していない（`classic_events_off` = classic 系が届かない環境）
    PermissionDialogUnreported {
        turn: String,
        classic_events_off: bool,
    },
    /// busy と判定されたが子プロセスも画面の busy パターンも無い
    StalledWithoutChildren,
    /// バックエンドセッションが無いので stale を見ない
    StaleNoBackend,

    // ---- 会話を保った再起動 ----
    /// ハーネス更新の流れ
    SessionRestartHarness,
    /// 引き継ぎの流れ
    SessionRestartHandoff,

    // ---- 突然死からの復旧（`tako orchestrator resume`） ----
    /// spawn した master のプロファイルの記録が無いので既定で組んだ
    ResumeNoProfileRecord,
    /// 会話を spawn と同じ起動で戻す案内（`session` は会話 ID の先頭 8 文字）
    ResumingWorker {
        worker: String,
        session: String,
        profile: String,
    },

    // ---- CLI の手元の行の見出し ----
    /// `エラー`（`エラー: <理由>` の見出し）
    ErrorLabel,
    /// `警告`（`警告: <文>` の見出し）
    WarningLabel,
    /// `[情報]`（設定の移行の知らせの見出し）
    InfoLabel,

    // ---- `tako telemetry` ----
    /// `  直近のレポート件数: <n>`
    TelemetryRecent(usize),
    /// `  未送信キュー: <n>`
    TelemetryQueued(usize),
    /// `  ログ: <path>`
    TelemetryLog(String),

    // ---- `tako fda` ----
    /// フルディスクアクセスは付与済み
    FdaGranted,
    /// フルディスクアクセスは未付与
    FdaNotGranted,
    /// 未付与のときに出るダイアログの説明
    FdaDialogMayAppear,
    /// 付与の手順
    FdaHowToGrant,
    /// システム設定を開いた
    FdaSettingsOpened,
    /// fda の状態の `description`: macOS 以外
    FdaNotApplicable,
    /// fda の状態の `description`: 付与済み
    FdaGrantedDescription,
    /// fda の状態の `description`: 未付与
    FdaNotGrantedDescription,

    // ---- `tako run` / `tako run-interactive` の `--wait` ----
    /// 対話コマンドを起動した（`status` は応答の値）
    RunInteractiveStarted { pane: u64, status: String },
    /// コマンドを実行中（`command` は応答の値）
    RunStarted { pane: u64, command: String },
    /// 実行前に未保存の編集を保存した（`panes` はペイン番号の並び）
    RunSavedBefore(String),
    /// 終わるまで待つ上限（秒）
    RunWaitBudget(u64),

    // ---- `tako master` / `tako solo` の起動の流れ ----
    /// MCP の登録パス（`~/.claude.json` の `mcpServers.tako.command`）が消えている（2 行）
    McpPathMissing(String),
    /// `[警告] <source> のモデル '<model>' は 1M コンテキスト版のため…`（2 行。`profile_warnings` と共有）
    OneMModelWarning { source: String, model: String },
    /// `master を起動しました: <場所>`（場所は `LaunchTab*` / `LaunchInline` の文）
    MasterLaunched(String),
    /// `solo を起動しました: <場所>`
    SoloLaunched(String),
    /// `タブ '<title>'（ペイン <pane>）`
    LaunchTabNamed { title: String, pane: u64 },
    /// `新しいタブ（ペイン <pane>）`
    LaunchNewTab(u64),
    /// `ペイン <pane>（インライン）`
    LaunchInline(u64),
    /// `プロファイル: <profile>（エージェント: <agent>、モデル: <model>、effort: <effort>）`
    LaunchProfile {
        profile: String,
        agent: String,
        model: String,
        effort: String,
    },
    /// `リミット後の自動復帰: 有効（プロファイル既定。#1140）`
    LimitResumeOn,
    /// `worker モデルポリシー: inherit（master と同じ <model> / <effort>）`
    WorkerPolicyInherit { model: String, effort: String },
    /// `worker モデルポリシー: inherit（master は <agent> のため claude worker へは非継承: …）`
    WorkerPolicyInheritNonClaude {
        agent: String,
        model: String,
        effort: String,
    },
    /// `worker モデルポリシー: fixed（<model> / <effort>）`
    WorkerPolicyFixed { model: String, effort: String },
    /// `worker モデルポリシー: delegate（master が判断）`
    WorkerPolicyDelegate,
    /// `projects 制限: <keys>`
    ProjectsRestriction(String),
    /// `ファイルツリーに追加: <names>`
    TreeFoldersAdded(String),
    /// `モード: solo（オーケストレーション無し・1 対 1 対話・worker spawn 禁止）`
    SoloMode,
    /// `アカウント: <name>（config dir: <dir>）`
    MasterAccount { name: String, config_dir: String },
    /// `アカウント: <name>（既定の資格情報 / CLAUDE_CONFIG_DIR 未設定）`
    MasterAccountDefault(String),
    /// `  有効にする場合: <command>`（Remote Control の入れ方）
    RemoteControlEnableHint(String),
    /// `  外す場合: <command>`（codex のサンドボックスの外し方）
    SandboxBypassHint(String),
    /// `TAKO_PANE_ID` が旧世代のペイン ID だった（2 行）
    StalePaneIdRemapped { old: u64, pane: u64 },
    /// `TAKO_PANE_ID` が呼び出し元ペインと一致しない（2 行）
    PaneIdMismatch { old: u64, pane: u64 },
    /// `  このシェルを使い続けるなら: unset TAKO_PANE_ID`
    UnsetPaneIdHint,
    /// 呼び出し元ペインを特定できない（`TAKO_PANE_ID` が今の tako に無い古い値）
    CallerPaneUnknownStale(u64),
    /// 呼び出し元ペインを特定できない（`TAKO_PANE_ID` 未設定）
    CallerPaneUnknownUnset,
    /// `  新しいタブを作ってそこで起動します`
    LaunchingInNewTab,

    // ---- GUI の居ないホスト（`host.rs` の既定 = `examples/mcp_host`）の応答の値 ----
    /// ssh の自動検知の `reason`（GUI が居ないので読めない）
    SshDetectionNoGui,

    // ---- 他モジュールが組む注記（応答の `warnings`。#2365） ----
    /// プロファイルの projects に projects.yaml に未登録のキー `<key>` がある（`profile_warnings`。2 行）
    ProfileProjectUnregistered(String),
    /// プロファイルの `<label>`（master_account / worker_account）のアカウント `<name>` が
    /// accounts.yaml に無い（`profile_warnings`。2 行）
    ProfileAccountUnregistered { label: String, name: String },
    /// 採用先 `<target>` が管轄を持たない汎用プロファイル（`decide_adopt`）
    AdoptGenericTarget(String),
    /// 採用先 `<target>` の system prompt が起動時の `<launched_as>` と違う（`decide_adopt`）
    AdoptPromptDiffers { target: String, launched_as: String },
    /// ctx% の画面と mod の報告が `<delta>` ポイントずれている（mod の値 `<percent>`% を採用。
    /// `claude_ctx::window_warning`）
    CtxModDrift { delta: i32, percent: u32 },
    /// ctx% の画面と transcript が `<delta>` ポイントずれている（transcript 側の文脈窓 `<window>`。
    /// `claude_ctx::window_warning`）
    CtxTranscriptDrift { delta: i32, window: u64 },
    /// 会話の再開: `<kind>` プロファイル `<name>` が無いので既定プロファイルで組んだ（`resume_launch`）
    ResumeProfileMissing { kind: ProfileKind, name: String },
    /// 会話の再開: worker のアカウント `<name>` を引けないのでプロファイルの env だけで組んだ
    /// （`why` は同じ言語で組む `ConfigError`）
    ResumeWorkerAccountUnresolved { name: String, why: ConfigError },
    /// 会話の再開: プロファイルの設定 dir `<path>` に会話の記録が無いので、記録のある方で組んだ
    ResumeConfigDirPinned(String),
    /// 会話の再開: プロファイルの CODEX_HOME `<current>` に会話の記録が無いので `<home>` で組んだ
    ResumeCodexHomePinned { current: String, home: String },
    /// worker の MCP: user scope の MCP を止める設定ファイルを書けなかった（`why` は同じ言語で組む）
    McpSettingsUnwritable(SessionRefusal),
    /// worker の MCP: 止める設定ファイルの置き場（データディレクトリ）が決まらない
    McpSettingsNoDataDir,
    /// worker の MCP: プロジェクトの mcp_servers の `<name>` が user scope にも .mcp.json にも無い
    McpKeepMissing(String),
    /// worker の MCP: プロファイルの worker_agents.claude.args に --settings がある
    McpSettingsOverridden,
    /// worker の MCP（codex）: CODEX_HOME を決められない
    McpCodexHomeUnresolved,
    /// worker の MCP（codex）: codex の設定を読めない（中身は置き場とエラー文）
    McpCodexConfigUnreadable(CodexConfigError),
    /// worker の MCP（codex）: mcp_servers の `<name>` が config.toml にも .codex/config.toml にも無い
    McpCodexKeepMissing(String),
    /// worker の MCP（codex）: `-c` で表せない名前 `<name>` は止めずに持ち込む
    McpCodexNameInexpressible(String),

    // ---- orchestrator の応答の warnings に並ぶ別モジュールの注記 ----
    /// 引き継ぎファイルの移行: projects.yaml が読めないので見送った（`handoff_store::ensure_migrated`）
    HandoffMigrationDeferred(ConfigError),
    /// 引き継ぎファイルの移行: 書き込みの断り（中身の文だけ。枠は付けない）
    HandoffMigrationWriteFailed(ConfigError),
    /// 引き継ぎファイルの移行: 原本を退避できないので中止した
    HandoffMigrationArchiveFailed(ConfigError),
    /// 引き継ぎファイルの移行: 移せないプロジェクトがあったので運用メモを書き換えなかった
    HandoffMigrationMemoKept,
    /// 宣言と実際のモデルの食い違い（#1638。`worker_status` の warnings と persist.log）。
    /// `account` は transcript が属したアカウント名（引けなければ `None` = 名指ししない）。
    /// 名前・モデル名だけを載せる（ホームパス・会話の中身は載せない）
    ModelMismatch {
        declared: String,
        actual: String,
        account: Option<String>,
    },
    /// `workers` 一覧の食い違いの行（worker と pane を前に置いた [`Self::ModelMismatch`]）
    WorkerModelMismatch {
        worker_id: String,
        pane: u64,
        declared: String,
        actual: String,
        account: Option<String>,
    },

    // ---- 引き継ぎの応答のほかの文・`workers` 一覧の再送コマンド ----
    /// 引き継ぎファイルの移行の 1 行サマリ（`MigrationOutcome::summary`。`handoffs migrate` の
    /// `summaries`）。`moved` は移した先のプロジェクトキー（空 = 移動なし）、`archived_to` は原本の退避先
    HandoffMigrated {
        profile: String,
        moved: Vec<String>,
        kept_lines: usize,
        archived_to: Option<String>,
    },
    /// プロファイル運用メモの肥大（#915 要件 3。旧 `tako_core::handoff::profile_memo_warning`）。
    /// `lines` は今の行数（目安は `PROFILE_MEMO_SOFT_LIMIT_LINES`）
    ProfileMemoOversized { profile: String, lines: usize },
    /// `workers` 一覧の `resend_command`（未達 worker へ同じ依頼文を送り直すテンプレート）。
    /// コピーして打つコマンドなので、表示言語に従うのはプレースホルダだけ（本体と引用は同じ）
    ResendCommand { pane: u64 },
}

impl ReplyNote {
    /// 表示言語を明示しての文言（#2365）。
    ///
    /// (バリアント, 言語) の網羅 match なので、バリアントを足すと日英の両方を書くまで
    /// コンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::ChangelogUntracked, Ja) => "git 管理外のファイル".into(),
            (Self::ChangelogUntracked, En) => "the file is not tracked by git".into(),
            (Self::DiskDiffDisk(path), Ja) => format!("{path}（ディスク）"),
            (Self::DiskDiffDisk(path), En) => format!("{path} (on disk)"),
            (Self::DiskDiffEditing(path), Ja) => format!("{path}（編集中）"),
            (Self::DiskDiffEditing(path), En) => format!("{path} (editing)"),
            (Self::TreeRemoteRowStays, Ja) => "リモート（SSH）の行はキーで動かさない".into(),
            (Self::TreeRemoteRowStays, En) => "remote (SSH) rows do not move with the keys".into(),

            (Self::AutosuggestAppliesTo, Ja) => {
                "既存ペインを含む tako 内の zsh（次のプロンプトから反映）".into()
            }
            (Self::AutosuggestAppliesTo, En) => {
                "zsh inside tako, including existing panes (takes effect from the next prompt)"
                    .into()
            }
            (Self::AutosuggestNote, Ja) => "ユーザーが自前で zsh-autosuggestions を導入しているペインでは \
                    tako は注入せず、この設定も効かない（二重注入ガード）。\
                    Tab 確定が働くのはゴースト表示中かつカーソルが行末のときだけで、\
                    それ以外の Tab は従来の補完のまま（tab=false にすると常に補完）"
                .into(),
            (Self::AutosuggestNote, En) => {
                "in panes where the user installed zsh-autosuggestions themselves, tako does not \
                 inject it and this setting has no effect (double-injection guard). Tab accepts \
                 only while a suggestion is shown and the cursor is at the end of the line; any \
                 other Tab keeps the usual completion (tab=false always completes)"
                    .into()
            }
            (Self::ModelsApplyCommand, Ja) => {
                "tako orchestrator profiles set <プロファイル> --model <id> --effort <値>".into()
            }
            (Self::ModelsApplyCommand, En) => {
                "tako orchestrator profiles set <profile> --model <id> --effort <value>".into()
            }
            (Self::BatterySettingsOpened, Ja) => "System Settings の Battery を開きました".into(),
            (Self::BatterySettingsOpened, En) => "opened Battery in System Settings".into(),
            (Self::SetupTimedOut, Ja) => "tako setup が上限内に終わらなかったので打ち切った。\
                     ターミナルで tako setup を実行して続きを確認すること"
                .into(),
            (Self::SetupTimedOut, En) => {
                "tako setup did not finish within the limit, so it was stopped. Run tako setup \
                 in a terminal to see the rest"
                    .into()
            }

            (Self::SoloLimitResumeScope, Ja) => "solo プロファイルは worker を spawn しないので、limit_resume が効くのは `tako solo` で立てた本人のペインだけです（worker への配布はありません）。\n  ペイン単位で切り替えるには `tako limit-resume on --pane <id>` を使ってください".into(),
            (Self::SoloLimitResumeScope, En) => {
                "a solo profile does not spawn workers, so limit_resume applies only to the pane \
                 started by `tako solo` itself (nothing is handed to workers).\n  To switch it \
                 per pane, use `tako limit-resume on --pane <id>`"
                    .into()
            }
            (Self::CodexBypassSandboxOff, Ja) => "codex は承認スキップとサンドボックス解除が同じフラグ（--dangerously-bypass-approvals-and-sandbox）なので、bypass_sandbox が false のあいだ skip_permissions は効きません（コマンド実行に承認プロンプトが出ます）。\n  外す場合は `--bypass-sandbox true`（サンドボックスと承認が両方無効になります）".into(),
            (Self::CodexBypassSandboxOff, En) => {
                "codex uses one flag (--dangerously-bypass-approvals-and-sandbox) for both \
                 skipping approvals and lifting the sandbox, so skip_permissions has no effect \
                 while bypass_sandbox is false (commands ask for approval).\n  To lift it, use \
                 `--bypass-sandbox true` (this disables both the sandbox and approvals)"
                    .into()
            }
            (
                Self::ProfileRemoteControlBlocked {
                    detail,
                    reason,
                    next_step,
                },
                Ja,
            ) => format!(
                "remote_control: true ですが、この環境では有効にできません（{}）。\n  {} / {}",
                detail,
                reason.text_in(Ja),
                next_step.text_in(Ja)
            ),
            (
                Self::ProfileRemoteControlBlocked {
                    detail,
                    reason,
                    next_step,
                },
                En,
            ) => format!(
                "remote_control is true, but it cannot be enabled in this environment ({}).\n  \
                 {} / {}",
                detail,
                reason.text_in(En),
                next_step.text_in(En)
            ),
            (Self::AccountsDefaultConfigDir(cd), Ja) => format!(
                "config_dir '{cd}' は claude の既定パスです。既定パスを明示指定すると \
                 CLAUDE_CONFIG_DIR が設定された状態になり、claude が Keychain の別エントリ\
                 （ハッシュ付き）を見に行くため既存ログインが未ログイン扱いになります。\
                 既定の資格情報をそのまま使うなら inherit=true で登録してください（#512）"
            ),
            (Self::AccountsDefaultConfigDir(cd), En) => format!(
                "config_dir '{cd}' is claude's default path. Setting the default path explicitly \
                 sets CLAUDE_CONFIG_DIR, so claude looks up a different Keychain entry (with a \
                 hash) and the existing login is treated as logged out. To keep using the default \
                 credentials, register it with inherit=true (#512)"
            ),
            (Self::PolicySameTabNoGuarantee, Ja) => "worker は常に spawn 元と同じタブへ置く（タブ分けなし）。桁数の保証なし（min_worker_cols=0）".into(),
            (Self::PolicySameTabNoGuarantee, En) => {
                "workers always go into the same tab as the spawning pane (no tab split). No \
                 column guarantee (min_worker_cols=0)"
                    .into()
            }
            (Self::PolicySameTabNoShrink(min_cols), Ja) => format!(
                "worker は常に spawn 元と同じタブへ置く（タブ分けなし）。下限 {} 桁を割っても自動縮小はしない（auto_shrink_font=false）",
                min_cols
            ),
            (Self::PolicySameTabNoShrink(min_cols), En) => format!(
                "workers always go into the same tab as the spawning pane (no tab split). The \
                 font is not shrunk even below the minimum of {min_cols} columns \
                 (auto_shrink_font=false)"
            ),
            (Self::PolicySameTabShrink { min_cols, floor_pct }, Ja) => format!(
                "worker は常に spawn 元と同じタブへ置く（タブ分けなし）。下限 {} 桁を割るときは worker ペインのフォントを自動で縮め、床は既定サイズの {}%（届かなければ床で置き cols_short を立てる）",
                min_cols, floor_pct
            ),
            (Self::PolicySameTabShrink { min_cols, floor_pct }, En) => format!(
                "workers always go into the same tab as the spawning pane (no tab split). Below \
                 the minimum of {min_cols} columns the worker pane's font shrinks automatically, \
                 down to {floor_pct}% of the default size (if that is still short, it stays at \
                 the floor and sets cols_short)"
            ),

            (
                Self::SelfProfileDefaulted {
                    pane,
                    role,
                    profile,
                    solo_pane,
                },
                Ja,
            ) => {
                let why = if *solo_pane {
                    "solo の profile は起動時の env（`solo:<名前>`）にしか無く、ペインの role ラベルからは解けない"
                } else {
                    "master / solo のペインではない"
                };
                format!(
                    "pane {}（role={role}）の profile が {} へ落ちている: {why}。\
                     master を名指しするか、`--pane` を省いて呼び出し元を解決させる",
                    pane, profile
                )
            }
            (
                Self::SelfProfileDefaulted {
                    pane,
                    role,
                    profile,
                    solo_pane,
                },
                En,
            ) => {
                let why = if *solo_pane {
                    "a solo profile lives only in the env at startup (`solo:<name>`) and cannot \
                     be read from the pane's role label"
                } else {
                    "it is not a master / solo pane"
                };
                format!(
                    "the profile of pane {pane} (role={role}) fell back to {profile}: {why}. Name \
                     the master pane, or omit `--pane` to resolve the caller"
                )
            }
            (
                Self::CtxThresholdClamped {
                    raw,
                    value,
                    min,
                    max,
                },
                Ja,
            ) => format!("ctx_threshold={raw} は値域外のため {value} へ丸めた（{min}〜{max}）"),
            (
                Self::CtxThresholdClamped {
                    raw,
                    value,
                    min,
                    max,
                },
                En,
            ) => format!("ctx_threshold={raw} is out of range, so it was rounded to {value} ({min}-{max})"),
            (Self::PreviousMasterClosedBySuccessor, Ja) => {
                "後任 master が引き継ぎ確認後に閉じる".into()
            }
            (Self::PreviousMasterClosedBySuccessor, En) => {
                "the successor master closes it after confirming the handoff".into()
            }
            (Self::PreviousMasterKept, Ja) => {
                "旧 master ペインを特定できなかったため閉じない".into()
            }
            (Self::PreviousMasterKept, En) => {
                "the previous master pane could not be identified, so it is not closed".into()
            }

            (
                Self::FontFitShort {
                    pct,
                    size,
                    cols,
                    min_cols,
                },
                Ja,
            ) => format!(
                "同じタブに置いた。フォントを床の {pct}%（{size}）まで縮めたが {} 桁で下限 {min_cols} 桁に届かない。\n  直すには: ウィンドウを広げる / worker を減らす / tako orchestrator layout --min-worker-cols <桁>",
                cols
            ),
            (
                Self::FontFitShort {
                    pct,
                    size,
                    cols,
                    min_cols,
                },
                En,
            ) => format!(
                "placed in the same tab. The font was shrunk to the floor of {pct}% ({size}), but \
                 at {cols} columns it is below the minimum of {min_cols}.\n  To fix: widen the \
                 window / reduce workers / tako orchestrator layout --min-worker-cols <columns>"
            ),
            (
                Self::FontFitShrunk {
                    pct,
                    size,
                    cols,
                    min_cols,
                },
                Ja,
            ) => format!(
                "同じタブに置き、worker ペインのフォントを {pct}%（{size}）へ縮めて {} 桁を確保した（下限 {min_cols} 桁）",
                cols
            ),
            (
                Self::FontFitShrunk {
                    pct,
                    size,
                    cols,
                    min_cols,
                },
                En,
            ) => format!(
                "placed in the same tab and shrank the worker pane's font to {pct}% ({size}) to \
                 get {cols} columns (minimum {min_cols})"
            ),
            (
                Self::PlacementWindowTooNarrow {
                    full_cols,
                    min_cols,
                },
                Ja,
            ) => format!(
                "ウィンドウ幅がそもそも下限に届かない（タブ全幅 {} 桁 < 下限 {min_cols} 桁）ので同じタブへ置いた。\n  直すには: ウィンドウを広げる / 文字サイズを下げる / tako orchestrator layout --min-worker-cols <桁>",
                full_cols
            ),
            (
                Self::PlacementWindowTooNarrow {
                    full_cols,
                    min_cols,
                },
                En,
            ) => format!(
                "the window itself is narrower than the minimum (full tab {full_cols} columns < \
                 minimum {min_cols}), so it was placed in the same tab.\n  To fix: widen the \
                 window / lower the font size / tako orchestrator layout --min-worker-cols \
                 <columns>"
            ),
            (
                Self::PlacementOverflowTab {
                    predicted,
                    min_cols,
                    tab,
                    cols,
                },
                Ja,
            ) => format!(
                "同じタブへ置くと worker ペインが {predicted} 桁（下限 {min_cols} 桁）になるので、\n  すでにこの master の worker が居るタブ {} へ置いた（{cols} 桁）",
                tab
            ),
            (
                Self::PlacementOverflowTab {
                    predicted,
                    min_cols,
                    tab,
                    cols,
                },
                En,
            ) => format!(
                "in the same tab the worker pane would get {predicted} columns (minimum \
                 {min_cols}), so\n  it was placed in tab {tab}, which already has this master's \
                 workers ({cols} columns)"
            ),
            (
                Self::PlacementNewTab {
                    predicted,
                    min_cols,
                },
                Ja,
            ) => format!(
                "同じタブへ置くと worker ペインが {predicted} 桁（下限 {min_cols} 桁）になるので、新しいタブへ置いた。\n  同じタブへ詰めたいときは: tako orchestrator layout --min-worker-cols <桁>（0 で保証しない）"
            ),
            (
                Self::PlacementNewTab {
                    predicted,
                    min_cols,
                },
                En,
            ) => format!(
                "in the same tab the worker pane would get {predicted} columns (minimum \
                 {min_cols}), so it was placed in a new tab.\n  To pack it into the same tab: \
                 tako orchestrator layout --min-worker-cols <columns> (0 = no guarantee)"
            ),
            (
                Self::SpawnRemoteControlBlocked {
                    detail,
                    reason,
                    next_step,
                },
                Ja,
            ) => format!(
                "remote_control: true ですが有効にできませんでした（{}）。{} / {}",
                detail,
                reason.text_in(Ja),
                next_step.text_in(Ja)
            ),
            (
                Self::SpawnRemoteControlBlocked {
                    detail,
                    reason,
                    next_step,
                },
                En,
            ) => format!(
                "remote_control is true, but it could not be enabled ({}). {} / {}",
                detail,
                reason.text_in(En),
                next_step.text_in(En)
            ),

            (Self::ModTurnWithoutDialog(turn), Ja) => format!(
                "mod は {} を報告しているが、画面に選択肢ダイアログが見えない\
                 （respond は画面のダイアログを読むので、画面に出るまで応答できない）",
                turn
            ),
            (Self::ModTurnWithoutDialog(turn), En) => format!(
                "the mod reports {turn}, but no choice dialog is visible on the screen (respond \
                 reads the dialog on the screen, so it cannot answer until the dialog appears)"
            ),
            (
                Self::PermissionDialogUnreported {
                    turn,
                    classic_events_off,
                },
                Ja,
            ) => {
                let why = if *classic_events_off {
                    "classic 系のイベントが mod へ届かない環境（classic_events=false）では、\
                     ルールの無い ask の権限ダイアログを mod から拾えない\
                     （ask ルール由来なら次の報告 = 1 秒以内で揃う）"
                } else {
                    "mod の報告がまだ追いついていない"
                };
                format!(
                    "画面に権限ダイアログがあるが mod は {}（{why}。画面の検出を採用）",
                    turn
                )
            }
            (
                Self::PermissionDialogUnreported {
                    turn,
                    classic_events_off,
                },
                En,
            ) => {
                let why = if *classic_events_off {
                    "where classic events do not reach the mod (classic_events=false), the mod \
                     cannot pick up an ask permission dialog that has no rule (one from an ask \
                     rule lines up with the next report, within 1 second)"
                } else {
                    "the mod's report has not caught up yet"
                };
                format!(
                    "a permission dialog is on the screen, but the mod reports {turn} ({why}; the \
                     screen detection is used)"
                )
            }
            (Self::StalledWithoutChildren, Ja) => {
                "busy と判定されたが実行中の子プロセスが無く、画面の busy パターンも無い".into()
            }
            (Self::StalledWithoutChildren, En) => {
                "judged busy, but there is no running child process and no busy pattern on the \
                 screen"
                    .into()
            }
            (Self::StaleNoBackend, Ja) => "バックエンドセッションなし（stale 検知対象外）".into(),
            (Self::StaleNoBackend, En) => {
                "no backend session (not subject to stale detection)".into()
            }

            (Self::SessionRestartHarness, Ja) => {
                "エージェントのプロセスを終了させ、落ちてシェルが戻ったのを確かめてから \
                 元の起動と同じ引数 + --resume の行を送達確認つきで送る（会話はそのまま続く）。\
                 送った後もすぐ落ちないかを見張り、結果は tako session-restart の last_restart に出る"
                    .into()
            }
            (Self::SessionRestartHarness, En) => {
                "ends the agent process, confirms the shell came back, then sends the line with \
                 the original launch arguments + --resume and checks that it arrived (the \
                 conversation continues). It then watches that the agent does not exit right \
                 away; the result shows in last_restart of tako session-restart"
                    .into()
            }
            (Self::SessionRestartHandoff, Ja) => {
                "引き継ぎの書き直しと tako_orchestrator_handoff の呼び出しを master へ依頼した。\
                 後任 master が同じタブに立ち、引き継ぎを確認してから前任のペインを閉じる \
                 （前任を閉じるのは後任なので、後任の起動が失敗しても master を失わない）。\
                 依頼が届いたかは tako session-restart の handoff_delivery に出る"
                    .into()
            }
            (Self::SessionRestartHandoff, En) => {
                "asked the master to rewrite the handoff and call tako_orchestrator_handoff. The \
                 successor master starts in the same tab and closes the previous pane after \
                 confirming the handoff (the successor closes it, so a failed start does not lose \
                 the master). Whether the request arrived shows in handoff_delivery of tako \
                 session-restart"
                    .into()
            }

            (Self::ResumeNoProfileRecord, Ja) => {
                "spawn した master のプロファイルの記録が無い（#2470 より前に spawn した worker）ので、\
                 既定プロファイルの設定で組んだ"
                    .into()
            }
            (Self::ResumeNoProfileRecord, En) => {
                "there is no record of the spawning master's profile (a worker spawned before \
                 #2470), so the default profile's settings were used"
                    .into()
            }
            (
                Self::ResumingWorker {
                    worker,
                    session,
                    profile,
                },
                Ja,
            ) => format!(
                "worker {worker} の会話（{session}…）を spawn と同じ起動で戻します（プロファイル: {}）",
                profile
            ),
            (
                Self::ResumingWorker {
                    worker,
                    session,
                    profile,
                },
                En,
            ) => format!(
                "resuming the conversation of worker {worker} ({session}…) with the same launch \
                 as spawn (profile: {profile})"
            ),

            (Self::ErrorLabel, Ja) => "エラー".into(),
            (Self::ErrorLabel, En) => "error".into(),
            (Self::WarningLabel, Ja) => "警告".into(),
            (Self::WarningLabel, En) => "warning".into(),
            (Self::InfoLabel, Ja) => "[情報]".into(),
            (Self::InfoLabel, En) => "[info]".into(),

            (Self::TelemetryRecent(n), Ja) => format!("  直近のレポート件数: {n}"),
            (Self::TelemetryRecent(n), En) => format!("  recent reports: {n}"),
            (Self::TelemetryQueued(n), Ja) => format!("  未送信キュー: {n}"),
            (Self::TelemetryQueued(n), En) => format!("  queued (not sent): {n}"),
            (Self::TelemetryLog(path), Ja) => format!("  ログ: {path}"),
            (Self::TelemetryLog(path), En) => format!("  log: {path}"),

            (Self::FdaGranted, Ja) => "[OK] フルディスクアクセス: 付与済み".into(),
            (Self::FdaGranted, En) => "[OK] Full Disk Access: granted".into(),
            (Self::FdaNotGranted, Ja) => "[任意] フルディスクアクセス: 未付与".into(),
            (Self::FdaNotGranted, En) => "[optional] Full Disk Access: not granted".into(),
            (Self::FdaDialogMayAppear, Ja) => {
                "  フォルダアクセス時に macOS の許可ダイアログが表示されることがあります".into()
            }
            (Self::FdaDialogMayAppear, En) => {
                "  macOS may show a permission dialog when tako accesses a folder".into()
            }
            (Self::FdaHowToGrant, Ja) => {
                "  付与方法: tako fda open → システム設定で tako を追加".into()
            }
            (Self::FdaHowToGrant, En) => {
                "  to grant it: tako fda open → add tako in System Settings".into()
            }
            (Self::FdaSettingsOpened, Ja) => {
                "システム設定を開きました。tako を「フルディスクアクセス」に追加してください".into()
            }
            (Self::FdaSettingsOpened, En) => {
                "opened System Settings. Add tako to Full Disk Access".into()
            }
            (Self::FdaNotApplicable, Ja) => {
                "macOS 以外のプラットフォームでは TCC は適用されません".into()
            }
            (Self::FdaNotApplicable, En) => "TCC does not apply on platforms other than macOS".into(),
            (Self::FdaGrantedDescription, Ja) => {
                "フルディスクアクセスが付与済みです。フォルダアクセスの許可ダイアログは表示されません"
                    .into()
            }
            (Self::FdaGrantedDescription, En) => {
                "Full Disk Access is granted. No permission dialog appears when tako accesses a \
                 folder"
                    .into()
            }
            (Self::FdaNotGrantedDescription, Ja) => {
                "フルディスクアクセスが未付与です。フォルダアクセス時に許可ダイアログが表示されることがあります"
                    .into()
            }
            (Self::FdaNotGrantedDescription, En) => {
                "Full Disk Access is not granted. A permission dialog may appear when tako \
                 accesses a folder"
                    .into()
            }

            (Self::RunInteractiveStarted { pane, status }, Ja) => {
                format!("pane {pane} で対話コマンドを起動しました（status: {}）", status)
            }
            (Self::RunInteractiveStarted { pane, status }, En) => {
                format!("started the interactive command in pane {pane} (status: {status})")
            }
            (Self::RunStarted { pane, command }, Ja) => {
                format!("pane {pane} でコマンドを実行中（command: {}）", command)
            }
            (Self::RunStarted { pane, command }, En) => {
                format!("running the command in pane {pane} (command: {command})")
            }
            (Self::RunSavedBefore(panes), Ja) => {
                format!("実行前に未保存の編集を保存しました（pane {}）", panes)
            }
            (Self::RunSavedBefore(panes), En) => {
                format!("saved unsaved edits before running (pane {panes})")
            }
            (Self::RunWaitBudget(secs), Ja) => format!("終わるまで最長 {} 秒待ちます", secs),
            (Self::RunWaitBudget(secs), En) => {
                format!("waiting up to {secs} seconds for it to finish")
            }

            (Self::McpPathMissing(cmd), Ja) => format!(
                "[警告] MCP 登録パスが消失しています: {cmd}\n        \
                 tako MCP なしで起動します。tako setup-mcp で修復してください。"
            ),
            (Self::McpPathMissing(cmd), En) => format!(
                "[warning] the registered MCP path is gone: {cmd}\n        \
                 starting without the tako MCP. Run tako setup-mcp to repair it."
            ),
            (Self::OneMModelWarning { source, model }, Ja) => format!(
                "[警告] {source} のモデル '{model}' は 1M コンテキスト版のため、Pro プランでは起動できない可能性があります（Max / API プラン向け）。\n  \
                 起動に失敗する場合は `tako orchestrator profiles set <プロファイル名> --clear-model` で解除してください"
            ),
            (Self::OneMModelWarning { source, model }, En) => format!(
                "[warning] the {source} model '{model}' is a 1M-context model, so it may not start \
                 on the Pro plan (it is meant for the Max / API plans).\n  \
                 If it fails to start, clear it with \
                 `tako orchestrator profiles set <profile> --clear-model`"
            ),
            (Self::MasterLaunched(location), Ja) => format!("master を起動しました: {location}"),
            (Self::MasterLaunched(location), En) => format!("launched master: {location}"),
            (Self::SoloLaunched(location), Ja) => format!("solo を起動しました: {location}"),
            (Self::SoloLaunched(location), En) => format!("launched solo: {location}"),
            (Self::LaunchTabNamed { title, pane }, Ja) => format!("タブ '{title}'（ペイン {pane}）"),
            (Self::LaunchTabNamed { title, pane }, En) => format!("tab '{title}' (pane {pane})"),
            (Self::LaunchNewTab(pane), Ja) => format!("新しいタブ（ペイン {pane}）"),
            (Self::LaunchNewTab(pane), En) => format!("a new tab (pane {pane})"),
            (Self::LaunchInline(pane), Ja) => format!("ペイン {pane}（インライン）"),
            (Self::LaunchInline(pane), En) => format!("pane {pane} (inline)"),
            (
                Self::LaunchProfile {
                    profile,
                    agent,
                    model,
                    effort,
                },
                Ja,
            ) => format!(
                "プロファイル: {profile}（エージェント: {agent}、モデル: {model}、effort: {effort}）"
            ),
            (
                Self::LaunchProfile {
                    profile,
                    agent,
                    model,
                    effort,
                },
                En,
            ) => format!("profile: {profile} (agent: {agent}, model: {model}, effort: {effort})"),
            (Self::LimitResumeOn, Ja) => "リミット後の自動復帰: 有効（プロファイル既定。#1140）".into(),
            (Self::LimitResumeOn, En) => {
                "auto-resume after usage limits: on (profile default; #1140)".into()
            }
            (Self::WorkerPolicyInherit { model, effort }, Ja) => {
                format!("worker モデルポリシー: inherit（master と同じ {model} / {effort}）")
            }
            (Self::WorkerPolicyInherit { model, effort }, En) => {
                format!("worker model policy: inherit (same as master: {model} / {effort})")
            }
            (
                Self::WorkerPolicyInheritNonClaude {
                    agent,
                    model,
                    effort,
                },
                Ja,
            ) => format!(
                "worker モデルポリシー: inherit（master は {agent} のため claude worker へは非継承: {model} / {effort}）"
            ),
            (
                Self::WorkerPolicyInheritNonClaude {
                    agent,
                    model,
                    effort,
                },
                En,
            ) => format!(
                "worker model policy: inherit (the master is {agent}, so claude workers do not \
                 inherit it: {model} / {effort})"
            ),
            (Self::WorkerPolicyFixed { model, effort }, Ja) => {
                format!("worker モデルポリシー: fixed（{model} / {effort}）")
            }
            (Self::WorkerPolicyFixed { model, effort }, En) => {
                format!("worker model policy: fixed ({model} / {effort})")
            }
            (Self::WorkerPolicyDelegate, Ja) => {
                "worker モデルポリシー: delegate（master が判断）".into()
            }
            (Self::WorkerPolicyDelegate, En) => {
                "worker model policy: delegate (the master decides)".into()
            }
            (Self::ProjectsRestriction(keys), Ja) => format!("projects 制限: {keys}"),
            (Self::ProjectsRestriction(keys), En) => format!("projects restriction: {keys}"),
            (Self::TreeFoldersAdded(names), Ja) => format!("ファイルツリーに追加: {names}"),
            (Self::TreeFoldersAdded(names), En) => format!("added to the file tree: {names}"),
            (Self::SoloMode, Ja) => {
                "モード: solo（オーケストレーション無し・1 対 1 対話・worker spawn 禁止）".into()
            }
            (Self::SoloMode, En) => {
                "mode: solo (no orchestration, one-to-one conversation, no worker spawns)".into()
            }
            (Self::MasterAccount { name, config_dir }, Ja) => {
                format!("アカウント: {name}（config dir: {config_dir}）")
            }
            (Self::MasterAccount { name, config_dir }, En) => {
                format!("account: {name} (config dir: {config_dir})")
            }
            (Self::MasterAccountDefault(name), Ja) => {
                format!("アカウント: {name}（既定の資格情報 / CLAUDE_CONFIG_DIR 未設定）")
            }
            (Self::MasterAccountDefault(name), En) => {
                format!("account: {name} (default credentials; CLAUDE_CONFIG_DIR not set)")
            }
            (Self::RemoteControlEnableHint(cmd), Ja) => format!("  有効にする場合: {cmd}"),
            (Self::RemoteControlEnableHint(cmd), En) => format!("  to turn it on: {cmd}"),
            (Self::SandboxBypassHint(cmd), Ja) => format!("  外す場合: {cmd}"),
            (Self::SandboxBypassHint(cmd), En) => format!("  to turn it off: {cmd}"),
            (Self::StalePaneIdRemapped { old, pane }, Ja) => format!(
                "[情報] TAKO_PANE_ID={old} は旧世代のペイン ID です（アプリ再起動をまたいだ値）\n  \
                 現世代のペイン {pane} へ読み替えて起動します"
            ),
            (Self::StalePaneIdRemapped { old, pane }, En) => format!(
                "[info] TAKO_PANE_ID={old} is a pane ID from an earlier app session (the value \
                 survived an app restart)\n  launching in pane {pane} of the current session \
                 instead"
            ),
            (Self::PaneIdMismatch { old, pane }, Ja) => format!(
                "[情報] TAKO_PANE_ID={old} は呼び出し元ペインと一致しません（シェルが古い値を持っています）\n  \
                 実際の呼び出し元ペイン {pane} で起動します"
            ),
            (Self::PaneIdMismatch { old, pane }, En) => format!(
                "[info] TAKO_PANE_ID={old} does not match the calling pane (this shell holds a \
                 stale value)\n  launching in the actual calling pane {pane}"
            ),
            (Self::UnsetPaneIdHint, Ja) => {
                "  このシェルを使い続けるなら: unset TAKO_PANE_ID".into()
            }
            (Self::UnsetPaneIdHint, En) => "  to keep using this shell: unset TAKO_PANE_ID".into(),
            (Self::CallerPaneUnknownStale(old), Ja) => format!(
                "[情報] 呼び出し元ペインを特定できません（TAKO_PANE_ID={old} は現在の tako に無い古い値）"
            ),
            (Self::CallerPaneUnknownStale(old), En) => format!(
                "[info] cannot identify the calling pane (TAKO_PANE_ID={old} is a stale value the \
                 running tako does not have)"
            ),
            (Self::CallerPaneUnknownUnset, Ja) => {
                "[情報] 呼び出し元ペインを特定できません（TAKO_PANE_ID 未設定）".into()
            }
            (Self::CallerPaneUnknownUnset, En) => {
                "[info] cannot identify the calling pane (TAKO_PANE_ID is not set)".into()
            }
            (Self::LaunchingInNewTab, Ja) => "  新しいタブを作ってそこで起動します".into(),
            (Self::LaunchingInNewTab, En) => "  opening a new tab and launching there".into(),

            (Self::SshDetectionNoGui, Ja) => "GUI が接続していないので検知状態を読めない".into(),
            (Self::SshDetectionNoGui, En) => {
                "cannot read the detection state because no GUI is connected".into()
            }

            (Self::ProfileProjectUnregistered(key), Ja) => format!(
                "プロジェクト '{key}' は projects.yaml に登録されていません（この状態では起動時にエラーになります）。\n  `tako orchestrator projects add {key} <パス>` で登録してください"
            ),
            (Self::ProfileProjectUnregistered(key), En) => format!(
                "project '{key}' is not registered in projects.yaml (launching in this state \
                 fails).\n  Register it with `tako orchestrator projects add {key} <path>`"
            ),
            (Self::ProfileAccountUnregistered { label, name }, Ja) => format!(
                "{label} のアカウント '{name}' は accounts.yaml に登録されていません（この状態では起動時にエラーになります）。\n  `tako orchestrator accounts add {name} --inherit` で登録してください"
            ),
            (Self::ProfileAccountUnregistered { label, name }, En) => format!(
                "the account '{name}' set in {label} is not registered in accounts.yaml \
                 (launching in this state fails).\n  Register it with \
                 `tako orchestrator accounts add {name} --inherit`"
            ),
            (Self::AdoptGenericTarget(target), Ja) => format!(
                "'{target}' は管轄プロジェクトを持たない汎用プロファイル（projects 未指定）。\
                 引き継ぎの宛先は変わるが管轄は決まらない"
            ),
            (Self::AdoptGenericTarget(target), En) => format!(
                "'{target}' is a generic profile with no projects of its own (projects is not \
                 set). The handoff destination changes, but no projects are assigned"
            ),
            (
                Self::AdoptPromptDiffers {
                    target,
                    launched_as,
                },
                Ja,
            ) => format!(
                "'{target}' は起動時の '{launched_as}' と違う system prompt を持っている\
                 （個別の prompt_blocks / system_prompt）。走っている会話の prompt は\
                 差し替えられないので、prompt ごと '{target}' にしたいときは \
                 tako orchestrator handoff で立て直す"
            ),
            (
                Self::AdoptPromptDiffers {
                    target,
                    launched_as,
                },
                En,
            ) => format!(
                "'{target}' has a different system prompt from the launch profile \
                 '{launched_as}' (its own prompt_blocks / system_prompt). The prompt of a running \
                 conversation cannot be replaced, so to switch the prompt to '{target}' as well, \
                 restart with tako orchestrator handoff"
            ),
            (Self::CtxModDrift { delta, percent }, Ja) => format!(
                "ctx% の画面と mod の報告が {delta} ポイントずれています\
                 （mod の値 {percent}% を採用。statusLine の更新待ちなら次のターンで揃います。\
                 揃わなければ画面の読み取りか mod の報告を疑う）"
            ),
            (Self::CtxModDrift { delta, percent }, En) => format!(
                "the ctx% on screen and the one reported by mod differ by {delta} points (using \
                 mod's {percent}%. If the statusLine has not refreshed yet, they match from the \
                 next turn; if they still differ, suspect the screen reading or the mod report)"
            ),
            (Self::CtxTranscriptDrift { delta, window }, Ja) => format!(
                "ctx% の画面と transcript が {delta} ポイントずれています\
                 （transcript 側で使った文脈窓 {window}。上流が窓の決め方を変えた合図。\
                 画面の値を採用しています）"
            ),
            (Self::CtxTranscriptDrift { delta, window }, En) => format!(
                "the ctx% on screen and the transcript differ by {delta} points (the transcript \
                 used a context window of {window}, a sign that upstream changed how the window \
                 is chosen; using the on-screen value)"
            ),
            (Self::ResumeProfileMissing { kind, name }, Ja) => format!(
                "{} プロファイル '{name}' が見つからないので既定プロファイルの設定で再開した\
                 （`{} {name}` のようにラベルだけで起動した master と同じ組み立て）",
                kind.as_str(),
                kind.launch_bin()
            ),
            (Self::ResumeProfileMissing { kind, name }, En) => format!(
                "{} profile '{name}' was not found, so the conversation was resumed with the \
                 default profile's settings (the same setup as a master started with just a \
                 label, like `{} {name}`)",
                kind.as_str(),
                kind.launch_bin()
            ),
            (Self::ResumeWorkerAccountUnresolved { name, why }, Ja) => format!(
                "worker のアカウント '{name}' を引けないので、プロファイルの env だけで再開した: {}",
                why.text_in(lang)
            ),
            (Self::ResumeWorkerAccountUnresolved { name, why }, En) => format!(
                "cannot resolve the worker's account '{name}', so the conversation was resumed \
                 with only the profile's env: {}",
                why.text_in(lang)
            ),
            (Self::ResumeConfigDirPinned(path), Ja) => format!(
                "プロファイルの設定 dir（{path}）に会話の記録が無いので、記録のある設定 dir で再開した"
            ),
            (Self::ResumeConfigDirPinned(path), En) => format!(
                "the profile's config dir ({path}) has no record of the conversation, so it was \
                 resumed in the config dir that has it"
            ),
            (Self::ResumeCodexHomePinned { current, home }, Ja) => format!(
                "プロファイルの CODEX_HOME（{current}）に会話の記録が無いので、記録のある置き場（{home}）で再開した"
            ),
            (Self::ResumeCodexHomePinned { current, home }, En) => format!(
                "the profile's CODEX_HOME ({current}) has no record of the conversation, so it \
                 was resumed in the location that has it ({home})"
            ),
            (Self::McpSettingsUnwritable(why), Ja) => format!(
                "user scope の MCP を止める設定ファイルを書けなかったので、今回は全部持ち込んで起動する: {}",
                why.text_in(lang)
            ),
            (Self::McpSettingsUnwritable(why), En) => format!(
                "could not write the settings file that turns off user-scope MCP servers, so this \
                 launch brings them all in: {}",
                why.text_in(lang)
            ),
            (Self::McpSettingsNoDataDir, Ja) => {
                "user scope の MCP を止める設定ファイルの置き場が無いので、今回は全部持ち込んで起動する: データディレクトリを特定できない"
                    .into()
            }
            (Self::McpSettingsNoDataDir, En) => {
                "there is no place for the settings file that turns off user-scope MCP servers, \
                 so this launch brings them all in: cannot determine the data directory"
                    .into()
            }
            (Self::McpKeepMissing(name), Ja) => format!(
                "プロジェクトの mcp_servers の '{name}' は user scope にもプロジェクトの .mcp.json にも見つからない（今は何も足されない）"
            ),
            (Self::McpKeepMissing(name), En) => format!(
                "'{name}' in the project's mcp_servers was not found in the user scope or in the \
                 project's .mcp.json (nothing is added for now)"
            ),
            (Self::McpSettingsOverridden, Ja) => {
                "プロファイルの worker_agents.claude.args に --settings があるため、user scope の MCP を止める設定が効かない（claude は後ろの --settings だけを読む）"
                    .into()
            }
            (Self::McpSettingsOverridden, En) => {
                "the profile's worker_agents.claude.args has --settings, so the setting that \
                 turns off user-scope MCP servers has no effect (claude reads only the last \
                 --settings)"
                    .into()
            }
            (Self::McpCodexHomeUnresolved, Ja) => {
                "codex の設定の置き場（CODEX_HOME）を決められないので、今回は user 設定の MCP を全部持ち込んで起動する"
                    .into()
            }
            (Self::McpCodexHomeUnresolved, En) => {
                "cannot determine where codex's settings are (CODEX_HOME), so this launch brings \
                 in all MCP servers from the user settings"
                    .into()
            }
            (Self::McpCodexConfigUnreadable(e), Ja) => {
                let why = match e {
                    CodexConfigError::Read { path, error } => format!("{path} を読めない: {error}"),
                    CodexConfigError::Toml { path, error } => {
                        format!("{path} を TOML として読めない: {error}")
                    }
                };
                format!(
                    "codex の設定を読めないので、今回は user 設定の MCP に何も足さずに起動する: {why}"
                )
            }
            (Self::McpCodexConfigUnreadable(e), En) => {
                let why = match e {
                    CodexConfigError::Read { path, error } => {
                        format!("cannot read {path}: {error}")
                    }
                    CodexConfigError::Toml { path, error } => {
                        format!("{path} is not valid TOML: {error}")
                    }
                };
                format!(
                    "cannot read codex's settings, so this launch leaves the MCP servers of the \
                     user settings as they are: {why}"
                )
            }
            (Self::McpCodexKeepMissing(name), Ja) => format!(
                "プロジェクトの mcp_servers の '{name}' は codex の config.toml にもプロジェクトの .codex/config.toml にも見つからない（今は何も足されない）"
            ),
            (Self::McpCodexKeepMissing(name), En) => format!(
                "'{name}' in the project's mcp_servers was not found in codex's config.toml or \
                 in the project's .codex/config.toml (nothing is added for now)"
            ),
            (Self::McpCodexNameInexpressible(name), Ja) => format!(
                "codex の -c は名前の '.' や空白などを表せないので、'{name}' は止めずに持ち込む"
            ),
            (Self::McpCodexNameInexpressible(name), En) => format!(
                "codex's -c cannot express names with '.', spaces and the like, so '{name}' is \
                 brought in without being turned off"
            ),

            (Self::HandoffMigrationDeferred(e), Ja) => format!(
                "projects.yaml が読めないため移行を見送った: {}",
                e.text_in(lang)
            ),
            (Self::HandoffMigrationDeferred(e), En) => format!(
                "the handoff migration was postponed because projects.yaml cannot be read: {}",
                e.text_in(lang)
            ),
            (Self::HandoffMigrationWriteFailed(e), _) => e.text_in(lang),
            (Self::HandoffMigrationArchiveFailed(e), Ja) => format!(
                "移行前のファイルを退避できないため中止した: {}",
                e.text_in(lang)
            ),
            (Self::HandoffMigrationArchiveFailed(e), En) => format!(
                "the handoff migration was stopped because the original file could not be \
                 archived: {}",
                e.text_in(lang)
            ),
            (Self::HandoffMigrationMemoKept, Ja) => {
                "移せないプロジェクトがあったため運用メモの書き換えを中止した（原本は退避済み）"
                    .into()
            }
            (Self::HandoffMigrationMemoKept, En) => {
                "the profile memo was left as it was because some projects could not be moved \
                 (the original is archived)"
                    .into()
            }
            (
                Self::ModelMismatch {
                    declared,
                    actual,
                    account,
                },
                Ja,
            ) => {
                let whose = account
                    .as_deref()
                    .map(|name| format!("アカウント {name} の "))
                    .unwrap_or_default();
                format!(
                    "モデルの食い違い: 宣言 {declared} → 実際 {actual}（{whose}entitlement かモデルの廃止で \
                     claude が別のモデルへ解決した。ledger は実際のモデルで集計する）"
                )
            }
            (
                Self::ModelMismatch {
                    declared,
                    actual,
                    account,
                },
                En,
            ) => {
                let whose = account
                    .as_deref()
                    .map(|name| format!("the entitlement of the account {name}"))
                    .unwrap_or_else(|| "the account's entitlement".to_string());
                format!(
                    "model mismatch: declared {declared} → actual {actual} (claude resolved to a \
                     different model because of {whose} or a model retirement; the ledger counts \
                     the actual model)"
                )
            }
            (
                Self::WorkerModelMismatch {
                    worker_id,
                    pane,
                    declared,
                    actual,
                    account,
                },
                Ja,
            ) => format!(
                "worker {worker_id}（pane {pane}）: {}",
                Self::ModelMismatch {
                    declared: declared.clone(),
                    actual: actual.clone(),
                    account: account.clone(),
                }
                .text_in(lang)
            ),
            (
                Self::WorkerModelMismatch {
                    worker_id,
                    pane,
                    declared,
                    actual,
                    account,
                },
                En,
            ) => format!(
                "worker {worker_id} (pane {pane}): {}",
                Self::ModelMismatch {
                    declared: declared.clone(),
                    actual: actual.clone(),
                    account: account.clone(),
                }
                .text_in(lang)
            ),
            (
                Self::HandoffMigrated {
                    profile,
                    moved,
                    kept_lines,
                    archived_to,
                },
                Ja,
            ) => format!(
                "handoff/{profile}.md をプロジェクト単位へ自動移行しました（#915）: {} / \
                 共通置き場に {kept_lines} 行を残し、移行前のファイルは {} へ退避",
                if moved.is_empty() {
                    "移動なし".to_string()
                } else {
                    moved.join(", ")
                },
                archived_to.as_deref().unwrap_or("(退避なし)")
            ),
            (
                Self::HandoffMigrated {
                    profile,
                    moved,
                    kept_lines,
                    archived_to,
                },
                En,
            ) => format!(
                "migrated handoff/{profile}.md to per-project files (#915): {} / kept \
                 {kept_lines} lines in the shared memo and archived the original to {}",
                if moved.is_empty() {
                    "nothing moved".to_string()
                } else {
                    moved.join(", ")
                },
                archived_to.as_deref().unwrap_or("(not archived)")
            ),
            // 英語は旧 `tako_core::handoff::profile_memo_warning_in` の英語の腕とバイト一致
            (Self::ProfileMemoOversized { profile, lines }, Ja) => format!(
                "プロファイル運用メモ handoff/{profile}.md が {lines} 行あります\
                 （目安 {PROFILE_MEMO_SOFT_LIMIT_LINES} 行）。\
                 プロジェクト固有の内容は handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md へ\
                 移してください（そこは管轄する master だけへ渡ります）"
            ),
            (Self::ProfileMemoOversized { profile, lines }, En) => format!(
                "The profile operating memo handoff/{profile}.md is {lines} lines \
                 (target {PROFILE_MEMO_SOFT_LIMIT_LINES}). Move project-specific content into \
                 handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md, which only reaches the master \
                 that owns that project"
            ),
            (Self::ResendCommand { pane }, Ja) => {
                format!("tako send --pane {pane} '<同じ依頼文>'")
            }
            (Self::ResendCommand { pane }, En) => {
                format!("tako send --pane {pane} '<same prompt>'")
            }
        }
    }
}

impl std::fmt::Display for ReplyNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl From<ReplyNote> for String {
    fn from(n: ReplyNote) -> Self {
        n.to_string()
    }
}
