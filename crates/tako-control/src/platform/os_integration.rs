//! OS シェル連携（抽象境界 B8）
//!
//! 「ファイルマネージャで表示」「既定アプリで開く」「URL を開く」「ゴミ箱へ移す」
//! 「通知を出す」など、**OS のシェルに委ねる操作**をここへ集約する。
//!
//! - macOS: 表示は tako 自身から `NSWorkspace activateFileViewerSelectingURLs:`
//!   （`platform::finder_reveal`。#2247）、開く系は `open` 系（`-a` / `-t` / `-n`）と `osascript`
//! - Windows: 表示は `explorer.exe /select,`、開く系は `ShellExecuteW`、
//!   ゴミ箱は `SHFileOperationW` + `FOF_ALLOWUNDO`（#617）。URL も `ShellExecuteW`（#1371）
//! - その他 unix: URL は `xdg-open`
//!
//! 呼び出し側（`dispatch` / UI / CLI）はこのモジュールだけを見る。
//! **呼び出し側に `#[cfg]` を書かない**（`.agent/plans/2026-07-windows-port-architecture.md` 原則 1）。
//!
//! ## Windows 実装の方針（#617）
//!
//! `windows-sys` を足さず、必要な関数だけを手書きで宣言する（B17 = `platform::ime` の
//! IMM32、`agents` の Toolhelp32、`sleep_guard` の IOKit と同じ方針）。
//! ゴミ箱は COM の `IFileOperation` でも実現できるが、単発の削除に COM の
//! インターフェース定義を持ち込むのは重い。`SHFileOperationW` は Vista 以降
//! 非推奨扱いながら現役で、二重 NUL 終端のパスと構造体 1 つで済む。
//!
//! **`FOF_ALLOWUNDO` は絶対パスでなければゴミ箱へ入らず、黙って完全削除になる**
//! （MSDN 明記）。相対パスを渡されても復元可能であることを保つため、絶対化は
//! 境界の内側で必ず行う。
//!
//! ## ここに置かないもの
//!
//! - **権限昇格**（`osascript … with administrator privileges`）は B9（スリープ防止 =
//!   `sleep_guard`）の内側に留める。「OS シェルに何かを開かせる」操作ではないうえ、
//!   汎用の昇格 API を B8 に置くと危険な踏み台になる。Windows の B9 実装
//!   （`SetThreadExecutionState`）は sudoers 相当を必要としないため、そもそも対応物が無い
//!
//! ## 断りの文（#2365）
//!
//! 断りは [`OsIntegrationError`] の (バリアント, 言語) の網羅 match の 1 か所に置く。
//! **macOS / Windows / その他の実装の文をすべて `cfg` の外に置く**ので、Windows の
//! `ShellExecuteW` / `SHFileOperationW` の説明も macOS の単体テストで日英を検査できる。
//! 従来の口（[`reveal`] ほか）は `String` を返し、文は**日本語のまま**（診断ログと、結果を捨てる
//! 呼び手が使う）。断りを利用者へ見せる口（dispatch・GUI の通知欄・更新の断り・プレビューのリンク）は
//! 型の口（`*_typed`）を通して表示言語で引く。GUI だけが呼ぶ「アプリを選んで開く」と「新しいプロセスで
//! 起動」は型の口だけを持つ

use std::path::Path;

use tako_core::i18n::Lang;
use tako_core::url_guard::UrlBlocked;

/// OS のシェルに委ねる操作（[`OsIntegrationError::Spawn`] / [`OsIntegrationError::ShellExecute`]
/// の枠）。何をしようとして失敗したかの短い言い方
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Finder で表示（macOS の `open -R`。#2247 の A/B）
    Finder,
    /// エクスプローラーで表示（Windows の `explorer.exe /select,`）
    Explorer,
    /// 既定アプリで開く
    DefaultApp,
    /// アプリを指定して開く（値は指定のまま）
    App(String),
    /// 既定のテキストエディタで開く
    TextEditor,
    /// URL を開く
    Url,
    /// アプリを新しいプロセスとして起動する（macOS の `open -n`）
    NewInstance,
    /// アプリ選択ダイアログ（Windows の `openas`）
    AppPicker,
}

impl Launch {
    /// (操作, 言語) の網羅 match。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Finder, Ja) => "Finder を開けない".to_string(),
            (Self::Finder, En) => "cannot open Finder".to_string(),
            (Self::Explorer, Ja) => "エクスプローラーを開けない".to_string(),
            (Self::Explorer, En) => "cannot open File Explorer".to_string(),
            (Self::DefaultApp, Ja) => "デフォルトアプリで開けない".to_string(),
            (Self::DefaultApp, En) => "cannot open with the default app".to_string(),
            (Self::App(app), Ja) => format!("アプリ '{app}' で開けない"),
            (Self::App(app), En) => format!("cannot open with the app '{app}'"),
            (Self::TextEditor, Ja) => "テキストエディタで開けない".to_string(),
            (Self::TextEditor, En) => "cannot open in a text editor".to_string(),
            (Self::Url, Ja) => "URL を開けない".to_string(),
            (Self::Url, En) => "cannot open the URL".to_string(),
            (Self::NewInstance, Ja) => "アプリを起動できない".to_string(),
            (Self::NewInstance, En) => "cannot launch the app".to_string(),
            (Self::AppPicker, Ja) => "アプリ選択ダイアログを開けない".to_string(),
            (Self::AppPicker, En) => "cannot open the app chooser".to_string(),
        }
    }
}

/// この環境に実装が無い操作（[`OsIntegrationError::Unsupported`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsFeature {
    /// ファイルマネージャでの表示
    Reveal,
    /// 既定アプリで開く
    OpenDefault,
    /// アプリを指定して開く
    OpenWith,
    /// アプリ選択ダイアログ
    AppPicker,
    /// テキストエディタで開く
    TextEditor,
    /// アプリの新規プロセス起動
    NewInstance,
    /// ゴミ箱への移動
    Trash,
}

impl OsFeature {
    /// (操作, 言語) の網羅 match。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(self, lang: Lang) -> &'static str {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Reveal, Ja) => "ファイルマネージャでの表示",
            (Self::Reveal, En) => "showing a file in the file manager",
            (Self::OpenDefault, Ja) => "既定アプリで開く操作",
            (Self::OpenDefault, En) => "opening with the default app",
            (Self::OpenWith, Ja) => "アプリを指定して開く操作",
            (Self::OpenWith, En) => "opening with a chosen app",
            (Self::AppPicker, Ja) => "アプリ選択ダイアログ",
            (Self::AppPicker, En) => "the app chooser",
            (Self::TextEditor, Ja) => "テキストエディタで開く操作",
            (Self::TextEditor, En) => "opening in a text editor",
            (Self::NewInstance, Ja) => "アプリの新規プロセス起動",
            (Self::NewInstance, En) => "launching the app as a new process",
            (Self::Trash, Ja) => "ゴミ箱への移動",
            (Self::Trash, En) => "moving to the Trash",
        }
    }
}

/// OS のシェルに委ねる操作の断り（#2365）。`error` / `detail` は OS・外部コマンドのエラー文
/// （言語に依らずそのまま載せる）、パスは表記のまま埋める。
///
/// macOS の Finder への頼み（`platform::finder_reveal`）・Windows の `ShellExecuteW` /
/// `SHFileOperationW` の戻り値の説明もここに置く（実装の `cfg` の外 = macOS でも日英を検査できる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsIntegrationError {
    // ---- 共通 ----
    /// URL の検査で止めた（理由は分類だけ。**URL そのものは載せない** = #1376）
    UrlBlocked(UrlBlocked),
    /// ランチャ（`open` / `explorer.exe` / `xdg-open`）を起こせない
    Spawn { what: Launch, error: String },
    /// この環境に実装が無い（その他 unix の全般・Windows の新規プロセス起動）
    Unsupported(OsFeature),

    // ---- macOS / その他 unix: 待つ版の URL ----
    /// macOS: `open` コマンドを実行できない
    OpenCommandFailed(String),
    /// その他 unix: URL ハンドラ（`xdg-open`）を実行できない
    UrlHandlerFailed(String),
    /// URL のハンドラが失敗の終了状態で終わった（`status` は終了状態の表記）
    UrlExitStatus { status: String, url: String },

    // ---- macOS: アプリ選択・ゴミ箱 ----
    /// アプリ選択の osascript を起こせない
    OsascriptSpawn(String),
    /// アプリ選択をキャンセルした
    AppPickCanceled,
    /// アプリ選択が空のパスを返した
    AppPathEmpty,
    /// ゴミ箱へ移せない（Finder への頼み・検証用の置き場 `TAKO_TRASH_DIR` への移動）
    TrashFailed(String),
    /// 検証用の置き場へ移す対象に名前が無い（パスは表記のまま）
    TrashNoName(String),

    // ---- macOS: Finder への頼み（`platform::finder_reveal`。#2247） ----
    /// AppKit を読み込めない
    AppKitUnavailable,
    /// NSWorkspace を使えない
    WorkspaceUnavailable,
    /// NSString へ渡す文字列に NUL がある
    NulInString,
    /// NSString を作れない
    NsStringFailed,
    /// ファイルの URL へ渡すパスに NUL がある
    NulInPath(String),
    /// ファイルの URL を作れない
    FileUrlFailed(String),
    /// 検証の記録: ファイルの URL からパスを読み戻せない
    RecordPathUnreadable,
    /// 検証の記録（`TAKO_REVEAL_RECORD`）を書けない
    RecordWrite { path: String, error: String },

    // ---- Windows ----
    /// パスに NUL がある（Win32 へ渡すと手前で切れて別のパスを指す）
    PathHasNul,
    /// `explorer.exe /select,` へ渡すパスに引用符がある
    PathHasQuote,
    /// パスを絶対化できない
    Absolutize(String),
    /// パスが存在しない（パスは絶対化した表記）
    PathMissing(String),
    /// `ShellExecuteW` が失敗（`code` は戻り値。説明は [`shell_execute_reason`]）
    ShellExecute { what: Launch, code: isize },
    /// `SHFileOperationW` が失敗（`code` は戻り値。説明は [`shfileop_reason`]）
    TrashShellOp(i32),
    /// 「完全に削除しますか？」を断った（ごみ箱へ入れられない対象）
    TrashAborted,
}

impl OsIntegrationError {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    /// （`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::UrlBlocked(reason), Ja) => {
                format!("この URL は開けない（{}）", reason.text_in(lang))
            }
            (Self::UrlBlocked(reason), En) => {
                format!("cannot open this URL ({})", reason.text_in(lang))
            }
            (Self::Spawn { what, error }, _) => format!("{}: {error}", what.text_in(lang)),
            (Self::Unsupported(feature), Ja) => {
                format!(
                    "{}はこのプラットフォームでは未対応です",
                    feature.text_in(lang)
                )
            }
            (Self::Unsupported(feature), En) => {
                format!(
                    "{} is not supported on this platform",
                    feature.text_in(lang)
                )
            }
            (Self::OpenCommandFailed(e), Ja) => format!("open コマンドの実行に失敗: {e}"),
            (Self::OpenCommandFailed(e), En) => format!("failed to run the open command: {e}"),
            (Self::UrlHandlerFailed(e), Ja) => format!("URL ハンドラの実行に失敗: {e}"),
            (Self::UrlHandlerFailed(e), En) => format!("failed to run the URL handler: {e}"),
            (Self::UrlExitStatus { status, url }, Ja) => {
                format!("URL を開けない（終了コード {status}）: {url}")
            }
            (Self::UrlExitStatus { status, url }, En) => {
                format!("cannot open the URL (exit code {status}): {url}")
            }
            (Self::OsascriptSpawn(e), Ja) => format!("osascript 起動に失敗: {e}"),
            (Self::OsascriptSpawn(e), En) => format!("failed to start osascript: {e}"),
            (Self::AppPickCanceled, Ja) => "アプリ選択がキャンセルされた".to_string(),
            (Self::AppPickCanceled, En) => "the app selection was canceled".to_string(),
            (Self::AppPathEmpty, Ja) => "アプリパスが空".to_string(),
            (Self::AppPathEmpty, En) => "the app path is empty".to_string(),
            (Self::TrashFailed(e), Ja) => format!("ゴミ箱への移動に失敗: {e}"),
            (Self::TrashFailed(e), En) => format!("failed to move to the Trash: {e}"),
            (Self::TrashNoName(path), Ja) => {
                format!("ゴミ箱への移動に失敗: 名前が無い: {path}")
            }
            (Self::TrashNoName(path), En) => {
                format!("failed to move to the Trash: the path has no file name: {path}")
            }
            (Self::AppKitUnavailable, Ja) => "Finder を開けない: AppKit を読み込めない".to_string(),
            (Self::AppKitUnavailable, En) => {
                "cannot open Finder: AppKit could not be loaded".to_string()
            }
            (Self::WorkspaceUnavailable, Ja) => {
                "Finder を開けない: NSWorkspace を使えない".to_string()
            }
            (Self::WorkspaceUnavailable, En) => {
                "cannot open Finder: NSWorkspace is unavailable".to_string()
            }
            (Self::NulInString, Ja) => "NUL を含む文字列".to_string(),
            (Self::NulInString, En) => "the string contains a NUL character".to_string(),
            (Self::NsStringFailed, Ja) => "NSString を作れない".to_string(),
            (Self::NsStringFailed, En) => "cannot create an NSString".to_string(),
            (Self::NulInPath(path), Ja) => format!("NUL を含むパス: {path}"),
            (Self::NulInPath(path), En) => format!("the path contains a NUL character: {path}"),
            (Self::FileUrlFailed(path), Ja) => format!("ファイルの URL を作れない: {path}"),
            (Self::FileUrlFailed(path), En) => {
                format!("cannot create a file URL for: {path}")
            }
            (Self::RecordPathUnreadable, Ja) => "ファイルの URL からパスを読み戻せない".to_string(),
            (Self::RecordPathUnreadable, En) => {
                "cannot read the path back from the file URL".to_string()
            }
            (Self::RecordWrite { path, error }, Ja) => {
                format!("記録を書けない（{path}）: {error}")
            }
            (Self::RecordWrite { path, error }, En) => {
                format!("cannot write the record ({path}): {error}")
            }
            (Self::PathHasNul, Ja) => "パスに NUL 文字が含まれている".to_string(),
            (Self::PathHasNul, En) => "the path contains a NUL character".to_string(),
            (Self::PathHasQuote, Ja) => "パスに引用符が含まれている".to_string(),
            (Self::PathHasQuote, En) => "the path contains a double quote".to_string(),
            (Self::Absolutize(e), Ja) => format!("パスを絶対化できない: {e}"),
            (Self::Absolutize(e), En) => format!("cannot make the path absolute: {e}"),
            (Self::PathMissing(path), Ja) => format!("パスが存在しない: {path}"),
            // dispatch の `Reason::PathNotFound` と同じ日本語なので英語も同じ文にそろえる
            (Self::PathMissing(path), En) => format!("path does not exist: {path}"),
            (Self::ShellExecute { what, code }, _) => format!(
                "{}: {}",
                what.text_in(lang),
                shell_execute_reason(*code, lang)
            ),
            (Self::TrashShellOp(code), Ja) => {
                format!("ごみ箱への移動に失敗: {}", shfileop_reason(*code, lang))
            }
            (Self::TrashShellOp(code), En) => format!(
                "failed to move to the Recycle Bin: {}",
                shfileop_reason(*code, lang)
            ),
            (Self::TrashAborted, Ja) => "ごみ箱への移動が取り消された".to_string(),
            (Self::TrashAborted, En) => "moving to the Recycle Bin was canceled".to_string(),
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(Lang::Ja)
    }
}

impl std::fmt::Display for OsIntegrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for OsIntegrationError {}

/// `ShellExecuteW` の失敗の説明（shellapi.h の `SE_ERR_*` と Win32 エラーが混在する）。
/// (戻り値, 言語) の網羅 match で、日本語は #2365 より前の固定文とバイト一致
pub fn shell_execute_reason(rc: isize, lang: Lang) -> String {
    use Lang::{En, Ja};
    let text = match (rc, lang) {
        (0, Ja) => "メモリまたはリソース不足",
        (0, En) => "out of memory or resources",
        (2, Ja) => "ファイルが見つからない",
        (2, En) => "file not found",
        (3, Ja) => "パスが見つからない",
        (3, En) => "path not found",
        (5, Ja) => "アクセスが拒否された",
        (5, En) => "access denied",
        (8, Ja) => "メモリ不足",
        (8, En) => "out of memory",
        (26, Ja) => "共有違反",
        (26, En) => "sharing violation",
        (27, Ja) => "ファイルの関連付けが不完全または無効",
        (27, En) => "the file association is incomplete or invalid",
        (28, Ja) => "タイムアウト",
        (28, En) => "timed out",
        (29, Ja) => "DDE トランザクションに失敗",
        (29, En) => "the DDE transaction failed",
        (31, Ja) => "この種類のファイルに関連付けられたアプリが無い",
        (31, En) => "no app is associated with this type of file",
        (32, Ja) => "関連付けられた DLL が見つからない",
        (32, En) => "the associated DLL was not found",
        (other, Ja) => return format!("ShellExecuteW が {other} を返した"),
        (other, En) => return format!("ShellExecuteW returned {other}"),
    };
    text.to_string()
}

/// `SHFileOperationW` の戻り値の説明（Win32 エラーコードではなく shell 独自の `DE_*`）。
/// (戻り値, 言語) の網羅 match で、日本語は #2365 より前の固定文とバイト一致
pub fn shfileop_reason(code: i32, lang: Lang) -> String {
    use Lang::{En, Ja};
    let text = match (code, lang) {
        (0x71, Ja) => "同じファイルを指している",
        (0x71, En) => "the source and the destination are the same file",
        (0x74, Ja) => "ルートディレクトリは操作できない",
        (0x74, En) => "the root directory cannot be operated on",
        (0x75, Ja) => "操作が取り消された",
        (0x75, En) => "the operation was canceled",
        (0x78, Ja) => "アクセスが拒否された",
        (0x78, En) => "access denied",
        (0x79, Ja) => "パスが MAX_PATH（260 文字）を超えている",
        (0x79, En) => "the path exceeds MAX_PATH (260 characters)",
        (0x7C, Ja) => "パスが不正",
        (0x7C, En) => "invalid path",
        (0x402, Ja) => "パスが見つからない（または不正）",
        (0x402, En) => "path not found (or invalid)",
        (other, Ja) => return format!("SHFileOperationW が 0x{other:X} を返した"),
        (other, En) => return format!("SHFileOperationW returned 0x{other:X}"),
    };
    text.to_string()
}

/// OS のファイルマネージャ。**UI の表記を出し分けるための値**（#617）。
///
/// 呼び名は [`reveal`] が実際に起動するものと 1:1 でなければならない
/// （「Finder で表示」と書いてあるボタンがエクスプローラーを開く、の逆も困る）ので、
/// 判定は境界の内側に置き、表記側は値を受け取るだけにする。
/// 値で持ち回すのは **macOS 上から Windows 側の表記も検査できる**ようにするため
/// （`settings_sleep::Device` と同じ作法。#905）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileManager {
    /// macOS の Finder
    Finder,
    /// Windows のエクスプローラー
    Explorer,
}

/// この環境のファイルマネージャ。
///
/// tako が配布するのは macOS と Windows だけなので 2 値で足りる
/// （その他 unix では [`reveal`] 自体が未対応エラーを返すため、表記が使われる場面が無い）
pub fn file_manager() -> FileManager {
    if cfg!(windows) {
        FileManager::Explorer
    } else {
        FileManager::Finder
    }
}

/// ファイルマネージャ（Finder / エクスプローラー）で対象を選択表示する。
///
/// 「Finder で表示」の入口（ファイルツリー・ターミナルのパスの ⌘+右クリック・プレビューのペイン・
/// 設定画面・右パネルの添付・CLI `tako file reveal` / MCP `tako_file_op`）はすべて dispatch の
/// `FileOpKind::Reveal` からここへ来る（#2247）。macOS は**このプロセス（tako の GUI）から**
/// NSWorkspace で Finder を前面へ出す（子の `open -R` では Finder が前に出てこなかった = #2247）。
/// CLI は GUI へ頼むだけで、自分のプロセスからは呼ばない（GUI が居なければ他の操作と同じく
/// 「繋がらない」で断る）
pub fn reveal(path: &Path) -> Result<(), String> {
    reveal_typed(path).map_err(OsIntegrationError::into_ja)
}

/// [`reveal`] の型の口（#2365。dispatch が表示言語で断る）
pub fn reveal_typed(path: &Path) -> Result<(), OsIntegrationError> {
    imp::reveal(path)
}

/// 「Finder で表示」を修正前の子の `open -R` へ戻す A/B（#2247。`TAKO_2247_LEGACY=1`）
pub fn reveal_legacy_2247() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_2247_LEGACY").is_ok_and(|v| v == "1"))
}

/// [`reveal`] の検証の記録の 1 行の頭（`scripts/test-preview-reveal-2247.sh` と visual-test が数える目印）
pub const REVEAL_RECORD_MARK: &str = "activateFileViewerSelectingURLs:";

/// 検証の口（#2247）: `TAKO_REVEAL_RECORD=<ファイル>` があれば、macOS の [`reveal`] は
/// NSWorkspace へ渡す直前の URL を 1 行書くだけで Finder を開かない
/// （本物の Finder の窓をユーザーの画面に出さずに、入口ごとの到達を数える）
// macOS の実装だけが使う
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn reveal_record() -> Option<std::path::PathBuf> {
    std::env::var_os("TAKO_REVEAL_RECORD")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
}

/// 既定アプリで開く
pub fn open_default(path: &Path) -> Result<(), String> {
    open_default_typed(path).map_err(OsIntegrationError::into_ja)
}

/// [`open_default`] の型の口（#2365）
pub fn open_default_typed(path: &Path) -> Result<(), OsIntegrationError> {
    imp::open_default(path)
}

/// アプリ名（またはアプリのパス）を指定して開く
pub fn open_with(app: &str, path: &Path) -> Result<(), String> {
    open_with_typed(app, path).map_err(OsIntegrationError::into_ja)
}

/// [`open_with`] の型の口（#2365）
pub fn open_with_typed(app: &str, path: &Path) -> Result<(), OsIntegrationError> {
    imp::open_with(app, path)
}

/// 既定のテキストエディタで開く（macOS の `open -t` 相当）
pub fn open_in_text_editor(path: &Path) -> Result<(), String> {
    imp::open_in_text_editor(path).map_err(OsIntegrationError::into_ja)
}

/// URL を既定ブラウザ / ハンドラで開く（起動するだけで完了は待たない）。
/// `x-apple.systempreferences:` のような OS 固有スキームもここを通す。
///
/// **URL はどのプラットフォームでも「1 つの値」のまま OS へ渡る**（#1371）。
/// シェルのコマンドライン文字列へ連結しないので、`&` `|` `^` `%` を含む URL が
/// 途中で切れたり、残りが別コマンドとして走ったりしない。
///
/// **渡す前にスキームを検査する**（#1376）。OS のハンドラは URL でない文字列
/// （ローカルの実行ファイルパス・UNC パス・`file:` URL）も開くので、ここを素通りさせると
/// 第三者由来の PDF / 画面のリンク 1 クリックで任意のプログラムが起動しうる
pub fn open_url(url: &str) -> Result<(), String> {
    open_url_typed(url).map_err(OsIntegrationError::into_ja)
}

/// [`open_url`] の型の口（#2365）
pub fn open_url_typed(url: &str) -> Result<(), OsIntegrationError> {
    imp::open_url(guarded_url(url)?)
}

/// 境界 B8 のスキーム検査（#1376 の保険）。判定は
/// [`tako_core::url_guard::check_os_handler_url`] の 1 実装が正本で、ここは通すだけ。
///
/// **エラーへ URL そのものを載せない**。呼び出し元の URL は PDF / ペインの内容に
/// 相当し、エラー文は診断ログへ流れる（AGENTS.md の絶対ルール）
fn guarded_url(url: &str) -> Result<&str, OsIntegrationError> {
    tako_core::url_guard::check_os_handler_url(url).map_err(OsIntegrationError::UrlBlocked)
}

/// URL を開き、**ハンドラを起動できたかまで確かめる**。
/// 候補 URL を順に試して最初に成功したものを採る用途（FDA のシステム設定パネル）で使う。
/// 成功可否が要らない場合は [`open_url`] を使う（macOS では待たない分ブロックしない）。
///
/// 待つのは**ランチャ**（macOS の `open` コマンド / Windows の `ShellExecuteW`）までで、
/// **ハンドラ本体（ブラウザや設定アプリ）の終了は待たない**。呼び出し側が知りたいのは
/// 「この URL を開けるハンドラがあるか」だけなので、粒度はこれで揃っている
pub fn open_url_wait(url: &str) -> Result<(), String> {
    open_url_wait_typed(url).map_err(OsIntegrationError::into_ja)
}

/// [`open_url_wait`] の本体（#2365。検査は型の口の側で通す = `String` の口は委ねるだけ）
fn open_url_wait_typed(url: &str) -> Result<(), OsIntegrationError> {
    imp::open_url_wait(guarded_url(url)?)
}

/// Windows で URL を開くときに `ShellExecuteW` へ渡す引数（#1371 の正本）。
///
/// 値で持ち回すのは **macOS 上からも Windows 側の形を検査できる**ようにするため
/// （[`FileManager`] と同じ作法。#617 / #905）。Windows 実機が無くても
/// 「cmd.exe が経路に居ない」「URL が 1 つの値のまま渡る」を番犬で固定できる
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsUrlLaunch {
    /// `lpOperation`。`None` = 既定の動詞（無ければ `open` に落ちる）。
    /// プロトコルハンドラが `open` 以外を既定にしていても開けるようにこちらを使う
    pub verb: Option<&'static str>,
    /// `lpFile`。**URL そのもの**（加工も分割もしない）
    pub file: String,
    /// `lpParameters` = **常に `None`**。ここは生のコマンドライン文字列なので、
    /// URL を入れるとシェルのメタ文字が効く余地を作ってしまう
    pub parameters: Option<String>,
}

/// Windows で URL を開くときの `ShellExecuteW` 引数を組む（純粋関数）。
///
/// **`cmd /C start` は使わない**（#1371）。`std::process::Command` の Windows 実装は
/// 空白かタブを含まない引数を引用符で囲まない（`Quote::Auto`）ため、cmd.exe が
/// 引用符の外の `&` を**コマンド区切り**として解釈する。tako が検出する URL は
/// 構造的に空白を含まず `&` は URL の正規の文字なので、クエリ文字列つきのリンクは
/// 常にそこで切れ、残りが別コマンドとして実行される（画面 / PDF / Markdown の中身が
/// 第三者由来なら、クリック 1 回で任意コマンドが走る）。
///
/// `ShellExecuteW` はコマンドラインを組まず `lpFile` をそのままハンドラへ渡すので、
/// シェルが経路から消える = メタ文字を解釈する層が無くなる
pub fn windows_url_launch(url: &str) -> WindowsUrlLaunch {
    WindowsUrlLaunch {
        verb: None,
        file: url.to_string(),
        parameters: None,
    }
}

/// アプリケーションを**新しいプロセスとして**起動する（macOS の `open -n` 相当）。
/// 自動更新後の再起動（B14）が使う（断りは `UpdateRefusal::RestartFailed` が表示言語で包む = #2365）
pub fn open_new_instance_typed(app: &Path) -> Result<(), OsIntegrationError> {
    imp::open_new_instance(app)
}

/// 「このアプリで開く…」= OS のアプリ選択 UI を出し、選ばれたアプリでパスを開く。
///
/// **1 操作として境界に置く**。macOS は「アプリ選択ダイアログ → `open -a`」の 2 段だが、
/// Windows の対応物は `openas` verb（「このファイルを開く方法を選んでください」）で
/// **選択と起動が分かれていない**ため、`pick_application()` のような部品に割ると
/// Windows 側が表現できない。キャンセルはエラー（呼び出し側は無視してよい）。
/// 呼び手はファイルツリーの通知欄だけなので型の口だけを持つ（表示言語で引く = #2365）
pub fn open_with_dialog_typed(path: &Path) -> Result<(), OsIntegrationError> {
    imp::open_with_dialog(path)
}

/// パスをゴミ箱へ移す（**完全削除にはしない**）。
///
/// macOS は Finder に委ねる（ゴミ箱から戻せる = ユーザーの期待どおり）。
/// **パスは AppleScript ソースへ連結せず `osascript` の argv として渡す**ため、
/// ファイル名に含まれる `"` `\` 改行がスクリプト構文へ割り込む余地が構造的に無い
/// （#80。エスケープの正しさに依存しない）。
/// Windows は `SHFileOperationW` + `FOF_ALLOWUNDO` でごみ箱へ入れる（#617）。
///
/// ゴミ箱の概念が無い環境（その他 unix）では**削除せずエラーにする**。
/// UI にもコマンドにも確認ダイアログが無い操作なので、
/// 「ゴミ箱のつもりで完全削除」だけは起こさない。
///
/// 検証用に `TAKO_TRASH_DIR=<フォルダ>` を立てると、OS のゴミ箱の代わりにそのフォルダへ
/// 移す（#1867。複数選択のごみ箱を実 GUI・CLI・MCP で確かめるときに**ユーザーのゴミ箱へ
/// fixture を入れない**ため。#1860 の `TAKO_FILE_PASTEBOARD` と同じ作法）。本物のゴミ箱と同じく
/// 同名があれば番号を付けて入れる（上書きしない）
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    move_to_trash_typed(path).map_err(OsIntegrationError::into_ja)
}

/// [`move_to_trash`] の型の口（#2365）
pub fn move_to_trash_typed(path: &Path) -> Result<(), OsIntegrationError> {
    if let Some(dir) = std::env::var_os("TAKO_TRASH_DIR").filter(|v| !v.is_empty()) {
        return move_into_test_trash(Path::new(&dir), path);
    }
    imp::move_to_trash(path)
}

/// `TAKO_TRASH_DIR` の行き先へ移す（同名があれば「名前 2」「名前 3」… = 上書きしない）
fn move_into_test_trash(dir: &Path, path: &Path) -> Result<(), OsIntegrationError> {
    let name = path
        .file_name()
        .ok_or_else(|| OsIntegrationError::TrashNoName(path.display().to_string()))?;
    let mut to = dir.join(name);
    let mut n = 2;
    while std::fs::symlink_metadata(&to).is_ok() {
        to = dir.join(format!("{} {n}", name.to_string_lossy()));
        n += 1;
    }
    std::fs::rename(path, &to).map_err(|e| OsIntegrationError::TrashFailed(e.to_string()))
}

/// デスクトップ通知を出す（best-effort）。
/// メッセージは argv で渡すため本文の内容がスクリプト構文へ割り込まない。
///
/// 戻り値は**通知を出せたか**。Windows は通知の実装が無く常に `false` を返す（#617）。
/// 黙って消える経路を作らないため、呼び出し側が代替表示へ倒せるようにしてある
/// （本格的なトースト通知 = WinRT `ToastNotification` は別 Issue）
pub fn notify(title: &str, message: &str) -> bool {
    imp::notify(title, message)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    pub fn reveal(path: &Path) -> Result<(), OsIntegrationError> {
        if super::reveal_legacy_2247() {
            // 修正前の形（#2247 の A/B）: 子の `open -R`。成功しても Finder が前に出てこなかった
            return spawn_open(&["-R".as_ref(), path.as_os_str()], Launch::Finder);
        }
        crate::platform::finder_reveal::reveal(path, super::reveal_record().as_deref())
    }

    pub fn open_default(path: &Path) -> Result<(), OsIntegrationError> {
        spawn_open(&[path.as_os_str()], Launch::DefaultApp)
    }

    pub fn open_with(app: &str, path: &Path) -> Result<(), OsIntegrationError> {
        spawn_open(
            &["-a".as_ref(), app.as_ref(), path.as_os_str()],
            Launch::App(app.to_string()),
        )
    }

    pub fn open_in_text_editor(path: &Path) -> Result<(), OsIntegrationError> {
        spawn_open(&["-t".as_ref(), path.as_os_str()], Launch::TextEditor)
    }

    pub fn open_url(url: &str) -> Result<(), OsIntegrationError> {
        spawn_open(&[url.as_ref()], Launch::Url)
    }

    pub fn open_url_wait(url: &str) -> Result<(), OsIntegrationError> {
        let status = std::process::Command::new("open")
            .arg(url)
            .status()
            .map_err(|e| OsIntegrationError::OpenCommandFailed(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(OsIntegrationError::UrlExitStatus {
                status: status.to_string(),
                url: url.to_string(),
            })
        }
    }

    pub fn open_new_instance(app: &Path) -> Result<(), OsIntegrationError> {
        spawn_open(&["-n".as_ref(), app.as_os_str()], Launch::NewInstance)
    }

    /// macOS は「選択」と「起動」が別操作なので、境界の内側で 2 段を組む
    pub fn open_with_dialog(path: &Path) -> Result<(), OsIntegrationError> {
        let app = pick_application()?;
        open_with(&app.to_string_lossy(), path)
    }

    fn pick_application() -> Result<std::path::PathBuf, OsIntegrationError> {
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose application as alias)")
            .output()
            .map_err(|e| OsIntegrationError::OsascriptSpawn(e.to_string()))?;
        if !output.status.success() {
            return Err(OsIntegrationError::AppPickCanceled);
        }
        let app_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if app_path.is_empty() {
            return Err(OsIntegrationError::AppPathEmpty);
        }
        Ok(std::path::PathBuf::from(app_path))
    }

    pub fn move_to_trash(path: &Path) -> Result<(), OsIntegrationError> {
        // argv 経由でパスを受け取るため、スクリプト本体にパスは一切現れない
        const SCRIPT: &str = "on run argv\n\
            tell application \"Finder\" to delete (POSIX file (item 1 of argv) as alias)\n\
            end run";
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(SCRIPT)
            .arg(path)
            .output()
            .map_err(|e| OsIntegrationError::TrashFailed(e.to_string()))?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(OsIntegrationError::TrashFailed(msg.into_owned()));
        }
        Ok(())
    }

    pub fn notify(title: &str, message: &str) -> bool {
        use std::process::Stdio;
        // 本文・タイトルとも argv 渡しにして injection を避ける
        let script = "on run argv\n\
            display notification (item 1 of argv) with title (item 2 of argv)\n\
            end run";
        std::process::Command::new("osascript")
            .args(["-e", script, message, title])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }

    fn spawn_open(args: &[&std::ffi::OsStr], what: Launch) -> Result<(), OsIntegrationError> {
        std::process::Command::new("open")
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|e| OsIntegrationError::Spawn {
                what,
                error: e.to_string(),
            })
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;

    // --- Win32 FFI --------------------------------------------------------
    // `windows-sys` を足さず、使う関数と構造体だけを宣言する（モジュール doc 参照）

    /// `SHFILEOPSTRUCTW`（shellapi.h）。
    ///
    /// shellapi.h はこの構造体を **32bit のときだけ** `pshpack1.h` で 1 バイト詰めにする
    /// （`windows-sys` も `target_arch = "x86"` にだけ `packed(1)` を付けている）。
    /// tako の Windows 配布は x64 だが、レイアウトを取り違えると引数が丸ごとずれるので
    /// 両方を宣言しておく
    #[repr(C)]
    #[cfg_attr(target_arch = "x86", repr(packed(1)))]
    struct ShFileOpStructW {
        hwnd: isize,
        w_func: u32,
        /// 二重 NUL 終端の UTF-16 文字列（複数パスを連結できる形式）
        p_from: *const u16,
        p_to: *const u16,
        /// `FILEOP_FLAGS` = `WORD`
        f_flags: u16,
        f_any_operations_aborted: i32,
        h_name_mappings: *mut core::ffi::c_void,
        lpsz_progress_title: *const u16,
    }

    /// `FO_DELETE`（shellapi.h）
    const FO_DELETE: u32 = 0x0003;
    /// 進捗ダイアログを出さない
    const FOF_SILENT: u16 = 0x0004;
    /// 「削除しますか？」を出さない（macOS の Finder 委譲と同じく、UI 側に確認が無い操作なので揃える）
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    /// **これが無いとごみ箱ではなく完全削除になる**
    const FOF_ALLOWUNDO: u16 = 0x0040;
    /// エラーダイアログを出さない（戻り値でエラーを返すのでこちらが正）
    const FOF_NOERRORUI: u16 = 0x0400;
    /// ごみ箱に入れられない対象（容量超過・ごみ箱無効のドライブ等）のとき、
    /// `FOF_NOCONFIRMATION` を**部分的に打ち消して**「完全に削除しますか？」を出させる。
    /// 「ごみ箱に移動」と表示しておいて黙って完全削除する事故を防ぐための保険
    const FOF_WANTNUKEWARNING: u16 = 0x4000;

    /// `SW_SHOWNORMAL`
    const SW_SHOWNORMAL: i32 = 1;
    /// `ShellExecuteW` は「32 より大きければ成功」（返るのは歴史的経緯で HINSTANCE）
    const SHELL_EXECUTE_SUCCESS_MIN: isize = 32;

    /// `COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE`
    const COINIT_STA: u32 = 0x2 | 0x4;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHFileOperationW(op: *mut ShFileOpStructW) -> i32;
        fn ShellExecuteW(
            hwnd: isize,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> isize;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut core::ffi::c_void, co_init: u32) -> i32;
        fn CoUninitialize();
    }

    /// シェル API（`ShellExecuteW` / `SHFileOperationW`）はシェル拡張へ処理を委ねるため、
    /// 呼び出しスレッドで COM が初期化されている必要がある。tako は IPC / MCP の
    /// ワーカースレッドからもファイル操作を dispatch するので、未初期化でも動くように
    /// ここで STA を張り、**張った回数ちょうど**だけ解放する
    struct ComGuard {
        need_uninit: bool,
    }

    impl ComGuard {
        fn new() -> Self {
            // SAFETY: reserved は仕様どおり NULL。戻り値だけで釣り合わせを決める
            let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_STA) };
            // S_OK(0) / S_FALSE(1) はこのスレッドの参照カウントが増えている。
            // RPC_E_CHANGED_MODE（既に MTA。負値）は増えていないので触らない
            Self {
                need_uninit: hr >= 0,
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.need_uninit {
                // SAFETY: CoInitializeEx が成功した回数と 1:1 で呼ぶ
                unsafe { CoUninitialize() };
            }
        }
    }

    /// UTF-16 + NUL 終端。途中に NUL があると Win32 側でそこまでしか読まれず
    /// **別のパスを指してしまう**ので、切り詰めずエラーにする
    fn wide(s: &OsStr) -> Result<Vec<u16>, OsIntegrationError> {
        let mut buf: Vec<u16> = s.encode_wide().collect();
        if buf.contains(&0) {
            return Err(OsIntegrationError::PathHasNul);
        }
        buf.push(0);
        Ok(buf)
    }

    /// 相対パスを絶対パスへ直す。
    ///
    /// `std::fs::canonicalize` は使わない。返るのが `\\?\C:\…`（verbatim 形式）で、
    /// explorer も `SHFileOperationW` もこの形式を解釈できないため。
    /// `std::path::absolute` は `.` / `..` の正規化までで済み、シェルが読める形を保つ
    pub(super) fn absolute(path: &Path) -> Result<std::path::PathBuf, OsIntegrationError> {
        std::path::absolute(path).map_err(|e| OsIntegrationError::Absolutize(e.to_string()))
    }

    /// `ShellExecuteW` の薄いラッパー。`verb` が `None` なら既定の動詞
    fn shell_execute(
        verb: Option<&str>,
        file: &OsStr,
        parameters: Option<&OsStr>,
        what: Launch,
    ) -> Result<(), OsIntegrationError> {
        let verb_w = verb.map(|v| wide(OsStr::new(v))).transpose()?;
        let file_w = wide(file)?;
        let params_w = parameters.map(wide).transpose()?;
        let _com = ComGuard::new();
        // SAFETY: 渡すポインタはいずれもこの呼び出しの間だけ生きていればよく、
        // 元のバッファ（verb_w / file_w / params_w）はこのスコープで保持している。
        // 省略可能な引数は仕様どおり NULL を渡す
        let rc = unsafe {
            ShellExecuteW(
                0,
                verb_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                file_w.as_ptr(),
                params_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if rc > SHELL_EXECUTE_SUCCESS_MIN {
            Ok(())
        } else {
            // 戻り値の説明（`SE_ERR_*` と Win32 エラー）は cfg の外の `shell_execute_reason`
            Err(OsIntegrationError::ShellExecute { what, code: rc })
        }
    }

    /// `explorer.exe` に渡すコマンドライン。
    ///
    /// **explorer は `CommandLineToArgvW` で引数を解釈しない**。生のコマンドラインを
    /// 自前で読み、先頭が `/select,` かどうかを見る。したがって
    ///
    /// - `/select,` とパスの間に空白を入れてはいけない（別の引数として捨てられる）
    /// - **引数全体を引用符でくくってもいけない**。`"/select,C:\a b\c.txt"` は
    ///   スイッチとして認識されず、既定フォルダ（ドキュメント）が開くだけになる
    ///   （実測: `Command::arg` の自動クォートで空白入りパスを渡すとこれが起きた）
    ///
    /// 正しい形は **パスだけを引用符でくくる** `/select,"C:\a b\c.txt"`。
    /// `raw_arg` で組み立てるため、引数の切れ目を壊しうる `"` を含むパスは拒否する
    /// （Windows のファイル名に `"` は使えないので、実在のパスを取りこぼすことはない）
    pub(super) fn select_argument(abs: &Path) -> Result<OsString, OsIntegrationError> {
        if abs.as_os_str().encode_wide().any(|c| c == u16::from(b'"')) {
            return Err(OsIntegrationError::PathHasQuote);
        }
        let mut arg = OsString::from("/select,\"");
        arg.push(abs.as_os_str());
        arg.push("\"");
        Ok(arg)
    }

    pub fn reveal(path: &Path) -> Result<(), OsIntegrationError> {
        let abs = absolute(path)?;
        // 存在しないパスを渡すと explorer は**既定フォルダ（ドキュメント）を開いて**しまう。
        // 「押したら関係ない窓が出た」より「開けなかった」と言う方が親切なので先に弾く
        if std::fs::symlink_metadata(&abs).is_err() {
            return Err(OsIntegrationError::PathMissing(abs.display().to_string()));
        }
        let mut cmd = std::process::Command::new("explorer.exe");
        // 自動クォートを避けるため raw_arg で渡す（上の doc の理由）
        cmd.raw_arg(select_argument(&abs)?);
        // explorer.exe は **成功しても終了コード 1 を返す**ので status は見ない
        // （待っても成否の判定に使えない。開けたかどうかは画面が正）
        tako_core::platform::process::no_console_window(&mut cmd)
            .spawn()
            .map(|_| ())
            .map_err(|e| OsIntegrationError::Spawn {
                what: Launch::Explorer,
                error: e.to_string(),
            })
    }

    pub fn open_default(path: &Path) -> Result<(), OsIntegrationError> {
        let abs = absolute(path)?;
        shell_execute(Some("open"), abs.as_os_str(), None, Launch::DefaultApp)
    }

    pub fn open_with(app: &str, path: &Path) -> Result<(), OsIntegrationError> {
        let abs = absolute(path)?;
        shell_execute(
            Some("open"),
            OsStr::new(app),
            Some(&quoted_argument(abs.as_os_str())),
            Launch::App(app.to_string()),
        )
    }

    /// Windows の「このアプリで開く…」は `openas` verb そのもの。
    /// 選択ダイアログの表示とアプリ起動を OS が 1 操作で行う
    pub fn open_with_dialog(path: &Path) -> Result<(), OsIntegrationError> {
        let abs = absolute(path)?;
        shell_execute(Some("openas"), abs.as_os_str(), None, Launch::AppPicker)
    }

    /// Windows に「既定のテキストエディタ」という概念（macOS の `open -t`）は無い。
    /// 拡張子の関連付け（`open_default`）だとテキストエディタで開くとは限らないので、
    /// 必ずテキストとして開ける メモ帳 を使う
    pub fn open_in_text_editor(path: &Path) -> Result<(), OsIntegrationError> {
        let abs = absolute(path)?;
        shell_execute(
            Some("open"),
            OsStr::new("notepad.exe"),
            Some(&quoted_argument(abs.as_os_str())),
            Launch::TextEditor,
        )
    }

    /// `ShellExecuteW` の `lpParameters` は**生のコマンドライン文字列**なので、
    /// 空白を含むパスは引用符でくくる。Windows のパスに `"` は使えないため、
    /// くくるだけで引数の切れ目が壊れない
    fn quoted_argument(path: &OsStr) -> OsString {
        let mut quoted = OsString::from("\"");
        quoted.push(path);
        quoted.push("\"");
        quoted
    }

    /// URL も `ShellExecuteW` で開く（#1371）。引数の組み方は境界の外に置いた
    /// [`windows_url_launch`] が正本で、macOS からも同じ形を検査できる
    pub fn open_url(url: &str) -> Result<(), OsIntegrationError> {
        let launch = windows_url_launch(url);
        shell_execute(
            launch.verb,
            OsStr::new(&launch.file),
            launch.parameters.as_deref().map(OsStr::new),
            Launch::Url,
        )
    }

    /// `ShellExecuteW` は**ハンドラを起動できたかを戻り値で返す**（32 より大きければ成功）
    /// ので、待つ版も同じ呼び出しで足りる。macOS の `open` コマンドもハンドラへ渡した
    /// 時点で終了するため、呼び出し側（FDA の候補 URL 総当たり）が見る意味は変わらない。
    ///
    /// **`ShellExecuteExW` + `SEE_MASK_NOCLOSEPROCESS` は使わない**。返るのはハンドラ本体
    /// （ブラウザ / 設定アプリ）のハンドルなので、待つとユーザーがそれを閉じるまで返らない
    pub fn open_url_wait(url: &str) -> Result<(), OsIntegrationError> {
        open_url(url)
    }

    pub fn open_new_instance(_app: &Path) -> Result<(), OsIntegrationError> {
        // 自動更新（B14）の Windows 経路はインストーラー起動 → 自プロセス終了なので
        // 「同じアプリをもう 1 つ起こす」対応物が無い（#528）
        Err(OsIntegrationError::Unsupported(OsFeature::NewInstance))
    }

    pub fn move_to_trash(path: &Path) -> Result<(), OsIntegrationError> {
        // FOF_ALLOWUNDO は**絶対パスでないとごみ箱へ入らず完全削除になる**（MSDN 明記）
        let abs = absolute(path)?;
        // 壊れたシンボリックリンクも対象にしたいので exists() ではなくメタデータで見る。
        // 存在しないパスに対する SHFileOperationW の戻り値に依存しないためでもある
        if std::fs::symlink_metadata(&abs).is_err() {
            return Err(OsIntegrationError::PathMissing(abs.display().to_string()));
        }
        // pFrom は二重 NUL 終端（複数パスを並べられる形式。1 件でも終端は 2 つ要る）
        let mut from = wide(abs.as_os_str())?;
        from.push(0);

        let mut op = ShFileOpStructW {
            hwnd: 0,
            w_func: FO_DELETE,
            p_from: from.as_ptr(),
            p_to: std::ptr::null(),
            f_flags: trash_flags(),
            f_any_operations_aborted: 0,
            h_name_mappings: std::ptr::null_mut(),
            lpsz_progress_title: std::ptr::null(),
        };
        let _com = ComGuard::new();
        // SAFETY: op は呼び出しの間スタック上に生きており、p_from が指す `from` も同様。
        // 使わないポインタ（p_to / hNameMappings / lpszProgressTitle）は仕様どおり NULL
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 {
            // 戻り値の説明（shell 独自の `DE_*`）は cfg の外の `shfileop_reason`
            return Err(OsIntegrationError::TrashShellOp(code));
        }
        // 「完全に削除しますか？」をユーザーが断ったときはここに落ちる（戻り値は 0）。
        // 消えていないので成功と報告してはいけない
        if op.f_any_operations_aborted != 0 {
            return Err(OsIntegrationError::TrashAborted);
        }
        Ok(())
    }

    /// ごみ箱移動のフラグ。**`FOF_ALLOWUNDO` が抜けたら完全削除になる**ので
    /// テストから固定できるよう関数に切り出してある
    pub(super) const fn trash_flags() -> u16 {
        FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_SILENT | FOF_NOERRORUI
    }

    /// Windows のデスクトップ通知は未実装（#617 で明示的に見送り）。
    /// 本文（リモート接続のデバイス名等）はログにも出さない
    pub fn notify(_title: &str, _message: &str) -> bool {
        tracing::debug!("OS 通知は Windows 未対応のため送信していない");
        false
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::*;

    pub fn reveal(_path: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::Reveal))
    }

    pub fn open_default(_path: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::OpenDefault))
    }

    pub fn open_with(_app: &str, _path: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::OpenWith))
    }

    pub fn open_with_dialog(_path: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::AppPicker))
    }

    pub fn open_in_text_editor(_path: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::TextEditor))
    }

    /// URL だけは非 macOS でも従来から動いていた（`open_preview` の cfg 分岐）。
    /// その挙動をそのままここへ引き取る
    pub fn open_url(url: &str) -> Result<(), OsIntegrationError> {
        url_command(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| OsIntegrationError::Spawn {
                what: Launch::Url,
                error: e.to_string(),
            })
    }

    pub fn open_url_wait(url: &str) -> Result<(), OsIntegrationError> {
        let status = url_command(url)
            .status()
            .map_err(|e| OsIntegrationError::UrlHandlerFailed(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(OsIntegrationError::UrlExitStatus {
                status: status.to_string(),
                url: url.to_string(),
            })
        }
    }

    fn url_command(url: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }

    pub fn open_new_instance(_app: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::NewInstance))
    }

    /// **削除へ劣化させない**。ゴミ箱のつもりで完全削除するくらいなら失敗した方がよい
    pub fn move_to_trash(_path: &Path) -> Result<(), OsIntegrationError> {
        Err(OsIntegrationError::Unsupported(OsFeature::Trash))
    }

    pub fn notify(_title: &str, _message: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ゴミ箱操作は「存在しないパス」を渡しても panic せずエラーで返る
    /// （dispatch 側は事前に存在チェックしているが、境界単体でも安全であること）
    #[test]
    fn 存在しないパスのゴミ箱移動はエラーになる() {
        let missing = std::env::temp_dir().join(format!("tako-no-such-{}", std::process::id()));
        assert!(move_to_trash(&missing).is_err());
    }

    /// `move_to_trash` の argv 渡しがインジェクションされないことを、Finder を使わず
    /// osascript の argv 挙動そのもので検証する（CI の macOS ランナーで決定的に通る）。
    /// 悪意ある文字列を argv item 1 に渡しても、AppleScript の構文（`do shell script`）
    /// として解釈されず、単なるデータとして扱われることを確認する。
    #[cfg(target_os = "macos")]
    #[test]
    fn trash_argvは悪意ある文字列をデータとして扱う() {
        // インジェクションが成功すると作られてしまう副作用ファイル（cwd 相対 = パスに / を含めない）
        let marker = std::env::temp_dir().join(format!("tako_trash_pwned_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let marker_str = marker.display().to_string();
        // " で文字列を閉じ do shell script を差し込もうとする典型的なインジェクション文字列
        let evil = format!("x\" do shell script \"touch {marker_str}\" ignoring \"");

        // move_to_trash と同じ argv 渡し方式（Finder 部分だけ「argv をそのまま返す」に差し替え）
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg("on run argv\nreturn item 1 of argv\nend run")
            .arg(&evil)
            .output()
            .expect("osascript の実行に失敗");
        assert!(out.status.success(), "osascript が失敗: {out:?}");

        // データとしてそのまま返る = スクリプト構文に割り込んでいない
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("do shell script"),
            "argv がデータとして扱われていない: {stdout:?}"
        );
        // 副作用ファイルが作られていない = インジェクション不成立
        assert!(
            !marker.exists(),
            "AppleScript インジェクションで副作用ファイルが作られた: {marker_str}"
        );
        let _ = std::fs::remove_file(&marker);
    }

    /// 実ファイルの e2e: 改行・引用符・バックスラッシュを含む悪意あるファイル名でも
    /// 安全にゴミ箱へ移動でき、かつインジェクションの副作用が起きないこと。
    /// 実際に Finder を操作しゴミ箱へ移すため、GUI セッションのある手元で明示実行する。
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Finder を操作しファイルをゴミ箱へ移すため手動確認用（cargo test -- --ignored）"]
    fn ゴミ箱移動は悪意あるファイル名を安全に扱う() {
        // 置き場も目印も一意な dir の中（#1666。固定名の目印は並行する別の実行と取り合う）
        let dir = tako_core::test_residue::ScratchDir::new("trash-e2e");
        let marker = dir.join("pwned");

        // 改行 / " / \ / do shell script を含むファイル名（/ と NUL 以外は macOS で合法）
        let evil_name = "ev\"il\n `do shell script` \\ .txt";
        let evil = dir.join(evil_name);
        std::fs::write(&evil, b"x").unwrap();
        assert!(evil.exists(), "テストファイルが作れていない");

        move_to_trash(&evil).expect("ゴミ箱への移動に失敗");

        assert!(!evil.exists(), "ファイルが削除されていない");
        assert!(!marker.exists(), "インジェクションの副作用が発生した");
    }

    /// 通知は「出せたか」を返す（#617）。出せない環境で黙って消えるのを防ぐため、
    /// 呼び出し側が代替表示へ倒せる形を型で固定する。
    /// 実際に通知を出すと `cargo test` のたびに通知センターへ積まれるので呼ばない
    #[test]
    fn 通知は成否を呼び出し側へ返す() {
        fn assert_returns_bool(_: fn(&str, &str) -> bool) {}
        assert_returns_bool(notify);
    }

    /// ファイルマネージャの呼び名（#617）は **`reveal` が実際に起動するもの**と
    /// 一致していなければならない。UI のラベルはこの値だけを見て決まるので、
    /// ここが逆になると「エクスプローラーで表示」が Finder を開く（逆も同様）
    #[test]
    fn ファイルマネージャの呼び名は実装と一致する() {
        let expected = if cfg!(windows) {
            FileManager::Explorer
        } else {
            FileManager::Finder
        };
        assert_eq!(file_manager(), expected);
    }

    /// Windows のごみ箱移動（#617）。実際にごみ箱へ入れる e2e は `#[ignore]`（macOS 側と同じ方針）
    #[cfg(windows)]
    mod windows {
        use super::super::*;

        /// `SHQUERYRBINFO`（shellapi.h）。x86 のみ 1 バイト詰め（`windows-sys` と同じ）
        #[repr(C)]
        #[cfg_attr(target_arch = "x86", repr(packed(1)))]
        struct ShQueryRbInfo {
            cb_size: u32,
            i64_size: i64,
            i64_num_items: i64,
        }

        #[link(name = "shell32")]
        unsafe extern "system" {
            fn SHQueryRecycleBinW(root: *const u16, info: *mut ShQueryRbInfo) -> i32;
        }

        /// ごみ箱に入っている項目数（全ドライブ）。**「消えた」と「ごみ箱へ入った」を
        /// 区別する**ために使う。`remove_file` に戻したらここが増えないので落ちる
        fn recycle_bin_items() -> i64 {
            let mut info = ShQueryRbInfo {
                cb_size: std::mem::size_of::<ShQueryRbInfo>() as u32,
                i64_size: 0,
                i64_num_items: 0,
            };
            // SAFETY: root は NULL（= 全ドライブ）。info は呼び出しの間スタック上に生きている
            let hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
            assert_eq!(hr, 0, "SHQueryRecycleBinW が失敗した: 0x{hr:X}");
            info.i64_num_items
        }

        /// **最重要の不変条件**: `FOF_ALLOWUNDO` が落ちると完全削除になり復元できない。
        /// フラグの組み合わせを数値で固定して、後から誰かが外したら落ちるようにする
        #[test]
        fn ごみ箱移動のフラグにallowundoが必ず入る() {
            const FOF_ALLOWUNDO: u16 = 0x0040;
            const FOF_WANTNUKEWARNING: u16 = 0x4000;
            let flags = imp::trash_flags();
            assert_eq!(
                flags & FOF_ALLOWUNDO,
                FOF_ALLOWUNDO,
                "FOF_ALLOWUNDO が無い = 完全削除になる"
            );
            // ごみ箱に入れられない対象で黙って完全削除しないための保険も外させない
            assert_eq!(flags & FOF_WANTNUKEWARNING, FOF_WANTNUKEWARNING);
            // 0x4454 = ALLOWUNDO | NOCONFIRMATION | WANTNUKEWARNING | SILENT | NOERRORUI
            assert_eq!(flags, 0x4454, "フラグ構成を変えたら意図を確認すること");
        }

        /// `FOF_ALLOWUNDO` は**絶対パスでないと効かない**（相対パスだと完全削除に落ちる）。
        /// かつ `\\?\` 形式（`canonicalize` の戻り）はシェル API が解釈できない。
        /// 境界の絶対化がこの 2 条件を満たすことを固定する
        #[test]
        fn 絶対化はverbatim形式にならない() {
            let cwd = std::env::current_dir().unwrap();
            let abs = imp::absolute(Path::new("Cargo.toml")).unwrap();
            assert!(abs.is_absolute(), "相対パスのままだと完全削除になる");
            assert_eq!(abs, cwd.join("Cargo.toml"));
            assert!(
                !abs.to_string_lossy().starts_with(r"\\?\"),
                "verbatim 形式は explorer / SHFileOperationW が解釈できない: {abs:?}"
            );
            // 既に絶対パスなら素通し（`..` は正規化される）
            let dotted = cwd.join("src").join("..").join("Cargo.toml");
            assert_eq!(imp::absolute(&dotted).unwrap(), cwd.join("Cargo.toml"));
        }

        /// reveal のコマンドラインは `/select,"<パス>"`。
        ///
        /// **カンマの直後にパスを置く**（空白を挟むと explorer がスイッチを見失う）ことと、
        /// **引用符でパスだけを囲む**（引数全体を囲むと選択されず既定フォルダが開く。実測）
        /// ことが同時に成り立つこと。空白・日本語でも形が崩れない
        #[test]
        fn revealの引数はパスだけを引用符で囲む() {
            for name in ["a.txt", "スペース あり.txt", "日本語のファイル.md"] {
                let path = std::path::PathBuf::from(r"C:\Users\test\Desktop").join(name);
                let arg = imp::select_argument(&path).expect("正常なパスでは組める");
                let s = arg.to_string_lossy();
                assert_eq!(s, format!("/select,\"{}\"", path.to_string_lossy()));
                assert!(!s.starts_with('"'), "引数全体を囲むと選択されない: {s}");
            }
        }

        /// `raw_arg` で渡すので、引数の切れ目を壊しうる `"` を含むパスは拒否する
        #[test]
        fn 引用符を含むパスは拒否する() {
            let evil = std::path::PathBuf::from("C:\\tmp\\a\" /select,\"C:\\Windows");
            assert!(imp::select_argument(&evil).is_err());
        }

        /// 存在しないパスは（SHFileOperationW の戻り値に頼らず）事前にエラーで返す
        #[test]
        fn 存在しないパスは事前にエラーになる() {
            let missing =
                std::env::temp_dir().join(format!("tako-trash-missing-{}", std::process::id()));
            let err = move_to_trash(&missing).unwrap_err();
            assert!(err.contains("存在しない"), "{err}");
        }

        /// 実 UI の e2e: エクスプローラーが開き、対象が選択される。
        /// ウィンドウを開くので手元で明示実行する（開いた窓は目視で確認して閉じる）
        #[test]
        #[ignore = "エクスプローラーの窓を開くため手動確認用（cargo test -- --ignored）"]
        fn revealの実e2e() {
            let dir = std::env::temp_dir().join(format!("tako_reveal_e2e_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("スペース あり.txt");
            std::fs::write(&file, b"tako").unwrap();
            reveal(&file).expect("エクスプローラーを開けない");
            // 窓が出るまで待つ。**ここでディレクトリを消さない**
            // （消すとエクスプローラーが窓を畳んでしまい、選択状態を確認できない）
            std::thread::sleep(std::time::Duration::from_secs(3));
            println!("選択されているはずのファイル: {}", file.display());
        }

        /// 実 UI の e2e: 既定アプリが起動する。アプリが立ち上がるので手元で明示実行する。
        ///
        /// **ディレクトリ**（ハンドラが必ず存在する = エクスプローラー）と**ファイル**の
        /// 両方を開く。ファイル側のハンドラは環境依存（拡張子の関連付け次第）なので、
        /// 経路そのものが生きているかはディレクトリ側で判定できるようにしてある
        #[test]
        #[ignore = "既定アプリを起動するため手動確認用（cargo test -- --ignored）"]
        fn デフォルトアプリで開く実e2e() {
            let dir = std::env::temp_dir().join(format!("tako_open_e2e_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("開く テスト.txt");
            std::fs::write(&file, "tako open default\n").unwrap();

            open_default(&dir).expect("ディレクトリを既定アプリで開けない");
            open_default(&file).expect("ファイルを既定アプリで開けない");
            std::thread::sleep(std::time::Duration::from_secs(3));
            // アプリが開いたままなので消さない（呼び出し側で後始末する）
            println!("開いたディレクトリ: {}", dir.display());
            println!("開いたファイル: {}", file.display());
        }

        /// 実ファイルの e2e: スクラッチファイルが**ごみ箱へ入る**（消えるだけでなく復元可能）。
        /// ごみ箱に実物を積むので手元で明示実行する（`cargo test -- --ignored`）。
        /// ごみ箱に入ったことの確認は PowerShell の Shell.Application で行う（#617 の証拠）
        #[test]
        #[ignore = "ごみ箱に実ファイルを積むため手動確認用（cargo test -- --ignored）"]
        fn ごみ箱移動の実e2e() {
            let dir = std::env::temp_dir().join(format!("tako_trash_e2e_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();

            // 空白・日本語つきのファイル / 読み取り専用ファイル / ディレクトリの 3 種
            let file = dir.join("スペース あり.txt");
            std::fs::write(&file, b"tako").unwrap();
            let readonly = dir.join("readonly.txt");
            std::fs::write(&readonly, b"tako").unwrap();
            let mut perm = std::fs::metadata(&readonly).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(&readonly, perm).unwrap();
            let subdir = dir.join("サブフォルダ");
            std::fs::create_dir_all(subdir.join("nested")).unwrap();
            std::fs::write(subdir.join("nested").join("a.txt"), b"tako").unwrap();

            let before = recycle_bin_items();
            let targets = [&file, &readonly, &subdir];
            for target in targets {
                move_to_trash(target).unwrap_or_else(|e| panic!("{}: {e}", target.display()));
                assert!(!target.exists(), "消えていない: {}", target.display());
            }
            // **完全削除ではなくごみ箱へ入った**ことを API で確かめる（目視に頼らない）。
            // 他のアプリが同時にごみ箱へ入れることがあるので下限で見る
            let after = recycle_bin_items();
            println!("ごみ箱の項目数: {before} -> {after}");
            assert!(
                after >= before + targets.len() as i64,
                "ごみ箱に入っていない（完全削除された疑い）: {before} -> {after}"
            );

            // MAX_PATH（260 文字）超え。SHFileOperationW は長いパスに対応しないので
            // **成功しても失敗しても「黙って完全削除」にならない**ことを確認する
            let mut deep = dir.clone();
            while deep.as_os_str().len() < 300 {
                deep.push("長い名前のフォルダ");
            }
            std::fs::create_dir_all(&deep).unwrap();
            let long_file = deep.join("long.txt");
            std::fs::write(&long_file, b"tako").unwrap();
            let long_result = move_to_trash(&long_file);
            println!("長いパス（{} 文字）の結果: {long_result:?}", {
                long_file.as_os_str().len()
            });
            if long_result.is_err() {
                assert!(long_file.exists(), "失敗したのにファイルが消えている");
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
