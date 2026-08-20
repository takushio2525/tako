//! 検証用の graceful quit トリガ（境界。#770）。
//!
//! GUI アプリの正規の終了経路（Cmd+Q / Dock 終了 / OS 終了）は `on_app_quit` を通り、
//! そこで layout 保存などの終了処理が走る。**この経路が tmux セッションを kill しない**
//! ことは喪失ゼロ保証の中核なので、隔離インスタンスで実測できる必要がある。
//!
//! ところが外から正規の quit を撃つ手段が無かった:
//!
//! - `SIGTERM` は既定でプロセスを即死させ `on_app_quit` を通らない（実測）
//! - System Events の `keystroke "q" using command down` は**グローバル送出**で、
//!   frontmost 切替とのレースで**別の tako（本番）に着弾して終了させた**（2026-08-06 実害）。
//!   pid を指定できないので検証には使えない
//!
//! そこで「対象 pid だけを確実に狙える」`SIGTERM` を、**隔離モードのときに限り**
//! 「quit 要求フラグを立てる」へ読み替える。フラグを見た UI 側が通常の quit を呼ぶので、
//! 実測しているのは本番と同じ終了経路そのものになる。
//!
//! 本番では何も仕掛けない = `SIGTERM` は従来どおり即死のままで挙動不変。

use std::sync::atomic::{AtomicBool, Ordering};

static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// 隔離モード（`TAKO_ISOLATED`）のときだけ SIGTERM を quit 要求へ読み替える。
/// 本番では何もしない。二重登録は無害（1 度だけ入れる）
pub fn install_for_isolated_verification() {
    if !isolated() || INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    install();
}

/// quit が要求されているか（UI 側の定期チェックから引く）。
/// 一度 true を返したら消費する（quit を二重に撃たない）
pub fn take_quit_request() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::SeqCst)
}

fn isolated() -> bool {
    matches!(
        std::env::var("TAKO_ISOLATED").ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

#[cfg(unix)]
fn install() {
    // シグナルハンドラでやってよいのは async-signal-safe な操作だけ。
    // ここではアトミックなフラグ設定しかしない
    extern "C" fn on_sigterm(_sig: libc::c_int) {
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn install() {
    // Windows に SIGTERM は無い。検証トリガも今は要らない（#467 のポート時に再検討）
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 本番では仕掛けを入れない() {
        // 環境変数に依存するのでフラグ操作の副作用だけを確認する
        // （テストは並列実行されるため env の書き換えはしない）
        assert!(!take_quit_request(), "初期状態では quit 要求は立っていない");
    }

    #[test]
    fn quit要求は一度だけ消費される() {
        QUIT_REQUESTED.store(true, Ordering::SeqCst);
        assert!(take_quit_request());
        assert!(
            !take_quit_request(),
            "2 度目は false = quit を二重に撃たない"
        );
    }
}
