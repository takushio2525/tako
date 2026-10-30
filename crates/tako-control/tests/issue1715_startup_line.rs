//! 起動時の 1 行が違反の種類を言い分けること（Issue #1715）
//!
//! #1645 で作業ログの「復活・重複」（`revived_entries`）を違反に足したが、
//! `startup_line` は種類を区別せず「起動時ロードが予算超過: N 件」と言っていた。
//! 直し方が違う（予算超過は `tako context-budget fix` で直る / 戻ったエントリは
//! `fix` では直らず、main から 2 ファイルを戻して自分の 1 件だけ書き直す）ので、
//! master が起動時に見る 1 行で見分けられないといけない。
//!
//! 実際の置き場（一時ディレクトリの規約 + 作業ログ + アーカイブ）を作り、
//! **`tako master` / `tako setup` が呼ぶ `startup_line`** と、
//! **MCP `tako_context_budget` の応答**（`tools/call` を本物の変換に通す）の両方で、
//! 復活だけ / 超過だけ / 両方 の 3 通りを日英で確かめる。
//!
//! **このファイルにテストは 1 本だけ置く**（`HOME` / `TAKO_DATA_DIR` / 表示言語は
//! プロセス全体の状態なので、同じバイナリに他のテストを混ぜると並列実行で干渉する）。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tako_control::protocol::Request;
use tako_core::i18n::Lang;

/// 作業ログ 1 件（見出し + 本文 1 行）
fn entry(day: usize, issue: usize, title: &str) -> String {
    format!("## 2026-10-{day:02}（#{issue} {title}）\n- 本文\n\n")
}

/// 置き場を 1 つ作る。`normal` 件の普通のエントリと、`revived` なら
/// アーカイブにも載っているエントリ（= rebase で戻ったもの）を末尾に足す
fn write_repo(dir: &Path, normal: usize, revived: bool, long_context: bool) {
    std::fs::create_dir_all(dir.join(".agent")).expect("置き場");
    let mut imports = String::from("- 作業ログ: @.agent/progress.md\n");
    if long_context {
        imports.push_str("- 現在: @.agent/activeContext.md\n");
        // 自動で直せない超過（現在状態は 80 行まで）
        let lines: String = (1..=90).map(|i| format!("- 行 {i}\n")).collect();
        std::fs::write(
            dir.join(".agent/activeContext.md"),
            format!("# Active Context\n\n{lines}"),
        )
        .expect("activeContext");
    }
    std::fs::write(dir.join("AGENTS.md"), format!("# 規約\n\n{imports}")).expect("AGENTS.md");

    let mut log = String::from("# Progress Log\n\n");
    for i in 1..=normal {
        log.push_str(&entry(i, 200 + i, "普通のエントリ"));
    }
    if revived {
        log.push_str(&entry(29, 100, "古いエントリ"));
    }
    std::fs::write(dir.join(".agent/progress.md"), log).expect("progress");
    std::fs::write(
        dir.join(".agent/progress-archive.md"),
        "# Progress Archive\n\n- 2026-10-29 #100 古いエントリ\n",
    )
    .expect("archive");
}

/// MCP の口（`tools/call tako_context_budget` を本物の変換に通す。実行係は
/// dispatch の `Request::ContextBudget` の check の腕と同じ 1 本 = `report`）
fn mcp(cwd: &Path) -> Value {
    let mut exec = |request: Request| -> Result<Value, String> {
        match request {
            Request::ContextBudget {
                action,
                cwd,
                profile,
                ..
            } => {
                assert_eq!(action.as_deref().unwrap_or("check"), "check");
                tako_control::context_budget::report(
                    &PathBuf::from(cwd.expect("cwd を渡している")),
                    profile.as_deref(),
                )
            }
            other => Err(format!("想定外の要求: {other:?}")),
        }
    };
    let mut session = tako_control::mcp::McpSession {
        caller_pane: None,
        caller_role: None,
        connected: true,
        exec: &mut exec,
    };
    let reply = tako_control::mcp::handle_message(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "tako_context_budget",
                           "arguments": {"cwd": cwd.display().to_string()}}}),
        &mut session,
    )
    .expect("tools/call には応答がある");
    let result = &reply["result"];
    assert_ne!(result["isError"], true, "{reply}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).expect("応答は JSON")
}

/// 起動時の 1 行（`tako master` / `tako setup` の口）と、MCP の応答に載る同じ 1 行。
/// **2 つが食い違わないこと**もここで固定する（1 実装から組む）
fn line(cwd: &Path) -> String {
    let startup = tako_control::context_budget::startup_line(cwd, None)
        .unwrap_or_else(|| panic!("{} は違反があるので 1 行出る", cwd.display()));
    eprintln!("[{}] {startup}", cwd.file_name().unwrap().to_string_lossy());
    let via_mcp = mcp(cwd);
    assert_eq!(
        via_mcp["startup_line"].as_str(),
        Some(startup.as_str()),
        "MCP の応答と起動時の 1 行が食い違う: {via_mcp:#}"
    );
    assert_eq!(startup.lines().count(), 1, "1 行に収める: {startup}");
    startup
}

#[test]
fn 起動時の1行が予算超過と作業ログの復活を言い分ける() {
    let base = std::env::temp_dir().join(format!("tako-1715-{}", std::process::id()));
    let home = base.join("h");
    let data = base.join("d");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&data).expect("data");
    // 隔離: グローバル指示ファイルもメモリも無い `HOME` に閉じるので、
    // 出る違反は置き場の作業ログ（と現在状態）のものだけになる
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("TAKO_DATA_DIR", &data);
    }

    let revived = base.join("revived");
    let over = base.join("over");
    let both = base.join("both");
    let not_fixable = base.join("not_fixable");
    write_repo(&revived, 3, true, false);
    // 21 件 = 件数（20）と作業日数（5）の 2 軸で超える。どちらも `fix` で直る
    write_repo(&over, 21, false, false);
    write_repo(&both, 21, true, false);
    write_repo(&not_fixable, 3, true, true);

    // 予算内で復活も無ければ黙る（#322）
    let clean = base.join("clean");
    write_repo(&clean, 3, false, false);
    assert_eq!(
        tako_control::context_budget::startup_line(&clean, None),
        None
    );
    assert!(mcp(&clean)["startup_line"].is_null());

    // ── 日本語 ──
    tako_core::i18n::set_lang(Lang::Ja);

    let l = line(&revived);
    assert!(
        l.starts_with("作業ログに rebase で戻った・重複したエントリ: 1 件"),
        "復活だけなのに予算超過と言っている: {l}"
    );
    assert!(!l.contains("予算超過"), "{l}");
    assert!(l.contains("fix では直らない"), "{l}");
    assert!(
        l.contains("progress.md:"),
        "どこに戻ったかを名指ししていない: {l}"
    );
    assert!(
        !l.contains("tako context-budget fix"),
        "直らない fix を勧めている: {l}"
    );
    assert!(l.ends_with("直し方: tako context-budget"), "{l}");

    let l = line(&over);
    assert!(l.starts_with("起動時ロードが予算超過: 2 件"), "{l}");
    assert!(
        l.contains("いま直す: tako context-budget fix（自動で直せる 2 件）"),
        "{l}"
    );
    assert!(!l.contains("rebase"), "超過だけなのに復活と言っている: {l}");

    let l = line(&both);
    assert!(
        l.starts_with("起動時ロードが予算超過: 2 件"),
        "復活を予算超過の件数に混ぜている: {l}"
    );
    assert!(
        l.contains("いま直す: tako context-budget fix（自動で直せる 2 件）"),
        "{l}"
    );
    assert!(
        l.contains("ほかに作業ログに rebase で戻った・重複したエントリ: 1 件"),
        "{l}"
    );
    assert!(l.contains("fix では直らない"), "{l}");

    // 自動で直せない超過 + 復活: 名指しは予算超過の側から選ぶ（復活の提案を拾わない）
    let l = line(&not_fixable);
    assert!(l.starts_with("起動時ロードが予算超過: 1 件"), "{l}");
    assert!(l.contains("activeContext.md: lines 92 > 80"), "{l}");
    assert!(
        !l.contains("revived_entries"),
        "復活を超過として名指ししている: {l}"
    );
    assert!(
        l.contains("ほかに作業ログに rebase で戻った・重複したエントリ: 1 件"),
        "{l}"
    );
    assert!(!l.contains("tako context-budget fix"), "{l}");

    // 件数は応答にも分けて載る（復活は予算超過の件数に混ぜない）
    let r = mcp(&both);
    assert_eq!(r["violations"], 3, "{r:#}");
    assert_eq!(r["revived_entries"], 1, "{r:#}");
    assert_eq!(r["fixable"], 2, "{r:#}");

    // ── 英語 ──
    tako_core::i18n::set_lang(Lang::En);

    let l = line(&revived);
    assert!(
        l.starts_with("Work log entries revived or duplicated by a rebase: 1"),
        "{l}"
    );
    assert!(!l.contains("over budget"), "{l}");
    assert!(l.contains("fix cannot repair them"), "{l}");
    assert!(!l.contains("tako context-budget fix"), "{l}");
    assert!(l.ends_with("How to fix: tako context-budget"), "{l}");

    let l = line(&over);
    assert!(
        l.starts_with("Startup load is over budget: 2 item(s)"),
        "{l}"
    );
    assert!(
        l.contains("Fix now: tako context-budget fix (2 auto-fixable)"),
        "{l}"
    );
    assert!(!l.contains("rebase"), "{l}");

    let l = line(&both);
    assert!(
        l.starts_with("Startup load is over budget: 2 item(s)"),
        "{l}"
    );
    assert!(
        l.contains("Also, work log entries revived or duplicated by a rebase: 1"),
        "{l}"
    );

    let l = line(&not_fixable);
    assert!(
        l.starts_with("Startup load is over budget: 1 item(s)"),
        "{l}"
    );
    assert!(l.contains("activeContext.md: lines 92 > 80. "), "{l}");
    assert!(!l.contains('。'), "英語の 1 行に和文の句点が混ざる: {l}");

    std::fs::remove_dir_all(&base).ok();
}
