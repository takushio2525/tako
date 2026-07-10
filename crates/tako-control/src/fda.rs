//! フルディスクアクセス (FDA) の検出と案内
//!
//! macOS TCC のフォルダアクセス許可ダイアログを一括で消すには、ユーザーが手動で
//! 「システム設定 → プライバシーとセキュリティ → フルディスクアクセス」に tako を追加する
//! 必要がある。このモジュールは FDA の付与状態をベストエフォートで検出し、
//! 設定画面を開く機能を提供する。

use std::path::PathBuf;

/// FDA が付与されているかをベストエフォートで検出する。
/// macOS に直接の API は無いため、FDA 保護下のパスへの read_dir 試行で判定する。
/// - true: FDA 付与済み（保護パスを読めた）
/// - false: 未付与（Permission denied）
/// - macOS 以外では常に true を返す（TCC が無いため判定不要）
pub fn is_granted() -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        // ~/Library/Mail は kTCCServiceSystemPolicyAllFiles で保護されている。
        // FDA が無いと read_dir で Permission denied になる（ディレクトリが存在する場合）。
        // 存在しない場合は別のプローブパスを試す
        let probes = fda_probe_paths();
        for path in &probes {
            if !path.exists() {
                continue;
            }
            match std::fs::read_dir(path) {
                Ok(_) => return true,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return false,
                Err(_) => continue, // 他のエラー（破損等）はスキップ
            }
        }
        // プローブパスがすべて存在しない場合は判定不能 → 付与済みと仮定
        // （ダイアログが出ていないなら問題ない）
        true
    }
}

/// FDA のプローブに使うパス群。存在する順に試行する
#[cfg(target_os = "macos")]
fn fda_probe_paths() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    vec![
        home.join("Library/Mail"),
        home.join("Library/Safari"),
        home.join("Library/Messages"),
        home.join("Library/Cookies"),
    ]
}

/// macOS のシステム設定を開いてフルディスクアクセスのパネルを表示する。
/// 成功したら Ok(())、開けなかったらエラーメッセージを返す
pub fn open_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // macOS 13+ のシステム設定 URL スキーム
        let urls = [
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles",
        ];
        for url in &urls {
            let status = std::process::Command::new("open")
                .arg(url)
                .status()
                .map_err(|e| format!("open コマンドの実行に失敗: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }
        Err("システム設定のフルディスクアクセスパネルを開けませんでした".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("フルディスクアクセスは macOS 専用の機能です".into())
    }
}

/// FDA のステータス情報（CLI / MCP 向け）
pub fn status_info() -> FdaStatus {
    FdaStatus {
        granted: is_granted(),
        platform_supported: cfg!(target_os = "macos"),
    }
}

/// FDA の状態を表す構造体
#[derive(Debug, Clone)]
pub struct FdaStatus {
    pub granted: bool,
    pub platform_supported: bool,
}

impl FdaStatus {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "granted": self.granted,
            "platform_supported": self.platform_supported,
            "description": if !self.platform_supported {
                "macOS 以外のプラットフォームでは TCC は適用されません"
            } else if self.granted {
                "フルディスクアクセスが付与済みです。フォルダアクセスの許可ダイアログは表示されません"
            } else {
                "フルディスクアクセスが未付与です。フォルダアクセス時に許可ダイアログが表示されることがあります"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_info_returns_valid_json() {
        let status = status_info();
        let json = status.to_json();
        assert!(json.get("granted").is_some());
        assert!(json.get("platform_supported").is_some());
        assert!(json.get("description").is_some());
    }

    #[test]
    fn is_granted_does_not_panic() {
        // パニックしないことと何らかの bool が返ることの確認
        let _result = is_granted();
    }
}
