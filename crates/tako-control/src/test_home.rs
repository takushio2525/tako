//! テストが `HOME` を一時的に差し替えるためのガード（#893）
//!
//! ホームを見る機能のテストは、実ユーザーのホームを読まないように `HOME` を
//! 一時ディレクトリへ向ける。この差し替えは
//!
//! 1. **元へ戻さないと後続のテストへ漏れる**（同一プロセスで並ぶので影響が残る）
//! 2. **panic すると戻す行に到達しない**（旧実装は手書きの復元だったので、
//!    assert が落ちた回だけ `HOME` が一時ディレクトリのまま残っていた）
//! 3. 元が**未設定**だった場合は `remove_var` で消さないと元に戻らない
//!
//! の 3 つを取りこぼしやすいので、`Drop` で必ず戻す 1 実装に寄せてある。
//!
//! ホーム解決の**正本**は [`tako_core::paths::home_dir`]（#870 / #893）。
//! ここは「解決」ではなく「差し替えと復元」なので、控えを取る目的で
//! `HOME` を直接読む唯一の場所（番犬 `home_dir_watchdog` が名指しで許可している）。

use std::ffi::OsString;
use std::path::Path;

/// `HOME` を差し替え、スコープを抜けたら（panic でも）元へ戻す
pub(crate) struct HomeGuard {
    original: Option<OsString>,
}

impl HomeGuard {
    /// `HOME` を `dir` へ向ける
    pub(crate) fn set(dir: &Path) -> Self {
        let original = std::env::var_os("HOME");
        std::env::set_var("HOME", dir);
        Self { original }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(home) => std::env::set_var("HOME", home),
            // 元が未設定なら消す（空文字を残すと `home_dir()` の意味が変わる）
            None => std::env::remove_var("HOME"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// panic しても元へ戻ること（旧実装の手書き復元はここで漏れていた）
    #[test]
    fn panicしてもhomeは元へ戻る() {
        let before = std::env::var_os("HOME");
        let tmp = std::env::temp_dir().join("tako-test-home-guard");
        let caught = std::panic::catch_unwind(|| {
            let _guard = HomeGuard::set(&tmp);
            assert_eq!(std::env::var_os("HOME"), Some(tmp.clone().into_os_string()));
            panic!("テスト本体が落ちた場合");
        });
        assert!(caught.is_err(), "panic は伝わる");
        assert_eq!(std::env::var_os("HOME"), before, "HOME は元へ戻る");
    }
}
