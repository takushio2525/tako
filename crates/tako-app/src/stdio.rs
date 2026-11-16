//! 出力マクロの差し替え（Issue #2936）
//!
//! **このクレートの `println!` / `print!` / `eprintln!` / `eprint!` は std のものではなく、
//! ここで定義した同名のマクロを指す**（`main.rs` の最初の `mod` 宣言の `#[macro_use]`）。
//! 実体は `tako_core::stdio` の 1 実装で、main が 2 文目で採る GUI の規則
//! （`tako_core::stdio::adopt_gui_rule`）により、標準出力・標準エラーへの書き込みの失敗は
//! 種類によらず捨てて続ける。std の版は書けないと panic し、アプリごと落ちていた
//! （Windows の実測は受け継いだ std ハンドルが書けない別物を指した `ERROR_ACCESS_DENIED`。
//! 理由と比べた案は `tako_core::stdio` の先頭）。書けた分の出力は std の版と同じ。
//! テストのビルドは main を通らないので std の版と同じ（テストハーネスの出力の捕捉も効く）

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! println {
    () => {
        ::tako_core::stdio::out(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        ::tako_core::stdio::out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! print {
    ($($arg:tt)*) => {
        ::tako_core::stdio::out(format_args!($($arg)*))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! eprintln {
    () => {
        ::tako_core::stdio::err(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        ::tako_core::stdio::err(format_args!("{}\n", format_args!($($arg)*)))
    };
}

#[allow(unused_macros)] // クレートによっては使わない版がある
macro_rules! eprint {
    ($($arg:tt)*) => {
        ::tako_core::stdio::err(format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use tako_core::test_child::{
        assert_same_output_on_usable_stderr, in_alone_child, libtest_name, run_broken_stderr_cases,
        stdio_child_report, StderrTarget,
    };
    use tako_core::test_residue::ScratchDir;

    /// 代表の行: ブラウザへ渡す URL を弾いたときの警告（`open_preview`）。弾いて返るだけで
    /// 副作用が無いので、子の中で何度走らせてもよい
    const URL_WARNING: &str = "warning: ブラウザで開けない URL（";

    fn representative_gui_writes() {
        crate::open_preview("javascript:alert(2936)");
        // このクレートの出力マクロそのもの（差し替えの宣言が効いているか）
        eprintln!("tako-app eprintln {}", 2936);
        eprint!("tako-app eprint の改行なし");
    }

    /// 修正前は std の `eprintln!` がこの行で panic し、GUI ごと落ちた（Windows の実測は
    /// `ERROR_ACCESS_DENIED`）。子の中で main と同じ口（`tako_core::stdio::adopt_gui_rule`）を採る
    #[test]
    fn issue2936_書けない標準エラーでもguiの出力で落ちない() {
        if in_alone_child() {
            stdio_child_report(representative_gui_writes);
            return;
        }
        let test = libtest_name(concat!(
            module_path!(),
            "::issue2936_書けない標準エラーでもguiの出力で落ちない"
        ));
        let scratch = ScratchDir::new("issue2936-app");
        let runs = run_broken_stderr_cases(test, &[], scratch.path()).expect("親で走る");
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
            }
        }
    }

    #[test]
    fn issue2936_使える標準エラーへの出力は新旧で同じ() {
        if in_alone_child() {
            stdio_child_report(representative_gui_writes);
            return;
        }
        let test = libtest_name(concat!(
            module_path!(),
            "::issue2936_使える標準エラーへの出力は新旧で同じ"
        ));
        let scratch = ScratchDir::new("issue2936-app-usable");
        let stderr =
            assert_same_output_on_usable_stderr(test, &[], scratch.path()).expect("親で走る");
        let text = String::from_utf8_lossy(&stderr);
        assert!(
            text.contains(URL_WARNING),
            "代表の行（open_preview の警告）が出ていない:\n{text}"
        );
        assert!(
            text.ends_with("tako-app eprintln 2936\ntako-app eprint の改行なし"),
            "このクレートの出力マクロの行が出ていない:\n{text}"
        );
    }
}
