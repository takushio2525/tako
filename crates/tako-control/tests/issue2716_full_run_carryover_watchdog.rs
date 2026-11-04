//! **#2716 の番犬**: visual-test の全節実行が「前の節の持ち越し」で止まる形へ戻さない。
//!
//! ## 何が起きていたのか
//!
//! 全節実行は 1 プロセスで節を順に回すので、前の節が残したものが次の節の場面へ載る。
//! 153 shelve-tab / #745 chat-table で止まる件（#2716）を調べると、その 2 節は今の main で
//! 止まらず（全節 10 回とも通過）、代わりに後ろの節が持ち越しで止まっていた:
//!
//! - md-edit-resume: 前の節が残した分割の隣に開くので、既定の窓（960×600）でプレビューが
//!   0.25 幅 = 編集ボタンの出ない幅（300px 未満）になり「編集ボタンが描かれない」（6/7）
//! - bracket-match: fixture（`.rs`）の言語サーバの解決が前の節から節の途中に着地し、未導入の帯が
//!   器を縮めて ② が基準の画素とずれた（1/7）
//! - lsp-context-menu: 言語サーバの実行ファイルの解決は ID ごとに覚えられ（`LspManager` の
//!   `resolved`。#1769）、env の差し替え口（`TAKO_LSP_BIN_<ID>`）を後から書いても引き直さない。
//!   先の `.rs` の節が覚えた「PATH のものが動かない」が勝ち、偽サーバへの差し替えが効かずに
//!   ① が「言語サーバの節が無い」（3/3。単独実行は差し替えが先なので通る）
//! - chat-table（#745）: dispatch で直に開いた後に積んだ読み込みを流しておらず、表はファイル
//!   監視の読み直し（fixture の書き込みを拾う）が来たときだけ揃っていた（#826 と同じ流し忘れ）
//!
//! ## ここで止めるもの
//!
//! 1. [`言語サーバの差し替え口は1実装を通す`] — tako-app の本文で差し替え口の名前
//!    （`override_env_name(`）を作ってよいのは読み（`std::env::var(`）と `vt_lsp_override` の中だけ。
//!    `"TAKO_LSP_BIN_…"` を `set_var` / `remove_var` へ直に渡す行も file:line で名指す
//! 2. [`差し替えの補助はrestartの口で覚えを捨てる`] — `vt_lsp_override` が書くときに `LspServer` の
//!    restart を dispatch し、旧の形（`TAKO_2716_LEGACY=lsp-override`）だけが通らない。外すときは
//!    env を消して restart の前に戻る（restart は開いた文書が無くても器を起こし直し、実の PATH の
//!    解決が次の節へ着地する = 持ち越しを自分で作る）
//! 3. [`持ち越しで止まった節は場面と状態を自分で決める`] — md-edit-resume が開く前に
//!    `vt2716_own_scene`（= `ensure_fresh_scene`）で場面を作る・chat-table が開いた後に
//!    `drain_pending_preview_loads` を呼ぶ・bracket-match が言語サーバを差し替え、状態の着地
//!    （`vt_lsp_settled`）を基準のフレームより前に待つ
//! 4. [`修正前の形を名指しできる`] — 修正前の形を注入すると 1〜3・5 のどれかが名指しで落ちる
//! 5. [`言語サーバの節は自動起動の前提を名指しで確かめる`] — 隔離起動の GUI は
//!    `TAKO_LSP_AUTOSTART=1` が無いと文書を開いても言語サーバを起こさない（#2048）。付けずに
//!    全節実行を起こすと lsp-context-menu ① が「言語サーバの節が無い」の FAILED に見えた（段 3 の
//!    切り分け）。`vt_require_lsp_autostart` が全節実行の頭と差し替えの補助で確かめ、欠けていれば
//!    `TAKO_VISUAL_TEST_UNMET` の行と終了コード 3（失敗 = 1 と別）で止まる
//!
//! ## 相方
//!
//! restart が解決の覚えと未導入の記録を捨てること（CLI `tako lsp restart` / MCP と同じ dispatch）は
//! `issue1769_lsp_followup.rs` の `servers_の2回目以降は解決を起こさず合図と_restart_で引き直す`。
//! 実 GUI の A/B（`TAKO_2716_LEGACY` / `TAKO_2716_INJECT`）は `.agent/conventions.md` の
//! 「画面の外の持ち越しも節が自分で決める」の項。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

const MAIN: &str = "crates/tako-app/src/main.rs";
const BRACKET: &str = "crates/tako-app/src/self_test/bracket_match_1691.rs";
const APP_SRC: &str = "crates/tako-app/src";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// `head` の後ろの最初の `{` から対の `}` までのバイト範囲（コードだけの眺めで括弧を数える）
fn block_range(code: &str, head: &str) -> Option<std::ops::Range<usize>> {
    let at = code.find(head)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in code.as_bytes()[open..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open..open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// 規則 1: 差し替え口の名前を補助の外で作る行・`"TAKO_LSP_BIN_` を直に書き換える行
fn override_outside_helper(rel: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let helper = block_range(&code, "fn vt_lsp_override(").unwrap_or(0..0);
    let mut hits = Vec::new();
    let mut offset = 0usize;
    for (ix, line) in code.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += line.len();
        if !line.contains("override_env_name(") || helper.contains(&start) {
            continue;
        }
        // 読み（差し替えが効いているかを見るだけ）は書き換えないので通す
        if line.contains("std::env::var(") || line.contains("std::env::var_os(") {
            continue;
        }
        hits.push(format!(
            "{rel}:{}: {}",
            ix + 1,
            src.lines().nth(ix).unwrap_or("").trim()
        ));
    }
    let literal = code_view::without_comments(src);
    for (ix, line) in literal.lines().enumerate() {
        let writes = line.contains("set_var(") || line.contains("remove_var(");
        if writes && line.contains("\"TAKO_LSP_BIN_") {
            hits.push(format!(
                "{rel}:{}: {}",
                ix + 1,
                src.lines().nth(ix).unwrap_or("").trim()
            ));
        }
    }
    hits
}

/// 規則 2: 補助が restart の口を通り、旧の形の切り替えを持つか（足りないものを返す）
fn helper_problems(src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let Some(range) = block_range(&code, "fn vt_lsp_override(") else {
        return vec!["`fn vt_lsp_override(` が無い（差し替え口を書く・外す 1 実装）".into()];
    };
    // 文字列（"restart" / 項目名）を見るのでコメントだけ潰した眺めの同じ範囲を読む
    let body = &code_view::without_comments(src)[range];
    let mut problems = Vec::new();
    if !(body.contains("Request::LspServer") && body.contains("action: \"restart\"")) {
        problems.push(
            "`vt_lsp_override` が restart の口（`Request::LspServer` の `action: \"restart\"`）を \
             dispatch していない（前の節が覚えた解決が勝ち、差し替えが効かない）"
                .into(),
        );
    }
    if !body.contains("name: Some(id") {
        problems.push(
            "`vt_lsp_override` の restart がサーバを名指ししていない（`name: Some(id…)`。\
             名指しなしは他のサーバまで起こし直す）"
                .into(),
        );
    }
    if !body.contains("legacy_2716(\"lsp-override\")") {
        problems.push(
            "`vt_lsp_override` に旧の形の切り替え（`legacy_2716(\"lsp-override\")`）が無い（A/B）"
                .into(),
        );
    }
    // 外す側は restart の前に戻る（restart は開いた文書が無くても器を起こし直す）
    let returns_early = block_range(body, "let Some(program) = program else").is_some_and(|r| {
        let removal = &body[r];
        removal.contains("remove_var(")
            && removal.contains("return id")
            && !removal.contains("Request::LspServer")
    });
    if !returns_early {
        problems.push(
            "`vt_lsp_override` の外す側（`None`）が env を消して restart の前に戻っていない\
             （`let Some(program) = program else { remove_var(…); return id; }`。restart は開いた文書が\
             無くても器を起こし直すので、外すたびに撃つと実の PATH の解決が次の節へ着地する）"
                .into(),
        );
    }
    problems
}

/// 両方が見つかり、`a` が `b` より前にあるか
fn before(a: Option<usize>, b: Option<usize>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a < b)
}

/// 規則 3: 持ち越しで止まった節が場面と状態を自分で決めているか（足りないものを返す）
fn section_problems(main: &str, bracket: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_view::code_view(main);
    match block_range(&code, "async fn vt2716_own_scene(").map(|r| &code[r]) {
        Some(body) if body.contains("ensure_fresh_scene(") => {}
        Some(_) => problems.push(
            "`vt2716_own_scene` が `ensure_fresh_scene(` を通っていない（場面を作らない）".into(),
        ),
        None => problems
            .push("`async fn vt2716_own_scene(` が無い（プレビューを開く節の場面づくり）".into()),
    }
    match block_range(&code, "async fn md_edit_resume_visual(").map(|r| &code[r]) {
        Some(body) => {
            let scene = body.find("vt2716_own_scene(");
            let open = body.find("Req::OpenFile");
            if !before(scene, open) {
                problems.push(
                    "md-edit-resume が開く前に `vt2716_own_scene(` で場面を作っていない（前の節の \
                     分割の隣に開くとプレビューが 0.25 幅になり編集ボタンが出ない）"
                        .into(),
                );
            }
            if !body.contains("anchor.unwrap_or_else(") {
                problems.push("md-edit-resume が場面の錨（`anchor`）から開いていない".into());
            }
        }
        None => problems.push("`async fn md_edit_resume_visual(` が見つからない".into()),
    }
    match block_range(&code, "async fn chat_table_visual(").map(|r| &code[r]) {
        Some(body) => {
            let open = body.find("Request::OpenFile");
            let drain = body.find("drain_pending_preview_loads(");
            if !before(open, drain) {
                problems.push(
                    "chat-table（#745）が dispatch で開いた後に `drain_pending_preview_loads(` を \
                     呼んでいない（表がファイル監視の読み直し頼みになる = #826 の流し忘れ）"
                        .into(),
                );
            }
        }
        None => problems.push("`async fn chat_table_visual(` が見つからない".into()),
    }
    let code = code_view::code_view(bracket);
    match block_range(&code, "async fn bracket_match_visual(").map(|r| &code[r]) {
        Some(body) => {
            let open = body.find("Req::OpenFile");
            let decide = body.find("vt_lsp_override(");
            if !before(decide, open) {
                problems.push(
                    "bracket-match が fixture を開く前に `vt_lsp_override(` で言語サーバを決めていない \
                     （前の節の解決が節の途中に着地する）"
                        .into(),
                );
            }
            let settled = body.find("vt_lsp_settled(");
            let reference = body.find("capture_frame(");
            if !before(settled, reference) {
                problems.push(
                    "bracket-match が基準のフレーム（最初の `capture_frame(`）より前に言語サーバの \
                     状態の着地（`vt_lsp_settled(`）を待っていない（帯が基準の後に出る）"
                        .into(),
                );
            }
        }
        None => problems.push("`async fn bracket_match_visual(` が見つからない".into()),
    }
    problems
}

/// 規則 5: 言語サーバの節の前提（自動起動）を、FAILED と別の名指しで確かめているか
fn autostart_problems(main: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_view::code_view(main);
    let strings = code_view::without_comments(main);
    match block_range(&code, "fn vt_require_lsp_autostart(") {
        Some(range) => {
            let body = &strings[range];
            for (needle, why) in [
                (
                    "autostart_from_env()",
                    "GUI が manager を作ったときと同じ判定",
                ),
                ("TAKO_VISUAL_TEST_UNMET", "FAILED と区別できる行"),
                (
                    "visual_exit(VISUAL_UNMET_EXIT_CODE)",
                    "失敗（1）と別の終了コード",
                ),
                ("legacy_2716(\"autostart\")", "A/B の旧の形"),
            ] {
                if !body.contains(needle) {
                    problems.push(format!(
                        "`vt_require_lsp_autostart` に `{needle}` が無い（{why}）"
                    ));
                }
            }
            if body.contains("fail(") {
                problems.push(
                    "`vt_require_lsp_autostart` が `fail(` で止まる（前提の欠けを FAILED と読み違える）"
                        .into(),
                );
            }
        }
        None => problems.push(
            "`fn vt_require_lsp_autostart(` が無い（言語サーバの節の前提を確かめる口）".into(),
        ),
    }
    if !main.contains("const VISUAL_UNMET_EXIT_CODE: i32 = 3;") {
        problems.push(
            "前提の欠けの終了コード `VISUAL_UNMET_EXIT_CODE` が 3 でない（失敗 = 1・置き先が無い = 4 と分ける）"
                .into(),
        );
    }
    match block_range(&code, "fn vt_lsp_override(").map(|r| &code[r]) {
        Some(body)
            if before(
                body.find("vt_require_lsp_autostart("),
                body.find("std::env::set_var("),
            ) => {}
        _ => problems.push(
            "`vt_lsp_override` が差し替えを書く前に `vt_require_lsp_autostart(` を通っていない"
                .into(),
        ),
    }
    // 全節実行の並びの頭（最後の `terminal_grid_visual(` = 全節の最初の節）より前で確かめる
    if !before(
        strings.find("vt_require_lsp_autostart(\"全節実行\")"),
        code.rfind("terminal_grid_visual(any, window, cx).await;"),
    ) {
        problems.push(
            "全節実行の頭（最初の節 `terminal_grid_visual` の前）で `vt_require_lsp_autostart(\"全節実行\")` \
             を通っていない（欠けたまま何分も走って lsp-context-menu ① の FAILED に見える）"
                .into(),
        );
    }
    problems
}

fn app_sources() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut files = Vec::new();
    rs_files(&root.join(APP_SRC), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&root)
                .expect("ワークスペースの中")
                .to_string_lossy()
                .replace('\\', "/");
            let src = read(&rel);
            (rel, src)
        })
        .collect()
}

#[test]
fn 言語サーバの差し替え口は1実装を通す() {
    let sources = app_sources();
    assert!(
        sources.len() > 50,
        "走査範囲が空振りしている（{} ファイル）",
        sources.len()
    );
    let hits: Vec<String> = sources
        .iter()
        .flat_map(|(rel, src)| override_outside_helper(rel, src))
        .collect();
    assert!(
        hits.is_empty(),
        "visual-test の節が言語サーバの差し替え口（`TAKO_LSP_BIN_<ID>`）を直に書き換えている。\
         全節実行では前の節が覚えた実行ファイルの解決が勝ち、差し替えが効かない（#2716 の \
         lsp-context-menu ①）。`vt_lsp_override(window, cx, vt_lsp_server_of(&path), Some(&fake))` で \
         書き、後片付けは `vt_lsp_override(window, cx, id, None)` で外す:\n{}",
        hits.join("\n")
    );
}

#[test]
fn 差し替えの補助はrestartの口で覚えを捨てる() {
    let problems = helper_problems(&read(MAIN));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 持ち越しで止まった節は場面と状態を自分で決める() {
    let problems = section_problems(&read(MAIN), &read(BRACKET));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 言語サーバの節は自動起動の前提を名指しで確かめる() {
    let problems = autostart_problems(&read(MAIN));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn 修正前の形を名指しできる() {
    let main = read(MAIN);
    let bracket = read(BRACKET);
    // 1. 節が差し替え口を直に書く（修正前の lsp-context-menu の形）
    let direct = format!(
        "{main}\nfn injected() {{\n    let rs_env = tako_core::lsp::servers::override_env_name(id);\n    \
         std::env::set_var(&rs_env, &fake);\n    std::env::remove_var(\"TAKO_LSP_BIN_RUST_ANALYZER\");\n}}\n"
    );
    let hits = override_outside_helper(MAIN, &direct);
    assert_eq!(
        hits.len(),
        2,
        "直に書く 2 行（名前を作る行・リテラルを外す行）を名指せる: {hits:?}"
    );
    assert!(hits
        .iter()
        .all(|h| h.starts_with("crates/tako-app/src/main.rs:")));
    // 2. 補助が restart を撃たない
    let no_restart = main.replacen("action: \"restart\".into()", "action: \"status\".into()", 1);
    assert_ne!(no_restart, main, "注入先（restart の行）が本文に在る");
    assert!(
        helper_problems(&no_restart)
            .iter()
            .any(|p| p.contains("restart の口")),
        "restart を撃たない補助を名指せる"
    );
    // 2b. 外すときも restart を撃つ（段 2 の途中の形 = 実の PATH の解決が次の節へ着地した）
    let restart_on_remove = main.replacen(
        "let Some(program) = program else {\n            std::env::remove_var(&env);\n            \
         return id;\n        };\n        vt_require_lsp_autostart(id);\n        \
         std::env::set_var(&env, program);",
        "vt_require_lsp_autostart(id);\n        match program {\n            \
         Some(program) => std::env::set_var(&env, program),\n            \
         None => std::env::remove_var(&env),\n        }",
        1,
    );
    assert_ne!(
        restart_on_remove, main,
        "注入先（外す側の早い戻り）が本文に在る"
    );
    assert!(
        helper_problems(&restart_on_remove)
            .iter()
            .any(|p| p.contains("外す側（`None`）")),
        "外すときも restart を撃つ補助を名指せる"
    );
    // 3a. md-edit-resume が場面を作らない（修正前）
    let stale_scene = main.replacen(
        "vt2716_own_scene(window, cx, \"md-edit-resume\").await",
        "None::<PaneId>",
        1,
    );
    assert_ne!(
        stale_scene, main,
        "注入先（md-edit-resume の場面づくり）が本文に在る"
    );
    assert!(
        section_problems(&stale_scene, &bracket)
            .iter()
            .any(|p| p.contains("md-edit-resume が開く前に")),
        "場面を作らない md-edit-resume を名指せる"
    );
    let helper_off = main.replacen(
        "Some(ensure_fresh_scene(window, cx, label).await)",
        "None",
        1,
    );
    assert_ne!(helper_off, main, "注入先（場面づくりの補助）が本文に在る");
    assert!(
        section_problems(&helper_off, &bracket)
            .iter()
            .any(|p| p.contains("`vt2716_own_scene` が")),
        "場面を作らない補助を名指せる"
    );
    // 3b. chat-table が流さない（修正前）
    let no_drain = {
        let code = code_view::code_view(&main);
        let range = block_range(&code, "async fn chat_table_visual(").expect("chat-table の本文");
        let body = main[range.clone()].replace("app.drain_pending_preview_loads(cx);", "");
        format!("{}{}{}", &main[..range.start], body, &main[range.end..])
    };
    assert_ne!(no_drain, main, "注入先（chat-table の流し）が本文に在る");
    assert!(
        section_problems(&no_drain, &bracket)
            .iter()
            .any(|p| p.contains("chat-table（#745）")),
        "流さない chat-table を名指せる"
    );
    // 3c. bracket-match が着地を待たない（修正前）
    let no_settle = bracket.replace("vt_lsp_settled(", "vt_lsp_any(");
    assert_ne!(
        no_settle, bracket,
        "注入先（bracket-match の着地の待ち）が本文に在る"
    );
    assert!(
        section_problems(&main, &no_settle)
            .iter()
            .any(|p| p.contains("状態の着地")),
        "着地を待たない bracket-match を名指せる"
    );
    // 5a. 全節実行の頭で自動起動の前提を確かめない（段 2 までの形）
    let no_head = main.replacen("vt_require_lsp_autostart(\"全節実行\");", "", 1);
    assert_ne!(no_head, main, "注入先（全節実行の頭の確かめ）が本文に在る");
    assert!(
        autostart_problems(&no_head)
            .iter()
            .any(|p| p.contains("全節実行の頭")),
        "頭で確かめない全節実行を名指せる"
    );
    // 5b. 前提の欠けを FAILED で止める（読み手が落ちたと読み違える形）
    let as_failed = main.replacen("visual_exit(VISUAL_UNMET_EXIT_CODE);", "fail(\"前提\");", 1);
    assert_ne!(as_failed, main, "注入先（前提の欠けの出口）が本文に在る");
    assert!(
        autostart_problems(&as_failed)
            .iter()
            .any(|p| p.contains("`fail(`")),
        "前提の欠けを FAILED で止める形を名指せる"
    );
}
