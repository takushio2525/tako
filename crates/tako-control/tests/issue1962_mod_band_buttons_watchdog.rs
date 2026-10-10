//! tako mod S7-3（#1962。FR-2.42.34〜）の番犬: 帯を next で包む・ボタン・バー・ステータスバーの区画
//!
//! # なぜ要るか
//!
//! tako mod は利用者の Claude Code の中で、**利用者が入れた他の mod と同じ描画の鎖**に並ぶ。帯の
//! フックが `next(e)` を呼ばずに自分の行だけを返すと、内側の他の mod の帯が tako のペインで黙って
//! 消える（S3 の実装がそうだった = 設計書 §9.5 の実測）。`claude plugin test` の個々の期待値は
//! 「tako の行が出る」だけでも通るので、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 帯（`AbovePrompt`）のフックの `return` は `next(e)`（譲る）・`stackBand(`（包む）・A/B の
//!    `drawBandS3(`（`band_style === 's3'` の行だけ）のどれか。`stackBand` は `next` の答えを上に
//!    残し、エンジンだけ（`type === 'engine'`）なら tako の行だけを返す
//! 2. 入力欄の下の行（`PromptHint`）のフックは必ず `next` へ流し、他の mod が足した `tail` の後ろへ
//!    足す（`hint` は書き換えない）。tail を描かない desktop では描かない
//! 3. ボタンの押下は語彙どおりの 4 経路（`$.command.run(` / `$.prompt.fill(` / tako が組んだ argv を
//!    `$.process.run(` へそのまま）。mod の中でシェルの文字列を組まない
//! 4. 報告の `renders` / `last_press` の契約が TS と Rust で同じ集合。`last_press` は種類と成否と時刻だけ。
//!    A/B の S3 の描き方では `renders` を送らない
//! 5. tako 側の区画を止める判断は 1 本: GUI は `claude_mod::status_bar_owner(` を通し、それは
//!    `lookup_pane(`（一次ソースと同じ引き当て = 45 秒）と `core::claude_bar_owner(` と A/B（`core::s7_legacy()`）を通る。
//!    ステータスバーは ctx のメーターと claude の 5h / 7d をその判断で外す。A/B の env の名前を
//!    文字列で読むのは `tako_core::claude_mod` だけ
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::collections::BTreeSet;
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
const STATUS_BAR: &str = "crates/tako-app/src/status_bar.rs";
/// env の名前を直に読んではいけない側（[`CORE`] の外のソース）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const S7_ENV: &str = "\"TAKO_1877_S7_LEGACY\"";

const BAND_OPEN: &str = "on('ui.render', { component: 'AbovePrompt' }";
const HINT_OPEN: &str = "on('ui.render', { component: 'PromptHint' }";
const REG_CLOSE: &str = "  }).catch(($, e, next) => next(e))";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` を含む行から、`close` で始まる最初の行までの窓（1 始まりの行番号つき）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with(close))
        .map(|i| start + 1 + i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初の行の番号（無ければ窓の先頭）
fn line_of(window: &(usize, String), needle: &str) -> usize {
    window
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map_or(window.0, |i| window.0 + i)
}

struct Sources {
    register: String,
    types: String,
    core: String,
    control: String,
    main: String,
    status_bar: String,
    /// [`SOURCE_DIRS`] の .rs（相対パス, 中身）
    others: Vec<(String, String)>,
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn current() -> Sources {
    let root = workspace_root();
    let mut others = Vec::new();
    for dir in SOURCE_DIRS {
        let mut files = Vec::new();
        rs_files(&root.join(dir), &mut files);
        files.sort();
        for f in files {
            let rel = f
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            others.push((rel.clone(), read(&rel)));
        }
    }
    Sources {
        register: read(REGISTER),
        types: read(TYPES),
        core: read(CORE),
        control: read(CONTROL),
        main: read(MAIN),
        status_bar: read(STATUS_BAR),
        others,
    }
}

/// 窓の中の `return` の行（コメントを除く）
fn returns(w: &(usize, String)) -> Vec<(usize, String)> {
    w.1.lines()
        .enumerate()
        .map(|(i, l)| {
            (
                w.0 + i,
                l.split("//").next().unwrap_or_default().to_string(),
            )
        })
        .filter(|(_, l)| l.contains("return "))
        .collect()
}

/// 1. 帯のフックは next を包む
fn scan_band(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, BAND_OPEN, REG_CLOSE) {
        Some(w) => {
            if !w.1.contains("await next(e)") {
                out.push(report(
                    REGISTER,
                    w.0,
                    "帯のフックが `await next(e)` で下の答え（他の mod の行）を受け取っていない",
                ));
            }
            for (line, code) in returns(&w) {
                let ok = code.contains("return next(e)")
                    || code.contains("return stackBand(")
                    || (code.contains("return drawBandS3(")
                        && code.contains("view.band_style === 's3'"));
                if !ok {
                    out.push(report(
                        REGISTER,
                        line,
                        "帯のフックが next の答えを包まずに返している（内側の他の mod の帯が消える。返すのは next(e) / stackBand( / A/B の drawBandS3( だけ）",
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "帯のフック（AbovePrompt）が見つからない（走査が空振り）",
        )),
    }
    match window(register, "function stackBand(", "}") {
        Some(w) => {
            for need in [
                "below.type === 'engine'",
                "flexDirection: 'column'",
                "children: [below, row]",
            ] {
                if !w.1.contains(need) {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!("stackBand に `{need}` が無い（他の mod の行を上に残し、エンジンだけなら tako の行だけ）"),
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "stackBand が見つからない（走査が空振り）",
        )),
    }
    out
}

/// 2. 入力欄の下の行のフックは next へ流し、他の mod の tail を残す
fn scan_hint(register: &str) -> Vec<String> {
    let Some(w) = window(register, HINT_OPEN, REG_CLOSE) else {
        return vec![report(
            REGISTER,
            1,
            "入力欄の下の行のフック（PromptHint）が見つからない（走査が空振り）",
        )];
    };
    let mut out = Vec::new();
    for (line, code) in returns(&w) {
        if !(code.contains("return next(e)") || code.contains("return next({ ...e,")) {
            out.push(report(
                REGISTER,
                line,
                "入力欄の下の行のフックが next へ流さずに返している（エンジンの行と他の mod の tail が消える）",
            ));
        }
    }
    for need in ["e.props.tail", "e.surface !== 'terminal'"] {
        if !w.1.contains(need) {
            out.push(report(
                REGISTER,
                w.0,
                &format!("入力欄の下の行のフックに `{need}` が無い（他の mod の tail の後ろへ足す・tail を描かない surface では描かない）"),
            ));
        }
    }
    if w.1.contains("hint:") {
        out.push(report(
            REGISTER,
            line_of(&w, "hint:"),
            "入力欄の下の行の hint を書き換えている（行そのものを置き換えるとエンジンの案内が消える。足すのは tail だけ）",
        ));
    }
    out
}

/// 3. ボタンの押下は語彙どおりの 4 経路・シェルの文字列を組まない
fn scan_press(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, "async function pressButton(", "}") {
        Some(w) => {
            for need in [
                "$.command.run(",
                "$.prompt.fill(",
                "$.process.run([cli, ...press.argv]",
                "lastPress = { kind: press.kind, ok, at:",
            ] {
                if !w.1.contains(need) {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!("pressButton に `{need}` が無い（slash / prompt は Claude Code の中・tako / shell は tako が組んだ引数をそのまま・結果は種類と成否だけ）"),
                    ));
                }
            }
            for bad in ["'sh'", "'-c'", "bash", ".join(' ')"] {
                if w.1.contains(bad) {
                    out.push(report(
                        REGISTER,
                        line_of(&w, bad),
                        &format!("pressButton が `{bad}` でコマンドを組んでいる（引数は tako が組む = view.button_args。mod でシェルの文字列を作らない）"),
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "pressButton が見つからない（走査が空振り）",
        )),
    }
    match window(register, "function buttonsOf(", "}") {
        Some(w) if w.1.contains("view.button_args?.[button.id]") => {}
        Some(w) => out.push(report(
            REGISTER,
            w.0,
            "buttonsOf が tako / shell の引数を view.button_args から引いていない（判断は tako 側）",
        )),
        None => out.push(report(
            REGISTER,
            1,
            "buttonsOf が見つからない（走査が空振り）",
        )),
    }
    out
}

/// TS の型の窓（`export type <name> = {` から最初の `}` 行まで）のキー
fn ts_keys(src: &str, name: &str) -> Option<(usize, BTreeSet<String>)> {
    let (at, w) = window(src, &format!("export type {name} = {{"), "}")?;
    let keys = w
        .lines()
        .skip(1)
        .filter_map(|l| {
            let rest = l.strip_prefix("  ")?;
            if rest.starts_with(' ') || rest.starts_with('/') || rest.starts_with('*') {
                return None;
            }
            let key: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!key.is_empty() && rest[key.len()..].trim_start_matches('?').starts_with(':'))
                .then_some(key)
        })
        .collect();
    Some((at, keys))
}

/// Rust の構造体の窓（`pub struct <name> {` から `}` 行まで）のフィールド
fn rs_fields(src: &str, name: &str) -> Option<(usize, BTreeSet<String>)> {
    let (at, w) = window(src, &format!("pub struct {name} {{"), "}")?;
    let keys = w
        .lines()
        .filter_map(|l| {
            let rest = l.trim_start().strip_prefix("pub ")?;
            let key: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!key.is_empty() && rest[key.len()..].starts_with(':')).then_some(key)
        })
        .collect();
    Some((at, keys))
}

/// 4. renders / last_press の契約と、A/B で renders を送らないこと
fn scan_contract(register: &str, types: &str, core: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (ts, rs) in [("TakoRenders", "ModRenders"), ("TakoPress", "ModPress")] {
        match (ts_keys(types, ts), rs_fields(core, rs)) {
            (Some((ts_at, ts_keys)), Some((rs_at, rs_keys))) => {
                if ts_keys != rs_keys {
                    out.push(report(
                        TYPES,
                        ts_at,
                        &format!(
                            "{ts}（TS）と {rs}（{CORE}:{rs_at}）のキーが食い違う（TS {ts_keys:?} / Rust {rs_keys:?}）"
                        ),
                    ));
                }
                if ts == "TakoPress" {
                    let want: BTreeSet<String> =
                        ["kind", "ok", "at"].iter().map(|s| s.to_string()).collect();
                    if ts_keys != want {
                        out.push(report(
                            TYPES,
                            ts_at,
                            "押した結果に種類・成否・時刻のほかを載せている（ラベル・コマンド・文は載せない）",
                        ));
                    }
                }
            }
            _ => out.push(report(
                TYPES,
                1,
                &format!("{ts} / {rs} が見つからない（走査が空振り）"),
            )),
        }
    }
    match window(register, "function currentRenders(", "}") {
        Some(w) if w.1.contains("if (lastStyle === 's3') return undefined") => {}
        Some(w) => out.push(report(
            REGISTER,
            w.0,
            "A/B（S3 の描き方）でも renders を送っている（S3 の mod は何も描いたと言わない = tako の表示を止めない）",
        )),
        None => out.push(report(
            REGISTER,
            1,
            "currentRenders が見つからない（走査が空振り）",
        )),
    }
    match window(register, "async function buildReport(", "}") {
        Some(w) if w.1.contains("renders: currentRenders()") => {}
        Some(w) => out.push(report(
            REGISTER,
            w.0,
            "buildReport が renders を currentRenders() の外で組んでいる",
        )),
        None => out.push(report(
            REGISTER,
            1,
            "buildReport が見つからない（走査が空振り）",
        )),
    }
    out
}

/// 5. tako 側の区画を止める判断は 1 本
fn scan_status_bar(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    match window(&s.main, "    fn refresh_agent_metrics(&mut self) {", "    }") {
        Some(w) if w.1.contains("tako_control::claude_mod::status_bar_owner(") => {}
        Some(w) => out.push(report(
            MAIN,
            w.0,
            "refresh_agent_metrics が claude_mod::status_bar_owner を通していない（ステータスバーの区画を止める判断が 2 つになる）",
        )),
        None => out.push(report(
            MAIN,
            1,
            "refresh_agent_metrics が見つからない（走査が空振り）",
        )),
    }
    for need in [
        "let claude_yields = self.claude_bar_owner.yields();",
        "LimitService::Claude if claude_yields => (None, None),",
        ".children((!claude_yields).then(",
    ] {
        if !s.status_bar.contains(need) {
            out.push(report(
                STATUS_BAR,
                1,
                &format!("ステータスバーに `{need}` が無い（mod がバーを描いているとき claude の 5h / 7d / ctx を出さない）"),
            ));
        }
    }
    match window(&s.control, "pub fn status_bar_owner(", "}") {
        Some(w) => {
            for need in [
                "lookup_pane(",
                "core::claude_bar_owner(",
                "core::s7_legacy()",
            ] {
                if !w.1.contains(need) {
                    out.push(report(
                        CONTROL,
                        w.0,
                        &format!("status_bar_owner に `{need}` が無い（新鮮な報告だけ = 一次ソースと同じ引き当て・判断は tako-core の 1 本・A/B を見る）"),
                    ));
                }
            }
        }
        None => out.push(report(
            CONTROL,
            1,
            "status_bar_owner が見つからない（走査が空振り）",
        )),
    }
    match window(&s.control, "fn band_view(", "}") {
        Some(w) => {
            for need in ["s7_legacy: core::s7_legacy()", "core::screen_shows_usage("] {
                if !w.1.contains(need) {
                    out.push(report(
                        CONTROL,
                        w.0,
                        &format!("band_view に `{need}` が無い（A/B と画面の読み取りをバーの判断へ渡す）"),
                    ));
                }
            }
        }
        None => out.push(report(
            CONTROL,
            1,
            "band_view が見つからない（走査が空振り）",
        )),
    }
    for (file, src) in &s.others {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if code.contains(S7_ENV) {
                out.push(report(
                    file,
                    i + 1,
                    &format!("{S7_ENV} を {CORE} の外で読んでいる（A/B の env の名前は 1 か所 = s7_legacy を通す）"),
                ));
            }
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_band(&s.register);
    out.extend(scan_hint(&s.register));
    out.extend(scan_press(&s.register));
    out.extend(scan_contract(&s.register, &s.types, &s.core));
    out.extend(scan_status_bar(s));
    out
}

#[test]
fn 帯とボタンとバーの構造が契約どおり() {
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
    let (_, w) = window(&s.register, BAND_OPEN, REG_CLOSE).unwrap();
    assert!(
        returns(&(0, w.clone())).len() >= 3,
        "帯のフックの return を拾えていない: {w}"
    );
    let (_, w) = window(&s.register, HINT_OPEN, REG_CLOSE).unwrap();
    assert!(returns(&(0, w.clone())).len() >= 2, "{w}");
    let (_, renders) = rs_fields(&s.core, "ModRenders").unwrap();
    assert!(
        renders.contains("usage_bar") && renders.contains("band_hook"),
        "{renders:?}"
    );
    let (_, press) = ts_keys(&s.types, "TakoPress").unwrap();
    assert_eq!(press.len(), 3, "{press:?}");
    let (_, w) = window(
        &s.main,
        "    fn refresh_agent_metrics(&mut self) {",
        "    }",
    )
    .unwrap();
    assert!(w.contains("status_bar_limits("), "窓が短すぎる: {w}");
    assert!(
        s.others.len() > 50,
        "走査した .rs が {} 本しかない",
        s.others.len()
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
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. 帯のフックが next を包まずに tako の行だけを返す（S3 の形へ戻す）
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    return stackBand($, e, below, bandRow($, e, view, columns, bar))",
            "    return bandRow($, e, view, columns, bar)",
            1,
        );
    });
    assert_named(&found, REGISTER, "next の答えを包まずに返している");
    let found = mutate(&|s| {
        s.register = s
            .register
            .replacen("    const below = await next(e)\n", "", 1);
    });
    assert_named(&found, REGISTER, "`await next(e)`");
    // 1b. A/B でもないのに S3 の描き方を返す
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    if (view.band_style === 's3') return drawBandS3($, e, view, columns)",
            "    if (columns < 40) return drawBandS3($, e, view, columns)",
            1,
        );
    });
    assert_named(&found, REGISTER, "next の答えを包まずに返している");
    // 1c. 下の mod の行を捨てる
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  return Box({ flexDirection: 'column', children: [below, row] })",
            "  return row",
            1,
        );
    });
    assert_named(&found, REGISTER, "children: [below, row]");
    // 2. 入力欄の下の行を自分で描く・hint を書き換える・他の mod の tail を捨てる
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    return next({ ...e, props: { ...e.props, tail } })",
            "    return $.ui.resolve(e).Text({ children: bar })",
            1,
        );
    });
    assert_named(&found, REGISTER, "next へ流さずに返している");
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    return next({ ...e, props: { ...e.props, tail } })",
            "    return next({ ...e, props: { ...e.props, hint: bar } })",
            1,
        );
    });
    assert_named(&found, REGISTER, "hint を書き換えている");
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    const tail = e.props.tail === undefined || e.props.tail === '' ? bar : `${e.props.tail} · ${bar}`",
            "    const tail = bar",
            1,
        );
    });
    assert_named(&found, REGISTER, "`e.props.tail`");
    // 3. mod の中でシェルの文字列を組む
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "      const done = await $.process.run([cli, ...press.argv], { timeoutMs: PRESS_TIMEOUT_MS })",
            "      const done = await $.process.run(['sh', '-c', press.argv.join(' ')], { timeoutMs: PRESS_TIMEOUT_MS })",
            1,
        );
    });
    assert_named(&found, REGISTER, "`'sh'`");
    assert_named(&found, REGISTER, "$.process.run([cli, ...press.argv]");
    // 4. 押した結果にラベルを載せる・A/B でも renders を送る・契約の片側だけ
    let found = mutate(&|s| {
        s.types = s.types.replacen(
            "  kind: 'slash' | 'tako' | 'shell' | 'prompt'\n",
            "  kind: 'slash' | 'tako' | 'shell' | 'prompt'\n  label: string\n",
            1,
        );
    });
    assert_named(&found, TYPES, "種類・成否・時刻のほか");
    assert_named(&found, TYPES, "TakoPress（TS）と ModPress");
    let found = mutate(&|s| {
        s.register = s
            .register
            .replacen("  if (lastStyle === 's3') return undefined\n", "", 1);
    });
    assert_named(&found, REGISTER, "A/B（S3 の描き方）でも renders");
    // 5. GUI が判断を通さない・ステータスバーが区画を外さない・新鮮さを見ない・env を外で読む
    let found = mutate(&|s| {
        s.main = s.main.replacen(
            "tako_control::claude_mod::status_bar_owner(",
            "tako_control::claude_mod::status_bar_owner_inline(",
            1,
        );
    });
    assert_named(&found, MAIN, "status_bar_owner を通していない");
    let found = mutate(&|s| {
        s.status_bar = s.status_bar.replacen(
            ".children((!claude_yields).then(",
            ".children(Some(()).map(",
            1,
        );
    });
    assert_named(&found, STATUS_BAR, ".children((!claude_yields).then(");
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "    let report = lookup_pane(host, focused, now).ok().map(|s| &s.report);",
            "    let report = host.claude_mod().and_then(|h| h.reports.get(&focused.as_u64())).map(|r| &r.report);",
            1,
        );
    });
    assert_named(&found, CONTROL, "`lookup_pane(`");
    let found = mutate(&|s| {
        let (_, src) = s
            .others
            .iter_mut()
            .find(|(f, _)| f == CONTROL)
            .expect("CONTROL は走査の対象");
        src.push_str("\nfn leak() -> bool { std::env::var(\"TAKO_1877_S7_LEGACY\").is_ok() }\n");
    });
    assert_named(&found, CONTROL, "TAKO_1877_S7_LEGACY");
}
