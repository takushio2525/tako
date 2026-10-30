//! アプリ内自動更新チェッカー
//!
//! GitHub Releases から安定版（stable）とテスト版（test = prerelease）の 2 チャンネルを
//! チェックし、ステータスバーにドロップダウンで通知する。配布系統（Homebrew / zip）を
//! 自動判別し、系統内で更新を実行する。更新完了後は自動再起動する。
//!
//! チャンネル制（#403）: GitHub Releases の prerelease フラグで stable / test を判別。
//! タグ例: stable = `v0.6.0`、test = `v0.6.0-test.1`。
//!
//! broken-brew 検知（#50）: brew upgrade 失敗等で「.app 実体あり・cask 台帳なし」の
//! 詰み状態を検知し、修復（`brew install --cask --force`）または zip フォールバックを提供。
//!
//! レート制限対策（#416）: gh CLI 認証トークンがあれば 5000req/h で API を叩く。
//! 2 チャンネル判定を /releases 一覧の 1 リクエストに統合。レート制限時はキャッシュ表示。
//!
//! プラットフォームフィルタ（#595）: 更新候補は「最新リリース」ではなく
//! **自分の環境向けアセットを含む最新リリース**。該当アセットが無いリリースは読み飛ばす。
//! macOS 先行リリース + Windows アセット後付け（#594 の運用）をしても、
//! Windows 側に「更新はあるがダウンロードできない」通知が出ない。
//! アセット命名規則の正は `tako_core::platform::release_assets`。
//!
//! Windows の適用（#2434）: 配布物は Inno Setup のインストーラーなので、zip の展開・`.app` の
//! 差し替えは使わない。インストーラーを検査して無言で起こし、tako が抜けたあとの入れ替えと
//! 起動し直しはインストーラーに任せる（`crate::platform::installer_update`。経路の振り分けは
//! [`apply_route`] の 1 か所）

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tako_core::i18n::Lang;
use tako_core::platform::release_assets::{self, Arch};
use tako_core::platform::support::Platform;

use crate::platform::installer_update;

/// 更新チェック間隔（24 時間）
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// チェック失敗時のリトライ間隔（1 時間）
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// ビルド時に埋め込んだバージョン。build.rs が `CARGO_PKG_VERSION` に、
/// リリースビルド時の `TAKO_WIN_NUM`（例: "3"）があれば `-win.3` を付与した文字列。
///
/// **これは「このバイナリをビルドしたときの版数」であって、更新判定や画面表示に使う
/// 版数とは限らない**。それらには `effective_current_version()` を使うこと（#723）。
/// ここを直接使ってよいのは HTTP の User-Agent だけ（「どのビルドが叩いたか」を知りたいため）
pub const CURRENT_VERSION: &str = env!("TAKO_FULL_VERSION");

/// macOS で `.app` が入る正本の置き場。**Dock のピンが指しているのはこのパス**なので、
/// 更新の途中でここが空になってはいけない（#1042。差し替えは
/// `tako_core::platform::bundle_install::replace_bundle_in_place` を通す）
pub const APP_INSTALL_PATH: &str = "/Applications/tako.app";

/// 更新判定と画面表示に使う「いま入っている版数」。
///
/// インストーラーが記録した版数（`DisplayVersion`）を最優先する。理由は #723:
/// すでに配布済みの `v0.5.13-win.1`〜`win.3` は `TAKO_WIN_NUM` 無しでビルドされており、
/// ビルド埋め込みの版数が一律 `0.5.13` になる。それを基準にすると「最新は win.3」と
/// 比べて永遠に「更新あり」になり、更新しても同じ版が入るだけの無限ループに陥る。
/// インストーラーの記録は win.1 の時点から正確なので、遡ってこの穴を塞げる。
///
/// ポータブル zip・開発ビルド・macOS では記録が無いので `CURRENT_VERSION` に落ちる。
///
/// 結果はプロセス内で 1 度だけ解決してキャッシュする。ステータスバーや設定ウィンドウの
/// 描画から呼ばれるので毎フレーム レジストリを引かせない（#212 / #168 で潰した
/// 「UI スレッドでの同期 syscall」を再び持ち込まないため）。実体が入れ替わるときは
/// 必ずプロセスも終わるので陳腐化しない。
pub fn effective_current_version() -> &'static str {
    static RESOLVED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(|| {
        resolve_current_version(
            tako_core::platform::install_info::installed_version().as_deref(),
            CURRENT_VERSION,
        )
    })
}

/// `effective_current_version` の判定本体（純粋関数。テストから直接叩ける）。
///
/// インストーラーの記録が**パースできる版数のときだけ**採用する。読めない値
/// （手で書き換えられた・別形式）に引きずられてビルド埋め込みの版数を捨てないため
fn resolve_current_version(installed: Option<&str>, built_in: &str) -> String {
    if let Some(raw) = installed {
        let normalized = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
        if ParsedVersion::parse(normalized).is_some() {
            return normalized.to_string();
        }
    }
    built_in.to_string()
}

const OWNER_REPO: &str = "takushio2525/tako";

/// GitHub API で直近リリースを取得する URL
const RELEASES_API_URL: &str =
    "https://api.github.com/repos/takushio2525/tako/releases?per_page=30";

/// 直前の成功結果キャッシュ（レート制限時に表示用）
static CACHED_UPDATES: Mutex<Option<ChannelUpdates>> = Mutex::new(None);

/// リリースチャンネル
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Test,
}

impl Channel {
    pub fn label(&self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Test => "test",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Channel::Stable => crate::ui_text::update::channel_stable(),
            Channel::Test => crate::ui_text::update::channel_test(),
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl Channel {
    /// `stable` / `test` を読む（読めなければ断りの型で返す。#2365）
    pub fn parse_typed(s: &str) -> Result<Self, UpdateRefusal> {
        match s {
            "stable" => Ok(Channel::Stable),
            "test" => Ok(Channel::Test),
            _ => Err(UpdateRefusal::UnknownChannel(s.to_string())),
        }
    }
}

impl std::str::FromStr for Channel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Channel::parse_typed(s).map_err(String::from)
    }
}

/// 更新チェック結果
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub channel: Channel,
    pub html_url: String,
    pub download_url: Option<String>,
    /// 実際に選ばれた配布物のファイル名（#595）。
    /// 「自分の環境向けのどれを掴んだか」を CLI / MCP から確認できるようにする
    pub asset_name: Option<String>,
    /// リリースノート本文（GitHub Releases の `body`。#616 のアップデート画面で表示）。
    /// 追加の API リクエストは要らない（/releases の応答に既に含まれている）
    pub notes: Option<String>,
    /// 配布物の大きさ（asset の `size`）。Windows の腕が受けたインストーラーと突き合わせる（#2434）
    pub asset_size: Option<u64>,
    /// 配布物の sha256（asset の `digest` = `sha256:<16 進>`。#2434）
    pub asset_digest: Option<String>,
}

/// 更新候補を選ぶ基準になる実行環境（#595）。
///
/// **`Platform::current()` を直に見ずに引数で受ける**のは、macOS 上から
/// 「Windows クライアントにはどう見えるか」をテストするため
/// （`platform::support` と同じ設計方針）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetEnv {
    pub platform: Platform,
    /// マトリクス外の CPU では `None` = 自分向けの配布物は存在しない
    pub arch: Option<Arch>,
}

impl TargetEnv {
    pub fn current() -> Self {
        Self {
            platform: Platform::current(),
            arch: Arch::current(),
        }
    }
}

/// 2 チャンネル同時チェック結果
#[derive(Debug, Clone, Default)]
pub struct ChannelUpdates {
    pub stable: Option<UpdateInfo>,
    pub test: Option<UpdateInfo>,
    /// レート制限でキャッシュから取得した場合の補助表示（例: 「制限中・約50分後にリセット」）
    pub rate_limit_note: Option<String>,
}

/// 更新チェックのエラー（#59: エラーと「更新なし」を区別する）。
///
/// 文は表示言語に従う（#2365）: 日英の対は [`CheckError::text_in`] と中身の
/// [`NetworkCause::text_in`] / [`ParseCause::text_in`] の網羅 match で、日本語は #2365 より前の
/// 固定文とバイト一致。tako が書く中身（404・想定外のステータス・読み取り / JSON の失敗）は文字列では
/// なくバリアントで持つ（文字列で持つと作った時点の言語に固まり、英語の枠に日本語の中身が混ざる）
#[derive(Debug, Clone)]
pub enum CheckError {
    /// GitHub API / Web のレート制限（X-RateLimit-Reset の UNIX timestamp を含む）
    RateLimit { retry_after: Option<u64> },
    /// ネットワークエラー（DNS 解決失敗、接続タイムアウト等）
    Network(NetworkCause),
    /// レスポンスのパースに失敗
    Parse(ParseCause),
}

/// [`CheckError::Network`] の中身
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkCause {
    /// HTTP の層の失敗（文は HTTP ライブラリのもの = 英語のまま）
    Transport(String),
    /// 404 = `リリースが見つからない（リポジトリ未公開または未リリース）`
    ReleasesNotFound,
    /// `予期しないステータスコード: <status>`
    UnexpectedStatus(u16),
}

/// [`CheckError::Parse`] の中身（`<e>` はライブラリの文）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseCause {
    /// `レスポンス読み取りエラー: <e>`
    ReadBody(String),
    /// `JSON パースエラー: <e>`
    Json(String),
}

impl NetworkCause {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::Transport(e), _) => e.clone(),
            (Self::ReleasesNotFound, Lang::Ja) => {
                "リリースが見つからない（リポジトリ未公開または未リリース）".into()
            }
            (Self::ReleasesNotFound, Lang::En) => {
                "releases not found (the repository is private or has no release yet)".into()
            }
            (Self::UnexpectedStatus(status), Lang::Ja) => {
                format!("予期しないステータスコード: {status}")
            }
            (Self::UnexpectedStatus(status), Lang::En) => {
                format!("unexpected status code: {status}")
            }
        }
    }
}

impl ParseCause {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::ReadBody(e), Lang::Ja) => format!("レスポンス読み取りエラー: {e}"),
            (Self::ReadBody(e), Lang::En) => format!("cannot read the response body: {e}"),
            (Self::Json(e), Lang::Ja) => format!("JSON パースエラー: {e}"),
            (Self::Json(e), Lang::En) => format!("JSON parse error: {e}"),
        }
    }
}

impl CheckError {
    /// 表示言語を明示しての文言（#2365。(バリアント, 言語) の網羅 match。中身も同じ言語で引く）
    pub fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (
                Self::RateLimit {
                    retry_after: Some(ts),
                },
                Lang::Ja,
            ) => {
                format!(
                    "GitHub レート制限中（リセット: {}）",
                    reset_eta_in(*ts, lang)
                )
            }
            (
                Self::RateLimit {
                    retry_after: Some(ts),
                },
                Lang::En,
            ) => {
                format!(
                    "GitHub rate limit reached (resets {})",
                    reset_eta_in(*ts, lang)
                )
            }
            (Self::RateLimit { retry_after: None }, Lang::Ja) => "GitHub レート制限中".into(),
            (Self::RateLimit { retry_after: None }, Lang::En) => "GitHub rate limit reached".into(),
            (Self::Network(cause), Lang::Ja) => {
                format!("ネットワークエラー: {}", cause.text_in(lang))
            }
            (Self::Network(cause), Lang::En) => format!("network error: {}", cause.text_in(lang)),
            (Self::Parse(cause), Lang::Ja) => {
                format!("レスポンス解析エラー: {}", cause.text_in(lang))
            }
            (Self::Parse(cause), Lang::En) => {
                format!("response parse error: {}", cause.text_in(lang))
            }
        }
    }

    /// 応答の `error`（`message` は組んだその場の表示言語 = [`Self::to_string`] と同じ言語）
    pub fn to_json(&self) -> serde_json::Value {
        self.to_json_in(tako_core::i18n::lang())
    }

    /// [`Self::to_json`] の表示言語を明示する版（`message` は中身だけ = #2365 より前と同じ形）
    pub fn to_json_in(&self, lang: Lang) -> serde_json::Value {
        match self {
            CheckError::RateLimit { retry_after } => serde_json::json!({
                "type": "rate_limit",
                "message": self.text_in(lang),
                "retry_after": retry_after,
            }),
            CheckError::Network(cause) => serde_json::json!({
                "type": "network",
                "message": cause.text_in(lang),
            }),
            CheckError::Parse(cause) => serde_json::json!({
                "type": "parse",
                "message": cause.text_in(lang),
            }),
        }
    }

    /// レート制限エラーならリセット時刻までの Duration を返す（最低 60 秒）
    pub fn retry_duration(&self) -> Duration {
        match self {
            CheckError::RateLimit {
                retry_after: Some(ts),
            } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Duration::from_secs(ts.saturating_sub(now).max(60))
            }
            _ => RETRY_INTERVAL,
        }
    }
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 表示言語は組んだその場の 1 回（中身も同じ言語 = #2365）
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

/// レート制限が解けるまでの相対表記（`約N分後` / `in ~N min`）。言語を引数で受ける（#2365。
/// 1 つの文の中で枠と同じ言語に引く。#608）
fn reset_eta_in(unix_ts: u64, lang: Lang) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let remaining = unix_ts.saturating_sub(now);
    let minutes = remaining / 60;
    match (remaining, minutes, lang) {
        (0, _, Lang::Ja) => "まもなく".into(),
        (0, _, Lang::En) => "soon".into(),
        (_, 0, Lang::Ja) => format!("約{remaining}秒後"),
        (_, 0, Lang::En) => format!("in ~{remaining}s"),
        (_, _, Lang::Ja) => format!("約{minutes}分後"),
        (_, _, Lang::En) => format!("in ~{minutes} min"),
    }
}

/// UI に公開する更新状態
#[derive(Debug, Clone)]
pub enum UpdateState {
    /// チェック中 or 更新なし
    Idle,
    /// 新しいバージョンが利用可能（1 チャンネル以上）
    Available(ChannelUpdates),
    /// テスト版の不安定警告確認中
    TestWarning(UpdateInfo),
    /// 更新確認ダイアログ表示中（チャンネル情報付き）
    ConfirmPending(UpdateInfo),
    /// ダウンロード/更新中
    Updating(String),
    /// 更新完了 — 自動再起動する
    Done(String),
    /// 更新失敗（brew 失敗時は zip フォールバックを提案）
    Failed(String),
    /// brew 更新失敗 → zip フォールバック提案中
    BrewFailedFallback {
        brew_error: String,
        info: UpdateInfo,
    },
    /// 更新チェック失敗（#59: エラーの可視化。静かにリトライする）
    CheckFailed(String),
}

impl UpdateState {
    /// 自動チェックの結果で上書きしてよい状態か（#616）。
    ///
    /// ユーザーが確認・更新の最中（TestWarning / ConfirmPending / Updating）や、
    /// まだ読まれていない結果（Failed / BrewFailedFallback）を裏で消さないための判定。
    /// 「一度閉じたら出さない」はここではなくカードのキー（`card_key`）が担う
    pub fn is_replaceable_by_check(&self) -> bool {
        matches!(
            self,
            UpdateState::Idle
                | UpdateState::Available(_)
                | UpdateState::Done(_)
                | UpdateState::CheckFailed(_)
        )
    }
}

/// 配布系統の判定結果
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum InstallMethod {
    /// Homebrew Cask で管理されている
    Homebrew,
    /// zip ダウンロード等の手動配置
    Zip,
    /// .app 実体はあるが brew の cask 台帳に登録されていない（#50）。
    /// brew upgrade 失敗等で台帳と実体が乖離した詰み状態
    BrokenBrew,
    /// Windows: Inno Setup のインストーラーで入れた先から動いている（#2434）。
    /// 更新はインストーラーを検査して無言で起こす
    WindowsInstaller,
    /// Windows: インストーラーで入れた先の外から動いている（ポータブル zip・開発ビルド）。
    /// インストーラーは前回の入れた先へ入れるので、アプリ内では更新しない（#2434）
    WindowsPortable,
}

impl InstallMethod {
    pub fn label(&self) -> &'static str {
        match self {
            InstallMethod::Homebrew => "homebrew",
            InstallMethod::Zip => "zip",
            InstallMethod::BrokenBrew => "broken-brew",
            InstallMethod::WindowsInstaller => "windows-installer",
            InstallMethod::WindowsPortable => "windows-portable",
        }
    }
}

/// 配布系統の判別（高速パス: ファイルパスのみ。brew サブプロセスを呼ばない）。
/// broken-brew の検知には `detect_install_method_full()` を使う
pub fn detect_install_method() -> InstallMethod {
    if Platform::current() == Platform::Windows {
        return windows_install_method();
    }
    detect_install_method_inner(is_app_in_caskroom(), is_exe_in_caskroom())
}

/// Windows の配布系統（#2434）。インストーラーの記録（レジストリ）を引くので
/// プロセス内で 1 度だけ解決する（更新画面の描画から呼ばれる。#212 / #168 と同じ理由）
fn windows_install_method() -> InstallMethod {
    static METHOD: std::sync::OnceLock<InstallMethod> = std::sync::OnceLock::new();
    *METHOD.get_or_init(|| {
        let location = tako_core::platform::install_info::install_location();
        let installed = std::env::current_exe().is_ok_and(|exe| {
            crate::platform::installer_update::runs_from_install_dir(location.as_deref(), &exe)
        });
        if installed {
            InstallMethod::WindowsInstaller
        } else {
            InstallMethod::WindowsPortable
        }
    })
}

/// broken-brew を含む完全な配布系統判別（低速: brew サブプロセスを呼ぶ可能性あり）。
/// background executor や CLI/MCP の status から呼ぶ。render パスでは呼ばない
pub fn detect_install_method_full() -> InstallMethod {
    // Windows に brew は無い（台帳の確認もしない）
    if Platform::current() == Platform::Windows {
        return windows_install_method();
    }
    let app_in_caskroom = is_app_in_caskroom();
    let exe_in_caskroom = is_exe_in_caskroom();
    let fast = detect_install_method_inner(app_in_caskroom, exe_in_caskroom);
    if fast != InstallMethod::Zip {
        return fast;
    }
    // **テストプロセスからは外部コマンドを起こさない**（#1253。#944 の
    // `claude agents --json` と同型）。ここは `brew --version` / `brew list --cask` を
    // 起こすので、`cargo test` がユーザーの `~/Library/Caches/Homebrew` を
    // 書き換えていた（実測: 空の HOME で回すと 698 ファイル出来る）。
    // 判定材料はファイルパスだけの高速パスに落とす = Zip
    if tako_core::paths::is_test_process() && !tako_core::paths::issue944_legacy() {
        return InstallMethod::Zip;
    }
    // Zip 判定だが、実は broken-brew かもしれない — brew の台帳を確認
    if applications_tako_app_exists() && is_brew_available() && !is_brew_cask_registered() {
        return InstallMethod::BrokenBrew;
    }
    InstallMethod::Zip
}

fn is_app_in_caskroom() -> bool {
    if let Some(bundle) = app_bundle_path() {
        let resolved = std::fs::canonicalize(&bundle).unwrap_or(bundle);
        return resolved.to_string_lossy().contains("/Caskroom/");
    }
    false
}

fn is_exe_in_caskroom() -> bool {
    if let Ok(exe) = std::env::current_exe() {
        let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
        let s = resolved.to_string_lossy();
        return s.contains("/Caskroom/") || s.contains("/Cellar/");
    }
    false
}

/// 高速パスの判定ロジック（テスト用に公開）
fn detect_install_method_inner(app_in_caskroom: bool, exe_in_caskroom: bool) -> InstallMethod {
    if app_in_caskroom || exe_in_caskroom {
        InstallMethod::Homebrew
    } else {
        InstallMethod::Zip
    }
}

/// `/Applications/tako.app` が存在するか（シンボリックリンクでも実体でも OK）
fn applications_tako_app_exists() -> bool {
    Path::new(APP_INSTALL_PATH).exists()
}

/// brew コマンドが使えるか
fn is_brew_available() -> bool {
    // #586: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// brew の cask 台帳に tako が登録されているか
fn is_brew_cask_registered() -> bool {
    // #586: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
        .args(["list", "--cask", "takushio2525/tako/tako"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// broken-brew 状態の詳細診断（CLI/MCP の status 用）
pub fn diagnose_broken_brew() -> Option<BrokenBrewDiagnosis> {
    if detect_install_method_full() != InstallMethod::BrokenBrew {
        return None;
    }
    Some(BrokenBrewDiagnosis {
        app_path: APP_INSTALL_PATH.into(),
        brew_available: true,
        cask_registered: false,
        repair_command: "brew install --cask takushio2525/tako/tako --force".into(),
    })
}

/// broken-brew 診断結果
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrokenBrewDiagnosis {
    pub app_path: String,
    pub brew_available: bool,
    pub cask_registered: bool,
    pub repair_command: String,
}

/// broken-brew の修復: `brew install --cask --force` で台帳を再締結する。
/// 文は組んだその場の表示言語（#2365。型で受けるなら [`repair_brew_typed`]）
pub fn repair_brew() -> Result<String, String> {
    repair_brew_typed()
        .map(|note| note.to_string())
        .map_err(String::from)
}

/// [`repair_brew`] の型の口
pub fn repair_brew_typed() -> Result<UpdateNote, UpdateRefusal> {
    let method = detect_install_method_full();
    if method != InstallMethod::BrokenBrew {
        return Err(UpdateRefusal::RepairNotNeeded(method));
    }
    // #586: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
            .args(["install", "--cask", "takushio2525/tako/tako", "--force"])
            .output()
            .map_err(|e| UpdateRefusal::BrewNotRun(e.to_string()))?;
    if output.status.success() {
        Ok(UpdateNote::Repaired)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(UpdateRefusal::BrewInstallFailed(stderr.into_owned()))
    }
}

/// 実行中の .app バンドルのパス（macOS 固有）
fn app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // /Applications/tako.app/Contents/MacOS/tako-app -> /Applications/tako.app
    let mut p = exe.as_path();
    loop {
        if p.extension().is_some_and(|e| e == "app") {
            return Some(p.to_path_buf());
        }
        p = p.parent()?;
    }
}

/// PATH 上の `tako` CLI 重複を検出する。
/// 自分のバンドル内 CLI と異なるパスに tako があれば警告対象。
pub fn detect_duplicate_cli() -> Vec<PathBuf> {
    let own_bundle = app_bundle_path();
    let mut seen = Vec::new();
    let Ok(path_var) = std::env::var("PATH") else {
        return seen;
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("tako");
        if !candidate.is_file() && !candidate.is_symlink() {
            continue;
        }
        let resolved = std::fs::canonicalize(&candidate).unwrap_or(candidate.clone());
        // 同じバンドル内なら OK
        if let Some(ref bundle) = own_bundle {
            let bundle_resolved = std::fs::canonicalize(bundle).unwrap_or_else(|_| bundle.clone());
            if resolved.starts_with(&bundle_resolved) {
                continue;
            }
        }
        if !seen
            .iter()
            .any(|p: &PathBuf| std::fs::canonicalize(p).unwrap_or(p.clone()) == resolved)
        {
            seen.push(candidate);
        }
    }
    seen
}

/// 指定チャンネルの最新版をチェック（#416: check_all_channels 1 本に統合）
pub fn check_channel(channel: Channel) -> Result<Option<UpdateInfo>, CheckError> {
    let updates = check_all_channels()?;
    Ok(match channel {
        Channel::Stable => updates.stable,
        Channel::Test => updates.test,
    })
}

/// `tako update apply` / `apply-zip` が当てる版（チャンネルの省略は stable）。チャンネルが読めない・
/// 確認に失敗した・新しい版が無いときは断る（#2365: 枠と中身を同じ言語で組む 1 か所）
pub fn update_target(channel: Option<&str>) -> Result<UpdateInfo, UpdateRefusal> {
    let channel = Channel::parse_typed(channel.unwrap_or("stable"))?;
    check_channel(channel)
        .map_err(UpdateRefusal::CheckFailed)?
        .ok_or(UpdateRefusal::NoNewVersion(channel))
}

/// パース済みバージョン（`X.Y.Z` または `X.Y.Z-test.N`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// None = 安定版、Some(n) = テスト版 `-test.N`
    pub test_num: Option<u32>,
    /// None = Windows 固有の反復ではない、Some(n) = `-win.N`（#723）。
    ///
    /// 同じ Cargo バージョンのまま Windows 向けプレビューを反復するための連番。
    /// これを解析できないと `-win.N` タグのリリースが更新候補から丸ごと落ちるうえ、
    /// インストーラーの記録（`DisplayVersion`）も「読めない値」として捨てられる
    pub win_num: Option<u32>,
}

impl ParsedVersion {
    pub fn parse(s: &str) -> Option<Self> {
        let (base, test_num, win_num) = if let Some((base, suffix)) = s.split_once("-test.") {
            (base, Some(suffix.parse::<u32>().ok()?), None)
        } else if let Some((base, suffix)) = s.split_once("-win.") {
            (base, None, Some(suffix.parse::<u32>().ok()?))
        } else if s.contains('-') {
            // `-test.N` / `-win.N` 以外のプレリリースサフィックス（例: `-rc.1`）は解析しない
            return None;
        } else {
            (s, None, None)
        };
        let parts: Vec<&str> = base.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(Self {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
            test_num,
            win_num,
        })
    }

    #[cfg(test)]
    pub fn channel(&self) -> Channel {
        if self.test_num.is_some() {
            Channel::Test
        } else {
            Channel::Stable
        }
    }

    /// 安定版ベースバージョン同士の比較用タプル
    fn base_tuple(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }

    /// サフィックスの種別と番号を正規化した比較用タプル。
    /// ランク: 安定版(2) > `-test.N`(1) > `-win.N`(0)。同ランク内は番号順。
    ///
    /// `-win.N` を最下位に置くのは semver の「プレリリースは正式版より古い」に沿わせるため。
    /// Windows 上での新旧判定はこの順序では足りないので `is_newer_release` が補正する
    fn suffix_rank(&self) -> (u8, u32) {
        match (self.test_num, self.win_num) {
            (None, None) => (2, 0),
            (Some(n), _) => (1, n),
            (_, Some(n)) => (0, n),
        }
    }
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 同じベースなら stable > test > win。`-win.N` を持たない版同士の結果は
        // suffix_rank 導入前（stable > test、test 同士は番号順）と完全に一致する
        self.base_tuple()
            .cmp(&other.base_tuple())
            .then_with(|| self.suffix_rank().cmp(&other.suffix_rank()))
    }
}

/// プラットフォームを考慮した「available は current より新しいか」判定（#723）。
///
/// semver の `Ord`（stable > prerelease）とは異なり、**Windows では `-win.N` を
/// 同ベースの stable 以上として扱う**。理由: Windows バイナリの版数が
/// `TAKO_WIN_NUM` 未指定でビルドされていると素の `X.Y.Z`（`win_num: None`）になり、
/// semver 順では `-win.N` が「古い」と判定されて配布済みプレビューへ更新できない。
///
/// 逆に macOS は semver 準拠のままにする。`-win.N` は Windows 向けの配布物しか
/// 持たないので macOS の更新候補には出てはいけない（アセット選択でも落ちるが、
/// 版数比較の段でも落として二重に守る）
fn is_newer_release(
    available: &ParsedVersion,
    current: &ParsedVersion,
    platform: Platform,
) -> bool {
    match available.base_tuple().cmp(&current.base_tuple()) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match platform {
            Platform::Windows => match (available.win_num, current.win_num) {
                // win.N 同士は番号順
                (Some(a), Some(c)) => a > c,
                // 現在が素の X.Y.Z（TAKO_WIN_NUM 未指定ビルド）なら win.N は「新しい」
                (Some(_), None) => true,
                _ => available.suffix_rank() > current.suffix_rank(),
            },
            Platform::MacOs => available.suffix_rank() > current.suffix_rank(),
        },
    }
}

/// semver 比較（a > b なら true）。`-test.N` サフィックス対応
#[cfg(test)]
fn is_newer(a: &str, b: &str) -> bool {
    match (ParsedVersion::parse(a), ParsedVersion::parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// gh CLI の認証トークンを取得（#416）。メモリ内のみで保持しログ・設定に書かない
fn gh_auth_token() -> Option<String> {
    // #586: 更新確認は GUI プロセスから走るのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new("gh"))
            .args(["auth", "token"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    Some(token)
}

/// レート制限中にキャッシュを見せるときの補助表示（`rate_limit_note`。#416 / 日英は #2365）
fn rate_limit_note_in(retry_after: Option<u64>, lang: Lang) -> String {
    match (retry_after, lang) {
        (Some(ts), Lang::Ja) => format!(
            "GitHub API 制限中（キャッシュ表示・リセット: {}）",
            reset_eta_in(ts, lang)
        ),
        (Some(ts), Lang::En) => format!(
            "GitHub API rate limited (showing the cache; resets {})",
            reset_eta_in(ts, lang)
        ),
        (None, Lang::Ja) => "GitHub API 制限中（キャッシュ表示）".into(),
        (None, Lang::En) => "GitHub API rate limited (showing the cache)".into(),
    }
}

/// 2 チャンネル同時チェック（GitHub API 1 リクエスト。#416）。
/// gh CLI トークンがあれば認証付き（5000req/h）、なければ未認証（60req/h）。
/// レート制限時はキャッシュがあれば rate_limit_note 付きで返す
pub fn check_all_channels() -> Result<ChannelUpdates, CheckError> {
    let token = gh_auth_token();

    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent();

    let mut req = agent
        .get(RELEASES_API_URL)
        .header("User-Agent", &format!("tako/{CURRENT_VERSION}"))
        .header("Accept", "application/vnd.github+json");
    if let Some(ref t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }

    let resp = req
        .call()
        .map_err(|e| CheckError::Network(NetworkCause::Transport(e.to_string())))?;

    let status = resp.status().as_u16();
    if status == 403 || status == 429 {
        let retry_after = resp
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        // キャッシュがあれば補助表示付きで返す
        if let Some(cached) = CACHED_UPDATES.lock().ok().and_then(|g| g.clone()) {
            let note = rate_limit_note_in(retry_after, tako_core::i18n::lang());
            return Ok(ChannelUpdates {
                rate_limit_note: Some(note),
                ..cached
            });
        }
        return Err(CheckError::RateLimit { retry_after });
    }
    if status == 404 {
        return Err(CheckError::Network(NetworkCause::ReleasesNotFound));
    }
    if status != 200 {
        return Err(CheckError::Network(NetworkCause::UnexpectedStatus(status)));
    }

    let body: String = resp
        .into_body()
        .read_to_string()
        .map_err(|e| CheckError::Parse(ParseCause::ReadBody(e.to_string())))?;
    let releases: Vec<serde_json::Value> = serde_json::from_str(&body)
        .map_err(|e| CheckError::Parse(ParseCause::Json(e.to_string())))?;

    let result = parse_releases(&releases);

    if let Ok(mut guard) = CACHED_UPDATES.lock() {
        *guard = Some(result.clone());
    }

    Ok(result)
}

/// [`select_asset`] が選んだ配布物
struct SelectedAsset {
    name: String,
    url: String,
    /// asset の `size`（#2434）
    size: Option<u64>,
    /// asset の `digest`（#2434）
    digest: Option<String>,
}

/// 1 リリースの `assets` から自分の環境向けの配布物を選ぶ（#595）。
///
/// **該当が無ければ `None`** = このリリースは自分にとって「更新候補ではない」。
fn select_asset(
    release: &serde_json::Value,
    platform: Platform,
    arch: Arch,
) -> Option<SelectedAsset> {
    let assets = release["assets"].as_array()?;
    let names: Vec<&str> = assets.iter().filter_map(|a| a["name"].as_str()).collect();
    let chosen = release_assets::select(names.iter().copied(), platform, arch)?;
    let asset = assets.iter().find(|a| a["name"].as_str() == Some(chosen));

    // 通常は GitHub が browser_download_url を返す。欠けていても命名規則から
    // 復元できる（URL の組み立て規則はここ 1 箇所）
    let url = asset
        .and_then(|a| a["browser_download_url"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let tag = release["tag_name"].as_str().unwrap_or_default();
            format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/{chosen}")
        });

    Some(SelectedAsset {
        name: chosen.to_string(),
        url,
        size: asset.and_then(|a| a["size"].as_u64()),
        digest: asset.and_then(|a| a["digest"].as_str()).map(str::to_string),
    })
}

/// /releases JSON 配列から ChannelUpdates をパースする（実行環境で判定）
fn parse_releases(releases: &[serde_json::Value]) -> ChannelUpdates {
    parse_releases_for(releases, TargetEnv::current(), effective_current_version())
}

/// 実行環境と現在バージョンを明示して判定する純関数（#595 のテスト用の入口）。
///
/// **リリースは新しい順に並んでいる前提**で、チャンネルごとに
/// 「現在より新しく、かつ自分の環境向けアセットを持つ」最初のリリースを採る。
/// アセットが後から追加された（`gh release upload`）場合、その時点で初めて更新として見える。
fn parse_releases_for(
    releases: &[serde_json::Value],
    env: TargetEnv,
    current_version: &str,
) -> ChannelUpdates {
    let current = ParsedVersion::parse(current_version);
    let mut result = ChannelUpdates::default();

    // 配布物が存在しないアーキテクチャでは更新候補を出さない
    // （出しても掴めるアセットが無く「更新できない更新通知」になる）
    let Some(arch) = env.arch else {
        return result;
    };

    for release in releases {
        if result.stable.is_some() && result.test.is_some() {
            break;
        }

        let tag = release["tag_name"].as_str().unwrap_or_default();
        let version_str = tag.strip_prefix('v').unwrap_or(tag);
        let Some(ver) = ParsedVersion::parse(version_str) else {
            continue;
        };
        let is_prerelease = release["prerelease"].as_bool().unwrap_or(false);
        let channel = if is_prerelease {
            Channel::Test
        } else {
            Channel::Stable
        };

        // 既に埋まっているチャンネルは以降のリリースを見る必要がない
        let slot_filled = match channel {
            Channel::Stable => result.stable.is_some(),
            Channel::Test => result.test.is_some(),
        };
        if slot_filled {
            continue;
        }

        if let Some(ref cur) = current {
            if !is_newer_release(&ver, cur, env.platform) {
                continue;
            }
        }

        // 自分の環境向けアセットが無いリリースは読み飛ばして次を探す（#595 要件 1）
        let Some(asset) = select_asset(release, env.platform, arch) else {
            continue;
        };

        let html_url = release["html_url"].as_str().unwrap_or_default().to_string();
        let notes = release["body"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let slot = match channel {
            Channel::Stable => &mut result.stable,
            Channel::Test => &mut result.test,
        };
        *slot = Some(UpdateInfo {
            version: version_str.to_string(),
            channel,
            html_url,
            download_url: Some(asset.url),
            asset_name: Some(asset.name),
            notes,
            asset_size: asset.size,
            asset_digest: asset.digest,
        });
    }

    result
}

/// 更新通知カード（#616）が案内している内容の一意キー。
///
/// **バージョン単位で「一度閉じたらもう出さない」を成立させるための鍵**。
/// 両チャンネルを含むので、片方だけ新しくなってもキーが変わり、カードが出直す。
/// 更新が 1 件も無ければ `None`（= 出すものが無い）
pub fn card_key(updates: &ChannelUpdates) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ref s) = updates.stable {
        parts.push(format!("stable:{}", s.version));
    }
    if let Some(ref t) = updates.test {
        parts.push(format!("test:{}", t.version));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// カードを出すか（#616）。`dismissed` は前回 × で閉じたときのキー。
/// 案内内容が変わればキーが変わるので、また出る
pub fn card_should_show(updates: &ChannelUpdates, dismissed: Option<&str>) -> bool {
    match card_key(updates) {
        Some(key) => dismissed != Some(key.as_str()),
        None => false,
    }
}

/// 更新の適用の結果（#2434）。`message` は CLI / MCP の応答と画面にそのまま出る
#[derive(Debug)]
pub struct Applied {
    pub message: String,
    /// 適用の後に何で起動し直すか
    pub restart: Restart,
}

/// 適用の後の起動し直し方（#2434）
#[derive(Debug)]
pub enum Restart {
    /// macOS: 差し替えた `.app` を新しいプロセスとして開く（ファイルはもう新しい版）
    OpenBundle,
    /// Windows: 検査済みのインストーラーを無言で起こし、自分は抜ける。
    /// **この時点ではまだ何も上書きしていない**（上書きと起動し直しはインストーラーが行う）
    RunInstaller(installer_update::PreparedInstaller),
}

impl Restart {
    /// 起動し直しに失敗したときの文言（macOS は更新済み・Windows は未適用で、意味が違う）
    pub fn failed_text(&self, err: &str) -> String {
        match self {
            Restart::OpenBundle => crate::ui_text::update::restart_failed(err),
            Restart::RunInstaller(_) => crate::ui_text::update::installer_launch_failed(err),
        }
    }
}

/// 更新の口（CLI / MCP `tako update apply` / `apply-zip` / `repair` と GUI の更新画面）の断り（#2365）。
///
/// dispatch は GUI の口の `Err(String)` をそのまま載せるので、文の言語はここで決まる。日英の対は
/// [`UpdateRefusal::text_in`] の (バリアント, 言語) の網羅 match の 1 か所で、日本語は #2365 より前の
/// 固定文とバイト一致（`tests::更新の口の断りは全バリアントが日英を持ち日本語は2365より前と一致`）。
/// 枠と中身（[`CheckError`]・[`installer_update::InstallerError`]）は同じ言語で引く。
/// 「見つからない」系は英語にも ` not found` を含め、「見つかりません」系は含めない
/// （`DispatchError::says_not_found` の当たり方を日英でそろえる）
#[derive(Debug, Clone)]
pub enum UpdateRefusal {
    /// `不明なチャンネル: "<s>"（stable / test のいずれか）`
    UnknownChannel(String),
    /// `更新チェックに失敗: <CheckError>`
    CheckFailed(CheckError),
    /// `<安定版|テスト版>の新しいバージョンが見つからない（既に最新版です）`
    NoNewVersion(Channel),
    /// `<InstallerError::NotInstalled>（リリースページ: <url>）`（Windows のポータブル版）
    PortableNotUpdatable { release_page: String },
    /// `Windows には zip で差し替える経路が無い（…）。…（リリースページ: <url>）`
    NoZipRouteOnWindows { release_page: String },
    /// `配布系統 <label> では更新できない`
    MethodCannotUpdate(InstallMethod),
    /// `<InstallerError>。今の版のまま使えます（手動で入れるならリリースページ: <url>）`
    InstallerRefused {
        error: installer_update::InstallerError,
        release_page: String,
    },
    /// `このリリースに Windows のインストーラーが見つからない（リリースページ: <url>）`
    NoWindowsInstaller { release_page: String },
    /// `brew の実行に失敗: <e>`
    BrewNotRun(String),
    /// `brew upgrade が失敗: <stderr>`（zip へ切り替えられるなら `[zip-fallback-available]` の行を添える）
    BrewUpgradeFailed { stderr: String, zip_fallback: bool },
    /// `ダウンロード用 ZIP アセットが見つかりません`
    NoZipAsset,
    /// `一時ディレクトリの作成に失敗: <e>`
    TempDirFailed(String),
    /// `ダウンロードに失敗: <e>`
    DownloadFailed(String),
    /// `ZIP ファイルの作成に失敗: <e>`
    ZipCreateFailed(String),
    /// `ダウンロードの書き込みに失敗: <e>`
    DownloadWriteFailed(String),
    /// `ditto による展開に失敗: <e>`
    DittoNotRun(String),
    /// `ZIP の展開に失敗: <stderr>`
    UnzipFailed(String),
    /// `展開した ZIP に tako.app が見つかりません`
    NoAppInZip,
    /// `bundle_install::replace_bundle_in_place` の断りをそのまま（tako-core の文。日英は Issue #2365 の
    /// 残り = 枠を持たないので混ざらない）
    BundleReplace(String),
    /// `現在の配布系統は <label> のため修復は不要です`
    RepairNotNeeded(InstallMethod),
    /// `brew install --force が失敗: <stderr>`
    BrewInstallFailed(String),
    /// `.app バンドルのパスが特定できない（CLI 単体実行？）`
    NoAppBundle,
    /// `再起動に失敗: <e>`
    RestartFailed(String),
    /// `<InstallerError>`（起動し直す対象の exe が引けない）
    Installer(installer_update::InstallerError),
    /// `<InstallerError>。今の版のまま使えます`（インストーラーを起こせない）
    InstallerNotLaunched(installer_update::InstallerError),
}

/// `brew upgrade` が失敗して zip へ切り替えられるときに添える行の印（#50。GUI と AI が読む目印 =
/// 日英で同じ）
pub const ZIP_FALLBACK_MARK: &str = "[zip-fallback-available]";

impl UpdateRefusal {
    /// 表示言語を明示しての文言（#2365）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::UnknownChannel(s), Ja) => {
                format!("不明なチャンネル: {s:?}（stable / test のいずれか）")
            }
            (Self::UnknownChannel(s), En) => {
                format!("unknown channel: {s:?} (one of stable / test)")
            }
            (Self::CheckFailed(e), Ja) => format!("更新チェックに失敗: {}", e.text_in(lang)),
            (Self::CheckFailed(e), En) => {
                format!("the update check failed: {}", e.text_in(lang))
            }
            (Self::NoNewVersion(Channel::Stable), Ja) => {
                "安定版の新しいバージョンが見つからない（既に最新版です）".into()
            }
            (Self::NoNewVersion(Channel::Test), Ja) => {
                "テスト版の新しいバージョンが見つからない（既に最新版です）".into()
            }
            (Self::NoNewVersion(Channel::Stable), En) => {
                "newer stable version not found (already up to date)".into()
            }
            (Self::NoNewVersion(Channel::Test), En) => {
                "newer test version not found (already up to date)".into()
            }
            (Self::PortableNotUpdatable { release_page }, Ja) => format!(
                "{}（リリースページ: {release_page}）",
                installer_update::InstallerError::NotInstalled.text_in(lang)
            ),
            (Self::PortableNotUpdatable { release_page }, En) => format!(
                "{} (release page: {release_page})",
                installer_update::InstallerError::NotInstalled.text_in(lang)
            ),
            (Self::NoZipRouteOnWindows { release_page }, Ja) => format!(
                "Windows には zip で差し替える経路が無い（zip は macOS の .app 用）。\
                 `tako update apply` でインストーラーから更新してください（リリースページ: {release_page}）"
            ),
            (Self::NoZipRouteOnWindows { release_page }, En) => format!(
                "Windows has no way to replace tako with a zip (the zip is for the macOS .app). \
                 Update from the installer with `tako update apply` (release page: {release_page})"
            ),
            (Self::MethodCannotUpdate(method), Ja) => {
                format!("配布系統 {} では更新できない", method.label())
            }
            (Self::MethodCannotUpdate(method), En) => {
                format!("cannot update with the install method {}", method.label())
            }
            (
                Self::InstallerRefused {
                    error,
                    release_page,
                },
                Ja,
            ) => format!(
                "{}。今の版のまま使えます（手動で入れるならリリースページ: {release_page}）",
                error.text_in(lang)
            ),
            (
                Self::InstallerRefused {
                    error,
                    release_page,
                },
                En,
            ) => format!(
                "{}. The current version keeps working (to install by hand, see the release page: {release_page})",
                error.text_in(lang)
            ),
            (Self::NoWindowsInstaller { release_page }, Ja) => format!(
                "このリリースに Windows のインストーラーが見つからない（リリースページ: {release_page}）"
            ),
            (Self::NoWindowsInstaller { release_page }, En) => format!(
                "Windows installer not found in this release (release page: {release_page})"
            ),
            (Self::BrewNotRun(e), Ja) => format!("brew の実行に失敗: {e}"),
            (Self::BrewNotRun(e), En) => format!("cannot run brew: {e}"),
            (
                Self::BrewUpgradeFailed {
                    stderr,
                    zip_fallback: false,
                },
                Ja,
            ) => format!("brew upgrade が失敗: {stderr}"),
            (
                Self::BrewUpgradeFailed {
                    stderr,
                    zip_fallback: true,
                },
                Ja,
            ) => format!(
                "brew upgrade が失敗: {stderr}\n{ZIP_FALLBACK_MARK} brew 更新に失敗しました。\
                 `tako update apply-zip` で zip 経由の更新が可能です"
            ),
            (
                Self::BrewUpgradeFailed {
                    stderr,
                    zip_fallback: false,
                },
                En,
            ) => format!("brew upgrade failed: {stderr}"),
            (
                Self::BrewUpgradeFailed {
                    stderr,
                    zip_fallback: true,
                },
                En,
            ) => format!(
                "brew upgrade failed: {stderr}\n{ZIP_FALLBACK_MARK} the brew update failed. \
                 `tako update apply-zip` can update through the zip instead"
            ),
            (Self::NoZipAsset, Ja) => "ダウンロード用 ZIP アセットが見つかりません".into(),
            (Self::NoZipAsset, En) => "the release has no ZIP asset to download".into(),
            (Self::TempDirFailed(e), Ja) => format!("一時ディレクトリの作成に失敗: {e}"),
            (Self::TempDirFailed(e), En) => format!("cannot create the temporary directory: {e}"),
            (Self::DownloadFailed(e), Ja) => format!("ダウンロードに失敗: {e}"),
            (Self::DownloadFailed(e), En) => format!("the download failed: {e}"),
            (Self::ZipCreateFailed(e), Ja) => format!("ZIP ファイルの作成に失敗: {e}"),
            (Self::ZipCreateFailed(e), En) => format!("cannot create the ZIP file: {e}"),
            (Self::DownloadWriteFailed(e), Ja) => format!("ダウンロードの書き込みに失敗: {e}"),
            (Self::DownloadWriteFailed(e), En) => format!("cannot write the download: {e}"),
            (Self::DittoNotRun(e), Ja) => format!("ditto による展開に失敗: {e}"),
            (Self::DittoNotRun(e), En) => format!("cannot extract with ditto: {e}"),
            (Self::UnzipFailed(stderr), Ja) => format!("ZIP の展開に失敗: {stderr}"),
            (Self::UnzipFailed(stderr), En) => format!("cannot extract the ZIP: {stderr}"),
            (Self::NoAppInZip, Ja) => "展開した ZIP に tako.app が見つかりません".into(),
            (Self::NoAppInZip, En) => "the extracted ZIP has no tako.app".into(),
            (Self::BundleReplace(text), Ja | En) => text.clone(),
            (Self::RepairNotNeeded(method), Ja) => {
                format!("現在の配布系統は {} のため修復は不要です", method.label())
            }
            (Self::RepairNotNeeded(method), En) => format!(
                "no repair is needed because the install method is {}",
                method.label()
            ),
            (Self::BrewInstallFailed(stderr), Ja) => {
                format!("brew install --force が失敗: {stderr}")
            }
            (Self::BrewInstallFailed(stderr), En) => {
                format!("brew install --force failed: {stderr}")
            }
            (Self::NoAppBundle, Ja) => {
                ".app バンドルのパスが特定できない（CLI 単体実行？）".into()
            }
            (Self::NoAppBundle, En) => {
                "cannot find the path of the .app bundle (running the CLI alone?)".into()
            }
            (Self::RestartFailed(e), Ja) => format!("再起動に失敗: {e}"),
            (Self::RestartFailed(e), En) => format!("the restart failed: {e}"),
            (Self::Installer(error), Ja | En) => error.text_in(lang),
            (Self::InstallerNotLaunched(error), Ja) => {
                format!("{}。今の版のまま使えます", error.text_in(lang))
            }
            (Self::InstallerNotLaunched(error), En) => {
                format!("{}. The current version keeps working", error.text_in(lang))
            }
        }
    }
}

impl std::fmt::Display for UpdateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 表示言語は組んだその場の 1 回（枠と中身を同じ言語で引く）
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

/// ControlHost の口は `Result<_, String>` なので、GUI は `?` でそのまま返せる（文はその場の表示言語）
impl From<UpdateRefusal> for String {
    fn from(r: UpdateRefusal) -> Self {
        r.to_string()
    }
}

/// 更新の適用・修復が済んだときの文（応答の `message` と更新画面。#2365）。日英の対は
/// [`UpdateNote::text_in`] の網羅 match の 1 か所で、日本語は #2365 より前の固定文とバイト一致
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateNote {
    /// `Homebrew で更新完了`
    HomebrewDone,
    /// `既に最新版です`（brew が「入っている」と答えた）
    AlreadyLatest,
    /// `ZIP で更新完了（差し替え: <手段>）`。`pin_may_drop` なら Dock のピンの注意を添える（#1042）
    ZipDone {
        strategy: &'static str,
        pin_may_drop: bool,
    },
    /// `インストーラーで更新（v<版> の sha256 が配布元と一致。…）`（#2434）
    InstallerReady { version: String },
    /// `brew の cask 台帳を再締結しました。以後 brew upgrade で更新できます`
    Repaired,
}

impl UpdateNote {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::HomebrewDone, Ja) => "Homebrew で更新完了".into(),
            (Self::HomebrewDone, En) => "updated with Homebrew".into(),
            (Self::AlreadyLatest, Ja) => "既に最新版です".into(),
            (Self::AlreadyLatest, En) => "already up to date".into(),
            (
                Self::ZipDone {
                    strategy,
                    pin_may_drop: false,
                },
                Ja,
            ) => format!("ZIP で更新完了（差し替え: {strategy}）"),
            (
                Self::ZipDone {
                    strategy,
                    pin_may_drop: true,
                },
                Ja,
            ) => format!(
                "ZIP で更新完了（差し替え: {strategy}。この環境ではアトミックな入れ替えが使えないため、\
                 Dock のピン留めが外れることがあります）"
            ),
            (
                Self::ZipDone {
                    strategy,
                    pin_may_drop: false,
                },
                En,
            ) => format!("updated with the ZIP (replacement: {strategy})"),
            (
                Self::ZipDone {
                    strategy,
                    pin_may_drop: true,
                },
                En,
            ) => format!(
                "updated with the ZIP (replacement: {strategy}; this environment cannot swap atomically, \
                 so the Dock pin may come off)"
            ),
            (Self::InstallerReady { version }, Ja) => format!(
                "インストーラーで更新（v{version} の sha256 が配布元と一致。tako を終了して入れ替え、終わったら起動し直します）"
            ),
            (Self::InstallerReady { version }, En) => format!(
                "updating with the installer (the sha256 of v{version} matches the release; tako quits, \
                 the installer replaces it and starts it again)"
            ),
            (Self::Repaired, Ja) => {
                "brew の cask 台帳を再締結しました。以後 brew upgrade で更新できます".into()
            }
            (Self::Repaired, En) => {
                "re-registered tako in the brew cask list; brew upgrade works from now on".into()
            }
        }
    }
}

impl std::fmt::Display for UpdateNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

/// 配布系統ごとの適用経路（#2434）。**Windows の配布系統が macOS の経路
/// （brew / zip の展開 / `.app` の差し替え）へ入らない**ことをここ 1 か所で決める
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyRoute {
    Homebrew,
    /// zip を展開して `.app` を差し替える（macOS 専用）
    MacZip,
    /// インストーラーを検査して無言で起こす（Windows 専用）
    WindowsInstaller,
    /// 適用できない（理由は配布系統の側にある）
    Refuse(InstallMethod),
}

/// `apply`（配布系統に任せる）の経路
fn apply_route(method: InstallMethod) -> ApplyRoute {
    match method {
        InstallMethod::Homebrew => ApplyRoute::Homebrew,
        // broken-brew では brew 経由の更新は不可能 → zip で直接更新
        InstallMethod::Zip | InstallMethod::BrokenBrew => ApplyRoute::MacZip,
        InstallMethod::WindowsInstaller => ApplyRoute::WindowsInstaller,
        InstallMethod::WindowsPortable => ApplyRoute::Refuse(method),
    }
}

/// `apply-zip`（brew 失敗時の zip への強制）の経路。Windows では zip で `.app` を
/// 差し替える手段が原理的に無いので断る
fn zip_route(method: InstallMethod) -> ApplyRoute {
    match method {
        InstallMethod::Homebrew | InstallMethod::Zip | InstallMethod::BrokenBrew => {
            ApplyRoute::MacZip
        }
        InstallMethod::WindowsInstaller | InstallMethod::WindowsPortable => {
            ApplyRoute::Refuse(method)
        }
    }
}

/// 更新を実行する（ブロッキング。background executor で呼ぶ）。
/// broken-brew 状態では zip フォールバックを使う。文は組んだその場の表示言語（#2365）
pub fn perform_update(info: &UpdateInfo) -> Result<Applied, String> {
    run_route(apply_route(detect_install_method_full()), info).map_err(String::from)
}

/// zip 強制更新（brew 失敗時のフォールバック用。macOS では配布系統を問わず zip で更新する）
pub fn perform_update_zip(info: &UpdateInfo) -> Result<Applied, String> {
    run_route(zip_route(detect_install_method_full()), info).map_err(String::from)
}

fn run_route(route: ApplyRoute, info: &UpdateInfo) -> Result<Applied, UpdateRefusal> {
    let open_bundle = |note: UpdateNote| Applied {
        message: note.to_string(),
        restart: Restart::OpenBundle,
    };
    match route {
        ApplyRoute::Homebrew => update_via_homebrew(info).map(open_bundle),
        ApplyRoute::MacZip => update_via_zip(info).map(open_bundle),
        ApplyRoute::WindowsInstaller => update_via_installer(info),
        ApplyRoute::Refuse(method) => Err(refusal(method, info)),
    }
}

/// 適用できない配布系統の断り
fn refusal(method: InstallMethod, info: &UpdateInfo) -> UpdateRefusal {
    let release_page = info.html_url.clone();
    match method {
        InstallMethod::WindowsPortable => UpdateRefusal::PortableNotUpdatable { release_page },
        InstallMethod::WindowsInstaller => UpdateRefusal::NoZipRouteOnWindows { release_page },
        other => UpdateRefusal::MethodCannotUpdate(other),
    }
}

/// Windows の腕（#2434）: インストーラーを取得元・サイズ・sha256・PE ヘッダで検査する。
/// **ここではまだ起動しない**（起動は layout を保存してから `restart_app` が行う）。
/// 失敗したら何も上書きしていない今の版のまま、理由とリリースページを返す
fn update_via_installer(info: &UpdateInfo) -> Result<Applied, UpdateRefusal> {
    let fail = |error: installer_update::InstallerError| UpdateRefusal::InstallerRefused {
        error,
        release_page: info.html_url.clone(),
    };
    let (Some(url), Some(asset_name)) = (info.download_url.as_deref(), info.asset_name.as_deref())
    else {
        return Err(UpdateRefusal::NoWindowsInstaller {
            release_page: info.html_url.clone(),
        });
    };
    installer_update::check_origin(url, OWNER_REPO, asset_name).map_err(fail)?;
    let expected = installer_update::Expected {
        asset_name,
        url,
        size: info.asset_size,
        digest: info.asset_digest.as_deref(),
    };
    // 検査できないものは置き場を作る前に断る（一時 dir に何も残さない）
    installer_update::checkable(&expected).map_err(fail)?;
    let dir =
        installer_update::fresh_attempt_dir(&installer_update::staging_root()).map_err(fail)?;
    let prepared = installer_update::download(&expected, &dir, &format!("tako/{CURRENT_VERSION}"))
        .map_err(fail)?;
    Ok(Applied {
        message: UpdateNote::InstallerReady {
            version: info.version.clone(),
        }
        .to_string(),
        restart: Restart::RunInstaller(prepared),
    })
}

fn update_via_homebrew(info: &UpdateInfo) -> Result<UpdateNote, UpdateRefusal> {
    // #586: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
            .args(["upgrade", "--cask", "takushio2525/tako/tako"])
            .output()
            .map_err(|e| UpdateRefusal::BrewNotRun(e.to_string()))?;
    if output.status.success() {
        Ok(UpdateNote::HomebrewDone)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already installed") {
            Ok(UpdateNote::AlreadyLatest)
        } else {
            // brew 失敗時は zip フォールバック可能であることを含めてエラーを返す（#50）
            Err(UpdateRefusal::BrewUpgradeFailed {
                stderr: stderr.into_owned(),
                zip_fallback: info.download_url.is_some(),
            })
        }
    }
}

fn update_via_zip(info: &UpdateInfo) -> Result<UpdateNote, UpdateRefusal> {
    let url = info
        .download_url
        .as_deref()
        .ok_or(UpdateRefusal::NoZipAsset)?;

    let tmp_dir = std::env::temp_dir().join("tako-update");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| UpdateRefusal::TempDirFailed(e.to_string()))?;
    let zip_path = tmp_dir.join("tako.zip");

    let mut body = ureq::get(url)
        .header("User-Agent", &format!("tako/{CURRENT_VERSION}"))
        .call()
        .map_err(|e| UpdateRefusal::DownloadFailed(e.to_string()))?
        .into_body();
    let mut file = std::fs::File::create(&zip_path)
        .map_err(|e| UpdateRefusal::ZipCreateFailed(e.to_string()))?;
    std::io::copy(&mut body.as_reader(), &mut file)
        .map_err(|e| UpdateRefusal::DownloadWriteFailed(e.to_string()))?;
    drop(file);

    let output = std::process::Command::new("ditto")
        .args([
            "-xk",
            &zip_path.to_string_lossy(),
            &tmp_dir.to_string_lossy(),
        ])
        .output()
        .map_err(|e| UpdateRefusal::DittoNotRun(e.to_string()))?;
    if !output.status.success() {
        return Err(UpdateRefusal::UnzipFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let extracted_app = find_app_bundle(&tmp_dir).ok_or(UpdateRefusal::NoAppInZip)?;

    // 置き場のパスを一度も空けずに差し替える（#1042）。
    // 「退避 → 新規コピー → 退避先を削除」にすると、Dock のピン（.app への file URL
    // ブックマーク）が退避先へ追従したところでその実体を消してしまい、ピンが外れる
    let dest = Path::new(APP_INSTALL_PATH);
    let strategy =
        tako_core::platform::bundle_install::replace_bundle_in_place(dest, &extracted_app)
            .map_err(UpdateRefusal::BundleReplace)?;

    let _ = std::fs::remove_dir_all(&tmp_dir);
    // 旧実装が残した退避（更新の途中で落ちた世代）が在れば片付ける
    let _ = std::fs::remove_dir_all(Path::new("/Applications/tako.app.bak"));

    // 手段の語（contents-swap / swap / fresh / move-aside）を必ず載せる。
    // dispatch の `message` からそのまま読めるので、CLI / MCP でも何が起きたか分かる（#1042）。
    // swap が使えない環境は、更新自体は済んでいるので成功だが、Dock のピンが外れうることは伏せない
    Ok(UpdateNote::ZipDone {
        strategy: strategy.label(),
        pin_may_drop: !strategy.keeps_path_occupied(),
    })
}

/// ディレクトリ内の *.app バンドルを再帰的に探す
fn find_app_bundle(dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "app") && path.is_dir() {
                return Some(path);
            }
            if path.is_dir() {
                if let Some(found) = find_app_bundle(&path) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// 適用の後に起動し直す。呼び出し元は成功したらすぐ `cx.quit()` で抜ける想定。
///
/// - macOS: `open -n` でバンドルを新プロセスとして起動し、自分は終了する
/// - Windows（#2434）: 検査済みのインストーラーを無言で起こす。インストーラーは
///   この tako の pid が消えるのを待ってから上書きし、終わったら tako を起こし直す。
///   起動できなければ `Err` = 何も上書きしていないので今の版のまま動き続ける
pub fn restart_app(restart: &Restart) -> Result<(), String> {
    match restart {
        Restart::OpenBundle => {
            let bundle = app_bundle_path().ok_or(UpdateRefusal::NoAppBundle)?;
            tako_control::platform::os_integration::open_new_instance(&bundle)
                .map_err(|e| UpdateRefusal::RestartFailed(e.to_string()))?;
            Ok(())
        }
        Restart::RunInstaller(prepared) => {
            let relaunch = installer_update::relaunch_target().map_err(UpdateRefusal::Installer)?;
            // 子は待たない（tako はこの後すぐ抜け、インストーラーが残って仕事をする）
            let child = installer_update::launch(prepared, std::process::id(), &relaunch)
                .map_err(UpdateRefusal::InstallerNotLaunched)?;
            tako_control::diag::persist_log(&format!(
                "更新: インストーラーを起動した（pid {}・sha256 {}・ログ {}）。\
                 tako を終了し、インストール後にインストーラーが起動し直す（#2434）",
                child.id(),
                prepared.sha256,
                prepared.log_path.display()
            ));
            Ok(())
        }
    }
}

/// dispatch 層に公開する更新情報の JSON 表現（broken-brew 診断を含む）
pub fn update_status_json() -> serde_json::Value {
    let method = detect_install_method_full();
    let duplicates = detect_duplicate_cli();
    let current = effective_current_version();
    // `-win.N` は Windows の正規配布なので stable 扱い（#723）
    let current_channel = if current.contains("-test.") {
        Channel::Test
    } else {
        Channel::Stable
    };
    let env = TargetEnv::current();
    let mut json = serde_json::json!({
        "current_version": current,
        "current_channel": current_channel.label(),
        "install_method": method.label(),
        "duplicate_cli": duplicates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        // 更新候補のフィルタ条件（#595）。「なぜ更新が出ないのか」を診断できるようにする
        "platform": env.platform.as_str(),
        "arch": env.arch.map(Arch::as_str),
        "asset_pattern": env.arch.map(|a| release_assets::asset_name(env.platform, a, "<tag>")),
    });
    if let Some(diag) = diagnose_broken_brew() {
        json["broken_brew"] = serde_json::json!({
            "app_path": diag.app_path,
            "brew_available": diag.brew_available,
            "cask_registered": diag.cask_registered,
            "repair_command": diag.repair_command,
            "hint": "brew の cask 台帳と .app 実体が乖離しています。\
                     `tako update repair` で台帳を再締結するか、\
                     `tako update apply-zip` で zip 経由の更新に切り替えてください",
        });
    }
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("invalid", "0.2.0"));
    }

    #[test]
    fn test_is_newer_with_test_suffix() {
        // テスト版 < 同ベースの安定版
        assert!(is_newer("0.6.0", "0.6.0-test.1"));
        assert!(is_newer("0.6.0", "0.6.0-test.99"));
        // テスト版同士
        assert!(is_newer("0.6.0-test.2", "0.6.0-test.1"));
        assert!(!is_newer("0.6.0-test.1", "0.6.0-test.2"));
        assert!(!is_newer("0.6.0-test.1", "0.6.0-test.1"));
        // 異なるベース
        assert!(is_newer("0.7.0-test.1", "0.6.0"));
        assert!(!is_newer("0.5.0-test.1", "0.6.0"));
        // テスト版 vs 旧安定版
        assert!(is_newer("0.6.0-test.1", "0.5.8"));
    }

    #[test]
    fn test_parsed_version() {
        let v = ParsedVersion::parse("0.6.0").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 6);
        assert_eq!(v.patch, 0);
        assert_eq!(v.test_num, None);
        assert_eq!(v.channel(), Channel::Stable);

        let v = ParsedVersion::parse("0.6.0-test.3").unwrap();
        assert_eq!(v.test_num, Some(3));
        assert_eq!(v.channel(), Channel::Test);

        assert!(ParsedVersion::parse("invalid").is_none());
        assert!(ParsedVersion::parse("0.6.0-rc.1").is_none());
    }

    #[test]
    fn test_channel_label() {
        assert_eq!(Channel::Stable.label(), "stable");
        assert_eq!(Channel::Test.label(), "test");
        // 表示ラベルは言語カタログ依存（#435）。**相対比較でも言語を固定しないと
        // 落ちる**ので、比較は言語が固定された 1 区間の中で行う（#1274）
        crate::ui_text::tests_support::for_each_lang(|| {
            assert_eq!(
                Channel::Stable.display_label(),
                crate::ui_text::update::channel_stable()
            );
            assert_eq!(
                Channel::Test.display_label(),
                crate::ui_text::update::channel_test()
            );
        });
    }

    #[test]
    fn test_channel_from_str() {
        assert_eq!("stable".parse::<Channel>().unwrap(), Channel::Stable);
        assert_eq!("test".parse::<Channel>().unwrap(), Channel::Test);
        assert!("unknown".parse::<Channel>().is_err());
    }

    #[test]
    fn test_detect_install_method_returns_value() {
        let method = detect_install_method();
        // Windows はインストーラー / ポータブルの 2 系統、それ以外は macOS の 3 系統（#2434）
        let expected: &[InstallMethod] = if Platform::current() == Platform::Windows {
            &[
                InstallMethod::WindowsInstaller,
                InstallMethod::WindowsPortable,
            ]
        } else {
            &[
                InstallMethod::Homebrew,
                InstallMethod::Zip,
                InstallMethod::BrokenBrew,
            ]
        };
        assert!(
            expected.contains(&method),
            "{method:?} はこのプラットフォームの配布系統 {expected:?} に無い"
        );
    }

    #[test]
    fn test_install_method_label() {
        assert_eq!(InstallMethod::Homebrew.label(), "homebrew");
        assert_eq!(InstallMethod::Zip.label(), "zip");
        assert_eq!(InstallMethod::BrokenBrew.label(), "broken-brew");
    }

    #[test]
    fn test_detect_duplicate_cli_runs() {
        let _ = detect_duplicate_cli();
    }

    #[test]
    fn test_update_status_json() {
        let json = update_status_json();
        assert!(json.get("current_version").is_some());
        assert!(json.get("current_channel").is_some());
        assert!(json.get("install_method").is_some());
        assert!(json.get("duplicate_cli").is_some());
        // CLI / MCP が読む版数は「いま入っている版数」でなければならない（#723 / 開発不変条件 5）。
        // ビルド埋め込みを出すと、インストーラー経由の `-win.N` で AI と画面が食い違う
        assert_eq!(
            json["current_version"].as_str(),
            Some(effective_current_version()),
            "status JSON の current_version が effective_current_version と一致しない"
        );
    }

    // --- CheckError ---

    #[test]
    fn test_check_error_display() {
        for lang in [Lang::Ja, Lang::En] {
            let e = CheckError::Network(NetworkCause::Transport("connection refused".into()));
            assert!(e.text_in(lang).contains("connection refused"));

            let e = CheckError::Parse(ParseCause::Json("bad json".into()));
            assert!(e.text_in(lang).contains("bad json"));
        }
        let e = CheckError::RateLimit { retry_after: None };
        assert!(e.text_in(Lang::Ja).contains("レート制限"));
        assert!(e.text_in(Lang::En).contains("rate limit"));
    }

    #[test]
    fn test_check_error_to_json() {
        let e = CheckError::RateLimit {
            retry_after: Some(1234567890),
        };
        let json = e.to_json();
        assert_eq!(json["type"], "rate_limit");
        assert_eq!(json["retry_after"], 1234567890);

        let e = CheckError::Network(NetworkCause::Transport("timeout".into()));
        let json = e.to_json();
        assert_eq!(json["type"], "network");
        assert_eq!(json["message"], "timeout");
        // tako が書く中身は表示言語に従う（#2365。`message` は中身だけ = 従来の形）
        let e = CheckError::Network(NetworkCause::UnexpectedStatus(500));
        assert_eq!(
            e.to_json_in(Lang::Ja)["message"],
            "予期しないステータスコード: 500"
        );
        assert_eq!(
            e.to_json_in(Lang::En)["message"],
            "unexpected status code: 500"
        );
    }

    #[test]
    fn test_check_error_retry_duration() {
        let e = CheckError::Network(NetworkCause::Transport("timeout".into()));
        assert_eq!(e.retry_duration(), RETRY_INTERVAL);

        let e = CheckError::RateLimit { retry_after: None };
        assert_eq!(e.retry_duration(), RETRY_INTERVAL);
    }

    #[test]
    fn test_format_reset_time() {
        // 遠い未来の timestamp → 「約N分後」形式（言語は引数で渡す = グローバルに触らない。#2365）
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300;
        let ja = reset_eta_in(future, Lang::Ja);
        assert!(ja == "約4分後" || ja == "約5分後", "{ja:?}");
        let en = reset_eta_in(future, Lang::En);
        assert!(en == "in ~4 min" || en == "in ~5 min", "{en:?}");
        // 1 分未満は秒
        let near = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 30;
        assert!(reset_eta_in(near, Lang::Ja).ends_with("秒後"));
        assert!(reset_eta_in(near, Lang::En).ends_with('s'));
        // 過去の timestamp → 「まもなく」
        assert_eq!(reset_eta_in(0, Lang::Ja), "まもなく");
        assert_eq!(reset_eta_in(0, Lang::En), "soon");
    }

    // --- broken-brew 判定ロジックの単体テスト（サブプロセス不要） ---

    #[test]
    fn test_inner_caskroom_detected_as_homebrew() {
        assert_eq!(
            detect_install_method_inner(true, false),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect_install_method_inner(false, true),
            InstallMethod::Homebrew
        );
        assert_eq!(
            detect_install_method_inner(true, true),
            InstallMethod::Homebrew
        );
    }

    #[test]
    fn test_inner_no_caskroom_detected_as_zip() {
        assert_eq!(
            detect_install_method_inner(false, false),
            InstallMethod::Zip
        );
    }

    #[test]
    fn test_broken_brew_detection_logic() {
        struct Case {
            app_in_caskroom: bool,
            app_exists: bool,
            brew_available: bool,
            cask_registered: bool,
            expected: InstallMethod,
        }
        let cases = [
            Case {
                app_in_caskroom: true,
                app_exists: true,
                brew_available: true,
                cask_registered: true,
                expected: InstallMethod::Homebrew,
            },
            Case {
                app_in_caskroom: false,
                app_exists: true,
                brew_available: false,
                cask_registered: false,
                expected: InstallMethod::Zip,
            },
            Case {
                app_in_caskroom: false,
                app_exists: false,
                brew_available: true,
                cask_registered: false,
                expected: InstallMethod::Zip,
            },
            Case {
                app_in_caskroom: false,
                app_exists: true,
                brew_available: true,
                cask_registered: false,
                expected: InstallMethod::BrokenBrew,
            },
            Case {
                app_in_caskroom: false,
                app_exists: true,
                brew_available: true,
                cask_registered: true,
                expected: InstallMethod::Zip,
            },
        ];
        for (i, c) in cases.iter().enumerate() {
            let fast = detect_install_method_inner(c.app_in_caskroom, false);
            let result = if fast != InstallMethod::Zip {
                fast
            } else if c.app_exists && c.brew_available && !c.cask_registered {
                InstallMethod::BrokenBrew
            } else {
                InstallMethod::Zip
            };
            assert_eq!(
                result, c.expected,
                "case {i}: expected {:?}, got {:?}",
                c.expected, result
            );
        }
    }

    #[test]
    fn test_repair_brew_rejects_non_broken() {
        let result = repair_brew_typed();
        let method = detect_install_method_full();
        if method != InstallMethod::BrokenBrew {
            let err = result.expect_err("壊れていない配布系統は修復しない");
            assert!(
                matches!(err, UpdateRefusal::RepairNotNeeded(m) if m == method),
                "{err:?}"
            );
            assert!(err.text_in(Lang::Ja).contains("修復は不要"));
            assert!(err.text_in(Lang::En).contains("no repair is needed"));
        }
    }

    // --- #1253: テストプロセスから brew を起こさない ---

    /// 子を名指しするときの目印（無ければ子の本体は走らない）
    const BREW_PROBE_ENV: &str = "TAKO_1253_BREW_PROBE";

    /// 子のテスト名（`--exact` で名指しするので、改名したらここも直す）
    const BREW_PROBE_CHILD: &str = "update_checker::tests::子プロセス_配布系統を判別する";

    /// 親から起こされたときだけ配布系統の判別を通す（本番なら brew を起こす経路）
    #[test]
    fn 子プロセス_配布系統を判別する() {
        if std::env::var_os(BREW_PROBE_ENV).is_none() {
            return;
        }
        let _ = detect_install_method_full();
    }

    /// **テストプロセスが `brew` を起こさない**ことを実測する（#1253）。
    ///
    /// 起きているかどうかは PATH の先頭へ置いた**偽 brew**が目印ファイルを
    /// 作るかで見る。`~/Library/Caches/Homebrew` の増減で見ると、キャッシュが
    /// 温まっている間は起きても増えない（実測で 1 度そう誤った）
    #[test]
    #[cfg(target_os = "macos")]
    fn テストプロセスはbrewを起こさない() {
        let dir = std::env::temp_dir().join(format!(
            "tako-1253-brew-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        let marker = dir.join("called");
        let fake = dir.join("brew");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", marker.display()),
        )
        .expect("偽 brew を書ける");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                .expect("偽 brew に実行権を付けられる");
        }

        let run = |legacy: bool| -> (bool, String) {
            let _ = std::fs::remove_file(&marker);
            let exe = std::env::current_exe().expect("テストバイナリのパス");
            let path = format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            );
            let mut cmd = std::process::Command::new(&exe);
            cmd.args(["--exact", BREW_PROBE_CHILD, "--test-threads=1"])
                .env(BREW_PROBE_ENV, "1")
                .env("PATH", path);
            if legacy {
                cmd.env("TAKO_944_LEGACY", "1");
            } else {
                cmd.env_remove("TAKO_944_LEGACY");
            }
            let out = cmd.output().expect("子テストプロセスを起こせる");
            let mut log = String::from_utf8_lossy(&out.stdout).to_string();
            log.push_str(&String::from_utf8_lossy(&out.stderr));
            assert!(
                log.contains("1 passed"),
                "子テストが実行されていない\n{log}"
            );
            (
                marker.is_file(),
                std::fs::read_to_string(&marker).unwrap_or_default(),
            )
        };

        let (called, calls) = run(false);
        assert!(
            !called,
            "テストプロセスが brew を起こした（#1253）: {calls}"
        );

        // A/B: 門番を切る（`TAKO_944_LEGACY=1`）と同じ子が brew を起こす。
        // **`/Applications/tako.app` が無い機械では経路自体に入らない**ので、
        // 検出力の実証はそのときだけ黙って落とす（測れていないことを明示する）
        if applications_tako_app_exists() {
            let (called, calls) = run(true);
            assert!(
                called,
                "旧挙動でも brew が起きない（検査に検出力が無い）: {calls}"
            );
        } else {
            eprintln!(
                "skip(A/B のみ): /Applications/tako.app が無い機械では \
                 detect_install_method_full が brew の経路へ入らない"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #416: parse_releases / gh トークン / キャッシュ ---

    /// リリース 1 件分の fixture。`assets` はアセット名の一覧から組み立てる
    fn release(tag: &str, prerelease: bool, assets: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "prerelease": prerelease,
            "html_url": format!("https://github.com/{OWNER_REPO}/releases/tag/{tag}"),
            "assets": assets.iter().map(|name| serde_json::json!({
                "name": name,
                "browser_download_url":
                    format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/{name}"),
            })).collect::<Vec<_>>(),
        })
    }

    const MAC: TargetEnv = TargetEnv {
        platform: Platform::MacOs,
        arch: Some(Arch::Arm64),
    };
    const WIN: TargetEnv = TargetEnv {
        platform: Platform::Windows,
        arch: Some(Arch::X86_64),
    };

    #[test]
    fn test_parse_releases_both_channels() {
        let arr = vec![
            release(
                "v99.0.1-test.1",
                true,
                &["tako-v99.0.1-test.1-macos-arm64.zip"],
            ),
            release("v99.0.0", false, &["tako-v99.0.0-macos-arm64.zip"]),
        ];
        let result = parse_releases_for(&arr, MAC, "0.5.13");
        assert_eq!(result.stable.as_ref().unwrap().version, "99.0.0");
        assert_eq!(result.test.as_ref().unwrap().version, "99.0.1-test.1");
        assert!(result.rate_limit_note.is_none());
        // 実アセットの URL をそのまま使う（合成 URL ではない）
        assert_eq!(
            result.stable.as_ref().unwrap().download_url.as_deref(),
            Some("https://github.com/takushio2525/tako/releases/download/v99.0.0/tako-v99.0.0-macos-arm64.zip")
        );
        assert_eq!(
            result.stable.as_ref().unwrap().asset_name.as_deref(),
            Some("tako-v99.0.0-macos-arm64.zip")
        );
    }

    #[test]
    fn test_parse_releases_skips_older() {
        let arr = vec![release("v0.0.1", false, &["tako-v0.0.1-macos-arm64.zip"])];
        let result = parse_releases_for(&arr, MAC, "0.5.13");
        assert!(result.stable.is_none());
        assert!(result.test.is_none());
    }

    #[test]
    fn test_parse_releases_invalid_tag() {
        let arr = vec![release("nightly", false, &["tako-nightly-macos-arm64.zip"])];
        let result = parse_releases_for(&arr, MAC, "0.5.13");
        assert!(result.stable.is_none());
    }

    // --- #723: `-win.N`（Windows のプレビュー反復）の版数解決 ---
    //
    // Windows は Cargo バージョンを据え置いたまま `-win.1` `-win.2` … と配布を回す。
    // 版数の出所が「ビルド埋め込み」だけだと配布済みの win.1〜3 が一律 X.Y.Z を名乗り、
    // 「最新は win.3」と比べて永遠に更新ありになる（= 更新しても同じ版が入る無限ループ）。

    #[test]
    fn build_script_emits_full_version() {
        // build.rs が TAKO_FULL_VERSION を emit していること。TAKO_WIN_NUM 無しのビルドでは
        // CARGO_PKG_VERSION と一致し、付きのビルドでは `-win.N` が後ろに付く
        assert!(
            CURRENT_VERSION == env!("CARGO_PKG_VERSION")
                || CURRENT_VERSION.starts_with(concat!(env!("CARGO_PKG_VERSION"), "-win.")),
            "TAKO_FULL_VERSION が CARGO_PKG_VERSION から乖離している: {CURRENT_VERSION}"
        );
        // 更新判定に使う版数は必ず解析できること（できないと比較が全部スキップされる）
        assert!(
            ParsedVersion::parse(effective_current_version()).is_some(),
            "effective_current_version が解析できない: {}",
            effective_current_version()
        );
    }

    #[test]
    fn parsed_version_reads_win_suffix() {
        let v = ParsedVersion::parse("0.5.13-win.3").expect("-win.N を解析できること");
        assert_eq!((v.major, v.minor, v.patch), (0, 5, 13));
        assert_eq!(v.win_num, Some(3));
        assert_eq!(v.test_num, None);
        // 数字でない / 空の連番は不正
        assert!(ParsedVersion::parse("0.5.13-win.").is_none());
        assert!(ParsedVersion::parse("0.5.13-win.x").is_none());
        // `-win.N` 以外のプレリリースは従来どおり解析しない
        assert!(ParsedVersion::parse("0.5.13-rc.1").is_none());
    }

    #[test]
    fn win_suffix_does_not_change_ordering_of_existing_versions() {
        // suffix_rank 導入で `-win.N` を持たない版同士の順序が変わっていないこと
        assert!(is_newer("0.6.0", "0.5.13"));
        assert!(is_newer("0.6.0", "0.6.0-test.1"));
        assert!(is_newer("0.6.0-test.2", "0.6.0-test.1"));
        assert!(!is_newer("0.6.0-test.1", "0.6.0"));
        assert!(!is_newer("0.5.13", "0.6.0"));
        // semver 準拠では `-win.N` は同ベースの stable より古い
        assert!(is_newer("0.5.13", "0.5.13-win.3"));
        assert!(!is_newer("0.5.13-win.3", "0.5.13"));
    }

    #[test]
    fn resolve_current_version_prefers_installer_record() {
        // インストーラーの記録が正（v 接頭辞は落とす）
        assert_eq!(
            resolve_current_version(Some("v0.5.13-win.3"), "0.5.13"),
            "0.5.13-win.3"
        );
        assert_eq!(
            resolve_current_version(Some("0.5.13-win.2"), "0.5.13"),
            "0.5.13-win.2"
        );
        // 前後の空白は落とす（レジストリ値は空白詰めのことがある）
        assert_eq!(
            resolve_current_version(Some("  v0.6.0  "), "0.5.13"),
            "0.6.0"
        );
    }

    #[test]
    fn resolve_current_version_falls_back() {
        // 記録が無い（ポータブル zip / 開発ビルド / macOS）
        assert_eq!(
            resolve_current_version(None, "0.5.13-win.4"),
            "0.5.13-win.4"
        );
        // 記録が読めない値ならビルド埋め込みを使う（手で書き換えられた等）
        assert_eq!(resolve_current_version(Some("garbage"), "0.5.13"), "0.5.13");
        assert_eq!(resolve_current_version(Some(""), "0.5.13"), "0.5.13");
        assert_eq!(
            resolve_current_version(Some("v0.6.0-rc.1"), "0.5.13"),
            "0.5.13"
        );
    }

    #[test]
    fn installed_win3_sees_no_update_for_win3() {
        // #723 の無限ループ回帰テスト: インストーラーが win.3 を記録していれば
        // 最新が win.3 でも「更新あり」にしない
        let arr = vec![release(
            "v0.5.13-win.3",
            false,
            &["tako-v0.5.13-win.3-windows-x86_64.exe"],
        )];
        let current = resolve_current_version(Some("v0.5.13-win.3"), "0.5.13");
        assert!(
            parse_releases_for(&arr, WIN, &current).stable.is_none(),
            "win.3 を入れているのに win.3 が更新として出た（無限ループ）"
        );

        // 記録が無いビルド埋め込みだけの場合は「更新あり」になる（既存 win.1〜3 の救済経路）。
        // semver 順では win.3 < 0.5.13 だが、Windows ではここを新しい扱いにする
        let fallback = resolve_current_version(None, "0.5.13");
        assert_eq!(
            parse_releases_for(&arr, WIN, &fallback)
                .stable
                .map(|i| i.version),
            Some("0.5.13-win.3".into())
        );

        // win.2 を入れていれば win.3 は更新（連番順）
        assert_eq!(
            parse_releases_for(&arr, WIN, "0.5.13-win.2")
                .stable
                .map(|i| i.version),
            Some("0.5.13-win.3".into())
        );
        // win.4 を入れていれば win.3 は更新ではない
        assert!(parse_releases_for(&arr, WIN, "0.5.13-win.4")
            .stable
            .is_none());
    }

    #[test]
    fn win_release_is_never_offered_on_macos() {
        // `-win.N` は Windows 向けのアセットしか持たない。仮に macOS の zip が
        // 付いていたとしても、版数比較の段で macOS には出さない（二重の防御）
        let arr = vec![release(
            "v0.5.13-win.3",
            false,
            &[
                "tako-v0.5.13-win.3-windows-x86_64.exe",
                "tako-v0.5.13-win.3-macos-arm64.zip",
            ],
        )];
        let updates = parse_releases_for(&arr, MAC, "0.5.13");
        assert!(updates.stable.is_none(), "macOS で -win.N は表示しない");
        assert!(updates.test.is_none(), "macOS で -win.N は表示しない");
    }

    // --- #595: プラットフォームフィルタ ---

    /// 受け入れ条件 1: mac のみのリリースを Windows クライアントがスキップする
    #[test]
    fn test_windows_skips_mac_only_release() {
        let arr = vec![release("v0.6.0", false, &["tako-v0.6.0-macos-arm64.zip"])];

        // macOS からは更新として見える
        assert_eq!(
            parse_releases_for(&arr, MAC, "0.5.13")
                .stable
                .map(|i| i.version),
            Some("0.6.0".into())
        );
        // Windows からは「自分向けの配布物が無い」ので更新候補にならない
        assert!(parse_releases_for(&arr, WIN, "0.5.13").stable.is_none());
    }

    /// 受け入れ条件 1: 両 OS 対応リリースは双方が更新候補にする
    #[test]
    fn test_both_platforms_release_visible_to_both() {
        let arr = vec![release(
            "v0.6.0",
            false,
            &[
                "tako-v0.6.0-macos-arm64.zip",
                "tako-v0.6.0-windows-x86_64.exe",
            ],
        )];
        let mac = parse_releases_for(&arr, MAC, "0.5.13").stable.unwrap();
        let win = parse_releases_for(&arr, WIN, "0.5.13").stable.unwrap();
        assert_eq!(mac.version, "0.6.0");
        assert_eq!(win.version, "0.6.0");
        // 掴むアセットが OS ごとに正しく分かれている（相互に取り違えない）
        assert_eq!(
            mac.asset_name.as_deref(),
            Some("tako-v0.6.0-macos-arm64.zip")
        );
        assert_eq!(
            win.asset_name.as_deref(),
            Some("tako-v0.6.0-windows-x86_64.exe")
        );
    }

    /// 受け入れ条件 1: アセット後付け（`gh release upload`）で初めて更新として見える
    #[test]
    fn test_windows_asset_added_later_becomes_visible() {
        let before = vec![release("v0.6.0", false, &["tako-v0.6.0-macos-arm64.zip"])];
        assert!(parse_releases_for(&before, WIN, "0.5.13").stable.is_none());

        // 同じタグに Windows アセットを追加した後
        let after = vec![release(
            "v0.6.0",
            false,
            &[
                "tako-v0.6.0-macos-arm64.zip",
                "tako-v0.6.0-windows-x86_64.exe",
            ],
        )];
        assert_eq!(
            parse_releases_for(&after, WIN, "0.5.13")
                .stable
                .map(|i| i.version),
            Some("0.6.0".into())
        );
        // macOS 側の見え方は前後で変わらない（後付けが既存ユーザーに影響しない）
        assert_eq!(
            parse_releases_for(&before, MAC, "0.5.13")
                .stable
                .map(|i| i.version),
            parse_releases_for(&after, MAC, "0.5.13")
                .stable
                .map(|i| i.version),
        );
    }

    /// エッジケース: 最新リリースに自 OS アセットが無く、一つ前にはある
    #[test]
    fn test_falls_back_to_older_release_with_matching_asset() {
        let arr = vec![
            release("v0.6.2", false, &["tako-v0.6.2-macos-arm64.zip"]),
            release("v0.6.1", false, &["tako-v0.6.1-macos-arm64.zip"]),
            release(
                "v0.6.0",
                false,
                &[
                    "tako-v0.6.0-macos-arm64.zip",
                    "tako-v0.6.0-windows-x86_64.exe",
                ],
            ),
        ];
        // macOS は最新の 0.6.2
        assert_eq!(
            parse_releases_for(&arr, MAC, "0.5.13")
                .stable
                .map(|i| i.version),
            Some("0.6.2".into())
        );
        // Windows は 2 つ読み飛ばして 0.6.0 に落ちる（更新できないバージョンを掴まない）
        let win = parse_releases_for(&arr, WIN, "0.5.13").stable.unwrap();
        assert_eq!(win.version, "0.6.0");
        assert_eq!(
            win.asset_name.as_deref(),
            Some("tako-v0.6.0-windows-x86_64.exe")
        );
    }

    /// エッジケース: 命名規則外のアセットが混ざっていても掴まない
    #[test]
    fn test_irregular_asset_names_are_not_matched() {
        let arr = vec![release(
            "v0.6.0",
            false,
            &[
                "checksums.txt",
                "tako.zip",
                "tako-v0.6.0-macos.zip",        // arch 欠落
                "tako-v0.6.0-linux-x86_64.zip", // 対象外 OS
                "SOURCE.tar.gz",
            ],
        )];
        assert!(parse_releases_for(&arr, MAC, "0.5.13").stable.is_none());
        assert!(parse_releases_for(&arr, WIN, "0.5.13").stable.is_none());

        // 規則に沿ったものが 1 つでもあれば掴む
        let arr = vec![release(
            "v0.6.0",
            false,
            &["checksums.txt", "tako-v0.6.0-macos-arm64.zip"],
        )];
        assert_eq!(
            parse_releases_for(&arr, MAC, "0.5.13")
                .stable
                .map(|i| i.version),
            Some("0.6.0".into())
        );
    }

    /// 受け入れ条件 3: チャンネル制（#403）との組み合わせ。
    /// Windows アセットが test 版だけに付いている状況で、stable / test が独立に解決される
    #[test]
    fn test_platform_filter_combines_with_channels() {
        let arr = vec![
            release(
                "v0.7.0-test.1",
                true,
                &[
                    "tako-v0.7.0-test.1-macos-arm64.zip",
                    "tako-v0.7.0-test.1-windows-x86_64.exe",
                ],
            ),
            release("v0.6.0", false, &["tako-v0.6.0-macos-arm64.zip"]),
            release(
                "v0.5.14",
                false,
                &[
                    "tako-v0.5.14-macos-arm64.zip",
                    "tako-v0.5.14-windows-x86_64.exe",
                ],
            ),
        ];
        // macOS: 素直に最新同士
        let mac = parse_releases_for(&arr, MAC, "0.5.13");
        assert_eq!(mac.stable.map(|i| i.version), Some("0.6.0".into()));
        assert_eq!(mac.test.map(|i| i.version), Some("0.7.0-test.1".into()));

        // Windows: test は 0.7.0-test.1、stable は 0.6.0 を飛ばして 0.5.14
        let win = parse_releases_for(&arr, WIN, "0.5.13");
        assert_eq!(win.stable.map(|i| i.version), Some("0.5.14".into()));
        assert_eq!(win.test.map(|i| i.version), Some("0.7.0-test.1".into()));
    }

    /// 受け入れ条件 2: 現行リリース群（macOS zip のみ）に対する macOS の判定が不変。
    /// 実際に配布済みのアセット名・prerelease フラグをそのまま fixture にしている
    #[test]
    fn test_macos_judgement_unchanged_on_current_releases() {
        let shipped: Vec<serde_json::Value> = [
            ("v0.5.13", true),
            ("v0.5.12", true),
            ("v0.5.11", true),
            ("v0.5.10", true),
            ("v0.5.9", false),
            ("v0.5.8", false),
            ("v0.5.7", false),
        ]
        .iter()
        .map(|(tag, pre)| release(tag, *pre, &[&format!("tako-{tag}-macos-arm64.zip")]))
        .collect();

        // v0.5.7 利用者から見た判定（修正前と同じ: stable=0.5.9 / test=0.5.13）
        let r = parse_releases_for(&shipped, MAC, "0.5.7");
        assert_eq!(
            r.stable.as_ref().map(|i| i.version.clone()),
            Some("0.5.9".into())
        );
        assert_eq!(
            r.test.as_ref().map(|i| i.version.clone()),
            Some("0.5.13".into())
        );
        assert_eq!(
            r.stable.as_ref().unwrap().download_url.as_deref(),
            Some("https://github.com/takushio2525/tako/releases/download/v0.5.9/tako-v0.5.9-macos-arm64.zip")
        );

        // 最新利用者には更新なし
        let r = parse_releases_for(&shipped, MAC, "0.5.13");
        assert!(r.stable.is_none() && r.test.is_none());

        // 現行リリース群には Windows 版が無い = Windows には何も出さない（#595 の主眼）
        let r = parse_releases_for(&shipped, WIN, "0.5.7");
        assert!(r.stable.is_none() && r.test.is_none());
    }

    /// 新旧比較用の判定結果（バージョン, ダウンロード URL）
    type Judgement = Option<(String, String)>;

    /// #595 修正前のアルゴリズム（assets を見ず URL を合成していた版）。
    /// 実リリース群に対して**新旧の判定が一致する**ことを示すためだけに残す
    fn parse_releases_before_595(
        releases: &[serde_json::Value],
        current_version: &str,
        arch: &str,
    ) -> (Judgement, Judgement) {
        let current = ParsedVersion::parse(current_version);
        let (mut stable, mut test) = (None, None);
        for release in releases {
            let tag = release["tag_name"].as_str().unwrap_or_default();
            let version_str = tag.strip_prefix('v').unwrap_or(tag);
            let Some(ver) = ParsedVersion::parse(version_str) else {
                continue;
            };
            let slot = if release["prerelease"].as_bool().unwrap_or(false) {
                &mut test
            } else {
                &mut stable
            };
            if let Some(ref cur) = current {
                if ver <= *cur {
                    continue;
                }
            }
            if slot.is_none() {
                *slot = Some((
                    version_str.to_string(),
                    format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/tako-{tag}-macos-{arch}.zip"),
                ));
            }
            if stable.is_some() && test.is_some() {
                break;
            }
        }
        (stable, test)
    }

    /// 受け入れ条件 2（実データ版）: **本番の実リリース一覧**に対して、
    /// macOS / arm64 の判定が #595 修正の前後で完全に一致する。
    ///
    /// fixture は `gh api repos/takushio2525/tako/releases` の実応答
    /// （testdata/releases_snapshot.json）。バージョンだけでなく URL まで一致を見る
    #[test]
    fn test_real_releases_macos_judgement_identical_to_before_595() {
        let raw = include_str!("../testdata/releases_snapshot.json");
        let releases: Vec<serde_json::Value> =
            serde_json::from_str(raw).expect("releases_snapshot.json");
        assert!(releases.len() > 20, "fixture が小さすぎる");

        // 実在する全バージョン + 現在バージョンを起点に総当たりで突き合わせる
        let mut versions: Vec<String> = releases
            .iter()
            .filter_map(|r| r["tag_name"].as_str())
            .map(|t| t.trim_start_matches('v').to_string())
            .collect();
        versions.push(CURRENT_VERSION.to_string());

        for v in &versions {
            if ParsedVersion::parse(v).is_none() {
                continue;
            }
            let (old_stable, old_test) = parse_releases_before_595(&releases, v, "arm64");
            let new = parse_releases_for(&releases, MAC, v);

            let as_pair = |i: &Option<UpdateInfo>| {
                i.as_ref().map(|i| {
                    (
                        i.version.clone(),
                        i.download_url.clone().unwrap_or_default(),
                    )
                })
            };
            assert_eq!(
                as_pair(&new.stable),
                old_stable,
                "stable が変わった（現在 {v}）"
            );
            assert_eq!(as_pair(&new.test), old_test, "test が変わった（現在 {v}）");
        }
    }

    /// #595 の主眼: 実リリース群には Windows 版が無いので Windows には何も出さない
    /// （修正前は mac の zip URL を掴んで「更新あり」と出ていた）
    #[test]
    fn test_real_releases_offer_nothing_to_windows() {
        let raw = include_str!("../testdata/releases_snapshot.json");
        let releases: Vec<serde_json::Value> =
            serde_json::from_str(raw).expect("releases_snapshot.json");

        let r = parse_releases_for(&releases, WIN, "0.0.1");
        assert!(
            r.stable.is_none() && r.test.is_none(),
            "Windows に更新候補が出ている: {:?} / {:?}",
            r.stable,
            r.test
        );
        // 修正前は同じ状況で「更新あり」と判定していた（対比）
        let (old_stable, _) = parse_releases_before_595(&releases, "0.0.1", "x86_64");
        assert!(old_stable.is_some(), "修正前の挙動の再現に失敗");
    }

    /// 対応する配布物が存在しないアーキテクチャでは更新候補を出さない
    #[test]
    fn test_unknown_arch_yields_no_update() {
        let arr = vec![release("v0.6.0", false, &["tako-v0.6.0-macos-arm64.zip"])];
        let env = TargetEnv {
            platform: Platform::MacOs,
            arch: None,
        };
        let r = parse_releases_for(&arr, env, "0.5.13");
        assert!(r.stable.is_none() && r.test.is_none());
    }

    /// browser_download_url が欠けていても命名規則から URL を復元する
    #[test]
    fn test_download_url_recovered_when_missing() {
        let arr = vec![serde_json::json!({
            "tag_name": "v0.6.0",
            "prerelease": false,
            "html_url": "",
            "assets": [{ "name": "tako-v0.6.0-macos-arm64.zip" }],
        })];
        let r = parse_releases_for(&arr, MAC, "0.5.13");
        assert_eq!(
            r.stable.unwrap().download_url.as_deref(),
            Some("https://github.com/takushio2525/tako/releases/download/v0.6.0/tako-v0.6.0-macos-arm64.zip")
        );
    }

    /// assets キーが無い（旧形式・取得漏れ）リリースは更新候補にしない
    #[test]
    fn test_release_without_assets_is_skipped() {
        let arr = vec![serde_json::json!({
            "tag_name": "v0.6.0", "prerelease": false, "html_url": "",
        })];
        assert!(parse_releases_for(&arr, MAC, "0.5.13").stable.is_none());
    }

    // --- #616: 更新通知カードのキー（バージョン単位の再表示抑止） ---

    fn updates_of(stable: Option<&str>, test: Option<&str>) -> ChannelUpdates {
        let info = |v: &str, ch: Channel| UpdateInfo {
            version: v.into(),
            channel: ch,
            html_url: String::new(),
            download_url: None,
            asset_name: None,
            notes: None,
            asset_size: None,
            asset_digest: None,
        };
        ChannelUpdates {
            stable: stable.map(|v| info(v, Channel::Stable)),
            test: test.map(|v| info(v, Channel::Test)),
            rate_limit_note: None,
        }
    }

    #[test]
    fn test_card_key_covers_both_channels() {
        assert_eq!(card_key(&updates_of(None, None)), None);
        assert_eq!(
            card_key(&updates_of(Some("0.6.1"), None)).as_deref(),
            Some("stable:0.6.1")
        );
        assert_eq!(
            card_key(&updates_of(None, Some("0.7.0-test.1"))).as_deref(),
            Some("test:0.7.0-test.1")
        );
        assert_eq!(
            card_key(&updates_of(Some("0.6.1"), Some("0.7.0-test.1"))).as_deref(),
            Some("stable:0.6.1 test:0.7.0-test.1")
        );
    }

    #[test]
    fn test_card_hidden_only_for_the_dismissed_version() {
        let u = updates_of(Some("0.6.1"), None);
        let key = card_key(&u).unwrap();
        // 未操作なら出る / 閉じた本人のキーでだけ抑止される
        assert!(card_should_show(&u, None));
        assert!(!card_should_show(&u, Some(&key)));
        // 新しいバージョンを検知したら出直す（しつこくはしないが、黙りもしない）
        let newer = updates_of(Some("0.6.2"), None);
        assert!(card_should_show(&newer, Some(&key)));
        // 片方だけ増えた場合も出直す
        let plus_test = updates_of(Some("0.6.1"), Some("0.7.0-test.1"));
        assert!(card_should_show(&plus_test, Some(&key)));
        // 更新が無ければ何があっても出さない
        assert!(!card_should_show(&updates_of(None, None), None));
    }

    /// リリースノートは /releases の応答から拾う（追加リクエスト無し。#616）
    #[test]
    fn test_release_notes_captured_and_trimmed() {
        let mut r = release("v99.0.0", false, &["tako-v99.0.0-macos-arm64.zip"]);
        r["body"] = serde_json::json!("\n## 変更点\n- 何か\n");
        let info = parse_releases_for(&[r], MAC, "0.5.13").stable.unwrap();
        assert_eq!(info.notes.as_deref(), Some("## 変更点\n- 何か"));

        // body が無い / 空白のみなら None（空欄を「ノートあり」と見せない）
        let mut empty = release("v99.0.0", false, &["tako-v99.0.0-macos-arm64.zip"]);
        empty["body"] = serde_json::json!("   \n ");
        assert!(parse_releases_for(&[empty], MAC, "0.5.13")
            .stable
            .unwrap()
            .notes
            .is_none());
        assert!(parse_releases_for(
            &[release("v99.0.0", false, &["tako-v99.0.0-macos-arm64.zip"])],
            MAC,
            "0.5.13"
        )
        .stable
        .unwrap()
        .notes
        .is_none());
    }

    #[test]
    fn test_gh_auth_token_does_not_panic() {
        // gh がなくても None を返すだけで panic しない
        let _ = gh_auth_token();
    }

    #[test]
    fn test_channel_updates_default_has_no_rate_note() {
        let u = ChannelUpdates::default();
        assert!(u.rate_limit_note.is_none());
    }

    #[test]
    fn test_check_channel_uses_all_channels() {
        // check_channel が check_all_channels 経由であることの型レベル確認
        // 実際の API 呼び出しはここではしないが、関数が存在し呼べることを確認
        let _ = std::panic::catch_unwind(|| {
            // ネットワーク不達なら Network エラーが返る（panic しない）
            let _ = check_channel(Channel::Stable);
        });
    }

    // --- #2434: Windows の適用経路 ---

    /// asset に `size` / `digest` まで載ったリリース（GitHub API の実際の形）
    fn release_with_digest(tag: &str, name: &str, size: u64, digest: &str) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "prerelease": false,
            "html_url": format!("https://github.com/{OWNER_REPO}/releases/tag/{tag}"),
            "assets": [{
                "name": name,
                "size": size,
                "digest": digest,
                "browser_download_url":
                    format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/{name}"),
            }],
        })
    }

    #[test]
    fn windowsの更新候補は配布元の大きさとsha256を持つ() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let releases = vec![release_with_digest(
            "v99.0.0",
            "tako-v99.0.0-windows-x86_64.exe",
            24_950_562,
            &digest,
        )];
        let stable = parse_releases_for(&releases, WIN, "0.8.39").stable.unwrap();
        assert_eq!(
            stable.asset_name.as_deref(),
            Some("tako-v99.0.0-windows-x86_64.exe")
        );
        assert_eq!(stable.asset_size, Some(24_950_562));
        assert_eq!(stable.asset_digest.as_deref(), Some(digest.as_str()));
        // 古い fixture（size / digest 無し）でも候補にはなる（検査は適用の時点で断る）
        let bare = vec![release(
            "v99.0.0",
            false,
            &["tako-v99.0.0-windows-x86_64.exe"],
        )];
        let stable = parse_releases_for(&bare, WIN, "0.8.39").stable.unwrap();
        assert_eq!((stable.asset_size, stable.asset_digest), (None, None));
    }

    #[test]
    fn 同じ版のwindowsインストーラーは更新候補にしない() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let releases = vec![release_with_digest(
            "v0.8.39",
            "tako-v0.8.39-windows-x86_64.exe",
            1,
            &digest,
        )];
        let updates = parse_releases_for(&releases, WIN, "0.8.39");
        assert!(updates.stable.is_none() && updates.test.is_none());
    }

    #[test]
    fn windowsの配布系統はmacosの経路へ入らない() {
        // apply: インストーラーで入れた tako はインストーラー、ポータブル版は断る
        assert_eq!(
            apply_route(InstallMethod::WindowsInstaller),
            ApplyRoute::WindowsInstaller
        );
        assert_eq!(
            apply_route(InstallMethod::WindowsPortable),
            ApplyRoute::Refuse(InstallMethod::WindowsPortable)
        );
        // apply-zip: Windows には zip で .app を差し替える経路が無い
        for method in [
            InstallMethod::WindowsInstaller,
            InstallMethod::WindowsPortable,
        ] {
            assert_eq!(zip_route(method), ApplyRoute::Refuse(method));
            assert!(!matches!(
                apply_route(method),
                ApplyRoute::MacZip | ApplyRoute::Homebrew
            ));
        }
        // macOS の経路は従来どおり
        assert_eq!(apply_route(InstallMethod::Homebrew), ApplyRoute::Homebrew);
        assert_eq!(apply_route(InstallMethod::Zip), ApplyRoute::MacZip);
        assert_eq!(apply_route(InstallMethod::BrokenBrew), ApplyRoute::MacZip);
        for method in [
            InstallMethod::Homebrew,
            InstallMethod::Zip,
            InstallMethod::BrokenBrew,
        ] {
            assert_eq!(zip_route(method), ApplyRoute::MacZip);
        }
        assert_eq!(InstallMethod::WindowsInstaller.label(), "windows-installer");
        assert_eq!(InstallMethod::WindowsPortable.label(), "windows-portable");
    }

    fn windows_info(url: &str, size: Option<u64>, digest: Option<&str>) -> UpdateInfo {
        UpdateInfo {
            version: "99.0.0".into(),
            channel: Channel::Stable,
            html_url: format!("https://github.com/{OWNER_REPO}/releases/tag/v99.0.0"),
            download_url: Some(url.into()),
            asset_name: Some("tako-v99.0.0-windows-x86_64.exe".into()),
            notes: None,
            asset_size: size,
            asset_digest: digest.map(str::to_string),
        }
    }

    #[test]
    fn windowsの適用は取得元と検査の材料が揃わなければ取りに行かずに断る() {
        let good =
            format!("https://github.com/{OWNER_REPO}/releases/download/v99.0.0/tako-v99.0.0-windows-x86_64.exe");
        let digest = format!("sha256:{}", "ab".repeat(32));
        // 取得元が違う
        let err = run_route(
            ApplyRoute::WindowsInstaller,
            &windows_info(
                "https://example.com/tako-v99.0.0-windows-x86_64.exe",
                Some(1),
                Some(&digest),
            ),
        )
        .unwrap_err()
        .text_in(Lang::Ja);
        assert!(err.contains("取得元"), "{err}");
        assert!(err.contains("今の版のまま使えます"), "{err}");
        assert!(
            err.contains("/releases/tag/v99.0.0"),
            "リリースページへ案内する: {err}"
        );
        // 配布元が sha256 を示していない
        let err = run_route(
            ApplyRoute::WindowsInstaller,
            &windows_info(&good, Some(1), None),
        )
        .unwrap_err()
        .text_in(Lang::Ja);
        assert!(err.contains("sha256"), "{err}");
        assert!(err.contains("今の版のまま使えます"), "{err}");
        // ポータブル版
        let err = run_route(
            ApplyRoute::Refuse(InstallMethod::WindowsPortable),
            &windows_info(&good, Some(1), Some(&digest)),
        )
        .unwrap_err()
        .text_in(Lang::Ja);
        assert!(err.contains("ポータブル版"), "{err}");
        // Windows での apply-zip
        let err = run_route(
            ApplyRoute::Refuse(InstallMethod::WindowsInstaller),
            &windows_info(&good, Some(1), Some(&digest)),
        )
        .unwrap_err()
        .text_in(Lang::Ja);
        assert!(err.contains("tako update apply"), "{err}");
    }

    #[test]
    fn 起動し直しに失敗したときの文面はwindowsでは未適用を言う() {
        let installer = Restart::RunInstaller(installer_update::PreparedInstaller {
            path: PathBuf::from("setup.exe"),
            log_path: PathBuf::from("setup.log"),
            size: 1,
            sha256: String::new(),
        });
        let mac = Restart::OpenBundle.failed_text("x");
        let win = installer.failed_text("x");
        assert_ne!(mac, win);
        assert!(
            win.contains("更新していません") || win.contains("nothing was updated"),
            "{win}"
        );
    }

    // --- #2365: 更新の口の断り・結果の文は表示言語に従う ---

    fn has_japanese(s: &str) -> Option<char> {
        s.chars()
            .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
    }

    /// 通し番号（網羅 match = バリアントを足すと見本の表を足すまでここが E0004 で落ちる）
    fn refusal_index(r: &UpdateRefusal) -> usize {
        match r {
            UpdateRefusal::UnknownChannel(_) => 0,
            UpdateRefusal::CheckFailed(_) => 1,
            UpdateRefusal::NoNewVersion(_) => 2,
            UpdateRefusal::PortableNotUpdatable { .. } => 3,
            UpdateRefusal::NoZipRouteOnWindows { .. } => 4,
            UpdateRefusal::MethodCannotUpdate(_) => 5,
            UpdateRefusal::InstallerRefused { .. } => 6,
            UpdateRefusal::NoWindowsInstaller { .. } => 7,
            UpdateRefusal::BrewNotRun(_) => 8,
            UpdateRefusal::BrewUpgradeFailed { .. } => 9,
            UpdateRefusal::NoZipAsset => 10,
            UpdateRefusal::TempDirFailed(_) => 11,
            UpdateRefusal::DownloadFailed(_) => 12,
            UpdateRefusal::ZipCreateFailed(_) => 13,
            UpdateRefusal::DownloadWriteFailed(_) => 14,
            UpdateRefusal::DittoNotRun(_) => 15,
            UpdateRefusal::UnzipFailed(_) => 16,
            UpdateRefusal::NoAppInZip => 17,
            UpdateRefusal::BundleReplace(_) => 18,
            UpdateRefusal::RepairNotNeeded(_) => 19,
            UpdateRefusal::BrewInstallFailed(_) => 20,
            UpdateRefusal::NoAppBundle => 21,
            UpdateRefusal::RestartFailed(_) => 22,
            UpdateRefusal::Installer(_) => 23,
            UpdateRefusal::InstallerNotLaunched(_) => 24,
        }
    }
    const REFUSAL_VARIANTS: usize = 25;

    fn note_index(n: &UpdateNote) -> usize {
        match n {
            UpdateNote::HomebrewDone => 0,
            UpdateNote::AlreadyLatest => 1,
            UpdateNote::ZipDone { .. } => 2,
            UpdateNote::InstallerReady { .. } => 3,
            UpdateNote::Repaired => 4,
        }
    }
    const NOTE_VARIANTS: usize = 5;

    fn installer_error_index(e: &installer_update::InstallerError) -> usize {
        use installer_update::InstallerError as I;
        match e {
            I::Origin(_) => 0,
            I::Unverifiable => 1,
            I::Network(_) => 2,
            I::Http(_) => 3,
            I::Size { .. } => 4,
            I::Digest => 5,
            I::NotExecutable => 6,
            I::Io(_) => 7,
            I::Launch(_) => 8,
            I::NotInstalled => 9,
        }
    }
    const INSTALLER_ERROR_VARIANTS: usize = 10;

    const PAGE: &str = "https://github.com/takushio2525/tako/releases/tag/v99.0.0";

    /// 見本と #2365 より前の日本語（旧実装の `format!` を同じ値で組んだ文）
    fn installer_error_table() -> Vec<(installer_update::InstallerError, &'static str)> {
        use installer_update::InstallerError as I;
        vec![
            (
                I::Origin("https://example.com/x.exe".into()),
                "インストーラーの取得元が tako の GitHub Releases ではないため取りに行かない: https://example.com/x.exe",
            ),
            (
                I::Unverifiable,
                "配布元がインストーラーのサイズか sha256 を示していないため、検査できないので起動しない",
            ),
            (
                I::Network("connection reset".into()),
                "インストーラーのダウンロードに失敗: connection reset",
            ),
            (I::Http(503), "インストーラーのダウンロードに失敗（HTTP 503）"),
            (
                I::Size { got: 10, want: 20 },
                "インストーラーの大きさが配布元の値と合わない（受信 10 バイト / 配布元 20 バイト。途中で切れた可能性がある）",
            ),
            (
                I::Digest,
                "インストーラーの sha256 が配布元の値と合わないので起動しない",
            ),
            (
                I::NotExecutable,
                "受けたファイルが Windows の実行ファイルではないので起動しない",
            ),
            (I::Io("disk full".into()), "インストーラーの保存に失敗: disk full"),
            (
                I::Launch("access denied".into()),
                "インストーラーを起動できない: access denied",
            ),
            (
                I::NotInstalled,
                "この tako はインストーラーで入れた場所から動いていない（ポータブル版・開発ビルド）ため、アプリ内では更新できない。リリースページから同じ形式の配布物を取り直してください",
            ),
        ]
    }

    fn refusal_table() -> Vec<(UpdateRefusal, String)> {
        use installer_update::InstallerError as I;
        let net = |c: NetworkCause| UpdateRefusal::CheckFailed(CheckError::Network(c));
        vec![
            (
                UpdateRefusal::UnknownChannel("beta".into()),
                "不明なチャンネル: \"beta\"（stable / test のいずれか）".into(),
            ),
            (
                net(NetworkCause::Transport("io: Connection refused".into())),
                "更新チェックに失敗: ネットワークエラー: io: Connection refused".into(),
            ),
            (
                net(NetworkCause::ReleasesNotFound),
                "更新チェックに失敗: ネットワークエラー: リリースが見つからない（リポジトリ未公開または未リリース）".into(),
            ),
            (
                net(NetworkCause::UnexpectedStatus(502)),
                "更新チェックに失敗: ネットワークエラー: 予期しないステータスコード: 502".into(),
            ),
            (
                UpdateRefusal::CheckFailed(CheckError::Parse(ParseCause::ReadBody("eof".into()))),
                "更新チェックに失敗: レスポンス解析エラー: レスポンス読み取りエラー: eof".into(),
            ),
            (
                UpdateRefusal::CheckFailed(CheckError::Parse(ParseCause::Json("expected value".into()))),
                "更新チェックに失敗: レスポンス解析エラー: JSON パースエラー: expected value".into(),
            ),
            (
                UpdateRefusal::CheckFailed(CheckError::RateLimit { retry_after: None }),
                "更新チェックに失敗: GitHub レート制限中".into(),
            ),
            (
                UpdateRefusal::CheckFailed(CheckError::RateLimit {
                    retry_after: Some(0),
                }),
                "更新チェックに失敗: GitHub レート制限中（リセット: まもなく）".into(),
            ),
            (
                UpdateRefusal::NoNewVersion(Channel::Stable),
                "安定版の新しいバージョンが見つからない（既に最新版です）".into(),
            ),
            (
                UpdateRefusal::NoNewVersion(Channel::Test),
                "テスト版の新しいバージョンが見つからない（既に最新版です）".into(),
            ),
            (
                UpdateRefusal::PortableNotUpdatable {
                    release_page: PAGE.into(),
                },
                format!(
                    "{}（リリースページ: {PAGE}）",
                    installer_error_table()[9].1
                ),
            ),
            (
                UpdateRefusal::NoZipRouteOnWindows {
                    release_page: PAGE.into(),
                },
                format!(
                    "Windows には zip で差し替える経路が無い（zip は macOS の .app 用）。`tako update apply` でインストーラーから更新してください（リリースページ: {PAGE}）"
                ),
            ),
            (
                UpdateRefusal::MethodCannotUpdate(InstallMethod::Zip),
                "配布系統 zip では更新できない".into(),
            ),
            (
                UpdateRefusal::InstallerRefused {
                    error: I::Digest,
                    release_page: PAGE.into(),
                },
                format!(
                    "インストーラーの sha256 が配布元の値と合わないので起動しない。今の版のまま使えます（手動で入れるならリリースページ: {PAGE}）"
                ),
            ),
            (
                UpdateRefusal::NoWindowsInstaller {
                    release_page: PAGE.into(),
                },
                format!("このリリースに Windows のインストーラーが見つからない（リリースページ: {PAGE}）"),
            ),
            (
                UpdateRefusal::BrewNotRun("No such file or directory (os error 2)".into()),
                "brew の実行に失敗: No such file or directory (os error 2)".into(),
            ),
            (
                UpdateRefusal::BrewUpgradeFailed {
                    stderr: "Error: boom".into(),
                    zip_fallback: true,
                },
                "brew upgrade が失敗: Error: boom\n[zip-fallback-available] brew 更新に失敗しました。`tako update apply-zip` で zip 経由の更新が可能です".into(),
            ),
            (
                UpdateRefusal::BrewUpgradeFailed {
                    stderr: "Error: boom".into(),
                    zip_fallback: false,
                },
                "brew upgrade が失敗: Error: boom".into(),
            ),
            (
                UpdateRefusal::NoZipAsset,
                "ダウンロード用 ZIP アセットが見つかりません".into(),
            ),
            (
                UpdateRefusal::TempDirFailed("Permission denied (os error 13)".into()),
                "一時ディレクトリの作成に失敗: Permission denied (os error 13)".into(),
            ),
            (
                UpdateRefusal::DownloadFailed("http status: 404".into()),
                "ダウンロードに失敗: http status: 404".into(),
            ),
            (
                UpdateRefusal::ZipCreateFailed("Permission denied (os error 13)".into()),
                "ZIP ファイルの作成に失敗: Permission denied (os error 13)".into(),
            ),
            (
                UpdateRefusal::DownloadWriteFailed("No space left on device (os error 28)".into()),
                "ダウンロードの書き込みに失敗: No space left on device (os error 28)".into(),
            ),
            (
                UpdateRefusal::DittoNotRun("No such file or directory (os error 2)".into()),
                "ditto による展開に失敗: No such file or directory (os error 2)".into(),
            ),
            (
                UpdateRefusal::UnzipFailed("ditto: Couldn't read PKZip signature".into()),
                "ZIP の展開に失敗: ditto: Couldn't read PKZip signature".into(),
            ),
            (
                UpdateRefusal::NoAppInZip,
                "展開した ZIP に tako.app が見つかりません".into(),
            ),
            (
                UpdateRefusal::BundleReplace("replace failed: os error 1".into()),
                "replace failed: os error 1".into(),
            ),
            (
                UpdateRefusal::RepairNotNeeded(InstallMethod::Homebrew),
                "現在の配布系統は homebrew のため修復は不要です".into(),
            ),
            (
                UpdateRefusal::BrewInstallFailed("Error: boom".into()),
                "brew install --force が失敗: Error: boom".into(),
            ),
            (
                UpdateRefusal::NoAppBundle,
                ".app バンドルのパスが特定できない（CLI 単体実行？）".into(),
            ),
            (
                UpdateRefusal::RestartFailed("open: exit status 1".into()),
                "再起動に失敗: open: exit status 1".into(),
            ),
            (
                UpdateRefusal::Installer(I::Launch("not found".into())),
                "インストーラーを起動できない: not found".into(),
            ),
            (
                UpdateRefusal::InstallerNotLaunched(I::Launch("access denied".into())),
                "インストーラーを起動できない: access denied。今の版のまま使えます".into(),
            ),
        ]
    }

    fn note_table() -> Vec<(UpdateNote, String)> {
        vec![
            (UpdateNote::HomebrewDone, "Homebrew で更新完了".into()),
            (UpdateNote::AlreadyLatest, "既に最新版です".into()),
            (
                UpdateNote::ZipDone {
                    strategy: "contents-swap",
                    pin_may_drop: false,
                },
                "ZIP で更新完了（差し替え: contents-swap）".into(),
            ),
            (
                UpdateNote::ZipDone {
                    strategy: "fresh",
                    pin_may_drop: true,
                },
                "ZIP で更新完了（差し替え: fresh。この環境ではアトミックな入れ替えが使えないため、Dock のピン留めが外れることがあります）".into(),
            ),
            (
                UpdateNote::InstallerReady {
                    version: "99.0.0".into(),
                },
                "インストーラーで更新（v99.0.0 の sha256 が配布元と一致。tako を終了して入れ替え、終わったら起動し直します）".into(),
            ),
            (
                UpdateNote::Repaired,
                "brew の cask 台帳を再締結しました。以後 brew upgrade で更新できます".into(),
            ),
        ]
    }

    /// 1 行の検査: 日本語が旧文とバイト一致・英語に日本語が無い・英語が空でも日本語と同じでもない
    /// （中身をそのまま出すもの = 文字列を持つだけのバリアントは日英が同じでよい）
    fn check_row(
        what: &str,
        ja: &str,
        en: &str,
        old: &str,
        passthrough: bool,
        failures: &mut Vec<String>,
    ) {
        if ja != old {
            failures.push(format!(
                "{what} の日本語が #2365 より前と違う\n  want: {old}\n  got:  {ja}"
            ));
        }
        if let Some(c) = has_japanese(en) {
            failures.push(format!("{what} の英語に日本語（{c}）が混ざる: {en}"));
        }
        if en.trim().is_empty() || (!passthrough && ja == en) {
            failures.push(format!(
                "{what} の英語が空か日本語と同じ（英語の書き忘れ）: {en}"
            ));
        }
    }

    #[test]
    fn 更新の口の断りは全バリアントが日英を持ち日本語は2365より前と一致() {
        let mut failures = Vec::new();
        let refusals = refusal_table();
        let mut seen = [false; REFUSAL_VARIANTS];
        for (r, old) in &refusals {
            seen[refusal_index(r)] = true;
            let passthrough = matches!(r, UpdateRefusal::BundleReplace(_));
            check_row(
                &format!("{r:?}"),
                &r.text_in(Lang::Ja),
                &r.text_in(Lang::En),
                old,
                passthrough,
                &mut failures,
            );
            // 「見つからない」の読み（リモートの 404 寄せ）は日英でそろう
            let (ja, en) = (r.text_in(Lang::Ja), r.text_in(Lang::En));
            if tako_control::DispatchError::says_not_found(&ja)
                != tako_control::DispatchError::says_not_found(&en)
            {
                failures.push(format!(
                    "{r:?}: 見つからないの読みが日英で違う\n  ja: {ja}\n  en: {en}"
                ));
            }
            // zip へ切り替える印は日英で同じ（GUI と AI が読む）
            if ja.contains(ZIP_FALLBACK_MARK) != en.contains(ZIP_FALLBACK_MARK) {
                failures.push(format!("{r:?}: {ZIP_FALLBACK_MARK} の有無が日英で違う"));
            }
        }
        for (i, hit) in seen.iter().enumerate() {
            if !hit {
                failures.push(format!(
                    "UpdateRefusal の見本に無いバリアント（通し番号）: {i}"
                ));
            }
        }
        let mut seen = [false; NOTE_VARIANTS];
        for (n, old) in note_table() {
            seen[note_index(&n)] = true;
            check_row(
                &format!("{n:?}"),
                &n.text_in(Lang::Ja),
                &n.text_in(Lang::En),
                &old,
                false,
                &mut failures,
            );
        }
        for (i, hit) in seen.iter().enumerate() {
            if !hit {
                failures.push(format!(
                    "UpdateNote の見本に無いバリアント（通し番号）: {i}"
                ));
            }
        }
        let mut seen = [false; INSTALLER_ERROR_VARIANTS];
        for (e, old) in installer_error_table() {
            seen[installer_error_index(&e)] = true;
            check_row(
                &format!("{e:?}"),
                &e.text_in(Lang::Ja),
                &e.text_in(Lang::En),
                old,
                false,
                &mut failures,
            );
        }
        for (i, hit) in seen.iter().enumerate() {
            if !hit {
                failures.push(format!(
                    "InstallerError の見本に無いバリアント（通し番号）: {i}"
                ));
            }
        }
        // 制限中の注記（update check の応答と通知カード）
        for (retry, old) in [
            (None, "GitHub API 制限中（キャッシュ表示）"),
            (
                Some(0),
                "GitHub API 制限中（キャッシュ表示・リセット: まもなく）",
            ),
        ] {
            check_row(
                &format!("rate_limit_note({retry:?})"),
                &rate_limit_note_in(retry, Lang::Ja),
                &rate_limit_note_in(retry, Lang::En),
                old,
                false,
                &mut failures,
            );
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// 応答の `error.message`（update check）も中身と同じ言語
    #[test]
    fn 更新の確認の失敗の応答は中身も表示言語() {
        let e = CheckError::Network(NetworkCause::ReleasesNotFound);
        let en = e.to_json_in(Lang::En);
        assert_eq!(en["type"], "network");
        assert!(
            has_japanese(en["message"].as_str().unwrap()).is_none(),
            "{en}"
        );
        assert_eq!(
            e.to_json_in(Lang::Ja)["message"],
            "リリースが見つからない（リポジトリ未公開または未リリース）"
        );
        let limited = CheckError::RateLimit {
            retry_after: Some(0),
        };
        assert_eq!(
            limited.to_json_in(Lang::En)["message"],
            "GitHub rate limit reached (resets soon)"
        );
    }
}
