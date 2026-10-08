//! tako mod（FR-2.42 / #1879）の番犬
//!
//! # なぜ要るか
//!
//! tako mod は**利用者の Claude Code の中で動く**ので、壊れ方が利用者の作業へ直に出る:
//! ゲートになるフック（`tool.call` / `tool.check` / `classic.PermissionRequest` 等）から
//! `.catch` を外すと、mod が投げた瞬間にツール呼び出しや権限ダイアログの判断が mod の失敗に
//! 引きずられる。報告に `text` や `tool_input` を 1 つ足すだけで、会話の本文やコマンドの引数が
//! tako のメモリと `tako mod` の出力へ流れる（AGENTS.md の絶対ルール）。どちらも CLI の応答の
//! 形は変わらないので、挙動の単体テストでは気づけない。ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. `register.ts` のすべての `on(...)` の登録に `.catch(($, e, next) => next(e))` が付く
//!    （判断を奪わず、失敗したら下へ流す = 設計書 §5）。ストリームのフック（`turn.step`。#1880）は
//!    async generator でしか書けないので、`.catch(async function* ($, e, next) {` +
//!    `return yield* next(e)`（= 素通し）を同じ意味の形として認める
//! 2. 報告に本文系のキーが無い: `register.ts` の `buildReport` の組み立て・`types/index.d.ts` の
//!    契約・Rust の `ModReport` のフィールドのどれにも [`BODY_KEYS`] が出ない
//! 3. MCP の `tako_mod` は `report` を受け付けない（AI が自分の状態を偽れるだけ = FR-2.42）
//! 4. 閉じたペインの報告を捨てる後始末が close の 3 経路すべてにある
//! 5. 注入はペインの env だけ: `spawn_session` が `claude_mod::pane_env(` を通し、`.app` の組み立て
//!    （`scripts/build-app.sh`）は mod を同梱先へ置かない（読み込みのたびに型定義が書かれ bundle が
//!    改変される = 設計書 §1.6）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const REGISTER: &str = "crates/tako-core/claude-mod/hooks/register.ts";
const TYPES: &str = "crates/tako-core/claude-mod/types/index.d.ts";
const CORE: &str = "crates/tako-core/src/claude_mod.rs";
const CONTROL: &str = "crates/tako-control/src/claude_mod.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const BUILD_APP: &str = "scripts/build-app.sh";

/// 報告に載せてはならないキー（会話の本文・プロンプト・ツールの引数）
const BODY_KEYS: &[&str] = &[
    "text",
    "answer",
    "prompt",
    "message",
    "messages",
    "content",
    "tool_input",
    "input",
    "command",
    "args",
    "arguments",
    "body",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// 失敗したら下へ流す `.catch` の形（ふつうのフック）
const CATCH_PLAIN: &str = ".catch(($, e, next) => next(e))";
/// 同じ意味のストリームのフック用の形（#1880。`turn.step` は async generator でしか書けない）
const CATCH_STREAM: &str = ".catch(async function* ($, e, next) {\n    return yield* next(e)\n  })";

/// `register.ts` の登録（`  on('…'` で始まる行から次の登録の手前まで）ごとに `.catch` を見る
fn scan_catch(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("on('"))
        .map(|(i, _)| i)
        .collect();
    let mut out = Vec::new();
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(lines.len());
        let block = lines[start..end].join("\n");
        if !block.contains(CATCH_PLAIN) && !block.contains(CATCH_STREAM) {
            out.push(report(
                REGISTER,
                start + 1,
                "登録に `.catch(($, e, next) => next(e))` が無い（mod が投げるとツール呼び出しや権限の判断が mod の失敗に引きずられる）",
            ));
        }
    }
    out
}

/// 窓の中に本文系のキーが**キーとして**出るか（`key:` / `key?:` / `"key"` の形）
fn body_keys_in(window: &str, base_line: usize, file: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, line) in window.lines().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        for key in BODY_KEYS {
            let as_key = [
                format!(" {key}:"),
                format!(" {key}?:"),
                format!("\"{key}\""),
                format!("pub {key}:"),
            ];
            if as_key.iter().any(|k| code.contains(k.as_str())) {
                out.push(report(
                    file,
                    base_line + i,
                    &format!("報告に本文系のキー `{key}` がある（会話の本文・プロンプト・ツールの引数を tako へ流さない）"),
                ));
            }
        }
    }
    out
}

/// 窓（`needle` を含む行から、同じ字下げの閉じまで）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

fn scan_body_keys(register: &str, types: &str, core: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, "async function buildReport(", "}") {
        Some((at, w)) => out.extend(body_keys_in(&w, at, REGISTER)),
        None => out.push(report(
            REGISTER,
            1,
            "buildReport が見つからない（走査が空振り）",
        )),
    }
    match window(types, "export type TakoModReport = {", "}") {
        Some((at, w)) => out.extend(body_keys_in(&w, at, TYPES)),
        None => out.push(report(
            TYPES,
            1,
            "TakoModReport が見つからない（走査が空振り）",
        )),
    }
    match window(core, "pub struct ModReport {", "}") {
        Some((at, w)) => out.extend(body_keys_in(&w, at, CORE)),
        None => out.push(report(CORE, 1, "ModReport が見つからない（走査が空振り）")),
    }
    out
}

fn scan_mcp(control: &str) -> Vec<String> {
    let Some(at) = control
        .lines()
        .position(|l| l.starts_with("pub const MCP_ACTIONS"))
    else {
        return vec![report(
            CONTROL,
            1,
            "MCP_ACTIONS が見つからない（走査が空振り）",
        )];
    };
    let line = control.lines().nth(at).unwrap_or_default();
    if line.contains("\"report\"") {
        return vec![report(
            CONTROL,
            at + 1,
            "MCP の tako_mod が report を受け付ける（AI が自分の状態を偽れるだけ = FR-2.42）",
        )];
    }
    Vec::new()
}

fn scan_main(main: &str) -> Vec<String> {
    let mut out = Vec::new();
    // close の 3 経路は送達の顛末の後始末（drop_prompt_delivery_state）を必ず呼ぶので、
    // その呼び出しごとに mod の後始末が隣にあるかを見る
    let lines: Vec<&str> = main.lines().collect();
    let mut paths = 0;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("self.drop_prompt_delivery_state(") {
            continue;
        }
        paths += 1;
        let near = lines[i..lines.len().min(i + 3)].join("\n");
        if !near.contains("self.drop_claude_mod_state(") {
            out.push(report(
                MAIN,
                i + 1,
                "close の経路に mod の後始末（drop_claude_mod_state）が無い（ペイン ID の再利用で前任の報告を名乗る）",
            ));
        }
    }
    if paths != 3 {
        out.push(report(
            MAIN,
            1,
            &format!("close の経路が 3 つではない（{paths} 本。数え方を見直す）"),
        ));
    }
    match window(main, "    fn spawn_session(", "    }") {
        Some((at, w)) if !w.contains("claude_mod::pane_env(") => out.push(report(
            MAIN,
            at,
            "spawn_session が claude_mod::pane_env を通していない（注入の判断を 1 実装へ寄せる）",
        )),
        Some(_) => {}
        None => out.push(report(
            MAIN,
            1,
            "spawn_session が見つからない（走査が空振り）",
        )),
    }
    out
}

fn scan_build_app(src: &str) -> Vec<String> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with('#') && l.contains("claude-mod"))
        .map(|(i, _)| {
            report(
                BUILD_APP,
                i + 1,
                "mod を .app の中へ置いている（Claude Code が読むたびに型定義を書き、署名済みの bundle が改変される）",
            )
        })
        .collect()
}

struct Sources {
    register: String,
    types: String,
    core: String,
    control: String,
    main: String,
    build_app: String,
}

fn current() -> Sources {
    Sources {
        register: read(REGISTER),
        types: read(TYPES),
        core: read(CORE),
        control: read(CONTROL),
        main: read(MAIN),
        build_app: read(BUILD_APP),
    }
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_catch(&s.register);
    out.extend(scan_body_keys(&s.register, &s.types, &s.core));
    out.extend(scan_mcp(&s.control));
    out.extend(scan_main(&s.main));
    out.extend(scan_build_app(&s.build_app));
    out
}

#[test]
fn tako_modの構造が契約どおり() {
    let found = all(&current());
    assert!(
        found.is_empty(),
        "契約から外れた箇所:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    let registrations = s
        .register
        .lines()
        .filter(|l| l.trim_start().starts_with("on('"))
        .count();
    assert!(
        registrations >= 7,
        "on(...) の登録が {registrations} 本しか採れない"
    );
    for (src, needle) in [
        (&s.register, "async function buildReport("),
        (&s.types, "export type TakoModReport = {"),
        (&s.core, "pub struct ModReport {"),
        (&s.main, "    fn spawn_session("),
    ] {
        assert!(src.contains(needle), "{needle} が見つからない");
    }
    // 報告の組み立てには実際のキーが並んでいる（窓が空でない）
    let (_, w) = window(&s.register, "async function buildReport(", "}").unwrap();
    assert!(
        w.contains("pending_tool:") && w.contains("rate_limits:"),
        "{w}"
    );
}

fn assert_named(found: &[String], file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入を名指しできない（{file} / {why}）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let base = current();
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. .catch を外す（tool.call の登録だけ）
    let found = mutate(&|s| {
        let at = s.register.find("on('tool.call'").unwrap();
        let rest = &s.register[at..];
        let catch = rest.find(".catch(($, e, next) => next(e))").unwrap();
        s.register.replace_range(
            at + catch..at + catch + ".catch(($, e, next) => next(e))".len(),
            "",
        );
    });
    assert_named(&found, REGISTER, ".catch");
    // 1b. ストリームのフック（turn.step。#1880）の素通しを外す
    let found = mutate(&|s| {
        assert!(
            s.register.contains(CATCH_STREAM),
            "turn.step の素通しが見つからない"
        );
        s.register = s.register.replacen(
            CATCH_STREAM,
            ".catch(async function* () {\n    return undefined\n  })",
            1,
        );
    });
    assert_named(&found, REGISTER, ".catch");
    // 2. 報告に本文を足す（mod / 契約 / Rust の 3 か所それぞれ）
    let found = mutate(&|s| {
        s.register = s
            .register
            .replacen("    turn,\n", "    turn,\n    text: lastText,\n", 1);
    });
    assert_named(&found, REGISTER, "`text`");
    let found = mutate(&|s| {
        s.types = s.types.replacen(
            "  turn: TakoTurn\n",
            "  turn: TakoTurn\n  tool_input?: unknown\n",
            1,
        );
    });
    assert_named(&found, TYPES, "`tool_input`");
    let found = mutate(&|s| {
        s.core = s.core.replacen(
            "    pub turn: ModTurn,\n",
            "    pub turn: ModTurn,\n    pub prompt: Option<String>,\n",
            1,
        );
    });
    assert_named(&found, CORE, "`prompt`");
    // 3. MCP に report を載せる
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "pub const MCP_ACTIONS: &[&str] = &[\"status\", \"on\", \"off\", \"band-on\", \"band-off\"];",
            "pub const MCP_ACTIONS: &[&str] = &[\"status\", \"on\", \"off\", \"band-on\", \"band-off\", \"report\"];",
            1,
        );
    });
    assert_named(&found, CONTROL, "report");
    // 4. close の経路から後始末を 1 つ外す
    let found = mutate(&|s| {
        s.main = s
            .main
            .replacen("        self.drop_claude_mod_state(pane);\n", "", 1);
    });
    assert_named(&found, MAIN, "drop_claude_mod_state");
    // 5. 注入を spawn_session の外へ / .app へ同梱
    let found = mutate(&|s| {
        s.main = s
            .main
            .replacen("claude_mod::pane_env(", "claude_mod::pane_env_inline(", 1);
    });
    assert_named(&found, MAIN, "pane_env");
    let found = mutate(&|s| {
        s.build_app
            .push_str("\ncp -R crates/tako-core/claude-mod \"$APP/Contents/Resources/\"\n");
    });
    assert_named(&found, BUILD_APP, ".app");
    // 注入の前の状態は綺麗（上の名指しは注入が原因）
    assert!(all(&base).is_empty());
}
