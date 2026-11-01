//! `tako check-health`（CLI の表示・dispatch の `issues`）と環境診断（[`crate::diagnostics`] =
//! `tako setup --check` と同じ行）の文（#2365）。
//!
//! 以前はどれも日本語の固定文だったので、英語表示でも check-health と `--check` の出力がほぼ全部
//! 日本語で返った。`--check` は「setup は日本語」の枠に入っていたが、診断は check-health と字面まで
//! 同じ 1 実装（#1505）なので、どちらも表示言語に従わせる（setup 系の断り `SetupRefusal` と揃える）。
//!
//! 形は [`crate::Reason`] / [`crate::ReplyNote`] と同じ: (バリアント, 言語) の網羅 match なので、
//! バリアントを足すと日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文と
//! バイト一致。値（パス・名前・秒数・件数）は表記のまま埋める。
//!
//! 他モジュールが組む文（`setup_bootstrap` の段と tako CLI の PATH・`shell_integration`・`setup_deps`・
//! `setup_remaining`・tako-core の理由）は、それぞれの `_in(lang)` / 言語つきの口が日英を持つ。
//! 中身が外から来る文（remote daemon が書いた自己検査の理由・OS / tmux のエラー・起動時に記録した
//! 窓の置き先の理由）はそのまま載せる。

use tako_core::i18n::Lang;

/// check-health と環境診断の文（#2365）
#[derive(Debug, Clone, PartialEq)]
pub enum HealthText {
    // ---- 共通 ----
    /// `理由不明`（理由の文が無いときの埋め草）
    UnknownReason,
    /// `不明`（値が無いときの埋め草）
    Unknown,
    /// ログインシェルの設定が入力を待っていないか（字下げは呼び手）
    LoginShellWaitHint,

    // ---- 診断: エージェント CLI ----
    /// `認証: 不明（N 秒で応答なし）`
    AuthUnknown {
        secs: u64,
    },
    /// `導入: 不明（…）` / `PATH: 不明（…）`（`install` = 導入の段）
    StepUnknown {
        install: bool,
        secs: u64,
    },
    /// 使える系統を確かめられない理由の 1 系統分（`text` は [`Self::AuthUnknown`] / [`Self::StepUnknown`]）
    AgentUnknown {
        agent: String,
        text: String,
    },
    /// 使える系統を確かめられない（`list` は [`Self::AgentUnknown`] を ` / ` で連ねたもの）
    AgentCliUnconfirmed(String),
    /// 使える系統が無い
    AgentCliNone,
    /// 使える系統が無いときの次の一手（字下げ込み）
    AgentCliNoneHint,
    /// 使える系統（`list` は ` / ` で連ねた系統名）
    AgentCliReady(String),
    /// `<agent>: 完了`
    AgentDone(String),
    /// 導入状況を読めない系統（`error` は読めなかった理由）
    AgentStatusUnreadable {
        agent: String,
        error: String,
    },
    /// 実行ファイルの検出を打ち切った系統（`names` は ` / ` で連ねた系統名）
    AgentsUnconfirmed {
        names: String,
        secs: u64,
    },
    /// `  エージェント CLI:`（見出し）
    AgentsHeading,
    /// どの系統も見つからない
    AgentsNoneFound,
    /// `検出: <names>`
    AgentsDetected(String),
    /// `認証済み`
    SignedIn,
    /// `未認証`
    SignedOut,
    /// `プラン不明`
    PlanUnknown,
    /// 検出した系統の 1 行（字下げとラベル込み）
    AgentDetectedLine {
        agent: String,
        path: String,
        auth: String,
    },
    /// 実行ファイルの検出を打ち切った系統の 1 行（字下げとラベル込み）
    AgentUnconfirmedLine {
        agent: String,
        secs: u64,
    },
    /// codex の役割
    RoleCodex,
    /// agy の役割
    RoleAgy,

    // ---- 診断: 依存 ----
    /// `<bin>: 見つかりません（必須 | 任意）`
    DepNotFound {
        bin: String,
        required: bool,
    },
    /// `      用途: <purpose>`（字下げ込み）
    DepPurpose(String),
    /// 任意依存が無いときの注記（字下げ込み）
    DepOptionalNote,

    // ---- 診断: フルディスクアクセス（macOS） ----
    /// 付与済み
    FdaGranted,
    /// 未付与（推奨）
    FdaNotGranted,
    /// 未付与のときの説明と付与の手順（字下げ込みの複数行。`\n` で区切る）
    FdaNotGrantedDetail,

    // ---- 診断: MCP 登録 ----
    /// `<name> MCP: tako が登録済み`
    McpRegistered(String),
    /// `<name> MCP: 登録済みだがパスが消失しています`
    McpPathGone(String),
    /// `         登録パス: <command>`（字下げ込み）
    McpRegisteredPath(String),
    /// 修復の案内（字下げ込み）
    McpRepairHint,
    /// Claude の登録状態を確認できない（`label` は打ち切った問い合わせ）
    McpClaudeUnconfirmed {
        label: String,
        secs: u64,
    },
    /// `<name> MCP: tako が未登録（…）`
    McpNotRegistered(String),
    /// `<name> MCP: 登録済みだが … env の転送設定がありません`
    McpEnvMissing(String),
    /// env の転送が無いとツールが 0 個（字下げ込み）
    McpNoTools,
    /// `tako setup-mcp --agent <name>` で入れ直す（字下げ込み）
    McpReinstall(String),
    /// codex / agy の登録状態を確認できない
    McpUnknown(String),

    // ---- 診断: セットアップと追従 ----
    /// config.yaml の置き場を特定できない
    ConfigNoHome,
    /// config.yaml が無い
    ConfigNotCreated,
    /// config.yaml を読めない（`error` は読めなかった理由）
    ConfigUnreadable(String),
    /// セットアップ: 未完了
    SetupIncomplete,
    /// セットアップ: 完了済み（`at` は日時。無ければ「日時不明」）
    SetupDone(Option<String>),
    /// アップデート追従: 最新
    UpdateLatest(u32),
    /// アップデート追従: 未適用の変更が N 件
    UpdatePending(usize),
    /// アップデート追従: 確認できない（`error` は理由）
    UpdateUnknown(String),
    /// アップデート追従: 未適用（setup 前。`current` は今のリビジョン）
    UpdateUnapplied(u32),
    /// 既定エージェント
    DefaultAgent(String),
    /// 申告・検出プラン（`plans` は `provider=plan` を `, ` で連ねたもの）
    Plans(String),

    // ---- 診断: 指示ファイル ----
    /// `<path>: 存在します`
    InstructionsExist(String),
    /// `<path>: 未作成`
    InstructionsMissing(String),

    // ---- 診断: 共通ルール同期 ----
    SyncNotConfigured,
    SyncUpToDate,
    SyncOutdated,
    /// 正本が見つからない（`path` は正本の置き場）
    SyncSourceMissing(String),
    /// 知らない状態（`status` は状態の語）
    SyncOther(String),
    /// 確認に失敗（`error` は理由）
    SyncFailed(String),

    // ---- 診断: スリープ防止 ----
    SleepOff,
    SleepOn {
        mode: String,
        power: String,
    },
    LidOff,
    LidSudoersMissing,
    LidOn,

    // ---- 診断: 設定共有（`--check` と `tako setup` の要約が同じ判定から組む行。ラベルと字下げ込み） ----
    /// 配線済み
    ShareLinked(String),
    /// 配線済みのときの使い方（`tako setup` の要約だけ）
    ShareLinkedUsage,
    /// 配線先が git リポジトリではない
    ShareBroken(String),
    /// 繋ぎ直すコマンド
    ShareRelink(String),
    /// 未配線（既に自力で共有している）
    ShareUnlinked,
    /// 既に別の git で管理されている置き場（`same` = tako の置き場と一致）
    ShareManaged {
        path: String,
        repo: String,
        same: bool,
    },
    /// 相乗りするコマンド
    ShareAdopt(String),
    /// 別のリポジトリを作ると二重管理になる
    ShareDuplicate,
    /// 未配線（初めて）。`next` は配線するコマンド
    ShareFresh(String),
    /// 初めての配線の説明 1（`tako setup` の要約だけ）
    ShareFreshWhat,
    /// 初めての配線の説明 2（`tako setup` の要約だけ）
    ShareFreshSafe,

    // ---- 診断: プロファイル ----
    /// `プロファイル: N 個（…）`（`names` は `, ` で連ねた名前）
    ProfilesCount {
        count: usize,
        names: String,
    },
    ProfilesNone,
    /// 確認に失敗（`error` は理由）
    ProfilesFailed(String),

    // ---- 診断: リモート公開 ----
    /// 前回は公開していたが止まっている
    RemoteStoppedUnexpectedly,
    /// 公開し直すコマンド（字下げ込み）
    RemoteStartHint,
    /// 停止中
    RemoteStopped,
    /// 稼働中だが tailnet から届かない
    RemoteUnreachable,
    /// 詳細を見るコマンド（字下げ込み）
    RemoteStatusHint,
    /// 稼働中（pid）
    RemoteRunning(u32),

    // ---- 診断: IPC の受け口 ----
    IpcUp,
    IpcDown,
    /// `         理由: <error>`（字下げ込み。`None` は理由不明）
    IpcDownReason(Option<String>),
    IpcNoDataDir,
    IpcTooDeep,
    /// ソケットパスが上限を超えている（字下げ込み）
    IpcPathTooLong {
        bytes: usize,
        limit: usize,
    },
    /// もっと浅い置き場で起動し直す（字下げ込み）
    IpcShallowerHint,
    IpcAppNotRunning,

    // ---- check-health の `issues` ----
    /// tako CLI が PATH に在るかを確かめられない
    CliPathUnconfirmed(u64),
    /// CLI のバージョンを確かめられない（`label` は打ち切った問い合わせ）
    VersionUnconfirmed {
        label: String,
        secs: u64,
    },
    /// CLI とアプリのバージョンが違う
    VersionMismatch {
        cli: Option<String>,
        app: String,
    },
    /// tmux が無い
    TmuxMissing,
    /// セッション永続化が無効
    PersistOff,
    /// IPC の受け口が立っていない（データディレクトリが深い）
    IpcNotBoundTooLong {
        bytes: usize,
        limit: usize,
    },
    /// IPC の受け口が立っていない（`error` は理由）
    IpcNotBound(Option<String>),
    /// 受け口がパスから切り離されたまま張り直せない（`reason` は見張りの判定の文）
    IpcDetachedStuck {
        reason: String,
        error: String,
        secs: u64,
    },
    /// 受け口がパスから切り離されている（別の tako の疑い）
    IpcDetached {
        reason: String,
        secs: u64,
    },
    /// 名前付きパイプの待ち受けを作れていない
    PipeDown {
        error: Option<String>,
        streak: u64,
        max_secs: u64,
    },
    /// 起動時に fd の上限を引き上げられなかった
    FdRaiseFailed {
        error: Option<String>,
        soft: Option<u64>,
    },
    /// fd の使用が上限に近い（`not_raised` は起動時に引き上げなかった結果の語）
    FdNearExhaustion {
        open: u64,
        soft: u64,
        percent: u64,
        remaining: u64,
        per_pane: u64,
        not_raised: Option<String>,
    },
    /// 器の tmux サーバーの fd の使用が上限に近い
    FdContainerNear {
        pid: u32,
        open: u64,
        soft: u64,
        percent: u64,
        remaining: u64,
        per_pane: u64,
        limit: ContainerLimitNote,
    },
    /// 器の tmux サーバーに問い合わせが届かない（`last` は前に読めた pid / soft / 使用数）
    FdContainerUnreachable {
        reason: String,
        last: Option<(u32, Option<u64>, Option<u64>)>,
        per_pane: u64,
    },
    /// ペインの文字コードを UTF-8 に固定できなかった（`reasons` は対象ごとの理由を ` / ` で連ねたもの）
    EncodingPinGaveUp {
        pane: u64,
        reasons: String,
    },
    /// claude のペインに mod の報告が無い（`lines` はペインごとの理由を ` / ` で連ねたもの）
    ModNoReport {
        count: usize,
        lines: String,
    },

    // ---- CLI の表示 ----
    /// tako アプリへ届かない（tako mod の節の理由。`reason` は届かなかった理由）
    AppUnreachable(String),
    /// `接続: 届かない（…）`
    ConnectionUnreachable(String),
    /// GUI へ届かなかったときの CLI の断り
    AppUnreachableRefusal,
    /// `健全性: 問題なし | 指摘あり`
    Healthy(bool),
    /// `バージョン: app=… cli=…`
    Versions {
        app: String,
        cli: Option<String>,
    },
    /// `fd: open / soft 本（percent%）`（値は `?` のことがある）
    FdUsage {
        open: String,
        soft: String,
        percent: String,
    },
    /// 起動時に引き上げた
    FdRaised {
        launch: String,
        soft: String,
    },
    /// 起動時から足りていた
    FdAlreadyEnough(String),
    /// hard の上限で引き上げられない
    FdHardCapped(String),
    /// 引き上げに失敗（`None` は理由不明）
    FdFailed(Option<String>),
    /// 引き上げない（A/B）
    FdLegacy,
    /// 器の tmux サーバーが起きていない
    ContainerNotRunning,
    /// 器の tmux サーバーへ問い合わせが届かない（`None` は理由不明）
    ContainerUnreachable(Option<String>),
    /// 器の tmux サーバーの使用数
    ContainerUsage {
        open: String,
        soft: String,
        percent: String,
    },
    /// 器が引き上げた値を受け継いだ
    ContainerRaised(String),
    /// 器が引き上げより前に起きた
    ContainerPredates(String),
    /// 器へ引き上げを渡さない（A/B）
    ContainerLegacy,
    /// tako が引き上げていないので器も起動時のまま
    ContainerLaunchValue,
    /// `  受け口: <endpoint>`
    IpcEndpoint(String),
    /// `  理由: <error>`
    IpcErrorLine(String),
    /// 見張りの回数
    WatchCounts {
        secs: u64,
        checks: u64,
        rebinds: u64,
    },
    /// 最後に張り直した記録
    WatchLastRebind {
        secs_ago: u64,
        reason: String,
    },
    /// 張り直せない理由
    WatchRebindError(String),
    /// 名前付きパイプの待ち受けを作れていない（CLI の 1 行）
    PipeDownLine {
        streak: u64,
        error: Option<String>,
        retry_ms: u64,
        max_secs: u64,
    },
    /// 名前付きパイプの待ち受けは正常
    PipeOk(u64),
    /// 名前付きパイプの待ち受けは正常・途切れたことがある
    PipeOkWithOutages {
        outages: u64,
        failures: u64,
    },
    /// 最後に戻った記録（[`Self::PipeOkWithOutages`] に続ける）
    PipeLastRecovery {
        secs_ago: u64,
        failures: u64,
        error: String,
    },
    /// 固定パスに生きた受け口が居たので一時パスで立てた
    FixedPathHeld(String),
    /// 固定パスの残骸を消して張り直した
    FixedPathReclaimed(String),
    /// GUI は動いているのに受け口が応答しない（`why` は [`Self::SocketRefused`] ほか）
    GuiAliveUnreachable {
        pid: u64,
        why: String,
    },
    /// ソケットファイルはあるが誰も受けていない
    SocketRefused,
    /// ソケットファイルが無い
    SocketMissing,
    /// 繋がらない
    NotConnected,
    /// GUI は動いていない
    GuiNotRunning(u64),
    /// GUI は動いていて受け口にも繋がる
    GuiReachable(u64),
    /// `  次の一手: …`
    NextStep(String),
    /// tako を起動する（次の一手）
    StartTako,
    /// 受け口が応答しないときの次の一手（`windows` = パイプの作り直し・`secs` は間隔）
    AliveUnreachableNext {
        windows: bool,
        secs: u64,
    },
    /// `tako setup --check` の見出し
    SetupCheckTitle,
    /// 1 行の中の補足の区切り（`fd: …・起動時 …`）
    NoteSeparator,
}

/// 器の tmux サーバーの上限の決まり方（[`HealthText::FdContainerNear`] の後ろに足す 1 句）
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerLimitNote {
    /// 引き上げた値を受け継いだ（何も足さない）
    Raised,
    /// 引き上げより前に起きた（`capacity` は詰まるペインのおおよその数）
    Predates { soft: u64, capacity: u64 },
    /// A/B で引き上げた値を渡していない
    Legacy,
    /// tako 自身が引き上げていない（`outcome` は起動時の結果の語。`None` は記録なし）
    LaunchValue(Option<String>),
}

impl HealthText {
    /// 表示言語を明示しての文言（#2365）。
    ///
    /// (バリアント, 言語) の網羅 match なので、バリアントを足すと日英の両方を書くまで
    /// コンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        let or_unknown = |v: &Option<String>, lang: Lang| {
            v.clone()
                .unwrap_or_else(|| Self::UnknownReason.text_in(lang))
        };
        match (self, lang) {
            (Self::UnknownReason, Ja) => "理由不明".into(),
            (Self::UnknownReason, En) => "unknown reason".into(),
            (Self::Unknown, Ja) => "不明".into(),
            (Self::Unknown, En) => "unknown".into(),
            (Self::LoginShellWaitHint, Ja) => {
                ".zprofile などのログインシェルの設定が入力を待っていないか確かめてください".into()
            }
            (Self::LoginShellWaitHint, En) => {
                "Check that login shell settings such as .zprofile are not waiting for input".into()
            }

            // ---- エージェント CLI ----
            (Self::AuthUnknown { secs }, Ja) => format!("認証: 不明（{secs} 秒で応答なし）"),
            (Self::AuthUnknown { secs }, En) => {
                format!("auth: unknown (no response in {secs} s)")
            }
            (Self::StepUnknown { install, secs }, Ja) => format!(
                "{}: 不明（ログインシェルが {secs} 秒で応答なし）",
                if *install { "導入" } else { "PATH" }
            ),
            (Self::StepUnknown { install, secs }, En) => format!(
                "{}: unknown (the login shell did not respond in {secs} s)",
                if *install { "install" } else { "PATH" }
            ),
            (Self::AgentUnknown { agent, text }, Ja) => format!("{agent} の{text}"),
            (Self::AgentUnknown { agent, text }, En) => format!("{agent} {text}"),
            (Self::AgentCliUnconfirmed(list), Ja) => {
                format!("エージェント CLI の導入: 使える系統を確かめられません（{list}）")
            }
            (Self::AgentCliUnconfirmed(list), En) => {
                format!("agent CLI setup: cannot confirm a usable agent ({list})")
            }
            (Self::AgentCliNone, Ja) => "エージェント CLI の導入: 使える系統がありません".into(),
            (Self::AgentCliNone, En) => "agent CLI setup: no usable agent".into(),
            (Self::AgentCliNoneHint, Ja) => {
                "         tako setup を実行すると、ここから最後まで案内します".into()
            }
            (Self::AgentCliNoneHint, En) => {
                "         Run tako setup to be guided from here to the end".into()
            }
            (Self::AgentCliReady(list), Ja) => {
                format!("エージェント CLI の導入: 使える系統 {list}（導入 / PATH / ログイン）")
            }
            (Self::AgentCliReady(list), En) => {
                format!("agent CLI setup: usable {list} (install / PATH / sign-in)")
            }
            (Self::AgentDone(agent), Ja) => format!("{agent}: 完了"),
            (Self::AgentDone(agent), En) => format!("{agent}: done"),
            (Self::AgentStatusUnreadable { agent, error }, Ja) => {
                format!("{agent}: 確認できません（{error}）")
            }
            (Self::AgentStatusUnreadable { agent, error }, En) => {
                format!("{agent}: cannot check ({error})")
            }
            (Self::AgentsUnconfirmed { names, secs }, Ja) => {
                format!("{names} を確かめられません（ログインシェルが {secs} 秒で応答なし）")
            }
            (Self::AgentsUnconfirmed { names, secs }, En) => format!(
                "cannot confirm {names} (the login shell did not respond in {secs} s)"
            ),
            (Self::AgentsHeading, Ja) => "  エージェント CLI:".into(),
            (Self::AgentsHeading, En) => "  Agent CLIs:".into(),
            (Self::AgentsNoneFound, Ja) => "claude / codex / agy のいずれも見つかりません".into(),
            (Self::AgentsNoneFound, En) => "none of claude / codex / agy was found".into(),
            (Self::AgentsDetected(names), Ja) => format!("検出: {names}"),
            (Self::AgentsDetected(names), En) => format!("detected: {names}"),
            (Self::SignedIn, Ja) => "認証済み".into(),
            (Self::SignedIn, En) => "signed in".into(),
            (Self::SignedOut, Ja) => "未認証".into(),
            (Self::SignedOut, En) => "not signed in".into(),
            (Self::PlanUnknown, Ja) => "プラン不明".into(),
            (Self::PlanUnknown, En) => "plan unknown".into(),
            (Self::AgentDetectedLine { agent, path, auth }, Ja) => {
                format!("    [検出] {agent}: {path}（{auth}）")
            }
            (Self::AgentDetectedLine { agent, path, auth }, En) => {
                format!("    [detected] {agent}: {path} ({auth})")
            }
            (Self::AgentUnconfirmedLine { agent, secs }, Ja) => format!(
                "    [警告] {agent}: 確かめられません（ログインシェルが {secs} 秒で応答なし）"
            ),
            (Self::AgentUnconfirmedLine { agent, secs }, En) => format!(
                "    [warning] {agent}: cannot confirm (the login shell did not respond in {secs} s)"
            ),
            (Self::RoleCodex, Ja) => "Codex: master 起動時にも一時注入".into(),
            (Self::RoleCodex, En) => "Codex: also injected temporarily when master starts".into(),
            (Self::RoleAgy, Ja) => "agy: worker 専用（master は非対応）".into(),
            (Self::RoleAgy, En) => "agy: worker only (cannot be master)".into(),

            // ---- 依存 ----
            (Self::DepNotFound { bin, required }, Ja) => format!(
                "{bin}: 見つかりません（{}）",
                if *required { "必須" } else { "任意" }
            ),
            (Self::DepNotFound { bin, required }, En) => format!(
                "{bin}: not found ({})",
                if *required { "required" } else { "optional" }
            ),
            (Self::DepPurpose(purpose), Ja) => format!("      用途: {purpose}"),
            (Self::DepPurpose(purpose), En) => format!("      used for: {purpose}"),
            (Self::DepOptionalNote, Ja) => {
                "      無くても tako 自体は動きますが、上記の機能が使えません".into()
            }
            (Self::DepOptionalNote, En) => {
                "      tako itself works without it, but the features above are unavailable".into()
            }

            // ---- フルディスクアクセス ----
            (Self::FdaGranted, Ja) => {
                "フルディスクアクセス: 付与済み（許可ダイアログは表示されません）".into()
            }
            (Self::FdaGranted, En) => {
                "Full Disk Access: granted (no permission dialogs appear)".into()
            }
            (Self::FdaNotGranted, Ja) => "フルディスクアクセス: 未付与（推奨）".into(),
            (Self::FdaNotGranted, En) => "Full Disk Access: not granted (recommended)".into(),
            (Self::FdaNotGrantedDetail, Ja) => [
                "      macOS が「tako.app から、ほかのアプリからのデータへのアクセス権を",
                "      求められています」と頻繁に表示する原因です。フルディスクアクセスを",
                "      付与すると、このダイアログが出なくなります。",
                "      設定方法: システム設定 → プライバシーとセキュリティ → フルディスクアクセス → tako を追加",
                "      付与方法: tako fda open でシステム設定を開き、tako を追加してください",
            ]
            .join("\n"),
            (Self::FdaNotGrantedDetail, En) => [
                "      This is why macOS often shows \"tako.app would like to access data from",
                "      other apps\". Granting Full Disk Access stops this dialog.",
                "      Where: System Settings → Privacy & Security → Full Disk Access → add tako",
                "      How: run tako fda open to open System Settings, then add tako",
            ]
            .join("\n"),

            // ---- MCP 登録 ----
            (Self::McpRegistered(name), Ja) => format!("{name} MCP: tako が登録済み"),
            (Self::McpRegistered(name), En) => format!("{name} MCP: tako is registered"),
            (Self::McpPathGone(name), Ja) => {
                format!("{name} MCP: 登録済みだがパスが消失しています")
            }
            (Self::McpPathGone(name), En) => {
                format!("{name} MCP: registered, but the path is gone")
            }
            (Self::McpRegisteredPath(command), Ja) => format!("         登録パス: {command}"),
            (Self::McpRegisteredPath(command), En) => {
                format!("         registered path: {command}")
            }
            (Self::McpRepairHint, Ja) => {
                "         tako setup または tako setup-mcp で修復できます".into()
            }
            (Self::McpRepairHint, En) => "         tako setup or tako setup-mcp repairs it".into(),
            (Self::McpClaudeUnconfirmed { label, secs }, Ja) => format!(
                "Claude MCP: 登録状態を確認できません（{label} が {secs} 秒で応答なし）"
            ),
            (Self::McpClaudeUnconfirmed { label, secs }, En) => format!(
                "Claude MCP: cannot check the registration ({label} did not respond in {secs} s)"
            ),
            (Self::McpNotRegistered(name), Ja) => {
                format!("{name} MCP: tako が未登録（tako setup-mcp で登録できます）")
            }
            (Self::McpNotRegistered(name), En) => {
                format!("{name} MCP: tako is not registered (tako setup-mcp registers it)")
            }
            (Self::McpEnvMissing(name), Ja) => format!(
                "{name} MCP: 登録済みだが tako と通信する env の転送設定がありません"
            ),
            (Self::McpEnvMissing(name), En) => format!(
                "{name} MCP: registered, but the env forwarding to talk to tako is missing"
            ),
            (Self::McpNoTools, Ja) => "         そのままだとツールが 0 個になります".into(),
            (Self::McpNoTools, En) => "         As it is, no tools are available".into(),
            (Self::McpReinstall(name), Ja) => {
                format!("         tako setup-mcp --agent {name} で入れ直せます")
            }
            (Self::McpReinstall(name), En) => {
                format!("         tako setup-mcp --agent {name} reinstalls it")
            }
            (Self::McpUnknown(name), Ja) => format!(
                "{name} MCP: 登録状態を確認できません（{name} mcp list を手で確認してください）"
            ),
            (Self::McpUnknown(name), En) => format!(
                "{name} MCP: cannot check the registration (check {name} mcp list by hand)"
            ),

            // ---- セットアップと追従 ----
            (Self::ConfigNoHome, Ja) => "config.yaml: 置き場を特定できません（HOME が未設定）".into(),
            (Self::ConfigNoHome, En) => "config.yaml: cannot locate it (HOME is not set)".into(),
            (Self::ConfigNotCreated, Ja) => "config.yaml: 未作成".into(),
            (Self::ConfigNotCreated, En) => "config.yaml: not created".into(),
            (Self::ConfigUnreadable(error), Ja) => format!("config.yaml: 読めません（{error}）"),
            (Self::ConfigUnreadable(error), En) => format!("config.yaml: cannot read ({error})"),
            (Self::SetupIncomplete, Ja) => "セットアップ: 未完了".into(),
            (Self::SetupIncomplete, En) => "setup: not completed".into(),
            (Self::SetupDone(at), Ja) => format!(
                "セットアップ: 完了済み ({})",
                at.as_deref().unwrap_or("日時不明")
            ),
            (Self::SetupDone(at), En) => format!(
                "setup: completed ({})",
                at.as_deref().unwrap_or("time unknown")
            ),
            (Self::UpdateLatest(rev), Ja) => format!("アップデート追従: 最新（rev {rev}）"),
            (Self::UpdateLatest(rev), En) => format!("update follow-up: up to date (rev {rev})"),
            (Self::UpdatePending(n), Ja) => format!(
                "アップデート追従: 未適用の setup 変更が {n} 件（tako setup --changes で詳細）"
            ),
            (Self::UpdatePending(n), En) => format!(
                "update follow-up: {n} setup change(s) not applied (details: tako setup --changes)"
            ),
            (Self::UpdateUnknown(error), Ja) => format!("アップデート追従: 確認できません（{error}）"),
            (Self::UpdateUnknown(error), En) => format!("update follow-up: cannot check ({error})"),
            (Self::UpdateUnapplied(current), Ja) => {
                format!("アップデート追従: 未適用（現在の setup リビジョンは {current}）")
            }
            (Self::UpdateUnapplied(current), En) => {
                format!("update follow-up: not applied (the current setup revision is {current})")
            }
            (Self::DefaultAgent(agent), Ja) => format!("既定エージェント: {agent}"),
            (Self::DefaultAgent(agent), En) => format!("default agent: {agent}"),
            (Self::Plans(plans), Ja) => format!("申告・検出プラン: {plans}"),
            (Self::Plans(plans), En) => format!("declared / detected plans: {plans}"),

            // ---- 指示ファイル ----
            (Self::InstructionsExist(path), Ja) => format!("{path}: 存在します"),
            (Self::InstructionsExist(path), En) => format!("{path}: exists"),
            (Self::InstructionsMissing(path), Ja) => format!("{path}: 未作成"),
            (Self::InstructionsMissing(path), En) => format!("{path}: not created"),

            // ---- 共通ルール同期 ----
            (Self::SyncNotConfigured, Ja) => "エージェント共通ルール同期: 未設定".into(),
            (Self::SyncNotConfigured, En) => "shared agent rules sync: not set up".into(),
            (Self::SyncUpToDate, Ja) => "エージェント共通ルール同期: 最新".into(),
            (Self::SyncUpToDate, En) => "shared agent rules sync: up to date".into(),
            (Self::SyncOutdated, Ja) => {
                "エージェント共通ルール同期: ずれあり（tako agents sync-rules で同期）".into()
            }
            (Self::SyncOutdated, En) => {
                "shared agent rules sync: out of sync (sync with tako agents sync-rules)".into()
            }
            (Self::SyncSourceMissing(path), Ja) => {
                format!("エージェント共通ルール同期: 正本が見つからない ({path})")
            }
            (Self::SyncSourceMissing(path), En) => {
                format!("shared agent rules sync: the source is not found ({path})")
            }
            (Self::SyncOther(status), Ja) => format!("エージェント共通ルール同期: {status}"),
            (Self::SyncOther(status), En) => format!("shared agent rules sync: {status}"),
            (Self::SyncFailed(error), Ja) => {
                format!("エージェント共通ルール同期: 確認失敗 ({error})")
            }
            (Self::SyncFailed(error), En) => {
                format!("shared agent rules sync: check failed ({error})")
            }

            // ---- スリープ防止 ----
            (Self::SleepOff, Ja) => {
                "スリープ防止: 無効（tako sleep-guard set --mode while-agents-running で有効化）"
                    .into()
            }
            (Self::SleepOff, En) => {
                "sleep prevention: off (turn it on with tako sleep-guard set --mode \
                 while-agents-running)"
                    .into()
            }
            (Self::SleepOn { mode, power }, Ja) => {
                format!("スリープ防止: mode={mode}, power={power}")
            }
            (Self::SleepOn { mode, power }, En) => {
                format!("sleep prevention: mode={mode}, power={power}")
            }
            (Self::LidOff, Ja) => {
                "蓋閉じ防止: 未設定（tako sleep-guard install-lid-sleep で有効化）".into()
            }
            (Self::LidOff, En) => {
                "lid-close prevention: not set up (turn it on with tako sleep-guard \
                 install-lid-sleep)"
                    .into()
            }
            (Self::LidSudoersMissing, Ja) => {
                "蓋閉じ防止: while-agents-running だが sudoers 未登録（tako sleep-guard install-lid-sleep で登録）"
                    .into()
            }
            (Self::LidSudoersMissing, En) => {
                "lid-close prevention: while-agents-running, but sudoers is not registered \
                 (register it with tako sleep-guard install-lid-sleep)"
                    .into()
            }
            (Self::LidOn, Ja) => "蓋閉じ防止: while-agents-running".into(),
            (Self::LidOn, En) => "lid-close prevention: while-agents-running".into(),

            // ---- 設定共有 ----
            (Self::ShareLinked(repo), Ja) => format!("  [OK] 設定共有: 配線済み（{repo}）"),
            (Self::ShareLinked(repo), En) => format!("  [OK] config sharing: linked ({repo})"),
            (Self::ShareLinkedUsage, Ja) => {
                "       差分は `tako config status`、同期は `tako config push` / `pull`".into()
            }
            (Self::ShareLinkedUsage, En) => {
                "       diff: `tako config status`, sync: `tako config push` / `pull`".into()
            }
            (Self::ShareBroken(repo), Ja) => {
                format!("  [警告] 設定共有: 配線先が git リポジトリではありません（{repo}）")
            }
            (Self::ShareBroken(repo), En) => format!(
                "  [warning] config sharing: the linked place is not a git repository ({repo})"
            ),
            (Self::ShareRelink(next), Ja) => format!("         `{next}` で繋ぎ直せます"),
            (Self::ShareRelink(next), En) => format!("         `{next}` relinks it"),
            (Self::ShareUnlinked, Ja) => "  [情報] 設定共有: 未配線".into(),
            (Self::ShareUnlinked, En) => "  [info] config sharing: not linked".into(),
            (Self::ShareManaged { path, repo, same }, Ja) => format!(
                "         {path} は既に {repo} で管理されています{}",
                if *same {
                    "（tako の置き場と一致）"
                } else {
                    "（tako の置き場とは別）"
                }
            ),
            (Self::ShareManaged { path, repo, same }, En) => format!(
                "         {path} is already managed by {repo}{}",
                if *same {
                    " (same place as tako's)"
                } else {
                    " (different from tako's place)"
                }
            ),
            (Self::ShareAdopt(next), Ja) => {
                format!("         同じリポジトリへ相乗りするなら `{next}`")
            }
            (Self::ShareAdopt(next), En) => {
                format!("         To share the same repository: `{next}`")
            }
            (Self::ShareDuplicate, Ja) => {
                "         別のリポジトリを作ると同じ内容が 2 箇所に並びます（二重管理）".into()
            }
            (Self::ShareDuplicate, En) => {
                "         A separate repository would keep the same content in 2 places \
                 (double management)"
                    .into()
            }
            (Self::ShareFresh(next), Ja) => format!(
                "  [情報] 設定共有: 未配線（複数デバイスで同じ AI 設定を使うなら `{next}`）"
            ),
            (Self::ShareFresh(next), En) => format!(
                "  [info] config sharing: not linked (to use the same AI settings on several \
                 devices: `{next}`)"
            ),
            (Self::ShareFreshWhat, Ja) => {
                "         claude のグローバル指示と tako の宣言的設定を git 1 本で共有します".into()
            }
            (Self::ShareFreshWhat, En) => {
                "         Shares claude's global instructions and tako's declarative settings \
                 in one git repository"
                    .into()
            }
            (Self::ShareFreshSafe, Ja) => {
                "         秘匿情報とこのマシン固有の状態は共有対象から構造的に外れます".into()
            }
            (Self::ShareFreshSafe, En) => {
                "         Secrets and this machine's own state are kept out by design".into()
            }

            // ---- プロファイル ----
            (Self::ProfilesCount { count, names }, Ja) => {
                format!("プロファイル: {count} 個（{names}）")
            }
            (Self::ProfilesCount { count, names }, En) => format!("profiles: {count} ({names})"),
            (Self::ProfilesNone, Ja) => {
                "プロファイル: 未作成（tako master で自動生成されます）".into()
            }
            (Self::ProfilesNone, En) => {
                "profiles: none yet (tako master creates one automatically)".into()
            }
            (Self::ProfilesFailed(error), Ja) => format!("プロファイル: 確認失敗 ({error})"),
            (Self::ProfilesFailed(error), En) => format!("profiles: check failed ({error})"),

            // ---- リモート公開 ----
            (Self::RemoteStoppedUnexpectedly, Ja) => {
                "リモート公開: 前回は公開していましたが、いまは止まっています".into()
            }
            (Self::RemoteStoppedUnexpectedly, En) => {
                "remote access: it was published last time, but it is stopped now".into()
            }
            (Self::RemoteStartHint, Ja) => "         tako remote start で公開し直せます".into(),
            (Self::RemoteStartHint, En) => {
                "         tako remote start publishes it again".into()
            }
            (Self::RemoteStopped, Ja) => {
                "リモート公開: 停止中（スマホから使うなら tako remote start）".into()
            }
            (Self::RemoteStopped, En) => {
                "remote access: stopped (to use it from a phone: tako remote start)".into()
            }
            (Self::RemoteUnreachable, Ja) => {
                "リモート公開: 稼働中ですが tailnet からは届いていません".into()
            }
            (Self::RemoteUnreachable, En) => {
                "remote access: running, but not reachable from the tailnet".into()
            }
            (Self::RemoteStatusHint, Ja) => {
                "         tako remote status で詳細を確認できます".into()
            }
            (Self::RemoteStatusHint, En) => "         See the details with tako remote status".into(),
            (Self::RemoteRunning(pid), Ja) => format!("リモート公開: 稼働中（pid {pid}）"),
            (Self::RemoteRunning(pid), En) => format!("remote access: running (pid {pid})"),

            // ---- IPC の受け口（診断） ----
            (Self::IpcUp, Ja) => "IPC の受け口: 立っています（tako CLI / MCP が届きます）".into(),
            (Self::IpcUp, En) => "IPC endpoint: up (tako CLI / MCP can reach it)".into(),
            (Self::IpcDown, Ja) => {
                "IPC の受け口: 立っていません（tako CLI / MCP は届きません）".into()
            }
            (Self::IpcDown, En) => "IPC endpoint: down (tako CLI / MCP cannot reach it)".into(),
            (Self::IpcDownReason(error), Ja) => {
                format!("         理由: {}", or_unknown(error, Ja))
            }
            (Self::IpcDownReason(error), En) => {
                format!("         reason: {}", or_unknown(error, En))
            }
            (Self::IpcNoDataDir, Ja) => "IPC の受け口: データディレクトリを特定できません".into(),
            (Self::IpcNoDataDir, En) => "IPC endpoint: cannot locate the data directory".into(),
            (Self::IpcTooDeep, Ja) => {
                "IPC の受け口: データディレクトリが深すぎます（tako CLI / MCP は届きません）".into()
            }
            (Self::IpcTooDeep, En) => {
                "IPC endpoint: the data directory is too deep (tako CLI / MCP cannot reach it)"
                    .into()
            }
            (Self::IpcPathTooLong { bytes, limit }, Ja) => {
                format!("         ソケットパスが {bytes} バイトで上限 {limit} バイトを超えています")
            }
            (Self::IpcPathTooLong { bytes, limit }, En) => {
                format!("         the socket path is {bytes} bytes, over the {limit}-byte limit")
            }
            (Self::IpcShallowerHint, Ja) => {
                "         もっと浅い TAKO_DATA_DIR で起動し直してください".into()
            }
            (Self::IpcShallowerHint, En) => {
                "         Restart with a shallower TAKO_DATA_DIR".into()
            }
            (Self::IpcAppNotRunning, Ja) => {
                "IPC の受け口: 届きません（tako アプリが起動していません）".into()
            }
            (Self::IpcAppNotRunning, En) => {
                "IPC endpoint: unreachable (the tako app is not running)".into()
            }

            // ---- check-health の `issues` ----
            (Self::CliPathUnconfirmed(secs), Ja) => format!(
                "tako CLI が PATH に在るかを確かめられない（ログインシェルが {secs} 秒で応答しない）。\
                 .zprofile などのログインシェルの設定が入力を待っていないか確かめること（#1533）"
            ),
            (Self::CliPathUnconfirmed(secs), En) => format!(
                "cannot confirm whether the tako CLI is on PATH (the login shell did not respond \
                 in {secs} s). Check that login shell settings such as .zprofile are not waiting \
                 for input (#1533)"
            ),
            (Self::VersionUnconfirmed { label, secs }, Ja) => {
                format!("CLI バージョンを確かめられない（`{label}` が {secs} 秒で応答しない）")
            }
            (Self::VersionUnconfirmed { label, secs }, En) => format!(
                "cannot confirm the CLI version (`{label}` did not respond in {secs} s)"
            ),
            (Self::VersionMismatch { cli, app }, Ja) => format!(
                "CLI バージョン ({}) とアプリバージョン ({app}) が不一致。\
                 build-app.sh --install で最新の CLI をインストールすること",
                cli.as_deref().unwrap_or("不明"),
            ),
            (Self::VersionMismatch { cli, app }, En) => format!(
                "the CLI version ({}) does not match the app version ({app}). \
                 Install the latest CLI with build-app.sh --install",
                cli.as_deref().unwrap_or("unknown"),
            ),
            (Self::TmuxMissing, Ja) => "tmux がインストールされていない。タブ構成の保存・復元は機能するが、\
                実行中プロセス・画面内容の復元（完全復元）は使えない。\
                brew install tmux でインストール可能"
                .into(),
            (Self::TmuxMissing, En) => "tmux is not installed. Saving and restoring the tab layout \
                works, but restoring running processes and screen contents (full restore) does \
                not. Install it with brew install tmux"
                .into(),
            (Self::PersistOff, Ja) => "セッション永続化が無効。tako persist on で有効にすると、\
                tako 再起動時にプロセスと画面内容が復元される"
                .into(),
            (Self::PersistOff, En) => "session persistence is off. Turn it on with tako persist on \
                to restore processes and screen contents when tako restarts"
                .into(),
            (Self::IpcNotBoundTooLong { bytes, limit }, Ja) => format!(
                "IPC の受け口が立っていない（tako CLI / MCP は届かない）: データディレクトリが深く、\
                 ソケットパスが {bytes} バイトで上限 {limit} バイトを超えている。\
                 もっと浅い TAKO_DATA_DIR で起動し直すこと"
            ),
            (Self::IpcNotBoundTooLong { bytes, limit }, En) => format!(
                "the IPC endpoint is not up (tako CLI / MCP cannot reach it): the data directory \
                 is deep, and the socket path is {bytes} bytes, over the {limit}-byte limit. \
                 Restart with a shallower TAKO_DATA_DIR"
            ),
            (Self::IpcNotBound(error), Ja) => format!(
                "IPC の受け口が立っていない（tako CLI / MCP は届かない）: {}",
                or_unknown(error, Ja)
            ),
            (Self::IpcNotBound(error), En) => format!(
                "the IPC endpoint is not up (tako CLI / MCP cannot reach it): {}",
                or_unknown(error, En)
            ),
            (
                Self::IpcDetachedStuck {
                    reason,
                    error,
                    secs,
                },
                Ja,
            ) => format!(
                "IPC の受け口がソケットのパスから切り離されている（tako CLI / MCP の stdio は届かない）: \
                 {reason}・張り直せない（{error}）。{secs} 秒ごとにやり直す。戻らなければ tako を再起動する（#2002）"
            ),
            (
                Self::IpcDetachedStuck {
                    reason,
                    error,
                    secs,
                },
                En,
            ) => format!(
                "the IPC endpoint is detached from the socket path (the stdio of tako CLI / MCP \
                 cannot reach it): {reason}; cannot reattach ({error}). tako retries every {secs} \
                 s. If it does not come back, restart tako (#2002)"
            ),
            (Self::IpcDetached { reason, secs }, Ja) => format!(
                "IPC の受け口がソケットのパスから切り離されている（tako CLI / MCP の stdio は届かない）: \
                 {reason}。同じデータディレクトリで別の tako が動いていないか確かめる。\
                 そのファイルが消えれば {secs} 秒以内に張り直す（#2002）"
            ),
            (Self::IpcDetached { reason, secs }, En) => format!(
                "the IPC endpoint is detached from the socket path (the stdio of tako CLI / MCP \
                 cannot reach it): {reason}. Check that another tako is not running with the same \
                 data directory. Once that file is gone, tako reattaches within {secs} s (#2002)"
            ),
            (
                Self::PipeDown {
                    error,
                    streak,
                    max_secs,
                },
                Ja,
            ) => format!(
                "IPC の名前付きパイプの待ち受けを作れていない（tako CLI / MCP の stdio は届かない）: {}・\
                 {streak} 回続けて失敗。間を置いて（最長 {max_secs} 秒ごとに）作り直している。戻らなければ tako を再起動する（#2361）",
                or_unknown(error, Ja)
            ),
            (
                Self::PipeDown {
                    error,
                    streak,
                    max_secs,
                },
                En,
            ) => format!(
                "cannot create the IPC named pipe listener (the stdio of tako CLI / MCP cannot \
                 reach it): {}; failed {streak} times in a row. tako retries at intervals (up to \
                 every {max_secs} s). If it does not come back, restart tako (#2361)",
                or_unknown(error, En)
            ),
            (Self::FdRaiseFailed { error, soft }, Ja) => format!(
                "起動時にファイル記述子（fd）の上限を引き上げられなかった（{}）。soft {} のまま動いているので、\
                 ペインが増えると tako CLI / MCP が届かなくなる。tako を再起動しても直らなければ \
                 launchctl limit maxfiles で上限を確かめる（#2359）",
                or_unknown(error, Ja),
                soft.map_or_else(|| "?".to_string(), |s| s.to_string()),
            ),
            (Self::FdRaiseFailed { error, soft }, En) => format!(
                "could not raise the file descriptor (fd) limit at startup ({}). tako runs with \
                 soft {}, so tako CLI / MCP stop reaching it as panes increase. If restarting tako \
                 does not fix it, check the limit with launchctl limit maxfiles (#2359)",
                or_unknown(error, En),
                soft.map_or_else(|| "?".to_string(), |s| s.to_string()),
            ),
            (
                Self::FdNearExhaustion {
                    open,
                    soft,
                    percent,
                    remaining,
                    per_pane,
                    not_raised,
                },
                Ja,
            ) => {
                let mut message = format!(
                    "ファイル記述子（fd）の使用が上限に近い: {open} / {soft} 本（{percent}%・残り {remaining} 本）。上限に達すると \
                     tako CLI / MCP が届かなくなり（繋がっても GUI がすぐ閉じる）、新しいペインも開けない。\
                     使っていないペイン・タブを閉じること（1 ペインで約 {per_pane} 本）"
                );
                if let Some(outcome) = not_raised {
                    message.push_str(&format!("。起動時に上限を引き上げていない（{outcome}）"));
                }
                message.push_str(
                    "。ペインの数に比べて多すぎるなら漏れの疑いがあるので persist.log を添えて報告する（#2359）",
                );
                message
            }
            (
                Self::FdNearExhaustion {
                    open,
                    soft,
                    percent,
                    remaining,
                    per_pane,
                    not_raised,
                },
                En,
            ) => {
                let mut message = format!(
                    "file descriptor (fd) use is close to the limit: {open} / {soft} ({percent}%, \
                     {remaining} left). At the limit, tako CLI / MCP stop reaching it (the GUI \
                     closes them right after connecting) and new panes cannot open. Close panes \
                     and tabs you are not using (about {per_pane} per pane)"
                );
                if let Some(outcome) = not_raised {
                    message.push_str(&format!(". tako did not raise the limit at startup ({outcome})"));
                }
                message.push_str(
                    ". If this is too many for the number of panes, it may be a leak; report it \
                     with persist.log (#2359)",
                );
                message
            }
            (
                Self::FdContainerNear {
                    pid,
                    open,
                    soft,
                    percent,
                    remaining,
                    per_pane,
                    limit,
                },
                Ja,
            ) => {
                let mut message = format!(
                    "器の tmux サーバー（pid {pid}）のファイル記述子（fd）の使用が上限に近い: {open} / {soft} 本\
                     （{percent}%・残り {remaining} 本）。上限に達すると器に新しいペインを作れず（create window failed: \
                     fork failed: Too many open files）、器への問い合わせ（画面の読み取り・生死の確認）も\
                     すべて返らなくなる。使っていないペインを閉じること（器の 1 ペインで約 \
                     {per_pane} 本）"
                );
                match limit {
                    ContainerLimitNote::Raised => {}
                    ContainerLimitNote::Predates { soft, capacity } => message.push_str(&format!(
                        "。このサーバーは fd の上限の引き上げより前に起きたので起動時の {soft} のまま\
                         （約 {capacity} ペインで詰まる）。上がるのは次にサーバーが起きたとき（器のセッションが\
                         すべて終わった後。tako を再起動しても器は残るので上がらない）"
                    )),
                    ContainerLimitNote::Legacy => message.push_str(&format!(
                        "。{} のため引き上げた値を渡していない",
                        tako_core::platform::fd_limit::CONTAINER_LEGACY_ENV
                    )),
                    ContainerLimitNote::LaunchValue(outcome) => message.push_str(&format!(
                        "。tako 自身が起動時に上限を引き上げていない（{}）ので器も起動時の値のまま",
                        outcome.as_deref().unwrap_or("記録なし"),
                    )),
                }
                message.push_str("（#2367）");
                message
            }
            (
                Self::FdContainerNear {
                    pid,
                    open,
                    soft,
                    percent,
                    remaining,
                    per_pane,
                    limit,
                },
                En,
            ) => {
                let mut message = format!(
                    "file descriptor (fd) use of the container tmux server (pid {pid}) is close to \
                     the limit: {open} / {soft} ({percent}%, {remaining} left). At the limit, the \
                     container cannot create new panes (create window failed: fork failed: Too \
                     many open files), and every query to it (reading the screen, checking if it \
                     is alive) stops returning. Close panes you are not using (about {per_pane} \
                     per container pane)"
                );
                match limit {
                    ContainerLimitNote::Raised => {}
                    ContainerLimitNote::Predates { soft, capacity } => message.push_str(&format!(
                        ". This server started before tako raised the fd limit, so it keeps the \
                         startup value {soft} (it fills up at about {capacity} panes). It goes up \
                         the next time the server starts (after all container sessions end; \
                         restarting tako keeps the container, so it does not go up)"
                    )),
                    ContainerLimitNote::Legacy => message.push_str(&format!(
                        ". The raised value is not passed because of {}",
                        tako_core::platform::fd_limit::CONTAINER_LEGACY_ENV
                    )),
                    ContainerLimitNote::LaunchValue(outcome) => message.push_str(&format!(
                        ". tako itself did not raise the limit at startup ({}), so the container \
                         keeps the startup value too",
                        outcome.as_deref().unwrap_or("no record"),
                    )),
                }
                message.push_str(" (#2367)");
                message
            }
            (
                Self::FdContainerUnreachable {
                    reason,
                    last,
                    per_pane,
                },
                Ja,
            ) => {
                let mut message = format!(
                    "器の tmux サーバーに問い合わせが届かない（{reason}）。fd の上限に届いたサーバーは\
                     生きたまま、すべての問い合わせをこう返す（tmux 3.6b）"
                );
                if let Some((pid, soft, open)) = last {
                    let num = |v: &Option<u64>| v.map_or_else(|| "?".to_string(), |n| n.to_string());
                    message.push_str(&format!(
                        "。前に読めたサーバー（pid {pid}）は soft {}・いまの使用数は {} 本",
                        num(soft),
                        num(open),
                    ));
                }
                message.push_str(&format!(
                    "。上限に届いているなら、使っていないペインを閉じると戻る（器の 1 ペインで約 \
                     {per_pane} 本）（#2367）"
                ));
                message
            }
            (
                Self::FdContainerUnreachable {
                    reason,
                    last,
                    per_pane,
                },
                En,
            ) => {
                let mut message = format!(
                    "queries do not reach the container tmux server ({reason}). A server that hit \
                     the fd limit stays alive and answers every query like this (tmux 3.6b)"
                );
                if let Some((pid, soft, open)) = last {
                    let num = |v: &Option<u64>| v.map_or_else(|| "?".to_string(), |n| n.to_string());
                    message.push_str(&format!(
                        ". The server read before (pid {pid}) had soft {}; it uses {} now",
                        num(soft),
                        num(open),
                    ));
                }
                message.push_str(&format!(
                    ". If it hit the limit, closing panes you are not using brings it back (about \
                     {per_pane} per container pane) (#2367)"
                ));
                message
            }
            (Self::EncodingPinGaveUp { pane, reasons }, Ja) => format!(
                "ペイン {pane} の文字コードを UTF-8 に固定できなかった（{reasons}）。\
                 このペインで UTF-8 のバイト列を書く子（type・CRT の printf 等）の出力は化ける。\
                 そのペインで chcp 65001 を実行すると直る（#2092）"
            ),
            (Self::EncodingPinGaveUp { pane, reasons }, En) => format!(
                "could not pin the code page of pane {pane} to UTF-8 ({reasons}). Output of \
                 children that write UTF-8 bytes in this pane (type, CRT printf, and so on) is \
                 garbled. Running chcp 65001 in that pane fixes it (#2092)"
            ),
            (Self::ModNoReport { count, lines }, Ja) => format!(
                "claude が動いているのに mod の報告が無いペインが {count} 個（{lines}）。詳しくは tako mod"
            ),
            (Self::ModNoReport { count, lines }, En) => format!(
                "{count} pane(s) run claude but have no mod report ({lines}). See tako mod for details"
            ),

            // ---- CLI の表示 ----
            (Self::AppUnreachable(reason), Ja) => format!("tako アプリへ届かない（{reason}）"),
            (Self::AppUnreachable(reason), En) => format!("cannot reach the tako app ({reason})"),
            (Self::ConnectionUnreachable(reason), Ja) => format!("接続: 届かない（{reason}）"),
            (Self::ConnectionUnreachable(reason), En) => format!("connection: unreachable ({reason})"),
            (Self::AppUnreachableRefusal, Ja) => {
                "tako アプリへ届かない（上の ipc 節が受け口の実測）".into()
            }
            (Self::AppUnreachableRefusal, En) => {
                "cannot reach the tako app (the ipc section above is the measured endpoint)".into()
            }
            (Self::Healthy(true), Ja) => "健全性: 問題なし".into(),
            (Self::Healthy(false), Ja) => "健全性: 指摘あり".into(),
            (Self::Healthy(true), En) => "health: no issues".into(),
            (Self::Healthy(false), En) => "health: issues found".into(),
            (Self::Versions { app, cli }, Ja) => format!(
                "バージョン: app={app} cli={}",
                cli.as_deref().unwrap_or("不明")
            ),
            (Self::Versions { app, cli }, En) => format!(
                "version: app={app} cli={}",
                cli.as_deref().unwrap_or("unknown")
            ),
            (
                Self::FdUsage {
                    open,
                    soft,
                    percent,
                },
                Ja,
            ) => format!("fd: {open} / {soft} 本（{percent}%）"),
            (
                Self::FdUsage {
                    open,
                    soft,
                    percent,
                },
                En,
            ) => format!("fd: {open} / {soft} ({percent}%)"),
            (Self::FdRaised { launch, soft }, Ja) => {
                format!("起動時 {launch} → {soft} に引き上げ（ペインのシェルには {launch}）")
            }
            (Self::FdRaised { launch, soft }, En) => {
                format!("raised from {launch} to {soft} at startup (pane shells get {launch})")
            }
            (Self::FdAlreadyEnough(launch), Ja) => format!("起動時から {launch}（引き上げは不要）"),
            (Self::FdAlreadyEnough(launch), En) => {
                format!("{launch} from startup (no need to raise)")
            }
            (Self::FdHardCapped(hard), Ja) => {
                format!("hard が {hard} で引き上げられない（起動時のまま）")
            }
            (Self::FdHardCapped(hard), En) => {
                format!("cannot raise because hard is {hard} (kept the startup value)")
            }
            (Self::FdFailed(error), Ja) => format!("引き上げに失敗（{}）", or_unknown(error, Ja)),
            (Self::FdFailed(error), En) => format!("failed to raise ({})", or_unknown(error, En)),
            (Self::FdLegacy, Ja) => format!(
                "引き上げない（{}）",
                tako_core::platform::fd_limit::LEGACY_ENV
            ),
            (Self::FdLegacy, En) => format!(
                "not raised ({})",
                tako_core::platform::fd_limit::LEGACY_ENV
            ),
            (Self::ContainerNotRunning, Ja) => "器の tmux サーバー: 起きていない".into(),
            (Self::ContainerNotRunning, En) => "container tmux server: not running".into(),
            (Self::ContainerUnreachable(reason), Ja) => format!(
                "器の tmux サーバー: 問い合わせが届かない（{}）",
                or_unknown(reason, Ja)
            ),
            (Self::ContainerUnreachable(reason), En) => format!(
                "container tmux server: queries do not reach it ({})",
                or_unknown(reason, En)
            ),
            (
                Self::ContainerUsage {
                    open,
                    soft,
                    percent,
                },
                Ja,
            ) => format!("器の tmux サーバー: {open} / {soft} 本（{percent}%）"),
            (
                Self::ContainerUsage {
                    open,
                    soft,
                    percent,
                },
                En,
            ) => format!("container tmux server: {open} / {soft} ({percent}%)"),
            (Self::ContainerRaised(children), Ja) => {
                format!("引き上げた値を受け継いだ（器のペインのシェルには {children}）")
            }
            (Self::ContainerRaised(children), En) => {
                format!("inherited the raised value (container pane shells get {children})")
            }
            (Self::ContainerPredates(capacity), Ja) => format!(
                "引き上げより前に起きたので起動時のまま（約 {capacity} ペインで詰まる。次にサーバーが起きたとき上がる）"
            ),
            (Self::ContainerPredates(capacity), En) => format!(
                "started before the raise, so it keeps the startup value (fills up at about \
                 {capacity} panes; it goes up the next time the server starts)"
            ),
            (Self::ContainerLegacy, Ja) => format!(
                "引き上げない（{}）",
                tako_core::platform::fd_limit::CONTAINER_LEGACY_ENV
            ),
            (Self::ContainerLegacy, En) => format!(
                "not raised ({})",
                tako_core::platform::fd_limit::CONTAINER_LEGACY_ENV
            ),
            (Self::ContainerLaunchValue, Ja) => {
                "tako が引き上げていないので起動時のまま".into()
            }
            (Self::ContainerLaunchValue, En) => {
                "keeps the startup value because tako did not raise it".into()
            }
            (Self::IpcEndpoint(endpoint), Ja) => format!("  受け口: {endpoint}"),
            (Self::IpcEndpoint(endpoint), En) => format!("  endpoint: {endpoint}"),
            (Self::IpcErrorLine(error), Ja) => format!("  理由: {error}"),
            (Self::IpcErrorLine(error), En) => format!("  reason: {error}"),
            (
                Self::WatchCounts {
                    secs,
                    checks,
                    rebinds,
                },
                Ja,
            ) => format!("  見張り: {secs} 秒ごと・確認 {checks} 回・張り直し {rebinds} 回"),
            (
                Self::WatchCounts {
                    secs,
                    checks,
                    rebinds,
                },
                En,
            ) => format!(
                "  watch: every {secs} s, checked {checks} times, reattached {rebinds} times"
            ),
            (Self::WatchLastRebind { secs_ago, reason }, Ja) => {
                format!("（最後: {secs_ago} 秒前・{reason}）")
            }
            (Self::WatchLastRebind { secs_ago, reason }, En) => {
                format!(" (last: {secs_ago} s ago, {reason})")
            }
            (Self::WatchRebindError(error), Ja) => format!("  張り直せない理由: {error}"),
            (Self::WatchRebindError(error), En) => format!("  why it cannot reattach: {error}"),
            (
                Self::PipeDownLine {
                    streak,
                    error,
                    retry_ms,
                    max_secs,
                },
                Ja,
            ) => format!(
                "  パイプの待ち受け: 作れていない（{streak} 回続けて失敗・{}）。{retry_ms} ms 後に作り直す（最長 {max_secs} 秒ごと）。\
                 この間は tako CLI / MCP の stdio が届かない（#2361）",
                or_unknown(error, Ja)
            ),
            (
                Self::PipeDownLine {
                    streak,
                    error,
                    retry_ms,
                    max_secs,
                },
                En,
            ) => format!(
                "  pipe listener: not created (failed {streak} times in a row, {}). tako retries in \
                 {retry_ms} ms (up to every {max_secs} s). Until then the stdio of tako CLI / MCP \
                 cannot reach it (#2361)",
                or_unknown(error, En)
            ),
            (Self::PipeOk(max_secs), Ja) => format!(
                "  パイプの待ち受け: 正常（作れなければ最長 {max_secs} 秒ごとに作り直す）"
            ),
            (Self::PipeOk(max_secs), En) => format!(
                "  pipe listener: OK (if it cannot be created, tako retries up to every {max_secs} s)"
            ),
            (Self::PipeOkWithOutages { outages, failures }, Ja) => format!(
                "  パイプの待ち受け: 正常・これまでに途切れ {outages} 回（作れなかった {failures} 回）"
            ),
            (Self::PipeOkWithOutages { outages, failures }, En) => format!(
                "  pipe listener: OK; interrupted {outages} times so far (failed to create {failures} times)"
            ),
            (
                Self::PipeLastRecovery {
                    secs_ago,
                    failures,
                    error,
                },
                Ja,
            ) => format!(
                "（最後: {secs_ago} 秒前に {failures} 回続けて作れなかった後に戻った・{error}）"
            ),
            (
                Self::PipeLastRecovery {
                    secs_ago,
                    failures,
                    error,
                },
                En,
            ) => format!(
                " (last: came back {secs_ago} s ago after failing {failures} times in a row, {error})"
            ),
            (Self::FixedPathHeld(reason), Ja) => format!(
                "  固定パス: 生きた受け口が居たので奪わずに一時パスで立てた（{reason}）。\
                 同じデータディレクトリの別の tako が動いている = このウィンドウはセカンダリ（#2360）"
            ),
            (Self::FixedPathHeld(reason), En) => format!(
                "  fixed path: a live endpoint was there, so tako started on a temporary path \
                 without taking it over ({reason}). Another tako with the same data directory is \
                 running = this window is secondary (#2360)"
            ),
            (Self::FixedPathReclaimed(reason), Ja) => {
                format!("  固定パス: 起動時に残骸を消して張り直した（{reason}）（#2360）")
            }
            (Self::FixedPathReclaimed(reason), En) => format!(
                "  fixed path: removed a leftover and reattached at startup ({reason}) (#2360)"
            ),
            (Self::GuiAliveUnreachable { pid, why }, Ja) => {
                format!("  GUI: 動いている（pid {pid}）のに受け口が応答しない（{why}）")
            }
            (Self::GuiAliveUnreachable { pid, why }, En) => {
                format!("  GUI: running (pid {pid}), but the endpoint does not answer ({why})")
            }
            (Self::SocketRefused, Ja) => "ソケットファイルはあるが誰も受けていない".into(),
            (Self::SocketRefused, En) => "the socket file exists, but nobody is listening".into(),
            (Self::SocketMissing, Ja) => "ソケットファイルが無い".into(),
            (Self::SocketMissing, En) => "there is no socket file".into(),
            (Self::NotConnected, Ja) => "繋がらない".into(),
            (Self::NotConnected, En) => "cannot connect".into(),
            (Self::GuiNotRunning(pid), Ja) => {
                format!("  GUI: 動いていない（接続情報の pid {pid} は居ない）")
            }
            (Self::GuiNotRunning(pid), En) => {
                format!("  GUI: not running (pid {pid} in the connection info is gone)")
            }
            (Self::GuiReachable(pid), Ja) => format!(
                "  GUI: 動いている（pid {pid}）・受け口にも繋がる（届かない理由は「接続:」の行）"
            ),
            (Self::GuiReachable(pid), En) => format!(
                "  GUI: running (pid {pid}) and the endpoint connects (see the \"connection:\" line \
                 for why it cannot be reached)"
            ),
            (Self::NextStep(next), Ja) => format!("  次の一手: {next}"),
            (Self::NextStep(next), En) => format!("  next step: {next}"),
            (Self::StartTako, Ja) => "tako を起動する".into(),
            (Self::StartTako, En) => "start tako".into(),
            (Self::AliveUnreachableNext { windows: true, secs }, Ja) => format!(
                "tako はパイプの待ち受けを作れなければ最長 {secs} 秒ごとに作り直す。少し待って tako check-health をもう一度。\
                 戻らなければ tako を再起動する（#2361）"
            ),
            (Self::AliveUnreachableNext { windows: true, secs }, En) => format!(
                "tako recreates the pipe listener up to every {secs} s if it cannot create it. Wait \
                 a little and run tako check-health again. If it does not come back, restart tako \
                 (#2361)"
            ),
            (
                Self::AliveUnreachableNext {
                    windows: false,
                    secs,
                },
                Ja,
            ) => format!(
                "tako は受け口を {secs} 秒ごとに確かめて張り直す。少し待って tako check-health をもう一度。\
                 戻らなければ tako を再起動する（#2002）"
            ),
            (
                Self::AliveUnreachableNext {
                    windows: false,
                    secs,
                },
                En,
            ) => format!(
                "tako checks the endpoint every {secs} s and reattaches it. Wait a little and run \
                 tako check-health again. If it does not come back, restart tako (#2002)"
            ),
            (Self::SetupCheckTitle, Ja) => "tako セットアップ 環境チェック".into(),
            (Self::SetupCheckTitle, En) => "tako setup environment check".into(),
            (Self::NoteSeparator, Ja) => "・".into(),
            (Self::NoteSeparator, En) => "; ".into(),
        }
    }
}

impl std::fmt::Display for HealthText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}
