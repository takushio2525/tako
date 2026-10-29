//! 番犬: worker の再開（再起動をまたぐ GUI の復元・`tako sessions resume`・会話を保った再起動）は
//! spawn と**同じ関数**で起動の引数を組む（#2473）
//!
//! ## なぜ止めるのか
//!
//! GUI を起こし直したときの復元（器が死んで `claude … --resume <id>` / `codex resume … <id>` で起こし直す
//! 経路 = `sessions::restore_plan`）は、#2473 の前はカタログのメタだけで起動コマンドを組んでいた。
//! spawn が付ける `--permission-mode auto`（プロファイルの `worker_agents.<agent>.args`）・`--remote-control`・
//! プロファイルの env・codex の `--model` / `-c model_reasoning_effort=` が再起動のたびに落ち、`--model` は
//! `claude agents` が報告した実モデルへ化けていた（隔離 GUI の実測 = `scripts/test-resume-launch-args-2473.sh`）。
//! ユーザー方針で worker は必ずオートモードなので、`--permission-mode auto` が落ちた worker は許可の
//! ダイアログで止まる。
//!
//! 単体テストは今の出力しか見ないので、同じ形の再発（新しい再開の経路がカタログのメタへ戻る・
//! 引数の組み立ての写しが経路ごとに増える）はソースの形で止める:
//!
//! 1. [`再起動をまたぐ復元は_worker_を_spawn_と同じ組み立てで組む`] — `restore_plan` が
//!    `restore_worker_command` → `resume_launch::worker_resume_launch` を通り、純関数側がそのコマンドを打つ。
//!    GUI（tako-app）が手掛かり（`restore_hints`）を渡す
//! 2. [`再開の入口は_worker_resume_launch_の_1_実装`] — `resume_launch_in`（`tako sessions resume` /
//!    会話を保った再起動）も同じ `worker_resume_launch` を通り、その本体が spawn の材料をすべて
//!    `build_worker_cmd_in` へ渡す
//! 3. [`spawn_と再開は同じ関数で引数を組む`] — 会話 ID を `WorkerLaunch` へ渡す（= 再開を組む）製品コードは
//!    resume_launch.rs の 1 か所だけ。再開の書式は `build_worker_cmd_in` の中で正本
//!    （`agent_resume::resume_spec`）から引く
//! 4. [`修正を戻す注入を名指しで落とす`] — 現行ソースから作り直した注入（#2473 の修正を 1 か所ずつ戻す）が
//!    1〜3 のどれかで `file:line` つきで落ちる（番犬が空振りしていないことの確認）

use std::path::{Path, PathBuf};

const SESSIONS: &str = "crates/tako-control/src/sessions.rs";
const RESUME: &str = "crates/tako-control/src/resume_launch.rs";
const AGENT: &str = "crates/tako-control/src/orchestrator/agent.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const APP: &str = "crates/tako-app/src/main.rs";

fn workspace_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）。
/// **`mod tests` の直前で切り**、行コメントは空行にする（行番号を保ったまま名指すため）
fn production_source(src: &str) -> String {
    let body = match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    };
    body.lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone)]
struct Sources {
    sessions: String,
    resume: String,
    agent: String,
    dispatch: String,
    app: String,
}

impl Sources {
    fn current() -> Self {
        let read = |rel: &str| {
            let src = std::fs::read_to_string(workspace_file(rel))
                .unwrap_or_else(|e| panic!("{rel} を読む: {e}"));
            production_source(&src)
        };
        Self {
            sessions: read(SESSIONS),
            resume: read(RESUME),
            agent: read(AGENT),
            dispatch: read(DISPATCH),
            app: read(APP),
        }
    }
}

/// `head` を含む行から、その行と同じ字下げで閉じる `}` までの窓（1 始まりの行番号つき）
fn window<'a>(src: &'a str, file: &str, head: &str) -> Result<Vec<(usize, &'a str)>, String> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(head))
        .ok_or_else(|| format!("{file}:1: `{head}` が無い（改名したら番犬も直す）"))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines[start + 1..]
        .iter()
        .position(|l| *l == close)
        .map(|i| start + 1 + i)
        .ok_or_else(|| format!("{file}:{}: `{head}` の窓が閉じない", start + 1))?;
    Ok((start..=end).map(|i| (i + 1, lines[i])).collect())
}

/// 窓の中に `pat` があること（無ければ窓の先頭行で名指す）
fn require(win: &[(usize, &str)], file: &str, pat: &str, why: &str) -> Result<(), String> {
    if win.iter().any(|(_, l)| l.contains(pat)) {
        return Ok(());
    }
    let head = win.first().map_or(0, |(n, _)| *n);
    Err(format!("{file}:{head}: {why}（`{pat}` が無い。#2473）"))
}

/// 窓の中に `pat` が無いこと（あれば最初の行で名指す）
fn forbid(win: &[(usize, &str)], file: &str, pat: &str, why: &str) -> Result<(), String> {
    match win.iter().find(|(_, l)| l.contains(pat)) {
        Some((n, _)) => Err(format!("{file}:{n}: {why}（`{pat}`。#2473）")),
        None => Ok(()),
    }
}

/// `WorkerLaunch {` から対応する `}` までを 1 件ずつ、先頭の行番号つきで切り出す（波括弧の釣り合いで数える）。
/// 直前が英数字のもの（`ResolvedWorkerLaunch {`）は別の型なので飛ばす
fn worker_launch_blocks(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = src[from..].find("WorkerLaunch {") {
        let start = from + rel;
        if src[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            from = start + "WorkerLaunch {".len();
            continue;
        }
        let mut depth = 0usize;
        let mut end = start;
        for (i, c) in src[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let line = src[..start].matches('\n').count() + 1;
        out.push((line, src[start..=end].to_string()));
        from = end + 1;
    }
    out
}

/// 1. GUI の復元は worker を spawn と同じ組み立てで組む
fn 復元の縛り(s: &Sources) -> Result<(), String> {
    let restore = window(&s.sessions, SESSIONS, "pub fn restore_plan(")?;
    require(
        &restore,
        SESSIONS,
        "restore_worker_command(&input, hints)",
        "再起動をまたぐ復元が worker を spawn と同じ組み立てで組んでいない（カタログのメタだけ = \
         `--permission-mode auto`・`--remote-control`・プロファイルの args / env が落ちる）",
    )?;
    require(
        &restore,
        SESSIONS,
        "worker_command: worker_command.as_deref()",
        "組んだコマンドを純関数へ渡していない",
    )?;
    let build = window(&s.sessions, SESSIONS, "fn restore_worker_command(")?;
    require(
        &build,
        SESSIONS,
        "crate::resume_launch::worker_resume_launch(",
        "復元が `tako sessions resume` / 会話を保った再起動と別の組み立てを通っている",
    )?;
    require(
        &build,
        SESSIONS,
        ".materialize()",
        "MCP の設定ファイルを書かずに起動する（復元はその場で打つ）",
    )?;
    let plan_in = window(&s.sessions, SESSIONS, "pub fn restore_plan_in(")?;
    require(
        &plan_in,
        SESSIONS,
        "input.worker_command",
        "組んだ worker のコマンドを打っていない",
    )?;
    // GUI が手掛かり（起動した master のプロファイル・レジストリの記録）を組んで渡す
    let lines: Vec<&str> = s.app.lines().collect();
    let call = lines
        .iter()
        .position(|l| l.contains("tako_control::sessions::restore_plan("))
        .ok_or_else(|| format!("{APP}:1: GUI の復元が restore_plan を呼んでいない（#2473）"))?;
    let args = lines[call..lines.len().min(call + 10)].join("\n");
    let args = &args[..args.find(");").unwrap_or(args.len())];
    if !args.contains("&hints") {
        return Err(format!(
            "{APP}:{}: GUI の復元が worker の手掛かりを restore_plan へ渡していない（#2473）",
            call + 1
        ));
    }
    if !lines[call.saturating_sub(12)..call]
        .iter()
        .any(|l| l.contains("let hints = tako_control::resume_launch::restore_hints("))
    {
        return Err(format!(
            "{APP}:{}: GUI の復元が worker の手掛かりを `resume_launch::restore_hints` で組んでいない \
             （起動した master のプロファイルが既定へ化け、レジストリの記録を使わない。#2473）",
            call + 1
        ));
    }
    Ok(())
}

/// 2. 再開の入口は worker_resume_launch の 1 実装で、spawn の材料をすべて渡す
fn 入口の縛り(s: &Sources) -> Result<(), String> {
    let entry = window(&s.resume, RESUME, "pub fn resume_launch_in(")?;
    require(
        &entry,
        RESUME,
        "worker_resume_launch(",
        "`tako sessions resume` / 会話を保った再起動の worker が GUI の復元と別の組み立てを通っている",
    )?;
    let route = window(&s.resume, RESUME, "pub fn worker_resume_launch(")?;
    require(
        &route,
        RESUME,
        "supports(agent, keys::RESUME_LAUNCH_ARGS)",
        "対象の系統を能力マトリクスへ問っていない（判断が散る = #982）",
    )?;
    require(
        &route,
        RESUME,
        "resume_worker_launch(",
        "worker の組み立て本体を通っていない",
    )?;
    let body = window(&s.resume, RESUME, "fn resume_worker_launch(")?;
    for (pat, why) in [
        (
            "orchestrator::agent::build_worker_cmd_in(",
            "spawn と同じ関数で組んでいない",
        ),
        (
            "resume: Some(session_id)",
            "会話 ID を spawn と同じ関数へ渡していない（書式の写しが増える）",
        ),
        (
            "extra_args: &launch.extra_args",
            "プロファイルの args（`--permission-mode auto` 等）が落ちる",
        ),
        (
            "remote_control: remote_control.enabled()",
            "Remote Control が spawn と別の判定になる",
        ),
        (
            "skip_permissions: launch.skip_permissions",
            "許可の判断が spawn と別になる",
        ),
        (
            "allow_sandbox_bypass: launch.allow_sandbox_bypass",
            "codex のサンドボックスの判断が spawn と別になる",
        ),
        ("env: &plan", "プロファイルの env が落ちる"),
        (
            "model: launch.model.as_deref()",
            "model が spawn と同じ解決を通っていない",
        ),
        (
            "effort: launch.effort.as_deref()",
            "effort が spawn と同じ解決を通っていない",
        ),
        (
            "profile.resolve_agent_launch(worker, model, effort)",
            "起動の引数を spawn と同じプロファイルの解決で決めていない",
        ),
        (
            "worker_env_plan(&profile, &mut warnings)",
            "env 計画を spawn と同じ材料（プロファイルの env + worker のアカウント）で組んでいない",
        ),
    ] {
        require(&body, RESUME, pat, why)?;
    }
    forbid(
        &body,
        RESUME,
        "with_resume_flag(",
        "worker の再開の書式を spawn の関数の外で足している（codex の `resume` の位置がずれる）",
    )?;
    Ok(())
}

/// 縛り 3: 会話 ID を `WorkerLaunch` へ渡すのは resume_launch.rs の再開の 1 か所だけで、
/// 書式は build_worker_cmd_in の中で正本から引く
fn 組み立ての縛り(s: &Sources) -> Result<(), String> {
    let build = window(&s.agent, AGENT, "pub fn build_worker_cmd_in(")?;
    require(
        &build,
        AGENT,
        "tako_core::agent_resume::resume_spec(",
        "再開の書式を正本（resume_spec）から引いていない",
    )?;
    require(
        &build,
        AGENT,
        "launch.resume",
        "会話 ID を組み立てへ使っていない",
    )?;
    // 会話 ID を WorkerLaunch へ渡す（= 再開を組む）製品コードは resume_launch.rs の 1 か所だけ。
    // 経路ごとに再開の写しを作ると、片方だけ spawn の引数が落ちる（#2473 の主題）
    let mut resumed = Vec::new();
    for (file, src) in [
        (DISPATCH, &s.dispatch),
        (RESUME, &s.resume),
        (SESSIONS, &s.sessions),
        (AGENT, &s.agent),
    ] {
        for (line, block) in worker_launch_blocks(src) {
            if block.contains("resume: Some(") {
                resumed.push((file, line));
            }
        }
    }
    if let Some((file, line)) = resumed.iter().find(|(f, _)| *f != RESUME) {
        return Err(format!(
            "{file}:{line}: 会話 ID を WorkerLaunch へ渡す場所が resume_launch.rs の外にもある \
             （再開の組み立ての写し。worker の再開は resume_launch::worker_resume_launch を通す。#2473）"
        ));
    }
    if resumed.len() != 1 {
        return Err(format!(
            "{RESUME}:1: 会話 ID を WorkerLaunch へ渡す場所が 1 か所ではない（{resumed:?}。#2473）"
        ));
    }
    Ok(())
}

/// 1 つの縛り。違反なら `file:line: 理由` を返す
type Check = fn(&Sources) -> Result<(), String>;

fn check_all(s: &Sources) -> Vec<String> {
    let checks: [Check; 3] = [復元の縛り, 入口の縛り, 組み立ての縛り];
    checks.iter().filter_map(|c| c(s).err()).collect()
}

#[test]
fn 再起動をまたぐ復元は_worker_を_spawn_と同じ組み立てで組む() {
    if let Err(e) = 復元の縛り(&Sources::current()) {
        panic!("{e}");
    }
}

#[test]
fn 再開の入口は_worker_resume_launch_の_1_実装() {
    if let Err(e) = 入口の縛り(&Sources::current()) {
        panic!("{e}");
    }
}

#[test]
fn spawn_と再開は同じ関数で引数を組む() {
    if let Err(e) = 組み立ての縛り(&Sources::current()) {
        panic!("{e}");
    }
}

/// 現行ソースから #2473 の修正を 1 か所ずつ戻した注入が、名指し（`file:line`）で落ちること
#[test]
fn 修正を戻す注入を名指しで落とす() {
    let base = Sources::current();
    assert!(
        check_all(&base).is_empty(),
        "現行ソースが番犬を通らない: {:?}",
        check_all(&base)
    );
    type Inject = fn(&mut Sources) -> bool;
    fn swap(src: &mut String, from: &str, to: &str) -> bool {
        if !src.contains(from) {
            return false;
        }
        *src = src.replacen(from, to, 1);
        true
    }
    let cases: &[(&str, &str, Inject)] = &[
        (
            "GUI の復元をカタログのメタへ戻す",
            SESSIONS,
            |s| {
                swap(
                    &mut s.sessions,
                    "let worker_command = restore_worker_command(&input, hints);",
                    "let worker_command: Option<String> = None;",
                )
            },
        ),
        ("復元を別の組み立てで組む", SESSIONS, |s| {
            swap(
                &mut s.sessions,
                "let launch = match crate::resume_launch::worker_resume_launch(",
                "let launch = match crate::resume_launch::catalog_launch(",
            )
        }),
        (
            "純関数が組んだコマンドを打たない",
            SESSIONS,
            |s| swap(&mut s.sessions, "input.worker_command", "None::<&str>"),
        ),
        ("GUI が手掛かりを組まない", APP, |s| {
            swap(
                &mut s.app,
                "let hints = tako_control::resume_launch::restore_hints(",
                "let hints = tako_control::resume_launch::ResumeHints::default_for(",
            )
        }),
        (
            "sessions resume / 会話を保った再起動の worker を別の組み立てへ戻す",
            RESUME,
            |s| {
                swap(
                    &mut s.resume,
                    "if let Some(launch) = worker_resume_launch(",
                    "if let Some(launch) = catalog_worker_launch(",
                )
            },
        ),
        ("プロファイルの args を落とす", RESUME, |s| {
            swap(
                    &mut s.resume,
                    "            extra_args: &launch.extra_args,\n            tako_bin: Some(tako_bin),",
                    "            extra_args: &[],\n            tako_bin: Some(tako_bin),",
                )
        }),
        ("Remote Control を落とす", RESUME, |s| {
            swap(
                    &mut s.resume,
                    "            remote_control: remote_control.enabled(),\n            extra_args: &launch.extra_args,\n            tako_bin: Some(tako_bin),",
                    "            remote_control: false,\n            extra_args: &launch.extra_args,\n            tako_bin: Some(tako_bin),",
                )
        }),
        (
            "再開の書式を spawn の関数の外で足す",
            RESUME,
            |s| {
                swap(&mut s.resume, "resume: Some(session_id),", "resume: None,")
                    && swap(
                        &mut s.resume,
                        "        command: cmd,",
                        "        command: with_resume_flag(cmd, session_id),",
                    )
            },
        ),
        ("書式を正本から引かない", AGENT, |s| {
            swap(
                &mut s.agent,
                "tako_core::agent_resume::resume_spec(launch.agent.into())",
                "local_resume_spec(launch.agent)",
            )
        }),
        (
            "再開の組み立ての写しを増やす",
            DISPATCH,
            |s| {
                swap(
                    &mut s.dispatch,
                    "        env: &profile_env,\n        resume: None,\n    });",
                    "        env: &profile_env,\n        resume: Some(session_id),\n    });",
                )
            },
        ),
    ];
    let mut missed = Vec::new();
    for (name, file, inject) in cases {
        let mut s = base.clone();
        assert!(
            inject(&mut s),
            "注入「{name}」の置換元が現行ソースに無い（ソースを直したら注入も作り直す）"
        );
        let errors = check_all(&s);
        if !errors.iter().any(|e| e.starts_with(&format!("{file}:"))) {
            missed.push(format!("{name}: {errors:?}"));
        }
    }
    assert!(missed.is_empty(), "名指しされなかった注入: {missed:#?}");
}
