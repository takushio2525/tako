//! GUI のプロセスで走る出力は、書けない標準エラーの下でもアプリを落とさない（Issue #2936）
//!
//! 修正前は std の `eprintln!` が書き込みの失敗で panic し、Windows の実機で器の取り合いの警告
//! （`backend::psmux` の `wrap_spawn` の `Claim::Foreign`）から
//! `failed printing to stderr: アクセスが拒否されました。 (os error 5)` を出して GUI ごと落ちた。
//! 直し方は `tako_core::stdio` の先頭（GUI の規則 = `adopt_gui_rule`）。
//!
//! 子（このテストバイナリをその 1 本だけで起こし直したもの）の標準エラーを壊し、子の中で GUI の
//! 規則を採ってから**本物の警告の行**を走らせる。A/B の旧（`TAKO_2936_LEGACY=1`）は規則を採らない
//! = 修正前の形で、同じ場面で panic することも見る。壊し方は
//!
//! - 読み手を閉じたパイプ（unix は EPIPE・Windows は `ERROR_NO_DATA`。親が後から閉じたパイプと同じ）
//! - 読み取り専用で開いたファイル（Windows は `ERROR_ACCESS_DENIED` = 実測と同じ os error 5）
//!
//! の 2 通り。Windows の CI は `.github/workflows/ci.yml` の名指しの段で走らせる。
//! lib のテストではなく専用の結合テストに置くのは、ライブラリを本番と同じ形（cfg(test) なし）で
//! リンクし、子へ渡すパイプを漏らしうる兄弟のテストを減らすため。

use std::path::PathBuf;

use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::terminal::SpawnOptions;
use tako_core::test_child::{
    assert_same_output_on_usable_stderr, in_alone_child, libtest_name, run_broken_stderr_cases,
    stdio_child_report, StderrTarget, STDIO_OUT_PREFIX,
};
use tako_core::test_residue::ScratchDir;

/// 器の取り合いの警告（`backend/psmux.rs` の `wrap_spawn`）の文の一部
const PSMUX_WARNING: &str = "は別インスタンス（pid ";

/// 子の本文: 同じオーナー記録の置き場を持つ 2 つの器で同じセッションへ attach する。
/// 1 つ目が記録を握り、2 つ目は `Claim::Foreign` で警告の行を標準エラーへ出す
/// （生きたインスタンスと同じ置き場で 2 つ目を起こした #2936 の実測と同じ形）
fn psmux_foreign_claim_warning() {
    let data = std::env::var_os("TAKO_DATA_DIR").expect("親が TAKO_DATA_DIR を渡す");
    let owners = PathBuf::from(data).join("backend-owners-2936");
    let backend = |socket: &str| {
        PsmuxBackend::with_parts(
            "psmux".into(),
            "3.3.7".into(),
            socket.into(),
            owners.clone(),
        )
    };
    let first = backend("tako-2936-a");
    let second = backend("tako-2936-b");
    let session = SessionRef::new("tako-293600000001").expect("器の名前");
    let _ = first.wrap_spawn(SpawnOptions::default(), &session);
    let _ = second.wrap_spawn(SpawnOptions::default(), &session);
    // 使える標準出力・標準エラーで新旧の出力を比べるための行（出力マクロが展開する 4 つの形）
    tako_core::stdio::out(format_args!(
        "{}\n",
        format_args!("{STDIO_OUT_PREFIX}println 日本語 {}", 2936)
    ));
    tako_core::stdio::out(format_args!("{STDIO_OUT_PREFIX}print\n"));
    tako_core::stdio::err(format_args!(
        "{}\n",
        format_args!("eprintln 日本語 {}", 2936)
    ));
    tako_core::stdio::err(format_args!("\n"));
    tako_core::stdio::err(format_args!("eprint の改行なし"));
}

#[test]
fn 書けない標準エラーでも器の取り合いの警告で落ちない() {
    if in_alone_child() {
        stdio_child_report(psmux_foreign_claim_warning);
        return;
    }
    let test = libtest_name(concat!(
        module_path!(),
        "::書けない標準エラーでも器の取り合いの警告で落ちない"
    ));
    let scratch = ScratchDir::new("issue2936-psmux");
    let data = scratch.path().to_string_lossy().into_owned();
    let runs = run_broken_stderr_cases(test, &[("TAKO_DATA_DIR", &data)], scratch.path())
        .expect("親で走る");
    // 読み取り専用のファイル: Windows は ERROR_ACCESS_DENIED（#2936 の実測と同じ os error 5）で
    // 書けず、修正前の形は panic する。unix は EBADF で std の版も黙って捨てる（落ちない）
    for run in runs
        .iter()
        .filter(|r| r.target == StderrTarget::ReadOnlyFile)
    {
        if cfg!(windows) {
            assert!(
                run.probe.ends_with(":5"),
                "Windows の読み取り専用のハンドルは ERROR_ACCESS_DENIED のはず: {}",
                run.summary()
            );
        } else {
            assert_eq!(
                run.probe,
                "ok",
                "unix の EBADF は std が捨てる: {}",
                run.summary()
            );
        }
    }
}

#[test]
fn 使える標準エラーへの出力は新旧で同じ() {
    if in_alone_child() {
        stdio_child_report(psmux_foreign_claim_warning);
        return;
    }
    let test = libtest_name(concat!(
        module_path!(),
        "::使える標準エラーへの出力は新旧で同じ"
    ));
    let scratch = ScratchDir::new("issue2936-usable");
    let data = scratch.path().to_string_lossy().into_owned();
    let stderr =
        assert_same_output_on_usable_stderr(test, &[("TAKO_DATA_DIR", &data)], scratch.path())
            .expect("親で走る");
    let text = String::from_utf8_lossy(&stderr);
    // 子の本文が本当に器の取り合いの警告の行を通っている（壊した向け先では見えないのでここで見る）
    assert!(
        text.contains(PSMUX_WARNING),
        "器の取り合いの警告が出ていない（Claim::Foreign の経路を通っていない）:\n{text}"
    );
    assert!(
        text.ends_with("eprintln 日本語 2936\n\neprint の改行なし"),
        "出力マクロの 4 つの形の行が出ていない:\n{text}"
    );
}
