//! visual-test `run-dropdown-1733`（FR-3.18.7 / Issue #1733。エピック #1726 の S5）: 実行ボタン横の
//! ドロップダウンを**実マウスで**開き、`RunResolve` の答えどおりに描かれ、行の操作が dispatch
//! `RunnerDefaults` を通って `run-configs.json` と次の `Run` に効くことを確かめる。
//!
//! 場面: 一時 dir（宣言 2 つの `a.py` + 偽の `.venv` + 偽の別の venv `alt/` + 宣言の無い `b.py`）。
//!
//! - ① `a.py` を開く → 一覧（background の `RunResolve`）が届き（メニューを開いて画像 `list`）、**別に取った `RunResolve` と字面一致**
//!   （プロファイルの名前とコマンド・候補の ID と自動 / 選択の印・項目の出典）。走らせるコマンドは
//!   `.venv` の python で、**dispatch `Run` の応答の `command` と一致**（A/B: `TAKO_1733_LEGACY=1` は一覧が
//!   `python3 a.py` のままなのでここで名指しの FAILED）
//! - ② ヘッダのシェブロンを実マウスで押す → メニューが開く・候補の行が描かれる（画像 `open`）
//! - ③ 候補 `alt` の行を押す → `run-configs.json` の projects に `alt` が入り、次の `Run` が `alt` の
//!   python で走る（画像 `runtime`）
//! - ④ 引数の行を押して欄を開き、実キーで `--verbose x` を打つ（画像 `edit`）→ Enter で保存（file）
//! - ⑤ 境界: 実行前コマンドの欄で空のまま Enter → 保存されず理由が出る / 環境変数に `1BAD=x` →
//!   dispatch が断った理由が欄の下に出て保存されない（画像 `env-invalid`）→ `APP_ENV=dev` は入る
//! - ⑥ 引数の出典タグを押す → 保存先が project へ移る（画像 `tag`）→ もう一度押すと file へ戻る
//! - ⑦ 次の `Run` の `command` が一覧の走らせるコマンドと一致し、引数が末尾に付く
//! - ⑧ 境界: 宣言の無い `b.py`（プロファイル 1 つ）はプロファイルの節を出さない（画像 `single`）/
//!   実行環境の候補 0 の答えを注入すると「候補が見つかりません」を出す（画像 `no-candidates`）
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` の下へ `run-dropdown-1733-<節>.png` で落とす。
//! 単独実行は `TAKO_VISUAL_ONLY=run-dropdown-1733`

use super::*;
use serde_json::Value;

const LABEL: &str = "run-dropdown-1733";

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

/// 偽の venv（`pyvenv.cfg` の版 + 実行できる `bin/python`）。中身は走った印を出すだけ
fn fake_venv(dir: &std::path::Path, version: &str) {
    let bin = dir.join("bin");
    let _ = std::fs::create_dir_all(&bin);
    let _ = std::fs::write(
        dir.join("pyvenv.cfg"),
        format!("home = /usr/bin\nversion = {version}\n"),
    );
    let python = bin.join("python");
    let _ = std::fs::write(
        &python,
        format!("#!/bin/sh\necho \"fake-python-{version} $*\"\n"),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755));
    }
}

fn probe_center(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
) -> Option<Point<Pixels>> {
    window
        .update(cx, |app, _, _| {
            app.panel_click_probe_bounds
                .borrow()
                .get(key)
                .map(|b| b.center())
        })
        .ok()
        .flatten()
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

/// メニューの中の行を、メニュー本体を実ホイールで送って見える位置まで出す（メニューは窓の高さで
/// 打ち切ってスクロールする。見えていない行は押せない）。メニューの外の要素は何もしない
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
        // 枠の余白（6px）の内側に収まっていれば見えている
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

/// 実マウスで押す（矩形が無ければ名指しで落とす）。メニューの中の行は先に見える位置まで送る
fn click_probe(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    key: &str,
    step: &str,
) {
    notify_and_draw(any, window, cx);
    reveal(any, window, cx, key);
    let at = probe_center(window, cx, key)
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} {step}: `{key}` の矩形が無い")));
    click_at(any, cx, at);
    notify_and_draw(any, window, cx);
}

/// 別に取った `RunResolve`（同期の dispatch = CLI `tako run <f> --list` と同じ答え）
fn resolve_now(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    path: &std::path::Path,
) -> Value {
    window
        .update(cx, |app, _, _| {
            tako_control::dispatch(
                app,
                tako_control::protocol::Request::RunResolve {
                    path: path.display().to_string(),
                    pane: Some(pane.as_u64()),
                    refresh: false,
                    boards: None,
                },
                PaneOrigin::Cli,
            )
        })
        .ok()
        .and_then(|r| {
            r.map_err(|e| println!("TAKO_VISUAL_1733: RunResolve が断られた: {e}"))
                .ok()
        })
        .unwrap_or(Value::Null)
}

/// 次の `Run`（CLI `tako run <f>` と同じ口）の応答
fn run_now(
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
    path: &std::path::Path,
) -> Value {
    window
        .update(cx, |app, _, cx| {
            let r = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Run {
                    path: path.display().to_string(),
                    pane: Some(pane.as_u64()),
                    tab: None,
                    profile: None,
                    command: None,
                    direction: None,
                    ratio: None,
                    auto_close: None,
                    focus: Some(false),
                    new_pane: None,
                },
                PaneOrigin::Cli,
            );
            for (p, options) in std::mem::take(&mut app.pending_attach) {
                if app.spawn_session(p, options, cx).is_err() {
                    app.remove_pane(p, cx);
                }
            }
            r
        })
        .ok()
        .and_then(|r| {
            r.map_err(|e| println!("TAKO_VISUAL_1733: Run が断られた: {e}"))
                .ok()
        })
        .unwrap_or(Value::Null)
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

/// 一覧の取り直し（書き込みの後の background の `RunResolve`）が届くまで待つ
async fn settle(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: PaneId,
) {
    let _ = wait_run_view(any, window, cx, pane, Duration::from_secs(15)).await;
}

/// メニューの完成品が `RunResolve` の答えと字面一致するか（違えば何が違うかを返す）
fn diff_with_resolve(view: &crate::run_menu_ui::RunView, v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let profiles: Vec<(String, String)> = v["profiles"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    (
                        p["profile"].as_str().unwrap_or_default().to_string(),
                        p["command"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let shown: Vec<(String, String)> = view
        .profiles
        .iter()
        .map(|p| (p.profile.clone(), p.command.clone()))
        .collect();
    if profiles != shown {
        out.push(format!("profiles: 応答 {profiles:?} / メニュー {shown:?}"));
    }
    let runtimes: Vec<(String, bool, bool)> = v["runtimes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    (
                        r["id"].as_str().unwrap_or_default().to_string(),
                        r["auto"].as_bool().unwrap_or(false),
                        r["selected"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let shown: Vec<(String, bool, bool)> = view
        .runtimes
        .iter()
        .map(|r| (r.id.clone(), r.auto, r.selected))
        .collect();
    if runtimes != shown {
        out.push(format!("runtimes: 応答 {runtimes:?} / メニュー {shown:?}"));
    }
    for (field, shown) in [
        ("args", &view.args),
        ("cwd", &view.cwd),
        ("before", &view.before),
        ("profile", &view.profile),
    ] {
        let src = v["config"][field]["source"].as_str().unwrap_or_default();
        let val = v["config"][field]["value"].as_str().map(str::to_string);
        if src != shown.source || val != shown.value {
            out.push(format!(
                "config.{field}: 応答 ({val:?}, {src}) / メニュー ({:?}, {})",
                shown.value, shown.source
            ));
        }
    }
    let kind = view.runtime_kind.clone().unwrap_or_default();
    let src = v["config"]["runtime"][&kind]["source"]
        .as_str()
        .unwrap_or("auto");
    if src != view.runtime_source {
        out.push(format!(
            "config.runtime.{kind}.source: 応答 {src} / メニュー {}",
            view.runtime_source
        ));
    }
    out
}

fn read_store() -> Value {
    tako_control::run_configs::store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// `run-configs.json` の `<scope>s` の中で、どれかの鍵の `field` を返す（鍵は正規化済みの絶対パス）
fn stored(store: &Value, scope: &str, field: &str) -> Vec<Value> {
    store[format!("{scope}s")]
        .as_object()
        .map(|m| {
            m.values()
                .map(|c| c[field].clone())
                .filter(|v| !v.is_null())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) async fn run_dropdown_1733_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    // 材料（短いパス。`TAKO_DATA_DIR` の外 = 一時 dir）
    let dir = std::env::temp_dir().join(format!("tako-st1733-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let dir = tako_core::platform::path::canonicalize(&dir).unwrap_or(dir);
    fake_venv(&dir.join(".venv"), "3.12.4");
    fake_venv(&dir.join("alt"), "3.11.9");
    let a = dir.join("a.py");
    let _ = std::fs::write(
        &a,
        "# tako:run: ${python} ${fileBase}\n# tako:run[test]: ${python} -m pytest ${fileBase}\nprint(\"a\")\n",
    );
    let b = dir.join("b.py");
    let _ = std::fs::write(&b, "print(\"b\")\n");
    let legacy = crate::run_menu_ui::legacy_1733();
    println!(
        "TAKO_VISUAL_1733: dir={} legacy={legacy} store={:?}",
        dir.display(),
        tako_control::run_configs::store_path()
    );

    // ① 開く → 一覧が届く → 別に取った RunResolve と字面一致 / Run のコマンドと一致
    let pv = window
        .update(cx, |app, _, cx| {
            app.drawer_visible = false;
            app.panel_visible = false;
            // 初回起動のバナーを閉じて縦の場所を空ける（メニューは窓の高さで打ち切る）
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
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL} ①: a.py を開けない")));
    let arrived = wait_run_view(any, window, cx, pv, Duration::from_secs(20)).await;
    check(
        arrived,
        &format!("visual-test {LABEL} ①: 一覧（RunResolve）が届く"),
    );
    let view = view_of(window, cx, pv).unwrap_or_default();
    let error = window
        .update(cx, |app, _, _| app.run_menu.error(pv).map(str::to_string))
        .ok()
        .flatten();
    if let Some(e) = &error {
        println!("TAKO_VISUAL_1733: ① 一覧の取得がエラー: {e}");
    }
    let fresh = resolve_now(window, cx, pv, &a);
    let menu_cmd = view
        .selected_row()
        .map(|r| r.command.clone())
        .unwrap_or_default();
    let ran = run_now(window, cx, pv, &a);
    let run_cmd = ran["command"].as_str().unwrap_or_default().to_string();
    println!(
        "TAKO_VISUAL_1733: ① menu_cmd={menu_cmd:?} run_cmd={run_cmd:?} runtime={:?} profiles={}",
        ran["runtime"]["id"],
        view.profiles.len()
    );
    // 判定の前にメニューを開いて撮る（A/B の旧の腕もここで 1 枚残る = 一覧のコマンドを画像で見比べる）
    notify_and_draw(any, window, cx);
    click_probe(
        any,
        window,
        cx,
        &format!("preview-run-dd-{}", pv.as_u64()),
        "①",
    );
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(5), &|cx| {
        window
            .update(cx, |app, _, _| app.run_menu.is_open(pv))
            .unwrap_or(false)
    })
    .await;
    dump(any, window, cx, "list");
    let _ = window.update(cx, |app, _, cx| {
        app.close_run_menu();
        cx.notify();
    });
    check(
        !run_cmd.is_empty() && menu_cmd == run_cmd,
        &format!(
            "visual-test {LABEL} ①: 一覧の走らせるコマンドと Run のコマンドが一致する（一覧 {menu_cmd:?} / Run {run_cmd:?}）"
        ),
    );
    check(
        menu_cmd.contains(".venv"),
        &format!("visual-test {LABEL} ①: 設定なしで .venv の python（{menu_cmd:?}）"),
    );
    let diffs = diff_with_resolve(&view, &fresh);
    check(
        diffs.is_empty(),
        &format!("visual-test {LABEL} ①: メニューの状態が RunResolve と字面一致: {diffs:?}"),
    );
    check(
        view.profiles.len() == 2
            && view
                .runtimes
                .iter()
                .any(|r| r.id.ends_with("/.venv") && r.auto && r.selected)
            && view.runtimes.iter().any(|r| r.id.ends_with("/alt")),
        &format!(
            "visual-test {LABEL} ①: 宣言 2 つ・自動の .venv・候補 alt（{:?}）",
            view.runtimes.iter().map(|r| &r.id).collect::<Vec<_>>()
        ),
    );

    // ② シェブロンを実マウスで押す → 開く
    // 実行ペインが増えたのでレイアウトが落ち着くまで 1 フレーム描く
    notify_and_draw(any, window, cx);
    click_probe(
        any,
        window,
        cx,
        &format!("preview-run-dd-{}", pv.as_u64()),
        "②",
    );
    let opened = vt1860_wait(any, window, cx, Duration::from_secs(5), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.run_menu.is_open(pv)
                    && app
                        .panel_click_probe_bounds
                        .borrow()
                        .contains_key("run-menu-runtime-1")
            })
            .unwrap_or(false)
    })
    .await;
    dump(any, window, cx, "open");
    check(
        opened,
        &format!("visual-test {LABEL} ②: シェブロンでメニューが開き候補が描かれる"),
    );

    // ③ 候補 alt を押す → run-configs.json と次の Run
    let alt_index = view
        .runtimes
        .iter()
        .position(|r| r.id.ends_with("/alt"))
        .unwrap_or(usize::MAX);
    click_probe(
        any,
        window,
        cx,
        &format!("run-menu-runtime-{alt_index}"),
        "③",
    );
    settle(any, window, cx, pv).await;
    let view = view_of(window, cx, pv).unwrap_or_default();
    let store = read_store();
    let saved_runtime = stored(&store, "project", "runtime");
    println!(
        "TAKO_VISUAL_1733: ③ runtime={:?} source={} store.projects.runtime={saved_runtime:?}",
        view.runtime.as_ref().map(|r| &r.id),
        view.runtime_source
    );
    check(
        view.runtime
            .as_ref()
            .is_some_and(|r| r.id.ends_with("/alt"))
            && view.runtime_source == "project",
        &format!("visual-test {LABEL} ③: 選んだ alt が project に保存されメニューに出る"),
    );
    check(
        saved_runtime.iter().any(|r| {
            r["python"]["key"]
                .as_str()
                .is_some_and(|k| k.ends_with("alt"))
        }),
        &format!("visual-test {LABEL} ③: run-configs.json の projects に alt（{saved_runtime:?}）"),
    );
    dump(any, window, cx, "runtime");
    let ran = run_now(window, cx, pv, &a);
    check(
        ran["runtime"]["id"]
            .as_str()
            .is_some_and(|id| id.ends_with("/alt"))
            && ran["command"].as_str().is_some_and(|c| c.contains("/alt/")),
        &format!(
            "visual-test {LABEL} ③: 次の Run が alt で走る（{:?} / {:?}）",
            ran["runtime"]["id"], ran["command"]
        ),
    );

    // ④ 引数の行を押して欄を開き、実キーで打つ → Enter で保存（file）
    let reopen = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.run_menu.is_open(pv))
            .unwrap_or(false)
    };
    if !reopen(cx) {
        click_probe(
            any,
            window,
            cx,
            &format!("preview-run-dd-{}", pv.as_u64()),
            "④",
        );
    }
    click_probe(any, window, cx, "run-menu-args-value", "④");
    let active = window
        .update(cx, |app, _, _| {
            app.run_menu_input_active() && app.app_text_input() == Some(AppTextInput::RunMenu)
        })
        .unwrap_or(false);
    check(
        active,
        &format!("visual-test {LABEL} ④: 欄が開き、打鍵と変換の宛先が欄になる"),
    );
    type_text(any, cx, "--verbose x", false);
    dump(any, window, cx, "edit");
    let typed = window
        .update(cx, |app, _, _| {
            app.run_menu
                .edit
                .as_ref()
                .map(|e| e.field.text().to_string())
        })
        .ok()
        .flatten();
    check(
        typed.as_deref() == Some("--verbose x"),
        &format!("visual-test {LABEL} ④: 実キーが欄に入る（{typed:?}）"),
    );
    press(any, cx, "enter");
    settle(any, window, cx, pv).await;
    let view = view_of(window, cx, pv).unwrap_or_default();
    check(
        view.args.value.as_deref() == Some("--verbose x") && view.args.source == "file",
        &format!(
            "visual-test {LABEL} ④: Enter で引数が file に保存される（{:?}）",
            view.args
        ),
    );

    // ⑤ 境界: 空の Enter は保存しない / 不正な環境変数名は dispatch の理由が出る
    click_probe(any, window, cx, "run-menu-before-value", "⑤");
    press(any, cx, "enter");
    notify_and_draw(any, window, cx);
    let (notice, still_open) = window
        .update(cx, |app, _, _| {
            (
                app.run_menu.edit.as_ref().and_then(|e| e.notice.clone()),
                app.run_menu.edit.is_some(),
            )
        })
        .unwrap_or_default();
    let before_saved = stored(&read_store(), "file", "before");
    println!(
        "TAKO_VISUAL_1733: ⑤ empty notice={notice:?} open={still_open} before={before_saved:?}"
    );
    check(
        still_open && notice.is_some() && before_saved.is_empty(),
        &format!("visual-test {LABEL} ⑤: 空の Enter は保存されず理由が出る（{notice:?}）"),
    );
    press(any, cx, "escape");
    click_probe(any, window, cx, "run-menu-env-add", "⑤");
    type_text(any, cx, "1BAD=x", true);
    notify_and_draw(any, window, cx);
    let notice = window
        .update(cx, |app, _, _| {
            app.run_menu.edit.as_ref().and_then(|e| e.notice.clone())
        })
        .ok()
        .flatten();
    // 理由の行を見える位置まで送ってから撮る
    reveal(any, window, cx, "run-menu-input-notice");
    dump(any, window, cx, "env-invalid");
    let env_saved = stored(&read_store(), "file", "env");
    println!("TAKO_VISUAL_1733: ⑤ bad env notice={notice:?} env={env_saved:?}");
    check(
        notice.as_deref().is_some_and(|n| n.contains("1BAD"))
            && !env_saved.iter().any(|e| e.get("1BAD").is_some()),
        &format!(
            "visual-test {LABEL} ⑤: 不正な名前は保存されず dispatch の理由が出る（{notice:?}）"
        ),
    );
    // 直して打ち直す（欄は開いたまま）
    let _ = window.update(cx, |app, _, cx| {
        if let Some(e) = app.run_menu.edit.as_mut() {
            e.field.clear();
        }
        cx.notify();
    });
    type_text(any, cx, "APP_ENV=dev", true);
    settle(any, window, cx, pv).await;
    let view = view_of(window, cx, pv).unwrap_or_default();
    check(
        view.env.iter().any(|(k, s)| {
            k == "APP_ENV" && s.value.as_deref() == Some("dev") && s.source == "file"
        }),
        &format!(
            "visual-test {LABEL} ⑤: APP_ENV=dev は file に入る（{:?}）",
            view.env
        ),
    );

    // ⑥ 出典タグで保存先を移す（file → project → file）
    click_probe(any, window, cx, "run-menu-args-tag", "⑥");
    settle(any, window, cx, pv).await;
    let moved = view_of(window, cx, pv).unwrap_or_default();
    let store = read_store();
    println!(
        "TAKO_VISUAL_1733: ⑥ args={:?} project.args={:?} file.args={:?}",
        moved.args,
        stored(&store, "project", "args"),
        stored(&store, "file", "args")
    );
    check(
        moved.args.source == "project"
            && moved.args.value.as_deref() == Some("--verbose x")
            && stored(&store, "project", "args").contains(&Value::from("--verbose x"))
            && stored(&store, "file", "args").is_empty(),
        &format!(
            "visual-test {LABEL} ⑥: 出典タグで引数が project へ移る（{:?}）",
            moved.args
        ),
    );
    dump(any, window, cx, "tag");
    click_probe(any, window, cx, "run-menu-args-tag", "⑥");
    settle(any, window, cx, pv).await;
    let back = view_of(window, cx, pv).unwrap_or_default();
    check(
        back.args.source == "file",
        &format!(
            "visual-test {LABEL} ⑥: もう一度押すと file へ戻る（{:?}）",
            back.args
        ),
    );

    // ⑦ 次の Run が一覧の走らせるコマンドと一致し、引数が付く
    let fresh = resolve_now(window, cx, pv, &a);
    let diffs = diff_with_resolve(&back, &fresh);
    check(
        diffs.is_empty(),
        &format!("visual-test {LABEL} ⑦: 書き込み後もメニューが RunResolve と字面一致: {diffs:?}"),
    );
    let menu_cmd = back
        .selected_row()
        .map(|r| r.command.clone())
        .unwrap_or_default();
    let ran = run_now(window, cx, pv, &a);
    let run_cmd = ran["command"].as_str().unwrap_or_default().to_string();
    println!("TAKO_VISUAL_1733: ⑦ menu_cmd={menu_cmd:?} run_cmd={run_cmd:?}");
    check(
        menu_cmd == run_cmd && run_cmd.ends_with("--verbose x") && run_cmd.contains("/alt/"),
        &format!(
            "visual-test {LABEL} ⑦: 一覧と Run が一致し引数が付く（{menu_cmd:?} / {run_cmd:?}）"
        ),
    );

    // ⑧ 境界: プロファイル 1 つ / 実行環境の候補 0
    let _ = window.update(cx, |app, _, cx| {
        app.close_run_menu();
        let _ = tako_control::dispatch(
            app,
            tako_control::protocol::Request::OpenFile {
                pane: Some(pv.as_u64()),
                path: b.display().to_string(),
                mode: Some(tako_control::protocol::PreviewModeWire::Code),
                direction: None,
                focus: Some(true),
                new_tab: false,
                line: None,
                column: None,
            },
            PaneOrigin::Cli,
        );
        app.drain_pending_highlights(cx);
        cx.notify();
    });
    let arrived = vt1860_wait(any, window, cx, Duration::from_secs(20), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.run_menu
                    .resolved
                    .get(&pv)
                    .is_some_and(|r| r.path.ends_with("b.py") && !r.loading && r.answer.is_some())
            })
            .unwrap_or(false)
    })
    .await;
    let single = view_of(window, cx, pv).unwrap_or_default();
    check(
        arrived && single.profiles.len() == 1 && single.runtime_kind.is_some(),
        &format!(
            "visual-test {LABEL} ⑧: 宣言の無い b.py はプロファイル 1 つ・実行環境の節あり（{}）",
            single.profiles.len()
        ),
    );
    click_probe(
        any,
        window,
        cx,
        &format!("preview-run-dd-{}", pv.as_u64()),
        "⑧",
    );
    let _ = vt1860_wait(any, window, cx, Duration::from_secs(5), &|cx| {
        window
            .update(cx, |app, _, _| {
                app.run_menu.is_open(pv) && !app.run_menu.loading(pv)
            })
            .unwrap_or(false)
    })
    .await;
    dump(any, window, cx, "single");
    // 候補 0 の答えを注入する（描画は完成品を読むだけなので、答えの形だけで決まる）
    let _ = window.update(cx, |app, _, cx| {
        if let Some(r) = app.run_menu.resolved.get_mut(&pv) {
            if let Some(Ok(v)) = r.answer.as_mut() {
                v.runtimes.clear();
                v.runtime = None;
            }
        }
        cx.notify();
    });
    dump(any, window, cx, "no-candidates");
    // 描画が落ちずにメニューが開いたまま（中身の「候補が見つかりません」は画像 `no-candidates` で見る）
    let empty_ok = window
        .update(cx, |app, _, _| {
            app.run_menu.is_open(pv) && app.run_menu.view(pv).is_some_and(|v| v.runtimes.is_empty())
        })
        .unwrap_or(false);
    println!(
        "TAKO_VISUAL_1733: ⑧ single profiles={} no-candidates={empty_ok}",
        single.profiles.len()
    );
    check(
        empty_ok,
        &format!("visual-test {LABEL} ⑧: 候補 0 の答えでもメニューが描ける"),
    );

    let _ = window.update(cx, |app, _, cx| {
        app.close_run_menu();
        cx.notify();
    });
    let _ = std::fs::remove_dir_all(&dir);
    println!("TAKO_VISUAL_1733: all_ok=true");
}
