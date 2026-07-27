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
             配布用の exe は windows ランナー（.github/workflows/windows-release.yml）で作ること"
        );
    }
}
