//! 本番コードだけの眺めを作る（番犬テストの共有部品）
//!
//! 走査型の番犬は「テストコードを検査対象から外す」ために、ソースを
//! `#[cfg(test)]` の位置で**切って**いた。この切り方はファイル中で
//! **最初に現れる `#[cfg(test)]` まで**しか残さないので、テスト用ヘルパの
//! `#[cfg(test)]` が 1 つ途中にあるだけで、それ以降の本番コードが番犬の
//! 視界から丸ごと消える（#1420）。#1403 の作業中に `remote.rs` で実測したときは、
//! 検査対象より後ろへ 4 行のヘルパを置いただけで走査範囲が 5,903 行 → 3,124 行
//! （全体の 70.7% → 37.1%）へ縮み、**番犬は 7 本とも緑のまま**だった。
//!
//! ここは**切らない**。テスト領域だけを空白へ潰して、残り全部を返す。
//!
//! - 途中にヘルパが 1 つあっても、消えるのは**そのヘルパだけ**
//! - テストモジュールが複数あっても、**あいだの本番コードは残る**
//! - **バイト長と行番号が保たれる**ので `file:line` の名指しがそのまま使える
//! - 潰す範囲は `code_view` で決めるが**中身は原文のまま**返すので、
//!   文字列リテラルを見る番犬（`body.contains("tako remote start")` など）が壊れない
//!
//! 「黙って縮んだ」の検出は [`production`] / [`production_with_floor`] が持つ
//! （本番コードが下限を割ったら、潰した先頭の領域を `file:line` で名指して落ちる）。
//! テスト側を見張る番犬のために裏返し（[`tests_only`]）も同じ 1 実装から出す。
//!
//! ## テスト領域の見つけ方（#1445）
//!
//! 初版はリテラルの `#[cfg(test)]` しか探していなかったので、
//! `#[cfg(all(test, unix))]` のように**属性が合成された**テストモジュール
//! （`tako-control/src/discovery.rs` など 8 ファイル 11 か所）が潰れず、**本番コードとして
//! 走査に混ざって**いた。番犬はテスト内の直書きを本番の違反として誤検出するか、
//! 逆にテスト内の文字列を「扱った証拠」として拾って緑になる（#1417 が踏んだ型）。
//!
//! いまは cfg 述語を読んで、`test` を**正の位置**に含むものをテスト領域とする
//! （[`mentions_test`]）。`#[cfg(not(test))]` は「テストでないとき」に載る
//! **本番コード**なので潰さない。A/B は `TAKO_1445_LEGACY=1`（[`cfg_mode`]）。

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// 位置決めだけに使う（コメント・文字列の中の `#[cfg(test)]` や `}` を数えないため）。
// 取り込む側から**二重に宣言させない**ため公開する（`clippy::duplicate_mod`）
#[path = "code_view.rs"]
pub mod code_view;

/// item 属性の入口（`#[cfg_attr(` は**別物**なのでここには当たらない）
const ATTR_HEAD: &str = "#[cfg(";

/// #1445 以前が探していた綴り（A/B の旧アームでだけ使う）
const LITERAL_ATTR: &str = "#[cfg(test)]";

/// cfg 述語の読み方（A/B の軸 = #1445）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CfgMode {
    /// `test` を**正の位置**に含む cfg 述語をテスト領域として潰す（現行）
    TestPredicate,
    /// リテラルの `#[cfg(test)]` だけを潰す（#1445 以前の再現）
    LiteralOnly,
}

/// 既定の読み方。`TAKO_1445_LEGACY=1` のときだけ #1445 以前へ戻す
pub fn cfg_mode() -> CfgMode {
    if std::env::var("TAKO_1445_LEGACY").is_ok_and(|v| v == "1") {
        CfgMode::LiteralOnly
    } else {
        CfgMode::TestPredicate
    }
}

/// 本番コードだけの眺めと、潰したテスト領域の内訳
pub struct Production {
    /// テスト領域を空白へ潰した本文（**原文とバイト長・行番号が同じ**）
    pub text: String,
    /// 潰した領域（`(開始行, 終了行)`。1 始まり・両端を含む）
    pub regions: Vec<(usize, usize)>,
    /// 潰したバイト数（改行を含む素の範囲長）
    pub removed_bytes: usize,
}

impl Production {
    /// 残った本番コードのバイト数
    pub fn kept_bytes(&self) -> usize {
        self.text.len().saturating_sub(self.removed_bytes)
    }

    /// 本番コードがファイル全体に占める割合（0.0〜1.0）
    pub fn coverage(&self) -> f64 {
        if self.text.is_empty() {
            return 1.0;
        }
        self.kept_bytes() as f64 / self.text.len() as f64
    }

    /// 潰した領域を人が読める形にする（落ちたときの手がかり）
    pub fn describe_regions(&self) -> String {
        if self.regions.is_empty() {
            return "なし".to_string();
        }
        self.regions
            .iter()
            .map(|(a, b)| format!("{a}〜{b} 行"))
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// 本番コードが全体に占める割合の既定の下限。
///
/// これを割るのは「テストが本番より厚い」ではなく「範囲取りが本番コードを
/// 飲み込んだ」ときなので落とす。下限を使っている番犬が読むファイルの現行最小は
/// `url_guard.rs` の 50.9% なので余裕がある。
///
/// ただしリポジトリ全体では `tmux_backend.rs` が 23.1%・`mcp/tests.rs` が 0.0%
/// （ファイルまるごとテスト）なので、**テストの厚いファイルを読む番犬は
/// [`production_with_floor`] で明示的に下げる**か、下限を使わず [`scan`] を直に呼ぶ
/// （`platform_parity` は 4 クレートの `src` を丸ごと走査するので後者）
pub const MIN_COVERAGE: f64 = 0.30;

/// テスト領域（`#[cfg(test)]` が付いた item）を空白へ潰した本文を返す。
///
/// 潰すのは属性行の行頭から item の終わり（対応する `}` か、宣言なら `;`）まで。
/// `#[cfg(test)] mod tests` も `#[cfg(test)] fn helper()` も
/// `#[cfg(test)] stmt();` も同じ扱いで、**どれが先に来ても残りの本番コードは消えない**
pub fn scan(src: &str) -> Production {
    scan_with_mode(src, cfg_mode())
}

/// [`scan`] の読み方を指定する版（A/B の 2 アームを 1 回の実行で並べるため）
pub fn scan_with_mode(src: &str, mode: CfgMode) -> Production {
    let view = code_view::code_view(src);
    let ranges = test_ranges(&view, mode);
    let mut out = src.as_bytes().to_vec();
    let mut removed = 0usize;
    for &(start, end) in &ranges {
        blank(&mut out, start, end);
        removed += end - start;
    }
    Production {
        text: to_text(out),
        regions: ranges
            .iter()
            .map(|&(s, e)| (line_at(&view, s), line_at(&view, e.saturating_sub(1))))
            .collect(),
        removed_bytes: removed,
    }
}

/// [`scan`] の裏返し: **テスト領域だけ**を残し、本番コードを空白へ潰した本文。
///
/// テスト側を見張る番犬（「テストが `Instant` を使っていない」等）が使う。
/// 行番号が保たれるので、こちらも `file:line` でそのまま名指しできる
pub fn tests_only(src: &str) -> String {
    tests_only_with_mode(src, cfg_mode())
}

/// [`tests_only`] の読み方を指定する版（A/B 用。境界は [`scan_with_mode`] と同じ 1 実装）
pub fn tests_only_with_mode(src: &str, mode: CfgMode) -> String {
    let view = code_view::code_view(src);
    let mut out = src.as_bytes().to_vec();
    let mut cursor = 0usize;
    for (start, end) in test_ranges(&view, mode) {
        blank(&mut out, cursor, start);
        cursor = end;
    }
    let len = out.len();
    blank(&mut out, cursor, len);
    to_text(out)
}

/// [`scan`] + 「黙って縮んでいない」ことの確認（下限は [`MIN_COVERAGE`]）
pub fn production(src: &str, rel: &str) -> String {
    production_with_floor(src, rel, MIN_COVERAGE)
}

/// [`scan`] + 下限を指定した確認。
///
/// 落ちるときは**潰した先頭の領域**を `rel:line` で名指す（どこから飲み込まれたかが
/// そのまま出る）。走査範囲が空になる形（目印の消失・ファイル改名）も同時に落とす
pub fn production_with_floor(src: &str, rel: &str, floor: f64) -> String {
    let scanned = scan(src);
    let head = scanned.regions.first().map(|(a, _)| *a).unwrap_or(1);
    assert!(
        scanned.kept_bytes() > 1000,
        "{rel}:{head} 走査範囲が空（本番コードが {} バイトしか残っていない）。\n\
         潰した領域: {}",
        scanned.kept_bytes(),
        scanned.describe_regions()
    );
    assert!(
        scanned.coverage() >= floor,
        "{rel}:{head} 走査範囲が全体の {:.1}% まで縮んだ（下限 {:.1}%）。\n\
         `#[cfg(test)]` の範囲取りが本番コードを飲み込んでいる（#1420）。\n\
         潰した領域: {}",
        scanned.coverage() * 100.0,
        floor * 100.0,
        scanned.describe_regions()
    );
    scanned.text
}

/// テスト専用の item のバイト範囲（`code_view` 上で数える）。
///
/// `#[cfg(` から対応する `)]` までを述語として読み、[`mentions_test`] が真なら
/// その item 全体を範囲にする。`#[cfg_attr(test, …)]` は**入口の綴りが違う**ので
/// ここには当たらない（`cfg_attr` は item 自体は本番ビルドにも載り、付ける属性だけを
/// 切り替えるものなので、潰すと本番コードが消える = 対象外にするのが正しい）
fn test_ranges(view: &str, mode: CfgMode) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    while let Some(hit) = view[cursor..].find(ATTR_HEAD) {
        let at = cursor + hit;
        let after_head = at + ATTR_HEAD.len();
        // 行頭に置かれた item 属性だけを見る（`#![cfg(test)]` や式中の綴りは対象外）
        let line_start = view[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
        if !view[line_start..at].trim().is_empty() {
            cursor = after_head;
            continue;
        }
        // `)]` で閉じるところまでが属性（複数行に跨る `#[cfg(all(\n test,\n …))]` も拾う）
        let Some(close) = matching_paren(view, after_head - 1) else {
            cursor = after_head;
            continue;
        };
        if view.as_bytes().get(close + 1) != Some(&b']') {
            cursor = after_head;
            continue;
        }
        let attr_end = close + 2;
        let is_test = match mode {
            CfgMode::TestPredicate => mentions_test(&view[after_head..close]),
            // #1445 以前: リテラルの綴りだけ（合成 cfg は本番コードとして残っていた）
            CfgMode::LiteralOnly => &view[at..attr_end] == LITERAL_ATTR,
        };
        if !is_test {
            cursor = attr_end;
            continue;
        }
        let end = item_end(view, attr_end);
        ranges.push((line_start, end));
        cursor = end;
    }
    ranges
}

/// cfg 述語が `test` を**正の位置**に含むか。
///
/// - `test` → 真
/// - `all(…)` / `any(…)` → 子のどれかが真なら真
///   （`any(test, feature = "visual-test")` は本番ビルドに載らないテスト用の足場なので潰す）
/// - `not(…)` → **偽**。`#[cfg(not(test))]` は「テストでないとき」に載る**本番コード**で、
///   潰すと本番が番犬の視界から消える（`orchestrator/mod.rs` などに実在する）
/// - それ以外の原子（`unix` / `windows` / `feature = "…"`）→ 偽。
///   `code_view` が文字列を空白へ潰しているので `feature = "test-util"` は当たらない
fn mentions_test(pred: &str) -> bool {
    let pred = pred.trim();
    if pred == "test" {
        return true;
    }
    for name in ["all", "any"] {
        if let Some(inner) = strip_call(pred, name) {
            return split_top(inner).into_iter().any(mentions_test);
        }
    }
    false
}

/// `name(…)` の中身（`all(test, unix)` → `test, unix`）
fn strip_call<'a>(pred: &'a str, name: &str) -> Option<&'a str> {
    let rest = pred.strip_prefix(name)?.trim_start();
    rest.strip_prefix('(')?.strip_suffix(')')
}

/// 深さ 0 のカンマで割る（`all(a, any(b, c)), d` を壊さない）
fn split_top(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

/// `open` の `(` に対応する `)` の位置（`code_view` 上なので文字列の中の括弧は数えない）
fn matching_paren(view: &str, open: usize) -> Option<usize> {
    let bytes = view.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// 属性の直後から item の終わりまで。対応する `}` か、宣言なら深さ 0 の `;`
fn item_end(view: &str, from: usize) -> usize {
    let bytes = view.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            // 釣り合わない `}`（切り出しに失敗）はそこで打ち切る
            b'}' if depth <= 1 => return i + 1,
            b'}' => depth -= 1,
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// `[start, end)` を空白で潰す（**改行は残す** = 行番号が保たれる）
fn blank(out: &mut [u8], start: usize, end: usize) {
    let end = end.min(out.len());
    for byte in &mut out[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// 位置決めに使う眺め（番犬が「コードとして残っているか」を見るときにも使う）
pub fn code_view_of(src: &str) -> String {
    code_view::code_view(src)
}

fn to_text(out: Vec<u8>) -> String {
    String::from_utf8(out).expect("空白で潰しても UTF-8 は壊れない")
}

/// バイト位置の行番号（1 始まり）
fn line_at(view: &str, offset: usize) -> usize {
    view[..offset.min(view.len())].matches('\n').count() + 1
}

// --------------------------------------------------------------- ファイル単位の境界

// 以下は `#[cfg(test)] mod x;` で読む**ファイル丸ごとのテスト**を見分ける。ファイルの中に
// `#[cfg(test)]` が無いので、[`tests_only`] だけでは本番コードとして扱ってしまう。
// #1790 の番犬が持っていた 1 実装を、テスト側を走査する番犬（#1868）と共有するために移した

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/src/` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
pub fn product_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} が読める: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("ディレクトリ項目").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ が読める");
    for entry in crates {
        let src = entry.expect("crate の項目").path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();
    rels
}

// --------------------------------------------------------------- テスト専用のファイル

/// `mod NAME;`（外のファイルを読む宣言）なら NAME
fn out_of_line_mod(line: &str) -> Option<&str> {
    let mut t = line.trim();
    for vis in ["pub(crate) ", "pub(super) ", "pub "] {
        if let Some(rest) = t.strip_prefix(vis) {
            t = rest;
            break;
        }
    }
    let name = t.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
}

/// 宣言の直前の属性行にある `#[path = "…"]` の値（コメント行と他の属性は飛ばす）
fn path_attr<'a>(raw_lines: &[&'a str], decl: usize) -> Option<&'a str> {
    for l in raw_lines[..decl].iter().rev() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("#[path = \"") {
            return rest.strip_suffix("\"]");
        }
        if !(t.starts_with("#[") || t.starts_with("//")) {
            return None;
        }
    }
    None
}

/// `parent` が `mod name;` で読むファイル（リポジトリルートからの相対パス）
fn resolve_mod(parent: &str, name: &str, path: Option<&str>) -> String {
    let dir = Path::new(parent).parent().expect("親のディレクトリ");
    let file = Path::new(parent)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    let is_mod_rs = matches!(file, "main.rs" | "lib.rs" | "mod.rs")
        || dir.file_name().is_some_and(|d| d == "bin");
    let candidates: Vec<PathBuf> = match path {
        Some(p) => vec![dir.join(p)],
        None => {
            let base = if is_mod_rs {
                dir.to_path_buf()
            } else {
                dir.join(file.trim_end_matches(".rs"))
            };
            vec![
                base.join(format!("{name}.rs")),
                base.join(name).join("mod.rs"),
            ]
        }
    };
    let root = repo_root();
    for c in &candidates {
        let normalized = root.join(c);
        if normalized.is_file() {
            let canon = normalized.canonicalize().expect("実在するファイル");
            return canon
                .strip_prefix(&root)
                .expect("リポジトリ内")
                .to_string_lossy()
                .replace('\\', "/");
        }
    }
    panic!(
        "{parent} の `mod {name};` が読むファイルが見つからない（候補 {candidates:?}）。\
         インラインの mod の中の宣言なら、この番犬の解決を足す"
    );
}

/// ファイル全体がテスト専用のもの（`#[cfg(test)] mod x;` で読むファイルと、その子孫）
pub fn test_only_files(files: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut frontier: Vec<String> = Vec::new();
    for rel in files {
        let raw = read(rel);
        let raw_lines: Vec<&str> = raw.lines().collect();
        let tests = code_view::code_view(&tests_only(&raw));
        for (k, l) in tests.lines().enumerate() {
            if let Some(name) = out_of_line_mod(l) {
                let child = resolve_mod(rel, name, path_attr(&raw_lines, k));
                if out.insert(child.clone()) {
                    frontier.push(child);
                }
            }
        }
    }
    // テスト専用のファイルが読む子は、cfg が付いていなくてもテスト専用
    while let Some(rel) = frontier.pop() {
        let raw = read(&rel);
        let raw_lines: Vec<&str> = raw.lines().collect();
        let code = code_view::code_view(&raw);
        for (k, l) in code.lines().enumerate() {
            if let Some(name) = out_of_line_mod(l) {
                let child = resolve_mod(&rel, name, path_attr(&raw_lines, k));
                if out.insert(child.clone()) {
                    frontier.push(child);
                }
            }
        }
    }
    out
}
