//! **#1816 の回帰テストと番犬**: tailscale の検出の続き（#1797 の後）。
//!
//! ## 何が起きていたのか
//!
//! #1797 で検出を `tailscale::detect_tailscale` の 1 本（動く / 在るが動かない / 無い）へ寄せたが、
//!
//! - GUI のリモートパネルと MCP `tako_remote_setup` は「在るが動かない」を **「未導入」** と出していた
//!   （`check_status` の JSON が `ok` / `missing` の 2 通りしか持たなかった）
//! - `tako setup` は上限 10 秒の検出を **3 回** 走らせていた（依存段 1 回 + 末尾の `check_status` と
//!   導入口の判定で 2 回）。`--version` が固まる CLI で setup が約 33 秒に延びた
//! - 依存段・`tako setup deps`・`tako setup --check` は「在るが動かない」も **`[OK]`** と出していた
//! - 検出はプロセスの PATH と固定の候補しか見ず、Dock から起こした `.app` の痩せた PATH では
//!   MacPorts / nix の tailscale を「未導入」と判定し、`--yes` では brew 版まで重ねて入れていた
//!
//! ## ここで止めるもの
//!
//! 1. [`setupの末尾は依存段の検出を引き直さない`] — 振る舞い（`--version` の起動回数を数える）
//! 2. [`末尾の1行は依存段の答えを受け取る`] — `run_setup` → `setup_summary_lines` の受け渡し
//! 3. [`check_statusのtailscale項目は検出の答えをそのまま出す`] — 2 通りへ戻らない
//! 4. [`依存段の表示は在るが動かないをokより先に言い分ける`] — 3 か所の表示
//! 5. [`検出はログインシェルのpathを上限つきで最後に引く`] — ④ の逃げ道が消えない・上限を失わない
//! 6. [`remote_setupの検出の段は1回だけ引く`] — [1/5] が 2 度待たない
//!
//! 2〜6 は落ちるとき **file:line で名指し**する。実経路（隔離 HOME + tailscale / brew / ログインシェルの
//! スタブで `tako setup` 等を走らせる）は `scripts/test-tailscale-detect-1816.sh`。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const TAILSCALE: &str = "crates/tako-control/src/tailscale.rs";
const REMOTE_SETUP: &str = "crates/tako-control/src/remote_setup.rs";
const DIAGNOSTICS: &str = "crates/tako-control/src/diagnostics.rs";
const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";

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

/// コメントだけを潰した眺め（文字列リテラルは残る）
fn non_comment(rel: &str) -> String {
    code_view::without_comments_checked(&production(rel), rel)
}

/// 関数 `name` の本体（コメントだけ潰した眺め）と、その先頭行番号。
/// 波括弧は**コメントと文字列を潰した眺め**で数える（`format!("{x}")` の括弧を数えない）
fn fn_body(rel: &str, name: &str) -> (String, usize) {
    let code = code_view::code_view(&production(rel));
    let needle = format!("fn {name}(");
    let head = code
        .match_indices(needle.as_str())
        .map(|(at, _)| at)
        .find(|at| *at == 0 || !code.as_bytes()[at - 1].is_ascii_alphanumeric())
        .unwrap_or_else(|| {
            panic!("{rel}: `fn {name}` が見つからない（改名したならこの番犬も直す）")
        });
    let open = head + code[head..].find('{').expect("fn の本体の `{`");
    let mut depth = 0usize;
    let mut close = None;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close.unwrap_or_else(|| panic!("{rel}: `fn {name}` の本体が閉じていない"));
    let view = non_comment(rel);
    let start_line = view[..open].matches('\n').count() + 1;
    (view[open..close].to_string(), start_line)
}

/// `body` の中の `at` バイト目の行番号
fn line_at(body: &str, base_line: usize, at: usize) -> usize {
    base_line + body[..at].matches('\n').count()
}

/// `needle` が出る位置（無ければ panic で名指し）
fn find_in(body: &str, base: usize, rel: &str, name: &str, needle: &str, why: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("{rel}:{base}: `fn {name}` に `{needle}` が無い。\n→ {why}"))
}

/// **1**: `tako setup` の末尾の 1 行（`setup_summary_lines`）は依存段の検出結果を使い回し、
/// tailscale の `--version` を**もう 1 度も起こさない**（#1816）。
///
/// スタブは `--version` を非 0 で返す（在るが動かない）。#1816 前は末尾が `check_status`
/// （検出 1 回）と導入口の判定（`status_of` = 検出 1 回）で 2 回起こしていた。
/// `TAKO_TAILSCALE_BIN` を一時 dir の中へ向けるので、実機の候補は 1 度も試されない
#[cfg(unix)]
#[test]
fn setupの末尾は依存段の検出を引き直さない() {
    // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(setupの末尾は依存段の検出を引き直さない) {
        return;
    }
    let dir = tako_core::test_residue::ScratchDir::new("tako-1816-reuse");
    let log = dir.path().join("calls.log");
    let cli = dir.path().join("tailscale");
    tako_core::fake_exec::write_script(
        &cli,
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho broken >&2\nexit 1\n",
            log.display()
        ),
    );
    let calls = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .filter(|l| *l == "--version")
            .count()
    };
    std::env::set_var("TAKO_TAILSCALE_BIN", &cli);
    let dep = tako_control::setup_deps::status_of("tailscale").expect("依存表に tailscale");
    let after_dep_stage = calls();
    let lines = tako_control::remote_setup::setup_summary_lines(Some(&dep));
    let after_summary = calls();
    std::env::remove_var("TAKO_TAILSCALE_BIN");

    assert_eq!(after_dep_stage, 1, "依存段の検出は 1 回");
    assert!(
        dep.unrunnable(),
        "依存段は在るが動かないと読む: {:?}",
        dep.detection
    );
    assert_eq!(
        after_summary, after_dep_stage,
        "末尾の 1 行が tailscale の検出を引き直した（`--version` を {} 回追加で起こした）。\n\
         → {REMOTE_SETUP} の `fn setup_summary_lines` は渡された依存段の答え（`DepStatus::detection`）を\
         `check_status_with` へ渡すこと。`check_status()` / `status_of` を呼ぶと上限 10 秒の検出を\
         1 回の実行で何度も待つ（#1816）",
        after_summary - after_dep_stage
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some("スマホからの接続: Tailscale を実行できませんでした。確かめ直す: tako remote setup"),
        "末尾は依存段の答え（在るが動かない）で言い分ける: {lines:?}"
    );
}

/// **2**: `run_setup` は依存段の答えを末尾の 1 行へ渡す（`None` を渡すと引き直す）
#[test]
fn 末尾の1行は依存段の答えを受け取る() {
    let (body, base) = fn_body(CLI_SETUP, "run_setup");
    let at = find_in(
        &body,
        base,
        CLI_SETUP,
        "run_setup",
        "setup_summary_lines(",
        "末尾の 1 行（#1507）を出していない",
    );
    let call = &body[at..at + body[at..].find(')').expect("呼び出しの `)`")];
    assert!(
        call.contains("tailscale"),
        "{CLI_SETUP}:{}: `setup_summary_lines` に依存段の答えを渡していない（`{call})`）。\n\
         → `run_dependency_check` が返す `DepCheckOutcome::tailscale` を渡すこと（#1816）",
        line_at(&body, base, at)
    );

    // 受け取った依存段の答え（`DepStatus::detection`）を `check_status_with` へ渡す。
    // 中で検出を引く `check_status()` は依存表に Tailscale が無いときの逃げ道だけ（呼ばれたかは 1 が数える）
    let (summary, base) = fn_body(REMOTE_SETUP, "setup_summary_lines");
    let detection = find_in(
        &summary,
        base,
        REMOTE_SETUP,
        "setup_summary_lines",
        ".detection",
        "依存段の答え（`DepStatus::detection`）を使い回すこと（#1816）",
    );
    let with = find_in(
        &summary,
        base,
        REMOTE_SETUP,
        "setup_summary_lines",
        "check_status_with(",
        "検出の答えを受け取る `check_status_with` を通すこと（#1816）",
    );
    if let Some(at) = summary.find("check_status()") {
        assert!(
            with < at && detection < at,
            "{REMOTE_SETUP}:{}: `fn setup_summary_lines` が依存段の答えより先に `check_status()`\
             （中で検出を引く）を呼んでいる（#1816）",
            line_at(&summary, base, at)
        );
    }
}

/// **3**: `check_status` の tailscale 項目は検出の答え（4 通り）をそのまま出す。
/// `cli_path`（= 動く CLI があるか）で `ok` / `missing` の 2 通りへ畳む形へ戻らない
#[test]
fn check_statusのtailscale項目は検出の答えをそのまま出す() {
    let (body, base) = fn_body(REMOTE_SETUP, "check_status");
    find_in(
        &body,
        base,
        REMOTE_SETUP,
        "check_status",
        "check_status_with(",
        "`check_status` は検出 1 回を `check_status_with` へ渡すだけにする（#1816）",
    );
    let (body, base) = fn_body(REMOTE_SETUP, "check_status_with");
    find_in(
        &body,
        base,
        REMOTE_SETUP,
        "check_status_with",
        "detection.slug()",
        "tailscale 項目の status は検出の答え（ok / unrunnable / missing / unknown）から出す。\
         2 通りへ畳むと GUI のパネルと MCP が在るものを「未導入」と言う（#1816）",
    );
    if let Some(at) = body.find("cli_path.is_some()") {
        panic!(
            "{REMOTE_SETUP}:{}: `fn check_status_with` が tailscale 項目を `cli_path` の有無（2 通り）で決めている（#1816）",
            line_at(&body, base, at)
        );
    }
    // GUI のパネルは ok 以外を不足に数える（`missing` だけを数えると在るが動かない CLI で「準備 OK」と出る）
    let panel = "crates/tako-app/src/remote_panel.rs";
    let (body, base) = fn_body(panel, "setup_blockers");
    find_in(
        &body,
        base,
        panel,
        "setup_blockers",
        "status != \"ok\"",
        "serve 以外の項目は ok 以外を不足に数えること（unrunnable / unknown も起動を妨げる。#1816）",
    );
}

/// **4**: 依存段・`tako setup deps`・`tako setup --check` は「在るが動かない」を `[OK]` より先に言い分ける
#[test]
fn 依存段の表示は在るが動かないをokより先に言い分ける() {
    // (ファイル, 関数, 言い分けの印, [OK] の印)
    let cases = [
        (
            CLI_SETUP,
            "run_dependency_check",
            "unrunnable_text()",
            "\"  [OK] {}: {path}\"",
        ),
        (
            CLI_SETUP,
            "print_deps_human",
            "dep[\"runnable\"]",
            "\"  [OK] {bin}: {path}\"",
        ),
        (DIAGNOSTICS, "dep_items", "unrunnable_text()", "Status::Ok"),
    ];
    for (rel, name, distinguish, ok) in cases {
        let (body, base) = fn_body(rel, name);
        let d = find_in(
            &body,
            base,
            rel,
            name,
            distinguish,
            "在るが動かない CLI を `[警告]` で言い分けること（#1816）",
        );
        let o = find_in(
            &body,
            base,
            rel,
            name,
            ok,
            "`[OK]` の行が無い（改めたならこの番犬も直す）",
        );
        assert!(
            d < o,
            "{rel}:{}: `fn {name}` が在るが動かない CLI を言い分ける前に `[OK]` を出している（#1816）",
            line_at(&body, base, o)
        );
    }
}

/// **5**: 検出は固定の候補で見つからないときだけ、ログインシェルの PATH を**上限つきで**最後に引く（④）
#[test]
fn 検出はログインシェルのpathを上限つきで最後に引く() {
    let (body, base) = fn_body(TAILSCALE, "detect_tailscale");
    find_in(
        &body,
        base,
        TAILSCALE,
        "detect_tailscale",
        "login_shell_lookup",
        "正本はログインシェルの引き方を `pick_candidate` へ渡すこと。外すと Dock 起動の痩せた PATH で\
         MacPorts / nix の tailscale を「未導入」と判定し、`--yes` で brew 版を重ねて入れる（#1816）",
    );
    let (body, base) = fn_body(TAILSCALE, "login_shell_lookup");
    find_in(
        &body,
        base,
        TAILSCALE,
        "login_shell_lookup",
        "exe::find_bounded(",
        "ログインシェルへの問い合わせは上限つきの 1 実装（`exe::find_bounded`）を通す。\
         打ち切りを運べないと「確かめられなかった」を「無い」と言って導入へ進む（#1533 / #1816）",
    );
    for banned in ["Command::new", "exe::find("] {
        if let Some(at) = body.find(banned) {
            panic!(
                "{TAILSCALE}:{}: `fn login_shell_lookup` が `{banned}` を使っている（上限・打ち切りの知らせを失う。#1816）",
                line_at(&body, base, at)
            );
        }
    }
    let (body, base) = fn_body(TAILSCALE, "pick_candidate");
    let probe_loop = find_in(
        &body,
        base,
        TAILSCALE,
        "pick_candidate",
        "for candidate in candidates",
        "固定の候補を先に試すこと",
    );
    let lookup = find_in(
        &body,
        base,
        TAILSCALE,
        "pick_candidate",
        "lookup(",
        "固定の候補で動く CLI が無ければ素の名前をログインシェルで引き直すこと（#1816）",
    );
    assert!(
        probe_loop < lookup,
        "{TAILSCALE}:{}: `fn pick_candidate` が固定の候補より先にログインシェルを引いている。\n\
         → App Store 版・brew 版の人にまで毎回ログインシェルを起こす（#1816）",
        line_at(&body, base, lookup)
    );
    find_in(
        &body,
        base,
        TAILSCALE,
        "pick_candidate",
        "Detection::Unconfirmed",
        "ログインシェルを打ち切ったら「無い」と言い切らない（#1816）",
    );
}

/// **6**: `tako remote setup` の [1/5] は検出を 1 回だけ引く（`setup_status()` + `detect_tailscale()` の
/// 2 回で、`--version` が固まる CLI に上限 10 秒を 2 度待っていた = #1816）
#[test]
fn remote_setupの検出の段は1回だけ引く() {
    // (関数, 検出の段の次 = 系統を決める段の入口)
    for (name, next_step) in [
        ("run_interactive", "choose_variant_interactive("),
        // #2365: 非対話の経路は表示言語で断る型の口を呼ぶ
        ("run_noninteractive", "decide_variant_typed("),
    ] {
        let (body, base) = fn_body(REMOTE_SETUP, name);
        let step = body.find(next_step).unwrap_or_else(|| {
            panic!("{REMOTE_SETUP}:{base}: `fn {name}` に `{next_step}` が無い（改めたならこの番犬も直す）")
        });
        let head = &body[..step];
        let detects: Vec<usize> = head
            .match_indices("detect_tailscale()")
            .map(|(at, _)| line_at(&body, base, at))
            .collect();
        assert_eq!(
            detects.len(),
            1,
            "{REMOTE_SETUP}:{base}: `fn {name}` の検出の段が `detect_tailscale()` を {} 回引いている（行 {detects:?}）",
            detects.len()
        );
        if let Some(at) = head.find("setup_status()") {
            panic!(
                "{REMOTE_SETUP}:{}: `fn {name}` の検出の段が `setup_status()`（中で検出を引く）も呼んでいる（#1816）",
                line_at(&body, base, at)
            );
        }
    }
}
