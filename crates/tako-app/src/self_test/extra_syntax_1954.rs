//! visual-test `extra-syntax`（FR-3.2 / Issue #1954）: two-face に構文が無かった PowerShell
//! （`.ps1` / `.psm1` / `.psd1`）が、足した構文で実際に塗られ、応答の `syntax` で読めるか。
//!
//! 場面: 一時 dir の 5 本（スクリプト・モジュール・マニフェスト・ヒアドキュメントの中の C#・
//! 空の `.ps1`）を dispatch の `OpenFile`（= `tako open` / MCP `tako_open_file` と同じ 1 本。
//! 表示種別は指定しない）で同じペインへ順に開く。
//! ① 開いた応答の `syntax`（#1948）と、塗り終えた後の `syntax`（`PreviewHost::preview_syntax`）が
//! `PowerShell`、`file_type` が `powershell` ② 本文のある 4 本が複数の構文色で塗られる
//! ③ ダークの面で消える字句が 0（構文を取り違えると `invalid` を地の色で描く）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1954_LEGACY=1` は #1954 以前（two-face の 213 構文の
//! まま = Plain Text）なので ① が FAILED = 同一バイナリでの A/B。画像は判定の前に
//! `TAKO_VISUAL_DUMP_DIR` へ `extra-syntax-<形式>.png` で落とす（新旧で dir を分けて回す）。
//! 単独実行は `TAKO_VISUAL_ONLY=extra-syntax`

use super::*;
use crate::preview::extra_samples_1954::*;

const SECTION: &str = "extra-syntax";

/// 開く形式・本文・画像の名前（`None` = 画像は落とさない）・複数色を求めるか
const CASES: &[(&str, &str, Option<&str>, bool)] = &[
    ("pack.ps1", PS_SCRIPT, Some("ps1"), true),
    ("Slug.psm1", PS_MODULE, Some("psm1"), true),
    ("Slug.psd1", PS_MANIFEST, Some("psd1"), true),
    // ヒアドキュメントの中の C#（two-face の C# へ結んだ参照を通る）
    (
        "Hasher.ps1",
        PS_CSHARP_HEREDOC,
        Some("csharp-heredoc"),
        true,
    ),
    ("empty.ps1", "", None, false),
];

/// 1 形式ぶんの観測
struct Observed {
    file: &'static str,
    want_colors: bool,
    file_type: Option<String>,
    opened_syntax: Option<String>,
    painted_syntax: Option<String>,
    painted: bool,
    colors: usize,
    /// ダークの面（`Theme::for_mode(Dark).background`）でコントラスト 1.5 未満の字句
    faint: Vec<String>,
}

pub(super) async fn extra_syntax_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    inject_section_failure(SECTION);
    ensure_fresh_scene(window, cx, SECTION).await;
    let dir = std::env::temp_dir().join(format!("tako-visual-1954-{}", std::process::id()));
    remove_on_fail(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("一時 dir");
    let dark = tako_core::theme::Theme::for_mode(tako_core::theme::ThemeMode::Dark);

    let mut pane: Option<PaneId> = None;
    let mut observed: Vec<Observed> = Vec::new();
    for &(file, text, dump, want_colors) in CASES {
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
        // 空の本文は描く行も塗る字句も無いので、行の描画と塗りの完了を待たない（描画だけ回す）
        let painted = if text.is_empty() {
            for _ in 0..4 {
                notify_and_draw(any, window, cx);
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
            true
        } else {
            check(
                wait_for_preview_maps(any, window, cx, target, false).await,
                &format!("visual-test {SECTION}: {file} の平文 paint"),
            );
            window
                .update(cx, |app, _, cx| app.drain_pending_highlights(cx))
                .ok();
            wait_for_preview_highlight(any, window, cx, target).await
        };
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
            crate::syntax_extra::legacy()
        );
        observed.push(item);
    }

    // 判定は全形式を開いて画像を落とした後（旧でも全部の絵が残る）
    for item in &observed {
        check(
            item.opened_syntax.as_deref() == Some("PowerShell")
                && item.painted_syntax.as_deref() == Some("PowerShell")
                && item.file_type.as_deref() == Some("powershell"),
            &format!(
                "① {} の応答の syntax が PowerShell・file_type が powershell（開いた応答 {:?} / \
                 塗った後 {:?} / file_type {:?}）(#1954)",
                item.file, item.opened_syntax, item.painted_syntax, item.file_type
            ),
        );
    }
    for item in observed.iter().filter(|item| item.want_colors) {
        check(
            item.painted && item.colors > 1,
            &format!(
                "② {} が複数の構文色で塗られる（{} 色）(#1954)",
                item.file, item.colors
            ),
        );
    }
    for item in &observed {
        check(
            item.faint.is_empty(),
            &format!(
                "③ {} にダークの面で消える字句が無い（{:?}）(#1954)",
                item.file, item.faint
            ),
        );
    }

    // 片付け: 開いたペインを閉じる
    if let Some(target) = pane {
        window
            .update(cx, |app, _, cx| {
                let _ = tako_control::dispatch(
                    app,
                    tako_control::protocol::Request::Close {
                        pane: Some(target.as_u64()),
                        force: true,
                        caller_role: None,
                        caller_pane: None,
                    },
                    PaneOrigin::Cli,
                );
                cx.notify();
            })
            .ok();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
