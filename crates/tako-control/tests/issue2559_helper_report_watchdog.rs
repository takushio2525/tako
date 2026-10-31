//! GUI がペインの外で起こす claude（自動命名の `claude -p`）の mod の報告が、受け継いだペインの
//! 報告として記録されないことの番犬（#2559）
//!
//! # なぜ要るか
//!
//! 10/31、master のペインの `tako_orchestrator_self` が `mod_turn.model = claude-haiku-4-5`・呼ぶたびに
//! 別の `session_id`・別アカウントの `rate_limits` を返した。master のペインから再起動された GUI は
//! 自分の env にそのペインの `TAKO_PANE_ID` と mod の注入（`TAKO_CLI` / `CLAUDE_CODE_PLUGIN_DIRS`）を
//! 抱えていて、自動命名の `claude -p` がそれを受け継ぎ、mod（設定 dir の `skills/tako` からも読まれる）が
//! 起きて master のペインの報告として haiku の 1 ターンを送っていた（終わりの `ended` で報告ごと消えた）。
//!
//! # 何を縛るか
//!
//! 1. 送る側: 自動命名の子の起動形（`claude_command`）が `helper_env` の差分を当てる。
//!    `spawn_claude` はその起動形を使い、ほかに `Command::new` を置かない
//! 2. mod の起床条件（`register.ts` の `wake` が `$.env.get` で読む env）がすべて `HELPER_DROP_ENV` にある
//!    （mod が新しい env で起きるようになったら、補助の子からも外す）
//! 3. 受ける側: `accept_report` が送り主の居場所（`report_origin_of`）を、帯のトグルの取り込み
//!    （`import_band`）と記録（`.accept(`）より前に見る
//! 4. CLI の `tako mod report` が自分の祖先（`report_ancestry()`）を添える
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓を固定し、[`修正前の形へ戻すと名指しする`] で
//! **修正前を再現した注入**が `file:line` で名指しされることを確かめる。判定そのものは
//! `tako_core::claude_mod` の単体テスト（`report_origin` の全分岐）が縛る。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const AUTORENAME: &str = "crates/tako-app/src/autorename.rs";
const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const REGISTER: &str = "crates/tako-core/claude-mod/hooks/register.ts";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read_raw(root: &Path, rel: &'static str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn read(root: &Path, rel: &'static str) -> String {
    production_range::production(&read_raw(root, rel), rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
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

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言行から N 行」では隣の関数を巻き込む）
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

/// 窓の中で `needle` を含む最初の**コードの**行の位置（0-based。無ければ None）
fn code_pos(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

struct Sources {
    autorename: String,
    control: String,
    core: String,
    cli: String,
    register: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        autorename: read(root, AUTORENAME),
        control: read(root, CONTROL_MOD),
        core: read(root, CORE_MOD),
        cli: read(root, CLI),
        // TypeScript は `#[cfg(test)]` を持たないので原文のまま
        register: read_raw(root, REGISTER),
    }
}

fn missing(file: &'static str, needle: &str) -> Offender {
    Offender {
        file,
        line: 0,
        why: format!("`{needle}` が見つからない（走査の空振り）"),
    }
}

/// 1. 送る側（自動命名の子の起動形）
fn scan_autorename(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn claude_command(") else {
        out.push(missing(AUTORENAME, "fn claude_command("));
        return out;
    };
    if code_pos(&window, "claude_mod::helper_env(").is_none()
        || code_pos(&window, "env_remove(").is_none()
    {
        out.push(Offender {
            file: AUTORENAME,
            line: at,
            why: "命名の `claude -p` の起動形が `helper_env` の差分を当てていない。GUI をペインの中から\
                  起こし直すと、子が受け継いだ TAKO_PANE_ID で mod がそのペインの報告を送る（#2559）"
                .into(),
        });
    }
    let end = at + window.lines().count();
    // 起動形の外に `Command::new` を置かない（置くと env の手当てを通らない起動ができる）
    for (i, l) in src.lines().enumerate() {
        let line = i + 1;
        if (at..end).contains(&line) || l.trim_start().starts_with("//") {
            continue;
        }
        if l.contains("Command::new(") {
            out.push(Offender {
                file: AUTORENAME,
                line,
                why: "`claude_command` の外で子を起こしている（env の手当て `helper_env` を通らない。#2559）"
                    .into(),
            });
        }
    }
    match fn_window(src, "fn spawn_claude(") {
        None => out.push(missing(AUTORENAME, "fn spawn_claude(")),
        Some((at, window)) => {
            if code_pos(&window, "claude_command(").is_none() {
                out.push(Offender {
                    file: AUTORENAME,
                    line: at,
                    why: "`spawn_claude` が `claude_command` の起動形を使っていない（#2559）"
                        .into(),
                });
            }
        }
    }
    out
}

/// `HELPER_DROP_ENV` の名前（定数は同じファイルの `pub const X: &str = "…";` で引く）と宣言行
fn helper_drop_env(core: &str) -> Option<(usize, Vec<String>)> {
    let decl = "pub const HELPER_DROP_ENV: &[&str] = &[";
    let start = core.find(decl)?;
    let line = core[..start].lines().count() + 1;
    let body = &core[start + decl.len()..];
    let body = &body[..body.find("];")?];
    let names = body
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with("//"))
        .filter_map(|t| {
            if let Some(lit) = t.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                return Some(lit.to_string());
            }
            let decl = format!("pub const {t}: &str = \"");
            let at = core.find(&decl)? + decl.len();
            Some(core[at..].split('"').next()?.to_string())
        })
        .collect();
    Some((line, names))
}

/// 2. mod の起床条件がすべて補助の子から外れている
fn scan_wake_gate(core: &str, register: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((decl_line, drop)) = helper_drop_env(core) else {
        out.push(missing(CORE_MOD, "pub const HELPER_DROP_ENV"));
        return out;
    };
    let Some((at, window)) = fn_window(register, "async function wake(") else {
        out.push(missing(REGISTER, "async function wake("));
        return out;
    };
    let mut seen = 0;
    for (i, l) in window.lines().enumerate() {
        let Some(rest) = l.split("$.env.get('").nth(1) else {
            continue;
        };
        let Some(name) = rest.split('\'').next() else {
            continue;
        };
        seen += 1;
        if !drop.iter().any(|d| d == name) {
            out.push(Offender {
                file: REGISTER,
                line: at + i,
                why: format!(
                    "mod の起床条件 `{name}` が {CORE_MOD}:{decl_line} の HELPER_DROP_ENV に無い。\
                     自動命名の `claude -p` が受け継ぐと mod が起きてそのペインの報告を送る（#2559）"
                ),
            });
        }
    }
    if seen == 0 {
        out.push(Offender {
            file: REGISTER,
            line: at,
            why: "`wake` の `$.env.get` が 1 つも見えない（走査の空振り）".into(),
        });
    }
    out
}

/// 3. 受ける側（送り主の居場所を記録より前に見る）
fn scan_accept(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn accept_report(") else {
        out.push(missing(CONTROL_MOD, "fn accept_report("));
        return out;
    };
    let origin = code_pos(&window, "report_origin_of(host, ancestry)");
    let band = code_pos(&window, "import_band(");
    let store = code_pos(&window, ".accept(pane_id");
    match (origin, band, store) {
        (Some(o), Some(b), Some(s)) if o < b && o < s => {}
        (None, _, _) => out.push(Offender {
            file: CONTROL_MOD,
            line: at,
            why: "報告の受け口が送り主の居場所（`report_origin_of`）を見ていない。ペインの外の claude の\
                  報告が受け継いだペインの報告として記録される（#2559）"
                .into(),
        }),
        (Some(o), _, _) => out.push(Offender {
            file: CONTROL_MOD,
            line: at + o,
            why: "送り主の居場所を帯の取り込み・記録より後で見ている（補助の報告で ui.json や報告が動く。#2559）"
                .into(),
        }),
    }
    match fn_window(src, "fn report_origin_of(") {
        None => out.push(missing(CONTROL_MOD, "fn report_origin_of(")),
        Some((at, window)) => {
            if code_pos(&window, "core::report_origin(").is_none()
                || code_pos(&window, "child_pid()").is_none()
            {
                out.push(Offender {
                    file: CONTROL_MOD,
                    line: at,
                    why: "送り主の居場所を `core::report_origin` とペインの pty の子で決めていない（#2559）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 4. CLI が祖先を添える
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let arm = "Command::Mod(args) if args.action.as_deref() == Some(\"report\") => Request::Mod {";
    let Some(start) = src.lines().position(|l| l.contains(arm)) else {
        out.push(missing(CLI, arm));
        return out;
    };
    let block: Vec<&str> = src.lines().skip(start).take(10).collect();
    match block.iter().position(|l| l.trim_start().starts_with("ancestry:")) {
        Some(i) if block[i].contains("report_ancestry()") => {}
        Some(i) => out.push(Offender {
            file: CLI,
            line: start + i + 1,
            why: "`tako mod report` が自分の祖先（`report_ancestry()`）を添えていない。GUI がペインの外の\
                  claude の報告を見分けられない（#2559）"
                .into(),
        }),
        None => out.push(Offender {
            file: CLI,
            line: start + 1,
            why: "`tako mod report` の要求に `ancestry` が無い（#2559）".into(),
        }),
    }
    match fn_window(src, "fn report_ancestry(") {
        None => out.push(missing(CLI, "fn report_ancestry(")),
        Some((at, window)) => {
            if code_pos(&window, "ancestor_pids(").is_none() {
                out.push(Offender {
                    file: CLI,
                    line: at,
                    why: "`report_ancestry` が祖先を引いていない（#2559）".into(),
                });
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut found = scan_autorename(&s.autorename);
    found.extend(scan_wake_gate(&s.core, &s.register));
    found.extend(scan_accept(&s.control));
    found.extend(scan_cli(&s.cli));
    found.iter().map(Offender::report).collect()
}

#[test]
fn ペインの外のclaudeの報告を記録しない配線が揃っている() {
    let found = all(&sources(&workspace_root()));
    assert!(
        found.is_empty(),
        "#2559 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for (src, file, needle) in [
        (&s.autorename, AUTORENAME, "fn claude_command("),
        (&s.autorename, AUTORENAME, "fn spawn_claude("),
        (&s.control, CONTROL_MOD, "fn accept_report("),
        (&s.control, CONTROL_MOD, "fn report_origin_of("),
        (&s.cli, CLI, "fn report_ancestry("),
        (&s.register, REGISTER, "async function wake("),
    ] {
        let (_, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{file} に `{needle}` が無い（番犬が空振りする）"));
        assert!(
            window.lines().count() >= 5,
            "{file} の `{needle}` の窓が短すぎる（{} 行）",
            window.lines().count()
        );
    }
    // 外す名前の表を読めていて、mod の起床条件の 2 つを含む（0 件なら全部が無意味に緑になる）
    let (_, drop) = helper_drop_env(&s.core).expect("HELPER_DROP_ENV を読める");
    for name in ["TAKO_PANE_ID", "TAKO_CLI"] {
        assert!(
            drop.iter().any(|d| d == name),
            "{name} が読めない: {drop:?}"
        );
    }
    // 起動形の外の `Command::new` の走査が本文を見ている（窓の外にも行がある）
    assert!(
        s.autorename.lines().count() > 500,
        "{AUTORENAME} の本番の眺めが短すぎる"
    );
}

/// 注入を当てる
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    let out = base.replacen(from, to, 1);
    assert!(out != base, "注入「{what}」の対象が見つからない: {from:?}");
    out
}

/// `src` の中で `needle` を含む最初のコードの行（1-based）
fn first_line(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("`{needle}` が見つからない"))
}

fn assert_named(found: &[String], want: &str, what: &str) {
    assert!(
        found.iter().any(|o| o.starts_with(want)),
        "{what}を名指しできていない（期待 {want}）: {found:#?}"
    );
}

#[test]
fn 修正前の形へ戻すと名指しする() {
    let root = workspace_root();
    let base = sources(&root);
    assert!(all(&base).is_empty(), "注入の前は緑であること");

    // 注入 1: #2559 前の起動形（GUI の env をそのまま渡す）
    let mut s = sources(&root);
    s.autorename = inject(
        &base.autorename,
        "tako_core::claude_mod::helper_env(inherited.as_deref(), sep)",
        "Vec::<(&str, Option<String>)>::new()",
        "env の手当ての無い起動形",
    );
    let want = format!(
        "{AUTORENAME}:{}",
        first_line(&s.autorename, "fn claude_command(")
    );
    assert_named(&all(&s), &want, "env の手当ての無い起動形");

    // 注入 2: #2559 前の spawn_claude（その場で `Command::new` する）
    let mut s = sources(&root);
    s.autorename = inject(
        &base.autorename,
        "let mut command = claude_command(bin, strict);",
        "let mut command = std::process::Command::new(bin); // 注入",
        "その場で起こす spawn_claude",
    );
    let found = all(&s);
    let want = format!(
        "{AUTORENAME}:{}",
        first_line(&s.autorename, "Command::new(bin); // 注入")
    );
    assert_named(&found, &want, "起動形の外の Command::new ");
    let want = format!(
        "{AUTORENAME}:{}",
        first_line(&s.autorename, "fn spawn_claude(")
    );
    assert_named(&found, &want, "起動形を使わない spawn_claude ");

    // 注入 3: mod が新しい env で起きるようになった（補助の子から外していない）
    let mut s = sources(&root);
    s.register = inject(
        &base.register,
        "  const path = await $.env.get('TAKO_CLI')\n",
        "  const path = await $.env.get('TAKO_CLI')\n  const gate = await $.env.get('TAKO_NEW_GATE')\n",
        "新しい起床条件",
    );
    let want = format!("{REGISTER}:{}", first_line(&s.register, "TAKO_NEW_GATE"));
    assert_named(&all(&s), &want, "外していない起床条件");

    // 注入 4: #2559 前の受け口（送り主の居場所を見ない）
    let mut s = sources(&root);
    s.control = inject(
        &base.control,
        "report_origin_of(host, ancestry)",
        "core::ReportOrigin::Unknown",
        "居場所を見ない受け口",
    );
    let want = format!(
        "{CONTROL_MOD}:{}",
        first_line(&s.control, "fn accept_report(")
    );
    assert_named(&all(&s), &want, "居場所を見ない受け口");

    // 注入 5: #2559 前の CLI（祖先を添えない）
    let mut s = sources(&root);
    s.cli = inject(
        &base.cli,
        "ancestry: report_ancestry(),",
        "ancestry: None,",
        "祖先を添えない CLI",
    );
    let arm = "Command::Mod(args) if args.action.as_deref() == Some(\"report\") => Request::Mod {";
    let arm_line = first_line(&s.cli, arm);
    let found = all(&s);
    assert!(
        found.iter().any(|o| {
            o.strip_prefix(&format!("{CLI}:"))
                .and_then(|r| r.split(' ').next())
                .and_then(|n| n.parse::<usize>().ok())
                .is_some_and(|n| (arm_line..arm_line + 10).contains(&n))
        }),
        "祖先を添えない CLI を名指しできていない（期待 {CLI}:{arm_line} の腕の中）: {found:#?}"
    );
}
