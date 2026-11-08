//! remote daemon が器に求める能力の門（#2771）。
//!
//! daemon（`tako remote serve`）は tako-app と別プロセスなので、ペインの画面・履歴は器（tmux / psmux）から
//! アウトオブプロセスで読む（ペイン一覧のスニペット・WS の画面・スクロールバック）。入力・close・respond ほかの
//! 操作は tako-app の IPC（`remote::AppIpcClient`）へ渡し、GUI が居なければ 503 で断るので、daemon が器へ
//! **送る**手段は起動の要件ではない（app 不在の resize だけは器を直に叩くが、失敗は 500 で返すだけ）。
//!
//! #2771 より前の門は送出（`detached_access`）まで求めていたので、採取だけの psmux（Windows の器。#519）と
//! 器なしでは必ず断り、Windows の `tako remote start` は 1 度も通らなかった。しかも門は待ち受けを開いて
//! port ファイルを書いた後にあったので、断るたびに port ファイルが残り、次の start が「stale な待ち受け」を
//! 毎回言っていた（そのポートを別のプロセスが取っていれば起動そのものを断る）。
//!
//! 判定は [`check`] の 1 実装（**純粋関数**。能力と OS を受け取る = macOS の CI でも psmux / 器なしの形を
//! 確かめられる）。呼び手は 2 つ:
//!
//! - daemon の起動（`remote::run_daemon`）: 待ち受けを開く前に呼ぶ（断ったときに socket / port ファイルを作らない）
//! - `tako remote setup` の器の段（`remote_setup`）: `tako remote start` を案内する前に器の導入を案内する
//!
//! A/B は `TAKO_2771_LEGACY=1`（送出を求める + daemon は port ファイルを書いた後で判定する = 修正前の判定と順序。
//! 断りの文は新しいまま）。setup の器の段も出さない

use tako_core::backend::BackendCapabilities;
use tako_core::platform::support::Platform;

use crate::RemoteRefusal;

/// A/B の env（修正前の門 = 送出を求める・daemon は port ファイルを書いた後で判定する）
pub const LEGACY_ENV: &str = "TAKO_2771_LEGACY";

/// A/B（`TAKO_2771_LEGACY=1`）が立っているか
pub fn legacy() -> bool {
    std::env::var_os(LEGACY_ENV).is_some()
}

/// その OS の器の名前（依存表の器の行 = `tako setup deps install` が入れるもの）
pub fn container_name(platform: Platform) -> &'static str {
    crate::setup_deps::container_dep(platform).bin
}

/// daemon が器に求める能力を満たすか（**純粋関数**）。
///
/// 要るのは画面の読み取り（`detached_capture`）だけ。送出（`detached_access`）は採取を含意するので
/// tmux も通る。`legacy` は修正前の判定（送出を求める）
pub fn check(
    caps: &BackendCapabilities,
    platform: Platform,
    legacy: bool,
) -> Result<(), RemoteRefusal> {
    let reachable = if legacy {
        caps.detached_access
    } else {
        caps.detached_capture
    };
    if reachable {
        return Ok(());
    }
    Err(RemoteRefusal::DaemonNoScreenCapture {
        container: container_name(platform),
        label: caps.label,
    })
}

/// いまのプロセスの器と OS で判定する（daemon の起動と setup の器の段が呼ぶ口）
pub fn check_current() -> Result<(), RemoteRefusal> {
    check(
        &tako_core::backend::capabilities(),
        Platform::current(),
        legacy(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::backend::ScrollbackAuthority;
    use tako_core::i18n::Lang;

    /// 実在する器の能力の形（`backend/{tmux,psmux,null}.rs` の申告と同じ。採取と送出の 2 つだけが判定に効く）
    fn caps(label: &'static str, capture: bool, access: bool) -> BackendCapabilities {
        BackendCapabilities {
            survives_app_exit: capture,
            detached_capture: capture,
            detached_access: access,
            scrollback: if capture {
                ScrollbackAuthority::Backend
            } else {
                ScrollbackAuthority::InProcess
            },
            osc_passthrough: true,
            quotes_program: true,
            keystrokes_ascii_only: false,
            extended_keys: true,
            suppresses_copy_mode_indicator: true,
            keeps_exit_output: true,
            keeps_empty_server: true,
            label,
        }
    }

    fn tmux() -> BackendCapabilities {
        caps("tmux", true, true)
    }
    fn psmux() -> BackendCapabilities {
        caps("psmux", true, false)
    }
    fn none() -> BackendCapabilities {
        caps("none", false, false)
    }

    #[test]
    fn 本物のtmuxは門を通る() {
        assert_eq!(check(&tmux(), Platform::MacOs, false), Ok(()));
    }

    /// #2771 の本題: psmux は送出を持たないが画面は読めるので通る（修正前は必ず断っていた）
    #[test]
    fn 採取だけのpsmuxは門を通る() {
        assert_eq!(
            check(&psmux(), Platform::Windows, false),
            Ok(()),
            "psmux（detached_capture: true / detached_access: false）を断っている = 門が送出を求めている（#2771 の修正前）"
        );
    }

    #[test]
    fn 器なしは何を入れれば使えるかを言って断る() {
        for (platform, container) in [(Platform::Windows, "psmux"), (Platform::MacOs, "tmux")] {
            let refusal = check(&none(), platform, false).expect_err("器なしは画面を読めない");
            assert_eq!(
                refusal,
                RemoteRefusal::DaemonNoScreenCapture {
                    container,
                    label: "none",
                },
                "{platform:?}: 断りがその OS の器を名指していない"
            );
            for lang in [Lang::Ja, Lang::En] {
                let text = refusal.text_in(lang);
                assert!(
                    text.contains("`tako setup deps install`") && text.contains(container),
                    "{platform:?} / {lang:?}: 断りに「何を入れれば使えるか」（器の名前と最簡の導入コマンド）が無い: {text}"
                );
                assert!(
                    text.contains("`tako remote start`"),
                    "{platform:?} / {lang:?}: 入れた後の一手が無い: {text}"
                );
            }
        }
    }

    /// A/B の腕は修正前の判定（送出を求める）に戻る = psmux を断る
    #[test]
    fn 修正前の判定は送出を求めてpsmuxを断る() {
        assert_eq!(check(&tmux(), Platform::MacOs, true), Ok(()));
        assert!(check(&psmux(), Platform::Windows, true).is_err());
        assert!(check(&none(), Platform::Windows, true).is_err());
    }

    /// 器の名前は依存表の器の行から引く（`tako setup deps install` が入れるものと割れない）
    #[test]
    fn 器の名前は依存表の器の行と同じ() {
        for platform in [Platform::MacOs, Platform::Windows] {
            assert_eq!(
                container_name(platform),
                crate::setup_deps::deps(platform)[0].bin,
                "{platform:?}"
            );
        }
    }
}
