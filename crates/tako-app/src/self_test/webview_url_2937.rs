//! visual-test `webview-url-2937`（Issue #2937）: Web ビュー（wry）が解釈できない・開かせるべきでない
//! URL を、どの入口から渡しても **GUI が生きたまま理由つきで断る**か。
//!
//! 修正前は `tako web open 'javascript:alert(2936)'` が正規化で `https://javascript:alert(2936)`
//! （ポートが数字でない形）になり、wry の `NSURL::URLWithString(…).unwrap()` が GUI ごと abort した。
//!
//! ① CLI / MCP と同じ dispatch（`Request::Web { action: "open" }`）で `javascript:alert(2937)` →
//!    断り（日本語の理由に `javascript:` と `tako web eval`）・分割したペインは閉じ戻る・Web ビューは増えない
//! ② 同じく表に無い scheme（`mailto:`）・macOS では NSURL が解釈できない形（`https://example.com:abc`）
//! ③ 今開ける形（`data:` の小さなページ）は開ける → 開いたページへ `nav javascript:alert(1)` は断り、
//!    URL は変わらない
//! ④ dock の URL 欄へ本番と同じ打鍵の入口（`dispatch_keystroke`）で `javascript:alert(1)` + Enter →
//!    dock は開いたまま・欄の下に理由・Web ビューは増えない。1 文字打ち足すと理由は消える
//! ⑤ アドレスバーへ同じく `javascript:alert(1)` + Enter → バーは開いたまま・本文に理由・URL は変わらない。
//!    Esc でバーを閉じると理由も消える
//! ⑥ 最後に GUI が生きている（dispatch の `list` が答える）
//!
//! **A/B**: `TAKO_2937_LEGACY=1`（修正前の正規化 + 形の検査だけ）は macOS では ① で GUI ごと abort する
//! （終了コードが 0 でも 1 でもない = FAILED の行も出ない）。単独実行は `TAKO_VISUAL_ONLY=webview-url-2937`
//! （`scripts/test-webview-url-2937.sh` が CLI の実経路と並べて回す）

use super::*;

const SECTION: &str = "webview-url-2937";

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）
fn dump(any: AnyWindowHandle, cx: &mut AsyncApp, name: &str) {
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    let Some((frame, _)) = capture_frame(any, cx) else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(name);
    if frame.save(&path).is_ok() {
        println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
    }
}

fn web_req(
    action: &str,
    url: Option<&str>,
    to: Option<&str>,
    pane: Option<PaneId>,
    id: Option<u64>,
) -> tako_control::protocol::Request {
    tako_control::protocol::Request::Web {
        action: action.into(),
        url: url.map(str::to_string),
        id,
        pane: pane.map(|p| p.as_u64()),
        direction: None,
        to: to.map(str::to_string),
        js: None,
        token: None,
        focus: None,
    }
}

/// CLI / MCP と同じ dispatch（応答は `Ok(値)` か断りの文）
fn dispatch_web(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    request: tako_control::protocol::Request,
) -> Result<serde_json::Value, String> {
    window
        .update(cx, |app, _, cx| {
            let mut result = tako_control::dispatch(app, request, PaneOrigin::Cli);
            let _ = app.after_dispatch(&mut result, false, cx);
            cx.notify();
            result.map_err(|e| e.text_in(tako_core::i18n::Lang::Ja))
        })
        .unwrap_or_else(|_| {
            fail(&format!(
                "visual-test {SECTION}: 窓を更新できない（GUI が落ちた？）"
            ))
        })
}

/// (タブ内のペイン数, Web ビューの数)
fn counts(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> (usize, usize) {
    window
        .update(cx, |app, _, _| {
            (
                app.workspace.active_tab().tree().panes().len(),
                app.webviews.len(),
            )
        })
        .unwrap_or((0, 0))
}

/// 本番と同じ打鍵の入口へ 1 文字ずつ流し、最後に `last`（`enter` / `escape`）を押す
fn type_then(any: AnyWindowHandle, cx: &mut AsyncApp, text: &str, last: Option<&str>) {
    let text = text.to_string();
    let last = last.map(str::to_string);
    let _ = any.update(cx, |_, win, cx| {
        for ch in text.chars() {
            win.dispatch_keystroke(
                Keystroke {
                    modifiers: Modifiers::default(),
                    key: ch.to_string(),
                    key_char: Some(ch.to_string()),
                },
                cx,
            );
        }
        if let Some(key) = last {
            win.dispatch_keystroke(Keystroke::parse(&key).unwrap(), cx);
        }
    });
}

pub(super) async fn webview_url_2937_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_core::i18n::Lang;

    inject_section_failure(SECTION);
    let fresh = ensure_fresh_scene(window, cx, SECTION).await;
    let lang_before = tako_core::i18n::lang();
    tako_core::i18n::set_lang(Lang::Ja);
    let mac = cfg!(target_os = "macos");

    // ① CLI / MCP の open（#2937 の実測の入力）
    let (panes0, webs0) = counts(window, cx);
    let refused = dispatch_web(
        window,
        cx,
        web_req(
            "open",
            Some("javascript:alert(2937)"),
            None,
            Some(fresh),
            None,
        ),
    );
    let (panes1, webs1) = counts(window, cx);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ① open javascript: → {refused:?} panes {panes0}→{panes1} webviews {webs0}→{webs1}"
    );
    let text = refused.as_ref().err().cloned().unwrap_or_default();
    check(
        text.contains("javascript: の URL は Web ビューで開かない")
            && text.contains("tako web eval"),
        &format!(
            "visual-test {SECTION} ①: open javascript: を理由と次の一手つきで断る（{refused:?}）"
        ),
    );
    check(
        panes1 == panes0 && webs1 == webs0,
        &format!(
            "visual-test {SECTION} ①: 断った open がペインも Web ビューも残さない（ペイン {panes0}→{panes1}・Web ビュー {webs0}→{webs1}）"
        ),
    );

    // ② 表に無い scheme・NSURL が解釈できない形
    let mailto = dispatch_web(
        window,
        cx,
        web_req("open", Some("mailto:someone"), None, Some(fresh), None),
    );
    check(
        mailto
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("mailto: の URL は Web ビューで開けない")),
        &format!("visual-test {SECTION} ②: 表に無い scheme を断る（{mailto:?}）"),
    );
    if mac {
        let bad_port = dispatch_web(
            window,
            cx,
            web_req(
                "open",
                Some("https://example.com:abc"),
                None,
                Some(fresh),
                None,
            ),
        );
        check(
            bad_port
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("URL を解釈できない")),
            &format!("visual-test {SECTION} ②: NSURL が解釈できない形を断る（{bad_port:?}）"),
        );
    }
    let (panes2, webs2) = counts(window, cx);
    check(
        panes2 == panes0 && webs2 == webs0,
        &format!("visual-test {SECTION} ②: 断った open がペインも Web ビューも残さない（{panes2} / {webs2}）"),
    );

    // ③ 今開ける形は開ける・開いたページへの nav javascript: は断る
    let page = "data:text/html,<title>t2937</title>ok";
    let opened = dispatch_web(
        window,
        cx,
        web_req("open", Some(page), None, Some(fresh), None),
    );
    let id = opened
        .as_ref()
        .ok()
        .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64))
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {SECTION} ③: data: のページを開けない（{opened:?}）"
            ))
        });
    let web_pane = window
        .update(cx, |app, _, _| {
            app.webviews
                .iter()
                .find(|e| e.id.as_u64() == id)
                .and_then(|e| e.pane)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            fail(&format!(
                "visual-test {SECTION} ③: 開いた Web ビューがペインに載っていない"
            ))
        });
    let nav = dispatch_web(
        window,
        cx,
        web_req(
            "navigate",
            None,
            Some("javascript:alert(1)"),
            None,
            Some(id),
        ),
    );
    let url_after_nav = window
        .update(cx, |app, _, _| {
            app.webviews
                .iter()
                .find(|e| e.id.as_u64() == id)
                .map(|e| e.current_url())
        })
        .ok()
        .flatten()
        .unwrap_or_default();
    println!("TAKO_VISUAL_PIXEL: {SECTION} ③ open data: → id {id} pane {web_pane:?} / nav javascript: → {nav:?} url {url_after_nav:?}");
    check(
        nav.as_ref()
            .err()
            .is_some_and(|e| e.contains("javascript: の URL は Web ビューで開かない")),
        &format!("visual-test {SECTION} ③: 開いたページへの nav javascript: を断る（{nav:?}）"),
    );
    check(
        !url_after_nav.starts_with("javascript:"),
        &format!("visual-test {SECTION} ③: 断った nav で URL が変わらない（{url_after_nav:?}）"),
    );

    // ④ dock の URL 欄（本番と同じ打鍵の入口）
    let _ = window.update(cx, |app, _, cx| {
        app.webview_dock_open = true;
        app.webview_dock_url.clear();
        app.focus_web_dock_url();
        cx.notify();
    });
    let webs_before_dock = counts(window, cx).1;
    type_then(any, cx, "javascript:alert(1)", Some("enter"));
    let (dock_open, dock_text, dock_reason, webs_after_dock) = window
        .update(cx, |app, _, _| {
            (
                app.webview_dock_open,
                app.webview_dock_url.text().to_string(),
                app.web_url_refusal_at(WebUrlPlace::Dock)
                    .map(|r| r.text_in(Lang::Ja)),
                app.webviews.len(),
            )
        })
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ④ dock open={dock_open} text={dock_text:?} reason={dock_reason:?} webviews {webs_before_dock}→{webs_after_dock}"
    );
    dump(any, cx, "webview-url-2937-dock.png");
    check(
        dock_open && dock_text == "javascript:alert(1)",
        &format!("visual-test {SECTION} ④: 断った dock は開いたまま打った文字列を残す（{dock_open} / {dock_text:?}）"),
    );
    check(
        dock_reason
            .as_deref()
            .is_some_and(|r| r.contains("javascript: の URL は Web ビューで開かない")),
        &format!("visual-test {SECTION} ④: dock の URL 欄の下に理由を出す（{dock_reason:?}）"),
    );
    check(
        webs_after_dock == webs_before_dock,
        &format!("visual-test {SECTION} ④: 断った dock が Web ビューを作らない（{webs_before_dock}→{webs_after_dock}）"),
    );
    type_then(any, cx, "x", None);
    let dock_reason_after_edit = window
        .update(cx, |app, _, _| {
            app.web_url_refusal_at(WebUrlPlace::Dock).is_some()
        })
        .unwrap_or(true);
    check(
        !dock_reason_after_edit,
        &format!("visual-test {SECTION} ④: 打ち直すと dock の理由は消える"),
    );
    let _ = window.update(cx, |app, _, cx| {
        app.webview_dock_url.clear();
        app.blur_web_dock_url();
        app.webview_dock_open = false;
        cx.notify();
    });

    // ⑤ アドレスバー（本番と同じ打鍵の入口）
    let _ = window.update(cx, |app, _, cx| {
        let _ = app.workspace.active_tab_mut().tree_mut().focus(web_pane);
        app.open_web_address_bar(web_pane);
        if let Some(field) = app.webview_address_bar.get_mut(&web_pane.as_u64()) {
            field.set_text(String::new());
        }
        cx.notify();
    });
    type_then(any, cx, "javascript:alert(1)", Some("enter"));
    let (bar_open, bar_reason, url_after_bar) = window
        .update(cx, |app, _, _| {
            (
                app.web_address_bar_editing() == Some(web_pane),
                app.web_url_refusal_at(WebUrlPlace::AddressBar(web_pane))
                    .map(|r| r.text_in(Lang::Ja)),
                app.webviews
                    .iter()
                    .find(|e| e.id.as_u64() == id)
                    .map(|e| e.current_url())
                    .unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} ⑤ address bar open={bar_open} reason={bar_reason:?} url={url_after_bar:?}"
    );
    dump(any, cx, "webview-url-2937-address-bar.png");
    check(
        bar_open,
        &format!("visual-test {SECTION} ⑤: 断ったアドレスバーは開いたまま（打ち直せる）"),
    );
    check(
        bar_reason
            .as_deref()
            .is_some_and(|r| r.contains("javascript: の URL は Web ビューで開かない")),
        &format!("visual-test {SECTION} ⑤: アドレスバーの下の本文に理由を出す（{bar_reason:?}）"),
    );
    check(
        !url_after_bar.starts_with("javascript:"),
        &format!(
            "visual-test {SECTION} ⑤: 断ったアドレスバーで URL が変わらない（{url_after_bar:?}）"
        ),
    );
    type_then(any, cx, "", Some("escape"));
    let (bar_closed, refusal_left) = window
        .update(cx, |app, _, _| {
            (
                app.web_address_bar_editing().is_none(),
                app.webview_url_refusal.is_some(),
            )
        })
        .unwrap_or((false, true));
    check(
        bar_closed && !refusal_left,
        &format!("visual-test {SECTION} ⑤: Esc でバーを閉じると理由も消える（閉じた={bar_closed}・残り={refusal_left}）"),
    );

    // ⑥ GUI が生きている + 後片付け
    let listed = dispatch_web(window, cx, web_req("list", None, None, None, None));
    check(
        listed.is_ok(),
        &format!("visual-test {SECTION} ⑥: 断った後も GUI が答える（{listed:?}）"),
    );
    let _ = dispatch_web(window, cx, web_req("close", None, None, None, Some(id)));
    tako_core::i18n::set_lang(lang_before);
    println!("TAKO_VISUAL_PIXEL: {SECTION} OK");
}
