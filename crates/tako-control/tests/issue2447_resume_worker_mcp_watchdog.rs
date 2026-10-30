//! 番犬: 再開（resume）で起こす worker も spawn と同じ MCP の断片を持つ（#2447）
//!
//! ## なぜ止めるのか
//!
//! worker は spawn で tako の MCP だけを持って起動する（#2412 の `--settings` / #2418 の
//! `-c mcp_servers.<名>.enabled=false` / #986 の tako の MCP の一時注入）。ところが再起動をまたいだ
//! 復元（`codex resume <id>` / `claude --resume <id>`）は別の組み立て（カタログのメタだけ）を通って
//! いたので、**GUI を起こし直すたびに対策が外れ**、復元した worker が user 設定の MCP を全部持って
//! 戻っていた（10/25 にメモリが尽きて PC が落ちた主因 = #1992 のコメント）。
//!
//! 単体テストは今の出力しか見ないので、同じ形の再発（新しい再開の経路が断片を通し忘れる・
//! 断片の写しが経路ごとに増える）はソースの形で止める:
//!
//! 1. [`再開の経路は_worker_の_mcp_を通す`] — GUI の復元（`restore_plan`）とカタログの再開
//!    （`resume_launch` の catalog）が `resume_worker_mcp` → `prepare_for_resume`（spawn と同じ判断）と
//!    `resume_mcp_args` → `worker_mcp_args`（spawn と同じ組み立て）を通る
//! 2. [`mcp_の断片を組むのは_worker_mcp_args_の_1_実装`] — `--settings` と codex の `-c` を起動コマンドへ
//!    書く製品コードが `agent::worker_mcp_args` の中だけ（master の tako の MCP の一時注入は別扱い）

use std::path::{Path, PathBuf};

fn workspace_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）。
/// **`mod tests` の直前で切り**、行コメントを落とす（#2412 の番犬と同じ切り方）
fn production_source(src: &str) -> String {
    let body = match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    };
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_production(rel: &str) -> String {
    let path = workspace_file(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読む: {e}"));
    production_source(&src)
}

/// `head` で始まる関数の本体（次のトップレベルの `fn` / `pub fn` の手前まで）
fn fn_body<'a>(src: &'a str, rel: &str, head: &str) -> &'a str {
    let start = src
        .find(head)
        .unwrap_or_else(|| panic!("{rel} に `{head}` が無い（改名したら番犬も直す）"));
    let rest = &src[start + head.len()..];
    let end = ["\nfn ", "\npub fn ", "\npub(crate) fn ", "\n#[derive"]
        .iter()
        .filter_map(|m| rest.find(m))
        .min()
        .unwrap_or(rest.len());
    &src[start..start + head.len() + end]
}

fn require(body: &str, rel: &str, needle: &str, why: &str) {
    assert!(
        body.contains(needle),
        "{rel}: `{needle}` が無い = {why}（#2447）:\n{body}"
    );
}

/// 1: GUI の復元とカタログの再開は、spawn と同じ判断・同じ組み立てを通る
#[test]
fn 再開の経路は_worker_の_mcp_を通す() {
    const SESSIONS: &str = "crates/tako-control/src/sessions.rs";
    const RESUME: &str = "crates/tako-control/src/resume_launch.rs";
    let sessions = read_production(SESSIONS);
    let resume = read_production(RESUME);

    let restore = fn_body(&sessions, SESSIONS, "pub fn restore_plan(");
    require(
        restore,
        SESSIONS,
        "restore_worker_mcp_args(",
        "再起動をまたいだ復元が worker の MCP を決めていない（復元した worker は user 設定の MCP を全部持ち込む）",
    );
    let decide = fn_body(&sessions, SESSIONS, "fn restore_worker_mcp_args(");
    for (needle, why) in [
        (
            "resume_worker_mcp(",
            "復元が spawn と同じ判断を通っていない",
        ),
        (
            "resume_mcp_args(",
            "復元が spawn と同じ組み立てを通っていない",
        ),
        (
            ".materialized()",
            "claude の `--settings` のファイルを書かずに起動する（復元はその場で打つ）",
        ),
    ] {
        require(decide, SESSIONS, needle, why);
    }
    let pick = fn_body(&sessions, SESSIONS, "pub fn resume_worker_mcp(");
    require(
        pick,
        SESSIONS,
        "worker_mcp::prepare_for_resume(",
        "再開の判断が spawn の worker_mcp::prepare と別の写しになっている",
    );
    let args = fn_body(&sessions, SESSIONS, "pub fn resume_mcp_args(");
    require(
        args,
        SESSIONS,
        "agent::worker_mcp_args(",
        "再開の断片が spawn の組み立てと別の写しになっている",
    );
    // 純関数の側: worker の記録を使うときだけ差し込む
    let plan_in = fn_body(&sessions, SESSIONS, "pub fn restore_plan_in(");
    require(
        plan_in,
        SESSIONS,
        "input.worker_mcp_args",
        "決めた断片を起動コマンドへ差し込んでいない",
    );

    // カタログの再開（codex の `tako sessions resume` / 会話を保った再起動）と claude の worker の再開
    require(
        &resume,
        RESUME,
        "crate::sessions::resume_worker_mcp(",
        "カタログの再開が worker の MCP を決めていない",
    );
    require(
        &resume,
        RESUME,
        "crate::sessions::resume_command_typed(session_id, entry, &mcp_args)",
        "カタログの再開が決めた断片をコマンドへ渡していない",
    );
    let worker = fn_body(&resume, RESUME, "fn resume_worker_launch(");
    require(
        worker,
        RESUME,
        "crate::worker_mcp::prepare_for_resume(",
        "claude の worker の再開が復元と別の判断を通っている",
    );
}

/// 2: `--settings` と codex の `-c`（止める宣言・tako の MCP の一時注入）を worker の起動コマンドへ
/// 書くのは `worker_mcp_args` の中だけ（spawn と再開で写しが分かれると片方だけ外れる）
#[test]
fn mcp_の断片を組むのは_worker_mcp_args_の_1_実装() {
    const AGENT: &str = "crates/tako-control/src/orchestrator/agent.rs";
    let agent = read_production(AGENT);
    let frag = fn_body(&agent, AGENT, "pub fn worker_mcp_args(");
    for needle in [
        "\" --settings {}\"",
        "codex_mcp_args(tako_bin, dialect)",
        "codex_off_args(",
    ] {
        require(
            frag,
            AGENT,
            needle,
            "worker の MCP の断片の組み立てが 1 実装から外れた",
        );
    }
    let spawn = fn_body(&agent, AGENT, "pub fn build_worker_cmd_in(");
    require(
        spawn,
        AGENT,
        "worker_mcp_args(",
        "spawn が worker_mcp_args を通らずに MCP の断片を組んでいる",
    );

    // 製品コード全体で、断片を書く場所を数える（master の一時注入 = orchestrator/mod.rs は別の起動）
    let dir = workspace_file("crates/tako-control/src");
    let mut settings_writers = Vec::new();
    let mut off_writers = Vec::new();
    let mut inject_writers = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let body = production_source(&std::fs::read_to_string(&p).unwrap());
            let rel = p
                .strip_prefix(&dir)
                .unwrap()
                .display()
                .to_string()
                .replace('\\', "/");
            for (needle, list) in [
                ("\" --settings {}\"", &mut settings_writers),
                ("codex_off_args(", &mut off_writers),
                ("codex_mcp_args(", &mut inject_writers),
            ] {
                let n = body.matches(needle).count();
                if n > 0 {
                    list.push(format!("{rel}×{n}"));
                }
            }
        }
    }
    settings_writers.sort();
    off_writers.sort();
    inject_writers.sort();
    assert_eq!(
        settings_writers,
        ["orchestrator/agent.rs×1"],
        "`--settings` を起動コマンドへ書く場所が worker_mcp_args の外にもある"
    );
    // worker_mcp.rs の 1 件は定義（`pub fn codex_off_args(`）
    assert_eq!(
        off_writers,
        ["orchestrator/agent.rs×1", "worker_mcp.rs×1"],
        "codex の止める -c を組む場所が worker_mcp_args の外にもある"
    );
    // agent.rs は定義 + worker_mcp_args の 1 件、mod.rs は master の起動（#986 の正本を共有する）
    assert_eq!(
        inject_writers,
        ["orchestrator/agent.rs×2", "orchestrator/mod.rs×1"],
        "tako の MCP の一時注入を組む場所が増えた（worker は worker_mcp_args を通す）"
    );
}
