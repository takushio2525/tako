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

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // `cfg!(target_os = "windows")` はビルドスクリプト自身（= ホスト）を指してしまうので使わない。
    // CARGO_CFG_TARGET_OS が「これから作るバイナリ」の OS。
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    // パスはパッケージルート（crates/tako-app）基準。cargo がビルドスクリプトの
    // カレントディレクトリをそこに固定する。バージョンはワークスペースの Cargo.toml が正
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
