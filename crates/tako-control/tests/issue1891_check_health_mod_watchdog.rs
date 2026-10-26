//! **#1891 の番犬**: `tako check-health` の tako mod の項目が `tako mod` と別の判定を持たないようにする。
//!
//! ## 何をしたのか
//!
//! `tako mod`（S1 #1879）は「mod を注入しない理由」と「claude が画面に居るのに mod の報告が無い
//! ペイン」の理由を出す。#1891 で同じものを `tako check-health`（MCP `tako_check_health`）にも載せ、
//! 報告の無いペインの理由を候補の羅列ではなく判別した 1 つにした（ペインの claude のコマンド行と
//! 起動時の env を背景の段で調べる）。**判定が 2 か所に書かれると、片方だけ直したときに
//! `tako mod` と `check-health` の答えが黙って割れる**のがこの Issue の性質。
//!
//! ## ここで止める 5 つ
//!
//! 1. [`check_healthは判定を持たずtako_modの2段を写す`] — `finish_check_health` /
//!    `collect_check_health_ctx` が `claude_mod::status_ctx` → `finish_status_in_background` →
//!    `health_section` を通り、判定の部品（鮮度・注入の記録・画面の claude・理由の判別・skills の判定）を
//!    直に触らないこと
//! 2. [`理由の判別は1本`] — `classify_no_report(` を呼ぶのは tako-control の `claude_mod.rs` だけ
//!    （tako-core は定義）
//! 3. [`人が読む行は1つの関数から出る`] — CLI の `tako mod` と `tako check-health` が 1 行目と理由の行を
//!    `claude_mod::text` で出し、文面や理由の読み出しを直書きしないこと
//! 4. [`実プロセスの調べは背景の段だけ`] — UI スレッドの段（`status_ctx`）と同期の腕（`status`）が
//!    調べ（プロセスの表・起動時の env）を呼ばず、`tako mod`（status）が `OffloadJob::ModStatus` で
//!    背景へ出ること（UI スレッドの子プロセス待ちの網羅は #1984 の呼び出しグラフの番犬）
//! 5. [`判定を書き足すと名指しされる`] — 1〜3 の走査が、修正前を再現した注入（check-health 側の
//!    独自の判定・CLI の文面の直書き・別ファイルからの判別）を file:line で名指しすること
//!
//! ## 相方
//!
//! 隔離 GUI で `tako mod` と `check-health` の字面が一致することの実測は
//! `scripts/test-check-health-mod-1891.sh`。判定そのものの単体テストは tako-core / tako-control の
//! `claude_mod` のテスト。

use std::path::{Path, PathBuf};

use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// check-health の側に現れてはいけない判定の部品（右は理由）
const JUDGES: &[(&str, &str)] = &[
    ("classify_no_report(", "報告の無い理由の判別"),
    ("is_fresh(", "報告の鮮度"),
    ("claude_tui::detect(", "画面の claude の判定"),
    (".decide(", "注入の判断"),
    ("OffReason", "注入しない理由"),
    ("NoReportCause", "報告の無い理由の語彙"),
    (".injections", "注入の記録"),
    (".reports", "報告の保持"),
    ("claude_mod::lookup(", "報告の引き当て"),
    ("process_env_vars(", "起動時の env の読み"),
    ("env_has_injection(", "注入の印の判定"),
    ("claude_mod_install::", "設定 dir ごとの skills/tako の判定"),
    ("\"no_report\"", "行の状態の語彙"),
    ("\"mod_absent\"", "報告が無い理由の語彙"),
    ("\"not_injected\"", "行の状態の語彙"),
];

/// CLI の `tako mod` / `check-health` の表示に現れてはいけない直書き（右は理由）
const CLI_LITERALS: &[(&str, &str)] = &[
    (
        "注入しない",
        "1 行目の文面（`text::injection_line` が持つ）",
    ),
    (
        "新しく作るペインへ注入する",
        "1 行目の文面（`text::injection_line` が持つ）",
    ),
    (
        "[\"reason\"][\"message\"]",
        "理由の直読み（`text::reason_line` を通す）",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// テスト領域を潰してコメントも落とした眺め（行番号は保たれる。#1420 / #1609）
fn code(rel: &str, src: &str) -> String {
    code_view::without_comments_checked(&production_range::production(src, rel), rel)
}

/// トップレベル関数 1 本の本体（行番号つき）
fn fn_body(rel: &str, text: &str, name: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for (index, line) in text.lines().enumerate() {
        if is_top_level_fn_head(line) {
            inside = fn_head_name(line) == Some(name);
            continue;
        }
        if inside {
            out.push((index + 1, line.to_string()));
        }
    }
    assert!(
        !out.is_empty(),
        "{rel} に `fn {name}` が無い（改名・移設したなら番犬も追うこと）"
    );
    out
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 本体の中で語に当たる行を `file:line  語（理由）` で名指しする
fn offenders(rel: &str, body: &[(usize, String)], words: &[(&str, &str)]) -> Vec<String> {
    let mut out = Vec::new();
    for (number, line) in body {
        for (word, why) in words {
            if line.contains(word) {
                out.push(format!(
                    "{rel}:{number}  `{word}`（{why}）: {}",
                    line.trim()
                ));
            }
        }
    }
    out
}

/// 必ず通る口が本体に在るか（無ければ名指しの文を返す）
fn missing(rel: &str, name: &str, body: &[(usize, String)], required: &[&str]) -> Vec<String> {
    let text = joined(body);
    required
        .iter()
        .filter(|word| !text.contains(**word))
        .map(|word| format!("{rel} の `fn {name}` が `{word}` を通っていない"))
        .collect()
}

/// 1 の走査（dispatch.rs の眺めを受ける = 注入の検査と同じ関数）
fn scan_check_health(dispatch: &str) -> Vec<String> {
    let mut out = Vec::new();
    let collect = fn_body(DISPATCH, dispatch, "collect_check_health_ctx");
    out.extend(missing(
        DISPATCH,
        "collect_check_health_ctx",
        &collect,
        &["claude_mod::status_ctx("],
    ));
    out.extend(offenders(DISPATCH, &collect, JUDGES));
    let finish = fn_body(DISPATCH, dispatch, "finish_check_health");
    out.extend(missing(
        DISPATCH,
        "finish_check_health",
        &finish,
        &[
            "claude_mod::finish_status_in_background(",
            "claude_mod::health_section(",
            "claude_mod::health_issues",
            "claude_mod::health_unavailable(",
        ],
    ));
    out.extend(offenders(DISPATCH, &finish, JUDGES));
    out
}

/// 3 の走査（CLI の眺めを受ける）
fn scan_cli(cli: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (name, required) in [
        // #2365: `tako mod` の行の組み立ては tako-control の `text::status_lines` へ移した（CLI は出力だけ）。
        // `status_lines` が 1 行目と理由の行を同じ関数で出すことは [`人が読む行は1つの関数から出る`] が見る
        ("print_mod", &["claude_mod::text::status_lines("][..]),
        (
            "print_check_health_mod",
            &[
                "claude_mod::text::injection_line(",
                "claude_mod::text::reason_line(",
                "print_mod_skills(",
            ][..],
        ),
    ] {
        let body = fn_body(CLI, cli, name);
        out.extend(missing(CLI, name, &body, required));
        out.extend(offenders(CLI, &body, CLI_LITERALS));
    }
    // 届かないときの不明も同じ節の組み立て（`health_unavailable`）と同じ表示を通る
    let offline = fn_body(CLI, cli, "check_health_cli");
    out.extend(missing(
        CLI,
        "check_health_cli",
        &offline,
        &["claude_mod::health_unavailable(", "print_check_health_mod("],
    ));
    let online = fn_body(CLI, cli, "print_check_health");
    out.extend(missing(
        CLI,
        "print_check_health",
        &online,
        &["print_check_health_mod("],
    ));
    out
}

/// 2 の走査（`(相対パス, 眺め)` の一覧を受ける）
fn scan_classify(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, text) in files {
        if rel == CONTROL_MOD || rel == CORE_MOD {
            continue;
        }
        for (index, line) in text.lines().enumerate() {
            if line.contains("classify_no_report(") {
                out.push(format!("{rel}:{}  {}", index + 1, line.trim()));
            }
        }
    }
    out
}

fn product_views() -> Vec<(String, String)> {
    let files = production_range::product_sources();
    let tests = production_range::test_only_files(&files);
    files
        .into_iter()
        .filter(|rel| !tests.contains(rel))
        .map(|rel| {
            let view = code_view::without_comments(&production_range::scan(&read(&rel)).text);
            (rel, view)
        })
        .collect()
}

/// 1
#[test]
fn check_healthは判定を持たずtako_modの2段を写す() {
    let hits = scan_check_health(&code(DISPATCH, &read(DISPATCH)));
    assert!(
        hits.is_empty(),
        "check-health の mod の項目が `tako mod` と別の道を通っている:\n  {}\n\n\
         判定は `claude_mod::status_ctx` / `finish_status_in_background` の 1 実装が持ち、\n\
         check-health は `health_section` で同じキーを写すだけにすること（#1891）",
        hits.join("\n  ")
    );
}

/// 2
#[test]
fn 理由の判別は1本() {
    let views = product_views();
    assert!(
        views.iter().any(|(rel, _)| rel == CONTROL_MOD),
        "走査が空振りしている（{CONTROL_MOD} が列挙に無い）"
    );
    let hits = scan_classify(&views);
    assert!(
        hits.is_empty(),
        "報告の無い理由の判別（`classify_no_report`）を {CONTROL_MOD} の外で呼んでいる:\n  {}\n\n\
         判別は `tako mod` の行を組む 1 実装だけが呼ぶこと（#1891）",
        hits.join("\n  ")
    );
    // 呼ぶ側は確かに呼んでいる（空振りの検査）
    let control = &views.iter().find(|(rel, _)| rel == CONTROL_MOD).unwrap().1;
    assert!(
        control.matches("core::classify_no_report(").count() >= 2,
        "{CONTROL_MOD} が行を組む段と仕上げの段の両方で `classify_no_report` を通っていない"
    );
}

/// 3
#[test]
fn 人が読む行は1つの関数から出る() {
    let hits = scan_cli(&code(CLI, &read(CLI)));
    assert!(
        hits.is_empty(),
        "`tako mod` と `tako check-health` の行が 1 つの関数から出ていない:\n  {}\n\n\
         1 行目と理由の行は `tako_control::claude_mod::text` が持つこと（#1891。字面が割れる）",
        hits.join("\n  ")
    );
    // 文面の正本は 1 か所（tako-control の text）
    let control = code(CONTROL_MOD, &read(CONTROL_MOD));
    // #2365: CLI の `tako mod` の行を組む `text::status_lines` も、1 行目と理由の行を同じ関数で出す
    // （`pub mod text` の中の関数なので、頭の行から同じ字下げの `}` までを本体とする）
    let mut lines = Vec::new();
    let mut inside = false;
    for (index, line) in control.lines().enumerate() {
        if line.starts_with("    pub fn status_lines(") {
            inside = true;
            continue;
        }
        if inside {
            if line == "    }" {
                break;
            }
            lines.push((index + 1, line.to_string()));
        }
    }
    assert!(
        !lines.is_empty(),
        "{CONTROL_MOD} に `text::status_lines` が無い（改名・移設したなら番犬も追うこと）"
    );
    let missed = missing(
        CONTROL_MOD,
        "status_lines",
        &lines,
        &["injection_line(", "reason_line(", "skills_lines("],
    );
    assert!(
        missed.is_empty(),
        "`tako mod` の行が check-health と同じ関数を通っていない:\n  {}",
        missed.join("\n  ")
    );
    assert_eq!(
        control
            .matches("\"tako mod: 新しく作るペインへ注入する\"")
            .count(),
        1,
        "{CONTROL_MOD} の 1 行目の文面が 1 か所ではない"
    );
}

/// 4
#[test]
fn 実プロセスの調べは背景の段だけ() {
    let control = code(CONTROL_MOD, &read(CONTROL_MOD));
    let probes: &[(&str, &str)] = &[
        ("probe_no_report(", "報告の無い claude の調べ"),
        ("finish_status_in_background(", "調べを含む仕上げ"),
        ("ProcessSnapshot", "プロセスの表 + 器の一覧"),
        ("process_env_vars(", "起動時の env の読み"),
    ];
    let mut hits = Vec::new();
    for name in ["status_ctx", "status", "pane_row"] {
        hits.extend(offenders(
            CONTROL_MOD,
            &fn_body(CONTROL_MOD, &control, name),
            probes,
        ));
    }
    assert!(
        hits.is_empty(),
        "UI スレッドで走りうる段が実プロセスを調べている:\n  {}",
        hits.join("\n  ")
    );
    let background = fn_body(CONTROL_MOD, &control, "finish_status_in_background");
    assert!(
        joined(&background).contains("probe_no_report("),
        "`finish_status_in_background` が調べ（`probe_no_report`）を通っていない = 理由が 1 つに決まらない"
    );

    let dispatch = code(DISPATCH, &read(DISPATCH));
    let prepare = joined(&fn_body(DISPATCH, &dispatch, "prepare_offload"));
    assert!(
        prepare.contains("OffloadJob::ModStatus") && prepare.contains("claude_mod::status_ctx("),
        "`prepare_offload` が `tako mod`（status）を背景のジョブ（`OffloadJob::ModStatus`）にしていない"
    );
}

/// 5: 修正前を再現した注入を名指しできる（走査が見逃す側へ倒れていないこと）
#[test]
fn 判定を書き足すと名指しされる() {
    // check-health に独自の判定を足す
    let dispatch = code(DISPATCH, &read(DISPATCH));
    let anchor = "    let healthy = issues.is_empty();";
    assert!(dispatch.contains(anchor), "注入の目印が見つからない");
    let injected = dispatch.replacen(
        anchor,
        "    let own = ctx.panes > 0 && \"no_report\" == \"no_report\";\n    let healthy = issues.is_empty();",
        1,
    );
    let hits = scan_check_health(&injected);
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&format!("{DISPATCH}:")) && h.contains("\"no_report\"")),
        "check-health に足した独自の判定を名指しできない: {hits:?}"
    );
    // 2 段を通らない形（写しを外す）
    let bypassed = dispatch.replace("claude_mod::health_section(", "own_section(");
    assert!(
        scan_check_health(&bypassed)
            .iter()
            .any(|h| h.contains("claude_mod::health_section(")),
        "check-health が `health_section` を通らない形を名指しできない"
    );

    // CLI に文面を直書きする
    let cli = code(CLI, &read(CLI));
    let anchor = "fn print_check_health_mod(section: &Value) {\n";
    assert!(cli.contains(anchor), "CLI の注入の目印が見つからない");
    let injected = cli.replacen(
        anchor,
        &format!("{anchor}    println!(\"tako mod: 注入しない\");\n"),
        1,
    );
    assert!(
        scan_cli(&injected)
            .iter()
            .any(|h| h.starts_with(&format!("{CLI}:")) && h.contains("注入しない")),
        "CLI に直書きした文面を名指しできない"
    );

    // 別のファイルから判別を呼ぶ
    let mut views = vec![(
        "crates/tako-control/src/elsewhere.rs".to_string(),
        "fn f() { let _ = tako_core::claude_mod::classify_no_report(None); }".to_string(),
    )];
    views.push((
        CONTROL_MOD.to_string(),
        "core::classify_no_report(".to_string(),
    ));
    let hits = scan_classify(&views);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].starts_with("crates/tako-control/src/elsewhere.rs:1"));
}
