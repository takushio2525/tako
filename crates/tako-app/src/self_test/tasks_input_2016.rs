//! visual-test `tasks-input`（Issue #2016）: 右パネル tasks ビューの返答欄と返答の履歴。
//!
//! 場面: 本番と同じ規模の置き場（59 件・約 100 KB の `user-tasks.yaml`）を作り、最新の 1 件を
//! **実マウスで**展開して返答欄を押す。素のシェルのペインに master の role を貼り、返答の配送先に
//! する（生きている master へ運ぶ = 普段の経路）。
//!
//! 1. IME: GPUI の NSView へ **NSTextInputClient のメッセージを直接送る**
//!    （`setMarkedText:` / `insertText:` / `unmarkText` / `firstRectForCharacterRange:`。
//!    macOS の IME が呼ぶのと同じ入口で、入力ハンドラの登録から通る）。変換中の読みが欄の中に
//!    描かれ（画素が変わる + 未確定の矩形が欄の内側）、候補窓の矩形が欄のキャレットに来て、
//!    確定・unmark の文字が欄に入るか
//! 2. 折り返し: 欄の幅を超える本文が欄の中で複数行になり（画素の行の数）、欄が伸びるか
//! 3. 上限: 行を積むと欄は [`crate::tasks_panel::COMMENT_MAX_LINES`] 行で止まり、
//!    キャレットは欄の中に見えたままか（欄の中でスクロールする）
//! 4. 送信の間: 「返す」を実マウスで押してから**欄が空のフレーム**を描き終えるまでの時間
//!    （5 回の中央値）と、やりとりに載るまでの時間
//! 5. 履歴: やりとりが古い順・経路 pc で積まれ、`show`（= `tako todo show` / MCP）と同じか
//! 6. エッジ: 本文なしの返答 / 送信中にパネルを閉じる / 積めなかったら欄へ戻して理由を出す
//!
//! 観測はすべて `TAKO_VISUAL_PIXEL: tasks-input …` に出してから**最後にまとめて判定する**
//! （A/B の `TAKO_2016_LEGACY=1` でも送信の間の数字が採れる。旧挙動は 1 / 2 / 3 / 4 で落ちる）。
//! 画像は `TAKO_VISUAL_DUMP_DIR` へ `tasks-input-<場面>-<fixed|legacy>.png` で落とす。
//! 単独実行は `TAKO_VISUAL_ONLY=tasks-input`

use super::*;
use std::ffi::c_void;

const SECTION: &str = "tasks-input";
/// 配送先の master に見立てるプロファイル名（role の末尾）
const PROFILE: &str = "visual2016";

// --- NSTextInputClient を NSView へ直接送る（IME が呼ぶのと同じ入口） ----------------

#[repr(C)]
#[derive(Clone, Copy)]
struct NsRange {
    location: usize,
    length: usize,
}
/// `NSNotFound`（= `NSIntegerMax`）。置き換え範囲の「無し」
const NS_NOT_FOUND: usize = isize::MAX as usize;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NsPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NsSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NsRect {
    origin: NsPoint,
    size: NsSize,
}

// 宣言はクレート内の既存（`video_player.rs` / `webview.rs`）と**同じ形**にする
// （別の型で宣言し直すと `clashing_extern_declarations` で落ちる）
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const u8) -> *const c_void;
    fn sel_registerName(name: *const u8) -> *const c_void;
    fn objc_msgSend(receiver: *const c_void, selector: *const c_void, ...) -> *const c_void;
}

fn sel(name: &std::ffi::CStr) -> *const c_void {
    // SAFETY: 終端つきの C 文字列を渡すだけ
    unsafe { sel_registerName(name.as_ptr() as *const u8) }
}

/// `[NSString stringWithUTF8String:]`（autorelease。テストの数回ぶんなので溜まっても小さい）
fn ns_string(text: &str) -> *const c_void {
    let c = std::ffi::CString::new(text).expect("NUL を含まない");
    type F = unsafe extern "C" fn(
        *const c_void,
        *const c_void,
        *const std::ffi::c_char,
    ) -> *const c_void;
    // SAFETY: 引数の型どおりの非可変長の形で呼び直す（ARM64 の可変長は引数をスタックへ積むので
    // 構造体・ポインタを可変長のまま渡さない = main.rs の `SetFrame` と同じ作法）
    unsafe {
        let f: F = std::mem::transmute(objc_msgSend as *const ());
        f(
            objc_getClass(c"NSString".as_ptr() as *const u8),
            sel(c"stringWithUTF8String:"),
            c.as_ptr(),
        )
    }
}

/// GPUI の NSView（入力ハンドラを持つ view）。窓が生きているあいだ有効
fn ns_view(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> Option<usize> {
    window
        .update(cx, |_, window, _| {
            let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
            match handle.as_raw() {
                raw_window_handle::RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as usize),
                _ => None,
            }
        })
        .ok()
        .flatten()
}

/// `setMarkedText:selectedRange:replacementRange:`（変換開始・変換中の読みの更新）
fn ime_set_marked(view: usize, text: &str, selected: std::ops::Range<usize>) {
    type F = unsafe extern "C" fn(*const c_void, *const c_void, *const c_void, NsRange, NsRange);
    // SAFETY: GPUI の view はこのセレクタを実装している（gpui_macos の NSTextInputClient）。
    // **GPUI の update の外**で呼ぶ（ハンドラが中で app を借りるので二重借用にしない）
    unsafe {
        let f: F = std::mem::transmute(objc_msgSend as *const ());
        f(
            view as *const c_void,
            sel(c"setMarkedText:selectedRange:replacementRange:"),
            ns_string(text),
            NsRange {
                location: selected.start,
                length: selected.len(),
            },
            NsRange {
                location: NS_NOT_FOUND,
                length: 0,
            },
        );
    }
}

/// `insertText:replacementRange:`（確定）
fn ime_insert(view: usize, text: &str) {
    type F = unsafe extern "C" fn(*const c_void, *const c_void, *const c_void, NsRange);
    // SAFETY: `ime_set_marked` と同じ
    unsafe {
        let f: F = std::mem::transmute(objc_msgSend as *const ());
        f(
            view as *const c_void,
            sel(c"insertText:replacementRange:"),
            ns_string(text),
            NsRange {
                location: NS_NOT_FOUND,
                length: 0,
            },
        );
    }
}

/// `unmarkText`（未確定のまま確定扱い。変換中に別の所を押したとき等）
fn ime_unmark(view: usize) {
    type F = unsafe extern "C" fn(*const c_void, *const c_void);
    // SAFETY: `ime_set_marked` と同じ
    unsafe {
        let f: F = std::mem::transmute(objc_msgSend as *const ());
        f(view as *const c_void, sel(c"unmarkText"));
    }
}

/// `firstRectForCharacterRange:actualRange:`（**候補窓の位置**。画面座標・左下原点）
fn ime_first_rect(view: usize, range: std::ops::Range<usize>) -> NsRect {
    type F = unsafe extern "C" fn(*const c_void, *const c_void, NsRange, *mut NsRange) -> NsRect;
    // SAFETY: NSRect は double 4 つの HFA なので ARM64 では d0〜d3 で返る（_stret は要らない）
    unsafe {
        let f: F = std::mem::transmute(objc_msgSend as *const ());
        f(
            view as *const c_void,
            sel(c"firstRectForCharacterRange:actualRange:"),
            NsRange {
                location: range.start,
                length: range.len(),
            },
            std::ptr::null_mut(),
        )
    }
}

/// gpui_macos の `get_frame` と同じ式（firstRect の画面座標を窓の座標へ戻すのに使う）
fn gpui_frame(view: usize) -> NsRect {
    type Obj = unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void;
    type Rect = unsafe extern "C" fn(*const c_void, *const c_void) -> NsRect;
    type Mask = unsafe extern "C" fn(*const c_void, *const c_void) -> usize;
    // SAFETY: NSView / NSWindow の読み取りだけ（メインスレッド）
    unsafe {
        let obj: Obj = std::mem::transmute(objc_msgSend as *const ());
        let rect: Rect = std::mem::transmute(objc_msgSend as *const ());
        let mask: Mask = std::mem::transmute(objc_msgSend as *const ());
        let ns_window = obj(view as *const c_void, sel(c"window"));
        let mut frame = rect(ns_window, sel(c"frame"));
        let content = rect(ns_window, sel(c"contentLayoutRect"));
        let style = mask(ns_window, sel(c"styleMask"));
        // NSWindowStyleMaskFullSizeContentView = 1 << 15
        if style & (1 << 15) == 0 {
            frame.origin.y -= frame.size.height - content.size.height;
        }
        frame
    }
}

/// firstRect（画面座標）→ 窓の座標（GPUI の論理 px・左上原点）
fn screen_to_window(frame: NsRect, r: NsRect) -> Bounds<Pixels> {
    let x = r.origin.x - frame.origin.x;
    let y = frame.origin.y + frame.size.height - r.origin.y - r.size.height;
    Bounds::new(
        point(px(x as f32), px(y as f32)),
        size(px(r.size.width as f32), px(r.size.height as f32)),
    )
}

// --- 画素 ----------------------------------------------------------------------

/// 矩形（論理 px）の中で背景と違う画素が載っている**行の帯**の数と、違う画素の総数。
/// 文字の 1 行 = 1 本の帯になる（行の間は背景だけの行が挟まる）。
/// 背景は矩形の左下の内側から採る。読み戻しの上下の向きに依存しないよう両向きで数え、
/// 濃い側を採る（`ime_preedit_visual` と同じ作法）
fn ink_bands(img: &image::RgbaImage, rect: Bounds<Pixels>, scale: f32) -> (usize, u64) {
    let (w, h) = img.dimensions();
    let x0 = ((f32::from(rect.left()) + 3.0) * scale).max(0.0) as u32;
    let x1 = (((f32::from(rect.right()) - 3.0) * scale).max(0.0) as u32).min(w);
    let y0r = ((f32::from(rect.top()) + 2.0) * scale).max(0.0) as u32;
    let y1r = (((f32::from(rect.bottom()) - 2.0) * scale).max(0.0) as u32).min(h);
    let measure = |flip: bool| -> (usize, u64) {
        let (y0, y1) = if flip {
            (h.saturating_sub(y1r), h.saturating_sub(y0r))
        } else {
            (y0r.min(h), y1r)
        };
        if x1 <= x0 || y1 <= y0 {
            return (0, 0);
        }
        let bg = *img.get_pixel(x0, y1 - 1);
        let differs = |p: &image::Rgba<u8>| {
            p.0.iter()
                .zip(bg.0.iter())
                .take(3)
                .any(|(a, b)| a.abs_diff(*b) > 40)
        };
        let mut bands = 0usize;
        let mut total = 0u64;
        let mut in_band = false;
        for y in y0..y1 {
            let n = (x0..x1).filter(|x| differs(img.get_pixel(*x, y))).count() as u64;
            total += n;
            // 1 行に 2 画素未満の差はアンチエイリアスのにじみとして捨てる
            let ink = n >= 2;
            if ink && !in_band {
                bands += 1;
            }
            in_band = ink;
        }
        (bands, total)
    };
    let a = measure(false);
    let b = measure(true);
    if b.1 > a.1 {
        b
    } else {
        a
    }
}

/// 2 枚の矩形内の違う画素の数（変換中の読みが欄に描かれたか）
fn diff_in(a: &image::RgbaImage, b: &image::RgbaImage, rect: Bounds<Pixels>, scale: f32) -> u64 {
    if a.dimensions() != b.dimensions() {
        return 0;
    }
    let (w, h) = a.dimensions();
    let x0 = (f32::from(rect.left()) * scale).max(0.0) as u32;
    let x1 = ((f32::from(rect.right()) * scale).max(0.0) as u32).min(w);
    let y0 = (f32::from(rect.top()) * scale).max(0.0) as u32;
    let y1 = ((f32::from(rect.bottom()) * scale).max(0.0) as u32).min(h);
    let mut n = 0;
    for (yy0, yy1) in [(y0, y1), (h.saturating_sub(y1), h.saturating_sub(y0))] {
        let mut m = 0;
        for y in yy0..yy1.min(h) {
            for x in x0..x1 {
                if a.get_pixel(x, y) != b.get_pixel(x, y) {
                    m += 1;
                }
            }
        }
        n = n.max(m);
    }
    n
}

fn contains(outer: Bounds<Pixels>, inner: Bounds<Pixels>, slack: f32) -> bool {
    inner.left() >= outer.left() - px(slack)
        && inner.right() <= outer.right() + px(slack)
        && inner.top() >= outer.top() - px(slack)
        && inner.bottom() <= outer.bottom() + px(slack)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

fn todo_request(action: &str, id: Option<String>) -> tako_control::protocol::Request {
    tako_control::protocol::Request::UserTask {
        action: action.to_string(),
        id,
        title: None,
        body: None,
        kind: None,
        status: None,
        all: None,
        project: None,
        attachments: None,
        copy_texts: None,
        links: None,
        due: None,
        decision: None,
        comment: None,
        via: None,
        pane: None,
        caller_role: None,
    }
}

pub(super) async fn tasks_input_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let legacy = crate::tasks_panel::legacy_2016();
    let arm = if legacy { "legacy" } else { "fixed" };
    let dump = std::env::var("TAKO_VISUAL_DUMP_DIR").ok();
    let shot = |img: &image::RgbaImage, name: &str| {
        if let Some(dir) = dump.as_ref() {
            let _ = std::fs::create_dir_all(dir);
            let path = std::path::Path::new(dir).join(format!("{SECTION}-{name}-{arm}.png"));
            if img.save(&path).is_ok() {
                println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
            }
        }
    };
    let say = |line: String| println!("TAKO_VISUAL_PIXEL: {SECTION} [{arm}] {line}");

    // **本番の todo に書かない**（置き場が隔離されていなければ何もしないで落ちる）
    let Some(store) = tako_control::user_tasks::store_path() else {
        fail("tasks-input: user-tasks.yaml の置き場を解決できない");
    };
    check(
        std::env::var_os("TAKO_USER_TASKS_FILE").is_some()
            || std::env::var_os("TAKO_ORCHESTRATOR_DIR").is_some(),
        &format!(
            "tasks-input: 置き場が隔離されていない（TAKO_ORCHESTRATOR_DIR か TAKO_USER_TASKS_FILE が要る。{}）",
            store.display()
        ),
    );

    let shell = ensure_fresh_scene(window, cx, SECTION).await;
    // 素のシェルを起票元 master に見立てる（返答の配送先 = 普段の「生きている master」の経路）
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Title {
                pane: Some(shell.as_u64()),
                title: None,
                role: Some(format!("orchestrator-master:{PROFILE}")),
            },
            PaneOrigin::Cli,
        );
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Panel {
                visible: Some(true),
                width: Some(320.0),
                view: Some(tako_control::protocol::PanelViewWire::Tasks),
                filetree: None,
                sidebar_width: None,
                show_hidden: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });

    // --- 素材: 本番と同じ規模（59 件・約 100 KB） ---
    let body: String = (0..30)
        .map(|i| format!("- 確認する項目 {i} について詳しく\n"))
        .collect();
    let target = window
        .update(cx, |app, _, cx| {
            let mut last = None;
            for i in 0..59 {
                let mut req = todo_request("add", None);
                if let tako_control::protocol::Request::UserTask {
                    title,
                    body: b,
                    project,
                    caller_role,
                    ..
                } = &mut req
                {
                    *title = Some(format!("タスク {i} の確認"));
                    *b = Some(body.clone());
                    *project = Some("tako".into());
                    *caller_role = Some(format!("orchestrator-master:{PROFILE}"));
                }
                last = tako_control::dispatch(app, req, PaneOrigin::Cli)
                    .ok()
                    .and_then(|v| v["id"].as_str().map(str::to_string));
            }
            app.refresh_user_tasks();
            cx.notify();
            last
        })
        .ok()
        .flatten();
    let Some(target) = target else {
        fail("tasks-input: 検証用のタスクを起票できない");
    };
    let store_bytes = std::fs::metadata(&store).map(|m| m.len()).unwrap_or(0);
    say(format!(
        "store tasks=59 bytes={store_bytes} target={target}"
    ));

    let probe = |cx: &mut AsyncApp, key: &str| -> Option<Bounds<Pixels>> {
        window
            .update(cx, |app, _, _| {
                app.panel_click_probe_bounds.borrow().get(key).copied()
            })
            .ok()
            .flatten()
    };
    let redraw = |cx: &mut AsyncApp| {
        let _ = window.update(cx, |app, _, cx| {
            app.panel_click_probe_bounds.borrow_mut().clear();
            cx.notify();
        });
        notify_and_draw(any, window, cx);
    };
    // パネルの一覧をホイールの実イベントで送り、`key` の矩形を一覧の見える範囲へ出す
    // （本文の長いタスクは返答欄が一覧の下へ押し出される = 普段もユーザーはホイールで送る）
    let bring_into_view = |cx: &mut AsyncApp, key: &str| -> Option<Bounds<Pixels>> {
        for _ in 0..40 {
            redraw(cx);
            let view = window
                .update(cx, |app, _, _| app.user_tasks.scroll.bounds())
                .ok()?;
            if let Some(b) = probe(cx, key) {
                if b.top() >= view.top() && b.bottom() <= view.bottom() {
                    return Some(b);
                }
                if b.top() < view.top() {
                    // 行き過ぎ（上へはみ出した）は少し戻す
                    let at = point(view.center().x, view.top() + px(30.0));
                    let _ = any.update(cx, |_, win, cx| {
                        win.dispatch_event(
                            gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                                position: at,
                                delta: ScrollDelta::Pixels(point(
                                    px(0.0),
                                    view.top() - b.top() + px(20.0),
                                )),
                                ..ScrollWheelEvent::default()
                            }),
                            cx,
                        );
                    });
                    continue;
                }
            }
            // 一覧の上端寄り（返答欄の中で回すと欄の中が送られる）でホイールを下へ
            let at = point(view.center().x, view.top() + px(30.0));
            let _ = any.update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: at,
                        delta: ScrollDelta::Pixels(point(px(0.0), px(-160.0))),
                        ..ScrollWheelEvent::default()
                    }),
                    cx,
                );
            });
        }
        None
    };
    let comment = |cx: &mut AsyncApp| -> String {
        window
            .update(cx, |app, _, _| app.user_tasks.comment.text().to_string())
            .unwrap_or_default()
    };

    // --- 展開（最新 = 一覧の先頭の行を実マウスで押す） ---
    redraw(cx);
    wait(cx, 100).await;
    redraw(cx);
    let Some(row) = probe(cx, "tasks-row-0") else {
        fail("tasks-input: 一覧の先頭の行の矩形が採れない");
    };
    click_at(any, cx, row.center());
    redraw(cx);
    let expanded = window
        .update(cx, |app, _, _| app.user_tasks.expanded.clone())
        .ok()
        .flatten();
    check(
        expanded.as_deref() == Some(target.as_str()),
        &format!("tasks-input: 押した行が展開される（{expanded:?} / 期待 {target}）"),
    );
    redraw(cx);
    let (frame0, scale) =
        capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
    shot(&frame0, "expanded");

    // --- 返答欄を実マウスで押して打つ ---
    let Some(box0) = bring_into_view(cx, "tasks-comment") else {
        fail("tasks-input: 返答欄を一覧の見える範囲へ出せない");
    };
    click_at(any, cx, box0.center());
    let focused = window
        .update(cx, |app, _, _| app.user_tasks.comment_focused)
        .unwrap_or(false);
    check(focused, "tasks-input: 返答欄を押すと欄が打鍵を受ける");
    type_text(any, cx, "abc", false);
    redraw(cx);
    redraw(cx);
    let one_line_box = probe(cx, "tasks-comment").unwrap_or(box0);
    let typed = comment(cx);
    say(format!(
        "typed={typed:?} box_h1={:?}",
        one_line_box.size.height
    ));

    // =============================================================
    // 1. IME（NSTextInputClient を NSView へ直接）
    // =============================================================
    let Some(view) = ns_view(window, cx) else {
        fail("tasks-input: GPUI の NSView が採れない");
    };
    let (before_ime, _) =
        capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
    ime_set_marked(view, "にほんご", 0..4);
    redraw(cx);
    redraw(cx);
    let (during_ime, _) =
        capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
    shot(&during_ime, "ime-marked");
    let ime_box = probe(cx, "tasks-comment").unwrap_or(one_line_box);
    let marked_rect = probe(cx, "tasks-comment-marked");
    let (ime_dest, comment_during) = window
        .update(cx, |app, _, _| {
            (
                app.ime.as_ref().map(|i| format!("{:?}", i.app_input)),
                app.user_tasks.comment.text().to_string(),
            )
        })
        .unwrap_or((None, String::new()));
    let ime_ink = diff_in(&before_ime, &during_ime, ime_box, scale);
    let marked_inside = marked_rect.is_some_and(|m| contains(ime_box, m, 1.0));
    // 候補窓の位置（IME が実際に聞く口）。窓の座標へ戻して欄の内側か
    let frame = gpui_frame(view);
    let first = ime_first_rect(view, 0..4);
    let cand = screen_to_window(frame, first);
    let cand_inside = cand.size.height > px(0.0)
        && contains(
            ime_box,
            Bounds::new(cand.origin, size(px(1.0), cand.size.height)),
            1.0,
        );
    say(format!(
        "ime dest={ime_dest:?} comment_during={comment_during:?} box={ime_box:?} \
         marked={marked_rect:?} marked_inside={marked_inside} ink_in_box={ime_ink} \
         candidate={cand:?} candidate_inside_box={cand_inside}"
    ));
    ime_insert(view, "日本語");
    redraw(cx);
    let after_commit = comment(cx);
    ime_set_marked(view, "かな", 0..2);
    redraw(cx);
    ime_unmark(view);
    redraw(cx);
    let after_unmark = comment(cx);
    say(format!(
        "ime commit={after_commit:?} unmark={after_unmark:?}"
    ));
    let ime_ok = ime_dest.as_deref() == Some("Some(TaskComment)")
        && comment_during == "abc"
        && marked_inside
        && ime_ink > 0
        && cand_inside
        && after_commit == "abc日本語"
        && after_unmark == "abc日本語かな";

    // =============================================================
    // 2. 折り返し
    // =============================================================
    let long = format!(
        " 折り返しの確認{}{}",
        "とても長い返答の文章が欄の幅を超えて続きます。".repeat(4),
        "ASCII_without_any_spaces_".repeat(3)
    );
    ime_insert(view, &long);
    redraw(cx);
    let _ = bring_into_view(cx, "tasks-comment");
    redraw(cx);
    let (wrap_frame, _) =
        capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
    shot(&wrap_frame, "wrap");
    let wrap_box = probe(cx, "tasks-comment").unwrap_or(one_line_box);
    let wrap_text = probe(cx, "tasks-comment-text");
    let (bands, ink) = ink_bands(&wrap_frame, wrap_box, scale);
    let text_inside = wrap_text.is_some_and(|t| t.right() <= wrap_box.right() + px(1.0));
    say(format!(
        "wrap len={} box_h={:?} (1 行のとき {:?}) bands={bands} ink={ink} text={wrap_text:?} \
         text_inside_box={text_inside}",
        comment(cx).chars().count(),
        wrap_box.size.height,
        one_line_box.size.height
    ));
    let wrap_ok = bands >= 3
        && wrap_box.size.height
            >= one_line_box.size.height + px(crate::tasks_panel::COMMENT_LINE_H)
        && text_inside;

    // =============================================================
    // 3. 上限（行を積んでも欄は止まり、キャレットは見えたまま）
    // =============================================================
    let many: String = (1..=30).map(|i| format!("\n{i} 行目")).collect();
    let _ = window.update(cx, |app, _, cx| app.task_comment_insert(&many, cx));
    redraw(cx);
    redraw(cx);
    let _ = bring_into_view(cx, "tasks-comment");
    redraw(cx);
    let (max_frame, _) =
        capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
    shot(&max_frame, "max-lines");
    let max_box = probe(cx, "tasks-comment").unwrap_or(one_line_box);
    let caret = probe(cx, "tasks-comment-caret");
    let cap =
        crate::tasks_panel::COMMENT_LINE_H * crate::tasks_panel::COMMENT_MAX_LINES + 8.0 + 2.0;
    let caret_visible = caret.is_some_and(|c| contains(max_box, c, 1.0));
    say(format!(
        "max box_h={:?} cap={cap} caret={caret:?} caret_visible={caret_visible}",
        max_box.size.height
    ));
    let max_ok = max_box.size.height <= px(cap + 1.0) && caret_visible;

    // =============================================================
    // 4. 送信の間（実マウスで「返す」→ 欄が空のフレームを描き終えるまで）
    // =============================================================
    let responses_of = |cx: &mut AsyncApp| -> usize {
        window
            .update(cx, |app, _, _| {
                app.user_tasks
                    .snapshot
                    .tasks
                    .iter()
                    .find(|t| t.id == target)
                    .map(|t| t.responses.len())
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    };
    let mut baseline_ms = f64::NAN;
    let mut pending_shown = true;
    let mut in_handler = Vec::new();
    let mut to_empty = Vec::new();
    let mut to_listed = Vec::new();
    let mut empty_every_time = true;
    for k in 0..5 {
        let _ = window.update(cx, |app, _, cx| {
            app.user_tasks.comment.clear();
            app.user_tasks.decision = Some("answered".to_string());
            app.task_comment_insert(&format!("{k} 回目の返答"), cx);
        });
        redraw(cx);
        let Some(send) = bring_into_view(cx, "tasks-send") else {
            fail("tasks-input: 「返す」を一覧の見える範囲へ出せない");
        };
        let before = responses_of(cx);
        // 押す前に乗せて 1 フレーム描く（hit test を確定させる。click_at と同じ前段）
        let pos = send.center();
        let _ = any.update(cx, |_, win, cx| {
            win.dispatch_event(
                gpui::PlatformInput::MouseMove(MouseMoveEvent {
                    position: pos,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            )
        });
        notify_and_draw(any, window, cx);
        // 切り分け: 何もしない所（一覧の上端の余白）を同じ手順で押したときのハンドラ時間
        if k == 0 {
            let view = window
                .update(cx, |app, _, _| app.user_tasks.scroll.bounds())
                .unwrap_or_default();
            let idle = point(view.right() - px(4.0), view.top() + px(2.0));
            let t = std::time::Instant::now();
            for input in [
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: idle,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: idle,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
            ] {
                let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
            }
            baseline_ms = t.elapsed().as_secs_f64() * 1000.0;
            say(format!("baseline idle_click_handler_ms={baseline_ms:.1}"));
            let _ = any.update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::PlatformInput::MouseMove(MouseMoveEvent {
                        position: pos,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                )
            });
            notify_and_draw(any, window, cx);
        }
        let t0 = std::time::Instant::now();
        for input in [
            gpui::PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: pos,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            gpui::PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: pos,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
        ] {
            let _ = any.update(cx, |_, win, cx| win.dispatch_event(input, cx));
        }
        // 押したハンドラが UI スレッドを握っていた時間（往復を待っていればここに乗る）
        in_handler.push(t0.elapsed().as_secs_f64() * 1000.0);
        // 修正後は返した中身が「送信中」として残る（欄を先に空にしても消えたように見せない）。
        // 旧実装は押した中で積み終えるので、この時点でもう一覧に載っている
        let (sending_now, listed_now) = window
            .update(cx, |app, _, _| {
                (
                    app.user_tasks.sending.iter().any(|s| s.id == target),
                    app.user_tasks
                        .snapshot
                        .tasks
                        .iter()
                        .find(|t| t.id == target)
                        .map(|t| t.responses.len())
                        .unwrap_or(0),
                )
            })
            .unwrap_or((false, 0));
        pending_shown &= sending_now || listed_now > before;
        // 押した次のフレーム（欄が空なら、このフレームに空が描かれる）
        notify_and_draw(any, window, cx);
        let frame_ms = t0.elapsed().as_secs_f64() * 1000.0;
        empty_every_time &= comment(cx).is_empty();
        to_empty.push(frame_ms);
        // やりとりに載るまで（積む → 運ぶ → 記録 → 読み直し）
        let mut listed = f64::NAN;
        for _ in 0..400 {
            if responses_of(cx) > before {
                listed = t0.elapsed().as_secs_f64() * 1000.0;
                break;
            }
            wait(cx, 5).await;
        }
        to_listed.push(listed);
    }
    let handler_ms = median(&mut in_handler.clone());
    let empty_ms = median(&mut to_empty.clone());
    let listed_ms = median(&mut to_listed.clone());
    let send_cost_ms = handler_ms - baseline_ms;
    say(format!(
        "send handler_ms={in_handler:.1?} median={handler_ms:.1} send_cost_ms={send_cost_ms:.1} \
         pending_or_listed_at_once={pending_shown} \
         click->empty_frame_ms={to_empty:.1?} median={empty_ms:.1} \
         click->listed_ms={to_listed:.1?} median={listed_ms:.1} empty_every_time={empty_every_time}"
    ));
    let _ = bring_into_view(cx, "tasks-send");
    redraw(cx);
    let (sent_frame, _) =
        capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
    shot(&sent_frame, "history");
    // 次のフレームで空 = 押した直後に描いたフレームが空で、**送信が押下に足す時間**（何もしない所を
    // 同じ手順で押したときとの差）が 1 フレームの半分（8 ms）未満。押下そのものの配送の費用は
    // ビルド（debug / release）と画面の重さで変わるので差で見る。旧実装はハンドラが往復
    // （積む → 運ぶ → 記録 → 一覧）を握るので、その間ぶん差が開く
    let lag_ok = empty_every_time
        && pending_shown
        && send_cost_ms.is_finite()
        && send_cost_ms < 8.0
        && to_listed.iter().all(|v| v.is_finite());

    // =============================================================
    // 5. 履歴（古い順・経路 pc。show と同じ）
    // =============================================================
    let shown = window
        .update(cx, |app, _, _| {
            tako_control::dispatch(
                app,
                todo_request("show", Some(target.clone())),
                PaneOrigin::Mcp,
            )
            .ok()
        })
        .ok()
        .flatten();
    let rows: Vec<(i64, String, String)> = shown
        .as_ref()
        .and_then(|v| v["responses"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|r| {
            (
                r["at"].as_i64().unwrap_or(0),
                r["via"].as_str().unwrap_or("").to_string(),
                r["comment"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let oldest_first = rows.windows(2).all(|w| w[0].0 <= w[1].0)
        && rows.first().is_some_and(|r| r.2.starts_with("0 回目"))
        && rows.last().is_some_and(|r| r.2.starts_with("4 回目"));
    let panel_rows: Vec<String> = window
        .update(cx, |app, _, _| {
            app.user_tasks
                .snapshot
                .tasks
                .iter()
                .find(|t| t.id == target)
                .map(|t| t.responses.iter().map(|r| r.comment.clone()).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let same_as_show = panel_rows == rows.iter().map(|r| r.2.clone()).collect::<Vec<_>>();
    say(format!(
        "history n={} oldest_first={oldest_first} via={:?} same_as_show={same_as_show}",
        rows.len(),
        rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>()
    ));
    let history_ok =
        rows.len() == 5 && oldest_first && rows.iter().all(|r| r.1 == "pc") && same_as_show;

    // =============================================================
    // 6. エッジ
    // =============================================================
    // (a) 本文なしの返答（判断だけ）
    let before = responses_of(cx);
    let _ = window.update(cx, |app, _, cx| {
        app.user_tasks.comment.clear();
        app.user_tasks.decision = Some("answered".to_string());
        app.user_task_respond(&target, cx);
    });
    for _ in 0..400 {
        if responses_of(cx) > before {
            break;
        }
        wait(cx, 5).await;
    }
    let bare_ok = responses_of(cx) == before + 1;
    // (b) 送信中にパネルを閉じる（落ちない・閉じたままでも積まれる・開けば読める）
    let before = responses_of(cx);
    let _ = window.update(cx, |app, _, cx| {
        app.user_tasks.decision = Some("answered".to_string());
        app.task_comment_insert("閉じても届く返答", cx);
        app.user_task_respond(&target, cx);
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Panel {
                visible: Some(false),
                width: None,
                view: None,
                filetree: None,
                sidebar_width: None,
                show_hidden: None,
            },
            PaneOrigin::Cli,
        );
        cx.notify();
    });
    for _ in 0..400 {
        let sending = window
            .update(cx, |app, _, _| app.user_tasks.sending.len())
            .unwrap_or(0);
        if sending == 0 {
            break;
        }
        notify_and_draw(any, window, cx);
        wait(cx, 5).await;
    }
    let _ = window.update(cx, |app, _, cx| {
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::Panel {
                visible: Some(true),
                width: None,
                view: Some(tako_control::protocol::PanelViewWire::Tasks),
                filetree: None,
                sidebar_width: None,
                show_hidden: None,
            },
            PaneOrigin::Cli,
        );
        app.refresh_user_tasks();
        app.user_tasks.expanded = Some(target.clone());
        cx.notify();
    });
    redraw(cx);
    let closed_ok = responses_of(cx) == before + 1;
    // (d) 続けて 3 回押しても押した順に積まれる（並行に積むと YAML のロックを取る順で入れ替わる）
    let _ = window.update(cx, |app, _, cx| {
        for i in 0..3 {
            app.user_tasks.decision = Some("answered".to_string());
            app.task_comment_insert(&format!("連続 {i}"), cx);
            app.user_task_respond(&target, cx);
        }
    });
    for _ in 0..600 {
        let sending = window
            .update(cx, |app, _, _| app.user_tasks.sending.len())
            .unwrap_or(0);
        if sending == 0 {
            break;
        }
        wait(cx, 5).await;
    }
    let burst: Vec<String> = window
        .update(cx, |app, _, _| {
            app.user_tasks
                .snapshot
                .tasks
                .iter()
                .find(|t| t.id == target)
                .map(|t| {
                    t.responses
                        .iter()
                        .filter(|r| r.comment.starts_with("連続 "))
                        .map(|r| r.comment.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let burst_ok = burst == ["連続 0", "連続 1", "連続 2"];
    say(format!("burst order={burst:?}"));
    // (c) 積めなかったら欄へ戻して理由を出す（積む段で対象が消えている = 他所で消された）。
    //     旧実装は同期で失敗するので欄が残る（この項目は修正後だけ見る）
    let mut restore_ok = true;
    if !legacy {
        let original = std::fs::read_to_string(&store).unwrap_or_default();
        let gone = original.replace(&format!("id: {target}\n"), "id: u-99999\n");
        let _ = std::fs::write(&store, gone);
        let _ = window.update(cx, |app, _, cx| {
            app.user_tasks.comment.clear();
            app.user_tasks.decision = Some("needs_change".to_string());
            app.task_comment_insert("戻ってくるはずの返答", cx);
            app.user_task_respond(&target, cx);
        });
        let cleared_at_once = comment(cx).is_empty();
        for _ in 0..400 {
            let sending = window
                .update(cx, |app, _, _| app.user_tasks.sending.len())
                .unwrap_or(0);
            if sending == 0 {
                break;
            }
            wait(cx, 5).await;
        }
        let (back, decision, notice) = window
            .update(cx, |app, _, _| {
                (
                    app.user_tasks.comment.text().to_string(),
                    app.user_tasks.decision.clone(),
                    app.user_tasks.notice.clone(),
                )
            })
            .unwrap_or_default();
        let _ = std::fs::write(&store, original);
        redraw(cx);
        let (fail_frame, _) =
            capture_frame(any, cx).unwrap_or_else(|| fail("tasks-input: フレーム採取"));
        shot(&fail_frame, "send-failed");
        say(format!(
            "restore cleared_at_once={cleared_at_once} back={back:?} decision={decision:?} notice={notice:?}"
        ));
        restore_ok = cleared_at_once
            && back == "戻ってくるはずの返答"
            && decision.as_deref() == Some("needs_change")
            && notice.is_some_and(|n| n.contains(&target));
        let _ = window.update(cx, |app, _, _| app.user_tasks.reset_form());
    }
    say(format!(
        "edge bare={bare_ok} close_while_sending={closed_ok} burst_in_order={burst_ok} \
         restore_on_failure={restore_ok}"
    ));

    // --- 後片付け（次の節へ持ち越さない） ---
    let _ = window.update(cx, |app, _, cx| {
        app.user_tasks.reset_form();
        app.user_tasks.expanded = None;
        app.panel_visible = false;
        app.panel_view = PanelView::Fleet;
        cx.notify();
    });
    let _ = std::fs::remove_file(&store);

    // --- 判定（観測は上で全部出してある） ---
    check(ime_ok, &format!("tasks-input [{arm}]: IME の変換中の読みが欄に描かれ、候補窓が欄のキャレットに出て、確定 / unmark が欄に入る (#2016)"));
    check(
        wrap_ok,
        &format!("tasks-input [{arm}]: 欄の幅を超える本文が折り返し、欄が行数ぶん伸びる (#2016)"),
    );
    check(
        max_ok,
        &format!(
            "tasks-input [{arm}]: 欄は上限の行数で止まり、キャレットは欄の中に見えたまま (#2016)"
        ),
    );
    check(lag_ok, &format!("tasks-input [{arm}]: 押した次のフレームで欄が空になる（送信が押下に足す時間 {send_cost_ms:.1} ms / 空のフレームまで {empty_ms:.1} ms）(#2016)"));
    check(
        history_ok,
        &format!("tasks-input [{arm}]: やりとりが古い順・経路 pc で積まれ show と同じ (#2016)"),
    );
    check(
        bare_ok && closed_ok && burst_ok && restore_ok,
        &format!("tasks-input [{arm}]: 本文なし / 送信中に閉じる / 続けて押しても順に積む / 積めなかったら戻す (#2016)"),
    );
    println!("TAKO_VISUAL_2016: ok arm={arm}");
}
