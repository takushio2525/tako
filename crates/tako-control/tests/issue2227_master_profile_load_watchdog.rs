//! master の起動で読めない `profiles/default.yaml` を黙って組み込み既定へ落とさない番犬（#2227）
//!
//! # なぜ要るか
//!
//! `tako master`（CLI）と `master_launch::plan`（スマホの「+ master」・返答の配送）は、
//! それぞれ `Err(_) if profile_name == "default" => Profile::default()` で読めない
//! default.yaml を握りつぶしていた。個人ルール（`prompt_blocks`）・モデルの抜けた master が
//! 警告なしに立ち、`-<key>` は #1825 で専用プロファイルが作られていないため直し方の無い
//! 「見つからない」で止まった（修正前の実測）。どちらも壊れ方が静かで、立てた master に
//! ルールが効いていないと気づくまで誰にも見えない。
//!
//! # 何を縛るか
//!
//! 1. CLI と `plan` がプロファイルを**同じ 1 実装**（`master_launch::load_profile`）で読み、
//!    自前の `Profile::load` / `Profile::default()` の握りつぶしへ戻らない
//! 2. `load_default_at` が継承元と同じ読み方（`project_profile_base_at` = 退避つき）を通り、
//!    `Err` を握りつぶさず warning にする
//! 3. `load_profile` が warning を persist.log へ出し、`-<key>` の「見つからない」に
//!    #1825 の理由（`missing_project_profile_reason`）を足す
//! 4. 理由の判定が継承元の読み取り（`check_project_profile_base`）と #1825 の理由文を通る
//! 5. warning が画面（CLI の `警告:` 行・応答の `warnings`）と master 本人の system prompt
//!    （`with_startup_warning`）へ届く
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**がそれぞれ注入した行の file:line で名指しされることを確かめる。
//! 範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const LAUNCH: &str = "crates/tako-control/src/orchestrator/master_launch.rs";
const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// 読み込みで**使ってはいけない**握りつぶし
const SWALLOWS: &[&str] = &[
    "unwrap_or_default()",
    ".ok()",
    "unwrap_or(",
    "unwrap_or_else(",
    ".unwrap()",
    ".expect(",
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
    production_range::production_with_floor(src, rel, 0.20)
}

#[derive(Clone)]
struct Sources {
    launch: String,
    orch: String,
    cli: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        launch: production(LAUNCH, &read(root, LAUNCH)),
        orch: production(ORCH, &read(root, ORCH)),
        cli: production(CLI, &read(root, CLI)),
    }
}

/// 関数の窓（宣言行の 1-based 行番号と、本文の行）。終わりは宣言行と同じ字下げの `}`
fn fn_window<'a>(src: &'a str, needle: &str) -> Option<(usize, Vec<&'a str>)> {
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
    Some((start + 1, lines[start..=end].to_vec()))
}

fn is_code(line: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with("//") && !t.is_empty()
}

fn window_has(window: &[&str], needle: &str) -> bool {
    window.iter().any(|l| is_code(l) && l.contains(needle))
}

/// 窓の中で `needle` を含むコード行の 1-based 行番号（すべて）
fn lines_with(at: usize, window: &[&str], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| is_code(l) && l.contains(needle))
        .map(|(i, _)| at + i)
        .collect()
}

/// 1. 起動の入口（CLI / `plan`）が共有の読み込みを通り、自前の握りつぶしを持たない
fn scan_entry(
    file: &'static str,
    src: &str,
    fn_needle: &str,
    loader: &str,
    out: &mut Vec<Offender>,
) {
    let Some((at, window)) = fn_window(src, fn_needle) else {
        out.push(Offender {
            file,
            line: 0,
            why: format!("`{fn_needle}` が見つからない（走査が空振り）"),
        });
        return;
    };
    if !window_has(&window, loader) {
        out.push(Offender {
            file,
            line: at,
            why: format!(
                "`{loader}` を通っていない = CLI / スマホ / 返答の配送で読み方が食い違う（#2227）"
            ),
        });
    }
    for bad in ["Profile::load(", "Profile::default()"] {
        for line in lines_with(at, &window, bad) {
            out.push(Offender {
                file,
                line,
                why: format!(
                    "起動の入口が `{bad}` を直接使っている = 読めない default.yaml を黙って \
                     組み込み既定へ落とす修正前の形（#2227）"
                ),
            });
        }
    }
    if !window_has(&window, "with_startup_warning(") {
        out.push(Offender {
            file,
            line: at,
            why: "system prompt に起動時の警告を添えていない = 全画面の TUI に隠れた `警告:` 行の \
                  ほかに master 本人へ届く道が無い（#2227）"
                .into(),
        });
    }
}

/// 2. / 3. / 5. 共有の読み込み（`master_launch.rs`）
fn scan_launch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    scan_entry(LAUNCH, src, "pub fn plan(", "load_profile(", &mut out);
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: LAUNCH,
            line,
            why,
        })
    };

    match fn_window(src, "pub fn load_default_at(") {
        None => push(0, "`load_default_at` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "project_profile_base_at(") {
                push(
                    at,
                    "default.yaml を継承元と同じ読み方（`project_profile_base_at` = 退避つき）で \
                     読んでいない = 壊れた中身が保全されない（#916 / #2227）"
                        .into(),
                );
            }
            if !window_has(&window, "warning: Some(") {
                push(
                    at,
                    "読めないときに warning を持たせていない = 黙って組み込み既定で立つ（#2227）"
                        .into(),
                );
            }
            for (i, line) in window.iter().enumerate() {
                if !is_code(line) {
                    continue;
                }
                if let Some(bad) = SWALLOWS.iter().find(|s| line.contains(**s)) {
                    push(
                        at + i,
                        format!(
                            "default.yaml の読み込みで `{bad}` を使っている = 読めない理由を \
                             握りつぶす（#2227）"
                        ),
                    );
                }
            }
        }
    }

    match fn_window(src, "pub fn load_profile(") {
        None => push(0, "`load_profile` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "load_default_at(") {
                push(
                    at,
                    "default の読み込みが `load_default_at` を通っていない（#2227）".into(),
                );
            }
            if !window_has(&window, "missing_project_profile_reason(") {
                push(
                    at,
                    "`-<key>` の「見つからない」に #1825 の理由と直し方を足していない（#2227）"
                        .into(),
                );
            }
            if lines_with(at, &window, "persist_log(").len() < 2 {
                push(
                    at,
                    "default の警告と `-<key>` の理由の両方を persist.log へ残していない（#2227）"
                        .into(),
                );
            }
        }
    }

    // 5. 応答（スマホの「+ master」）に warnings を載せる
    match fn_window(src, "pub fn to_json(") {
        None => push(0, "`to_json` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "\"warnings\"") {
                push(
                    at,
                    "起動の応答に `warnings` が無い = スマホから立てた master の警告が画面へ出ない \
                     （#2227）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 4. 理由の判定（`orchestrator/mod.rs`）
fn scan_orchestrator(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: ORCH,
            line,
            why,
        })
    };
    match fn_window(src, "pub fn missing_project_profile_reason(") {
        None => push(
            0,
            "`missing_project_profile_reason` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "check_project_profile_base()") {
                push(
                    at,
                    "継承元を `check_project_profile_base` で見ていない = `migrate status` / \
                     `projects add` と判定が食い違う（#1825 / #2227）"
                        .into(),
                );
            }
        }
    }
    match fn_window(src, "fn missing_project_profile_reason_from(") {
        None => push(
            0,
            "`missing_project_profile_reason_from` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "not_inherited_reason(") {
                push(
                    at,
                    "理由文が #1825 の `not_inherited_reason` を通っていない = `projects add` と \
                     直し方の文が食い違う（#2227）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 1. / 5. CLI（`tako master`）
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    scan_entry(
        CLI,
        src,
        "fn orchestrator_master(",
        "master_launch::load_profile(",
        &mut out,
    );
    if let Some((at, window)) = fn_window(src, "fn orchestrator_master(") {
        if !window_has(&window, "eprintln!(\"警告: {w}\")") {
            out.push(Offender {
                file: CLI,
                line: at,
                why: "読み込みの warning を `警告:` 行で画面へ出していない（#2227）".into(),
            });
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    scan_launch(&s.launch)
        .iter()
        .chain(scan_orchestrator(&s.orch).iter())
        .chain(scan_cli(&s.cli).iter())
        .map(Offender::report)
        .collect()
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for (rel, src, needle) in [
        (LAUNCH, &s.launch, "pub fn plan("),
        (LAUNCH, &s.launch, "pub fn load_profile("),
        (LAUNCH, &s.launch, "pub fn load_default_at("),
        (LAUNCH, &s.launch, "pub fn to_json("),
        (ORCH, &s.orch, "pub fn missing_project_profile_reason("),
        (ORCH, &s.orch, "fn missing_project_profile_reason_from("),
        (CLI, &s.cli, "fn orchestrator_master("),
    ] {
        assert!(
            fn_window(src, needle).is_some(),
            "{rel}: `{needle}` の窓が採れない（本番コードの範囲取りが壊れている）"
        );
    }
}

#[test]
fn 現行実装は違反ゼロ() {
    let found = all(&sources(&workspace_root()));
    assert!(
        found.is_empty(),
        "読めない default.yaml で master を黙って立てない（#2227）の不変条件に違反: {found:#?}"
    );
}

/// 関数の窓の中でだけ `from` を `to` へ置き換える（ファイル全体を置換すると
/// 同じ綴りの別の関数まで壊れて「何を測ったか」が濁る）
fn replace_in_fn(src: &str, fn_needle: &str, from: &str, to: &str) -> String {
    let Some((at, window)) = fn_window(src, fn_needle) else {
        panic!("`{fn_needle}` の窓が採れない");
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = at - 1;
    let end = start + window.len();
    let body = window.join("\n");
    let replaced = body.replacen(from, to, 1);
    assert_ne!(
        replaced, body,
        "`{fn_needle}` の中に注入の対象が無い: {from}"
    );
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.push(replaced);
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// 注入の 1 件（どのファイルの・どの関数の中で・何を何へ・名指されるべき行の目印）
struct Injection {
    rel: &'static str,
    fn_needle: &'static str,
    from: &'static str,
    to: &'static str,
    marker: &'static str,
}

/// 注入して、注入した関数の中の `marker` の行が file:line で名指しされることを確かめる
fn assert_named(base: &Sources, inj: &Injection) {
    let mut injected = base.clone();
    let target = match inj.rel {
        LAUNCH => &mut injected.launch,
        ORCH => &mut injected.orch,
        CLI => &mut injected.cli,
        other => panic!("未知のファイル: {other}"),
    };
    *target = replace_in_fn(target, inj.fn_needle, inj.from, inj.to);
    let (at, window) = fn_window(target, inj.fn_needle).expect("注入後も窓が採れる");
    let line = window
        .iter()
        .position(|l| l.contains(inj.marker))
        .map(|i| at + i)
        .unwrap_or_else(|| panic!("`{}` が注入後の窓に無い", inj.marker));
    let expected = format!("{}:{line} ", inj.rel);
    let found = all(&injected);
    assert!(
        found.iter().any(|o| o.starts_with(&expected)),
        "注入（{} → {}）を {}:{line} で名指しできていない: {found:#?}",
        inj.from,
        inj.to,
        inj.rel
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let s = sources(&workspace_root());
    for inj in [
        // 注入 1: plan が修正前そのもの（自前で読んで default を組み込み既定へ握りつぶす）
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn plan(",
            from: "let LaunchProfile { profile, warning } = load_profile(profile_name)?;",
            to: "let (profile, warning) = (Profile::load(profile_name).unwrap_or_else(|_| Profile::default()), None::<String>);",
            marker: "Profile::load(profile_name)",
        },
        // 注入 2: CLI が修正前そのもの
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_master(",
            from: "} = orchestrator::master_launch::load_profile(profile_name)?;",
            to: "} = orchestrator::master_launch::LaunchProfile { profile: orchestrator::Profile::load(profile_name).unwrap_or_else(|_| orchestrator::Profile::default()), warning: None };",
            marker: "orchestrator::Profile::load(profile_name)",
        },
        // 注入 3: CLI が警告を画面へ出さない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_master(",
            from: "eprintln!(\"警告: {w}\");",
            to: "let _ = w;",
            marker: "fn orchestrator_master(",
        },
        // 注入 4: CLI が master 本人の prompt へ警告を添えない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_master(",
            from: "orchestrator::master_launch::with_startup_warning(",
            to: "std::convert::identity((",
            marker: "fn orchestrator_master(",
        },
        // 注入 5: default の読み込みが理由を握りつぶす
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_default_at(",
            from: "match orchestrator::project_profile_base_at(path) {",
            to: "match Ok::<Profile, String>(orchestrator::project_profile_base_at(path).unwrap_or_default()) {",
            marker: "project_profile_base_at(path).unwrap_or_default()",
        },
        // 注入 6: `-<key>` の「見つからない」に理由を足さない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_profile(",
            from: "orchestrator::missing_project_profile_reason(name)",
            to: "None::<String>",
            marker: "pub fn load_profile(",
        },
        // 注入 7: スマホの応答に warnings を載せない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn to_json(",
            from: "\"warnings\": self.warning.iter().collect::<Vec<_>>(),",
            to: "",
            marker: "pub fn to_json(",
        },
        // 注入 8: 理由の判定が継承元を見ない（`migrate status` / `projects add` と食い違う）
        Injection {
            rel: ORCH,
            fn_needle: "pub fn missing_project_profile_reason(",
            from: "check_project_profile_base(),",
            to: "Ok::<(), String>(()),",
            marker: "pub fn missing_project_profile_reason(",
        },
    ] {
        assert_named(&s, &inj);
    }
}
