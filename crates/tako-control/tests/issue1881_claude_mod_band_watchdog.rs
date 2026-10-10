//! tako mod S3（#1881。FR-2.42.13〜17）の番犬: Claude Code の画面の帯とサイドバー
//!
//! # なぜ要るか
//!
//! 帯は**利用者の Claude Code の画面の中**に出るので、崩れ方が利用者の作業へ直に出る:
//! 帯の木に行を 1 つ足すだけで、21 行のペインでは Claude Code が `↓ N more` に畳み（S0 の実測）、
//! サイドバーを `session.start` から開けば 144 桁以上の端末で頼まれずに居座る。帯に ctx の閾値の
//! 判断を mod 側で書き足すと、tako の閾値（とそれを見せる `tako mod`）と食い違う。どれも
//! `claude plugin test` の個々の期待値は通りうるので、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. tako の行は 1 行: S7-3（#1962）から帯は他の mod の行を包むので、縛るのは tako の行。
//!    `bandRow`（区切りの Text とボタンの横並び）は `flexDirection: 'row'` で縦に並べず、A/B の
//!    `drawBandS3` は `wrap: 'truncate-end'` の `Text` 1 本で `Box` / `flexDirection` / `Button` を
//!    使わない。どちらも幅に合わせて詰め（`fitBand(`）、帯のフックは幅（`bodyColumns`）を読み、
//!    調査票（`hasSurvey`）には譲り、木を直に組まない（`next` を包むことは
//!    `issue1962_mod_band_buttons_watchdog` が縛る）
//! 2. 帯に出すものの判断は tako: `segmentsOf` は `view.warnings` だけを読み、`view.ctx` /
//!    `view.rate_limits`（サイドバー用の全部入り）や worker の状態（承認待ち）を見ない
//! 3. サイドバーは頼まれずに開かない: `$.ui.open(` は `command.run`（`/tako`）の登録の中だけ
//! 4. 帯の準備の失敗が報告（S1 / S2）を止めない: `wake` は報告の timer を張ってから `try` の中で
//!    `$.store` / `/tako` の登録（`registerCommand`）をし、`syncStore` は `$.store.get` を `try` で包む
//! 5. 契約の一致: TS の `TakoView` / `TakoBand` / `TakoWorker` のキーと Rust の `BandView` /
//!    `ModBand` / `BandWorker` のフィールドが同じ集合。帯の状態（`TakoBand` / `ModBand`）は
//!    描いた文字列を持たない
//! 6. A/B（`TAKO_1877_S3_LEGACY`）と検証用の閾値（`TAKO_1881_BAND_THRESHOLD`）の env の名前を
//!    文字列で読むのは `tako_core::claude_mod` の 1 か所だけ。応答の組み立て（`snapshot`）は
//!    `legacy` のとき `view` を載せない
//! 7. worker の数え方は 1 実装: 帯（`claude_mod::band_view`）と右パネル orch が
//!    `Workspace::workers_of(` を通し、orch に inline の規則（`master_count`）を戻さない
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
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";
/// env の名前を直に読んではいけない側（[`CORE`] の外のソース）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const ENVS: &[&str] = &["\"TAKO_1877_S3_LEGACY\"", "\"TAKO_1881_BAND_THRESHOLD\""];
/// 帯の状態に置いてはならないキー（描いた文字列・名前）
const TEXT_KEYS: &[&str] = &["text", "line", "label", "title", "name", "content", "body"];

const BAND_OPEN: &str = "on('ui.render', { component: 'AbovePrompt' }";
const PANE_OPEN: &str = "on('ui.render', { component: 'Pane'";
const COMMAND_OPEN: &str = "on('command.run', { command: 'tako' }";
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
    right_panel: String,
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
        right_panel: read(RIGHT_PANEL),
        others,
    }
}

/// 1. 帯は 1 行（`Text` 1 本・幅に合わせて詰める・調査票に譲る）
fn scan_band(register: &str) -> Vec<String> {
    // `barText(` のような別の名前の末尾には当てない（直前が識別子の文字でない呼び出しだけ）
    let calls = |src: &str, name: &str| {
        src.match_indices(name).find_map(|(at, _)| {
            let before = src[..at].chars().next_back();
            (!before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| src[..at].lines().count().max(1))
        })
    };
    let mut out = Vec::new();
    match window(register, BAND_OPEN, REG_CLOSE) {
        Some(w) => {
            for bad in ["Box(", "Button(", "Text("] {
                if let Some(nth) = calls(&w.1, bad) {
                    out.push(report(
                        REGISTER,
                        w.0 + nth - 1,
                        &format!("帯のフックが `{bad}` で木を直に組んでいる（tako の行は bandRow / drawBandS3 だけが組む = 1 行の形を 1 か所で縛る）"),
                    ));
                }
            }
            for need in ["e.props.bodyColumns", "e.props.hasSurvey"] {
                if !w.1.contains(need) {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!(
                            "帯の描画に `{need}` が無い（幅に合わせて 1 行に詰め、調査票に譲る）"
                        ),
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "帯の描画（AbovePrompt）が見つからない（走査が空振り）",
        )),
    }
    // tako の行（S7-3 #1962）: 横に 1 行。縦に並べると 21 行のペインで `↓ N more` に畳まれる
    match window(register, "function bandRow(", "}") {
        Some(w) => {
            for need in ["wrap: 'truncate-end'", "fitBand(", "flexDirection: 'row'"] {
                if !w.1.contains(need) {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!(
                            "tako の行に `{need}` が無い（区切りとボタンを横に 1 行で詰める）"
                        ),
                    ));
                }
            }
            if w.1.contains("'column'") {
                out.push(report(
                    REGISTER,
                    line_of(&w, "'column'"),
                    "tako の行が縦に並ぶ（`'column'`。帯の行数は他の mod と共有 = tako はボタン込みで 1 行）",
                ));
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "bandRow が見つからない（走査が空振り）",
        )),
    }
    // A/B（TAKO_1877_S7_LEGACY）の S3 の描き方: Text 1 本
    match window(register, "function drawBandS3(", "}") {
        Some(w) => {
            for bad in ["Box(", "flexDirection", "Button("] {
                if w.1.contains(bad) {
                    out.push(report(
                        REGISTER,
                        line_of(&w, bad),
                        &format!("S3 の帯の描画に `{bad}` がある（S3 の帯は 1 行の Text 1 本。行を足すと 21 行のペインで `↓ N more` に畳まれる）"),
                    ));
                }
            }
            for need in ["wrap: 'truncate-end'", "return Text(", "fitBand("] {
                if !w.1.contains(need) {
                    out.push(report(
                        REGISTER,
                        w.0,
                        &format!("S3 の帯の描画に `{need}` が無い（幅に合わせて 1 行に詰める）"),
                    ));
                }
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "drawBandS3 が見つからない（走査が空振り）",
        )),
    }
    out
}

/// 2. 帯に出すものの判断は tako（mod は view.warnings だけを読む）
fn scan_segments(register: &str) -> Vec<String> {
    let Some(w) = window(register, "function segmentsOf(", "}") else {
        return vec![report(
            REGISTER,
            1,
            "segmentsOf が見つからない（走査が空振り）",
        )];
    };
    let mut out = Vec::new();
    if !w.1.contains("view.warnings") {
        out.push(report(
            REGISTER,
            w.0,
            "帯の警告を view.warnings から読んでいない（閾値の判断は tako 側 = tako mod と食い違わない）",
        ));
    }
    for bad in [
        "view.ctx",
        "view.rate_limits",
        "thresholds",
        "permission",
        "承認",
        ".state",
    ] {
        if w.1.contains(bad) {
            out.push(report(
                REGISTER,
                line_of(&w, bad),
                &format!("帯の区切りが `{bad}` を見ている（閾値の判断は tako の view.warnings だけ・承認待ちは帯に出さない）"),
            ));
        }
    }
    out
}

/// 3. サイドバーは頼まれずに開かない（`$.ui.open(` は `/tako` の中だけ）
fn scan_open(register: &str) -> Vec<String> {
    let Some(cmd) = window(register, COMMAND_OPEN, REG_CLOSE) else {
        return vec![report(
            REGISTER,
            1,
            "`/tako` の登録が見つからない（走査が空振り）",
        )];
    };
    let inside = cmd.0..cmd.0 + cmd.1.lines().count();
    register
        .lines()
        .enumerate()
        .filter(|(i, l)| l.contains("$.ui.open(") && !inside.contains(&(i + 1)))
        .map(|(i, _)| {
            report(
                REGISTER,
                i + 1,
                "`/tako` の外で `$.ui.open(` を呼んでいる（サイドバーは頼まれずに開かない = 144 桁以上の端末で居座る）",
            )
        })
        .collect()
}

/// 4. 帯の準備の失敗が報告を止めない
fn scan_wake(register: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, "async function wake(", "}") {
        Some(w) => {
            let at = |needle: &str| w.1.find(needle);
            match (
                at("$.clock.every("),
                at("try {"),
                at("syncStore("),
                at("registerCommand("),
            ) {
                (Some(timer), Some(try_at), Some(sync), Some(reg))
                    if timer < try_at && try_at < sync && try_at < reg => {}
                _ => out.push(report(
                    REGISTER,
                    line_of(&w, "syncStore("),
                    "wake が報告の timer を張る前に、または try の外で $.store / /tako の登録をしている（帯の準備の失敗が報告を止める）",
                )),
            }
        }
        None => out.push(report(REGISTER, 1, "wake が見つからない（走査が空振り）")),
    }
    match window(register, "async function syncStore(", "}") {
        Some(w) => {
            let try_at = w.1.find("try {");
            let get_at = w.1.find("$.store.get(");
            if !matches!((try_at, get_at), (Some(t), Some(g)) if t < g) {
                out.push(report(
                    REGISTER,
                    line_of(&w, "$.store.get("),
                    "syncStore が $.store.get を try で包んでいない（読めないと報告ごと止まる）",
                ));
            }
        }
        None => out.push(report(
            REGISTER,
            1,
            "syncStore が見つからない（走査が空振り）",
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

/// 5. 契約の一致と、帯の状態が文字列を持たないこと
fn scan_contract(types: &str, core: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (ts, rs) in [
        ("TakoView", "BandView"),
        ("TakoBand", "ModBand"),
        ("TakoWorker", "BandWorker"),
    ] {
        match (ts_keys(types, ts), rs_fields(core, rs)) {
            (Some((ts_at, ts_keys)), Some((rs_at, rs_keys))) => {
                if ts_keys != rs_keys {
                    let only_ts: Vec<_> = ts_keys.difference(&rs_keys).collect();
                    let only_rs: Vec<_> = rs_keys.difference(&ts_keys).collect();
                    out.push(report(
                        TYPES,
                        ts_at,
                        &format!(
                            "{ts}（TS）と {rs}（{CORE}:{rs_at}）のキーが食い違う（TS だけ {only_ts:?} / Rust だけ {only_rs:?}）"
                        ),
                    ));
                }
                if ts == "TakoBand" {
                    for key in TEXT_KEYS {
                        if ts_keys.contains(*key) {
                            out.push(report(
                                TYPES,
                                ts_at,
                                &format!(
                                    "帯の状態に `{key}` がある（描いた文字列は報告に載せない）"
                                ),
                            ));
                        }
                        if rs_keys.contains(*key) {
                            out.push(report(
                                CORE,
                                rs_at,
                                &format!(
                                    "帯の状態に `{key}` がある（描いた文字列は報告に載せない）"
                                ),
                            ));
                        }
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
    out
}

/// 6. env の名前は tako_core::claude_mod の 1 か所・legacy で view を載せない
fn scan_env(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for (file, src) in &s.others {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            for env in ENVS {
                if code.contains(env) {
                    out.push(report(
                        file,
                        i + 1,
                        &format!("{env} を {CORE} の外で読んでいる（A/B・検証用の env の名前は 1 か所 = s3_legacy / band_thresholds を通す）"),
                    ));
                }
            }
        }
    }
    match window(&s.control, "pub(crate) fn snapshot(", "}") {
        Some(w) => {
            if !(w.1.contains("!legacy") && w.1.contains("band_view(")) {
                out.push(report(
                    CONTROL,
                    w.0,
                    "snapshot が legacy（TAKO_1877_S3_LEGACY）でも帯の材料を載せうる",
                ));
            }
        }
        None => out.push(report(
            CONTROL,
            1,
            "snapshot が見つからない（走査が空振り）",
        )),
    }
    if !s
        .control
        .contains("snapshot(host, pane_id, accepted, core::s3_legacy())")
    {
        out.push(report(
            CONTROL,
            1,
            "accept_report が A/B（core::s3_legacy()）を snapshot へ渡していない",
        ));
    }
    out
}

/// 7. worker の数え方は 1 実装
fn scan_workers(control: &str, right_panel: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(control, "fn band_view(", "}") {
        Some(w) if w.1.contains(".workers_of(") => {}
        Some(w) => out.push(report(
            CONTROL,
            w.0,
            "帯の worker を Workspace::workers_of で引いていない（右パネル orch と数が食い違う）",
        )),
        None => out.push(report(
            CONTROL,
            1,
            "band_view が見つからない（走査が空振り）",
        )),
    }
    match window(right_panel, "fn render_orch_view(", "    }") {
        Some(w) => {
            if !w.1.contains(".workers_of(") {
                out.push(report(
                    RIGHT_PANEL,
                    w.0,
                    "orch ビューが Workspace::workers_of を通していない（帯と数が食い違う）",
                ));
            }
            if w.1.contains("master_count") {
                out.push(report(
                    RIGHT_PANEL,
                    line_of(&w, "master_count"),
                    "orch ビューに worker の紐づけ規則（master_count）が inline で戻っている（1 実装は Workspace::workers_of）",
                ));
            }
        }
        None => out.push(report(
            RIGHT_PANEL,
            1,
            "render_orch_view が見つからない（走査が空振り）",
        )),
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_band(&s.register);
    out.extend(scan_segments(&s.register));
    out.extend(scan_open(&s.register));
    out.extend(scan_wake(&s.register));
    out.extend(scan_contract(&s.types, &s.core));
    out.extend(scan_env(s));
    out.extend(scan_workers(&s.control, &s.right_panel));
    out
}

#[test]
fn 帯とサイドバーの構造が契約どおり() {
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
    for (src, needle) in [
        (&s.register, BAND_OPEN),
        (&s.register, PANE_OPEN),
        (&s.register, COMMAND_OPEN),
        (&s.register, "function segmentsOf("),
        (&s.register, "async function wake("),
        (&s.register, "async function syncStore("),
        (&s.control, "pub(crate) fn snapshot("),
        (&s.control, "fn band_view("),
        (&s.right_panel, "fn render_orch_view("),
    ] {
        assert!(src.contains(needle), "{needle} が見つからない");
    }
    // 契約の窓に実際のキーが並んでいる
    let (_, view) = ts_keys(&s.types, "TakoView").unwrap();
    assert!(
        view.contains("warnings") && view.contains("worker_count"),
        "{view:?}"
    );
    let (_, band) = rs_fields(&s.core, "ModBand").unwrap();
    assert!(
        band.contains("segments") && band.contains("hidden"),
        "{band:?}"
    );
    // 帯の窓（フック・tako の行・S3 の描き方）の中身が採れている
    let (_, w) = window(&s.register, BAND_OPEN, REG_CLOSE).unwrap();
    assert!(
        w.contains("e.props.bodyColumns") && w.contains("bandRow("),
        "{w}"
    );
    let (_, w) = window(&s.register, "function bandRow(", "}").unwrap();
    assert!(w.contains("fitBand(") && w.contains("Button("), "{w}");
    let (_, w) = window(&s.register, "function drawBandS3(", "}").unwrap();
    assert!(w.contains("fitBand(") && w.contains("return Text("), "{w}");
    // env の走査の対象が採れている
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
    // 1. S3 の帯に 2 行目（Box の縦並び）を足す
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  return Text({ dimColor: true, wrap: 'truncate-end', children: parts })",
            "  return Box({ flexDirection: 'column', children: [Text({ children: parts }), Text({ children: 'more' })] })",
            1,
        );
    });
    assert_named(&found, REGISTER, "`Box(`");
    assert_named(&found, REGISTER, "wrap: 'truncate-end'");
    // 1a. tako の行（S7-3）を縦に並べる（ボタンを 2 行目へ）
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  return Box({ flexDirection: 'row', columnGap: BUTTON_GAP, children: [line, ...buttons] })",
            "  return Box({ flexDirection: 'column', children: [line, ...buttons] })",
            1,
        );
    });
    assert_named(&found, REGISTER, "`'column'`");
    assert_named(&found, REGISTER, "flexDirection: 'row'");
    // 1c. 帯のフックで木を直に組む（tako の行の形を 2 つの関数の外へ散らす）
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    if (view.band_style === 's3') return drawBandS3($, e, view, columns)",
            "    if (view.band_style === 's3') return $.ui.resolve(e).Text({ children: 'tako' })",
            1,
        );
    });
    assert_named(&found, REGISTER, "`Text(`");
    // 1b. 調査票に譲らない
    let found = mutate(&|s| {
        s.register = s.register.replacen(" || e.props.hasSurvey", "", 1);
    });
    assert_named(&found, REGISTER, "e.props.hasSurvey");
    // 2. 帯の区切りが mod 側で閾値を判断する
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "    view.warnings.forEach((warn, i) => out.push(warning(warn, i, 3)))",
            "    if ((view.ctx?.percent ?? 0) >= 80) out.push({ kind: 'ctx', label: 'ctx', rank: 3 })\n    view.warnings.forEach((warn, i) => out.push(warning(warn, i, 3)))",
            1,
        );
    });
    assert_named(&found, REGISTER, "`view.ctx`");
    // 3. サイドバーを頼まれずに開く（session.start = wake から）
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  timer = $.clock.every(FLUSH_MS, () => {",
            "  void $.ui.open({ id: PANE_ID, title: 'tako' })\n  timer = $.clock.every(FLUSH_MS, () => {",
            1,
        );
    });
    assert_named(&found, REGISTER, "`$.ui.open(`");
    // 4. 帯の準備を try の外・timer の前へ
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  timer = $.clock.every(FLUSH_MS, () => {",
            "  await syncStore($)\n  timer = $.clock.every(FLUSH_MS, () => {",
            1,
        );
    });
    assert_named(&found, REGISTER, "wake が報告の timer");
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  let stored: unknown\n  try {\n    stored = await $.store.get(BAND_KEY)\n  } catch {\n    return\n  }",
            "  const stored: unknown = await $.store.get(BAND_KEY)",
            1,
        );
    });
    assert_named(&found, REGISTER, "syncStore");
    // 5. 契約の片側だけにキーを足す・帯の状態に描いた文字列を足す
    let found = mutate(&|s| {
        s.types = s.types.replacen(
            "  worker_count: number\n",
            "  worker_count: number\n  extra: number\n",
            1,
        );
    });
    assert_named(&found, TYPES, "TakoView（TS）と BandView");
    let found = mutate(&|s| {
        s.core = s.core.replacen(
            "    pub shown: bool,\n",
            "    pub shown: bool,\n    pub line: String,\n",
            1,
        );
        s.types = s.types.replacen(
            "  shown: boolean\n",
            "  shown: boolean\n  line: string\n",
            1,
        );
    });
    assert_named(&found, TYPES, "`line`");
    assert_named(&found, CORE, "`line`");
    // 6. A/B の env を CORE の外で読む・legacy でも view を載せる
    let found = mutate(&|s| {
        let (_, src) = s
            .others
            .iter_mut()
            .find(|(f, _)| f == CONTROL)
            .expect("CONTROL は走査の対象");
        src.push_str("\nfn leak() -> bool { std::env::var(\"TAKO_1877_S3_LEGACY\").is_ok() }\n");
    });
    assert_named(&found, CONTROL, "TAKO_1877_S3_LEGACY");
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "if accepted == Accepted::Stored && !legacy {",
            "if accepted == Accepted::Stored {",
            1,
        );
    });
    assert_named(&found, CONTROL, "snapshot が legacy");
    // 7. orch に inline の規則を戻す・帯が workers_of を通さない
    let found = mutate(&|s| {
        s.right_panel = s.right_panel.replacen(
            "                        .workers_of(pane.id())",
            "                        .workers_of(pane.id()); let master_count = 1; let _ = master_count",
            1,
        );
    });
    assert_named(&found, RIGHT_PANEL, "master_count");
    let found = mutate(&|s| {
        s.control = s
            .control
            .replacen(".workers_of(pane)", ".panes_of(pane)", 1);
    });
    assert_named(&found, CONTROL, "workers_of");
}
