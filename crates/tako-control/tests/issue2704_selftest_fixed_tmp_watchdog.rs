//! **#2704 の番犬**: セルフテスト / visual-test の節は、一時 dir / ファイルを固定名で作らない。
//! 節の一時の置き場は終了の後片付けに載る。
//!
//! ## なぜ縛るのか
//!
//! セルフテストの項目 60（`tako-osc-e2e`）と項目 122（`tako-st915-a` / `-b` / `-x`）は、pid の入らない固定名の
//! 一時 dir を作り、成功・FAILED・SIGTERM のどの出口でも残していた（#2686 の段 1 で実測）。名前に pid が無いので
//! 持ち主を確かめて消せず（#2686 の線引き = 自分の pid の名前だけを消す）、同じ `$TMPDIR` で並列に走る別の
//! セルフテストと同じ名前を取り合う。いまは節の一時の置き場を pid 入りの名前で作って `remove_on_fail`（#2051）で
//! 登録し、その登録簿が `tako_core::scoped_dirs::claim` にも載る = 終了の後片付け `cleanup_process_scoped` が
//! `remove_claimed` でどの出口でも消す。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. **固定名で作らない**: セルフテストのコード（`main.rs` の `mod self_test {` の中と `src/self_test/*.rs`）で、
//!    `temp_dir()` に `.join(` する文（`temp_dir()` を束ねた変数への `.join(` を含む）は名前に pid
//!    （`std::process::id()` / `pid`）を入れる。A/B の枝など意図した固定名は、その行か直前 [`ALLOW_ABOVE`] 行に
//!    `tako:fixed-tmp-ok <理由>` を書く
//! 2. **片付けに載せる**: 登録の口 `remove_on_fail` が `scoped_dirs::claim(` を呼ぶ・終了の後片付け
//!    `cleanup_process_scoped` が `scoped_dirs::remove_claimed(` を呼ぶ
//! 3. **持ち主を確かめる**: `claim` は覚える前に・`remove_claimed` は消す前に `names_pid(`（名前に自分の pid が
//!    入るか）を見る

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const SELF_TEST_DIR: &str = "crates/tako-app/src/self_test";
const SCOPED: &str = "crates/tako-core/src/scoped_dirs.rs";
const ALLOW_MARKER: &str = "tako:fixed-tmp-ok";
const ALLOW_ABOVE: usize = 2;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

#[path = "common/production_range.rs"]
mod production_range;

fn raw(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す = 行番号はずれない）
fn code(rel: &str, src: &str) -> String {
    let production = if rel == MAIN {
        production_range::production(src, rel)
    } else {
        production_range::scan(src).text
    };
    production_range::code_view::without_comments(&production)
}

fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> usize {
    let view = production_range::code_view_of(src);
    let bytes = view.as_bytes();
    let start = view[open..]
        .find('{')
        .map(|i| open + i)
        .expect("`{` がある");
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("対応する `}}` が無い");
}

fn fn_span(src: &str, sig: &str, rel: &str) -> (usize, usize) {
    let at = src.find(sig).unwrap_or_else(|| {
        panic!("{rel} に `{sig}` が無い（構造が変わったらこの番犬も見直すこと）")
    });
    (at, block_end(src, at))
}

/// `at` を含む文（直前の `;` / `{` / `}` の後から次の `;` まで）の範囲
fn statement_span(src: &str, at: usize) -> (usize, usize) {
    let start = src[..at].rfind([';', '{', '}']).map_or(0, |i| i + 1);
    let end = src[at..].find(';').map_or(src.len(), |i| at + i);
    (start, end)
}

fn names_pid(stmt: &str) -> bool {
    stmt.contains("process::id()")
        || stmt
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| w == "pid")
}

/// 意図した固定名の目印（その行か直前 [`ALLOW_ABOVE`] 行の生のコメント）
fn allowed(raw: &str, line: usize) -> bool {
    let lines: Vec<&str> = raw.lines().collect();
    let from = line.saturating_sub(ALLOW_ABOVE + 1);
    lines[from..line.min(lines.len())]
        .iter()
        .any(|l| l.contains(ALLOW_MARKER))
}

/// セルフテストのコードの範囲（ファイル・走査する眺め・生の本文・範囲）
fn self_test_sources() -> Vec<(String, String, String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let raw_main = raw(MAIN);
    let main = code(MAIN, &raw_main);
    let start = main
        .find("\nmod self_test {\n")
        .map(|i| i + 1)
        .unwrap_or_else(|| {
            panic!("{MAIN} に `mod self_test {{` が無い = この番犬は何も見ていない")
        });
    let end = block_end(&main, start);
    out.push((MAIN.to_string(), main, raw_main, start..end));
    let mut files: Vec<PathBuf> = std::fs::read_dir(repo_root().join(SELF_TEST_DIR))
        .expect("self_test の dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 10,
        "{SELF_TEST_DIR} の .rs が少なすぎる: {}",
        files.len()
    );
    for path in files {
        let rel = format!(
            "{SELF_TEST_DIR}/{}",
            path.file_name().unwrap().to_string_lossy()
        );
        let raw_src = raw(&rel);
        let view = code(&rel, &raw_src);
        let len = view.len();
        out.push((rel, view, raw_src, 0..len));
    }
    out
}

/// 固定名で一時の置き場を作る文を file:line で返す
fn fixed_tmp_offenders(
    rel: &str,
    view: &str,
    raw_src: &str,
    range: std::ops::Range<usize>,
) -> Vec<String> {
    let mut out = Vec::new();
    let body = &view[range.clone()];
    // `temp_dir()` を束ねた変数（`let tmp = …temp_dir()…;` で `.join(` の無いもの）
    let mut tmp_vars: Vec<String> = Vec::new();
    for (i, _) in body.match_indices("temp_dir()") {
        let at = range.start + i;
        let (s, e) = statement_span(view, at);
        let stmt = &view[s..e];
        if stmt.contains(".join(") {
            let line = line_at(view, at);
            if !names_pid(stmt) && !allowed(raw_src, line) {
                out.push(format!(
                    "{rel}:{line} がセルフテストの一時の置き場を固定名で作っている（{}）。名前に pid を入れる\
                     （`format!(\"…-{{}}\", std::process::id())`）= 並列の別のセルフテストと取り合わず、終了の後片付けが\
                     持ち主を確かめて消せる。意図した固定名は `// {ALLOW_MARKER} <理由>`",
                    stmt.split_whitespace().collect::<Vec<_>>().join(" ")
                ));
            }
        } else if let Some(name) = stmt
            .trim_start()
            .strip_prefix("let ")
            .and_then(|r| r.trim_start_matches("mut ").split([' ', ':', '=']).next())
        {
            if !name.is_empty() {
                tmp_vars.push(name.to_string());
            }
        }
    }
    for var in tmp_vars {
        let needle = format!("{var}.join(");
        for (i, _) in body.match_indices(&needle) {
            let at = range.start + i;
            // 変数名の頭が別の識別子の続きでないこと（`raw_tmp.join(` を `tmp` と読まない）
            if view[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            let (s, e) = statement_span(view, at);
            let stmt = &view[s..e];
            let line = line_at(view, at);
            if !names_pid(stmt) && !allowed(raw_src, line) {
                out.push(format!(
                    "{rel}:{line} がセルフテストの一時の置き場（`temp_dir()` を束ねた `{var}`）を固定名で作っている（{}）。\
                     名前に pid を入れる。意図した固定名は `// {ALLOW_MARKER} <理由>`",
                    stmt.split_whitespace().collect::<Vec<_>>().join(" ")
                ));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ 1. 固定名で作らない

#[test]
fn issue2704_セルフテストの節は一時の置き場を固定名で作らない() {
    let mut out = Vec::new();
    let mut seen = 0usize;
    for (rel, view, raw_src, range) in self_test_sources() {
        seen += view[range.clone()].matches("temp_dir()").count();
        out.extend(fixed_tmp_offenders(&rel, &view, &raw_src, range));
    }
    assert!(
        seen >= 50,
        "走査が空振りしている（`temp_dir()` が {seen} か所しか見えない）"
    );
    assert!(out.is_empty(), "#2704:\n{}", out.join("\n"));
}

/// 走査そのものの確かめ: 固定名の形（直に・束ねた変数で）を名指し、pid 入り・目印つきは通す
#[test]
fn issue2704_固定名の形を名指し_pid_入りと目印つきは通す() {
    let src = "mod self_test {\n\
        fn a() {\n\
            let d = std::env::temp_dir().join(\"tako-osc-e2e\");\n\
            let ok = std::env::temp_dir().join(format!(\"tako-x-{}\", std::process::id()));\n\
            let tmp = canonicalize(&std::env::temp_dir());\n\
            let raw = tmp.join(\"tako-fixed\");\n\
            let raw2 = tmp.join(format!(\"tako-y-{pid}\"));\n\
            // tako:fixed-tmp-ok A/B\n\
            let legacy = std::env::temp_dir().join(\"tako-old\");\n\
        }\n\
    }\n";
    let got = fixed_tmp_offenders("x.rs", src, src, 0..src.len());
    assert_eq!(got.len(), 2, "{got:#?}");
    assert!(got[0].starts_with("x.rs:3 "), "{}", got[0]);
    assert!(got[1].starts_with("x.rs:6 "), "{}", got[1]);
}

// ------------------------------------------------------------------ 2. 片付けに載せる

#[test]
fn issue2704_節の登録簿は終了の後片付けに載る() {
    let mut out = Vec::new();
    let raw_main = raw(MAIN);
    let main = code(MAIN, &raw_main);
    for (sig, call, what) in [
        (
            "pub(crate) fn remove_on_fail(",
            "tako_core::scoped_dirs::claim(",
            "節が作る一時の置き場の登録（`remove_on_fail`）が終了の後片付けへ載らない = 成功・SIGTERM・visual-test の\
             出口で、節の末尾まで届かなかった物が残る",
        ),
        (
            "fn cleanup_process_scoped(",
            "tako_core::scoped_dirs::remove_claimed(",
            "終了の後片付けが節の一時の置き場を消さない = 成功・FAILED・SIGTERM のどの出口でも残る",
        ),
    ] {
        let (start, end) = fn_span(&main, sig, MAIN);
        if !main[start..end].contains(call) {
            out.push(format!(
                "{MAIN}:{} の `{}` が `{call}` を呼んでいない: {what}（#2704）",
                line_at(&main, start),
                sig.trim_end_matches('(')
            ));
        }
    }
    assert!(out.is_empty(), "#2704:\n{}", out.join("\n"));
}

// ------------------------------------------------------------------ 3. 持ち主を確かめる

#[test]
fn issue2704_覚える口と消す口は名前に自分の_pid_が入るかを見る() {
    let raw_src = raw(SCOPED);
    let src = code(SCOPED, &raw_src);
    let mut out = Vec::new();
    for (sig, act) in [
        ("pub fn claim(", ".push("),
        ("pub fn remove_claimed(", "remove_path("),
    ] {
        let (start, end) = fn_span(&src, sig, SCOPED);
        let body = &src[start..end];
        let guard = body.find("names_pid(");
        let act_at = body.find(act);
        if act_at.is_none_or(|a| guard.is_none_or(|g| g > a)) {
            out.push(format!(
                "{SCOPED}:{} の `{}` が `{act}` の前に `names_pid(`（名前に自分の pid が入るか）を見ていない = 固定名・\
                 他人の置き場まで覚えて消す（#2704）",
                line_at(&src, start),
                sig.trim_end_matches('(')
            ));
        }
    }
    assert!(out.is_empty(), "#2704:\n{}", out.join("\n"));
}
