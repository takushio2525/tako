//! visual-test `completion-cancel`（FR-3.35 / Issue #1909）: 補完の打鍵の要求の取り消しの番号を
//! **UI スレッドで先に取る**ことを、実 GUI の打鍵経路で確かめる（偽サーバの `loading`）。
//!
//! 競合: GUI は打鍵の要求を背景へ渡す。背景が走り出す前に語の外へ出る（空白 = 一覧と「読み込み中」を
//! 閉じて `cancel_completion`）と、番号を背景で取る形（#1909 の前）は取り消しを追い越して自分が最新に
//! なり、読み込みが済むまで待ち続ける（サーバが答えずに待たせる読み込みの後半なら待ちの表にも残る）。
//! 普段は隙間が数マイクロ秒なので、背景の走り出しを合図のファイルまで止める注入
//! （`TAKO_1909_INJECT_HOLD`）で、その間に取り消しが入る順序を作る（実時間の遅れに任せない）。
//!
//! 相: ①読み込み中に 1 文字打つ → 背景が列へ入って合図を待つ（`inflight` = 1）②空白を打つ →
//! 「読み込み中」が消える ③合図で背景が走り出す → **サーバへ問い合わせずに抜け、`inflight` /
//! `pending_requests` が 0**（旧い形はここでサーバへ届き、待ちの表に残る）④閉じた直後にもう一度打つ →
//! 新しい要求は取り消されず、読み込みが済むと一覧が出る。終わったら `inflight` / `pending_requests` は 0。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1909_LEGACY=1`（GUI が番号を背景で取る = #1909 の前）は
//! ③で名指しの FAILED になる = 同一バイナリでの A/B。単独実行は `TAKO_VISUAL_ONLY=completion-cancel`

use super::*;

/// 条件が立つまで描き直しながら待つ（上限つき）
async fn until(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    limit: Duration,
    done: &dyn Fn(&mut AsyncApp) -> bool,
) -> bool {
    vt1860_wait(any, window, cx, limit, done).await
}

/// manager の観測口（`tako lsp status` と同じ JSON）: (`inflight.completion`, `pending_requests`)
fn lanes(window: WindowHandle<TakoApp>, cx: &mut AsyncApp) -> (u64, u64) {
    window
        .update(cx, |app, _, _| {
            let status = app.lsp.status(None);
            (
                status["inflight"]["completion"]
                    .as_u64()
                    .unwrap_or(u64::MAX),
                status["servers"][0]["pending_requests"]
                    .as_u64()
                    .unwrap_or(u64::MAX),
            )
        })
        .unwrap_or((u64::MAX, u64::MAX))
}

/// 偽サーバのログ（`TAKO_LSP_FAKE_LOG`）に届いた補完の問い合わせの数
fn asked(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m["method"] == "textDocument/completion")
        .count()
}

/// 偽サーバへ届いた**要求**（id 付き）のメソッドの数（診断用。待ちの表に何が残っているかを見分ける）
fn requests(log: &std::path::Path) -> std::collections::BTreeMap<String, usize> {
    let mut out = std::collections::BTreeMap::new();
    for m in std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|m| m.get("id").is_some())
    {
        if let Some(method) = m["method"].as_str() {
            *out.entry(method.to_string()).or_insert(0) += 1;
        }
    }
    out
}

pub(super) async fn completion_cancel_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    const LABEL: &str = "completion-cancel";
    inject_section_failure(LABEL);
    let work = std::env::temp_dir().join(format!("tako-visual-1909-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("visual-test completion-cancel の置き場");
    let (log, gate, hold) = (
        work.join("received.jsonl"),
        work.join("loading-done"),
        work.join("hold"),
    );
    // 偽サーバは起動のたびに env を読む（scene が編集モードへ入った時点で起きる）。読み込みは合図
    // （`gate`）まで終わらない = 読み込みの終わりを実時間に任せない（#1922）。読み込み中の補完は
    // 答えずに待たせる（rust-analyzer の後半）= 取り消しを追い越した要求は待ちの表に残る
    std::env::set_var("TAKO_LSP_FAKE_SCENARIO", "loading");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_MS", "0");
    std::env::set_var("TAKO_LSP_FAKE_LOADING_UNTIL", &gate);
    std::env::set_var("TAKO_LSP_FAKE_LOG", &log);
    let source = "fn main() {\n    let total = 1;\n    \n}\n";
    let rules = serde_json::json!({
        "items": [{ "label": "alpha" }, { "label": "beta" }],
        "hold_while_loading": true,
    });
    let (pane, dir, override_env) =
        completion_scene(any, window, cx, LABEL, Some(rules), source, (3, 4)).await;
    let path = window
        .update(cx, |app, _, _| {
            app.preview_edits
                .get(&pane)
                .map(|e| e.buffer.path().to_path_buf())
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {LABEL}: 編集バッファが無い")));
    let loading = move |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.lsp.server_loading(&path))
            .unwrap_or(false)
    };
    let note = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| app.lsp_completion.loading.is_some())
            .unwrap_or(false)
    };
    let legacy = tako_control::lsp::completion::legacy_1909();
    // 起動が済んで（握手の `initialize` が待ちの表から外れて）から読み込み中になったのを待つ。
    // `server_loading` は起動中も真なので、それだけで進むと ③ の待ちの表に `initialize` が残って見える
    let running_loading = |cx: &mut AsyncApp| {
        window
            .update(cx, |app, _, _| {
                let server = &app.lsp.status(None)["servers"][0];
                server["state"] == "running" && server["loading"] == true
            })
            .unwrap_or(false)
    };
    check(
        until(any, window, cx, Duration::from_secs(30), &|cx| {
            running_loading(cx) && loading(cx)
        })
        .await,
        &format!("visual-test {LABEL}: 偽サーバが起動して読み込み中と知らせる（素材の前提）"),
    );

    // ① 背景の走り出しを合図まで止めて 1 文字打つ → 背景が列へ入って止まる
    std::env::set_var("TAKO_1909_INJECT_HOLD", &hold);
    completion_type(any, cx, 'a');
    let entered = until(any, window, cx, Duration::from_secs(10), &|cx| {
        lanes(window, cx).0 == 1
    })
    .await;
    let noted = note(cx);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ① legacy={legacy} entered={entered} note={noted} lanes={:?} asked={} requests={:?}",
        lanes(window, cx),
        asked(&log),
        requests(&log)
    );
    check(
        entered && noted,
        &format!("visual-test {LABEL} ①: 読み込み中に打つと背景が列へ入り、「読み込み中」が出る（素材の前提）"),
    );

    // ② 背景が走り出す前に語の外へ出る（空白）→ 一覧と「読み込み中」を閉じて取り消す
    completion_type(any, cx, ' ');
    let closed = until(any, window, cx, Duration::from_secs(3), &|cx| !note(cx)).await;
    check(
        closed,
        &format!("visual-test {LABEL} ②: 空白で「読み込み中」が消える（取り消しが出た）"),
    );

    // ③ 合図で背景が走り出す → 問い合わせずに抜ける（先に取った番号は古い）。旧い形は取り消しを
    //    追い越してサーバへ届き、読み込み待ちに残る = どちらかの状態が立つまで待つ（実時間は測らない）
    std::fs::write(&hold, b"").expect("合図のファイル");
    let settled = until(any, window, cx, Duration::from_secs(30), &|cx| {
        lanes(window, cx).0 == 0 || asked(&log) > 0
    })
    .await;
    // サーバへ届いていれば待ちの表へ載ったのを見てから判定する（届いた = 送った後）
    if asked(&log) > 0 {
        let _ = until(any, window, cx, Duration::from_secs(10), &|cx| {
            lanes(window, cx).1 > 0
        })
        .await;
    }
    let (inflight, pending) = lanes(window, cx);
    let leaked = asked(&log);
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ③ settled={settled} inflight={inflight} pending_requests={pending} asked={leaked} loading={} requests={:?}",
        loading(cx),
        requests(&log)
    );
    check(
        settled && leaked == 0 && inflight == 0 && pending == 0,
        &format!(
            "visual-test {LABEL} ③: 閉じた後に遅れて走り出した背景は問い合わせずに抜け、列にも待ちの表にも残らない (#1909。inflight={inflight} pending_requests={pending} 問い合わせ={leaked})"
        ),
    );

    // ④ 閉じた直後にもう一度打つ → 新しい要求は取り消されず、読み込みが済むと一覧が出る
    completion_type(any, cx, 'a');
    let asked_again = until(any, window, cx, Duration::from_secs(10), &|_| {
        asked(&log) > leaked
    })
    .await;
    std::fs::write(&gate, b"").expect("読み込みの合図");
    let listed = until(any, window, cx, Duration::from_secs(30), &|cx| {
        window
            .update(cx, |app, _, _| app.lsp_completion.popup.is_some())
            .unwrap_or(false)
    })
    .await;
    let drained = until(any, window, cx, Duration::from_secs(10), &|cx| {
        lanes(window, cx) == (0, 0)
    })
    .await;
    let items = window
        .update(cx, |app, _, _| {
            app.lsp_completion.popup.as_ref().map(|p| p.order.len())
        })
        .ok()
        .flatten();
    println!(
        "TAKO_VISUAL_PIXEL: {LABEL} ④ asked_again={asked_again} listed={listed} items={items:?} drained={drained} lanes={:?}",
        lanes(window, cx)
    );
    check(
        asked_again && listed && items == Some(2),
        &format!("visual-test {LABEL} ④: 閉じた直後の打鍵は取り消されず、読み込みが済むと一覧が出る ({items:?})"),
    );
    check(
        drained,
        &format!("visual-test {LABEL} ④: 一覧が出た後は inflight / pending_requests が 0 に戻る"),
    );
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        if let Some((frame, _)) = capture_frame(any, cx) {
            let _ = frame.save(std::path::Path::new(&dump).join("completion-cancel.png"));
        }
    }
    press(any, cx, "escape");

    for name in [
        "TAKO_1909_INJECT_HOLD",
        "TAKO_LSP_FAKE_SCENARIO",
        "TAKO_LSP_FAKE_LOADING_MS",
        "TAKO_LSP_FAKE_LOADING_UNTIL",
        "TAKO_LSP_FAKE_LOG",
        "TAKO_LSP_FAKE_COMPLETION",
    ] {
        std::env::remove_var(name);
    }
    if let Some(name) = override_env {
        std::env::remove_var(name);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&work);
}
