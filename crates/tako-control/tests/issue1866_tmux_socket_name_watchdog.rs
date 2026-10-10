//! 番犬: tako-core の実 tmux テストが**同じ器の名前を 2 本で使わない**（Issue #1866）・
//! テストの器が**残骸掃除の拾う名前**で、**利用者の設定を読まずに**起きる（Issue #1874）
//!
//! tako-core の lib テストは 1 つのプロセスで並列に走り、実 tmux のテストは
//! `TmuxTestGuard::new(vec![<器の名前>…])` で器を 1 本ずつ持つ（Drop で `kill-server` と
//! ソケットファイルの除去を行う）。名前がテスト同士で重なると、先に終わった側の Drop が
//! **隣のテストの生きている器を畳み、ソケットファイルまで消す**。残された側の tmux 呼び出しは
//! `error connecting to …(No such file or directory)` になり、kill-session が黙って届かず
//! 「セッションの終わりでクライアントも終わる」で落ちる（#1866 の
//! `tmux_backend::tests::issue1857_…` の落ち方と同じ形。ソケットファイルだけを消す注入で
//! 同じ行・同じ文言を再現した）。
//!
//! 今は全部一意。テストをコピーして増やしたときに器の名前の付け替えを忘れる形を、
//! **両方の file:line** を名指しして落とす。
//!
//! 見る形: `TmuxTestGuard::new(vec![a.clone(), b.clone()])` の各変数について、同じ関数の
//! 手前にある `let a = format!("<書式>", …)` の書式文字列。書式には pid（`{}`）が
//! 入っていること（入っていないと別プロセスの全体テストと取り合う = #1300 の型）。
//!
//! ## #1874: 残骸掃除が拾う名前・`-f` つきの起動
//!
//! テストの器は、テストプロセスが途中で殺されると Drop が走らずに残る（中のシェルや
//! `sleep` ごと生き続ける）。残った器を回収するのは**名前で判定する掃除**だけなので、
//! 名前が掃除の拾う形から外れると誰にも回収されない。
//!
//! | 掃除 | 拾う名前 | 見張る範囲 |
//! |---|---|---|
//! | `TmuxTestGuard` の残骸掃除（tako-core の lib テストの起動ごとに 1 回） | `tmux_backend.rs` の `TEST_SOCKET_PREFIXES` のどれかで始まる | `crates/tako-core/src/` のテスト |
//! | `tako tmux cleanup --servers`（製品） | `tako-` で始まる | それ以外のテスト |
//!
//! どちらも所有者を**名前の pid**（`-` 区切りで数字だけの区画）で見分けるので、pid は
//! 1 区画まるごと（`…-{}` / `…-{}-<用途>`）で入れる。#1874 の時点で外れていたのは
//! `tako-coretest1857-<pid>`（接頭辞の `-` 抜け）・`ct1105-<pid>`（`tako` で始めないのは
//! #1105 の検査の中身なので `tk-coretest-` へ寄せた）・固定名の `tako-e2e-571` /
//! `tako-e2e-577`（製品の掃除は `571` を所有 pid と読み違える）。
//!
//! もう 1 つは `-f`。tmux は `-f` を渡さずに器を起こすと**利用者の `~/.tmux.conf`** を読む
//! （prefix・status・フック・`run-shell` までテストの器で動く）。修正前の main を、読まれたら
//! 器の名前を記録する `.tmux.conf` を置いた偽の HOME で走らせると、tmux_e2e 経由の器・
//! scrollback_capture・remote_scrollback・dispatch の 3 本・#1857・`loc` が読んでいた。
//! テストが自分で `new-session` を叩くときは `-f /dev/null` を渡す（製品の経路
//! `wrap_options` はバックエンドの conf を `-f` で渡すので、テスト側に `new-session` の字面が無い）。
//!
//! 見る形（どちらも**テスト領域だけ**。`src` は `production_range::tests_only` で切る）:
//! **#1918 で tako-app のセルフテストも足した**（`main.rs` の `mod self_test { … }` と
//! `src/self_test/` 配下）。セルフテストは `cfg(test)` ではなく本番バイナリに入るので
//! `tests_only` では見えず、`-f` なしの器が 6 か所残っていた: 字面の `"new-session"` が
//! 4 か所（48 / 68 / 73 / 103）と、ペインのシェルへ `tmux -L <器> new-session …` を
//! **打ち込む**形が 2 か所（1d = 固定名 `takoST` / 61f）。後者は `"new-session"` の字面が
//! 無いので、文字列の中の `-L … new-session` を別の規則（[`shell_new_sessions`]）で見る。
//! 103（#772）と 61f は**そのインスタンスの backend のソケット**へ器を立てるので、`/dev/null`
//! ではなく製品と同じ conf（`tmux_backend::ensure_conf`）を `-f` で渡す（そこで初めて起きた
//! サーバーを後続のペインも使うため）。規則はどれも「器を起こすコマンドに `-f` がある」
//!
//! - 名前: 器の名前を受け取る書き方（[`NAME_SLOTS`] / [`NAME_SECOND_ARG`] /
//!   `TmuxTestGuard::new(vec![…])`）へ渡した変数を、手前の `let x = format!("…", …)` /
//!   `const X: &str = "…"` まで辿る
//! - `-f`: `"new-session"` を含む文に `"-f"` があるか。文の中の変数が手前で `"-f"` を含む
//!   式に束縛されていてもよい（tmux / psmux 共用の e2e が `let conf = &["-f", …]` で渡す形）
//!
//! psmux（Windows）の e2e も #1918 から同じ規則で見る（それまでは `-f` の効き方が未実測で
//! 除外していた。CI の Windows ランナーで v3.3.7 / v3.3.8 が `-f NUL` で `~/.tmux.conf` を
//! 読まないことを実測した = `.agent/conventions.md` の #1874 / #1918 の節）。
//!
//! 対象外:
//!
//! | 形 | 例 | なぜ対象外か |
//! |---|---|---|
//! | 既存のセッションへのグループ（`new-session -t`） | `new-session -d -t orig -s view` | 相手のセッションがある = 器は手前で起きている |
//! | 引数の検査（`for` / 器を叩かない `assert`） | `for forbidden in ["new-session", …]` | 器を起こさない |
//! | 名前を辿れない式 | 関数の引数・`self.socket`・`socket_for("1259")` | 定義側で見る（`socket_for` は `tmux_e2e_watchdog.rs`） |

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;
use production_range::code_view::{code_view, without_comments};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 器 1 本の宣言（書式と、`let` の行番号 = 1 始まり）
#[derive(Debug, PartialEq, Eq)]
struct Socket {
    template: String,
    line: usize,
}

/// 名前を辿れなかった `TmuxTestGuard::new` の行（形が崩れた = 番犬が見られない）
#[derive(Debug, PartialEq, Eq)]
struct Untraced {
    var: String,
    line: usize,
}

/// `TmuxTestGuard::new(vec![…])` へ渡した変数を、手前の `let <変数> = format!("…"` まで辿る
fn guarded_sockets(src: &str) -> (Vec<Socket>, Vec<Untraced>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut found = Vec::new();
    let mut untraced = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let Some(at) = line.find("TmuxTestGuard::new(vec![") else {
            continue;
        };
        let args = &line[at + "TmuxTestGuard::new(vec![".len()..];
        let args = args.split(']').next().unwrap_or("");
        for var in args
            .split(',')
            .map(|a| a.trim().trim_end_matches(".clone()").trim())
            .filter(|a| !a.is_empty())
        {
            let needle = format!("let {var} = format!(\"");
            // 同じ関数の手前（テスト 1 本の中）だけを見る
            let traced = (0..i).rev().take(80).find_map(|j| {
                let at = lines[j].find(&needle)?;
                let rest = &lines[j][at + needle.len()..];
                let end = rest.find('"')?;
                Some(Socket {
                    template: rest[..end].to_string(),
                    line: j + 1,
                })
            });
            match traced {
                Some(socket) => found.push(socket),
                None => untraced.push(Untraced {
                    var: var.to_string(),
                    line: i + 1,
                }),
            }
        }
    }
    (found, untraced)
}

#[test]
fn tako_coreの実tmuxテストは器の名前を重ねない() {
    let root = repo_root();
    let mut files = Vec::new();
    collect(&root.join("crates/tako-core/src"), &mut files);
    files.sort();

    let mut by_template: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut problems: Vec<String> = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let (sockets, untraced) = guarded_sockets(&src);
        for u in untraced {
            problems.push(format!(
                "  {rel}:{} `TmuxTestGuard::new` に渡した `{}` の名前を辿れない\
                 （手前に `let {} = format!(\"…\", std::process::id())` の形で置くこと）",
                u.line, u.var, u.var
            ));
        }
        for s in sockets {
            if !s.template.contains("{}") {
                problems.push(format!(
                    "  {rel}:{} 器の名前 `{}` に pid（`{{}}`）が無い（別プロセスと取り合う = #1300）",
                    s.line, s.template
                ));
            }
            by_template
                .entry(s.template)
                .or_default()
                .push(format!("{rel}:{}", s.line));
        }
    }
    for (template, places) in &by_template {
        if places.len() > 1 {
            problems.push(format!(
                "  器の名前 `{template}` を {} 本のテストが使っている（先に終わった側の Drop が \
                 隣の器を畳む）: {}",
                places.len(),
                places.join(" / ")
            ));
        }
    }
    // 走査そのものが空振りしていない（形が変わって 1 本も拾えないと、緑のまま見張りが消える）
    assert!(
        by_template.len() >= 20,
        "TmuxTestGuard の器を {} 本しか拾えない（走査の形が実装とずれている）",
        by_template.len()
    );
    assert!(
        problems.is_empty(),
        "tako-core の実 tmux テストの器の名前が重なっている・辿れない（#1866）:\n{}",
        problems.join("\n")
    );
}

// ---- #1874: 残骸掃除が拾う名前・`-f` つきの起動 ----

/// この番犬自身（説明文と固定テストの断片に違反の形を書く）
const SELF_FILE: &str = "crates/tako-control/tests/issue1866_tmux_socket_name_watchdog.rs";

/// psmux（Windows）の e2e。#1918 までは `-f` の規則から外していた（効き方が未実測だった）
const PSMUX_E2E: &str = "crates/tako-core/tests/psmux_backend.rs";

/// 器の名前を受け取る書き方（この直後の式が `-L` へ渡る名前）
const NAME_SLOTS: &[&str] = &[
    "\"-L\",",
    "\"-L\".into(),",
    ".arg(\"-L\").arg(",
    "tmux_command(Some(",
    "run_tmux(Some(",
    "\"TAKO_TMUX_SOCKET\",",
];

/// 2 番目の引数が器の名前になる製品の関数（`-L` は製品側が足す）
const NAME_SECOND_ARG: &[&str] = &["wrap_options(", "reattach_options("];

/// 走査する 1 ファイルの**テスト領域だけ**（本番コードは空白。行番号は原文と同じ）
struct TestSource {
    /// リポジトリルートからの相対パス
    rel: String,
    /// コメントだけ潰した眺め（文字列リテラルの中身を読む）
    lit: String,
    /// コメントと文字列を潰した眺め（括弧と `;` を数える）
    code: String,
}

impl TestSource {
    fn new(rel: &str, text: &str) -> Self {
        Self {
            rel: rel.to_string(),
            lit: without_comments(text),
            code: code_view(text),
        }
    }
}

/// tako-app のセルフテストの置き場（#1918）。セルフテストは `cfg(test)` ではない
/// （本番バイナリに入り `TAKO_SELF_TEST` で走る）ので `tests_only` では空白に潰れるが、
/// 器を自分で起こすのはテストと同じなので同じ規則で見る
const SELF_TEST_MAIN: &str = "crates/tako-app/src/main.rs";
/// `main.rs` の中のセルフテスト本体（0 桁目の `mod self_test {` から 0 桁目の `}` まで）
const SELF_TEST_MOD: &str = "mod self_test {";
/// セルフテストから `mod` で切り出したファイルの置き場（丸ごとセルフテスト）
const SELF_TEST_DIR: &str = "crates/tako-app/src/self_test/";

/// `mod self_test { … }` のバイト範囲（行頭から閉じ括弧の行末まで）
fn self_test_range(src: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    let mut start = None;
    for line in src.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        match start {
            None if body == SELF_TEST_MOD => start = Some(offset),
            Some(s) if body == "}" => return Some((s, offset + line.len())),
            _ => {}
        }
        offset += line.len();
    }
    None
}

/// `main.rs` のテスト領域に、セルフテストの範囲の原文を重ねた本文（行番号は原文と同じ）
fn with_self_test(src: &str, tests_only: String) -> String {
    let Some((start, end)) = self_test_range(src) else {
        return tests_only;
    };
    let mut text = tests_only;
    // `tests_only` は改行以外を空白へ置き換えるだけなのでバイト位置が一致する
    text.replace_range(start..end, &src[start..end]);
    text
}

/// 全クレートの `src`（テスト領域）と `tests`（丸ごと）と、tako-app のセルフテスト
fn test_sources(root: &Path) -> Vec<TestSource> {
    let mut files = Vec::new();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates を読める");
    for krate in crates.flatten() {
        collect(&krate.path().join("src"), &mut files);
        collect(&krate.path().join("tests"), &mut files);
    }
    files.sort();
    files
        .iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
                .replace('\\', "/");
            if rel == SELF_FILE {
                return None;
            }
            let src = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
            // `mod tests;` で切り出したファイル（`…/tests.rs`）と `tests/` 配下は丸ごとテスト。
            // セルフテストから切り出したファイルも丸ごと（#1918）
            let whole = rel.contains("/tests/")
                || rel.starts_with(SELF_TEST_DIR)
                || path.file_name().is_some_and(|n| n == "tests.rs");
            let text = if whole {
                src
            } else if rel == SELF_TEST_MAIN {
                with_self_test(&src, production_range::tests_only(&src))
            } else {
                production_range::tests_only(&src)
            };
            Some(TestSource::new(&rel, &text))
        })
        .collect()
}

/// `TmuxTestGuard` の残骸掃除が拾う接頭辞（正本は `tmux_backend.rs` の
/// `TEST_SOCKET_PREFIXES`。`#[cfg(test)]` の定数は tako-control から見えないのでソースから読む）
fn guard_prefixes(root: &Path) -> Vec<String> {
    let path = root.join("crates/tako-core/src/tmux_backend.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読める: {e}", path.display()));
    let decl = src
        .split("const TEST_SOCKET_PREFIXES: &[&str] = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .expect(
            "tmux_backend.rs に `const TEST_SOCKET_PREFIXES: &[&str] = …;` が無い\
             （残骸掃除の接頭辞の正本。改名したらこの番犬も追う）",
        );
    let prefixes: Vec<String> = decl
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert!(
        prefixes.iter().any(|p| p == "tako-coretest-"),
        "TEST_SOCKET_PREFIXES を読めない（`tako-coretest-` が無い）: {prefixes:?}"
    );
    prefixes
}

/// バイト位置の行番号（1 始まり）
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// `offset` から `lines` 行さかのぼった行頭
fn lines_back(text: &str, offset: usize, lines: usize) -> usize {
    text[..offset]
        .rmatch_indices('\n')
        .nth(lines)
        .map(|(at, _)| at + 1)
        .unwrap_or(0)
}

/// `at` から始まる式の終わり（深さ 0 の `,` / `;` / 閉じ括弧の位置）
fn expr_end(code: &[u8], at: usize) -> usize {
    let mut depth = 0usize;
    let mut i = at;
    while i < code.len() {
        match code[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            b',' | b';' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    i
}

/// `&socket` / `socket.clone()` / `E2E_SOCKET` のような**変数 1 つ**の式なら、その名前
fn plain_ident(expr: &str) -> Option<&str> {
    let mut e = expr.trim();
    e = e.strip_prefix('&').unwrap_or(e).trim();
    loop {
        let before = e;
        for suffix in [
            ".clone()",
            ".as_str()",
            ".to_string()",
            ".as_ref()",
            ".into()",
        ] {
            e = e.strip_suffix(suffix).unwrap_or(e);
        }
        if e == before {
            break;
        }
    }
    let mut chars = e.chars();
    let head = chars.next()?;
    ((head.is_ascii_alphabetic() || head == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(e)
}

/// 器の名前として `-L` へ渡る変数と、その位置
fn name_uses(src: &TestSource) -> Vec<(String, usize)> {
    let code = src.code.as_bytes();
    let mut out = Vec::new();
    let push = |start: usize, end: usize, out: &mut Vec<(String, usize)>| {
        if let Some(ident) = plain_ident(&src.lit[start..end]) {
            out.push((ident.to_string(), start));
        }
    };
    for slot in NAME_SLOTS {
        for (at, _) in src.lit.match_indices(slot) {
            let start = at + slot.len();
            push(start, expr_end(code, start), &mut out);
        }
    }
    for func in NAME_SECOND_ARG {
        for (at, _) in src.lit.match_indices(func) {
            let first_end = expr_end(code, at + func.len());
            if code.get(first_end) == Some(&b',') {
                push(first_end + 1, expr_end(code, first_end + 1), &mut out);
            }
        }
    }
    const GUARD: &str = "TmuxTestGuard::new(vec![";
    for (at, _) in src.lit.match_indices(GUARD) {
        let mut start = at + GUARD.len();
        loop {
            let end = expr_end(code, start);
            push(start, end, &mut out);
            if code.get(end) != Some(&b',') {
                break;
            }
            start = end + 1;
        }
    }
    out
}

/// 器の名前の定義
#[derive(Debug, PartialEq, Eq)]
struct NameDef {
    line: usize,
    /// `format!` の書式か、固定名の文字列
    template: String,
    /// 書式の引数に `std::process::id()` があるか
    pid_arg: bool,
}

/// `ident` の定義を、`before` の手前（同じテストの中 = 200 行以内）の `let` か、
/// ファイルの `const` まで辿る。`format!` と文字列リテラル以外の初期化
/// （`socket_for("1259")` など）は辿らない
fn name_definition(src: &TestSource, ident: &str, before: usize) -> Option<NameDef> {
    let lit = &src.lit;
    let window = lines_back(lit, before, 200);
    let lets = [
        format!("let {ident} = "),
        format!("let mut {ident} = "),
        format!("let {ident}: String = "),
    ];
    let (at, len) = lets
        .iter()
        .filter_map(|p| {
            lit[window..before]
                .rfind(p.as_str())
                .map(|at| (window + at, p.len()))
        })
        .max_by_key(|&(at, _)| at)
        .or_else(|| {
            let p = format!("const {ident}: &str = ");
            lit.find(&p).map(|at| (at, p.len()))
        })?;
    let init = &lit[at + len..];
    let line = line_of(lit, at);
    if let Some(rest) = init.strip_prefix("format!(\"") {
        let end = expr_end(src.code.as_bytes(), at + len);
        Some(NameDef {
            line,
            template: rest[..rest.find('"')?].to_string(),
            pid_arg: lit[at + len..end].contains("process::id()"),
        })
    } else {
        let rest = init.strip_prefix('"')?;
        Some(NameDef {
            line,
            template: rest[..rest.find('"')?].to_string(),
            pid_arg: false,
        })
    }
}

/// 名前が残骸掃除に拾われない理由（拾われるなら `None`）
fn name_problem(def: &NameDef, prefixes: &[String]) -> Option<String> {
    let base = def.template.trim_end_matches('=');
    if !prefixes.iter().any(|p| base.starts_with(p.as_str())) {
        let names: Vec<String> = prefixes.iter().map(|p| format!("`{p}`")).collect();
        return Some(format!("接頭辞が {} のどれでもない", names.join(" / ")));
    }
    if !def.pid_arg || !base.split('-').any(|seg| seg == "{}") {
        return Some(
            "pid（`std::process::id()`）が `-` 区切りの 1 区画で入っていない（所有者を名前から読めない）"
                .to_string(),
        );
    }
    None
}

#[test]
fn テストの器の名前は残骸掃除が拾える() {
    let root = repo_root();
    let guard = guard_prefixes(&root);
    let product = vec!["tako-".to_string()];
    let mut checked = 0usize;
    let mut problems: BTreeSet<String> = BTreeSet::new();
    for src in test_sources(&root) {
        let prefixes = if src.rel.starts_with("crates/tako-core/src/") {
            &guard
        } else {
            &product
        };
        let mut seen = BTreeSet::new();
        for (ident, at) in name_uses(&src) {
            let Some(def) = name_definition(&src, &ident, at) else {
                continue;
            };
            if !seen.insert(def.line) {
                continue;
            }
            checked += 1;
            if let Some(why) = name_problem(&def, prefixes) {
                problems.insert(format!(
                    "  {}:{} 器の名前 `{}`: {why}",
                    src.rel, def.line, def.template
                ));
            }
        }
    }
    // 走査そのものが空振りしていない（形が変わって拾えなくなると、緑のまま見張りが消える）
    assert!(
        checked >= 30,
        "テストの器の名前を {checked} 本しか辿れない（走査の形が実装とずれている）"
    );
    assert!(
        problems.is_empty(),
        "テストの器の名前が残骸掃除の拾う形から外れている（#1874。テストプロセスが途中で\
         殺されると器が誰にも回収されない。tako-core の lib テストは `TEST_SOCKET_PREFIXES`、\
         それ以外は `tako-` で始め、pid を `-` 区切りの 1 区画で入れる）:\n{}",
        problems.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// `at` を含む文の範囲 `[start, end)`。区切りは文の終わりの `;` と、文を囲むブロックの
/// `{` / `}`（`code` の眺めで数えるので、文字列の中の `;` / `{` には当たらない）
fn statement_around(code: &[u8], at: usize) -> (usize, usize) {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut i = at;
    while i > 0 {
        i -= 1;
        match code[i] {
            b')' | b']' => depth += 1,
            b'(' | b'[' => depth = depth.saturating_sub(1),
            b'{' | b'}' if depth == 0 => {
                start = i + 1;
                break;
            }
            b'{' => depth -= 1,
            b'}' => depth += 1,
            b';' if depth == 0 => {
                start = i + 1;
                break;
            }
            _ => {}
        }
    }
    let mut depth = 0usize;
    // 文を囲む括弧を出たか（出た後の `{` はブロックの始まり = 文の終わり）
    let mut exited = false;
    let mut j = at;
    let end = loop {
        let Some(&c) = code.get(j) else {
            break code.len();
        };
        match c {
            b'(' | b'[' => depth += 1,
            b'{' if depth == 0 && exited => break j,
            b'{' => depth += 1,
            b')' | b']' if depth == 0 => exited = true,
            b')' | b']' => depth -= 1,
            b'}' if depth == 0 => break j,
            b'}' => depth -= 1,
            b';' if depth == 0 => break j + 1,
            _ => {}
        }
        j += 1;
    };
    (start, end)
}

/// 器を叩かない文（引数の検査）か
fn is_inspection(stmt: &str) -> bool {
    const RUNS: &[&str] = &[
        ".args(",
        ".arg(",
        ".raw(",
        ".output()",
        ".status()",
        ".spawn()",
        "run(",
        "tmux(",
        "run_tmux(",
        "new_session(",
    ];
    let t = stmt.trim_start();
    t.starts_with("for ") || (t.contains("assert") && !RUNS.iter().any(|r| t.contains(r)))
}

/// 文の中の変数が、手前（40 行以内）で `"-f"` を含む式に束縛されているか
/// （`let conf: &[&str] = match … { Tmux => &["-f", "/dev/null"], _ => &[] };` を
/// `run(bin, &[…, conf, &["new-session", …]].concat())` で使う形）
fn bound_conf(src: &TestSource, start: usize, stmt: &str) -> bool {
    let window = lines_back(&src.lit, start, 40);
    let before = &src.lit[window..start];
    stmt.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
        .any(|ident| {
            [format!("let {ident} = "), format!("let {ident}: ")]
                .iter()
                .any(|p| {
                    before.rfind(p.as_str()).is_some_and(|at| {
                        let from = window + at;
                        let (_, end) = statement_around(src.code.as_bytes(), from + 4);
                        src.lit[from..end].contains("\"-f\"")
                    })
                })
        })
}

/// `"new-session"` の位置 `at` の文が `-f` なしで器を起こしていれば、その文の頭の 1 行
fn missing_conf(src: &TestSource, at: usize) -> Option<String> {
    let (start, end) = statement_around(src.code.as_bytes(), at);
    let stmt = &src.lit[start..end];
    if stmt.contains("\"-f\"")
        || stmt.contains("\"-t\"")
        || is_inspection(stmt)
        || bound_conf(src, start, stmt)
    {
        return None;
    }
    let head = stmt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    Some(head.to_string())
}

/// シェルの 1 コマンドの区切り（`||` は `|` で切れる）
const SHELL_SEPARATORS: &[&str] = &["&&", ";", "|"];

/// 文字列に埋めたシェルのコマンドで器を起こす `new-session` の位置と、`-f` があるか（#1918）。
///
/// セルフテストはペインのシェルへ `tmux -L <器> new-session …` を**打ち込んで**器を立てる
/// ことがあり、`"new-session"` の字面の規則では見えない（1d が固定名 `takoST` + `-f` なしの
/// まま残っていた）。文字列の中の `new-session` のうち、同じコマンドの手前に `-L` がある
/// （= 器を指している）ものだけを数える（「tmux new-session が失敗した」のような文言は
/// `-L` が無いので当たらない）。`-t`（既存のセッションへのグループ）は器を起こさないので数えない
fn shell_new_sessions(src: &TestSource) -> Vec<(usize, bool)> {
    let code = src.code.as_bytes();
    let mut out = Vec::new();
    for (at, _) in src.lit.match_indices("new-session") {
        // `"new-session"` の字面は [`missing_conf`] が見る。文字列の外（`code` で残る所）は見ない
        if src.lit[..at].ends_with('"') || code.get(at) != Some(&b' ') {
            continue;
        }
        let line_start = src.lit[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = src.lit[at..].find('\n').map_or(src.lit.len(), |i| at + i);
        let before = &src.lit[line_start..at];
        let cmd_start = SHELL_SEPARATORS
            .iter()
            .filter_map(|s| before.rfind(s).map(|i| i + s.len()))
            .max()
            .unwrap_or(0);
        let cmd = &before[cmd_start..];
        let Some(l_at) = cmd.find("-L ") else {
            continue;
        };
        let after = &src.lit[at..line_end];
        let after_end = SHELL_SEPARATORS
            .iter()
            .filter_map(|s| after.find(s))
            .min()
            .unwrap_or(after.len());
        if after[..after_end].contains(" -t ") {
            continue;
        }
        out.push((at, cmd[l_at..].contains(" -f ")));
    }
    out
}

/// `-f` の規則に当たる `"new-session"` の数・文字列に埋めたシェルの `new-session` の数・違反
fn conf_problems(sources: &[TestSource]) -> (usize, usize, Vec<String>) {
    let mut seen = 0usize;
    let mut shell_seen = 0usize;
    let mut problems = Vec::new();
    for src in sources {
        for (at, _) in src.lit.match_indices("\"new-session\"") {
            seen += 1;
            if let Some(head) = missing_conf(src, at) {
                problems.push(format!(
                    "  {}:{} `-f` なしで器を起こしている（利用者の `~/.tmux.conf` を読む）: {head}",
                    src.rel,
                    line_of(&src.lit, at)
                ));
            }
        }
        for (at, has_conf) in shell_new_sessions(src) {
            shell_seen += 1;
            if !has_conf {
                let line = line_of(&src.lit, at);
                let head = src.lit.lines().nth(line - 1).unwrap_or("").trim();
                problems.push(format!(
                    "  {}:{line} シェルへ打ち込むコマンドが `-f` なしで器を起こしている\
                     （`-L <器>` と `new-session` の間に `-f` が無い）: {head}",
                    src.rel
                ));
            }
        }
    }
    (seen, shell_seen, problems)
}

#[test]
fn テストが起こす器は利用者の設定を読まない() {
    let root = repo_root();
    let sources = test_sources(&root);
    // psmux の e2e も見ている（#1918 で除外を外した。改名で黙って外れない）
    assert!(
        sources
            .iter()
            .any(|s| s.rel == PSMUX_E2E && s.lit.contains("\"new-session\"")),
        "{PSMUX_E2E} の `\"new-session\"` を走査していない（改名したらこの番犬も追う）"
    );
    let (seen, _, problems) = conf_problems(&sources);
    assert!(
        seen >= 15,
        "テストの `\"new-session\"` を {seen} 個しか拾えない（走査の形が実装とずれている）"
    );
    // セルフテストの器も見ている（`mod self_test` の改名・移動で、緑のまま見張りが消えない。
    // #1918 の時点で字面が 4 か所 = 48 / 68 / 73 / 103、シェルへ打ち込む形が 2 か所 = 1d / 61f）
    let self_test = sources
        .iter()
        .find(|s| s.rel == SELF_TEST_MAIN)
        .expect("tako-app の main.rs を走査している");
    let in_self_test = self_test.lit.matches("\"new-session\"").count();
    assert!(
        in_self_test >= 4,
        "{SELF_TEST_MAIN} のセルフテスト（`{SELF_TEST_MOD}`）から `\"new-session\"` を \
         {in_self_test} 個しか拾えない（範囲の切り出しが実装とずれている）"
    );
    let shell_in_self_test = shell_new_sessions(self_test).len();
    assert!(
        shell_in_self_test >= 2,
        "{SELF_TEST_MAIN} のセルフテストからシェルへ打ち込む `tmux -L … new-session` を \
         {shell_in_self_test} 個しか拾えない（走査の形が実装とずれている）"
    );
    assert!(
        problems.is_empty(),
        "テストが `-f` なしで tmux の器を起こしている（#1874。`-f /dev/null` を渡して利用者の \
         `~/.tmux.conf` を読ませない）:\n{}",
        problems.join("\n")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 渡した変数を手前のformatまで辿る() {
        let src = r#"
    fn a() {
        let backend = format!("tako-coretest-nestw-{}", std::process::id());
        let nested = format!("tako-coretest-nestw-in-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![backend.clone(), nested.clone()]);
    }
"#;
        let (found, untraced) = guarded_sockets(src);
        assert!(untraced.is_empty(), "{untraced:?}");
        assert_eq!(
            found,
            vec![
                Socket {
                    template: "tako-coretest-nestw-{}".into(),
                    line: 3
                },
                Socket {
                    template: "tako-coretest-nestw-in-{}".into(),
                    line: 4
                },
            ]
        );
    }

    #[test]
    fn 辿れない名前とコメントの行() {
        let src = r#"
    // TmuxTestGuard::new(vec![socket.clone()]) は説明文
    fn b(socket: String) {
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
    }
"#;
        let (found, untraced) = guarded_sockets(src);
        assert!(found.is_empty());
        assert_eq!(
            untraced,
            vec![Untraced {
                var: "socket".into(),
                line: 4
            }]
        );
    }

    #[test]
    fn セルフテストの範囲だけを原文へ戻す() {
        let src = "fn product() {\n    run(&[\"new-session\"]);\n}\n\
                   mod self_test {\n    fn a() {\n        run(&[\"new-session\"]);\n    }\n}\n\
                   fn after() {}\n";
        let (start, end) = self_test_range(src).expect("範囲を拾う");
        assert!(src[start..].starts_with(SELF_TEST_MOD));
        assert!(src[..end].ends_with("    }\n}\n"), "0 桁目の `}}` で閉じる");
        // 本番コードは空白のまま、セルフテストだけが読める（行番号は変わらない）
        let blanked: String = src
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        let merged = with_self_test(src, blanked);
        assert_eq!(merged.lines().count(), src.lines().count());
        assert_eq!(merged.matches("\"new-session\"").count(), 1);
        assert_eq!(
            merged.lines().nth(5).map(str::trim),
            Some("run(&[\"new-session\"]);")
        );
        // セルフテストが無ければ何も足さない
        assert!(self_test_range("fn a() {}\n").is_none());
    }

    /// 断片の `"new-session"` の違反行（行番号だけ）
    fn conf_lines(text: &str) -> Vec<usize> {
        let src = TestSource::new("crates/x/tests/fixture.rs", text);
        src.lit
            .match_indices("\"new-session\"")
            .filter(|(at, _)| missing_conf(&src, *at).is_some())
            .map(|(at, _)| line_of(&src.lit, at))
            .collect()
    }

    #[test]
    fn f無しのnew_sessionを文ごとに名指しする() {
        let text = r##"
fn a(socket: &str) {
    let _ = run_tmux(Some(&socket), &["new-session", "-d", "-s", "x"]);
    let _ = run_tmux(Some(&socket), &["-f", "/dev/null", "new-session", "-d"]);
    tmux(&["list-windows", "-F", "#{window_index};{x}"]);
    tmux(&["new-session", "-d"]);
    let mut full = vec!["new-session"];
}
"##;
        // 1 つ目と、`-f` つきの文の後ろの 2 つ（文字列の中の `;` / `{` で文を取り違えない）
        assert_eq!(conf_lines(text), vec![3, 6, 7]);
    }

    #[test]
    fn シェルへ打ち込むnew_sessionは器を指すものだけ数えfを見る() {
        let text = r#"
fn a(sock: &str, bin: &str) {
    type_text("if command -v tmux; then tmux -L takoST kill-server 2>/dev/null; \
               tmux -L takoST new-session -d 'sleep 5' && echo OK; fi");
    type_text(&format!("tmux -L {sock} -f /dev/null new-session -d && tmux -L {sock} kill-server"));
    assert!(ok, "tmux new-session が失敗した: 実行: tmux -L {sock} {args}");
    type_text(&format!("{bin} -L {sock} new-session -d -t orig -s view"));
    let new_session = run(&["new-session"]);
}
"#;
        let src = TestSource::new("crates/x/tests/fixture.rs", text);
        let found: Vec<(usize, bool)> = shell_new_sessions(&src)
            .into_iter()
            .map(|(at, conf)| (line_of(&src.lit, at), conf))
            .collect();
        // 継続行の `-L takoST new-session`（`-f` なし）と、`-f` つきの 1 本だけ。
        // 文言・グループ化（`-t`）・`"new-session"` の字面・識別子は数えない
        assert_eq!(found, vec![(4, false), (5, true)]);
    }

    #[test]
    fn グループ化と引数の検査とfを束縛した変数は対象外() {
        let text = r#"
fn a(bin: &str, socket: &str, args: Vec<String>) {
    run(&["new-session", "-d", "-t", orig, "-s", view]);
    for forbidden in ["new-session", "-A", "-D"] {
        assert!(!args.iter().any(|a| a == forbidden), "{forbidden}");
    }
    assert!(args.contains(&"new-session".to_string()));
    let conf: &[&str] = match binary() {
        Binary::Tmux { .. } => &["-f", "/dev/null"],
        _ => &[],
    };
    let sized = run(bin, &[&["-L", socket][..], conf, &["new-session", "-d"]].concat());
    const NEW_SESSION: [&str; 3] = ["-f", "/dev/null", "new-session"];
}
"#;
        assert_eq!(conf_lines(text), Vec::<usize>::new());
        // 束縛が無ければ同じ形でも落とす（束縛の判定が素通しになっていない）
        let unbound = r#"
fn a(bin: &str, socket: &str, conf: &[&str]) {
    let sized = run(bin, &[&["-L", socket][..], conf, &["new-session", "-d"]].concat());
}
"#;
        assert_eq!(conf_lines(unbound), vec![3]);
    }

    /// 断片の器の名前の違反（定義の行と理由の有無）
    fn name_lines(rel: &str, text: &str, prefixes: &[&str]) -> Vec<(usize, bool)> {
        let src = TestSource::new(rel, text);
        let prefixes: Vec<String> = prefixes.iter().map(|p| p.to_string()).collect();
        let mut out: Vec<(usize, bool)> = name_uses(&src)
            .into_iter()
            .filter_map(|(ident, at)| name_definition(&src, &ident, at))
            .map(|def| (def.line, name_problem(&def, &prefixes).is_some()))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn 器の名前を定義まで辿り接頭辞とpidを見る() {
        let text = r#"
fn a() {
    let socket = format!("tako-coretest1857-{}", std::process::id());
    let _g = TmuxTestGuard::new(vec![socket.clone()]);
}
fn b() {
    let socket = format!("tk-coretest-1105-{}", std::process::id());
    let _g = TmuxTestGuard::new(vec![socket.clone()]);
    let s = wrap_options(base.clone(), &socket, session);
}
fn c() {
    let socket = format!("tako-coretest-{}-grouped", std::process::id());
    let out = crate::tmux::tmux_command(Some(&socket)).args(["-f", "/dev/null"]);
}
"#;
        let guard = ["tako-coretest-", "tk-coretest-"];
        // 3 行目だけが接頭辞から外れる（`-` 抜け）。7 行目は `tk-coretest-`、12 行目は pid が途中
        assert_eq!(
            name_lines("crates/tako-core/src/x.rs", text, &guard),
            vec![(3, true), (7, false), (12, false)]
        );
    }

    #[test]
    fn 固定名と辿れない名前() {
        // 断片は #1300 の番犬（`tmux_e2e_watchdog.rs`）の形（`Command::new("tmux")` と
        // `"tako-e2e-<数字>"`）に当たらない綴りで書く（向こうは tests/ 直下を走査する）
        let text = r#"
    const FIXED_SOCKET: &str = "tako-fixed-571";
    fn a(bin: &str) {
        let _ = Command::new(bin).args(["-L", FIXED_SOCKET, "kill-server"]).output();
        std::env::set_var("TAKO_TMUX_SOCKET", FIXED_SOCKET);
    }
    fn run(bin: &str, socket: &str) {
        let _ = Command::new(bin).args(["-L", socket, "kill-server"]).output();
    }
    fn b() {
        let socket = format!("tako-972test-{}-{tag}", std::process::id());
        let _ = Command::new(&bin).args(["-L", &socket, "-f", "/dev/null", "new-session"]);
        let other = format!("ct1105-{}", std::process::id());
        let _ = Command::new(&bin).arg("-L").arg(&other).output();
    }
"#;
        // 固定名（pid なし）と `tako-` で始まらない名前を落とし、引数（`socket`）は辿らない
        assert_eq!(
            name_lines("crates/tako-control/tests/x.rs", text, &["tako-"]),
            vec![(2, true), (11, false), (13, true)]
        );
    }
}
