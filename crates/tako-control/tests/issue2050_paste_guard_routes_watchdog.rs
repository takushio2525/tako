//! 番犬: 貼り付けの確認を広げた経路（#2050）が**本文・検出値を診断ログへ出さない**・
//! **判定の 1 実装を引く**。
//!
//! ## 何を止めたいのか
//!
//! #2050 で判定（`tako_core::paste_guard::scan_with`）を通す経路が 4 つ増えた:
//!
//! - `tako send` / MCP `tako_send_input`（dispatch の `send_paste_gate`）
//! - スマホの入力（remote.rs の `/api/panes/<id>/input` と `input_reply`）
//! - mod の送信時の確認（dispatch の `paste_guard_check_json` と mod の `guardPrompt`）
//! - GUI の「警告だけ」の段の通知（`show_paste_warning` / `render_paste_warning`）
//!
//! どれも本文（送る文字）を手に持つので、壊れ方は「本文・伏せ字・指紋を診断ログ
//! （persist.log / perf.log / stderr / mod のデバッグログ）へ書く」と「判定を自前で書く」の 2 つ。
//! persist.log へ書いてよいのは `paste_guard::log_line`（規則名・件数・バイト数）の 1 行だけ
//! （AGENTS.md の絶対ルール）。ここはソース走査で押さえ、**落ちるときは file:line で名指し**。
//!
//! 相方: 判定そのものは `tako_core::paste_guard` の単体、経路の振る舞いは dispatch の単体
//! （`issue2050_*`）と実経路テスト `scripts/test-paste-guard-2050.sh`。
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① dispatch.rs の `send_paste_gate` に `crate::diag::persist_log(text);` を足す /
//! ② remote.rs の入力の腕の `json!({ "route": p, "bytes": input_bytes })` を
//!    `json!({ "route": p, "text": text })` へ /
//! ③ register.ts の `guardPrompt` の `$.ui.log('tako mod: paste-guard check failed'` を
//!    `` $.ui.log(`tako mod: ${text}` `` へ /
//! ④ dispatch.rs の `paste_guard_check_json` の `pg::scan_with(` を自前の判定（`text.contains("ghp_")`）へ

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const DISPATCH_REL: &str = "crates/tako-control/src/dispatch.rs";
const REMOTE_REL: &str = "crates/tako-control/src/remote.rs";
const STATE_REL: &str = "crates/tako-control/src/paste_guard.rs";
const APP_REL: &str = "crates/tako-app/src/main.rs";
const MOD_REL: &str = "crates/tako-core/claude-mod/hooks/register.ts";
const CORE_REL: &str = "crates/tako-core/src/paste_guard.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `start` を含む行から、`end` に一致する行（`end` が空なら同じ字下げの `}`）までを
/// 行番号つきで返す（コメント行を落とす）
fn window(rel: &str, src: &str, start: &str, end: Option<&str>) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains(start))
        .unwrap_or_else(|| panic!("{rel}: 目印 {start:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[at].len() - lines[at].trim_start().len());
    let close = format!("{indent}}}");
    let stop = lines
        .iter()
        .enumerate()
        .skip(at + 1)
        .find(|(_, l)| match end {
            Some(e) => l.contains(e),
            None => **l == close,
        })
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {start:?} の範囲を閉じる行が無い"));
    let body: Vec<(usize, String)> = (at..stop)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= 3,
        "{rel}:{}: {start:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        at + 1,
        body.len()
    );
    body
}

/// 診断ログへ書く口（本文を手に持つ経路で使ってよいのは persist_log + log_line だけ）
const BANNED: &[&str] = &[
    "eprintln!",
    "println!",
    "tracing::",
    "dbg!",
    "perf_span(",
    "log::",
];

/// 本文を手に持つ範囲で、診断ログへ書くのが `log_line` の 1 行だけであること
fn assert_logs_only_log_line(rel: &str, body: &[(usize, String)], what: &str) -> Vec<String> {
    let mut bad = Vec::new();
    for (i, (line, text)) in body.iter().enumerate() {
        for banned in BANNED {
            if text.contains(banned) {
                bad.push(format!(
                    "{rel}:{line}: {what} で {banned} を使っている（本文・伏せ字を診断ログへ出さない = \
                     AGENTS.md の絶対ルール / #2050）: {}",
                    text.trim()
                ));
            }
        }
        if let Some(after) = text.split("persist_log(").nth(1) {
            let arg = match after.trim() {
                "" => body.get(i + 1).map(|(_, t)| t.trim()).unwrap_or(""),
                a => a,
            };
            let arg = arg.trim_start_matches('&');
            if !(arg.starts_with("pg::log_line(")
                || arg.starts_with("tako_core::paste_guard::log_line(")
                || arg.starts_with("paste_guard::log_line("))
            {
                bad.push(format!(
                    "{rel}:{line}: {what} が persist.log へ log_line 以外を書いている（規則名と件数だけ = \
                     #2050）: {}",
                    text.trim()
                ));
            }
        }
    }
    bad
}

/// TS の 1 行から、文字列の中身を除いたコードの識別子を取り出す（テンプレートの `${…}` の中は残す）。
/// `'…check failed'` の語は拾わず、`` `${text}` `` の `text` は拾う
fn code_tokens(src: &str) -> Vec<String> {
    let mut code = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('`') if depth == 0 => {
                if c == '`' {
                    quote = None;
                } else if c == '$' && chars.peek() == Some(&'{') {
                    chars.next();
                    depth = 1;
                    code.push(' ');
                }
            }
            Some('`') => {
                // テンプレートの `${…}` の中 = コード
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            code.push(' ');
                            continue;
                        }
                    }
                    _ => {}
                }
                code.push(c);
            }
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if matches!(c, '\'' | '"' | '`') {
                    quote = Some(c);
                    code.push(' ');
                } else {
                    code.push(c);
                }
            }
        }
    }
    code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn 識別子の取り出しは文字列の中身を拾わずテンプレートの式を拾う() {
    assert_eq!(
        code_tokens("'tako mod: paste-guard check failed', { to: 'debug' })"),
        ["to"]
    );
    assert_eq!(
        code_tokens("`tako mod: paste-guard exit ${done.exitCode}`, { to: 'debug' })"),
        ["done", "exitCode", "to"]
    );
    assert!(code_tokens("`tako mod: ${text}`)").contains(&"text".to_string()));
    assert!(code_tokens("String(err).slice(0, 200))").contains(&"err".to_string()));
}

fn fail_if(bad: Vec<String>) {
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// ① `tako send` / MCP `tako_send_input` と mod の送信時の確認（dispatch）
#[test]
fn sendとmodの確認は本文を診断ログへ書かず判定の1実装を引く() {
    let src = read(DISPATCH_REL);
    let mut bad = Vec::new();
    for (sig, what) in [
        ("fn send_paste_gate(", "tako send / tako_send_input の確認"),
        ("fn paste_guard_check_json(", "mod の送信時の確認"),
    ] {
        let body = window(DISPATCH_REL, &src, sig, None);
        bad.extend(assert_logs_only_log_line(DISPATCH_REL, &body, what));
        let joined: String = body
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !joined.contains("pg::scan_with(") {
            bad.push(format!(
                "{DISPATCH_REL}:{}: {what} が判定の 1 実装（tako_core::paste_guard::scan_with）を引いていない（#2050）",
                body[0].0
            ));
        }
    }
    // Send の腕は送る前に確認を通し、確かめる段では送達の経路へ入らない
    let arm = window(
        DISPATCH_REL,
        &src,
        "            let (warning, ack) =",
        Some("        Request::Read {"),
    );
    let joined: String = arm
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let gate = joined.find("send_paste_gate(");
    let blocked = joined.find("SendGate::Blocked(reply) => return Ok(reply)");
    let deliver = joined.find("send_to_pane(");
    if !matches!((gate, blocked, deliver), (Some(g), Some(b), Some(d)) if g < b && b < d) {
        bad.push(format!(
            "{DISPATCH_REL}:{}: Send の腕の順序が崩れた（確認 → 確かめる段なら返す → それ以外だけ送る = #2050）: \
             gate={gate:?} blocked={blocked:?} deliver={deliver:?}",
            arm[0].0
        ));
    }
    fail_if(bad);
}

/// ② スマホの入力（remote.rs）: 監査は数だけ・応答は伏せ字だけ・診断ログへ本文を書かない
#[test]
fn スマホの入力は本文を監査にもログにも書かない() {
    let src = read(REMOTE_REL);
    let mut bad = Vec::new();
    let arm = window(
        REMOTE_REL,
        &src,
        r#"(tiny_http::Method::Post, p) if p.starts_with("/api/panes/") && p.ends_with("/input") => {"#,
        Some("(tiny_http::Method::Post, p) if p.starts_with(\"/api/panes/\") && p.ends_with(\"/respond\")"),
    );
    bad.extend(assert_logs_only_log_line(REMOTE_REL, &arm, "スマホの入力"));
    for (line, text) in &arm {
        if text.contains(".audit(") || text.contains("json!({ \"route\"") {
            let next: String = arm
                .iter()
                .skip_while(|(l, _)| l < line)
                .take(5)
                .map(|(_, t)| t.as_str())
                .collect();
            if next.contains("\"text\"") || next.contains("text.clone()") || next.contains("&text")
            {
                bad.push(format!(
                    "{REMOTE_REL}:{line}: スマホの入力の監査に本文を載せている（数だけ = #2050）: {}",
                    text.trim()
                ));
            }
        }
    }
    if !arm.iter().any(|(_, l)| l.contains("Relay::Human")) {
        bad.push(format!(
            "{REMOTE_REL}:{}: スマホの入力が人の入力の印（Relay::Human）を付けていない（確認の段を素通りする = #2050）",
            arm[0].0
        ));
    }
    let reply = window(
        REMOTE_REL,
        &src,
        "fn input_reply(reply: &Value) -> (u16, Value) {",
        None,
    );
    bad.extend(assert_logs_only_log_line(
        REMOTE_REL,
        &reply,
        "スマホの入力の応答",
    ));
    fail_if(bad);
}

/// ③ 経路をまたぐ状態（指紋と数だけ）は何もログへ書かない・`Acks` に Debug を付けない
#[test]
fn 経路をまたぐ状態は指紋と数だけを持ちログへ書かない() {
    let state = read(STATE_REL);
    let mut bad = Vec::new();
    for (i, line) in state.lines().enumerate() {
        if is_comment(line) {
            continue;
        }
        for banned in BANNED.iter().chain(&["persist_log("]) {
            if line.contains(banned) {
                bad.push(format!(
                    "{STATE_REL}:{}: 経路をまたぐ状態がログへ書いている（指紋も数も診断ログへ出す理由が無い = #2050）: {}",
                    i + 1,
                    line.trim()
                ));
            }
        }
    }
    let core = read(CORE_REL);
    let lines: Vec<&str> = core.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("pub struct Acks {"))
        .unwrap_or_else(|| panic!("{CORE_REL}: pub struct Acks が消えている"));
    for back in 1..=6 {
        let l = lines[at - back];
        if !l.trim_start().starts_with("#[") && !l.trim_start().starts_with("///") {
            break;
        }
        if l.contains("derive") && l.contains("Debug") {
            bad.push(format!(
                "{CORE_REL}:{}: Acks に Debug が付いた（`{{:?}}` 1 つで指紋がログへ出る道 = #2050）",
                at - back + 1
            ));
        }
    }
    fail_if(bad);
}

/// ④ GUI の「警告だけ」の段: 通知は伏せ字だけを持ち、通知の経路はログへ書かない
#[test]
fn guiの警告だけの通知は本文を持たずログへ書かない() {
    let src = read(APP_REL);
    let mut bad = Vec::new();
    let st = window(APP_REL, &src, "struct PasteWarning {", None);
    for (line, text) in &st {
        let t = text.trim();
        if t.starts_with("text:") || t.contains(": String,") {
            bad.push(format!(
                "{APP_REL}:{line}: PasteWarning が本文を持っている（規則と伏せ字だけ = #2050）: {t}"
            ));
        }
    }
    let lines: Vec<&str> = src.lines().collect();
    let at = st[0].0 - 1;
    for back in 1..=4 {
        let l = lines[at - back];
        if !l.trim_start().starts_with("#[") && !l.trim_start().starts_with("///") {
            break;
        }
        if l.contains("derive") && l.contains("Debug") {
            bad.push(format!(
                "{APP_REL}:{}: PasteWarning に Debug が付いた（#2050）",
                at - back + 1
            ));
        }
    }
    for (sig, what) in [
        ("    fn show_paste_warning(", "警告だけの通知"),
        (
            "    fn render_paste_warning(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {",
            "警告だけの通知の描画",
        ),
        (
            "    fn note_flow_delivered(&self, pane: PaneId, text: &str) {",
            "流した値の記録",
        ),
    ] {
        let body = window(APP_REL, &src, sig, None);
        bad.extend(assert_logs_only_log_line(APP_REL, &body, what));
        for (line, text) in &body {
            if text.contains("persist_log(") {
                bad.push(format!(
                    "{APP_REL}:{line}: {what} が persist.log へ書いている（記録は関所の 1 行だけ = #2050）: {}",
                    text.trim()
                ));
            }
        }
    }
    fail_if(bad);
}

/// ⑤ mod の送信時の確認: 本文は標準入力で渡し（argv に載せない）、デバッグログへは終了コードだけ
#[test]
fn modの送信時の確認は本文を標準入力で渡しデバッグログへ書かない() {
    let src = read(MOD_REL);
    let mut bad = Vec::new();
    let mut body = window(
        MOD_REL,
        &src,
        "async function guardPrompt($: EngineInterface, text: string): Promise<'pass' | 'drop'> {",
        None,
    );
    // やめた文を入力欄へ戻す補助も本文を持つので同じ規則で見る
    body.extend(window(
        MOD_REL,
        &src,
        "async function restoreDraft($: EngineInterface, text: string): Promise<void> {",
        None,
    ));
    let body = &body[..];
    let joined: String = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if !joined.contains("stdin: text") {
        bad.push(format!(
            "{MOD_REL}:{}: guardPrompt が本文を標準入力で渡していない（argv はプロセス一覧・ログに残る = #2050）",
            body[0].0
        ));
    }
    if !joined.contains("'paste-guard', 'check', '--pane'") {
        bad.push(format!(
            "{MOD_REL}:{}: guardPrompt が tako の判定（paste-guard check --pane）を引いていない（判定は tako の 1 実装 = #2050）",
            body[0].0
        ));
    }
    for (line, text) in body {
        if text.contains("process.run(") && text.contains("text") && !text.contains("stdin") {
            bad.push(format!(
                "{MOD_REL}:{line}: 本文を argv に載せている（#2050）: {}",
                text.trim()
            ));
        }
        if let Some(after) = text.split("$.ui.log(").nth(1) {
            let tokens = code_tokens(after);
            for leak in ["text", "stdout", "found", "check", "findings", "err", "e"] {
                if tokens.iter().any(|t| t == leak) {
                    bad.push(format!(
                        "{MOD_REL}:{line}: mod のデバッグログへ本文・判定の中身（{leak}）を書いている（終了コードだけ = #2050）: {}",
                        text.trim()
                    ));
                }
            }
        }
    }
    fail_if(bad);
}
