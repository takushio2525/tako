//! プレビューのライブリロード設定（Issue #233）。
//!
//! GPUI や OS のファイル監視 API へ依存せず、GUI・dispatch・CLI・MCP が同じ
//! ON/OFF セマンティクスを使うためのドメイン状態を提供する。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewReloadState {
    enabled: bool,
}

impl PreviewReloadState {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn enabled(self) -> bool {
        self.enabled
    }

    /// 有効状態を更新し、実際に変化したかを返す。
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return false;
        }
        self.enabled = enabled;
        true
    }
}

impl Default for PreviewReloadState {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 既定は有効で変更有無を返す() {
        let mut state = PreviewReloadState::default();
        assert!(state.enabled());
        assert!(!state.set_enabled(true));
        assert!(state.set_enabled(false));
        assert!(!state.enabled());
    }
}
