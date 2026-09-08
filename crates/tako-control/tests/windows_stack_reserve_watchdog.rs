//! 番犬: Windows のスタック予約宣言（`/stack:`）が落ちていない（#1133）
//!
//! tako の Windows 版が動くのは、実行ファイルへ「メインスレッドに 8 MiB 予約する」と
//! 宣言してあるからで、その宣言は **`tako-app` / `tako-cli` の `build.rs` が出す
//! `cargo:rustc-link-arg=/stack:…` の 1 行**にしか現れない。だから
//!
//! - build.rs を書き換える / リソース埋め込みの整理でこの行を落とす
//! - 数値だけ小さくする
//! - `tako_core::platform::stack` 側の要求量だけ上げて写しを直し忘れる
//!
//! のどれでも**無言で**落ちる。落ちた結果は「Windows の debug ビルドが
//! `thread 'main' has overflowed its stack` で**プロセスごと死ぬ**」で、
//! セルフテストの判定行も `FAILED` も出ないので原因が分からない
//! （#1133 は項目 80 で 2/2 これになった）。そして **macOS では原理的に再現しない**
//! （既定 8 MiB）ので、CI の macOS ジョブでは気づけない。
//!
//! そこで `dpi`（#1063）と同じ二段構えにする:
//!
//! 1. **ここ（macOS でも走る）**: 宣言が両クレートに在り、数値が
//!    `tako_core::platform::stack::REQUIRED_MAIN_STACK_BYTES` と一致すること
//! 2. **実プロセス（Windows）**: セルフテストが起動直後に実測し、足りなければ
//!    項目 80 まで行かずに `FAILED`（`shortfall_note` の文言）を出す
//!
//! 1 だけでは「リンクに効いたか」までは見られず、2 だけでは Windows 実機を
//! 回すまで気づけない。両方あって初めて「宣言を落とした」と「効いていない」を
//! 早い段階で捕まえられる。
//!
//! 実測（どの関数が何 KiB 使っているか / 7 commit で +12 KiB しか伸びていないこと）は
//! `tako_core::platform::stack` のモジュールコメントと #1133 を参照。

use std::path::{Path, PathBuf};
use tako_core::platform::stack;

/// 宣言を置く 2 クレート。build.rs はクレートをまたいで共有できないので写しが 2 本ある
const CRATES: [&str; 2] = ["tako-app", "tako-cli"];

fn workspace_root() -> PathBuf {
    // crates/tako-control -> crates -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("ワークスペースルートを辿れない")
        .to_path_buf()
}

fn build_script(krate: &str) -> (PathBuf, String) {
    let path = workspace_root().join("crates").join(krate).join("build.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    (path, src)
}

/// `/stack:<数値>` の数値を取り出す
fn declared_reserve(src: &str) -> Option<u64> {
    let rest = src.split("/stack:").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[test]
fn 両クレートがスタック予約を宣言している() {
    for krate in CRATES {
        let (path, src) = build_script(krate);
        let declared = declared_reserve(&src).unwrap_or_else(|| {
            panic!(
                "{} が `cargo:rustc-link-arg=/stack:…` を出していない。\
                 Windows/MSVC は既定 1 MiB なので、GPUI の描画フレーム（実測 135〜564KiB）を\
                 積んだ時点でプロセスごと落ちる（#1133）。宣言は\
                 tako_core::platform::stack::windows_link_arg() = `{}`",
                path.display(),
                stack::windows_link_arg()
            )
        });
        assert_eq!(
            declared,
            stack::REQUIRED_MAIN_STACK_BYTES,
            "{} の宣言 {declared} が tako_core::platform::stack::REQUIRED_MAIN_STACK_BYTES \
             ({}) と違う。数値を変えるときは両方の build.rs と core を対で直すこと（#1133）",
            path.display(),
            stack::REQUIRED_MAIN_STACK_BYTES
        );
        assert!(
            src.contains(&format!(
                "cargo:rustc-link-arg={}",
                stack::windows_link_arg()
            )),
            "{} の宣言が `cargo:rustc-link-arg={}` の形になっていない（#1133）",
            path.display(),
            stack::windows_link_arg()
        );
    }
}

#[test]
fn 宣言はmsvcターゲットだけに限っている() {
    // gnu ツールチェーンのリンカ引数は書式が違う（`-Wl,--stack`）。
    // 無条件に出すと mingw ビルドがリンクエラーで落ちる
    for krate in CRATES {
        let (path, src) = build_script(krate);
        let head = src
            .split("/stack:")
            .next()
            .expect("split は必ず 1 要素返す");
        assert!(
            head.contains("CARGO_CFG_TARGET_ENV") && head.contains("msvc"),
            "{} が `/stack:` を msvc 限定にしていない（#1133）。\
             gnu ツールチェーンは書式が違うのでリンクが落ちる",
            path.display()
        );
        assert!(
            head.contains("CARGO_CFG_TARGET_OS"),
            "{} が Windows ターゲット限定の判定より後ろで宣言していない（#1133）。\
             ホストの cfg! ではなくターゲットの OS を見ること",
            path.display()
        );
    }
}

#[test]
fn セルフテストが起動直後に実測する側を持っている() {
    // 二段構えの 2 段目。ここが消えると、宣言が効かないビルドで
    // 「項目 80 で沈黙して死ぬ」状態（#1133 の症状そのもの）へ戻る
    let path = workspace_root().join("crates/tako-app/src/main.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    assert!(
        src.contains("platform::stack::current_thread_reserve")
            && src.contains("platform::stack::shortfall_note"),
        "{} が起動直後にスタック予約量を実測していない（#1133）。\
         宣言が効いていないビルドは判定行も出さずに死ぬので、\
         セルフテストは最初に自分の前提を測ること",
        path.display()
    );
}
