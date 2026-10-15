//! 実 tmux を使うテストの入口（Issue #2106）
//!
//! 実 tmux を使うテストは、tmux が無い環境（tmux を入れていない開発機・Windows）では
//! skip して `ok` と出る。cargo test は通ったテストの stderr を捨てるので、
//! **skip したかどうかはログから見えない**。macOS の CI ランナーの image には tmux が無く、
//! 実 tmux の番犬（#2095 の pane-died・#1857 の attach 専用・#2100 の conf 差し替え ほか）は
//! CI で 1 度も走っていなかった（#2106 の実測: 該当テストのバイナリの所要が 0.00〜0.03 秒）。
//!
//! そこで「前提が揃わないので飛ばす」判断をこの 1 実装へ寄せる:
//! - [`REQUIRE_ENV`]（`TAKO_TEST_REQUIRE_TMUX=1`）が立っていたら、skip の代わりに panic する。
//!   立てるのは CI の macOS ジョブ（tmux を入れる段の後ろ）だけ。手元で tmux の無い開発者の
//!   `cargo test` は従来どおり skip で通る
//! - [`LEDGER_ENV`]（`TAKO_TEST_TMUX_LEDGER=<file>`）が立っていたら、門を通った（= 実 tmux で
//!   走った）テストの名前を 1 行ずつ追記する。CI は `scripts/ci-real-tmux-report.sh` で件数と
//!   名前をログへ出す（0 件なら落ちる）
//!
//! テストからは「前提を問う → 揃わなければ return」の形で使う:
//!
//! ```ignore
//! if !tako_core::tmux_test_gate::container() {
//!     return;
//! }
//! ```
//!
//! 番犬 `crates/tako-control/tests/issue2106_real_tmux_gate_watchdog.rs` が、門を通らない
//! skip（`eprintln!("skip: tmux …")` の直書き）の再登場・env の効き目・CI の配線を見張る。

use std::io::Write;
use std::panic::Location;

/// 立っていたら実 tmux のテストの skip を panic に変える（CI の macOS ジョブだけが立てる）
pub const REQUIRE_ENV: &str = "TAKO_TEST_REQUIRE_TMUX";

/// 立っていたら、門を通ったテストの名前をこのファイルへ追記する
pub const LEDGER_ENV: &str = "TAKO_TEST_TMUX_LEDGER";

/// 器（tako の tmux バックエンド）が使えるか。器を通して tmux を叩くテストの既定の入口
#[track_caller]
pub fn container() -> bool {
    require(
        crate::backend::capabilities().survives_app_exit,
        "tmux が無い環境",
    )
}

/// 本物の tmux（tmux だけを名乗る CLI。psmux の名前貸しではない）が使えるか。
/// 判定はターゲット構文と同じ [`crate::tmux::announces_only_tmux`] の 1 本
#[track_caller]
pub fn real_tmux() -> bool {
    require(
        crate::tmux::version_announcement().is_some_and(crate::tmux::announces_only_tmux),
        "本物の tmux が無い環境",
    )
}

/// 前提（`available`）が揃っていれば台帳へ記録して `true`。揃っていなければ
/// [`skip`]（`missing` = 欠けている前提）して `false`
#[track_caller]
pub fn require(available: bool, missing: &str) -> bool {
    if available {
        record(Location::caller());
    } else {
        skip(missing);
    }
    available
}

/// [`require`] の `Option` 版。`Some` なら台帳へ記録してそのまま返す
#[track_caller]
pub fn require_some<T>(value: Option<T>, missing: &str) -> Option<T> {
    require(value.is_some(), missing);
    value
}

/// 実 tmux のテストが前提を満たせず飛ばす（tmux 不在・zsh 不在・先行サーバーを
/// 立てられない 等）。[`REQUIRE_ENV`] が立っていれば panic する
#[track_caller]
pub fn skip(reason: &str) {
    let at = Location::caller();
    if skip_forbidden(std::env::var(REQUIRE_ENV).ok().as_deref()) {
        panic!(
            "実 tmux のテスト {} を飛ばそうとした: {reason}（{}:{}）。\
             {REQUIRE_ENV} が立っている（CI）ので skip を許さない。\
             tmux の無い手元で走らせるなら {REQUIRE_ENV} を外す（#2106）",
            test_name(),
            at.file(),
            at.line()
        );
    }
    eprintln!(
        "skip: {reason}（実 tmux のテスト。{}:{}。#2106）",
        at.file(),
        at.line()
    );
}

/// skip を禁じるか（純関数）。`value` = [`REQUIRE_ENV`] の値。
/// 未設定・空・`0` だけが「許す」で、それ以外（`1` / `true` 等）は禁じる側へ倒す
/// （CI の綴り違いで黙って skip へ戻らないため）
pub fn skip_forbidden(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|v| !v.is_empty() && v != "0")
}

/// 台帳の 1 行（純関数）: `<テスト名>\t<file>:<line>\n`。
/// テスト名はタブ・改行を空白へ寄せる（1 行 1 テストを崩さない）
pub fn ledger_line(test: &str, file: &str, line: u32) -> String {
    let test: String = test
        .chars()
        .map(|c| if c == '\t' || c == '\n' { ' ' } else { c })
        .collect();
    format!("{test}\t{file}:{line}\n")
}

/// いま走っているテストの名前。libtest はテストごとにテスト名のスレッドで走らせる
fn test_name() -> String {
    std::thread::current()
        .name()
        .filter(|n| *n != "main")
        .unwrap_or("?")
        .to_string()
}

fn record(at: &Location<'_>) {
    let Some(path) = std::env::var_os(LEDGER_ENV).filter(|p| !p.is_empty()) else {
        return;
    };
    let line = ledger_line(&test_name(), at.file(), at.line());
    // 1 行を 1 回の write で追記する（並列のテストスレッド・テストバイナリが同じ台帳へ書く）
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        eprintln!("{LEDGER_ENV} へ書けない（{}）: {e}", path.to_string_lossy());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 未設定と空と0だけがskipを許す() {
        assert!(!skip_forbidden(None));
        assert!(!skip_forbidden(Some("")));
        assert!(!skip_forbidden(Some("  ")));
        assert!(!skip_forbidden(Some("0")));
        assert!(skip_forbidden(Some("1")));
        assert!(skip_forbidden(Some(" 1 ")));
        assert!(skip_forbidden(Some("true")));
    }

    #[test]
    fn 台帳の行はテスト名と場所をタブで区切った1行() {
        assert_eq!(
            ledger_line("tmux_backend::tests::a", "crates/x.rs", 12),
            "tmux_backend::tests::a\tcrates/x.rs:12\n"
        );
        assert_eq!(
            ledger_line("a\tb\nc", "f.rs", 1),
            "a b c\tf.rs:1\n",
            "テスト名のタブ・改行で行が割れない"
        );
    }
}
