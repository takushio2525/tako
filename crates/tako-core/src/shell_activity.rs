//! シェル統合が知らせた「PATH が変わりうる出来事」の通し番号（#1769）
//!
//! シェル統合（OSC 7 / 133。FR-2.4.1）は cwd とコマンドの実行状態だけを知らせ、**PATH そのものは
//! 知らせない**。そこで「PATH（とそこに在る実行ファイル）が変わりうる出来事」= **cwd の変化**と
//! **コマンドの終わり**（`brew install` / `rustup component add` / profile の編集・読み直しを含む）を
//! 数える。LSP のサーバ解決のキャッシュ（`tako_control::lsp::manager`）は、引いたときの番号と
//! 今の番号が違えば引き直す。
//!
//! 番号はプロセス全体で 1 つ（GUI の全ペインのシェルが同じ番号を進める。manager も同じ GUI
//! プロセスにある）。進めるのは `TerminalSession::process_osc_event` の 1 か所で、PTY の流れも
//! 側路（#766 の `feed_osc_bytes`）もここを通る。アイドル中は何も起きない（タイマーを持たない = #772）

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::osc_tap::{OscEvent, PromptMark};

static EPOCH: AtomicU64 = AtomicU64::new(0);

/// 今の通し番号
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::SeqCst)
}

/// 1 つ進める（PATH が変わりうる出来事が起きた）
pub fn note() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// その出来事は「PATH が変わりうる」か。`cwd` はそのペインの今の cwd（出来事の前）。
/// 同じ cwd の知らせ直し（プロンプトのたびに OSC 7 を送る設定がある）は数えない
pub fn is_activity(cwd: Option<&Path>, event: &OscEvent) -> bool {
    match event {
        OscEvent::CwdChanged(path) => cwd != Some(path.as_path()),
        OscEvent::Mark(PromptMark::CommandFinished(_)) => true,
        OscEvent::Mark(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cwd_の変化とコマンドの終わりだけを数える() {
        let here = PathBuf::from("/w/a");
        let moved = OscEvent::CwdChanged(PathBuf::from("/w/b"));
        let same = OscEvent::CwdChanged(here.clone());
        assert!(is_activity(Some(&here), &moved));
        assert!(is_activity(None, &same), "最初の cwd の知らせは変化");
        assert!(
            !is_activity(Some(&here), &same),
            "同じ cwd の知らせ直しは数えない"
        );
        assert!(is_activity(
            Some(&here),
            &OscEvent::Mark(PromptMark::CommandFinished(Some(0)))
        ));
        assert!(is_activity(
            Some(&here),
            &OscEvent::Mark(PromptMark::CommandFinished(None))
        ));
        for mark in [
            PromptMark::PromptStart,
            PromptMark::CommandStart,
            PromptMark::CommandExecuted,
        ] {
            assert!(!is_activity(Some(&here), &OscEvent::Mark(mark)), "{mark:?}");
        }
    }

    #[test]
    fn 進めると番号が増える() {
        let before = epoch();
        note();
        assert!(epoch() > before);
    }
}
