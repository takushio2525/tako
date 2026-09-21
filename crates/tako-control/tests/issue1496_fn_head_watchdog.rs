//! **#1496 の番犬**: 番犬の関数名追跡は共有ヘルパ（`tako_core::source_scan`）を通る。
//!
//! ## 何が壊れていたか
//!
//! 走査型の番犬は違反行を**囲んでいる関数名**で名指しして報告する。その追跡を
//! `line.trim_start().strip_prefix("fn ")` で書くと **`pub(crate) fn` / `async fn` が
//! 関数の頭に見えない**ので、中の違反が**手前の関数の名前**で報告される。
//! 実例（#1491 の worker が観測）は `pub(crate) fn kill_shelved_tab_clicked` の中の
//! 違反が、手前の `fn shelved_tab_groups` の中として報告されたもの。
//! **検出そのものは効く（落ちる）が名指しが嘘になる**ので、読み手が違う関数を探す。
//!
//! 壊れ方が「落ちない」ではなく「落ちるが嘘をつく」なので、テストは緑のまま残る。
//! 規約文だけでは同型が残る（実測で 5 か所あった）ので番犬で止める。
//!
//! ## 棚卸し（2026-09-21 時点）
//!
//! ヘルパへ寄せた（行ベースで「この行は関数の頭」を判定していたもの）:
//!
//! | 場所 | 旧実装 | 取りこぼしていた形 |
//! |---|---|---|
//! | `tako-app/src/main.rs`（#770 の 2 本） | `strip_prefix("fn ")` | 可視性・`async`・`const` すべて |
//! | `remote_scrollback_watchdog.rs` | `pub(crate) `/`pub `/`async ` を剥がす | `pub(super)` / `const` / `unsafe` / `extern "C"` |
//! | `issue841_peer_process_watchdog.rs` | 4 種の完全前置 | `async` / `const` / `unsafe` / `pub(in …)` |
//! | `test_residue_watchdog.rs` | `HEADS` 10 種 | `pub async` / `pub const` / `pub(in …)` |
//! | `setup_bootstrap_agent_watchdog.rs` | `strip_prefix("pub fn ")` | `pub async` / `pub(crate)` |
//!
//! 残したもの（**#1496 の癖は無い**ので触らない。検出力を動かさないため）:
//!
//! - `test_timing_watchdog.rs` の `enclosing_fn` / `tako-app/src/ui_text/lang_watchdog.rs`
//!   の `fn_defs` — `fn ` を**語として**探す位置ベース。修飾子の前置に依らず正しい名前を返す
//! - `issue1376_url_scheme_watchdog.rs` — 「`fn ` を含む行」の逆引き。`pub(crate) fn …` の
//!   行も当たるので #1496 の癖は無い（コメント行にも当たる緩さは別の話で、
//!   狭めると検査範囲が広がって**検出が弱くなる**側へ倒れるので動かさない）
//! - `psmux_cleanup_timeout_watchdog.rs` / `psmux_e2e_wait_watchdog.rs` の
//!   `starts_with("fn legacy_")` — 特定の命名の A/B アームを探すもので、関数追跡ではない

use std::path::{Path, PathBuf};

/// この番犬自身（禁止パターンの文字列そのものを持っているので走査から外す）
const SELF_FILE: &str = "issue1496_fn_head_watchdog.rs";

/// 行ベースの「この行は関数の頭」判定の直書き。
/// 見つけたら `tako_core::source_scan::fn_head_name` を通すこと
const BANNED: &[&str] = &[
    "strip_prefix(\"fn \")",
    "strip_prefix(\"pub fn \")",
    "strip_prefix(\"pub(crate) fn \")",
    "strip_prefix(\"pub(super) fn \")",
    "strip_prefix(\"async fn \")",
    "starts_with(\"fn \")",
    "starts_with(\"pub fn \")",
    "starts_with(\"pub(crate) fn \")",
    "starts_with(\"async fn \")",
    "split_once(\"fn \")",
];

/// 直書きを許す場所（ファイル, 囲んでいる関数名, 理由）。**黙って増やさない**
const ALLOWED: &[(&str, &str, &str)] = &[(
    "crates/tako-app/src/main.rs",
    "legacy",
    "#1496 の材料テストが持つ**旧規則の再現**。`pub(crate) fn` の中の違反が手前の \
     関数名で報告されることを同じ実行の中で見せるためのもので、番犬の名指しには使わない",
)];

/// ヘルパへ寄せた番犬（ファイル, 呼んでいるはずの入口）。
/// 巻き戻し・改名で**ヘルパを通らなくなった**ことに気づけるようにする
const MIGRATED: &[(&str, &str)] = &[
    ("crates/tako-app/src/main.rs", "source_scan::fn_head_name"),
    (
        "crates/tako-control/tests/remote_scrollback_watchdog.rs",
        "source_scan::fn_head_name",
    ),
    (
        "crates/tako-control/tests/issue841_peer_process_watchdog.rs",
        "source_scan::fn_head_name",
    ),
    (
        "crates/tako-control/tests/setup_bootstrap_agent_watchdog.rs",
        "source_scan::fn_head_name",
    ),
    (
        "crates/tako-control/tests/test_residue_watchdog.rs",
        "source_scan::is_top_level_fn_head",
    ),
    (
        "crates/tako-control/tests/test_residue_watchdog.rs",
        "source_scan::fn_head_name",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// `crates/**/*.rs` を集める（`src/` も `tests/` も見る。番犬は両方に住んでいる）
fn crate_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(&repo_root().join("crates"), &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn rel_of(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// **本体**: 行ベースの関数頭判定の直書きが、許可した 1 箇所以外に無いこと。
///
/// 名指しはヘルパ自身で行う（この番犬が壊れていれば自分の報告も嘘になる = 自己適用）
#[test]
fn 番犬の関数名追跡は共有ヘルパを通る() {
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    for path in crate_sources() {
        if path.file_name().is_some_and(|n| n == SELF_FILE) {
            continue;
        }
        let rel = rel_of(&path);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        scanned += 1;
        let mut current = String::from("(ファイル先頭)");
        for (i, line) in src.lines().enumerate() {
            if let Some(name) = tako_core::source_scan::fn_head_name(line) {
                current = name.to_string();
            }
            // 説明文の中のパターンは対象外（この番犬の理由文・移行の注記）
            if line.trim_start().starts_with("//") {
                continue;
            }
            let Some(hit) = BANNED.iter().find(|b| line.contains(**b)) else {
                continue;
            };
            if ALLOWED
                .iter()
                .any(|(f, func, _)| *f == rel && *func == current)
            {
                continue;
            }
            offenders.push(format!("{rel}:{} （{current} の中）: {hit}", i + 1));
        }
    }
    assert!(
        scanned > 100,
        "走査が当たっていない（{scanned} ファイルしか読めていない）"
    );
    assert!(
        offenders.is_empty(),
        "番犬の関数名追跡が直書きで残っている:\n  {}\n\
         → `tako_core::source_scan::fn_head_name` を通してください。\
         `pub(crate) fn` / `async fn` を関数の頭と見なさないと、中の違反が\
         **手前の関数名**で報告されます（#1496）",
        offenders.join("\n  ")
    );
}

/// 寄せた先が実際にヘルパを呼んでいること（許可リストだけ残して
/// 中身を書き戻した、改名で空振りした、を検出する）
#[test]
fn ヘルパへ寄せた番犬が実際にヘルパを呼んでいる() {
    for (rel, needle) in MIGRATED {
        let path = repo_root().join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
        assert!(
            src.contains(needle),
            "{rel} が `{needle}` を呼んでいない。関数追跡を書き戻したか、\
             ヘルパを改名したまま（#1496）"
        );
    }
    // 許可リストの免除先が実在すること（改名で穴が開いたまま気づかない、を防ぐ）
    for (rel, func, why) in ALLOWED {
        let src = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
        assert!(
            src.lines()
                .any(|l| tako_core::source_scan::fn_head_name(l) == Some(*func)),
            "{rel} に免除先の `fn {func}` が無い（根拠: {why}）。改名したら免除も直す"
        );
    }
}
