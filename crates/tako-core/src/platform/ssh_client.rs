//! SSH クライアントの能力（抽象境界 B26。#1090）
//!
//! ## なぜ要るか
//!
//! tako の SSH 系は **システムの `ssh` / `sftp` を子プロセスで呼ぶ**（理由は
//! [`crate::remote_fs`] のモジュール doc）。呼ぶ相手が OS 同梱の OpenSSH なので、
//! **同じ名前のコマンドでも扱えるオプションが違う**。#65 の設計は
//! 「ツリー（sftp）と対話ペインが同じ ControlMaster のソケットを共有し、
//! パスワード認証しか無い相手でも一度ログインすれば以後追加認証が要らない」で、
//! これは **接続多重化（ControlMaster / ControlPath / ControlPersist）** に乗っている。
//!
//! Windows の OpenSSH（`C:\Program Files\OpenSSH\ssh.exe`）は**この多重化を実装していない**。
//! 渡すと接続そのものが壊れる。同じホスト・同じ機でオプションだけを変えた実測
//! （OpenSSH_for_Windows_10.0p2 / Windows 11。#1090）:
//!
//! | 渡したオプション | exit | 出力 |
//! |---|---|---|
//! | 多重化**なし** | `255` | `ssh: Could not resolve hostname …`（正しい失敗） |
//! | 多重化**あり** | `-1` | `getsockname failed: Not a socket` / `Read from remote host …` |
//!
//! `-1` は「ssh 自身の失敗 = 255」という OpenSSH の約束から外れるので、
//! 失敗を見張っている層（ペインのスクリプト・[`crate::ssh_progress`]）がどちらも
//! 素通しし、**接続が無言で死ぬ**のが #1090 の症状だった。
//!
//! ## 何を捨てることになるか
//!
//! 多重化が無いプラットフォームでは
//!
//! - 操作（`sftp` のバッチ 1 回・対話ペインの `ssh` 1 本）ごとに**独立した接続**になる。
//!   鍵・agent で入れる相手なら見た目は変わらないが、**パスワードしか無い相手は
//!   操作のたびに認証が要る**（#65 の「一度ログインすれば以後不要」が成立しない）
//! - 「いま繋がっているか」を**ソケットの有無で安く判定できない**
//!   （[`crate::remote_fs::liveness`] が `Unknown` を返す理由）
//!
//! この縮退は [`NO_MULTIPLEXING`] 1 箇所で文言を定義し、対応マトリクス
//! （[`super::support`]）と診断がそこから引く。
//!
//! ## 判定は純粋関数
//!
//! [`multiplexing`] は [`Platform`] を引数で受けるので、**macOS 上から Windows 側の
//! 挙動を検証できる**（`support` / `dpi` / `window_lifecycle` と同じ作法）。
//! 実行時の値は [`multiplexing_available`] が返す（A/B の env をここで吸収する）。

use super::support::{Note, Platform};

/// #1090 以前の挙動へ戻す A/B の env。
///
/// 立てると「多重化は常に使える」「ペインの失敗判定は exit 255 だけ」に戻るので、
/// **同一バイナリで無言死を再現できる**（検出力の実証用）
pub const LEGACY_ENV: &str = "TAKO_1090_LEGACY";

/// そのプラットフォームの OpenSSH が接続多重化（ControlMaster）を扱えるか。
///
/// **純粋関数**。Windows は実装が無く、渡すと接続が壊れる（モジュール doc の実測表）
pub const fn multiplexing(platform: Platform) -> bool {
    match platform {
        Platform::MacOs => true,
        Platform::Windows => false,
    }
}

/// 実行中のプラットフォームで多重化を使うか。
///
/// [`LEGACY_ENV`] が立っているときは**プラットフォームに関わらず使う**
/// （= #1090 以前の挙動。Windows では接続が壊れる形へ戻る）
pub fn multiplexing_available() -> bool {
    if legacy() {
        return true;
    }
    multiplexing(Platform::current())
}

/// A/B の env が立っているか
pub fn legacy() -> bool {
    matches!(
        std::env::var(LEGACY_ENV).ok().as_deref(),
        Some("1") | Some("true")
    )
}

/// 多重化が無いプラットフォームの縮退。**文言はここ 1 箇所**（設計 §3・§4）
pub const NO_MULTIPLEXING: Note = Note::new(
    "Windows の OpenSSH は接続多重化（ControlMaster）に対応しないため、操作ごとに独立した SSH 接続になる。鍵・ssh-agent で入れる相手は変わらないが、パスワード認証しか無い相手はツリーの展開やファイルの取得のたびに認証が要る。接続が生きているかもソケットで判定できないので、切断後の自動再接続（#1040）も armed にならない（#1090）",
    "The Windows OpenSSH client has no connection multiplexing (ControlMaster), so every operation opens its own SSH connection. Hosts reachable by key or ssh-agent behave the same, but a password-only host asks for credentials on every tree expansion or file fetch. Liveness cannot be determined from a control socket, so automatic reconnection after a drop (#1040) does not arm either (#1090)",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 多重化はmacosだけが扱える() {
        assert!(multiplexing(Platform::MacOs));
        assert!(!multiplexing(Platform::Windows));
    }

    #[test]
    fn 縮退の理由は日英とも中身がある() {
        assert!(NO_MULTIPLEXING.ja().contains("ControlMaster"));
        assert!(NO_MULTIPLEXING.en().contains("ControlMaster"));
        // 追跡できるように Issue 番号を必ず残す
        assert!(NO_MULTIPLEXING.ja().contains("#1090"));
        assert!(NO_MULTIPLEXING.en().contains("#1090"));
    }
}
