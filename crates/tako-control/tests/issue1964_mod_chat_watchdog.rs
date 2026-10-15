//! tako mod S7-6（#1964。FR-2.42.48〜）の番犬: チャット風の描画を mod で描く・GUI モードでチャットを mod に任せる
//!
//! # なぜ要るか
//!
//! GUI モードの claude のペインは「GPUI のチャットビュー」と「mod が描いた本物の Claude Code の画面」の
//! **どちらか一方**を出す。どちらを出すかの判断が 2 か所に散ると、片方だけ直したときに mod が止まっても
//! GPUI のチャットへ戻らない（真っさらなターミナルが残る）か、mod が描いているのに GPUI が覆う（二重）が
//! 黙って起きる。mod の側も、会話の行を描き替えるフックが `next` の答えを捨てれば利用者の他の mod の描画を
//! 消し、Copy が画面の文字列を写せば折り返しの改行が混ざる。`claude plugin test` の期待値はどれも
//! 「吹き出しが出る」だけでも通るので、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 持ち主の判断は 1 本: GUI のペインの表示（`pane_display_input`）と 2 秒 tick の指紋
//!    （`refresh_mod_chat_owners`。main.rs の 2 秒ポーリングが呼ぶ）は `tako_control::claude_mod::chat_owner(`
//!    を通り、それは `lookup_pane(`（一次ソースと同じ 45 秒）・`core::chat_owner(`・表示モード・A/B
//!    （`core::chat_legacy()` と `core::s7_legacy()`）を通る。`tako mod` の各ペインの `chat` も `chat_owner_json(`
//! 2. A/B の env（`TAKO_1964_LEGACY`）の名前を文字列で読むのは `tako_core::claude_mod` だけ
//! 3. 会話の行のフック（`UserMessage` / `AssistantMessage` / `ToolUse` の要約）は `next(e)` か、
//!    `await next(e)` の答えを包む `drawBubble(` / `drawCodeCopy(` / `drawToolSummary(` だけを返し、
//!    それぞれ下の描画を捨てない（エンジンの行なら置き換え、他の mod の描画は中 / 上に残す）
//! 4. Copy: `$.ui.copy(` に**論理文字列**（コードブロックの中身 = `codeBlocks(` が返す `source`）を渡す。
//!    mod の中でシェルの文字列を組まない・押した結果は種類と成否だけ。Copy のボタンの key は返答の id と順番で
//!    固定（描き直しで mount し直さない = Claude Code が描いた直後 300 ms のクリックを捨てる窓を作り直さない）
//! 5. `renders.chat` は tako が描かせた組（応答の `view.chat`）だけ。会話の行の数（`chat_seq`）は
//!    `await next(e)` の後に数え、すぐ報告するのは `view.chat.notify` のときだけ
//! 6. 契約: `TakoChat` / `TakoChatRenders`（TS）と `ChatView` / `ModChatRenders`（Rust）のキーが同じ集合
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
const CHAT_VIEW: &str = "crates/tako-app/src/chat_view.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
/// env の名前を直に読んではいけない側（[`CORE`] の外のソース）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const CHAT_ENV: &str = "\"TAKO_1964_LEGACY\"";

const USER_OPEN: &str = "on('ui.render', { component: 'UserMessage' }";
const ASSIST_OPEN: &str = "on('ui.render', { component: 'AssistantMessage' }";
const USE_OPEN: &str = "on('ui.render', { component: 'ToolUse' }";
const APPEND_OPEN: &str = "on('session.append'";
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

struct Sources {
    register: String,
    types: String,
    core: String,
    control: String,
    chat_view: String,
    main: String,
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
        chat_view: read(CHAT_VIEW),
        main: read(MAIN),
        others,
    }
}

/// 窓に `need` がすべてあるか（無いものを名指す）
fn needs(out: &mut Vec<String>, file: &str, w: &(usize, String), list: &[&str], why: &str) {
    for need in list {
        if !w.1.contains(need) {
            out.push(report(file, w.0, &format!("`{need}` が無い（{why}）")));
        }
    }
}

/// 1 / 2. GUI モードのチャットの持ち主の判断は 1 本・env は tako-core だけ
fn scan_owner(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    match window(&s.main, "    fn pane_display_input(", "    }") {
        Some(w) => needs(
            &mut out,
            MAIN,
            &w,
            &["mod_chat: self.ui_mode.is_gui()", "tako_control::claude_mod::chat_owner(self, pane_id"],
            "ペインの表示は chat_owner の 1 本で決める = mod が止まれば GPUI のチャットへ戻る・描いているのに覆わない",
        ),
        None => out.push(report(MAIN, 1, "pane_display_input が見つからない（走査が空振り）")),
    }
    match window(
        &s.chat_view,
        "    pub(crate) fn refresh_mod_chat_owners(",
        "    }",
    ) {
        Some(w) => needs(
            &mut out,
            CHAT_VIEW,
            &w,
            &["tako_control::claude_mod::chat_owner("],
            "2 秒 tick の指紋もペインの表示と同じ判断で取る",
        ),
        None => out.push(report(
            CHAT_VIEW,
            1,
            "refresh_mod_chat_owners が見つからない（走査が空振り）",
        )),
    }
    if !s.main.contains("if app.refresh_mod_chat_owners() {") {
        out.push(report(
            MAIN,
            1,
            "2 秒ポーリングが refresh_mod_chat_owners を呼んでいない（報告の失効・claude の終了で GPUI のチャットへ戻らない）",
        ));
    }
    match window(&s.control, "pub fn chat_owner(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &[
                "lookup_pane(",
                "core::chat_owner(",
                "core::chat_legacy()",
                "core::s7_legacy()",
                "host.ui_mode().is_gui()",
            ],
            "新鮮な報告だけ = 一次ソースと同じ引き当て・判断は tako-core の 1 本・表示モードと A/B を見る",
        ),
        None => out.push(report(CONTROL, 1, "chat_owner が見つからない（走査が空振り）")),
    }
    match window(&s.control, "fn band_view(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &["ui_mode_gui: host.ui_mode().is_gui()", "chat_legacy: core::chat_legacy()"],
            "応答の view.chat は表示モードと A/B から組む（ターミナル表示の利用者に吹き出しを出さない）",
        ),
        None => out.push(report(CONTROL, 1, "band_view が見つからない（走査が空振り）")),
    }
    if !s
        .control
        .contains("row[\"chat\"] = chat_owner_json(host, id, now);")
    {
        out.push(report(
            CONTROL,
            1,
            "tako mod の各ペインの chat が chat_owner_json を通していない（CLI / MCP が GPUI と同じ判断を読めない）",
        ));
    }
    for (file, src) in &s.others {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if code.contains(CHAT_ENV) {
                out.push(report(
                    file,
                    i + 1,
                    &format!("{CHAT_ENV} を {CORE} の外で読んでいる（A/B の env の名前は 1 か所 = chat_legacy を通す）"),
                ));
            }
        }
    }
    out
}

/// 3. 会話の行のフックは next を包む
fn scan_rows(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, draw, name) in [
        (
            USER_OPEN,
            "return drawBubble($, e, view, below)",
            "UserMessage",
        ),
        (
            ASSIST_OPEN,
            "return drawCodeCopy($, e, view, below, blocks)",
            "AssistantMessage",
        ),
        (
            USE_OPEN,
            "return drawToolSummary($, e, view, below, open)",
            "ToolUse",
        ),
    ] {
        match window(register, open, REG_CLOSE) {
            Some(w) => {
                if !w.1.contains("const below = await next(e)") {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!("{name} のフックが `await next(e)` で下の答え（他の mod の描画）を受け取っていない"),
                    ));
                }
                if !w.1.contains(draw) {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!("{name} のフックが `{draw}` を返していない"),
                    ));
                }
                for (line, code) in returns(&w) {
                    let ok = code.contains("return next(e)")
                        || code.contains(draw)
                        // ToolUse はカード（#1963）も同じフック
                        || code.contains("return drawCardRow($, e, view, below)");
                    if !ok {
                        out.push(report(
                            REGISTER,
                            line,
                            &format!("{name} のフックが next の答えを包まずに返している（返すのは next(e) / {draw} だけ）"),
                        ));
                    }
                }
            }
            None => out.push(report(
                REGISTER,
                1,
                &format!("{name} のフックが見つからない（走査が空振り）"),
            )),
        }
    }
    for (open, list, why) in [
        (
            "function drawBubble(",
            &[
                "below.type === 'engine' ? Text({ wrap: 'wrap', children: e.props.text }) : below",
                "justifyContent: 'flex-end'",
                "borderStyle: 'round'",
            ][..],
            "エンジンの行なら本文で描き直し、他の mod の描画は吹き出しの中に包む・右寄せの枠",
        ),
        (
            "function drawCodeCopy(",
            &[
                "children: [below, copies]",
                "key: `tako-code-${e.requestId}-${i}`",
                "copyCode($, block.source, press)",
            ][..],
            "返答の描画を上に残してその下に Copy・key は返答の id と順番で固定・写すのはブロックの論理文字列",
        ),
        (
            "function drawToolSummary(",
            &["if (!open && below.type === 'engine') return row", "open ? [row, below] : [below, row]"][..],
            "エンジンの行だけ要約に置き換え、他の mod の描画と「詳細」のエンジンの行は残す",
        ),
    ] {
        match window(register, open, "}") {
            Some(w) => needs(&mut out, REGISTER, &w, list, why),
            None => out.push(report(REGISTER, 1, &format!("{open} が見つからない（走査が空振り）"))),
        }
    }
    out
}

/// 4 / 5. 押下と報告
fn scan_press(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, "async function copyCode(", "}") {
        Some(w) => {
            needs(
                &mut out,
                REGISTER,
                &w,
                &[
                    "$.ui.copy({ text, surface: press.surface })",
                    "lastPress = { kind: 'code_copy', ok, at:",
                ],
                "Copy は論理文字列を $.ui.copy へ・結果は種類と成否だけ",
            );
            for bad in ["'sh'", "'-c'", "$.process.run(", ".join(' ')"] {
                if w.1.contains(bad) {
                    out.push(report(
                        REGISTER,
                        line_of(&w, bad),
                        &format!("copyCode が `{bad}` を使っている（コードの本文をシェル・CLI の引数へ流さない）"),
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "copyCode が見つからない（走査が空振り）",
        )),
    }
    if !register.contains("function codeBlocks(text: string): CodeBlock[]") {
        out.push(report(
            REGISTER,
            1,
            "codeBlocks が見つからない（走査が空振り）",
        ));
    }
    match window(register, "function chatRenders(", "}") {
        Some(w) => needs(
            &mut out,
            REGISTER,
            &w,
            &["if (chatView === undefined) return undefined"],
            "renders.chat は tako が描かせた組だけ（描かせていない tako にチャットを描いたと言わない）",
        ),
        None => out.push(report(REGISTER, 1, "chatRenders が見つからない（走査が空振り）")),
    }
    match window(register, APPEND_OPEN, REG_CLOSE) {
        Some(w) => {
            needs(
                &mut out,
                REGISTER,
                &w,
                &[
                    "const done = await next(e)",
                    "e.agentId === undefined",
                    "chatSeq += 1",
                    "if (chatView?.notify === true) dirty = true",
                ],
                "数えるのは transcript に書かれた後（next の後）・本筋だけ・すぐ報告するのは tako が通知を頼んだときだけ",
            );
            let next_at = line_of(&w, "await next(e)");
            let count_at = line_of(&w, "chatSeq += 1");
            if count_at < next_at {
                out.push(report(
                    REGISTER,
                    count_at,
                    "chat_seq を next の前に数えている（書かれる前に知らせると GPUI のチャットが古い transcript を読む）",
                ));
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "session.append のフックが見つからない（走査が空振り）",
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

/// 6. 応答と報告のチャットの契約
fn scan_contract(types: &str, core: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (ts, rs) in [
        ("TakoChat", "ChatView"),
        ("TakoChatRenders", "ModChatRenders"),
    ] {
        match (ts_keys(types, ts), rs_fields(core, rs)) {
            (Some((ts_at, ts_keys)), Some((rs_at, rs_keys))) => {
                if ts_keys != rs_keys {
                    out.push(report(
                        TYPES,
                        ts_at,
                        &format!("{ts}（TS）と {rs}（{CORE}:{rs_at}）のキーが食い違う（TS {ts_keys:?} / Rust {rs_keys:?}）"),
                    ));
                }
            }
            _ => out.push(report(
                TYPES,
                1,
                &format!("{ts} / {rs} が見つからない（走査が空振り）"),
            )),
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_owner(s);
    out.extend(scan_rows(&s.register));
    out.extend(scan_press(&s.register));
    out.extend(scan_contract(&s.types, &s.core));
    out
}

#[test]
fn チャットの判断と描画の構造が契約どおり() {
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
    for open in [USER_OPEN, ASSIST_OPEN, USE_OPEN] {
        let (_, w) = window(&s.register, open, REG_CLOSE).unwrap();
        assert!(
            returns(&(0, w.clone())).len() >= 2,
            "{open} の return を拾えていない: {w}"
        );
    }
    let (_, w) = window(&s.register, "async function copyCode(", "}").unwrap();
    assert!(
        w.contains("$.ui.copy(") && w.contains("dirty = true"),
        "窓が短すぎる: {w}"
    );
    let (_, w) = window(&s.register, APPEND_OPEN, REG_CLOSE).unwrap();
    assert!(w.contains("return done"), "窓が短すぎる: {w}");
    let (_, keys) = ts_keys(&s.types, "TakoChat").unwrap();
    assert!(keys.contains("mode") && keys.contains("notify"), "{keys:?}");
    let (_, keys) = rs_fields(&s.core, "ModChatRenders").unwrap();
    assert!(keys.contains("tool_summary"), "{keys:?}");
    let (_, w) = window(&s.main, "    fn pane_display_input(", "    }").unwrap();
    assert!(w.contains("settle:"), "窓が短すぎる: {w}");
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

/// 注入が現行ソースに当たったか（当たらなければ注入そのものが古い = 見逃す側へ倒れる）
fn replace_once(src: &mut String, from: &str, to: &str) {
    assert!(
        src.contains(from),
        "注入の元の字面が現行ソースに無い: {from}"
    );
    *src = src.replacen(from, to, 1);
}

#[test]
fn 逆戻りを名指しできる() {
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. ペインの表示が持ち主の判断を見ない（mod が描いているのに GPUI のチャットで覆う = #1964 前）
    let found = mutate(&|s| {
        replace_once(
            &mut s.main,
            "tako_control::claude_mod::chat_owner(self, pane_id",
            "tako_control::claude_mod::status_bar_owner(self, pane_id",
        );
    });
    assert_named(&found, MAIN, "chat_owner(self, pane_id");
    // 1b. 2 秒 tick が集合の変化を知らせない（mod が止まっても GPUI のチャットへ戻らない）
    let found = mutate(&|s| {
        replace_once(
            &mut s.main,
            "if app.refresh_mod_chat_owners() {",
            "if false {",
        );
    });
    assert_named(&found, MAIN, "refresh_mod_chat_owners を呼んでいない");
    // 1c. 新鮮さを見ずに報告を引く
    let found = mutate(&|s| {
        replace_once(
            &mut s.control,
            "        stored: lookup_pane(host, pane, now).ok(),\n        legacy: core::chat_legacy() || core::s7_legacy(),",
            "        stored: host.claude_mod().and_then(|h| h.reports.get(&pane.as_u64())),\n        legacy: core::chat_legacy() || core::s7_legacy(),",
        );
    });
    assert_named(&found, CONTROL, "`lookup_pane(`");
    // 1d. A/B を見ない・表示モードを見ない
    let found = mutate(&|s| {
        replace_once(
            &mut s.control,
            "        legacy: core::chat_legacy() || core::s7_legacy(),",
            "        legacy: false,",
        );
    });
    assert_named(&found, CONTROL, "`core::chat_legacy()`");
    let found = mutate(&|s| {
        replace_once(
            &mut s.control,
            "        ui_mode_gui: host.ui_mode().is_gui(),\n        chat_legacy",
            "        ui_mode_gui: true,\n        chat_legacy",
        );
    });
    assert_named(&found, CONTROL, "ui_mode_gui: host.ui_mode().is_gui()");
    // 2. env を tako-core の外で読む
    let found = mutate(&|s| {
        let (_, src) = s
            .others
            .iter_mut()
            .find(|(f, _)| f == CHAT_VIEW)
            .expect("CHAT_VIEW は走査の対象");
        src.push_str("\nfn leak() -> bool { std::env::var(\"TAKO_1964_LEGACY\").is_ok() }\n");
    });
    assert_named(&found, CHAT_VIEW, "TAKO_1964_LEGACY");
    // 3. 発話の行が下の答えを捨てる・他の mod の描画を吹き出しから捨てる
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    const below = await next(e)\n    return drawBubble($, e, view, below)",
            "    return drawBubble($, e, view, { type: 'engine', ref: 0 })",
        );
    });
    assert_named(&found, REGISTER, "UserMessage のフックが `await next(e)`");
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "  const inner = below.type === 'engine' ? Text({ wrap: 'wrap', children: e.props.text }) : below",
            "  const inner = Text({ wrap: 'wrap', children: e.props.text })",
        );
    });
    assert_named(&found, REGISTER, "children: e.props.text }) : below");
    // 3b. 返答の行を Copy だけに置き換える・key が描き直しのたびに変わる
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "  return Box({ flexDirection: 'column', children: [below, copies] })",
            "  return row",
        );
    });
    assert_named(&found, REGISTER, "children: [below, copies]");
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "key: `tako-code-${e.requestId}-${i}`",
            "key: `tako-code-${Math.random()}`",
        );
    });
    assert_named(&found, REGISTER, "tako-code-${e.requestId}-${i}");
    // 3c. ツールの行が他の mod の描画を捨てる
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "  if (!open && below.type === 'engine') return row",
            "  if (!open) return row",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "if (!open && below.type === 'engine') return row",
    );
    // 4. Copy が画面の文字列を写す・シェルへ流す
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "copyCode($, block.source, press)",
            "copyCode($, e.props.text, press)",
        );
    });
    assert_named(&found, REGISTER, "copyCode($, block.source, press)");
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    ok = (await $.ui.copy({ text, surface: press.surface })).isCopied",
            "    ok = (await $.process.run(['sh', '-c', `printf %s '${text}' | pbcopy`])).exitCode === 0",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "$.ui.copy({ text, surface: press.surface })",
    );
    assert_named(&found, REGISTER, "`'sh'`");
    // 5. 描かせていない tako にもチャットを描いたと言う・書かれる前に数える・いつでも報告を急ぐ
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "  if (chatView === undefined) return undefined\n",
            "",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "if (chatView === undefined) return undefined",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "      if (chatView?.notify === true) dirty = true",
            "      dirty = true",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "if (chatView?.notify === true) dirty = true",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "  on('session.append', async ($, e, next) => {\n    const done = await next(e)\n    if (e.agentId === undefined && (e.message.type === 'user' || e.message.type === 'assistant')) {\n      chatSeq += 1",
            "  on('session.append', async ($, e, next) => {\n    if (e.agentId === undefined && (e.message.type === 'user' || e.message.type === 'assistant')) {\n      chatSeq += 1\n    }\n    const done = await next(e)\n    if (e.agentId === undefined && (e.message.type === 'user' || e.message.type === 'assistant')) {",
        );
    });
    assert_named(&found, REGISTER, "chat_seq を next の前に数えている");
    // 6. 契約のキーが片方だけ増える
    let found = mutate(&|s| {
        replace_once(
            &mut s.types,
            "  hotkey: string\n  /** 会話の行が足されたらすぐ報告する",
            "  hotkey: string\n  extra: number\n  /** 会話の行が足されたらすぐ報告する",
        );
    });
    assert_named(&found, TYPES, "TakoChat（TS）と ChatView");
}
