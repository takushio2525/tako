//! 残りの `claude agents --json` 走査が mod の報告の後ろに居るかの番犬（#2041。エピック #2011）
//!
//! # なぜ要るか
//!
//! #2017 で監視の走査を mod の報告で置き換えた後に、走査を起こす口が 2 つ残っていた。
//!
//! 1. GUI モードのチャット表示（`chat_view::load_chat_refresh` → `agents::live_claude_sessions`）が
//!    2 秒 tick のたびに**全アカウント**を問い、鮮度 30 秒ごとにアカウント数ぶん `claude agents --json`
//!    を起こしていた（mod の報告が会話 ID を運んでいても）
//! 2. `/clear`・`/resume` の後もレジストリの会話 ID が spawn 直後のまま残り、worker_status は名指しが
//!    報告と食い違う（`session_mismatch`）ので 5 秒ごとに走査し続けていた（答える会話 ID も `/clear` 前の
//!    会話のまま。器の無いペインはセッションカタログの走査が止まる構成なのでずっと）
//!
//! # 何を縛るか
//!
//! 1. `load_chat_refresh` は live 解決を `chat_live_sessions` 経由で引く（全アカウントの
//!    `live_claude_sessions` を直に呼ばない）
//! 2. `chat_live_sessions` が全アカウントの `live_claude_sessions` を呼ぶのは A/B の腕
//!    （`session_legacy()`）だけで、それ以外は `chat_scan_ask` の範囲を `live_claude_sessions_asking` で問う
//! 3. `chat_scan_ask` は mod の報告が答えたペインを外す（`mod_live.is_none()`）。`collect_chat_targets` は
//!    監視と同じ `monitor_plan` で報告から `LiveClaudeSession::from_mod` を組む
//! 4. 絞った live 解決は `ScanScope::Under` で走査先を絞る
//! 5. `finish_worker_status` は `follow_session` を `monitor_plan` より先に呼び、その答えで名指しを
//!    差し替え（`let session_id = followed…`）、レジストリへ書き戻す（`record_session_detected`）
//! 6. A/B の env の名前（`TAKO_2041_LEGACY`）を読むのは `claude_mod::session_legacy` だけ
//!
//! 挙動そのもの（報告があれば問い合わせ 0 回・`/clear` 後の 1 回目でレジストリが更新され以後も 0 回）は
//! `chat_view::tests::issue2041_*` と `dispatch::tests::issue2041_*` が同じスレッドの問い合わせ回数で
//! 確かめる。ここは並びの番犬で、[`逆戻りを名指しできる`] が修正前を再現した注入を file:line で
//! 名指しすることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const CHAT: &str = "crates/tako-app/src/chat_view.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";
const ENV_NAME: &str = "TAKO_2041_LEGACY";

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
    file: String,
    line: usize,
    why: String,
}

impl Offender {
    fn new(file: &str, line: usize, why: String) -> Self {
        Self {
            file: file.to_string(),
            line,
            why,
        }
    }

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

/// 窓の中で `needle` を含むコードの行すべて（0-based の相対位置）
fn all(window: &[String], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

fn first(window: &[String], needle: &str) -> Option<usize> {
    all(window, needle).into_iter().next()
}

/// 窓が無いことを違反として返す（走査の空振りで緑にならない）
fn missing(file: &str, head: &str) -> Offender {
    Offender::new(
        file,
        0,
        format!("`{head}` が見つからない（走査が空振り。改名したなら番犬も追うこと）"),
    )
}

/// 全アカウントを問う live 解決の呼び出し（`_asking` / `_by_backend` / `_in` は別物）
fn whole_live_calls(window: &[String]) -> Vec<usize> {
    all(window, "live_claude_sessions(")
        .into_iter()
        .filter(|i| {
            let l = &window[*i];
            !l.contains("fn live_claude_sessions(") && !l.contains("live_claude_sessions_in(")
        })
        .collect()
}

/// 1〜3. GUI モードのチャット表示
fn scan_chat(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    // 1. 読み取りの入口
    let head = "fn load_chat_refresh(";
    match fn_window(src, head) {
        None => out.push(missing(CHAT, head)),
        Some((at, w)) => {
            for i in whole_live_calls(&w) {
                out.push(Offender::new(
                    CHAT,
                    at + i,
                    format!(
                        "`{head}` が全アカウントの `live_claude_sessions` を直に呼んでいる \
                         （mod の報告が答えたペインまで 30 秒ごとに走査する = #2041 の逆戻り。\
                         `chat_live_sessions` を通す）"
                    ),
                ));
            }
            if first(&w, "chat_live_sessions(").is_none() {
                out.push(Offender::new(
                    CHAT,
                    at,
                    format!("`{head}` に `chat_live_sessions(` が無い（mod の報告で答える口が外れた = #2041）"),
                ));
            }
        }
    }
    // 2. live 解決の本体
    let head = "fn chat_live_sessions(";
    match fn_window(src, head) {
        None => out.push(missing(CHAT, head)),
        Some((at, w)) => {
            let legacy = first(&w, "session_legacy()");
            for i in whole_live_calls(&w) {
                // A/B の腕（`session_legacy()` の直後の return）だけが全アカウントを問ってよい
                if legacy.is_none_or(|l| i < l || i > l + 2) {
                    out.push(Offender::new(
                        CHAT,
                        at + i,
                        format!(
                            "`{head}` が A/B の腕の外で全アカウントの `live_claude_sessions` を呼んでいる \
                             （mod の報告の有無にかかわらず走査する = #2041 の逆戻り）"
                        ),
                    ));
                }
            }
            for (needle, why) in [
                ("chat_scan_ask(", "答えの要るペインを数えない"),
                (
                    "live_claude_sessions_asking(",
                    "走査を答えの要るペインへ絞らない",
                ),
                ("mod_live", "mod の報告の答えを入れない"),
            ] {
                if first(&w, needle).is_none() {
                    out.push(Offender::new(
                        CHAT,
                        at,
                        format!("`{head}` に `{needle}` が無い: {why}（#2041）"),
                    ));
                }
            }
        }
    }
    // 3. 問う先の数え方と報告の写し
    for (head, needle, why) in [
        (
            "fn chat_scan_ask(",
            "mod_live.is_none()",
            "mod の報告が答えたペインまで走査へ回す",
        ),
        (
            "fn collect_chat_targets(",
            "monitor_plan(",
            "報告で答えるかの判断が監視と別になった",
        ),
        (
            "fn collect_chat_targets(",
            "LiveClaudeSession::from_mod(",
            "報告から live 解決を組まない",
        ),
    ] {
        match fn_window(src, head) {
            None => out.push(missing(CHAT, head)),
            Some((at, w)) => {
                if first(&w, needle).is_none() {
                    out.push(Offender::new(
                        CHAT,
                        at,
                        format!("`{head}` に `{needle}` が無い: {why}（#2041）"),
                    ));
                }
            }
        }
    }
    out
}

/// 4. 絞った live 解決は走査先を絞る
fn scan_agents(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let head = "fn live_claude_sessions_in(";
    match fn_window(src, head) {
        None => out.push(missing(AGENTS, head)),
        Some((at, w)) => {
            if first(&w, "ScanScope::Under(").is_none() {
                out.push(Offender::new(
                    AGENTS,
                    at,
                    format!(
                        "`{head}` に `ScanScope::Under(` が無い: 答えの要るペインを問うときも全アカウントを \
                         起こす（#2041）"
                    ),
                ));
            }
        }
    }
    out
}

/// 5. worker_status はレジストリの会話 ID を報告へ追従させてから mod の判断を通す
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let head = "fn finish_worker_status(";
    let Some((at, w)) = fn_window(src, head) else {
        out.push(missing(DISPATCH, head));
        return out;
    };
    let follow = first(&w, "follow_session(");
    let rename = first(&w, "let session_id = followed");
    let plan = first(&w, "monitor_plan(");
    // 追従した会話 ID（`new_sid`）を書く行（lazy 昇格の `record_session_detected` は別物）
    let write = w.iter().position(|l| {
        !is_comment(l) && l.contains("record_session_detected") && l.contains("new_sid")
    });
    match (follow, rename, plan) {
        (Some(f), Some(r), Some(p)) if f < r && r < p => {}
        _ => {
            let line = plan.or(follow).map_or(at, |i| at + i);
            out.push(Offender::new(
                DISPATCH,
                line,
                format!(
                    "`{head}` が `follow_session` の答えで名指しを差し替えてから `monitor_plan` を呼んでいない \
                     （`/clear` の後もレジストリの古い会話 ID で `session_mismatch` へ落ちて走査し続ける = \
                     #2041 の逆戻り）"
                ),
            ));
        }
    }
    match (follow, write) {
        (Some(f), Some(wr)) if f < wr => {}
        _ => out.push(Offender::new(
            DISPATCH,
            follow.map_or(at, |i| at + i),
            format!(
                "`{head}` が追従した会話 ID をレジストリへ書き戻さない（`record_session_detected`）: \
                 次の照会も古い会話 ID を名指しする（#2041）"
            ),
        )),
    }
    out
}

/// 6. A/B の env の名前を読むのは `claude_mod::session_legacy` だけ
fn scan_env(files: &[(String, String)]) -> Vec<Offender> {
    let mut out = Vec::new();
    let literal = format!("\"{ENV_NAME}\"");
    for (rel, src) in files {
        for (i, line) in src.lines().enumerate() {
            if is_comment(line) || !line.contains(&literal) {
                continue;
            }
            let allowed = rel == CORE_MOD && line.contains("pub const SESSION_LEGACY_ENV");
            if !allowed {
                out.push(Offender::new(
                    rel,
                    i + 1,
                    format!(
                        "`{ENV_NAME}` を直に読んでいる（A/B の入口は `claude_mod::session_legacy` の 1 つ。\
                         散らばると片方の腕だけ戻る）"
                    ),
                ));
            }
        }
    }
    out
}

/// `crates/*/src` の .rs すべて（テスト領域は潰す）
fn production_sources(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name != "target" && name != "tests" && !name.starts_with('.') {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") {
                // 区切りは `/` へ揃えてから見る（Windows の区切りは `\` = 揃えずに見ると 1 件も拾わない）
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel.contains("/src/") {
                    out.push((rel.clone(), read(root, &rel)));
                }
            }
        }
    }
    out
}

struct Sources {
    chat: String,
    agents: String,
    dispatch: String,
    all: Vec<(String, String)>,
}

impl Sources {
    fn load() -> Self {
        let root = workspace_root();
        Self {
            chat: read(&root, CHAT),
            agents: read(&root, AGENTS),
            dispatch: read(&root, DISPATCH),
            all: production_sources(&root),
        }
    }

    fn offenders(&self) -> Vec<Offender> {
        let mut out = scan_chat(&self.chat);
        out.extend(scan_agents(&self.agents));
        out.extend(scan_dispatch(&self.dispatch));
        out.extend(scan_env(&self.all));
        out
    }
}

#[test]
fn 残りの走査はmodの報告の後ろに居る() {
    let offenders = Sources::load().offenders();
    assert!(
        offenders.is_empty(),
        "#2041 の逆戻り:\n{}",
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
        (&s.chat, "fn load_chat_refresh("),
        (&s.chat, "fn chat_live_sessions("),
        (&s.chat, "fn chat_scan_ask("),
        (&s.chat, "fn collect_chat_targets("),
        (&s.agents, "fn live_claude_sessions_in("),
        (&s.dispatch, "fn finish_worker_status("),
    ] {
        let (_, w) = fn_window(src, head).unwrap_or_else(|| panic!("`{head}` の窓が採れる"));
        assert!(w.len() > 5, "`{head}` の窓が短すぎる（{} 行）", w.len());
    }
    // env の走査が本番のソースを実際に見ている（定義の 1 行に当たる）
    let defined = s
        .all
        .iter()
        .filter(|(rel, src)| rel == CORE_MOD && src.contains(&format!("\"{ENV_NAME}\"")))
        .count();
    assert_eq!(defined, 1, "{CORE_MOD} の定義を見つけられない");
    assert!(
        s.all.iter().any(|(rel, _)| rel == CHAT),
        "tako-app のソースが走査に入っていない"
    );
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

    // ① チャットの読み取りが全アカウントの live 解決を直に呼ぶ（修正前の呼び方）
    let c = inject(
        &s.chat,
        "        false => chat_live_sessions(&targets, &direct),",
        "        false => tako_control::agents::live_claude_sessions(&direct),",
        "①",
    );
    expect_named(
        "チャットが全アカウントを直に問う",
        scan_chat(&c),
        CHAT,
        "fn load_chat_refresh(",
    );

    // ② A/B の腕の外で全アカウントを問う（mod の報告の有無にかかわらず走査）
    let c = inject(
        &s.chat,
        "    let (backends, ask_direct) = chat_scan_ask(targets);\n",
        "    let _ = tako_control::agents::live_claude_sessions(direct);\n    let (backends, ask_direct) = chat_scan_ask(targets);\n",
        "②",
    );
    expect_named(
        "A/B の腕の外で全アカウント",
        scan_chat(&c),
        CHAT,
        "A/B の腕の外",
    );

    // ③ 報告が答えたペインまで走査へ回す
    let c = inject(
        &s.chat,
        "        .filter(|t| t.agent_running && t.mod_live.is_none())",
        "        .filter(|t| t.agent_running)",
        "③",
    );
    expect_named(
        "報告の答えたペインも問う",
        scan_chat(&c),
        CHAT,
        "fn chat_scan_ask(",
    );

    // ④ 絞った live 解決が全アカウントを起こす
    let a = inject(
        &s.agents,
        "            list_agents_scoped(AgentScanFreshness::Ui, &ScanScope::Under(roots))",
        "            list_agents_with_freshness(AgentScanFreshness::Ui)",
        "④",
    );
    expect_named(
        "絞った live 解決が全アカウント",
        scan_agents(&a),
        AGENTS,
        "fn live_claude_sessions_in(",
    );

    // ⑤ worker_status が追従の答えを名指しに使わない（修正前 = レジストリの古い会話 ID のまま）
    let d = inject(
        &s.dispatch,
        "    let session_id = followed.as_deref().or(session_id);\n",
        "",
        "⑤",
    );
    expect_named(
        "名指しを差し替えない",
        scan_dispatch(&d),
        DISPATCH,
        "follow_session",
    );

    // ⑥ mod の判断を追従より先に置く（1 回目の照会は古い名指しで判断して走査する）
    let d = inject(
        &s.dispatch,
        "    let mod_view = mod_state.view();\n",
        "    let mod_view = mod_state.view();\n    let _first = tako_core::claude_mod::monitor_plan(&mod_view, mod_state.at(), session_id, false);\n",
        "⑥",
    );
    expect_named(
        "mod の判断を追従より先に",
        scan_dispatch(&d),
        DISPATCH,
        "follow_session",
    );

    // ⑦ 追従した会話 ID をレジストリへ書き戻さない
    let d = s
        .dispatch
        .replace(
            "orchestrator::registry::record_session_detected(ts, new_sid)",
            "Ok::<(), String>(())",
        )
        .replace(
            "orchestrator::registry::record_session_detected_by_pane(pane_id, new_sid)",
            "Ok::<(), String>(())",
        );
    assert_ne!(d, s.dispatch, "注入「⑦」の置換元がある");
    expect_named(
        "レジストリへ書き戻さない",
        scan_dispatch(&d),
        DISPATCH,
        "書き戻さない",
    );

    // ⑧ A/B の env を別の場所で直に読む
    let mut all = s.all.clone();
    let chat = all
        .iter_mut()
        .find(|(rel, _)| rel == CHAT)
        .expect("chat_view.rs が走査に入っている");
    chat.1 = inject(
        &chat.1,
        "    if tako_core::claude_mod::session_legacy() {",
        "    if std::env::var_os(\"TAKO_2041_LEGACY\").is_some() {",
        "⑧",
    );
    expect_named("env を直に読む", scan_env(&all), CHAT, ENV_NAME);
}
