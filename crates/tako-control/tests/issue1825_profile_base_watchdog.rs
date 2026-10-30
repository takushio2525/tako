//! 専用プロファイルの継承元（`profiles/default.yaml`）を黙って既定へ落とさない番犬（#1825）
//!
//! # なぜ要るか
//!
//! `projects add` / `adopt` / 起動時の差分検出（#1453）が作る専用プロファイルは、
//! default.yaml の個人ルール（`prompt_blocks`）・モデルを継いで作る。継承元の読み込みが
//! `unwrap_or_default()` のような**握りつぶし**へ戻ると、壊れた default.yaml のまま
//! 個人ルールの抜けたプロファイルが「default から継承」と表示されて作られる（修正前の実測）。
//! 生成は既存に触らない（冪等）ので、default.yaml を直しても**二度と追いつかない**。
//! 壊れ方が静かで、次に master を立ててルールが効いていないと気づくまで誰にも見えない。
//!
//! # 何を縛るか
//!
//! 1. 継承元を読む 3 関数（`project_profile_base` / `project_profile_base_at` /
//!    `read_profile_base_at`）が読めない内容を既定へ落とさない（無いファイルだけは既定）
//! 2. 読めないときは退避の共通口（`quarantine_unreadable`。#916 / #1819）を通る
//! 3. `ensure_project_profile` が継承元の `Err` を握りつぶさず `NotInherited` で返す
//! 4. `projects add` の dispatch が理由を応答の `warnings` と persist.log へ出す
//! 5. `adopt` が継承できなかった理由で止まる（「見つからない」に化けさせない）
//! 6. `tako migrate status` の予告が継承元を見る（`run` と食い違わない）
//! 7. CLI が応答の `warnings` を表示する
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**がそれぞれ注入した行の file:line で名指しされることを確かめる。
//! 範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MIGRATIONS: &str = "crates/tako-control/src/migrations.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// 継承元の読み込みで**使ってはいけない**握りつぶし
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

struct Sources {
    orch: String,
    dispatch: String,
    migrations: String,
    cli: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        orch: production(ORCH, &read(root, ORCH)),
        dispatch: production(DISPATCH, &read(root, DISPATCH)),
        migrations: production(MIGRATIONS, &read(root, MIGRATIONS)),
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

/// 窓の中で `needle` を含む最初のコード行の 1-based 行番号
fn line_with(at: usize, window: &[&str], needle: &str) -> Option<usize> {
    window
        .iter()
        .position(|l| is_code(l) && l.contains(needle))
        .map(|i| at + i)
}

fn window_has(window: &[&str], needle: &str) -> bool {
    window.iter().any(|l| is_code(l) && l.contains(needle))
}

/// 1. / 2. / 3. 継承元の読み込みと生成（`orchestrator/mod.rs`）
fn scan_orchestrator(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: ORCH,
            line,
            why,
        })
    };

    for needle in [
        "pub fn project_profile_base(",
        "pub fn project_profile_base_at(",
        "pub fn profile_base_at(",
        "fn read_profile_base_at(",
    ] {
        let Some((at, window)) = fn_window(src, needle) else {
            push(0, format!("`{needle}` が見つからない（走査が空振り）"));
            continue;
        };
        if !window[0].contains("Result<") {
            push(
                at,
                format!(
                    "`{needle}` が `Result` を返さない = 読めない default.yaml を呼び手へ \
                     知らせる口が無い（#1825）"
                ),
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
                        "継承元の読み込みで `{bad}` を使っている = 読めない default.yaml を \
                         既定へ黙って落とし、個人ルールの抜けたプロファイルを作る（#1825）"
                    ),
                );
            }
            // 既定へ落としてよいのは「無い」ときだけ（直前 2 行に NotFound の腕がある）
            if line.contains("default_profile()") || line.contains("Profile::default()") {
                let guarded = window[i.saturating_sub(2)..i]
                    .iter()
                    .any(|l| l.contains("ErrorKind::NotFound"));
                if !guarded {
                    push(
                        at + i,
                        "「無い」以外で組み込み既定を返している = 読めない default.yaml を \
                         既定扱いにしている（#1825）"
                            .into(),
                    );
                }
            }
        }
    }

    // 2. 読めない中身は退避の共通口を通す（本体は種別つきの `profile_base_at`。#2233 で
    //    `tako solo` の default と共有するため切り出した。継承元はそこへ委ねる）
    if let Some((at, window)) = fn_window(src, "pub fn profile_base_at(") {
        if !window_has(&window, "quarantine_unreadable(") {
            push(
                at,
                "読めない default.yaml を退避の共通口（`quarantine_unreadable`）へ通していない \
                 = 中身が保全されない（#916 / #1819 / #1825）"
                    .into(),
            );
        }
    }
    if let Some((at, window)) = fn_window(src, "pub fn project_profile_base_at(") {
        // `read_profile_base_at(` も部分一致で当たるので、退避つきの本体の呼び出しだけを数える
        let delegates = window.iter().any(|l| {
            is_code(l)
                && l.contains("profile_base_at(ProfileKind::Master")
                && !l.contains("read_profile_base_at(")
        });
        if !delegates {
            push(
                at,
                "継承元が退避つきの本体（`profile_base_at`）を通っていない = 読めない中身が \
                 保全されない（#1825 / #2233）"
                    .into(),
            );
        }
    }

    // 3. 生成は継承元の Err を NotInherited で返す
    match fn_window(src, "pub fn ensure_project_profile(") {
        None => push(
            0,
            "`ensure_project_profile` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => match line_with(at, &window, "project_profile_base()") {
            None => push(
                at,
                "継承元（`project_profile_base()`）を通っていない（#1453 / #1825）".into(),
            ),
            Some(line) => {
                let call = window[line - at];
                if let Some(bad) = SWALLOWS.iter().find(|s| call.contains(**s)) {
                    push(
                        line,
                        format!(
                            "継承元の `Err` を `{bad}` で握りつぶしている = 壊れた default.yaml \
                             で個人ルールの抜けたプロファイルを作る（#1825）"
                        ),
                    );
                }
                if !window_has(&window, "ProfileGen::NotInherited") {
                    push(
                        line,
                        "継承元を読めないときに `NotInherited` を返していない = 理由が \
                         warnings / persist.log へ届かない（#1825）"
                            .into(),
                    );
                }
            }
        },
    }
    out
}

/// 4. / 5. dispatch（`projects add` と `adopt`）
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };

    match fn_window(src, "pub fn dispatch_orchestrator_projects(") {
        None => push(
            0,
            "`dispatch_orchestrator_projects` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let gen_line = line_with(at, &window, "ensure_project_profile(&key)").unwrap_or(at);
            for (needle, why) in [
                (
                    // rustfmt が `gen` と `.warning()` を別の行へ割るので呼び出しだけを見る
                    ".warning()",
                    "継承できなかった理由（`gen.warning()`）を拾っていない",
                ),
                (
                    "\"warnings\": warnings",
                    "応答に `warnings` を載せていない = MCP の呼び手に届かない",
                ),
                ("persist_log(", "persist.log へ残していない"),
            ] {
                if !window_has(&window, needle) {
                    push(gen_line, format!("`projects add` が {why}（#1825）"));
                }
            }
        }
    }

    match fn_window(src, "fn dispatch_orchestrator_adopt(") {
        None => push(
            0,
            "`dispatch_orchestrator_adopt` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let gen_line = line_with(at, &window, "ensure_project_profile(name)");
            let check = line_with(at, &window, "gen.warning()");
            // 採用先の読み込みは表示言語で断る型の口（#2365 の `Profile::load_typed`）
            let load = line_with(at, &window, "Profile::load_typed(name)");
            match (gen_line, check, load) {
                (Some(_), Some(c), Some(l)) if c < l => {}
                (g, _, _) => push(
                    g.unwrap_or(at),
                    "`adopt` が継承できなかった理由（`gen.warning()`）で止まらない = \
                     「プロファイルが見つからない」に化けて直し方が届かない（#1825）"
                        .into(),
                ),
            }
        }
    }
    out
}

/// 6. `tako migrate status` の予告
fn scan_migrations(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn project_profile_reports(") {
        None => out.push(Offender {
            file: MIGRATIONS,
            line: 0,
            why: "`project_profile_reports` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !window_has(&window, "check_project_profile_base()") {
                out.push(Offender {
                    file: MIGRATIONS,
                    line: line_with(at, &window, "Mode::Check").unwrap_or(at),
                    why: "確認モードが継承元を見ていない = status は「作る」と予告し run は \
                          作らない、の食い違い（#1825）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 7. CLI が警告を出す
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn orchestrator_projects_cli(") {
        None => out.push(Offender {
            file: CLI,
            line: 0,
            why: "`orchestrator_projects_cli` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !window_has(&window, "res[\"warnings\"]") {
                out.push(Offender {
                    file: CLI,
                    line: line_with(at, &window, "ProjectsCommand::Add").unwrap_or(at),
                    why: "`projects add` の CLI が応答の `warnings` を表示しない = 継承できずに \
                          作らなかったことが端末に出ない（#1825）"
                        .into(),
                });
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    scan_orchestrator(&s.orch)
        .iter()
        .chain(scan_dispatch(&s.dispatch).iter())
        .chain(scan_migrations(&s.migrations).iter())
        .chain(scan_cli(&s.cli).iter())
        .map(Offender::report)
        .collect()
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for (rel, src, needle) in [
        (ORCH, &s.orch, "pub fn project_profile_base("),
        (ORCH, &s.orch, "pub fn project_profile_base_at("),
        (ORCH, &s.orch, "fn read_profile_base_at("),
        (ORCH, &s.orch, "pub fn ensure_project_profile("),
        (
            DISPATCH,
            &s.dispatch,
            "pub fn dispatch_orchestrator_projects(",
        ),
        (DISPATCH, &s.dispatch, "fn dispatch_orchestrator_adopt("),
        (MIGRATIONS, &s.migrations, "fn project_profile_reports("),
        (CLI, &s.cli, "fn orchestrator_projects_cli("),
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
        "継承元を黙って既定へ落とさない（#1825）の不変条件に違反: {found:#?}"
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
    let mut injected = Sources {
        orch: base.orch.clone(),
        dispatch: base.dispatch.clone(),
        migrations: base.migrations.clone(),
        cli: base.cli.clone(),
    };
    let target = match inj.rel {
        ORCH => &mut injected.orch,
        DISPATCH => &mut injected.dispatch,
        MIGRATIONS => &mut injected.migrations,
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
        // 注入 1: 修正前そのもの（生成が継承元の Err を握りつぶして組み込み既定で作る）
        Injection {
            rel: ORCH,
            fn_needle: "pub fn ensure_project_profile(",
            from: "match project_profile_base() {",
            to: "match Ok::<Profile, String>(project_profile_base().unwrap_or_default()) {",
            marker: "project_profile_base().unwrap_or_default()",
        },
        // 注入 2: 読み込み本体が解釈の失敗を既定へ落とす
        Injection {
            rel: ORCH,
            fn_needle: "fn read_profile_base_at(",
            from: "serde_yaml::from_str(&text).map_err(|e| tako_core::load_issue::yaml_error_reason(&e, &text))",
            to: "Ok(serde_yaml::from_str(&text).unwrap_or_default())",
            marker: "serde_yaml::from_str(&text).unwrap_or_default()",
        },
        // 注入 3: 読めない中身を退避しない
        Injection {
            rel: ORCH,
            fn_needle: "pub fn profile_base_at(",
            from: "quarantine_unreadable(",
            to: "quarantine_slot_hint(",
            marker: "pub fn profile_base_at(",
        },
        // 注入 3b: 継承元が退避つきの本体を通らない（読むだけの判定で済ませる）
        Injection {
            rel: ORCH,
            fn_needle: "pub fn project_profile_base_at(",
            from: "profile_base_at(ProfileKind::Master, path)",
            to: "read_profile_base_at(ProfileKind::Master, path)",
            marker: "pub fn project_profile_base_at(",
        },
        // 注入 4: projects add が warnings を応答へ載せない
        Injection {
            rel: DISPATCH,
            fn_needle: "pub fn dispatch_orchestrator_projects(",
            from: "\"warnings\": warnings,",
            to: "",
            marker: "let gen = orchestrator::ensure_project_profile(&key);",
        },
        // 注入 5: adopt が理由で止まらない（「見つからない」に化ける）
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_adopt(",
            from: "if let Some(w) = gen.warning() {",
            to: "if let Some(w) = None::<&str> {",
            marker: "let gen = orchestrator::ensure_project_profile(name);",
        },
        // 注入 6: migrate status の予告が継承元を見ない
        Injection {
            rel: MIGRATIONS,
            fn_needle: "fn project_profile_reports(",
            from: "orchestrator::check_project_profile_base()",
            to: "Ok::<(), String>(())",
            marker: "if mode == Mode::Check {",
        },
        // 注入 7: CLI が warnings を出さない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_projects_cli(",
            from: "res[\"warnings\"]",
            to: "res[\"warnings_unused\"]",
            marker: "ProjectsCommand::Add {",
        },
    ] {
        assert_named(&s, &inj);
    }
}
