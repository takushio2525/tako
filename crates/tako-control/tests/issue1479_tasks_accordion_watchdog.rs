//! 右パネル tasks ビューの「その場で展開」とタブのバッジを縛る番犬（Issue #1479）
//!
//! # なぜ要るか
//!
//! #1450 B2 の tasks ビューは、一覧を全部描き切ってから**末尾に 1 枚だけ**詳細を積み、
//! さらに「選択が無ければ一覧の先頭」へ落としていた。どちらも**動いては見える**
//! （詳細は出るし、押せば中身が変わる）ので目視では気づけないが、実際には
//!
//! - 押した行と詳細がパネル 1 枚ぶん以上離れる = ユーザーには「押しても何も起きない」
//! - 畳んだ状態を表せない = 「もう一度押して閉じる」が構造的に作れない
//!
//! になっていた（#1479 のユーザー原文「押したらその場所からドロップダウンで拡大される
//! 感じにしてほしい」）。直したあとに同じ形へ戻る道が 3 つあるので、ここで塞ぐ:
//!
//! 1. 詳細を描く場所が**行の外**（列の末尾）へ戻る / 2 か所になる
//! 2. 展開の解決が**先頭へ落ちる**ようになる（畳めなくなる）
//! 3. 絞り込みを変える経路が増え、そこだけ**畳み忘れる**
//! 4. ポーリング（2 秒）が展開を畳む = 開いた詳細が勝手に閉じる
//!
//! # バッジ側（#1479 B）
//!
//! タブ列が幅に収まらないと、最後に積んである tasks のバッジが右端で切れて
//! **22 が 2 に見える**。数字が部分的に見えるのは「出ない」より悪い（読み手は
//! 残った桁を件数だと信じる）ので、①幅から詰め方を決める純関数を通ること
//! ②バッジは `flex_none`・ラベルは `min_w(0)` で**縮むのはラベル側**であること
//! ③件数の文字は `panel_tab_badge` の 1 実装を通ること（`to_string()` を直に
//! 描くと桁が増えるたびに列が伸びる）を縛る。
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で窓が
//! 採れていることを固定し、[`逆戻りを名指しできる`] で**注入 10 通り**が `file:line`
//! で名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const TASKS_PANEL: &str = "crates/tako-app/src/tasks_panel.rs";
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";

/// 絞り込みを変える代入（**どれも畳みを伴わなければならない**）
const FILTER_SETTERS: &[&str] = &[
    "user_tasks.kind_filter = ",
    "user_tasks.project_filter = ",
    "user_tasks.show_done = ",
];

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

/// コードの行だけを走査して `needle` を含む行（0-based）をすべて返す
fn code_lines_with(src: &str, needle: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

/// `needle` を含む最初のコード行（0-based）
fn code_line_with(src: &str, needle: &str) -> Option<usize> {
    code_lines_with(src, needle).first().copied()
}

struct Sources {
    tasks_panel: String,
    right_panel: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            tasks_panel: prod(root, TASKS_PANEL),
            right_panel: prod(root, RIGHT_PANEL),
        }
    }
}

/// 1: 詳細は**押した行の器の子**として 1 か所だけで描かれる
fn scan_inline_detail(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let calls = code_lines_with(src, "render_task_detail(task");
    // 呼び出しが 1 つも無ければ走査が壊れている / 2 つ以上なら描く場所が割れている
    if calls.len() != 1 {
        out.push(Offender {
            file: TASKS_PANEL,
            line: calls.first().map_or(0, |i| i + 1),
            why: format!(
                "詳細を描く呼び出しが {} か所ある（#1479。開くのは 1 件なので\
                 描く場所も 1 か所。2 か所あると「行の下」と「列の末尾」の\
                 両方に出る絵が作れてしまう）",
                calls.len()
            ),
        });
    }
    if let Some(at) = calls.first() {
        let line = src.lines().nth(*at).unwrap_or_default();
        // **行 1 つぶんの器（item）の子**であること（列（scroll）へ直に積まない）
        if !line.contains("item = item.child(") {
            out.push(Offender {
                file: TASKS_PANEL,
                line: at + 1,
                why: "詳細が行の器（`item`）の子になっていない（#1479。\
                      `scroll` へ直に積むと列の末尾に出る = 直す前の形）"
                    .into(),
            });
        }
    }
    // 旧経路（列の末尾へ 1 枚積む）の綴りが戻っていないこと
    if let Some(at) = code_line_with(src, "scroll = scroll.child(self.render_task_detail") {
        out.push(Offender {
            file: TASKS_PANEL,
            line: at + 1,
            why: "詳細を一覧の**末尾**へ積んでいる（#1479 で消した旧経路）".into(),
        });
    }
    out
}

/// 2: 展開の解決は**先頭へ落とさない**（畳んだ状態を持てる）
fn scan_no_fallback(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn expanded_task<") {
        None => out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "`expanded_task` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            for mark in ["visible.first()", "or_else("] {
                if window.contains(mark) {
                    out.push(Offender {
                        file: TASKS_PANEL,
                        line: at,
                        why: format!(
                            "展開の解決が `{mark}` で**先頭へ落ちている**（#1479。\
                             落とすと「どれも開いていない」を表せず、\
                             もう一度押して閉じる操作が作れない）"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 3: 行の押下はトグルを通り、一括 dismiss に食われない（#496 / #503 の規約）
fn scan_row_toggle(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn render_tasks_view(") else {
        return vec![Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "`render_tasks_view` が見つからない（走査が空振り）".into(),
        }];
    };
    let row_at = code_line_with(&window, "(\"tasks-row\", i)").map(|i| at + i);
    if row_at.is_none() {
        out.push(Offender {
            file: TASKS_PANEL,
            line: at,
            why: "一覧の行（`(\"tasks-row\", i)`）が見つからない（走査が空振り）".into(),
        });
    }
    let line = row_at.unwrap_or(at);
    for (mark, why) in [
        (
            "user_tasks.toggle(",
            "行の押下が `toggle` を通っていない（#1479。開く / 閉じるの判断を\
             押下側で書くと、同時に 2 件開く・閉じられないが生える）",
        ),
        (
            "scroll.scroll_to_item(",
            "展開した行を見える位置へ運んでいない（#1479。画面外で開くと\
             ユーザーには何も起きていないように見える）",
        ),
        (
            "on_mouse_down(",
            "行が `on_mouse_down` で伝播を止めていない（#496 / #503。\
             トグルは押下で畳まれた直後に `on_click` が開き直すので、\
             開いた状態から閉じられなくなる）",
        ),
        (
            "cx.stop_propagation()",
            "行の押下が `stop_propagation` を呼んでいない（#496 / #503）",
        ),
    ] {
        if !window.contains(mark) {
            out.push(Offender {
                file: TASKS_PANEL,
                line,
                why: why.to_string(),
            });
        }
    }
    // 開閉の矢印は**描画プリミティブ**（絵文字禁止 = FR-2.40.14）
    if !window.contains("CHEVRON_DOWN") || !window.contains("CHEVRON_RIGHT") {
        out.push(Offender {
            file: TASKS_PANEL,
            line,
            why: "開閉の矢印が svg（`CHEVRON_*`）になっていない（#1479 / #217）".into(),
        });
    }
    out
}

/// 4: 絞り込みを変える経路は**どれも畳む**
fn scan_filter_collapses(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    for setter in FILTER_SETTERS {
        let hits = code_lines_with(src, setter);
        if hits.is_empty() {
            out.push(Offender {
                file: TASKS_PANEL,
                line: 0,
                why: format!("`{setter}` が 1 つも無い（走査が空振り）"),
            });
        }
        for at in hits {
            // 同じハンドラの中で畳んでいること（直後の数行を見る）
            let tail = lines
                .iter()
                .skip(at + 1)
                .take(4)
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            if !tail.contains("collapse()") {
                out.push(Offender {
                    file: TASKS_PANEL,
                    line: at + 1,
                    why: format!(
                        "`{setter}` のあとで畳んでいない（#1479。見えている集合が\
                         変わるのに展開が残ると、無い行の詳細が出たままになる）"
                    ),
                });
            }
        }
    }
    out
}

/// 5: ポーリングは展開を畳まない（開いた詳細が勝手に閉じない）
fn scan_polling_keeps_expansion(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for needle in ["fn refresh_user_tasks(", "fn tick_user_tasks("] {
        match fn_window(src, needle) {
            None => out.push(Offender {
                file: TASKS_PANEL,
                line: 0,
                why: format!("`{needle}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                for mark in ["collapse()", "expanded = "] {
                    if window.contains(mark) {
                        out.push(Offender {
                            file: TASKS_PANEL,
                            line: at,
                            why: format!(
                                "ポーリング（`{needle}`）が `{mark}` で展開を触っている\
                                 （#1479。2 秒ごとに詳細が閉じる）"
                            ),
                        });
                    }
                }
            }
        }
    }
    out
}

/// 6: バッジは幅を見て決め、縮むのはラベル側（#1479 B）
fn scan_badge(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = fn_window(src, "fn render_panel(") else {
        return vec![Offender {
            file: RIGHT_PANEL,
            line: 0,
            why: "`render_panel` が見つからない（走査が空振り）".into(),
        }];
    };
    for (mark, why) in [
        (
            "panel_tab_badge(open_tasks)",
            "バッジの文字が `panel_tab_badge` を通っていない（#1479 B。\
             件数をそのまま描くと桁が増えるたびにタブ列が伸びて数字が切れる）",
        ),
        (
            "panel_tab_density(self.panel_width",
            "詰め方を幅から決めていない（#1479 B。既定の padding で並べると\
             320px でタブ列が溢れてバッジが切れる）",
        ),
    ] {
        if !window.contains(mark) {
            out.push(Offender {
                file: RIGHT_PANEL,
                line: at,
                why: why.to_string(),
            });
        }
    }
    if window.contains("open_tasks.to_string()") {
        out.push(Offender {
            file: RIGHT_PANEL,
            line: code_line_with(&window, "open_tasks.to_string()").map_or(at, |i| at + i),
            why: "件数を直に描いている（#1479 B。`panel_tab_badge` の 1 実装を通す）".into(),
        });
    }
    // バッジの帯は**絶対に縮まない**（縮めば数字が欠ける）
    match code_line_with(&window, ".when_some(badge_text") {
        None => out.push(Offender {
            file: RIGHT_PANEL,
            line: at,
            why: "バッジを描く枝（`when_some(badge_text…)`）が無い（走査が空振り）".into(),
        }),
        Some(rel) => {
            let block = window
                .lines()
                .skip(rel)
                .take(14)
                .collect::<Vec<_>>()
                .join("\n");
            if !block.contains(".flex_none()") {
                out.push(Offender {
                    file: RIGHT_PANEL,
                    line: at + rel,
                    why: "バッジが `flex_none` でない（#1479 B。taffy が縮めると\
                          数字の一部だけが見える = 22 が 2 に読める）"
                        .into(),
                });
            }
        }
    }
    // ラベル側が縮める設定になっている（belt。見積りが甘くても数字は欠けない）
    if !window.contains("min_w(px(0.0))") {
        out.push(Offender {
            file: RIGHT_PANEL,
            line: at,
            why: "タブのラベルが `min_w(0)` で縮まない（#1479 B。min-content で\
                  突っ張るとタブ列がそのまま溢れ、末尾のバッジが切れる）"
                .into(),
        });
    }
    out
}

fn scan_all(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    out.extend(scan_inline_detail(&sources.tasks_panel));
    out.extend(scan_no_fallback(&sources.tasks_panel));
    out.extend(scan_row_toggle(&sources.tasks_panel));
    out.extend(scan_filter_collapses(&sources.tasks_panel));
    out.extend(scan_polling_keeps_expansion(&sources.tasks_panel));
    out.extend(scan_badge(&sources.right_panel));
    out
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[test]
fn アコーディオンとバッジの構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1479 の構造が崩れている:\n{}",
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
    let sources = Sources::load(&root);
    for (label, src, needle) in [
        ("ビュー本体", &sources.tasks_panel, "fn render_tasks_view("),
        ("展開の解決", &sources.tasks_panel, "fn expanded_task<"),
        ("畳む", &sources.tasks_panel, "fn collapse(&mut self)"),
        ("トグル", &sources.tasks_panel, "fn toggle(&mut self"),
        ("詳細", &sources.tasks_panel, "fn render_task_detail("),
        ("タブ列", &sources.right_panel, "fn render_panel("),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{label}（{needle}）の窓が採れない = 走査が壊れている"));
        assert!(at > 0, "{label} の行番号が 0");
        assert!(
            window.lines().count() > 4,
            "{label} の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
    // 本番コードが潰れていない（範囲取りの事故で走査対象が消えていない）
    assert!(
        sources.tasks_panel.lines().count() > 400,
        "{TASKS_PANEL} の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        sources.tasks_panel.lines().count()
    );
    assert!(
        sources.right_panel.lines().count() > 400,
        "{RIGHT_PANEL} の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        sources.right_panel.lines().count()
    );
    // 絞り込みの代入が実在する（`FILTER_SETTERS` の綴りが古くなっていない）
    for setter in FILTER_SETTERS {
        assert!(
            !code_lines_with(&sources.tasks_panel, setter).is_empty(),
            "{TASKS_PANEL}: `{setter}` が無い（走査が空振り）"
        );
    }
}

/// A/B の逃げ道は **Issue ごとに別の env**（#1422 の規約）。
/// B2（`TAKO_1450B2_LEGACY` = ポーリングの抑止）と同じ env を読むと、
/// 片方のアームがもう片方の回帰を隠す
#[test]
fn abの逃げ道は1479専用のenvを読む() {
    let root = workspace_root();
    let src = read(&root, RIGHT_PANEL);
    assert!(
        src.contains("TAKO_1479_LEGACY"),
        "{RIGHT_PANEL}: #1479 の A/B（`TAKO_1479_LEGACY`）が無い"
    );
    let (_, window) = fn_window(&src, "fn legacy_1479(").expect("`legacy_1479` が無い");
    for other in ["TAKO_1450_LEGACY", "TAKO_1450B2_LEGACY", "TAKO_1472_LEGACY"] {
        assert!(
            !window.contains(other),
            "{RIGHT_PANEL}: #1479 の A/B が `{other}` を読んでいる（軸が混ざると、\
             片方のアームがもう片方の回帰を隠す。#1422）"
        );
    }
}

/// **修正前を再現した注入**が `file:line` で名指しされることを確かめる。
/// 検出力の実証がここ（緑を作るのは簡単だが、落とせることの証明は注入でしかできない）
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) 詳細を列の末尾へ積む（直す前の形）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "                item = item.child(self.render_task_detail(task, cx));",
        "                let _ = &task;",
    );
    s.tasks_panel = s.tasks_panel.replace(
        "        root.child(scroll)",
        "        if let Some(task) = expanded.as_ref() {\n\
         \x20           scroll = scroll.child(self.render_task_detail(task, cx));\n\
         \x20       }\n        root.child(scroll)",
    );
    expect_hit(&s, TASKS_PANEL, "詳細を一覧の**末尾**へ積んでいる");

    // (2) 詳細を描く場所が 2 か所になる
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "        root.child(scroll)",
        "        let _ = self.render_task_detail(task, cx);\n        root.child(scroll)",
    );
    expect_hit(&s, TASKS_PANEL, "詳細を描く呼び出しが 2 か所ある");

    // (3) 展開の解決が先頭へ落ちる（畳めなくなる）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "            .and_then(|id| visible.iter().find(|t| &t.id == id).copied())\n    }",
        "            .and_then(|id| visible.iter().find(|t| &t.id == id).copied())\n\
         \x20           .or_else(|| visible.first().copied())\n    }",
    );
    expect_hit(&s, TASKS_PANEL, "先頭へ落ちている");

    // (4) 行の押下がトグルを通らない（開くだけになる = 閉じられない）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "if this.user_tasks.toggle(&id) {",
        "if { this.user_tasks.expanded = Some(id.clone()); true } {",
    );
    expect_hit(&s, TASKS_PANEL, "`toggle` を通っていない");

    // (5) 押下が一括 dismiss に食われる（#496 の型）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tasks_panel, "fn render_tasks_view(").expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &window,
        &window
            .replace(
                ".on_mouse_down(\n                    MouseButton::Left,\n                    cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),\n                )",
                "",
            )
            .replace("cx.stop_propagation();", ""),
    );
    expect_hit(&s, TASKS_PANEL, "`on_mouse_down` で伝播を止めていない");

    // (6) 展開した行を見える位置へ運ばない
    let mut s = Sources::load(&root);
    s.tasks_panel = s
        .tasks_panel
        .replace("this.user_tasks.scroll.scroll_to_item(i);", "");
    expect_hit(&s, TASKS_PANEL, "見える位置へ運んでいない");

    // (7) 絞り込みを変えたのに畳まない（無い行の詳細が残る）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "                        this.user_tasks.kind_filter = Some(k.clone());\n                        this.user_tasks.collapse();",
        "                        this.user_tasks.kind_filter = Some(k.clone());",
    );
    expect_hit(&s, TASKS_PANEL, "のあとで畳んでいない");

    // (8) ポーリングが展開を畳む（2 秒ごとに詳細が閉じる）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "    pub(crate) fn refresh_user_tasks(&mut self) {",
        "    pub(crate) fn refresh_user_tasks(&mut self) {\n        self.user_tasks.collapse();",
    );
    expect_hit(&s, TASKS_PANEL, "展開を触っている");

    // (9) バッジの幅を見ずに既定 padding で並べる（直す前 = 320px で切れる）
    let mut s = Sources::load(&root);
    s.right_panel = s.right_panel.replace(
        "            panel_tab_density(self.panel_width, badge_text.as_deref())",
        "            PanelTabDensity::Full",
    );
    expect_hit(&s, RIGHT_PANEL, "詰め方を幅から決めていない");

    // (10) バッジが縮む（数字の一部だけが見える）
    let mut s = Sources::load(&root);
    s.right_panel = s.right_panel.replace(
        "                                        // **絶対に縮まない**（縮めば数字が欠ける = #1479）\n                                        .flex_none()\n",
        "",
    );
    expect_hit(&s, RIGHT_PANEL, "`flex_none` でない");

    // (11) 件数を直に描く（桁が増えるたびに列が伸びる）
    let mut s = Sources::load(&root);
    s.right_panel = s.right_panel.replace(
        "        let badge_text = panel_tab_badge(open_tasks);",
        "        let badge_text = Some(open_tasks.to_string());",
    );
    expect_hit(&s, RIGHT_PANEL, "`panel_tab_badge` を通っていない");
}

/// 注入した眺めが、その file と文言で名指しされること
fn expect_hit(sources: &Sources, file: &str, fragment: &str) {
    let offenders = scan_all(sources);
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(fragment)),
        "注入（{file} / {fragment}）を検出できていない。検出できたのは:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
    // 名指しは `file:line` で追える形であること（行番号 0 は追えない）
    for o in offenders
        .iter()
        .filter(|o| o.file == file && o.why.contains(fragment))
    {
        assert!(
            o.line > 0,
            "行番号が 0 のまま名指ししている: {}",
            o.report()
        );
    }
}
