//! tako.exe（CLI）へ Windows のアイコン / バージョン情報リソースを埋め込む（#587）。
//!
//! GUI 側（tako-app/build.rs）と対の内容。CLI 単体でもエクスプローラーとコンソール
//! ウィンドウのアイコン、プロパティのバージョン情報が正しく出るようにする。
//! ガードの設計意図は tako-app/build.rs の冒頭コメントを参照（変更するときは対で直す）。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // ホストではなく「これから作るバイナリ」の OS を見る
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

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
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon/tako.ico");
    res.set("ProductName", "tako");
    res.set("FileDescription", "tako CLI - control tako panes and tabs");
    res.set("CompanyName", "tako project");
    // パッケージ名は tako-cli だが、配布されるファイル名は tako.exe。
    // winresource の既定はパッケージ名なので明示的に上書きする
    res.set("InternalName", "tako.exe");
    res.set("OriginalFilename", "tako.exe");
    res.set(
        "LegalCopyright",
        "Copyright (C) 2026 tako project. Licensed under GPL-3.0-or-later.",
    );
    res.compile()
        .expect("Windows リソース（アイコン / バージョン情報）の埋め込みに失敗した");
}

#[cfg(not(windows))]
fn embed_windows_resources() {
    if std::env::var("PROFILE").unwrap_or_default() == "release" {
        println!(
            "cargo::warning=非 Windows ホストのためアイコン / バージョン情報リソースを埋め込めなかった。\
             配布用の exe は Windows 実機（installer/windows/release-windows.ps1）で作ること"
        );
    }
}
