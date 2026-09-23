//! Code Runner の既定表が「OS で出し分けられる形」から戻らないための番犬（Issue #1655）。
//!
//! ## 何を止めるか
//!
//! 1. **`runner.rs` と既定表へ `#[cfg(windows)]` / `#[cfg(unix)]` を撒く形**。
//!    属性で出し分けると、macOS のビルドに Windows の表が**存在しない**ので、
//!    「Windows で `.py` が何に解決されるか」を macOS の単体から 1 マスも検査できない。
//!    正しい形は `Platform` を引数で受ける純粋関数 + `cfg!`（#1616 の作法）で、
//!    実行中の OS を見るのは `Platform::current()` の中の 1 行だけ。
//! 2. **既定表の直書きが `runner.rs` へ戻ること**。表が 2 か所に住むと、片方だけに
//!    拡張子が足されて「macOS では走るのに Windows では既定が無い」へ逆戻りする。
//!
//! ## 表の中身そのものは tako-core の単体が見る
//!
//! 「Windows 側に `./` や `python3` が混ざっていないか」は**データを見るほう**が強い
//! （表の書き方を変えても検出力が落ちない）ので、
//! `tako_core::platform::runner_defaults` の単体
//! （`windowsの既定にposixの形が混ざっていない` ほか）が持つ。ここが見るのは**配線**だけ。
//!
//! ## 眺めの選び方
//!
//! - **属性の不在**（規則 1）は `code_view`（コメントも文字列も潰す）。止めたいのは
//!   コンパイルされる属性なので、説明文の中に綴りが現れるのは違反ではない
//!   （このファイルの doc がまさにそう）。#1609 の「不在検査は全文」の**例外**にあたる
//! - **テンプレートの不在**（規則 2）は `without_comments`（文字列は囲みごと残る）を
//!   `production`（テスト領域を潰す）に重ねる。表の中身は文字列リテラルなので
//!   `code_view` では見えず、テスト領域には期待値として正しく存在する
//! - **配線の存在**（肯定の存在確認）は `without_comments`（#1609 の寄せ先）

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};
use production_range::production;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読める: {e}"))
}

const RUNNER: &str = "crates/tako-core/src/runner.rs";
const TABLE: &str = "crates/tako-core/src/platform/runner_defaults.rs";

/// 属性としての OS 分岐（`cfg!(` のマクロ形は**対象外** = そちらが正しい形）
const BANNED_ATTRS: &[&str] = &[
    "#[cfg(windows)]",
    "#[cfg(unix)]",
    "#[cfg(target_os",
    "#[cfg(not(windows))]",
    "#[cfg(not(unix))]",
    "#[cfg_attr(windows",
    "#[cfg_attr(unix",
];

fn 行番号(src: &str, byte: usize) -> usize {
    src[..byte].lines().count().max(1)
}

/// 規則 1: 属性の OS 分岐が無い
fn 属性分岐を探す(src: &str, rel: &str) -> Result<(), String> {
    let view = code_view(src);
    if !view.contains("fn ") {
        return Err(format!("{rel}: 眺めが空振りしている（走査先の取り違え）"));
    }
    for banned in BANNED_ATTRS {
        if let Some(pos) = view.find(banned) {
            return Err(format!(
                "{rel}:{} に `{banned}` がある。OS 分岐は属性ではなく `Platform` 引数 + `cfg!` \
                 で書く（属性で分けると macOS の単体から Windows の表を検査できなくなる \
                 = Issue #1655 / #1616）",
                行番号(src, pos)
            ));
        }
    }
    Ok(())
}

/// 規則 2: コマンドテンプレートの直書きが `runner.rs` の本番コードへ戻っていない
fn 直書きを探す(src: &str, rel: &str) -> Result<(), String> {
    let view = without_comments_checked(&production(src, rel), rel);
    if let Some(pos) = view.find("${fileBase}") {
        return Err(format!(
            "{rel}:{} にコマンドテンプレートが直書きされている。既定は \
             `platform::runner_defaults::TABLE` の 1 枚に置く（Issue #1655）",
            行番号(src, pos)
        ));
    }
    Ok(())
}

#[test]
fn 既定表とその入口にosの属性分岐が無い() {
    for rel in [RUNNER, TABLE] {
        属性分岐を探す(&read(rel), rel).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn 既定表はplatform境界の1枚だけ() {
    直書きを探す(&read(RUNNER), RUNNER).unwrap_or_else(|e| panic!("{e}"));
    let view = without_comments_checked(&read(RUNNER), RUNNER);
    assert!(
        view.contains("platform::runner_defaults::builtin_defaults"),
        "runner.rs が境界の表を引いていない（表を直書きへ戻した？ = Issue #1655）"
    );
}

/// 表側は「OS 列を引く関数」を持ち、実行中の OS を見るのは入口の 1 行だけ
#[test]
fn 表はplatformを引数で受ける() {
    let table = without_comments_checked(&read(TABLE), TABLE);
    assert!(
        table.contains("pub fn builtin_defaults(platform: Platform)"),
        "表が OS を引数で受けていない（macOS から Windows の表を引けなくなる = #1655）"
    );
    assert!(
        !code_view(&read(TABLE)).contains("Platform::current()"),
        "表の中で実行中の OS を見ている（列を選ぶのは呼び出し側の仕事 = #1655）"
    );
    assert!(
        without_comments_checked(&read(RUNNER), RUNNER).contains("Platform::current()"),
        "入口が実行中の OS を解決していない（#1655）"
    );
}

/// CLI / MCP へ出る一覧が**境界の表を通って**いる（開発不変条件: UI でできることは
/// AI からもできる）。`dispatch` が自前の表を持ち始めたら、拡張子を足しても
/// `tako run-default` の一覧に出ない形へ戻る
#[test]
fn run_defaultの一覧は境界の表から出ている() {
    let rel = "crates/tako-control/src/dispatch.rs";
    let view = without_comments_checked(&production(&read(rel), rel), rel);
    for needed in [
        "tako_core::merged_defaults(&settings.runner_defaults)",
        "tako_core::builtin_defaults()",
    ] {
        assert!(
            view.contains(needed),
            "{rel}: `{needed}` を通っていない（一覧が境界の表から離れた = Issue #1655）"
        );
    }
}

// --- 番犬そのものが効いていることの自己検査（実ファイルへの注入 A/B）---

#[test]
fn 注入した違反をfile_lineで名指しする() {
    // 1. 属性の cfg を表へ撒く
    let table = read(TABLE);
    let anchor = "pub fn builtin_defaults(platform: Platform)";
    let injected = table.replace(anchor, &format!("#[cfg(windows)]\n{anchor}"));
    let err = 属性分岐を探す(&injected, TABLE).expect_err("属性の注入を検出できていない");
    assert!(err.contains("#[cfg(windows)]"), "{err}");
    assert!(
        err.contains(&format!("{TABLE}:")),
        "名指ししていない: {err}"
    );

    // 2. `#[cfg(unix)]` も同じく落ちる
    let injected = table.replace(anchor, &format!("#[cfg(unix)]\n{anchor}"));
    assert!(属性分岐を探す(&injected, TABLE).is_err());

    // 3. 同じ綴りがコメントにあるだけでは落ちない（このファイルの doc がその形）
    let commented = table.replace(
        anchor,
        &format!("// #[cfg(windows)] へは戻さない\n{anchor}"),
    );
    属性分岐を探す(&commented, TABLE).expect("コメントの中の綴りを違反として拾っている");

    // 4. `cfg!` のマクロ形は正しい形なので落ちない
    let macro_form = table.replace(
        anchor,
        &format!("fn dummy() -> bool {{ cfg!(windows) }}\n{anchor}"),
    );
    属性分岐を探す(&macro_form, TABLE).expect("`cfg!` を違反として拾っている");

    // 5. テンプレートの直書きを runner.rs の本番領域へ戻す
    let runner = read(RUNNER);
    let anchor = "pub fn builtin_defaults() -> Vec<(&'static str, &'static str)> {";
    assert!(runner.contains(anchor), "入口の綴りが変わった: {RUNNER}");
    let injected = runner.replace(
        anchor,
        &format!("{anchor}\n    let _legacy = [(\"py\", \"python3 ${{fileBase}}\")];"),
    );
    let err = 直書きを探す(&injected, RUNNER).expect_err("直書きを検出できていない");
    assert!(
        err.contains(&format!("{RUNNER}:")),
        "名指ししていない: {err}"
    );

    // 6. 同じ綴りがテスト領域（期待値の表）にあるのは違反ではない
    直書きを探す(&runner, RUNNER).expect("本番領域が汚れている");
    assert!(
        runner.contains("${fileBase}"),
        "テスト領域に期待値が無い = 5 の A/B が意味を失っている"
    );
}
