//! 蓋を閉じたまま走らせ続ける制御（境界 B9 の蓋ぶん）の **macOS の手段**: sudoers の限定登録と
//! `sudo -n pmset -a disablesleep 0/1`（#218）。
//!
//! #2857 で `sleep_guard.rs` から移した（中身は移設の前後でトークン列が一致。違いは可視性だけ =
//! `sleep_guard` が使う定数へ `pub`、親の腕が呼ぶ判定へ `pub(super)`）。移す前と同じく **どの OS でもコンパイルする**
//! （`sleep_guard::is_sudoers_installed` は dispatch が OS を問わず返す欄で、残留の判定
//! [`should_clear_residual`] と倒した主の判定 [`decide`] は Windows の CI でも単体テストを回す）。
//! 実際に `pmset` を起こすのは macOS の腕（`super` の `cfg(target_os = "macos")` の塊）だけ。
//!
//! sudoers への登録・削除そのもの（osascript の管理者プロンプト）は断りの型（`OsRefusal`）を返すので
//! `sleep_guard` に残している（`install_sudoers_typed` / `remove_sudoers_done`）。
//!
//! ## 倒した主の記録（#2904）
//!
//! `disablesleep` は**機械全体の設定**なので、#2904 より前の「いまの値だけを見て戻す」腕は、検証起動の
//! tako の正常終了・設定 off のどちらでも**別の tako（本番の GUI）が倒した上書き**を区別できなかった
//! （本番の persist.log の再有効化 362 回に対し本番自身の解除は 5 回 = 本番の外から 0 へ戻され続けた。
//! 統合ログで、本番以外の tako-app の正規の終了 93 回のうち 92 回の 4〜70 ms 後に `sudo` が起きていた）。
//! そこで電源プランの腕（#1373）と同じ所有者つきの記録（`lid-guard.json` の `method: pmset`）を持ち、
//! 倒す・戻すを次の 1 実装で決める:
//!
//! - 判定 [`decide`]: 倒したいか × いまの値 × 記録に対する立場 → [`PmsetAction`]
//! - **自分のもの以外**（持ち主の死んだ記録）へ手を出すときだけ、起動時の残留チェックと**同じ門**
//!   （[`residual_gate`] = 隔離されていない・他の tako が居ない）を通す
//! - 自分が倒した上書きには門を掛けない（掛けると、検証起動が走っている間に終了した本番が
//!   自分の上書きを残す = #218 が防いでいる「蓋を閉じてもスリープしない」残留になる）
//! - 記録の無い上書き（tako 以外・別の data dir の tako・#2904 より前の版が倒したもの）は戻さない

#[cfg(any(target_os = "macos", test))]
use super::RecordClaim;

pub const SUDOERS_FILE: &str = "/etc/sudoers.d/tako-sleep-guard";
pub const SUDOERS_CONTENT: &str = "\
# tako sleep guard: pmset disablesleep のみ NOPASSWD (#218)
# アンインストール: sudo rm /etc/sudoers.d/tako-sleep-guard
%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0
%admin ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1
";

/// 記録（`lid-guard.json`）の `scheme` に入れる手段名（#2904。電源プランの GUID と取り違えない）
pub const RECORD_SCHEME: &str = "pmset-disablesleep";

/// A/B: `1` で #2904 より前の腕（記録を持たず、いまの値だけを見て倒す・戻す。設定 off の間は触らない）
pub const LEGACY_ENV: &str = "TAKO_2904_LEGACY";

/// 検証用の偽の `sudo` / `pmset` の置き場（#2904）。**検証プロセス**
/// （`tako_core::paths::is_verification_process` = テスト・隔離起動・セルフテスト）のときだけ読み、
/// 本番の GUI では効かない。中身:
///
/// - `<dir>/sudo`: `sudo` の代わりに起こすもの（引数は本物と同じ `-n /usr/bin/pmset -a disablesleep 0|1`）
/// - `<dir>/SleepDisabled`: いまの値（`1` = 倒れている）。IORegistry の代わりに読む
/// - `<dir>/sudoers`: 在れば sudoers 登録済みとみなす
pub const FAKE_ENV: &str = "TAKO_2904_FAKE_PMSET";

/// A/B（[`LEGACY_ENV`]）が立っているか
pub fn legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var(LEGACY_ENV).as_deref() == Ok("1"))
}

/// 偽の置き場（[`FAKE_ENV`]）。検証プロセスでなければ常に `None` = 本物を使う
fn fake_dir() -> Option<&'static std::path::Path> {
    static FAKE: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    FAKE.get_or_init(|| {
        if !tako_core::paths::is_verification_process() {
            return None;
        }
        std::env::var_os(FAKE_ENV)
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
    })
    .as_deref()
}

/// 偽のいまの値（検証のときだけ）。`None` = 偽の置き場が無い = 本物（IORegistry）を読む
pub fn fake_sleep_disabled() -> Option<bool> {
    let dir = fake_dir()?;
    Some(
        std::fs::read_to_string(dir.join("SleepDisabled"))
            .map(|s| s.trim() == "1")
            .unwrap_or(false),
    )
}

/// sudoers.d/tako-sleep-guard が登録済みか（**macOS 固有の手段**）
pub fn is_sudoers_installed() -> bool {
    if let Some(fake) = fake_dir() {
        return fake.join("sudoers").exists();
    }
    std::path::Path::new(SUDOERS_FILE).exists()
}

/// sudo pmset -a disablesleep 0/1 を実行する（sudoers 登録済み前提、NOPASSWD）。
///
/// **戻す（`false`）を呼んでよいのは記録を確かめた経路だけ**（`super::run_pmset_action` /
/// 起動時の残留チェック `super::clear_startup_residual` / A/B の旧い腕。番犬
/// `issue2904_lid_owner_watchdog` が他の呼び手を file:line で名指す）
pub fn set_disablesleep(enable: bool) -> Result<(), String> {
    if !is_sudoers_installed() {
        return Err("sudoers 未登録（tako setup --lid-sleep で登録してください）".to_string());
    }
    let val = if enable { "1" } else { "0" };
    // GUI のスリープ防止の周期が状態の変わり目ごとに起こすので PATH で引いた絶対パスで起こす（#2519）。
    // 検証プロセスで偽の置き場があれば、偽の `sudo` を本物と同じ引数で起こす（#2904）
    let sudo: std::ffi::OsString = match fake_dir() {
        Some(fake) => fake.join("sudo").into_os_string(),
        None => tako_core::platform::exe::resolve_for_spawn("sudo").into(),
    };
    let output = std::process::Command::new(sudo)
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

/// 自分のもの以外へ手を出してよいかの門（#449 / #2904）。起動時の残留チェックと**同じ 1 実装**。
/// `Ok(())` なら通る、`Err(理由)` なら触らない。
///
/// `other_instance_running` はプロセス一覧の走査なので、隔離で決まったら引かない（閉包で受ける）
#[cfg(any(target_os = "macos", test))]
pub(super) fn residual_gate(
    is_isolated: bool,
    other_instance_running: impl FnOnce() -> bool,
) -> Result<(), &'static str> {
    if is_isolated {
        return Err("隔離モード（TAKO_ISOLATED）のためスキップ");
    }
    if other_instance_running() {
        return Err("他の tako プロセスが動作中のためスキップ");
    }
    Ok(())
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
    residual_gate(is_isolated, || other_instance_running)?;
    if !sudoers_installed {
        return Err("sudoers 未登録");
    }
    if !sleep_disabled {
        return Err("disablesleep=0（残留なし）");
    }
    Ok(())
}

/// macOS の腕が次にすること（#2904）。判定は [`decide`]、実行は親の腕（`super::run_pmset_action`）
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PmsetAction {
    /// 何もしない
    Nothing,
    /// 倒す: 記録（持ち主 = 自分）を置いてから `disablesleep 1`
    Acquire,
    /// 持ち主の死んだ上書きを引き取る: `pmset` は打たず、記録の持ち主だけ自分へ書き換える
    /// （引き取らないと、要らなくなったときに戻す主が居ない）
    TakeOver,
    /// 戻す: `disablesleep 0` を打ってから記録を捨てる
    Restore,
    /// 記録だけ捨てる（いまの値はもう 0 = 誰かが戻した。持ち主の記録を残すと、後で別の主が倒した
    /// 上書きを自分のものと取り違える）
    Forget,
}

/// 倒したいか（`enable`）× いまの値（`disabled_now`）× 記録に対する立場（`claim`。記録が無ければ
/// `None`）から次の操作を決める純粋関数（#2904）。
///
/// - 戻すのは**自分の記録**（[`RecordClaim::Mine`]）だけ。記録が無い・生きた他人の記録は戻さない
/// - 持ち主の死んだ記録（[`RecordClaim::Abandoned`] / 持ち主の無い [`RecordClaim::Unowned`]）は、
///   `gate`（[`residual_gate`]）を通ったときだけ戻す・引き取る。値は機械全体で共有なので、死んだ
///   持ち主の記録があっても、いまの 1 が別の data dir の tako のものでないとは言い切れない
/// - 倒すのは、生きた他人の記録が無いとき（倒れていない値を 1 にするのは誰のものも奪わない）
///
/// `gate` はプロセス一覧を走査しうるので、要る枝でだけ引く
#[cfg(any(target_os = "macos", test))]
pub(super) fn decide(
    enable: bool,
    disabled_now: bool,
    claim: Option<RecordClaim>,
    gate: impl FnOnce() -> Result<(), &'static str>,
) -> PmsetAction {
    use PmsetAction::*;
    use RecordClaim::{Abandoned, Foreign, Mine, Unowned};
    match (enable, disabled_now, claim) {
        // 倒したい・いま倒れていない
        (true, false, Some(Foreign { .. })) => Nothing,
        (true, false, None | Some(Mine | Abandoned | Unowned)) => Acquire,
        // 倒したい・もう倒れている
        (true, true, Some(Mine)) => Nothing,
        (true, true, Some(Abandoned | Unowned)) => match gate() {
            Ok(()) => TakeOver,
            Err(_) => Nothing,
        },
        (true, true, None | Some(Foreign { .. })) => Nothing,
        // 戻したい・倒れている
        (false, true, Some(Mine)) => Restore,
        (false, true, Some(Abandoned | Unowned)) => match gate() {
            Ok(()) => Restore,
            Err(_) => Nothing,
        },
        (false, true, None | Some(Foreign { .. })) => Nothing,
        // 戻したい・もう戻っている: 自分の（か持ち主の死んだ）記録だけ捨てる
        (false, false, Some(Mine | Abandoned | Unowned)) => Forget,
        (false, false, None | Some(Foreign { .. })) => Nothing,
    }
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

    // --- #2904: 倒した主の判定 ---

    const OPEN: fn() -> Result<(), &'static str> = || Ok(());
    const CLOSED: fn() -> Result<(), &'static str> = || Err("門が閉じている");
    /// 引かれてはいけない門（引いたら落とす = 要らない枝でプロセス一覧を走査していない）
    const UNTOUCHED: fn() -> Result<(), &'static str> = || panic!("この枝で門を引いた");

    const FOREIGN: RecordClaim = RecordClaim::Foreign { pid: 4242 };

    /// #2904 の症状 1: **記録の無い上書き・生きた他人の上書きは、終了時（= 戻したい）に戻さない**。
    /// 修正前の腕は `sudoers && いまの値` だけで `disablesleep 0` を打っていた
    #[test]
    fn 自分のものでない上書きは戻さない() {
        for claim in [None, Some(FOREIGN)] {
            assert_eq!(
                decide(false, true, claim, UNTOUCHED),
                PmsetAction::Nothing,
                "{claim:?} の上書きを戻した"
            );
        }
    }

    /// 自分が倒した上書きは**門を見ずに**戻す（他の tako が走っていても自分の残留を残さない）
    #[test]
    fn 自分の上書きは門を見ずに戻す() {
        assert_eq!(
            decide(false, true, Some(RecordClaim::Mine), UNTOUCHED),
            PmsetAction::Restore
        );
        assert_eq!(
            decide(true, true, Some(RecordClaim::Mine), UNTOUCHED),
            PmsetAction::Nothing,
            "自分が倒しているなら再要求は何もしない"
        );
    }

    /// 持ち主の死んだ記録は**起動時と同じ門**を通ったときだけ戻す・引き取る（#2904 の案 A）
    #[test]
    fn 持ち主の死んだ記録は門を通ったときだけ戻す() {
        for claim in [RecordClaim::Abandoned, RecordClaim::Unowned] {
            assert_eq!(decide(false, true, Some(claim), OPEN), PmsetAction::Restore);
            assert_eq!(
                decide(false, true, Some(claim), CLOSED),
                PmsetAction::Nothing,
                "門が閉じているのに {claim:?} を戻した"
            );
            assert_eq!(decide(true, true, Some(claim), OPEN), PmsetAction::TakeOver);
            assert_eq!(
                decide(true, true, Some(claim), CLOSED),
                PmsetAction::Nothing,
                "門が閉じているのに {claim:?} を引き取った"
            );
        }
    }

    /// 倒れていない値を倒すのは、生きた他人の記録が無いときだけ
    #[test]
    fn 倒すのは生きた他人の記録が無いとき() {
        for claim in [
            None,
            Some(RecordClaim::Mine),
            Some(RecordClaim::Abandoned),
            Some(RecordClaim::Unowned),
        ] {
            assert_eq!(
                decide(true, false, claim, UNTOUCHED),
                PmsetAction::Acquire,
                "{claim:?}"
            );
        }
        assert_eq!(
            decide(true, false, Some(FOREIGN), UNTOUCHED),
            PmsetAction::Nothing,
            "生きた他人の記録を書き換えない（相手は次の tick で自分で倒し直す）"
        );
        assert_eq!(
            decide(true, true, None, UNTOUCHED),
            PmsetAction::Nothing,
            "記録の無い上書きを自分のものとして引き取らない"
        );
    }

    /// もう戻っているなら、自分の（か持ち主の死んだ）記録だけ捨てる。生きた他人の記録は残す
    #[test]
    fn 戻っている値の記録だけ捨てる() {
        for claim in [
            RecordClaim::Mine,
            RecordClaim::Abandoned,
            RecordClaim::Unowned,
        ] {
            assert_eq!(
                decide(false, false, Some(claim), UNTOUCHED),
                PmsetAction::Forget
            );
        }
        assert_eq!(
            decide(false, false, Some(FOREIGN), UNTOUCHED),
            PmsetAction::Nothing
        );
        assert_eq!(decide(false, false, None, UNTOUCHED), PmsetAction::Nothing);
    }

    /// 門の材料は起動時の残留チェックと同じ（隔離が先 = 走査を引かない）
    #[test]
    fn 門は隔離なら走査を引かない() {
        assert!(residual_gate(true, || panic!("隔離なのに走査した")).is_err());
        assert!(residual_gate(false, || true).is_err());
        assert!(residual_gate(false, || false).is_ok());
    }
}
