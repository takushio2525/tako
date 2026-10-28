//! 関数の呼び出しグラフを字面から組む（番犬テストの共有部品。#2042）
//!
//! UI スレッドで子プロセスを待たないかの番犬（#1984）は、UI スレッドで走る関数の窓の中に
//! **禁止する口の綴りが直書きされていないか**だけを見ていた。口を別名の関数で包むと
//! （#2017 の `resolve_session_id_for_pane_via_host` = `resolve_session_id_for_backend` を包んだ口）
//! 窓の中には包んだ側の名前しか出ないので見逃す。
//!
//! ここは入口の関数から**呼び出しを辿って到達できる関数**を集める。禁止の口（葉）は
//! 到達した関数の本体で探すので、何段包んでも辿った先で見つかる。
//!
//! 取り込む側は `production_range` を同じ名前で宣言しておくこと（眺めの作り方を共有する）。
//!
//! # 読み方（字面だけ。型は読まない）
//!
//! - 入力は `production_range::scan` の本文（テスト領域を潰した = 行番号が保たれる）を
//!   `code_view` でコメントと文字列も潰した眺め。括弧の対応がコメントや文字列に乱されない
//! - 関数は頭に `fn 名前` を持つ `{` の本体。持ち主（`impl 型` / `trait 型` / 素の関数）は
//!   直に囲むブロックの頭から読む。本体の無い宣言（trait の `fn f(&self);`）は関数にしない
//! - 呼び出しの解決は**確かに当たる形だけ**（誤検知を出さないため）:
//!   - `f(` → 呼び手の中の入れ子の関数 → 同じファイルの素の関数 → `use` で取り込んだ関数。
//!     呼び手が同じ名前を `let` で束ねていればクロージャなので辿らない
//!   - `a::b::f(` → モジュールのパスの素の関数（`crate::` / `self::` / `super::` / クレート名 /
//!     `use` で取り込んだモジュール名から起点を決める。起点が見えないパスは末尾の一致）
//!   - `Type::f(` / `Self::f(` / `self.f(` → 持ち主が `Type`（`Self` / `self` は呼び手の持ち主）の関数 `f`
//!   - それ以外の `x.f(`（受け手の型が字面から決まらないメソッド呼び出し）は**辿らない**。
//!     名前だけで繋ぐと、std と同名のメソッド（`get` / `status`）を 1 つ定義しただけで
//!     無関係な呼び出しが全部そこへ繋がり、変更から遠い場所で誤検知が出る
//! - 辿らない領域（UI スレッドの経路の関数の中でも、そこは製品の既定の UI スレッドでは走らない）。
//!   領域の中では呼び出しも葉も見ない:
//!   - 先取りの外側の腕（`Probed::Outside =>`）: UI スレッドの dispatch は先取りの内側で
//!     走るので、この腕は background と直呼びのときだけ通る（#1984 の読む側の関数の形）
//!   - A/B の旧経路の腕: 名前が `legacy()` で終わる関数**だけ**を条件に置いた
//!     `if …legacy() {` の then 側 / `if !…legacy() {…} else {…}` の else 側 /
//!     match のガード `… if …legacy() =>` の腕。`&&` などで他の条件と組んだものは読まない
//!     （見逃す側へ倒さない）
//!   - 別スレッドへ渡すクロージャ（`thread::spawn(` / `thread::Builder::new()…spawn(` の引数）

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap, VecDeque};

/// 走査する 1 ファイル（コメント・文字列・テスト領域を潰した眺め）
#[derive(Clone)]
pub struct Source {
    /// ワークスペースからの相対パス（名指しに使う）
    pub rel: String,
    /// クレート名（`tako_control` など。パスの先頭のクレート名を照合する）
    pub krate: String,
    /// モジュールのパス（`orchestrator/agent_cli.rs` → `["orchestrator", "agent_cli"]`）
    pub module: Vec<String>,
    /// コードだけの眺め（バイト長・行番号は原文と同じ）
    pub code: String,
    line_starts: Vec<usize>,
    /// `use` で取り込んだ名前 → パス（`use crate::agents::{list_agents as la}` → `la` → `crate::agents::list_agents`）
    imports: HashMap<String, Vec<String>>,
    /// このファイルの関数（`file` は [`Graph::build`] が付け直す）。注入で 1 ファイルだけ
    /// 差し替えたグラフを組み直すとき、他のファイルを読み直さないためにここで持つ
    fns: Vec<FnDef>,
}

impl Source {
    /// `rel` は `crates/<クレート>/src/<…>.rs`。`text` はテスト領域を潰した本文
    pub fn new(rel: &str, text: &str) -> Self {
        let code = crate::production_range::code_view::code_view(text);
        let mut line_starts = vec![0];
        line_starts.extend(code.match_indices('\n').map(|(i, _)| i + 1));
        let (krate, module) = module_of(rel);
        let imports = imports_of(&code);
        let mut source = Self {
            rel: rel.to_string(),
            krate,
            module,
            code,
            line_starts,
            imports,
            fns: Vec::new(),
        };
        source.fns = collect_fns(&source);
        source
    }

    /// このファイルの関数（本体の `{` の順。呼び出しを辿らず、位置を囲む関数だけ引く番犬が使う = #2424）
    pub fn fns(&self) -> &[FnDef] {
        &self.fns
    }

    /// バイト位置の 1 始まりの行番号
    pub fn line_of(&self, at: usize) -> usize {
        match self.line_starts.binary_search(&at) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }
}

/// `crates/tako-core/src/orchestrator/mod.rs` → (`tako_core`, `["orchestrator"]`)
fn module_of(rel: &str) -> (String, Vec<String>) {
    let parts: Vec<&str> = rel.split('/').collect();
    let krate = parts
        .iter()
        .position(|p| *p == "crates")
        .and_then(|i| parts.get(i + 1))
        .map(|c| c.replace('-', "_"))
        .unwrap_or_default();
    let mut module: Vec<String> = parts
        .iter()
        .skip_while(|p| **p != "src")
        .skip(1)
        .map(|p| p.trim_end_matches(".rs").to_string())
        .collect();
    if matches!(
        module.last().map(String::as_str),
        Some("mod") | Some("lib") | Some("main")
    ) {
        module.pop();
    }
    (krate, module)
}

/// ファイルの `use` 宣言（入れ子の `{…}` と `as` を開く。`*` は名前が決まらないので読まない）
fn imports_of(code: &str) -> HashMap<String, Vec<String>> {
    let b = code.as_bytes();
    let mut out = HashMap::new();
    let mut from = 0;
    while let Some(i) = code[from..].find("use") {
        let at = from + i;
        from = at + 3;
        let boundary = at == 0 || !ident_byte(b[at - 1]);
        let spaced = b.get(at + 3).is_some_and(|c| c.is_ascii_whitespace());
        if !boundary || !spaced {
            continue;
        }
        let Some(end) = code[at..].find(';').map(|e| at + e) else {
            break;
        };
        // 改行は空白 1 つへ（`use a::{\n    b,\n};` を 1 行として読む）
        let tree: String = code[at + 3..end]
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        use_tree(&[], tree.trim(), &mut out);
    }
    out
}

/// `a::b::{c, d::e as f}` を `名前 → パス` へ開く
fn use_tree(prefix: &[String], tree: &str, out: &mut HashMap<String, Vec<String>>) {
    let tree = tree.trim();
    if let Some(open) = tree.find('{') {
        let mut base = prefix.to_vec();
        base.extend(path_segments(&tree[..open]));
        let inner = tree[open + 1..].trim_end().trim_end_matches('}');
        // 深さ 0 の `,` で分ける
        let mut depth = 0i32;
        let mut part = String::new();
        for c in inner.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if c == ',' && depth == 0 {
                use_tree(&base, &part, out);
                part.clear();
            } else {
                part.push(c);
            }
        }
        use_tree(&base, &part, out);
        return;
    }
    let (path, alias) = match tree.split_once(" as ") {
        Some((p, a)) => (p, Some(a.trim())),
        None => (tree, None),
    };
    let mut full = prefix.to_vec();
    full.extend(path_segments(path));
    let Some(last) = full.last().cloned() else {
        return;
    };
    if last == "*" || last == "self" {
        return;
    }
    let name = alias.unwrap_or(&last).to_string();
    if name != "_" && name.bytes().all(ident_byte) {
        out.insert(name, full);
    }
}

/// `a :: b ::` → `["a", "b"]`
fn path_segments(path: &str) -> Vec<String> {
    path.split("::")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 関数の持ち主
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// 素の関数（モジュール直下・関数の中の入れ子）
    Free,
    /// `impl 型` / `impl Trait for 型` / `trait 型` の中（型の最後の名前）
    Type(String),
}

/// 1 つの関数定義
#[derive(Debug, Clone)]
pub struct FnDef {
    /// [`Graph::sources`] の添字
    pub file: usize,
    pub name: String,
    /// `fn` の行（1 始まり）
    pub line: usize,
    /// `fn` の位置（バイト位置。ここから本体の `{` までが引数の並び）
    pub head: usize,
    /// 本体の `{` から対応する `}` まで（バイト位置。`}` を含む）
    pub body: (usize, usize),
    pub owner: Owner,
}

/// 呼び出しの形
#[derive(Debug, Clone, PartialEq, Eq)]
enum CallForm {
    /// `f(`
    Plain,
    /// `a::b::f(`（修飾の各段）
    Path(Vec<String>),
    /// `self.f(` / `Self::f(`
    SelfMethod,
    /// `x.f(`（受け手の型が字面から決まらない = 辿らない）
    Method,
}

/// 本体の中の 1 つの呼び出し
#[derive(Debug, Clone)]
struct Call {
    at: usize,
    name: String,
    form: CallForm,
}

/// 到達の記録（どこから来たか）
#[derive(Debug, Clone)]
pub struct Reach {
    /// 呼んだ側の関数（入口は `None`）
    pub from: Option<usize>,
    /// 呼んだ側の本体で呼び出した行（入口は `fn` の行）
    pub call_line: usize,
}

/// 走査範囲全体の関数と呼び出し
pub struct Graph {
    pub sources: Vec<Source>,
    pub fns: Vec<FnDef>,
    by_name: HashMap<String, Vec<usize>>,
    /// ファイルごとの関数の添字の範囲（`fns` はファイル順に並ぶ）
    file_fns: Vec<std::ops::Range<usize>>,
}

/// 識別子の文字
fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// 呼び出しに見えるが関数ではない語
const NOT_CALLS: &[&str] = &[
    "if", "while", "match", "return", "for", "in", "as", "loop", "move", "let", "else", "ref",
    "mut", "where", "impl", "dyn", "fn", "unsafe", "async", "await", "break", "continue",
];

/// `open` の開き括弧（`{` / `(` / `[`）に対応する閉じ括弧の位置
fn matching(code: &[u8], open: usize) -> Option<usize> {
    let (o, c) = match code[open] {
        b'{' => (b'{', b'}'),
        b'(' => (b'(', b')'),
        b'[' => (b'[', b']'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (i, &b) in code.iter().enumerate().skip(open) {
        if b == o {
            depth += 1;
        } else if b == c {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// ブロックの頭（直前の `;` / `{` / `}` から `{` まで）から属性と可視性を外した残り
fn header_core(header: &str) -> &str {
    let mut s = header.trim();
    loop {
        if s.starts_with("#[") || s.starts_with("#![") {
            // 属性は角括弧の対応で外す
            let open = s.find('[').unwrap_or(0);
            match matching(s.as_bytes(), open) {
                Some(end) => s = s[end + 1..].trim_start(),
                None => break,
            }
            continue;
        }
        if let Some(rest) = s.strip_prefix("pub(") {
            match rest.find(')') {
                Some(end) => s = rest[end + 1..].trim_start(),
                None => break,
            }
            continue;
        }
        let kw = ["pub", "unsafe", "default"].into_iter().find(|kw| {
            s.strip_prefix(kw)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        });
        match kw {
            Some(kw) => s = s[kw.len()..].trim_start(),
            None => break,
        }
    }
    s
}

/// `impl<T> Trait for Foo<T>` / `impl Foo` / `trait Foo` の型の最後の名前
fn owner_of(core: &str) -> Option<String> {
    let rest = if let Some(r) = core.strip_prefix("impl") {
        if !(r.starts_with('<') || r.starts_with(char::is_whitespace)) {
            return None;
        }
        let r = r.trim_start();
        // impl の型引数 `<…>` を外す
        let r = if r.starts_with('<') {
            let mut depth = 0i32;
            let mut end = r.len();
            for (i, c) in r.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            &r[end..]
        } else {
            r
        };
        match r.find(" for ") {
            Some(i) => &r[i + " for ".len()..],
            None => r,
        }
    } else {
        core.strip_prefix("trait ")?
    };
    // パスの最後の名前（`crate::host::Foo<'a>` → `Foo`）
    let path: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    let last = path.rsplit("::").next().unwrap_or("").to_string();
    (!last.is_empty()).then_some(last)
}

/// 頭が関数定義なら、その名前と `fn` の位置（`core` の中のバイト位置）
fn fn_name_of(core: &str) -> Option<(String, usize)> {
    let b = core.as_bytes();
    let mut from = 0;
    while let Some(i) = core[from..].find("fn") {
        let at = from + i;
        from = at + 2;
        let before_ok = at == 0 || !ident_byte(b[at - 1]);
        if !before_ok || !b.get(at + 2).is_some_and(|c| c.is_ascii_whitespace()) {
            continue;
        }
        let rest = core[at + 2..].trim_start();
        // 名前は Unicode の識別子として読む（テストの日本語の関数名。`ident_byte` の ASCII だけだと
        // `fn 復元は…()` が関数に見えず、囲みの関数を引く番犬で中の行が関数の外になる = #2424）
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let next = rest[name.len()..].trim_start();
        if !name.is_empty() && (next.starts_with('(') || next.starts_with('<')) {
            return Some((name, at));
        }
    }
    None
}

impl Graph {
    pub fn build(sources: Vec<Source>) -> Self {
        let mut fns = Vec::new();
        let mut file_fns = Vec::new();
        for (fi, src) in sources.iter().enumerate() {
            let start = fns.len();
            fns.extend(src.fns.iter().cloned().map(|f| FnDef { file: fi, ..f }));
            file_fns.push(start..fns.len());
        }
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, f) in fns.iter().enumerate() {
            by_name.entry(f.name.clone()).or_default().push(i);
        }
        Self {
            sources,
            fns,
            by_name,
            file_fns,
        }
    }

    /// ファイル `rel` の関数 `name`（同名が複数なら全部）
    pub fn find(&self, rel: &str, name: &str) -> Vec<usize> {
        self.named(name, &|g| self.sources[g.file].rel == rel)
    }

    /// 関数の本体のうち「UI スレッドの経路として読む」部分（入れ子の関数・辿らない領域を除く）。
    /// `(開始, 終了)` の並び（終了は含まない）
    pub fn live_ranges(&self, f: usize) -> Vec<(usize, usize)> {
        let def = &self.fns[f];
        let code = self.sources[def.file].code.as_bytes();
        let (start, end) = (def.body.0, def.body.1 + 1);
        let mut holes: Vec<(usize, usize)> = self.fns[self.file_fns[def.file].clone()]
            .iter()
            .filter(|g| g.body.0 > def.body.0 && g.body.1 < def.body.1)
            .map(|g| (g.body.0, g.body.1 + 1))
            .collect();
        holes.extend(skipped_regions(code, start, end));
        holes.sort();
        let mut out = Vec::new();
        let mut cursor = start;
        for (s, e) in holes {
            if s > cursor {
                out.push((cursor, s));
            }
            cursor = cursor.max(e);
        }
        if cursor < end {
            out.push((cursor, end));
        }
        out
    }

    /// 関数の本体のうち、入れ子の関数と **A/B の旧経路の腕だけ**を除いた範囲（#1533）。
    ///
    /// [`Self::live_ranges`] と違い、別スレッドへ渡すクロージャ・先取りの外側の腕は**含める**。
    /// 「UI スレッドで走るか」ではなく「製品の既定の経路で走るか」を問う番犬のための眺め
    /// （ログインシェルを上限なしで待つ形は、どのスレッドで待っても呼び手が固まる）
    pub fn product_ranges(&self, f: usize) -> Vec<(usize, usize)> {
        let def = &self.fns[f];
        let code = self.sources[def.file].code.as_bytes();
        let (start, end) = (def.body.0, def.body.1 + 1);
        let mut holes: Vec<(usize, usize)> = self.fns[self.file_fns[def.file].clone()]
            .iter()
            .filter(|g| g.body.0 > def.body.0 && g.body.1 < def.body.1)
            .map(|g| (g.body.0, g.body.1 + 1))
            .collect();
        let text = std::str::from_utf8(&code[start..end]).unwrap_or("");
        for (i, _) in text.match_indices("legacy()") {
            holes.extend(legacy_region(code, start, start + i, end));
        }
        holes.sort();
        let mut out = Vec::new();
        let mut cursor = start;
        for (s, e) in holes {
            if s > cursor {
                out.push((cursor, s));
            }
            cursor = cursor.max(e);
        }
        if cursor < end {
            out.push((cursor, end));
        }
        out
    }

    /// 関数の生きた範囲の呼び出し
    fn calls(&self, f: usize) -> Vec<Call> {
        let def = &self.fns[f];
        let code = self.sources[def.file].code.as_bytes();
        let mut out = Vec::new();
        for (s, e) in self.live_ranges(f) {
            for p in s..e {
                if code[p] == b'(' {
                    out.extend(call_at(code, p));
                }
            }
        }
        out
    }

    /// 関数 `f` が生きた範囲で呼ぶ関数（解決できたものだけ）
    pub fn callees(&self, f: usize) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .calls(f)
            .iter()
            .flat_map(|c| self.resolve(f, c))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// 呼び出しの行き先（確かに当たる形だけ。[`CallForm::Method`] は辿らない）
    fn resolve(&self, caller: usize, call: &Call) -> Vec<usize> {
        let def = &self.fns[caller];
        let src = &self.sources[def.file];
        match &call.form {
            CallForm::Method => Vec::new(),
            CallForm::SelfMethod => match &def.owner {
                Owner::Type(t) => self.named(&call.name, &|g| g.owner == Owner::Type(t.clone())),
                // 入れ子の関数の中の `self.` は外側の持ち主だが、字面から決めない
                Owner::Free => Vec::new(),
            },
            CallForm::Plain => {
                // 呼び手の中で同じ名前を `let` で束ねていれば、それはクロージャ（関数ではない）
                if self.binds_locally(caller, &call.name) {
                    return Vec::new();
                }
                let nested = self.named(&call.name, &|g| {
                    g.file == def.file && g.body.0 > def.body.0 && g.body.1 < def.body.1
                });
                if !nested.is_empty() {
                    return nested;
                }
                let same = self.named(&call.name, &|g| {
                    g.file == def.file && g.owner == Owner::Free
                });
                if !same.is_empty() {
                    return same;
                }
                match src.imports.get(&call.name) {
                    Some(path) if path.len() >= 2 => {
                        let (name, quals) = path.split_last().expect("2 段以上");
                        self.resolve_path(def, quals, name, 0)
                    }
                    _ => Vec::new(),
                }
            }
            CallForm::Path(quals) => self.resolve_path(def, quals, &call.name, 0),
        }
    }

    /// 名前が `name` で `pred` を満たす関数
    fn named(&self, name: &str, pred: &dyn Fn(&FnDef) -> bool) -> Vec<usize> {
        self.by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&i| pred(&self.fns[i]))
            .collect()
    }

    /// 呼び手が `name` をその場で束ねている（`let name` / 引数 `name:` / クロージャの引数 `|name|`）。
    /// そのとき `name(` は同名の関数ではなく、束ねたクロージャ・関数値を呼んでいる
    fn binds_locally(&self, caller: usize, name: &str) -> bool {
        let def = &self.fns[caller];
        let code = &self.sources[def.file].code;
        let body = &code[def.body.0..=def.body.1];
        let sig = &code[def.head..def.body.0];
        // 識別子で始まる綴りは語の頭からの一致に限る（`xlet name` / `my_name:` を拾わない）
        let starts_word = |text: &str, pat: &str, i: usize| {
            !pat.bytes().next().is_some_and(ident_byte)
                || i == 0
                || !ident_byte(text.as_bytes()[i - 1])
        };
        let bound = |text: &str, pat: &str, tail_ok: &dyn Fn(Option<&u8>) -> bool| {
            text.match_indices(pat).any(|(i, _)| {
                starts_word(text, pat, i) && tail_ok(text.as_bytes().get(i + pat.len()))
            })
        };
        let word_end = |c: Option<&u8>| c.is_none_or(|c| !ident_byte(*c));
        ["let ", "let mut "]
            .iter()
            .any(|head| bound(body, &format!("{head}{name}"), &word_end))
            // 引数 `name: T`（`name::Type` のパスは束ねではない）
            || bound(sig, &format!("{name}:"), &|c| c != Some(&b':'))
            // クロージャの先頭の引数（`|name|` / `|name, …|` / `|name: T|` / `|mut name|`）と
            // 最後の引数（`|…, name|`）。途中の `, name,` は普通の引数の並びと区別できないので読まない
            || ["|", "|mut "].iter().any(|head| {
                bound(body, &format!("{head}{name}"), &|c| {
                    matches!(c, Some(b'|') | Some(b',') | Some(b':'))
                })
            })
            || bound(body, &format!(", {name}|"), &|_| true)
    }

    /// `quals::name(` の行き先（`depth` は `use` の展開の段数 = 循環の歯止め）
    fn resolve_path(&self, def: &FnDef, quals: &[String], name: &str, depth: usize) -> Vec<usize> {
        let src = &self.sources[def.file];
        let Some(last) = quals.last() else {
            return Vec::new();
        };
        if last.starts_with(|c: char| c.is_ascii_uppercase()) {
            // `Type::f(` / `Self::f(`（型の前の段は見ない = 型の名前で照合する）
            let ty = if last == "Self" {
                match &def.owner {
                    Owner::Type(t) => t.clone(),
                    Owner::Free => return Vec::new(),
                }
            } else {
                last.clone()
            };
            return self.named(name, &|g| g.owner == Owner::Type(ty.clone()));
        }
        // モジュールのパス。先頭の段で起点（クレートとモジュール）を決める
        match quals[0].as_str() {
            "crate" => self.in_module(Some(&src.krate), &quals[1..], name, true),
            "self" | "super" => {
                let mut module = src.module.clone();
                let mut rest = quals;
                while let Some((seg, tail)) = rest.split_first() {
                    match seg.as_str() {
                        "self" => {}
                        "super" => {
                            module.pop();
                        }
                        _ => break,
                    }
                    rest = tail;
                }
                module.extend(rest.iter().cloned());
                self.in_module(Some(&src.krate), &module, name, true)
            }
            k if k.starts_with("tako_") => self.in_module(Some(k), &quals[1..], name, true),
            first => match src.imports.get(first) {
                // `use` で取り込んだモジュール名（`use crate::orchestrator;` → `orchestrator::f(`）
                Some(path) if depth < 4 => {
                    let mut expanded = path.clone();
                    expanded.extend(quals[1..].iter().cloned());
                    self.resolve_path(def, &expanded, name, depth + 1)
                }
                Some(_) => Vec::new(),
                // 起点の見えないパス（子モジュール・再公開）は末尾の一致で照合する
                None => self.in_module(None, quals, name, false),
            },
        }
    }

    /// モジュール `module` の素の関数 `name`（`exact` でなければ末尾の一致）
    fn in_module(
        &self,
        krate: Option<&str>,
        module: &[String],
        name: &str,
        exact: bool,
    ) -> Vec<usize> {
        self.named(name, &|g| {
            let s = &self.sources[g.file];
            if g.owner != Owner::Free || krate.is_some_and(|k| s.krate != k) {
                return false;
            }
            if exact {
                s.module == module
            } else {
                s.module.ends_with(module)
            }
        })
    }

    /// 入口から到達できる関数（入口を含む）と、どこから来たか（幅優先 = 最短の道筋）。
    ///
    /// `barrier(呼び手, 行き先)` が真の辺は辿らない（集約点・既知の残り）
    pub fn reach(
        &self,
        entries: &[usize],
        barrier: &dyn Fn(usize, usize) -> bool,
    ) -> HashMap<usize, Reach> {
        let mut seen: HashMap<usize, Reach> = HashMap::new();
        let mut queue = VecDeque::new();
        for &e in entries {
            seen.entry(e).or_insert(Reach {
                from: None,
                call_line: self.fns[e].line,
            });
            queue.push_back(e);
        }
        while let Some(f) = queue.pop_front() {
            let src = &self.sources[self.fns[f].file];
            for call in self.calls(f) {
                for g in self.resolve(f, &call) {
                    if g == f || seen.contains_key(&g) || barrier(f, g) {
                        continue;
                    }
                    seen.insert(
                        g,
                        Reach {
                            from: Some(f),
                            call_line: src.line_of(call.at),
                        },
                    );
                    queue.push_back(g);
                }
            }
        }
        seen
    }

    /// 入口から `f` までの道筋（入口が先頭。各段は「関数」と「前の段の本体で呼んだ行」）
    pub fn path_to(&self, reach: &HashMap<usize, Reach>, f: usize) -> Vec<(usize, Option<usize>)> {
        let mut steps = Vec::new();
        let mut cur = Some(f);
        while let Some(c) = cur {
            let r = &reach[&c];
            steps.push((c, r.from.map(|_| r.call_line)));
            cur = r.from;
        }
        steps.reverse();
        steps
    }

    /// 関数の生きた範囲で `needle` を含む行（1 始まり）
    pub fn hits(&self, f: usize, needle: &str) -> Vec<usize> {
        let def = &self.fns[f];
        let src = &self.sources[def.file];
        let mut lines = BTreeSet::new();
        for (s, e) in self.live_ranges(f) {
            for (i, _) in src.code[s..e].match_indices(needle) {
                lines.insert(src.line_of(s + i));
            }
        }
        lines.into_iter().collect()
    }
}

/// 1 ファイルの関数定義を集める（`{` ごとに頭を読み、囲むブロックの積み上げを保つ。`file` は仮の 0）
fn collect_fns(src: &Source) -> Vec<FnDef> {
    let code = src.code.as_bytes();
    let mut out = Vec::new();
    // 積み上げ: impl / trait のブロックならその型（関数の本体・その他のブロックは `None`）
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut last_delim = 0usize;
    for (i, &b) in code.iter().enumerate() {
        match b {
            b'{' => {
                let header = &src.code[last_delim..i];
                let core = header_core(header);
                // `core` は `header` の部分文字列（前後の空白と属性・可視性を外しただけ）
                let core_at = last_delim + (core.as_ptr() as usize - header.as_ptr() as usize);
                let fn_head = fn_name_of(core);
                if let Some((name, off)) = &fn_head {
                    let owner = match stack.last() {
                        // 直に囲むのが impl / trait ならその型。関数の中の入れ子・モジュール直下は素の関数
                        Some(Some(t)) => Owner::Type(t.clone()),
                        _ => Owner::Free,
                    };
                    out.push(FnDef {
                        file: 0,
                        name: name.clone(),
                        line: src.line_of(core_at + off),
                        head: core_at + off,
                        body: (i, matching(code, i).unwrap_or(code.len() - 1)),
                        owner,
                    });
                }
                stack.push(if fn_head.is_some() {
                    None
                } else {
                    owner_of(core)
                });
                last_delim = i + 1;
            }
            b'}' => {
                stack.pop();
                last_delim = i + 1;
            }
            b';' => last_delim = i + 1,
            _ => {}
        }
    }
    out
}

/// `(` の位置 `p` が呼び出しなら、その名前と形
fn call_at(code: &[u8], p: usize) -> Option<Call> {
    // ターボフィッシュ `f::<T>(` は `::<…>` を飛ばす
    let mut end = p;
    if end > 0 && code[end - 1] == b'>' {
        let mut depth = 0i32;
        let mut k = end;
        while k > 0 {
            k -= 1;
            match code[k] {
                b'>' => depth += 1,
                b'<' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        if !code[..k].ends_with(b"::") {
            return None;
        }
        end = k - 2;
    }
    let mut start = end;
    while start > 0 && ident_byte(code[start - 1]) {
        start -= 1;
    }
    if start == end || code[start].is_ascii_digit() {
        return None;
    }
    let name = std::str::from_utf8(&code[start..end]).ok()?.to_string();
    if NOT_CALLS.contains(&name.as_str()) {
        return None;
    }
    // 直前（空白・改行を飛ばす。メソッドの鎖は行をまたぐ）
    let mut k = start;
    while k > 0 && code[k - 1].is_ascii_whitespace() {
        k -= 1;
    }
    if k > 0 && code[k - 1] == b'.' {
        // `self.f(` だけは持ち主が決まる（`x.self` は無いので直前が `.` でない `self`）
        let mut r = k - 1;
        while r > 0 && code[r - 1].is_ascii_whitespace() {
            r -= 1;
        }
        let mut rs = r;
        while rs > 0 && ident_byte(code[rs - 1]) {
            rs -= 1;
        }
        let on_self = &code[rs..r] == b"self" && (rs == 0 || code[rs - 1] != b'.');
        let form = if on_self {
            CallForm::SelfMethod
        } else {
            CallForm::Method
        };
        return Some(Call {
            at: start,
            name,
            form,
        });
    }
    if code[..start].ends_with(b"::") {
        if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            // `Enum::Variant(`
            return None;
        }
        // 修飾を前へ辿る（`a::b::f`）
        let mut quals = Vec::new();
        let mut q = start - 2;
        loop {
            let mut qs = q;
            while qs > 0 && ident_byte(code[qs - 1]) {
                qs -= 1;
            }
            if qs == q {
                // `<T as Trait>::f` / `Vec::<u8>::new` などは読まない
                return None;
            }
            quals.push(std::str::from_utf8(&code[qs..q]).ok()?.to_string());
            if code[..qs].ends_with(b"::") {
                q = qs - 2;
            } else {
                break;
            }
        }
        quals.reverse();
        return Some(Call {
            at: start,
            name,
            form: CallForm::Path(quals),
        });
    }
    // 定義の頭（`fn f(`）は呼び出しではない
    let mut d = k;
    while d > 0 && ident_byte(code[d - 1]) {
        d -= 1;
    }
    if &code[d..k] == b"fn" || name.starts_with(|c: char| c.is_ascii_uppercase()) {
        // 大文字はタプル構造体・列挙子（`Some(` / `Ok(` / `Foo(`）
        return None;
    }
    Some(Call {
        at: start,
        name,
        form: CallForm::Plain,
    })
}

/// 本体 `[from, to)` の中の辿らない領域（先取りの外側の腕・A/B の旧経路の腕・別スレッドのクロージャ）
fn skipped_regions(code: &[u8], from: usize, to: usize) -> Vec<(usize, usize)> {
    let text = std::str::from_utf8(&code[from..to]).unwrap_or("");
    let mut out = Vec::new();
    for (i, _) in text.match_indices("Probed::Outside") {
        let at = from + i;
        if let Some(arrow) = find_from(code, at, to, b"=>") {
            out.push((at, arm_end(code, arrow + 2, to)));
        }
    }
    for (i, _) in text.match_indices("legacy()") {
        out.extend(legacy_region(code, from, from + i, to));
    }
    for head in ["thread::spawn(", "thread::Builder::new()"] {
        for (i, _) in text.match_indices(head) {
            let Some(s) = find_from(code, from + i, to, b"spawn(") else {
                continue;
            };
            let open = s + "spawn".len();
            if let Some(close) = matching(code, open) {
                out.push((open, close + 1));
            }
        }
    }
    out
}

/// `[at, to)` で最初に `pat` が現れる位置
fn find_from(code: &[u8], at: usize, to: usize, pat: &[u8]) -> Option<usize> {
    (at..to.saturating_sub(pat.len() - 1)).find(|&k| code[k..].starts_with(pat))
}

/// match の腕の終わり（`=>` の直後 `from` から。ブロックなら対応する `}` の後、
/// 式なら深さ 0 の `,` の後か、閉じ括弧の手前）
fn arm_end(code: &[u8], from: usize, to: usize) -> usize {
    let mut k = from;
    while k < to && code[k].is_ascii_whitespace() {
        k += 1;
    }
    if k < to && code[k] == b'{' {
        return matching(code, k).map(|e| e + 1).unwrap_or(to);
    }
    let mut depth = 0i32;
    while k < to {
        match code[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return k;
                }
                depth -= 1;
            }
            b',' if depth == 0 => return k + 1,
            _ => {}
        }
        k += 1;
    }
    to
}

/// `legacy()` の呼び出し（`at` は `legacy()` の位置）が A/B の分岐の条件なら、旧経路の腕の範囲
fn legacy_region(code: &[u8], from: usize, at: usize, to: usize) -> Option<(usize, usize)> {
    // 呼び出しの頭（`tako_core::probe::issue1984_legacy` の先頭）と、直前の `!`
    let mut s = at;
    while s > from && (ident_byte(code[s - 1]) || code[s - 1] == b':') {
        s -= 1;
    }
    let mut k = s;
    while k > from && code[k - 1].is_ascii_whitespace() {
        k -= 1;
    }
    let negated = k > from && code[k - 1] == b'!';
    let mut h = if negated { k - 1 } else { s };
    while h > from && code[h - 1].is_ascii_whitespace() {
        h -= 1;
    }
    // 条件の直前の語が `if` であること（`a && legacy()` のような組み合わせは読まない）
    let is_if = code[from..h].ends_with(b"if") && (h < from + 3 || !ident_byte(code[h - 3]));
    if !is_if {
        return None;
    }
    // 条件の直後: `{`（if 式）か `=>`（match のガード）
    let mut e = at + "legacy()".len();
    while e < to && code[e].is_ascii_whitespace() {
        e += 1;
    }
    if e < to && code[e] == b'{' {
        let then_end = matching(code, e)?;
        if !negated {
            return Some((e, then_end + 1));
        }
        // `if !…legacy() { 新 } else { 旧 }` の else 側（`else if` は読まない）
        let mut x = then_end + 1;
        while x < to && code[x].is_ascii_whitespace() {
            x += 1;
        }
        if !code[x..to].starts_with(b"else") {
            return None;
        }
        let mut y = x + "else".len();
        while y < to && code[y].is_ascii_whitespace() {
            y += 1;
        }
        if y < to && code[y] == b'{' {
            return Some((y, matching(code, y)? + 1));
        }
        return None;
    }
    if !negated && code[e..to].starts_with(b"=>") {
        return Some((e, arm_end(code, e + 2, to)));
    }
    None
}
