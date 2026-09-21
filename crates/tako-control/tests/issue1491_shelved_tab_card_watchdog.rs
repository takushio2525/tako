//! 退避タブが「タブ形のカード 1 枚」で在り続けることを縛る番犬（Issue #1491）
//!
//! # なぜ要るか
//!
//! ユーザー原文は「タブごと復帰ボタンにするんじゃなく、カードごとタブにして欲しい」。
//! #1489 の形（10px の見出し + 隅の小さな「タブごと復帰」テキストボタン + 配下ペインの
//! サムネイルカード N 枚）は**動いては見える**（押せば戻る）ので目視では気づけないが、
//! ユーザーには「ペインがバラバラに並んでいて、隅の小さなボタンを探して押す」になる。
//! 直したあと同じ形へ戻る道が 6 つあるので、ここで塞ぐ:
//!
//! 1. 「タブごと復帰」テキストボタン（`restore_tab()` / `op_unshelve_tab()`）が
//!    本番の描画へ戻る（A/B の腕 `TAKO_1491_LEGACY_ARM` の中だけは正しいので除外する）
//! 2. カード本体の `on_click`（= どこを押しても復帰）が消えて、押せるのが隅のボタンだけに戻る
//! 3. 退避タブが `bg_groups` へ積まれて**配下ペインのサムネイルカード列**が復活する
//! 4. ドロワーと右パネルがそれぞれ自前でカードを描き始める（形が 2 つに割れる）
//! 5. タブ形の描画がタブバーと分かれる（タブバーを触ったとき片方だけ取り残される）
//! 6. × が 2 段確認を飛ばす / `stop_propagation` を失って**押した瞬間に復帰**する
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので、[`走査が空振りしていない`] で対象の窓が
//! 採れていることを固定し、[`逆戻りを名指しできる`] で**注入 10 通り**が `file:line` で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DRAWER: &str = "crates/tako-app/src/drawer.rs";
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";
const TAB_SHAPE: &str = "crates/tako-app/src/tab_shape.rs";
const TAB_BAR: &str = "crates/tako-app/src/tab_bar.rs";

/// A/B の旧経路アーム（`TAKO_1491_LEGACY=1` = #1489 の形）の囲み
const ARM_BEGIN: &str = "TAKO_1491_LEGACY_ARM 開始";
const ARM_END: &str = "TAKO_1491_LEGACY_ARM 終了";

/// タブ形の語彙のうち、**タブバーと退避タブカードが必ず共用する**もの。
/// ここが片方だけ手書きへ戻ると「同じタブなのに 2 つの見た目」になる
const SHARED_SHAPE: &[(&str, &str)] = &[
    (
        "crate::tab_shape::tab_pill_shell(",
        "タブピルの器（高さ・角丸・余白・フォント）",
    ),
    ("crate::tab_shape::tab_state_dot(", "状態ドット"),
    ("crate::tab_shape::tab_state_color(", "状態ドットの色"),
    (
        "crate::tab_shape::tab_pill_badge(",
        "タブピルに載る小バッジ",
    ),
    (
        "crate::tab_shape::tab_pill_slot(",
        "タブピル右端のボタン 1 個ぶんの器",
    ),
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

/// A/B の旧経路アームを**空行で潰した**眺め（行番号は保つので `file:line` が狂わない）
fn without_legacy_arm(src: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in src.lines() {
        if line.contains(ARM_BEGIN) {
            inside = true;
        }
        if !inside && !line.contains(ARM_END) {
            out.push_str(line);
        }
        if line.contains(ARM_END) {
            inside = false;
        }
        out.push('\n');
    }
    out
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

/// `needle` を含む最初のコード行（1-based。コメント行は見ない）
fn code_line_with(src: &str, needle: &str) -> Option<usize> {
    src.lines()
        .enumerate()
        .find(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, _)| i + 1)
}

/// コード行に `needle` が現れるか（コメントに書いてあるだけでは通さない）
fn has_code(src: &str, needle: &str) -> bool {
    code_line_with(src, needle).is_some()
}

struct Sources {
    drawer: String,
    right_panel: String,
    tab_shape: String,
    tab_bar: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            drawer: prod(root, DRAWER),
            right_panel: prod(root, RIGHT_PANEL),
            tab_shape: prod(root, TAB_SHAPE),
            tab_bar: prod(root, TAB_BAR),
        }
    }

    /// 本番の描画（A/B の旧経路アームを除いたもの）
    fn live_drawer(&self) -> String {
        without_legacy_arm(&self.drawer)
    }
    fn live_right_panel(&self) -> String {
        without_legacy_arm(&self.right_panel)
    }
}

/// 1: 「タブごと復帰」テキストボタンが本番の描画に居ない（A/B の腕は除外）
fn scan_no_restore_button(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let drawer = s.live_drawer();
    if let Some(line) = code_line_with(&drawer, "ui_text::drawer::restore_tab()") {
        out.push(Offender {
            file: DRAWER,
            line,
            why: "たまり場の本番描画が「タブごと復帰」テキストボタンを出している（#1491。\
                  カードのどこを押しても復帰するので、隅の小さなボタンは要らない）"
                .into(),
        });
    }
    if let Some(line) = code_line_with(&drawer, "ui_text::drawer::shelved_tab_group(") {
        out.push(Offender {
            file: DRAWER,
            line,
            why: "たまり場の本番描画が退避タブの**見出し行**を出している（#1491。\
                  タブ名・ペイン数はカード自身が持つ）"
                .into(),
        });
    }
    let panel = s.live_right_panel();
    if let Some(line) = code_line_with(&panel, "ui_text::panel::op_unshelve_tab()") {
        out.push(Offender {
            file: RIGHT_PANEL,
            line,
            why: "右パネルの本番描画が「タブごと復帰」テキストボタンを出している（#1491）".into(),
        });
    }
    out
}

/// 2: カード本体を押せば復帰する（新しい経路は作らない = 既存の `unshelve_tab_clicked`）
fn scan_card_is_clickable(s: &Sources) -> Vec<Offender> {
    let Some((at, window)) = fn_window(&s.tab_shape, "fn render_shelved_tab_card(") else {
        return vec![Offender {
            file: TAB_SHAPE,
            line: 0,
            why: "`render_shelved_tab_card` が無い（#1491。タブ形カードの 1 実装が消えた）".into(),
        }];
    };
    let mut out = Vec::new();
    if !has_code(&window, ".on_click(cx.listener(move |this, _, _, cx| {\n")
        && !window.contains("this.unshelve_tab_clicked(tab_id, cx);")
    {
        out.push(Offender {
            file: TAB_SHAPE,
            line: at,
            why: "カード本体に復帰の `on_click` が無い（#1491。ユーザー原文\
                  「カードごとタブにして欲しい」= どこを押しても戻る）"
                .into(),
        });
    }
    if !window.contains("this.unshelve_tab_clicked(tab_id, cx);") {
        out.push(Offender {
            file: TAB_SHAPE,
            line: at,
            why: "復帰が既存の `unshelve_tab_clicked`（= `Request::Foreground { tab }` と\
                  同じ 1 経路）を通っていない（#1491。別経路を作ると CLI / MCP と挙動が割れる）"
                .into(),
        });
    }
    // × は 2 段確認 + `stop_propagation`（本体クリックと分ける）
    if !window.contains("this.bg_pending_kill_tab = Some(tab_id);") {
        out.push(Offender {
            file: TAB_SHAPE,
            line: at,
            why: "× が 2 段確認の 1 段目（`bg_pending_kill_tab`）を通っていない（#1491。\
                  押した瞬間に配下ペインが死ぬ）"
                .into(),
        });
    }
    if !window.contains("this.kill_shelved_tab_clicked(tab_id, cx);") {
        out.push(Offender {
            file: TAB_SHAPE,
            line: at,
            why: "× の確定がタブごと kill の 1 実装（`kill_shelved_tab_clicked`）を\
                  通っていない（#1491）"
                .into(),
        });
    }
    let kill_arm = window.split("kill_id, key").nth(1).unwrap_or("");
    if !kill_arm.contains("cx.stop_propagation();") {
        out.push(Offender {
            file: TAB_SHAPE,
            line: at,
            why: "× が `stop_propagation` していない（#1491。× を押すと本体の\
                  on_click も走って**確認を出しながら復帰する**）"
                .into(),
        });
    }
    // 合成マウスで押すための実矩形（ハンドラ直呼びでは #496 型を検出できない）
    if !window.contains("-shelved-tab-{key}") {
        out.push(Offender {
            file: TAB_SHAPE,
            line: at,
            why: "カード本体に実矩形（probe）が無い（#1491。visual-test 153 が\
                  合成マウスでカード中央を押せない）"
                .into(),
        });
    }
    out
}

/// 3: 退避タブは 1 枚のカード（配下ペインのサムネイルカード列を積まない）
fn scan_no_pane_cards(s: &Sources) -> Vec<Offender> {
    let drawer = s.live_drawer();
    let mut out = Vec::new();
    if let Some(line) = code_line_with(&drawer, "Some(group.tab),") {
        out.push(Offender {
            file: DRAWER,
            line,
            why: "退避タブを `bg_groups` へ積んでいる（#1491。配下ペインのサムネイル\
                  カードが N 枚並ぶ #1489 の形へ逆戻り）"
                .into(),
        });
    }
    out
}

/// 4: ドロワーと右パネルが**同じ 1 実装**のカードを描く
fn scan_one_card_impl(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src) in [
        (DRAWER, s.live_drawer()),
        (RIGHT_PANEL, s.live_right_panel()),
    ] {
        if !has_code(&src, "render_shelved_tab_card(") {
            out.push(Offender {
                file,
                line: 0,
                why: "`render_shelved_tab_card` を通っていない（#1491。置き場ごとに\
                      カードを描くと、ドロワーと右パネルで形が割れる）"
                    .into(),
            });
        }
    }
    // 右パネルは配下ペイン行を畳んで ▸ で開く
    let panel = s.live_right_panel();
    if !has_code(&panel, "shelved_tab_expanded.contains(") {
        out.push(Offender {
            file: RIGHT_PANEL,
            line: 0,
            why: "退避タブの配下ペイン行が畳まれていない（#1491。カードの下に畳んで ▸ で開く）"
                .into(),
        });
    }
    if !has_code(&panel, "ui_icon::CHEVRON_RIGHT") {
        out.push(Offender {
            file: RIGHT_PANEL,
            line: 0,
            why: "開閉の矢印が既存の `CHEVRON_*` 定数でない（#1491 / UI 絵文字ゼロの規約）".into(),
        });
    }
    out
}

/// 5: タブ形の描画をタブバーと共用している（見た目が割れない）
fn scan_shared_shape(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (needle, label) in SHARED_SHAPE {
        if has_code(&s.tab_bar, needle) {
            continue;
        }
        out.push(Offender {
            file: TAB_BAR,
            line: 0,
            why: format!(
                "{label}を `tab_shape` から取らず手書きしている（#1491。タブバーを\
                 触ったときにたまり場のタブ形カードだけ取り残される）"
            ),
        });
    }
    out
}

/// 6: A/B の口（同一バイナリで #1489 の形を再現でき、env を読むのは 1 か所）
fn scan_ab(s: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    if !has_code(&s.tab_shape, "TAKO_1491_LEGACY") {
        out.push(Offender {
            file: TAB_SHAPE,
            line: 0,
            why: "`TAKO_1491_LEGACY` を読む場所が 0 か所（A/B で #1489 の形を再現できない）".into(),
        });
    }
    for (file, src) in [(DRAWER, &s.drawer), (RIGHT_PANEL, &s.right_panel)] {
        if let Some(line) = code_line_with(src, "std::env::var(\"TAKO_1491_LEGACY\")") {
            out.push(Offender {
                file,
                line,
                why: "A/B の env を描画側で直接読んでいる（#1491。読むのは `tab_shape` の\
                      1 か所。2 か所に増えると片方だけ legacy に取り残される）"
                    .into(),
            });
        }
    }
    out
}

fn scan_all(s: &Sources) -> Vec<Offender> {
    let mut out = scan_no_restore_button(s);
    out.extend(scan_card_is_clickable(s));
    out.extend(scan_no_pane_cards(s));
    out.extend(scan_one_card_impl(s));
    out.extend(scan_shared_shape(s));
    out.extend(scan_ab(s));
    out
}

/// 本体: 現行 main に逆戻りが無い
#[test]
fn 退避タブがタブ形のカード1枚で在り続けている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1491 の逆戻り:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査が空振りしていない（窓が採れないと全部が無意味に緑になる）
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let s = Sources::load(&root);
    for (label, src, needle) in [
        ("タブ形カード", &s.tab_shape, "fn render_shelved_tab_card("),
        ("タブピルの器", &s.tab_shape, "fn tab_pill_shell("),
        ("たまり場", &s.drawer, "fn render_drawer("),
        ("ペインカード", &s.drawer, "fn render_shelf_card("),
        ("タブバー", &s.tab_bar, "fn render_tab_bar("),
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
    for (file, src, least) in [
        (DRAWER, &s.drawer, 300usize),
        (RIGHT_PANEL, &s.right_panel, 400),
        (TAB_SHAPE, &s.tab_shape, 150),
        (TAB_BAR, &s.tab_bar, 400),
    ] {
        assert!(
            src.lines().count() > least,
            "{file} の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
            src.lines().count()
        );
    }
    // A/B の腕が実在し、潰すと本当に短くなる（囲みの綴りが古くなっていない）
    for (file, src) in [(DRAWER, &s.drawer), (RIGHT_PANEL, &s.right_panel)] {
        let live = without_legacy_arm(src);
        let dropped = src
            .lines()
            .zip(live.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            dropped > 5,
            "{file}: `{ARM_BEGIN}` の囲みが見つからない（A/B の腕を除外できていない = \
             legacy のボタンを本番と取り違える）"
        );
    }
}

/// 注入した逆戻りを `file:line` で名指しできる（見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    let expect_hit = |s: &Sources, file: &str, needle: &str| {
        let offenders = scan_all(s);
        let hit = offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(needle));
        assert!(
            hit,
            "注入した逆戻り（{file} / {needle}）を名指しできていない:\n{}",
            offenders
                .iter()
                .map(Offender::report)
                .collect::<Vec<_>>()
                .join("\n")
        );
    };

    // (1) たまり場の本番描画に「タブごと復帰」ボタンが戻る
    let mut s = Sources::load(&root);
    s.drawer = s.drawer.replace(
        "        if bg_groups.is_empty() && !has_tab_cards {",
        "        let _ = crate::ui_text::drawer::restore_tab();\n\
         \x20       if bg_groups.is_empty() && !has_tab_cards {",
    );
    expect_hit(&s, DRAWER, "テキストボタンを出している");

    // (2) たまり場の本番描画に見出し行が戻る
    let mut s = Sources::load(&root);
    s.drawer = s.drawer.replace(
        "        if bg_groups.is_empty() && !has_tab_cards {",
        "        let _ = crate::ui_text::drawer::shelved_tab_group(\"x\", 1);\n\
         \x20       if bg_groups.is_empty() && !has_tab_cards {",
    );
    expect_hit(&s, DRAWER, "見出し行");

    // (3) 右パネルの本番描画に「タブごと復帰」ボタンが戻る
    let mut s = Sources::load(&root);
    s.right_panel = s.right_panel.replace(
        "            let mut head = div().flex().flex_row().items_center().gap_1();",
        "            let mut head = div().flex().flex_row().items_center().gap_1();\n\
         \x20           head = head.child(crate::ui_text::panel::op_unshelve_tab());",
    );
    expect_hit(&s, RIGHT_PANEL, "テキストボタンを出している");

    // (4) カード本体の復帰クリックが消える（押せるのが隅のボタンだけに戻る）
    let mut s = Sources::load(&root);
    s.tab_shape = s
        .tab_shape
        .replace("this.unshelve_tab_clicked(tab_id, cx);", "let _ = tab_id;");
    expect_hit(&s, TAB_SHAPE, "同じ 1 経路");

    // (5) × が 2 段確認を飛ばす（押した瞬間に配下ペインが死ぬ）
    let mut s = Sources::load(&root);
    s.tab_shape = s.tab_shape.replace(
        "this.bg_pending_kill_tab = Some(tab_id);",
        "this.kill_shelved_tab_clicked(tab_id, cx);",
    );
    expect_hit(&s, TAB_SHAPE, "2 段確認の 1 段目");

    // (6) × が伝播を止めない（確認を出しながら復帰する）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tab_shape, "fn render_shelved_tab_card(").expect("窓");
    let kill_head = window.split("kill_id, key").nth(1).unwrap_or("");
    s.tab_shape = s.tab_shape.replace(
        kill_head,
        &kill_head.replacen("cx.stop_propagation();", "", 1),
    );
    expect_hit(&s, TAB_SHAPE, "stop_propagation");

    // (7) カードの実矩形が消える（合成マウスで押せない = #496 型を検出できない）
    let mut s = Sources::load(&root);
    s.tab_shape = s
        .tab_shape
        .replace("-shelved-tab-{key}", "-shelved-tab-gone");
    expect_hit(&s, TAB_SHAPE, "実矩形");

    // (8) 退避タブが `bg_groups` へ積まれる（サムネイルカード列の復活）
    let mut s = Sources::load(&root);
    s.drawer = s.drawer.replace(
        "            // TAKO_1491_LEGACY_ARM 開始（#1491 前の形。番犬はここを対象外にする）",
        "",
    );
    s.drawer = s
        .drawer
        .replace("            // TAKO_1491_LEGACY_ARM 終了", "");
    expect_hit(&s, DRAWER, "サムネイル");

    // (9) 右パネルが自前でカードを描く（形が 2 つに割れる）
    let mut s = Sources::load(&root);
    s.right_panel = s.right_panel.replace(
        "                    .child(self.render_shelved_tab_card(",
        "                    .child(self.render_background_row_stub(",
    );
    expect_hit(&s, RIGHT_PANEL, "形が割れる");

    // (10) タブバーがタブ形を手書きへ戻す
    let mut s = Sources::load(&root);
    s.tab_bar = s
        .tab_bar
        .replace("crate::tab_shape::tab_pill_shell(", "div_pill_shell(");
    expect_hit(&s, TAB_BAR, "手書き");

    // (11) A/B の env を描画側で直接読む（2 か所に増える）
    let mut s = Sources::load(&root);
    s.drawer = s.drawer.replace(
        "let legacy1491 = crate::tab_shape::shelved_tab_card_legacy();",
        "let legacy1491 = std::env::var(\"TAKO_1491_LEGACY\").as_deref() == Ok(\"1\");",
    );
    expect_hit(&s, DRAWER, "描画側で直接読んで");
}
