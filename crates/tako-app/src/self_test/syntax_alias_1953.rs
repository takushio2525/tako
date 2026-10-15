//! visual-test `syntax-alias`（FR-3.2 / Issue #1953）: 既存の構文への別名で色の穴を埋めた形式が、
//! 実際に開くとその構文で塗られ、応答の `syntax` で読めるか。
//!
//! 場面: 一時 dir の 7 形式（`.h` の C++ / Objective-C のヘッダ・`.m` の MATLAB / Objective-C・
//! `.pde`・`.jsonc`・`.json5`）を dispatch の `OpenFile`（= `tako open` / MCP `tako_open_file` と
//! 同じ 1 本。表示種別は指定しない）で同じペインへ順に開く。
//! ① 開いた応答の `syntax`（#1948）と、塗り終えた後の `syntax`（`PreviewHost::preview_syntax`）が
//! 形式ごとの構文名になる ② 複数の構文色で塗られる（`.h` / `.m` の MATLAB / `.pde` / `.jsonc` /
//! `.json5`）③ `.json5` はダークの面で消える字句が 0（JSON の構文なら `invalid` を地の色で描く）
//! ④ 手掛かりの無い `.m`（Objective-C）を編集モードで `%` の行つきに置き換えると、塗り直しで
//! MATLAB へ切り替わる（本文の判定は打鍵ごとの塗り直しでも引き直す）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1953_LEGACY=1` は #1953 以前（`.h` = Objective-C・
//! `.m` = 常に Objective-C・ほかは Plain Text）なので ① が FAILED = 同一バイナリでの A/B。
//! 画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ `syntax-alias-<形式>.png` で落とす（新旧で dir を
//! 分けて回す）。単独実行は `TAKO_VISUAL_ONLY=syntax-alias`

use super::*;
use crate::preview::alias_samples_1953::*;

const SECTION: &str = "syntax-alias";

/// 開く形式・本文・塗るべき構文・画像の名前（`None` = 画像は落とさない）・複数色を求めるか
const CASES: &[(&str, &str, &str, Option<&str>, bool)] = &[
    ("geo.h", C_HEADER, "C++", Some("h"), true),
    ("solve.m", MATLAB_SOURCE, "MATLAB", Some("m-matlab"), true),
    (
        "renovate.json5",
        JSON5_CONFIG,
        "JavaScript",
        Some("json5"),
        true,
    ),
    ("sketch.pde", PROCESSING_SKETCH, "Java", Some("pde"), true),
    (
        "settings.jsonc",
        JSONC_SETTINGS,
        "JSON",
        Some("jsonc"),
        true,
    ),
    // 本文の手掛かりで Objective-C のまま（#1953 以前と同じ）
    ("Counter.h", OBJC_HEADER, "Objective-C", None, true),
    ("Counter.m", OBJC_SOURCE, "Objective-C", None, true),
];

/// 1 形式ぶんの観測
struct Observed {
    file: &'static str,
    expected: &'static str,
    want_colors: bool,
    file_type: Option<String>,
    opened_syntax: Option<String>,
    painted_syntax: Option<String>,
    painted: bool,
    colors: usize,
    /// ダークの面（`Theme::for_mode(Dark).background`）でコントラスト 1.5 未満の字句
    faint: Vec<String>,
}

pub(super) async fn syntax_alias_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    inject_section_failure(SECTION);
    ensure_fresh_scene(window, cx, SECTION).await;
    let dir = std::env::temp_dir().join(format!("tako-visual-1953-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("一時 dir");
    let dark = tako_core::theme::Theme::for_mode(tako_core::theme::ThemeMode::Dark);

    let mut pane: Option<PaneId> = None;
    let mut observed: Vec<Observed> = Vec::new();
    for &(file, text, expected, dump, want_colors) in CASES {
        let path = dir.join(file);
        std::fs::write(&path, text).expect("本文を書ける");
        let reply = window
            .update(cx, |app, _, cx| {
                let reply = tako_control::dispatch(
                    app,
                    tako_control::protocol::Request::OpenFile {
                        pane: pane.map(PaneId::as_u64),
                        path: path.display().to_string(),
                        mode: None,
                        direction: pane
                            .is_none()
                            .then_some(tako_control::protocol::Direction::Right),
                        focus: Some(true),
                        new_tab: false,
                        line: None,
                        column: None,
                    },
                    PaneOrigin::Cli,
                )
                .unwrap_or_else(|e| {
                    fail(&format!("visual-test {SECTION}: {file} を開けない: {e:?}"))
                });
                cx.notify();
                reply
            })
            .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: OpenFile")));
        let Some(raw) = reply["pane"].as_u64() else {
            fail(&format!(
                "visual-test {SECTION}: {file}: OpenFile 応答に pane が無い"
            ))
        };
        let target = PaneId::from_raw(raw);
        pane = Some(target);
        check(
            reply["mode"].as_str() == Some("code"),
            &format!(
                "visual-test {SECTION}: {file} の振り分けが code（実際: {})",
                reply["mode"]
            ),
        );
        check(
            wait_for_preview_maps(any, window, cx, target, false).await,
            &format!("visual-test {SECTION}: {file} の平文 paint"),
        );
        window
            .update(cx, |app, _, cx| app.drain_pending_highlights(cx))
            .ok();
        let painted = wait_for_preview_highlight(any, window, cx, target).await;
        let (lines, painted_syntax) = window
            .update(cx, |app, _, _| {
                let lines = match app.previews.get(&target).map(|s| &s.content) {
                    Some(preview::PreviewContent::Code(lines)) => lines.clone(),
                    _ => Vec::new(),
                };
                (
                    lines,
                    tako_control::PreviewHost::preview_syntax(app, target),
                )
            })
            .unwrap_or_default();
        let colors = lines
            .iter()
            .flatten()
            .filter_map(|span| span.color)
            .map(|c| (c.r, c.g, c.b))
            .collect::<std::collections::HashSet<_>>()
            .len();
        let faint = lines
            .iter()
            .flatten()
            .filter(|span| !span.text.trim().is_empty())
            .filter(|span| {
                span.color
                    .is_some_and(|c| c.contrast_ratio(dark.background) < 1.5)
            })
            .map(|span| span.text.clone())
            .collect::<Vec<_>>();
        // 画像は判定の前に落とす（A/B の旧でも絵が残るように）
        if let (Some(name), Ok(dump)) = (dump, std::env::var("TAKO_VISUAL_DUMP_DIR")) {
            if let Some((frame, _)) = capture_frame(any, cx) {
                let dump = std::path::Path::new(&dump);
                let _ = std::fs::create_dir_all(dump);
                let path = dump.join(format!("{SECTION}-{name}.png"));
                if frame.save(&path).is_ok() {
                    println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
                }
            }
        }
        let item = Observed {
            file,
            expected,
            want_colors,
            file_type: reply["file_type"].as_str().map(str::to_string),
            opened_syntax: reply["syntax"].as_str().map(str::to_string),
            painted_syntax,
            painted,
            colors,
            faint,
        };
        println!(
            "TAKO_VISUAL_PIXEL: {SECTION} file={} file_type={:?} opened_syntax={:?} \
             painted_syntax={:?} painted={} colors={} faint={} legacy={}",
            item.file,
            item.file_type,
            item.opened_syntax,
            item.painted_syntax,
            item.painted,
            item.colors,
            item.faint.len(),
            preview::syntax_alias_legacy()
        );
        observed.push(item);
    }

    // 判定は全形式を開いて画像を落とした後（旧でも全部の絵が残る）
    for item in &observed {
        check(
            item.opened_syntax.as_deref() == Some(item.expected)
                && item.painted_syntax.as_deref() == Some(item.expected),
            &format!(
                "① {} の応答の syntax が {}（開いた応答 {:?} / 塗った後 {:?}）(#1953)",
                item.file, item.expected, item.opened_syntax, item.painted_syntax
            ),
        );
    }
    for item in observed.iter().filter(|item| item.want_colors) {
        check(
            item.painted && item.colors > 1,
            &format!(
                "② {} が複数の構文色で塗られる（{} 色）(#1953)",
                item.file, item.colors
            ),
        );
    }
    for item in &observed {
        check(
            item.faint.is_empty(),
            &format!(
                "③ {} にダークの面で消える字句が無い（{:?}）(#1953)",
                item.file, item.faint
            ),
        );
    }

    // ④ 編集中に本文の手掛かりを足すと、塗り直しで構文が切り替わる（手掛かりの無い `.m` に
    // `%` の行を足すと Objective-C → MATLAB）。置き換えは CLI / MCP と同じ dispatch（PreviewApply）
    let Some(target) = pane else {
        fail(&format!("visual-test {SECTION}: 開いたペインが無い"))
    };
    let draft = dir.join("draft.m");
    std::fs::write(&draft, "x = 1;\n").expect("本文を書ける");
    let opened = window
        .update(cx, |app, _, cx| {
            let reply = tako_control::dispatch(
                app,
                tako_control::protocol::Request::OpenFile {
                    pane: Some(target.as_u64()),
                    path: draft.display().to_string(),
                    mode: None,
                    direction: None,
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            );
            cx.notify();
            reply.ok()
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: draft.m を開けない")));
    let draft_pane = opened["pane"]
        .as_u64()
        .map(PaneId::from_raw)
        .unwrap_or(target);
    let before_edit = opened["syntax"].as_str().map(str::to_string);
    check(
        wait_for_preview_maps(any, window, cx, draft_pane, false).await,
        &format!("visual-test {SECTION}: draft.m の平文 paint"),
    );
    let (editing, applied_syntax) = window
        .update(cx, |app, _, cx| {
            let _ = app.workspace.active_tab_mut().tree_mut().focus(draft_pane);
            let edit = tako_control::dispatch(
                app,
                tako_control::protocol::Request::PreviewEdit {
                    pane: Some(draft_pane.as_u64()),
                    enabled: Some(true),
                },
                PaneOrigin::Cli,
            );
            let apply = tako_control::dispatch(
                app,
                tako_control::protocol::Request::PreviewApply {
                    pane: Some(draft_pane.as_u64()),
                    text: "% メモ\nx = 1;\n".to_string(),
                },
                PaneOrigin::Cli,
            );
            cx.notify();
            (
                edit.ok()
                    .and_then(|v| v["editing"].as_bool())
                    .unwrap_or(false),
                apply
                    .ok()
                    .and_then(|v| v["syntax"].as_str().map(str::to_string)),
            )
        })
        .unwrap_or_default();
    // 塗り直しを状態で待つ（上限 10 秒）
    let mut after_edit = None;
    for _ in 0..100 {
        notify_and_draw(any, window, cx);
        window
            .update(cx, |app, _, cx| app.drain_pending_highlights(cx))
            .ok();
        after_edit = window
            .update(cx, |app, _, _| {
                tako_control::PreviewHost::preview_syntax(app, draft_pane)
            })
            .ok()
            .flatten();
        if after_edit.as_deref() == Some("MATLAB") {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} file=draft.m before_edit={before_edit:?} editing={editing} \
         applied_syntax={applied_syntax:?} after_edit={after_edit:?}"
    );
    check(
        before_edit.as_deref() == Some("Objective-C") && editing,
        &format!(
            "④ 手掛かりの無い draft.m は Objective-C のまま・編集モードに入れる（{before_edit:?} / {editing}）"
        ),
    );
    check(
        after_edit.as_deref() == Some("MATLAB"),
        &format!(
            "④ 編集で `%` の行を足すと塗り直しで MATLAB へ切り替わる（実際: {after_edit:?}）(#1953)"
        ),
    );

    // 片付け: 編集を捨てて閉じる（保存しない）
    window
        .update(cx, |app, _, cx| {
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Close {
                    pane: Some(draft_pane.as_u64()),
                    force: true,
                    caller_role: None,
                },
                PaneOrigin::Cli,
            );
            cx.notify();
        })
        .ok();
    let _ = std::fs::remove_dir_all(&dir);
}
