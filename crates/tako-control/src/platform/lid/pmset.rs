//! 蓋を閉じたまま走らせ続ける制御（境界 B9 の蓋ぶん）の **macOS の手段**: sudoers の限定登録と
//! `sudo -n pmset -a disablesleep 0/1`（#218）。
//!
//! #2857 で `sleep_guard.rs` から移した（中身は移設の前後でトークン列が一致。違いは可視性だけ =
//! `sleep_guard` が使う定数へ `pub`、親の腕が呼ぶ判定へ `pub(super)`）。移す前と同じく **どの OS でもコンパイルする**
//! （`sleep_guard::is_sudoers_installed` は dispatch が OS を問わず返す欄で、残留の判定
//! [`should_clear_residual`] は Windows の CI でも単体テストを回す）。実際に `pmset` を起こすのは
//! macOS の腕（`super` の `cfg(target_os = "macos")` の塊）だけ。
//!
//! sudoers への登録・削除そのもの（osascript の管理者プロンプト）は断りの型（`OsRefusal`）を返すので
//! `sleep_guard` に残している（`install_sudoers_typed` / `remove_sudoers_done`）。

pub const SUDOERS_FILE: &str = "/etc/sudoers.d/tako-sleep-guard";
pub const SUDOERS_CONTENT: &str = "\
# tako sleep guard: pmset disablesleep のみ NOPASSWD (#218)
# アンインストール: sudo rm /etc/sudoers.d/tako-sleep-guard
%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0
%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1
";

/// sudoers.d/tako-sleep-guard が登録済みか（**macOS 固有の手段**）
pub fn is_sudoers_installed() -> bool {
    std::path::Path::new(SUDOERS_FILE).exists()
}

/// sudo pmset -a disablesleep 0/1 を実行する（sudoers 登録済み前提、NOPASSWD）
pub fn set_disablesleep(enable: bool) -> Result<(), String> {
    if !is_sudoers_installed() {
        return Err("sudoers 未登録（tako setup --lid-sleep で登録してください）".to_string());
    }
    let val = if enable { "1" } else { "0" };
    // GUI のスリープ防止の周期が状態の変わり目ごとに起こすので PATH で引いた絶対パスで起こす（#2519）
    let output = std::process::Command::new(tako_core::platform::exe::resolve_for_spawn("sudo"))
        .args(["-n", "/usr/bin/pmset", "-a", "disablesleep", val])
        .output()
        .map_err(|e| format!("pmset の実行に失敗: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("pmset disablesleep {val} に失敗: {stderr}"))
    }
}

/// 残留解除を行うべきかを判定する（#449: テスト可能な純粋関数）。
/// Ok(()) なら解除すべき、Err(理由) ならスキップ。呼び手は macOS の腕の残留解除（`super::clear_startup_residual`）とテストだけ
/// （Windows は `platform::lid::should_clear_residual`。#1537）
#[cfg(any(target_os = "macos", test))]
pub(super) fn should_clear_residual(
    is_isolated: bool,
    other_instance_running: bool,
    sudoers_installed: bool,
    sleep_disabled: bool,
) -> Result<(), &'static str> {
    if is_isolated {
        return Err("隔離モード（TAKO_ISOLATED）のためスキップ");
    }
    if other_instance_running {
        return Err("他の tako プロセスが動作中のためスキップ");
    }
    if !sudoers_installed {
        return Err("sudoers 未登録");
    }
    if !sleep_disabled {
        return Err("disablesleep=0（残留なし）");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- #449: should_clear_residual のガード条件テスト ---

    #[test]
    fn residual_skip_when_isolated() {
        let r = should_clear_residual(true, false, true, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("隔離モード"));
    }

    #[test]
    fn residual_skip_when_other_instance_running() {
        let r = should_clear_residual(false, true, true, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("他の tako"));
    }

    #[test]
    fn residual_skip_when_no_sudoers() {
        let r = should_clear_residual(false, false, false, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("sudoers"));
    }

    #[test]
    fn residual_skip_when_not_disabled() {
        let r = should_clear_residual(false, false, true, false);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("残留なし"));
    }

    #[test]
    fn residual_clear_when_solo_and_disabled() {
        let r = should_clear_residual(false, false, true, true);
        assert!(r.is_ok());
    }

    #[test]
    fn residual_isolated_takes_priority_over_other_checks() {
        let r = should_clear_residual(true, true, true, true);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("隔離モード"));
    }
}
