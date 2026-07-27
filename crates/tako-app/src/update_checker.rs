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
//! プラットフォーム分岐（#528）: チェック層は共通。配布アセットの解決と更新実行だけが
//! `UpdateTarget` で分かれる。**Windows は `tako-setup-{tag}-x64.exe` が実在するリリース
//! だけを「更新あり」とし**（macOS だけの夜間リリースで押せない通知を出さないため）、
//! 更新は実行中 exe を置き換えられない制約からインストーラーへ委譲する。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

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
    #[allow(dead_code)]
    pub html_url: String,
    pub download_url: Option<String>,
    /// リリース JSON が申告するアセットのバイト数（Windows の整合確認に使う）。
    /// macOS 経路は URL 決め打ちでアセット一覧を見ないため常に None
    #[allow(dead_code)]
    pub download_size: Option<u64>,
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
    /// ユーザーが閉じた（次回起動まで非表示）
    Dismissed,
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
    // #628: GUI プロセスからの起動なのでコンソールウィンドウを出させない
    tako_core::platform::process::no_console_window(&mut std::process::Command::new("brew"))
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// brew の cask 台帳に tako が登録されているか
fn is_brew_cask_registered() -> bool {
    // #628: GUI プロセスからの起動なのでコンソールウィンドウを出させない
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
    // #628: GUI プロセスからの起動なのでコンソールウィンドウを出させない
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
    let output = std::process::Command::new("gh")
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

/// 配布アセットの選び方（プラットフォームごと）。
///
/// **判定を純粋関数に閉じ込めるための型**であり、`cfg!` は `current()` の 1 箇所だけに置く。
/// こうしておくと macOS 上の `cargo test` からでも Windows 側の挙動を検証できる
/// （`platform::support` と同じ方針。#515）。
/// #528 の B14 で `trait UpdateChannel` に切り出すときは、この分岐がそのまま実装の境界になる
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateTarget {
    MacOs,
    Windows,
}

impl UpdateTarget {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::MacOs
        }
    }
}

/// 実行中プラットフォームのアーキテクチャ表記（macOS のアセット名で使う）
fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

/// Windows インストーラーのアセット名。`installer/windows/build-installer.ps1` の
/// `OutputBaseFilename` と 1:1（食い違うと通知が永久に出なくなるので変えるときは両方直す）
fn windows_setup_asset_name(tag: &str) -> String {
    format!("tako-setup-{tag}-x64.exe")
}

/// リリース JSON の assets から名前一致するものを探し (ダウンロード URL, バイト数) を返す。
/// browser_download_url が欠けていた場合はタグから URL を組み立てる
fn find_release_asset(
    release: &serde_json::Value,
    tag: &str,
    name: &str,
) -> Option<(String, Option<u64>)> {
    let asset = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(name))?;
    let url = asset["browser_download_url"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/{name}")
        });
    Some((url, asset["size"].as_u64()))
}

/// このプラットフォームで実際にインストールできるアセットを解決する。
///
/// - macOS: 従来どおり URL 決め打ち（アセット一覧は見ない = 挙動不変）
/// - Windows: `tako-setup-{tag}-x64.exe` が**実在するときだけ** Some。
///   macOS だけの夜間リリースでは None になり、押しても失敗する通知を出さずに済む（#528）
fn resolve_download_asset(
    release: &serde_json::Value,
    tag: &str,
    target: UpdateTarget,
    arch: &str,
) -> Option<(String, Option<u64>)> {
    match target {
        UpdateTarget::MacOs => Some((
            format!(
                "https://github.com/{OWNER_REPO}/releases/download/{tag}/tako-{tag}-macos-{arch}.zip"
            ),
            None,
        )),
        UpdateTarget::Windows => {
            find_release_asset(release, tag, &windows_setup_asset_name(tag))
        }
    }
}

/// /releases JSON 配列から ChannelUpdates をパースする（テスト用にも公開）
fn parse_releases(releases: &[serde_json::Value]) -> ChannelUpdates {
    parse_releases_for(releases, UpdateTarget::current(), current_arch())
}

/// `parse_releases` の本体。プラットフォームと arch を引数に取るので、
/// macOS 上からでも Windows 側の判定を検証できる
fn parse_releases_for(
    releases: &[serde_json::Value],
    target: UpdateTarget,
    arch: &str,
) -> ChannelUpdates {
    let current = ParsedVersion::parse(CURRENT_VERSION);

    let mut result = ChannelUpdates::default();

    for release in releases {
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

        if let Some(ref cur) = current {
            if ver <= *cur {
                continue;
            }
        }

        // このプラットフォーム向けの配布物が無いリリースは「更新あり」と言ってはいけない。
        // 読み飛ばして次（それでも現行版より新しい）のリリースを見る
        let Some((download_url, download_size)) =
            resolve_download_asset(release, tag, target, arch)
        else {
            continue;
        };

        let html_url = release["html_url"].as_str().unwrap_or_default().to_string();

        let slot = match channel {
            Channel::Stable => &mut result.stable,
            Channel::Test => &mut result.test,
        };
        if slot.is_none() {
            *slot = Some(UpdateInfo {
                version: version_str.to_string(),
                channel,
                html_url,
                download_url: Some(download_url),
                download_size,
            });
        }

        if result.stable.is_some() && result.test.is_some() {
            break;
        }
    }

    result
}

/// 更新を実行する（ブロッキング。background executor で呼ぶ）。
/// broken-brew 状態では zip フォールバックを使う
pub fn perform_update(info: &UpdateInfo) -> Result<String, String> {
    // Windows は実行中の exe を自分で置き換えられないため、差し替えごとインストーラーへ委譲する。
    // 分岐は `UpdateTarget` に閉じ、呼び出し側（status_bar / dispatch）は素のまま
    if UpdateTarget::current() == UpdateTarget::Windows {
        return update_via_windows_installer(info);
    }
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
    // zip フォールバックは brew 詰まりの救済手段（macOS 専用）。Windows には brew 経路が
    // 無いうえ、zip を展開しても実行中の exe は置き換えられないので、素直に断って案内する
    if UpdateTarget::current() == UpdateTarget::Windows {
        return Err(windows_manual_hint(
            info,
            "zip フォールバックは macOS 専用です。Windows はインストーラー経由で更新します",
        ));
    }
    update_via_zip(info)
}

fn update_via_homebrew(info: &UpdateInfo) -> Result<String, String> {
    let output = std::process::Command::new("brew")
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

// --- Windows: インストーラー委譲による更新（#528） ---

/// インストーラーへ渡す引数。`/SILENT` はウィザードを出さず進捗だけ、
/// `/NORESTART` は「更新に再起動が要る」と判断されても OS を再起動させないための保険
fn windows_installer_args() -> &'static [&'static str] {
    &["/SILENT", "/NORESTART"]
}

/// 手動更新の案内を添えたエラー文面を作る（DL・起動のどこで失敗しても行き止まりにしない）
fn windows_manual_hint(info: &UpdateInfo, err: &str) -> String {
    format!(
        "{err}\n手動更新: {} から tako-setup-...-x64.exe を取得してください",
        release_page_url(info)
    )
}

/// 案内に出すリリースページ URL（リリース個別ページ → 無ければ一覧ページ）
fn release_page_url(info: &UpdateInfo) -> String {
    if info.html_url.is_empty() {
        format!("https://github.com/{OWNER_REPO}/releases")
    } else {
        info.html_url.clone()
    }
}

/// ダウンロードしたインストーラーの整合確認（純粋関数。テストから直接叩ける）。
/// GitHub はアセットのチェックサムを配らないので、申告サイズと PE ヘッダで
/// 「途中で切れた」「HTML のエラーページを掴んだ」を弾く
fn verify_installer_bytes(
    head: &[u8],
    downloaded: u64,
    expected: Option<u64>,
) -> Result<(), String> {
    if downloaded == 0 {
        return Err("ダウンロードしたインストーラーが空です".into());
    }
    if let Some(expected) = expected {
        if expected != downloaded {
            return Err(format!(
                "ダウンロードしたインストーラーのサイズが一致しません（期待 {expected} バイト / 実際 {downloaded} バイト）"
            ));
        }
    }
    if head.first() != Some(&b'M') || head.get(1) != Some(&b'Z') {
        return Err("ダウンロードしたファイルが Windows 実行ファイルではありません".into());
    }
    Ok(())
}

/// Windows: 新版インストーラー（Inno Setup 製 setup exe）を取得してサイレント起動する。
///
/// 実行中の exe は自分では置き換えられないので、差し替えはインストーラーに任せ、
/// tako 側は起動を見届けたらすぐ終了する（呼び出し元が `restart_app()` → `cx.quit()`）。
/// インストーラーを待たないのは意図的で、待つと Restart Manager がこちらを閉じにきて
/// 相互待ちになるため
fn update_via_windows_installer(info: &UpdateInfo) -> Result<String, String> {
    let url = info.download_url.as_deref().ok_or_else(|| {
        windows_manual_hint(info, "Windows 用インストーラーのアセットが見つかりません")
    })?;

    let tmp_dir = std::env::temp_dir().join("tako-update");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| windows_manual_hint(info, &format!("一時ディレクトリの作成に失敗: {e}")))?;
    let setup_path = tmp_dir.join(windows_setup_asset_name(&format!("v{}", info.version)));

    let downloaded =
        download_to_file(url, &setup_path).map_err(|e| windows_manual_hint(info, &e))?;
    let head = read_file_head(&setup_path, 2);
    verify_installer_bytes(&head, downloaded, info.download_size)
        .map_err(|e| windows_manual_hint(info, &e))?;

    std::process::Command::new(&setup_path)
        .args(windows_installer_args())
        .spawn()
        .map_err(|e| windows_manual_hint(info, &format!("インストーラーの起動に失敗: {e}")))?;

    Ok(format!(
        "v{} のインストーラーを起動しました（tako を終了して差し替えます）",
        info.version
    ))
}

/// ファイル先頭の n バイトだけ読む（マジックナンバー確認用。数十 MB を全部載せない）
fn read_file_head(path: &Path, n: usize) -> Vec<u8> {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut buf = Vec::with_capacity(n);
    let _ = file.take(n as u64).read_to_end(&mut buf);
    buf
}

/// URL の中身をファイルへ保存し、書き込んだバイト数を返す
fn download_to_file(url: &str, dest: &Path) -> Result<u64, String> {
    let mut body = ureq::get(url)
        .header("User-Agent", &format!("tako/{CURRENT_VERSION}"))
        .call()
        .map_err(|e| format!("ダウンロードに失敗: {e}"))?
        .into_body();
    let mut file =
        std::fs::File::create(dest).map_err(|e| format!("保存先ファイルの作成に失敗: {e}"))?;
    let written = std::io::copy(&mut body.as_reader(), &mut file)
        .map_err(|e| format!("ダウンロードの書き込みに失敗: {e}"))?;
    Ok(written)
}

/// .app バンドルを自動再起動する。呼び出し元プロセスは exit(0) で終了する想定。
/// macOS の `open -n` でバンドルを新プロセスとして起動し、自分は終了する。
///
/// Windows では何も起動しない。実行中の exe を握ったままだとインストーラーが
/// ファイルを差し替えられないので、ここは「終了してよい」を返すだけにして、
/// 新版の起動はインストーラー完了後にユーザーが行う（インストーラーの `[Run]` は
/// `skipifsilent` 付きでサイレント時は走らないため。自動再起動は #528 の残タスク）
pub fn restart_app() -> Result<(), String> {
    if UpdateTarget::current() == UpdateTarget::Windows {
        return Ok(());
    }
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
    let mut json = serde_json::json!({
        "current_version": CURRENT_VERSION,
        "current_channel": current_channel.label(),
        "install_method": method.label(),
        "duplicate_cli": duplicates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
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

    #[test]
    fn test_parse_releases_both_channels() {
        // 両プラットフォームの配布物が揃ったリリース = どの OS で走らせても同じ結果になる
        // （#528: アセットの無いリリースは Windows では通知しないので、素の JSON では
        //   ホスト依存になってしまう）
        let arr = vec![
            release_json(
                "v99.0.0",
                false,
                &["tako-v99.0.0-macos-arm64.zip", "tako-setup-v99.0.0-x64.exe"],
            ),
            release_json(
                "v99.0.1-test.1",
                true,
                &[
                    "tako-v99.0.1-test.1-macos-arm64.zip",
                    "tako-setup-v99.0.1-test.1-x64.exe",
                ],
            ),
        ];
        let result = parse_releases(&arr);
        assert!(result.stable.is_some());
        assert_eq!(result.stable.as_ref().unwrap().version, "99.0.0");
        assert!(result.test.is_some());
        assert_eq!(result.test.as_ref().unwrap().version, "99.0.1-test.1");
        assert!(result.rate_limit_note.is_none());
    }

    #[test]
    fn test_parse_releases_skips_older() {
        let releases = serde_json::json!([
            { "tag_name": "v0.0.1", "prerelease": false, "html_url": "" },
        ]);
        let arr: Vec<serde_json::Value> = serde_json::from_value(releases).unwrap();
        let result = parse_releases(&arr);
        assert!(result.stable.is_none());
        assert!(result.test.is_none());
    }

    #[test]
    fn test_parse_releases_invalid_tag() {
        let releases = serde_json::json!([
            { "tag_name": "nightly", "prerelease": false, "html_url": "" },
        ]);
        let arr: Vec<serde_json::Value> = serde_json::from_value(releases).unwrap();
        let result = parse_releases(&arr);
        assert!(result.stable.is_none());
    }

    // --- #528: プラットフォームごとの配布アセット解決 ---

    /// テスト用のリリース JSON を組み立てる。`assets` はアセット名のリスト
    fn release_json(tag: &str, prerelease: bool, assets: &[&str]) -> serde_json::Value {
        let assets: Vec<serde_json::Value> = assets
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "size": 12345,
                    "browser_download_url":
                        format!("https://github.com/{OWNER_REPO}/releases/download/{tag}/{name}"),
                })
            })
            .collect();
        serde_json::json!({
            "tag_name": tag,
            "prerelease": prerelease,
            "html_url": format!("https://github.com/{OWNER_REPO}/releases/tag/{tag}"),
            "assets": assets,
        })
    }

    #[test]
    fn test_windows_setup_asset_name_matches_installer() {
        // installer/windows/build-installer.ps1 の OutputBaseFilename と 1:1
        assert_eq!(
            windows_setup_asset_name("v0.6.0"),
            "tako-setup-v0.6.0-x64.exe"
        );
    }

    #[test]
    fn test_parse_releases_macos_url_unchanged() {
        // macOS は従来どおりアセット一覧を見ずに URL を組み立てる（挙動不変）
        let arr = vec![release_json("v99.0.0", false, &[])];
        let result = parse_releases_for(&arr, UpdateTarget::MacOs, "arm64");
        let info = result.stable.expect("macOS では通知が出る");
        assert_eq!(
            info.download_url.as_deref(),
            Some("https://github.com/takushio2525/tako/releases/download/v99.0.0/tako-v99.0.0-macos-arm64.zip")
        );
        assert_eq!(info.download_size, None);
        // arch はそのまま名前に載る
        let result = parse_releases_for(&arr, UpdateTarget::MacOs, "x86_64");
        assert!(result
            .stable
            .unwrap()
            .download_url
            .unwrap()
            .contains("macos-x86_64"));
    }

    #[test]
    fn test_parse_releases_windows_ignores_macos_only_release() {
        // 実例（v0.5.13）と同じ形: macOS の zip しか無いリリース。
        // Windows で通知を出すと「押すと必ず失敗する」ので通知そのものを出さない
        let arr = vec![release_json(
            "v99.0.0",
            false,
            &["tako-v99.0.0-macos-arm64.zip"],
        )];
        let win = parse_releases_for(&arr, UpdateTarget::Windows, "x86_64");
        assert!(
            win.stable.is_none(),
            "Windows 用アセットが無いのに通知が出た"
        );
        assert!(win.test.is_none());
        // 同じ JSON で macOS には通知が出る = 「現行版より新しい」ことは満たしている
        let mac = parse_releases_for(&arr, UpdateTarget::MacOs, "arm64");
        assert!(
            mac.stable.is_some(),
            "フィクスチャが現行版より古い（テストが空振り）"
        );
    }

    #[test]
    fn test_parse_releases_windows_uses_setup_asset() {
        let arr = vec![release_json(
            "v99.0.0",
            false,
            &["tako-v99.0.0-macos-arm64.zip", "tako-setup-v99.0.0-x64.exe"],
        )];
        let info = parse_releases_for(&arr, UpdateTarget::Windows, "x86_64")
            .stable
            .expect("Windows 用アセットがあるので通知が出る");
        assert_eq!(
            info.download_url.as_deref(),
            Some("https://github.com/takushio2525/tako/releases/download/v99.0.0/tako-setup-v99.0.0-x64.exe")
        );
        // 整合確認に使う申告サイズを拾っている
        assert_eq!(info.download_size, Some(12345));
    }

    #[test]
    fn test_parse_releases_windows_falls_back_to_older_release_with_asset() {
        // 最新の安定版が macOS だけでも、Windows 版がある 1 つ前（現行版より新しい）を拾う
        let arr = vec![
            release_json("v99.1.0", false, &["tako-v99.1.0-macos-arm64.zip"]),
            release_json("v99.0.0", false, &["tako-setup-v99.0.0-x64.exe"]),
        ];
        let info = parse_releases_for(&arr, UpdateTarget::Windows, "x86_64")
            .stable
            .expect("Windows 版のある古い方を拾う");
        assert_eq!(info.version, "99.0.0");
        // macOS は最新をそのまま拾う
        let mac = parse_releases_for(&arr, UpdateTarget::MacOs, "arm64")
            .stable
            .unwrap();
        assert_eq!(mac.version, "99.1.0");
    }

    #[test]
    fn test_parse_releases_windows_empty_or_missing_assets() {
        // assets が空配列
        let arr = vec![release_json("v99.0.0", false, &[])];
        assert!(parse_releases_for(&arr, UpdateTarget::Windows, "x86_64")
            .stable
            .is_none());
        // assets キー自体が無い（API 形式が変わった / 部分レスポンス）
        let arr = vec![serde_json::json!({
            "tag_name": "v99.0.0",
            "prerelease": false,
            "html_url": "",
        })];
        assert!(parse_releases_for(&arr, UpdateTarget::Windows, "x86_64")
            .stable
            .is_none());
        // macOS 側は従来どおり（アセット一覧に依存しない）
        assert!(parse_releases_for(&arr, UpdateTarget::MacOs, "arm64")
            .stable
            .is_some());
    }

    #[test]
    fn test_parse_releases_windows_channels_independent() {
        // stable は macOS だけ / test には Windows 版がある → test だけ通知
        let arr = vec![
            release_json("v99.1.0", false, &["tako-v99.1.0-macos-arm64.zip"]),
            release_json(
                "v99.1.0-test.1",
                true,
                &["tako-setup-v99.1.0-test.1-x64.exe"],
            ),
        ];
        let win = parse_releases_for(&arr, UpdateTarget::Windows, "x86_64");
        assert!(win.stable.is_none());
        assert_eq!(win.test.as_ref().unwrap().version, "99.1.0-test.1");
        assert_eq!(win.test.as_ref().unwrap().channel, Channel::Test);

        // 逆（stable に Windows 版 / test は macOS だけ）
        let arr = vec![
            release_json("v99.1.0", false, &["tako-setup-v99.1.0-x64.exe"]),
            release_json(
                "v99.1.0-test.1",
                true,
                &["tako-v99.1.0-test.1-macos-arm64.zip"],
            ),
        ];
        let win = parse_releases_for(&arr, UpdateTarget::Windows, "x86_64");
        assert_eq!(win.stable.as_ref().unwrap().version, "99.1.0");
        assert!(win.test.is_none());
        // macOS は両方出る
        let mac = parse_releases_for(&arr, UpdateTarget::MacOs, "arm64");
        assert!(mac.stable.is_some() && mac.test.is_some());
    }

    #[test]
    fn test_find_release_asset_url_fallback() {
        // browser_download_url が欠けていてもタグから組み立てる
        let release = serde_json::json!({
            "assets": [{ "name": "tako-setup-v9.9.9-x64.exe" }],
        });
        let (url, size) =
            find_release_asset(&release, "v9.9.9", "tako-setup-v9.9.9-x64.exe").unwrap();
        assert_eq!(
            url,
            "https://github.com/takushio2525/tako/releases/download/v9.9.9/tako-setup-v9.9.9-x64.exe"
        );
        assert_eq!(size, None);
        // 名前が違えば None
        assert!(find_release_asset(&release, "v9.9.9", "tako-setup-v9.9.8-x64.exe").is_none());
    }

    // --- #528: Windows 更新実行のヘルパー ---

    #[test]
    fn test_windows_installer_args_are_silent() {
        let args = windows_installer_args();
        assert!(
            args.contains(&"/SILENT"),
            "サイレント起動でないとウィザードで止まる"
        );
        assert!(args.contains(&"/NORESTART"));
    }

    #[test]
    fn test_verify_installer_bytes() {
        // 正常（PE ヘッダ MZ + 申告サイズ一致）
        assert!(verify_installer_bytes(b"MZ", 100, Some(100)).is_ok());
        // 申告サイズが無くても MZ なら通す
        assert!(verify_installer_bytes(b"MZ", 100, None).is_ok());
        // 空
        assert!(verify_installer_bytes(b"", 0, None)
            .unwrap_err()
            .contains("空"));
        // サイズ不一致（途中で切れた）
        let err = verify_installer_bytes(b"MZ", 99, Some(100)).unwrap_err();
        assert!(err.contains("サイズ"), "{err}");
        // HTML のエラーページを掴んだ
        let err = verify_installer_bytes(b"<!", 100, Some(100)).unwrap_err();
        assert!(err.contains("実行ファイル"), "{err}");
    }

    #[test]
    fn test_release_page_url_and_manual_hint() {
        let mut info = UpdateInfo {
            version: "99.0.0".into(),
            channel: Channel::Stable,
            html_url: "https://github.com/takushio2525/tako/releases/tag/v99.0.0".into(),
            download_url: None,
            download_size: None,
        };
        assert_eq!(release_page_url(&info), info.html_url);
        let hint = windows_manual_hint(&info, "起動に失敗");
        assert!(hint.contains("起動に失敗") && hint.contains(&info.html_url));

        // html_url が空ならリリース一覧へ誘導する
        info.html_url = String::new();
        assert_eq!(
            release_page_url(&info),
            "https://github.com/takushio2525/tako/releases"
        );
    }

    #[test]
    fn test_update_target_current_matches_platform() {
        let expected = if cfg!(windows) {
            UpdateTarget::Windows
        } else {
            UpdateTarget::MacOs
        };
        assert_eq!(UpdateTarget::current(), expected);
    }

    #[test]
    fn test_perform_update_zip_rejected_on_windows() {
        let info = UpdateInfo {
            version: "99.0.0".into(),
            channel: Channel::Stable,
            html_url: String::new(),
            download_url: Some("https://example.invalid/tako.zip".into()),
            download_size: None,
        };
        if UpdateTarget::current() == UpdateTarget::Windows {
            let err = perform_update_zip(&info).unwrap_err();
            assert!(err.contains("macOS 専用"), "{err}");
            assert!(err.contains("releases"), "手動更新の案内が無い: {err}");
        }
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
