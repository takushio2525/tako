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

// ===== Issue #2936: 書けない標準エラーの下で GUI の出力の経路を走らせる =====
//
// 子の標準エラーだけを壊す（標準出力は親が読むパイプのまま）。libtest は自分の標準出力へ
// 書けないと本文を走らせずに終わるので、結果は本文が標準出力へ出す印（`STDIO_MARK`）で運ぶ。
// 使い方（親と子は同じテスト関数。子では `in_alone_child()` が真）:
//
//     if in_alone_child() { stdio_child_report(|| 出力の経路を走らせる()); return; }
//     let runs = run_broken_stderr_cases(NAME, &[], scratch.path()).unwrap();

/// 子の本文が標準出力へ出す結果の印
const STDIO_MARK: &str = "TAKO2936_STDIO";
/// 子の本文が出した行（使える標準出力への出力を新旧で比べる）の頭
pub const STDIO_OUT_PREFIX: &str = "TAKO2936_OUT ";

/// 書き込みの経路を走らせる子の数を 1 本に絞る（#1748 と同じ理由。子へ渡すパイプが兄弟の子へ
/// 漏れると読み手が生き残り、壊したはずの標準エラーが書けてしまう）
static STDIO_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 子の標準エラーの向け先（#2936）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StderrTarget {
    /// 読み手を閉じ済みのパイプ（最初の書き込みで unix は EPIPE・Windows は `ERROR_NO_DATA`）
    ClosedPipe,
    /// 読み取り専用で開いたファイル（Windows は `ERROR_ACCESS_DENIED` = #2936 の実測と同じ
    /// os error 5。unix は EBADF で、std の版もこれは黙って捨てる）
    ReadOnlyFile,
    /// 親が読むパイプ（書ける）
    Pipe,
    /// 書けるファイル
    File,
}

/// 子の 1 回の結果（#2936）
#[derive(Debug, Clone)]
pub struct StdioRun {
    pub target: StderrTarget,
    pub legacy: bool,
    /// 規則を採る前に標準エラーへ 1 バイト書けたか（`ok` / `err:<ErrorKind>:<os error>`）
    pub probe: String,
    /// GUI の規則を採ったか（A/B の旧なら false）
    pub adopted: bool,
    /// `ok` / `panic:<メッセージ>`
    pub result: String,
    /// 子の標準出力のうち、本文が [`STDIO_OUT_PREFIX`] で出した行
    pub out_lines: Vec<String>,
    /// 子の標準エラーの中身（向け先が [`StderrTarget::Pipe`] / [`StderrTarget::File`] のときだけ）
    pub stderr: Vec<u8>,
}

impl StdioRun {
    /// 規則を採る前の標準エラーが書けなかった（= 修正前なら panic する場面だった）
    pub fn stderr_was_broken(&self) -> bool {
        self.probe.starts_with("err:")
    }

    /// 結果の 1 行（テストの出力へ残す）
    pub fn summary(&self) -> String {
        format!(
            "{STDIO_MARK} target={:?} legacy={} probe={} adopted={} result={}",
            self.target, self.legacy, self.probe, self.adopted, self.result
        )
    }
}

/// 子の本文（#2936）。標準エラーへ 1 バイト書けるかを確かめ、GUI の規則を採り
/// （[`crate::stdio::GUI_RULE_LEGACY_ENV`] なら採らない）、`writes` を走らせて結果を印で返す。
/// `writes` の panic は受け止める（受け止めないと子のテストが落ちて結果を運べない）
pub fn stdio_child_report(writes: impl FnOnce() + std::panic::UnwindSafe) {
    use std::io::Write;
    // 規則を採る前に、std の版が書けるかを直に確かめる（向け先を壊したつもりで壊れていない
    // 場面 = 兄弟の子へ読み手が漏れた / std が黙って捨てる EBADF を、親が見分けるため）
    let probe = match std::io::stderr().write_all(b"\n") {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("err:{:?}:{}", e.kind(), e.raw_os_error().unwrap_or(-1)),
    };
    let adopted = crate::stdio::adopt_gui_rule();
    let result = match std::panic::catch_unwind(writes) {
        Ok(()) => "ok".to_string(),
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            format!("panic:{}", message.replace('\n', " "))
        }
    };
    // 標準出力は親が読むパイプ（書ける）
    println!("{STDIO_MARK} probe={probe} adopted={adopted} result={result}");
}

/// [`rerun_alone`] と同じ子（その 1 本だけ）を、標準エラーの向け先と A/B を指定して 1 回起こす。
/// 子では `None`（本文がそのまま走る）
pub fn rerun_with_stderr(
    test: &str,
    target: StderrTarget,
    legacy: bool,
    set: &[(&str, &str)],
    scratch: &std::path::Path,
) -> Option<StdioRun> {
    if in_alone_child() {
        return None;
    }
    let _serial = STDIO_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let file = scratch.join(format!("stderr-{target:?}-{legacy}.txt"));
    let stderr = match target {
        StderrTarget::ClosedPipe => {
            let (reader, writer) = std::io::pipe().expect("パイプを作れない");
            drop(reader);
            Stdio::from(writer)
        }
        StderrTarget::ReadOnlyFile => {
            std::fs::write(&file, b"").expect("読み取り専用にするファイルを作れない");
            Stdio::from(std::fs::File::open(&file).expect("読み取り専用で開けない"))
        }
        StderrTarget::Pipe => Stdio::piped(),
        StderrTarget::File => {
            Stdio::from(std::fs::File::create(&file).expect("標準エラーのファイルを作れない"))
        }
    };
    let exe = std::env::current_exe().expect("テストバイナリのパス");
    let mut cmd = std::process::Command::new(exe);
    crate::platform::process::no_console_window(&mut cmd);
    cmd.args([
        "--exact",
        test,
        "--include-ignored",
        "--test-threads=1",
        "--nocapture",
    ])
    .env(CHILD_ENV, test)
    .env_remove(crate::stdio::GUI_RULE_LEGACY_ENV)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(stderr);
    if legacy {
        cmd.env(crate::stdio::GUI_RULE_LEGACY_ENV, "1");
    }
    for (key, value) in set {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("子のテストを起こせる");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "子（{test}・{target:?}・legacy={legacy}）が失敗した（{}）:\n{stdout}",
        out.status
    );
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "子が {test} を走らせていない（名前の打ち間違い・改名漏れ？）:\n{stdout}"
    );
    // libtest は `test <名前> ... ` の後ろに改行を置かずに本文の出力を続けるので、印は行の途中からも拾う
    let report = stdout
        .lines()
        .find_map(|l| l.find(STDIO_MARK).map(|at| &l[at + STDIO_MARK.len()..]))
        .unwrap_or_else(|| {
            panic!(
                "子が結果の印を出していない（本文が stdio_child_report を通っていない）:\n{stdout}"
            )
        });
    let field = |name: &str| {
        report
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .unwrap_or_default()
            .to_string()
    };
    // `result=` は末尾の欄で、panic のメッセージに空白が入るので残り全部を取る
    let result = report
        .split_once(" result=")
        .map(|(_, r)| r.to_string())
        .unwrap_or_default();
    let stderr = match target {
        StderrTarget::Pipe => out.stderr,
        StderrTarget::File => std::fs::read(&file).expect("標準エラーのファイルを読めない"),
        _ => Vec::new(),
    };
    Some(StdioRun {
        target,
        legacy,
        probe: field("probe"),
        adopted: field("adopted") == "true",
        result,
        out_lines: stdout
            .lines()
            .filter_map(|l| l.find(STDIO_OUT_PREFIX).map(|at| l[at..].to_string()))
            .collect(),
        stderr,
    })
}

/// 書けない標準エラー 2 通り（[`StderrTarget::ClosedPipe`] / [`StderrTarget::ReadOnlyFile`]）×
/// 新旧で子を走らせ、次を確かめて結果を返す（#2936。子では `None`）:
///
/// - 新（GUI の規則を採る）は向け先によらず走り切る
/// - 旧（`TAKO_2936_LEGACY=1`）は、規則の前の書き込みが失敗した向け先では
///   `failed printing to stderr` で panic する（= 修正前に落ちていた場面を作れている）
/// - 閉じたパイプは必ず書けない（書けたら兄弟の子へ読み手が漏れたので、5 回まで起こし直す）
///
/// 読み取り専用のファイルが書けないか（Windows は `ERROR_ACCESS_DENIED`・unix は std が黙って
/// 捨てる EBADF）は OS で違うので、OS ごとの期待は呼び手のテストが結果を見て確かめる
pub fn run_broken_stderr_cases(
    test: &str,
    set: &[(&str, &str)],
    scratch: &std::path::Path,
) -> Option<Vec<StdioRun>> {
    if in_alone_child() {
        return None;
    }
    let mut runs = Vec::new();
    for target in [StderrTarget::ClosedPipe, StderrTarget::ReadOnlyFile] {
        for legacy in [false, true] {
            let mut attempt = 0;
            let run = loop {
                attempt += 1;
                let run = rerun_with_stderr(test, target, legacy, set, scratch)?;
                if target != StderrTarget::ClosedPipe || run.stderr_was_broken() || attempt >= 5 {
                    break run;
                }
            };
            let line = run.summary();
            println!("{line}");
            assert_eq!(run.adopted, !legacy, "A/B の旧だけが規則を採らない: {line}");
            if target == StderrTarget::ClosedPipe {
                assert!(
                    run.stderr_was_broken(),
                    "読み手を閉じたパイプへ 5 回とも書けた（読み手が兄弟の子へ漏れている）: {line}"
                );
            }
            if !legacy {
                assert_eq!(
                    run.result, "ok",
                    "GUI の規則を採ったのに書けない標準エラーで panic した: {line}"
                );
            } else if run.stderr_was_broken() {
                assert!(
                    run.result.starts_with("panic:failed printing to stderr"),
                    "A/B の旧（修正前の形）が書けない標準エラーで panic していない = 場面を作れていない: {line}"
                );
            } else {
                assert_eq!(run.result, "ok", "書ける向け先で旧が panic した: {line}");
            }
            runs.push(run);
        }
    }
    Some(runs)
}

/// `pid ` の直後の数字を `<pid>` へ伏せる（子ごとに違う値。それ以外のバイトは触らない）
pub fn mask_pids(bytes: &[u8]) -> Vec<u8> {
    const KEY: &[u8] = b"pid ";
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(KEY) {
            out.extend_from_slice(KEY);
            i += KEY.len();
            let digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 0 {
                out.extend_from_slice(b"<pid>");
                i += digits;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 使える標準エラー（パイプ・ファイル）と標準出力へは、新旧で 1 バイトも変わらないことを
/// 確かめ、新の標準エラーの中身を返す（#2936。子では `None`）。子ごとに違う pid の数字だけは
/// [`mask_pids`] で伏せて比べる
pub fn assert_same_output_on_usable_stderr(
    test: &str,
    set: &[(&str, &str)],
    scratch: &std::path::Path,
) -> Option<Vec<u8>> {
    if in_alone_child() {
        return None;
    }
    let mut first = None;
    for target in [StderrTarget::Pipe, StderrTarget::File] {
        let new = rerun_with_stderr(test, target, false, set, scratch)?;
        let old = rerun_with_stderr(test, target, true, set, scratch)?;
        println!("{}\n{}", new.summary(), old.summary());
        for run in [&new, &old] {
            assert_eq!(
                run.result,
                "ok",
                "使える標準エラーで panic した: {}",
                run.summary()
            );
            assert!(
                !run.stderr_was_broken(),
                "使えるはずの標準エラーへ書けない: {}",
                run.summary()
            );
        }
        // 子ごとに違う pid（器の取り合いの警告が名指す）の数字だけを伏せて、残りをバイト列で比べる
        let (new_err, old_err) = (mask_pids(&new.stderr), mask_pids(&old.stderr));
        assert_eq!(
            String::from_utf8_lossy(&new_err),
            String::from_utf8_lossy(&old_err),
            "{target:?} へ向けた標準エラーの中身が新旧で違う"
        );
        assert_eq!(
            new_err, old_err,
            "{target:?} の標準エラーのバイト列が新旧で違う"
        );
        assert_eq!(new.out_lines, old.out_lines, "標準出力の行が新旧で違う");
        first.get_or_insert(new.stderr);
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_pidsはpidの直後の数字だけを伏せる() {
        assert_eq!(
            mask_pids("器 x は別インスタンス（pid 84588）が使用中\npid x / pid 7".as_bytes()),
            "器 x は別インスタンス（pid <pid>）が使用中\npid x / pid <pid>".as_bytes()
        );
        assert_eq!(mask_pids(b""), b"");
    }

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
