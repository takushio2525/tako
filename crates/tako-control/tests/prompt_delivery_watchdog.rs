//! 番犬: 送達フローが**無音で止まらない**（Issue #1259）
//!
//! ## なぜ止めるのか
//!
//! `tako_send_input(await_prompt=true)` は `{"queued": true}` を即返し、実際の送達は
//! `tako-app` の `drive_prompt_flows` が 500ms tick で回す。#1259 の本番では
//! そのフローが 2 回とも届かず、**痕跡がどこにも残らなかった**。痕跡が残る口は
//! 当時 3 つあり、後続 send（`PromptDeliveryFlow::FollowUpSend`）ではその全部が空振りする:
//!
//! 1. worker レジストリ: `record_prompt_delivery_at` は先頭で
//!    `if flow != PromptDeliveryFlow::SpawnPrompt { return Ok(()) }` = **spawn 専用**
//! 2. `eprintln!`: GUI（.app）の stderr は誰も読めない
//! 3. `persist.log`: 書くのは `delivery::try_peer` / `log_fallback` = **貼り付け段へ
//!    到達した後**だけ
//!
//! 結果、`WaitPromptReady` で入力欄が読めない・peer の背景試行が返らない・起動コマンド
//! 待ちで保留、のいずれでも「queued: true なのに何も起きず、ログも 0 行」になった。
//!
//! ## 何を違反とするか
//!
//! `drive_prompt_flows` を対象に 5 つ:
//!
//! - **① 進めずにフローを積み直す枝は必ず理由を記録する**
//!   （`remaining.push(flow)` を含むブロックに `note_flow` / `finish_flow` /
//!   `flow.stall = Some(` のどれかがある）
//! - **② 総合タイムアウトの枝は `persist.log` へ残す**（`finish_flow` を呼ぶ）
//! - **③ 入力欄待ちには「読めなかった」枝がある**（`await_prompt` だと
//!   `input_line(&lines).is_some()` が唯一の入口なので、else が無いと無音になる）
//! - **④ peer の背景試行は上限判定を通す**（`None => PeerStep::Pending` 直書きを禁じる）
//! - **⑤ レジストリが後続 send を弾く以上、`persist.log` の記録は外せない**
//!   （`record_prompt_delivery_at` の spawn 限定ガードと ② の対応を結ぶ）
//!
//! ①〜④ は違反行を**行番号で名指し**する。ブロックの切り出しはインデントで行う
//! （`rustfmt` が通っている前提。CI は `cargo fmt --all --check` を回している）

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 関数の本体を「`fn <name>` の行から、同じインデントで閉じる `}` まで」で切り出し、
/// (元ファイルでの 1-origin 行番号, 行) の並びで返す
fn function_lines(src: &str, signature: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{signature} が見つからない（関数名が変わった？）"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let mut out = vec![(start + 1, lines[start].to_string())];
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        out.push((i + 1, line.to_string()));
        let trimmed = line.trim_start();
        let here = line.len() - trimmed.len();
        if trimmed == "}" && here == indent {
            break;
        }
    }
    out
}

/// `at` 行を含む最小のブロック（開き `{` から対応する閉じ `}` まで）。
///
/// **`at` の後ろも含める**のが要点: 記録の呼び出しは警告の `eprintln!` より
/// 後ろに書かれるので、前方だけ見ると「記録していない」と誤検知する
fn enclosing_block(body: &[(usize, String)], at: usize) -> Vec<&str> {
    let indent_of = |s: &str| s.len() - s.trim_start().len();
    let depth = indent_of(&body[at].1);
    let mut from = at;
    while from > 0 {
        from -= 1;
        let line = &body[from].1;
        if line.trim().is_empty() {
            continue;
        }
        if indent_of(line) < depth && line.trim_end().ends_with('{') {
            break;
        }
    }
    let open_indent = indent_of(&body[from].1);
    let mut to = at;
    for (i, (_, line)) in body.iter().enumerate().skip(at + 1) {
        to = i;
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if indent_of(line) == open_indent && trimmed.starts_with('}') {
            break;
        }
    }
    body[from..=to].iter().map(|(_, l)| l.as_str()).collect()
}

const APP: &str = "crates/tako-app/src/main.rs";
const REGISTRY: &str = "crates/tako-control/src/orchestrator/registry.rs";

/// ① 進めずに積み直す枝は必ず理由を記録する
#[test]
fn 送達フローは進めない理由を必ず記録する() {
    let src = read(APP);
    let body = function_lines(&src, "fn drive_prompt_flows(&mut self)");
    let mut offenders = Vec::new();
    let mut pushes = 0;
    for (idx, (lineno, line)) in body.iter().enumerate() {
        if !line.contains("remaining.push(flow)") {
            continue;
        }
        pushes += 1;
        let block = enclosing_block(&body, idx).join("\n");
        let records = block.contains("note_flow(")
            || block.contains("finish_flow(")
            || block.contains("flow.stall = Some(");
        if !records {
            offenders.push(format!("{APP}:{lineno}: {}", line.trim()));
        }
    }
    assert!(
        pushes >= 4,
        "`remaining.push(flow)` が {pushes} 箇所しか無い（走査が空振りしている）"
    );
    assert!(
        offenders.is_empty(),
        "進めずに積み直すのに理由を記録していない枝がある（#1259 の無音経路）:\n{}",
        offenders.join("\n")
    );
}

/// ② 総合タイムアウトは `persist.log` へ残す（レジストリは後続 send を弾くので）
#[test]
fn 送達フローの打ち切りはpersistlogへ残す() {
    let src = read(APP);
    let body = function_lines(&src, "fn drive_prompt_flows(&mut self)");
    let at = body
        .iter()
        .position(|(_, l)| l.contains("プロンプト送達フローがタイムアウト"))
        .expect("総合タイムアウトの警告が見つからない");
    let block = enclosing_block(&body, at).join("\n");
    assert!(
        block.contains("finish_flow("),
        "{APP}:{} 打ち切りが eprintln だけで終わっている（#1259: レジストリは \
         FollowUpSend を弾くので persist.log へ残さないと痕跡が 0 行になる）",
        body[at].0
    );
}

/// ③ 入力欄待ちには「読めなかった」枝がある
#[test]
fn 入力欄が読めないときの理由が置かれている() {
    let src = read(APP);
    let body = function_lines(&src, "fn drive_prompt_flows(&mut self)");
    let placed = body.iter().any(|(_, l)| l.contains("Stall::NoInputBox"));
    assert!(
        placed,
        "{APP} の drive_prompt_flows に `Stall::NoInputBox` を置く枝が無い。\
         `await_prompt=true` では `input_line(&lines).is_some()` が唯一の入口なので、\
         入力欄が読めない画面では**どの枝も踏まず何も記録しない**（#1259 の真因）"
    );
}

/// ④ peer の背景試行は上限判定を通す（返らない試行を無限に待たない）
#[test]
fn peerの背景試行は上限判定を通す() {
    let src = read(APP);
    let body = function_lines(&src, "fn drive_peer_attempt(");
    let offenders: Vec<String> = body
        .iter()
        .filter(|(_, l)| {
            let t = l.trim();
            t.starts_with("None =>") && t.contains("PeerStep::Pending")
        })
        .map(|(lineno, line)| format!("{APP}:{lineno}: {}", line.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "peer の結果待ちを無条件に Pending へ倒している（#1259: `try_peer` は \
         `ps` / `tmux list-panes -a` を上限なしで叩くので返らないことがある。\
         `tako_core::prompt_delivery::peer_wait` の判断を通す）:\n{}",
        offenders.join("\n")
    );
    let all = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all.contains("peer_timeout_step("),
        "上限判定（peer_timeout_step）を経由していない"
    );
}

/// ④' 上限で諦めるときの二重投函を CAS で塞いでいる（#1259 / #790）。
///
/// 「段階を読んでから落ちる」形では、読んだ直後に背景スレッドが書き始めるので
/// 同じ指示が 2 回届く。判定は `PeerAttemptState` の CAS を通し、`try_peer` は
/// **書き込みの直前に続行可否を訊いて**、否なら 1 バイトも書かずに引き返すこと
#[test]
fn peerの打ち切りは二重投函を塞いでいる() {
    let app = read(APP);
    let peer = function_lines(&app, "fn peer_timeout_step(")
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        peer.contains("peer_wait_now("),
        "{APP} の peer_timeout_step が CAS 版（peer_wait_now）を使っていない\
         （段階を読むだけの判定では諦めた直後の送信を止められない）"
    );

    let delivery = read("crates/tako-control/src/delivery.rs");
    let body = function_lines(&delivery, "pub fn try_peer(")
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let sending = body
        .find("observer.mark(PeerPhase::Sending)")
        .expect("try_peer が Sending を知らせていない");
    let head = &body[..sending];
    assert!(
        head.trim_end().ends_with("if !"),
        "try_peer が `Sending` の続行可否を見ていない（#1259: 見ずに書くと、\
         呼び出し側が上限で諦めた後にも送ってしまい二重投函になる）"
    );
    let tail = &body[sending..];
    let ret = tail
        .find("return PeerAttempt::Fallback")
        .expect("続行不可のときに引き返していない");
    let send = tail
        .find("peer_messaging::send(")
        .expect("送信の呼び出しが見つからない");
    assert!(
        ret < send,
        "引き返しが送信より後ろにある（1 バイト書いてから諦めている）"
    );
}

/// ⑤ レジストリが後続 send を弾く以上、persist.log の記録は外せない。
///
/// 「なぜ ② が要るのか」の根拠をコード側に固定する。ガードが消えて
/// レジストリが後続 send も追うようになったら、この番犬が理由を添えて落ちる
#[test]
fn レジストリが後続sendを弾くことと打ち切りの記録が対応している() {
    let registry = read(REGISTRY);
    let guarded = registry.contains("if flow != PromptDeliveryFlow::SpawnPrompt");
    let app = read(APP);
    let body = function_lines(&app, "fn drive_prompt_flows(&mut self)")
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let logged = body.contains("finish_flow(");
    assert!(
        !guarded || logged,
        "{REGISTRY} は後続 send（FollowUpSend）の送達結果を捨てるのに、\
         {APP} の送達フローは persist.log へ残していない（#1259 の再来）"
    );
    assert!(
        guarded || logged,
        "レジストリのガードが消えている。後続 send の未達がレジストリへ載るなら \
         #1259 の前提が変わるので、番犬とコメントを見直すこと"
    );
}

/// 理由コードの語彙が `tako_core::prompt_delivery` の 1 実装から来ている
/// （送達フロー・応答・CLI・persist.log が別々の綴りを持たない）
#[test]
fn 理由コードの語彙は一箇所から来ている() {
    let app = read(APP);
    let body = function_lines(&app, "fn drive_prompt_flows(&mut self)")
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // 生文字列の理由（"no_input_box" 等）を直書きしていないこと。
    // 綴りは `Stall::…` 経由でしか出さない
    // `unverified_reason`（#530 からの別語彙。`choice_dialog` / `paste_not_reflected`）は
    // 対象外。ここで見るのは **#1259 で足した停滞理由**の綴り
    for code in [
        "no_input_box",
        "peer_pending",
        "hold_for_command_flow",
        "session_gone",
        "behind_flow",
        "tui_not_ready",
        "enter_resend",
        "queue_drain",
        "peer_send_stalled",
    ] {
        let quoted = format!("\"{code}\"");
        assert!(
            !body.contains(&quoted),
            "{APP} の drive_prompt_flows が理由コード {quoted} を直書きしている\
             （`tako_core::prompt_delivery::Stall` から引く）"
        );
    }
}
