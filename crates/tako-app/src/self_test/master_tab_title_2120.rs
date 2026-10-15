//! visual-test `master-tab-title`（Issue #2120）: **復元したままのタブバー**を撮り、
//! master のいるタブが役割名の手動名（`master` / `master-<名前>`）で固定されていないかを見る。
//!
//! 場面は節の中で作らない。`scripts/test-master-tab-title-2120.sh` が実 CLI
//! （`tako master` / `tako orchestrator adopt` / `tako tab rename`）で組んだ layout を、
//! この節を付けた GUI が**再起動で復元した状態**をそのまま撮る（復元側の解除 =
//! `tako_control::layout::release_legacy_master_title` を通った後の姿が要るため）。
//!
//! ① 復元が済み、タブが `TAKO_2120_EXPECT_TABS` 枚（既定 2）以上になるまで待つ
//! ② タブ名・出どころ・タブバーに出る表示名を 1 行ずつ出す（`TAKO_VISUAL_TAB:`）
//! ③ 画像を判定の前に `TAKO_VISUAL_DUMP_DIR` へ `master-tab-title.png`（全体）と
//!    `master-tab-title-bar.png`（上端の帯）で落とす
//! ④ master のペインがいるタブが役割名の手動名なら落とす（#2120 の症状そのもの）
//!
//! 修正前（`TAKO_2120_LEGACY=1`）で旧い layout を復元すると ④ で落ちる。
//! 単独実行は `TAKO_VISUAL_ONLY=master-tab-title`

use super::*;

const SECTION: &str = "master-tab-title";

/// 上端の帯（タブバーを含む）として切り出す高さ（論理 px）
const BAR_CROP_HEIGHT: f32 = 96.0;

pub(super) async fn master_tab_title_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let expect: usize = std::env::var("TAKO_2120_EXPECT_TABS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    // ① 復元（tmux への再接続を含む）が済むまで待つ
    let mut tabs = 0;
    for _ in 0..200 {
        tabs = window
            .update(cx, |app, _, _| app.workspace.tabs().len())
            .unwrap_or(0);
        if tabs >= expect {
            break;
        }
        wait(cx, 50).await;
    }
    check(
        tabs >= expect,
        &format!("visual-test {SECTION}: 復元したタブが {tabs} 枚（{expect} 枚を待った）"),
    );
    // 画面の再接続・タイトルの反映を待ってから撮る
    for _ in 0..20 {
        notify_and_draw(any, window, cx);
        wait(cx, 100).await;
    }

    // ② タブ名・出どころ・タブバーの表示名（default はフォーカスペインの OSC タイトル）
    let rows: Vec<(u64, String, &'static str, String, bool)> = window
        .update(cx, |app, _, _| {
            app.workspace
                .tabs()
                .iter()
                .map(|tab| {
                    let label = if tab.title_source() == TitleSource::Default {
                        app.terminals
                            .get(&tab.tree().focused())
                            .and_then(|s| s.title())
                            .unwrap_or(tab.title())
                            .to_string()
                    } else {
                        tab.title().to_string()
                    };
                    let hosts_master = tab.tree().panes().iter().any(|p| {
                        p.role()
                            .and_then(tako_core::handoff::master_profile_of_role)
                            .is_some()
                    });
                    (
                        tab.id().as_u64(),
                        tab.title().to_string(),
                        tab.title_source().as_str(),
                        label,
                        hosts_master,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for (id, title, source, label, hosts_master) in &rows {
        println!(
            "TAKO_VISUAL_TAB: {SECTION} id={id} title={title} source={source} label={label} master={hosts_master}"
        );
    }

    // ③ 画像は判定の前に落とす（修正前のビルドでも絵が残るように）
    let (frame, scale) = capture_frame(any, cx)
        .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: フレームを撮れない")));
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        let dump = std::path::Path::new(&dump);
        let _ = std::fs::create_dir_all(dump);
        let path = dump.join("master-tab-title.png");
        if frame.save(&path).is_ok() {
            println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
        }
        let (w, h) = frame.dimensions();
        let crop_h = ((BAR_CROP_HEIGHT * scale) as u32).min(h);
        let bar = image::imageops::crop_imm(&frame, 0, 0, w, crop_h).to_image();
        let path = dump.join("master-tab-title-bar.png");
        if bar.save(&path).is_ok() {
            println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
        }
    }

    // ④ master のいるタブが役割名の手動名で固定されていない
    for (id, title, source, _, hosts_master) in &rows {
        check(
            !(*hosts_master
                && *source == "manual"
                && tako_core::master_tab::is_legacy_role_title(title)),
            &format!(
                "visual-test {SECTION}: master のいるタブ {id} が役割名の手動名 '{title}' で固定されている"
            ),
        );
    }
}
