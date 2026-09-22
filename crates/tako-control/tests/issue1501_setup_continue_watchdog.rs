//! **#1501 の番犬**: `tako setup` が詰まった段で **exit 1 して残りの段を捨てる**形へ
//! 戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! `run_setup` は 3 か所で自分で `Err` を作って早期 return していた:
//!
//! - **Z2** ゼロスタート導入の `run_bootstrap_stage(assume_yes)?` — 認証段
//!   （bootstrap [3/3]）はブラウザ操作が要るので tako が代行できない（#1129）。
//!   そこが `Err` なので、新品の Mac では `tako setup` が
//!   `error: Claude アカウントへのログインが必要です` で **exit 1**
//! - **Z3** 選んだ系統が未認証 → `Err("… は未認証です")`
//! - **Z4** `missing_required` が非空 → `Err("必須の依存ツールが不足しています")`
//!
//! どれも**一番手前の段**なので、以降の段（依存導入 / MCP 登録 / 指示ファイル /
//! プロファイル / テンプレ / シェル統合 / PATH 設置）が **1 つも走らない**。
//! 棚卸し（#1500 の R1 / R2 / R3）では 3 通りすべてで
//! `profiles/default.yaml`・`~/.claude/CLAUDE.md`・テンプレ・MCP 登録が
//! **全部未作成**のまま終わっていた = 「setup を走らせても何も整わない」。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`run_setupは自分でerrを作って早期returnしない`] — 形そのものの再発
//! 2. [`詰まる段は止められる型を返さない`] — 段の戻り値が `Result` へ戻ると
//!    呼び出し側の `?` 1 文字で全段が飛ぶ
//! 3. [`run_setupは残りをまとめて表示する`] — 残りの集約・表示が消える
//! 4. [`run_checkも同じ1実装で残りを出す`] — CLI / MCP の 1:1（開発不変条件）
//! 5. [`残りの文面はsetup_remainingの1実装だけが持つ`] — CLI 側の直書き
//! 6. [`止める判断はenvゲートの1か所だけ`] — A/B の逃げ道が製品経路へ漏れる
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 実経路（隔離 HOME で未認証 / 未導入の `tako setup` を 3 通り + `--check` +
//! dispatch 同条件 + A/B）は `scripts/test-setup-continue-1501.sh`。判断そのもの
//! （同じ道の畳み込み・並び順・最簡コマンド）は `tako_control::setup_remaining` の
//! 単体テスト。ここは**配線が外れていないこと**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const SETUP: &str = "crates/tako-cli/src/setup.rs";
const REMAINING: &str = "crates/tako-control/src/setup_remaining.rs";
/// `tako setup --check` の項目の正本（#1505 で CLI から移った）
const DIAGNOSTICS: &str = "crates/tako-control/src/diagnostics.rs";

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

/// トップレベル関数 1 本の本体（行番号つき）。関数の頭の判定は `source_scan` の
/// 1 実装（#1496。`pub fn` / `async fn` を取りこぼさない）
fn fn_body(text: &str, name: &str) -> Vec<(usize, String)> {
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
        "{SETUP} に `fn {name}` が無い（改名したなら番犬も追うこと）"
    );
    out
}

/// コメントでない行だけ（文言の検査では潰さない = 原文のまま見る）
fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with("//")
}

/// 1: 早期 return の形そのものを禁じる（Z2 / Z3 / Z4 が戻る唯一の書き方）
#[test]
fn run_setupは自分でerrを作って早期returnしない() {
    let text = production(SETUP);
    let hits: Vec<usize> = fn_body(&text, "run_setup")
        .into_iter()
        .filter(|(_, line)| is_code(line) && line.contains("return Err("))
        .map(|(line_no, _)| line_no)
        .collect();
    assert!(
        hits.is_empty(),
        "{SETUP}:{hits:?} で `run_setup` が自分で Err を作って早期 return している。\n\
         詰まった段は `Remaining` を積んで続けること（#1501。ここで返すと\n\
         依存 / MCP / 指示ファイル / プロファイル / テンプレが 1 つも走らない）。\n\
         止めてよいのは設定の破損・書き出しの失敗のような `?` 伝播だけ"
    );
}

/// 2: 詰まる段の戻り値。`Result` へ戻すと呼び出し側の `?` 1 文字で全段が飛ぶ
#[test]
fn 詰まる段は止められる型を返さない() {
    let text = production(SETUP);
    // (関数名, 戻り値に在るべき型, 在ってはいけない型)
    let expectations = [
        ("run_bootstrap_stage", "-> BootstrapOutcome", "Result"),
        ("run_bootstrap_for", "-> Vec<Remaining>", "Result"),
        ("configure_agent_mcp", "-> Vec<Remaining>", "Result"),
        (
            "run_dependency_check",
            "(Vec<DetectedAgent>, Vec<Remaining>)",
            "Vec<String>",
        ),
    ];
    for (name, want, forbid) in expectations {
        let head = text
            .lines()
            .enumerate()
            .filter(|(_, line)| is_top_level_fn_head(line))
            .find(|(_, line)| fn_head_name(line) == Some(name))
            .map(|(index, line)| (index + 1, line.to_string()));
        let (line_no, head) = head
            .unwrap_or_else(|| panic!("{SETUP} に `fn {name}` が無い（改名したなら番犬も追う）"));
        // 戻り値が次の行へ折り返される形も拾う（rustfmt は長い頭を折る）
        let window: String = text
            .lines()
            .skip(line_no.saturating_sub(1))
            .take(8)
            .collect::<Vec<_>>()
            .join("\n");
        let signature = format!("{head}\n{window}");
        assert!(
            signature.contains(want),
            "{SETUP}:{line_no} `fn {name}` の戻り値に `{want}` が無い。\n\
             詰まりは戻り値で運ぶこと（#1501。`Result` へ戻すと呼び出し側の `?` で\n\
             setup 全体が止まり、残りの段が捨てられる）"
        );
        assert!(
            !signature
                .lines()
                .take_while(|line| !line.contains('{'))
                .any(|line| line.contains(forbid)),
            "{SETUP}:{line_no} `fn {name}` が `{forbid}` を返す形へ戻っている（#1501）"
        );
    }
}

/// 3: 残りの集約と表示（消えると「何が残っているか」が誰にも分からなくなる）
#[test]
fn run_setupは残りをまとめて表示する() {
    let text = production(SETUP);
    let body = fn_body(&text, "run_setup");
    for needle in [
        "setup_remaining::summarize",
        "setup_remaining::render",
        "setup_remaining::legacy_stop",
    ] {
        assert!(
            body.iter()
                .any(|(_, line)| is_code(line) && line.contains(needle)),
            "{SETUP} の `run_setup` が `{needle}` を呼んでいない（#1501）。\n\
             残りの集約・表示・A/B はこの 3 本を通すこと"
        );
    }
}

/// 4: `--check` も同じ結末（UI でできることは CLI / MCP でもできる = 開発不変条件）
#[test]
fn run_checkも同じ1実装で残りを出す() {
    let text = production(SETUP);
    let body = fn_body(&text, "run_check");
    assert!(
        body.iter()
            .any(|(_, line)| is_code(line) && line.contains("setup_remaining::render")),
        "{SETUP} の `run_check` が `setup_remaining::render` を呼んでいない（#1501）。\n\
         `tako setup` と `tako setup --check` は同じ残りを同じ文面で出すこと"
    );
    // 判定の写しを CLI 側へ持たない（段 → 残り作業の対応は setup_remaining が正本）。
    // **#1505 で `--check` の項目そのものが診断の正本へ移った**ので、写しを通す場所も
    // そちら（`diagnostics::shortest_bootstrap_remaining`）。縛る中身は同じ
    let diagnostics = production(DIAGNOSTICS);
    assert!(
        diagnostics.contains("RemainingKind::for_step"),
        "{DIAGNOSTICS} が `RemainingKind::for_step` を通っていない（#1501 / #1505）。\n\
         段から残り作業への写しは 1 実装（別に書くと `setup` と `--check` で名前がずれる）"
    );
}

/// 5: 文面の正本は 1 か所（CLI 側に直書きすると `--check` と食い違う）
#[test]
fn 残りの文面はsetup_remainingの1実装だけが持つ() {
    let heading = "残り {} 件";
    let remaining = production(REMAINING);
    assert!(
        remaining.contains(heading),
        "{REMAINING} が残りの見出しを組んでいない（#1501。文面の正本はここ）"
    );
    let setup = production(SETUP);
    let hits: Vec<usize> = setup
        .lines()
        .enumerate()
        .filter(|(_, line)| is_code(line) && line.contains("（ここから先は人の操作が必要です）"))
        .map(|(index, _)| index + 1)
        .collect();
    assert!(
        hits.is_empty(),
        "{SETUP}:{hits:?} が残りの見出しを直書きしている（#1501）。\n\
         `setup_remaining::render` の 1 実装を通すこと"
    );
}

/// 6: 「止める」判断は A/B の env ゲート 1 か所だけ（製品経路へ漏らさない）
#[test]
fn 止める判断はenvゲートの1か所だけ() {
    let remaining = production(REMAINING);
    let gate = remaining
        .lines()
        .enumerate()
        .find(|(_, line)| is_code(line) && line.contains("TAKO_1501_LEGACY"))
        .map(|(index, _)| index + 1);
    let gate = gate.unwrap_or_else(|| {
        panic!("{REMAINING} に A/B の env ゲート（TAKO_1501_LEGACY）が無い（#1501）")
    });
    // ゲートを読むのは `legacy_mode` 1 本で、止める側はそれを通る
    assert!(
        remaining.contains("pub fn legacy_mode()") && remaining.contains("if !legacy_mode()"),
        "{REMAINING}:{gate} の A/B が `legacy_mode()` 経由でなくなっている（#1501）"
    );
    let setup = production(SETUP);
    let leaks: Vec<usize> = setup
        .lines()
        .enumerate()
        .filter(|(_, line)| is_code(line) && line.contains("TAKO_1501_LEGACY"))
        .map(|(index, _)| index + 1)
        .collect();
    assert!(
        leaks.is_empty(),
        "{SETUP}:{leaks:?} が A/B の env を直に読んでいる（#1501）。\n\
         判断は `setup_remaining::legacy_stop` の 1 か所に置くこと"
    );
}

/// 実経路テストが居ること（#1501 の受け入れ検査はスクリプト側が持つ）
#[test]
fn 実経路テストが登録されている() {
    let script = repo_root().join("scripts/test-setup-continue-1501.sh");
    assert!(
        script.is_file(),
        "scripts/test-setup-continue-1501.sh が無い（#1501 の実測がここに居る）"
    );
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml を読める");
    assert!(
        ci.contains("scripts/test-setup-continue-1501.sh"),
        ".github/workflows/ci.yml に test-setup-continue-1501.sh の登録が無い（#1501）"
    );
}
