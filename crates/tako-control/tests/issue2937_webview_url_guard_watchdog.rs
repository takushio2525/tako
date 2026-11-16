//! **#2937 の番犬**: Web ビュー（wry）へ URL を渡す前に、必ず同じ 1 つの検査を通す。
//!
//! ## なぜ止めるのか
//!
//! wry 0.57.1 の macOS 実装は `NSURL::URLWithString(…).unwrap()` で URL を作るので、NSURL が
//! 解釈できない文字列を渡すと **GUI ごと abort** する。#334（空白入り）で入口の検査を足したが、
//! `tako web open 'javascript:alert(2936)'` は正規化が `https://javascript:alert(2936)`
//! （ポートが数字でない形）にしてしまい、検査を抜けて落ちた（#2937）。
//!
//! 直した形（`tako_control::web_url` の 1 実装 = 形・scheme の表・境界 B38 の NSURL の解釈）は、
//! 次の 1 行の変更で黙って崩れる:
//!
//! 1. [`wryへurlを渡す直前は検査を通る`] — wry へ URL を渡す呼び出し（`with_url` / `load_url`）の
//!    手前から `validate_url` が外れる・検査の無い新しい呼び出しが増える
//! 2. [`gui側の口は1実装へ委ねている`] — `webview::validate_url` / `normalize_url` が自前の規則を持ち直す
//!    （#334 の形の検査だけに戻る = javascript: が https のホストになって再び落ちる）
//! 3. [`検査の3段が残っている`] — `web_url::check` から scheme の表・解釈の確かめが外れる
//! 4. [`入口はどれも同じ検査を通る`] — dock・アドレスバー・target=_blank・生成待ち・wry の無い
//!    エントリの navigate・CLI / MCP（dispatch）のどれかが検査を通らない経路へ変わる
//!
//! 規則そのもの（何を通し何を断るか）は `web_url` の単体テストが持つ。ここは**構造**を見る。
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で窓が採れていることを
//! 固定し、[`逆戻りを名指しできる`] で注入 8 通りが `file:line` で名指しされることを確かめる。
//! 本番コードの範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const WEBVIEW: &str = "crates/tako-app/src/webview.rs";
const WEB_URL: &str = "crates/tako-control/src/web_url.rs";
const APP_SRC: &str = "crates/tako-app/src";

/// wry へ URL を渡す呼び出しの綴り（`WebViewBuilder` の生成時と `WebView` の読み込み）
const WRY_URL_HANDOFFS: &[&str] = &[
    ".with_url(",
    ".with_url_and_headers(",
    ".load_url(",
    ".load_url_with_headers(",
];

/// 検査を通った目印（`webview::validate_url` = `web_url::check` の GUI 側の口）
const CHECK: &str = "validate_url(";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 走査する本文（本番コードだけ = テスト領域は空白へ潰して行番号を保つ）
struct Sources {
    /// tako-app の `src` 配下の全 `.rs`（相対パス, 本番コード）
    app: Vec<(String, String)>,
    web_url: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        let mut app = Vec::new();
        collect_rs(&root.join(APP_SRC), root, &mut app);
        app.sort();
        let web_url = production_range::scan(&read(root, WEB_URL)).text;
        Self { app, web_url }
    }

    fn get(&self, rel: &str) -> &str {
        &self
            .app
            .iter()
            .find(|(p, _)| p == rel)
            .unwrap_or_else(|| panic!("{rel} を読めていない"))
            .1
    }

    fn set(&mut self, rel: &str, text: String) {
        let slot = self
            .app
            .iter_mut()
            .find(|(p, _)| p == rel)
            .unwrap_or_else(|| panic!("{rel} を読めていない"));
        slot.1 = text;
    }
}

fn read(root: &Path, rel: &str) -> String {
    let path = root.join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

fn collect_rs(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{} を読めない: {e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path
                .strip_prefix(root)
                .expect("ワークスペースの中")
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).expect("tako-app のソース");
            out.push((rel, production_range::scan(&src).text));
        }
    }
}

/// 行が関数の頭（`fn 名前(`）か。クロージャ（`move |…|`）は関数ではない
fn is_fn_head(line: &str) -> bool {
    let t = line.trim_start();
    let t = t
        .strip_prefix("pub(crate) ")
        .or_else(|| t.strip_prefix("pub(super) "))
        .or_else(|| t.strip_prefix("pub "))
        .unwrap_or(t);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let t = t.strip_prefix("unsafe ").unwrap_or(t);
    let t = t.strip_prefix("const ").unwrap_or(t);
    t.starts_with("fn ")
}

/// 0 始まりの行 `at` を含む関数の頭の行（0 始まり）
fn enclosing_fn(lines: &[&str], at: usize) -> Option<usize> {
    (0..=at).rev().find(|&i| is_fn_head(lines[i]))
}

/// 関数の本体（頭の行から、頭と同じ字下げの `}` まで）。`(1 始まりの頭の行, 本文)`
fn fn_body<'a>(src: &'a str, head: &str) -> Option<(usize, &'a str)> {
    let start = src.find(head)?;
    // 頭が `\n` で始まる指定（字下げ 0 の関数）にも対応する
    let start = if head.starts_with('\n') {
        start + 1
    } else {
        start
    };
    let line = src[..start].matches('\n').count() + 1;
    let indent = src[start..].len() - src[start..].trim_start().len();
    let indent = &src[start..start + indent];
    let close = format!("\n{indent}}}");
    let end = src[start..]
        .find(&close)
        .map(|i| start + i + close.len())
        .unwrap_or(src.len());
    Some((line, &src[start..end]))
}

// ---- 1. wry へ渡す直前 ----

/// wry へ URL を渡す呼び出しのうち、同じ関数の手前で検査を通っていないもの（`file:line: 理由`）
fn unchecked_handoffs(s: &Sources) -> Vec<String> {
    let mut hits = Vec::new();
    for (rel, src) in &s.app {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") || !WRY_URL_HANDOFFS.iter().any(|h| line.contains(h)) {
                continue;
            }
            let checked = enclosing_fn(&lines, i).is_some_and(|head| {
                lines[head..i]
                    .iter()
                    .any(|l| !l.trim_start().starts_with("//") && l.contains(CHECK))
            });
            if !checked {
                hits.push(format!(
                    "{rel}:{}: wry へ URL を渡す前に `{CHECK}` を通っていない（NSURL が解釈できない \
                     URL で wry の unwrap が GUI ごと abort する = #334 / #2937）",
                    i + 1
                ));
            }
        }
    }
    hits
}

/// wry へ URL を渡す呼び出しの数（空振りの検出用）
fn handoff_count(s: &Sources) -> usize {
    s.app
        .iter()
        .flat_map(|(_, src)| src.lines())
        .filter(|l| {
            !l.trim_start().starts_with("//") && WRY_URL_HANDOFFS.iter().any(|h| l.contains(h))
        })
        .count()
}

#[test]
fn wryへurlを渡す直前は検査を通る() {
    let hits = unchecked_handoffs(&Sources::load(&workspace_root()));
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

// ---- 2. GUI 側の口 ----

fn delegation_breaks(s: &Sources) -> Vec<String> {
    let mut hits = Vec::new();
    let webview = s.get(WEBVIEW);
    for (head, delegate, legacy_spelling) in [
        (
            "\npub fn validate_url(",
            "tako_control::web_url::check(",
            "is_ascii_whitespace",
        ),
        (
            "\npub fn normalize_url(",
            "tako_control::web_url::normalize(",
            "starts_with(\"http://\")",
        ),
    ] {
        let Some((line, body)) = fn_body(webview, head) else {
            hits.push(format!(
                "{WEBVIEW}: `{}` が見つからない（改名で番犬が空振りする）",
                head.trim()
            ));
            continue;
        };
        if !body.contains(delegate) || body.contains(legacy_spelling) {
            hits.push(format!(
                "{WEBVIEW}:{line}: `{}` が `{delegate}` の 1 実装へ委ねていない（自前の規則を持つと \
                 scheme の表と NSURL の確かめが抜け、javascript: で再び abort する = #2937）",
                head.trim()
            ));
        }
    }
    hits
}

#[test]
fn gui側の口は1実装へ委ねている() {
    let hits = delegation_breaks(&Sources::load(&workspace_root()));
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

// ---- 3. 検査の 3 段 ----

fn stage_breaks(s: &Sources) -> Vec<String> {
    let mut hits = Vec::new();
    let Some((line, body)) = fn_body(&s.web_url, "\npub fn check_with(") else {
        return vec![format!("{WEB_URL}: `pub fn check_with(` が見つからない")];
    };
    for (needle, why) in [
        ("UrlHasWhitespace", "形の検査（空白 = #334）"),
        ("UrlHasControlChar", "形の検査（制御文字 = #334）"),
        (
            "SCRIPT_SCHEMES",
            "スクリプトの scheme（javascript: / vbscript:）の断り",
        ),
        ("OPENABLE_SCHEMES", "開く scheme の表"),
        ("parses(url)", "wry と同じ解釈の確かめ（境界 B38）"),
    ] {
        if !body.contains(needle) {
            hits.push(format!(
                "{WEB_URL}:{line}: `check_with` から{why}が外れている（`{needle}`。#2937）"
            ));
        }
    }
    match fn_body(&s.web_url, "\npub fn check(") {
        Some((line, body)) if !body.contains("platform::webview_url::webview_parses") => {
            hits.push(format!(
                "{WEB_URL}:{line}: `check` が境界 B38 の本物の確かめ（`platform::webview_url::webview_parses`）\
                 を渡していない（macOS で NSURL が nil を返す形が wry へ届く。#2937）"
            ));
        }
        Some(_) => {}
        None => hits.push(format!("{WEB_URL}: `pub fn check(` が見つからない")),
    }
    hits
}

#[test]
fn 検査の3段が残っている() {
    let hits = stage_breaks(&Sources::load(&workspace_root()));
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

// ---- 4. 入口 ----

/// 入口ごとに「どの関数がどの綴りを通るか」（`.agent/architecture.md`「Web ビューへ渡す URL の検査
/// （#334 / #2937）」の入口表と同じ）
const ENTRIES: &[(&str, &str, &str, &str)] = &[
    // (ファイル, 関数の頭, 通るべき綴り, 入口の名前)
    (
        MAIN,
        "    fn web_open(&mut self, pane: PaneId, url: &str)",
        "self.create_webview(&webview::normalize_url(url))",
        "CLI / MCP の web open・ポート提案のチップ（dispatch 経由）",
    ),
    (
        MAIN,
        "    fn web_navigate(&mut self, id: u64, to: &str)",
        "e.navigate(to)",
        "CLI / MCP の web nav（dispatch 経由）",
    ),
    (
        MAIN,
        "    fn accept_port_suggestion(",
        "Request::Web {",
        "ポート提案のチップ（CLI と同じ dispatch）",
    ),
    (
        MAIN,
        "    fn open_webview_from_dock(",
        "self.accept_typed_web_url(WebUrlPlace::Dock, url)",
        "dock の URL 欄（Enter・「開く」）",
    ),
    (
        MAIN,
        "    fn handle_webview_address_bar_key(",
        ".accept_typed_web_url(WebUrlPlace::AddressBar(pane_id), &typed)",
        "アドレスバー（Enter）",
    ),
    (
        MAIN,
        "    fn accept_typed_web_url(",
        "webview::validate_url(&normalized)",
        "GUI の欄の検査（dock・アドレスバー）",
    ),
    (
        MAIN,
        "    fn process_pending_new_windows(",
        "webview::validate_url(&normalized)",
        "ページの target=_blank / window.open",
    ),
    (
        MAIN,
        "    fn create_webview(&mut self, url: &str)",
        "webview::WebViewEntry::queued(id, url)",
        "生成（CLI / MCP・dock・起動時の復元）の生成待ち",
    ),
    (
        WEBVIEW,
        "    pub fn queued(id: WebViewId, url: &str)",
        "validate_url(url)?",
        "生成待ちのエントリ（Windows の生成タスク・起動時の復元）",
    ),
    (
        WEBVIEW,
        "    fn navigate_pending(&self, to: &str)",
        "validate_url(&url)?",
        "wry の無いエントリの navigate（保留の「開く」・生成待ちの URL の差し替え）",
    ),
    (
        WEBVIEW,
        "    pub fn retry(&self)",
        "self.navigate(&url)",
        "エラー画面の「再読み込み」",
    ),
];

fn entry_breaks(s: &Sources) -> Vec<String> {
    let mut hits = Vec::new();
    for &(rel, head, needle, name) in ENTRIES {
        match fn_body(s.get(rel), head) {
            Some((line, body)) if !body.contains(needle) => hits.push(format!(
                "{rel}:{line}: 入口「{name}」が同じ検査を通っていない（`{needle}` が無い。#2937）"
            )),
            Some(_) => {}
            None => hits.push(format!(
                "{rel}: 入口「{name}」の `{}` が見つからない（改名で番犬が空振りする）",
                head.trim()
            )),
        }
    }
    hits
}

#[test]
fn 入口はどれも同じ検査を通る() {
    let hits = entry_breaks(&Sources::load(&workspace_root()));
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

// ---- 空振り・注入 ----

fn scan_all(s: &Sources) -> Vec<String> {
    let mut all = unchecked_handoffs(s);
    all.extend(delegation_breaks(s));
    all.extend(stage_breaks(s));
    all.extend(entry_breaks(s));
    all
}

#[test]
fn 走査が空振りしていない() {
    let s = Sources::load(&workspace_root());
    // 生成（`build_view` の `with_url`）と読み込み（`navigate` の `load_url`）の 2 つ
    assert_eq!(
        handoff_count(&s),
        2,
        "wry へ URL を渡す呼び出しの数が変わった。増えたなら番犬の綴り（WRY_URL_HANDOFFS）と\n\
         入口表（ENTRIES・architecture.md の #2937 の小節）を見直す。0 なら走査が空振りしている"
    );
    assert!(
        s.app.len() > 20,
        "tako-app のソースを読めていない（{} 本）",
        s.app.len()
    );
}

/// 注入した本文で、`rel` を名指す行が少なくとも 1 本出るか
fn expect_hit(s: &Sources, rel: &str, phrase: &str) {
    let hits = scan_all(s);
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&format!("{rel}:")) && h.contains(phrase)),
        "注入を名指しできなかった（{rel} / {phrase}）:\n{}",
        hits.join("\n")
    );
}

fn replaced(src: &str, from: &str, to: &str) -> String {
    assert!(src.contains(from), "注入の目印が古い: {from}");
    src.replacen(from, to, 1)
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) 生成の手前から検査を外す
    let mut s = Sources::load(&root);
    let body = replaced(
        s.get(WEBVIEW),
        "    ) -> Result<wry::WebView, String> {\n        validate_url(url)?;\n",
        "    ) -> Result<wry::WebView, String> {\n",
    );
    s.set(WEBVIEW, body);
    expect_hit(&s, WEBVIEW, "wry へ URL を渡す前に");

    // (2) 読み込みの手前から検査を外す
    let mut s = Sources::load(&root);
    let body = replaced(
        s.get(WEBVIEW),
        "                let url = normalize_url(url);\n                validate_url(&url)?;\n                let track",
        "                let url = normalize_url(url);\n                let track",
    );
    s.set(WEBVIEW, body);
    expect_hit(&s, WEBVIEW, "wry へ URL を渡す前に");

    // (3) 検査の無い読み込みを main.rs に足す
    let mut s = Sources::load(&root);
    let body = replaced(
        s.get(MAIN),
        "    fn web_destroy(&mut self, id: u64) -> Option<PaneId> {",
        "    fn web_load_raw(&self, view: &wry::WebView, url: &str) {\n\
         \x20       let _ = view.load_url(url);\n    }\n\n\
         \x20   fn web_destroy(&mut self, id: u64) -> Option<PaneId> {",
    );
    s.set(MAIN, body);
    expect_hit(&s, MAIN, "wry へ URL を渡す前に");

    // (4) GUI 側の検査を #334 の形だけへ戻す
    let mut s = Sources::load(&root);
    let body = replaced(
        s.get(WEBVIEW),
        "    tako_control::web_url::check(url)\n",
        "    if url.chars().any(|c| c.is_ascii_whitespace()) {\n\
         \x20       return Err(HostRefusal::UrlHasWhitespace(url.to_string()));\n    }\n    Ok(())\n",
    );
    s.set(WEBVIEW, body);
    expect_hit(&s, WEBVIEW, "1 実装へ委ねていない");

    // (5) 正規化を修正前の 5 つの接頭辞へ戻す
    let mut s = Sources::load(&root);
    let body = replaced(
        s.get(WEBVIEW),
        "    tako_control::web_url::normalize(input)\n",
        "    if input.starts_with(\"http://\") { return input.to_string(); }\n\
         \x20   format!(\"https://{input}\")\n",
    );
    s.set(WEBVIEW, body);
    expect_hit(&s, WEBVIEW, "1 実装へ委ねていない");

    // (6) scheme の表を外す
    let mut s = Sources::load(&root);
    s.web_url = replaced(
        &s.web_url,
        "        if SCRIPT_SCHEMES.contains(&lower.as_str()) {",
        "        if false {",
    );
    expect_hit(&s, WEB_URL, "スクリプトの scheme");

    // (7) 境界 B38 の確かめを渡さない
    let mut s = Sources::load(&root);
    s.web_url = replaced(
        &s.web_url,
        "check_with(url, legacy(), crate::platform::webview_url::webview_parses)",
        "check_with(url, legacy(), |_| true)",
    );
    expect_hit(&s, WEB_URL, "境界 B38 の本物の確かめ");

    // (8) dock の入口が検査を通らずに作る
    let mut s = Sources::load(&root);
    let body = replaced(
        s.get(MAIN),
        "self.accept_typed_web_url(WebUrlPlace::Dock, url)",
        "Some(webview::normalize_url(url))",
    );
    s.set(MAIN, body);
    expect_hit(&s, MAIN, "dock の URL 欄");
}
