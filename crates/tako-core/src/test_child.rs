//! テストを「その 1 本だけが走る子プロセス」で走らせ直す（Issue #2400。テスト専用）
//!
//! ## なぜ要るか
//!
//! env はプロセスに 1 つで、`cargo test` は 1 つのテストバイナリの中でテストを並列に走らせる。
//! テストが `std::env::set_var` / `remove_var` で env を書き換えると、その窓に並走する
//! **別のテスト**が同じ env を読んで結果が変わる（Rust 2024 で `set_var` が unsafe になった理由と同じ）。
//! lib のテスト（`crates/*/src`）は crate ごとに数千本が 1 プロセスに乗るので、モジュールごとの
//! `Mutex` では**別のモジュールの読み手**を止められない。
//!
//! 実物（#2400 の A/B）: `remote` のテストが自分の `TEST_ENV_LOCK` の下で `TAKO_841_LEGACY=1` を
//! 立てた窓に、`platform::local_endpoint` のテストが別のロック（`ENV_LOCK`）の下で `verify_peer` を引き、
//! 「tailscaled でないローカルプロセスの接続は拒否される」が `NotRequired` に化けて落ちた。
//!
//! ## 作り
//!
//! - [`run_alone!`]（[`rerun_alone`]）: 親は自分のテストバイナリを `--exact <名前> --test-threads=1` で
//!   起こし直し、子が**その 1 本を通した**ことを確かめて `true` を返す（呼び出し側は本文を走らせずに
//!   戻る）。子では `false` を返して本文がそのまま走る。子のプロセスには他のテストが居ないので、
//!   本文の `set_var` は誰にも漏れない
//! - [`assert_alone`]: env を書き換えるヘルパ（`HomeGuard::set` など）の頭で呼ぶ。子の外で呼ばれたら
//!   panic する（呼び出し側のテストが `run_alone!` を通し忘れたことを、漏れる前に名指す）
//!
//! 番犬 `crates/tako-control/tests/issue2400_test_env_write_watchdog.rs` は、テストの中の
//! `set_var(` / `remove_var(` を、囲む関数がこのどちらか（または #2391 の `rerun_without`）を
//! 先に通していなければ `file:line` で落とす。規約は `.agent/conventions.md` の Issue #2400 の節。
//!
//! 製品の経路からは呼ばない（`test_residue` と同じく、テストが crate をまたいで使うので公開している）。

use std::process::Stdio;

/// 子だけが持つ目印（値は子が走らせるテストの名前）
const CHILD_ENV: &str = "TAKO_2400_ALONE_CHILD";

/// このテスト関数を「その 1 本だけが走る子プロセス」で走らせ直す。
///
/// 親では子が通ったことを確かめて `true`（= 呼び出し側は本文を走らせずに戻る）、子では `false`。
/// 名前は `module_path!()` から組むので、lib のテスト（`dispatch::tests::名前`）でも
/// 結合テスト（`名前`）でも関数名だけを渡せばよい。
///
/// ```ignore
/// #[test]
/// fn env_を立てて旧挙動を見る() {
///     if tako_core::run_alone!(env_を立てて旧挙動を見る) {
///         return;
///     }
///     std::env::set_var("TAKO_XXX_LEGACY", "1"); // 子の中だけ（他のテストは居ない）
/// }
/// ```
#[macro_export]
macro_rules! run_alone {
    ($test:ident) => {
        $crate::test_child::rerun_alone(
            $crate::test_child::libtest_name(concat!(module_path!(), "::", stringify!($test))),
            &[],
        )
    };
}

/// `module_path!()` + `::` + 関数名から、libtest が使うテストの名前を出す（先頭の crate 名を落とす）
pub fn libtest_name(path: &str) -> &str {
    path.split_once("::").map_or(path, |(_, rest)| rest)
}

/// いまこのプロセスが [`rerun_alone`] の子か（= 他のテストが居ない）
pub fn in_alone_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

/// env を書き換えるヘルパの頭で呼ぶ。[`rerun_alone`] の子の外なら panic する。
///
/// `what` は panic の文に出す名前（`"HomeGuard::set"` など）
pub fn assert_alone(what: &str) {
    assert!(
        in_alone_child(),
        "{what} はプロセスの env を書き換えるので、`tako_core::run_alone!(<テスト名>)` を通した子の中で \
         呼ぶこと（親のプロセスでは並走する他のテストへ漏れる。#2400）"
    );
}

/// `test`（libtest の名前）を `remove` の env を外した子で走らせ直す。
///
/// 親では子がその 1 本を通したことを確かめて `true`、子では `false`。
/// 子の出力はそのまま親の出力へ写す（libtest が親のテストの出力として捕まえ、落ちたときに出す）。
/// `--include-ignored`: 親が `--ignored` で走らせた手動専用のテストも、子で同じ 1 本が走る
pub fn rerun_alone(test: &str, remove: &[&str]) -> bool {
    if std::env::var_os(CHILD_ENV).is_some_and(|v| v == test) {
        return false;
    }
    let exe = std::env::current_exe().expect("テストバイナリのパス");
    let mut cmd = std::process::Command::new(exe);
    crate::platform::process::no_console_window(&mut cmd);
    // `--test-threads=1`: 子の出力の形を親の env（`RUST_TEST_THREADS`）に依らず 1 つに決める（#2248）
    cmd.args([
        "--exact",
        test,
        "--include-ignored",
        "--test-threads=1",
        "--nocapture",
    ])
    .env(CHILD_ENV, test)
    .stdin(Stdio::null());
    for key in remove {
        cmd.env_remove(key);
    }
    let out = cmd.output().expect("子のテストを起こせる");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    assert!(
        out.status.success(),
        "子（{test}）が失敗した（{}）:\n{stdout}{stderr}",
        out.status
    );
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "子が {test} を走らせていない（名前の打ち間違い・改名漏れ？）:\n{stdout}{stderr}"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libtestの名前はcrate名を落とした残り() {
        assert_eq!(
            libtest_name("tako_control::dispatch::tests::名前"),
            "dispatch::tests::名前"
        );
        assert_eq!(libtest_name("issue2400_watchdog::名前"), "名前");
        assert_eq!(libtest_name("名前だけ"), "名前だけ");
    }

    /// 子の中でだけ env を書き、親のプロセスへは漏れない（子の env は親に戻らない）
    #[test]
    fn 子で書いたenvは親へ漏れない() {
        const KEY: &str = "TAKO_2400_TEST_CHILD_ONLY";
        if crate::run_alone!(子で書いたenvは親へ漏れない) {
            assert!(
                std::env::var_os(KEY).is_none(),
                "子で立てた env が親に見えている"
            );
            return;
        }
        assert!(in_alone_child());
        assert_alone("子で書いたenvは親へ漏れない");
        std::env::set_var(KEY, "1");
        assert_eq!(std::env::var(KEY).as_deref(), Ok("1"));
    }

    /// 名前を打ち間違えると子は 0 本で終わる（終了コードは 0）。それを緑にせず名指して落ちる
    #[test]
    fn 名前を打ち間違えた子は0本で緑にしない() {
        if in_alone_child() {
            return;
        }
        let caught = std::panic::catch_unwind(|| {
            rerun_alone("test_child::tests::存在しないテストの名前", &[])
        });
        let msg = caught
            .expect_err("0 本の子を通した")
            .downcast::<String>()
            .map(|s| *s)
            .unwrap_or_default();
        assert!(
            msg.contains("走らせていない") && msg.contains("存在しないテストの名前"),
            "{msg}"
        );
    }

    /// 手動専用（`#[ignore]`）のテストも、親を `--ignored` で走らせれば子で同じ 1 本が走る
    /// （子は `--include-ignored`。CI では走らない）
    #[test]
    #[ignore = "親を --ignored で走らせたときの子の起こし方を見る（手動）"]
    fn 手動専用のテストも子で同じ1本が走る() {
        if crate::run_alone!(手動専用のテストも子で同じ1本が走る) {
            return;
        }
        assert!(in_alone_child(), "子で走っていない");
    }

    /// 子の外で assert_alone を呼ぶと panic する（通し忘れを漏れる前に名指す）
    #[test]
    fn 子の外のassert_aloneはpanicする() {
        if in_alone_child() {
            // 子として起こされた回（別のテストの子）では見ない
            return;
        }
        let caught = std::panic::catch_unwind(|| assert_alone("HomeGuard::set"));
        let msg = caught
            .expect_err("子の外で通った")
            .downcast::<String>()
            .map(|s| *s)
            .unwrap_or_default();
        assert!(
            msg.contains("HomeGuard::set") && msg.contains("run_alone!"),
            "{msg}"
        );
    }
}
