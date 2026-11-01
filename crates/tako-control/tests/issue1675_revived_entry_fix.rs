//! `tako context-budget fix` が「アーカイブへ移したのに作業ログにも残っている」二重の状態から
//! 1 回で収束すること（Issue #1675）
//!
//! `fix` の移送は「作業ログからエントリを消す + アーカイブへ 1 行足す」の対。rebase / merge の
//! 3-way マージが**消す側だけ**を失う（衝突マーカーなしで復活 = #1645 の型）と、同じエントリが
//! 両方に残る。修正前の `fix` は、戻ったエントリが予算の内側にあると「変更なし（予算内）」で
//! 何もせず、`check` は「fix では直らない」と言い続けていた（9/23〜24 は 6 本中 4 本を人手で落とした）。
//!
//! 実際の置き場（一時ディレクトリの規約 + 作業ログ + アーカイブ）を作り、
//! **CLI `tako context-budget` と MCP `tako_context_budget` が呼ぶ `report` / `fix`** で確かめる:
//!
//! - 二重の状態へ `fix` を 2 回当てると 1 回目で収束し、2 回目は何も変えない（バイト一致）
//! - `check` は violations で名指しし（file:line の両側）、`next_step` に `fix` を示す（日英）
//! - どれが戻ったものか決まらない形（同じ見出しで本文の違うエントリ）は `fix` が触らない
//! - persist.log には件数と行番号だけを残し、エントリの中身は載せない
//!
//! `HOME` / `TAKO_DATA_DIR` / 表示言語はプロセス全体の状態なので、各テストは
//! `run_alone!` の子（その 1 本だけが走るプロセス）の中で書き換える（#2400）。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tako_control::context_budget as cb;
use tako_control::protocol::Request;
use tako_core::i18n::Lang;
use tako_core::test_residue::ScratchDir;

/// 予算内の作業ログ（普通のエントリ 3 件）の先頭へ、アーカイブ済みのエントリが戻った形
const DOUBLED_PROGRESS: &str = "\
# Progress Log

> 説明

## 2026-10-28（#100: 戻ったエントリ）
- 本文 E

## 2026-10-29（#101: 普通 1）
- 本文

## 2026-10-30（#102: 普通 2）
- 本文

## 2026-10-31（#103: 普通 3）
- 本文
";

const DOUBLED_ARCHIVE: &str = "\
# Progress Archive

- 2026-10-20 #90: もっと古い
- 2026-10-28 #100: 戻ったエントリ
";

/// 戻ったエントリだけを落とした形（残すエントリの本文は 1 文字も変えない）
const CONVERGED_PROGRESS: &str = "\
# Progress Log

> 説明

## 2026-10-29（#101: 普通 1）
- 本文

## 2026-10-30（#102: 普通 2）
- 本文

## 2026-10-31（#103: 普通 3）
- 本文
";

/// 隔離: グローバル指示ファイルもメモリも無い `HOME` と、persist.log の書き先を一時 dir へ向ける。
/// 呼ぶのは `run_alone!` の子の中だけ
fn isolate(scratch: &Path) -> PathBuf {
    tako_core::test_child::assert_alone("isolate");
    let home = scratch.join("h");
    let data = scratch.join("d");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&data).expect("data");
    std::env::set_var("HOME", &home);
    std::env::set_var("USERPROFILE", &home);
    std::env::set_var("TAKO_DATA_DIR", &data);
    data
}

fn write_repo(dir: &Path, progress: &str, archive: &str) {
    std::fs::create_dir_all(dir.join(".agent")).expect("置き場");
    std::fs::write(
        dir.join("AGENTS.md"),
        "# 規約\n\n- 作業ログ: @.agent/progress.md\n",
    )
    .expect("AGENTS.md");
    std::fs::write(dir.join(".agent/progress.md"), progress).expect("progress");
    std::fs::write(dir.join(".agent/progress-archive.md"), archive).expect("archive");
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).expect(rel)
}

fn persist_log(data: &Path) -> String {
    std::fs::read_to_string(data.join("persist.log")).unwrap_or_default()
}

/// 作業ログの項目の violations のうち、復活・重複のもの
fn revived_violations(report: &Value) -> Vec<Value> {
    report["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|it| it["kind"] == "progress_log")
        .flat_map(|it| it["violations"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["metric"] == "revived_entries")
        .collect()
}

/// MCP の口（`tools/call tako_context_budget` を本物の変換に通す。実行係は dispatch の
/// `Request::ContextBudget` の腕と同じ 2 本 = `report` / `fix`）
fn mcp(cwd: &Path, arguments: Value) -> Value {
    let mut exec = |request: Request| -> Result<Value, String> {
        match request {
            Request::ContextBudget {
                action,
                cwd,
                profile,
                dry_run,
                ..
            } => {
                let dir = PathBuf::from(cwd.expect("cwd を渡している"));
                match action.as_deref().unwrap_or("check") {
                    "check" => cb::report(&dir, profile.as_deref()),
                    "fix" => cb::fix(&dir, profile.as_deref(), dry_run),
                    other => Err(format!("想定外の action: {other}")),
                }
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
    let mut args = arguments;
    args["cwd"] = json!(cwd.display().to_string());
    let reply = tako_control::mcp::handle_message(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "tako_context_budget", "arguments": args}}),
        &mut session,
    )
    .expect("tools/call には応答がある");
    let result = &reply["result"];
    assert_ne!(result["isError"], true, "{reply}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).expect("応答は JSON")
}

#[test]
fn 二重の状態へfixを2回当てると1回目で収束し2回目は何も変えない() {
    if tako_core::run_alone!(二重の状態へfixを2回当てると1回目で収束し2回目は何も変えない)
    {
        return;
    }
    let scratch = ScratchDir::new("tako-1675-fix");
    let data = isolate(&scratch);
    let repo = scratch.join("r");
    write_repo(&repo, DOUBLED_PROGRESS, DOUBLED_ARCHIVE);
    tako_core::i18n::set_lang(Lang::Ja);

    // ── check: violations で名指しし、直し方は fix ──
    let r = cb::report(&repo, None).expect("check");
    assert_eq!(r["revived_entries"], 1, "{r:#}");
    assert_eq!(r["revived_fixable"], 1, "{r:#}");
    assert_eq!(r["fixable"], 1, "fix で直るものとして数える: {r:#}");
    let vs = revived_violations(&r);
    assert_eq!(vs.len(), 1, "{r:#}");
    let v = &vs[0];
    assert_eq!(v["kind"], "archived_entry_revived");
    assert_eq!(v["fixable"], true);
    assert_eq!(v["next_step"], "tako context-budget fix", "{v:#}");
    let detail = v["detail"].as_str().unwrap();
    assert!(
        detail.contains(".agent/progress.md:5 ") && detail.contains(".agent/progress-archive.md:4"),
        "両側を file:line で名指しする: {detail}"
    );
    assert!(
        v["note"]
            .as_str()
            .unwrap()
            .contains("`tako context-budget fix` が作業ログ側を落とす"),
        "{v:#}"
    );
    assert!(
        r["proposals"].as_array().unwrap().is_empty(),
        "fix で直るものを人の判断が要る提案に載せない: {r:#}"
    );
    let startup = r["startup_line"].as_str().unwrap();
    assert!(
        startup.contains("作業ログに rebase で戻ったエントリ: 1 件")
            && startup.ends_with("いま直す: tako context-budget fix（自動で直せる 1 件）"),
        "{startup}"
    );

    // ── fix 1 回目: 戻ったエントリだけを作業ログから落とす ──
    let out = cb::fix(&repo, None, false).expect("fix");
    assert_eq!(out["changed"], 1, "{out:#}");
    assert_eq!(out["ok"], true, "収束した: {out:#}");
    assert_eq!(out["violations_after"], 0, "{out:#}");
    let c = &out["changes"][0];
    assert_eq!(c["entries_before"], 4, "{c:#}");
    assert_eq!(c["entries_kept"], 3, "{c:#}");
    assert_eq!(c["entries_archived"], 0, "予算内なので移送はしない: {c:#}");
    assert_eq!(c["entries_dropped_revived"], 1, "{c:#}");
    assert_eq!(c["entries_preserved"], true, "{c:#}");
    assert!(
        c["dropped_revived"][0]
            .as_str()
            .is_some_and(|l| l.contains(".agent/progress.md:5 ")),
        "{c:#}"
    );
    assert_eq!(read(&repo, ".agent/progress.md"), CONVERGED_PROGRESS);
    assert_eq!(
        read(&repo, ".agent/progress-archive.md"),
        DOUBLED_ARCHIVE,
        "アーカイブは 1 文字も変えない"
    );

    // persist.log: 件数と行番号だけ。エントリの中身（見出し・本文）は載せない
    let log = persist_log(&data);
    let line = log
        .lines()
        .find(|l| l.contains("[context-budget]"))
        .unwrap_or_else(|| panic!("persist.log に記録が無い:\n{log}"));
    assert!(
        line.contains("件数=1") && line.contains(" 行=5 ") && line.contains(" 行=4（#1675）"),
        "{line}"
    );
    assert!(
        !line.contains("戻ったエントリ") && !line.contains("本文"),
        "エントリの中身を診断ログに出している: {line}"
    );

    // ── fix 2 回目: 何も変えない（バイト一致・記録も増えない） ──
    let out = cb::fix(&repo, None, false).expect("fix 2 回目");
    assert_eq!(out["changed"], 0, "{out:#}");
    assert_eq!(out["changes"][0]["changed"], false, "{out:#}");
    assert!(
        out["changes"][0].get("entries_dropped_revived").is_none(),
        "{out:#}"
    );
    assert_eq!(read(&repo, ".agent/progress.md"), CONVERGED_PROGRESS);
    assert_eq!(read(&repo, ".agent/progress-archive.md"), DOUBLED_ARCHIVE);
    assert_eq!(persist_log(&data), log, "何もしない回は記録しない");

    let r = cb::report(&repo, None).expect("check 後");
    assert_eq!(r["ok"], true, "{r:#}");
    assert_eq!(r["revived_entries"], 0, "{r:#}");
}

#[test]
fn 英語表示でもmcpの応答で名指しと直し方が英語で出る() {
    if tako_core::run_alone!(英語表示でもmcpの応答で名指しと直し方が英語で出る)
    {
        return;
    }
    let scratch = ScratchDir::new("tako-1675-en");
    isolate(&scratch);
    let repo = scratch.join("r");
    write_repo(&repo, DOUBLED_PROGRESS, DOUBLED_ARCHIVE);
    tako_core::i18n::set_lang(Lang::En);

    let r = mcp(&repo, json!({}));
    let vs = revived_violations(&r);
    assert_eq!(vs.len(), 1, "{r:#}");
    let v = &vs[0];
    assert_eq!(v["next_step"], "tako context-budget fix", "{v:#}");
    let note = v["note"].as_str().unwrap();
    assert!(
        note.contains("`tako context-budget fix` drops the work-log copy"),
        "{note}"
    );
    assert!(
        !note.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)),
        "英語の文に仮名が混ざる: {note}"
    );
    assert!(
        v["detail"]
            .as_str()
            .unwrap()
            .contains("archived entry revived"),
        "{v:#}"
    );
    let startup = r["startup_line"].as_str().unwrap();
    assert!(
        startup.starts_with("Work log entries revived by a rebase: 1 (")
            && startup.ends_with("Fix now: tako context-budget fix (1 auto-fixable)"),
        "{startup}"
    );

    // dry-run は書かずに、落とす予定を名指しする
    let dry = mcp(&repo, json!({"action": "fix", "dry_run": true}));
    assert_eq!(dry["changes"][0]["entries_dropped_revived"], 1, "{dry:#}");
    assert_eq!(
        read(&repo, ".agent/progress.md"),
        DOUBLED_PROGRESS,
        "dry-run は書かない"
    );

    let out = mcp(&repo, json!({"action": "fix"}));
    assert_eq!(out["ok"], true, "{out:#}");
    assert_eq!(read(&repo, ".agent/progress.md"), CONVERGED_PROGRESS);
    assert_eq!(
        cb::dropped_line_in(1, Lang::En),
        "Drop from the work log (already in the archive = entries revived by a rebase): 1"
    );

    // 検算で止まったときの断りは、除去を含む回だけ内訳に除去を足す（日英）
    let refusal = tako_control::Reason::BudgetEntriesMismatch {
        label: ".agent/progress.md".into(),
        before: 4,
        kept: 3,
        archived: 0,
        dropped: 2,
    };
    assert_eq!(
        refusal.text_in(Lang::Ja),
        ".agent/progress.md: 移送でエントリ数が合わない（前 4 / 残 3 + 移送 0 + 除去 2。\
         除去はアーカイブに同じ 1 行が在るものだけ）ので書き込まない"
    );
    assert_eq!(
        refusal.text_in(Lang::En),
        ".agent/progress.md: the entry count does not add up after the move (before 4 / kept 3 \
         + moved 0 + dropped 2; an entry is dropped only when the archive has the same line), \
         so nothing is written"
    );
}

/// 誤って消す側の境界: 同じ見出しで本文の違うエントリが並ぶと、どれが戻ったものか決まらない。
/// `fix` は 1 文字も書かず、`check` は手で戻す手順を提案として名指しする
#[test]
fn どれが戻ったか決まらない二重はfixが触らず手順を名指しする() {
    if tako_core::run_alone!(どれが戻ったか決まらない二重はfixが触らず手順を名指しする)
    {
        return;
    }
    let scratch = ScratchDir::new("tako-1675-ambiguous");
    let data = isolate(&scratch);
    let repo = scratch.join("r");
    let progress = "\
# Progress Log

## 2026-10-28（#100: 同じ見出し）
- 本文 A

## 2026-10-28（#100: 同じ見出し）
- 本文 B

## 2026-10-29（#101: 普通 1）
- 本文
";
    let archive = "# Progress Archive\n\n- 2026-10-28 #100: 同じ見出し\n";
    write_repo(&repo, progress, archive);
    tako_core::i18n::set_lang(Lang::Ja);

    let out = cb::fix(&repo, None, false).expect("fix");
    assert_eq!(out["changed"], 0, "{out:#}");
    assert_eq!(
        read(&repo, ".agent/progress.md"),
        progress,
        "本文 A も B も残す"
    );
    assert_eq!(read(&repo, ".agent/progress-archive.md"), archive);
    assert!(
        !persist_log(&data).contains("[context-budget]"),
        "何も落としていないのに記録している"
    );

    let r = cb::report(&repo, None).expect("check");
    assert_eq!(r["revived_fixable"], 0, "{r:#}");
    assert!(
        revived_violations(&r).iter().all(|v| v["fixable"] == false),
        "{r:#}"
    );
    let manual: Vec<&Value> = r["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["kind"] == "archived_entry_revived")
        .collect();
    assert_eq!(manual.len(), 2, "2 件とも名指しする: {r:#}");
    assert!(
        manual[0]["next_step"]
            .as_str()
            .unwrap()
            .contains("どれが戻ったものか機械には決まらない"),
        "{r:#}"
    );
    assert!(
        r["startup_line"]
            .as_str()
            .unwrap()
            .contains("fix では直らない"),
        "{r:#}"
    );
}
