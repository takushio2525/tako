//! 個人の追記（local-rules.md）の区切りを上げる案の番犬（Issue #1981）
//!
//! #1477 の自動分割は「区切りがまだ無いファイル」へ 1 回入れるだけなので、その後に
//! 利用者が常時部へ規則を書き足して取り分（5,632 B）を超えても `tako migrate status` は
//! up_to_date のまま、`tako migrate run` も直さなかった。#1981 で
//!
//! 1. `tako migrate status` にだけ「N バイト超過。区切りを『## X』の前へ上げる案」を出し
//! 2. 明示の `tako migrate run`（MCP `tako_migrate` の run）でだけ当て
//! 3. 自動の発火点（`tako setup` / GUI・master 起動時）は**動かさない**
//!
//! ようにした。ここが縛るのはこの 3 つと、当てたときに**区切りの行を除いた本文が
//! バイト単位で一致する**こと・`tako context-budget` の違反が消えること。
//!
//! CLI（`tako migrate`）は `migrations::report_json` を直に呼ぶので、ここでも同じ関数を叩く。
//! MCP は `tools/call tako_migrate` を本物の変換（`handle_message`）に通す。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use tako_control::migrations::{self, Mode};
use tako_control::protocol::Request;
use tako_core::context_budget as budget;
use tako_core::migration::SchemaId;
use tako_core::prompt_append::{self, MARKER};

/// **本番の data dir と HOME を読ませない**。このバイナリ全体で同じ 1 か所を指す
/// （`migrations::run` はプロファイルを全部走査するので、テスト同士は [`serial`] で直列にする）
// tako:env-write-ok このバイナリの全テストが先にこれを通し（serial() で 1 本ずつ）、入れる値は 1 つ（OnceLock）で書き換えはこの 1 回だけ（#2400）
fn sandbox() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("tako-test-data-{}", std::process::id()));
        let dir = base.join("marker-move-1981");
        let _ = std::fs::create_dir_all(dir.join("data/orchestrator/profiles"));
        tako_core::test_residue::arm_self_cleanup(&base);
        std::env::set_var("TAKO_DATA_DIR", dir.join("data"));
        std::env::set_var("HOME", &dir);
        dir
    })
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 追記ファイルを置き、それを指すプロファイルを 1 本だけにする（前のテストの残りは消す）
fn setup(name: &str, text: &str) -> PathBuf {
    let home = sandbox();
    let profiles = home.join("data/orchestrator/profiles");
    let _ = std::fs::remove_dir_all(&profiles);
    std::fs::create_dir_all(&profiles).unwrap();
    let file = home.join(format!("{name}.md"));
    for side in [".pre-v1.bak", ".pre-v2.bak"] {
        let _ = std::fs::remove_file(home.join(format!("{name}.md{side}")));
    }
    std::fs::write(&file, text).unwrap();
    std::fs::write(
        profiles.join(format!("{name}.yaml")),
        format!("prompt_blocks:\n  append: ~/{name}.md\n"),
    )
    .unwrap();
    file
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// CLI の口（`tako migrate [run] --schema prompt_append` = `migrate_local` と同じ関数）
fn cli(action: &str) -> Value {
    migrations::report_json(action, Some("prompt_append")).expect("migrate")
}

/// MCP の口（`tools/call tako_migrate` を本物の変換に通す。実行係は dispatch の
/// `Request::Migrate` の腕と同じ 1 本 = `report_json`）
fn mcp(args: Value) -> Value {
    let mut exec = |request: Request| -> Result<Value, String> {
        match request {
            Request::Migrate { action, schema } => {
                migrations::report_json(action.as_deref().unwrap_or("status"), schema.as_deref())
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
                "params": {"name": "tako_migrate", "arguments": args}}),
        &mut session,
    )
    .expect("tools/call には応答がある");
    let result = &reply["result"];
    assert_ne!(result["isError"], true, "{reply}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).expect("応答は JSON")
}

/// その追記ファイルの 1 行（`files[]` の中から）
fn entry<'a>(report: &'a Value, path: &Path) -> &'a Value {
    report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path.display().to_string())
        .unwrap_or_else(|| panic!("{} が報告に無い: {report:#}", path.display()))
}

fn cost(text: &str) -> usize {
    let s = prompt_append::split(text);
    prompt_append::always_cost(s.always, s.on_demand)
}

/// #1477 の自動分割を当てた fixture の常時部の末尾（区切りの直前）へ、利用者が
/// 規則を書き足した姿。節が 1 本増え、取り分を `over` バイト以上超える
/// （分割直後は取り分に余りがあるので、余りを食い切るまで行を足す）
fn grown_after_split(over: usize) -> String {
    let original = include_str!("fixtures/prompt_append_oversized.md");
    let split = prompt_append::insert_marker(original, budget::PROMPT_APPEND_MAX_BYTES)
        .expect("#1477 で切れる");
    assert!(
        cost(&split) <= budget::PROMPT_APPEND_MAX_BYTES,
        "前提: 分割直後は収まる"
    );
    let at = split.find(MARKER).unwrap();
    let mut lines = 1;
    loop {
        let section = format!(
            "## あとから書き足した規則\n\n{}\n",
            "- 書き足した行\n".repeat(lines)
        );
        let grown = format!("{}{section}{}", &split[..at], &split[at..]);
        if cost(&grown) >= budget::PROMPT_APPEND_MAX_BYTES + over {
            return grown;
        }
        lines += 1;
    }
}

/// 受け入れ条件 1・2・4: 案は status の 1 か所に出て、明示の run でだけ当たる
#[test]
fn 取り分を超えた常時部は案が出て_明示のrunで見出しの前へ上がる() {
    let _g = serial();
    let text = grown_after_split(300);
    let file = setup("grown", &text);
    let over = cost(&text) - budget::PROMPT_APPEND_MAX_BYTES;

    // ① status（CLI）に案が出る。書き換えない
    let status = cli("status");
    let e = entry(&status, &file);
    assert_eq!(e["state"], "proposed", "{e:#}");
    assert_eq!(e["approvable"], true);
    assert_eq!(e["over_bytes"], over);
    assert_eq!(e["limit"], budget::PROMPT_APPEND_MAX_BYTES);
    assert_eq!(e["move_before"], "## あとから書き足した規則");
    assert_eq!(e["moves_to_on_demand"], json!(["あとから書き足した規則"]));
    assert_eq!(e["approve"], "tako migrate run");
    assert!(
        e["proposal_ja"]
            .as_str()
            .unwrap()
            .contains(&format!("{over} バイト超過")),
        "{e:#}"
    );
    assert_eq!(status["proposed"], 1);
    assert!(status["notice"]
        .as_str()
        .unwrap()
        .contains("tako migrate run"));
    assert_eq!(read(&file), text, "status は書き換えない");

    // ② MCP の status も同じ案を返す（1:1）
    let via_mcp = mcp(json!({"schema": "prompt_append"}));
    assert_eq!(entry(&via_mcp, &file), e, "CLI と MCP で案が食い違う");

    // ③ 自動の発火点（setup / 起動時と同じ `run(Apply)`）は動かさない・案も出さない
    let auto = migrations::run(Mode::Apply, Some(SchemaId::PromptAppend));
    let auto_entry = auto.files.iter().find(|f| f.path == file).unwrap();
    assert_eq!(auto_entry.outcome.kind(), "up_to_date", "{auto_entry:?}");
    assert_eq!(auto.notice(), None, "起動時の一言に出さない（案は 1 か所）");
    assert_eq!(read(&file), text, "自動の発火点が勝手に動かした");
    // `tako setup` の段（`run_data_migration_stage` が呼ぶ 1 本）も同じ
    let lines = migrations::setup_lines();
    assert!(
        lines.iter().all(|l| !l.contains("grown.md")),
        "setup が追記に触れた: {lines:?}"
    );
    assert_eq!(read(&file), text, "tako setup が勝手に動かした");

    // ④ 明示の run（MCP）で当たる
    let run = mcp(json!({"action": "run", "schema": "prompt_append"}));
    let r = entry(&run, &file);
    assert_eq!(r["state"], "migrated", "{r:#}");
    assert_eq!(r["from"], 2);
    assert_eq!(r["to"], 2);
    let after = read(&file);
    // 区切りの行を除いた本文がバイト単位で一致する
    assert_eq!(
        prompt_append::strip_marker(&after).as_bytes(),
        prompt_append::strip_marker(&text).as_bytes()
    );
    assert_eq!(after.len(), text.len(), "区切りの行は 1 本のまま");
    let s = prompt_append::split(&after);
    assert!(
        s.on_demand.starts_with("## あとから書き足した規則\n"),
        "{after}"
    );
    assert!(cost(&after) <= budget::PROMPT_APPEND_MAX_BYTES);
    // 旧内容は移行と同じ作法で退避（`.pre-v2.bak`）
    let backup = PathBuf::from(r["backup"].as_str().unwrap());
    assert!(backup.ends_with("grown.md.pre-v2.bak"), "{backup:?}");
    assert_eq!(read(&backup), text, "退避は当てる前の中身");

    // ⑤ 2 回目は何もしない（冪等）。status も静かになる
    let again = cli("run");
    assert_eq!(entry(&again, &file)["state"], "up_to_date");
    assert_eq!(read(&file), after);
    assert_eq!(cli("status")["proposed"], 0);
}

/// 受け入れ条件 2: 当てると system prompt の予算違反が消える（`tako context-budget` の 1 実装で測る）
#[test]
fn 当てるとcontext_budgetの違反が消える() {
    let _g = serial();
    // base（tako が作る側）の大きさに依らず違反が出るよう、超過を大きめに取る
    let text = grown_after_split(budget::PROMPT_APPEND_MAX_BYTES);
    let file = setup("budget", &text);
    let cwd = sandbox().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let prompt_violations = || -> usize {
        let report = tako_control::context_budget::report(&cwd, Some("budget")).unwrap();
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|it| it["kind"] == "system_prompt")
            .map(|it| it["violations"].as_array().map_or(0, Vec::len))
            .sum()
    };
    assert!(prompt_violations() > 0, "前提: 当てる前は違反がある");
    assert_eq!(entry(&cli("run"), &file)["state"], "migrated");
    assert_eq!(prompt_violations(), 0, "当てたのに違反が残る");
}

/// 受け入れ条件 3: 取り分内なら何も出さない・区切りが無いものは #1477 のまま
#[test]
fn 取り分内なら何も出さず_区切りが無いものは1477のまま() {
    let _g = serial();
    // 区切りがあって取り分内
    let fits = format!("# ルール\n\n## 言語\n\n日本語\n{MARKER}\n## 詳細\n\n本文\n");
    let file = setup("fits", &fits);
    let status = cli("status");
    assert_eq!(entry(&status, &file)["state"], "up_to_date");
    assert_eq!(status["proposed"], 0);
    assert_eq!(status["notice"], Value::Null);
    cli("run");
    assert_eq!(read(&file), fits);

    // 区切りが無く取り分を超える = #1477 の自動分割（案ではなく移行の予告）
    let unmarked = include_str!("fixtures/prompt_append_oversized.md");
    let file = setup("unmarked", unmarked);
    let e = entry(&cli("status"), &file).clone();
    assert_eq!(e["state"], "migrated", "{e:#}");
    assert_eq!(e["from"], 1);
    // 自動の発火点がこれまでどおり区切りを入れる
    migrations::run(Mode::Apply, Some(SchemaId::PromptAppend));
    assert!(read(&file).contains(MARKER));
    assert!(sandbox().join("unmarked.md.pre-v1.bak").is_file());
}

/// 境界: 上げても収まらないものは「動かせない」と出し、run でも触らない
#[test]
fn 上げても収まらないものは案内だけで動かさない() {
    let _g = serial();
    // 常時部に題名以外の見出しが無い
    let flat = format!(
        "{}{MARKER}\n## 詳細\n\n本文\n",
        "見出しの無い常時の規則\n".repeat(400)
    );
    // 題名 + 先頭の節だけで取り分を超える
    let fat = format!(
        "# ルール\n\n{}\n## 小さい節\n\nx\n{MARKER}\n## 詳細\n\n本文\n",
        "前書き".repeat(2000)
    );
    for (name, text) in [("flat", flat), ("fat", fat)] {
        let file = setup(name, &text);
        let e = entry(&cli("status"), &file).clone();
        assert_eq!(e["state"], "proposed", "{name}: {e:#}");
        assert_eq!(e["approvable"], false, "{name}");
        assert!(
            e.get("approve").is_none(),
            "{name}: 当たらない案に承認コマンドを出さない"
        );
        assert!(
            e["proposal_ja"]
                .as_str()
                .unwrap()
                .contains("区切りは動かさない"),
            "{name}: {e:#}"
        );
        let run = cli("run");
        assert_eq!(entry(&run, &file)["state"], "proposed", "{name}");
        assert_eq!(read(&file), text, "{name}: 動かせないのに書き換えた");
        assert!(!sandbox().join(format!("{name}.md.pre-v2.bak")).exists());
    }

    // 区切りが先頭 = 常時部が空。索引だけなら収まる（何も出さない）
    let top = format!("{MARKER}\n# ルール\n\n## A\n\naaa\n");
    let file = setup("top", &top);
    assert_eq!(entry(&cli("status"), &file)["state"], "up_to_date");
}
