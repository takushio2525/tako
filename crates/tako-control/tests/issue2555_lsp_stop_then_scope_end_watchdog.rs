//! 言語サーバの e2e が「止めた直後に置き場を手放す」形の番犬（#2555）
//!
//! # なぜ要るか
//!
//! `LspManager::stop`（`tako lsp stop` と同じ口）は**止め終わるのを待たずに返る**（shutdown → exit →
//! 終了待ちは `begin_stop` の別スレッド）。e2e がテストの最後を `manager.stop(None)` で終えると、
//! 直後の置き場の `remove_dir_all` と、偽サーバ（`tako-lsp-fake`）が `exit` を受けて `received.jsonl` を
//! `create(true)` で開き直す書き込みが競る。中身を消した後に作り直された `received.jsonl` で最後の rmdir が
//! 失敗し、置き場が `$TMPDIR` に消え残る。テストは緑のままなので気づけず、10/28 から
//! `tako-2035-*` が 263 個・`tako-2048-*` が 85 個たまった（修正前の木で 10 回回すと 2035 は 20 個・
//! 2048 は 2 個増えた。中身はどれも `exit` の 1 行）。
//!
//! # 縛ること
//!
//! `crates/tako-control/tests/*.rs` のうち言語サーバの manager を使うファイルで、`.stop(None)` /
//! `.stop(Some(…))` を呼んだら、**同じブロックを抜けるまでに**止め終わりを待つ（`wait…(` の待ちか
//! `shutdown_all(`）。待たずにブロックを抜ける形を file:line で名指す。テストの最後に止めるだけなら
//! `shutdown_all`（偽サーバの終了まで待つ）を使う。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で既知の止め方（待つ 5 か所と免除 1 か所）がその通りに分類されることを固定し、
//! [`逆戻りを名指しできる`] で**現行ソースから作り直した注入**（#2555 の直す前の形）が file:line で
//! 名指しされることを確かめる（注入は `cargo test` の中で完結し、ファイルは触らない）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const TESTS_DIR: &str = "crates/tako-control/tests";

/// この番犬自身（走査の語を文字列で持つので外す）
const SELF_NAME: &str = "issue2555_lsp_stop_then_scope_end_watchdog.rs";

/// 言語サーバの manager を使うファイルの目印
const MANAGER_MARK: &str = "LspManager";

/// 止め終わるのを待たない止め方（`tako lsp stop` と同じ口）
const STOPS: &[&str] = &[".stop(None)", ".stop(Some("];

/// 止め終わりを待つ形（コードだけの眺めで探す = 識別子の一部）
const WAITS: &[&str] = &["wait", "shutdown_all("];

/// 待たなくてよいと決めた止め方（ファイル名, 関数, 理由）
const EXEMPT: &[(&str, &str, &str)] = &[(
    "issue2048_lsp_isolated_autostart.rs",
    "隔離で起こさなかった器を_stop_したら_stop_の理由",
    "隔離で起こさなかった器を止める（spawn 0 を確かめるテスト = 止め終わりを待つプロセスが無い）",
)];

/// 待つ形として固定する既知の止め方（ファイル名, 関数）
const KNOWN_WAITING: &[(&str, &str)] = &[
    (
        "issue1678_lsp_e2e.rs",
        "送られたメッセージ列が固定値と一致する",
    ),
    (
        "issue1679_lsp_diagnostics.rs",
        "閉じたら保持は0になり止めても残さない",
    ),
    (
        "issue1944_lsp_fetch.rs",
        "取っている途中の_stop_は取れても起こさない",
    ),
    (
        "issue2034_lsp_watched_files.rs",
        "解除や停止の後は知らせず監視を落とす",
    ),
    ("issue2454_lsp_job_windows.rs", "stop"),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// `crates/tako-control/tests/*.rs`（この番犬を除く）を名前順に（相対パス, 本文）
fn current() -> Vec<(String, String)> {
    let dir = workspace_root().join(TESTS_DIR);
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{TESTS_DIR} が読める: {e}"))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_string();
            let is_rs = path.extension().and_then(|e| e.to_str()) == Some("rs");
            (is_rs && name != SELF_NAME).then(|| {
                let src = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("{name} が読める: {e}"));
                (format!("{TESTS_DIR}/{name}"), src)
            })
        })
        .collect();
    files.sort();
    files
}

/// 止める呼び出し 1 つ
#[derive(Debug)]
struct StopSite {
    file: String,
    /// 呼び出しの行（1-based）
    line: usize,
    /// 囲んでいる関数（見つからなければ空）
    func: String,
    /// 呼び出しの後ろから、同じブロックを抜けるまでのコード
    rest: String,
}

impl StopSite {
    fn waits(&self) -> bool {
        WAITS.iter().any(|w| self.rest.contains(w))
    }

    fn exempt(&self) -> bool {
        EXEMPT
            .iter()
            .any(|(file, func, _)| self.file.ends_with(file) && self.func == *func)
    }
}

/// `from` から、同じブロックを抜ける `}` の手前まで（入れ子の `{}` は中身ごと含める）
fn rest_of_block(code: &str, from: usize) -> &str {
    let mut depth = 0usize;
    for (i, c) in code[from..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return &code[from..from + i],
            '}' => depth -= 1,
            _ => {}
        }
    }
    &code[from..]
}

/// `at`（バイト位置）を含む関数の名前（その位置より前で最後の関数の頭）
fn enclosing_fn(code: &str, at: usize) -> String {
    code[..at]
        .lines()
        .rev()
        .find_map(tako_core::source_scan::fn_head_name)
        .unwrap_or_default()
        .to_string()
}

/// 言語サーバの manager を使うファイルの、止める呼び出しを全部
fn scan_stops(sources: &[(String, String)]) -> Vec<StopSite> {
    let mut out = Vec::new();
    for (file, src) in sources {
        // コメントと文字列を潰す（説明文の `stop(None)` や文字列の `{` を数えない）。バイト長は変わらない
        let code = code_view::code_view(src);
        if !code.contains(MANAGER_MARK) {
            continue;
        }
        for stop in STOPS {
            for (at, _) in code.match_indices(stop) {
                let after = at + stop.len();
                out.push(StopSite {
                    file: file.clone(),
                    line: code[..at].matches('\n').count() + 1,
                    func: enclosing_fn(&code, at),
                    rest: rest_of_block(&code, after).to_string(),
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    out
}

/// `file:line — 理由` の一覧（違反が無ければ空）
fn scan(sources: &[(String, String)]) -> Vec<String> {
    scan_stops(sources)
        .iter()
        .filter(|s| !s.waits() && !s.exempt())
        .map(|s| {
            format!(
                "{}:{} — fn {} が `stop` の後、止め終わりを待たずにブロックを抜ける（`stop` は shutdown → exit を\
                 別スレッドへ渡して返る = 直後に置き場を消すと偽サーバの `exit` の書き込みで置き場が消え残る。\
                 #2555）。止めるだけなら `shutdown_all` で、stop の経路を見るなら `stopped` を待つ",
                s.file, s.line, s.func
            )
        })
        .collect()
}

#[test]
fn 止めた後は止め終わりを待ってから置き場を手放す() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let sites = scan_stops(&current());
    let find = |file: &str, func: &str| {
        sites
            .iter()
            .find(|s| s.file.ends_with(file) && s.func == func)
            .unwrap_or_else(|| panic!("{file} の fn {func} の stop が採れない: {sites:#?}"))
    };
    for (file, func) in KNOWN_WAITING {
        let site = find(file, func);
        assert!(
            site.waits(),
            "{file} の fn {func} の stop を「待つ」と見ていない（ブロックの取り方が狭い）: {site:#?}"
        );
    }
    for (file, func, _) in EXEMPT {
        let site = find(file, func);
        assert!(
            !site.waits(),
            "{file} の fn {func} は待つ形になった = EXEMPT から外す: {site:#?}"
        );
    }
}

/// `from` を `to` へ 1 回置き換える（注入元が現行ソースに無ければ落ちる）
fn replaced(label: &str, src: &str, from: &str, to: &str) -> String {
    assert_eq!(
        src.matches(from).count(),
        1,
        "{label}: 注入元 {from:?} が現行ソースにちょうど 1 つない"
    );
    src.replacen(from, to, 1)
}

/// 注入した行の目印（コメント = 走査の眺めでは潰れる）
const MARK: &str = "// 注入 #2555";

/// 注入 1 つぶん: `file` の本文を `inject` で変えたソースで、目印の行が名指しされること
fn assert_named(label: &str, file: &str, inject: impl Fn(&str) -> String) {
    let mut sources = current();
    let slot = sources
        .iter_mut()
        .find(|(f, _)| f.ends_with(file))
        .unwrap_or_else(|| panic!("{label}: {file} が無い"));
    slot.1 = inject(&slot.1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(MARK))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 目印が無い"));
    let expected = format!("{}:{line} ", slot.0);
    let reports = scan(&sources);
    let named = reports.iter().find(|r| r.starts_with(&expected));
    assert!(
        named.is_some(),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

const E2E_2035: &str = "issue2035_lsp_init_options.rs";
const E2E_2048: &str = "issue2048_lsp_isolated_autostart.rs";

#[test]
fn 逆戻りを名指しできる() {
    // A. #2555 の直す前: 2035 のテストの最後が `stop`（ブロックの最後の文）
    let label = "A 2035 の最後が stop";
    assert_named(label, E2E_2035, |src| {
        replaced(
            label,
            src,
            "    manager.shutdown_all(Duration::from_secs(2));\n}\n\n/// 上書きの入口の検査",
            &format!("    manager.stop(None); {MARK}\n}}\n\n/// 上書きの入口の検査"),
        )
    });
    // B. 2048 のループの中（器ごとの置き場がループの 1 周で消える）
    let label = "B 2048 のループの最後が stop";
    assert_named(label, E2E_2048, |src| {
        replaced(
            label,
            src,
            "        manager.shutdown_all(Duration::from_secs(2));\n    }\n}",
            &format!("        manager.stop(None); {MARK}\n    }}\n}}"),
        )
    });
    // C. stop の後に待たない文を挟んでからブロックを抜ける
    let label = "C 2035 の stop の後に検査だけ";
    assert_named(label, E2E_2035, |src| {
        replaced(
            label,
            src,
            "    assert_eq!(report[\"legacy\"], json!(false));\n    manager.shutdown_all(Duration::from_secs(2));\n}",
            &format!(
                "    manager.stop(None); {MARK}\n    assert_eq!(report[\"legacy\"], json!(false));\n}}"
            ),
        )
    });
    // D. `Some(…)` で名指して止める形も同じ
    let label = "D 2048 の Some で止める";
    assert_named(label, E2E_2048, |src| {
        replaced(
            label,
            src,
            "    assert!(!manager.document_server(&path).expect(\"ある\").held);\n    manager.shutdown_all(Duration::from_secs(2));\n}",
            &format!(
                "    assert!(!manager.document_server(&path).expect(\"ある\").held);\n    manager.stop(Some(\"rust-analyzer\")); {MARK}\n}}"
            ),
        )
    });
}
