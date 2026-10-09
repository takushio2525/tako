//! #1967 の番犬: 「セッションを引き継いで再起動」の後に会話が戻らない、を作った配線の構造
//!
//! # なぜ要るか
//!
//! 本番（10/9）で minige の master（35 桁のペイン）が再起動の後に戻らなかった。真因は 3 つで、
//! どれも「関数 1 つの呼び先・ガード 1 つ」で元へ戻る:
//!
//! 1. **終了時の案内から途中までの ID を採った**。claude は終了時に
//!    `Resume this session with: claude --resume <id>` を出し、tako はそれを権威として
//!    ID を差し替える（#1067）。細いペインでは案内が折り返すのに **1 行の中だけ**を読み、
//!    形の検査も「8 文字以上」だけだったので、`…-b897-065b9` で切れた ID で resume を
//!    実行した（zsh の履歴に実行の記録が残っていた = claude は会話が見つからずに終わる）
//! 2. **再開コマンドがカタログのメタだけで組まれていた**。master の会話はカタログに
//!    model / effort が一度も記録されず、system prompt・Remote Control・許可モードは
//!    元から載らない。master を再開すると effort が medium・運用規則が落ちた claude が立った
//! 3. **送った後を誰も見ていなかった**。建て直しは「送った」で終わり、すぐ落ちても
//!    ペインにはシェルだけが残った。persist.log にも再起動の記録が 1 行も無かった
//!
//! 振る舞いは各 crate の単体テストと隔離 GUI の実測（`scripts/test-session-restart-1967.sh`）が
//! 見る。ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 再開コマンドは元の起動の正本を通す（master / solo = `build_master_cmd_with_plan_in`、
//!    worker = `build_worker_cmd_in`）
//! 2. ペインの再起動と `tako sessions resume` が同じ 1 実装（`resume_launch`）を通す
//! 3. 終了時の案内は折り返しに依らず UUID の全長で読み、記録が実在するときだけ差し替える
//! 4. 落ちてもシェルがプロンプトへ戻るまで打たない（`shell_back`）
//! 5. 送った後を見張り、送達は #640 / #1940 の送達フローを通す
//! 6. 失敗も開始も persist.log へ出し、失敗はペインのバナーへ出す
//! 7. 記録の実在・進行中・シェルの待機を判断の材料へ載せる
//! 8. A/B の env（`TAKO_1967_LEGACY`）を文字列で読むのは `session_restart` の 1 か所だけ
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const CORE: &str = "crates/tako-core/src/session_restart.rs";
const RESUME: &str = "crates/tako-control/src/resume_launch.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const LEGACY_ENV: &str = "\"TAKO_1967_LEGACY\"";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// `needle` を含む最初の行から、その行と同じ字下げで閉じる `}` までの窓
/// （1 始まりの行番号つき）
fn window<'a>(src: &'a str, needle: &str) -> Option<Vec<(usize, &'a str)>> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines[start + 1..]
        .iter()
        .position(|l| *l == close)
        .map(|i| start + 1 + i)?;
    Some((start..=end).map(|i| (i + 1, lines[i])).collect::<Vec<_>>())
}

fn first_line(win: &[(usize, &str)], pat: &str) -> Option<usize> {
    win.iter().find(|(_, l)| l.contains(pat)).map(|(n, _)| *n)
}

fn head(win: &[(usize, &str)]) -> usize {
    win.first().map_or(0, |(n, _)| *n)
}

/// 1 つの縛り。違反なら `file:line: 理由` を返す
type Check = fn(&Sources) -> Result<(), String>;

#[derive(Clone)]
struct Sources {
    core: String,
    resume: String,
    dispatch: String,
    app: String,
}

impl Sources {
    fn current() -> Self {
        Self {
            core: read(CORE),
            resume: read(RESUME),
            dispatch: read(DISPATCH),
            app: read(APP),
        }
    }
}

fn need<'a>(src: &'a str, file: &str, needle: &str) -> Result<Vec<(usize, &'a str)>, String> {
    window(src, needle).ok_or_else(|| format!("{file}:1: `{needle}` の窓が見つからない"))
}

/// 窓の中に `pat` があること（無ければ窓の先頭行で名指す）
fn require(win: &[(usize, &str)], file: &str, pat: &str, why: &str) -> Result<usize, String> {
    first_line(win, pat).ok_or_else(|| format!("{file}:{}: {why}（`{pat}` が無い）", head(win)))
}

/// 窓の中に `pat` が無いこと
fn forbid(win: &[(usize, &str)], file: &str, pat: &str, why: &str) -> Result<(), String> {
    match first_line(win, pat) {
        Some(n) => Err(format!("{file}:{n}: {why}（`{pat}`）")),
        None => Ok(()),
    }
}

/// 1. 再開コマンドは元の起動の正本を通し、末尾に `--resume` を足すだけ
fn 再開コマンドは元の起動の正本を通す(s: &Sources) -> Result<(), String> {
    let master = need(&s.resume, RESUME, "fn resume_profile_launch(")?;
    require(
        &master,
        RESUME,
        "orchestrator::build_master_cmd_with_plan_in(",
        "master / solo の再開が `tako master` と同じ組み立てを通っていない \
         = model / effort / system prompt / Remote Control が落ちる（#1967 の主題）",
    )?;
    require(
        &master,
        RESUME,
        "pin_config_dir(",
        "設定 dir を会話の記録の所在へ寄せていない（記録の無い dir で resume すると落ちる）",
    )?;
    let worker = need(&s.resume, RESUME, "fn resume_worker_launch(")?;
    require(
        &worker,
        RESUME,
        "orchestrator::agent::build_worker_cmd_in(",
        "worker の再開が spawn と同じ組み立てを通っていない = 許可モード・追加引数が落ちる",
    )?;
    for win in [&master, &worker] {
        require(
            win,
            RESUME,
            "with_resume_flag(",
            "`--resume <id>` を書式の正本から足していない",
        )?;
    }
    Ok(())
}

/// 2. ペインの再起動と `tako sessions resume` が同じ 1 実装を通す
fn 再起動と_sessions_resume_は同じ組み立て(s: &Sources) -> Result<(), String> {
    for (needle, call) in [
        (
            "fn build_session_restart_plan(",
            "crate::resume_launch::resume_launch_in(",
        ),
        (
            "fn dispatch_sessions_resume(",
            "crate::resume_launch::resume_launch(",
        ),
    ] {
        let win = need(&s.dispatch, DISPATCH, needle)?;
        require(
            &win,
            DISPATCH,
            call,
            "再開コマンドを resume_launch の 1 実装で組んでいない（2 系統に分かれると片方だけ引数が落ちる）",
        )?;
        forbid(
            &win,
            DISPATCH,
            "sessions::resume_command(",
            "カタログのメタだけの組み立てを直に呼んでいる（#1967 の主題）",
        )?;
    }
    Ok(())
}

/// 3. 終了時の案内は折り返しに依らず UUID の全長で読み、記録が実在するときだけ差し替える
fn 案内は折り返しに依らず記録で確かめる(s: &Sources) -> Result<(), String> {
    let parse = need(&s.core, CORE, "pub fn parse_resume_hint(screen")?;
    require(
        &parse,
        CORE,
        "squash_lines(",
        "案内を 1 行の中だけで読んでいる = 細いペインで折り返した途中までの ID を採る（#1967 の ②）",
    )?;
    require(
        &parse,
        CORE,
        "leading_uuid(",
        "ID を UUID の全長で読んでいない（途中で切れた ID が形の検査を通る）",
    )?;
    let apply = need(&s.app, APP, "fn apply_resume_hint_to(")?;
    let checked = require(
        &apply,
        APP,
        "tako_control::transcript::find_transcript(&hint)",
        "画面の案内の ID を、記録の実在を確かめずに採っている",
    )?;
    let applied = require(
        &apply,
        APP,
        "sr::apply_resume_hint(",
        "画面の案内で差し替える段が無い",
    )?;
    if checked > applied {
        return Err(format!(
            "{APP}:{applied}: 記録の実在を確かめる前に差し替えている"
        ));
    }
    let parse_call = require(
        &apply,
        APP,
        "sr::parse_resume_hint(&screen)",
        "新しい読み方を通していない",
    )?;
    if !apply
        .iter()
        .any(|(n, l)| *n < parse_call && l.contains("if entry.legacy"))
    {
        return Err(format!(
            "{APP}:{parse_call}: 旧い読み方（1 行だけ）を旧挙動の腕の外で使っている"
        ));
    }
    Ok(())
}

/// 4. 落ちてもシェルがプロンプトへ戻るまで打たない
fn 落ちてもシェルが戻るまで打たない(s: &Sources) -> Result<(), String> {
    let step = need(&s.core, CORE, "pub fn relaunch_step(")?;
    require(
        &step,
        CORE,
        "if shell_back || elapsed_secs >= RELAUNCH_TIMEOUT_SECS {",
        "落ちたら即打つ = シェルの起動フックの最中の先行入力になる（#1940 と同じ型）",
    )?;
    let drive = need(&s.app, APP, "fn drive_agent_relaunches(&mut self)")?;
    require(
        &drive,
        APP,
        "sr::shell_back(",
        "シェルの戻り（プロンプトの印）を見ていない",
    )?;
    require(
        &drive,
        APP,
        "sr::relaunch_step(alive, shell_back,",
        "relaunch_step へシェルの戻りを渡していない",
    )?;
    let dispatch = need(&s.dispatch, DISPATCH, "fn dispatch_session_restart(")?;
    let before = require(
        &dispatch,
        DISPATCH,
        "let prompts_before = plan",
        "プロンプトの印の基準を控えていない",
    )?;
    let terminate = require(
        &dispatch,
        DISPATCH,
        "crate::platform::process::terminate(pid, false)",
        "終了要求が見当たらない",
    )?;
    if before > terminate {
        return Err(format!(
            "{DISPATCH}:{before}: プロンプトの印の基準を終了要求の**後**に控えている（戻った後の値を基準にしうる）"
        ));
    }
    Ok(())
}

/// 5. 送った後を見張り、送達は #640 / #1940 の送達フローを通す
fn 送った後を見張る(s: &Sources) -> Result<(), String> {
    let drive = need(&s.app, APP, "fn drive_agent_relaunches(&mut self)")?;
    require(
        &drive,
        APP,
        "sr::launch_verdict(",
        "送った後にすぐ落ちたかを見張っていない = 失敗してもシェルだけが残って誰も気づかない",
    )?;
    require(
        &drive,
        APP,
        "sr::MAX_RELAUNCH_ATTEMPTS",
        "すぐ落ちたときの打ち直しに上限が無い / 打ち直さない",
    )?;
    let launch = need(&s.app, APP, "fn launch_relaunch(&mut self")?;
    require(
        &launch,
        APP,
        "tako_core::shell_send::ShellSendFlow::new(",
        "建て直しの起動コマンドが #640 / #1940 の送達フローを通っていない",
    )?;
    require(
        &launch,
        APP,
        "relaunch: true",
        "送達フローの顛末が見張りへ渡らない",
    )?;
    forbid(
        &launch,
        APP,
        "session.write(",
        "起動コマンドを送達フローを介さずに直書きしている",
    )?;
    Ok(())
}

/// 6. 失敗も開始も persist.log へ出し、失敗はペインのバナーへ出す（本文は出さない）
fn 失敗を黙らない(s: &Sources) -> Result<(), String> {
    let finish = need(&s.app, APP, "fn finish_relaunch(")?;
    require(
        &finish,
        APP,
        "結果=起動",
        "立ち上がったことを persist.log へ残していない",
    )?;
    require(
        &finish,
        APP,
        "結果=失敗",
        "失敗を persist.log へ残していない",
    )?;
    require(
        &finish,
        APP,
        "self.set_agent_restart_notice(pane, Some(msg))",
        "失敗をペインのバナーへ出していない",
    )?;
    let dispatch = need(&s.dispatch, DISPATCH, "fn dispatch_session_restart(")?;
    for (pat, why) in [
        (
            "セッション再起動: 開始 pane={pane_raw} mode=harness",
            "harness の開始を persist.log へ残していない",
        ),
        (
            "セッション再起動: 開始 pane={pane_raw} mode=handoff",
            "handoff の開始を persist.log へ残していない",
        ),
        (
            "セッション再起動: 断った pane={pane_raw}",
            "断ったことを persist.log へ残していない（押したのに何も起きない、を追えない）",
        ),
    ] {
        require(&dispatch, DISPATCH, pat, why)?;
    }
    // 本文（env の並び・パス）を診断ログへ出さない（AGENTS.md の絶対ルール）
    for (n, l) in &dispatch {
        if l.contains("persist_log") && (l.contains("command") || l.contains("{command}")) {
            return Err(format!(
                "{DISPATCH}:{n}: 起動コマンドの本文を persist.log へ出している"
            ));
        }
    }
    Ok(())
}

/// 7. 記録の実在・進行中・シェルの待機を判断の材料へ載せる
fn 判断の材料が揃っている(s: &Sources) -> Result<(), String> {
    let plan = need(&s.dispatch, DISPATCH, "fn build_session_restart_plan(")?;
    for (pat, why) in [
        (
            "conversation_found: location.is_some(),",
            "会話の記録の実在を確かめずに終了させる（resume が落ちて会話も画面も失う）",
        ),
        (
            "restart_in_progress,",
            "建て直しの最中に重ねて押せる（立ち上がったばかりのエージェントをまた終了させる）",
        ),
        (
            "shell_idle,",
            "エージェントが終わったペイン（シェルだけ）を同じ操作で戻せない",
        ),
    ] {
        require(&plan, DISPATCH, pat, why)?;
    }
    let can = need(&s.core, CORE, "pub fn can_restart(")?;
    let in_progress = require(
        &can,
        CORE,
        "return Err(RestartBlock::RestartInProgress);",
        "進行中の再起動を断っていない",
    )?;
    let busy = require(
        &can,
        CORE,
        "return Err(RestartBlock::Busy);",
        "生成中の判定が無い",
    )?;
    if in_progress > busy {
        return Err(format!(
            "{CORE}:{in_progress}: 進行中の判定が生成中の判定より後（建て直し中の画面を生成中と読み違える）"
        ));
    }
    require(
        &can,
        CORE,
        "return Err(RestartBlock::ConversationMissing);",
        "記録の無い会話を断っていない",
    )?;
    Ok(())
}

const CHECKS: &[(&str, Check)] = &[
    (
        "再開コマンドは元の起動の正本を通す",
        再開コマンドは元の起動の正本を通す,
    ),
    (
        "再起動と sessions resume は同じ組み立て",
        再起動と_sessions_resume_は同じ組み立て,
    ),
    (
        "案内は折り返しに依らず記録で確かめる",
        案内は折り返しに依らず記録で確かめる,
    ),
    (
        "落ちてもシェルが戻るまで打たない",
        落ちてもシェルが戻るまで打たない,
    ),
    ("送った後を見張る", 送った後を見張る),
    ("失敗を黙らない", 失敗を黙らない),
    ("判断の材料が揃っている", 判断の材料が揃っている),
];

#[test]
fn 再起動の配線が保たれている() {
    let s = Sources::current();
    let failures: Vec<String> = CHECKS
        .iter()
        .filter_map(|(name, check)| check(&s).err().map(|e| format!("[{name}] {e}")))
        .collect();
    assert!(
        failures.is_empty(),
        "#1967 の配線が崩れた:\n{}",
        failures.join("\n")
    );
}

#[test]
fn ab_のenvを読むのは1か所だけ() {
    let root = workspace_root();
    let mut readers = Vec::new();
    for dir in SOURCE_DIRS {
        for file in walk(&root.join(dir)) {
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (i, line) in src.lines().enumerate() {
                if line.contains(LEGACY_ENV) {
                    let rel = file
                        .strip_prefix(&root)
                        .unwrap_or(&file)
                        .display()
                        .to_string();
                    readers.push(format!("{rel}:{}", i + 1));
                }
            }
        }
    }
    assert_eq!(
        readers.len(),
        1,
        "{LEGACY_ENV} を読むのは {CORE} の legacy_1967 だけ: {readers:?}"
    );
    assert!(readers[0].starts_with(CORE), "{readers:?}");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::current();
    for (file, src, needle) in [
        (CORE, &s.core, "pub fn parse_resume_hint(screen"),
        (CORE, &s.core, "pub fn relaunch_step("),
        (CORE, &s.core, "pub fn can_restart("),
        (RESUME, &s.resume, "fn resume_profile_launch("),
        (RESUME, &s.resume, "fn resume_worker_launch("),
        (DISPATCH, &s.dispatch, "fn build_session_restart_plan("),
        (DISPATCH, &s.dispatch, "fn dispatch_session_restart("),
        (DISPATCH, &s.dispatch, "fn dispatch_sessions_resume("),
        (APP, &s.app, "fn drive_agent_relaunches(&mut self)"),
        (APP, &s.app, "fn apply_resume_hint_to("),
        (APP, &s.app, "fn launch_relaunch(&mut self"),
        (APP, &s.app, "fn finish_relaunch("),
    ] {
        let win = window(src, needle).unwrap_or_else(|| panic!("{file}: `{needle}` の窓が採れる"));
        assert!(
            win.len() > 5,
            "{file}: `{needle}` の窓が短すぎる（{} 行）",
            win.len()
        );
    }
}

/// 現行ソースから作り直した注入が、file:line で名指しされること
#[test]
fn 逆戻りを名指しできる() {
    let base = Sources::current();
    type Inject = fn(&mut Sources) -> bool;
    let cases: &[(&str, &str, Inject)] = &[
        (
            "master の再開をカタログのメタへ戻す",
            RESUME,
            |s| {
                replace_once(
                    &mut s.resume,
                    "let launch = orchestrator::build_master_cmd_with_plan_in(",
                    "let launch = orchestrator::build_master_cmd_legacy(",
                )
            },
        ),
        (
            "worker の再開を spawn の組み立てから外す",
            RESUME,
            |s| {
                replace_once(
                    &mut s.resume,
                    "let cmd = orchestrator::agent::build_worker_cmd_in(",
                    "let cmd = crate::sessions::catalog_cmd(",
                )
            },
        ),
        (
            "ペインの再起動をカタログの組み立てへ戻す",
            DISPATCH,
            |s| {
                replace_once(
                    &mut s.dispatch,
                    "Some(id) => match crate::resume_launch::resume_launch_in(",
                    "Some(id) => match crate::sessions::resume_command(",
                )
            },
        ),
        (
            "sessions resume をカタログの組み立てへ戻す",
            DISPATCH,
            |s| {
                replace_once(
                    &mut s.dispatch,
                    "let launch = crate::resume_launch::resume_launch(&session_id, &entry, &hints)",
                    "let launch = crate::sessions::resume_command(&session_id, &entry)",
                )
            },
        ),
        ("案内を 1 行だけで読む", CORE, |s| {
            replace_once(
                &mut s.core,
                "    let joined = squash_lines(screen);",
                "    let joined = screen.last().cloned().unwrap_or_default();",
            )
        }),
        ("案内の ID の記録を確かめない", APP, |s| {
            replace_once(
                &mut s.app,
                "if !entry.legacy && tako_control::transcript::find_transcript(&hint).is_none() {",
                "if false {",
            )
        }),
        ("落ちたら即打つ（コア）", CORE, |s| {
            replace_once(
                &mut s.core,
                "        if shell_back || elapsed_secs >= RELAUNCH_TIMEOUT_SECS {",
                "        if true {",
            )
        }),
        ("落ちたら即打つ（アプリ）", APP, |s| {
            replace_once(
                &mut s.app,
                "                        || sr::shell_back(",
                "                        || always_back(",
            )
        }),
        ("基準を終了要求の後に控える", DISPATCH, |s| {
            let from = "            let prompts_before = plan";
            let Some(start) = s.dispatch.find(from) else {
                return false;
            };
            // `let prompts_before = …;` の文を終了要求の後ろへ移す
            let end = s.dispatch[start..]
                .find(".filter(|n| *n > 0);\n")
                .map(|i| start + i + ".filter(|n| *n > 0);\n".len());
            let Some(end) = end else { return false };
            let stmt = s.dispatch[start..end].to_string();
            s.dispatch.replace_range(start..end, "");
            let anchor = "            let command = launch.command.clone();\n";
            if s.dispatch.matches(anchor).count() != 1 {
                return false;
            }
            s.dispatch = s.dispatch.replacen(anchor, &format!("{stmt}{anchor}"), 1);
            true
        }),
        ("送った後を見張らない", APP, |s| {
            replace_once(
                &mut s.app,
                "                    match sr::launch_verdict(",
                "                    match never_watch(",
            )
        }),
        ("建て直しを送達フローの外で書く", APP, |s| {
            replace_once(
                &mut s.app,
                "            flow: tako_core::shell_send::ShellSendFlow::new(entry.command.clone()),",
                "            flow: raw_write(entry.command.clone()),",
            )
        }),
        ("失敗を persist.log へ残さない", APP, |s| {
            replace_once(
                &mut s.app,
                "\"セッション再起動: 結果=失敗 pane={} 理由={} 終了コード={} 試行={}\",",
                "\"\",",
            )
        }),
        ("開始を persist.log へ残さない", DISPATCH, |s| {
            replace_once(
                &mut s.dispatch,
                "\"セッション再起動: 開始 pane={pane_raw} mode=harness",
                "\"",
            )
        }),
        ("記録の実在を見ない", DISPATCH, |s| {
            replace_once(
                &mut s.dispatch,
                "            conversation_found: location.is_some(),",
                "            conversation_found: true,",
            )
        }),
        ("重ねた再起動を断らない", CORE, |s| {
            replace_once(
                &mut s.core,
                "        return Err(RestartBlock::RestartInProgress);",
                "        // 断らない",
            )
        }),
    ];
    for (name, file, inject) in cases {
        let mut s = base.clone();
        assert!(
            inject(&mut s),
            "注入「{name}」の元の字面が現行ソースに無い（番犬と実装がずれた）"
        );
        let failures: Vec<String> = CHECKS
            .iter()
            .filter_map(|(_, check)| check(&s).err())
            .collect();
        assert!(!failures.is_empty(), "注入「{name}」を番犬が見逃した");
        let named = failures.iter().any(|f| {
            f.starts_with(&format!("{file}:"))
                && f[file.len() + 1..]
                    .split(':')
                    .next()
                    .is_some_and(|n| n.parse::<usize>().is_ok_and(|n| n > 1))
        });
        assert!(
            named,
            "注入「{name}」は {file}:<行> で名指しされる: {failures:?}"
        );
    }
}

fn replace_once(src: &mut String, from: &str, to: &str) -> bool {
    if src.matches(from).count() != 1 {
        return false;
    }
    *src = src.replacen(from, to, 1);
    true
}
