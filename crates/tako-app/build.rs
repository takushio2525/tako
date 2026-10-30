//! tako-app.exe（GUI）へ Windows のアイコン / バージョン情報リソースを埋め込む（#587）。
//!
//! これが無いとタスクバー・Alt+Tab・エクスプローラーが既定の白いアイコンのままになる。
//! インストーラーが作るショートカットだけは `IconFilename` で `.ico` を直接参照するので
//! 正しく見えてしまい、見落としやすい。アイコンの正は `assets/icon/tako.ico`
//! （`installer/windows/make-icon.ps1` が A 案 SVG から生成する）。
//!
//! macOS ビルドに副作用が無いことは二重のガードで担保する:
//!   1. **ターゲット**が Windows でなければ即 return（macOS ネイティブビルドは何もしない）
//!   2. **ホスト**が Windows でなければリソースコンパイルをしない。rc.exe は Windows SDK に
//!      しか無く、macOS からのクロス検査（`scripts/check-windows.sh`）を落とさないため。
//!      配布 exe は Windows 実機（`installer/windows/release-windows.ps1`）で作るので
//!      配布物には影響しない
//!
//! アイコングループのリソース ID は 1（winresource の既定）でなければならない。
//! gpui の Windows バックエンドがウィンドウクラスの hIcon として「自モジュールの
//! リソース ID 1」を LoadImageW で直接引くため（`gpui_windows/src/platform.rs` の
//! `load_icon`）。ID が変わるとエクスプローラーの表示は保たれたまま、タスクバーと
//! Alt+Tab だけが既定アイコンへ戻る。
//!
//! 2 の分岐は tako-cli/build.rs にも同じ形で置いてある。ビルドスクリプトは
//! クレートをまたいで共有できず、共有のためだけにヘルパークレートを足す方が高くつくため
//! （両方 40 行弱で、変更するときは対で直す）。
//!
//! もう 1 つの役目は、two-face に無い構文（#1954。`syntaxes/` の `.sublime-syntax`）をビルド時に
//! syntect の構文定義へ読んでおくこと（`compile_extra_syntaxes`。全ターゲット・全プロファイル）。
//!
//! 3 つ目は、構文の塗りの正規表現へ前置きを足した文脈をビルド時に作っておくこと
//! （`generate_prefiltered_syntaxes`。#1917 / #1935。全ターゲット・全プロファイル）。

// 書き換えの本体と直列化の鏡はアプリと同じファイルを読む（起動時に差し込む側と形を揃える）。
// 鏡のうち実行時だけが使う定数（#1935 の A/B の対象表）はここでは読まない
#[allow(dead_code)]
#[path = "src/syntax_mirror.rs"]
mod syntax_mirror;
#[path = "src/syntax_rewrite.rs"]
mod syntax_rewrite;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // TAKO_FULL_VERSION: Cargo.toml の version に、リリースビルド時の TAKO_WIN_NUM
    // （例: "3"）があれば `-win.3` を付与。update_checker.rs が自バイナリの正確な版数として
    // 使い、-win.N 間の更新検知を可能にする。全プラットフォーム・全プロファイルで emit する
    emit_full_version();

    // two-face に無い構文（#1954）。全ターゲットで要るので Windows の判定より前に置く
    compile_extra_syntaxes();

    // 塗りの正規表現の前置き（#1917 / #1935）。同上
    generate_prefiltered_syntaxes();

    // `cfg!(target_os = "windows")` はビルドスクリプト自身（= ホスト）を指してしまうので使わない。
    // CARGO_CFG_TARGET_OS が「これから作るバイナリ」の OS。
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    // パスはパッケージルート（crates/tako-app）基準。cargo がビルドスクリプトの
    // カレントディレクトリをそこに固定する。バージョンはワークスペースの Cargo.toml が正
    // #1133: Windows/MSVC はメインスレッドのスタックが既定 1 MiB（PE ヘッダの
    // SizeOfStackReserve = 0x100000）しかない。GPUI アプリの debug ビルドは
    // ビルダー形の中間値が -O0 でスタックスロット再利用されないため 1 関数のフレームが
    // 数百 KiB になり（実測: render_tmux_view 564KiB / mcp::catalog::tools 828KiB /
    // セルフテストの poll 479KiB）、入れ子で呼ぶと 1 MiB を食い潰して
    // **判定行も出さずプロセスごと落ちる**。macOS / Linux は既定 8 MiB なので落ちない。
    // GPUI 本家（Zed の crates/zed/build.rs）も同じ理由で同じ値を宣言している。
    // 予約はアドレス空間だけなので 64bit では実コストが無い。
    // 数値の正は tako_core::platform::stack::REQUIRED_MAIN_STACK_BYTES で、
    // 写しは crates/tako-control/tests/windows_stack_reserve_watchdog.rs が拘束する。
    // gnu ツールチェーンはリンカ引数の書式が違う（-Wl,--stack）ので msvc だけに限る
    if std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "msvc" {
        println!("cargo:rustc-link-arg=/stack:8388608");
    }

    println!("cargo:rerun-if-changed=../../assets/icon/tako.ico");
    println!("cargo:rerun-if-changed=../../Cargo.toml");

    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    // 文字列はすべて ASCII にする。VERSIONINFO の言語ブロックは既定で US English (0x0409) で、
    // そこに非 ASCII を混ぜるとコードページ次第でプロパティ画面が化ける
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon/tako.ico");
    res.set("ProductName", "tako");
    res.set("FileDescription", "tako - AI agent fleet terminal");
    res.set("CompanyName", "tako project");
    res.set("InternalName", "tako-app.exe");
    res.set("OriginalFilename", "tako-app.exe");
    res.set(
        "LegalCopyright",
        "Copyright (C) 2026 tako project. Licensed under GPL-3.0-or-later.",
    );
    // FileVersion / ProductVersion は CARGO_PKG_VERSION（= ワークスペースの version）から
    // winresource が自動で入れる
    res.compile()
        .expect("Windows リソース（アイコン / バージョン情報）の埋め込みに失敗した");
}

#[cfg(not(windows))]
fn embed_windows_resources() {
    // 配布物を非 Windows ホストで作ろうとしたときだけ警告する。macOS のクロス検査は
    // PROFILE=debug なので、`scripts/check-windows.sh` の出力は増えない
    if std::env::var("PROFILE").unwrap_or_default() == "release" {
        println!(
            "cargo::warning=非 Windows ホストのためアイコン / バージョン情報リソースを埋め込めなかった。\
             配布用の exe は Windows 実機（installer/windows/release-windows.ps1）で作ること"
        );
    }
}

/// two-face に無い構文（#1954）を YAML（`.sublime-syntax`）から syntect の構文定義へ読み、
/// 直列化して `OUT_DIR/extra-syntaxes.bin` へ置く。起動時は YAML を読まない
/// （YAML の読み込みは release で 57 ms かかり、YAML のパーサを配布バイナリへ入れることになる）。
/// ほかの構文への参照の結びつけは、起動時に `src/syntax_extra.rs` が既存の構文セットに対して行う。
///
/// ここで落とすもの（どれも見逃すと、塗る途中や 1 行目での構文の判定で syntect が panic する）:
/// - 正規表現が fancy-regex で組めない（two-face が PowerShell を外していた理由。bat 同梱の
///   PowerShell はサブルーチン呼び出し `\g<1>` を使う）
/// - `first_line_match` の変数（`{{…}}`）。syntect は文脈の正規表現の変数は展開するが
///   `first_line_match` は展開しないので、ここで展開してから組めるかを見る
fn compile_extra_syntaxes() {
    use syntect::parsing::syntax_definition::Pattern;
    use syntect::parsing::{Regex, SyntaxDefinition};

    /// 構文のファイルと、読む前に当てる置き換え（`(元, 後)`。元が無ければ上流が変わったので落とす）
    const SOURCES: &[(&str, &[(&str, &str)])] = &[(
        "syntaxes/PowerShell/PowerShell.sublime-syntax",
        // 埋め込みの C# は上流の別ファイル（Sublime Text 4 の `extends` で書かれていて syntect は
        // 読めない）を名指すので、two-face の C# へ向ける（置き換えの内容は PROVENANCE.md）
        &[("scope:source.cs.embedded.powershell", "scope:source.cs")],
    )];

    let mut defs = Vec::new();
    for (path, replaces) in SOURCES {
        println!("cargo:rerun-if-changed={path}");
        let mut text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("{path} を読めない（#1954）: {e}"));
        for (from, to) in *replaces {
            assert!(
                text.contains(from),
                "{path}: 置き換える `{from}` が見つからない。上流の構文が変わったので \
                 PROVENANCE.md と置き換えを見直すこと（#1954）"
            );
            text = text.replace(from, to);
        }
        let mut def = SyntaxDefinition::load_from_str(&text, true, None)
            .unwrap_or_else(|e| panic!("{path} を構文として読めない（#1954）: {e:?}"));
        if let Some(line) = def.first_line_match.take() {
            def.first_line_match = Some(expand_variables(&line, &def.variables, path));
        }
        let mut regexes: Vec<(String, &str)> = def
            .first_line_match
            .iter()
            .map(|r| ("first_line_match".to_string(), r.as_str()))
            .collect();
        for (name, context) in &def.contexts {
            for pattern in &context.patterns {
                if let Pattern::Match(m) = pattern {
                    regexes.push((name.clone(), m.regex.regex_str()));
                }
            }
        }
        for (context, regex) in regexes {
            if let Some(e) = Regex::try_compile(regex) {
                panic!(
                    "{path}: 文脈 {context} の正規表現を fancy-regex で組めない（#1954）\n  \
                     {regex}\n  {e}"
                );
            }
        }
        defs.push(def);
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("extra-syntaxes.bin");
    std::fs::write(&out, syntect::dumps::dump_binary(&defs))
        .unwrap_or_else(|e| panic!("{} を書けない（#1954）: {e}", out.display()));
}

/// 対象の構文（`src/syntax_mirror.rs` の `TARGETS`）の文脈の正規表現へ「当たらない行で
/// VM を起こさない」等価な前置きを足し、`OUT_DIR/prefiltered-syntaxes.bin` へ置く（#1917 / #1935）。
/// 起動時は `src/syntax_prefilter.rs` がこれを土台の構文セットへ差し込むだけで、正規表現は読まない
/// （実行時に書き換えると全 214 構文で release 約 340 ms・debug はその約 10 倍かかる）。
///
/// 書き換えられなければ（syntect の直列化の形が変わった等）警告を出して空の表を置く。
/// そのときアプリは元の正規表現のまま塗り、単体テスト（`syntax_prefilter::tests`）が名指しで落ちる
fn generate_prefiltered_syntaxes() {
    println!("cargo:rerun-if-changed=src/syntax_rewrite.rs");
    println!("cargo:rerun-if-changed=src/syntax_mirror.rs");
    let names: Vec<&str> = syntax_mirror::TARGETS.iter().map(|t| t.name).collect();
    let generated = syntax_rewrite::generate(&two_face::syntax::extra_newlines(), &names);
    let bytes = generated.unwrap_or_else(|e| {
        println!(
            "cargo::warning=塗りの正規表現の前置きを作れなかった（元の正規表現のまま塗る。#1935）: {e}"
        );
        bincode::serialize(&Vec::<syntax_mirror::PrefilteredSyntax>::new()).expect("空の表")
    });
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("prefiltered-syntaxes.bin");
    std::fs::write(&out, bytes)
        .unwrap_or_else(|e| panic!("{} を書けない（#1935）: {e}", out.display()));
}

/// `{{名前}}` を構文の変数で展開する（変数の中の変数も。syntect の文脈の正規表現と同じ扱い）
fn expand_variables(
    text: &str,
    variables: &std::collections::HashMap<String, String>,
    path: &str,
) -> String {
    let mut out = text.to_string();
    for _ in 0..64 {
        let Some(start) = out.find("{{") else {
            return out;
        };
        let end = out[start..]
            .find("}}")
            .map(|at| start + at)
            .unwrap_or_else(|| panic!("{path}: `{{{{` が閉じていない（#1954）"));
        let name = &out[start + 2..end];
        let value = variables
            .get(name)
            .unwrap_or_else(|| panic!("{path}: 変数 {name} が無い（#1954）"));
        out = format!("{}{value}{}", &out[..start], &out[end + 2..]);
    }
    panic!("{path}: 変数の展開が終わらない（#1954）");
}

fn emit_full_version() {
    let pkg = std::env::var("CARGO_PKG_VERSION").unwrap();
    let full = match std::env::var("TAKO_WIN_NUM") {
        Ok(n) if !n.is_empty() => format!("{pkg}-win.{n}"),
        _ => pkg,
    };
    println!("cargo:rustc-env=TAKO_FULL_VERSION={full}");
}
