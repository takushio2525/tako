//! 監視の走査（`claude agents --json`）が mod の報告の後ろに居るかの番犬（#2017）
//!
//! # なぜ要るか
//!
//! 本番の tako-app の回収済みの子プロセスの CPU は 1 コアの 24.4%（本体 18.7% より大きい）で、
//! 中身は master の監視（`worker_status` / `orchestrator self`）が起こす
//! `zsh -l -c '…; claude agents --json'` だけだった（登録アカウント 4 本 × 5 秒ごと）。
//! #1880 で状態の一次ソースは tako mod の報告になっていたのに、会話 ID が取れると
//! **mod の報告の有無にかかわらず**走査し、その状態を後段で mod の値に上書きして捨てていた。
//!
//! # 何を縛るか
//!
//! 1. `finish_worker_status` は mod の判断（`monitor_plan`）を会話 ID の解決と状態の照会より**先に**
//!    行い、状態の照会（`query_agent_status`）は mod が答えなかった腕だけで呼ぶ
//! 2. `orchestrator self` / `report` も同じ判断を通す（report は UI スレッドで報告の会話 ID を写すだけ）
//! 3. GUI のセッションカタログの走査は mod が答えたペインを除いた範囲（`list_agents_for_scan_scoped`）
//! 4. 走査を起こすときは範囲で絞る（会話 ID = `ScanScope::Session` / ペイン = `ScanScope::Under`）
//! 5. 走査は解決済みの実体を直接起こす口を先に試す（ログインシェルは戻り道）
//!
//! 挙動そのもの（新しい報告があれば問い合わせ 0 回・無い / 古い / 止まった報告では走査へ落ちる）は
//! `dispatch::tests::issue2017_*` が同じスレッドの問い合わせ回数で確かめる。ここは並びの番犬で、
//! [`逆戻りを名指しできる`] が修正前を再現した注入を file:line で名指しすることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号は保たれる。#1420）
fn read(root: &Path, rel: &str) -> String {
    let src =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::scan(&src).text
}

#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((
        start + 1,
        lines[start..=end].iter().map(|l| l.to_string()).collect(),
    ))
}

/// 窓の中で `needle` を含む最初のコードの行（0-based の相対位置）
fn first(window: &[String], needle: &str) -> Option<usize> {
    all(window, needle).into_iter().next()
}

/// 窓の中で `needle` を含むコードの行すべて（0-based の相対位置）
fn all(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

struct Sources {
    dispatch: String,
    orch: String,
    agents: String,
    main: String,
}

impl Sources {
    fn load() -> Self {
        let root = workspace_root();
        Self {
            dispatch: read(&root, DISPATCH),
            orch: read(&root, ORCH),
            agents: read(&root, AGENTS),
            main: read(&root, MAIN),
        }
    }

    fn offenders(&self) -> Vec<Offender> {
        let mut out = scan_dispatch(&self.dispatch);
        out.extend(scan_scope(&self.orch, &self.agents));
        out.extend(scan_catalog(&self.main));
        out
    }
}

/// 窓が無いことを違反として返す（走査の空振りで緑にならない）
fn missing(file: &'static str, head: &str) -> Offender {
    Offender {
        file,
        line: 0,
        why: format!("`{head}` が見つからない（走査が空振り。改名したなら番犬も追うこと）"),
    }
}

/// 1 / 2. 監視の 3 口が mod の判断を走査より先に通す
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    // 1. worker_status
    let head = "fn finish_worker_status(";
    match fn_window(src, head) {
        None => out.push(missing(DISPATCH, head)),
        Some((at, w)) => {
            let plan = first(&w, "monitor_plan(");
            let skip = first(&w, "monitor.skips_scan()");
            for (needle, why) in [
                (
                    "resolve_session_id_for_backend",
                    "会話 ID の解決（`claude agents --json` + 器の pid 祖先辿り）",
                ),
                (
                    "query_agent_status(",
                    "状態の照会（`claude agents --json`）",
                ),
            ] {
                let hits = all(&w, needle);
                if hits.is_empty() {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: format!("`{head}` に `{needle}` が無い（走査へ落ちる腕が消えた = mod の無い claude を見失う）"),
                    });
                }
                for i in hits.into_iter().filter(|i| plan.is_none_or(|p| p > *i)) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at + i,
                        why: format!(
                            "`{head}` が mod の判断（`monitor_plan`）より先に {why} を呼んでいる \
                             （新しい mod の報告があっても走査する = #2017 の逆戻り）"
                        ),
                    });
                }
            }
            for i in all(&w, "query_agent_status(") {
                if skip.is_none_or(|s| s > i) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at + i,
                        why: format!(
                            "`{head}` の状態の照会が「mod が答えた」腕（`monitor.skips_scan()`）の後ろに無い \
                             （mod の報告の状態を後段で上書きするために走査する = #2017 の逆戻り）"
                        ),
                    });
                }
            }
        }
    }
    // 2. self / report
    for (head, needle, why) in [
        (
            "fn dispatch_orchestrator_self(",
            "monitor_plan(",
            "self が mod の判断を通さない（新しい報告があっても走査する）",
        ),
        (
            "fn collect_report_ctx(",
            "monitor_plan(",
            "report が UI スレッドで mod の報告の会話 ID を写さない",
        ),
        (
            "fn finish_orchestrator_report(",
            "mod_session.or_else(",
            "report が mod の会話 ID より先に走査する",
        ),
    ] {
        match fn_window(src, head) {
            None => out.push(missing(DISPATCH, head)),
            Some((at, w)) => {
                if first(&w, needle).is_none() {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: format!("`{head}` に `{needle}` が無い: {why}（#2017）"),
                    });
                }
            }
        }
    }
    out
}

/// 4 / 5. 走査を起こすときは範囲で絞り、直接起動を先に試す
fn scan_scope(orch: &str, agents: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src, head, needle, why) in [
        (
            ORCH,
            orch,
            "pub fn query_agent_status(",
            "ScanScope::Session(",
            "状態の照会が全アカウントを起こす（会話の居る走査先へ絞らない）",
        ),
        (
            AGENTS,
            agents,
            "pub fn resolve_session_id_for_backend(",
            "ScanScope::Under(",
            "会話 ID の解決が全アカウントを起こす（そのペインの claude の走査先へ絞らない）",
        ),
    ] {
        match fn_window(src, head) {
            None => out.push(missing(file, head)),
            Some((at, w)) => {
                if first(&w, needle).is_none() {
                    out.push(Offender {
                        file,
                        line: at,
                        why: format!("`{head}` に `{needle}` が無い: {why}（#2017）"),
                    });
                }
            }
        }
    }
    let head = "fn run_claude_agents_json_live(";
    match fn_window(orch, head) {
        None => out.push(missing(ORCH, head)),
        Some((at, w)) => {
            let direct = first(&w, "direct_agents_scan_command()");
            let shell = first(&w, "user_env_cli(");
            if let Some(i) = shell {
                if direct.is_none_or(|d| d > i) {
                    out.push(Offender {
                        file: ORCH,
                        line: at + i,
                        why: format!(
                            "`{head}` が直接起動（`direct_agents_scan_command`）より先にログインシェルの \
                             計画を組んでいる（走査 1 本ごとに rc を読むシェルが起きる = #2017 の逆戻り）"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 3. GUI のセッションカタログの走査が mod の答えたペインを除いた範囲で問う
fn scan_catalog(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = main.lines().collect();
    let Some(loop_at) = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains("CLAUDE_SESSION_SCAN_INTERVAL);"))
    else {
        out.push(missing(MAIN, "CLAUDE_SESSION_SCAN_INTERVAL"));
        return out;
    };
    // 定期スキャンのループ（間隔の判定から次の `.detach();` まで）
    let end = lines
        .iter()
        .enumerate()
        .skip(loop_at)
        .find(|(_, l)| l.trim() == ".detach();")
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    let window = &lines[loop_at..=end];
    let has = |needle: &str| {
        window
            .iter()
            .position(|l| !is_comment(l) && l.contains(needle))
    };
    if let Some(i) = has("list_agents_for_scan(") {
        out.push(Offender {
            file: MAIN,
            line: loop_at + i + 1,
            why: "セッションカタログの走査が全ペインを `claude agents --json` に問う \
                  （mod の報告が答えたペインまで走査する = #2017 の逆戻り。`list_agents_for_scan_scoped`）"
                .into(),
        });
    }
    for needle in [
        "monitor_plan(",
        "list_agents_for_scan_scoped(",
        "merge_detected(",
    ] {
        if has(needle).is_none() {
            out.push(Offender {
                file: MAIN,
                line: loop_at + 1,
                why: format!(
                    "セッションカタログの走査に `{needle}` が無い（mod の報告で答えたペインを走査から外す \
                     形が崩れた = #2017）"
                ),
            });
        }
    }
    out
}

#[test]
fn 監視の走査はmodの報告の後ろに居る() {
    let offenders = Sources::load().offenders();
    assert!(
        offenders.is_empty(),
        "#2017 の逆戻り:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::load();
    for (src, head) in [
        (&s.dispatch, "fn finish_worker_status("),
        (&s.dispatch, "fn dispatch_orchestrator_self("),
        (&s.dispatch, "fn collect_report_ctx("),
        (&s.dispatch, "fn finish_orchestrator_report("),
        (&s.orch, "pub fn query_agent_status("),
        (&s.orch, "fn run_claude_agents_json_live("),
        (&s.agents, "pub fn resolve_session_id_for_backend("),
    ] {
        let (_, w) = fn_window(src, head).unwrap_or_else(|| panic!("`{head}` の窓が採れる"));
        assert!(w.len() > 5, "`{head}` の窓が短すぎる（{} 行）", w.len());
    }
    assert!(s.main.contains("CLAUDE_SESSION_SCAN_INTERVAL);"));
}

#[test]
fn 逆戻りを名指しできる() {
    let s = Sources::load();
    assert!(s.offenders().is_empty(), "前提: 現行は緑");

    let expect_named = |label: &str, offenders: Vec<Offender>, file: &str, needle: &str| {
        let hit = offenders
            .iter()
            .find(|o| o.file == file && o.line > 0 && o.why.contains(needle));
        assert!(
            hit.is_some(),
            "注入「{label}」を名指しできない: {:?}",
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
        eprintln!("注入「{label}」→ {}", hit.unwrap().report());
    };
    let inject = |src: &str, from: &str, to: &str, label: &str| {
        let injected = src.replacen(from, to, 1);
        assert_ne!(injected, src, "注入「{label}」の置換元がある");
        injected
    };

    // ① worker_status が mod の判断の前に状態を照会する（修正前 = 会話 ID が取れたら無条件に走査）
    let d = inject(
        &s.dispatch,
        "    let mod_view = mod_state.view();\n",
        "    let mod_view = mod_state.view();\n    let _ = orchestrator::query_agent_status(\"x\");\n",
        "①",
    );
    expect_named(
        "worker_status の判断の前に走査",
        scan_dispatch(&d),
        DISPATCH,
        "fn finish_worker_status(",
    );

    // ② worker_status の状態の照会を mod の腕の前へ戻す
    let d = inject(
        &s.dispatch,
        "    let (status, mut ctx_percent) = if let (true, Ok(stored)) = (monitor.skips_scan(), &mod_view) {",
        "    let (status, mut ctx_percent) = if let Some(sid) = resolved_sid.as_deref() {\n        let agent = orchestrator::query_agent_status(sid);\n        (agent.status, agent.ctx_percent)\n    } else if let (true, Ok(stored)) = (monitor.skips_scan(), &mod_view) {",
        "②",
    );
    expect_named(
        "状態の照会を mod の腕の前へ",
        scan_dispatch(&d),
        DISPATCH,
        "monitor.skips_scan()",
    );

    // ③ report が mod の会話 ID を使わない
    let d = inject(
        &s.dispatch,
        "        mod_session.or_else(|| {",
        "        None.or_else(|| {",
        "③",
    );
    expect_named(
        "report の会話 ID を走査だけで",
        scan_dispatch(&d),
        DISPATCH,
        "fn finish_orchestrator_report(",
    );

    // ④ 状態の照会を全アカウントへ戻す
    let o = inject(
        &s.orch,
        "        &ScanScope::Session(session_id.to_string()),",
        "        &ScanScope::All,",
        "④",
    );
    expect_named(
        "状態の照会を全アカウントへ",
        scan_scope(&o, &s.agents),
        ORCH,
        "pub fn query_agent_status(",
    );

    // ⑤ ログインシェルを先に組む
    let o = inject(
        &s.orch,
        "    if let Some(mut command) = direct_agents_scan_command() {",
        "    let _ = tako_core::platform::child_cmd::user_env_cli(\"\", \"claude\", &[]);\n    if let Some(mut command) = direct_agents_scan_command() {",
        "⑤",
    );
    expect_named(
        "ログインシェルを先に",
        scan_scope(&o, &s.agents),
        ORCH,
        "fn run_claude_agents_json_live(",
    );

    // ⑥ カタログが全ペインを問う（修正前の list_agents_for_scan）
    let m = inject(
        &s.main,
        "tako_control::agents::list_agents_for_scan_scoped(",
        "tako_control::agents::list_agents_for_scan(",
        "⑥",
    );
    expect_named(
        "カタログが全ペインを問う",
        scan_catalog(&m),
        MAIN,
        "list_agents_for_scan",
    );
}
