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

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tako_core::platform::release_assets::{self, Arch};
use tako_core::platform::support::Platform;

/// 更新チェック間隔（24 時間）
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// チェック失敗時のリトライ間隔（1 時間）
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// 現在のバージョン（Cargo.toml から埋め込み）
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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

impl std::str::FromStr for Channel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Channel::Stable),
            "test" => Ok(Channel::Test),
            _ => Err(format!(
                "不明なチャンネル: {s:?}（stable / test のいずれか）"
            )),
        }
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

/// 更新チェックのエラー（#59: エラーと「更新なし」を区別する）
#[derive(Debug, Clone)]
pub enum CheckError {
    /// GitHub API / Web のレート制限（X-RateLimit-Reset の UNIX timestamp を含む）
    RateLimit { retry_after: Option<u64> },
    /// ネットワークエラー（DNS 解決失敗、接続タイムアウト等）
    Network(String),
    /// レスポンスのパースに失敗
    Parse(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::RateLimit {
                retry_after: Some(ts),
            } => {
                write!(
                    f,
                    "GitHub レート制限中（リセット: {}）",
                    format_reset_time(*ts)
                )
            }
            CheckError::RateLimit { retry_after: None } => {
                write!(f, "GitHub レート制限中")
            }
            CheckError::Network(msg) => write!(f, "ネットワークエラー: {msg}"),
            CheckError::Parse(msg) => write!(f, "レスポンス解析エラー: {msg}"),
        }
    }
}

impl CheckError {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            CheckError::RateLimit { retry_after } => serde_json::json!({
                "type": "rate_limit",
                "message": self.to_string(),
                "retry_after": retry_after,
            }),
            CheckError::Network(msg) => serde_json::json!({
                "type": "network",
                "message": msg,
            }),
            CheckError::Parse(msg) => serde_json::json!({
                "type": "parse",
                "message": msg,
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

fn format_reset_time(unix_ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if unix_ts > now {
        let remaining = unix_ts - now;
        let minutes = remaining / 60;
        if minutes > 0 {
            crate::ui_text::update::eta_minutes(minutes)
        } else {
            crate::ui_text::update::eta_seconds(remaining)
        }
    } else {
        crate::ui_text::update::eta_soon().into()
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
}

impl InstallMethod {
    pub fn label(&self) -> &'static str {
        match self {
            InstallMethod::Homebrew => "homebrew",
            InstallMethod::Zip => "zip",
            InstallMethod::BrokenBrew => "broken-brew",
        }
    }
}

/// 配布系統の判別（高速パス: ファイルパスのみ。brew サブプロセスを呼ばない）。
/// broken-brew の検知には `detect_install_method_full()` を使う
pub fn detect_install_method() -> InstallMethod {
    detect_install_method_inner(is_app_in_caskroom(), is_exe_in_caskroom())
}

/// broken-brew を含む完全な配布系統判別（低速: brew サブプロセスを呼ぶ可能性あり）。
/// background executor や CLI/MCP の status から呼ぶ。render パスでは呼ばない
pub fn detect_install_method_full() -> InstallMethod {
    let app_in_caskroom = is_app_in_caskroom();
    let exe_in_caskroom = is_exe_in_caskroom();
    let fast = detect_install_method_inner(app_in_caskroom, exe_in_caskroom);
    if fast != InstallMethod::Zip {
        return fast;
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
    Path::new("/Applications/tako.app").exists()
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
        app_path: "/Applications/tako.app".into(),
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

/// broken-brew の修復: `brew install --cask --force` で台帳を再締結する
pub fn repair_brew() -> Result<String, String> {
    let method = detect_install_method_full();
    if method != InstallMethod::BrokenBrew {
        return Err(format!(
            "現在の配布系統は {0} のため修復は不要です",
            method.label()
        ));
    }
    // #586: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
            .args(["install", "--cask", "takushio2525/tako/tako", "--force"])
            .output()
            .map_err(|e| format!("brew の実行に失敗: {e}"))?;
    if output.status.success() {
        Ok("brew の cask 台帳を再締結しました。以後 brew upgrade で更新できます".into())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("brew install --force が失敗: {stderr}"))
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

/// パース済みバージョン（`X.Y.Z` または `X.Y.Z-test.N`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// None = 安定版、Some(n) = テスト版 `-test.N`
    pub test_num: Option<u32>,
}

impl ParsedVersion {
    pub fn parse(s: &str) -> Option<Self> {
        let (base, test_num) = if let Some((base, suffix)) = s.split_once("-test.") {
            (base, Some(suffix.parse::<u32>().ok()?))
        } else if s.contains('-') {
            // `-test.N` 以外のプレリリースサフィックス（例: `-rc.1`）はテスト版扱い
            return None;
        } else {
            (s, None)
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
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.base_tuple().cmp(&other.base_tuple()).then_with(|| {
            match (self.test_num, other.test_num) {
                // 同じベースなら: stable(None) > test(Some)
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => a.cmp(&b),
                (None, None) => std::cmp::Ordering::Equal,
            }
        })
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

    let resp = req.call().map_err(|e| CheckError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if status == 403 || status == 429 {
        let retry_after = resp
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        // キャッシュがあれば補助表示付きで返す
        if let Some(cached) = CACHED_UPDATES.lock().ok().and_then(|g| g.clone()) {
            let note = match retry_after {
                Some(ts) => format!(
                    "GitHub API 制限中（キャッシュ表示・リセット: {}）",
                    format_reset_time(ts)
                ),
                None => "GitHub API 制限中（キャッシュ表示）".into(),
            };
            return Ok(ChannelUpdates {
                rate_limit_note: Some(note),
                ..cached
            });
        }
        return Err(CheckError::RateLimit { retry_after });
    }
    if status == 404 {
        return Err(CheckError::Network(
            "リリースが見つからない（リポジトリ未公開または未リリース）".into(),
        ));
    }
    if status != 200 {
        return Err(CheckError::Network(format!(
            "予期しないステータスコード: {status}"
        )));
    }

    let body: String = resp
        .into_body()
        .read_to_string()
        .map_err(|e| CheckError::Parse(format!("レスポンス読み取りエラー: {e}")))?;
    let releases: Vec<serde_json::Value> = serde_json::from_str(&body)
        .map_err(|e| CheckError::Parse(format!("JSON パースエラー: {e}")))?;

    let result = parse_releases(&releases);

    if let Ok(mut guard) = CACHED_UPDATES.lock() {
        *guard = Some(result.clone());
    }

    Ok(result)
}

/// 1 リリースの `assets` から自分の環境向けの配布物を選ぶ（#595）。
///
/// 戻り値は `(アセット名, ダウンロード URL)`。**該当が無ければ `None`** =
/// このリリースは自分にとって「更新候補ではない」。
fn select_asset(
    release: &serde_json::Value,
    platform: Platform,
    arch: Arch,
) -> Option<(String, String)> {
    let assets = release["assets"].as_array()?;
    let names: Vec<&str> = assets.iter().filter_map(|a| a["name"].as_str()).collect();
    let chosen = release_assets::select(names.iter().copied(), platform, arch)?;

    // 通常は GitHub が browser_download_url を返す。欠けていても命名規則から
    // 復元できる（URL の組み立て規則はここ 1 箇所）
    let url = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(chosen))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let tag = release["tag_name"].as_str().unwrap_or_default();
            format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/{chosen}")
        });

    Some((chosen.to_string(), url))
}

/// /releases JSON 配列から ChannelUpdates をパースする（実行環境で判定）
fn parse_releases(releases: &[serde_json::Value]) -> ChannelUpdates {
    parse_releases_for(releases, TargetEnv::current(), CURRENT_VERSION)
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
            if ver <= *cur {
                continue;
            }
        }

        // 自分の環境向けアセットが無いリリースは読み飛ばして次を探す（#595 要件 1）
        let Some((asset_name, download_url)) = select_asset(release, env.platform, arch) else {
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
            download_url: Some(download_url),
            asset_name: Some(asset_name),
            notes,
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

/// 更新を実行する（ブロッキング。background executor で呼ぶ）。
/// broken-brew 状態では zip フォールバックを使う
pub fn perform_update(info: &UpdateInfo) -> Result<String, String> {
    match detect_install_method_full() {
        InstallMethod::Homebrew => update_via_homebrew(info),
        InstallMethod::Zip => update_via_zip(info),
        InstallMethod::BrokenBrew => {
            // broken-brew では brew 経由の更新は不可能 → zip で直接更新
            update_via_zip(info)
        }
    }
}

/// zip 強制更新（brew 失敗時のフォールバック用。配布系統を問わず zip で更新する）
pub fn perform_update_zip(info: &UpdateInfo) -> Result<String, String> {
    update_via_zip(info)
}

fn update_via_homebrew(info: &UpdateInfo) -> Result<String, String> {
    // #586: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    let output =
        tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
            .args(["upgrade", "--cask", "takushio2525/tako/tako"])
            .output()
            .map_err(|e| format!("brew の実行に失敗: {e}"))?;
    if output.status.success() {
        Ok("Homebrew で更新完了".into())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already installed") {
            Ok("既に最新版です".into())
        } else {
            // brew 失敗時は zip フォールバック可能であることを含めてエラーを返す（#50）
            let has_zip = info.download_url.is_some();
            let fallback_hint = if has_zip {
                "\n[zip-fallback-available] brew 更新に失敗しました。`tako update apply-zip` で zip 経由の更新が可能です"
            } else {
                ""
            };
            Err(format!("brew upgrade が失敗: {stderr}{fallback_hint}"))
        }
    }
}

fn update_via_zip(info: &UpdateInfo) -> Result<String, String> {
    let url = info
        .download_url
        .as_deref()
        .ok_or_else(|| "ダウンロード用 ZIP アセットが見つかりません".to_string())?;

    let tmp_dir = std::env::temp_dir().join("tako-update");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("一時ディレクトリの作成に失敗: {e}"))?;
    let zip_path = tmp_dir.join("tako.zip");

    let mut body = ureq::get(url)
        .header("User-Agent", &format!("tako/{CURRENT_VERSION}"))
        .call()
        .map_err(|e| format!("ダウンロードに失敗: {e}"))?
        .into_body();
    let mut file =
        std::fs::File::create(&zip_path).map_err(|e| format!("ZIP ファイルの作成に失敗: {e}"))?;
    std::io::copy(&mut body.as_reader(), &mut file)
        .map_err(|e| format!("ダウンロードの書き込みに失敗: {e}"))?;
    drop(file);

    let output = std::process::Command::new("ditto")
        .args([
            "-xk",
            &zip_path.to_string_lossy(),
            &tmp_dir.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("ditto による展開に失敗: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ZIP の展開に失敗: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let extracted_app = find_app_bundle(&tmp_dir)
        .ok_or_else(|| "展開した ZIP に tako.app が見つかりません".to_string())?;

    let dest = Path::new("/Applications/tako.app");
    if dest.exists() {
        let backup = Path::new("/Applications/tako.app.bak");
        let _ = std::fs::remove_dir_all(backup);
        std::fs::rename(dest, backup)
            .map_err(|e| format!("/Applications/tako.app のバックアップに失敗: {e}"))?;
    }
    let output = std::process::Command::new("ditto")
        .args([
            &extracted_app.to_string_lossy().to_string(),
            &dest.to_string_lossy().to_string(),
        ])
        .output()
        .map_err(|e| format!("アプリのコピーに失敗: {e}"))?;
    if !output.status.success() {
        let backup = Path::new("/Applications/tako.app.bak");
        if backup.exists() {
            let _ = std::fs::rename(backup, dest);
        }
        return Err(format!(
            "アプリのインストールに失敗: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
    let _ = std::fs::remove_dir_all(Path::new("/Applications/tako.app.bak"));

    Ok("ZIP で更新完了".into())
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

/// .app バンドルを自動再起動する。呼び出し元プロセスは exit(0) で終了する想定。
/// macOS の `open -n` でバンドルを新プロセスとして起動し、自分は終了する。
pub fn restart_app() -> Result<(), String> {
    let bundle = app_bundle_path()
        .ok_or_else(|| ".app バンドルのパスが特定できない（CLI 単体実行？）".to_string())?;
    tako_control::platform::os_integration::open_new_instance(&bundle)
        .map_err(|e| format!("再起動に失敗: {e}"))?;
    Ok(())
}

/// dispatch 層に公開する更新情報の JSON 表現（broken-brew 診断を含む）
pub fn update_status_json() -> serde_json::Value {
    let method = detect_install_method_full();
    let duplicates = detect_duplicate_cli();
    let current_channel = if CURRENT_VERSION.contains("-test.") {
        Channel::Test
    } else {
        Channel::Stable
    };
    let env = TargetEnv::current();
    let mut json = serde_json::json!({
        "current_version": CURRENT_VERSION,
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
        // 表示ラベルは言語カタログ依存（#435）。相対比較で言語グローバルに依存しない
        assert_eq!(
            Channel::Stable.display_label(),
            crate::ui_text::update::channel_stable()
        );
        assert_eq!(
            Channel::Test.display_label(),
            crate::ui_text::update::channel_test()
        );
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
        assert!(
            method == InstallMethod::Homebrew
                || method == InstallMethod::Zip
                || method == InstallMethod::BrokenBrew
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
    }

    // --- CheckError ---

    #[test]
    fn test_check_error_display() {
        let e = CheckError::Network("connection refused".into());
        assert!(e.to_string().contains("connection refused"));

        let e = CheckError::RateLimit { retry_after: None };
        assert!(e.to_string().contains("レート制限"));

        let e = CheckError::Parse("bad json".into());
        assert!(e.to_string().contains("bad json"));
    }

    #[test]
    fn test_check_error_to_json() {
        let e = CheckError::RateLimit {
            retry_after: Some(1234567890),
        };
        let json = e.to_json();
        assert_eq!(json["type"], "rate_limit");
        assert_eq!(json["retry_after"], 1234567890);

        let e = CheckError::Network("timeout".into());
        let json = e.to_json();
        assert_eq!(json["type"], "network");
    }

    #[test]
    fn test_check_error_retry_duration() {
        let e = CheckError::Network("timeout".into());
        assert_eq!(e.retry_duration(), RETRY_INTERVAL);

        let e = CheckError::RateLimit { retry_after: None };
        assert_eq!(e.retry_duration(), RETRY_INTERVAL);
    }

    #[test]
    fn test_format_reset_time() {
        // 遠い未来の timestamp → 「約N分後」形式
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300;
        let s = format_reset_time(future);
        // 言語カタログ依存（#435）: 分または秒の相対表記のどちらか
        assert!(
            s == crate::ui_text::update::eta_minutes(4)
                || s == crate::ui_text::update::eta_minutes(5)
                || s.contains(&crate::ui_text::update::eta_seconds(0)[..2]),
            "相対表記になっていない: {s:?}"
        );

        // 過去の timestamp → 「まもなく」
        assert_eq!(format_reset_time(0), crate::ui_text::update::eta_soon());
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
        let result = repair_brew();
        if detect_install_method_full() != InstallMethod::BrokenBrew {
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("修復は不要"));
        }
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
}
