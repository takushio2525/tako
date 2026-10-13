//! tako mod S7-4（#1963。FR-2.42.41〜）の番犬: コマンドカードを mod で描く・描いたカードだけ GPUI の帯から外す
//!
//! # なぜ要るか
//!
//! カードは**同じものが 2 か所に出うる**（mod が Claude Code の画面に描くカードと、GPUI のカード帯）。
//! どちらを出すかの判断が 2 か所に散ると、片方だけ直したときに二重表示（viewport が帯のぶん縮む）か、
//! どこにも出ない（mod は描いていないのに GPUI も引っ込める）が黙って起きる。mod の側も、会話の行を
//! 描き替えるフックが `next` を包まなければ他の mod の描画を消し、Copy が `$.ui.copy` の拒否を無視すれば
//! 何も写らないまま「押した」ことになる。`claude plugin test` の期待値はどちらも「カードが出る」だけでも
//! 通るので、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. GPUI のカード帯から外す判断は 1 本: `command_card_rows` が `cards_drawn_by_mod(` で外し、2 秒 tick の
//!    `refresh_mod_card_owners` も同じ関数で集合を取る（main.rs の 2 秒ポーリングがそれを呼ぶ）。
//!    `cards_drawn_by_mod` は `card_owner(` を通り、`card_owner` は `lookup_pane(`（一次ソースと同じ 45 秒の
//!    引き当て）・`core::card_owner(`・A/B（`core::cards_legacy()` と `core::s7_legacy()`）を通る。
//!    `tako show-command --list` / `show` の `owner` も同じ `card_owner_json(` を通る
//! 2. A/B の env（`TAKO_1963_LEGACY`）の名前を文字列で読むのは `tako_core::claude_mod` だけ
//! 3. 会話の行: `ToolGroup` のフックは `next(e)` か `isExpanded: true` にした `next` だけを返す（グループを
//!    自分で描かない）。`ToolUse` のフックは `next(e)` か、`await next(e)` の答えを包む `drawCardRow(` を返し、
//!    `drawCardRow` は下の mod が描いていればその下に足す
//! 4. 押下: Copy は `$.ui.copy(` に論理文字列（`sel.command.text`）をそのまま渡し、写らなければ（`isCopied`）
//!    tako が組んだ `copy_args` で CLI へ落ちる。Run / 閉じるは tako が組んだ `run_args` / `dismiss_args` をそのまま。
//!    mod の中でシェルの文字列を組まない
//! 5. `renders.cards` は tako が描かせたとき（応答に `view.cards`）だけ送る。`tako_show_command` の呼び出しが
//!    返ったらすぐ報告する（heartbeat の 15 秒を待たない = GPUI の帯を保留する猶予に間に合わせる）
//! 6. 応答のカードの契約（`TakoCards` / `TakoCard` / `TakoCardCommand` と Rust の `CardsView` / `CardView` /
//!    `CardCommandView`）のキーが同じ集合
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
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CARD_UI: &str = "crates/tako-app/src/command_card_ui.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
/// env の名前を直に読んではいけない側（[`CORE`] の外のソース）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const CARDS_ENV: &str = "\"TAKO_1963_LEGACY\"";

const GROUP_OPEN: &str = "on('ui.render', { component: 'ToolGroup' }";
const USE_OPEN: &str = "on('ui.render', { component: 'ToolUse' }";
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
    dispatch: String,
    card_ui: String,
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
        dispatch: read(DISPATCH),
        card_ui: read(CARD_UI),
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

/// 1 / 2. GPUI のカード帯から外す判断は 1 本・env は tako-core だけ
fn scan_owner(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    match window(&s.card_ui, "    pub(crate) fn command_card_rows(", "    }") {
        Some(w) => needs(
            &mut out,
            CARD_UI,
            &w,
            &[
                "tako_control::claude_mod::cards_drawn_by_mod(",
                ".filter(|c| !by_mod.contains(&c.id().as_u64()))",
            ],
            "GPUI のカード帯は mod が描いたカードを cards_drawn_by_mod の 1 本で外す = 二重表示と viewport の縮みを作らない",
        ),
        None => out.push(report(CARD_UI, 1, "command_card_rows が見つからない（走査が空振り）")),
    }
    match window(
        &s.card_ui,
        "    pub(crate) fn refresh_mod_card_owners(",
        "    }",
    ) {
        Some(w) => needs(
            &mut out,
            CARD_UI,
            &w,
            &["tako_control::claude_mod::cards_drawn_by_mod("],
            "2 秒 tick の集合も帯と同じ判断で取る",
        ),
        None => out.push(report(
            CARD_UI,
            1,
            "refresh_mod_card_owners が見つからない（走査が空振り）",
        )),
    }
    if !s.main.contains("if app.refresh_mod_card_owners() {") {
        out.push(report(
            MAIN,
            1,
            "2 秒ポーリングが refresh_mod_card_owners を呼んでいない（報告の失効・claude の終了で GPUI の帯へ戻らない）",
        ));
    }
    match window(&s.control, "pub fn card_owner(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &[
                "lookup_pane(",
                "core::card_owner(",
                "core::cards_legacy()",
                "core::s7_legacy()",
                "card.shown_at()",
            ],
            "新鮮な報告だけ = 一次ソースと同じ引き当て・判断は tako-core の 1 本・A/B を見る・拾う猶予はカードの時刻",
        ),
        None => out.push(report(CONTROL, 1, "card_owner が見つからない（走査が空振り）")),
    }
    match window(&s.control, "pub fn cards_drawn_by_mod(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &["card_owner(host, card, now).yields()"],
            "外す集合も card_owner の 1 本",
        ),
        None => out.push(report(
            CONTROL,
            1,
            "cards_drawn_by_mod が見つからない（走査が空振り）",
        )),
    }
    match window(&s.control, "fn band_view(", "}") {
        Some(w) => needs(
            &mut out,
            CONTROL,
            &w,
            &["cards_legacy: core::cards_legacy()", ".command_cards()"],
            "応答の view.cards は store のカードと A/B から組む",
        ),
        None => out.push(report(
            CONTROL,
            1,
            "band_view が見つからない（走査が空振り）",
        )),
    }
    let owners = s
        .dispatch
        .matches("crate::claude_mod::card_owner_json(")
        .count();
    if owners < 2 {
        out.push(report(
            DISPATCH,
            1,
            &format!("show-command の show / list が card_owner_json を通していない（{owners} 箇所。CLI / MCP が GPUI と同じ判断を読めない）"),
        ));
    }
    for (file, src) in &s.others {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if code.contains(CARDS_ENV) {
                out.push(report(
                    file,
                    i + 1,
                    &format!("{CARDS_ENV} を {CORE} の外で読んでいる（A/B の env の名前は 1 か所 = cards_legacy を通す）"),
                ));
            }
        }
    }
    out
}

/// 3. 会話の行のフックは next を包む
fn scan_rows(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, GROUP_OPEN, REG_CLOSE) {
        Some(w) => {
            for (line, code) in returns(&w) {
                let ok = code.contains("return next(e)")
                    || code
                        .contains("return next({ ...e, props: { ...e.props, isExpanded: true } })");
                if !ok {
                    out.push(report(
                        REGISTER,
                        line,
                        "ToolGroup のフックが next へ流さずに返している（グループを自分で描くと他の mod とエンジンの描画が消える。展開は isExpanded を書き換えて next へ）",
                    ));
                }
            }
            if !w.1.contains("isShowCall(") {
                out.push(report(REGISTER, w.0, "ToolGroup のフックが tako_show_command を含むグループだけに絞っていない（`isShowCall(`）"));
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "ToolGroup のフックが見つからない（走査が空振り）",
        )),
    }
    match window(register, USE_OPEN, REG_CLOSE) {
        Some(w) => {
            if !w.1.contains("await next(e)") {
                out.push(report(
                    REGISTER,
                    w.0,
                    "ToolUse のフックが `await next(e)` で下の答え（他の mod の描画）を受け取っていない",
                ));
            }
            for (line, code) in returns(&w) {
                if !(code.contains("return next(e)")
                    || code.contains("return drawCardRow($, e, view, below)"))
                {
                    out.push(report(
                        REGISTER,
                        line,
                        "ToolUse のフックが next の答えを包まずに返している（返すのは next(e) / drawCardRow( だけ）",
                    ));
                }
            }
            if !w.1.contains("isShowCall(e.props.tool, e.props.input)") {
                out.push(report(
                    REGISTER,
                    w.0,
                    "ToolUse のフックが tako_show_command の show の行だけに絞っていない",
                ));
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "ToolUse のフックが見つからない（走査が空振り）",
        )),
    }
    match window(register, "function drawCardRow(", "}") {
        Some(w) => needs(
            &mut out,
            REGISTER,
            &w,
            &[
                "below.type === 'engine'",
                "children: [below, box]",
                "Code({ source: command.text",
            ],
            "下の mod が描いていればその下に足す・コマンドは論理文字列を Code で",
        ),
        None => out.push(report(
            REGISTER,
            1,
            "drawCardRow が見つからない（走査が空振り）",
        )),
    }
    out
}

/// 4 / 5. 押下と報告
fn scan_press(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, "async function pressCard(", "}") {
        Some(w) => {
            needs(
                &mut out,
                REGISTER,
                &w,
                &[
                    "$.ui.copy({ text: sel.command.text, surface: press.surface })",
                    "if (!copied.isCopied && cli !== undefined)",
                    "$.process.run([cli, ...sel.command.copy_args]",
                    "op === 'run' ? sel.command.run_args : sel.card.dismiss_args",
                    "$.process.run([cli, ...argv]",
                    "lastPress = { kind, ok, at:",
                ],
                "Copy は論理文字列を $.ui.copy へ・拒まれたら tako の CLI・Run / 閉じるは tako が組んだ引数・結果は種類と成否だけ",
            );
            for bad in ["'sh'", "'-c'", "bash", ".join(' ')"] {
                if w.1.contains(bad) {
                    out.push(report(
                        REGISTER,
                        line_of(&w, bad),
                        &format!("pressCard が `{bad}` でコマンドを組んでいる（引数は tako が組む = view.cards。mod でシェルの文字列を作らない）"),
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "pressCard が見つからない（走査が空振り）",
        )),
    }
    match window(register, "function currentRenders(", "}") {
        Some(w) => needs(
            &mut out,
            REGISTER,
            &w,
            &["cards: cardsOffered ? bandCards : undefined"],
            "renders.cards は tako が描かせたときだけ（描かせていない tako にカードを描いたと言わない）",
        ),
        None => out.push(report(REGISTER, 1, "currentRenders が見つからない（走査が空振り）")),
    }
    match window(register, "on('tool.call'", REG_CLOSE) {
        Some(w) => needs(
            &mut out,
            REGISTER,
            &w,
            &["if (SHOW_TOOL.test(e.tool)) dirty = true"],
            "tako_show_command が返ったらすぐ報告する（GPUI の帯を保留する猶予に間に合わせる）",
        ),
        None => out.push(report(
            REGISTER,
            1,
            "tool.call のフックが見つからない（走査が空振り）",
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

/// 6. 応答のカードの契約
fn scan_contract(types: &str, core: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (ts, rs) in [
        ("TakoCards", "CardsView"),
        ("TakoCard", "CardView"),
        ("TakoCardCommand", "CardCommandView"),
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
fn カードの判断と描画の構造が契約どおり() {
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
    let (_, w) = window(&s.register, GROUP_OPEN, REG_CLOSE).unwrap();
    assert!(
        returns(&(0, w.clone())).len() >= 3,
        "ToolGroup の return を拾えていない: {w}"
    );
    let (_, w) = window(&s.register, USE_OPEN, REG_CLOSE).unwrap();
    assert!(
        returns(&(0, w.clone())).len() >= 3,
        "ToolUse の return を拾えていない: {w}"
    );
    let (_, w) = window(&s.register, "async function pressCard(", "}").unwrap();
    assert!(
        w.contains("$.ui.copy(") && w.contains("dirty = true"),
        "窓が短すぎる: {w}"
    );
    let (_, keys) = ts_keys(&s.types, "TakoCardCommand").unwrap();
    assert!(
        keys.contains("text") && keys.contains("run_args"),
        "{keys:?}"
    );
    let (_, keys) = rs_fields(&s.core, "CardView").unwrap();
    assert!(keys.contains("dismiss_args"), "{keys:?}");
    let (_, w) = window(&s.card_ui, "    pub(crate) fn command_card_rows(", "    }").unwrap();
    assert!(w.contains(".collect()"), "窓が短すぎる: {w}");
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
    // 1. GPUI のカード帯が mod の描いたカードも出す（S7-4 前へ戻す = 二重表示・viewport が縮む）
    let found = mutate(&|s| {
        replace_once(
            &mut s.card_ui,
            "            .filter(|c| !by_mod.contains(&c.id().as_u64()))\n",
            "",
        );
    });
    assert_named(&found, CARD_UI, ".filter(|c| !by_mod.contains(");
    // 1b. 2 秒 tick が集合の変化を知らせない（報告の失効・claude の終了で帯へ戻らない）
    let found = mutate(&|s| {
        replace_once(
            &mut s.main,
            "if app.refresh_mod_card_owners() {",
            "if false {",
        );
    });
    assert_named(&found, MAIN, "refresh_mod_card_owners を呼んでいない");
    // 1c. 新鮮さを見ずに報告を引く（mod が止まっても GPUI の帯へ戻らない）
    let found = mutate(&|s| {
        replace_once(
            &mut s.control,
            "        stored: lookup_pane(host, card.pane(), now).ok(),",
            "        stored: host.claude_mod().and_then(|h| h.reports.get(&card.pane().as_u64())),",
        );
    });
    assert_named(&found, CONTROL, "`lookup_pane(`");
    // 1d. A/B を見ない
    let found = mutate(&|s| {
        replace_once(
            &mut s.control,
            "        legacy: core::cards_legacy() || core::s7_legacy(),",
            "        legacy: false,",
        );
    });
    assert_named(&found, CONTROL, "`core::cards_legacy()`");
    // 1e. CLI / MCP の持ち主が GPUI と別の判断
    let found = mutate(&|s| {
        s.dispatch = s.dispatch.replace(
            "crate::claude_mod::card_owner_json(",
            "card_owner_json_local(",
        );
    });
    assert_named(&found, DISPATCH, "card_owner_json を通していない");
    // 2. env を tako-core の外で読む
    let found = mutate(&|s| {
        let (_, src) = s
            .others
            .iter_mut()
            .find(|(f, _)| f == CARD_UI)
            .expect("CARD_UI は走査の対象");
        src.push_str("\nfn leak() -> bool { std::env::var(\"TAKO_1963_LEGACY\").is_ok() }\n");
    });
    assert_named(&found, CARD_UI, "TAKO_1963_LEGACY");
    // 3. 会話の行: グループを自分で描く・ToolUse が下の答えを捨てる
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    return next({ ...e, props: { ...e.props, isExpanded: true } })",
            "    return $.ui.resolve(e).Text({ children: 'tako card' })",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "ToolGroup のフックが next へ流さずに返している",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    const below = await next(e)\n    return drawCardRow($, e, view, below)",
            "    return drawCardRow($, e, view, { type: 'engine', ref: 0 })",
        );
    });
    assert_named(&found, REGISTER, "`await next(e)`");
    assert_named(
        &found,
        REGISTER,
        "ToolUse のフックが next の答えを包まずに返している",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "  return Box({ flexDirection: 'column', children: [below, box] })",
            "  return box",
        );
    });
    assert_named(&found, REGISTER, "children: [below, box]");
    // 4. Copy が拒否を無視する・シェルの文字列を組む
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "      if (!copied.isCopied && cli !== undefined) {",
            "      if (false) {",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "if (!copied.isCopied && cli !== undefined)",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "      const done = await $.process.run([cli, ...argv], { timeoutMs: PRESS_TIMEOUT_MS })",
            "      const done = await $.process.run(['sh', '-c', sel.command.text], { timeoutMs: PRESS_TIMEOUT_MS })",
        );
    });
    assert_named(&found, REGISTER, "`'sh'`");
    assert_named(&found, REGISTER, "$.process.run([cli, ...argv]");
    // 5. 描かせていない tako にもカードを描いたと言う・呼び出しの後に報告を急がない
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    cards: cardsOffered ? bandCards : undefined,",
            "    cards: bandCards,",
        );
    });
    assert_named(
        &found,
        REGISTER,
        "cards: cardsOffered ? bandCards : undefined",
    );
    let found = mutate(&|s| {
        replace_once(
            &mut s.register,
            "    if (SHOW_TOOL.test(e.tool)) dirty = true\n",
            "",
        );
    });
    assert_named(&found, REGISTER, "if (SHOW_TOOL.test(e.tool)) dirty = true");
    // 6. 契約の片側だけを変える
    let found = mutate(&|s| {
        replace_once(
            &mut s.types,
            "  copy_args: string[]\n",
            "  copy_argv: string[]\n",
        );
    });
    assert_named(&found, TYPES, "TakoCardCommand（TS）と CardCommandView");
}
