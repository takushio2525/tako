//! Code Runner の実行環境の表が「言語を足す = 行を足すだけ」の形から戻らないための番犬（Issue #1729）。
//!
//! ## 何を止めるか
//!
//! 1. **表の外に言語・道具の名前を書く形**（`if kind.id == "python"` / `dir.join("pyvenv.cfg")`）。
//!    知識が表の外へ漏れると、言語を足すたびに戦略の実装まで直すことになり、
//!    表を読んでも「何を見ているか」が分からなくなる。禁止する語は**表そのものから集める**ので、
//!    行を足せば見張る語も自動で増える（写しの一覧を持たない）
//! 2. **属性の OS 分岐**（`#[cfg(windows)]`）。macOS の単体から Windows の列を検査できなくなる
//!    （#1655 / #1616 の作法。列を選ぶのは `Platform` 引数）
//! 3. **検出の中で実行中の OS を見る形**（`Platform::current()`）。列を選ぶのは呼び出し側の仕事
//! 4. **検出の中で子プロセスを起こす形**。検出は stat と先頭読みだけ（設計書の Tier F）で、
//!    `Run` の経路から同期で呼ばれる。子プロセスでしか分からないものは `needs_probe` で返し、
//!    確かめるのは tako-control の Tier P（`probe::output_with_timeout`）
//!
//! ## 眺めの選び方
//!
//! - 規則 1 は `without_comments`（文字列は囲みごと残る）を `production`（テスト領域を潰す）に
//!   重ねる。止めたいのはコードの中の文字列で、説明文に名前が出るのは違反ではない
//! - 規則 2〜4 は `code_view`（コメントも文字列も潰す）。止めたいのはコンパイルされる形

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};
use production_range::production;
use std::path::{Path, PathBuf};
use tako_core::runtime_env::{Detector, Marker, KINDS};

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

/// 表の外で、言語・道具の名前を持ってはいけない本番コード
const OUTSIDE_TABLE: &[&str] = &[
    "crates/tako-core/src/runtime_env/mod.rs",
    "crates/tako-core/src/runtime_env/detect.rs",
    "crates/tako-core/src/runner_config.rs",
];
/// 表そのもの（規則 2〜4 はここも見る）
const TABLE: &str = "crates/tako-core/src/runtime_env/kinds.rs";

const BANNED_ATTRS: &[&str] = &[
    "#[cfg(windows)]",
    "#[cfg(unix)]",
    "#[cfg(target_os",
    "#[cfg(not(windows))]",
    "#[cfg(not(unix))]",
    "#[cfg_attr(windows",
    "#[cfg_attr(unix",
];

const BANNED_CALLS: &[(&str, &str)] = &[
    (
        "Platform::current()",
        "列を選ぶのは呼び出し側（`Platform` 引数で受ける）",
    ),
    (
        "Command::new",
        "検出は stat と先頭読みだけ（子プロセスは Tier P = tako-control）",
    ),
    (
        "process::Command",
        "検出は stat と先頭読みだけ（子プロセスは Tier P = tako-control）",
    ),
];

fn 行番号(src: &str, byte: usize) -> usize {
    src[..byte].lines().count().max(1)
}

/// 表が持つ**言語・道具の名前**（戦略の汎用な語 = `bin` / `run` / `name` は含めない）
fn 表の語彙() -> Vec<&'static str> {
    let mut words: Vec<&'static str> = Vec::new();
    for kind in KINDS {
        words.extend([kind.id, kind.variable, kind.wrapped_program]);
        words.extend([kind.fallback.macos, kind.fallback.windows]);
        words.extend(kind.extensions.iter().copied());
        for det in kind.detectors {
            words.push(det.id());
            match det {
                Detector::Wrapper(s) => {
                    words.extend([s.label, s.program.macos, s.program.windows]);
                    for m in s.markers {
                        match m {
                            Marker::File(f) => words.push(f),
                            Marker::TomlTable { file, table } => words.extend([*file, *table]),
                        }
                    }
                    if let Some(e) = &s.in_project_env {
                        words.extend(e.names.iter().copied());
                        words.push(e.marker);
                        words.extend(e.layout.env.iter().map(|(v, _)| *v));
                    }
                }
                Detector::MarkerDir(s) => {
                    words.extend(s.names.iter().copied());
                    words.push(s.marker);
                    words.extend(s.layout.env.iter().map(|(v, _)| *v));
                }
                Detector::NamedEnvList(s) => {
                    words.push(s.label_prefix.trim_end_matches([':', ' ']));
                    words.extend(s.project_files.iter().copied());
                    words.extend(s.layout.env.iter().map(|(v, _)| *v));
                }
                Detector::VersionFile(s) => {
                    words.push(s.label_prefix.trim());
                    words.extend(s.files.iter().copied());
                    words.extend(s.root_env.iter().copied());
                }
                Detector::PathLookup(s) => {
                    words.extend(s.file_names.macos.iter().copied());
                    words.extend(s.file_names.windows.iter().copied());
                }
                Detector::EnvSwitch(s) => {
                    words.extend(s.files.iter().copied());
                    words.push(s.var);
                }
            }
        }
    }
    words.retain(|w| !w.is_empty());
    words.sort_unstable();
    words.dedup();
    words
}

/// 規則 1: 表の語彙が、表の外の本番コードに**文字列リテラル**として現れない
fn 表の外の名前を探す(src: &str, rel: &str, words: &[&str]) -> Result<(), String> {
    let view = without_comments_checked(&production(src, rel), rel);
    for word in words {
        let literal = format!("\"{word}\"");
        if let Some(pos) = view.find(&literal) {
            return Err(format!(
                "{rel}:{} に表の語 {literal} が直書きされている。言語・道具の知識は \
                 `runtime_env::kinds::KINDS` の 1 枚だけが持つ（戦略は表の値から組む = Issue #1729）",
                行番号(src, pos)
            ));
        }
    }
    Ok(())
}

/// 規則 2〜4: 属性の OS 分岐・実行中の OS・子プロセス
fn 禁じた形を探す(src: &str, rel: &str) -> Result<(), String> {
    let view = code_view(src);
    if !view.contains("fn ") && !view.contains("const ") {
        return Err(format!("{rel}: 眺めが空振りしている（走査先の取り違え）"));
    }
    for banned in BANNED_ATTRS {
        if let Some(pos) = view.find(banned) {
            return Err(format!(
                "{rel}:{} に `{banned}` がある。OS の差は `PerOs` の 2 列 + `Platform` 引数で書く \
                 （属性で分けると macOS の単体から Windows の列を検査できない = #1655 / #1729）",
                行番号(src, pos)
            ));
        }
    }
    let prod = code_view(&production(src, rel));
    for (banned, why) in BANNED_CALLS {
        if let Some(pos) = prod.find(banned) {
            return Err(format!(
                "{rel}:{} に `{banned}` がある。{why}（Issue #1729）",
                行番号(src, pos)
            ));
        }
    }
    Ok(())
}

#[test]
fn 表の語彙は見張るのに十分な数がある() {
    let words = 表の語彙();
    // 空振り（表の読み方が壊れて語が集まらない）なら番犬は常に緑になる
    assert!(words.len() >= 20, "表の語彙が少なすぎる: {words:?}");
    for kind in KINDS {
        assert!(words.contains(&kind.id), "{} が語彙に無い", kind.id);
        for det in kind.detectors {
            assert!(words.contains(&det.id()), "{} が語彙に無い", det.id());
        }
    }
}

#[test]
fn 表の外に言語と道具の名前を書かない() {
    let words = 表の語彙();
    for rel in OUTSIDE_TABLE {
        表の外の名前を探す(&read(rel), rel, &words).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn 表と戦略に属性のos分岐と実行中のosと子プロセスが無い() {
    for rel in OUTSIDE_TABLE.iter().chain(std::iter::once(&TABLE)) {
        禁じた形を探す(&read(rel), rel).unwrap_or_else(|e| panic!("{e}"));
    }
}

// --- 番犬そのものが効いていることの自己検査（実ファイルへの注入 A/B）---

const DETECT: &str = "crates/tako-core/src/runtime_env/detect.rs";

fn detect_with(extra: &str) -> String {
    let src = read(DETECT);
    let anchor = "pub fn detect(";
    assert!(src.contains(anchor), "注入の目印が変わった: {DETECT}");
    src.replacen(anchor, &format!("{extra}\n{anchor}"), 1)
}

#[test]
fn 注入した表の語をfile_lineで名指しする() {
    let words = 表の語彙();
    // 1. 印のファイル名を戦略へ直書きする
    let injected = detect_with("fn legacy() -> &'static str { \"pyvenv.cfg\" }");
    let err = 表の外の名前を探す(&injected, DETECT, &words).expect_err("直書きを検出できていない");
    assert!(
        err.contains(&format!("{DETECT}:")),
        "名指ししていない: {err}"
    );
    assert!(err.contains("\"pyvenv.cfg\""), "{err}");

    // 2. kind の id で分岐する
    let injected = detect_with("fn legacy(k: &str) -> bool { k == \"python\" }");
    assert!(表の外の名前を探す(&injected, DETECT, &words).is_err());

    // 3. 説明文に名前が出るだけでは落ちない
    let commented = detect_with("// \"pyvenv.cfg\" のような名前は表に書く");
    表の外の名前を探す(&commented, DETECT, &words)
        .expect("コメントの中の綴りを違反として拾っている");

    // 4. テスト領域の期待値は違反ではない
    let in_tests = detect_with("#[cfg(test)]\nmod legacy { const X: &str = \"pyvenv.cfg\"; }");
    表の外の名前を探す(&in_tests, DETECT, &words).expect("テスト領域を本番として拾っている");

    // 5. 注入前の実物は緑
    表の外の名前を探す(&read(DETECT), DETECT, &words).expect("本番領域が汚れている");
}

#[test]
fn 注入した属性分岐と子プロセスをfile_lineで名指しする() {
    for (extra, needle) in [
        ("#[cfg(windows)]\nfn legacy() {}", "#[cfg(windows)]"),
        (
            "fn legacy() { let _ = std::process::Command::new(\"x\"); }",
            "Command::new",
        ),
        (
            "fn legacy() -> Platform { Platform::current() }",
            "Platform::current()",
        ),
    ] {
        let err = 禁じた形を探す(&detect_with(extra), DETECT).expect_err(needle);
        assert!(err.contains(needle), "{err}");
        assert!(
            err.contains(&format!("{DETECT}:")),
            "名指ししていない: {err}"
        );
    }
    // `cfg!` のマクロ形は違反ではない（列を選ぶ正しい形の中身）
    禁じた形を探す(
        &detect_with("fn legacy() -> bool { cfg!(windows) }"),
        DETECT,
    )
    .expect("`cfg!` を違反として拾っている");
}
