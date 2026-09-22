//! **#1505 の番犬**: 環境診断の正本が 2 つへ割れる形へ戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! `tako setup --check`（tako-cli）と `tako check-health`（dispatch）が、同じ
//! 「この環境で tako は使えるか」を**別々の実装で**答えていた。`--check` には
//! シェル統合・tako CLI の PATH・更新・remote の行が 1 つも無く、PATH は
//! `check-health` だけが別口で見ていた（#1500 の棚卸し Z19 / R8 の実測）。
//! 片方へ項目を足しても、もう片方は黙って古いままになる形だった。
//!
//! ## ここで止める 5 つ
//!
//! 1. [`run_checkは正本から組む`] — `--check` が `diagnostics::collect` 以外から
//!    項目を組む形（`[OK]` の直書き・判定関数の直呼び）を file:line で落とす。
//!    **1 行書き足すだけで症状が戻る**のがこの Issue の性質
//! 2. [`check_healthも同じ正本を通す`] — 応答へ `diagnostics` を載せていること。
//!    片方だけ寄せても「答えが 2 種類」は直らない
//! 3. [`tmuxの判定を二度書かない`] — 寄せ先も縛る（#1496 の 2 本立て）。
//!    `check_health` が `which("tmux")` を持ち直すと 1 は緑のまま症状だけ戻る
//! 4. [`表示とjsonは同じ項目から出る`] — 実際に組んで、行・キー・残りが
//!    1 つの `Report` から出ていることを確かめる（runtime）
//! 5. [`重い診断はuiスレッドを離れる`] — `CheckHealth` が offload に載っていること。
//!    正本はエージェント CLI へ問い合わせる（実測 10.8 秒）ので、UI スレッドで
//!    走らせると窓が止まる
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 4 経路（`--check` / `check-health` のローカル・dispatch・MCP）が同じ状態へ
//! 同じ答えを出すことの実測は `scripts/test-setup-check-single-source-1505.sh`。
//! ここは**配線が外れていないこと**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

// 肯定の存在確認が見る眺め（#1609）。全文へ `contains` すると走査先の doc コメントに
// 書いた同じ綴りで真になり、実体が消えても緑のまま残る。
// **`production_range` が抱えている方を使う**（同じファイルを 2 度 `mod` すると
// `clippy::duplicate_mod` で落ちる）
use production_range::code_view;

const SETUP: &str = "crates/tako-cli/src/setup.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";

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

/// **肯定の存在確認**（「この呼び出しが在る」）が見る眺め = コメントを落とした本文。
///
/// バイト長と行番号が保たれるので、`fn_body` で切った区間をそのまま使える（#1609）
fn production_code(rel: &str) -> String {
    code_view::without_comments_checked(&production(rel), rel)
}

/// コメントでない行だけ（`//` で始まる行は対象外）
fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with("//")
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
        "{rel} に `fn {name}` が無い（改名・移設したなら番犬も追うこと）"
    );
    out
}

/// 本体の中で条件に当たる行を「file:line」で名指しする
fn offenders_in(rel: &str, body: &[(usize, String)], hit: impl Fn(&str) -> bool) -> Vec<String> {
    body.iter()
        .filter(|(_, line)| is_code(line) && hit(line))
        .map(|(number, line)| format!("{rel}:{number}  {}", line.trim()))
        .collect()
}

/// 1: `--check` は正本から組む（判定も文面もここに持たない）
#[test]
fn run_checkは正本から組む() {
    let text = production(SETUP);
    let code = production_code(SETUP);
    let body = fn_body(SETUP, &text, "run_check");
    // 肯定の存在確認はコメントを落とした眺めで（#1609）
    let joined: String = fn_body(SETUP, &code, "run_check")
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("diagnostics::collect()"),
        "{SETUP} の `run_check` が `tako_control::diagnostics::collect()` を通っていない（#1505）"
    );

    // 表示の重さラベルを直書きしない（正本が組んだ行をそのまま出すだけにする）
    let labels = ["[OK]", "[不足]", "[警告]", "[情報]", "[任意]", "[検出]"];
    let hits = offenders_in(SETUP, &body, |line| {
        labels.iter().any(|label| line.contains(label))
    });
    assert!(
        hits.is_empty(),
        "`--check` が項目を自前で組んでいる:\n  {}\n\n\
         診断の行は `tako_control::diagnostics` の 1 実装が持つこと（#1505）。\n\
         ここへ 1 行足すと `tako check-health` には出ない項目が生まれ、\n\
         「UI / CLI / MCP で同じ答え」（設計原則 5）が黙って破れる。",
        hits.join("\n  ")
    );

    // 判定関数の直呼びも禁じる（表示だけ寄せても判断が 2 つなら同じこと）
    let judges = [
        "status_all(",
        "setup_deps::status(",
        "agent_mcp::state(",
        "claude_health",
        "tako_cli_path_check_line",
        "shell_integration::check_line",
        "agents_sync::status(",
        "list_profiles(",
        "config_share_lines(",
        "fda::is_granted(",
    ];
    let hits = offenders_in(SETUP, &body, |line| {
        judges.iter().any(|needle| line.contains(needle))
    });
    assert!(
        hits.is_empty(),
        "`--check` が判定を直に呼んでいる:\n  {}\n\n\
         判定は `tako_control::diagnostics::collect` の中だけで行うこと（#1505）",
        hits.join("\n  ")
    );
}

/// 2: `check_health` も同じ正本を通し、応答へ `diagnostics` を載せる
#[test]
fn check_healthも同じ正本を通す() {
    let code = production_code(DISPATCH);
    let joined: String = fn_body(DISPATCH, &code, "finish_check_health")
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("diagnostics::collect()"),
        "{DISPATCH} の `finish_check_health` が正本を通っていない（#1505）"
    );
    assert!(
        joined.contains("\"diagnostics\""),
        "`check_health` の応答に `diagnostics` 節が無い（#1505。\
         CLI / MCP から項目を読めない = `--check` と照合できない）"
    );
    // MCP も同じ dispatch を通る（ここが外れると MCP だけ別の答えになる）
    let mcp = production_code(MCP_REQUEST);
    assert!(
        mcp.contains("\"tako_check_health\" => Request::CheckHealth"),
        "MCP の `tako_check_health` が `Request::CheckHealth` へ写っていない（#1441 / #1505）"
    );
}

/// 3: tmux の有無を 2 か所で判定しない（寄せ先も縛る）
#[test]
fn tmuxの判定を二度書かない() {
    let text = production(DISPATCH);
    let body = fn_body(DISPATCH, &text, "finish_check_health");
    let hits = offenders_in(DISPATCH, &body, |line| line.contains("which(\"tmux\")"));
    assert!(
        hits.is_empty(),
        "`check_health` が tmux の有無を自前で判定している:\n  {}\n\n\
         判定は診断項目の正本（`diagnostics` の `dep.<器>`）から引くこと（#1505）。\n\
         2 か所で判定すると `tako setup --check` と答えが割れる。",
        hits.join("\n  ")
    );
}

/// 4: 表示（`--check`）と `--json`（`check-health`）が同じ項目から出る
#[test]
fn 表示とjsonは同じ項目から出る() {
    let report = tako_control::diagnostics::collect();
    assert!(
        report.items.len() >= 10,
        "診断の項目が少なすぎる（{} 件）。collect が途中で落ちている",
        report.items.len()
    );

    let value = report.to_json();
    let keys: Vec<String> = value["keys"]
        .as_array()
        .expect("keys 配列")
        .iter()
        .map(|k| k.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(keys, report.keys(), "`--json` のキーが表示と揃っていない");

    // キーは重複しない（機械照合の前提）
    let mut sorted = keys.clone();
    sorted.sort();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(before, sorted.len(), "診断のキーが重複している: {keys:?}");

    // `--json` の行はすべて `--check` が出す行の中にある（字面まで同じ）
    let lines = report.lines();
    for item in value["items"].as_array().expect("items 配列") {
        for line in item["lines"].as_array().expect("lines 配列") {
            let line = line.as_str().unwrap_or_default().to_string();
            assert!(
                lines.contains(&line),
                "`--json` にしか無い行がある: {line:?}（表示と JSON の出どころが割れている）"
            );
        }
    }

    // 状態によらず必ず出る項目（#1505 の受け入れ条件。Z19 で `--check` に無かった
    // シェル統合 / tako CLI の PATH / 更新 / remote を含む）
    let mut required = vec![
        "agent_cli",
        "agents_detected",
        "tako_cli_path",
        "shell_integration",
        "dep.git",
        "dep.tailscale",
        "setup",
        "update",
        "agents_sync",
        "sleep_guard",
        "config_share",
        "profiles",
        "remote",
        "ipc",
    ];
    // 器の名前は OS で変わる（macOS = tmux / Windows = psmux）ので表から引く
    let container = format!(
        "dep.{}",
        tako_control::setup_deps::current_deps()
            .first()
            .expect("器の依存")
            .bin
    );
    required.push(&container);
    for key in required {
        assert!(
            keys.iter().any(|k| k == key),
            "診断の項目 {key} が無い（#1505）。いまの項目: {keys:?}"
        );
    }
}

/// 5: 重い診断は UI スレッドを離れる（GUI が 14 秒止まらない）
#[test]
fn 重い診断はuiスレッドを離れる() {
    let text = production(DISPATCH);
    let code = production_code(DISPATCH);
    let joined: String = fn_body(DISPATCH, &code, "prepare_offload")
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("Request::CheckHealth"),
        "{DISPATCH} の `prepare_offload` が `CheckHealth` を background へ出していない（#1505）。\n\
         正本はエージェント CLI へ問い合わせる（実測 10.8 秒）ので、\n\
         UI スレッドで走らせると `tako_check_health` のたびに窓が止まる。"
    );
    // UI スレッドで採るのは workspace の要約とスレッド依存の実測だけ
    // （ここでプロセスを起こさない）
    let ctx = fn_body(DISPATCH, &text, "collect_check_health_ctx");
    let hits = offenders_in(DISPATCH, &ctx, |line| {
        line.contains("diagnostics::collect") || line.contains("Command::new")
    });
    assert!(
        hits.is_empty(),
        "UI スレッド側の文脈収集が重い処理を抱えている:\n  {}\n\n\
         `collect_check_health_ctx` は workspace とスレッド依存の実測だけにすること（#1505）",
        hits.join("\n  ")
    );
    // DPI 認識レベルは**スレッドごと**の問い合わせ（Windows の
    // `GetThreadDpiAwarenessContext`）なので、background で測ると #1063 の申告が狂う
    let ctx_text: String = fn_body(DISPATCH, &code, "collect_check_health_ctx")
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ctx_text.contains("dpi::process_awareness()"),
        "{DISPATCH}: DPI 認識レベルを UI スレッドで採っていない（#1063 / #1505）。\n\
         `GetThreadDpiAwarenessContext` はスレッドごとの問い合わせなので、\n\
         background executor で測ると申告が実際の窓と食い違いうる。"
    );
    let finish = fn_body(DISPATCH, &text, "finish_check_health");
    let hits = offenders_in(DISPATCH, &finish, |line| {
        line.contains("dpi::process_awareness()") || line.contains("degraded_note_here()")
    });
    assert!(
        hits.is_empty(),
        "background 側で DPI を測り直している:\n  {}\n\n\
         測るのは UI スレッド 1 か所（`CheckHealthCtx`）にすること（#1063 / #1505）",
        hits.join("\n  ")
    );
}
