//! 番犬: Windows の PerMonitorV2 マニフェストが落ちていない（#1063）
//!
//! tako の Windows 版が「物理ピクセルをそのまま扱う」前提で動けるのは、実行ファイルへ
//! PerMonitorV2 のマニフェストが埋め込まれているからで、その埋め込みは
//! **gpui の既定フィーチャ `windows-manifest`** が行っている
//! （`crates/gpui/resources/windows/gpui.manifest.xml` を `embed-resource` で焼く）。
//! tako 自身のコードには 1 行も現れないので、
//!
//! - gpui の rev 追従でフィーチャ名 / 既定が変わる
//! - `default-features = false` を足す
//! - 依存を別クレートへ移す
//!
//! のどれでも**無言で**落ちる。落ちるとプロセスは DPI 非認識になり、OS が座標を
//! 仮想化して描画結果を拡大するだけなので**一見動いてしまう**（ぼやけるだけ）。
//! そして macOS では原理的に再現しないので、CI の macOS ジョブでは気づけない。
//!
//! そこで二段構えにする:
//!
//! 1. **ここ（macOS でも走る）**: 依存の宣言が既定フィーチャを殺していないこと
//! 2. **実機（Windows）の GUI プロセス**: セルフテスト項目 139 が
//!    `tako_core::platform::dpi::process_awareness()` の実測で `PerMonitorV2` を見る
//!
//! 1 だけでは「リンクされたか」までは見られず、2 だけでは Windows 実機を回すまで
//! 気づけない。両方あって初めて「入れ忘れ」と「落ちた」を早い段階で捕まえられる。
//!
//! 2 が**セルフテスト（= GUI プロセス）でなければならない**のは、マニフェストが
//! `tako-app.exe` にだけ焼かれるため。`cargo test` のテストバイナリと `tako.exe`（CLI）は
//! gpui に依存しないので Windows でも DPI 非認識で動く（実測。窓を持たないのでそれで正しい）。
//!
//! 実測の詳細（何が本当に壊れていて、何が計測の錯覚だったか）は #1063 を参照。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // crates/tako-control -> crates -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("ワークスペースルートを辿れない")
        .to_path_buf()
}

fn workspace_manifest() -> String {
    let path = workspace_root().join("Cargo.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// `<name> = { ... }` の 1 行を取り出す（依存宣言は 1 行で書かれている）
fn dependency_line(manifest: &str, name: &str) -> String {
    let prefix = format!("{name} = ");
    manifest
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| {
            panic!(
                "ワークスペースの Cargo.toml に `{name}` の依存宣言が無い。\
                 依存の書き方を変えたならこの番犬も直すこと（#1063）"
            )
        })
        .to_string()
}

#[test]
fn gpuiの既定フィーチャを殺していない() {
    let manifest = workspace_manifest();
    for name in ["gpui", "gpui_platform"] {
        let line = dependency_line(&manifest, name);
        let disables_defaults =
            line.contains("default-features = false") || line.contains("default_features = false");
        // 既定を切るなら windows-manifest を明示で戻すこと（切ること自体は禁じない）
        let keeps_manifest = line.contains("windows-manifest");
        assert!(
            !disables_defaults || keeps_manifest,
            "{name} が既定フィーチャを切っているのに `windows-manifest` を明示していない。\
             Windows 版の PerMonitorV2 マニフェストが落ちて DPI 非認識になる（#1063）。\
             宣言: {line}"
        );
    }
}

#[test]
fn gpuiのマニフェスト埋め込みはgpui側にしか無い() {
    // tako 自身が RT_MANIFEST を焼き始めると gpui のものと二重になる。
    // アイコン / バージョン情報は winresource で焼いているが、そちらは
    // マニフェストを既定で持たない（`set_manifest` を呼んだときだけ）。
    // 将来ここへ手を入れるなら、gpui 側と重複しないことを確認してからにする
    for krate in ["tako-app", "tako-cli"] {
        let path = workspace_root().join("crates").join(krate).join("build.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
        let sets_manifest = src.contains("set_manifest");
        assert!(
            !sets_manifest,
            "{krate}/build.rs が独自のマニフェストを焼こうとしている。\
             gpui の PerMonitorV2 マニフェストと RT_MANIFEST が二重になる（#1063）。\
             本当に必要なら gpui 側の埋め込みを切ってから 1 本にすること。{}",
            path.display()
        );
    }
}

/// 計測の道具そのものが DPI 非認識だと、#1063 と同じ錯覚（`GetWindowRect` は
/// 仮想化された値・スクリーンキャプチャは物理ピクセル）をまた作る。
/// リポジトリへ置く計測スクリプトは awareness を必ず自分で宣言する
#[test]
fn 計測スクリプトはdpi認識を自分で宣言する() {
    let path = workspace_root().join("scripts/windows/measure-window.ps1");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    assert!(
        src.contains("SetProcessDpiAwarenessContext"),
        "{} が SetProcessDpiAwarenessContext を呼んでいない（#1063）",
        path.display()
    );
    assert!(
        src.contains("GetAwarenessFromDpiAwarenessContext"),
        "{} が自分の awareness を検算していない（宣言が失敗しても気づけない。#1063）",
        path.display()
    );
    // PowerShell 5.1 は BOM の無い .ps1 を CP932 として読む。非 ASCII を混ぜると
    // 行が食われて**構文エラーも出さずに次の行が消える**（このセッションで実際に踏んだ）
    let non_ascii: Vec<(usize, &str)> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.is_ascii())
        .map(|(i, l)| (i + 1, l))
        .collect();
    assert!(
        non_ascii.is_empty(),
        "{} に非 ASCII 文字がある（PowerShell 5.1 が CP932 で読むと次の行ごと消える）: {:?}",
        path.display(),
        non_ascii
    );
}
