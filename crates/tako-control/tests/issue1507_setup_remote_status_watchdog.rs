//! **#1507 の番犬**: `tako setup` の末尾が**リモート（スマホ）の状態を見ずに**
//! 同じ 1 行を出す形へ戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! setup の末尾は `スマホからリモート接続するには: tako remote setup` の固定文 1 行で、
//! Tailscale が未導入でも・ログイン済みで公開まで済んでいても**同じ案内**だった
//! （#1500 の棚卸し Z17 / 実測 R5 / R9）。setup 本体は remote の状態を 1 つも見ていなかった。
//!
//! ## この形
//!
//! 末尾は `remote_setup::setup_summary_lines()` の 1 実装が出す:
//! `remote_setup::check_status()`（読み取りだけ）の JSON から `phone_readiness` が状態を決め、
//! `phone_readiness_line` が 1 行にする。Tailscale が未導入なら次の一手は依存の導入口
//! （`setup_deps::next_step_line` = #1499 / #1524 の 1 実装）から引く。待ちは tailscale 側の
//! 上限で打ち切られ、打ち切りは #1503 の文面で知らせる。
//!
//! ## ここで止めるもの
//!
//! 1. [`setupは状態を見ない固定文を出さない`] — 固定文へ 1 行戻すだけで症状が戻る
//! 2. [`setupの末尾は状態の1実装を呼ぶ`] — 禁止だけでなく**寄せ先も縛る**（#1496 の 2 本立て）
//! 3. [`要約はcheck_statusの結果だけから組む`] — setup 用に tailscale を別口で聞くと、
//!    `tako remote setup` / GUI のリモートパネルと答えが割れる
//! 4. [`未導入の次の一手は依存の導入口から引く`] — 導入の案内を 2 か所で持たない（#1509）
//! 5. [`tailscaleの検出の待ちに上限がある`] — `--version` の素の `.status()` は上限を持たない
//!    （相手が固まると setup の末尾ごと固まる = #1503 の規約）
//! 6. [`打ち切りを未起動へ畳まない`] — `status --json` の打ち切りを「起動していません」と
//!    言い切らない（#1503 の「打ち切ったら必ず知らせる」）
//!
//! 落ちるときは **file:line で名指し**する。状態ごとの字面は `remote_setup` の単体テスト
//! （`状態ごとの1行を固定する`）、実経路（隔離 HOME + tailscale スタブで `--yes` / 非 TTY /
//! `--answers` / 時間切れ）は `scripts/test-setup-remote-status-1507.sh` が持つ。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const SETUP: &str = "crates/tako-cli/src/setup.rs";
const REMOTE_SETUP: &str = "crates/tako-control/src/remote_setup.rs";
const TAILSCALE: &str = "crates/tako-control/src/tailscale.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番領域（テスト領域を潰した全文。長さと行番号は保たれる）
fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// コメントだけを潰した眺め（文字列リテラルは残る = 画面へ出る文字も見える）
fn non_comment(rel: &str) -> String {
    code_view::without_comments_checked(&production(rel), rel)
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// 関数 `name` の本体（`{` から対応する `}` まで）のバイト範囲。
///
/// 波括弧は**コメントと文字列を潰した眺め**で数える（`format!("{x}")` の括弧を数えない）。
/// 眺めはバイト長を保つので、同じ範囲をコメントだけ潰した眺めへそのまま当てられる
fn fn_range(rel: &str, name: &str) -> std::ops::Range<usize> {
    let code = code_view::code_view(&production(rel));
    let needle = format!("fn {name}(");
    let head = code
        .match_indices(needle.as_str())
        .map(|(at, _)| at)
        // `fn foo(` が `fn xfn foo(` の一部に当たらないよう、直前が識別子の文字でないものだけ
        .find(|at| *at == 0 || !code.as_bytes()[at - 1].is_ascii_alphanumeric())
        .unwrap_or_else(|| {
            panic!("{rel}: `fn {name}` が見つからない（改名したならこの番犬も直す）")
        });
    let open = head + code[head..].find('{').expect("fn の本体の `{`");
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open..open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("{rel}: `fn {name}` の本体が閉じていない");
}

/// 関数本体（コメントだけ潰した眺め）と、その先頭行番号
fn fn_body(rel: &str, name: &str) -> (String, usize) {
    let view = non_comment(rel);
    let range = fn_range(rel, name);
    let start_line = line_of(&view, range.start);
    (view[range].to_string(), start_line)
}

/// `needle` が出る行を `rel:line` で並べる（`base_line` は本体の先頭行）
fn hits(body: &str, base_line: usize, rel: &str, needle: &str) -> Vec<String> {
    body.match_indices(needle)
        .map(|(at, _)| {
            let line = base_line + body[..at].matches('\n').count();
            format!("{rel}:{line}: `{needle}`")
        })
        .collect()
}

/// **1**: setup が「状態を見ない固定文」を自分で出していない。
///
/// 次に打つコマンド（`tako remote setup`）は状態ごとに `phone_readiness_line` が選ぶ。
/// setup.rs の本番コードにこの語が現れた = 状態と無関係に案内している
#[test]
fn setupは状態を見ない固定文を出さない() {
    let view = non_comment(SETUP);
    let mut offenders = hits(&view, 1, SETUP, "スマホからリモート接続するには");
    offenders.extend(hits(&view, 1, SETUP, "tako remote setup"));
    assert!(
        offenders.is_empty(),
        "setup の末尾がリモートの状態を見ずに固定の案内を出している:\n  {}\n\
         → `tako_control::remote_setup::setup_summary_lines()` の行をそのまま出してください。\
         状態（未導入 / 未ログイン / 公開済み …）と次に打つコマンドはそちらが決めます（#1507）",
        offenders.join("\n  ")
    );
}

/// **2**: 寄せ先を実際に呼んでいる（1 の禁止だけだと、行ごと消えても緑になる）
#[test]
fn setupの末尾は状態の1実装を呼ぶ() {
    let view = non_comment(SETUP);
    let calls = view.matches("remote_setup::setup_summary_lines()").count();
    assert_eq!(
        calls, 1,
        "{SETUP}: `remote_setup::setup_summary_lines()` の呼び出しが {calls} 件（期待 1）。\n\
         setup の末尾のリモート状態の行はこの 1 実装から出す（#1507）"
    );
}

/// **3**: 要約は `check_status` の結果だけから組む（setup 用の別口の問い合わせを作らない）
#[test]
fn 要約はcheck_statusの結果だけから組む() {
    let (body, base) = fn_body(REMOTE_SETUP, "setup_summary_lines");
    for needle in [
        "check_status()",
        "phone_readiness(&status)",
        "phone_readiness_line(",
    ] {
        assert!(
            body.contains(needle),
            "{REMOTE_SETUP}:{base}: `setup_summary_lines` が `{needle}` を通っていない。\n\
             状態は `check_status` の JSON から `phone_readiness` が決める（#1507）"
        );
    }
    let offenders = hits(&body, base, REMOTE_SETUP, "tailscale::");
    assert!(
        offenders.is_empty(),
        "setup の要約が tailscale を別口で問い合わせている:\n  {}\n\
         → `check_status()` の JSON だけを読んでください。別口で聞くと \
         `tako remote setup` / GUI のリモートパネルと答えが割れます（#1507）",
        offenders.join("\n  ")
    );
}

/// **4**: 未導入のときの次の一手は依存の導入口から引く（導入の案内を 2 か所で組まない）
#[test]
fn 未導入の次の一手は依存の導入口から引く() {
    let (step, base) = fn_body(REMOTE_SETUP, "tailscale_install_step");
    assert!(
        step.contains("setup_deps::next_step_line("),
        "{REMOTE_SETUP}:{base}: `tailscale_install_step` が `setup_deps::next_step_line` を\
         通っていない。未導入の案内は依存チェック段と同じ文面にする（#1499 / #1524 / #1507）"
    );
    let (line, base) = fn_body(REMOTE_SETUP, "phone_readiness_line");
    let offenders = hits(&line, base, REMOTE_SETUP, "setup deps");
    assert!(
        offenders.is_empty(),
        "1 行の組み立てが導入口のコマンドを自分で書いている:\n  {}\n\
         → 未導入のときの次の一手は引数 `install_step`（= `setup_deps::next_step_line`）から\
         受け取ってください（#1507）",
        offenders.join("\n  ")
    );
}

/// **5**: tailscale の検出（`--version`）の待ちに上限がある
#[test]
fn tailscaleの検出の待ちに上限がある() {
    let (body, base) = fn_body(TAILSCALE, "runnable");
    let mut offenders = hits(&body, base, TAILSCALE, ".status()");
    offenders.extend(hits(&body, base, TAILSCALE, ".output()"));
    assert!(
        offenders.is_empty(),
        "tailscale の検出が上限の無い待ちに戻っている:\n  {}\n\
         → `tako_core::probe::output_with_timeout` を通してください。素の待ちは相手が\
         固まると `tako setup` の末尾ごと固まります（#1503 / #1507）",
        offenders.join("\n  ")
    );
    assert!(
        body.contains("tako_core::probe::output_with_timeout("),
        "{TAILSCALE}:{base}: `runnable` が待ちの 1 実装（`tako_core::probe::output_with_timeout`）を\
         通っていない（#1503 / #1507）"
    );
}

/// **6**: `status --json` の打ち切りを「起動していない」へ畳まず、応答にも載せる
#[test]
fn 打ち切りを未起動へ畳まない() {
    let (status_on, base) = fn_body(TAILSCALE, "setup_status_on");
    assert!(
        status_on.contains("RunError::TimedOut(") && status_on.contains("timed_out"),
        "{TAILSCALE}:{base}: `setup_status_on` が打ち切りを `timed_out` へ残していない。\n\
         残さないと setup の末尾が打ち切りを「起動していません」と言い切る（#1507）"
    );
    let (check, base) = fn_body(REMOTE_SETUP, "check_status");
    for needle in ["\"timeouts\"", "status.timed_out", "RunError::TimedOut("] {
        assert!(
            check.contains(needle),
            "{REMOTE_SETUP}:{base}: `check_status` が `{needle}` を持っていない。\n\
             打ち切りは `timeouts` へ載せる（GUI / MCP からも読める = 設計原則 5。#1507）"
        );
    }
    let (readiness, base) = fn_body(REMOTE_SETUP, "phone_readiness");
    assert!(
        readiness.contains("status_timeouts(status)"),
        "{REMOTE_SETUP}:{base}: `phone_readiness` が打ち切りを見ていない（#1507）"
    );
}
