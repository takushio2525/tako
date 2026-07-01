//! アプリ内自動更新チェッカー
//!
//! GitHub Releases API を定期確認し、新しい安定版があればステータスバーに通知。
//! ボタン押下で Homebrew or ZIP 差し替えによる自動更新を実行する。

use std::path::Path;
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
    /// ダウンロード/更新中
    Updating(String),
    /// 更新完了（再起動を促す）
    Done(String),
    /// 更新失敗
    Failed(String),
    /// ユーザーが閉じた（次回起動まで非表示）
    Dismissed,
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
    // 安定版のみ（draft でも prerelease でもない）
    let stable: Vec<&GhRelease> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .collect();
    let latest = stable.first()?;
    let latest_ver = latest.tag_name.trim_start_matches('v');
    if !is_newer(latest_ver, CURRENT_VERSION) {
        return None;
    }
    // macOS 用 ZIP アセットを探す
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
fn is_newer(a: &str, b: &str) -> bool {
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

/// インストール方式の判定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Homebrew,
    Manual,
}

pub fn detect_install_method() -> InstallMethod {
    // Homebrew Cask 経由のシンボリックリンクの存在で判定
    let homebrew_path = Path::new("/opt/homebrew/bin/tako");
    if homebrew_path.exists() || homebrew_path.is_symlink() {
        return InstallMethod::Homebrew;
    }
    // Intel Mac
    let usr_local = Path::new("/usr/local/bin/tako");
    if usr_local.exists() || usr_local.is_symlink() {
        return InstallMethod::Homebrew;
    }
    InstallMethod::Manual
}

/// 更新を実行する（ブロッキング。background executor で呼ぶ）。
/// 戻り値は (成功メッセージ or エラーメッセージ, 成功したか)
pub fn perform_update(info: &UpdateInfo) -> Result<String, String> {
    match detect_install_method() {
        InstallMethod::Homebrew => update_via_homebrew(),
        InstallMethod::Manual => update_via_zip(info),
    }
}

fn update_via_homebrew() -> Result<String, String> {
    let output = std::process::Command::new("brew")
        .args(["upgrade", "--cask", "takushio2525/tako/tako"])
        .output()
        .map_err(|e| format!("brew の実行に失敗: {e}"))?;
    if output.status.success() {
        Ok("Homebrew で更新完了。アプリを再起動してください".into())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // "already installed" は成功扱い
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

    // ダウンロード
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

    // ditto で展開（macOS のフレームワーク・署名を正しく扱う）
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

    // tako.app を探す
    let extracted_app = find_app_bundle(&tmp_dir)
        .ok_or_else(|| "展開した ZIP に tako.app が見つかりません".to_string())?;

    // /Applications/tako.app を差し替え
    let dest = Path::new("/Applications/tako.app");
    if dest.exists() {
        // バックアップを作ってから差し替え（rm -rf は危険なのでリネーム）
        let backup = Path::new("/Applications/tako.app.bak");
        let _ = std::fs::remove_dir_all(backup);
        std::fs::rename(dest, backup)
            .map_err(|e| format!("/Applications/tako.app のバックアップに失敗: {e}"))?;
    }
    // ditto でコピー（cp -R より安全）
    let output = std::process::Command::new("ditto")
        .args([
            &extracted_app.to_string_lossy().to_string(),
            &dest.to_string_lossy().to_string(),
        ])
        .output()
        .map_err(|e| format!("アプリのコピーに失敗: {e}"))?;
    if !output.status.success() {
        // バックアップから復旧
        let backup = Path::new("/Applications/tako.app.bak");
        if backup.exists() {
            let _ = std::fs::rename(backup, dest);
        }
        return Err(format!(
            "アプリのインストールに失敗: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // クリーンアップ
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let _ = std::fs::remove_dir_all(Path::new("/Applications/tako.app.bak"));

    Ok("更新完了。アプリを再起動してください".into())
}

/// ディレクトリ内の *.app バンドルを再帰的に探す
fn find_app_bundle(dir: &Path) -> Option<std::path::PathBuf> {
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
}
