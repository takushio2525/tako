//! #2231 の番犬: 文字として読めない中身もバイトのまま退避へ移す口を機械で固定する
//!
//! 1. **読み手は退避込みの組み立て口を通る**: `recent.json` / `remote/shortcuts.json` の core の
//!    読み手は、読めなかった理由を `LoadIssue::read_failed`、解釈できなかった理由を
//!    `LoadIssue::unparsable` で組む（どちらも退避まで済ませる）。`LoadIssue::ReadFailed { … }` を
//!    直に組むと、UTF-8 でない中身が退避されずに既定値へ落ち、次の保存が元の中身を消す
//!    （#2231 の症状）。理由を直に組んでよいのは正本（`load_issue.rs`）だけ
//! 2. **組み立て口は退避まで済ませる**: `read_failed` は `quarantine_raw` を、`unparsable` は
//!    `quarantine_unreadable` と（写せないときの受け皿の）`quarantine_raw` を呼ぶ
//! 3. **退避の番号を自前で探さない**: 空いている番号を探して rename する実装は
//!    `migration::quarantine_raw` の 1 か所だけ（layout の復元失敗の後始末も同じ口 = #1824）。
//!    番号付きの退避先（`quarantine_slot_path`）を本番コードで直に引くと、満杯時の押し出し・
//!    同じ中身を積まない規則が片方にだけ載る
//!
//! 走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域とコメントと文字列は見ない）。
//! 落ちるときは **file:line で名指し**する。検出力は [`植えた回帰を名指せる`] で固定する。

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

/// 退避込みの組み立て口を通るべき読み手
const READERS: &[&str] = &[
    "crates/tako-core/src/recent.rs",
    "crates/tako-core/src/remote_shortcuts.rs",
];
const VIA: &[&str] = &["LoadIssue::read_failed(", "LoadIssue::unparsable("];

/// 退避を飛ばして理由を直に組む形
const DIRECT: &[&str] = &["ReadFailed {", "Unparsable {"];

/// 番号付きの退避先を直に引く形
const SLOT: &str = "quarantine_slot_path(";

/// 番号付きの退避先を直に引いてよい既存の 1 か所（#2231 の範囲外）。
/// ui.json（#1960）の UTF-8 でない中身の写しで、1 本目が既にあると 2 回目の中身を写さない。
/// 寄せるときはここから外す（増やさない）
const SLOT_KNOWN: &[&str] = &["crates/tako-core/src/claude_mod_ui.rs"];

fn line_of(text: &str, byte: usize) -> usize {
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

/// 本番コードのコードだけ（テスト領域・コメント・文字列を空白へ。バイト長と行番号は原文のまま）
fn code(src: &str) -> String {
    code_view(&production_range::scan(src).text)
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
    let mut depth = 0usize;
    for (i, b) in code[open..].bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (line_of(&code, start), code[open..=open + i].to_string());
                }
            }
            _ => {}
        }
    }
    panic!("{rel} の {head} の本体が閉じない");
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
    for must in READERS
        .iter()
        .chain(SLOT_KNOWN)
        .chain([&LOAD_ISSUE, &MIGRATION, &DATA_FILES])
    {
        assert!(
            out.iter().any(|(rel, _)| rel == must),
            "{must} が見つからない（改名したならこの番犬も直す）"
        );
    }
    out
}

fn source(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

#[test]
fn 読み手は退避込みの組み立て口を通る() {
    // 「口を呼んでいない」と「直に組んだ所（file:line）」を 1 つの一覧で出す
    // （片方で止めると、直に組んだ行を名指さずに落ちる）
    let mut found = Vec::new();
    for rel in READERS {
        let src = source(rel);
        for via in VIA {
            if code_hits(rel, &src, &[via]).is_empty() {
                found.push(format!("{rel} が {via} を呼んでいない"));
            }
        }
    }
    for (rel, src) in sources() {
        if rel == LOAD_ISSUE || rel == DATA_FILES {
            continue;
        }
        found.extend(code_hits(&rel, &src, DIRECT));
    }
    assert!(
        found.is_empty(),
        "recent.json / remote/shortcuts.json の読み手は理由を退避込みの口（`LoadIssue::read_failed` / \
         `LoadIssue::unparsable`）で組み、LoadIssue を直に組まない（直に組むと UTF-8 でない中身が \
         退避されず、次の保存が元の中身を消す = #2231）: {found:?}"
    );
}

#[test]
fn 組み立て口は退避まで済ませる() {
    let src = source(LOAD_ISSUE);
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
    let mut found = Vec::new();
    let mut known = 0;
    for (rel, src) in sources() {
        if rel == MIGRATION {
            continue;
        }
        let hits = code_hits(&rel, &src, &[SLOT]);
        if SLOT_KNOWN.contains(&rel.as_str()) {
            known += hits.len();
            continue;
        }
        found.extend(hits);
    }
    assert_eq!(
        known, 1,
        "既知の 1 か所（{SLOT_KNOWN:?}）が見えない・または増えた（寄せたならこの表から外す）"
    );
    assert!(
        found.is_empty(),
        "番号付きの退避は migration の口（`quarantine_unreadable` / `quarantine_raw`）へ寄せる \
         （自前で空きを探すと満杯時の押し出し・同じ中身を積まない規則が片方にだけ載る = #2231）: \
         {found:?}"
    );
}

/// 検出力: 回帰を植えると file:line で名指す（テストとコメントと文字列の中は見ない）
#[test]
fn 植えた回帰を名指せる() {
    let planted = "fn load(p: &Path) {\n    let e = read();\n    return LoadIssue::ReadFailed { reason: e, quarantine: None };\n    let free = quarantine_slot_path(p, 2);\n}\n";
    assert_eq!(
        code_hits("x.rs", planted, DIRECT),
        vec!["x.rs:3 に ReadFailed {".to_string()]
    );
    assert_eq!(
        code_hits("x.rs", planted, &[SLOT]),
        vec!["x.rs:4 に quarantine_slot_path(".to_string()]
    );
    assert!(
        code_hits("x.rs", planted, VIA).is_empty(),
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
}
