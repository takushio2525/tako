//! visual-test `setup-deps-2433`（FR-2.33.18 / Issue #2433）: 設定画面「セットアップ」の「任意の依存ツール」に、
//! tako が入れられる依存のボタンが出て、Windows の UAC が出る依存（Git）は「導入（管理者の確認あり）」と
//! 書いたボタン、利用者の権限で入る依存（psmux）は「導入」のボタンになるか。導入済みはパス、手で入れる
//! ものは導入コマンドで、ボタンを出さない。
//!
//! macOS の上で Windows の腕を描くため、一覧は `TAKO_2433_DEPS_FIXTURE`（検証のプロセスだけが読む JSON の配列。
//! 形は `tako setup deps --json` の `deps`）で差し替える。**ボタンは押さない**: 押下が送るのは CLI / MCP と同じ
//! `tako_setup_deps` の install で、本物の導入器（brew / winget）を起こす（送る要求の形は単体テスト、
//! 起こした結果は Windows の CI の `scripts/windows/test-deps-install-2433.ps1` が見る）。
//!
//! 画像は `TAKO_VISUAL_DUMP_DIR` へ `setup-deps-2433-both.png`（① 2 つとも未導入）/
//! `setup-deps-2433-psmux-installed.png`（② psmux が入った後）で落とす。一覧は節が `TAKO_2433_DEPS_FIXTURE` の
//! 置き場へ書き、タブを切り替えて読み直させる。
//! 単独実行は `TAKO_VISUAL_ONLY=setup-deps-2433`（`scripts/test-setup-deps-gui-2433.sh` が呼ぶ）

use super::*;

const SECTION: &str = "setup-deps-2433";

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

/// 描かれたボタンの実矩形（無ければ `None`）
fn button_rect(
    settings: WindowHandle<crate::settings_window::SettingsWindow>,
    cx: &mut AsyncApp,
    bin: &str,
) -> Option<crate::form_layout::Rect> {
    let key = format!("ctl:btn-dep-{bin}");
    settings
        .update(cx, |view, _, _| {
            view.vt_probe_bounds()
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, r)| *r)
        })
        .ok()
        .flatten()
}

/// Windows の `tako setup deps --json` の `deps` と同じ形の 1 件（winget は見つかっている）
fn windows_dep(bin: &str, found: Option<&str>, privilege: &str, pkg: &str) -> serde_json::Value {
    let scope = if privilege == "user" {
        " --scope user --silent"
    } else {
        ""
    };
    serde_json::json!({
        "bin": bin,
        "found": found,
        "required": false,
        "purpose": "test",
        "install_command": format!(
            "winget install --id {pkg} --exact --source winget{scope} --disable-interactivity \
             --accept-source-agreements --accept-package-agreements"
        ),
        "can_run": true,
        "privilege": privilege,
        "installer_found": "C:\\Users\\winuser\\AppData\\Local\\Microsoft\\WindowsApps\\winget.exe",
        "install_dir": null,
        "hint": "https://example.invalid",
        "lookup_timeout": null,
        "runnable": null,
    })
}

const LINKS_PSMUX: &str = "C:\\Users\\winuser\\AppData\\Local\\Microsoft\\WinGet\\Links\\psmux.exe";

/// 一覧を書いて、タブを切り替えて読み直させ（一覧の差し替えは読み直しの中で同期に効く）、描き終わって
/// 静止した絵とボタンの実矩形を返す
async fn show(
    window: WindowHandle<TakoApp>,
    settings: WindowHandle<crate::settings_window::SettingsWindow>,
    cx: &mut AsyncApp,
    fixture: &std::path::Path,
    deps: serde_json::Value,
) -> (
    image::RgbaImage,
    Option<crate::form_layout::Rect>,
    Option<crate::form_layout::Rect>,
) {
    use crate::settings_window::SettingsTab;
    if std::fs::write(fixture, deps.to_string()).is_err() {
        fail(&format!("visual-test {SECTION}: 一覧を書けない"));
    }
    let _ = settings.update(cx, |view, _, cx| {
        view.set_tab(SettingsTab::General, cx);
        view.set_tab(SettingsTab::Setup, cx);
        cx.notify();
    });
    let settings_any: AnyWindowHandle = settings.into();
    let frame = match wait_for_quiet_frame(
        settings_any,
        window,
        cx,
        SECTION,
        text_wait_budget(40, 100, 15),
        |_, _| true,
    )
    .await
    {
        QuietFrame::Quiet(frame, _) => frame,
        QuietFrame::Moving => capture_frame(settings_any, cx)
            .map(|(frame, _)| frame)
            .unwrap_or_else(|| fail(&format!("visual-test {SECTION}: 設定画面を撮れない"))),
        QuietFrame::NoCapture => fail(&format!("visual-test {SECTION}: 設定画面を撮れない")),
    };
    (
        frame,
        button_rect(settings, cx, "git"),
        button_rect(settings, cx, "psmux"),
    )
}

pub(super) async fn setup_deps_visual(
    _any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use crate::settings_window::SettingsTab;
    let Some(fixture) = std::env::var_os("TAKO_2433_DEPS_FIXTURE").map(std::path::PathBuf::from)
    else {
        fail(&format!(
            "visual-test {SECTION}: TAKO_2433_DEPS_FIXTURE が無い（一覧を書く置き場を渡して起こす）"
        ));
    };
    // 開いた時点の読み込みから差し替えが効くよう、窓を開く前に一覧を書く
    let both = serde_json::json!([
        windows_dep("psmux", None, "user", "marlocarlo.psmux"),
        windows_dep("git", None, "elevated", "Git.Git"),
    ]);
    if std::fs::write(&fixture, both.to_string()).is_err() {
        fail(&format!("visual-test {SECTION}: 一覧を書けない"));
    }
    let _ = window.update(cx, |app, _, cx| {
        app.open_settings_window_impl(Some(SettingsTab::Setup), cx);
    });
    // 窓は defer で開く = 状態（ハンドルが立つ）で待つ
    wait_for_app_state(
        window,
        cx,
        "setup-deps-2433: 設定ウィンドウ",
        state_wait_budget(Duration::from_secs(4), machine_busy()),
        |app| app.settings_window_handle.is_some(),
    )
    .await;
    let Some(settings) = window
        .update(cx, |app, _, _| app.settings_window_handle)
        .ok()
        .flatten()
    else {
        fail(&format!("visual-test {SECTION}: 設定ウィンドウが開かない"));
    };
    let _ = settings.update(cx, |_, window, cx| {
        window.resize(size(px(760.), px(720.)));
        cx.notify();
    });
    let width = |r: crate::form_layout::Rect| r.x1 - r.x0;

    // ① psmux（利用者の権限）と Git（UAC）が未導入 = ボタン 2 つ。Git は「導入（管理者の確認あり）」
    let (frame, git, psmux) = show(window, settings, cx, &fixture, both).await;
    dump(&frame, "setup-deps-2433-both.png");
    let (Some(git), Some(psmux)) = (git, psmux) else {
        fail(&format!(
            "visual-test {SECTION} ①: 入れられる依存のボタンが描かれていない（git={git:?} psmux={psmux:?}）"
        ));
    };
    // UAC の出る Git は「導入（管理者の確認あり）」= 「導入」より明らかに広い
    check(
        width(git) > width(psmux) + 40.0,
        &format!(
            "visual-test {SECTION} ①: UAC の出る依存のボタンは「管理者の確認あり」と書く（幅 git={:.0} psmux={:.0}）",
            width(git),
            width(psmux)
        ),
    );

    // ② psmux が入った後 = psmux はパスを出してボタンを消し、Git のボタンだけが残る
    let after = serde_json::json!([
        windows_dep("psmux", Some(LINKS_PSMUX), "user", "marlocarlo.psmux"),
        windows_dep("git", None, "elevated", "Git.Git"),
    ]);
    let (frame, git2, psmux2) = show(window, settings, cx, &fixture, after).await;
    dump(&frame, "setup-deps-2433-psmux-installed.png");
    check(
        git2.is_some() && psmux2.is_none(),
        &format!(
            "visual-test {SECTION} ②: 導入済みの依存にはボタンを出さない（git={git2:?} psmux={psmux2:?}）"
        ),
    );
    println!(
        "TAKO_2433_GUI: git_btn_w={:.0} psmux_btn_w={:.0} after_install_psmux_btn={}",
        width(git),
        width(psmux),
        psmux2.is_some()
    );
}
