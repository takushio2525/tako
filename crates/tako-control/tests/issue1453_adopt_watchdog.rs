//! 採用（adopt）と専用プロファイルの自動生成の番犬（#1453）
//!
//! # なぜ要るか
//!
//! #1453 の「素の master をその場で専用へ寄せる」は、**新しい状態を 1 つも足さずに**
//! 成り立っている。書き換えるのは**ペインの role ラベル 1 つ**で、`self` / `handoff` /
//! spawn 既定 / 自動ハンドオフはそこから追従する（`resolve_master_profile` が
//! 非既定の pane_role を既定の caller_role より優先する = #854）。
//!
//! 軽さと引き換えに、壊れ方が**静か**になる:
//!
//! - adopt を通さず `set_role` で master の role を書き換える実装が別の場所に生えると、
//!   採用の判断（専用起動の master・系統の食い違い・冪等）を**迂回した経路**が増える。
//!   増えたことは誰にも見えず、テストも通る
//! - 自動生成が既存を上書きする形に退化すると、利用者がプロファイルへ書いた設定が
//!   `projects add` のたびに消える。消えたことは次に master を立てるまで分からない
//! - 汎用の判定が `name == "default"` のハードコードへ戻ると、`codex` で起動した
//!   master が「専用」と誤判定されて採用できなくなる（判定は中身で決める = #1453 §3）
//! - `projects add` が生成を呼ばなくなる / 一括生成が migration の登録から外れると、
//!   自動化は「新しく登録したものだけ」になり、既存プロジェクトが取り残される
//!
//! # 何を縛るか
//!
//! 1. master の role ラベルを貼る本番コードが**既知の 4 か所**だけである
//!    （起動 = `Request::Title` / 引き継ぎの後任 / 会話を引き継いだ再起動 / 採用）
//! 2. 自動生成が既存ファイルを触らない（`is_file()` で返す + `create_profile_at` を通る）
//! 3. 汎用の判定が `projects` の中身で決まる（名前のハードコードでない）
//! 4. `projects add` の dispatch が `ensure_project_profile` を呼ぶ
//! 5. migration の登録簿が一括生成（`ensure_project_profiles`）を呼ぶ
//! 6. 採用の判断が**起動時のプロファイル**を見る（採用後の現在値ではない）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜6 はすべて無意味に緑になるので、[`走査が空振りしていない`]で
//! 窓が採れていることを固定し、[`逆戻りを名指しできる`]で**修正前を再現した 6 通りの注入**が
//! file:line で名指しされることを確かめる。範囲取りは #1420 の 1 実装を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MIGRATIONS: &str = "crates/tako-control/src/migrations.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// master の role ラベル（`orchestrator-master[:<profile>]`）を**貼ってよい**本番の経路。
/// 増やすときは「なぜ採用の判断を迂回してよいか」を添えてここへ書く
const KNOWN_ROLE_WRITERS: &[(&str, &str)] = &[
    (
        "apply_master_pane_profile_defaults(pane, Some(r.as_str()));",
        "起動（`Request::Title`）。`tako master` / `tako solo` / リモート起動が role を貼る入口",
    ),
    (
        "pane_obj.set_role(Some(new_role.clone()));",
        "引き継ぎの後任（`dispatch_orchestrator_handoff`）。新しく立てたペインへ貼る",
    ),
    (
        "pane_obj.set_role(Some(role_value.to_string()));",
        "会話を引き継いだセッション再起動（#1067）。同じ役割を貼り直す",
    ),
    (
        "pane_obj.set_role(Some(new_role.clone()));\n        pane_obj.set_title",
        "採用（#1453 の `dispatch_orchestrator_adopt`）",
    ),
];

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

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる）
fn production(rel: &'static str, src: &str) -> String {
    // dispatch.rs はテストが厚い（後半が丸ごと `#[cfg(test)]`）ので下限を明示的に下げる
    production_range::production_with_floor(src, rel, 0.20)
}

fn sources(root: &Path) -> (String, String, String, String) {
    (
        production(ORCH, &read(root, ORCH)),
        production(DISPATCH, &read(root, DISPATCH)),
        production(MIGRATIONS, &read(root, MIGRATIONS)),
        production(CLI, &read(root, CLI)),
    )
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 指定した関数の窓の中でだけ文字列を置き換える（注入用）。
/// ファイル全体を置換すると無関係な実装まで壊れて「何を測ったか」が濁る
fn replace_in_fn(src: &str, needle: &str, from: &str, to: &str) -> String {
    let Some((at, window)) = fn_window(src, needle) else {
        return src.to_string();
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = at - 1;
    let end = start + window.lines().count();
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.push(window.replace(from, to));
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// 注釈を落とした窓（アンチパターンを説明した注釈で落ちないようにする）
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 自動生成と採用判定（`orchestrator/mod.rs`）の検査
fn scan_orchestrator(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: ORCH,
            line,
            why,
        })
    };

    // 2. 生成が既存を触らない
    match fn_window(src, "pub fn ensure_project_profile(") {
        None => push(
            0,
            "`ensure_project_profile` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("path.is_file()") {
                push(
                    at,
                    "既存ファイルの検査（`path.is_file()`）が無い = `projects add` のたびに \
                     利用者の設定を上書きしうる（#1453 は冪等が条件）"
                        .into(),
                );
            }
            if !code.contains("create_profile_at(") {
                push(
                    at,
                    "`create_profile_at` を通っていない（ロック内で存在確認する TOCTOU 安全な \
                     1 実装を迂回している。#169）"
                        .into(),
                );
            }
            if !code.contains("CreateError::Exists") {
                push(
                    at,
                    "並行生成で先を越された場合（`CreateError::Exists`）を失敗として扱っている \
                     = 冪等でない"
                        .into(),
                );
            }
            if !code.contains("project_profile_base()") {
                push(
                    at,
                    "継承元（`project_profile_base` = default）を通っていない = \
                     利用者の個人ルールを引き継がない（#1453 §4）"
                        .into(),
                );
            }
        }
    }

    // 3. 汎用の判定は中身で決める（名前のハードコードでない）
    match fn_window(src, "pub fn is_generic_profile(") {
        None => push(
            0,
            "`is_generic_profile` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("projects") {
                push(at, "汎用の判定が `projects` を見ていない".into());
            }
            if code.contains("\"default\"") || code.contains("DEFAULT_PROFILE") {
                push(
                    at,
                    "汎用の判定が名前のハードコードへ退化している（`codex` も汎用の入口なので \
                     名前で決めると codex master が採用できなくなる。#1453 §3）"
                        .into(),
                );
            }
        }
    }

    // 6. 採用の判断は「起動時のプロファイル」を見る
    match fn_window(src, "pub fn decide_adopt(") {
        None => push(0, "`decide_adopt` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("launched") {
                push(
                    at,
                    "拒否の判断が起動時のプロファイル（`launched`）を見ていない。\
                     採用後の現在値で判断すると、一度採用した master が採用し直せず \
                     `adopt default` でも戻れなくなる（#1453）"
                        .into(),
                );
            }
            if !code.contains("resolve_master_agent()") {
                push(
                    at,
                    "master の系統（`master_agent`）を見ていない = 引き継ぎの後任が \
                     黙って別系統で立ち上がる（codex master の後任が claude になる）"
                        .into(),
                );
            }
            if !code.contains("AdoptDecision::Unchanged") {
                push(
                    at,
                    "同じプロファイルへの 2 回目（冪等）を扱っていない".into(),
                );
            }
        }
    }
    out
}

/// dispatch 側（role の書き換え口と `projects add`）の検査
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };

    // 1. master の role ラベルを貼る本番の口が既知の集合だけか
    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if !line.contains("master_pane_role(") {
            continue;
        }
        // 組み立てているだけ（貼っていない）行は対象外。`set_role` を伴う窓の中だけを見る
        let known = KNOWN_ROLE_WRITERS
            .iter()
            .any(|(needle, _)| src.contains(needle));
        if !known {
            push(
                i + 1,
                "master の role ラベルを貼る本番の口が既知の集合から外れている（採用の判断を \
                 迂回した経路が増えていないか確かめる。#1453）"
                    .into(),
            );
        }
    }
    let writers = src
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains("set_role(Some("))
        .count();
    // 既知は 3 か所（引き継ぎの後任 / 再起動 / 採用）+ `Request::Title` の 1 か所は
    // `set_role((!r.is_empty()).then_some(..))` なのでこの数え方には入らない
    if writers > KNOWN_ROLE_WRITERS.len() {
        push(
            0,
            format!(
                "role を貼る本番の口が {writers} か所に増えている（既知は {}）。\
                 master の role を採用（adopt）以外から書き換える経路が増えると、\
                 専用起動の拒否・系統の検査・冪等がその経路だけ抜ける（#1453）",
                KNOWN_ROLE_WRITERS.len()
            ),
        );
    }

    // 採用の 1 実装そのもの
    match fn_window(src, "fn dispatch_orchestrator_adopt(") {
        None => push(
            0,
            "`dispatch_orchestrator_adopt` が見つからない（採用の 1 実装が消えている）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            for (needle, why) in [
                (
                    "decide_adopt(",
                    "採用の判断（`decide_adopt`）を通っていない = 専用起動の master や \
                     系統違いをそのまま通す",
                ),
                (
                    "master_pane_role(",
                    "role ラベルの組み立てを 1 実装（`master_pane_role`）から採っていない \
                     = 表示用と env 用の語彙を取り違える（#761）",
                ),
                (
                    "apply_master_pane_profile_defaults(",
                    "採用先のプロファイル既定（#1140 の自動復帰）を配っていない",
                ),
                (
                    "legacy_auto_profile()",
                    "A/B の口（`TAKO_1453_LEGACY`）を通っていない = 旧挙動と比べられない",
                ),
                (
                    "master_profile_of_any_role",
                    "起動時のプロファイル（caller_role 由来）を解決していない = \
                     採用後の値で拒否を判断してしまう",
                ),
            ] {
                if !code.contains(needle) {
                    push(at, why.into());
                }
            }
        }
    }

    // 4. `projects add` が生成を呼ぶ
    match fn_window(src, "fn dispatch_orchestrator_projects(") {
        None => push(
            0,
            "`dispatch_orchestrator_projects` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !code_only(&window).contains("ensure_project_profile(") {
                push(
                    at,
                    "`projects add` が専用プロファイルを作らない（#1453 の期待 1。\
                     登録と生成が離れると、新しいプロジェクトだけ自動化から漏れる）"
                        .into(),
                );
            }
        }
    }
    out
}

/// migration の登録簿（既存プロジェクトの一括生成）の検査
fn scan_migrations(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: MIGRATIONS,
            line,
            why,
        })
    };
    match fn_window(src, "fn project_profile_reports(") {
        None => push(
            0,
            "`project_profile_reports` が無い（既存プロジェクトの一括生成が登録簿から \
             外れている = 登録済みのプロジェクトが自動化に取り残される。#1453 §5）"
                .into(),
        ),
        Some((at, window)) => {
            if !code_only(&window).contains("ensure_project_profiles()") {
                push(
                    at,
                    "一括生成の 1 実装（`ensure_project_profiles`）を通っていない".into(),
                );
            }
        }
    }
    if !code_only(src).contains("if spec.id == SchemaId::Profiles {") {
        push(
            0,
            "`run` が Profiles の段で一括生成を呼んでいない（発火点 3 か所 = setup / \
             GUI 起動 / CLI のどこからも揃わなくなる）"
                .into(),
        );
    }
    out
}

/// CLI が写しを持たず dispatch の 1 本を通るか（設計原則 5 = CLI / MCP 1:1）
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: CLI,
            line,
            why,
        })
    };
    match fn_window(src, "fn orchestrator_projects_cli(") {
        None => push(
            0,
            "`orchestrator_projects_cli` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("dispatch_orchestrator_projects(") {
                push(
                    at,
                    "CLI の `projects` が dispatch の 1 本を通っていない（写しを持つと \
                     #1453 の自動生成のように**片方だけ効く**機能ができる。実測で踏んだ）"
                        .into(),
                );
            }
            if code.contains("ProjectsConfig::mutate(") {
                push(
                    at + code_line_with(&window, "ProjectsConfig::mutate(").unwrap_or(0),
                    "CLI が projects.yaml を自分で書き換えている（書き込みは dispatch 側の \
                     1 実装に寄せる。#1453）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

fn all(orch: &str, dispatch: &str, migrations: &str, cli: &str) -> Vec<String> {
    scan_orchestrator(orch)
        .iter()
        .chain(scan_dispatch(dispatch).iter())
        .chain(scan_migrations(migrations).iter())
        .chain(scan_cli(cli).iter())
        .map(Offender::report)
        .collect()
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let (orch, dispatch, migrations, cli) = sources(&root);
    for (rel, src, needle) in [
        (ORCH, &orch, "pub fn ensure_project_profile("),
        (ORCH, &orch, "pub fn decide_adopt("),
        (ORCH, &orch, "pub fn is_generic_profile("),
        (DISPATCH, &dispatch, "fn dispatch_orchestrator_adopt("),
        (DISPATCH, &dispatch, "fn dispatch_orchestrator_projects("),
        (MIGRATIONS, &migrations, "fn project_profile_reports("),
        (CLI, &cli, "fn orchestrator_projects_cli("),
    ] {
        assert!(
            fn_window(src, needle).is_some(),
            "{rel}: `{needle}` の窓が採れない（本番コードの範囲取りが壊れている）"
        );
    }
}

#[test]
fn 現行実装は違反ゼロ() {
    let root = workspace_root();
    let (orch, dispatch, migrations, cli) = sources(&root);
    let found = all(&orch, &dispatch, &migrations, &cli);
    assert!(
        found.is_empty(),
        "採用（#1453）の不変条件に違反: {found:#?}"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let (orch, dispatch, migrations, cli) = sources(&root);

    // 注入 1: 生成が既存を上書きする（利用者の設定が `projects add` のたびに消える）
    let overwrite = orch.replace("    if path.is_file() {", "    if false {");
    assert_ne!(overwrite, orch, "注入 1 の対象が見つからない");
    let found = all(&overwrite, &dispatch, &migrations, &cli);
    assert!(
        found.iter().any(|o| o.starts_with(ORCH)),
        "既存を上書きする形への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 2: 汎用の判定を名前のハードコードへ戻す（codex master が採用できなくなる）
    let hardcoded = orch.replace(
        "    profile.projects.as_ref().is_none_or(|p| p.is_empty())",
        "    let _ = profile;\n    name == \"default\"",
    );
    assert_ne!(hardcoded, orch, "注入 2 の対象が見つからない");
    let found = all(&hardcoded, &dispatch, &migrations, &cli);
    assert!(
        found.iter().any(|o| o.contains("名前のハードコード")),
        "名前決め打ちへの逆戻りを名指しできていない: {found:?}"
    );

    // 注入 3: 採用の判断が起動時ではなく現在値を見る（採用し直せず `adopt default`
    // でも戻れなくなる = 実装中に実際に踏んだ形）。`decide_adopt` の窓の中でだけ
    // 起動時プロファイルの名前を潰す
    let by_current = replace_in_fn(&orch, "pub fn decide_adopt(", "launched", "current");
    assert_ne!(by_current, orch, "注入 3 の対象が見つからない");
    let found = all(&by_current, &dispatch, &migrations, &cli);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(ORCH) && o.contains("起動時のプロファイル")),
        "現在値で判断する形への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 4: `projects add` が生成を呼ばなくなる
    let no_gen = dispatch.replace(
        "            let gen = orchestrator::ensure_project_profile(&key);",
        "            let gen = orchestrator::ProfileGen::Skipped { reason: String::new() };",
    );
    assert_ne!(no_gen, dispatch, "注入 4 の対象が見つからない");
    let found = all(&orch, &no_gen, &migrations, &cli);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(DISPATCH) && o.contains("専用プロファイルを作らない")),
        "登録だけして生成しない形への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 5: 採用が判断を飛ばして role を貼る
    let no_decide = dispatch.replace(
        "    let decision = orchestrator::decide_adopt(",
        "    let decision = orchestrator::AdoptDecision::Adopt { warnings: vec![] };\n    let _unused = (",
    );
    assert_ne!(no_decide, dispatch, "注入 5 の対象が見つからない");
    let found = all(&orch, &no_decide, &migrations, &cli);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(DISPATCH) && o.contains("decide_adopt")),
        "判断を飛ばす形への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 7: CLI が dispatch の写しを持つ（#1453 の実測で踏んだ形。
    // MCP からは生成が効くのに CLI からは効かない）
    let cli_copy = replace_in_fn(
        &cli,
        "fn orchestrator_projects_cli(",
        "tako_control::dispatch::dispatch_orchestrator_projects(",
        "local_projects_add(",
    );
    assert_ne!(cli_copy, cli, "注入 7 の対象が見つからない");
    let found = all(&orch, &dispatch, &migrations, &cli_copy);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(CLI) && o.contains("1 本を通っていない")),
        "CLI が写しを持つ形への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 6: 一括生成が登録簿から外れる（既存プロジェクトが取り残される）
    let no_bulk = migrations.replace(
        "fn project_profile_reports(",
        "fn project_profile_reports_disabled(",
    );
    assert_ne!(no_bulk, migrations, "注入 6 の対象が見つからない");
    let found = all(&orch, &dispatch, &no_bulk, &cli);
    assert!(
        found.iter().any(|o| o.starts_with(MIGRATIONS)),
        "一括生成が外れた形を名指しできていない: {found:?}"
    );
}

/// A/B の口は Issue ごとに 1 つ（逃げ道が増えると「どちらの腕か」が言えなくなる）
#[test]
fn ab_の口は1つだけ() {
    let root = workspace_root();
    let (orch, dispatch, migrations, cli) = sources(&root);
    for (rel, src) in [
        (ORCH, &orch),
        (DISPATCH, &dispatch),
        (MIGRATIONS, &migrations),
        (CLI, &cli),
    ] {
        let names: Vec<&str> = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains("TAKO_1453"))
            .collect();
        for l in &names {
            assert!(
                l.contains("TAKO_1453_LEGACY"),
                "{rel}: #1453 の A/B の口が増えている: {l}"
            );
        }
    }
    // 読む場所も 1 か所（`legacy_auto_profile`）だけ。数えるのは**読む式**で、
    // 診断文に綴られた名前は数えない（理由を書くほど落ちる番犬にしない）
    let readers = orch
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .filter(|l| {
            l.contains("var_os(\"TAKO_1453_LEGACY\")") || l.contains("var(\"TAKO_1453_LEGACY\")")
        })
        .count();
    assert_eq!(
        readers, 1,
        "`TAKO_1453_LEGACY` を読む本番の口が 1 つでない（`legacy_auto_profile` へ寄せる）"
    );
}
