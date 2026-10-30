//! **#2400 の番犬**: テストがプロセスの env を `set_var` / `remove_var` で書き換えて、
//! 同じテストバイナリの中で並走する**他のテスト**へ漏らさない。
//!
//! ## なぜ止めるのか
//!
//! env はプロセスに 1 つで、`cargo test` は 1 つのテストバイナリの中でテストを並列に走らせる。
//! lib のテスト（`crates/*/src`）は crate ごとに数千本が 1 プロセスに乗り、モジュールごとの
//! `Mutex` は**別のモジュールの読み手**を止めない。実物: `remote` のテストが `TEST_ENV_LOCK` の下で
//! `TAKO_841_LEGACY=1` を立てた窓に、`platform::local_endpoint` のテストが別の `ENV_LOCK` の下で
//! `verify_peer` を引き、「tailscaled でない接続は拒否」が `NotRequired` に化けて落ちた（#2400 の A/B）。
//! `HOME` / `TAKO_DATA_DIR` を書き換えるテストは、並走する全部のテストの置き場を差し替えていた。
//!
//! ## 何を固定するか
//!
//! テストのコード（`crates/*/src` の `#[cfg(test)]` の領域とテスト専用のファイル、`crates/*/tests`）の
//! `set_var(` / `remove_var(` を、次のどれかでなければ `file:line` で落とす。
//!
//! 1. **そのテストバイナリにテストが 1 本だけ**（`crates/*/tests/*.rs` の直下で `#[test]` が 1 つ）=
//!    漏れる先のテストが同じプロセスに居ない。2 本目を足した時点でこの番犬が落ちる
//! 2. **囲む関数が先に子の入口を通している**: `tako_core::run_alone!(…)`（その 1 本だけの子で
//!    走らせ直す）・`tako_core::test_child::assert_alone(…)`（子の外なら panic するヘルパの頭）・
//!    #2391 の `rerun_without(…)`
//! 3. **理由つきの目印**: 書き換えの段落（空行で区切られたひとかたまり）か、囲む関数の doc に
//!    `// tako:env-write-ok <理由>`。バイナリの全テストが同じロックで 1 本ずつ走るなど、
//!    漏れないことをその場で説明できるものだけに使う（直列化は最後の手段）
//!
//! コメントと文字列の中（他の番犬の注入用の断片や説明文）は数えない（`code_view` で潰す）。
//! 製品のコード（`TAKO_SELF_TEST` のセルフテスト・visual-test を含む）は見ない（範囲外。#2400）。
//!
//! ## 目印で通しても、手で戻す対は落とす（#2520）
//!
//! 3 の目印の理由「全テストが同じロックで 1 本ずつ走る」は**並走**の話で、書き換えた後に
//! **同じ関数の中で手で戻す**形（`set_var(K, ..)` … `remove_var(K)`）は、間で panic すると戻らない。
//! ロックは毒になっても次のテストが取るので、同じバイナリで後に走るテストが書き換えた値を読んで
//! 連鎖で落ち、本当の失敗を隠す（実物: `issue1977_lsp_install_followup` の旧腕が `TAKO_1977_LEGACY=1` を
//! 残し、後の 2 本が旧腕で走って落ちた）。目印で通した書き換えのうち、いちばん内側の同じ関数の中で
//! 同じ env を `;` を挟んで書き直すもの（`match` の腕どうしの `set_var` / `remove_var` は 1 つの書き換え）を
//! `file:line` で落とす。`Drop` の中で戻す形は書き換えと関数が分かれるので落とさない（panic でも走る）。
//! 既にある同じ形は [`KNOWN_RESTORE_PAIRS`] に件数と理由で載せ、増えても減っても落とす。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙と分類が実ファイルで生きていることを、
//! [`注入した書き換えを名指しする`] で実ファイルへの注入（入口の無い書き換え・2 本目のテスト・
//! 理由の無い目印・入口より前の書き換え・目印つきの手で戻す対）が `file:line` で名指しされることを
//! 確かめる（一覧の件数のずれ = 増えた・減った・無くなったも）。[`KNOWN_RESTORE_PAIRS`] の件数の一致は、
//! 手で戻す対の判定が実ファイルで生きている証拠を兼ねる。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

/// 書き換えの呼び出し（`env_remove(` は `Command` の子だけの話なので数えない）
const NEEDLES: &[&str] = &["set_var(", "remove_var("];

/// 子の入口（囲む関数がこれを書き換えより前に通していれば、書き換えは子の中だけで起きる）
const GUARDS: &[&str] = &[
    "run_alone!(",
    "rerun_alone(",
    "assert_alone(",
    "rerun_without(",
];

/// 理由つきで通す目印
const MARKER: &str = "tako:env-write-ok";

/// 目印で通した書き換えを同じ関数の中で手で戻す対が既にあるファイルと、その中の対の数・理由
/// （#2520。増えても減っても落とす = 直したら件数を下げるか行を消す）
const KNOWN_RESTORE_PAIRS: &[(&str, usize, &str)] = &[
    (
        "crates/tako-control/tests/issue1893_lsp_hover_followup.rs",
        1,
        "#2525 で子の中へ寄せる残り（`TAKO_1893_LEGACY`）",
    ),
    (
        "crates/tako-control/tests/issue1909_lsp_completion_ticket.rs",
        1,
        "#2525 で子の中へ寄せる残り（`TAKO_1909_INJECT_HOLD`）",
    ),
    (
        "crates/tako-control/tests/issue2034_lsp_watched_files.rs",
        2,
        "#2525 で子の中へ寄せる残り（`TAKO_2034_LEGACY`。1 つは `#[ignore]` の実測用）",
    ),
    (
        "crates/tako-control/tests/lid_residual_windows.rs",
        4,
        "#2525 で子の中へ寄せる残り（`TAKO_DATA_DIR`。`#![cfg(windows)]`）",
    ),
    (
        "crates/tako-core/tests/pane_term_env.rs",
        3,
        "#2525 の残り（`TERM` / `COLORTERM` はペインが親の env を継ぐことを測るので親で書く = `Drop` で戻す形へ）",
    ),
    (
        "crates/tako-core/tests/issue2092_pin_hold.rs",
        1,
        "漏れない: 全テストが頭で通す `serial()` が `ENV_KEYS` を消すので、間で panic して残っても次のテストの頭で消える",
    ),
];

/// どこのコードか
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// `crates/*/src` の `#[cfg(test)]` の領域
    LibTests,
    /// `crates/*/src` のテスト専用のファイル（`#[cfg(test)] mod x;` で読むもの）
    LibTestFile,
    /// `crates/*/tests/*.rs`（1 ファイル = 1 テストバイナリ）
    Integration,
    /// `crates/*/tests/<dir>/*.rs`（複数のバイナリに取り込まれる部品）
    IntegrationPart,
}

/// 通した理由（集計に出す）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Pass {
    SingleTestBinary,
    Guarded,
    Marked,
}

#[derive(Default)]
struct Scan {
    violations: Vec<String>,
    passed: BTreeMap<Pass, usize>,
    /// 目印で通した書き換えを手で戻す対（ファイル → (書き換えの行, env の第 1 引数)。#2520）
    restore_pairs: BTreeMap<String, Vec<(usize, String)>>,
}

impl Scan {
    fn seen(&self) -> usize {
        self.violations.len() + self.passed.values().sum::<usize>()
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// 関数の本体の範囲（`fn` の行頭, 本体を閉じる `}`）。`view` はコメントと文字列を潰した眺め。
///
/// 関数ポインタ型（`fn(u32)`）は名前が無いので数えない。宣言だけの `fn x();` は `;` で捨てる
/// （引数の配列型 `[u8; 4]` の `;` は括弧の中なので捨てない）
fn fn_ranges(view: &str) -> Vec<(usize, usize)> {
    let b = view.as_bytes();
    let mut out = Vec::new();
    let mut stack: Vec<Option<usize>> = Vec::new();
    let mut pending: Option<usize> = None;
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"fn") && (i == 0 || !is_ident_byte(b[i - 1])) {
            let mut j = i + 2;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\n' || b[j] == b'\t') {
                j += 1;
            }
            if j > i + 2
                && j < b.len()
                && (b[j].is_ascii_alphabetic() || b[j] == b'_' || b[j] >= 0x80)
            {
                pending = Some(view[..i].rfind('\n').map_or(0, |n| n + 1));
                depth = 0;
                i = j;
                continue;
            }
        }
        match b[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b';' if depth <= 0 => pending = None,
            b'{' => stack.push(pending.take()),
            b'}' => {
                if let Some(Some(head)) = stack.pop() {
                    out.push((head, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

/// 目印の後ろに理由があるか（`tako:env-write-ok` だけの行は通さない）
fn marked(line: &str) -> bool {
    line.split_once(MARKER).is_some_and(|(_, rest)| {
        !rest
            .trim_start_matches(|c: char| c == ':' || c == '：' || c.is_whitespace())
            .trim()
            .is_empty()
    })
}

/// `raw` の `at` の行を含む段落（上へ空行か関数の頭まで）か、関数の doc に目印があるか
fn has_marker(raw: &str, at: usize, fn_head: Option<usize>) -> bool {
    let lines: Vec<&str> = raw.lines().collect();
    let hit = line_of(raw, at) - 1;
    let head = fn_head.map(|h| line_of(raw, h) - 1);
    let mut k = hit;
    loop {
        let l = lines[k];
        if l.trim().is_empty() {
            break;
        }
        if marked(l) {
            return true;
        }
        if Some(k) == head || k == 0 {
            break;
        }
        k -= 1;
    }
    // 関数の doc（属性とコメントの行だけが続くあいだ）
    if let Some(h) = head {
        let mut k = h;
        while k > 0 {
            k -= 1;
            let t = lines[k].trim_start();
            if !(t.starts_with("//") || t.starts_with("#[")) {
                break;
            }
            if marked(t) {
                return true;
            }
        }
    }
    false
}

/// 1 ファイルを見る。`raw` は原文、`kind` は置き場
fn scan_file(rel: &str, raw: &str, kind: Kind, scan: &mut Scan) {
    let region = match kind {
        Kind::LibTests => code_view::code_view(&production_range::tests_only(raw)),
        Kind::LibTestFile | Kind::Integration | Kind::IntegrationPart => code_view::code_view(raw),
    };
    let single_test_binary = kind == Kind::Integration
        && region.matches("#[test]").count() + region.matches("#[tokio::test").count() == 1;
    let ranges = fn_ranges(&region);
    // 目印で通した書き換え（位置, いちばん内側の関数の頭, env の第 1 引数）
    let mut marked: Vec<(usize, Option<usize>, String)> = Vec::new();
    for needle in NEEDLES {
        for (at, _) in region.match_indices(needle) {
            if at > 0 && is_ident_byte(region.as_bytes()[at - 1]) {
                continue;
            }
            if single_test_binary {
                *scan.passed.entry(Pass::SingleTestBinary).or_default() += 1;
                continue;
            }
            // 囲む関数のうちいちばん外側（テスト関数・ヘルパ・impl のメソッド）
            let outer = ranges
                .iter()
                .filter(|(h, e)| *h <= at && at <= *e)
                .min_by_key(|(h, _)| *h)
                .copied();
            if let Some((head, _)) = outer {
                let before = &region[head..at];
                if GUARDS.iter().any(|g| before.contains(g)) {
                    *scan.passed.entry(Pass::Guarded).or_default() += 1;
                    continue;
                }
            }
            let arg = first_arg(raw, at, needle);
            if has_marker(raw, at, outer.map(|(h, _)| h)) {
                *scan.passed.entry(Pass::Marked).or_default() += 1;
                let inner = ranges
                    .iter()
                    .filter(|(h, e)| *h <= at && at <= *e)
                    .max_by_key(|(h, _)| *h)
                    .map(|(h, _)| *h);
                marked.push((at, inner, arg.to_string()));
                continue;
            }
            scan.violations.push(format!(
                "{rel}:{}: テストがプロセスの env を書き換えている（{needle}{arg}…）。env はプロセスに 1 つで、\
                 同じテストバイナリの中で並走する他のテストが同じ値を読む（#2400）。\
                 → テストの頭で `if tako_core::run_alone!(<テスト名>) {{ return; }}` を通して子の中で書くか、\
                 製品の関数へ値を引数で渡す。書き換えるヘルパなら頭で `tako_core::test_child::assert_alone(..)`。\
                 本当に漏れないなら、その段落か関数の doc に `// {MARKER} <理由>`",
                line_of(raw, at)
            ));
        }
    }
    let pairs = hand_restores(&region, marked);
    if !pairs.is_empty() {
        scan.restore_pairs
            .entry(rel.to_string())
            .or_default()
            .extend(pairs.into_iter().map(|(at, key)| (line_of(raw, at), key)));
    }
}

/// 書き換えの第 1 引数（env の名前の式）。区切りは ASCII なので文字境界（固定幅では切らない = #2043）
fn first_arg<'a>(raw: &'a str, at: usize, needle: &str) -> &'a str {
    let rest = raw[at + needle.len()..].trim_start();
    rest[..rest.find([',', ')', '\n']).unwrap_or(rest.len())].trim()
}

/// 目印で通した書き換えのうち、いちばん内側の同じ関数の中で同じ env を `;` を挟んで後から
/// 書き直すもの（手で戻す対。#2520）を、対ごとに最初の書き換えの位置と env で返す。
/// `match` の腕どうし（`Some(v) => set_var(K, v), None => remove_var(K)`）は `;` を挟まないので
/// 1 つの書き換え。`Drop` の中で戻す形は書き換えと関数が分かれるので対にならない
fn hand_restores(
    region: &str,
    mut marked: Vec<(usize, Option<usize>, String)>,
) -> Vec<(usize, String)> {
    marked.sort_by_key(|(at, _, _)| *at);
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut paired: Vec<(usize, &str)> = Vec::new();
    for (i, (at, inner, key)) in marked.iter().enumerate() {
        let Some(inner) = *inner else {
            continue;
        };
        if paired.contains(&(inner, key.as_str())) {
            continue;
        }
        let restored = marked[i + 1..]
            .iter()
            .any(|(later, later_inner, later_key)| {
                *later_inner == Some(inner) && later_key == key && region[*at..*later].contains(';')
            });
        if restored {
            paired.push((inner, key));
            out.push((*at, key.clone()));
        }
    }
    out
}

/// 手で戻す対の判定（#2520）。`known`（本番は [`KNOWN_RESTORE_PAIRS`]）に無いファイルの対は 1 つずつ
/// `file:line` で、載っているファイルは対の数が宣言と違えば対の行を並べて落とす。宣言したファイルの
/// 対が無くなったことは全ファイルを見た走査（`whole`）でだけ落とす
fn restore_findings(scan: &Scan, known: &[(&str, usize, &str)], whole: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, pairs) in &scan.restore_pairs {
        if known.iter().any(|(f, _, _)| f == rel) {
            continue;
        }
        for (line, key) in pairs {
            out.push(format!(
                "{rel}:{line}: テストが env（{key}）を書き換え、同じ関数の中で手で戻している。間で panic すると\
                 戻らず、同じテストバイナリで後に走るテストが書き換えた値を読んで連鎖で落ちる（#2520。目印の\
                 「1 本ずつ走る」は並走を防ぐだけで、この漏れは防がない）。\
                 → テストの頭で `if tako_core::run_alone!(<テスト名>) {{ return; }}` を通して子の中で書く\
                 （戻しは要らない）か、製品の関数へ値を引数で渡す。親で書くしかないなら `Drop` で戻す（panic でも走る）"
            ));
        }
    }
    for (file, want, why) in known {
        let pairs = scan.restore_pairs.get(*file);
        if pairs.is_none() && !whole {
            continue;
        }
        let got = pairs.map_or(0, Vec::len);
        if got != *want {
            let lines: Vec<String> = pairs
                .into_iter()
                .flatten()
                .map(|(line, key)| format!("{file}:{line}（{key}）"))
                .collect();
            out.push(format!(
                "{file}: 目印つきで手で戻す対が {got} 件（宣言は {want} 件。理由: {why}）。\
                 増えたなら子の中で書く（`run_alone!`）、減ったなら KNOWN_RESTORE_PAIRS を直す: {}",
                lines.join(" / ")
            ));
        }
    }
    out
}

/// `crates/*/tests` の `.rs` を（リポジトリルートからの相対パス, 置き場）で列挙する
fn integration_sources() -> Vec<(String, Kind)> {
    fn walk(dir: &Path, top: bool, out: &mut Vec<(PathBuf, Kind)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, false, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let kind = if top {
                    Kind::Integration
                } else {
                    Kind::IntegrationPart
                };
                out.push((path, kind));
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates/ が読める")
        .flatten()
    {
        walk(&entry.path().join("tests"), true, &mut files);
    }
    let mut out: Vec<(String, Kind)> = files
        .into_iter()
        .map(|(p, k)| {
            let rel = p
                .strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/");
            (rel, k)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn scan_repo() -> (Scan, usize) {
    let mut scan = Scan::default();
    let lib = production_range::product_sources();
    let test_files = production_range::test_only_files(&lib);
    let mut files = 0usize;
    for rel in &lib {
        let kind = if test_files.contains(rel) {
            Kind::LibTestFile
        } else {
            Kind::LibTests
        };
        scan_file(rel, &read(rel), kind, &mut scan);
        files += 1;
    }
    for (rel, kind) in integration_sources() {
        scan_file(&rel, &read(&rel), kind, &mut scan);
        files += 1;
    }
    (scan, files)
}

#[test]
fn テストはプロセスのenvを書き換えて並走するテストへ漏らさない() {
    let (scan, _) = scan_repo();
    assert!(scan.violations.is_empty(), "{}", scan.violations.join("\n"));
}

#[test]
fn 目印で通した書き換えを手で戻して後のテストへ漏らさない() {
    let (scan, _) = scan_repo();
    let found = restore_findings(&scan, KNOWN_RESTORE_PAIRS, true);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let (scan, files) = scan_repo();
    println!(
        "見たファイル {files} 個・書き換え {} 個: {:?}",
        scan.seen(),
        scan.passed
    );
    assert!(files >= 800, "列挙が縮んでいる（{files} 個）");
    // 3 つの通し方がどれも実ファイルで生きている（どれかの判定が空振りすると 0 になる）
    for pass in [Pass::SingleTestBinary, Pass::Guarded, Pass::Marked] {
        assert!(
            scan.passed.get(&pass).copied().unwrap_or(0) >= 5,
            "{pass:?} で通した書き換えが少なすぎる（判定が空振りしている？）: {:?}",
            scan.passed
        );
    }
}

/// 注入: 実ファイルの本文を書き換えて渡し、`file:line` で名指しされることを確かめる
/// （リポジトリのファイルは触らない）
#[test]
fn 注入した書き換えを名指しする() {
    let violations = |rel: &str, text: &str, kind: Kind| {
        let mut scan = Scan::default();
        scan_file(rel, text, kind, &mut scan);
        scan.violations
    };

    // (1) lib のテストに入口の無い書き換えを足す（#2400 の前の形）
    let rel = "crates/tako-control/src/lsp/xref.rs";
    let src = read(rel);
    assert!(
        violations(rel, &src, Kind::LibTests).is_empty(),
        "現行が通らない"
    );
    let anchor = "    fn 種類の呼び名は日英とも_5_つ別() {\n";
    assert!(src.contains(anchor), "注入 (1) の足場が無い");
    let injected = src.replacen(
        anchor,
        &format!("{anchor}        std::env::set_var(\"TAKO_LSP_XREF_TIMEOUT_SECS\", \"1\");\n"),
        1,
    );
    let want = line_of(&injected, injected.find(anchor).expect("足場")) + 1;
    let v = violations(rel, &injected, Kind::LibTests);
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{rel}:{want}:"))),
        "注入 (1) を {rel}:{want} で名指せない: {v:?}"
    );

    // (2) 入口より**前**の書き換えは子の外（親）で走るので名指す
    let guarded = "        if tako_core::run_alone!(";
    let at = src
        .find(guarded)
        .expect("注入 (2) の足場（run_alone! の行）が無い");
    let head = src[..at].rfind('\n').map_or(0, |n| n + 1);
    let injected = format!(
        "{}        std::env::remove_var(\"TAKO_LSP_XREF_TIMEOUT_SECS\");\n{}",
        &src[..head],
        &src[head..]
    );
    let want = line_of(&injected, head);
    let v = violations(rel, &injected, Kind::LibTests);
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{rel}:{want}:"))),
        "注入 (2) を {rel}:{want} で名指せない: {v:?}"
    );

    // (3) 1 本だけのバイナリは通すが、2 本目のテストを足すと名指す
    let rel = "crates/tako-control/tests/issue372_legacy_ab.rs";
    let src = read(rel);
    assert!(
        src.contains("set_var("),
        "注入 (3) の足場（書き換え）が無い"
    );
    assert!(
        violations(rel, &src, Kind::Integration).is_empty(),
        "1 本だけのバイナリを名指した"
    );
    let injected = format!("{src}\n#[test]\nfn 二本目() {{}}\n");
    let v = violations(rel, &injected, Kind::Integration);
    let want = line_of(&src, src.find("set_var(").expect("書き換え"));
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{rel}:{want}:"))),
        "注入 (3) を {rel}:{want} で名指せない: {v:?}"
    );

    // (4) 理由の無い目印は通さない。理由があれば通す
    let rel = "crates/tako-control/src/x.rs";
    let body = |marker: &str| {
        format!(
            "#[cfg(test)]\nmod tests {{\n    #[test]\n    fn t() {{\n        {marker}\n        \
             std::env::set_var(\"TAKO_X\", \"1\");\n    }}\n}}\n"
        )
    };
    let v = violations(rel, &body("// tako:env-write-ok"), Kind::LibTests);
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{rel}:6:"))),
        "注入 (4) の理由の無い目印を通した: {v:?}"
    );
    assert!(
        violations(
            rel,
            &body("// tako:env-write-ok このバイナリの全テストが serial() で 1 本ずつ"),
            Kind::LibTests
        )
        .is_empty(),
        "理由つきの目印を通さない"
    );

    // (5) 文字列とコメントの中・製品のコード・`env_remove` は数えない
    let text = "fn product() {\n    std::env::set_var(\"TAKO_X\", \"1\");\n}\n#[cfg(test)]\nmod tests {\n    \
                #[test]\n    fn t() {\n        // std::env::set_var(\"TAKO_X\", \"1\");\n        \
                let _ = \"std::env::remove_var(\\\"HOME\\\")\";\n        \
                std::process::Command::new(\"x\").env_remove(\"HOME\");\n    }\n}\n";
    assert!(
        violations(rel, text, Kind::LibTests).is_empty(),
        "数えないものを名指した"
    );

    // (6) #2520: issue1977 の旧腕を直す前の形（目印つきで書き換え、同じ関数の中で手で戻す）へ戻すと
    // 名指す。目印だけ（戻さない）は手で戻す対と数えず、`Drop` で戻す形（issue1930）も名指さない
    let restores = |rel: &str, text: &str| {
        let mut scan = Scan::default();
        scan_file(rel, text, Kind::Integration, &mut scan);
        assert!(
            scan.violations.is_empty(),
            "{rel} の目印か入口が効いていない: {:?}",
            scan.violations
        );
        restore_findings(&scan, KNOWN_RESTORE_PAIRS, false)
    };
    let rel = "crates/tako-control/tests/issue1977_lsp_install_followup.rs";
    let src = read(rel);
    assert!(restores(rel, &src).is_empty(), "現行の {rel} を名指した");
    let guard = "fn 旧挙動では片付けない() {\n    let _env = serial();\n    \
                 if tako_core::run_alone!(旧挙動では片付けない) {\n        return;\n    }\n";
    assert!(src.contains(guard), "注入 (6) の足場（旧腕の入口）が無い");
    let marked_only = src.replacen(
        guard,
        "// tako:env-write-ok このバイナリの全テストが serial() で 1 本ずつ走るので、並走する読み手が居ない\n\
         fn 旧挙動では片付けない() {\n    let _env = serial();\n",
        1,
    );
    assert!(
        restores(rel, &marked_only).is_empty(),
        "戻さない目印つきの書き換えを手で戻す対と数えた"
    );
    let read_back = "    let cleanup = manager.status(None)[\"fetch\"][\"cleanup\"].clone();\n";
    assert!(
        marked_only.contains(read_back),
        "注入 (6) の足場（片付けの読み出し）が無い"
    );
    let injected = marked_only.replacen(
        read_back,
        &format!("{read_back}    std::env::remove_var(\"TAKO_1977_LEGACY\");\n"),
        1,
    );
    let body = injected.find("fn 旧挙動では片付けない()").expect("旧腕");
    let want = line_of(
        &injected,
        body + injected[body..]
            .find("std::env::set_var(")
            .expect("旧腕の書き換え"),
    );
    let v = restores(rel, &injected);
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{rel}:{want}:"))),
        "注入 (6) を {rel}:{want} で名指せない: {v:?}"
    );
    let rel = "crates/tako-control/tests/issue1930_lsp_stale_empty.rs";
    let src = read(rel);
    assert!(
        src.contains("impl Drop for Restore") && src.contains("tako:env-write-ok"),
        "注入 (6) の対照（目印つきで Drop で戻す形）が無い"
    );
    assert!(
        restores(rel, &src).is_empty(),
        "Drop で戻す形を手で戻す対と名指した"
    );

    // (7) 一覧に載ったファイルは件数で縛る: 対を足す（増えた）・宣言より少ない（直したのに一覧が古い）・
    // 無くなった、のどれも名指す。本物の issue1893（1 対）を使う
    let rel = "crates/tako-control/tests/issue1893_lsp_hover_followup.rs";
    let src = read(rel);
    let scan_of = |text: &str| {
        let mut scan = Scan::default();
        scan_file(rel, text, Kind::Integration, &mut scan);
        scan
    };
    let declared = |n: usize| [(rel, n, "注入")];
    assert!(
        restore_findings(&scan_of(&src), &declared(1), true).is_empty(),
        "宣言どおりの 1 対を名指した"
    );
    let added = format!(
        "{src}\n#[test]\n// tako:env-write-ok 注入\nfn 二つ目の対() {{\n    \
         std::env::set_var(\"TAKO_1893_LEGACY\", \"1\");\n    let _ = 1;\n    \
         std::env::remove_var(\"TAKO_1893_LEGACY\");\n}}\n"
    );
    let v = restore_findings(&scan_of(&added), &declared(1), false);
    assert!(
        v.iter()
            .any(|m| m.starts_with(&format!("{rel}: 目印つきで手で戻す対が 2 件（宣言は 1 件"))),
        "増えた対を名指せない: {v:?}"
    );
    let v = restore_findings(&scan_of(&src), &declared(2), true);
    assert!(
        v.iter().any(|m| m.contains("1 件（宣言は 2 件")),
        "減った対（一覧が古い）を名指せない: {v:?}"
    );
    let v = restore_findings(&Scan::default(), &declared(1), true);
    assert!(
        v.iter().any(|m| m.contains("0 件（宣言は 1 件")),
        "対が無くなったファイルを名指せない: {v:?}"
    );
}
