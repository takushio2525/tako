//! 検証プロセス（テスト・`TAKO_ISOLATED`・セルフテスト）が本物のユーザー環境変数 Path（利用者全体の
//! レジストリ `HKCU\Environment\Path`）へ書かないことの番犬（#2959）
//!
//! # なぜ要るか
//!
//! Windows の実機の確認（11/16・#2931 のバッチ C）で、HOME / data dir を砂場へ向けた検証の
//! `tako setup`（CLI と隔離 GUI の「セットアップを実行」）が、PATH の段で本物のレジストリへ
//! `<砂場の HOME>\.local\bin` を書き足した。レジストリはアカウント全体のもので、HOME を向け直しても
//! 逃がせない（unix の同じ段は HOME 配下の profile へ書くので砂場に閉じる）。
//! 門は境界 B23 の `user_path::skip_for_verification`（判定は `paths::is_verification_process` の 1 実装）。
//!
//! # 何を縛るか
//!
//! 1. 境界の書き込み `write_typed` は、`imp::write` より前に門を見る（門を通り忘れた呼び手の最後の砦）。
//!    `write` は `write_typed` を通り、`imp::write` を呼ぶのは `write_typed` だけ
//! 2. 門の判定は `paths::is_verification_process` の 1 実装（新しい判定を増やさない）
//! 3. 本物のレジストリへ書く口（`user_path::write(` / `user_path::write_typed(` / 本物の口
//!    `user_path::Registry` / `tako setup undo` の `RegistryStore`）は既知の 4 か所にしか無い。
//!    新しい書き手は file:line で名指す（門を通してから、ここへ足す）
//! 4. 既知の 4 か所はそれぞれ門の後ろにいる:
//!    - setup の足す段 `ensure_user_path_dirs` / 外す段 `undo_user_path` は、本物の口を渡す**同じ呼び出しで**
//!      門の答えを渡し、中身（`*_with`）は門が立ったら読む前に返す
//!    - `tako setup undo` の `from_process` は、門を見てから、検証プロセスでないときだけ `RegistryStore` を作る
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので [`走査が空振りしていない`] で窓を固定し、
//! [`修正前の形へ戻すと名指しする`] で**修正前を再現した注入**が file:line で名指しされることを確かめる

use std::collections::BTreeMap;
use std::path::Path;

#[path = "common/production_range.rs"]
mod production_range;

const USER_PATH: &str = "crates/tako-core/src/platform/user_path.rs";
const BOOTSTRAP: &str = "crates/tako-control/src/setup_bootstrap.rs";
const UNDO: &str = "crates/tako-control/src/setup_undo.rs";

/// 門の呼び出し（呼び手はこの形で書く）
const GATE: &str = "user_path::skip_for_verification()";

/// 本物のレジストリへ書く口の印
const WRITERS: &[&str] = &[
    "user_path::write(",
    "user_path::write_typed(",
    "user_path::Registry",
    "Box::new(RegistryStore)",
];

/// 既知の書き手（ファイル・窓の頭・印 = [`WRITERS`] のどれか）
const KNOWN: &[(&str, &str, &str)] = &[
    (
        BOOTSTRAP,
        "fn ensure_user_path_dirs(",
        "user_path::Registry",
    ),
    (BOOTSTRAP, "fn undo_user_path(", "user_path::Registry"),
    (
        UNDO,
        "impl UserPathStore for RegistryStore",
        "user_path::write(",
    ),
    (UNDO, "pub fn from_process()", "Box::new(RegistryStore)"),
];

/// 本番コードの眺め（ファイル → テスト領域を潰した本文。行番号は原文のまま）
type Sources = BTreeMap<String, String>;

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番のソース全部（テスト専用のファイルは除く）。門を持つ 3 本は「黙って縮んでいない」も確かめる
fn load() -> Sources {
    let root = repo_root();
    let product = production_range::product_sources();
    let test_only = production_range::test_only_files(&product);
    product
        .into_iter()
        .filter(|rel| !test_only.contains(rel))
        .map(|rel| {
            let raw = std::fs::read_to_string(root.join(&rel))
                .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
            let text = if [USER_PATH, BOOTSTRAP, UNDO].contains(&rel.as_str()) {
                production_range::production(&raw, &rel)
            } else {
                production_range::scan(&raw).text
            };
            (rel, text)
        })
        .collect()
}

/// コメントの行か
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("///") || t.starts_with("//!")
}

/// 関数（impl）の窓: 宣言行の 1-based 行番号と、終わりの行（宣言行と同じ字下げの `}`）の 1-based 行番号
fn window(src: &str, head: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains(head))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)?;
    Some((start + 1, end + 1))
}

/// 窓の中で `needle` を含む最初のコードの行（1-based）
fn first_in(src: &str, (from, to): (usize, usize), needle: &str) -> Option<usize> {
    src.lines()
        .enumerate()
        .skip(from - 1)
        .take(to + 1 - from)
        .find(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i + 1)
}

fn line(src: &str, n: usize) -> &str {
    src.lines().nth(n - 1).unwrap_or_default()
}

/// 1〜4 の違反（`file:line — 理由`）
fn violations(sources: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    let get = |rel: &str| sources.get(rel).map(String::as_str).unwrap_or_default();

    // 1. 境界の最後の砦
    let up = get(USER_PATH);
    match window(up, "pub fn write_typed(") {
        None => out.push(format!("{USER_PATH}:0 — `write_typed` が無い（走査の空振り）")),
        Some(w) => match (first_in(up, w, "skip_for_verification()"), first_in(up, w, "imp::write(")) {
            (Some(g), Some(wr)) if g < wr => {}
            (_, Some(wr)) => out.push(format!(
                "{USER_PATH}:{wr} — `write_typed` が門（`skip_for_verification()`）より前に本物のレジストリへ書く。\
                 門を通り忘れた呼び手が検証プロセスから利用者全体の Path を書き換える（#2959）"
            )),
            (_, None) => out.push(format!(
                "{USER_PATH}:{} — `write_typed` に `imp::write(` が無い（走査の空振り）",
                w.0
            )),
        },
    }
    match window(up, "pub fn write(") {
        Some(w) if first_in(up, w, "write_typed(value)").is_some() => {}
        Some(w) => out.push(format!(
            "{USER_PATH}:{} — `write` が `write_typed` を通らない = 最後の砦を迂回する（#2959）",
            w.0
        )),
        None => out.push(format!("{USER_PATH}:0 — `write` が無い（走査の空振り）")),
    }
    let typed = window(up, "pub fn write_typed(");
    for (i, l) in up.lines().enumerate() {
        let n = i + 1;
        let inside = typed.is_some_and(|(a, b)| (a..=b).contains(&n));
        if !inside && !is_comment(l) && l.contains("imp::write(") {
            out.push(format!(
                "{USER_PATH}:{n} — `write_typed` の外から `imp::write(` を呼ぶ = 最後の砦を迂回する（#2959）"
            ));
        }
    }

    // 2. 判定は 1 実装
    match window(up, "pub fn skip_for_verification()") {
        Some(w) if first_in(up, w, "crate::paths::is_verification_process()").is_some() => {}
        Some(w) => out.push(format!(
            "{USER_PATH}:{} — 門が `paths::is_verification_process()` 以外で検証プロセスを判定している。\
             判定を増やすと「窓は砂場・Path は本番」の半端が生まれる（#1253 / #2959）",
            w.0
        )),
        None => out.push(format!(
            "{USER_PATH}:0 — `skip_for_verification` が無い（走査の空振り）"
        )),
    }

    // 3. 書き手は既知の 4 か所だけ
    for (rel, src) in sources {
        if rel == USER_PATH {
            continue;
        }
        for (i, l) in src.lines().enumerate() {
            let n = i + 1;
            if is_comment(l) {
                continue;
            }
            let imports =
                l.contains("user_path::{") && (l.contains("write") || l.contains("Registry"));
            let Some(mark) = WRITERS
                .iter()
                .find(|w| l.contains(**w))
                .copied()
                .or(imports.then_some("use"))
            else {
                continue;
            };
            let known = KNOWN.iter().any(|(file, head, m)| {
                *file == rel.as_str()
                    && *m == mark
                    && window(src, head).is_some_and(|(a, b)| (a..=b).contains(&n))
            });
            if !known {
                out.push(format!(
                    "{rel}:{n} — 本物のユーザー環境変数 Path へ書く新しい口（`{}`）。検証プロセスの門 \
                     `{GATE}` を通してから、この番犬の既知の表へ足す（#2959）",
                    l.trim()
                ));
            }
        }
    }

    // 4. 既知の書き手は門の後ろ
    let bs = get(BOOTSTRAP);
    for (head, inner) in [
        (
            "fn ensure_user_path_dirs(",
            "fn ensure_user_path_dirs_with(",
        ),
        ("fn undo_user_path(", "fn undo_user_path_dirs_with("),
    ] {
        match window(bs, head) {
            None => out.push(format!("{BOOTSTRAP}:0 — `{head}` が無い（走査の空振り）")),
            Some(w) => match first_in(bs, w, "&user_path::Registry") {
                None => out.push(format!(
                    "{BOOTSTRAP}:{} — `{head}` に本物の口（`&user_path::Registry`）が無い（走査の空振り）",
                    w.0
                )),
                Some(reg) if reg > 1 && line(bs, reg - 1).contains(GATE) => {}
                Some(reg) => out.push(format!(
                    "{BOOTSTRAP}:{reg} — `{head}` が本物のレジストリの口を、門の答え（`{GATE}`）を渡さずに使う。\
                     検証プロセスの setup が利用者全体の Path を書き換える（#2959）"
                )),
            },
        }
        match window(bs, inner) {
            None => out.push(format!("{BOOTSTRAP}:0 — `{inner}` が無い（走査の空振り）")),
            Some(w) => {
                let skip = first_in(bs, w, "if skip {");
                let touch = [
                    first_in(bs, w, "store.read()"),
                    first_in(bs, w, "store.write("),
                ]
                .into_iter()
                .flatten()
                .min();
                match (skip, touch) {
                    (Some(s), Some(t)) if s < t => {}
                    (_, Some(t)) => out.push(format!(
                        "{BOOTSTRAP}:{t} — `{inner}` が門の答え（`if skip {{`）で返る前に Path を読み書きする（#2959）"
                    )),
                    (_, None) => out.push(format!(
                        "{BOOTSTRAP}:{} — `{inner}` に Path の読み書きが無い（走査の空振り）",
                        w.0
                    )),
                }
            }
        }
    }
    let ud = get(UNDO);
    match window(ud, "pub fn from_process()") {
        None => out.push(format!("{UNDO}:0 — `from_process` が無い（走査の空振り）")),
        Some(w) => match (first_in(ud, w, GATE), first_in(ud, w, "Box::new(RegistryStore)")) {
            (Some(g), Some(r))
                if g < r && (g..r).any(|n| line(ud, n).contains("!verification")) => {}
            (_, Some(r)) => out.push(format!(
                "{UNDO}:{r} — `from_process` が門（`{GATE}`）と `!verification` を見ずに本物のレジストリの口を作る。\
                 検証プロセスの `tako setup undo` が利用者全体の Path を書き換える（#2788 / #2959）"
            )),
            (_, None) => out.push(format!(
                "{UNDO}:{} — `from_process` に `RegistryStore` が無い（走査の空振り）",
                w.0
            )),
        },
    }
    out
}

#[test]
fn 検証プロセスは本物のユーザー環境変数pathへ書かない() {
    let found = violations(&load());
    assert!(
        found.is_empty(),
        "検証プロセスの門（#2959）を通らない書き手がある:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let sources = load();
    assert!(
        sources.len() > 100,
        "本番のソースが少なすぎる（{} 本）",
        sources.len()
    );
    for (file, head, mark) in KNOWN {
        let src = sources
            .get(*file)
            .unwrap_or_else(|| panic!("{file} が無い"));
        let w = window(src, head).unwrap_or_else(|| panic!("{file}: `{head}` の窓が無い"));
        assert!(
            first_in(src, w, mark).is_some(),
            "{file}:{} — `{head}` に `{mark}` が無い（表が古い）",
            w.0
        );
    }
}

/// 置き換え 1 か所（無ければ注入の前提が崩れている = 落とす）
fn inject(sources: &Sources, rel: &str, from: &str, to: &str) -> Sources {
    let mut out = sources.clone();
    let src = out.get_mut(rel).unwrap_or_else(|| panic!("{rel} が無い"));
    assert!(src.contains(from), "{rel}: 注入の元 `{from}` が無い");
    *src = src.replacen(from, to, 1);
    out
}

#[test]
fn 修正前の形へ戻すと名指しする() {
    let sources = load();
    assert!(violations(&sources).is_empty(), "前提: 今の木は緑");
    let cases: [(&str, &str, &str, &str); 6] = [
        // setup の足す段が門の答えを渡さない（#2959 の不具合そのもの）
        (
            BOOTSTRAP,
            "        user_path::skip_for_verification(),\n        &user_path::Registry,\n        tako_core::i18n::lang(),\n    )\n}",
            "        false,\n        &user_path::Registry,\n        tako_core::i18n::lang(),\n    )\n}",
            BOOTSTRAP,
        ),
        // 中身が門で返る前に読む
        (BOOTSTRAP, "    if skip {\n        let process_has", "    if false {\n        let process_has", BOOTSTRAP),
        // 最後の砦を外す
        (
            USER_PATH,
            "    if skip_for_verification() {\n        return Err(UserPathError::VerificationProcess);\n    }\n    imp::write(value)",
            "    imp::write(value)",
            USER_PATH,
        ),
        // 判定を増やす
        (
            USER_PATH,
            "crate::paths::is_verification_process(),",
            "std::env::var_os(\"TAKO_ISOLATED\").is_some(),",
            USER_PATH,
        ),
        // undo の門を外す
        (UNDO, "if user_path::skip_for_verification() {", "if false {", UNDO),
        // 新しい書き手
        (
            "crates/tako-control/src/setup.rs",
            "\n",
            "\nfn added() { let _ = tako_core::platform::user_path::write_typed(&v); }\n",
            "crates/tako-control/src/setup.rs",
        ),
    ];
    for (rel, from, to, want) in cases {
        let found = violations(&inject(&sources, rel, from, to));
        assert!(
            found.iter().any(|v| v.starts_with(&format!("{want}:"))),
            "{rel} の `{}` を戻しても {want} を名指さない: {found:?}",
            from.lines().next().unwrap_or_default()
        );
    }
}
