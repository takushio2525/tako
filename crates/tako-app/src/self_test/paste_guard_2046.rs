//! visual-test `paste-guard`（FR-4.12 / Issue #2046）: 鍵らしき文字を貼ると PTY へ流す前に確認が出て、
//! やめると PTY へ 1 バイトも届かず、送ると届くか。設定 off なら確認なしで届くか。
//!
//! **本物のクリップボードには書かない**（偽の鍵でも OS のクリップボードと履歴に残る）。⌘V と同じ
//! `TakoApp::paste()` を通し、クリップボードの代わりに visual-test だけの差し込み口
//! （`injected_paste`）から本文を読ませる（振り分け・関所は本番と同じ関数）。
//! 受け手は `stty raw -echo; exec cat > <file>` のペイン（届いたバイトをそのままファイルへ）。
//! ブラケットペーストを要求するアプリ（claude 等）の代役は同じものに `ESC[?2004h` を出させる。
//!
//! 「1 バイトも届かない」は**後から送った目印で確かめる**: PTY は順序を保つので、やめた本文が
//! 書かれていれば目印より前に現れる。ファイルが「前の中身 + 目印」と一致すれば何も届いていない。
//!
//! ① 普通の文章（日本語・改行入り）は確認なしで届く
//! ② 鍵を貼ると確認が出る（規則・画素）。Esc → やめる・Enter → やめる（Enter も PTY へ流れない）・
//!    「やめる」ボタン・カードの外を押す → やめる。どれも目印の前に何も届かない
//! ③ 確認中の次の貼り付けは積まない（答えた後も届かない）
//! ④ 「送る」ボタン → 本文がそのまま届く
//! ⑤ 空・改行だけは確認しない
//! ⑥ 設定 off → 確認なしで届く・on に戻すと次から確認
//! ⑦ 複数の鍵の混在 → 3 規則を並べる・CLI / MCP と同じ答え（dispatch の respond）でやめる
//! ⑧ 巨大な貼り付け（1 MiB）→ 先頭の鍵で確認が出る（打ち切りの申告）・判定の所要
//! ⑨ ブラケットペーストを要求するアプリ → 送ると `ESC[200~` 本文 `ESC[201~` で届く
//! ⑩ 設定画面「一般」の「貼り付けの確認」トグルを実マウスで押す → 切ると確認なしで届き、戻すと確認が出る
//! ⑪ persist.log に本文・伏せ字が出ない（規則名と件数だけ）
//!
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ `paste-guard-dialog.png` / `paste-guard-multi.png` /
//! `paste-guard-settings-off.png` で落とす。
//! `TAKO_2046_LEGACY=1`（同一バイナリの旧挙動）は ② で FAILED になる。
//! 単独実行は `TAKO_VISUAL_ONLY=paste-guard`

use super::*;

const SECTION: &str = "paste-guard";

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// 偽の鍵（**本物を書かない**。前置詞と本体を実行時に連結し、本体は `EXAMPLE` を含む）
fn fake_key(prefix: &str, body: &str) -> String {
    format!("{prefix}{body}")
}

fn dispatch_req(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    request: tako_control::protocol::Request,
) -> serde_json::Value {
    window
        .update(cx, |app, _, cx| {
            let mut result = tako_control::dispatch(app, request, PaneOrigin::Cli);
            let _ = app.after_dispatch(&mut result, false, cx);
            cx.notify();
            result
        })
        .ok()
        .and_then(Result::ok)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: dispatch が通らない")))
}

fn paste_guard_req(
    enabled: Option<bool>,
    respond: Option<&str>,
) -> tako_control::protocol::Request {
    tako_control::protocol::Request::PasteGuard {
        enabled,
        check: None,
        respond: respond.map(str::to_string),
    }
}

/// 受け手のペイン（届いたバイトをそのまま `sink` へ）。`bracketed` ならブラケットペーストを要求する
fn sink_pane(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    from: PaneId,
    sink: &std::path::Path,
    bracketed: bool,
) -> PaneId {
    let request = if bracketed {
        "printf '\\033[?2004h'; "
    } else {
        ""
    };
    let script = format!(
        "{request}stty raw -echo; printf SINK-READY; exec cat > '{}'",
        sink.display()
    );
    let reply = dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Split {
            pane: Some(from.as_u64()),
            tab: None,
            direction: Some(tako_control::protocol::Direction::Right),
            ratio: None,
            command: Some(vec!["sh".into(), "-c".into(), script]),
            cwd: None,
            focus: Some(true),
        },
    );
    reply["pane"]
        .as_u64()
        .map(PaneId::from_raw)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 分割の応答に pane が無い")))
}

fn focus(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) {
    dispatch_req(
        window,
        cx,
        tako_control::protocol::Request::Focus {
            pane: Some(pane.as_u64()),
            direction: None,
        },
    );
}

fn screen_of(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pane: PaneId) -> String {
    window
        .update(cx, |app, _, _| {
            app.terminals
                .get(&pane)
                .map(|s| s.visible_lines().join("\n"))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// ⌘V と同じ `paste()` を通す（クリップボードの代わりに差し込み口から読ませる）
fn paste(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, text: &str) {
    let text = text.to_string();
    let _ = window.update(cx, |app, _, cx| {
        app.injected_paste = Some(text);
        app.paste(cx);
        cx.notify();
    });
}

/// 確認待ちの規則（無ければ `None`）
fn pending_rules(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) -> Option<Vec<tako_core::paste_guard::Rule>> {
    window
        .update(cx, |app, _, _| {
            app.pending_paste.as_ref().map(|p| p.report.rules())
        })
        .ok()
        .flatten()
}

fn stats(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) -> tako_core::paste_guard::PasteGuardStats {
    window
        .update(cx, |app, _, _| app.paste_guard_stats.clone())
        .unwrap_or_default()
}

fn read_sink(sink: &std::path::Path) -> Vec<u8> {
    std::fs::read(sink).unwrap_or_default()
}

/// 受け手のファイルが `want` になるまで待つ（上限 3 秒）。最後に読んだ中身を返す
async fn wait_sink(cx: &mut AsyncApp, sink: &std::path::Path, want: &[u8]) -> Vec<u8> {
    let started = std::time::Instant::now();
    loop {
        let got = read_sink(sink);
        if got == want || started.elapsed() > Duration::from_secs(3) {
            return got;
        }
        sleep(cx, 50).await;
    }
}

/// 目印を貼り、ファイルが「前の中身 + 目印」で揃うか（= 目印より前に何も届いていない）
async fn mark_only(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    sink: &std::path::Path,
    expected: &mut Vec<u8>,
    mark: &str,
) -> bool {
    paste(window, cx, mark);
    expected.extend_from_slice(mark.as_bytes());
    let got = wait_sink(cx, sink, expected).await;
    got == *expected
}

fn rect(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, id: &str) -> Option<Bounds<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.input_rects
                .borrow()
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, b)| *b)
        })
        .ok()
        .flatten()
}

/// 確認のダイアログが描かれ、ボタンの矩形が採れるまで描く
async fn draw_until_dialog(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) -> bool {
    for _ in 0..40 {
        notify_and_draw(any, window, cx);
        sleep(cx, 30).await;
        if rect(window, cx, "paste-guard-send").is_some()
            && rect(window, cx, "paste-guard-cancel").is_some()
        {
            return true;
        }
    }
    false
}

/// 確認が閉じたあとに古い矩形で押さないよう、採った矩形を捨てる
fn forget_rects(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    let _ = window.update(cx, |app, _, _| {
        app.input_rects
            .borrow_mut()
            .retain(|(i, _)| !i.starts_with("paste-guard-"));
    });
}

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width / 2.0,
        b.origin.y + b.size.height / 2.0,
    )
}

/// フレームの矩形（論理 px）の中で、2 枚の画素が違う数と矩形の画素数
fn diff_in(
    before: &image::RgbaImage,
    after: &image::RgbaImage,
    rect: Bounds<Pixels>,
    scale: f32,
) -> (u64, u64) {
    let (w, h) = after.dimensions();
    let x0 = ((f32::from(rect.origin.x) * scale) as u32).min(w);
    let y0 = ((f32::from(rect.origin.y) * scale) as u32).min(h);
    let x1 = ((f32::from(rect.origin.x + rect.size.width) * scale) as u32).min(w);
    let y1 = ((f32::from(rect.origin.y + rect.size.height) * scale) as u32).min(h);
    let mut changed = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if before.dimensions() == after.dimensions()
                && before.get_pixel(x, y) != after.get_pixel(x, y)
            {
                changed += 1;
            }
        }
    }
    (changed, u64::from(x1 - x0) * u64::from(y1 - y0))
}

fn dump(image: &image::RgbaImage, name: &str) {
    if let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dir = std::path::Path::new(&dir);
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(name);
        if image.save(&path).is_ok() {
            println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
        }
    }
}

/// 秘密の 8 文字の部分列のどれかが `haystack` に出ているか（伏せ字は `…` を挟むので当たらない）
fn leaks(haystack: &str, secret: &str) -> bool {
    let chars: Vec<char> = secret.chars().collect();
    chars.windows(8).any(|w| {
        let piece: String = w.iter().collect();
        haystack.contains(&piece)
    })
}

pub(super) async fn paste_guard_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_core::paste_guard::Rule;
    let origin = ensure_fresh_scene(window, cx, SECTION).await;
    let dir = std::env::temp_dir().join(format!("tk2046-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 一時 dir を作れない: {e}")));
    let sink = dir.join("plain.bin");
    let sink_b = dir.join("bracketed.bin");
    let _ = window.update(cx, |app, _, cx| {
        app.input_rect_probe = true;
        cx.notify();
    });

    // 受け手 2 枚（素の cat / ブラケットペーストを要求する cat）
    let plain = sink_pane(window, cx, origin, &sink, false);
    let bracketed = sink_pane(window, cx, plain, &sink_b, true);
    for pane in [plain, bracketed] {
        let started = std::time::Instant::now();
        while !screen_of(window, cx, pane).contains("SINK-READY") {
            check(
                started.elapsed() < Duration::from_secs(15),
                &format!("visual-test {SECTION}: 前提 = 受け手のペイン {pane} が立つ"),
            );
            notify_and_draw(any, window, cx);
            sleep(cx, 50).await;
        }
    }
    focus(window, cx, plain);
    let mut expected: Vec<u8> = Vec::new();
    check(
        mark_only(window, cx, &sink, &mut expected, "MARK0").await,
        &format!("visual-test {SECTION}: 前提 = 目印が受け手へ届く"),
    );

    let anthropic = fake_key("sk-ant-", "api03-EXAMPLEaaaaBBBBccccDDDDeeee1234ABCD");
    let github = fake_key("ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst");
    let aws = fake_key("AKIA", "EXAMPLE000000000");
    let secret_text = format!("この鍵で試して: {anthropic}");

    // ① 普通の文章（日本語・改行入り）は確認なしで届く（改行は CR へ正規化される）
    let prose = "普通の長文です。API キーの話をしていても\n塊に分かれるので確認は出ません。";
    paste(window, cx, prose);
    expected.extend_from_slice(prose.replace('\n', "\r").as_bytes());
    let got = wait_sink(cx, &sink, &expected).await;
    check(
        pending_rules(window, cx).is_none() && got == expected,
        &format!(
            "visual-test {SECTION} ①: 普通の文章は確認なしで届く (pending={:?} bytes={} want={})",
            pending_rules(window, cx),
            got.len(),
            expected.len()
        ),
    );

    // ② 鍵を貼ると確認が出る
    notify_and_draw(any, window, cx);
    let (before, scale) = capture_frame(any, cx).unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: 出す前のフレームを撮れない"
        ))
    });
    paste(window, cx, &secret_text);
    let rules = pending_rules(window, cx);
    let drawn = draw_until_dialog(any, window, cx).await;
    let (after, _) = capture_frame(any, cx).unwrap_or_else(|| {
        fail(&format!(
            "visual-test {SECTION}: 出した後のフレームを撮れない"
        ))
    });
    dump(&after, "paste-guard-dialog.png");
    let dialog = rect(window, cx, "paste-guard-dialog");
    let (changed, total) = dialog
        .map(|d| diff_in(&before, &after, d, scale))
        .unwrap_or((0, 0));
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} dialog rules={:?} drawn={drawn} changed={changed}/{total}",
        rules
            .as_ref()
            .map(|r| r.iter().map(|x| x.as_str()).collect::<Vec<_>>())
    );
    check(
        rules == Some(vec![Rule::AnthropicApiKey]),
        &format!(
            "visual-test {SECTION} ②: 鍵を貼ると確認が出る（規則 anthropic_api_key） rules={:?}",
            rules
                .as_ref()
                .map(|r| r.iter().map(|x| x.as_str()).collect::<Vec<_>>())
        ),
    );
    check(
        drawn && total > 0 && changed * 2 >= total,
        &format!(
            "visual-test {SECTION} ②: 確認のカードが画面に描かれる (changed={changed}/{total})"
        ),
    );
    // Esc → やめる
    press(any, cx, "escape");
    forget_rects(window, cx);
    check(
        pending_rules(window, cx).is_none()
            && mark_only(window, cx, &sink, &mut expected, "MARK1").await,
        &format!("visual-test {SECTION} ②: Esc でやめると PTY へ 1 バイトも届かない"),
    );
    // Enter → やめる（Enter そのものも PTY へ流れない）
    paste(window, cx, &secret_text);
    check(
        pending_rules(window, cx).is_some(),
        &format!("visual-test {SECTION} ②: 2 回目の貼り付けでも確認が出る"),
    );
    press(any, cx, "enter");
    forget_rects(window, cx);
    check(
        pending_rules(window, cx).is_none()
            && mark_only(window, cx, &sink, &mut expected, "MARK2").await,
        &format!("visual-test {SECTION} ②: Enter はやめる側で、本文も改行も届かない"),
    );
    // 「やめる」ボタン（実マウス）
    paste(window, cx, &secret_text);
    check(
        draw_until_dialog(any, window, cx).await,
        &format!("visual-test {SECTION} ②: やめるボタンが描かれる"),
    );
    let cancel = rect(window, cx, "paste-guard-cancel")
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: やめるボタンの矩形が無い")));
    click_at(any, cx, center(cancel));
    forget_rects(window, cx);
    check(
        pending_rules(window, cx).is_none()
            && mark_only(window, cx, &sink, &mut expected, "MARK3").await,
        &format!("visual-test {SECTION} ②: やめるボタンで PTY へ 1 バイトも届かない"),
    );

    // ③ 確認中の次の貼り付けは積まない。カードの外を押す → やめる
    paste(window, cx, &secret_text);
    paste(window, cx, "QUEUED-TEXT");
    let still = pending_rules(window, cx);
    check(
        draw_until_dialog(any, window, cx).await,
        &format!("visual-test {SECTION} ③: 確認が描かれる"),
    );
    let card = rect(window, cx, "paste-guard-dialog")
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: カードの矩形が無い")));
    click_at(
        any,
        cx,
        point(
            card.origin.x + card.size.width / 2.0,
            card.origin.y + card.size.height + px(24.0),
        ),
    );
    forget_rects(window, cx);
    check(
        still == Some(vec![Rule::AnthropicApiKey])
            && pending_rules(window, cx).is_none()
            && mark_only(window, cx, &sink, &mut expected, "MARK4").await,
        &format!(
            "visual-test {SECTION} ③: 確認中の貼り付けは積まず、カードの外を押すとやめる（何も届かない）"
        ),
    );

    // ④ 「送る」ボタン（実マウス）→ 本文がそのまま届く
    paste(window, cx, &secret_text);
    check(
        draw_until_dialog(any, window, cx).await,
        &format!("visual-test {SECTION} ④: 送るボタンが描かれる"),
    );
    let send = rect(window, cx, "paste-guard-send")
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 送るボタンの矩形が無い")));
    click_at(any, cx, center(send));
    forget_rects(window, cx);
    expected.extend_from_slice(secret_text.as_bytes());
    let got = wait_sink(cx, &sink, &expected).await;
    check(
        pending_rules(window, cx).is_none() && got == expected,
        &format!(
            "visual-test {SECTION} ④: 送るボタンで本文がそのまま届く (bytes={} want={})",
            got.len(),
            expected.len()
        ),
    );

    // ⑤ 空・改行だけは確認しない
    paste(window, cx, "");
    paste(window, cx, "\n\n");
    expected.extend_from_slice(b"\r\r");
    let got = wait_sink(cx, &sink, &expected).await;
    check(
        pending_rules(window, cx).is_none() && got == expected,
        &format!("visual-test {SECTION} ⑤: 空・改行だけは確認なしで素通しする"),
    );

    // ⑥ 設定 off → 確認なしで届く。on へ戻すと次から確認
    let off = dispatch_req(window, cx, paste_guard_req(Some(false), None));
    paste(window, cx, &secret_text);
    expected.extend_from_slice(secret_text.as_bytes());
    let got = wait_sink(cx, &sink, &expected).await;
    check(
        off["enabled"] == serde_json::json!(false)
            && pending_rules(window, cx).is_none()
            && got == expected,
        &format!("visual-test {SECTION} ⑥: 設定 off なら確認なしで届く"),
    );
    let on = dispatch_req(window, cx, paste_guard_req(Some(true), None));
    paste(window, cx, &secret_text);
    check(
        on["enabled"] == serde_json::json!(true) && pending_rules(window, cx).is_some(),
        &format!("visual-test {SECTION} ⑥: on へ戻すと次の貼り付けから確認が出る"),
    );
    dispatch_req(window, cx, paste_guard_req(None, Some("cancel")));

    // ⑦ 複数の鍵の混在 → 3 規則。CLI / MCP と同じ答え（dispatch の respond）でやめる
    let multi = format!("ANTHROPIC={anthropic}\ngh: {github}\naws {aws}\n");
    paste(window, cx, &multi);
    let rules = pending_rules(window, cx);
    let status = dispatch_req(window, cx, paste_guard_req(None, None));
    let _ = draw_until_dialog(any, window, cx).await;
    if let Some((frame, _)) = capture_frame(any, cx) {
        dump(&frame, "paste-guard-multi.png");
    }
    let rendered = status.to_string();
    check(
        rules
            == Some(vec![
                Rule::AnthropicApiKey,
                Rule::GithubToken,
                Rule::AwsAccessKey,
            ])
            && status["pending"]["pane"] == serde_json::json!(plain.as_u64())
            && !leaks(&rendered, &anthropic)
            && !leaks(&rendered, &github),
        &format!(
            "visual-test {SECTION} ⑦: 3 規則を並べ、状態の応答は伏せ字だけ rules={:?}",
            rules
                .as_ref()
                .map(|r| r.iter().map(|x| x.as_str()).collect::<Vec<_>>())
        ),
    );
    let answered = dispatch_req(window, cx, paste_guard_req(None, Some("cancel")));
    forget_rects(window, cx);
    check(
        answered["responded"] == serde_json::json!("cancel")
            && pending_rules(window, cx).is_none()
            && mark_only(window, cx, &sink, &mut expected, "MARK5").await,
        &format!("visual-test {SECTION} ⑦: dispatch の respond=cancel でやめると何も届かない"),
    );

    // ⑧ 巨大な貼り付け（1 MiB）: 先頭の鍵で確認が出る・打ち切りを申告する・判定の所要
    let filler = "lorem ipsum dolor sit amet, 日本語も混ぜた長い本文。".repeat(16_000);
    let huge = format!("{anthropic}\n{filler}");
    paste(window, cx, &huge);
    let huge_pending = window
        .update(cx, |app, _, _| {
            app.pending_paste
                .as_ref()
                .map(|p| (p.report.truncated(), p.report.total_bytes))
        })
        .ok()
        .flatten();
    let huge_stats = stats(window, cx);
    dispatch_req(window, cx, paste_guard_req(None, Some("cancel")));
    forget_rects(window, cx);
    // 鍵の無い巨大な貼り付け（300 KB）は確認なしで届く
    let plain_huge = "lorem ipsum dolor sit amet ".repeat(11_000);
    paste(window, cx, &plain_huge);
    let plain_stats = stats(window, cx);
    expected.extend_from_slice(plain_huge.as_bytes());
    let got = wait_sink(cx, &sink, &expected).await;
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} huge bytes={} scan_us={} scanned={} / plain_huge bytes={} scan_us={}",
        huge.len(),
        huge_stats.last_scan_us,
        huge_stats.last_scanned_bytes,
        plain_huge.len(),
        plain_stats.last_scan_us
    );
    check(
        huge_pending.is_some_and(|(t, n)| t && n == huge.len())
            && huge_stats.last_scanned_bytes == tako_core::paste_guard::MAX_SCAN_BYTES
            && pending_rules(window, cx).is_none()
            && got == expected,
        &format!(
            "visual-test {SECTION} ⑧: 巨大な貼り付けは先頭だけを見て確認し、鍵が無ければ届く \
             (pending={huge_pending:?} bytes={} want={})",
            got.len(),
            expected.len()
        ),
    );

    // ⑨ ブラケットペーストを要求するアプリ: 送ると ESC[200~ 本文 ESC[201~ で届く
    focus(window, cx, bracketed);
    let mut expected_b: Vec<u8> = Vec::new();
    paste(window, cx, "plain-text");
    expected_b.extend_from_slice(b"\x1b[200~plain-text\x1b[201~");
    let got_b = wait_sink(cx, &sink_b, &expected_b).await;
    check(
        got_b == expected_b,
        &format!(
            "visual-test {SECTION} ⑨: 前提 = 受け手がブラケットペーストを要求している (bytes={})",
            got_b.len()
        ),
    );
    paste(window, cx, &secret_text);
    let pending_pane =
        dispatch_req(window, cx, paste_guard_req(None, None))["pending"]["pane"].as_u64();
    dispatch_req(window, cx, paste_guard_req(None, Some("send")));
    forget_rects(window, cx);
    expected_b.extend_from_slice(format!("\x1b[200~{secret_text}\x1b[201~").as_bytes());
    let got_b = wait_sink(cx, &sink_b, &expected_b).await;
    check(
        pending_pane == Some(bracketed.as_u64()) && got_b == expected_b,
        &format!(
            "visual-test {SECTION} ⑨: ブラケットペーストのアプリへも確認してから包んで届く (bytes={} want={})",
            got_b.len(),
            expected_b.len()
        ),
    );

    // ⑩ 設定画面「一般」の「貼り付けの確認」トグルを実マウスで押す → 次の貼り付けから効く
    use crate::settings_window::SettingsTab;
    let _ = window.update(cx, |app, _, cx| {
        app.open_settings_window_impl(Some(SettingsTab::General), cx);
    });
    let mut settings = None;
    for _ in 0..40 {
        settings = window
            .update(cx, |app, _, _| app.settings_window_handle)
            .ok()
            .flatten();
        if settings.is_some() {
            break;
        }
        sleep(cx, 100).await;
    }
    let Some(settings) = settings else {
        fail(&format!(
            "visual-test {SECTION} ⑩: 設定ウィンドウが開かない"
        ));
    };
    let settings_any: AnyWindowHandle = settings.into();
    let _ = settings.update(cx, |view, window, cx| {
        view.set_tab(SettingsTab::General, cx);
        window.resize(size(px(760.), px(600.)));
        cx.notify();
    });
    sleep(cx, 250).await;
    let _ = capture_frame(settings_any, cx);
    // トグルの実矩形（枠の外ならそこまでスクロールしてから採り直す = `profiles` 節と同じ作法）
    let find_toggle = |cx: &mut AsyncApp| {
        settings
            .update(cx, |view, _, _| {
                let handle = view.scroll_handle(SettingsTab::General);
                (
                    view.vt_probe_bounds()
                        .iter()
                        .find(|(key, _)| key == "ctl:toggle-paste_guard")
                        .map(|(_, r)| *r),
                    crate::settings_window::to_rect(handle.bounds()),
                    f32::from(handle.offset().y),
                )
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION} ⑩: トグルの実測矩形")))
    };
    let press_toggle = async |cx: &mut AsyncApp| {
        let (target, view, offset) = find_toggle(cx);
        let Some(mut target) = target else {
            fail(&format!(
                "visual-test {SECTION} ⑩: 「貼り付けの確認」のトグルが描かれていない"
            ));
        };
        if target.y0 < view.y0 || target.y1 > view.y1 {
            let want = offset - (target.y0 - view.y0 - 60.0);
            let _ = settings.update(cx, |view, _, cx| {
                view.scroll_handle(SettingsTab::General)
                    .set_offset(point(px(0.), px(want)));
                cx.notify();
            });
            sleep(cx, 250).await;
            let _ = capture_frame(settings_any, cx);
            target = find_toggle(cx).0.unwrap_or_else(|| {
                fail(&format!(
                    "visual-test {SECTION} ⑩: スクロール後にトグルが消えた"
                ))
            });
        }
        click_at(
            settings_any,
            cx,
            point(
                px((target.x0 + target.x1) / 2.),
                px((target.y0 + target.y1) / 2.),
            ),
        );
        sleep(cx, 300).await;
    };
    let guard_on = |window: WindowHandle<TakoApp>, cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.paste_guard)
            .unwrap_or(true)
    };
    // 永続化はセルフテスト（TAKO_SELF_TEST）では止まるので、その時だけ見ない
    let persisted = |want: bool| {
        std::env::var_os("TAKO_SELF_TEST").is_some()
            || tako_control::settings::load().paste_guard == want
    };
    press_toggle(cx).await;
    if let Some((frame, _)) = capture_frame(settings_any, cx) {
        dump(&frame, "paste-guard-settings-off.png");
    }
    let off_now = !guard_on(window, cx) && persisted(false);
    paste(window, cx, &secret_text);
    expected_b.extend_from_slice(format!("\x1b[200~{secret_text}\x1b[201~").as_bytes());
    let got_b = wait_sink(cx, &sink_b, &expected_b).await;
    check(
        off_now && pending_rules(window, cx).is_none() && got_b == expected_b,
        &format!(
            "visual-test {SECTION} ⑩: 設定画面のトグルで切ると、次の貼り付けは確認なしで届く \
             (off={off_now} bytes={} want={})",
            got_b.len(),
            expected_b.len()
        ),
    );
    press_toggle(cx).await;
    let on_now = guard_on(window, cx) && persisted(true);
    paste(window, cx, &secret_text);
    check(
        on_now && pending_rules(window, cx).is_some(),
        &format!(
            "visual-test {SECTION} ⑩: 設定画面のトグルで戻すと、次の貼り付けから確認が出る (on={on_now})"
        ),
    );
    dispatch_req(window, cx, paste_guard_req(None, Some("cancel")));
    forget_rects(window, cx);
    let _ = settings_any.update(cx, |_, w, _| w.remove_window());

    // ⑪ persist.log に本文・伏せ字が出ない（規則名と件数だけ）
    let log = tako_control::diag::persist_log_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let masked = tako_core::paste_guard::scan(&anthropic)
        .findings
        .first()
        .map(|f| f.masked.clone())
        .unwrap_or_default();
    let final_stats = stats(window, cx);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} stats checked={} prompted={} sent={} cancelled={} log_lines={}",
        final_stats.checked,
        final_stats.prompted,
        final_stats.sent,
        final_stats.cancelled,
        log.lines().filter(|l| l.contains("貼り付けの確認")).count()
    );
    check(
        log.contains("貼り付けの確認: prompted")
            && log.contains("rules=[anthropic_api_key]")
            && !leaks(&log, &anthropic)
            && !leaks(&log, &github)
            && !leaks(&log, &aws)
            && !log.contains(&masked)
            && !log.contains("QUEUED-TEXT"),
        &format!("visual-test {SECTION} ⑪: persist.log に本文・伏せ字が出ない（規則名と件数だけ）"),
    );
    check(
        final_stats.prompted == 10 && final_stats.sent == 2 && final_stats.cancelled == 8,
        &format!(
            "visual-test {SECTION}: 累計が操作と一致する (prompted={} sent={} cancelled={})",
            final_stats.prompted, final_stats.sent, final_stats.cancelled
        ),
    );

    // 後片付け（以降の節へ持ち越さない）
    let _ = window.update(cx, |app, _, cx| {
        app.input_rect_probe = false;
        cx.notify();
    });
    let _ = std::fs::remove_dir_all(&dir);
}
