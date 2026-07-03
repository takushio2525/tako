//! アプリ内自動更新チェッカー
//!
//! GitHub Releases API を定期確認し、新しい安定版があればステータスバーに通知。
//! 配布系統（Homebrew / zip 手動配置）を自動判別し、系統内で更新を実行する。
//! 更新完了後は自動再起動する（#30 のタブ永続化で構成は復元される）。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 更新チェック間隔（24 時間）
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 現在のバージョン（Cargo.toml から埋め込み）
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const RELEASES_URL: &str = "https://api.github.com/repos/takushio2525/tako/releases";

/// GitHub Release の最小構造（API レスポンスの必要フィールドだけ）
#[derive(Debug, Clone, serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    html_url: String,
    assets: Vec<GhAsset>,
}

/// リリースアセット
#[derive(Debug, Clone, serde::Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// 更新チェック結果
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    #[allow(dead_code)]
    pub html_url: String,
    pub download_url: Option<String>,
}

/// UI に公開する更新状態
#[derive(Debug, Clone)]
pub enum UpdateState {
    /// チェック中 or 更新なし
    Idle,
    /// 新しいバージョンが利用可能
    Available(UpdateInfo),
    /// 更新確認ダイアログ表示中
    ConfirmPending(UpdateInfo),
    /// ダウンロード/更新中
    Updating(String),
    /// 更新完了 — 自動再起動する
    Done(String),
    /// 更新失敗
    Failed(String),
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
}

impl InstallMethod {
    pub fn label(&self) -> &'static str {
        match self {
            InstallMethod::Homebrew => "homebrew",
            InstallMethod::Zip => "zip",
        }
    }
}

/// 配布系統の判別 — 実行中の .app が Caskroom 配下にあるかで判定する。
/// `/opt/homebrew/bin/tako` のシンボリックリンク存在だけでは zip 版に brew を
/// 被せたケースと区別できないため、**バンドル自体の出自**を見る。
pub fn detect_install_method() -> InstallMethod {
    if let Some(bundle) = app_bundle_path() {
        let resolved = std::fs::canonicalize(&bundle).unwrap_or(bundle);
        let s = resolved.to_string_lossy();
        // Homebrew Cask は /opt/homebrew/Caskroom/tako/... or /usr/local/Caskroom/tako/...
        // にインストールし、/Applications にシンボリックリンクを張る
        if s.contains("/Caskroom/") {
            return InstallMethod::Homebrew;
        }
    }
    // バンドルパスが取れない場合（CLI 単体実行）は tako バイナリの出自で判定
    if let Ok(exe) = std::env::current_exe() {
        let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
        let s = resolved.to_string_lossy();
        if s.contains("/Caskroom/") || s.contains("/Cellar/") {
            return InstallMethod::Homebrew;
        }
    }
    InstallMethod::Zip
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

/// GitHub Releases から最新の安定版を取得する（ブロッキング。background executor で呼ぶ）
pub fn check_latest() -> Option<UpdateInfo> {
    let body: String = ureq::get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", &format!("tako/{CURRENT_VERSION}"))
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let releases: Vec<GhRelease> = serde_json::from_str(&body).ok()?;
    let stable: Vec<&GhRelease> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .collect();
    let latest = stable.first()?;
    let latest_ver = latest.tag_name.trim_start_matches('v');
    if !is_newer(latest_ver, CURRENT_VERSION) {
        return None;
    }
    let zip_asset = latest.assets.iter().find(|a| {
        let name = a.name.to_lowercase();
        name.ends_with(".zip")
            && (name.contains("macos") || name.contains("darwin") || name.contains("tako"))
    });
    Some(UpdateInfo {
        version: latest_ver.to_string(),
        html_url: latest.html_url.clone(),
        download_url: zip_asset.map(|a| a.browser_download_url.clone()),
    })
}

/// semver 比較（a > b なら true）。不正な形式は false
pub fn is_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// 更新を実行する（ブロッキング。background executor で呼ぶ）
pub fn perform_update(info: &UpdateInfo) -> Result<String, String> {
    match detect_install_method() {
        InstallMethod::Homebrew => update_via_homebrew(),
        InstallMethod::Zip => update_via_zip(info),
    }
}

fn update_via_homebrew() -> Result<String, String> {
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
            Err(format!("brew upgrade が失敗: {stderr}"))
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
    std::process::Command::new("open")
        .args(["-n", &bundle.to_string_lossy()])
        .spawn()
        .map_err(|e| format!("再起動に失敗: {e}"))?;
    Ok(())
}

/// dispatch 層に公開する更新情報の JSON 表現
pub fn update_status_json() -> serde_json::Value {
    let method = detect_install_method();
    let duplicates = detect_duplicate_cli();
    serde_json::json!({
        "current_version": CURRENT_VERSION,
        "install_method": method.label(),
        "duplicate_cli": duplicates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    })
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
    fn test_detect_install_method_returns_value() {
        let method = detect_install_method();
        assert!(method == InstallMethod::Homebrew || method == InstallMethod::Zip);
    }

    #[test]
    fn test_install_method_label() {
        assert_eq!(InstallMethod::Homebrew.label(), "homebrew");
        assert_eq!(InstallMethod::Zip.label(), "zip");
    }

    #[test]
    fn test_detect_duplicate_cli_runs() {
        let _ = detect_duplicate_cli();
    }

    #[test]
    fn test_update_status_json() {
        let json = update_status_json();
        assert!(json.get("current_version").is_some());
        assert!(json.get("install_method").is_some());
        assert!(json.get("duplicate_cli").is_some());
    }
}
