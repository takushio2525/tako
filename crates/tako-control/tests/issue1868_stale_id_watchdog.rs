//! **#1868 の番犬**: テストが「存在しないペイン / タブ / ウィンドウ」を決め打ちの数値で
//! 表さない。
//!
//! ## なぜ止めるのか
//!
//! ペイン・タブ・ウィンドウの ID は、それぞれ**プロセス全体で共有**の採番器で振られる。
//! テストのバイナリは 1 プロセスで数千のテストを並列に走らせるので、`305` / `9999` のような
//! 決め打ちの「無い ID」は、同じバイナリの他のテストが何枚作ったかしだいで、**そのテスト
//! 自身の Workspace に実在しうる**。実物: #1860 でテストが増えた後、dispatch のテストが
//! stale のつもりの `305` を引き当て、CI の 3 並列で毎回落ちた（2ecab5d）。#1868 で
//! テストの領域全体を洗い出し、`tako_core::unused_id`（採番器から 1 つ取る）へ寄せた。
//!
//! ## 何を固定するか
//!
//! テストの領域（`crates/*/tests/` の全体と、`crates/*/src/` の `test` を正の位置に含む cfg の
//! 中・`#[cfg(test)] mod x;` で読むファイル = `production_range` と同じ読み）で、**ID の位置**に
//! ある整数リテラルのうち、次の文脈のものを落とす:
//!
//! 1. **見つからないことを確かめている**: `assert` / `matches!` の中の
//!    `PaneNotFound(9999)` / `TabNotFound(…)` / `WindowNotFound(…)`
//! 2. **無いものの名前で束ねている**: `let stale_id = 99999_u64;` /
//!    `let ghost = TabId::from_raw(9999);` / `let missing = dispatch(…{ pane: Some(9999), … })` /
//!    `gone.source_pane = PaneId::from_raw(987_654);`（名前の語: stale / ghost / gone / missing /
//!    absent / nonexist… / bogus / phantom / vanished。`dead` / `unknown` は状態の名前に使われる
//!    ので数えない）
//! 3. **無いものだとコメントで言っている**: 同じ行の末尾か、その文の直上のコメントに
//!    「存在しない」「現存しない」「実在しない」「居ない」「stale」「ghost」「absent」「nonexist」
//! 4. **無いもの用の数字の形**: 同じ数字が 3 桁以上並ぶもの（`999` / `9_999` / `88888`）
//!
//! ID の位置 = `PaneId::from_raw(N)` / `TabId::from_raw(N)` / `WindowId::from_raw(N)` と、
//! 欄 [`ID_FIELDS`] の `N` / `Some(N)`。欄 `window` は tmux の window 番号（採番器と無関係）にも
//! 使われるので 4. だけで見る。
//!
//! 直し方は `tako_core::unused_id::{pane, tab, window}`。`u64::MAX` 系の定数は使わない
//! （`TabId::from_raw` / `WindowId::from_raw` が採番器を `fetch_max(id + 1)` で進めるので、
//! dispatch を通ると採番器が張り付いて 0 へ巻き戻る。理由の全文は `unused_id` のモジュール doc）。
//!
//! 通すもの: 同じ行か 1 行上に `// 固定 ID 安全: <理由>`。ペインを採番しない純粋な表
//! （固定値だけで組んだ入力を関数へ渡す）の「無いもの」は、`unused_id` の値が表の小さい
//! 固定値と重なりうるので、固定値のまま目印を書く方が正しい。
//!
//! 既知の穴: 引数の位置だけで ID と分かる形（`collect_worker_status_ctx(&host, 999_999)` /
//! `release(&mut host, "release", Some(9999))`）は、行番号や上限の引数と見分けられないので
//! 見ない（#1868 ではこの形の 3 か所も直した）。JSON の `"pane": N` も見ない。
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で列挙・テスト専用ファイルの見分け・ID の位置の検出・目印の
//! 通過が実ファイルで生きていることを、[`修正前の形を実際に落とす`] で #1860 / #1868 で直した
//! 形が `file:line` で名指しされ、通すべき形（純粋な表・tmux の window 番号・エラー値の
//! 組み立て）が通ることを、[`直した実ファイルを直書きへ戻すと名指しする`] で**実際のファイルの
//! 1 行を修正前へ戻した本文**が落ちることを確かめる（注入は `cargo test` の中で完結し、
//! リポジトリのファイルは触らない）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// 本番コードとテスト領域の境界（ファイル単位を含む）は 1 実装（#1420 / #1445 / #1790）
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `crates/*/tests/` の `.rs` を再帰で列挙する（リポジトリルートからの相対パス。並びは固定）
fn test_sources() -> Vec<String> {
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
        let tests = entry.expect("crate の項目").path().join("tests");
        if tests.is_dir() {
            walk(&tests, &mut files);
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

// --------------------------------------------------------------- 語彙

/// 通す目印（同じ行か 1 行上）
const MARK: &str = "固定 ID 安全:";

/// ID を入れる欄（`window` は別扱い = 4. だけで見る）
const ID_FIELDS: [&str; 11] = [
    "pane",
    "tab",
    "group_tab",
    "pane_id",
    "tab_id",
    "window_id",
    "source_pane",
    "target_pane",
    "caller_pane",
    "master_pane",
    "preview",
];

/// tmux の window 番号にも使われる欄（同じ数字の並びだけを見る）
const WINDOW_FIELD: &str = "window";

/// `from_raw` で ID を組む型
const ID_TYPES: [&str; 3] = ["PaneId", "TabId", "WindowId"];

/// 「無いもの」を表す名前の部品（`_` で割った 1 つ。`nonexist` は前方一致）
const STALE_NAME_PARTS: [&str; 9] = [
    "stale", "ghost", "gone", "missing", "absent", "bogus", "phantom", "vanished", "nonexist",
];

/// 「無いもの」を言うコメントの語
const STALE_COMMENT_WORDS: [&str; 8] = [
    "存在しない",
    "現存しない",
    "実在しない",
    "居ない",
    "stale",
    "ghost",
    "absent",
    "nonexist",
];

// --------------------------------------------------------------- 字句の部品

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `s[at..]` が整数リテラルで始まるなら `(数字だけ, 終わりの位置)`。
/// `99_999` / `99999_u64` / `7u64` を読み、`1.0` / `0x10` / `12ms` は読まない
fn int_literal(s: &str, at: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if !bytes.get(at).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut end = at;
    while bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
    {
        end += 1;
    }
    if s[end..].starts_with("u64") {
        end += 3;
    }
    if s[end..]
        .chars()
        .next()
        .is_some_and(|c| is_ident(c) || c == '.')
    {
        return None;
    }
    let digits: String = s[at..end]
        .trim_end_matches("u64")
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    Some((digits, end))
}

fn skip_spaces(s: &str, mut at: usize) -> usize {
    while s.as_bytes().get(at) == Some(&b' ') {
        at += 1;
    }
    at
}

/// ID の種類（直し方の案内に使う）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Pane,
    Tab,
    Window,
}

impl Kind {
    fn of_field(name: &str) -> Kind {
        match name {
            "tab" | "group_tab" | "tab_id" | "TabId" | "Tab" => Kind::Tab,
            "window" | "window_id" | "WindowId" | "Window" => Kind::Window,
            _ => Kind::Pane,
        }
    }

    fn helper(self) -> &'static str {
        match self {
            Kind::Pane => "tako_core::unused_id::pane()",
            Kind::Tab => "tako_core::unused_id::tab()",
            Kind::Window => "tako_core::unused_id::window()",
        }
    }
}

/// 1 行（`code_view` 上 = コメントと文字列は空白）の ID の位置にある整数リテラル
#[derive(Clone, Debug)]
struct IdLiteral {
    digits: String,
    kind: Kind,
    /// 欄が `window`（tmux の window 番号と区別できない）
    window_field: bool,
}

fn id_literals(code_line: &str) -> Vec<IdLiteral> {
    let mut out = Vec::new();
    // 欄 `name: N` / `name: Some(N)`
    for (name, window_field) in ID_FIELDS
        .iter()
        .map(|f| (*f, false))
        .chain([(WINDOW_FIELD, true)])
    {
        let mut from = 0;
        while let Some(hit) = code_line[from..].find(name) {
            let at = from + hit;
            from = at + name.len();
            if code_line[..at]
                .chars()
                .next_back()
                .is_some_and(|c| is_ident(c) || c == '.')
            {
                continue;
            }
            let mut i = skip_spaces(code_line, at + name.len());
            if code_line.as_bytes().get(i) != Some(&b':') || code_line[i..].starts_with("::") {
                continue;
            }
            i = skip_spaces(code_line, i + 1);
            if code_line[i..].starts_with("Some(") {
                i = skip_spaces(code_line, i + "Some(".len());
            }
            if let Some((digits, _)) = int_literal(code_line, i) {
                out.push(IdLiteral {
                    digits,
                    kind: Kind::of_field(name),
                    window_field,
                });
            }
        }
    }
    // `PaneId::from_raw(N)` / `TabId::…` / `WindowId::…`
    for ty in ID_TYPES {
        let head = format!("{ty}::from_raw(");
        let mut from = 0;
        while let Some(hit) = code_line[from..].find(&head) {
            let at = from + hit;
            from = at + head.len();
            if code_line[..at].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            let i = skip_spaces(code_line, at + head.len());
            if let Some((digits, end)) = int_literal(code_line, i) {
                if code_line.as_bytes().get(skip_spaces(code_line, end)) == Some(&b')') {
                    out.push(IdLiteral {
                        digits,
                        kind: Kind::of_field(ty),
                        window_field: false,
                    });
                }
            }
        }
    }
    out
}

/// 同じ数字が 3 桁以上並ぶ（`999` / `9999` / `88888`）
fn repdigit(digits: &str) -> bool {
    let mut chars = digits.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    digits.len() >= 3 && chars.all(|c| c == first)
}

fn stale_name(name: &str) -> bool {
    name.split('_').any(|part| {
        STALE_NAME_PARTS
            .iter()
            .any(|w| part == *w || (*w == "nonexist" && part.starts_with(w)))
    })
}

fn stale_comment(text: &str) -> bool {
    let lower = text.to_lowercase();
    STALE_COMMENT_WORDS.iter().any(|w| lower.contains(w))
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// 行の末尾のコメント（`code_view` で潰れた `//` 以降の原文）。文字列の中の `//` は数えない
fn trailing_comment<'a>(raw: &'a str, code: &str) -> &'a str {
    let mut from = 0;
    while let Some(hit) = raw[from..].find("//") {
        let at = from + hit;
        if !code.get(at..).is_some_and(|rest| rest.starts_with("//")) {
            return &raw[at..];
        }
        from = at + 2;
    }
    ""
}

/// 制御の塊を開く行（`fn` / `for` / `if` / クロージャ …）。文の頭を探す遡りはここで止まる
fn opens_block(code: &str) -> bool {
    let t = code.trim();
    if !t.ends_with('{') {
        return false;
    }
    const HEADS: [&str; 14] = [
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "async fn ",
        "for ",
        "if ",
        "} else",
        "else",
        "loop",
        "while ",
        "match ",
        "mod ",
        "impl",
        "unsafe ",
    ];
    HEADS.iter().any(|h| t.starts_with(h)) || t.ends_with("| {") || t.ends_with("=> {")
}

/// `i` 行目を含む文の**直上のコメント塊**（無ければ空）。
///
/// 文の頭はインデントで探す（rustfmt 済みなので、文の続きの行は頭より深い）。頭と同じ深さで
/// `;` / `}` で終わる行（前の文）・制御の塊を開く行・空行に当たったらコメントは無い
fn statement_comment(raw: &[&str], code: &[&str], i: usize) -> String {
    let mut min = indent(code[i]);
    let mut j = i;
    while j > 0 {
        j -= 1;
        let (r, c) = (raw[j].trim(), code[j].trim());
        if r.is_empty() {
            return String::new();
        }
        if c.is_empty() {
            if r.starts_with("//") {
                let mut block = Vec::new();
                let mut k = j + 1;
                while k > 0 && raw[k - 1].trim().starts_with("//") && code[k - 1].trim().is_empty()
                {
                    k -= 1;
                    block.push(raw[k]);
                }
                return block.join("\n");
            }
            continue;
        }
        let ind = indent(code[j]);
        if opens_block(code[j]) && ind < min {
            return String::new();
        }
        if ind < min {
            min = ind;
        } else if ind == min && (c.ends_with(';') || c.ends_with('}') || c == "{") {
            return String::new();
        }
    }
    String::new()
}

/// `i` 行目を含む文が `assert` / `matches!` の中か（`NotFound(N)` を組み立てただけの
/// 表 = エラー分類の検査などは数えない）
fn inside_assertion(code: &[&str], i: usize) -> bool {
    let mut j = i;
    loop {
        let c = code[j];
        if c.contains("assert") || c.contains("matches!(") {
            return true;
        }
        if j == 0 || i - j >= 20 {
            return false;
        }
        j -= 1;
        let t = code[j].trim_end();
        if t.ends_with(';') || t.ends_with('{') || t.ends_with('}') {
            return false;
        }
    }
}

fn marked(raw: &[&str], i: usize) -> bool {
    raw[i].contains(MARK) || (i > 0 && raw[i - 1].contains(MARK))
}

// --------------------------------------------------------------- 判定

/// 違反 1 件（行は 1 始まり）
#[derive(Clone, Debug)]
struct Offender {
    line: usize,
    digits: String,
    kind: Kind,
    why: &'static str,
}

/// 1 ファイルの走査の結果
#[derive(Default)]
struct FileScan {
    offenders: Vec<Offender>,
    /// ID の位置にある整数リテラルの数（文脈を問わず。検出が生きている証拠）
    id_literals: usize,
    /// 目印で通した件数
    marked: usize,
    /// `NotFound(N)` を組み立てただけ（assert の外）として通した件数
    constructed: usize,
    /// 直し方の補助（`unused_id::`）をコードとして使っている箇所の数
    helper_uses: usize,
}

/// `let 名前 … = 右辺;` / `名前.欄 = 右辺;` の右辺に ID の位置の整数リテラルがあれば、
/// その行（0 始まり）とリテラル
fn bound_literal(code: &[&str], start: usize, rhs_col: usize) -> Option<(usize, IdLiteral)> {
    let mut depth = 0i32;
    for (k, line) in code.iter().enumerate().skip(start) {
        let from = if k == start { rhs_col } else { 0 };
        let seg = &line[from..];
        // 右辺がリテラルそのもの（`let stale_id = 99999_u64;`）
        if k == start {
            let t = seg.trim_start();
            let at = seg.len() - t.len();
            if let Some((digits, end)) = int_literal(seg, at) {
                if seg[end..].trim_start().starts_with(';') {
                    return Some((
                        k,
                        IdLiteral {
                            digits,
                            kind: Kind::Pane,
                            window_field: false,
                        },
                    ));
                }
            }
        }
        if let Some(lit) = id_literals(seg).into_iter().find(|l| !l.window_field) {
            return Some((k, lit));
        }
        for ch in seg.chars() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ';' if depth <= 0 => return None,
                _ => {}
            }
        }
        if depth < 0 || k - start > 40 {
            return None;
        }
    }
    None
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）。
/// `test_only` = ファイル全体がテスト（`crates/*/tests/` と `#[cfg(test)] mod x;` で読むファイル）
fn scan_source(src: &str, test_only: bool) -> FileScan {
    let view = if test_only {
        src.to_string()
    } else {
        production_range::tests_only(src)
    };
    let code_text = code_view::code_view(&view);
    let raw: Vec<&str> = src.lines().collect();
    let code: Vec<&str> = code_text.lines().collect();
    let mut scan = FileScan {
        helper_uses: code_text.matches("unused_id::").count(),
        ..FileScan::default()
    };
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut push =
        |scan: &mut FileScan, line: usize, lit: &IdLiteral, kind: Kind, why: &'static str| {
            if seen.insert(line) {
                scan.offenders.push(Offender {
                    line: line + 1,
                    digits: lit.digits.clone(),
                    kind,
                    why,
                });
            }
        };
    for i in 0..code.len().min(raw.len()) {
        let c = code[i];
        if c.trim().is_empty() {
            continue;
        }
        // 1. `assert` / `matches!` の中の `PaneNotFound(N)` 等
        for (prefix, kind) in [
            ("PaneNotFound(", Kind::Pane),
            ("TabNotFound(", Kind::Tab),
            ("WindowNotFound(", Kind::Window),
        ] {
            let mut from = 0;
            while let Some(hit) = c[from..].find(prefix) {
                let at = from + hit;
                from = at + prefix.len();
                let j = skip_spaces(c, at + prefix.len());
                let Some((digits, end)) = int_literal(c, j) else {
                    continue;
                };
                if c.as_bytes().get(skip_spaces(c, end)) != Some(&b')') {
                    continue;
                }
                let lit = IdLiteral {
                    digits,
                    kind,
                    window_field: false,
                };
                if marked(&raw, i) {
                    scan.marked += 1;
                } else if inside_assertion(&code, i) {
                    push(&mut scan, i, &lit, kind, "見つからないことを確かめている");
                } else {
                    scan.constructed += 1;
                }
            }
        }
        // 2. 無いものの名前で束ねる（`let ghost = …;` / `gone.source_pane = …;`）
        let t = c.trim_start();
        let lead = c.len() - t.len();
        let binding: Option<String> = if let Some(rest) = t.strip_prefix("let ") {
            let rest = rest.strip_prefix("mut ").unwrap_or(rest);
            Some(rest.chars().take_while(|ch| is_ident(*ch)).collect())
        } else {
            let name: String = t.chars().take_while(|ch| is_ident(*ch)).collect();
            t[name.len()..].starts_with('.').then_some(name)
        };
        if binding.as_deref().is_some_and(stale_name) {
            // 最初の単独の `=`（`==` / `=>` / `!=` / `<=` / `>=` は数えない）
            let bytes = c.as_bytes();
            let eq = (lead..bytes.len()).find(|&k| {
                bytes[k] == b'='
                    && bytes.get(k + 1) != Some(&b'=')
                    && bytes.get(k + 1) != Some(&b'>')
                    && !matches!(
                        bytes.get(k.wrapping_sub(1)),
                        Some(b'=' | b'!' | b'<' | b'>')
                    )
            });
            if let Some(eq) = eq {
                if let Some((k, lit)) = bound_literal(&code, i, eq + 1) {
                    if marked(&raw, i) || marked(&raw, k) {
                        scan.marked += 1;
                    } else {
                        push(&mut scan, k, &lit, lit.kind, "無いものの名前で束ねている");
                    }
                }
            }
        }
        // 3. / 4. ID の位置のリテラル
        for lit in id_literals(c) {
            scan.id_literals += 1;
            let rep = repdigit(&lit.digits);
            let commented = !lit.window_field
                && (stale_comment(trailing_comment(raw[i], c))
                    || stale_comment(&statement_comment(&raw, &code, i)));
            if !(rep || commented) {
                continue;
            }
            if marked(&raw, i) {
                scan.marked += 1;
                continue;
            }
            let why = if rep {
                "無いもの用の数字の形（同じ数字の並び）"
            } else {
                "無いものだとコメントで言っている"
            };
            push(&mut scan, i, &lit, lit.kind, why);
        }
    }
    scan.offenders.sort_by_key(|o| o.line);
    scan
}

fn offenders(src: &str, rel: &str, test_only: bool) -> Vec<String> {
    offenders_of(rel, &scan_source(src, test_only))
}

fn offenders_of(rel: &str, scan: &FileScan) -> Vec<String> {
    scan.offenders
        .iter()
        .map(|o| {
            format!(
                "{rel}:{} — 「存在しない ID」を決め打ちの {} で表している（{}）。ID はプロセス共有の\
                 採番器で振られるので、同じテストバイナリのテストが増えると実在しうる（#1860 / #1868）。\
                 `{}` で採番器から 1 つ取る。ペインを採番しない純粋な表なら、同じ行か 1 行上に \
                 `// {MARK} <理由>` を書く",
                o.line,
                o.digits,
                o.why,
                o.kind.helper()
            )
        })
        .collect()
}

/// テストの領域全体の走査の結果（3 本のテストで共有する）
struct Scanned {
    files: Vec<String>,
    test_only: BTreeSet<String>,
    scans: Vec<(String, FileScan)>,
}

fn scanned() -> &'static Scanned {
    static SCANNED: OnceLock<Scanned> = OnceLock::new();
    SCANNED.get_or_init(|| {
        let product = production_range::product_sources();
        let test_only = production_range::test_only_files(&product);
        let tests = test_sources();
        let scans = product
            .iter()
            .map(|rel| (rel.clone(), test_only.contains(rel)))
            .chain(tests.iter().map(|rel| (rel.clone(), true)))
            .map(|(rel, whole)| {
                let scan = scan_source(&read(&rel), whole);
                (rel, scan)
            })
            .collect();
        let mut files = product;
        files.extend(tests);
        Scanned {
            files,
            test_only,
            scans,
        }
    })
}

// --------------------------------------------------------------- テスト

#[test]
fn テストが存在しないidを決め打ちしていない() {
    let found: Vec<String> = scanned()
        .scans
        .iter()
        .flat_map(|(rel, scan)| offenders_of(rel, scan))
        .collect();
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていないこと（列挙が壊れて 0 件・検出が死んで 0 件で緑になるのを防ぐ）
#[test]
fn 走査が空振りしていない() {
    let Scanned {
        files,
        test_only,
        scans,
    } = scanned();
    for must in [
        "crates/tako-control/src/dispatch.rs",
        "crates/tako-core/src/workspace.rs",
        "crates/tako-app/src/main.rs",
        "crates/tako-control/tests/issue1679_lsp_diagnostics.rs",
    ] {
        assert!(
            files.iter().any(|f| f == must),
            "{must} が列挙に無い（crates/*/src・crates/*/tests の列挙が壊れている）"
        );
    }
    // `#[cfg(test)] mod tests;` で読むファイルを丸ごとテストとして見ている
    assert!(
        test_only.contains("crates/tako-control/src/runtime_probe/tests.rs"),
        "テスト専用ファイルの見分けが壊れている: {test_only:?}"
    );
    let total = |f: fn(&FileScan) -> usize| scans.iter().map(|(_, s)| f(s)).sum::<usize>();
    let (literals, marked, constructed, helper_uses) = (
        total(|s| s.id_literals),
        total(|s| s.marked),
        total(|s| s.constructed),
        total(|s| s.helper_uses),
    );
    eprintln!(
        "走査: {} ファイル（丸ごとテスト {} 件）・ID の位置のリテラル {literals} 件・目印で通した \
         {marked} 件・assert の外の NotFound {constructed} 件・unused_id の利用 {helper_uses} 件",
        files.len(),
        test_only.len()
    );
    // ID の位置のリテラルを拾えている（dispatch のテストだけで数百ある）
    assert!(
        literals >= 200,
        "ID の位置の検出が死んでいる: {literals} 件"
    );
    // 目印で通した純粋な表（workspace.rs の復帰先の表・wait.rs の実行の台帳）を拾えている
    assert!(marked >= 2, "目印の通過が働いていない: {marked} 件");
    // `NotFound(N)` を組み立てるだけの表（issue1399 のエラー分類）を assert と分けている
    assert!(constructed >= 1, "assert の外の NotFound を見分けていない");
    // 直し方の補助が実際に使われている（寄せた先が消えたら気づく。コメントと文字列は数えない）
    assert!(
        helper_uses >= 30,
        "unused_id の利用が少なすぎる: {helper_uses} 件"
    );
}

/// #1860 / #1868 で直した修正前の形が `file:line` で名指しされ、通すべき形は通ること
#[test]
fn 修正前の形を実際に落とす() {
    let tests = |body: &str| format!("#[cfg(test)]\nmod tests {{\n{body}}}\n");
    // (名前, 本文, 名指しされる行)
    let bad: [(&str, String, usize); 12] = [
        (
            "#1860 の 1 本目（文の直上のコメントで stale と言う）",
            tests(
                "    fn t() {\n        // stale map に登録の無い旧 ID（#567 の実事象: ペイン 305 は既に存在しない）\n        let result = dispatch(\n            &mut host,\n            Request::ResolvePane {\n                pane: Some(305),\n                caller_pid: None,\n            },\n        );\n    }\n",
            ),
            8,
        ),
        (
            "#1860 の 2 本目（行末のコメント）",
            tests(
                "    fn t() {\n        let r = dispatch(\n            Request::OrchestratorSelf {\n                pane: Some(305), // 現存しない stale ID\n            },\n        );\n    }\n",
            ),
            6,
        ),
        (
            "NotFound の assert",
            tests("    fn t() {\n        assert!(matches!(missing, Err(DispatchError::PaneNotFound(9999))));\n    }\n"),
            4,
        ),
        (
            "複数行の assert_eq の NotFound",
            tests("    fn t() {\n        assert_eq!(\n            err,\n            DispatchError::TabNotFound(42)\n        );\n    }\n"),
            6,
        ),
        (
            "let stale_id = 99999_u64",
            tests("    fn t() {\n        let stale_id = 99999_u64;\n    }\n"),
            4,
        ),
        (
            "let ghost = TabId::from_raw(…)",
            tests("    fn t() {\n        let ghost = TabId::from_raw(77777);\n    }\n"),
            4,
        ),
        (
            "let missing = dispatch(… pane: Some(N) …)",
            tests("    fn t() {\n        let missing = dispatch(\n            &mut host,\n            Request::LimitResume {\n                pane: Some(305),\n            },\n        );\n    }\n"),
            7,
        ),
        (
            "gone.source_pane = PaneId::from_raw(…)",
            tests("    fn t() {\n        gone.source_pane = PaneId::from_raw(987_654);\n    }\n"),
            4,
        ),
        (
            "同じ数字の並び（tab 欄）",
            tests("    fn t() {\n        let other = dispatch(Request::TreeSelection { tab: Some(9999) });\n    }\n"),
            4,
        ),
        (
            "同じ数字の並び（tako の window 欄）",
            tests("    fn t() {\n        let r = dispatch(Request::WindowFocus { window: 99999 });\n    }\n"),
            4,
        ),
        (
            "同じ数字の並び（preview 欄の from_raw）",
            tests("    fn t() {\n        host.preview = PaneId::from_raw(9_999);\n    }\n"),
            4,
        ),
        (
            "crates/*/tests の直書き（ファイル全体がテスト）",
            "fn t() {\n    let ghost = WindowId::from_raw(88888);\n}\n".to_string(),
            2,
        ),
    ];
    for (name, src, line) in &bad {
        let whole = !src.starts_with("#[cfg(test)]");
        let found = offenders(src, "x.rs", whole);
        assert!(
            found.len() == 1 && found[0].starts_with(&format!("x.rs:{line} ")),
            "{name}: x.rs:{line} を名指ししない: {found:#?}"
        );
    }

    // 本番コードの領域は見ない（テストの領域の外）
    assert!(offenders(
        "fn f() {\n    let ghost = TabId::from_raw(9999);\n}\n",
        "x.rs",
        false
    )
    .is_empty());

    let ok: [(&str, String); 9] = [
        (
            "目印つきの純粋な表",
            tests("    fn t() {\n        // 固定 ID 安全: 純粋関数へ渡す表は a / b だけ\n        let gone = WindowId::from_raw(9003);\n    }\n"),
        ),
        (
            "目印つきの同じ数字の並び",
            tests("    fn t() {\n        let e = RunEntry {\n            // 固定 ID 安全: 台帳だけを見る\n            pane_id: 999,\n        };\n    }\n"),
        ),
        (
            "エラー値を組み立てるだけ（assert の外）",
            tests("    fn t() {\n        use x::E;\n        let all = [\n            E::PaneNotFound(1),\n            E::TabNotFound(2),\n        ];\n    }\n"),
        ),
        (
            "状態の名前（dead / unknown）",
            tests("    fn t() {\n        let dead = PaneId::from_raw(1);\n        let unknown = finish(Ctx {\n            pane_id: 0,\n        });\n    }\n"),
        ),
        (
            "tmux の window 番号（コメントは存在しない window 番号）",
            tests("    fn t() {\n        // 存在しない window 番号は落ちる\n        let err = dispatch(\n            Request::TmuxSelectWindow {\n                pane: Some(pane),\n                window: 9,\n            },\n        );\n    }\n"),
        ),
        (
            "採番器から取った値",
            tests("    fn t() {\n        let stale = tako_core::unused_id::pane().as_u64();\n        let ghost = crate::unused_id::tab();\n        assert!(matches!(e, DispatchError::PaneNotFound(id) if id == stale));\n    }\n"),
        ),
        (
            "fn の doc が stale を語っても、最初の文の固定値は数えない",
            tests("    /// 存在しないペインの扱い\n    #[test]\n    fn t() {\n        let r = run(Request::Split { pane: 3 });\n    }\n"),
        ),
        (
            "行番号・pid など ID の位置ではない数",
            tests("    fn t() {\n        let gone = collect(&host, 999_999);\n        let stale = dir.join(\"tako-selftest-999999\");\n        let c = open(&mut host, &code, Some(999_999), None);\n    }\n"),
        ),
        (
            "文字列とコメントの中は見ない",
            tests("    fn t() {\n        // let ghost = TabId::from_raw(9999);\n        let s = \"pane: Some(9999)\";\n    }\n"),
        ),
    ];
    for (name, src) in &ok {
        let found = offenders(src, "x.rs", false);
        assert!(
            found.is_empty(),
            "{name}: 通すべき形を名指しした: {found:#?}"
        );
    }
}

/// 実際のファイルの直した 1 行を修正前へ戻した本文が、その行で名指しされること
#[test]
fn 直した実ファイルを直書きへ戻すと名指しする() {
    // (ファイル, 今の行, 修正前の行)。今の行はファイルの中で最初に現れるものを戻す
    let reverts = [
        (
            "crates/tako-control/src/dispatch.rs",
            "pane: Some(stale), // 現存しない stale ID（決め打ちの 305 は実在しうる）",
            "pane: Some(305), // 現存しない stale ID（決め打ちの 305 は実在しうる）",
        ),
        (
            "crates/tako-control/src/dispatch.rs",
            "let stale_id = tako_core::unused_id::pane().as_u64();",
            "let stale_id = 99999_u64;",
        ),
        (
            "crates/tako-control/src/dispatch.rs",
            "gone.source_pane = tako_core::unused_id::pane();",
            "gone.source_pane = PaneId::from_raw(987_654);",
        ),
        (
            "crates/tako-core/src/workspace.rs",
            "let ghost = crate::unused_id::tab();",
            "let ghost = TabId::from_raw(9999);",
        ),
        (
            "crates/tako-core/src/pane_tree.rs",
            "let ghost = crate::unused_id::pane();",
            "let ghost = PaneId::from_raw(9_999_999);",
        ),
        (
            "crates/tako-control/tests/issue1679_lsp_diagnostics.rs",
            "host.preview = tako_core::unused_id::pane();",
            "host.preview = PaneId::from_raw(9_999);",
        ),
    ];
    for (rel, now, before) in reverts {
        let src = read(rel);
        let at = src
            .find(now)
            .unwrap_or_else(|| panic!("{rel} に今の行 `{now}` が無い（直した形が変わった）"));
        let line = src[..at].matches('\n').count() + 1;
        let whole = rel.contains("/tests/");
        assert!(
            offenders(&src, rel, whole).is_empty(),
            "{rel} は今の形で緑のはず"
        );
        let reverted = format!("{}{before}{}", &src[..at], &src[at + now.len()..]);
        let found = offenders(&reverted, rel, whole);
        assert!(
            found
                .iter()
                .any(|f| f.starts_with(&format!("{rel}:{line} "))),
            "{rel}:{line} を修正前（`{before}`）へ戻しても名指ししない: {found:#?}"
        );
    }
}
