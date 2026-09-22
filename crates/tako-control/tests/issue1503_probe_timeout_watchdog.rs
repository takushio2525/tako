//! **#1503 の番犬**: setup の probe と setup 本体の待ちから**上限が消える**形へ
//! 戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! `tako setup` はエージェント CLI の状態を外部コマンドへ聞いて決めるが、
//! その問い合わせが全部 `std::process::Command::output()` で、**待ち時間の
//! 上限が無かった**。`claude mcp list` は登録済みの MCP サーバへ 1 台ずつ
//! 繋いで健全性を見るので、サーバが 1 つ無応答だと返らなくなる。
//! #1500 の棚卸し R4 では `tako setup` が**無言で 6 分固まり続けた**。
//! dispatch（GUI / MCP 経路）は `wait_with_output()` で子を待つので、
//! そちらはボタンが永久に回った。
//!
//! ## ここで止める 5 つ
//!
//! 1. [`setupのprobeに素のoutputが残っていない`] — 上限を持たない待ちの再発。
//!    **1 行書き戻すだけで症状が戻る**のがこの Issue の性質
//! 2. [`command_outputが上限つきの1実装を通している`] — 禁止だけでなく
//!    **寄せ先も縛る**（#1496 の 2 本立て）。`command_output` が素通しになると
//!    1 は緑のまま症状だけ戻る
//! 3. [`mcpの健全性確認もcommand_outputを通る`] — R4 の実物。ここだけ自前で
//!    `Command` を組むと probe が 2 系統になる
//! 4. [`dispatchのsetup待ちに上限がある`] — GUI / MCP 経路の蓋
//! 5. [`上限を外す指定を作らない`] — env で「無制限」を許すと #1503 が
//!    設定 1 つで戻る。A/B のゲートも 1 か所だけ
//! 6. [`読み切りにも予算が掛かっている`] — 子の終了後に `read_to_end` で待つ形は
//!    **孫がパイプを持っていると返らない**（`Command::output()` の穴。同時に塞いだ）
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 実経路（隔離 HOME + 無応答スタブ claude で `tako setup --yes` が上限で
//! 次へ進み完走すること・A/B・dispatch 経路）は
//! `scripts/test-setup-probe-timeout-1503.sh`。待ちそのものの挙動
//! （打ち切り・部分出力・env の解釈・知らせのラウンドトリップ）は
//! `tako_core::probe` の単体テスト。ここは**配線が外れていないこと**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

const SETUP: &str = "crates/tako-cli/src/setup.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const PROBE: &str = "crates/tako-core/src/probe.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// コメントでない行だけ（`//` で始まる行は対象外）
fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with("//")
}

/// 違反行を「file:line（囲んでいる関数名）」で名指しする。
/// 関数の頭の判定は `source_scan` の 1 実装（#1496。`pub fn` / `async fn` を落とさない）
fn offenders(rel: &str, text: &str, hit: impl Fn(&str) -> bool) -> Vec<String> {
    let mut current = "(トップレベル)".to_string();
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if is_top_level_fn_head(line) {
            if let Some(name) = fn_head_name(line) {
                current = name.to_string();
            }
        }
        if is_code(line) && hit(line) {
            out.push(format!("{rel}:{}（fn {current}）", index + 1));
        }
    }
    out
}

/// トップレベル関数 1 本の本体（行番号つき）
fn fn_body(rel: &str, text: &str, name: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for (index, line) in text.lines().enumerate() {
        if is_top_level_fn_head(line) {
            inside = fn_head_name(line) == Some(name);
            continue;
        }
        if inside {
            out.push((index + 1, line.to_string()));
        }
    }
    assert!(
        !out.is_empty(),
        "{rel} に `fn {name}` が無い（改名したなら番犬も追うこと）"
    );
    out
}

/// 1: 上限を持たない待ちの再発。`setup.rs` の probe は全部 `command_output` を通す
#[test]
fn setupのprobeに素のoutputが残っていない() {
    let text = production(SETUP);
    let hits = offenders(SETUP, &text, |line| {
        line.contains(".output()") || line.contains(".wait_with_output()")
    });
    assert!(
        hits.is_empty(),
        "上限を持たない待ちが復活している:\n  {}\n\n\
         `tako setup` の外部コマンド呼び出しは `command_output` を通すこと（#1503）。\n\
         `.output()` は待ち時間の上限を持たないので、無応答の claude で setup が\n\
         **無言で固まる**（#1500 の R4 = 実測 6 分）",
        hits.join("\n  ")
    );
}

/// 2: 寄せ先も縛る。`command_output` が素通しになると 1 は緑のまま症状が戻る
#[test]
fn command_outputが上限つきの1実装を通している() {
    let text = production(SETUP);
    let body = fn_body(SETUP, &text, "command_output");
    for needle in [
        "tako_core::probe::output_with_timeout",
        "tako_core::probe::probe_timeout()",
        "timeout_notice()",
    ] {
        assert!(
            body.iter()
                .any(|(_, line)| is_code(line) && line.contains(needle)),
            "{SETUP} の `fn command_output` が `{needle}` を使っていない（#1503）。\n\
             上限は `tako_core::probe` の 1 実装が持ち、打ち切ったら**必ず知らせる**\n\
             （無言で諦めると症状が「固まる」から「黙って誤判定する」へ移るだけ）"
        );
    }
}

/// 3: R4 の実物。`claude mcp list` が自前の `Command` へ戻ると probe が 2 系統になる
#[test]
fn mcpの健全性確認もcommand_outputを通る() {
    let text = production(SETUP);
    let body = fn_body(SETUP, &text, "check_claude_mcp_health");
    let hits: Vec<usize> = body
        .iter()
        .filter(|(_, line)| is_code(line) && line.contains("Command::new"))
        .map(|(line_no, _)| *line_no)
        .collect();
    assert!(
        hits.is_empty(),
        "{SETUP}:{hits:?} `check_claude_mcp_health` が自前で `Command` を組んでいる（#1503）。\n\
         `claude mcp list` は無応答の MCP サーバ 1 台で返らなくなる = R4 の実物なので、\n\
         必ず上限つきの `command_output` を通すこと"
    );
    assert!(
        body.iter()
            .any(|(_, line)| is_code(line) && line.contains("command_output(")),
        "{SETUP} の `check_claude_mcp_health` が `command_output` を呼んでいない（#1503）"
    );
}

/// 4: GUI / MCP 経路の蓋。ここが外れるとボタンが永久に回る
#[test]
fn dispatchのsetup待ちに上限がある() {
    let text = production(DISPATCH);
    // 予算の決定（`run_setup_cli`）と待ちの本体（`run_setup_cli_within`）で 2 本
    assert!(
        fn_body(DISPATCH, &text, "run_setup_cli")
            .iter()
            .any(|(_, line)| is_code(line)
                && line.contains("tako_core::probe::setup_run_timeout()")),
        "{DISPATCH} の `fn run_setup_cli` が既定の上限（`setup_run_timeout()`）を渡していない（#1503）"
    );
    let body = fn_body(DISPATCH, &text, "run_setup_cli_within");
    let hits: Vec<usize> = body
        .iter()
        .filter(|(_, line)| is_code(line) && line.contains("wait_with_output()"))
        .map(|(line_no, _)| *line_no)
        .collect();
    assert!(
        hits.is_empty(),
        "{DISPATCH}:{hits:?} `run_setup_cli` が `wait_with_output()` で待ちっぱなしに\n\
         戻っている（#1503）。dispatch は GUI / MCP の経路なので、ここが無限だと\n\
         セットアップのボタンが永久に回る"
    );
    for needle in [
        "tako_core::probe::wait_with_timeout",
        "tako_core::probe::parse_notices",
        "\"timed_out\"",
        "\"probe_timeouts\"",
    ] {
        assert!(
            body.iter()
                .any(|(_, line)| is_code(line) && line.contains(needle)),
            "{DISPATCH} の `fn run_setup_cli_within` に `{needle}` が無い（#1503）。\n\
             待ちの上限と**打ち切りの事実**は応答に載せること（開発不変条件:\n\
             UI で分かることは MCP / CLI からも分かる）"
        );
    }
}

/// 5: 上限を外す指定を作らない。A/B のゲートも 1 か所だけ
#[test]
fn 上限を外す指定を作らない() {
    let text = production(PROBE);
    // 「0 = 無制限」を作らないことは `parse_timeout_secs` の単体テストが見る。
    // ここは **env ゲートが増えていないこと**を見る（増えると逃げ道が製品経路へ漏れる）
    let gates = offenders(PROBE, &text, |line| line.contains("TAKO_1503_LEGACY"));
    assert_eq!(
        gates.len(),
        1,
        "#1503 の A/B ゲートは `fn legacy_unbounded` の 1 か所だけ。実際は:\n  {}",
        gates.join("\n  ")
    );
    // 製品コードの他クレートへ漏れていないこと
    for rel in [SETUP, DISPATCH] {
        let hits = offenders(rel, &production(rel), |line| {
            line.contains("TAKO_1503_LEGACY")
        });
        assert!(
            hits.is_empty(),
            "A/B のゲートが `{PROBE}` の外へ出ている:\n  {}",
            hits.join("\n  ")
        );
    }
    assert!(
        production(PROBE).contains("pub const DEFAULT_PROBE_TIMEOUT"),
        "{PROBE} の既定上限の名前が変わった（呼び手と番犬も追うこと）"
    );
}

/// 6: 読み切りにも予算が掛かっている。
///
/// 子が終わっても、子が残した孫がパイプの書き手として居ると `read_to_end` は返らない
/// （`Command::output()` の穴）。上限を「終了待ち」だけに掛けると、**終了後の
/// 読み切りで無限に待つ**形が残る。注入で消しても単体テストは確率的にしか落ちない
/// （速い子だと吸い出しが先に終わる）ので、配線をここで見る
#[test]
fn 読み切りにも予算が掛かっている() {
    let text = production(PROBE);
    let body = fn_body(PROBE, &text, "wait_with_timeout");
    let grace: Vec<usize> = body
        .iter()
        .filter(|(_, line)| {
            is_code(line) && line.contains("out.finished()") && line.contains("err.finished()")
        })
        .map(|(line_no, _)| *line_no)
        .collect();
    assert_eq!(
        grace.len(),
        1,
        "{PROBE} の `fn wait_with_timeout` から**終了後の読み切りに予算を掛ける待ち**が\n\
         消えている（#1503）。`out.finished() && err.finished()` を予算内で待つ形が\n\
         1 か所だけ在ること。実際に見つかった行: {grace:?}"
    );
    for needle in ["fn drain", "join()"] {
        let hits = offenders(PROBE, &text, |line| line.contains(needle));
        match needle {
            // 吸い出し係そのものが消えたら、パイプが詰まる子で上限が効かなくなる
            "fn drain" => assert!(
                !hits.is_empty(),
                "{PROBE} から吸い出し係が消えている（#1503）"
            ),
            // join すると「孫が書き手として残る」形でそこから返らなくなる
            _ => assert!(
                hits.is_empty(),
                "{PROBE} が吸い出しスレッドを join している:\n  {}\n\
                 孫がパイプを持つと `read` が返らないので、join した時点で上限が無意味になる",
                hits.join("\n  ")
            ),
        }
    }
}
