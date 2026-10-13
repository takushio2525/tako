//! 参照検索・呼び出し階層・型階層（#1685）の番犬
//!
//! # なぜ要るか
//!
//! 次のどれも **1 行で壊せる**のに、CLI の応答の形は変わらず、単体も緑のまま:
//!
//! - ペインを閉じる経路で結果を消さない → 保持件数が 0 に戻らない（閉じたペインの結果が溜まる）。
//!   dispatch は読むたびに閉じたペインの結果を捨てる保険を持つので、CLI の `retained` は 0 に見えて
//!   GUI のメモリにだけ残る
//! - 問い合わせ・子の読み込みを `prepare_offload` から外す → UI スレッドで言語サーバを待つ
//!   （#1976 / #1979 / #1984 の型。大きなプロジェクトの参照検索は数秒〜数十秒）
//! - 右パネルの行のクリック・開閉の印・× が dispatch を通らない → 画面でしかできない操作になり、
//!   展開状態が `tako lsp results` から読めない（#1407 と同じ穴）
//! - 握手の能力の読み直しで型の知らないキーを落とす → 型階層を申告したサーバを「未対応」と読み違える
//!
//! 挙動は単体・e2e・visual-test `xref-1685` が測り、ここは**構造**を縛って file:line で名指す。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const PANEL: &str = "crates/tako-app/src/references_panel.rs";

const FILES: [&str; 5] = [DISPATCH, MANAGER, RENDER, APP, PANEL];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの規則: `file` の `decl` で始まる関数の中に `must` がすべてあり、`must_not` が無い
struct Rule {
    file: &'static str,
    decl: &'static str,
    must: &'static [&'static str],
    must_not: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    // --- 閉じたら消す（保持件数を 0 へ戻す）---
    Rule {
        file: RENDER,
        decl: "    pub(crate) fn drop_preview_pane_state(",
        must: &["self.lsp_xref.remove(pane_id);"],
        must_not: &[],
        why: "ペインを閉じる経路（× / タブ × / ⌘W）で参照・階層の結果を消していない（保持件数が 0 に戻らない）",
    },
    Rule {
        file: APP,
        decl: "    fn kill_shelved_pane(",
        must: &["self.lsp_xref.remove(pane_id);"],
        must_not: &[],
        why: "たまり場の kill で参照・階層の結果を消していない（保持件数が 0 に戻らない）",
    },
    Rule {
        file: DISPATCH,
        decl: "pub fn lsp_results(",
        must: &["store.retain_panes("],
        must_not: &[],
        why: "読むときに閉じたペインの結果を捨てていない（閉じる経路の取りこぼしの保険が無い）",
    },
    // --- 言語サーバを UI スレッドで待たない ---
    Rule {
        file: DISPATCH,
        decl: "pub fn prepare_offload(",
        must: &[
            "OffloadJob::LspXref(Box::new(job))",
            "OffloadJob::LspXrefExpand(Box::new(job))",
        ],
        must_not: &[],
        why: "参照・階層の問い合わせ / 子の読み込みを background へ出していない（UI スレッドで言語サーバを待つ）",
    },
    Rule {
        file: PANEL,
        decl: "    pub(crate) fn request_lsp_xref(",
        must: &["tako_control::prepare_offload(self, &request)", "job.run_staged()"],
        must_not: &["job.run()"],
        why: "右クリックの検索が CLI / MCP と同じ 3 段（prepare_offload → run_staged → finish_offload）を通っていない",
    },
    // --- 画面の操作は CLI / MCP と同じ口 ---
    Rule {
        file: PANEL,
        decl: "    fn open_xref_item(",
        must: &[
            "Request::LspResults {",
            "LSP_OPEN_RESULT_ACTION",
            "tako_control::dispatch(self, request",
        ],
        must_not: &[],
        why: "右パネルの行のクリックが `tako lsp open-result` と同じ要求を通っていない（画面だけの着地）",
    },
    Rule {
        file: PANEL,
        decl: "    fn toggle_xref_node(",
        must: &[
            "Request::LspResults {",
            "LSP_EXPAND_ACTION",
            "LSP_COLLAPSE_ACTION",
            "tako_control::prepare_offload(self, &request)",
        ],
        must_not: &[],
        why: "ツリーの開閉の印が `tako lsp expand` / `collapse` と同じ要求を通っていない（展開状態が CLI / MCP から読めない = #1407 の穴）",
    },
    Rule {
        file: PANEL,
        decl: "    fn clear_xref(",
        must: &["Request::LspResults {", "LSP_CLEAR_RESULTS_ACTION"],
        must_not: &[],
        why: "見出しの × が `tako lsp clear-results` と同じ要求を通っていない",
    },
    // --- 握手の能力の読み直しで型の知らないキーを落とさない ---
    Rule {
        file: MANAGER,
        decl: "fn parse_capabilities(",
        must: &["typed.insert(key.clone(), value.clone());"],
        must_not: &[],
        why: "lsp-types が知らない能力のキー（typeHierarchyProvider）を落としている（型階層を申告したサーバを未対応と読み違える）",
    },
];

/// `decl` で始まる関数の窓（宣言の行 1 始まり, 本文の行）。閉じ括弧は宣言と同じ字下げの `}`
fn fn_window(src: &str, decl: &str) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.starts_with(decl))?;
    let indent = decl.len() - decl.trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = (start..lines.len()).find(|&j| lines[j] == close)?;
    Some((
        start + 1,
        lines[start..=end].iter().map(|l| l.to_string()).collect(),
    ))
}

/// 違反（`ファイル:行 — 理由`）。`sources` は `FILES` と同じ順
fn scan(sources: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for rule in RULES {
        let index = FILES.iter().position(|f| *f == rule.file).expect("FILES");
        let src = code_view::without_comments(&sources[index]);
        let Some((at, window)) = fn_window(&src, rule.decl) else {
            out.push(format!(
                "{}:0 — `{}` が見つからない（走査が空振り）",
                rule.file,
                rule.decl.trim()
            ));
            continue;
        };
        let body = window.join("\n");
        for must in rule.must {
            if !body.contains(must) {
                out.push(format!(
                    "{}:{at} — {}（`{must}` が無い）",
                    rule.file, rule.why
                ));
            }
        }
        for not in rule.must_not {
            if let Some(i) = window.iter().position(|l| l.contains(not)) {
                out.push(format!(
                    "{}:{} — {}（`{not}` がある）",
                    rule.file,
                    at + i,
                    rule.why
                ));
            }
        }
    }
    out
}

fn current() -> Vec<String> {
    FILES.iter().map(|f| read(f)).collect()
}

#[test]
fn 参照と階層の構造が崩れていない() {
    let offenders = scan(&current());
    assert!(
        offenders.is_empty(),
        "参照検索・呼び出し階層・型階層の構造が崩れている（#1685）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let sources = current();
    for rule in RULES {
        let index = FILES.iter().position(|f| *f == rule.file).unwrap();
        let src = code_view::without_comments(&sources[index]);
        let (_, window) = fn_window(&src, rule.decl)
            .unwrap_or_else(|| panic!("{} の `{}` が採れない", rule.file, rule.decl.trim()));
        assert!(window.len() > 2, "{} の窓が空", rule.decl.trim());
    }
}

/// 規則ごとに、現行ソースから `must` の 1 つ目を消した注入が**その関数の宣言の行**で名指しされる
#[test]
fn 逆戻りを名指しできる() {
    let sources = current();
    for rule in RULES {
        let index = FILES.iter().position(|f| *f == rule.file).unwrap();
        let from = rule.must[0];
        assert!(
            sources[index].contains(from),
            "{}: 注入元 {from:?} が現行ソースに無い",
            rule.file
        );
        // 同じ字面が手前の関数にもあるので、**その規則の宣言より後ろ**の最初の 1 つを外す
        let decl_at = sources[index]
            .find(&format!("\n{}", rule.decl))
            .unwrap_or_else(|| panic!("{} の宣言が無い", rule.decl.trim()));
        let (head, tail) = sources[index].split_at(decl_at);
        assert!(
            tail.contains(from),
            "{}: 注入元 {from:?} が宣言の後ろに無い",
            rule.file
        );
        let mut injected = sources.clone();
        injected[index] = format!("{head}{}", tail.replacen(from, "/* 注入で外した */", 1));
        let line = injected[index]
            .lines()
            .position(|l| l.starts_with(rule.decl))
            .map(|i| i + 1)
            .unwrap_or_else(|| panic!("{} の宣言が無い", rule.decl.trim()));
        let expected = format!("{}:{line} ", rule.file);
        let reports = scan(&injected);
        assert!(
            reports.iter().any(|r| r.starts_with(&expected)),
            "{} を外した注入を {expected} で名指ししていない: {reports:?}",
            from
        );
    }
    // must_not: 右クリックの検索を UI スレッドで待つ（`run_staged` を `run` へ）
    let index = FILES.iter().position(|f| *f == PANEL).unwrap();
    let mut injected = sources.clone();
    injected[index] = sources[index].replacen(
        ".spawn(async move { job.run_staged() });",
        ".spawn(async move { job.run() });",
        1,
    );
    assert_ne!(injected[index], sources[index], "注入元が現行ソースに無い");
    let line = injected[index]
        .lines()
        .position(|l| l.contains("job.run() });"))
        .map(|i| i + 1)
        .unwrap();
    let reports = scan(&injected);
    assert!(
        reports
            .iter()
            .any(|r| r.starts_with(&format!("{PANEL}:{line} "))),
        "UI スレッドで待つ注入を名指ししていない: {reports:?}"
    );
}
