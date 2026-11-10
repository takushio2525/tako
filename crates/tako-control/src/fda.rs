//! フルディスクアクセス (FDA) の検出と案内
//!
//! macOS TCC のフォルダアクセス許可ダイアログを一括で消すには、ユーザーが手動で
//! 「システム設定 → プライバシーとセキュリティ → フルディスクアクセス」に tako を追加する
//! 必要がある。このモジュールは FDA の付与状態をベストエフォートで検出し、
//! 設定画面を開く機能を提供する。

// OS ごとの腕（対象の OS か・付与の判定・設定画面を開く）は境界 B15 `crate::platform::fda` が持つ
// （#2859）。このモジュールは OS を意識しない層 = 日本語の口と CLI / MCP 向けの状態の組み立てだけを持つ
pub use crate::platform::fda::{is_granted, open_settings_typed};

/// macOS のシステム設定を開いてフルディスクアクセスのパネルを表示する。
/// 成功したら Ok(())、開けなかったらエラーメッセージ（日本語）を返す
pub fn open_settings() -> Result<(), String> {
    open_settings_typed().map_err(crate::OsRefusal::into_ja)
}

/// FDA のステータス情報（CLI / MCP 向け）
pub fn status_info() -> FdaStatus {
    FdaStatus {
        granted: is_granted(),
        platform_supported: crate::platform::fda::supported(),
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
            // 組んだ時点の表示言語（#2365。CLI の直呼びは呼ぶ前に表示言語を決める）
            "description": if !self.platform_supported {
                crate::ReplyNote::FdaNotApplicable
            } else if self.granted {
                crate::ReplyNote::FdaGrantedDescription
            } else {
                crate::ReplyNote::FdaNotGrantedDescription
            }
            .to_string()
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
