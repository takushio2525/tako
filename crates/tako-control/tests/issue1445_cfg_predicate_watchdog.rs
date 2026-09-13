//! **#1445 の番犬**: 合成 cfg のテストモジュールが本番コードとして走査に混ざらない。
//!
//! ## なぜ止めるのか
//!
//! #1420 で入れた走査範囲の 1 実装（`common/production_range.rs`）は、テスト領域の
//! 検出に**リテラルの `#[cfg(test)]`** しか使っていなかった。そのため
//! `#[cfg(all(test, unix))]` のように属性が合成されたテストモジュール
//! （`discovery.rs` / `ipc.rs` / `tmux_backend.rs` など 8 ファイル 11 か所）が潰されず、
//! **本番コードとして走査に混ざって**いた。番犬はテスト内の直書きを本番の違反として
//! 誤検出するか、逆にテスト内の文字列を「扱った証拠」として拾って緑になる
//! （#1417 が踏んだ型）。#1441 の番犬は自前で正規化して回避していた。
//!
//! ## 何を固定するか
//!
//! 1. [`合成cfgのテストモジュールは本番の走査に現れない`] / [`実ファイルへ注入しても本番の走査に現れない`]
//!    — 受け入れ条件 1。A/B の旧アーム（`CfgMode::LiteralOnly` = `TAKO_1445_LEGACY=1`）では
//!    **現れる**ことまで測る（測れないなら検出力を測ったつもりになる）
//! 2. [`notテストと素の原子は本番コードとして残る`] ほか — 潰しすぎない側。`#[cfg(not(test))]` は「テストでないとき」に
//!    載る**本番コード**なので残る。`#[cfg_attr(test, …)]` も対象外
//! 3. [`広げても下限を跨いだファイルは無い`] — 潰す領域が増えたことで走査範囲が
//!    `MIN_COVERAGE` を割ったファイルが出ていない（受け入れ条件 2 の機械版）
//! 4. [`自前のcfg正規化が戻っていない`] — 番犬ごとの正規化の写しが復活していない

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

// `code_view` は共有部品の中から借りる（別途 `#[path]` で宣言すると同じファイルが
// 2 回コンパイルされる = `clippy::duplicate_mod`）
use production_range::code_view;
use production_range::CfgMode;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// cfg 属性を組む。
///
/// **この番犬自身が `#[cfg(all(test…` を直書きしない**ための道具。直書きすると
/// [`自前のcfg正規化が戻っていない`] の対象から自分だけ外す必要が出てしまう
/// （許可リストを持つ番犬は、その 1 行ぶんだけ検出力が落ちる）
fn attr(pred: &str) -> String {
    format!("#[cfg({pred})]")
}

/// 新旧 2 アームの眺めを 1 回で作る（`(新, 旧)`）
fn arms(src: &str) -> (String, String) {
    (
        production_range::scan_with_mode(src, CfgMode::TestPredicate).text,
        production_range::scan_with_mode(src, CfgMode::LiteralOnly).text,
    )
}

/// テストモジュールの中にだけ置く目印（本番へ混ざったら一目で分かる綴り）
const NEEDLE: &str = "tako_1445_テスト側にしか無い綴り";

/// `pred` を述語に持つテストモジュール 1 つと、その前後の本番コード
fn fixture(pred: &str) -> String {
    format!(
        "pub fn 前() -> u32 {{\n    1\n}}\n\n{}\nmod tests {{\n    const X: &str = \"{NEEDLE}\";\n}}\n\npub fn 後() -> u32 {{\n    2\n}}\n",
        attr(pred)
    )
}

#[test]
fn 合成cfgのテストモジュールは本番の走査に現れない() {
    // 受け入れ条件 1。`all` は順序を入れ替えても・入れ子でも拾う
    for pred in [
        "all(test, unix)",
        "all(unix, test)",
        "all(test, target_os = \"macos\")",
        "any(test, feature = \"visual-test\")",
        "all(unix, any(test, feature = \"visual-test\"))",
    ] {
        let src = fixture(pred);
        let (new, old) = arms(&src);
        assert!(
            !new.contains(NEEDLE),
            "{pred}: テスト領域が本番の走査に混ざっている:\n{new}"
        );
        for keep in ["pub fn 前", "pub fn 後"] {
            assert!(new.contains(keep), "{pred}: {keep} が消えている:\n{new}");
        }
        // A/B の旧アーム: 同じ入力で**混ざる**ことを測る
        assert!(
            old.contains(NEEDLE),
            "{pred}: 旧アームで混ざらない = A/B が成立していない（検出力を測れない）"
        );
    }
}

#[test]
fn 複数行に跨る属性でも潰す() {
    // `rustfmt` が畳まない長さの述語は複数行になる
    let src = format!(
        "pub fn 前() {{}}\n\n#[cfg(all(\n    test,\n    unix\n))]\nmod tests {{\n    const X: &str = \"{NEEDLE}\";\n}}\n\npub fn 後() {{}}\n"
    );
    let (new, old) = arms(&src);
    assert!(!new.contains(NEEDLE), "複数行の属性を拾えていない:\n{new}");
    assert!(
        new.contains("pub fn 後"),
        "後ろの本番コードが消えた:\n{new}"
    );
    assert!(old.contains(NEEDLE), "旧アームで混ざらない = A/B 不成立");
}

#[test]
fn 実ファイルへ注入しても本番の走査に現れない() {
    // 受け入れ条件 1 の実ファイル版。`discovery.rs`（#1445 の発見元）と
    // `ipc.rs` の unix / windows の両アームで測る
    for (rel, marker, kept) in [
        (
            "crates/tako-control/src/discovery.rs",
            "mod tests {",
            "pub fn live_primary_pid(",
        ),
        (
            "crates/tako-control/src/ipc.rs",
            "mod tests {",
            "pub struct IpcServer {",
        ),
        (
            "crates/tako-control/src/ipc.rs",
            "mod windows_tests {",
            "pub struct IpcServer {",
        ),
    ] {
        let src = read(rel);
        let at = src
            .find(marker)
            .unwrap_or_else(|| panic!("{rel}: {marker} が見つからない（改名したら番犬も直す）"))
            + marker.len();
        let injected = format!(
            "{}\n    const 注入: &str = \"{NEEDLE}\";{}",
            &src[..at],
            &src[at..]
        );
        let (new, old) = arms(&injected);
        assert!(
            !new.contains(NEEDLE),
            "{rel} の {marker} に置いた文字列が本番の走査に現れる（#1445）"
        );
        assert!(
            old.contains(NEEDLE),
            "{rel} の {marker}: 旧アームで現れない = A/B が成立していない"
        );
        // 本番側は残る（潰しすぎていない）
        assert!(
            new.contains(kept),
            "{rel}: 本番コード（{kept}）まで潰している"
        );
    }
}

#[test]
fn 実ファイルの合成cfgが素で潰れている() {
    // 注入ではなく**実物**で確認する。`discovery.rs` のテストモジュールにしか
    // 無い綴りが、素の走査結果から消えていること
    let src = read("crates/tako-control/src/discovery.rs");
    let needle = "tako-discovery-test-";
    assert!(src.contains(needle), "目印が消えた（改名したら番犬も直す）");
    let (new, old) = arms(&src);
    assert!(
        !new.contains(needle),
        "合成 cfg（{}）のテストモジュールが潰れていない（#1445）",
        attr("all(test, unix)")
    );
    assert!(old.contains(needle), "旧アームで潰れている = A/B 不成立");
}

#[test]
fn notテストと素の原子は本番コードとして残る() {
    // 潰しすぎない側。`#[cfg(not(test))]` は「テストでないとき」に載る**本番コード**で、
    // 潰すと本番が番犬の視界から消える（`orchestrator/mod.rs` などに実在する）
    for pred in [
        "not(test)",
        "all(not(test), unix)",
        "unix",
        "feature = \"visual-test\"",
        "not(feature = \"visual-test\")",
    ] {
        let src = format!(
            "{}\npub fn 本番() -> &'static str {{\n    \"{NEEDLE}\"\n}}\n",
            attr(pred)
        );
        let scanned = production_range::scan_with_mode(&src, CfgMode::TestPredicate);
        assert!(
            scanned.regions.is_empty() && scanned.text.contains(NEEDLE),
            "{pred}: 本番コードを潰している（潰した領域 {:?}）",
            scanned.regions
        );
    }
}

#[test]
fn featureの文字列にtestが入っていても潰さない() {
    // `code_view` が文字列を空白へ潰しているので、述語の中の `"test-util"` は
    // 原子として見えない（見えると本番コードを丸ごと落とす）
    let src = format!(
        "#[cfg(feature = \"test-util\")]\npub fn 本番() -> &'static str {{\n    \"{NEEDLE}\"\n}}\n"
    );
    let scanned = production_range::scan_with_mode(&src, CfgMode::TestPredicate);
    assert!(
        scanned.regions.is_empty(),
        "`feature = \"test-util\"` を test と読んでいる（{:?}）",
        scanned.regions
    );
}

#[test]
fn cfgattrは対象外() {
    // `cfg_attr` は item 自体を本番ビルドにも載せ、**付ける属性だけ**を切り替える。
    // 潰すと本番コードが消えるので入口の綴り（`#[cfg(`）から外れている
    let src = format!(
        "#[cfg_attr(test, allow(dead_code))]\npub fn 本番() -> &'static str {{\n    \"{NEEDLE}\"\n}}\n"
    );
    let scanned = production_range::scan_with_mode(&src, CfgMode::TestPredicate);
    assert!(
        scanned.regions.is_empty() && scanned.text.contains(NEEDLE),
        "`cfg_attr` の item を潰している（{:?}）",
        scanned.regions
    );
}

#[test]
fn 同じ行に属性が複数並んでもitemちょうどで潰す() {
    let src = format!(
        "pub fn 前() {{}}\n{} #[allow(dead_code)] mod tests {{ const X: &str = \"{NEEDLE}\"; }}\npub fn 後() {{}}\n",
        attr("all(test, unix)")
    );
    let (new, _) = arms(&src);
    assert!(!new.contains(NEEDLE), "潰しきれていない:\n{new}");
    assert!(
        new.contains("pub fn 前") && new.contains("pub fn 後"),
        "item の外まで潰している:\n{new}"
    );
}

#[test]
fn 入れ子のモジュールの中でも潰す() {
    let src = format!(
        "pub mod outer {{\n    pub fn 本番() {{}}\n\n    {}\n    mod tests {{\n        const X: &str = \"{NEEDLE}\";\n    }}\n\n    pub fn 後() {{}}\n}}\n",
        attr("all(test, unix)")
    );
    let (new, old) = arms(&src);
    assert!(!new.contains(NEEDLE), "入れ子を拾えていない:\n{new}");
    for keep in ["pub fn 本番", "pub fn 後"] {
        assert!(new.contains(keep), "{keep} が消えている:\n{new}");
    }
    assert!(old.contains(NEEDLE), "旧アームで混ざらない = A/B 不成立");
}

#[test]
fn 裏返しも同じ境界を使う() {
    let src = fixture("all(test, unix)");
    let tests = production_range::tests_only_with_mode(&src, CfgMode::TestPredicate);
    assert!(tests.contains(NEEDLE), "テスト側が残っていない:\n{tests}");
    assert!(
        !tests.contains("pub fn 前") && !tests.contains("pub fn 後"),
        "本番側が残っている:\n{tests}"
    );
    assert_eq!(tests.len(), src.len(), "行番号がずれる");
}

// --- 実ファイル全部で測る（受け入れ条件 2 の機械版）-----------------------------

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn src_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        rust_files(&root.join("crates").join(crate_dir).join("src"), &mut files);
    }
    files.sort();
    assert!(
        files.len() > 100,
        "走査先が間違っている（{} 本）",
        files.len()
    );
    files
}

#[test]
fn 広げても下限を跨いだファイルは無い() {
    // 受け入れ条件 2: 潰す領域が増えたぶんで走査範囲が下限を割ったファイルが無い。
    // **同じファイルの新旧を比べる**ので、もともとテストの厚いファイル
    // （`tmux_backend.rs` の 22.5% 等）では落ちない = #1445 が原因のときだけ落ちる
    let root = repo_root();
    let mut crossed = Vec::new();
    let mut grew = Vec::new();
    for path in src_files() {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        let new = production_range::scan_with_mode(&src, CfgMode::TestPredicate);
        let old = production_range::scan_with_mode(&src, CfgMode::LiteralOnly);
        // 広げた側が**狭くなることはない**（潰す領域は増える一方）
        if new.kept_bytes() > old.kept_bytes() {
            grew.push(format!(
                "{rel}: 本番の残りが増えた（{} → {} バイト）",
                old.kept_bytes(),
                new.kept_bytes()
            ));
        }
        if old.coverage() >= production_range::MIN_COVERAGE
            && new.coverage() < production_range::MIN_COVERAGE
        {
            crossed.push(format!(
                "{rel}: {:.1}% → {:.1}%（下限 {:.1}%）",
                old.coverage() * 100.0,
                new.coverage() * 100.0,
                production_range::MIN_COVERAGE * 100.0
            ));
        }
    }
    assert!(
        grew.is_empty(),
        "cfg 述語を読む側が本番コードを取りこぼしている:\n  {}",
        grew.join("\n  ")
    );
    assert!(
        crossed.is_empty(),
        "#1445 で潰す領域が増えたぶんだけで走査範囲が下限を割ったファイルがある:\n  {}\n\
         → その番犬は `production_with_floor` で下限を下げるか、読む先を絞ってください",
        crossed.join("\n  ")
    );
}

// --- 番犬の番犬 -----------------------------------------------------------------

#[test]
fn abの逃げ道は共有部品が読む() {
    // A/B（`TAKO_1445_LEGACY=1`）が黙って死ぬと、旧アームで測っているつもりの
    // 検査がすべて新アームの繰り返しになる
    let src = read("crates/tako-control/tests/common/production_range.rs");
    let code = code_view::without_comment_lines(&src);
    for needle in [
        "TAKO_1445_LEGACY",
        "CfgMode::LiteralOnly",
        "CfgMode::TestPredicate",
    ] {
        assert!(code.contains(needle), "共有部品から {needle} が消えている");
    }
    for (func, at) in [
        ("pub fn scan(src: &str) -> Production {", "scan"),
        ("pub fn tests_only(src: &str) -> String {", "tests_only"),
    ] {
        let body = code
            .split_once(func)
            .unwrap_or_else(|| panic!("{at} の定義が見つからない（改名したら番犬も直す）"))
            .1;
        let head: String = body.chars().take(200).collect();
        assert!(
            head.contains("cfg_mode()"),
            "{at} が既定の読み方（cfg_mode）を通っていない = env で切り替わらない"
        );
    }
}

/// 番犬ごとに cfg 属性を書き換えてから共有部品へ食わせる形（#1441 が一時的に持っていた
/// 写し）が戻っていないこと。**この番犬自身も対象**にするため、fixture の属性は
/// [`attr`] で組み立てて直書きしない
#[test]
fn 自前のcfg正規化が戻っていない() {
    let root = repo_root();
    let mut files = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        rust_files(
            &root.join("crates").join(crate_dir).join("tests"),
            &mut files,
        );
    }
    files.sort();
    assert!(
        files.len() > 50,
        "走査先が間違っている（{} 本）",
        files.len()
    );

    // needle は**実行時に組む**（直書きするとこの番犬自身が引っかかり、
    // 自分だけ許可リストへ逃がすことになる = その 1 行ぶん検出力が落ちる）
    let head = format!("#[{}(", "cfg");
    let needles = [
        format!("{head}all(test"),
        format!("{head}any(test"),
        format!("normalize_{}_attrs", "test"),
    ];
    let mut offenders = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        // 行番号は**原文**で数える（説明文で落ちないようコメント行だけ飛ばす）
        for (idx, line) in src.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for needle in &needles {
                if line.contains(needle.as_str()) {
                    offenders.push(format!("{rel}:{} `{needle}`", idx + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "番犬が自前で cfg 属性を正規化している:\n  {}\n\
         → `common/production_range.rs` が cfg 述語を読むので不要です（#1445）",
        offenders.join("\n  ")
    );
}

#[test]
fn 寄せ先を通り続けている() {
    // 受け入れ条件 3: #1441 の番犬が 1 実装を素の入力で呼ぶだけになっている
    let src = read("crates/tako-control/tests/issue1441_ipc_socket_watchdog.rs");
    let code = code_view::without_comment_lines(&src);
    assert!(
        code.contains("#[path = \"common/production_range.rs\"]"),
        "1 実装を通らなくなっている"
    );
    assert!(
        code.contains("production_range::scan(&src).text"),
        "素の入力で呼んでいない（自前の前処理が復活した可能性）"
    );
}
