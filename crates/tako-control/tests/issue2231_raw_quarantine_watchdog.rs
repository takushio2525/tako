//! #2231 / #2238 の番犬: 文字として読めない中身もバイトのまま退避へ移す口を機械で固定する
//!
//! 1. **読み手は退避込みの組み立て口を通る**: 「読めなければ既定値へ落とす」読み手（[`READERS`]。
//!    recent.json / remote/shortcuts.json = #2231、settings.json / run-configs.json /
//!    claude-mod/ui.json = #2238）は、読めなかった理由を `LoadIssue::read_failed`、解釈できなかった
//!    理由を `LoadIssue::unparsable` で組む（どちらも退避まで済ませる。settings と ui.json は解釈の
//!    誤りを自分の形で扱うので `read_failed` だけ）。`LoadIssue::ReadFailed { … }` を直に組むと、
//!    UTF-8 でない中身が退避されずに既定値へ落ち、次の保存が元の中身を消す（#2231 / #2238 の症状）。
//!    理由を直に組んでよいのは正本（`load_issue.rs`）と記録の口（`data_files.rs`）だけ。読み手は
//!    文字として読めるかを自前で判定しない（`String::from_utf8(` でバイトから文字列にすると、
//!    読めなかった中身が `read_failed` を通らない = #2238 以前の ui.json の形）
//! 2. **組み立て口は退避まで済ませる**: `read_failed` は `quarantine_raw` を、`unparsable` は
//!    `quarantine_unreadable` と（写せないときの受け皿の）`quarantine_raw` を呼ぶ
//! 3. **退避の番号を自前で探さない**: 空いている番号を探して rename する実装は
//!    `migration::quarantine_raw` の 1 か所だけ（layout の復元失敗の後始末も同じ口 = #1824）。
//!    番号付きの退避先（`quarantine_slot_path`）を本番コードで直に引くと、満杯時の押し出し・
//!    同じ中身を積まない規則が片方にだけ載る。既知の例外は無い（[`SLOT_KNOWN`] は空。#2238 で
//!    ui.json を寄せた）
//!
//! A/B の旧挙動（`TAKO_<N>_LEGACY=1`）は `fn legacy_…` に閉じ込める決まりで、その本体は 1・3 の
//! 走査から外す（修正前の形をそのまま残して比べるのが A/B の目的）。`legacy_` の fn の外へ
//! 漏れた形は名指す。
//!
//! 走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域とコメントと文字列は見ない）。
//! 落ちるときは **file:line で名指し**する。検出力は [`植えた回帰を名指せる`] と
//! [`本物の読み手へ植えた回帰を名指せる`] で固定する。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::code_view;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 理由と退避の正本
const LOAD_ISSUE: &str = "crates/tako-core/src/load_issue.rs";
/// 退避の番号の正本
const MIGRATION: &str = "crates/tako-core/src/migration.rs";
/// 理由を受け取って記録する口（`match` の型として `ReadFailed { … }` を書く）
const DATA_FILES: &str = "crates/tako-control/src/data_files.rs";

const READ_FAILED: &str = "LoadIssue::read_failed(";
const UNPARSABLE: &str = "LoadIssue::unparsable(";

/// 退避込みの組み立て口を通るべき読み手と、通るべき口
const READERS: &[(&str, &[&str])] = &[
    ("crates/tako-core/src/recent.rs", &[READ_FAILED, UNPARSABLE]),
    (
        "crates/tako-core/src/remote_shortcuts.rs",
        &[READ_FAILED, UNPARSABLE],
    ),
    (
        "crates/tako-control/src/run_configs.rs",
        &[READ_FAILED, UNPARSABLE],
    ),
    // 解釈の誤りは settings の申告（壊れた中身ごとに 1 回 = #1819・理由 = #2230）で扱う
    ("crates/tako-control/src/settings.rs", &[READ_FAILED]),
    // 解釈の誤りは寛容な読み込み（読める部分を使って壊れた部分を退避 = #1960）で扱う
    ("crates/tako-core/src/claude_mod_ui.rs", &[READ_FAILED]),
];

/// 退避を飛ばして理由を直に組む形
const DIRECT: &[&str] = &["ReadFailed {", "Unparsable {"];

/// 読み手が文字として読めるかを自前で判定する形
const SELF_UTF8: &str = "String::from_utf8(";

/// 番号付きの退避先を直に引く形
const SLOT: &str = "quarantine_slot_path(";

/// 番号付きの退避先を直に引いてよい既知の所（空。#2238 で ui.json を寄せた）。増やさない
const SLOT_KNOWN: &[&str] = &[];

/// A/B の旧挙動を閉じ込める fn の頭（本体は走査しない）
const LEGACY_FN: &str = "fn legacy_";

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

/// `{` から対応する `}` までの終わり（`}` の位置）
fn closing_brace(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in code[open..].bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `fn legacy_…` の本体を空白へ（改行は残す = 行番号もバイト長も変わらない）
fn without_legacy_fns(code: &str) -> String {
    let mut bytes = code.as_bytes().to_vec();
    let mut from = 0;
    while let Some(at) = code[from..].find(LEGACY_FN).map(|i| from + i) {
        let Some(open) = code[at..].find('{').map(|i| at + i) else {
            break;
        };
        // 本体の無い宣言（`fn legacy_x();`）は次の fn の本体を消さない
        if code[at..open].contains(';') {
            from = at + LEGACY_FN.len();
            continue;
        }
        let end = closing_brace(code, open).unwrap_or(code.len() - 1);
        for b in &mut bytes[at..=end] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        from = end + 1;
    }
    // 消したのは ASCII の `fn` から `}` までの全体なので、文字の途中で切れていない
    String::from_utf8(bytes).expect("UTF-8 のまま")
}

/// 本番コードのコードだけ（テスト領域・コメント・文字列・`fn legacy_…` の本体を空白へ。
/// バイト長と行番号は原文のまま）
fn code(src: &str) -> String {
    without_legacy_fns(&code_view(&production_range::scan(src).text))
}

/// コードに `needles` が現れた所を `file:line` で
fn code_hits(rel: &str, src: &str, needles: &[&str]) -> Vec<String> {
    let code = code(src);
    let mut out = Vec::new();
    for needle in needles {
        for (at, _) in code.match_indices(needle) {
            out.push(format!("{rel}:{} に {needle}", line_of(&code, at)));
        }
    }
    out.sort();
    out
}

/// `fn <name>(` の本体（`{` から対応する `}` まで）と、その fn の行番号
fn fn_body(rel: &str, src: &str, name: &str) -> (usize, String) {
    let code = code(src);
    let head = format!("fn {name}(");
    let start = code
        .find(&head)
        .unwrap_or_else(|| panic!("{rel} に {head} が無い（改名したならこの番犬も直す）"));
    let open = start + code[start..].find('{').expect("本体がある");
    let end =
        closing_brace(&code, open).unwrap_or_else(|| panic!("{rel} の {head} の本体が閉じない"));
    (line_of(&code, start), code[open..=end].to_string())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 全クレートの `src` の `(相対パス, 中身)`（統合テストの `tests/` は見ない）
fn sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates"))
        .expect("crates を読める")
        .flatten()
    {
        walk(&entry.path().join("src"), &mut files);
    }
    files.sort();
    let out: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(p).unwrap_or_default())
        })
        .collect();
    assert!(
        out.len() > 100,
        "走査が壊れている（*.rs が {} 件しか見つからない）",
        out.len()
    );
    for must in READERS.iter().map(|(rel, _)| rel).chain(SLOT_KNOWN).chain([
        &LOAD_ISSUE,
        &MIGRATION,
        &DATA_FILES,
    ]) {
        assert!(
            out.iter().any(|(rel, _)| rel == must),
            "{must} が見つからない（改名したならこの番犬も直す）"
        );
    }
    out
}

fn source_in<'a>(files: &'a [(String, String)], rel: &str) -> &'a str {
    files
        .iter()
        .find(|(r, _)| r == rel)
        .map(|(_, src)| src.as_str())
        .unwrap_or_else(|| panic!("{rel} が無い"))
}

/// 1 の違反（「口を呼んでいない」と「直に組んだ / 自前で判定した所（file:line）」を 1 つの一覧で。
/// 片方で止めると、直に組んだ行を名指さずに落ちる）
fn reader_findings(files: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    for (rel, needs) in READERS {
        let src = source_in(files, rel);
        for via in *needs {
            if code_hits(rel, src, &[via]).is_empty() {
                found.push(format!("{rel} が {via} を呼んでいない"));
            }
        }
        found.extend(code_hits(rel, src, &[SELF_UTF8]));
    }
    for (rel, src) in files {
        if rel == LOAD_ISSUE || rel == DATA_FILES {
            continue;
        }
        found.extend(code_hits(rel, src, DIRECT));
    }
    found
}

/// 3 の違反（既知の所の数え漏れ = 寄せたのに表に残っている、も名指す）
fn slot_findings(files: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    for (rel, src) in files {
        if rel == MIGRATION {
            continue;
        }
        let hits = code_hits(rel, src, &[SLOT]);
        if SLOT_KNOWN.contains(&rel.as_str()) {
            if hits.is_empty() {
                found.push(format!(
                    "{rel} は {SLOT} を引かなくなった（SLOT_KNOWN から外す）"
                ));
            }
            continue;
        }
        found.extend(hits);
    }
    found
}

#[test]
fn 読み手は退避込みの組み立て口を通る() {
    let found = reader_findings(&sources());
    assert!(
        found.is_empty(),
        "読めなければ既定値へ落とす読み手（recent / shortcuts / settings / run-configs / ui.json）は \
         理由を退避込みの口（`LoadIssue::read_failed` / `LoadIssue::unparsable`）で組み、LoadIssue を \
         直に組まず、UTF-8 かどうかを自前で判定しない（UTF-8 でない中身が退避されず、次の保存が \
         元の中身を消す = #2231 / #2238）: {found:?}"
    );
}

#[test]
fn 組み立て口は退避まで済ませる() {
    let src = std::fs::read_to_string(repo_root().join(LOAD_ISSUE))
        .unwrap_or_else(|e| panic!("{LOAD_ISSUE} を読む: {e}"));
    let mut found = Vec::new();
    for (name, calls) in [
        ("read_failed", &["quarantine_raw("][..]),
        (
            "unparsable",
            &["quarantine_unreadable(", "quarantine_raw("][..],
        ),
    ] {
        let (line, body) = fn_body(LOAD_ISSUE, &src, name);
        for call in calls {
            if !body.contains(call) {
                found.push(format!(
                    "{LOAD_ISSUE}:{line}: fn {name} が {call} を呼んでいない"
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "読めない・解釈できない中身は既定値へ落とす前に退避する（文字として読めない中身は \
         バイトのまま = #2231 / #916）: {found:?}"
    );
}

#[test]
fn 退避の番号を自前で探さない() {
    assert!(
        SLOT_KNOWN.is_empty(),
        "既知の例外は増やさない（寄せる。#2238 で空になった）: {SLOT_KNOWN:?}"
    );
    let found = slot_findings(&sources());
    assert!(
        found.is_empty(),
        "番号付きの退避は migration の口（`quarantine_unreadable` / `quarantine_raw`）へ寄せる \
         （自前で空きを探すと満杯時の押し出し・同じ中身を積まない規則が片方にだけ載る = #2231 / #2238）: \
         {found:?}"
    );
}

/// 検出力: 回帰を植えると file:line で名指す（テストとコメントと文字列と `fn legacy_…` の中は見ない）
#[test]
fn 植えた回帰を名指せる() {
    let planted = "fn load(p: &Path) {\n    let e = read();\n    return LoadIssue::ReadFailed { reason: e, quarantine: None };\n    let free = quarantine_slot_path(p, 2);\n    let t = String::from_utf8(b);\n}\n";
    assert_eq!(
        code_hits("x.rs", planted, DIRECT),
        vec!["x.rs:3 に ReadFailed {".to_string()]
    );
    assert_eq!(
        code_hits("x.rs", planted, &[SLOT]),
        vec!["x.rs:4 に quarantine_slot_path(".to_string()]
    );
    assert_eq!(
        code_hits("x.rs", planted, &[SELF_UTF8]),
        vec!["x.rs:5 に String::from_utf8(".to_string()]
    );
    assert!(
        code_hits("x.rs", planted, &[READ_FAILED, UNPARSABLE]).is_empty(),
        "口を通っていない"
    );

    let skipped = "pub fn read_failed(p: &Path) -> Option<Self> {\n    let _ = \"quarantine_raw(\";\n    // quarantine_raw(p)\n    None\n}\n";
    let (line, body) = fn_body("x.rs", skipped, "read_failed");
    assert_eq!(line, 1);
    assert!(
        !body.contains("quarantine_raw("),
        "文字列とコメントの中の呼び出しは数えない"
    );

    let ignored = "// LoadIssue::ReadFailed { reason } / quarantine_slot_path(p, 1)\nfn f() { let _ = \"ReadFailed {\"; }\n#[cfg(test)]\nmod tests {\n    fn g() { let _ = LoadIssue::ReadFailed { reason, quarantine: None }; quarantine_slot_path(p, 1); }\n}\n";
    assert!(code_hits("x.rs", ignored, DIRECT).is_empty());
    assert!(code_hits("x.rs", ignored, &[SLOT]).is_empty());

    // A/B の旧挙動は `fn legacy_…` の中だけ見ない（外へ漏れたら名指す）
    let legacy = "fn legacy_2238_load(p: &Path) -> Loaded {\n    let t = String::from_utf8(b);\n    if x { let s = quarantine_slot_path(p, 1); }\n}\nfn load(p: &Path) {\n    let s = quarantine_slot_path(p, 1);\n}\n";
    assert_eq!(
        code_hits("x.rs", legacy, &[SLOT, SELF_UTF8]),
        vec!["x.rs:6 に quarantine_slot_path(".to_string()]
    );
    let decl = "trait T {\n    fn legacy_x();\n}\nfn load(p: &Path) {\n    let s = quarantine_slot_path(p, 1);\n}\n";
    assert_eq!(
        code_hits("x.rs", decl, &[SLOT]),
        vec!["x.rs:5 に quarantine_slot_path(".to_string()],
        "本体の無い宣言は後ろの fn を隠さない"
    );
}

/// 検出力（本物）: 実際の読み手へ回帰を植えると、その file:line を名指す
#[test]
fn 本物の読み手へ植えた回帰を名指せる() {
    let base = sources();
    assert!(reader_findings(&base).is_empty() && slot_findings(&base).is_empty());

    // (file, 植える前, 植えた後, 名指すべき違反の頭)
    let cases: &[(&str, &str, &str, &str)] = &[
        // settings: 理由を直に組む（退避を飛ばす）
        (
            "crates/tako-control/src/settings.rs",
            "tako_core::load_issue::LoadIssue::read_failed(path, e)?",
            "Some(tako_core::load_issue::LoadIssue::ReadFailed { reason: String::new(), quarantine: None })?",
            "crates/tako-control/src/settings.rs:",
        ),
        // run-configs: 読めなければ黙って既定値（#2238 以前の形）
        (
            "crates/tako-control/src/run_configs.rs",
            "LoadIssue::read_failed(path, &e)",
            "None",
            "crates/tako-control/src/run_configs.rs が LoadIssue::read_failed( を呼んでいない",
        ),
        // ui.json: UTF-8 を自前で判定して 1 本目へ写す（#2238 以前の形を legacy の外へ戻す）
        (
            "crates/tako-core/src/claude_mod_ui.rs",
            "Err(e) => unreadable_bytes(path, &e),",
            "Err(_) => { let b = std::fs::read(path).unwrap(); let _ = String::from_utf8(b); let _ = crate::migration::quarantine_slot_path(path, 1); unreachable!() }",
            "crates/tako-core/src/claude_mod_ui.rs:",
        ),
    ];
    for (file, from, to, want) in cases {
        let mut injected = base.clone();
        let (_, src) = injected
            .iter_mut()
            .find(|(rel, _)| rel == file)
            .expect("植える先がある");
        assert_eq!(
            src.matches(from).count(),
            1,
            "{file} に植える元 `{from}` が 1 か所だけ無い（直したならこの表も直す）"
        );
        let line = line_of(src, src.find(from).expect("植える元がある"));
        *src = src.replacen(from, to, 1);
        let mut found = reader_findings(&injected);
        found.extend(slot_findings(&injected));
        let named = if want.ends_with(':') {
            format!("{want}{line} に")
        } else {
            want.to_string()
        };
        assert!(
            found.iter().any(|f| f.starts_with(&named)),
            "{file} への植え込み（`{to}`）を `{named}` で名指せない: {found:#?}"
        );
    }
}
