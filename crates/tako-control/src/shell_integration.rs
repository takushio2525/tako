//! シェル統合の配置操作（#525）— CLI・MCP・dispatch が共有する 1 実装
//!
//! 判定と書き込みの本体は `tako_core::shell_integration`（抽象境界 B13）。
//! ここは「action の受理」と「応答 JSON の形」だけに責任を持つ。
//! `platform::report` と同じで **GUI を必要としない**ので、CLI からはローカル呼び出しで、
//! MCP からは dispatch 経由で、まったく同じ結果になる。

use serde_json::{json, Value};
use tako_core::shell_integration as si;

/// 受理する操作。**既定は `status`**（#322: 素のコマンドが一番安全な既定で動く）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Status,
    Install,
    Uninstall,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "status" => Some(Self::Status),
            "install" => Some(Self::Install),
            "uninstall" => Some(Self::Uninstall),
            _ => None,
        }
    }

    pub const VALUES: &'static [&'static str] = &["status", "install", "uninstall"];
}

/// 操作を実行して応答 JSON を返す。
///
/// **どの action でも最後に `status` を載せる**。install / uninstall の直後に
/// 「いま実際にどうなっているか」を別呼び出しで確かめさせないため
/// （AI が 1 往復で完結できる = 開発不変条件 5 の趣旨）
pub fn run(action: Option<&str>) -> Result<Value, String> {
    let raw = action.unwrap_or("status");
    let action = Action::parse(raw).ok_or_else(|| {
        format!(
            "不明な action: {raw:?}（{} のいずれか）",
            Action::VALUES.join(" / ")
        )
    })?;

    let changes = match action {
        Action::Status => Vec::new(),
        Action::Install => si::install()?,
        Action::Uninstall => si::uninstall()?,
    };

    let mut out = si::status().describe();
    out["action"] = json!(raw);
    out["changes"] = json!(changes.iter().map(|c| c.describe()).collect::<Vec<_>>());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_の受理と拒否() {
        assert_eq!(Action::parse("status"), Some(Action::Status));
        assert_eq!(Action::parse("install"), Some(Action::Install));
        assert_eq!(Action::parse("uninstall"), Some(Action::Uninstall));
        assert_eq!(Action::parse("Install"), None, "大文字は受理しない");
        assert_eq!(Action::parse(""), None);
        assert_eq!(Action::parse("remove"), None);
    }

    #[test]
    fn 既定は_status_で状態だけ返す() {
        let out = run(None).expect("status は常に成功する");
        assert_eq!(out["action"], "status");
        assert_eq!(out["changes"], json!([]));
        // 応答の形（CLI / MCP が読むキー）が揃っていること
        for key in [
            "delivery",
            "shells",
            "installed",
            "effective",
            "targets",
            "blocked_by_backend",
        ] {
            assert!(out.get(key).is_some(), "{key} が応答に無い: {out}");
        }
    }

    #[test]
    fn 不明な_action_は選択肢つきで拒否する() {
        let err = run(Some("enable")).expect_err("不明な action は失敗する");
        assert!(err.contains("enable"), "{err}");
        // 何を渡せばいいかがエラーだけで分かること
        for v in Action::VALUES {
            assert!(err.contains(v), "選択肢 {v} が案内に無い: {err}");
        }
    }

    /// unix は env 注入で完結するので配置対象を持たない。
    /// **「配置済み」と報告されるが「解除するものは無い」**という組み合わせを固定する
    #[cfg(unix)]
    #[test]
    fn unix_は自動配置で解除対象を持たない() {
        let out = run(None).unwrap();
        assert_eq!(out["delivery"], "automatic");
        assert_eq!(out["installed"], true);
        assert_eq!(out["targets"], json!([]));

        let err = run(Some("uninstall")).expect_err("解除する配置が無い");
        assert!(err.contains("環境変数"), "{err}");
    }
}
