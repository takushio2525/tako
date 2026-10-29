//! 番犬: 突然死の復旧案内（`resume_command`）は「打った時点で spawn と同じ組み立て」の 1 行（#2470）
//!
//! ## なぜ止めるのか
//!
//! worker_status / watch / workers の `resume_command` は、#2470 の前は照会のたびに
//! `cd <cwd> && claude --model … --effort … --resume <id>` を手で組んでいた。spawn の起動
//! （`build_worker_cmd_in`）とは別の写しなので、#2412 の `--settings`（user scope の MCP を止める）・
//! プロファイルの args（本番は全プロファイルが `--permission-mode auto`）と env（`GH_CONFIG_DIR` 等）・
//! `TAKO_ORCHESTRATOR_ROLE` が全部落ち、案内どおりに戻した worker は MCP を全部持ち、許可ダイアログで止まった。
//!
//! いまは案内を `tako orchestrator resume <worker_id>` の 1 行にし、起動コマンドは**打った時点で**
//! GUI の復元・`tako sessions resume` と同じ 1 実装（`resume_launch::worker_resume_launch`）で組む。
//! 照会のたびに組むと spawn と同じ判断のために user scope の `.claude.json`（1 本 260〜350 KB ×
//! アカウント数）を読むことになるので、照会の経路は**ファイルを読まない**。単体テストは今の出力しか
//! 見ないので、同じ形の再発はソースの形で止める:
//!
//! 1. [`照会の案内は手組みもファイルの読み込みもしない`] — `registry::resume_command` に claude の起動の
//!    手組み・会話の記録の探索・設定の読み込みが無い（修正前の形は `TAKO_2470_LEGACY=1` の腕だけ）
//! 2. [`組み立て本体を呼ぶのは打った時点の_cli_だけ`] — `registry_resume_launch` / 修正前の
//!    `legacy_resume_command` を呼ぶ製品コードが決まった場所だけ（照会の経路から呼ばない）
//! 3. [`打った時点の組み立ては_1_実装を通る`] — CLI の `orchestrator_resume` が `registry_resume_launch` →
//!    `worker_resume_launch`（spawn の `build_worker_cmd_in`）を通り、MCP の設定ファイルを書いてから走らせる
//! 4. [`無人の復旧は_path_の_tako_に頼らない`] — supervisor の自動復旧（`recover_dead`）が死んだペインへ
//!    打つ 1 行は、表示用の素の `tako …` ではなく `unattended_resume_line`（動いている tako と同じ場所の
//!    CLI の絶対パス）。PATH の先頭に古い開発ビルドの `tako` が居る機械では、素の `tako` は
//!    `orchestrator resume` を知らずに落ちる
//!
//! 落ちたときは `ファイル:行` を名指しする。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// テストモジュールより手前の行（1 始まりの行番号つき）。行コメントは落とす
fn production_lines(src: &str) -> Vec<(usize, &str)> {
    let end = src.find("\n#[cfg(test)]\nmod tests").unwrap_or(src.len());
    src[..end]
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

/// `head` で始まる関数の行（次のトップレベルの項目の手前まで）
fn fn_lines<'a>(src: &'a str, rel: &str, head: &str) -> Vec<(usize, &'a str)> {
    let lines = production_lines(src);
    let start = lines
        .iter()
        .position(|(_, l)| l.starts_with(head))
        .unwrap_or_else(|| panic!("{rel} に `{head}` が無い（改名したら番犬も直す）"));
    let mut out = vec![lines[start]];
    for &(n, l) in &lines[start + 1..] {
        let top = [
            "fn ",
            "pub fn ",
            "pub(crate) fn ",
            "#[",
            "///",
            "impl ",
            "pub struct ",
        ]
        .iter()
        .any(|m| l.starts_with(m));
        if top {
            break;
        }
        out.push((n, l));
    }
    out
}

fn require(lines: &[(usize, &str)], rel: &str, needle: &str, why: &str) {
    if !lines.iter().any(|(_, l)| l.contains(needle)) {
        panic!(
            "{rel}:{}: `{needle}` が無い = {why}（#2470）",
            lines.first().map(|(n, _)| *n).unwrap_or(0)
        );
    }
}

fn forbid(lines: &[(usize, &str)], rel: &str, needle: &str, why: &str) {
    if let Some((n, l)) = lines.iter().find(|(_, l)| l.contains(needle)) {
        panic!(
            "{rel}:{n}: `{needle}` がある = {why}（#2470）\n  {}",
            l.trim()
        );
    }
}

/// 製品コード（各 crate の src）で `needle` を含む行を `ファイル:行` で集める
fn production_hits(needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack: Vec<PathBuf> = ["tako-core", "tako-control", "tako-cli", "tako-app"]
        .iter()
        .map(|c| workspace_root().join("crates").join(c).join("src"))
        .collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            let rel = p
                .strip_prefix(workspace_root())
                .unwrap_or(&p)
                .display()
                .to_string()
                .replace('\\', "/")
                .trim_start_matches("./")
                .to_string();
            for (n, l) in production_lines(&src) {
                if l.contains(needle) {
                    hits.push(format!("{rel}:{n}"));
                }
            }
        }
    }
    hits.sort();
    hits
}

const REGISTRY: &str = "crates/tako-control/src/orchestrator/registry.rs";
const RESUME: &str = "crates/tako-control/src/resume_launch.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// 1: 照会の案内は 1 行を返すだけ（起動の手組み・会話の記録の探索・設定の読み込みをしない）
#[test]
fn 照会の案内は手組みもファイルの読み込みもしない() {
    let src = read(REGISTRY);
    let body = fn_lines(&src, REGISTRY, "pub fn resume_command(");
    // 禁止語を先に見る（修正前の形へ戻したとき、差し込まれた行そのものを名指しする）
    for (needle, why) in [
        (
            "\"claude\")",
            "照会が claude の起動コマンドを手で組んでいる（spawn と別の写し = --settings・許可モード・env が落ちる）",
        ),
        ("--model", "照会が起動の引数を手で組んでいる（spawn と別の写し）"),
        (
            "--resume",
            "照会が --resume の起動コマンドを手で組んでいる（spawn と別の写し）",
        ),
        (
            "resume_env_prefix",
            "照会のたびに会話の記録の所在を探している",
        ),
        ("locate_transcript", "照会のたびに会話の記録を探している"),
        ("std::fs", "照会がファイルを読んでいる"),
        ("::load(", "照会が設定を読んでいる"),
        (
            "worker_mcp",
            "照会が MCP の判断（user scope の .claude.json の読み込み）を通っている",
        ),
        (
            "resume_launch",
            "照会が起動の組み立て本体（.claude.json を読む）を呼んでいる",
        ),
    ] {
        forbid(&body, REGISTRY, needle, why);
    }
    require(
        &body,
        REGISTRY,
        "\"tako orchestrator resume {worker_id}\"",
        "案内が打った時点で組む 1 行になっていない",
    );
    // 修正前の形へ戻るのは A/B の腕だけ
    require(
        &body,
        REGISTRY,
        "if legacy_2470() {",
        "修正前の案内へ戻る腕が A/B の判定の外にある",
    );
}

/// 2: 組み立て本体（.claude.json を読む）と修正前の案内を呼ぶのは決まった場所だけ
#[test]
fn 組み立て本体を呼ぶのは打った時点の_cli_だけ() {
    let launch = production_hits("registry_resume_launch(");
    let allowed_launch = [RESUME, CLI];
    let stray: Vec<&String> = launch
        .iter()
        .filter(|h| {
            !allowed_launch
                .iter()
                .any(|a| h.starts_with(&format!("{a}:")))
        })
        .collect();
    assert!(
        stray.is_empty(),
        "registry_resume_launch を照会などの経路から呼んでいる = 照会のたびに user scope の \
         .claude.json を読む（打った時点の CLI だけが呼ぶ。#2470）: {stray:?}"
    );
    assert!(
        launch.iter().any(|h| h.starts_with(&format!("{CLI}:"))),
        "CLI が registry_resume_launch を呼んでいない（#2470）: {launch:?}"
    );

    let legacy = production_hits("legacy_resume_command(");
    let allowed_legacy = [REGISTRY, CLI];
    let stray: Vec<&String> = legacy
        .iter()
        .filter(|h| {
            !allowed_legacy
                .iter()
                .any(|a| h.starts_with(&format!("{a}:")))
        })
        .collect();
    assert!(
        stray.is_empty(),
        "修正前の案内（spawn と別の写し）を A/B の腕以外から使っている（#2470）: {stray:?}"
    );

    // 照会の 2 か所（worker_status / workers）は純関数の案内を通す
    let dispatch_rel = "crates/tako-control/src/dispatch.rs";
    let dispatch = read(dispatch_rel);
    let lines = production_lines(&dispatch);
    require(
        &lines,
        dispatch_rel,
        "orchestrator::registry::resume_command(id, e)",
        "worker_status の案内が registry::resume_command（純関数）を通っていない",
    );
    let registry = read(REGISTRY);
    let list = fn_lines(&registry, REGISTRY, "pub fn list_payload(");
    require(
        &list,
        REGISTRY,
        "resume_command(id, e)",
        "workers の案内が resume_command（純関数）を通っていない",
    );
}

/// 3: 打った時点の組み立ては GUI の復元・`tako sessions resume` と同じ 1 実装を通り、
/// MCP の設定ファイルを書いてから、このシェルで走らせる
#[test]
fn 打った時点の組み立ては_1_実装を通る() {
    let cli = read(CLI);
    let body = fn_lines(&cli, CLI, "fn orchestrator_resume(");
    for (needle, why) in [
        (
            "resume_launch::registry_resume_launch(entry)",
            "CLI が spawn と同じ組み立てを通らずに起動コマンドを作っている",
        ),
        (
            "launch.materialize()",
            "MCP の絞り込みの設定ファイルを書かずに起動する（--settings のファイルが無い）",
        ),
        (
            "run_launch_line(&command)",
            "組み立てた 1 行をこのシェルで走らせていない",
        ),
    ] {
        require(&body, CLI, needle, why);
    }
    for (needle, why) in [
        ("\"claude\")", "CLI が claude の起動を手で組んでいる"),
        ("--model", "CLI が起動の引数を手で組んでいる"),
        ("--resume", "CLI が --resume を手で足している"),
    ] {
        forbid(&body, CLI, needle, why);
    }

    let src = read(RESUME);
    let inner = fn_lines(&src, RESUME, "pub fn registry_resume_launch_in(");
    require(
        &inner,
        RESUME,
        "worker_resume_launch(",
        "復旧が GUI の復元・`tako sessions resume` と別の組み立てを通っている",
    );
    for needle in ["build_worker_cmd", "--settings", "--model"] {
        forbid(
            &inner,
            RESUME,
            needle,
            "復旧が spawn の組み立ての写しを持っている（1 実装から外れた）",
        );
    }
}

/// 4: 無人の復旧（supervisor）は PATH の先頭の `tako` に頼らず、動いている tako と同じ場所の CLI を打つ
#[test]
fn 無人の復旧は_path_の_tako_に頼らない() {
    const SUPERVISOR: &str = "crates/tako-control/src/orchestrator/supervisor.rs";
    let src = read(SUPERVISOR);
    let dead = fn_lines(&src, SUPERVISOR, "pub fn recover_dead(");
    forbid(
        &dead,
        SUPERVISOR,
        "format!(\"{cmd}\\r\")",
        "表示用の案内（素の `tako …`）をそのまま打っている = PATH の先頭の古い tako に当たる",
    );
    require(
        &dead,
        SUPERVISOR,
        "unattended_resume_line(cmd, &ctx.resume_shell)",
        "無人で打つ 1 行を動いている tako と同じ場所の CLI へ書き換えていない",
    );
    let lp = fn_lines(&src, SUPERVISOR, "pub fn supervisor_loop(");
    require(
        &lp,
        SUPERVISOR,
        "ResumeShell::current()",
        "supervisor が打ち込む先のシェル（隣の CLI）を実物から引いていない",
    );

    let registry = read(REGISTRY);
    let line = fn_lines(&registry, REGISTRY, "pub fn unattended_resume_line(");
    for (needle, why) in [
        ("shell.cli", "隣の CLI を使っていない"),
        (
            "ShellDialect::PowerShell => format!(\"& {program} {args}\")",
            "PowerShell で引用したパスを呼び出し演算子なしで打っている（文字列として評価され起動しない）",
        ),
    ] {
        require(&line, REGISTRY, needle, why);
    }
    let current = fn_lines(&registry, REGISTRY, "    pub fn current() -> Self {");
    require(
        &current,
        REGISTRY,
        "tako_cli_path::current_cli()",
        "隣の CLI を実体の在り処の正本（tako_cli_path::current_cli）から引いていない",
    );
}
