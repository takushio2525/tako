//! Windows の Web ビューが「見える」「打鍵が戻る」形を縛る番犬（Issue #2429 / #2430）
//!
//! # なぜ要るか
//!
//! - **#2429**: gpui_windows は既定で DirectComposition の合成を使い、合成の木を**子窓より上**に
//!   重ねる（`CreateTargetForHwnd(hwnd, true)`）。WebView2 の子 HWND はその下に入り、`state: ready`
//!   なのにペインは暗いまま。gpui を作る前に `GPUI_DISABLE_DIRECT_COMPOSITION=1` を立てると見える
//!   （実機の A/B）。gpui がこの env を読むのは platform を作る 1 回だけなので、**立てる位置**
//!   （`application()` の前・スレッドを起こす前）と**外す位置**（`application()` の後）が肝になる
//! - **#2430**: Windows の `key_monitor` が空で「宛先が Web ビューか」が常に偽だったので、
//!   `return_webview_key_focus`（#1481）が入口で抜け、ターミナルをクリックしても打鍵がページへ
//!   流れ続けた。宛先の読み書きは境界 B11 `platform::webview_host` の Win32 の腕へ振り分ける
//!
//! # 何を縛るか
//!
//! 1. `main` が gpui を作る前・最初のスレッドの前に合成を決め、作った後に外して記録する
//! 2. `GPUI_DISABLE_DIRECT_COMPOSITION` を書く・読むのは境界のファイルだけ（散ると立てる位置が割れる）
//! 3. Web ビューの宛先の 3 つの口が Win32 の親を境界の腕へ振り分ける（`key_focus_owner` も OS を問わない）
//! 4. 境界の Win32 の腕が実体を読む・動かす（定数の偽へ戻っていない）
//! 5. セルフテスト項目 71 / 155 のアドレスバーに Windows の無条件スキップが戻っていない・
//!    項目 71 の #1481 群が macOS だけに閉じていない
//! 6. A/B の口（`TAKO_2429_LEGACY` / `TAKO_2430_LEGACY`）は 1 か所ずつ
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

const GPUI_ENV: &str = "GPUI_DISABLE_DIRECT_COMPOSITION";

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

/// コメントを除いて `needle` を含む最初の行（1-based）
fn first_code_line(src: &str, needle: &str) -> Option<usize> {
    src.lines()
        .position(|l| !is_comment(l) && l.contains(needle))
        .map(|i| i + 1)
}

/// `[from, to)` の行の範囲（1-based の開始行・本文）。`from` / `to` は行の目印
fn region(src: &str, from: &str, to: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(from))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.contains(to))
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..end].join("\n")))
}

struct Sources {
    main: String,
    webview: String,
    host: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            main: production_range::production(&read(root, MAIN), MAIN),
            webview: production_range::production(&read(root, WEBVIEW), WEBVIEW),
            host: production_range::production(&read(root, HOST), HOST),
        }
    }
}

/// 1. `main` が gpui を作る前・最初のスレッドの前に合成を決め、作った後に外して記録する
fn scan_startup_order(main: &str) -> Vec<Offender> {
    let Some((at, body)) = fn_window(main, "fn main() {") else {
        return vec![Offender::new(
            MAIN,
            0,
            "`fn main()` が見つからない（走査が空振り）",
        )];
    };
    let line_of = |needle: &str| first_code_line(&body, needle).map(|l| at + l - 1);
    let mut out = Vec::new();
    let prepare = line_of("platform::webview_host::prepare_compositor()");
    let thread = line_of("spawn_stall_watchdog()");
    let app = line_of("application()");
    let after = line_of("compositor.after_platform_created()");
    let logged = line_of("compositor.log_line()");
    match (prepare, thread, app) {
        (None, _, _) => out.push(Offender::new(
            MAIN,
            at,
            "`main` が gpui を作る前に描画の合成を決めていない（#2429。Windows の Web ビューの中身が \
             DirectComposition の木の下に隠れる。`platform::webview_host::prepare_compositor()` を \
             `application()` の前で呼ぶ）",
        )),
        (Some(p), Some(t), Some(a)) if p < t && p < a => {}
        (Some(p), _, _) => out.push(Offender::new(
            MAIN,
            p,
            "描画の合成を決める位置が最初のスレッド（`spawn_stall_watchdog`）か `application()` より後\
             （#2429。gpui は platform を作る 1 回だけ env を読み、env の書き換えは別スレッドの読みと競る）",
        )),
    }
    match (app, after) {
        (Some(a), Some(f)) if a < f => {}
        (_, None) => out.push(Offender::new(
            MAIN,
            at,
            "gpui を作った後に tako が立てた env を外していない（#2429。ペインの子 = シェル・別の gpui \
             アプリへ tako の内部の選択が渡る。`compositor.after_platform_created()`）",
        )),
        (_, Some(f)) => out.push(Offender::new(
            MAIN,
            f,
            "tako が立てた env を `application()` より前に外している（#2429。gpui が読む前に消えるので \
             DirectComposition が切れず、Web ビューが暗いまま）",
        )),
    }
    if logged.is_none() {
        out.push(Offender::new(
            MAIN,
            at,
            "描画の合成の判断を persist.log へ記録していない（#2429。`compositor.log_line()`）",
        ));
    }
    out
}

/// 2. `GPUI_DISABLE_DIRECT_COMPOSITION` を書く・読むのは境界のファイルだけ
///
/// `sources` は（リポジトリ相対のパス, 本番コードの眺め）
fn scan_env_single_place(sources: &[(String, String)]) -> Vec<Offender> {
    let mut out = Vec::new();
    for (rel, view) in sources {
        if rel == HOST {
            continue;
        }
        for (i, line) in view.lines().enumerate() {
            if !is_comment(line) && line.contains(GPUI_ENV) {
                out.push(Offender {
                    file: leak(rel.clone()),
                    line: i + 1,
                    why: format!(
                        "`{GPUI_ENV}` を境界 B11（{HOST}）の外で扱っている（#2429。立てる位置・外す位置・\
                         利用者の値を上書きしない判断が 2 か所に割れる）"
                    ),
                });
            }
        }
    }
    out
}

/// 報告の `file` へ載せるための `'static` 化（テストの中で数件だけ）
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// 走査対象の Rust ソース（`crates/*/src` の下・リポジトリ相対の `/` 区切り）の本番コードの眺め
fn rust_sources(root: &Path) -> Vec<(String, String)> {
    production_range::product_sources()
        .into_iter()
        .map(|rel| {
            let view = production_range::scan(&read(root, &rel)).text;
            (rel, view)
        })
        .collect()
}

/// 3. Web ビューの宛先の 3 つの口が Win32 の親を境界の腕へ振り分ける
fn scan_focus_routes(webview: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (decl, arm) in [
        (
            "pub fn webview_holds_key_focus(",
            "webview_host::win32_child_holds_key_focus(",
        ),
        (
            "pub fn return_key_focus_to_app(",
            "webview_host::win32_return_key_focus(",
        ),
        (
            "pub fn key_focus_owner(",
            "webview_host::win32_key_focus_owner(",
        ),
    ] {
        let Some((at, body)) = fn_window(webview, decl) else {
            out.push(Offender::new(
                WEBVIEW,
                0,
                format!("`{decl}` が見つからない（走査が空振り）"),
            ));
            continue;
        };
        if !(body.contains("win32_hwnd()") && body.contains(arm)) {
            out.push(Offender::new(
                WEBVIEW,
                at,
                format!(
                    "`{decl}…` が Win32 の親を境界 B11 の `{arm}…` へ振り分けていない（#2430。\
                     Windows の宛先は AppKit の `key_monitor` を通ると常に偽 = ターミナルをクリックしても\
                     打鍵がページへ流れ続ける）"
                ),
            ));
        }
        // 口の直前の属性に OS の cfg が無い（セルフテストの #1481 群は Windows でも素性を読む）
        let lines: Vec<&str> = webview.lines().collect();
        let mut i = at.saturating_sub(1);
        while i > 0 {
            let prev = lines[i - 1].trim_start();
            if prev.starts_with("#[cfg(") {
                out.push(Offender::new(
                    WEBVIEW,
                    i,
                    format!(
                        "`{decl}…` が OS の cfg で閉じている（#2430。宛先の口は OS を問わない形で、\
                         振り分けは窓のハンドルの種類で行う）"
                    ),
                ));
                break;
            }
            if !(prev.starts_with("///") || prev.starts_with("#[")) {
                break;
            }
            i -= 1;
        }
    }
    out
}

/// 4. 境界の Win32 の腕が実体を読む・動かす（定数の偽へ戻っていない）
fn scan_host_arms(host: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (decl, must) in [
        (
            "pub(crate) fn win32_child_holds_key_focus(",
            &["win32::focus_of(", "win32::is_child("][..],
        ),
        (
            "pub(crate) fn win32_return_key_focus(",
            &["win32::set_focus("][..],
        ),
        (
            "pub(crate) fn prepare_compositor()",
            &["set_var(GPUI_DISABLE_DIRECT_COMPOSITION"][..],
        ),
        (
            "pub(crate) fn after_platform_created(",
            &["remove_var(GPUI_DISABLE_DIRECT_COMPOSITION"][..],
        ),
    ] {
        let Some((at, body)) = fn_window(host, decl) else {
            out.push(Offender::new(
                HOST,
                0,
                format!("`{decl}` が見つからない（走査が空振り）"),
            ));
            continue;
        };
        for m in must {
            if !body.contains(m) {
                out.push(Offender::new(
                    HOST,
                    at,
                    format!(
                        "`{decl}…` が `{m}…` を通っていない（#2429 / #2430。Win32 の腕が実体を読まない・\
                         動かさない形へ戻ると、Windows で宛先が戻らない / 合成が切れない）"
                    ),
                ));
            }
        }
    }
    for (needle, why) in [
        (
            "fn GetGUIThreadInfo(",
            "宛先を gpui の窓のスレッドの入力の状態から読む",
        ),
        (
            "fn IsChild(",
            "宛先が gpui の窓の子孫か（別プロセスの WebView2 の子窓）",
        ),
        (
            "fn SetFocus(",
            "宛先を gpui の窓へ戻す（wry の focus_parent と同じ）",
        ),
    ] {
        if first_code_line(host, needle).is_none() {
            out.push(Offender::new(
                HOST,
                0,
                format!("`{needle}…` の宣言が無い（#2430。{why}）"),
            ));
        }
    }
    out
}

/// 5. セルフテスト項目 71 / 155 のアドレスバーに Windows の無条件スキップが戻っていない・
///    項目 71 の #1481 群が macOS だけに閉じていない
fn scan_self_test(main: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let item71 = region(
        main,
        "// 71. Web ビューペイン e2e",
        "// 72. worker spawn レイアウトエンジン",
    );
    let addr155 = region(
        main,
        "// --- 2. Web ビューのアドレスバー ---",
        "// --- 3. Web dock の URL 欄 ---",
    );
    for (name, found) in [("項目 71", &item71), ("項目 155 のアドレスバー", &addr155)] {
        let Some((at, body)) = found else {
            out.push(Offender::new(
                MAIN,
                0,
                format!("{name} の目印が見つからない（走査が空振り）"),
            ));
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            let windows_skip = line.contains("cfg!(target_os = \"windows\")")
                || line.contains("cfg!(windows)")
                || line.contains("TAKO_SELF_TEST_SKIPPED: 71")
                || line.contains("TAKO_SELF_TEST_SKIPPED: 155 のアドレスバー");
            if windows_skip {
                out.push(Offender::new(
                    MAIN,
                    at + i,
                    format!(
                        "{name}: Windows の無条件スキップが戻っている（#2429 / #2430 / #2445。\
                         実機で落ちないことを確かめた後は Windows でも走らせる。走らない理由が残る節だけを\
                         理由つきで飛ばす）"
                    ),
                ));
            }
        }
    }
    if let Some((at, body)) = &item71 {
        let lines: Vec<&str> = body.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            let macos_only = t.starts_with("#[cfg(target_os = \"macos\")]")
                || t.starts_with("#[cfg(not(windows))]")
                || t.starts_with("#[cfg(not(target_os = \"windows\"))]");
            if !macos_only {
                continue;
            }
            // 直後の数行に #1481 群の印（宛先の素性を読む口）があれば、それを OS で閉じている
            let next = lines[i + 1..(i + 12).min(lines.len())].join("\n");
            if next.contains("key_focus_owner") || next.contains("fn owner(") {
                out.push(Offender::new(
                    MAIN,
                    at + i,
                    "項目 71 の #1481 の宛先の検査が macOS だけに閉じている（#2430。Windows の宛先 = \
                     gpui の窓のスレッドの focus も同じ口で読める。閉じると回帰を機械で捕まえられない）",
                ));
            }
        }
    }
    out
}

/// 6. A/B の口は 1 か所ずつ
fn scan_legacy_gates(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src, name) in [
        (HOST, &s.host, "TAKO_2429_LEGACY"),
        (WEBVIEW, &s.webview, "TAKO_2430_LEGACY"),
    ] {
        let hits: Vec<usize> = src
            .lines()
            .enumerate()
            .filter(|(_, l)| !is_comment(l) && !l.trim_start().starts_with("///"))
            .filter(|(_, l)| l.contains(&format!("\"{name}\"")))
            .map(|(i, _)| i + 1)
            .collect();
        if hits.len() != 1 {
            out.push(Offender::new(
                file,
                hits.first().copied().unwrap_or(0),
                format!(
                    "`{name}` を読む場所が {} か所（A/B の口は 1 か所。無くなると修正前を再現できず、\
                     増えると腕ごとに挙動が割れる）",
                    hits.len()
                ),
            ));
        }
    }
    out
}

fn scan_all(root: &Path, s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    out.extend(scan_startup_order(&s.main));
    out.extend(scan_env_single_place(&rust_sources(root)));
    out.extend(scan_focus_routes(&s.webview));
    out.extend(scan_host_arms(&s.host));
    out.extend(scan_self_test(&s.main));
    out.extend(scan_legacy_gates(s));
    out
}

fn reports(offenders: &[Offender]) -> String {
    offenders
        .iter()
        .map(Offender::report)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn windowsのwebviewが見えて打鍵が戻る構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&root, &Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#2429 / #2430 の構造が崩れている:\n{}",
        reports(&offenders)
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = Sources::load(&root);
    assert!(fn_window(&s.main, "fn main() {").is_some(), "main の窓");
    for decl in [
        "pub fn webview_holds_key_focus(",
        "pub fn return_key_focus_to_app(",
        "pub fn key_focus_owner(",
    ] {
        assert!(fn_window(&s.webview, decl).is_some(), "{decl}");
    }
    let item71 = region(
        &s.main,
        "// 71. Web ビューペイン e2e",
        "// 72. worker spawn レイアウトエンジン",
    )
    .expect("項目 71 の窓");
    assert!(
        item71.1.contains("key_focus_owner") && item71.1.contains("fn owner("),
        "項目 71 の窓に #1481 群が入っている"
    );
    let addr = region(
        &s.main,
        "// --- 2. Web ビューのアドレスバー ---",
        "// --- 3. Web dock の URL 欄 ---",
    )
    .expect("項目 155 のアドレスバーの窓");
    assert!(addr.1.contains("web-addr-bar"), "項目 155 の窓の中身");
    // env の走査は全 crate を見ている（境界のファイル自身は数に入れない）
    let sources = rust_sources(&root);
    assert!(sources.len() > 100, "走査したファイル数 {}", sources.len());
    assert!(sources.iter().any(|(rel, _)| rel == HOST), "境界のファイル");
    assert!(sources.iter().any(|(rel, _)| rel == MAIN), "main.rs");
}

/// 修正前の形を再現した注入が 1 本ずつ名指されること
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let base = Sources::load(&root);
    let main = &base.main;
    let webview = &base.webview;
    let host = &base.host;

    // I1: 合成を決める呼び出しが消える（Windows の Web ビューが暗いまま）
    let i1 = main.replace(
        "platform::webview_host::prepare_compositor()",
        "platform::webview_host::not_called()",
    );
    let r = scan_startup_order(&i1);
    assert!(
        r.iter().any(|o| o.why.contains("合成を決めていない")),
        "I1: {}",
        reports(&r)
    );

    // I2: tako が立てた env を application() より前に外す（gpui が読む前に消える）
    let i2 = {
        let lines: Vec<&str> = main.lines().collect();
        let after = lines
            .iter()
            .position(|l| l.contains("compositor.after_platform_created();"))
            .expect("外す行");
        let app = lines
            .iter()
            .position(|l| l.contains("application()"))
            .expect("application の行");
        let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let moved = v.remove(after);
        v.insert(app, moved);
        v.join("\n")
    };
    let r = scan_startup_order(&i2);
    assert!(
        r.iter().any(|o| o.why.contains("より前に外している")),
        "I2: {}",
        reports(&r)
    );

    // I3: 境界の外で env を直に立てる（main.rs へ set_var を足す）。境界のファイル自身は数えない
    let i3 = vec![
        (
            MAIN.to_string(),
            format!("fn main() {{\n    std::env::set_var(\"{GPUI_ENV}\", \"1\");\n}}\n"),
        ),
        (HOST.to_string(), host.clone()),
    ];
    let r = scan_env_single_place(&i3);
    assert!(
        r.len() == 1 && r[0].file == MAIN && r[0].line == 2,
        "I3: {}",
        reports(&r)
    );

    // I4: 宛先の口が AppKit の key_monitor だけを通る（修正前 = Windows で常に偽）
    let i4 = webview.replace(
        "Some(hwnd) => {\n            !legacy_2430() && crate::platform::webview_host::win32_child_holds_key_focus(hwnd)\n        }",
        "Some(_) => false,",
    );
    assert_ne!(&i4, webview, "I4 の注入が当たっていない");
    let r = scan_focus_routes(&i4);
    assert!(
        r.iter().any(
            |o| o.why.contains("webview_holds_key_focus") && o.why.contains("振り分けていない")
        ),
        "I4: {}",
        reports(&r)
    );

    // I5: key_focus_owner を macOS だけに閉じる（#2430 の前の形）
    let i5 = webview.replace(
        "pub fn key_focus_owner(",
        "#[cfg(target_os = \"macos\")]\npub fn key_focus_owner(",
    );
    let r = scan_focus_routes(&i5);
    assert!(
        r.iter().any(|o| o.why.contains("OS の cfg で閉じている")),
        "I5: {}",
        reports(&r)
    );

    // I6: 境界の Win32 の腕が定数の偽へ戻る
    let i6 = host.replace(
        "focus != 0 && focus != parent && win32::is_child(parent, focus)",
        "false",
    );
    assert_ne!(&i6, host, "I6 の注入が当たっていない");
    let r = scan_host_arms(&i6);
    assert!(
        r.iter().any(|o| o.why.contains("win32::is_child(")),
        "I6: {}",
        reports(&r)
    );

    // I7: 項目 71 に Windows の無条件スキップが戻る（目印の次の行 = 窓の中へ足す）
    let i7 = {
        let lines: Vec<&str> = main.lines().collect();
        let mark = lines
            .iter()
            .position(|l| l.contains("// 71. Web ビューペイン e2e"))
            .expect("目印");
        let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        v.insert(
            mark + 1,
            "            if cfg!(target_os = \"windows\") { println!(\"skip\"); }".into(),
        );
        v.join("\n")
    };
    let r = scan_self_test(&i7);
    assert!(
        r.iter()
            .any(|o| o.why.contains("項目 71: Windows の無条件スキップ")),
        "I7: {}",
        reports(&r)
    );

    // I8: 項目 155 のアドレスバーに Windows の無条件スキップが戻る
    let i8 = main.replace(
        "        // --- 2. Web ビューのアドレスバー ---\n",
        "        // --- 2. Web ビューのアドレスバー ---\n        if cfg!(target_os = \"windows\") { println!(\"TAKO_SELF_TEST_SKIPPED: 155 のアドレスバー\"); }\n",
    );
    assert_ne!(&i8, main, "I8 の注入が当たっていない");
    let r = scan_self_test(&i8);
    assert!(
        r.iter().any(|o| o
            .why
            .contains("項目 155 のアドレスバー: Windows の無条件スキップ")),
        "I8: {}",
        reports(&r)
    );

    // I9: 項目 71 の #1481 群を macOS だけに閉じる
    let i9 = {
        let lines: Vec<&str> = main.lines().collect();
        let owner = lines
            .iter()
            .position(|l| l.contains("/// 宛先が tako に在る（Web ビューが持っておらず"))
            .expect("#1481 群の頭");
        let mut v: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        // 群を開くブロックの `{` の上へ cfg を戻す
        let open = (0..owner)
            .rev()
            .find(|i| v[*i].trim() == "{")
            .expect("#1481 群のブロック");
        let indent = " ".repeat(v[open].len() - v[open].trim_start().len());
        v.insert(open, format!("{indent}#[cfg(target_os = \"macos\")]"));
        v.join("\n")
    };
    let r = scan_self_test(&i9);
    assert!(
        r.iter().any(|o| o.why.contains("macOS だけに閉じている")),
        "I9: {}",
        reports(&r)
    );

    // I10: A/B の口が消える
    let i10 = Sources {
        main: main.clone(),
        webview: webview.replace("\"TAKO_2430_LEGACY\"", "\"TAKO_XXXX\""),
        host: host.clone(),
    };
    let r = scan_legacy_gates(&i10);
    assert!(
        r.iter().any(|o| o.why.contains("TAKO_2430_LEGACY")),
        "I10: {}",
        reports(&r)
    );
}
