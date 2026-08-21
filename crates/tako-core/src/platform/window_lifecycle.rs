//! ウィンドウが 0 枚になったときにアプリを終わらせるかどうか（境界。#872）。
//!
//! この判断は **UI ツールキットに任せてはいけない**。GPUI の既定（`QuitMode::Default`）は
//! 「macOS 以外では最後のウィンドウが閉じたらアプリを終了する」で、しかも終了は
//! `PostQuitMessage(0)` → `ExitProcess(0)` なので **診断を 1 行も残さずに終了コード 0 で
//! プロセスが消える**。tako は内部都合で一時的にウィンドウを 0 枚にする経路を持つ
//! （`sync_viewports` の閉じ直し・セルフテストの `remove_window`）ため、
//! そこを踏むと「panic でも FAILED でもない無音終了」になる（#872 の症状）。
//!
//! そこで方針をここに 1 本だけ置き、実行（`cx.quit()`）は UI 側の close ハンドラが行う。
//! プラットフォームで違うのは**理由**だけ:
//!
//! - macOS: ウィンドウ 0 枚でもアプリは生き続けるのが標準（Dock から再表示できる）。
//!   tako も `on_reopen` で同一 entity のウィンドウを開き直す（#312 / #381）
//! - Windows: ウィンドウ 0 枚から戻す標準の手段が無い（`on_reopen` 相当が発火しない）。
//!   最後のウィンドウを閉じたらアプリを終了するのが標準
//!
//! 判定は純粋関数なので **macOS 上から Windows 側の方針を検証できる**（`support` と同じ作法）。

use super::support::Platform;

/// 最後のウィンドウ（ビューポート）が閉じられたときのアプリの寿命
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastWindowClose {
    /// プロセスを生かして再表示を待つ（macOS: Dock 復帰）
    KeepAliveForReopen,
    /// アプリを終了する（窓 0 枚から戻る手段が無いプラットフォーム）
    Quit,
}

impl LastWindowClose {
    /// アプリを終了するか
    pub fn quits(self) -> bool {
        matches!(self, Self::Quit)
    }

    /// 診断ログ（persist.log）へ書く理由。ユーザー向け UI 文言ではないので日本語のみ
    pub fn reason(self) -> &'static str {
        match self {
            Self::KeepAliveForReopen => "ウィンドウ 0 枚でもプロセスは生存（Dock 復帰で開き直す）",
            Self::Quit => "ウィンドウ 0 枚から戻る手段が無いためアプリを終了する",
        }
    }
}

/// プラットフォームごとの方針（純粋関数）
pub fn last_window_close(platform: Platform) -> LastWindowClose {
    match platform {
        Platform::MacOs => LastWindowClose::KeepAliveForReopen,
        Platform::Windows => LastWindowClose::Quit,
    }
}

/// この環境の方針。`Platform::current()` は未知の OS を macOS 側へ倒すが、
/// 「Dock 復帰がある」のは macOS だけなのでここは `target_os` を直接見る
/// （GPUI の `QuitMode::Default` の分岐条件と同じ形にしておく）
pub fn last_window_close_here() -> LastWindowClose {
    if cfg!(target_os = "macos") {
        last_window_close(Platform::MacOs)
    } else {
        last_window_close(Platform::Windows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macosはウィンドウ0枚でも生存しwindowsは終了する() {
        assert_eq!(
            last_window_close(Platform::MacOs),
            LastWindowClose::KeepAliveForReopen
        );
        assert!(!last_window_close(Platform::MacOs).quits());
        assert_eq!(last_window_close(Platform::Windows), LastWindowClose::Quit);
        assert!(last_window_close(Platform::Windows).quits());
    }

    #[test]
    fn この環境の方針はtarget_osと一致する() {
        let here = last_window_close_here();
        if cfg!(target_os = "macos") {
            assert_eq!(here, LastWindowClose::KeepAliveForReopen);
        } else {
            assert_eq!(here, LastWindowClose::Quit);
        }
    }

    #[test]
    fn 理由はどちらの方針でも空でない() {
        for p in [Platform::MacOs, Platform::Windows] {
            assert!(!last_window_close(p).reason().is_empty());
        }
    }
}
