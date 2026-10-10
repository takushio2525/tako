//! tako mod S7-1（#1959。FR-2.42.25〜）の番犬: Claude Code の設定 dir の `skills/tako` への導入
//!
//! # なぜ要るか
//!
//! この機能は**利用者の Claude Code の設定 dir に書く**唯一の経路で、崩れ方が利用者の環境へ直に出る:
//! 番人（`write_guard`）を外せば検証プロセスが本物の `~/.claude*` へ写しを置き、走っている全セッションに
//! ホットリロードで効く。設定 dir へ書く道が 2 本に増えれば、管理印・衝突の検出・差し替えの作法を
//! 片方が欠く。mod 側の休眠を外せば、利用者が `/plugin` で止めた tako が env の注入で動き続ける。
//! どれも単体テストの期待値は通りうるので、ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 書くのは 1 実装: 設定 dir の `skills/` へのパス（`.join("skills")`）と管理印の
//!    名前（`".tako-managed"`）を書くのは `tako_core::claude_mod_install` だけ。`core::apply(` を呼ぶのは
//!    `tako_control::claude_mod_install` の `sync_targets` / `uninstall_targets` だけ
//! 2. 番人を通す: `Context::blocked` と `uninstall_targets` は `write_guard(` を通す。既定の設定 dir は
//!    `default_config_dir()` を通す（`claude_default_config_dir()` を `target_inputs` が直に使わない）
//! 3. settings 系のファイルを書かない: core の書き込み（`fs::write` / `rename` / `remove_dir_all` /
//!    `create_dir_all`）は `write_copy` / `remove_copy` / `sweep_leftovers` の中だけ
//! 4. 配線: setup が `run_setup_stage(`、GUI が `sync_for_gui(`（起動時と差分検出の 2 か所）と
//!    `refine_injection(`、`tako mod`（status）が `status_json(`、MCP の action に install / uninstall
//! 5. mod の休眠: `wake` は timer を張る前に `userDisabled($)` を見る・報告に `dormant` を載せる・
//!    TS の `SKILLS_ID` と Rust の `SKILLS_PLUGIN_ID` が同じ
//! 6. 検証用の env（版の差し替え・途中で落とす）は検証プロセスでだけ効き、env の名前を文字列で読むのは
//!    core の 1 か所だけ
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const CORE: &str = "crates/tako-core/src/claude_mod_install.rs";
const CONTROL: &str = "crates/tako-control/src/claude_mod_install.rs";
const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const SETUP: &str = "crates/tako-cli/src/setup.rs";
const APP: &str = "crates/tako-app/src/main.rs";
const REGISTER: &str = "crates/tako-core/claude-mod/hooks/register.ts";
/// 設定 dir の skills/ へ書いてはいけない側（[`CORE`] の外）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-core/src",
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const ENVS: &[&str] = &[
    "\"TAKO_1959_LEGACY\"",
    "\"TAKO_1959_CONFIG_DIRS\"",
    "\"TAKO_1959_MOD_VERSION\"",
    "\"TAKO_1959_INJECT_CRASH\"",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` を含む行から、`close` と一致する最初の行までの窓（1 始まりの行番号つき）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| *l == close)
        .map(|i| start + 1 + i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 本番コードだけの眺め（テスト領域を空白へ潰す。行番号は保たれる = #1420 の 1 実装）
fn product_part(src: &str) -> String {
    production_range::scan(src).text
}

/// `i` 行目（0 始まり）を中に持つ関数の名前（その行より前で最も近い関数の頭。#1496 の 1 実装）
fn owner_fn<'a>(lines: &[&'a str], i: usize) -> &'a str {
    lines[..i]
        .iter()
        .rev()
        .find_map(|l| fn_head_name(l))
        .unwrap_or("")
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
}

struct Sources {
    core: String,
    control: String,
    control_mod: String,
    setup: String,
    app: String,
    register: String,
    /// [`SOURCE_DIRS`] の .rs（相対パス, 本番コードの眺め = [`product_part`] 済み）。プロセスで
    /// 1 回だけ作って共有する（注入のたびに全部を作り直すと 1 つで数秒かかる）。
    /// [`CORE`] / [`CONTROL`] / [`APP`] は [`others_of`] が差し替えた中身から作り直す
    others: &'static [(String, String)],
    /// 注入の検査が [`Sources::app`] を差し替えたか（眺めを作り直すのはそのときだけ。巨大なため）
    app_dirty: bool,
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

fn others_views() -> &'static [(String, String)] {
    static VIEWS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    VIEWS.get_or_init(|| {
        let root = workspace_root();
        let mut files = Vec::new();
        for dir in SOURCE_DIRS {
            rs_files(&root.join(dir), &mut files);
        }
        files.sort();
        files
            .iter()
            .map(|p| {
                let rel = p
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let raw = std::fs::read_to_string(p).unwrap();
                // 探す字面を 1 つも含まないファイルは眺めを作らない（違反は出ようが無い。
                // 巨大なファイルの走査がデバッグビルドで秒単位かかるのを避ける）
                let relevant = SCANNED_NEEDLES.iter().any(|n| raw.contains(n))
                    || ENVS.iter().any(|n| raw.contains(n));
                (
                    rel,
                    if relevant {
                        product_part(&raw)
                    } else {
                        String::new()
                    },
                )
            })
            .collect()
    })
}

/// [`scan_single_writer`] が全ソースで探す字面（[`ENVS`] は [`scan_env`]）
const SCANNED_NEEDLES: &[&str] = &[
    ".join(\"skills\")",
    "\".tako-managed\"",
    "claude_mod_install::apply(",
];

fn current() -> Sources {
    Sources {
        core: read(CORE),
        control: read(CONTROL),
        control_mod: read(CONTROL_MOD),
        setup: read(SETUP),
        app: read(APP),
        register: read(REGISTER),
        others: others_views(),
        app_dirty: false,
    }
}

/// 本番コードの眺めの一覧。core / control（と差し替えたときの app）は注入の検査で中身を
/// 変えるので作り直し、残りは [`others_views`] の共有のものを使う
fn others_of(s: &Sources) -> Vec<(&str, std::borrow::Cow<'_, str>)> {
    s.others
        .iter()
        .map(|(rel, text)| {
            let view = match rel.as_str() {
                CORE => std::borrow::Cow::Owned(product_part(&s.core)),
                CONTROL => std::borrow::Cow::Owned(product_part(&s.control)),
                APP if s.app_dirty => std::borrow::Cow::Owned(product_part(&s.app)),
                _ => std::borrow::Cow::Borrowed(text.as_str()),
            };
            (rel.as_str(), view)
        })
        .collect()
}

// --- 1. 書くのは 1 実装 ---------------------------------------------------------------

fn scan_single_writer(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, product) in others_of(s) {
        for (i, line) in product.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            if rel != CORE
                && (line.contains(".join(\"skills\")") || line.contains("\".tako-managed\""))
            {
                out.push(report(
                    rel,
                    i + 1,
                    "設定 dir の skills/ と管理印を書くのは tako_core::claude_mod_install だけ（`.join(\"skills\")` / `\".tako-managed\"`）",
                ));
            }
            if rel != CONTROL && line.contains("claude_mod_install::apply(") {
                out.push(report(
                    rel,
                    i + 1,
                    "`claude_mod_install::apply(` を呼ぶのは tako_control::claude_mod_install の sync_targets / uninstall_targets だけ",
                ));
            }
        }
    }
    let product = product_part(&s.control);
    let lines: Vec<&str> = product.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("core::apply(") && !is_comment(line) {
            let owner = owner_fn(&lines, i);
            if owner != "sync_targets" && owner != "uninstall_targets" {
                out.push(report(
                    CONTROL,
                    i + 1,
                    "`core::apply(` は sync_targets / uninstall_targets の中だけ（判断と番人を通らない書き込みを作らない）",
                ));
            }
        }
    }
    out
}

// --- 2. 番人を通す ---------------------------------------------------------------------

fn scan_guard(control: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (needle, close) in [
        ("    fn blocked(&self, dir: &Path)", "    }"),
        ("pub fn uninstall_targets(", "}"),
    ] {
        match window(control, needle, close) {
            Some((line, w)) if !w.contains("write_guard(") => out.push(report(
                CONTROL,
                line,
                "書く前に `write_guard(` を通す（検証プロセスは一時 dir の外へ書かない）",
            )),
            Some(_) => {}
            None => out.push(report(CONTROL, 1, &format!("{needle} が見つからない"))),
        }
    }
    match window(control, "pub fn target_inputs(", "}") {
        Some((line, w)) => {
            if !w.contains("default_config_dir()") || w.contains("claude_default_config_dir()") {
                out.push(report(
                    CONTROL,
                    line,
                    "既定の設定 dir は `default_config_dir()` を通す（検証プロセスで本物の ~/.claude を置く先にしない）",
                ));
            }
        }
        None => out.push(report(CONTROL, 1, "pub fn target_inputs( が見つからない")),
    }
    out
}

// --- 3. settings 系のファイルを書かない ---------------------------------------------------

const WRITERS: &[&str] = &[
    "fs::write(",
    "fs::rename(",
    "fs::remove_dir_all(",
    "fs::create_dir_all(",
    "fs::remove_file(",
    "fs::copy(",
];
const ALLOWED_WRITER_FNS: &[&str] = &["write_copy", "remove_copy", "sweep_leftovers"];

fn scan_core_writes(core: &str) -> Vec<String> {
    let mut out = Vec::new();
    let product = product_part(core);
    let lines: Vec<&str> = product.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if is_comment(line) || !WRITERS.iter().any(|w| line.contains(w)) {
            continue;
        }
        if !ALLOWED_WRITER_FNS.contains(&owner_fn(&lines, i)) {
            out.push(report(
                CORE,
                i + 1,
                "設定 dir への書き込みは write_copy / remove_copy / sweep_leftovers の中だけ（settings 系のファイルは読むだけ）",
            ));
        }
    }
    out
}

// --- 4. 配線 -----------------------------------------------------------------------------

fn scan_wiring(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    let mut need = |file: &str, src: &str, needle: &str, at_least: usize, why: &str| {
        let n = src
            .lines()
            .filter(|l| !is_comment(l) && l.contains(needle))
            .count();
        if n < at_least {
            out.push(report(file, 1, why));
        }
    };
    need(
        SETUP,
        &s.setup,
        "claude_mod_install::run_setup_stage(",
        1,
        "setup の段が `claude_mod_install::run_setup_stage(` を呼ばない（setup で入らない）",
    );
    need(
        APP,
        &s.app,
        "claude_mod_install::sync_for_gui(",
        2,
        "GUI が起動時と差分検出の 2 か所で `claude_mod_install::sync_for_gui(` を呼ばない",
    );
    need(
        APP,
        &s.app,
        "claude_mod_install::refine_injection(",
        1,
        "ペインの注入の判断が `claude_mod_install::refine_injection(` を通らない（衝突しても注入する）",
    );
    need(
        CONTROL_MOD,
        &s.control_mod,
        "claude_mod_install::status_json(",
        1,
        "`tako mod`（status）が `claude_mod_install::status_json(` を載せない",
    );
    // 1 行でも複数行でも、宣言の行から `];` で終わる行まで
    let lines: Vec<&str> = s.control_mod.lines().collect();
    match lines
        .iter()
        .position(|l| l.starts_with("pub const MCP_ACTIONS"))
    {
        Some(start) => {
            let end = lines[start..]
                .iter()
                .position(|l| l.trim_end().ends_with("];"))
                .map_or(lines.len(), |i| start + i + 1);
            let decl = lines[start..end].join("\n");
            if !decl.contains("\"install\"") || !decl.contains("\"uninstall\"") {
                out.push(report(
                    CONTROL_MOD,
                    start + 1,
                    "MCP の action に install / uninstall が無い（設計原則 5: CLI の操作は MCP からも）",
                ));
            }
        }
        None => out.push(report(
            CONTROL_MOD,
            1,
            "pub const MCP_ACTIONS が見つからない",
        )),
    }
    out
}

// --- 5. mod の休眠 ---------------------------------------------------------------------

fn scan_dormant(register: &str, core: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(register, "async function wake(", "}") {
        Some((line, w)) => {
            let disabled = w.find("await userDisabled($)");
            let timer = w.find("timer = $.clock.every(");
            if disabled.is_none() || timer.is_none() || disabled > timer {
                out.push(report(
                    REGISTER,
                    line,
                    "wake は timer を張る前に `await userDisabled($)` を見る（/plugin で止めた利用者の選択に従う）",
                ));
            }
        }
        None => out.push(report(REGISTER, 1, "async function wake( が見つからない")),
    }
    match window(register, "async function buildReport(", "}") {
        Some((line, w)) if !w.contains("    dormant,") => out.push(report(
            REGISTER,
            line,
            "報告に `dormant` を載せる（tako が user_disabled を出せない）",
        )),
        Some(_) => {}
        None => out.push(report(
            REGISTER,
            1,
            "async function buildReport( が見つからない",
        )),
    }
    let ts_id = register
        .lines()
        .position(|l| l.starts_with("const SKILLS_ID = "))
        .map(|i| (i + 1, register.lines().nth(i).unwrap_or_default()));
    let rs_ok = core.contains("pub const SKILLS_PLUGIN_ID: &str = \"tako@skills-dir\";");
    match ts_id {
        Some((_, l)) if l.contains("'tako@skills-dir'") && rs_ok => {}
        Some((line, _)) => out.push(report(
            REGISTER,
            line,
            "TS の SKILLS_ID と Rust の SKILLS_PLUGIN_ID が tako@skills-dir で揃っていない",
        )),
        None => out.push(report(REGISTER, 1, "const SKILLS_ID が見つからない")),
    }
    out
}

// --- 6. 検証用の env ---------------------------------------------------------------------

fn scan_env(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for (rel, text) in others_of(s) {
        if rel == CORE {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if let Some(env) = ENVS.iter().find(|e| line.contains(*e)) {
                out.push(report(
                    rel,
                    i + 1,
                    &format!(
                        "{env} を文字列で読むのは tako_core::claude_mod_install の 1 か所だけ"
                    ),
                ));
            }
        }
    }
    for (needle, close) in [
        ("    pub fn current() -> Self {", "    }"),
        ("fn inject_crash(", "}"),
    ] {
        match window(&s.core, needle, close) {
            Some((line, w)) if !w.contains("is_verification_process()") => out.push(report(
                CORE,
                line,
                "検証用の env は `is_verification_process()` のときだけ効かせる（製品の経路で版を偽らない・落ちない）",
            )),
            Some(_) => {}
            None => out.push(report(CORE, 1, &format!("{needle} が見つからない"))),
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_single_writer(s);
    out.extend(scan_guard(&s.control));
    out.extend(scan_core_writes(&s.core));
    out.extend(scan_wiring(s));
    out.extend(scan_dormant(&s.register, &s.core));
    out.extend(scan_env(s));
    out
}

#[test]
fn 導入の構造が契約どおり() {
    let found = all(&current());
    assert!(
        found.is_empty(),
        "tako mod の skills-dir への導入（#1959）の構造が崩れている:\n{}",
        found.join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    for (src, needle) in [
        (&s.control, "    fn blocked(&self, dir: &Path)"),
        (&s.control, "pub fn uninstall_targets("),
        (&s.control, "pub fn target_inputs("),
        (&s.core, "pub fn write_copy("),
        (&s.core, "    pub fn current() -> Self {"),
        (&s.core, "fn inject_crash("),
        (&s.register, "async function wake("),
        (&s.register, "async function buildReport("),
        (&s.control_mod, "pub const MCP_ACTIONS"),
    ] {
        assert!(src.contains(needle), "{needle} が見つからない");
    }
    // 書き込みの走査が core の書き込みを実際に拾っている（許された関数の中にある）
    let product = product_part(&s.core);
    assert!(
        product
            .lines()
            .filter(|l| WRITERS.iter().any(|w| l.contains(w)))
            .count()
            >= 4,
        "core の書き込みが採れていない"
    );
    // ソースの走査が core と control を含めて採れている
    assert!(s.others.iter().any(|(rel, _)| rel == CORE));
    assert!(s.others.iter().any(|(rel, _)| rel == CONTROL));
    assert!(
        s.others.len() > 100,
        "走査したソースが少なすぎる: {}",
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

/// 名指しの行番号が注入した行を指しているか
fn line_of_report(found: &[String], file: &str, why: &str) -> usize {
    found
        .iter()
        .find(|f| f.starts_with(file) && f.contains(why))
        .and_then(|f| f[file.len() + 1..].split(':').next()?.parse().ok())
        .unwrap_or(0)
}

#[test]
fn 逆戻りを名指しできる() {
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. 番人を外す（Context::blocked が write_guard を通さない）
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "            core::write_guard(dir, self.verification),",
            "            Ok(()),",
            1,
        );
    });
    assert_named(&found, CONTROL, "`write_guard(`");
    // 1b. uninstall も番人を通さない
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "            let guard = core::write_guard(&target.dir, ctx.verification);",
            "            let guard: Result<(), String> = Ok(());",
            1,
        );
    });
    assert_named(&found, CONTROL, "`write_guard(`");
    // 2. 既定の設定 dir を本物の ~/.claude にする
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "        default_dir: default_config_dir(),",
            "        default_dir: crate::orchestrator::claude_default_config_dir(),",
            1,
        );
    });
    assert_named(&found, CONTROL, "`default_config_dir()`");
    // 3. 別の場所から skills/ へ書く（GUI が自前で印を置く）
    let found = mutate(&|s| {
        s.app = s.app.replacen(
            "    fn drop_claude_mod_state(&mut self, pane_id: PaneId) {",
            "    fn drop_claude_mod_state(&mut self, pane_id: PaneId) {\n        let _ = std::fs::write(std::path::Path::new(\"/c\").join(\"skills\").join(\".tako-managed\"), \"\");",
            1,
        );
        s.app_dirty = true;
    });
    assert_named(&found, APP, "`.join(\"skills\")`");
    let injected = current()
        .app
        .lines()
        .position(|l| l.contains("    fn drop_claude_mod_state(&mut self, pane_id: PaneId) {"))
        .unwrap()
        + 2;
    assert_eq!(
        line_of_report(&found, APP, "`.join(\"skills\")`"),
        injected,
        "{found:?}"
    );
    // 3b. 判断を通らずに apply を呼ぶ
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "pub fn status_json(",
            "fn sneaky(d: &Path, e: &Expected) {\n    let _ = core::apply(d, Action::Write, e, false);\n}\n\npub fn status_json(",
            1,
        );
    });
    assert_named(&found, CONTROL, "`core::apply(`");
    // 4. core が settings.json を書く
    let found = mutate(&|s| {
        s.core = s.core.replacen(
            "fn read_json(path: &Path) -> Option<serde_json::Value> {",
            "fn read_json(path: &Path) -> Option<serde_json::Value> {\n    let _ = std::fs::write(path, \"{}\");",
            1,
        );
    });
    assert_named(&found, CORE, "write_copy / remove_copy / sweep_leftovers");
    let injected = current()
        .core
        .lines()
        .position(|l| l.contains("fn read_json(path: &Path) -> Option<serde_json::Value> {"))
        .unwrap()
        + 2;
    assert_eq!(
        line_of_report(&found, CORE, "write_copy / remove_copy / sweep_leftovers"),
        injected
    );
    // 5. 配線を外す（setup の段・GUI の差分検出・注入の判断・status・MCP）
    let found = mutate(&|s| {
        s.setup = s.setup.replace(
            "claude_mod_install::run_setup_stage(",
            "claude_mod_install::dry(",
        );
    });
    assert_named(&found, SETUP, "`claude_mod_install::run_setup_stage(`");
    let found = mutate(&|s| {
        s.app = s.app.replacen(
            "tako_control::claude_mod_install::sync_for_gui(&dirs, true, &skills_ctx)",
            "Vec::<String>::new()",
            1,
        );
    });
    assert_named(&found, APP, "2 か所");
    let found = mutate(&|s| {
        s.app = s.app.replace(
            "tako_control::claude_mod_install::refine_injection(",
            "std::convert::identity(",
        );
    });
    assert_named(&found, APP, "`claude_mod_install::refine_injection(`");
    let found = mutate(&|s| {
        s.control_mod = s.control_mod.replacen(
            "pub const MCP_ACTIONS: &[&str] = &[\n    \"status\",\n    \"on\",\n    \"off\",\n    \"band-on\",\n    \"band-off\",\n    \"ui\",\n    \"install\",\n    \"uninstall\",\n];",
            "pub const MCP_ACTIONS: &[&str] = &[\"status\", \"on\", \"off\", \"band-on\", \"band-off\", \"ui\"];",
            1,
        );
    });
    assert_named(&found, CONTROL_MOD, "install / uninstall");
    // 6. mod の休眠を外す・報告に dormant を載せない・id がずれる
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "  if (await userDisabled($)) {\n    await goDormant($, 'user_disabled')\n    return\n  }\n  dirty = true",
            "  dirty = true",
            1,
        );
    });
    assert_named(&found, REGISTER, "`await userDisabled($)`");
    let found = mutate(&|s| {
        s.register = s.register.replacen("    dormant,\n", "", 1);
    });
    assert_named(&found, REGISTER, "`dormant`");
    let found = mutate(&|s| {
        s.register = s.register.replacen(
            "const SKILLS_ID = 'tako@skills-dir'",
            "const SKILLS_ID = 'tako@skills'",
            1,
        );
    });
    assert_named(&found, REGISTER, "SKILLS_ID");
    // 7. 検証用の env を製品の経路でも効かせる・別の場所で文字列で読む
    let found = mutate(&|s| {
        s.core = s.core.replacen(
            "            .filter(|v| !v.trim().is_empty() && crate::paths::is_verification_process());",
            "            .filter(|v| !v.trim().is_empty());",
            1,
        );
    });
    assert_named(&found, CORE, "`is_verification_process()`");
    let found = mutate(&|s| {
        s.control = s.control.replacen(
            "pub fn status_json(",
            "fn legacy_again() -> bool {\n    std::env::var_os(\"TAKO_1959_LEGACY\").is_some()\n}\n\npub fn status_json(",
            1,
        );
    });
    assert_named(&found, CONTROL, "\"TAKO_1959_LEGACY\"");
}
