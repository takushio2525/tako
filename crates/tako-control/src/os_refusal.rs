//! OS の設定・統合の操作（フルディスクアクセス・スリープ防止 / 蓋閉じ継続・シェル統合）の断り（#2365）。
//!
//! dispatch の `tako_fda` / `tako_sleep_guard` / `tako_shell_integration` は、以前はこれらの口の
//! `Err(String)`（日本語の固定文）を `Operation` / `InvalidParams` の中身としてそのまま載せていたので、
//! 英語表示でも MCP の応答の理由だけ日本語で返った。日英の対は [`OsRefusal::text_in`] の
//! (バリアント, 言語) の網羅 match の 1 か所に置く（形は [`crate::SessionRefusal`] と同じ）。
//! 日本語は #2365 より前の固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
//!
//! # 口は 2 つ
//!
//! 従来の口（`fda::open_settings` / `sleep_guard::parse_battery_floor` ほか）は `String` を返し、文は
//! **日本語のまま**（CLI の `tako fda` / `tako sleep-guard` / `tako shell-integration` は GUI を通さない
//! 手元の処理で、表示全体が日本語）。断りを表示言語で返す dispatch だけが型の口（`*_typed`）を通す。
//!
//! # 枠と中身を一緒に移したもの
//!
//! 蓋閉じ継続の解除（`sleep_guard::teardown_lid_control_typed`。Windows の中身は `platform::lid` の
//! [`LidError`]）と System Settings の Battery を開く口（`sleep_guard::open_battery_settings_typed`。
//! 中身は `platform::os_integration` の [`OsIntegrationError`]）は、枠だけ移すと 1 つの断りに日英が
//! 混ざるので、中身の型を包んで同じ言語で組む。ファイル操作（Finder / エクスプローラーで表示・
//! 既定アプリ / 指定のアプリで開く・ゴミ箱へ移す）の断りは枠を持たず中身をそのまま載せる
//! （[`OsRefusal::Integration`]）

use tako_core::i18n::Lang;
use tako_core::shell_integration::SiError;

use crate::platform::lid::LidError;
use crate::platform::os_integration::OsIntegrationError;
use crate::sleep_guard::{LID_BATTERY_FLOOR_MAX, LID_BATTERY_FLOOR_MIN};
use crate::{DispatchError, Reason};

/// OS の設定・統合の操作の断り。`error` / `detail` は OS・外部コマンドのエラー文（言語に依らずそのまま
/// 載せる）
#[derive(Debug, Clone, PartialEq)]
pub enum OsRefusal {
    // ---- フルディスクアクセス ----
    /// システム設定のフルディスクアクセスのパネルを開けない（候補の URL をすべて試した）
    FdaPanelUnopenable,
    /// macOS 以外（フルディスクアクセスは macOS の仕組み）
    FdaMacOnly,

    // ---- スリープ防止・蓋閉じ継続 ----
    /// 蓋閉じ継続の残量下限が受け付ける範囲の外（値は指定のまま）
    BatteryFloorOutOfRange(i64),
    /// この OS では蓋閉じ継続を使えない
    LidUnsupported,
    /// osascript（管理者権限の確認）を起動できない
    OsascriptFailed(String),
    /// 管理者権限の確認をユーザーがキャンセルした
    UserCanceled,
    /// sudoers の登録に失敗（`detail` は osascript の出力）
    SudoersInstallFailed(String),
    /// sudoers の削除に失敗（`detail` は osascript の出力）
    SudoersRemoveFailed(String),
    /// 蓋閉じ継続の解除に失敗（macOS 以外。中身は `platform::lid` の断り）
    LidTeardownFailed(LidError),
    /// System Settings の Battery を開けない（中身は `os_integration` の断り）
    BatterySettingsUnopenable(OsIntegrationError),
    /// macOS 以外（System Settings の Battery は macOS の画面）
    BatterySettingsMacOnly,

    // ---- OS のシェルに委ねるファイル操作 ----
    /// Finder / エクスプローラーで表示・既定アプリ / 指定のアプリで開く・ゴミ箱へ移すの断り
    /// （枠を持たず、文は `os_integration` の表のまま）
    Integration(OsIntegrationError),

    // ---- シェル統合 ----
    /// `tako_shell_integration` の action が status / install / uninstall のどれでもない
    UnknownShellIntegrationAction(String),
    /// 配置操作の断り（文は tako-core の `SiError` の表）
    ShellIntegration(SiError),
}

impl OsRefusal {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::FdaPanelUnopenable, Ja) => {
                "システム設定のフルディスクアクセスパネルを開けませんでした".to_string()
            }
            (Self::FdaPanelUnopenable, En) => {
                "could not open the Full Disk Access pane in System Settings".to_string()
            }
            (Self::FdaMacOnly, Ja) => "フルディスクアクセスは macOS 専用の機能です".to_string(),
            (Self::FdaMacOnly, En) => "Full Disk Access is a macOS-only feature".to_string(),
            (Self::BatteryFloorOutOfRange(percent), Ja) => format!(
                "残量下限は {LID_BATTERY_FLOOR_MIN}〜{LID_BATTERY_FLOOR_MAX} の範囲で指定してください（指定値: {percent}）"
            ),
            (Self::BatteryFloorOutOfRange(percent), En) => format!(
                "the battery floor must be between {LID_BATTERY_FLOOR_MIN} and \
                 {LID_BATTERY_FLOOR_MAX} (got: {percent})"
            ),
            (Self::LidUnsupported, Ja) => "この OS では蓋閉じ継続に対応していません".to_string(),
            (Self::LidUnsupported, En) => {
                "this OS does not support staying awake with the lid closed".to_string()
            }
            (Self::OsascriptFailed(e), Ja) => format!("osascript の実行に失敗: {e}"),
            (Self::OsascriptFailed(e), En) => format!("failed to run osascript: {e}"),
            (Self::UserCanceled, Ja) => "ユーザーがキャンセルしました".to_string(),
            (Self::UserCanceled, En) => "canceled by the user".to_string(),
            (Self::SudoersInstallFailed(detail), Ja) => format!("sudoers 登録に失敗: {detail}"),
            (Self::SudoersInstallFailed(detail), En) => {
                format!("failed to install the sudoers entry: {detail}")
            }
            (Self::SudoersRemoveFailed(detail), Ja) => format!("sudoers 削除に失敗: {detail}"),
            (Self::SudoersRemoveFailed(detail), En) => {
                format!("failed to remove the sudoers entry: {detail}")
            }
            (Self::LidTeardownFailed(e), Ja) => {
                format!("蓋閉じ継続の解除に失敗: {}", e.text_in(lang))
            }
            (Self::LidTeardownFailed(e), En) => format!(
                "failed to stop staying awake with the lid closed: {}",
                e.text_in(lang)
            ),
            (Self::BatterySettingsUnopenable(e), Ja) => {
                format!("System Settings を開けません: {}", e.text_in(lang))
            }
            (Self::BatterySettingsUnopenable(e), En) => {
                format!("cannot open System Settings: {}", e.text_in(lang))
            }
            (Self::BatterySettingsMacOnly, Ja) => "macOS 以外では非対応".to_string(),
            (Self::BatterySettingsMacOnly, En) => {
                "the Battery pane in System Settings is macOS-only".to_string()
            }
            (Self::Integration(e), _) => e.text_in(lang),
            (Self::UnknownShellIntegrationAction(got), _) => Reason::UnknownValue {
                key: "action",
                got: got.clone(),
                allowed: crate::shell_integration::Action::VALUES.join(" / "),
            }
            .text_in(lang),
            (Self::ShellIntegration(e), _) => e.text_in(lang),
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(Lang::Ja)
    }

    /// 「無効なパラメータ: 」の断りへ載せる（枠と同じ表示言語で組む）
    pub fn invalid(self) -> DispatchError {
        DispatchError::InvalidParams(self.to_string())
    }

    /// 前置きの無い理由の文として載せる（表示言語で組む）
    pub fn operation(self) -> DispatchError {
        DispatchError::Operation(self.to_string())
    }
}

impl std::fmt::Display for OsRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for OsRefusal {}

impl From<SiError> for OsRefusal {
    fn from(e: SiError) -> Self {
        Self::ShellIntegration(e)
    }
}

impl From<OsIntegrationError> for OsRefusal {
    fn from(e: OsIntegrationError) -> Self {
        Self::Integration(e)
    }
}
