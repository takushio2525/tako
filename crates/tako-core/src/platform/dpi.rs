//! プロセスの DPI 認識レベル（抽象境界 B24。#1063）
//!
//! ## なぜ要るか
//!
//! Windows で「アプリが物理ピクセルをそのまま扱うか、OS に座標を仮想化してもらうか」は
//! **プロセス単位の属性**で、実行ファイルへ埋め込まれたマニフェストが決める。tako は
//! gpui のマニフェスト（`crates/gpui/resources/windows/gpui.manifest.xml` を
//! `windows-manifest` フィーチャが埋め込む）で **PerMonitorV2** を宣言していて、
//! gpui の Windows バックエンドはその前提で組まれている:
//!
//! - `WM_SIZE` / `GetWindowRect` が**物理ピクセル**で届く
//! - 倍率は `GetDpiForWindow(hwnd) / 96`（`gpui_windows` の `scale_factor`）
//! - レイアウトはその倍率で割った論理ピクセルで組み、描画時に掛け戻す
//!
//! この宣言が落ちるとプロセスは DPI 非認識になり、OS が座標を仮想化して描画結果を
//! 拡大する（`GetDpiForWindow` は 96 を返す）。**画面はぼやけるだけで一見動く**ので
//! 気づきにくく、しかも macOS では原理的に再現しない。宣言は tako 自身のコードではなく
//! **依存クレートの既定フィーチャ**に乗っているので、rev 追従や `default-features = false`
//! で無言で消え得る。だから「消えたら分かる」ようにここで実測して申告する。
//!
//! ## 測るときの罠（#1063 の教訓）
//!
//! この属性は**問い合わせる側のプロセス**にも効く。DPI 非認識のプロセスから
//! `GetWindowRect` を呼ぶと 1/倍率 に縮んだ値が返るのに、`BitBlt` によるスクリーン
//! キャプチャは**物理ピクセルのまま**返る。両者を混ぜると
//! 「ウインドウ 1550px に対して中身が 1886px = 1.22 倍あふれている」ように見える
//! （実測: 真の値は client 1920x1020・中身 1886px で 34px 余っていた）。
//! 計測する側は必ず PerMonitorV2 を宣言すること。道具は
//! `scripts/windows/measure-window.ps1` に置いてある。
//!
//! 判定は純粋関数なので **macOS 上から Windows 側の期待値と文言を検証できる**
//! （`support` / `window_lifecycle` と同じ作法）。

use super::support::{Note, Platform};

/// プロセスの DPI 認識レベル。
///
/// Windows の `DPI_AWARENESS` に対応するが、V1 と V2 の区別（非クライアント領域と
/// 子ウィンドウの自動スケール）は gpui の前提に関わるので分けて持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpiAwareness {
    /// この OS には「プロセスの DPI 認識」という概念が無い（macOS は OS が
    /// 論理座標で統一し、倍率はウィンドウの backing scale factor として届く）
    NotApplicable,
    /// DPI 非認識。OS が座標を仮想化し、描画結果を拡大する（ぼやける）
    Unaware,
    /// システム DPI 認識。プライマリと違う倍率のモニタへ移すと仮想化される
    System,
    /// モニタ単位の DPI 認識（V1）。非クライアント領域が追従しない
    PerMonitor,
    /// モニタ単位の DPI 認識（V2）。tako / gpui が要求する水準
    PerMonitorV2,
    /// 問い合わせに失敗した（API が無い等）
    Unknown,
}

impl DpiAwareness {
    /// 診断ログ・応答 JSON に載せる安定した識別子
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Unaware => "unaware",
            Self::System => "system",
            Self::PerMonitor => "per_monitor",
            Self::PerMonitorV2 => "per_monitor_v2",
            Self::Unknown => "unknown",
        }
    }
}

/// そのプラットフォームで期待する水準（純粋関数）
pub fn expected_for(platform: Platform) -> DpiAwareness {
    match platform {
        Platform::MacOs => DpiAwareness::NotApplicable,
        Platform::Windows => DpiAwareness::PerMonitorV2,
    }
}

/// 実測値が期待どおりか（純粋関数）
pub fn is_expected(actual: DpiAwareness, platform: Platform) -> bool {
    actual == expected_for(platform)
}

/// 期待から外れているときの説明（日英）。期待どおりなら `None`。
///
/// **`Unknown` を「壊れている」と言い切らない**: 問い合わせに失敗しただけで、
/// 実際の描画が正しいこともある（#982 で「上流に手段が無い」と「まだ調べていない」を
/// 混ぜないと決めたのと同じ理由）。
pub fn degraded_note(actual: DpiAwareness, platform: Platform) -> Option<Note> {
    if is_expected(actual, platform) {
        return None;
    }
    Some(match actual {
        DpiAwareness::Unaware => Note::new(
            "このプロセスは DPI 非認識で動いている。OS が座標を仮想化して描画結果を拡大するため、\
             文字がぼやけ、ウインドウのピクセル数と tako が組むレイアウトの寸法が食い違う。\
             実行ファイルへ PerMonitorV2 のマニフェストが埋め込まれているか確認すること",
            "This process is running DPI-unaware. Windows virtualises coordinates and stretches \
             the rendered output, so text looks blurry and the window's pixel size disagrees with \
             the layout tako builds. Check that the executable embeds the PerMonitorV2 manifest.",
        ),
        DpiAwareness::System => Note::new(
            "このプロセスはシステム DPI 認識で動いている。プライマリと倍率が違うモニタへ \
             移すと座標が仮想化される。PerMonitorV2 のマニフェストを確認すること",
            "This process is running system-DPI-aware. Coordinates get virtualised on monitors \
             whose scale differs from the primary one. Check the PerMonitorV2 manifest.",
        ),
        DpiAwareness::PerMonitor => Note::new(
            "このプロセスはモニタ単位 DPI 認識（V1）で動いている。非クライアント領域が \
             倍率に追従しない。PerMonitorV2 のマニフェストを確認すること",
            "This process is running per-monitor DPI aware (V1); the non-client area does not \
             follow the scale factor. Check the PerMonitorV2 manifest.",
        ),
        DpiAwareness::Unknown => Note::new(
            "プロセスの DPI 認識レベルを問い合わせられなかった。描画倍率がおかしいときは \
             まずここを疑うこと",
            "Could not query the process DPI awareness. If the rendering scale looks wrong, \
             start here.",
        ),
        // 期待どおりの値は上で弾いているので、残るのは
        // 「そのプラットフォームで期待していない NotApplicable / PerMonitorV2」だけ
        DpiAwareness::NotApplicable => Note::new(
            "この OS では DPI 認識レベルを持たないはずなのに、期待値と食い違っている",
            "This OS is not expected to carry a process DPI awareness level, yet the value \
             disagrees with the expectation.",
        ),
        DpiAwareness::PerMonitorV2 => Note::new(
            "この OS では DPI 認識レベルを持たないはずなのに、値が返ってきた",
            "This OS is not expected to carry a process DPI awareness level, yet a value was \
             returned.",
        ),
    })
}

/// この環境の期待値
pub fn expected_here() -> DpiAwareness {
    if cfg!(windows) {
        expected_for(Platform::Windows)
    } else {
        expected_for(Platform::MacOs)
    }
}

/// 実測。**この呼び出しは何も変えない**（awareness を設定しには行かない）
pub fn process_awareness() -> DpiAwareness {
    imp::process_awareness()
}

/// 実測が期待どおりか
pub fn healthy_here() -> bool {
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::MacOs
    };
    is_expected(process_awareness(), platform)
}

/// 期待から外れているときの説明（日英）。期待どおりなら `None`
pub fn degraded_note_here() -> Option<Note> {
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::MacOs
    };
    degraded_note(process_awareness(), platform)
}

#[cfg(not(windows))]
mod imp {
    use super::DpiAwareness;

    pub fn process_awareness() -> DpiAwareness {
        DpiAwareness::NotApplicable
    }
}

#[cfg(windows)]
mod imp {
    use super::DpiAwareness;
    use std::ffi::c_void;

    /// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`（`windef.h` の `(HANDLE)-4`）
    const PER_MONITOR_AWARE_V2: *mut c_void = -4isize as *mut c_void;

    // `DPI_AWARENESS`（`windef.h`）
    const DPI_AWARENESS_UNAWARE: i32 = 0;
    const DPI_AWARENESS_SYSTEM_AWARE: i32 = 1;
    const DPI_AWARENESS_PER_MONITOR_AWARE: i32 = 2;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetThreadDpiAwarenessContext() -> *mut c_void;
        fn GetAwarenessFromDpiAwarenessContext(value: *mut c_void) -> i32;
        fn AreDpiAwarenessContextsEqual(a: *mut c_void, b: *mut c_void) -> i32;
    }

    /// tako は awareness を実行時に設定しない（マニフェストが唯一の決定要因）ので、
    /// スレッドの文脈 = プロセス既定になる。`GetProcessDpiAwareness` ではなく
    /// こちらを見るのは、V1 / V2 の区別が取れるのがこの経路だけだから
    pub fn process_awareness() -> DpiAwareness {
        let ctx = unsafe { GetThreadDpiAwarenessContext() };
        if ctx.is_null() {
            return DpiAwareness::Unknown;
        }
        match unsafe { GetAwarenessFromDpiAwarenessContext(ctx) } {
            DPI_AWARENESS_UNAWARE => DpiAwareness::Unaware,
            DPI_AWARENESS_SYSTEM_AWARE => DpiAwareness::System,
            DPI_AWARENESS_PER_MONITOR_AWARE => {
                if unsafe { AreDpiAwarenessContextsEqual(ctx, PER_MONITOR_AWARE_V2) } != 0 {
                    DpiAwareness::PerMonitorV2
                } else {
                    DpiAwareness::PerMonitor
                }
            }
            // DPI_AWARENESS_INVALID (-1) を含む
            _ => DpiAwareness::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn 期待値はwindowsだけper_monitor_v2() {
        assert_eq!(expected_for(Platform::Windows), DpiAwareness::PerMonitorV2);
        assert_eq!(expected_for(Platform::MacOs), DpiAwareness::NotApplicable);
    }

    #[test]
    fn 期待どおりなら説明は出ない() {
        assert!(degraded_note(DpiAwareness::PerMonitorV2, Platform::Windows).is_none());
        assert!(degraded_note(DpiAwareness::NotApplicable, Platform::MacOs).is_none());
    }

    #[test]
    fn 縮退はすべて日英の説明を持つ() {
        // macOS 上から Windows 側の全パターンを検証できるのがこの形の狙い
        for actual in [
            DpiAwareness::Unaware,
            DpiAwareness::System,
            DpiAwareness::PerMonitor,
            DpiAwareness::Unknown,
            DpiAwareness::NotApplicable,
        ] {
            let note = degraded_note(actual, Platform::Windows)
                .unwrap_or_else(|| panic!("{actual:?} に説明が無い"));
            assert!(
                !note.text_in(Lang::Ja).is_empty(),
                "{actual:?} の日本語が空"
            );
            assert!(!note.text_in(Lang::En).is_empty(), "{actual:?} の英語が空");
            assert_ne!(
                note.text_in(Lang::Ja),
                note.text_in(Lang::En),
                "{actual:?} の日英が同一"
            );
        }
    }

    #[test]
    fn 非認識の説明はマニフェストへ誘導する() {
        // 「黙って縮退しない」= 次の一手（どこを見るか）を必ず含める
        let note = degraded_note(DpiAwareness::Unaware, Platform::Windows).unwrap();
        assert!(note.text_in(Lang::Ja).contains("マニフェスト"));
        assert!(note.text_in(Lang::En).contains("manifest"));
    }

    #[test]
    fn 識別子は全パターンで一意() {
        let all = [
            DpiAwareness::NotApplicable,
            DpiAwareness::Unaware,
            DpiAwareness::System,
            DpiAwareness::PerMonitor,
            DpiAwareness::PerMonitorV2,
            DpiAwareness::Unknown,
        ];
        let mut seen: Vec<&str> = all.iter().map(|a| a.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "識別子が重複している");
    }

    #[test]
    fn この環境の実測は期待どおり() {
        // macOS: NotApplicable。Windows 実機: マニフェストが効いていれば PerMonitorV2。
        // **落ちたらマニフェストが埋め込まれていない**（#1063 が本物になる条件）
        let actual = process_awareness();
        assert_eq!(
            actual,
            expected_here(),
            "DPI 認識レベルが期待と違う: {actual:?}（{:?}）",
            degraded_note_here().map(|n| n.text_in(Lang::Ja))
        );
        assert!(healthy_here());
        assert!(degraded_note_here().is_none());
    }
}
