//! Web ビューを見たあとに打鍵が tako へ戻ることを縛る番犬（Issue #1481）
//!
//! # なぜ要るか
//!
//! Web ビューは GPUI の合成レイヤの**上**に乗るネイティブビュー（macOS = WKWebView）で、
//! ページをクリックすると **AppKit のキー入力の宛先**（NSWindow の first responder）が
//! そちらへ移る。tako 側でペインのフォーカスを動かしても（クリック・⌘数字・
//! `tako focus`）AppKit の宛先は動かないので、**ターミナルへ戻っても打鍵がページに
//! 吸われ続ける**のが #1481（ユーザー原文「ウェブビュー見た後，ターミナルに対する
//! プロンプトの入力ができなくなってる」）。
//!
//! 直したあとに同じ形へ戻る道が 5 つあるので、ここで塞ぐ:
//!
//! 1. 宛先を戻す呼び出しが `clear_text_input_focus`（= 入力対象が変わる全経路。
//!    #503 と同じ集合）から外れる。**経路ごとに足す形へ散らすと必ず 1 つ漏れる**
//! 2. Web ビューの破棄経路が増え、そこだけ**壊す前に戻し忘れる**
//!    （宛先のまま消えるとウィンドウの first responder が宙に浮き、どこへも届かない）
//! 3. 宛先を `contentView` にする綴りが戻る。gpui は自前の描画ビューを `contentView` の
//!    **サブビュー**として足すので、素の `NSView` である `contentView` は
//!    `acceptsFirstResponder` が NO = 宛先になれない（#326 の取り違え）
//! 4. AppKit を触る綴りが `webview.rs` の外（`main.rs`）へ散る
//! 5. `focus_direction` が**フォーカスを動かす前**にクリアへ入る = 古いフォーカスで
//!    「戻すかどうか」を判定してしまう
//!
//! 併せて、隠すときに宛先を返す既存経路（`sync_frame(None)` の `focus_parent()`）と
//! A/B の口（`TAKO_1481_LEGACY`）が消えていないことも見る。
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で窓が
//! 採れていることを固定し、[`逆戻りを名指しできる`] で**注入 8 通り**が `file:line` で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const WEBVIEW: &str = "crates/tako-app/src/webview.rs";

/// `webviews`（Web ビューの保管列）を縮める呼び出し。**どれも壊す前に宛先を戻す**必要がある
const SHRINK_VERBS: &[&str] = &[".remove(", ".drain(", ".retain(", ".clear()"];

/// セルフテストの後始末（製品の破棄経路ではない）。件数まで固定するので増減で落ちる
const SHRINK_ALLOWED_IN_SELF_TEST: usize = 1;

/// セルフテストの後始末を見分ける目印（同じ綴りが本番へ紛れても効く形にはしない。
/// **目印が古くなったら [`走査が空振りしていない`] が落ちる**）
const SELF_TEST_MARKER: &str = "項目 71 の close テストに影響する";

/// AppKit の宛先を触る綴り。`webview.rs` の 1 実装の外へ出してはならない
const APPKIT_SPELLINGS: &[&str] = &["makeFirstResponder", "firstResponder", "contentView"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（#1420 の 1 実装。**切らずにテスト領域だけを潰す**）
fn prod(root: &Path, rel: &'static str) -> String {
    production_range::production(&read(root, rel), rel)
}

#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（1-based の宣言行と本文）。終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// コメント行を除いて `needle` を含む行（0-based）をすべて返す
fn code_lines_with(src: &str, needle: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

struct Sources {
    main: String,
    webview: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            main: prod(root, MAIN),
            webview: prod(root, WEBVIEW),
        }
    }
}

/// 1: 宛先の復帰は `clear_text_input_focus`（入力対象が変わる全経路）の中に在る
fn scan_clear_route(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "fn clear_text_input_focus(&mut self)") else {
        return vec![Offender {
            file: MAIN,
            line: 0,
            why: "`clear_text_input_focus` が見つからない（走査が空振り）".into(),
        }];
    };
    if window.contains("self.return_webview_key_focus(false)") {
        return Vec::new();
    }
    vec![Offender {
        file: MAIN,
        line: at,
        why: "`clear_text_input_focus` が Web ビューのキー入力の宛先を戻していない\
              （#1481。ここは入力対象が変わる全経路（#503 と同じ集合）から呼ばれる\
              1 実装なので、経路ごとに足す形へ散らすと必ずどれかが漏れる）"
            .into(),
    }]
}

/// `webviews` を縮める呼び出しを拾う（**行をまたいだ綴りも拾う**。
/// `app\n.webviews\n.drain(..)` のようにメソッド連鎖が折り返されていても見逃さない）
fn shrink_sites(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("//") {
            continue;
        }
        let Some(verb) = SHRINK_VERBS.iter().find(|v| l.contains(**v)) else {
            continue;
        };
        let on_this_line = l.contains("webviews");
        let folded = i
            .checked_sub(1)
            .map(|j| lines[j].trim_end().ends_with(".webviews"))
            .unwrap_or(false);
        if on_this_line || folded {
            out.push((i, format!("webviews{verb}")));
        }
    }
    out
}

/// 2: Web ビューを壊す前に宛先を戻している（破棄経路の戻し忘れ）
fn scan_destroy_routes(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut product_sites = 0usize;
    let mut self_test_sites = 0usize;
    for (at, spelling) in shrink_sites(src) {
        // セルフテストの後始末（使い捨ての webview を畳む）は製品の破棄経路ではない
        if lines[at.saturating_sub(5)..=at]
            .iter()
            .any(|l| l.contains(SELF_TEST_MARKER))
        {
            self_test_sites += 1;
            continue;
        }
        product_sites += 1;
        let from = at.saturating_sub(8);
        let returned = lines[from..at]
            .iter()
            .any(|l| l.contains("self.return_webview_key_focus(true)"));
        if !returned {
            out.push(Offender {
                file: MAIN,
                line: at + 1,
                why: format!(
                    "`{spelling}` の直前で宛先を戻していない（#1481。\
                     宛先のまま WKWebView を壊すとウィンドウの first responder が\
                     宙に浮き、ターミナルにもどこにも打鍵が届かない）"
                ),
            });
        }
    }
    if product_sites != 2 {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: format!(
                "Web ビューの破棄経路が {product_sites} か所ある（#1481 の時点では\
                 `webview_close_button`（× ボタン）と `web_destroy`（dispatch close）の\
                 2 か所。増えた経路は壊す前に宛先を戻すこと。件数もここで固定する）"
            ),
        });
    }
    if self_test_sites != SHRINK_ALLOWED_IN_SELF_TEST {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: format!(
                "セルフテストの後始末が {self_test_sites} か所（想定 \
                 {SHRINK_ALLOWED_IN_SELF_TEST}）。走査の目印が古くなっていないか確かめる"
            ),
        });
    }
    out
}

/// 3: 宛先は `contentView` ではなく wry の親（gpui の描画ビュー）
fn scan_restore_target(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "unsafe fn restore_from_window(") else {
        return vec![Offender {
            file: WEBVIEW,
            line: 0,
            why: "`restore_from_window` が見つからない（走査が空振り）".into(),
        }];
    };
    let mut out = Vec::new();
    if window.contains("contentView") {
        out.push(Offender {
            file: WEBVIEW,
            line: at,
            why: "宛先を `contentView` にしている（#1481 / #326。gpui は描画ビューを\
                  `contentView` の**サブビュー**として足すので、素の `NSView` である\
                  `contentView` は `acceptsFirstResponder` が NO = 宛先になれない。\
                  実測では `makeFirstResponder:` が false を返し宛先は WKWebView のまま）"
                .into(),
        });
    }
    if !window.contains("sel(\"superview\")") {
        out.push(Offender {
            file: WEBVIEW,
            line: at,
            why: "宛先を実体（宛先を持っている WKWebView の `superview` = wry へ渡した\
                  親ビュー）から引いていない（#1481）"
                .into(),
        });
    }
    out
}

/// 4: AppKit を触る綴りは `webview.rs` の 1 実装の中だけ
fn scan_single_appkit_impl(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for spelling in APPKIT_SPELLINGS {
        for at in code_lines_with(src, spelling) {
            out.push(Offender {
                file: MAIN,
                line: at + 1,
                why: format!(
                    "AppKit のキー入力の宛先（`{spelling}`）を `main.rs` で直に触っている\
                     （#1481。読む・戻すは `webview.rs` の 1 実装
                     （`webview_holds_key_focus` / `return_key_focus_to_app`）を通す。\
                     2 か所に増えると宛先の取り違えが片方だけ直る）"
                ),
            });
        }
    }
    out
}

/// 5: 隠すときに宛先を返す既存経路が消えていない
fn scan_hide_returns_focus(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "pub fn sync_frame(&mut self") else {
        return vec![Offender {
            file: WEBVIEW,
            line: 0,
            why: "`sync_frame` が見つからない（走査が空振り）".into(),
        }];
    };
    if window.contains("self.view.focus_parent()") {
        return Vec::new();
    }
    vec![Offender {
        file: WEBVIEW,
        line: at,
        why: "隠す前に宛先を親へ返していない（#1481 / #326。不可視の webview が\
              宛先のままだと打鍵が見えないページに吸われる）"
            .into(),
    }]
}

/// 6: `focus_direction` は**フォーカスを動かしたあと**にクリアへ入る
fn scan_focus_direction_order(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "fn focus_direction(&mut self") else {
        return vec![Offender {
            file: MAIN,
            line: 0,
            why: "`focus_direction` が見つからない（走査が空振り）".into(),
        }];
    };
    let moved = window
        .lines()
        .position(|l| l.contains(".focus_direction(direction)"));
    let cleared = window
        .lines()
        .position(|l| l.contains("self.clear_text_input_focus()"));
    match (moved, cleared) {
        (Some(m), Some(c)) if m < c => Vec::new(),
        (Some(_), Some(_)) => vec![Offender {
            file: MAIN,
            line: at,
            why: "フォーカスを動かす**前**にクリアへ入っている（#1481。宛先を戻すかは\
                  「移動先が Web ビューか」で決まるので、古いフォーカスで判定すると\
                  Web ビューから出る移動で戻し忘れる）"
                .into(),
        }],
        _ => vec![Offender {
            file: MAIN,
            line: at,
            why: "`focus_direction` の中で移動とクリアの綴りが見つからない（走査が空振り）".into(),
        }],
    }
}

/// 7: ペイン本体のクリックは入力対象の切り替えを**明示に**通る
/// （ルート div の伝播に頼らない = `stop_propagation` が入った瞬間に静かに再発する）
fn scan_pane_click_route(src: &str) -> Vec<Offender> {
    let Some((at, window)) = fn_window(src, "fn on_pane_mouse_down(") else {
        return vec![Offender {
            file: MAIN,
            line: 0,
            why: "`on_pane_mouse_down` が見つからない（走査が空振り）".into(),
        }];
    };
    if window.contains("self.clear_text_input_focus();") {
        return Vec::new();
    }
    vec![Offender {
        file: MAIN,
        line: at,
        why: "ペイン本体のクリックが入力対象の切り替えを通っていない（#1481。\
              ルート div の一括 dismiss へ伝播で頼ると、ここへ `stop_propagation` が\
              入った瞬間に「クリックしても打鍵が Web ビューから戻らない」が静かに再発する）"
            .into(),
    }]
}

/// 8: A/B の口は 1 か所（`TAKO_1481_LEGACY`）
fn scan_legacy_gate(src: &str) -> Vec<Offender> {
    let hits = code_lines_with(src, "TAKO_1481_LEGACY");
    if hits.len() == 1 {
        return Vec::new();
    }
    vec![Offender {
        file: MAIN,
        line: hits.first().map_or(0, |i| i + 1),
        why: format!(
            "`TAKO_1481_LEGACY` を読む場所が {} か所（#1481。A/B の口は\
             `webview_key_focus_legacy` の 1 か所。無くなると症状を再現できず、\
             増えると腕ごとに挙動が割れる）",
            hits.len()
        ),
    }]
}

fn scan_all(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    out.extend(scan_clear_route(&s.main));
    out.extend(scan_destroy_routes(&s.main));
    out.extend(scan_restore_target(&s.webview));
    out.extend(scan_single_appkit_impl(&s.main));
    out.extend(scan_hide_returns_focus(&s.webview));
    out.extend(scan_focus_direction_order(&s.main));
    out.extend(scan_pane_click_route(&s.main));
    out.extend(scan_legacy_gate(&s.main));
    out
}

#[test]
fn webビューの後に打鍵が戻る構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1481 の構造が崩れている:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査が空振りしていれば上のテストは無意味に緑になるので、窓が採れていることを固定する
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = Sources::load(&root);
    for (label, src, needle, file) in [
        (
            "入力対象の切り替え",
            &s.main,
            "fn clear_text_input_focus(&mut self)",
            MAIN,
        ),
        (
            "宛先の復帰",
            &s.main,
            "fn return_webview_key_focus(&mut self",
            MAIN,
        ),
        (
            "方向フォーカス",
            &s.main,
            "fn focus_direction(&mut self",
            MAIN,
        ),
        (
            "宛先の復帰（AppKit）",
            &s.webview,
            "unsafe fn restore_from_window(",
            WEBVIEW,
        ),
        (
            "宛先の読み取り",
            &s.webview,
            "unsafe fn webview_holding_focus(",
            WEBVIEW,
        ),
        (
            "フレーム同期",
            &s.webview,
            "pub fn sync_frame(&mut self",
            WEBVIEW,
        ),
    ] {
        let (at, window) = fn_window(src, needle).unwrap_or_else(|| {
            panic!("{file}: {label}（{needle}）の窓が採れない = 走査が壊れている")
        });
        assert!(at > 0, "{label} の行番号が 0");
        assert!(
            window.lines().count() > 3,
            "{label} の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
    // 本番コードが潰れていない（範囲取りの事故で走査対象が消えていない）
    assert!(
        s.main.lines().count() > 40_000,
        "{MAIN} の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        s.main.lines().count()
    );
    assert!(
        s.webview.lines().count() > 400,
        "{WEBVIEW} の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        s.webview.lines().count()
    );
    // 縮める呼び出しが実在する（`SHRINK_VERBS` の綴りが古くなっていない）
    let sites = shrink_sites(&s.main);
    assert!(
        sites.len() >= 3,
        "{MAIN}: `webviews` を縮める呼び出しが {} か所しか拾えていない（走査が空振り。\
         想定は製品 2 か所 + セルフテストの後始末 1 か所）",
        sites.len()
    );
    // セルフテストの後始末の目印が古くなっていない（古くなると製品の違反として誤検出する）
    assert!(
        s.main.contains(SELF_TEST_MARKER),
        "{MAIN}: セルフテストの後始末の目印 `{SELF_TEST_MARKER}` が無い（走査が空振り）"
    );
}

fn expect_hit(s: &Sources, file: &str, needle: &str) {
    let offenders = scan_all(s);
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(needle)),
        "注入したのに名指しされない（needle={needle:?}）:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 直す前の形へ戻す注入が、すべて `file:line` で名指しされること
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) 入力対象が変わっても宛先を戻さない（#1481 そのもの）
    let mut s = Sources::load(&root);
    s.main = s.main.replace("self.return_webview_key_focus(false);", "");
    expect_hit(&s, MAIN, "宛先を戻していない");

    // (2) 破棄の前に戻さない（× ボタン / dispatch close の片方）
    let mut s = Sources::load(&root);
    s.main = s
        .main
        .replacen("self.return_webview_key_focus(true);", "", 1);
    expect_hit(&s, MAIN, "の直前で宛先を戻していない");

    // (3) 破棄経路が増えたのに戻していない
    let mut s = Sources::load(&root);
    s.main = s.main.replace(
        "    fn web_navigate(&mut self, id: u64, to: &str)",
        "    fn web_forget(&mut self, idx: usize) {\n\
         \x20       self.webviews.remove(idx);\n    }\n\n\
         \x20   fn web_navigate(&mut self, id: u64, to: &str)",
    );
    expect_hit(&s, MAIN, "の直前で宛先を戻していない");

    // (4) 宛先を `contentView` に戻す（#326 の取り違え）
    let mut s = Sources::load(&root);
    s.webview = s.webview.replace(
        "            f_id(wk, sel(\"superview\"))",
        "            f_id(window, sel(\"contentView\"))",
    );
    expect_hit(&s, WEBVIEW, "`contentView` にしている");

    // (5) AppKit を main.rs で直に触る
    let mut s = Sources::load(&root);
    s.main = s.main.replace(
        "    fn return_webview_key_focus(&mut self, force: bool) {",
        "    fn return_webview_key_focus(&mut self, force: bool) {\n\
         \x20       let _ = \"makeFirstResponder\";",
    );
    expect_hit(&s, MAIN, "で直に触っている");

    // (6) 隠すときの戻しを外す
    let mut s = Sources::load(&root);
    s.webview = s.webview.replace("let _ = self.view.focus_parent();", "");
    expect_hit(&s, WEBVIEW, "隠す前に宛先を親へ返していない");

    // (7) 移動より前にクリアへ入る（古いフォーカスで判定する）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.main, "fn focus_direction(&mut self").expect("窓");
    let reordered =
        "    fn focus_direction(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {\n\
         \x20       self.clear_text_input_focus();\n\
         \x20       self.workspace\n\
         \x20           .active_tab_mut()\n\
         \x20           .tree_mut()\n\
         \x20           .focus_direction(direction);\n\
         \x20       cx.notify();\n    }";
    s.main = s.main.replace(&window, reordered);
    expect_hit(&s, MAIN, "クリアへ入っている");

    // (8) ペイン本体のクリックがルートの伝播頼みへ戻る
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.main, "fn on_pane_mouse_down(").expect("窓");
    s.main = s.main.replace(
        &window,
        &window.replace("        self.clear_text_input_focus();\n", ""),
    );
    expect_hit(&s, MAIN, "入力対象の切り替えを通っていない");

    // (9) A/B の口が消える（症状を再現できなくなる）
    let mut s = Sources::load(&root);
    s.main = s
        .main
        .replace("std::env::var_os(\"TAKO_1481_LEGACY\").is_some()", "false");
    expect_hit(&s, MAIN, "を読む場所が 0 か所");
}
