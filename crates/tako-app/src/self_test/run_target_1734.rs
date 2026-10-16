//! visual-test `run-target-1734`（FR-3.18.9 / Issue #1734。エピック #1726 の S6）: 実行ボタン横の
//! ドロップダウンの「実行先」の行を**実マウスで**切り替え、**実マウスの再生ボタン**がその実行先で走ることを
//! 確かめる（dispatch `RunnerDefaults { target }` → `run-configs.json` → 次の `Run`）。
//!
//! 場面: 一時 dir（`.git` + 宣言 1 つの `a.sh`）。
//!
//! - ① `a.sh` を開く → シェブロンを実マウスで押す → 実行先の行（同じペイン / 新しいペイン）が描かれ、
//!   既定は「同じペイン」・出典は既定（画像 `open`）
//! - ② 「新しいペイン」を押す → `run-configs.json` の files に `target: "new"`・出典はファイル（画像 `new`）・
//!   選択肢が出典タグに重ならない（④ でも見る）
//! - ③ 再生ボタンを実マウスで 2 回押す → タブのペインが **2 枚**増える（画像 `two`。A/B:
//!   `TAKO_1734_LEGACY=1` は保存しても使い回すので 1 枚しか増えず、ここで名指しの FAILED）
//! - ④ 「同じペイン」を押す → `target: "reuse"`（保存先は今の出典 = ファイル）→ 再生ボタン 2 回で増えない（画像 `reuse`）
//! - ⑤ 出典タグを押す → 保存先がプロジェクトへ移る（files から消え projects に入る）→ × で既定へ戻る
//!   （どこにも残らない・出典は既定。画像 `reset`）
//! - ⑥ 境界: 今と同じ選択肢（既定の「同じペイン」）を押しても何も書かない
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ `run-target-1734-<節>.png` で落とす。
//! 単独実行は `TAKO_VISUAL_ONLY=run-target-1734`（`scripts/test-run-target-1734.sh` が新旧を回す）

use super::*;
use serde_json::Value;

const LABEL: &str = "run-target-1734";

/// 画像を落とす（`TAKO_VISUAL_DUMP_DIR` があるときだけ）
fn dump(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp, name: &str) {
    notify_and_draw(any, window, cx);
    let Ok(dir) = std::env::var("TAKO_VISUAL_DUMP_DIR") else {
        return;
    };
    let Some((frame, _)) = capture_frame(any, cx) else {
        return;
    };
    let dir = std::path::Path::new(&dir);
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("{LABEL}-{name}.png"));
    if frame.save(&path).is_ok() {
        println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
    }
}

fn probe_bounds(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
) -> Option<gpui::Bounds<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds.borrow().get(key).copied()
        })
        .ok()
        .flatten()
}

/// メニューの中の行を、メニュー本体を実ホイールで送って見える位置まで出す（実行先の行はメニューの末尾）
fn reveal(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp, key: &str) {
    if !key.starts_with("run-menu-") {
        return;
    }
    for _ in 0..40 {
        notify_and_draw(any, window, cx);
        let (Some(menu), Some(b)) = (
            probe_bounds(window, cx, "run-menu"),
            probe_bounds(window, cx, key),
        ) else {
            return;
        };
        let top = menu.top() + px(6.0);
        let bottom = menu.bottom() - px(6.0);
        if b.top() >= top && b.bottom() <= bottom {
            return;
        }
        let dy = if b.bottom() > bottom {
            -(b.bottom() - bottom + px(12.0))
        } else {
            top - b.top() + px(12.0)
        };
        let at = menu.center();
        let _ = any.update(cx, |_, win, cx| {
            win.dispatch_event(
                gpui::PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: at,
                    delta: ScrollDelta::Pixels(point(px(0.0), dy)),
                    ..ScrollWheelEvent::default()
                }),
                cx,
            );
        });
    }
}

/// 実マウスで押す（矩形が無ければ名指しで落とす）
fn click_probe(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
    step: &str,
) {
    notify_and_draw(any, window, cx);
    reveal(any, window, cx, key);
    let at = probe_bounds(window, cx, key)
        .map(|b| b.center())
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} {step}: `{key}` の矩形が無い")));
    click_at(any, cx, at);
    notify_and_draw(any, window, cx);
}

/// 実行先の選択肢が出典タグに重ならない（右の選択肢の右端 ≤ タグの左端。#1734 の実画面で
/// 「新しいペイン」がタグの下へ潜った形を名指しで落とす）
fn check_no_overlap(window: WindowHandle<TakoApp>, cx: &mut AsyncApp, step: &str) {
    let choice = probe_bounds(window, cx, "run-menu-target-new");
    let tag = probe_bounds(window, cx, "run-menu-target-tag");
    let (Some(choice), Some(tag)) = (choice, tag) else {
        fail(&format!(
            "visual-test {LABEL} {step}: 実行先の選択肢か出典タグの矩形が無い"
        ));
    };
    check(
        choice.right() <= tag.left(),
        &format!(
            "visual-test {LABEL} {step}: 実行先の選択肢が出典タグに重ならない（選択肢の右端 {:?} / タグの左端 {:?}）",
            choice.right(),
            tag.left()
        ),
    );
}

/// 開いているタブのペインの数
fn pane_count(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> usize {
    window
        .update(cx, |app, _, _| {
            app.workspace.active_tab().tree().panes().len()
        })
        .unwrap_or(0)
}

/// メニューの完成品（描画が読むもの）
fn view_of(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) -> Option<crate::run_menu_ui::RunView> {
    window
        .update(cx, |app, _, _| app.run_menu.view(pane).cloned())
        .ok()
        .flatten()
}

fn read_store() -> Value {
    tako_control::run_configs::store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// `run-configs.json` の `<scope>s` に保存された `target` の値（どの鍵のものでも）
fn stored_targets(scope: &str) -> Vec<String> {
    read_store()[format!("{scope}s")]
        .as_object()
        .map(|m| {
            m.values()
                .filter_map(|c| c["target"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// メニューを開く（開いていなければシェブロンを実マウスで押し、答えが届くまで待つ）
async fn open_menu(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pv: PaneId,
    step: &str,
) {
    let open = window
        .update(cx, |app, _, _| app.run_menu.is_open(pv))
        .unwrap_or(false);
    if !open {
        click_probe(
            any,
            window,
            cx,
            &format!("preview-run-dd-{}", pv.as_u64()),
            step,
        );
    }
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(5), &|cx| {
        window
            .update(cx, |app, _, _| app.run_menu.is_open(pv))
            .unwrap_or(false)
    })
    .await;
    let _ = wait_run_view(any, window, cx, pv, Duration::from_secs(15)).await;
    notify_and_draw(any, window, cx);
}

fn close_menu(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) {
    let _ = window.update(cx, |app, _, cx| {
        app.close_run_menu();
        cx.notify();
    });
}

/// 再生ボタンを実マウスで 1 回押す（押した後に増えたペインへフォーカスが移っても、
/// 再生ボタンはプレビューのヘッダにある）
fn press_play(any: AnyWindowHandle, window: WindowHandle<TakoApp>, cx: &mut AsyncApp, pv: PaneId) {
    click_probe(
        any,
        window,
        cx,
        &format!("preview-run-{}", pv.as_u64()),
        "再生",
    );
    // 割った後のレイアウトでヘッダの矩形を描き直す
    notify_and_draw(any, window, cx);
}

pub(super) async fn run_target_1734_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let dir = std::env::temp_dir().join(format!("tako-st1734-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(dir.join(".git"));
    let dir = tako_core::platform::path::canonicalize(&dir).unwrap_or(dir);
    let a = dir.join("a.sh");
    let _ = std::fs::write(&a, "# tako:run: echo tako-1734\necho a\n");
    let legacy = tako_control::run_configs::legacy_1734();
    println!(
        "TAKO_VISUAL_1734: dir={} legacy={legacy} store={:?}",
        dir.display(),
        tako_control::run_configs::store_path()
    );

    // ① 開く → 実行先の行・既定は同じペイン
    let pv = window
        .update(cx, |app, _, cx| {
            app.drawer_visible = false;
            app.panel_visible = false;
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Welcome {
                    action: Some("dismiss".into()),
                },
                PaneOrigin::Cli,
            );
            let root = app.workspace.active_tab().tree().focused();
            let r = tako_control::dispatch(
                app,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(root.as_u64()),
                    path: a.display().to_string(),
                    mode: Some(tako_control::protocol::PreviewModeWire::Code),
                    direction: Some(tako_control::protocol::Direction::Right),
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            );
            app.drain_pending_highlights(cx);
            cx.notify();
            r.ok()
                .and_then(|v| v["pane"].as_u64())
                .map(PaneId::from_raw)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: a.sh を開けない")));
    let arrived = wait_run_view(any, window, cx, pv, Duration::from_secs(20)).await;
    check(
        arrived,
        &format!("visual-test {LABEL} ①: 一覧（RunResolve）が届く"),
    );
    open_menu(any, window, cx, pv, "①").await;
    reveal(any, window, cx, "run-menu-target-new");
    dump(any, window, cx, "open");
    let view = view_of(window, cx, pv).unwrap_or_default();
    check(
        probe_bounds(window, cx, "run-menu-target-reuse").is_some()
            && probe_bounds(window, cx, "run-menu-target-new").is_some(),
        &format!("visual-test {LABEL} ①: 実行先の行（同じペイン / 新しいペイン）が描かれる"),
    );
    check(
        view.run_target() == tako_core::runner_config::RunTarget::Reuse
            && view.target.source == "default",
        &format!(
            "visual-test {LABEL} ①: 既定は同じペイン・出典は既定（{:?} / {}）",
            view.target.value, view.target.source
        ),
    );

    // ⑥ 境界: 今と同じ選択肢を押しても書かない
    click_probe(any, window, cx, "run-menu-target-reuse", "⑥");
    let store_now = read_store();
    check(
        stored_targets("file").is_empty() && stored_targets("project").is_empty(),
        &format!("visual-test {LABEL} ⑥: 今と同じ選択肢は保存しない（{store_now}）"),
    );

    // ② 新しいペインを選ぶ → ファイルに保存
    click_probe(any, window, cx, "run-menu-target-new", "②");
    let _ = wait_run_view(any, window, cx, pv, Duration::from_secs(15)).await;
    reveal(any, window, cx, "run-menu-target-new");
    dump(any, window, cx, "new");
    check_no_overlap(window, cx, "②");
    let view = view_of(window, cx, pv).unwrap_or_default();
    let files = stored_targets("file");
    println!("TAKO_VISUAL_1734: ② files.target={files:?}");
    check(
        files == vec!["new".to_string()],
        &format!("visual-test {LABEL} ②: run-configs.json の files に target new（{files:?}）"),
    );
    check(
        view.run_target() == tako_core::runner_config::RunTarget::New
            && view.target.source == "file",
        &format!(
            "visual-test {LABEL} ②: メニューの実行先が新しいペイン・出典はファイル（{:?} / {}）",
            view.target.value, view.target.source
        ),
    );
    close_menu(window, cx);

    // ③ 再生ボタンを実マウスで 2 回 → 2 枚増える
    let before = pane_count(window, cx);
    press_play(any, window, cx, pv);
    press_play(any, window, cx, pv);
    let after = pane_count(window, cx);
    println!("TAKO_VISUAL_1734: ③ panes {before} -> {after}");
    dump(any, window, cx, "two");
    check(
        after == before + 2,
        &format!(
            "visual-test {LABEL} ③: target=new で再生ボタン 2 回 = ペインが 2 枚増える（{before} → {after}）"
        ),
    );

    // ④ 同じペインを選ぶ → 2 回押しても増えない
    open_menu(any, window, cx, pv, "④").await;
    click_probe(any, window, cx, "run-menu-target-reuse", "④");
    let _ = wait_run_view(any, window, cx, pv, Duration::from_secs(15)).await;
    let files = stored_targets("file");
    check(
        files == vec!["reuse".to_string()],
        &format!(
            "visual-test {LABEL} ④: 保存先は今の出典（ファイル）のまま target reuse（{files:?}）"
        ),
    );
    close_menu(window, cx);
    let before = pane_count(window, cx);
    press_play(any, window, cx, pv);
    press_play(any, window, cx, pv);
    let after = pane_count(window, cx);
    println!("TAKO_VISUAL_1734: ④ panes {before} -> {after}");
    open_menu(any, window, cx, pv, "④").await;
    reveal(any, window, cx, "run-menu-target-reuse");
    dump(any, window, cx, "reuse");
    check_no_overlap(window, cx, "④");
    check(
        after == before,
        &format!(
            "visual-test {LABEL} ④: target=reuse で再生ボタン 2 回 = ペインは増えない（{before} → {after}）"
        ),
    );

    // ⑤ 出典タグ → プロジェクトへ移る → × で既定へ
    click_probe(any, window, cx, "run-menu-target-tag", "⑤");
    let _ = wait_run_view(any, window, cx, pv, Duration::from_secs(15)).await;
    let (files, projects) = (stored_targets("file"), stored_targets("project"));
    check(
        files.is_empty() && projects == vec!["reuse".to_string()],
        &format!(
            "visual-test {LABEL} ⑤: 出典タグで保存先がプロジェクトへ移る（files {files:?} / projects {projects:?}）"
        ),
    );
    click_probe(any, window, cx, "run-menu-target-reset", "⑤");
    let _ = wait_run_view(any, window, cx, pv, Duration::from_secs(15)).await;
    reveal(any, window, cx, "run-menu-target-reuse");
    dump(any, window, cx, "reset");
    let view = view_of(window, cx, pv).unwrap_or_default();
    let (files, projects) = (stored_targets("file"), stored_targets("project"));
    check(
        files.is_empty() && projects.is_empty() && view.target.source == "default",
        &format!(
            "visual-test {LABEL} ⑤: × で既定へ戻る（files {files:?} / projects {projects:?} / 出典 {}）",
            view.target.source
        ),
    );
    close_menu(window, cx);
    let _ = std::fs::remove_dir_all(&dir);
}
