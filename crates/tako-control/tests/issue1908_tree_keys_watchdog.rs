//! **#1908 の番犬**: ファイルツリーの ↑ / ↓ / ← / → / Enter / ⇧⌘↑ / ⇧⌘↓（Windows は
//! Shift+Ctrl+Home / End）・選択の CLI / MCP・残り時間の数え下ろし（FR-3.40）の要所が外れない
//! ようにする。
//!
//! ## ここで止める 5 つ
//!
//! 1. [`キーは修飾まで見て当てる`] — 素の矢印・Enter は修飾が混ざると当たらない（⌥↑ のペイン移動・
//!    ⌘← を奪わない）。端までは macOS = ⇧⌘↑ / ⇧⌘↓・Windows = Shift+Ctrl+Home / End だけ
//! 2. [`画面のキーはcliとmcpと同じdispatchを通る`] — 画面だけで選択を動かすと CLI / MCP と割れる
//!    （設計原則 5 の 1:1）。A/B（`TAKO_1908_LEGACY=1`）が #1908 のキーにだけ効く
//! 3. [`dispatchはcoreの結果を当てるだけ`] — 状態遷移の正本は `tree_select::on_key`。リモートの行は
//!    動かさない・Enter は `OpenFile`（行を押したのと同じ）・開閉は host の 1 本
//! 4. [`cliとmcpは同じ要求になる`] — CLI `tako tree selection` と MCP `tako_tree_folder` の
//!    `action=selection` が同じ `Request::TreeSelection`。MCP の `key` の語彙は core の `Key` と同じ
//! 5. [`残り時間は最後の進みから数え下ろす`] — 「いま」までの平均で割ると 1 チャンクごとにのこぎり状に
//!    揺れる。進んだ時刻は「いま」より先に読む（後に読むとその 1 回だけ旧式へ落ちる）
//!
//! 各規則は `Result` を返す検査関数で、本物のソースに当てる本体と、要所を消した写しに当てて
//! **file:line で名指すか**を確かめる注入（`注入_*`）を同じ関数で回す
//! （検出力の無い番犬は緑のまま腐る）。
//!
//! ## 相方
//!
//! 中身は `tako_core::tree_select`（`on_key` / `extend_to_edge`）・`platform::keys`・`file_copy` の
//! 単体（新旧の振れ幅の数字）、dispatch の応答は `dispatch::tests::issue1908_*`、MCP の振り分けは
//! `mcp::tests`、実キーと CLI / MCP の字面一致は visual-test `tree-keys` +
//! `scripts/test-tree-keys-1908.sh`。ここは**配線が外れていないこと**だけを見る

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const SELECT: &str = "crates/tako-core/src/tree_select.rs";
const COPY: &str = "crates/tako-core/src/file_copy.rs";
const KEYS: &str = "crates/tako-core/src/platform/keys.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象 1 ファイルの眺め（コメントだけ潰した `view` と、文字列も潰した `code`。同じバイト長）
struct Source {
    rel: &'static str,
    code: String,
    view: String,
}

fn source(rel: &'static str) -> Source {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    Source::from_text(rel, &src)
}

/// 関数 1 本（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: &'static str,
    name: String,
    line: usize,
    text: String,
}

type Check = Result<(), String>;

impl Source {
    fn from_text(rel: &'static str, src: &str) -> Self {
        let prod = production_range::production(src, rel);
        let source = Self {
            rel,
            code: code_view(&prod),
            view: without_comments_checked(&prod, rel),
        };
        assert_eq!(
            source.code.len(),
            source.view.len(),
            "{rel}:1 2 つの眺めのバイト長が食い違う"
        );
        source
    }

    fn body(&self, name: &str) -> Result<Body, String> {
        let mut offset = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if fn_head_name(line.trim_start()) != Some(name) {
                continue;
            }
            let Some(open) = self.code[start..].find('{').map(|i| i + start) else {
                break;
            };
            let mut depth = 0usize;
            let mut end = open;
            for (i, byte) in self.code[open..].bytes().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return Ok(Body {
                rel: self.rel,
                name: name.to_string(),
                line: self.code[..start].lines().count() + 1,
                text: self.view[open..end].to_string(),
            });
        }
        Err(format!(
            "{}:1 `fn {name}` が見つからない。\n改名したなら番犬の名前も直すこと（#1908 の配線が外れたまま緑になる）",
            self.rel
        ))
    }

    fn line_with(&self, needle: &str) -> Option<usize> {
        self.view
            .lines()
            .position(|line| line.contains(needle))
            .map(|i| i + 1)
    }

    /// `arm` の行から `window` 行以内に `needle` が在ること（match の腕の配線を見る）
    fn arm_calls(&self, arm: &str, needle: &str, window: usize, why: &str) -> Check {
        let Some(line) = self.line_with(arm) else {
            return Err(format!("{}:1 `{arm}` の腕が無い。\n{why}", self.rel));
        };
        let found = self
            .view
            .lines()
            .skip(line - 1)
            .take(window)
            .any(|l| l.contains(needle));
        if found {
            Ok(())
        } else {
            Err(format!(
                "{}:{line} `{arm}` の腕が `{needle}` を通っていない。\n{why}",
                self.rel
            ))
        }
    }
}

impl Body {
    fn at(&self, needle: &str) -> Option<usize> {
        self.text.find(needle)
    }

    fn must_contain(&self, needle: &str, why: &str) -> Check {
        if self.text.contains(needle) {
            return Ok(());
        }
        Err(format!(
            "{}:{} `fn {}` が `{needle}` を通っていない。\n{why}",
            self.rel, self.line, self.name
        ))
    }

    /// `first` が `then` より前に在ること（両方在る前提。無ければその旨で名指す）
    fn must_order(&self, first: &str, then: &str, why: &str) -> Check {
        match (self.at(first), self.at(then)) {
            (Some(a), Some(b)) if a < b => Ok(()),
            (a, b) => Err(format!(
                "{}:{} `fn {}` で `{first}`（{a:?}）が `{then}`（{b:?}）より前に無い。\n{why}",
                self.rel, self.line, self.name
            )),
        }
    }
}

// --- 規則（本物にも注入にも同じ関数を当てる） -------------------------------------

fn rule_keys(keys: &Source) -> Check {
    let key = keys.body("tree_select_key")?;
    for (needle, why) in [
        (
            "let plain = !shift && !platform_key && !control && !alt;",
            "素の矢印・Enter の判定が修飾を見ていない（⌥↑ のペイン移動・⌘← 等の別の割り当てを奪う）",
        ),
        (
            "Platform::MacOs => shift && platform_key && !control && !alt,",
            "macOS の端まで（⇧⌘↑ / ⇧⌘↓）が修飾を見ていない（⌃⇧↑ 等を奪う）",
        ),
        (
            "Platform::Windows => shift && control && !platform_key && !alt,",
            "Windows の端まで（Shift+Ctrl+Home / End）が修飾を見ていない（Shift+Home 単独等を奪う）",
        ),
        (
            "\"enter\" if plain => Some(TreeSelectKey::Open),",
            "Enter が修飾無しだけになっていない（⇧Enter の改行等を奪う）",
        ),
        (
            "\"home\" if platform == Platform::Windows && to_edge => Some(TreeSelectKey::ExtendTop),",
            "Windows の Shift+Ctrl+Home が端まで（先頭）へ当たっていない",
        ),
    ] {
        key.must_contain(needle, why)?;
    }
    Ok(())
}

fn rule_gui_dispatch(sidebar: &Source) -> Check {
    let action = sidebar.body("tree_select_key_action")?;
    action.must_contain(
        "tako_control::protocol::Request::TreeSelection {",
        "画面のキーが CLI / MCP と同じ dispatch を通っていない（画面だけの経路 = 1:1 が崩れる）",
    )?;
    action.must_contain(
        "key: Some(select.as_str().to_string()),",
        "画面のキーが core の語彙（`Key::as_str` = CLI / MCP の `key`）で dispatch へ渡っていない",
    )?;
    let handle = sidebar.body("handle_tree_clip_keystroke")?;
    handle.must_contain(
        "(key.since_1908() && tako_core::file_copy::legacy_1908())",
        "A/B（`TAKO_1908_LEGACY=1`）が #1908 のキーにだけ効いていない（同一バイナリで旧挙動へ戻せない）",
    )
}

fn rule_dispatch(dispatch: &Source) -> Check {
    dispatch.arm_calls(
        "Request::TreeSelection { path, key, tab } => {",
        "dispatch_tree_selection(host, path, key, tab, origin)",
        3,
        "`TreeSelection` が選択の dispatch へ届いていない",
    )?;
    let body = dispatch.body("dispatch_tree_selection")?;
    body.must_order(
        "if remote {",
        "on_key(&prev, key, &rows)",
        "リモート（SSH）の行の上のキーが core へ渡っている（ローカルの並びに載らない行を動かす）",
    )?;
    body.must_contain(
        "host.set_tree_expanded(&dir, expanded);",
        "← / → / Enter の開閉が host の 1 本を通っていない",
    )?;
    body.must_contain(
        "Request::OpenFile {",
        "Enter が行を押したとき（`open_file_row` = `OpenFile`）と同じ口で開いていない",
    )?;
    body.must_contain(
        "if !host.filetree_visible() {",
        "閉じたツリーの選択を動かしている（キーが効かない画面の状態を CLI / MCP だけが変える）",
    )
}

fn rule_core(select: &Source) -> Check {
    let on_key = select.body("on_key")?;
    on_key.must_contain(
        "Key::ExtendTop => return KeyOutcome::Select(extend_to_edge(prev, Step::Up, &order)),",
        "⇧⌘↑ が core の `extend_to_edge`（⇧クリックと同じ `apply`）を通っていない",
    )?;
    on_key.must_contain(
        "if row.root {",
        "見出しの ← が親へ上がる（ワークスペースの外の行を選ぶ）",
    )?;
    select.body("extend_to_edge")?.must_contain(
        "apply(Some(prev), edge, ClickKind::Range, order)",
        "端までが ⇧クリック（起点からの範囲）と同じ `apply` を通っていない（起点が動く）",
    )
}

fn rule_cli_mcp(cli: &Source, mcp: &Source) -> Check {
    cli.arm_calls(
        "TreeCommand::Selection { path, key, tab } => Request::TreeSelection {",
        "path: path.as_deref().map(resolve_cli_path),",
        3,
        "CLI `tako tree selection` が MCP と同じ要求になっていない（パスは CLI の cwd 基準で絶対化）",
    )?;
    mcp.arm_calls(
        "\"tako_tree_folder\" if str_arg(args, \"action\")?.as_deref() == Some(\"selection\") => {",
        "Request::TreeSelection {",
        3,
        "MCP `tako_tree_folder` の `selection` が CLI と同じ要求になっていない",
    )
}

fn rule_eta(copy: &Source) -> Check {
    copy.body("eta")?.must_contain(
        "let left = estimate(snap, legacy_1908());",
        "帯・`eta_secs` の残り時間が #1908 の式（と A/B）を通っていない",
    )?;
    let estimate = copy.body("estimate")?;
    for (needle, why) in [
        (
            "let base = ratio * at;",
            "速さを最後の進みの時点で測っていない（「いま」までの平均で割る = のこぎり状に揺れる）",
        ),
        (
            "base - idle",
            "最後の進みから数え下ろしていない（次の進みまで見積もりが増える）",
        ),
        (
            "base - stall + (idle - stall) * ratio",
            "止まったときに見積もりが伸びない（止まっている間に 0 へ落ちて「まもなく完了」と言い続ける）",
        ),
    ] {
        estimate.must_contain(needle, why)?;
    }
    copy.body("add_done")?.must_contain(
        "self.bytes_at_nanos.store(nanos, Ordering::Relaxed);",
        "バイトが進んだ時刻を残していない（数え下ろしの起点が無い）",
    )?;
    copy.body("snapshot")?.must_order(
        "let bytes_at =",
        "copying_for: self",
        "進んだ時刻を「いま」より後に読んでいる（間に届いた進みで時刻がいまを越え、その 1 回だけ旧式へ落ちる）",
    )
}

// --- 本体 ---------------------------------------------------------------------------

fn ok(check: Check) {
    if let Err(e) = check {
        panic!("{e}");
    }
}

#[test]
fn キーは修飾まで見て当てる() {
    ok(rule_keys(&source(KEYS)));
}

#[test]
fn 画面のキーはcliとmcpと同じdispatchを通る() {
    ok(rule_gui_dispatch(&source(SIDEBAR)));
}

#[test]
fn dispatchはcoreの結果を当てるだけ() {
    ok(rule_dispatch(&source(DISPATCH)));
    ok(rule_core(&source(SELECT)));
}

#[test]
fn cliとmcpは同じ要求になる() {
    ok(rule_cli_mcp(&source(CLI), &source(MCP_REQUEST)));
    // MCP の `key` の語彙は core の `Key` と同じ（片方だけ増やすと CLI / MCP と画面が割れる）
    let tools = tako_control::mcp::tools();
    let tree = tools
        .iter()
        .find(|t| t["name"] == "tako_tree_folder")
        .expect("tako_tree_folder がカタログに無い");
    let props = &tree["inputSchema"]["properties"];
    let keys: Vec<&str> = props["key"]["enum"]
        .as_array()
        .expect("key の enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let core: Vec<&str> = tako_core::tree_select::Key::ALL
        .iter()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(keys, core, "MCP の key の語彙が core の Key とずれている");
    assert!(
        props["action"]["enum"]
            .as_array()
            .expect("action の enum")
            .iter()
            .any(|v| v == "selection"),
        "tako_tree_folder の action に selection が無い"
    );
}

#[test]
fn 残り時間は最後の進みから数え下ろす() {
    ok(rule_eta(&source(COPY)));
}

// --- 注入: 要所を消した写しで、規則が file:line を名指して落ちるか -------------------

/// `rel` の本文から `from` を `to` へ 1 か所書き換えた写し
fn injected(rel: &'static str, from: &str, to: &str) -> Source {
    let src = std::fs::read_to_string(repo_root().join(rel)).expect("読める");
    assert!(
        src.contains(from),
        "{rel}:1 注入の目印 `{from}` が本文に無い（番犬の注入を直すこと）"
    );
    Source::from_text(rel, &src.replacen(from, to, 1))
}

fn must_name(check: Check, rel: &str, what: &str) {
    let err = check.expect_err("要所を消した写しでも規則が通った = 検出力が無い");
    assert!(
        err.starts_with(&format!("{rel}:")) && err.contains(what),
        "名指しが file:line + 対象になっていない: {err}"
    );
}

#[test]
fn 注入_修飾を見ないキーの判定を名指す() {
    let keys = injected(
        KEYS,
        "let plain = !shift && !platform_key && !control && !alt;",
        "let plain = true;",
    );
    must_name(rule_keys(&keys), KEYS, "fn tree_select_key");
    let keys = injected(
        KEYS,
        "Platform::Windows => shift && control && !platform_key && !alt,",
        "Platform::Windows => shift,",
    );
    must_name(rule_keys(&keys), KEYS, "fn tree_select_key");
}

#[test]
fn 注入_画面だけで選択を動かすと名指す() {
    let sidebar = injected(
        SIDEBAR,
        "tako_control::protocol::Request::TreeSelection {",
        "tako_control::protocol::Request::List {",
    );
    must_name(
        rule_gui_dispatch(&sidebar),
        SIDEBAR,
        "fn tree_select_key_action",
    );
    let sidebar = injected(
        SIDEBAR,
        "(key.since_1908() && tako_core::file_copy::legacy_1908())",
        "false",
    );
    must_name(
        rule_gui_dispatch(&sidebar),
        SIDEBAR,
        "fn handle_tree_clip_keystroke",
    );
}

#[test]
fn 注入_dispatchがリモートを動かしenterを自前で開くと名指す() {
    let dispatch = injected(
        DISPATCH,
        "            if remote {\n",
        "            if false {\n",
    );
    must_name(
        rule_dispatch(&dispatch),
        DISPATCH,
        "fn dispatch_tree_selection",
    );
    let dispatch = injected(
        DISPATCH,
        "                    host.set_tree_expanded(&dir, expanded);",
        "                    let _ = (&dir, expanded);",
    );
    must_name(
        rule_dispatch(&dispatch),
        DISPATCH,
        "fn dispatch_tree_selection",
    );
    let select = injected(
        SELECT,
        "            if row.root {\n",
        "            if false {\n",
    );
    must_name(rule_core(&select), SELECT, "fn on_key");
}

#[test]
fn 注入_cliとmcpの要求が割れると名指す() {
    let cli = injected(
        CLI,
        "TreeCommand::Selection { path, key, tab } => Request::TreeSelection {\n                path: path.as_deref().map(resolve_cli_path),",
        "TreeCommand::Selection { path, key, tab } => Request::TreeSelection {\n                path: path.clone(),",
    );
    must_name(
        rule_cli_mcp(&cli, &source(MCP_REQUEST)),
        CLI,
        "TreeCommand::Selection",
    );
    let mcp = injected(
        MCP_REQUEST,
        "\"tako_tree_folder\" if str_arg(args, \"action\")?.as_deref() == Some(\"selection\") => {",
        "\"tako_tree_folder_x\" if false => {",
    );
    must_name(
        rule_cli_mcp(&source(CLI), &mcp),
        MCP_REQUEST,
        "tako_tree_folder",
    );
}

#[test]
fn 注入_いままでの平均で割ると名指す() {
    let copy = injected(COPY, "let base = ratio * at;", "let base = ratio * now;");
    must_name(rule_eta(&copy), COPY, "fn estimate");
    let copy = injected(
        COPY,
        "let left = estimate(snap, legacy_1908());",
        "let left = estimate(snap, true);",
    );
    must_name(rule_eta(&copy), COPY, "fn eta");
    let copy = injected(
        COPY,
        "                self.bytes_at_nanos.store(nanos, Ordering::Relaxed);",
        "                let _ = nanos;",
    );
    must_name(rule_eta(&copy), COPY, "fn add_done");
}
