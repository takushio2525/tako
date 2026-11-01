//! remote 系（会話の末尾・端末の登録・daemon の admin API・スクロールバック・リモートのセットアップ）の
//! 断り（#2365）。
//!
//! dispatch の `tako_remote_messages` / `tako_remote_devices` / `tako_remote_scrollback` /
//! `tako_remote_setup` と CLI の `tako remote messages / devices / scrollback / setup`（手元の処理）は、以前は
//! `transcript` / `remote` / `remote_auth` / `remote_setup` / `tailscale` の `Err(String)`（日本語の固定文）を
//! そのまま載せていたので、英語表示でも理由だけ日本語で返った。日英の対は [`RemoteRefusal::text_in`] の
//! (バリアント, 言語) の網羅 match の 1 か所に置く（形は [`crate::SessionRefusal`] と同じ）。
//! 日本語は #2365 より前の固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
//!
//! daemon を起こしている途中（#2608 の印が効いている間）の明示の start / stop の断り
//! （`DaemonStarting` / `DaemonStartingStop`。#2613）もここに置く。`remote::spawn_daemon` /
//! `daemon_stop` は `String` を返す従来の口のまま、この型の表示（呼び手のプロセスの表示言語）を載せる。
//! 隔離中（`TAKO_ISOLATED`）に本物の tailscale の serve を張らなかった断り（`IsolatedServe`。#2619）と、
//! 対話の setup が Tailscale を導入しなかった断り（`IsolatedInstall`。#2624）も同じ形。
//!
//! # 口
//!
//! dispatch と CLI だけが呼ぶ口（`remote::devices_*` / `admin_request` / `scrollback*` /
//! `remote_setup::run_noninteractive`）はこの型を返す。対話のウィザードや起動時の断りも使う口
//! （`transcript::read_messages` / `DeviceRegistry::open` / `revoke` / `set_role` /
//! `remote_setup::decide_variant` / `tailscale::save_variant`）は `String` を返す従来の形を残し、文は
//! **日本語のまま**（起動時の断り・日本語の対話）。dispatch と CLI は型の口（`*_typed`）を通す。
//!
//! daemon の HTTP API（PWA と管理 API）が返す本文の `error` も、この型を daemon の表示言語で文にして
//! 載せる（#2365 の続き。以前は handler 群が日本語の固定文を直に書いていた）。daemon の表示言語は
//! 起動した時点で設定から決まり（#1077）、`tako lang` を切り替えても起こし直すまで前の言語のまま。
//! PWA は `error` を表示に使うだけで、分岐はステータスコードと `kind` / `paste_guard` で行う
//! （日本語の字面に依らない）。daemon の中で型の口（`*_typed`）を呼び、`to_string()` で文にする。
//!
//! daemon の admin API の断り（端末が登録されていない・昇格は画面からだけ）は、本文の `error` に加えて
//! `kind` を返し、頼んだ側（[`crate::remote::admin_request`]）が `kind` から呼び手の表示言語で
//! 組み直す（daemon と呼び手の表示言語が違ってもそろう）。`kind` の無い本文（旧版の daemon・それ以外の
//! 断り）は `error` を手を加えずに載せる。
//!
//! OS / serde / tmux のエラー文は言語に依らずそのまま載せる。「見つからない」系は英語にも ` not found` を
//! 含め、[`crate::DispatchError::says_not_found`] の当たり方を日英でそろえる。

use tako_core::backend::BackendError;
use tako_core::i18n::Lang;
use tako_core::load_issue::JsonReason;

use crate::DispatchError;

/// daemon の admin API が「登録されていない端末」で断ったときの `kind`（本文の `device_id` と組で返す）
pub const KIND_DEVICE_NOT_REGISTERED: &str = "device_not_registered";

/// remote 系の断り。`error` は OS / serde のエラー文（言語に依らずそのまま載せる）、
/// ID・パス・名前は表記のまま埋める
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteRefusal {
    // ---- 会話の末尾（remote messages） ----
    /// 会話 ID の形が不正（パストラバーサル対策で弾いた）
    SessionIdInvalid,
    /// その会話の記録（transcript）が無い
    TranscriptNotFound(String),
    /// 記録を開けない
    TranscriptOpen(String),

    // ---- 端末の登録（devices.json） ----
    /// devices.json を読めない
    DevicesRead(String),
    /// devices.json を JSON として解釈できない（`path` はそのファイル）
    DevicesParse { path: String, reason: JsonReason },
    /// devices.json の中身を組めない（serde のエラー文）
    DevicesBuild(String),
    /// devices.json を書けない
    DevicesWrite(String),
    /// その ID の端末は登録されていない
    DeviceNotRegistered(String),
    /// role の値が不正
    RoleInvalid(String),
    /// 権限を上げる操作は tako の画面からだけ（CLI / MCP / スマホは降格と削除だけ。#1452）
    UpgradeRequiresGui,

    // ---- daemon の admin API ----
    /// リモートサーバー（daemon）が動いていない
    DaemonNotRunning,
    /// daemon の待ち受け先を記録から引けない
    DaemonEndpointUnknown,
    /// 管理トークンを読めない
    AdminTokenRead(String),
    /// daemon と通信できない（`endpoint` は待ち受け先の表記）
    DaemonUnreachable { endpoint: String, error: String },
    /// daemon の応答にヘッダの終わりが無い
    DaemonResponseNoHeader,
    /// daemon の応答にステータス行が無い
    DaemonResponseNoStatus,
    /// daemon の応答の本文を JSON として読めない（serde のエラー文）
    DaemonResponseParse(String),
    /// daemon が 2xx 以外で断った
    DaemonError { status: u16, message: DaemonMessage },

    // ---- スクロールバック ----
    /// 対象指定（ペイン ID / セッション名）が空
    ScrollbackTargetEmpty,
    /// 対象指定からセッション名を取り出せない（`:0.1` のような形）
    ScrollbackTargetNoSession(String),
    /// 器のセッションの履歴を読めない
    ScrollbackUnreadable {
        session: String,
        cause: ScrollbackCause,
    },
    /// ペインの器のセッションを解決できない（CLI の手元の処理 = 稼働中の tako-app へ問い合わせる経路）
    ScrollbackPaneUnresolved(u64),
    /// #972 の A/B（`TAKO_972_LEGACY=1` = 修正前の再現）の断り。修正前の文をそのまま載せる
    ScrollbackLegacy(String),

    // ---- リモートのセットアップ ----
    /// MCP / dispatch へ渡した answers の形が違う（serde のエラー文）
    SetupAnswersInvalid(String),
    /// CLI の `--answers` の JSON を読めない（serde のエラー文）
    SetupAnswersJsonInvalid(String),
    /// Tailscale の系統の指定が不正
    TailscaleVariantInvalid(String),
    /// 自動判定が選んだ系統の名前を解決できない
    TailscaleVariantUnresolved(String),
    /// 系統の選択を保存する置き場（state ディレクトリ）を解決できない
    VariantStateDirUnresolved,
    /// 系統の選択を保存する置き場を作れない
    VariantStateDirCreate(String),
    /// 系統の選択を書けない
    VariantSave(String),
    /// MagicDNS 名を取れない
    MagicDnsMissing,

    // ---- daemon を起こしている途中（#2608 の印が効いている。#2613） ----
    /// 起こしている途中なので、2 本目の daemon を起こさなかった（`tako remote start` / 起動ボタン / MCP）
    DaemonStarting {
        marker: crate::remote_starting::StartingMarker,
        now: u64,
    },
    /// 起こしている途中なので止めなかった（止めても進行中の起動が新版を起こし直す / 止める手掛かりが無い）
    DaemonStartingStop {
        marker: crate::remote_starting::StartingMarker,
        now: u64,
    },

    // ---- 隔離中の serve（#2619） ----
    /// 隔離中（`TAKO_ISOLATED`）なので本物の tailscale の serve を張らなかった
    /// （`tako remote start` / 起動ボタン / MCP / daemon の直起動 / UDS の setup。門は `remote_isolated_serve`）
    IsolatedServe,
    /// 隔離中なので、対話の `tako remote setup` が未導入の Tailscale を導入しなかった（#2624。
    /// 導入器はこの機械全体へ入れる。通すのは `TAKO_REMOTE_ISOLATED_SERVE=1` だけ）
    IsolatedInstall,

    // ---- daemon の HTTP API（PWA・管理 API が受け取る本文の `error`。#2365 の続き） ----
    /// 管理 API へ管理トークン無しで来た
    AdminTokenRequired,
    /// 本文に `device_id` が無い
    DeviceIdRequired,
    /// 管理 API に無い経路
    AdminEndpointNotFound,
    /// API に無い経路
    ApiEndpointNotFound,
    /// パスからペイン ID を取り出せない
    PaneIdInvalid,
    /// パスからセッション ID を取り出せない
    SessionIdMalformed,
    /// パスからタブ ID を取り出せない
    TabIdInvalid,
    /// SSH の対象が数値のペイン ID でない
    SshPaneIdNotNumeric,
    /// respond の対象が数値のペイン ID でない
    RespondPaneIdNotNumeric,
    /// respond の本文に `choice` が無い
    ChoiceRequired,
    /// resize の大きさの指定が無い（app 不在の tmux 直の経路）
    ResizeSizeRequired,
    /// 内部のロックが毒（他のスレッドが握ったまま panic した）
    Internal,
    /// tako app が動いていないので、app 経由の操作を断った
    AppNotRunning(AppVia),
    /// タブが一覧に無い（日本語は tako-core の `WorkspaceError::TabNotFound` と同じ文）
    TabNotFound(u64),
    /// ペインを解決できない（中身は渡されたペインの指定の表記）
    PaneNotFound(String),
    /// 鍵らしき文字が入っていたので送らなかった（#2050）
    InputSecretSuspected,
    /// 本文を読めない
    BodyUnreadable,
    /// 本文を JSON として読めない（serde のエラー文）
    BodyJsonInvalid(String),
    /// Origin が daemon の URL と違う（中身はヘッダの値）
    OriginNotAllowed(String),
    /// serve 経由を名乗る接続の接続元を確かめられない（#841）
    PeerUnverified(crate::platform::local_endpoint::PeerReject),
    /// serve 経由でない（`X-Forwarded-For` の無い）アクセス
    ServeOnly,
    /// `X-Forwarded-For` が空
    ForwardedForEmpty,
    /// `X-Forwarded-For` はあるが `X-Forwarded-Host` が無い
    ForwardedHostMissing,
    /// `X-Forwarded-Host` が自ノードの名前と違う（中身は受け取った値）
    ForwardedHostMismatch(String),
    /// 接続元が tailnet のノードではない（中身は接続元の IP）
    PeerNotInTailnet(String),
    /// 接続元の identity を確かめられない（中身は whois のエラー文）
    WhoisFailed(String),
    /// ペアリングされていない端末
    DeviceNotPaired,
    /// 端末の role が足りない
    RoleTooLow {
        required: crate::remote_auth::DeviceRole,
        current: crate::remote_auth::DeviceRole,
    },
    /// 承認 / 拒否の対象の保留中の要求が無い（中身は端末の ID）
    PairingRequestNotPending(String),
    /// WebSocket の要求に tako のサブプロトコルが無い
    WsProtocolRequired,
    /// WebSocket の要求に `pane` のクエリが無い
    WsPaneQueryRequired,
    /// WebSocket の要求に `Sec-WebSocket-Key` が無い
    WsKeyMissing,
    /// アップロードが上限を超えた（中身は上限の MB）
    UploadTooLarge(u64),
    /// アップロードが multipart/form-data でない
    UploadNotMultipart,
    /// multipart にファイルが無い
    UploadNoFile,
    /// multipart に `pane` が無い
    UploadPaneRequired,
    /// multipart のファイルの中身を取り出せない
    UploadDataUnreadable,
    /// ファイル名がパスを含む・空
    UploadFileNameInvalid,
    /// 対象ペインの作業ディレクトリを引けない
    UploadPaneCwdUnknown,
    /// 置き場（`.tako-remote-uploads/`）を作れない（OS のエラー文）
    UploadDirCreate(String),
    /// 置き場がシンボリックリンク
    UploadDirSymlink,
    /// 書き先が既存のシンボリックリンク
    UploadTargetSymlink,
    /// 置き場のパスを解決できない（OS のエラー文）
    UploadPathResolve(String),
    /// ファイルを書けない（OS のエラー文）
    UploadWrite(String),
    /// 書いた先が置き場の外だった
    UploadTraversal,
    /// tmux を起こせない（OS のエラー文）
    TmuxSpawn(String),
    /// tmux の出力を読むスレッドを起こせない（`stream` は `stdout` / `stderr`）
    TmuxDrainThread { stream: &'static str, error: String },
    /// tmux が時間内に終わらない（`command` は tmux のサブコマンド）
    TmuxTimeout { command: String, secs: u64 },
    /// tmux の終わりを待てない（OS のエラー文）
    TmuxWait(String),
    /// tmux capture-pane が失敗した（tmux 自身の出力は手を加えずに載る）
    TmuxCaptureFailed(String),
}

/// tako app 経由でしか行えない daemon の操作（[`RemoteRefusal::AppNotRunning`] の括弧の中身）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppVia {
    /// 括弧を付けない「tako app が稼働していない」
    Plain,
    /// スマホからの入力（`POST /api/panes/:id/input`）
    Input,
    /// 承認ダイアログへの応答（`POST /api/panes/:id/respond`）
    Respond,
    /// ペインを閉じる（`POST /api/panes/:id/close`）
    Close,
    /// タブを作る（`POST /api/tabs`）
    TabNew,
    /// master を起こす（`POST /api/tabs/:id/master`）
    MasterLaunch,
    /// 人がやることの操作（`/api/tasks…`）
    Tasks,
    /// コマンドカードの操作（`/api/cards…`）
    Cards,
}

/// daemon が断ったときの本文の中身
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonMessage {
    /// `kind` から組み直した断り（呼び手の表示言語で組む）
    Refusal(Box<RemoteRefusal>),
    /// `kind` の無い本文の `error`（daemon の文。手を加えずに載せる）
    Text(String),
    /// 本文に `error` が無い
    Missing,
}

/// スクロールバックを読めなかった理由
#[derive(Debug, Clone, PartialEq)]
pub enum ScrollbackCause {
    /// 器にアウトオブプロセスの画面採取の手段が無い（または器のセッション名として使えない）
    NoCapture,
    /// 器が断った（文は器のエラーの表。tmux 自身の出力は手を加えずに載る）
    Backend(BackendError),
}

impl RemoteRefusal {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::SessionIdInvalid, Ja) => {
                "session_id の形式が不正（英数とハイフンのみ）".to_string()
            }
            (Self::SessionIdInvalid, En) => {
                "invalid session_id (letters, digits and hyphens only)".to_string()
            }
            (Self::TranscriptNotFound(id), Ja) => {
                format!("session {id} の transcript が見つからない")
            }
            // ` not found` を含める（`DispatchError::says_not_found` の当たり方を日英でそろえる）
            (Self::TranscriptNotFound(id), En) => {
                format!("transcript of session {id} not found")
            }
            (Self::TranscriptOpen(error), Ja) => format!("transcript を開けない: {error}"),
            (Self::TranscriptOpen(error), En) => format!("cannot open the transcript: {error}"),
            (Self::DevicesRead(error), Ja) => format!("devices.json の読み取りに失敗: {error}"),
            (Self::DevicesRead(error), En) => format!("failed to read devices.json: {error}"),
            (Self::DevicesParse { path, reason }, Ja) => {
                format!(
                    "devices.json の解釈に失敗（{path}）: {}",
                    reason.text_in(lang)
                )
            }
            (Self::DevicesParse { path, reason }, En) => {
                format!(
                    "failed to parse devices.json ({path}): {}",
                    reason.text_in(lang)
                )
            }
            (Self::DevicesBuild(error), Ja) => format!("devices.json の構築に失敗: {error}"),
            (Self::DevicesBuild(error), En) => format!("failed to build devices.json: {error}"),
            (Self::DevicesWrite(error), Ja) => format!("devices.json の書き込みに失敗: {error}"),
            (Self::DevicesWrite(error), En) => format!("failed to write devices.json: {error}"),
            (Self::DeviceNotRegistered(id), Ja) => format!("登録されていないデバイス: {id}"),
            // 日本語が「見つからない」と言わないので、英語も ` not found` を含めない
            (Self::DeviceNotRegistered(id), En) => format!("device is not registered: {id}"),
            (Self::RoleInvalid(role), Ja) => {
                format!("不正な role: {role}（observe / interact / manage / admin）")
            }
            (Self::RoleInvalid(role), En) => {
                format!("invalid role: {role} (observe / interact / manage / admin)")
            }
            (Self::UpgradeRequiresGui, Ja) => {
                "権限を上げる操作は tako の画面（設定 → リモート、または承認\
                 ダイアログ）からのみ行えます。CLI / MCP / スマホからは\
                 降格と削除だけができます"
                    .to_string()
            }
            (Self::UpgradeRequiresGui, En) => {
                "raising a role is only possible from the tako window (Settings → Remote, or the \
                 approval dialog). The CLI / MCP / phone can only lower a role or remove a device"
                    .to_string()
            }
            (Self::DaemonNotRunning, Ja) => "リモートサーバーが起動していない".to_string(),
            (Self::DaemonNotRunning, En) => "the remote server is not running".to_string(),
            (Self::DaemonEndpointUnknown, Ja) => {
                "リモートサーバーの待ち受け先を特定できない（state ファイルが無い）".to_string()
            }
            (Self::DaemonEndpointUnknown, En) => {
                "cannot determine where the remote server listens (no state file)".to_string()
            }
            (Self::AdminTokenRead(error), Ja) => format!("管理トークンの読み取りに失敗: {error}"),
            (Self::AdminTokenRead(error), En) => {
                format!("failed to read the admin token: {error}")
            }
            (Self::DaemonUnreachable { endpoint, error }, Ja) => {
                format!("daemon との通信に失敗 ({endpoint}): {error}")
            }
            (Self::DaemonUnreachable { endpoint, error }, En) => {
                format!("failed to talk to the daemon ({endpoint}): {error}")
            }
            (Self::DaemonResponseNoHeader, Ja) => {
                "daemon の応答が不正（ヘッダ境界なし）".to_string()
            }
            (Self::DaemonResponseNoHeader, En) => {
                "invalid daemon response (no end of headers)".to_string()
            }
            (Self::DaemonResponseNoStatus, Ja) => {
                "daemon の応答が不正（ステータス行なし）".to_string()
            }
            (Self::DaemonResponseNoStatus, En) => {
                "invalid daemon response (no status line)".to_string()
            }
            (Self::DaemonResponseParse(error), Ja) => {
                format!("daemon の応答を解釈できない: {error}")
            }
            (Self::DaemonResponseParse(error), En) => {
                format!("cannot parse the daemon response: {error}")
            }
            (Self::DaemonError { status, message }, Ja) => {
                format!(
                    "daemon がエラーを返した ({status}): {}",
                    message.text_in(lang)
                )
            }
            (Self::DaemonError { status, message }, En) => {
                format!(
                    "the daemon returned an error ({status}): {}",
                    message.text_in(lang)
                )
            }
            (Self::ScrollbackTargetEmpty, Ja) => "ペイン ID / セッション名が空".to_string(),
            (Self::ScrollbackTargetEmpty, En) => "pane ID / session name is empty".to_string(),
            (Self::ScrollbackTargetNoSession(spec), Ja) => {
                format!("対象指定 '{spec}' からセッション名を取り出せない")
            }
            (Self::ScrollbackTargetNoSession(spec), En) => {
                format!("cannot take a session name from the target '{spec}'")
            }
            (Self::ScrollbackUnreadable { session, cause }, Ja) => {
                format!(
                    "セッション {session} の履歴を読めない: {}",
                    cause.text_in(lang)
                )
            }
            (Self::ScrollbackUnreadable { session, cause }, En) => {
                format!(
                    "cannot read the history of session {session}: {}",
                    cause.text_in(lang)
                )
            }
            (Self::ScrollbackPaneUnresolved(id), Ja) => format!(
                "ペイン {id} の器のセッションを解決できない\
                 （tako-app が起動していて、そのペインが器を持っているかを確認する。\
                 セッション名を直接渡すこともできる）"
            ),
            (Self::ScrollbackPaneUnresolved(id), En) => format!(
                "cannot resolve the persistence session of pane {id} (check that tako-app is \
                 running and that the pane has one; you can also pass the session name directly)"
            ),
            // 修正前の再現の文（A/B の腕）なので日英とも手を加えずに載せる
            (Self::ScrollbackLegacy(text), _) => text.clone(),
            (Self::SetupAnswersInvalid(error), Ja) => {
                format!("remote setup answers が不正: {error}")
            }
            (Self::SetupAnswersInvalid(error), En) => {
                format!("invalid remote setup answers: {error}")
            }
            (Self::SetupAnswersJsonInvalid(error), Ja) => format!("answers JSON が不正: {error}"),
            (Self::SetupAnswersJsonInvalid(error), En) => {
                format!("invalid answers JSON: {error}")
            }
            (Self::TailscaleVariantInvalid(key), Ja) => format!(
                "Tailscale 系統の指定が不正です: {key}（auto | gui | standalone）。\
                 standalone を選ぶには tailscaled の LocalAPI socket が必要です"
            ),
            (Self::TailscaleVariantInvalid(key), En) => format!(
                "invalid Tailscale variant: {key} (auto | gui | standalone). \
                 Choosing standalone needs the LocalAPI socket of tailscaled"
            ),
            (Self::TailscaleVariantUnresolved(key), Ja) => {
                format!("Tailscale 系統を解決できない: {key}")
            }
            (Self::TailscaleVariantUnresolved(key), En) => {
                format!("cannot resolve the Tailscale variant: {key}")
            }
            (Self::VariantStateDirUnresolved, Ja) => "state ディレクトリを解決できない".to_string(),
            (Self::VariantStateDirUnresolved, En) => {
                "cannot determine the state directory".to_string()
            }
            (Self::VariantStateDirCreate(error), Ja) => {
                format!("state ディレクトリの作成に失敗: {error}")
            }
            (Self::VariantStateDirCreate(error), En) => {
                format!("failed to create the state directory: {error}")
            }
            (Self::VariantSave(error), Ja) => format!("Tailscale 系統の保存に失敗: {error}"),
            (Self::VariantSave(error), En) => {
                format!("failed to save the Tailscale variant: {error}")
            }
            (Self::MagicDnsMissing, Ja) => "MagicDNS 名を取得できません".to_string(),
            (Self::MagicDnsMissing, En) => "cannot get the MagicDNS name".to_string(),
            // stop を案内しない（#2608 の status と同じ。止めると立ち上がりかけの新版を止めさせる）
            (Self::DaemonStarting { marker, now }, Ja) => format!(
                "{}。2 本目の daemon を起こさないよう、この起動は行いませんでした。\
                 終わるまで待ってから `tako remote status` を読み直してください",
                crate::remote_starting::doing_in(marker, *now, lang)
            ),
            (Self::DaemonStarting { marker, now }, En) => format!(
                "{}. This start was not carried out so as not to launch a second daemon. \
                 Wait for it to finish, then check `tako remote status` again",
                crate::remote_starting::doing_in(marker, *now, lang)
            ),
            (Self::DaemonStartingStop { marker, now }, Ja) => format!(
                "{}。いま止めても進行中の起動が daemon を起こし直すため、止めていません。\
                 終わるまで待ってから `tako remote stop` で止めてください",
                crate::remote_starting::doing_in(marker, *now, lang)
            ),
            (Self::DaemonStartingStop { marker, now }, En) => format!(
                "{}. It was not stopped, because the start in progress would launch the daemon again. \
                 Wait for it to finish, then stop it with `tako remote stop`",
                crate::remote_starting::doing_in(marker, *now, lang)
            ),
            (Self::IsolatedServe, Ja) => format!(
                "隔離中（TAKO_ISOLATED）なので、tailscale の serve を張らずに止めました。\
                 serve はこの機械の Tailscale のノード全体の設定で、隔離の外にある本番の公開や\
                 利用者の serve 設定を書き換えるためです。serve を張らずに試すなら TAKO_REMOTE_TEST_MODE=1、\
                 偽の tailscale で試すなら TAKO_TAILSCALE_BIN=<偽物のパス>、本物の serve まで確かめるなら \
                 {}=1 を付けてください",
                crate::remote_isolated_serve::OPT_IN_ENV
            ),
            (Self::IsolatedServe, En) => format!(
                "Stopped without setting up tailscale serve because this process is isolated (TAKO_ISOLATED). \
                 serve is a machine-wide setting of this Tailscale node, outside the sandbox, so it could \
                 overwrite the live remote access or your own serve config. To try without serve, set \
                 TAKO_REMOTE_TEST_MODE=1; to use a fake tailscale, set TAKO_TAILSCALE_BIN=<path to the fake>; \
                 to exercise the real serve, set {}=1",
                crate::remote_isolated_serve::OPT_IN_ENV
            ),
            (Self::IsolatedInstall, Ja) => format!(
                "隔離中（TAKO_ISOLATED）なので、Tailscale を導入せずに止めました。導入はこの機械全体への\
                 インストールで、隔離の外にあるためです（偽の TAKO_TAILSCALE_BIN では導入器は偽物になりません）。\
                 導入まで確かめるなら {}=1 を付けてください",
                crate::remote_isolated_serve::OPT_IN_ENV
            ),
            (Self::IsolatedInstall, En) => format!(
                "Stopped without installing Tailscale because this process is isolated (TAKO_ISOLATED). \
                 Installing is machine-wide and outside the sandbox (a fake TAKO_TAILSCALE_BIN does not make \
                 the installer fake). To exercise the install, set {}=1",
                crate::remote_isolated_serve::OPT_IN_ENV
            ),
            (Self::AdminTokenRequired, Ja) => "管理 API はローカルの管理トークンが必要".to_string(),
            (Self::AdminTokenRequired, En) => {
                "the admin API needs the local admin token".to_string()
            }
            (Self::DeviceIdRequired, Ja) => "device_id が必要".to_string(),
            (Self::DeviceIdRequired, En) => "device_id is required".to_string(),
            (Self::AdminEndpointNotFound, Ja) => "管理 API エンドポイントが見つからない".to_string(),
            (Self::AdminEndpointNotFound, En) => "admin API endpoint not found".to_string(),
            (Self::ApiEndpointNotFound, Ja) => "API エンドポイントが見つからない".to_string(),
            (Self::ApiEndpointNotFound, En) => "API endpoint not found".to_string(),
            (Self::PaneIdInvalid, Ja) => "無効なペイン ID".to_string(),
            (Self::PaneIdInvalid, En) => "invalid pane ID".to_string(),
            (Self::SessionIdMalformed, Ja) => "無効なセッション ID".to_string(),
            (Self::SessionIdMalformed, En) => "invalid session ID".to_string(),
            (Self::TabIdInvalid, Ja) => "無効なタブ ID".to_string(),
            (Self::TabIdInvalid, En) => "invalid tab ID".to_string(),
            (Self::SshPaneIdNotNumeric, Ja) => {
                "SSH の対象は数値ペイン ID のみ（一覧の id を渡す）".to_string()
            }
            (Self::SshPaneIdNotNumeric, En) => {
                "the SSH target must be a numeric pane ID (pass an id from the list)".to_string()
            }
            (Self::RespondPaneIdNotNumeric, Ja) => {
                "respond は数値ペイン ID のみ（app 稼働時の一覧から取得してください）".to_string()
            }
            (Self::RespondPaneIdNotNumeric, En) => {
                "respond takes a numeric pane ID only (take one from the list while the app is running)"
                    .to_string()
            }
            (Self::ChoiceRequired, Ja) => "choice が必要（番号または yes/no）".to_string(),
            (Self::ChoiceRequired, En) => "choice is required (a number or yes/no)".to_string(),
            (Self::ResizeSizeRequired, Ja) => {
                "cols と rows（正の整数）か reset=true を指定する".to_string()
            }
            (Self::ResizeSizeRequired, En) => {
                "specify cols and rows (positive integers) or reset=true".to_string()
            }
            (Self::Internal, Ja) => "内部エラー".to_string(),
            (Self::Internal, En) => "internal error".to_string(),
            (Self::AppNotRunning(via), Ja) => match via.what_in(lang) {
                Some(what) => format!("tako app が稼働していない（{what}は app 経由のみ）"),
                None => "tako app が稼働していない".to_string(),
            },
            (Self::AppNotRunning(via), En) => match via.what_in(lang) {
                Some(what) => {
                    format!("the tako app is not running ({what} only works through the app)")
                }
                None => "the tako app is not running".to_string(),
            },
            (Self::TabNotFound(id), Ja) => format!("タブ {id} が見つからない"),
            (Self::TabNotFound(id), En) => format!("tab {id} not found"),
            (Self::PaneNotFound(pane), Ja) => format!("ペイン '{pane}' が見つからない"),
            (Self::PaneNotFound(pane), En) => format!("pane '{pane}' not found"),
            (Self::InputSecretSuspected, Ja) => {
                "鍵らしき文字が入っている（送っていない）".to_string()
            }
            (Self::InputSecretSuspected, En) => {
                "the text looks like it contains a secret (not sent)".to_string()
            }
            (Self::BodyUnreadable, Ja) => "リクエストボディの読み取りに失敗".to_string(),
            (Self::BodyUnreadable, En) => "failed to read the request body".to_string(),
            (Self::BodyJsonInvalid(error), Ja) => format!("JSON パースエラー: {error}"),
            (Self::BodyJsonInvalid(error), En) => format!("JSON parse error: {error}"),
            (Self::OriginNotAllowed(origin), Ja) => format!("Origin '{origin}' は許可されていない"),
            (Self::OriginNotAllowed(origin), En) => format!("Origin '{origin}' is not allowed"),
            (Self::PeerUnverified(reject), Ja) => format!(
                "serve 経由を名乗る接続の接続元を検証できない: {}",
                reject.describe_in(lang)
            ),
            (Self::PeerUnverified(reject), En) => format!(
                "cannot verify the peer of a connection that claims to come through serve: {}",
                reject.describe_in(lang)
            ),
            (Self::ServeOnly, Ja) => "tailscale serve 経由のアクセスのみ受け付ける".to_string(),
            (Self::ServeOnly, En) => "only access through tailscale serve is accepted".to_string(),
            (Self::ForwardedForEmpty, Ja) => "X-Forwarded-For が空".to_string(),
            (Self::ForwardedForEmpty, En) => "X-Forwarded-For is empty".to_string(),
            (Self::ForwardedHostMissing, Ja) => {
                "X-Forwarded-For はあるが X-Forwarded-Host が欠落（serve 経由でない可能性）"
                    .to_string()
            }
            (Self::ForwardedHostMissing, En) => {
                "X-Forwarded-For is present but X-Forwarded-Host is missing (possibly not through serve)"
                    .to_string()
            }
            (Self::ForwardedHostMismatch(host), Ja) => {
                format!("X-Forwarded-Host が期待値と不一致（受信: {host}）")
            }
            (Self::ForwardedHostMismatch(host), En) => {
                format!("X-Forwarded-Host does not match the expected value (received: {host})")
            }
            // 日本語も英語も whois の言い方（`peer not found`）をそのまま括弧に残す
            (Self::PeerNotInTailnet(ip), Ja) => {
                format!("接続元 {ip} は tailnet 上のノードではない（whois: peer not found）")
            }
            (Self::PeerNotInTailnet(ip), En) => {
                format!("the peer {ip} is not a node on the tailnet (whois: peer not found)")
            }
            (Self::WhoisFailed(error), Ja) => format!("接続元の identity 検証に失敗: {error}"),
            (Self::WhoisFailed(error), En) => {
                format!("failed to verify the identity of the peer: {error}")
            }
            (Self::DeviceNotPaired, Ja) => {
                "この端末はペアリングされていない。PWA のペアリング画面から要求してください"
                    .to_string()
            }
            (Self::DeviceNotPaired, En) => {
                "this device is not paired. Request pairing from the pairing screen of the PWA"
                    .to_string()
            }
            (Self::RoleTooLow { required, current }, Ja) => format!(
                "この操作には {} 以上の role が必要（現在: {}）。\
                 role の変更はペアリング画面から要求できます",
                required.as_str(),
                current.as_str()
            ),
            (Self::RoleTooLow { required, current }, En) => format!(
                "this operation needs the {} role or higher (current: {}). \
                 You can request a role change from the pairing screen",
                required.as_str(),
                current.as_str()
            ),
            // 日本語が「見つからない」と言わないので、英語も ` not found` を含めない
            (Self::PairingRequestNotPending(id), Ja) => {
                format!("保留中のペアリング要求が無い: {id}")
            }
            (Self::PairingRequestNotPending(id), En) => {
                format!("there is no pending pairing request: {id}")
            }
            (Self::WsProtocolRequired, Ja) => format!(
                "Sec-WebSocket-Protocol に '{}' が必要",
                crate::remote::WS_PROTOCOL
            ),
            (Self::WsProtocolRequired, En) => format!(
                "Sec-WebSocket-Protocol must include '{}'",
                crate::remote::WS_PROTOCOL
            ),
            (Self::WsPaneQueryRequired, Ja) => "pane クエリが必要".to_string(),
            (Self::WsPaneQueryRequired, En) => "the pane query is required".to_string(),
            (Self::WsKeyMissing, Ja) => "Sec-WebSocket-Key ヘッダが無い".to_string(),
            (Self::WsKeyMissing, En) => "the Sec-WebSocket-Key header is missing".to_string(),
            (Self::UploadTooLarge(mb), Ja) => {
                format!("ファイルサイズが上限 ({mb}MB) を超えています")
            }
            (Self::UploadTooLarge(mb), En) => format!("the file exceeds the size limit ({mb}MB)"),
            (Self::UploadNotMultipart, Ja) => "multipart/form-data が必要です".to_string(),
            (Self::UploadNotMultipart, En) => "multipart/form-data is required".to_string(),
            (Self::UploadNoFile, Ja) => "ファイルが含まれていません".to_string(),
            (Self::UploadNoFile, En) => "the request contains no file".to_string(),
            (Self::UploadPaneRequired, Ja) => "pane パラメータが必要です".to_string(),
            (Self::UploadPaneRequired, En) => "the pane parameter is required".to_string(),
            (Self::UploadDataUnreadable, Ja) => "ファイルデータの読み取りに失敗".to_string(),
            (Self::UploadDataUnreadable, En) => "failed to read the file data".to_string(),
            (Self::UploadFileNameInvalid, Ja) => "不正なファイル名です".to_string(),
            (Self::UploadFileNameInvalid, En) => "invalid file name".to_string(),
            (Self::UploadPaneCwdUnknown, Ja) => {
                "対象ペインの作業ディレクトリが取得できません".to_string()
            }
            (Self::UploadPaneCwdUnknown, En) => {
                "cannot get the working directory of the target pane".to_string()
            }
            (Self::UploadDirCreate(error), Ja) => {
                format!("アップロードディレクトリの作成に失敗: {error}")
            }
            (Self::UploadDirCreate(error), En) => {
                format!("failed to create the upload directory: {error}")
            }
            (Self::UploadDirSymlink, Ja) => {
                "アップロード先がシンボリックリンクのため拒否しました".to_string()
            }
            (Self::UploadDirSymlink, En) => {
                "refused because the upload directory is a symbolic link".to_string()
            }
            (Self::UploadTargetSymlink, Ja) => {
                "既存のシンボリックリンクへの上書きは拒否しました".to_string()
            }
            (Self::UploadTargetSymlink, En) => {
                "refused to overwrite an existing symbolic link".to_string()
            }
            (Self::UploadPathResolve(error), Ja) => format!("パス解決に失敗: {error}"),
            (Self::UploadPathResolve(error), En) => format!("failed to resolve the path: {error}"),
            (Self::UploadWrite(error), Ja) => format!("ファイル書き込みに失敗: {error}"),
            (Self::UploadWrite(error), En) => format!("failed to write the file: {error}"),
            (Self::UploadTraversal, Ja) => "パス traversal が検出されました".to_string(),
            (Self::UploadTraversal, En) => "path traversal detected".to_string(),
            (Self::TmuxSpawn(error), Ja) => format!("tmux コマンドの起動に失敗: {error}"),
            (Self::TmuxSpawn(error), En) => format!("failed to start the tmux command: {error}"),
            (Self::TmuxDrainThread { stream, error }, Ja) => {
                format!("{stream} drain スレッドの起動に失敗: {error}")
            }
            (Self::TmuxDrainThread { stream, error }, En) => {
                format!("failed to start the {stream} drain thread: {error}")
            }
            (Self::TmuxTimeout { command, secs }, Ja) => {
                format!("tmux {command} がタイムアウト（{secs}秒）")
            }
            (Self::TmuxTimeout { command, secs }, En) => {
                format!("tmux {command} timed out ({secs}s)")
            }
            (Self::TmuxWait(error), Ja) => format!("プロセスの待機に失敗: {error}"),
            (Self::TmuxWait(error), En) => format!("failed to wait for the process: {error}"),
            (Self::TmuxCaptureFailed(stderr), Ja) => {
                format!("tmux capture-pane がエラー: {stderr}")
            }
            (Self::TmuxCaptureFailed(stderr), En) => {
                format!("tmux capture-pane failed: {stderr}")
            }
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(Lang::Ja)
    }

    /// 「無効なパラメータ: 」の断りへ載せる（表示言語で組む）
    pub fn invalid(self) -> DispatchError {
        DispatchError::InvalidParams(self.to_string())
    }

    /// 前置きの無い理由の文として載せる（表示言語で組む）
    pub fn operation(self) -> DispatchError {
        DispatchError::Operation(self.to_string())
    }

    /// daemon の admin API の本文の `kind`（頼んだ側が組み直せる断りだけ。それ以外は `None`）
    pub fn admin_kind(&self) -> Option<&'static str> {
        match self {
            Self::DeviceNotRegistered(_) => Some(KIND_DEVICE_NOT_REGISTERED),
            Self::UpgradeRequiresGui => Some(crate::remote_role::DENY_UPGRADE_REQUIRES_GUI),
            _ => None,
        }
    }

    /// daemon の admin API が断るときの本文。`error` は daemon の表示言語（起動した時点で決まる。
    /// 日本語表示は修正前とバイト一致。旧版の頼み手はこれをそのまま出す）で、頼んだ側が組み直せる
    /// 断りは `kind`（と端末の `device_id`）も添える（[`Self::from_admin_body`] が読み、呼び手の
    /// 表示言語で組み直す）
    pub fn admin_body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({ "error": self.to_string() });
        if let Some(kind) = self.admin_kind() {
            body["kind"] = serde_json::json!(kind);
        }
        if let Self::DeviceNotRegistered(id) = self {
            body["device_id"] = serde_json::json!(id);
        }
        body
    }

    /// daemon の admin API の断りの本文を読む（[`Self::admin_kind`] の逆。`kind` が無い・知らない
    /// ものは `error` をそのまま載せる）
    pub fn from_admin_body(body: &serde_json::Value) -> DaemonMessage {
        let kind = body["kind"].as_str();
        if kind == Some(KIND_DEVICE_NOT_REGISTERED) {
            if let Some(id) = body["device_id"].as_str() {
                return DaemonMessage::Refusal(Box::new(Self::DeviceNotRegistered(id.to_string())));
            }
        }
        if kind == Some(crate::remote_role::DENY_UPGRADE_REQUIRES_GUI) {
            return DaemonMessage::Refusal(Box::new(Self::UpgradeRequiresGui));
        }
        match body["error"].as_str() {
            Some(text) => DaemonMessage::Text(text.to_string()),
            None => DaemonMessage::Missing,
        }
    }
}

impl DaemonMessage {
    /// 表示言語を明示しての文言。日本語は #2365 より前とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::Refusal(r), _) => r.text_in(lang),
            // daemon の文は手を加えずに載せる
            (Self::Text(text), _) => text.clone(),
            (Self::Missing, Lang::Ja) => "不明なエラー".to_string(),
            (Self::Missing, Lang::En) => "unknown error".to_string(),
        }
    }
}

impl AppVia {
    /// 「…は app 経由のみ」の操作の名前（[`Self::Plain`] は括弧を付けないので `None`）。
    /// 日本語は #2365 より前の固定文とバイト一致（close だけ英単語の後ろに空白が入っていた）
    pub fn what_in(self, lang: Lang) -> Option<&'static str> {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Plain, _) => None,
            (Self::Input, Ja) => Some("リモートからの入力"),
            (Self::Input, En) => Some("remote input"),
            (Self::Respond, Ja) => Some("リモートからの承認応答"),
            (Self::Respond, En) => Some("answering dialogs remotely"),
            (Self::Close, Ja) => Some("リモートからの close "),
            (Self::Close, En) => Some("closing panes remotely"),
            (Self::TabNew, Ja) => Some("リモートからのタブ作成"),
            (Self::TabNew, En) => Some("creating tabs remotely"),
            (Self::MasterLaunch, Ja) => Some("リモートからの master 起動"),
            (Self::MasterLaunch, En) => Some("launching a master remotely"),
            (Self::Tasks, Ja) => Some("タスクの操作"),
            (Self::Tasks, En) => Some("task operations"),
            (Self::Cards, Ja) => Some("コマンドカードの操作"),
            (Self::Cards, En) => Some("command card operations"),
        }
    }
}

impl ScrollbackCause {
    /// 表示言語を明示しての文言（日本語は #2365 より前とバイト一致）
    pub fn text_in(&self, lang: Lang) -> String {
        match self {
            Self::NoCapture => crate::reach::no_detached_capture_note_in(lang),
            Self::Backend(e) => e.text_in(lang),
        }
    }
}

impl std::fmt::Display for RemoteRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for RemoteRefusal {}
