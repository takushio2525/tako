//! 言語サーバの取得（#1944）の構造の番犬
//!
//! # なぜ要るか
//!
//! 取得の約束（検証してから使う・途中の物を使わない・グローバルを先に見る・UI を待たせない・
//! 利用者の環境を汚さない）は、どれも**1 行で崩せる**のに、崩しても普段は動いて見える
//! （配布元は正しい物を返し、ネットワークは速く、グローバルが無い機では差が出ない）。
//! 挙動は e2e（`issue1944_lsp_fetch`。ローカルの配布元 + 偽サーバの gzip）が測り、ここは**構造**を
//! 縛って file:line で名指す（ネットワークの無い CI でも落ちる）。
//!
//! # 縛ること
//!
//! - 取得物はハッシュを固定値と突き合わせ、合わなければ捨てる（`download`）
//! - 取ってから展開し、印を書いてから版の段へ rename する。印の無い段は使わない
//! - 起こすときはグローバル（差し替えの env / PATH）を先に見て、無いときだけ置き場 → 取得
//! - 失敗の記録があるあいだは開き直しても取りに行かない（オフラインで開くたびに取りに行かない）
//! - 画面の状態の口（`document_server`）はログインシェルもネットワークも待たない
//! - シェル統合の合図で引き直すのは背景のスレッドで、間隔をあける
//! - 画面の「入れる」は背景で取る（UI スレッドで取得を待たない）
//! - `TAKO_1944_LEGACY=1` で取得の設定が作られない（A/B）
//! - UI への状態の知らせは診断の口とは別（診断の口は URI だけ = #1679）で、GUI はそれを受けて描き直す
//! - LSP のモジュールに npm / brew / winget 等の導入器を起こす値を書かない（グローバルを汚さない。
//!   導入コマンドの案内は検出表 `servers.rs` の値だけ）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。
//! 導入器の文字列の走査の範囲取りは `common/production_range.rs` の 1 実装（テスト領域とコメントだけを
//! 空白へ潰す = 行番号は原文のまま）。

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments_checked;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const FETCH: &str = "crates/tako-control/src/lsp/fetch.rs";
const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";
const GUI: &str = "crates/tako-app/src/lsp_status_ui.rs";
const CORE_FETCH: &str = "crates/tako-core/src/lsp/fetch.rs";
const ARCHIVE: &str = "crates/tako-control/src/lsp/archive.rs";
const APP: &str = "crates/tako-app/src/main.rs";

const FILES: &[&str] = &[FETCH, MANAGER, GUI, CORE_FETCH, ARCHIVE, APP];

/// 導入器を起こす値（LSP のモジュールの文字列リテラルに現れてはいけない）
const INSTALLERS: &[&str] = &[
    "npm", "npx", "brew", "winget", "pip", "pip3", "-g", "--global",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.starts_with(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let pad = " ".repeat(indent);
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == format!("{pad}}}"))
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_only(text: &str) -> String {
    text.lines()
        .map(|l| match l.find("//") {
            Some(i) if !l[..i].ends_with(':') => &l[..i],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の行（1-based）
fn line_in(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| at + i)
}

/// 規則 1 つ: `file` の関数 `decl` のコードが `must` をすべて含み、`must_not` を含まず、
/// `order` の組は前のものが先に現れる
struct Rule {
    file: &'static str,
    decl: &'static str,
    must: &'static [&'static str],
    must_not: &'static [&'static str],
    order: &'static [(&'static str, &'static str)],
    why: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        file: FETCH,
        decl: "fn download(",
        must: &[
            "if !hasher.matches(asset.digest) {",
            "return Err(FetchError::Digest {",
            "let _ = std::fs::remove_file(dest);",
        ],
        must_not: &[],
        order: &[("hasher.update(", "if !hasher.matches(asset.digest) {")],
        why: "取得物のハッシュを固定値と突き合わせずに使う / 合わない物を残す",
    },
    Rule {
        file: FETCH,
        decl: "fn install_assets(",
        must: &[
            "let bytes = download(",
            "staging.join(table::MARKER)",
            "std::fs::rename(&staging, dir)",
        ],
        must_not: &[],
        order: &[
            ("let bytes = download(", "archive::"),
            ("staging.join(table::MARKER)", "std::fs::rename(&staging, dir)"),
        ],
        why: "検証の前に展開する / 済んだ印を書く前に版の段へ置く（途中で落ちた物が「入っている」に見える）",
    },
    Rule {
        file: FETCH,
        decl: "pub fn installed(",
        must: &["dir.join(table::MARKER).is_file()"],
        must_not: &[],
        order: &[],
        why: "済んだ印を見ずに置き場の物を使う（途中で落ちた・ディスクが尽きた跡を起こす）",
    },
    Rule {
        file: FETCH,
        decl: "    pub fn from_env() -> Option<Self> {",
        must: &["if legacy() {", "return None;"],
        must_not: &[],
        order: &[("if legacy() {", "data_dir()")],
        why: "`TAKO_1944_LEGACY=1` で取得が止まらない（A/B が効かない）",
    },
    Rule {
        file: MANAGER,
        decl: "    fn run_start(&self, key: &ServerKey, generation: u64) {",
        must: &["missing => match self.start_managed("],
        must_not: &["fetch::ensure(", "fetch::installed("],
        order: &[("Launch::Found { plan, program_path }", "self.start_managed(")],
        why: "グローバル（差し替えの env / PATH）より先に置き場・取得を見る（グローバルの版を使わない）",
    },
    Rule {
        file: MANAGER,
        decl: "    fn start_managed(",
        must: &[
            "fetch::installed(config, spec)",
            "config.auto && config.can_fetch(spec) && !failed_before",
        ],
        must_not: &[],
        order: &[("fetch::installed(config, spec)", "self.fetch_and_start(")],
        why: "置き場を見ずに取り直す / 失敗の後も開くたびに取りに行く（オフラインで開くたびに待つ）",
    },
    Rule {
        file: MANAGER,
        decl: "    fn document_server(&self, path: &Path) -> Option<DocumentServer> {",
        must: &["fetch::progress_of(config, spec)"],
        must_not: &[
            "fetch::installed(",
            "fetch::ensure(",
            "self.resolve(",
            "find_with_timeout",
        ],
        order: &[],
        why: "画面の状態の口（描くたびに UI スレッドで呼ぶ）がログインシェル・ネットワークで待つ",
    },
    Rule {
        file: MANAGER,
        decl: "    fn on_shell_activity(&self) {",
        must: &["RECHECK_INTERVAL", "std::thread::Builder::new()"],
        must_not: &["self.resolve("],
        order: &[],
        why: "シェル統合の合図を出したスレッドでログインシェルを待つ / 間隔をあけずにコマンドごと引き直す",
    },
    Rule {
        file: GUI,
        decl: "    fn lsp_status_action(",
        must: &["cx.background_executor()", "manager.install(Some(id))"],
        must_not: &[],
        order: &[("cx.background_executor()", "manager.install(Some(id))")],
        why: "画面の「入れる」が UI スレッドで取得を待つ（取っているあいだ UI が固まる）",
    },
    Rule {
        file: MANAGER,
        decl: "fn notify_refresh(inner: &mut Inner) {",
        must: &["inner.ui_refresh_wanted = true;", "wake_ui(inner);"],
        must_not: &["notify_diagnostics("],
        order: &[],
        why: "状態の知らせを診断の口へ混ぜる（#1679 の「診断の口は URI だけ」を崩す）/ 作り直しを求めない",
    },
    Rule {
        file: APP,
        decl: "    fn spawn_lsp_state_loop(&self, cx: &mut Context<Self>) {",
        must: &["self.lsp.state_events()", "app.lsp.take_refresh_wanted()", "app.refresh_lsp_links(cx)"],
        must_not: &[],
        order: &[],
        why: "状態の知らせを受けて描き直さない / 断っていた文書を開き直さない（取れても restart まで出ない）",
    },
];

/// LSP のモジュールの本番コード（テスト領域とコメントを落とす）に、導入器を起こす値の文字列リテラルが
/// 現れたら名指す（グローバルの npm / brew / winget へ入れない。案内の文字列は検出表だけが持つ）
fn installer_literals(file: &str, src: &str) -> Vec<String> {
    let production = without_comments_checked(&production_range::scan(src).text, file);
    let mut out = Vec::new();
    for (i, line) in production.lines().enumerate() {
        for name in INSTALLERS {
            if line.contains(&format!("\"{name}\"")) {
                out.push(format!(
                    "{file}:{} — 導入器 `{name}` を起こす値を書いている（グローバルを汚さない。取得は配布元の取得物を検証して data dir へ置く）",
                    i + 1
                ));
            }
        }
    }
    out
}

/// `file:line — 理由` の一覧（違反が無ければ空）
fn scan(sources: &[(&'static str, String)]) -> Vec<String> {
    let src_of = |file: &str| &sources.iter().find(|(f, _)| *f == file).unwrap().1;
    let mut out = Vec::new();
    for rule in RULES {
        let Some((at, window)) = fn_window(src_of(rule.file), rule.decl) else {
            out.push(format!(
                "{}:0 — `{}` が見つからない（走査が空振り）",
                rule.file, rule.decl
            ));
            continue;
        };
        let code = code_only(&window);
        for needle in rule.must {
            if !code.contains(needle) {
                out.push(format!(
                    "{}:{at} — {}（`{needle}` が無い）",
                    rule.file, rule.why
                ));
            }
        }
        for needle in rule.must_not {
            if code.contains(needle) {
                let line = line_in(at, &window, needle).unwrap_or(at);
                out.push(format!(
                    "{}:{line} — {}（`{needle}` が在る）",
                    rule.file, rule.why
                ));
            }
        }
        for (first, then) in rule.order {
            if let (Some(a), Some(b)) = (code.find(first), code.find(then)) {
                if a > b {
                    let line = line_in(at, &window, then).unwrap_or(at);
                    out.push(format!(
                        "{}:{line} — {}（`{then}` が `{first}` より前）",
                        rule.file, rule.why
                    ));
                }
            }
        }
    }
    for file in [FETCH, MANAGER, GUI, CORE_FETCH, ARCHIVE] {
        out.extend(installer_literals(file, src_of(file)));
    }
    out
}

fn current() -> Vec<(&'static str, String)> {
    FILES.iter().map(|f| (*f, read(f))).collect()
}

#[test]
fn 取得の約束を守っている() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let sources = current();
    for rule in RULES {
        let src = &sources.iter().find(|(f, _)| *f == rule.file).unwrap().1;
        let (_, window) = fn_window(src, rule.decl)
            .unwrap_or_else(|| panic!("{}: `{}` の窓が採れない", rule.file, rule.decl));
        assert!(
            window.lines().count() >= 3,
            "{}: `{}` の窓が短すぎる",
            rule.file,
            rule.decl
        );
    }
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` の行が名指しされること
fn assert_named(label: &str, file: &'static str, (from, to): (&str, &str), needle: &str) {
    let mut sources = current();
    let slot = sources.iter_mut().find(|(f, _)| *f == file).unwrap();
    assert!(
        slot.1.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    slot.1 = slot.1.replacen(from, to, 1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let reports = scan(&sources);
    let expected = format!("{file}:{line} ");
    let named = reports.iter().find(|r| r.starts_with(&expected));
    assert!(
        named.is_some(),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

#[test]
fn 逆戻りを名指しできる() {
    // A. ハッシュを確かめずに使う
    assert_named(
        "A 検証を外す",
        FETCH,
        ("    if !hasher.matches(asset.digest) {", "    if false {"),
        "fn download(",
    );
    // B. 展開してから取る（順序の逆転）= 検証前の物を置く形の代表
    assert_named(
        "B 印を書く前に rename",
        FETCH,
        (
            "    std::fs::write(\n        staging.join(table::MARKER),",
            "    let _ = std::fs::rename(&staging, dir);\n    std::fs::write(\n        staging.join(table::MARKER),",
        ),
        "    let _ = std::fs::rename(&staging, dir);",
    );
    // C. 印を見ずに置き場を使う
    assert_named(
        "C 印を見ない",
        FETCH,
        (
            "    if !dir.join(table::MARKER).is_file() {\n        return None;\n    }\n    launch_plan(config, spec, fetch, &dir, None)",
            "    launch_plan(config, spec, fetch, &dir, None)",
        ),
        "pub fn installed(",
    );
    // D. A/B を外す
    assert_named(
        "D legacy を見ない",
        FETCH,
        (
            "        if legacy() {\n            return None;\n        }\n        let root",
            "        let root",
        ),
        "    pub fn from_env() -> Option<Self> {",
    );
    // E. グローバルより先に置き場を見る
    assert_named(
        "E 置き場を先に",
        MANAGER,
        (
            "        let (launch, _) = self.resolve(spec);\n        // #1944",
            "        let _early = tako_control_fetch_first(fetch::installed(self.fetch.as_ref().unwrap(), spec));\n        let (launch, _) = self.resolve(spec);\n        // #1944",
        ),
        "fetch::installed(self.fetch",
    );
    // F. 失敗の後も開くたびに取りに行く
    assert_named(
        "F 失敗を見ない",
        MANAGER,
        (
            "            if config.auto && config.can_fetch(spec) && !failed_before {",
            "            if config.auto && config.can_fetch(spec) {",
        ),
        "    fn start_managed(",
    );
    // G. 画面の状態の口で置き場の起こし方（Node.js をログインシェルで探す）を引く
    assert_named(
        "G UI の口で待つ",
        MANAGER,
        (
            "        let can_install = self\n            .fetch",
            "        let _plan = self.fetch.as_ref().and_then(|c| fetch::installed(c, spec));\n        let can_install = self\n            .fetch",
        ),
        "let _plan = self.fetch.as_ref().and_then(|c| fetch::installed(c, spec));",
    );
    // H. 合図のスレッドで引き直す
    assert_named(
        "H 合図のスレッドで解決",
        MANAGER,
        (
            "        inner.rechecking = true;",
            "        let _ = self.resolve(servers::SERVERS.first().unwrap());\n        inner.rechecking = true;",
        ),
        "let _ = self.resolve(servers::SERVERS.first().unwrap());",
    );
    // I. 「入れる」を UI スレッドで待つ
    assert_named(
        "I UI で取る",
        GUI,
        (
            "        cx.background_executor()\n            .spawn(async move {",
            "        manager.install(Some(id));\n        cx.background_executor()\n            .spawn(async move {",
        ),
        "        manager.install(Some(id));",
    );
    // K. 状態の知らせを診断の口へ戻す（初版の形）
    assert_named(
        "K 診断の口へ混ぜる",
        MANAGER,
        (
            "    inner.ui_refresh_wanted = true;\n    wake_ui(inner);",
            "    inner.ui_refresh_wanted = true;\n    notify_diagnostics(inner, \"\");",
        ),
        "fn notify_refresh(inner: &mut Inner) {",
    );
    // J. グローバルの npm を起こす
    assert_named(
        "J npm を起こす",
        FETCH,
        (
            "/// 取得物のハッシュを計算する",
            "pub fn global_install() {\n    let _ = std::process::Command::new(\"npm\").args([\"install\", \"-g\"]);\n}\n\n/// 取得物のハッシュを計算する",
        ),
        "Command::new(\"npm\")",
    );
}
