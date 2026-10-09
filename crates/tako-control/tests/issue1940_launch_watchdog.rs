//! #1940 の番犬: spawn の起動コマンドが化けない・依頼文が届く前に delivered と言わない・
//! 起動の失敗を黙らない、の配線の構造
//!
//! # なぜ要るか
//!
//! 本番（10/9）で worker が立たなかった 4 件は、どれも単体テストでは見えない配線の
//! 抜けだった。起動コマンドの送達（`tako_core::shell_send`）は画面の全文一致でしか
//! 確かめておらず、2〜3 行のペインでは全文が画面に出ないので書き直しを使い切り、
//! 最後に**消していない行へ**本文 + Enter を書き足して `autoexport` に化けた。
//! 会話の検出（起動直後）で `prompt_delivery=delivered` を返し、送達フローが諦めた後に
//! `undelivered` へ反転した。入力欄を描けない短いペインでは peer 送達を試さないまま
//! 120 秒待って諦め、起動したエージェントがすぐ終わっても spawn は成功のままだった。
//!
//! どれも「関数 1 つの順序・ガード 1 つ」で元へ戻るので、**構造**を縛って file:line で
//! 名指す。振る舞いは各 crate の単体テストと実測（`tests/issue1940_launch_e2e.rs`）が見る。
//!
//! # 何を縛るか
//!
//! 1. 書き直しを使い切った後の書き切りは行を捨ててから（`recover` が `FinalClear` を経る）
//! 2. 全文を画面に出せない寸法では書き直さない（`tick_echo` が `echo_unviewable` を先に見る）
//! 3. 実行の確認はシェル統合の印を見る（`tick_submitted` が `marks.executed` を見る）
//! 4. 会話の検出は到達の証拠にしない（`prompt_delivered_at` を書くのも、session_id を
//!    delivered にするのも `legacy_1940` の腕だけ）
//! 5. tako-app の起動コマンド送達は観測材料（寸法・印）を渡し、打ち切りを起動失敗として残す
//! 6. tako-app の依頼文の送達は、起動したエージェントの終了と短いペインを見る
//! 7. 起動に失敗した worker へは再送の引き金（`prompt_undelivered`）を出さない
//! 8. A/B の env（`TAKO_1940_LEGACY`）を文字列で読むのは `shell_send` の 1 か所だけ
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

const SHELL_SEND: &str = "crates/tako-core/src/shell_send.rs";
const REGISTRY: &str = "crates/tako-control/src/orchestrator/registry.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const LEGACY_ENV: &str = "\"TAKO_1940_LEGACY\"";

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

struct Sources {
    shell_send: String,
    registry: String,
    app: String,
    dispatch: String,
}

impl Sources {
    fn current() -> Self {
        Self {
            shell_send: read(SHELL_SEND),
            registry: read(REGISTRY),
            app: read(APP),
            dispatch: read(DISPATCH),
        }
    }
}

fn need<'a>(src: &'a str, file: &str, needle: &str) -> Result<Vec<(usize, &'a str)>, String> {
    window(src, needle).ok_or_else(|| format!("{file}:1: `{needle}` の窓が見つからない"))
}

/// 1. 書き直しを使い切った後は `FinalClear`（Ctrl+C）を経てから書き切る
fn 書き切りの前に行を捨てる(s: &Sources) -> Result<(), String> {
    let win = need(&s.shell_send, SHELL_SEND, "fn recover(&mut self)")?;
    let guard = first_line(&win, "if self.legacy").ok_or_else(|| {
        format!(
            "{SHELL_SEND}:{}: recover が旧挙動の腕を `self.legacy` で分けていない",
            head(&win)
        )
    })?;
    let clear = first_line(&win, "Stage::FinalClear").ok_or_else(|| {
        format!(
            "{SHELL_SEND}:{}: 書き直しを使い切った後に行を捨てる段（FinalClear）を経ていない \
             = 消していない行へ本文 + Enter を書き足す（#1940 の `autoexport`）",
            head(&win)
        )
    })?;
    // 旧挙動の腕の外で `write_through` を直に呼んでいないこと
    for (n, l) in &win {
        if l.contains("self.write_through()") && *n > guard + 3 {
            return Err(format!(
                "{SHELL_SEND}:{n}: 行を捨てずに書き切っている（旧挙動の腕の外）"
            ));
        }
    }
    let fin = need(&s.shell_send, SHELL_SEND, "fn tick_final_clear(")?;
    let cleared = first_line(&fin, "line_cleared(");
    let wrote = first_line(&fin, "write_through(");
    match (cleared, wrote) {
        (Some(c), Some(w)) if c < w => {}
        _ => {
            return Err(format!(
                "{SHELL_SEND}:{}: FinalClear が行の消えきりを待たずに書き切る",
                head(&fin)
            ))
        }
    }
    let _ = clear;
    Ok(())
}

/// 2. 全文を画面に出せない寸法では、書き直す前に `echo_unviewable` で判断する
fn 畳まれた寸法では書き直さない(s: &Sources) -> Result<(), String> {
    let win = need(&s.shell_send, SHELL_SEND, "fn tick_echo(")?;
    let judged = first_line(&win, "echo_unviewable(");
    let recover = first_line(&win, "self.recover()");
    match (judged, recover) {
        (Some(j), Some(r)) if j < r => Ok(()),
        _ => Err(format!(
            "{SHELL_SEND}:{}: tick_echo が寸法（echo_unviewable）を見ずに書き直しへ進む \
             = 2〜3 行のペインで書き直しを使い切る",
            head(&win)
        )),
    }
}

/// 3. 実行の確認はシェル統合の印（preexec の回数）を見る
fn 実行は印で確かめる(s: &Sources) -> Result<(), String> {
    let win = need(&s.shell_send, SHELL_SEND, "fn tick_submitted(")?;
    if first_line(&win, "marks.executed").is_none() {
        return Err(format!(
            "{SHELL_SEND}:{}: tick_submitted が実行の印（marks.executed）を見ていない",
            head(&win)
        ));
    }
    if first_line(&win, "self.typeahead").is_none() {
        return Err(format!(
            "{SHELL_SEND}:{}: 起動フック中の先行入力を区別していない（画面の変化を実行と取り違える）",
            head(&win)
        ));
    }
    Ok(())
}

/// 4. 会話の検出は到達の証拠にしない（旧挙動の腕だけが到達とみなす）
fn 会話の検出を到達とみなさない(s: &Sources) -> Result<(), String> {
    let win = need(
        &s.registry,
        REGISTRY,
        "pub fn prompt_delivery_assessment_with(",
    )?;
    for (i, (n, l)) in win.iter().enumerate() {
        if !l.contains("session_id.is_some()") {
            continue;
        }
        let returns_delivered = win[i..(i + 3).min(win.len())]
            .iter()
            .any(|(_, l)| l.contains("PromptDelivery::Delivered"));
        if returns_delivered && !l.contains("legacy_1940") {
            return Err(format!(
                "{REGISTRY}:{n}: 会話の検出（session_id）だけで delivered を返している \
                 = 届く前に delivered → 諦めた後に undelivered と反転する（#1940）"
            ));
        }
    }
    for needle in [
        "fn record_session_detected_at(",
        "pub fn record_session_detected_by_pane(",
    ] {
        let win = need(&s.registry, REGISTRY, needle)?;
        for (i, (n, l)) in win.iter().enumerate() {
            if !l.contains("entry.prompt_delivered_at = Some(") {
                continue;
            }
            let guarded = i > 0 && win[i - 1].1.contains("if legacy &&");
            if !guarded {
                return Err(format!(
                    "{REGISTRY}:{n}: 会話の検出で到達の時刻を書いている（旧挙動の腕の外）"
                ));
            }
        }
    }
    Ok(())
}

/// 5. 起動コマンドの送達は寸法と印を渡し、打ち切りを起動失敗として残す
fn 起動コマンドの送達は観測材料を渡し失敗を残す(
    s: &Sources,
) -> Result<(), String> {
    let win = need(&s.app, APP, "fn drive_command_flows(&mut self)")?;
    let at = head(&win);
    for (pat, why) in [
        (".observe(&", "寸法と印を渡す口（observe）を通っていない"),
        ("shell_marks()", "シェル統合の印を渡していない"),
        ("session.size()", "ペインの寸法を渡していない"),
        (
            "LaunchFailure::CommandFlowTimeout",
            "打ち切りを起動失敗として残していない（spawn が成功のまま黙る）",
        ),
        (
            "launch_baselines.insert(",
            "起動した時点の印を控えていない（エージェントの終了を見られない）",
        ),
    ] {
        if first_line(&win, pat).is_none() {
            return Err(format!("{APP}:{at}: drive_command_flows が{why}"));
        }
    }
    if let Some(n) = first_line(&win, ".flow.tick(") {
        return Err(format!(
            "{APP}:{n}: 画面だけの口（tick）で起動コマンドを送っている"
        ));
    }
    Ok(())
}

/// 6. 依頼文の送達は、起動したエージェントの終了と短いペインを見る
fn 依頼文の送達は終了と短いペインを見る(s: &Sources) -> Result<(), String> {
    let win = need(&s.app, APP, "fn drive_prompt_flows(&mut self)")?;
    let at = head(&win);
    for (pat, why) in [
        (
            "launched_agent_exit(",
            "起動したエージェントの終了を見ていない（120 秒 no_input_box で待つ）",
        ),
        (
            "LaunchFailure::AgentExited",
            "エージェントの終了を起動失敗として残していない",
        ),
        (
            "SHORT_PANE_ROWS",
            "短いペインを見ていない（入力欄を描けないペインで peer を試さない）",
        ),
        ("Stall::PaneTooShort", "短いペインの理由を言っていない"),
    ] {
        if first_line(&win, pat).is_none() {
            return Err(format!("{APP}:{at}: drive_prompt_flows が{why}"));
        }
    }
    // 短いペインの枝は peer 送達を試す（キー経路へは落とせない）
    let short = first_line(&win, "SHORT_PANE_ROWS").unwrap();
    let tail: Vec<_> = win.iter().filter(|(n, _)| *n > short).take(25).collect();
    if !tail.iter().any(|(_, l)| l.contains("drive_peer_attempt(")) {
        return Err(format!(
            "{APP}:{short}: 短いペインの枝が peer 送達を試していない"
        ));
    }
    if tail.iter().any(|(_, l)| l.contains("paste_prompt(")) {
        return Err(format!(
            "{APP}:{short}: 短いペインの枝が貼り付けへ落ちている（入力欄が無い）"
        ));
    }
    Ok(())
}

/// 7. 起動に失敗した worker へは再送の引き金（`prompt_undelivered`）を出さない
fn 起動失敗へ再送の引き金を出さない(s: &Sources) -> Result<(), String> {
    let win = need(&s.dispatch, DISPATCH, "fn apply_worker_status_corrections(")?;
    let at = first_line(&win, "WorkerEventKind::PromptUndelivered").ok_or_else(|| {
        format!(
            "{DISPATCH}:{}: prompt_undelivered の発火元が見つからない",
            head(&win)
        )
    })?;
    // 発火の if（`PromptUndelivered` の数行上）が起動失敗で絞られていること
    let guarded = win
        .iter()
        .filter(|(n, _)| *n < at && *n + 8 >= at)
        .any(|(_, l)| l.contains("!launch_failed"));
    if !guarded {
        return Err(format!(
            "{DISPATCH}:{at}: 起動に失敗した worker へも prompt_undelivered を出す \
             = 自動再送が依頼文をシェルへ打ち込む"
        ));
    }
    Ok(())
}

const CHECKS: &[(&str, Check)] = &[
    (
        "起動失敗へ再送の引き金を出さない",
        起動失敗へ再送の引き金を出さない,
    ),
    ("書き切りの前に行を捨てる", 書き切りの前に行を捨てる),
    ("畳まれた寸法では書き直さない", 畳まれた寸法では書き直さない),
    ("実行は印で確かめる", 実行は印で確かめる),
    ("会話の検出を到達とみなさない", 会話の検出を到達とみなさない),
    (
        "起動コマンドの送達は観測材料を渡し失敗を残す",
        起動コマンドの送達は観測材料を渡し失敗を残す,
    ),
    (
        "依頼文の送達は終了と短いペインを見る",
        依頼文の送達は終了と短いペインを見る,
    ),
];

#[test]
fn 起動と送達の配線が保たれている() {
    let s = Sources::current();
    let failures: Vec<String> = CHECKS
        .iter()
        .filter_map(|(_, check)| check(&s).err())
        .collect();
    assert!(
        failures.is_empty(),
        "#1940 の配線が崩れている:\n{}",
        failures.join("\n")
    );
}

#[test]
fn ab_のenvを読むのは1か所だけ() {
    let root = workspace_root();
    let mut hits = Vec::new();
    for dir in SOURCE_DIRS {
        for entry in walk(&root.join(dir)) {
            let rel = entry
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&entry).unwrap_or_default();
            for (i, l) in src.lines().enumerate() {
                if l.contains(LEGACY_ENV) {
                    hits.push(format!("{rel}:{}", i + 1));
                }
            }
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "{LEGACY_ENV} を読むのは {SHELL_SEND} の legacy_1940 だけ: {hits:?}"
    );
    assert!(hits[0].starts_with(SHELL_SEND), "{hits:?}");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
    out
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::current();
    for (file, src, needle) in [
        (SHELL_SEND, &s.shell_send, "fn recover(&mut self)"),
        (SHELL_SEND, &s.shell_send, "fn tick_final_clear("),
        (SHELL_SEND, &s.shell_send, "fn tick_echo("),
        (SHELL_SEND, &s.shell_send, "fn tick_submitted("),
        (
            REGISTRY,
            &s.registry,
            "pub fn prompt_delivery_assessment_with(",
        ),
        (REGISTRY, &s.registry, "fn record_session_detected_at("),
        (
            REGISTRY,
            &s.registry,
            "pub fn record_session_detected_by_pane(",
        ),
        (APP, &s.app, "fn drive_command_flows(&mut self)"),
        (APP, &s.app, "fn drive_prompt_flows(&mut self)"),
        (DISPATCH, &s.dispatch, "fn apply_worker_status_corrections("),
    ] {
        let win = window(src, needle).unwrap_or_else(|| panic!("{file}: `{needle}` の窓が採れる"));
        assert!(
            win.len() > 5,
            "{file}: `{needle}` の窓が短すぎる（{} 行）",
            win.len()
        );
    }
}

/// 現行ソースから作り直した注入（修正を外した形）が file:line で名指しされる
#[test]
fn 逆戻りを名指しできる() {
    let base = Sources::current();
    type Inject = fn(&mut Sources) -> bool;
    let cases: &[(&str, &str, Inject)] = &[
        (
            "書き切りの前の行クリアを外す",
            SHELL_SEND,
            |s| {
                let from = "            self.enter(Stage::FinalClear);\n            return ShellSendAction::Write(vec![CTRL_C]);";
                replace_once(
                    &mut s.shell_send,
                    from,
                    "            return self.write_through();",
                )
            },
        ),
        ("寸法の判断を外す", SHELL_SEND, |s| {
            replace_once(
                &mut s.shell_send,
                "&& echo_unviewable(obs.cols, obs.screen.len(), &self.command)",
                "&& false",
            )
        }),
        ("実行の印を見ない", SHELL_SEND, |s| {
            replace_once(
                &mut s.shell_send,
                ".is_some_and(|base| obs.marks.executed > base.executed);",
                ".is_some_and(|_| false);",
            )
        }),
        ("会話の検出を delivered に戻す", REGISTRY, |s| {
            replace_once(
                &mut s.registry,
                "(legacy_1940 && entry.session_id.is_some())",
                "entry.session_id.is_some()",
            )
        }),
        (
            "会話の検出で到達の時刻を書く",
            REGISTRY,
            |s| {
                replace_once(
                &mut s.registry,
                "確認（`record_prompt_delivery`）だけが書く\n                if legacy && entry.prompt_delivered_at.is_none() {",
                "確認（`record_prompt_delivery`）だけが書く\n                if entry.prompt_delivered_at.is_none() {",
            )
            },
        ),
        ("起動コマンドを画面だけで送る", APP, |s| {
            replace_once(
                &mut s.app,
                "match entry.flow.observe(&observed) {",
                "match entry.flow.tick(&screen) {",
            )
        }),
        (
            "打ち切りを起動失敗として残さない",
            APP,
            |s| {
                replace_once(
                &mut s.app,
                "                    tako_control::orchestrator::registry::LaunchFailure::CommandFlowTimeout,",
                "                    unreachable!(),",
            )
            },
        ),
        ("エージェントの終了を見ない", APP, |s| {
            replace_once(
                &mut s.app,
                "launched_agent_exit(self.launch_baselines.get(&flow.pane), session)",
                "None::<Option<i32>>",
            )
        }),
        (
            "起動失敗へも再送の引き金を出す",
            DISPATCH,
            |s| {
                replace_once(
                    &mut s.dispatch,
                    "prompt_delivery_final.filter(|_| !launch_failed)",
                    "prompt_delivery_final",
                )
            },
        ),
        ("短いペインで貼り付けへ落ちる", APP, |s| {
            replace_once(
                &mut s.app,
                "                            let step = Self::drive_peer_attempt(&mut flow, peer_ctx.as_ref());\n                            if Self::settle_peer_step(&mut flow, step, elapsed, &mut states)\n                                .is_some()",
                "                            Self::paste_prompt(&flow, session, &backend_for_flow);\n                            if false",
            )
        }),
    ];
    for (name, file, inject) in cases {
        let mut s = Sources {
            shell_send: base.shell_send.clone(),
            registry: base.registry.clone(),
            app: base.app.clone(),
            dispatch: base.dispatch.clone(),
        };
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
