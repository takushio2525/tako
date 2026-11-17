//! Web ビューのページが宛先のあいだも tako のショートカットが tako へ届く形を縛る番犬（Issue #2948）
//!
//! # なぜ要るか
//!
//! macOS は #326 の NSEvent の local monitor が、ページ（WKWebView）が宛先のあいだに押された
//! tako の ⌘ ショートカットを先取りして宛先を gpui へ戻す。Windows にはこれに当たる口が無く
//! （`key_monitor::install` は空）、WebView2 の子窓は別プロセスなので打鍵は gpui の窓プロシージャを
//! 通らない = **Ctrl+Shift+T（新しいタブ）がページに飲まれる**（11/16 の実機で確認）。
//! #2948 で WebView2 の `AcceleratorKeyPressed` から tako の割り当てに当たる打鍵だけを先取りし、
//! gpui の前景タスクが宛先を戻して同じ打鍵を配る形にした。直したあとに同じ形へ戻る道を塞ぐ:
//!
//! 1. 境界 B11 の Windows の腕が空に戻る（`add_AcceleratorKeyPressed` を付けない・`Handled` に
//!    しない・判定を通さない）、公開口が Windows で腕を呼ばない
//! 2. Web ビューを作る口（`build_view`）が Win32 の親で先取りを付けない
//! 3. 判定が 1 実装を通らない（WebView2 の側が `keybindings::webview_key_route` を通らない・
//!    macOS の monitor が自前の表 = `WEBVIEW_PASSTHROUGH_KEYS` へ戻る = **OS ごとに表を 2 つ持つ**）
//! 4. 先取りした打鍵を受ける前景タスクが、宛先を戻さない・打鍵を配らない・起動時に立たない
//! 5. A/B の口（`TAKO_2948_LEGACY`）が 2 か所以上で読まれる
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で修正前の形を
//! 再現した注入を 1 本ずつ当てて `file:line` で名指されることを確かめる。範囲取りは #1420 の
//! 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const WEBVIEW: &str = "crates/tako-app/src/webview.rs";
const HOST: &str = "crates/tako-app/src/platform/webview_host.rs";
const KEYBINDINGS: &str = "crates/tako-app/src/keybindings.rs";
const CORE_KEYS: &str = "crates/tako-core/src/platform/keys.rs";

const LEGACY_ENV: &str = "TAKO_2948_LEGACY";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn new(file: &'static str, line: usize, why: impl Into<String>) -> Self {
        Self {
            file,
            line,
            why: why.into(),
        }
    }

    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn reports(offenders: &[Offender]) -> String {
    offenders
        .iter()
        .map(Offender::report)
        .collect::<Vec<_>>()
        .join("\n")
}

/// コメント行か（`//` で始まる行）
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 関数の窓（1-based の宣言行と本文）。終わりは**宣言行と同じ字下げの `}`**
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
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中でコメントを除いて `needle` を含む行（窓の先頭からの 1-based）
fn code_line_in(body: &str, needle: &str) -> Option<usize> {
    body.lines()
        .position(|l| !is_comment(l) && l.contains(needle))
        .map(|i| i + 1)
}

struct Sources {
    main: String,
    webview: String,
    host: String,
    keybindings: String,
    core_keys: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            main: production_range::production(&read(root, MAIN), MAIN),
            webview: production_range::production(&read(root, WEBVIEW), WEBVIEW),
            host: production_range::production(&read(root, HOST), HOST),
            keybindings: production_range::production(&read(root, KEYBINDINGS), KEYBINDINGS),
            core_keys: production_range::production(&read(root, CORE_KEYS), CORE_KEYS),
        }
    }
}

/// 窓に `needle` の行が無ければ、窓の宣言行を名指す
fn require(
    out: &mut Vec<Offender>,
    file: &'static str,
    window: &(usize, String),
    needle: &str,
    why: &str,
) {
    if code_line_in(&window.1, needle).is_none() {
        out.push(Offender::new(
            file,
            window.0,
            format!("{why}（`{needle}` が無い）"),
        ));
    }
}

/// 1. 境界 B11 の Windows の腕が実体を持つ（空・定数の偽へ戻っていない）
fn scan_windows_arm(host: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(host, "fn watch_accelerators(") {
        None => out.push(Offender::new(
            HOST,
            0,
            "Windows の腕 `watch_accelerators` が無い（#2948。ページが宛先のあいだ Ctrl+Shift+T などの \
             tako のショートカットがページに飲まれる）",
        )),
        Some(w) => {
            require(
                &mut out,
                HOST,
                &w,
                "add_AcceleratorKeyPressed(",
                "Windows の腕が WebView2 の AcceleratorKeyPressed を付けていない（#2948。打鍵が tako へ来ない）",
            );
            require(
                &mut out,
                HOST,
                &w,
                "SetHandled(true)",
                "先取りした打鍵をページへ渡さないようにしていない（#2948。ページと tako の両方へ届く）",
            );
            require(
                &mut out,
                HOST,
                &w,
                "if steal(keystroke)",
                "打鍵を判定（呼び手の `steal` = 1 実装）へ通していない（#2948。どの打鍵を先取りするかが \
                 macOS の #326 と別の決まりになる）",
            );
            require(
                &mut out,
                HOST,
                &w,
                "COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN",
                "押した瞬間だけを見ていない（#2948。離したときも先取りして 2 回配る）",
            );
        }
    }
    match fn_window(host, "pub(crate) fn watch_app_shortcuts(") {
        None => out.push(Offender::new(
            HOST,
            0,
            "公開口 `watch_app_shortcuts` が無い（#2948）",
        )),
        Some(w) => require(
            &mut out,
            HOST,
            &w,
            "win32::watch_accelerators(view, steal)",
            "公開口が Windows で腕を呼んでいない（#2948。`key_monitor::install` が空だった修正前と同じ）",
        ),
    }
    out
}

/// 2. Web ビューを作る口が Win32 の親で先取りを付ける
fn scan_build_view(webview: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some(w) = fn_window(webview, "fn build_view(") else {
        return vec![Offender::new(
            WEBVIEW,
            0,
            "`build_view` が見つからない（走査が空振り）",
        )];
    };
    let built = code_line_in(&w.1, ".build_as_child(");
    let watched = code_line_in(&w.1, "watch_app_shortcuts(&view, steal_app_shortcut)");
    match (built, watched) {
        (_, None) => out.push(Offender::new(
            WEBVIEW,
            w.0,
            "Web ビューを作るときに打鍵の先取り（`watch_app_shortcuts(&view, steal_app_shortcut)`）を\
             付けていない（#2948。Windows でページが宛先のあいだ tako のショートカットが飲まれる）",
        )),
        (Some(b), Some(s)) if b < s => {}
        (_, Some(s)) => out.push(Offender::new(
            WEBVIEW,
            w.0 + s - 1,
            "打鍵の先取りを Web ビューを作る前に付けている（#2948。付ける相手が無い）",
        )),
    }
    require(
        &mut out,
        WEBVIEW,
        &w,
        "window.win32_hwnd().is_some()",
        "先取りを親の窓の種類（Win32）で振り分けていない（#2948。macOS は #326 の monitor が受け持つ）",
    );
    require(
        &mut out,
        WEBVIEW,
        &w,
        "!legacy_2948()",
        "A/B の口（`TAKO_2948_LEGACY=1` で修正前へ戻す）を通していない（#2948）",
    );
    out
}

/// 3. 判定は 1 実装（両 OS が `tako_core::platform::keys::webview_key_route` と 1 つの表を通る）
fn scan_single_decision(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    // WebView2 の側: `steal_app_shortcut` → `keybindings::webview_key_route` → tako-core の 1 実装
    match fn_window(&s.webview, "fn steal_app_shortcut(") {
        None => out.push(Offender::new(
            WEBVIEW,
            0,
            "`steal_app_shortcut` が無い（#2948）",
        )),
        Some(w) => {
            require(
                &mut out,
                WEBVIEW,
                &w,
                "crate::keybindings::webview_key_route(&keystroke)",
                "WebView2 の打鍵の判定が 1 実装（`keybindings::webview_key_route`）を通っていない（#2948。\
                 OS ごとに先取りの決まりが割れる）",
            );
            require(
                &mut out,
                WEBVIEW,
                &w,
                "unbounded_send(keystroke)",
                "先取りした打鍵を gpui の前景タスクへ渡していない（#2948。ページにも tako にも届かない）",
            );
        }
    }
    match fn_window(&s.keybindings, "fn webview_key_route_in(") {
        None => out.push(Offender::new(
            KEYBINDINGS,
            0,
            "`webview_key_route_in` が無い（#2948）",
        )),
        Some(w) => {
            require(
                &mut out,
                KEYBINDINGS,
                &w,
                "tako_core::platform::keys::webview_key_route(",
                "キーバインド側の判定が tako-core の 1 実装を通っていない（#2948）",
            );
            require(
                &mut out,
                KEYBINDINGS,
                &w,
                "match_keystrokes(",
                "tako の割り当てかを GPUI 自身の照合で求めていない（#2948。判定した打鍵と GPUI が配る打鍵がずれる）",
            );
        }
    }
    // macOS の側: monitor → `routes_to_page` → 境界の `macos_key_routes_to_page` → 1 実装
    match fn_window(&s.webview, "unsafe extern \"C\" fn monitor_invoke(") {
        None => out.push(Offender::new(
            WEBVIEW,
            0,
            "macOS の monitor（`monitor_invoke`）が見つからない（走査が空振り）",
        )),
        Some(w) => require(
            &mut out,
            WEBVIEW,
            &w,
            "routes_to_page(key_code, flags)",
            "macOS の monitor が 1 実装の判定（`routes_to_page`）を通っていない（#2948。OS ごとに表を 2 つ持つ）",
        ),
    }
    match fn_window(&s.webview, "fn routes_to_page(") {
        None => out.push(Offender::new(
            WEBVIEW,
            0,
            "`routes_to_page` が無い（#2948）",
        )),
        Some(w) => require(
            &mut out,
            WEBVIEW,
            &w,
            "macos_key_routes_to_page(key_code, flags)",
            "macOS の判定が境界の `macos_key_routes_to_page`（1 実装へ渡す口）を通っていない（#2948）",
        ),
    }
    match fn_window(&s.host, "pub(crate) fn macos_key_routes_to_page(") {
        None => out.push(Offender::new(
            HOST,
            0,
            "`macos_key_routes_to_page` が無い（#2948）",
        )),
        Some(w) => require(
            &mut out,
            HOST,
            &w,
            "webview_key_route(Platform::MacOs",
            "macOS の判定が tako-core の 1 実装を通っていない（#2948）",
        ),
    }
    // 表は tako-core の 1 つだけ。#326 の virtual key code の表が戻っていない
    for (file, view) in [(WEBVIEW, &s.webview), (HOST, &s.host)] {
        for (i, line) in view.lines().enumerate() {
            if !is_comment(line)
                && (line.contains("WEBVIEW_PASSTHROUGH_KEYS")
                    || line.contains("fn is_webview_key("))
            {
                out.push(Offender::new(
                    file,
                    i + 1,
                    "ページに残す打鍵の表を tako-core の `WEBVIEW_PAGE_KEYS` の外に持っている（#2948。\
                     OS ごとに表を 2 つ持つと、片方だけ直して食い違う）",
                ));
            }
        }
    }
    if code_line_in(&s.core_keys, "pub const WEBVIEW_PAGE_KEYS").is_none() {
        out.push(Offender::new(
            CORE_KEYS,
            0,
            "ページに残す打鍵の表 `WEBVIEW_PAGE_KEYS` が無い（#2948）",
        ));
    }
    out
}

/// 4. 先取りした打鍵を受ける前景タスクが、宛先を戻して打鍵を配り、起動時に立つ
fn scan_relay(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(main, "fn spawn_webview_shortcut_relay(") {
        None => out.push(Offender::new(
            MAIN,
            0,
            "先取りした打鍵を配る前景タスク `spawn_webview_shortcut_relay` が無い（#2948）",
        )),
        Some(w) => {
            require(
                &mut out,
                MAIN,
                &w,
                "webview::set_app_shortcut_sink(tx)",
                "前景タスクが先取りの送り先を差し替えていない（#2948。WebView2 の側が送れず、ページへ流れる）",
            );
            require(
                &mut out,
                MAIN,
                &w,
                "app.return_webview_key_focus(true)",
                "先取りした後に宛先を tako へ戻していない（#2948。#326 と違い、次の打鍵もページへ流れる）",
            );
            require(
                &mut out,
                MAIN,
                &w,
                "window.dispatch_keystroke(keystroke, cx)",
                "先取りした打鍵を gpui のキーバインドへ配っていない（#2948。ページにも tako にも届かない）",
            );
        }
    }
    if code_line_in(main, "Self::spawn_webview_shortcut_relay(cx);").is_none() {
        out.push(Offender::new(
            MAIN,
            0,
            "起動時に前景タスク（`Self::spawn_webview_shortcut_relay(cx)`）を立てていない（#2948）",
        ));
    }
    out
}

/// 5. A/B の口は 1 か所（`TAKO_2948_LEGACY` を読むのは `legacy_2948` だけ）
fn scan_legacy_single(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((from, body)) = fn_window(&s.webview, "fn legacy_2948(") else {
        return vec![Offender::new(
            WEBVIEW,
            0,
            "A/B の口 `legacy_2948`（`TAKO_2948_LEGACY=1` で修正前へ戻す）が無い（#2948）",
        )];
    };
    let to = from + body.lines().count() - 1;
    let mut inside = 0usize;
    for (file, view) in [
        (MAIN, &s.main),
        (WEBVIEW, &s.webview),
        (HOST, &s.host),
        (KEYBINDINGS, &s.keybindings),
    ] {
        for (i, line) in view.lines().enumerate() {
            if is_comment(line) || !line.contains(&format!("\"{LEGACY_ENV}\"")) {
                continue;
            }
            if file == WEBVIEW && (from..=to).contains(&(i + 1)) {
                inside += 1;
            } else {
                out.push(Offender::new(
                    file,
                    i + 1,
                    format!(
                        "A/B の口 `{LEGACY_ENV}` を `webview.rs` の `legacy_2948` の外で読んでいる（#2948。\
                         散ると A/B の片方だけが修正前へ戻る）"
                    ),
                ));
            }
        }
    }
    if inside != 1 {
        out.push(Offender::new(
            WEBVIEW,
            from,
            format!("`legacy_2948` が `{LEGACY_ENV}` を読んでいない（#2948。A/B が効かない）"),
        ));
    }
    out
}

fn scan_all(s: &Sources) -> Vec<Offender> {
    let mut out = scan_windows_arm(&s.host);
    out.extend(scan_build_view(&s.webview));
    out.extend(scan_single_decision(s));
    out.extend(scan_relay(&s.main));
    out.extend(scan_legacy_single(s));
    out
}

#[test]
fn ページが宛先のあいだもtakoのショートカットが届く形を保つ() {
    let root = workspace_root();
    let s = Sources::load(&root);
    let offenders = scan_all(&s);
    assert!(offenders.is_empty(), "{}", reports(&offenders));
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = Sources::load(&root);
    assert!(fn_window(&s.host, "fn watch_accelerators(").is_some());
    assert!(fn_window(&s.host, "pub(crate) fn watch_app_shortcuts(").is_some());
    assert!(fn_window(&s.host, "pub(crate) fn macos_key_routes_to_page(").is_some());
    assert!(fn_window(&s.webview, "fn build_view(").is_some());
    assert!(fn_window(&s.webview, "fn steal_app_shortcut(").is_some());
    assert!(fn_window(&s.webview, "unsafe extern \"C\" fn monitor_invoke(").is_some());
    assert!(fn_window(&s.webview, "fn routes_to_page(").is_some());
    assert!(fn_window(&s.keybindings, "fn webview_key_route_in(").is_some());
    assert!(fn_window(&s.main, "fn spawn_webview_shortcut_relay(").is_some());
    // 窓は関数の終わりまで採れている（宣言行だけの空振りでない）
    let arm = fn_window(&s.host, "fn watch_accelerators(").unwrap();
    assert!(
        arm.1.lines().count() > 20,
        "Windows の腕の窓 {} 行",
        arm.1.lines().count()
    );
}

/// 注入した `src` の `from` を `to` へ置き換える（置き換え先が無ければ注入の空振りとして落とす）
fn inject(src: &str, from: &str, to: &str) -> String {
    assert!(src.contains(from), "注入の目印が無い: {from}");
    src.replacen(from, to, 1)
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let base = Sources::load(&root);
    assert!(scan_all(&base).is_empty(), "基準が緑");

    // I1: Windows の腕が AcceleratorKeyPressed を付けない（修正前 = `key_monitor::install` が空）
    let host = inject(
        &base.host,
        "unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }.is_ok()",
        "{ let _ = (controller, handler, token); false }",
    );
    let r = scan_windows_arm(&host);
    assert!(
        r.iter()
            .any(|o| o.file == HOST && o.line > 0 && o.why.contains("AcceleratorKeyPressed")),
        "I1: {}",
        reports(&r)
    );

    // I2: 公開口が Windows で腕を呼ばない
    let host = inject(
        &base.host,
        "win32::watch_accelerators(view, steal)",
        "{ let _ = (view, steal); false }",
    );
    let r = scan_windows_arm(&host);
    assert!(
        r.iter()
            .any(|o| o.line > 0 && o.why.contains("腕を呼んでいない")),
        "I2: {}",
        reports(&r)
    );

    // I3: 先取りした打鍵をページへ渡してしまう（Handled にしない）
    let host = inject(&base.host, "args.SetHandled(true)?", "let _ = &args");
    let r = scan_windows_arm(&host);
    assert!(
        r.iter()
            .any(|o| o.line > 0 && o.why.contains("ページへ渡さない")),
        "I3: {}",
        reports(&r)
    );

    // I4: Web ビューを作るときに先取りを付けない
    let webview = inject(
        &base.webview,
        "!crate::platform::webview_host::watch_app_shortcuts(&view, steal_app_shortcut)",
        "false",
    );
    let r = scan_build_view(&webview);
    assert!(
        r.iter()
            .any(|o| o.file == WEBVIEW && o.line > 0 && o.why.contains("付けていない")),
        "I4: {}",
        reports(&r)
    );

    // I5: WebView2 の側が 1 実装を通らない（tako-core の判定を飛ばして全部先取り）
    let mut s = Sources::load(&root);
    s.webview = inject(
        &s.webview,
        "if crate::keybindings::webview_key_route(&keystroke)",
        "if tako_core::platform::keys::WebviewKeyRoute::App",
    );
    let r = scan_single_decision(&s);
    assert!(
        r.iter()
            .any(|o| o.file == WEBVIEW && o.line > 0 && o.why.contains("1 実装")),
        "I5: {}",
        reports(&r)
    );

    // I6: macOS の monitor が自前の表へ戻る（OS ごとに表を 2 つ持つ）
    let mut s = Sources::load(&root);
    s.webview = inject(
        &s.webview,
        "        if routes_to_page(key_code, flags) {",
        "        const WEBVIEW_PASSTHROUGH_KEYS: &[u16] = &[0x00, 0x08];\n        if WEBVIEW_PASSTHROUGH_KEYS.contains(&key_code) {",
    );
    let r = scan_single_decision(&s);
    assert!(
        r.iter()
            .any(|o| o.file == WEBVIEW && o.line > 0 && o.why.contains("routes_to_page")),
        "I6: {}",
        reports(&r)
    );
    assert!(
        r.iter()
            .any(|o| o.file == WEBVIEW && o.line > 0 && o.why.contains("表を 2 つ")),
        "I6: {}",
        reports(&r)
    );

    // I7: 前景タスクが宛先を戻さない・打鍵を配らない
    let main = inject(
        &base.main,
        "                    app.return_webview_key_focus(true);\n",
        "",
    );
    let main = inject(
        &main,
        "window.dispatch_keystroke(keystroke, cx);",
        "let _ = (keystroke, window, cx);",
    );
    let r = scan_relay(&main);
    assert!(
        r.iter()
            .any(|o| o.file == MAIN && o.line > 0 && o.why.contains("宛先を tako へ戻していない")),
        "I7: {}",
        reports(&r)
    );
    assert!(
        r.iter()
            .any(|o| o.file == MAIN && o.line > 0 && o.why.contains("配っていない")),
        "I7: {}",
        reports(&r)
    );

    // I8: A/B の口を 2 か所目で読む
    let mut s = Sources::load(&root);
    s.main = format!(
        "{}\nfn extra() -> bool {{\n    std::env::var_os(\"{LEGACY_ENV}\").is_some()\n}}\n",
        s.main
    );
    let r = scan_legacy_single(&s);
    assert!(
        r.len() == 1 && r[0].file == MAIN && r[0].line > 0,
        "I8: {}",
        reports(&r)
    );
}
