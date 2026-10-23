//! visual-test `palette-alias`（Issue #2340）: コマンドパレットが**表記を揃える前の旧表記**
//! （検索だけに効く別名 `ui_text::palette::cmd_aliases`）でも引けるかを、実 GUI のパレットで見る。
//!
//! 日英それぞれで、テーマの項目（両言語とも旧表記がある）について:
//! ① パレットを開き、旧表記を打鍵・⌘V・IME の確定と同じ挿入関数（`insert_app_text_input`。#1750）で
//!    入れる（クリップボードには触らない）
//! ② 候補の先頭がテーマの項目で、そのラベルは**いまの表記**（旧表記は表示しない）・候補に 1 回だけ
//! ③ 画像を判定の前に `TAKO_VISUAL_DUMP_DIR` へ `palette-alias-<ja|en>.png` で落とす
//! ④ 実キーの Enter でテーマが反転し、パレットが閉じる（後始末でテーマを戻す）
//!
//! 別名を外すとこの節は ② で落ちる（旧表記がどの候補にも当たらない）。
//! 単独実行は `TAKO_VISUAL_ONLY=palette-alias`

use super::*;

const SECTION: &str = "palette-alias";

pub(super) async fn palette_alias_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    use tako_core::i18n::Lang;
    let wait =
        |cx: &mut AsyncApp, ms: u64| cx.background_executor().timer(Duration::from_millis(ms));
    let original = tako_core::i18n::lang();
    for lang in [Lang::Ja, Lang::En] {
        tako_core::i18n::set_lang(lang);
        let tag = match lang {
            Lang::Ja => "ja",
            Lang::En => "en",
        };
        // 人が打つ語なので別名の表からは引かない（別名を外したら ② で落ちるように）。
        // #1830 より前のラベルそのまま
        let old = match lang {
            Lang::Ja => "テーマをライト/ダーク切替",
            Lang::En => "Toggle light/dark theme",
        };
        let label = crate::ui_text::palette::cmd_label("toggle-theme");
        check(
            !label.to_lowercase().contains(&old.to_lowercase()),
            &format!(
                "visual-test {SECTION}（{tag}）: 旧表記 {old:?} がいまのラベル {label:?} で引けてしまう\
                 （別名の検査にならない）"
            ),
        );

        // ① 開いて旧表記を入れる
        let _ = window.update(cx, |app: &mut TakoApp, window, cx| {
            app.open_command_palette(window, cx);
            app.insert_app_text_input(AppTextInput::Palette, old, cx);
        });
        for _ in 0..3 {
            notify_and_draw(any, window, cx);
            wait(cx, 100).await;
        }

        // ② 候補を読む（画面に並ぶのと同じ `palette_items`）
        let (query, first, theme_hits, total) = window
            .update(cx, |app: &mut TakoApp, _, _| {
                let query = app
                    .command_palette
                    .as_ref()
                    .map(|p| p.query.clone())
                    .unwrap_or_default();
                let items = app.palette_items(&query);
                let first = items.first().map(|item| match item {
                    PaletteItem::Command(label, id) => format!("{id}={label}"),
                    other => format!("（固定コマンド以外）{}", other.label()),
                });
                let theme_hits = items
                    .iter()
                    .filter(|item| matches!(item, PaletteItem::Command(_, "toggle-theme")))
                    .count();
                (query, first, theme_hits, items.len())
            })
            .unwrap_or_default();
        println!(
            "TAKO_VISUAL_2340: lang={tag} query={query:?} first={first:?} \
             theme_hits={theme_hits} total={total}"
        );

        // ③ 判定の前に撮る
        if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
            if let Some((frame, _)) = capture_frame(any, cx) {
                let dump = std::path::Path::new(&dump);
                let _ = std::fs::create_dir_all(dump);
                let path = dump.join(format!("{SECTION}-{tag}.png"));
                if frame.save(&path).is_ok() {
                    println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
                }
            }
        }
        check(
            query == old,
            &format!("visual-test {SECTION}（{tag}）: 入力欄が旧表記にならない（{query:?}）"),
        );
        check(
            first.as_deref() == Some(format!("toggle-theme={label}").as_str()) && theme_hits == 1,
            &format!(
                "visual-test {SECTION}（{tag}）: 旧表記 {old:?} で候補の先頭がテーマの項目（ラベル \
                 {label:?}）にならないか、2 回以上出た（先頭 {first:?}・テーマ {theme_hits} 件）"
            ),
        );

        // ④ 実キーの Enter で実行される
        let before = window
            .update(cx, |app: &mut TakoApp, _, _| app.theme.mode)
            .ok();
        press(any, cx, "enter");
        wait(cx, 200).await;
        let (after, closed) = window
            .update(cx, |app: &mut TakoApp, _, _| {
                (Some(app.theme.mode), app.command_palette.is_none())
            })
            .unwrap_or((None, false));
        check(
            before.is_some() && after != before && closed,
            &format!(
                "visual-test {SECTION}（{tag}）: 旧表記で選んだ項目の Enter でテーマが反転しない\
                 （前 {before:?}・後 {after:?}・閉じた {closed}）"
            ),
        );
        // 後始末: テーマを戻す
        let _ = window.update(cx, |app: &mut TakoApp, _, cx| {
            app.toggle_theme(cx);
        });
        wait(cx, 100).await;
    }
    tako_core::i18n::set_lang(original);
    println!("TAKO_VISUAL_2340: done");
}
