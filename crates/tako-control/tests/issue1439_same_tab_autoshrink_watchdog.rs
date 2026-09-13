//! worker を別タブへ逃がさず、同じタブでフォントを縮めて桁数を確保する番犬（#1439）
//!
//! # なぜ要るか
//!
//! #1132 は下限桁数（既定 60 桁）を割る spawn を**別のタブ**へ逃がしていた。
//! 狭いペインで claude TUI がハード折り返しして検知が壊れる問題（#1123）には
//! 効いていたが、「1 グループ = 1 タブで集約監視する」という tako のコンセプトを
//! 壊す（master のタブから worker が見えない）。実測でも 8 体 spawn すると
//! **タブが 8 枚**に散った。
//!
//! #1439 では必ず同じタブに置き、足りない桁数は worker ペインのフォントを
//! 縮めて確保する。逆戻りは 4 つの形で起こりうるので、静的に縛る:
//!
//! 1. 配置の分岐（`plan_worker_placement`）が既定の腕で `new_tab` / `overflow_tab`
//!    へ落ちる（= タブが増える）
//! 2. 当て直し（`worker_font::refit_worker_area`）が**床を見ない** / 領域の 1 枚しか
//!    見ない（= 読めないほど縮む / 既存 worker が割ったまま残る）
//! 3. spawn 応答から `font_scale` / `font_size` / `cols_short` が落ちる
//!    （= 縮んだことも届かなかったことも master に分からない = 無言の縮小）
//! 4. close 後のリフローで当て直さない（= 幅が戻ってもフォントが小さいまま）
//! 5. GUI 側で手のズーム（cmd +/-）を自動縮小が踏み潰す
//! 6. セルフテスト項目 72b が「同じタブ」「床 + cols_short」を**見ない**形に戻る
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜6 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で窓が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**修正前を再現した 7 通りの注入**が file:line で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装を通す

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const WORKER_FONT: &str = "crates/tako-control/src/worker_font.rs";
const APP: &str = "crates/tako-app/src/main.rs";

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

/// 本番コードだけの眺め（テスト領域は空白へ潰れる。行番号は原文のまま）
fn production(rel: &'static str, src: &str) -> String {
    production_range::production_with_floor(src, rel, 0.2)
}

/// 1 つの違反（`ファイル:行 — 理由`）
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

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（自由関数 = 0 桁 / メソッド = 4 桁 / 入れ子 = 8 桁）
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

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// 配置の分岐と spawn 応答（`dispatch.rs`）
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let src = production(DISPATCH, src);
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };

    // 1. 既定の腕は `new_tab` / `overflow_tab` へ落ちない
    match fn_window(&src, "fn plan_worker_placement(") {
        None => push(
            0,
            "`plan_worker_placement` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let same = code_line_with(&window, "arm == PlacementArm::SameTab");
            let escape = code_line_with(&window, "\"new_tab\"")
                .or_else(|| code_line_with(&window, "\"overflow_tab\""));
            match (same, escape) {
                (None, Some(i)) => push(
                    at + i,
                    "既定の腕（SameTab）で打ち切らずに別タブの配置へ落ちる。\
                     worker は必ず spawn 元と同じタブへ置くこと（#1439）"
                        .into(),
                ),
                (Some(s), Some(e)) if s > e => push(
                    at + e,
                    format!(
                        "別タブの配置（{} 行目）が既定の腕の打ち切り（{} 行目）より前にある（#1439）",
                        at + e,
                        at + s
                    ),
                ),
                _ => {}
            }
            if !window.contains("arm: PlacementArm") {
                push(
                    at,
                    "腕を引数で受けていない（env 直読みだと 3 腕をテストから回せない。#1439）"
                        .into(),
                );
            }
        }
    }

    // 2. 腕の判断は 1 実装（`worker_font::PlacementArm`）から取る
    if !src.contains("use crate::worker_font::PlacementArm;") {
        push(
            0,
            "配置の腕を `worker_font::PlacementArm` から取っていない（配置とフォントで\
             腕がズレると、A/B の対照が片方だけ効く。#1439）"
                .into(),
        );
    }
    for needle in [
        "var_os(\"TAKO_1439_LEGACY\")",
        "var_os(\"TAKO_1132_LEGACY\")",
    ] {
        if let Some(i) = src.lines().position(|l| l.contains(needle)) {
            push(
                i + 1,
                format!(
                    "`{needle}` を dispatch で直に読んでいる（腕の判断は worker_font の 1 実装。#1439）"
                ),
            );
        }
    }

    // 3. spawn 応答に縮小の結果が載る
    match fn_window(&src, "fn dispatch_orchestrator_spawn(") {
        None => push(
            0,
            "`dispatch_orchestrator_spawn` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            for (key, why) in [
                ("\"font_scale\"", "適用したフォント倍率"),
                ("\"font_size\"", "適用した絶対フォントサイズ"),
                ("\"cols_short\"", "床でも下限に届かなかったこと"),
            ] {
                if !window.contains(key) {
                    push(
                        at,
                        format!("spawn 応答に {key} が無い（{why}が master に届かない = 無言の縮小。#1439）"),
                    );
                }
            }
            // 桁数は**当て直した後の実測**を優先する（見積もりのままだと嘘になる）
            if let Some(i) = code_line_with(&window, "\"pane_cols\":") {
                let line = window.lines().nth(i).unwrap_or_default();
                if !line.contains("placement_cols") {
                    push(
                        at + i,
                        "`pane_cols` が既定フォントでの見積もりのまま（縮めた後の実桁数を載せること。#1439）"
                            .into(),
                    );
                }
            }
            if code_line_with(&window, "worker_font::refit_worker_area(").is_none() {
                push(
                    at,
                    "spawn 後に worker 領域のフォントを当て直していない（#1439）".into(),
                );
            }
        }
    }

    // 4. close 後のリフローでも当て直す
    match fn_window(&src, "Request::Close {") {
        None => push(
            0,
            "`Request::Close` の腕が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let reflow = code_line_with(&window, "reflow_workers(");
            let refit = code_line_with(&window, "worker_font::refit_worker_area(");
            match (reflow, refit) {
                (Some(r), None) => push(
                    at + r,
                    "close 後のリフローでフォントを当て直していない（幅が戻っても小さいまま。#1439）"
                        .into(),
                ),
                (Some(r), Some(f)) if f < r => push(
                    at + f,
                    "リフローより前に当て直している（当てる材料が古い矩形になる。#1439）".into(),
                ),
                _ => {}
            }
        }
    }
    out
}

/// 当て直しの 1 実装（`worker_font.rs`）
fn scan_worker_font(src: &str) -> Vec<Offender> {
    let src = production(WORKER_FONT, src);
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: WORKER_FONT,
            line,
            why,
        })
    };
    match fn_window(&src, "pub fn refit_worker_area(") {
        None => push(
            0,
            "`refit_worker_area` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            // 床は設定から採る（ハードコードすると設定が効かない・読めないほど縮む）
            match code_line_with(&window, "fit_worker_font(") {
                None => push(
                    at,
                    "`fit_worker_font` を通していない（段の選び方が二重実装）".into(),
                ),
                Some(i) => {
                    let code = code_only(&window);
                    if !code.contains("let floor = layout.min_worker_font_scale;")
                        && !code.contains("layout.min_worker_font_scale")
                    {
                        push(
                            at + i,
                            "当てはめの床が設定（`min_worker_font_scale`）から来ていない（#1439）"
                                .into(),
                        );
                    }
                }
            }
            // 領域まるごと当て直す（grid は 1 体足すと既存の列も細くなる）
            if code_line_with(&window, "worker_area_shares(").is_none() {
                push(
                    at,
                    "worker 領域まるごとを当て直していない（新しいペインだけ縮めても\
                     既存 worker が下限を割ったまま残る。#1439）"
                        .into(),
                );
            }
            // 自動縮小が無効なら当てていたぶんを外す
            let code = code_only(&window);
            if !code.contains("set_worker_font_scale(*pane, None)") {
                push(
                    at,
                    "自動縮小を切ったときに縮小を外していない（設定を戻しても見た目が戻らない。#1439）"
                        .into(),
                );
            }
        }
    }
    // `min_worker_cols == 0` は「保証しない」= 縮めない。A/B の対照でも縮めない
    match fn_window(&src, "pub fn enabled(") {
        None => push(0, "`enabled` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("min_worker_cols > 0") {
                push(
                    at,
                    "`min_worker_cols == 0`（保証しない）でも縮めてしまう（#1132 の約束を破る）"
                        .into(),
                );
            }
            if !code.contains("PlacementArm::from_env()") {
                push(
                    at,
                    "A/B の腕を見ていない（対照（#1132 / #1132 前）でも縮んでしまい、\
                     旧挙動を再現できない = A/B が互いの回帰を隠す。#1439）"
                        .into(),
                );
            }
        }
    }
    // 腕は Issue ごとに 1 対 1 の env を読む
    for (func, env) in [
        ("fn legacy_split_tabs()", "TAKO_1439_LEGACY"),
        ("fn legacy_worker_min_width()", "TAKO_1132_LEGACY"),
    ] {
        match fn_window(&src, func) {
            None => push(
                0,
                format!("`{func}` が見つからない（A/B の逃げ道が消えた）"),
            ),
            Some((at, window)) => {
                if !window.contains(env) {
                    push(
                        at,
                        format!(
                            "`{func}` が `{env}` を読んでいない（A/B が別 Issue の env と混線する。#1439）"
                        ),
                    );
                }
            }
        }
    }
    // 実際に env を**読む**場所（注釈の言及は数えない）は Issue ごとに 1 か所だけ
    for env in ["TAKO_1439_LEGACY", "TAKO_1132_LEGACY"] {
        let needle = format!("var_os(\"{env}\")");
        let hits = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && l.contains(&needle))
            .count();
        if hits != 1 {
            let line = src
                .lines()
                .position(|l| l.contains(&needle))
                .map(|i| i + 1)
                .unwrap_or(0);
            push(
                line,
                format!("`{env}` を読む場所が {hits} か所ある（A/B の逃げ道は 1 実装。#1439）"),
            );
        }
    }
    out
}

/// GUI 側（`tako-app/src/main.rs`）
fn scan_app(src: &str) -> Vec<Offender> {
    let src = production(APP, src);
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: APP,
            line,
            why,
        })
    };

    // 手のズームを自動縮小が踏み潰さない
    match fn_window(&src, "fn set_worker_font_scale(") {
        None => push(
            0,
            "`set_worker_font_scale` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            // 「手で決めたか」を**読む**こと（印を書くだけでは踏み潰しを防げない）
            if !code.contains("auto_font_panes.contains(") {
                push(
                    at,
                    "自動縮小と手のズームを見分けていない（cmd +/- の指定を踏み潰す。#1439）"
                        .into(),
                );
            }
            if !code.contains("pane_cell_sizes.remove(") {
                push(
                    at,
                    "セル寸法を捨てていない（フォントを変えても grid / PTY が古いまま。#1439）"
                        .into(),
                );
            }
        }
    }
    match fn_window(&src, "fn zoom_focused_pane(") {
        None => push(
            0,
            "`zoom_focused_pane` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !code_only(&window).contains("auto_font_panes.remove(") {
                push(
                    at,
                    "手でズームしても自動縮小の印が残る（次の当て直しで上書きされる。#1439）"
                        .into(),
                );
            }
        }
    }
    // 段は tako-core の 1 実装から作る
    match fn_window(&src, "fn refresh_font_scale_cells(") {
        None => push(
            0,
            "`refresh_font_scale_cells` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window.contains("worker_font_scale_steps(") {
                push(
                    at,
                    "倍率の段を自前で作っている（tako-core と食い違うと当てはめが空振りする。#1439）"
                        .into(),
                );
            }
            if !code_only(&window).contains("pane_font_size_for_scale(") {
                push(
                    at,
                    "段のセル幅を**実際に当てる絶対サイズ**で測っていない（8pt クランプを見落とす。#1439）"
                        .into(),
                );
            }
        }
    }
    // セルフテスト項目 72b が「同じタブ」と「床 + cols_short」を見ている
    let item = src
        .split_once("// 72b. #1439:")
        .map(|(head, rest)| (head.lines().count() + 1, rest));
    match item {
        None => push(
            0,
            "セルフテスト項目 72b（#1439）が見つからない（走査が空振り）".into(),
        ),
        Some((at, rest)) => {
            let window: String = rest.lines().take(220).collect::<Vec<_>>().join("\n");
            for (needle, why) in [
                (
                    "placement == \"same_tab\" && out_tab == lt_tab",
                    "同じタブへ置いたことを見ていない（タブが増えても緑になる）",
                ),
                ("short", "床でも届かないこと（cols_short）を見ていない"),
                (
                    "tabs_after == tabs_before_1439",
                    "タブが増えていないことを見ていない",
                ),
            ] {
                if !window.contains(needle) {
                    push(at, format!("項目 72b が {why}（#1439）"));
                }
            }
        }
    }
    out
}

fn all(dispatch: &str, worker_font: &str, app: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(scan_dispatch(dispatch).iter().map(Offender::report));
    out.extend(scan_worker_font(worker_font).iter().map(Offender::report));
    out.extend(scan_app(app).iter().map(Offender::report));
    out
}

fn sources() -> (String, String, String) {
    let root = workspace_root();
    (
        read(&root, DISPATCH),
        read(&root, WORKER_FONT),
        read(&root, APP),
    )
}

/// 走査が窓を採れていること（空振りなら他の検査は全部無意味に緑）
#[test]
fn 走査が空振りしていない() {
    let (dispatch, worker_font, app) = sources();
    for (rel, src, needles) in [
        (
            DISPATCH,
            &dispatch,
            vec![
                "fn plan_worker_placement(",
                "fn dispatch_orchestrator_spawn(",
                "Request::Close {",
                "use crate::worker_font::PlacementArm;",
            ],
        ),
        (
            WORKER_FONT,
            &worker_font,
            vec![
                "pub fn refit_worker_area(",
                "pub fn enabled(",
                "fn legacy_split_tabs()",
                "fn legacy_worker_min_width()",
            ],
        ),
        (
            APP,
            &app,
            vec![
                "fn set_worker_font_scale(",
                "fn zoom_focused_pane(",
                "fn refresh_font_scale_cells(",
                "// 72b. #1439:",
            ],
        ),
    ] {
        let view = production(rel, src);
        for needle in needles {
            assert!(
                fn_window(&view, needle).is_some() || view.contains(needle),
                "{rel} の窓 `{needle}` が採れない（走査が壊れている）"
            );
        }
    }
}

/// 現行のソースは違反ゼロ
#[test]
fn 現行実装は同じタブと自動縮小を守っている() {
    let (dispatch, worker_font, app) = sources();
    let found = all(&dispatch, &worker_font, &app);
    assert!(found.is_empty(), "#1439 の逆戻り:\n{}", found.join("\n"));
}

/// 修正前を再現した注入が file:line で名指しされること（見逃す側へ倒れない）
#[test]
fn 逆戻りを名指しできる() {
    let (dispatch, worker_font, app) = sources();

    // 注入 1: 既定の腕で打ち切らず、別タブの配置へ落ちる（#1132 の挙動へ逆戻り）
    let split = dispatch.replace("    if arm == PlacementArm::SameTab {", "    if false {");
    assert!(split != dispatch, "注入 1 の対象が見つからない");
    let want = split
        .lines()
        .position(|l| l.contains("\"overflow_tab\"") || l.contains("\"new_tab\""))
        .map(|i| i + 1);
    let found = all(&split, &worker_font, &app);
    assert!(
        found.iter().any(|o| o.contains(DISPATCH)),
        "別タブへ逃がす形への逆戻りを名指しできていない（期待行 {want:?}）: {found:?}"
    );

    // 注入 2: 床を設定から取らず、いちばん小さい倍率まで縮める
    let no_floor = worker_font.replace(
        "    let floor = layout.min_worker_font_scale;",
        "    let floor = tako_core::spawn_layout::WORKER_FONT_SCALE_MIN;",
    );
    assert!(no_floor != worker_font, "注入 2 の対象が見つからない");
    let found = all(&dispatch, &no_floor, &app);
    assert!(
        found.iter().any(|o| o.contains(WORKER_FONT)),
        "床を無視する形を名指しできていない: {found:?}"
    );

    // 注入 3: 応答から `cols_short` を落とす（届かなかったことが master に出ない）
    let silent = dispatch.replace("\"cols_short\"", "\"_cols_short\"");
    assert!(silent != dispatch, "注入 3 の対象が見つからない");
    let found = all(&silent, &worker_font, &app);
    assert!(
        found.iter().any(|o| o.contains(DISPATCH)),
        "無言の縮小（cols_short 欠落）を名指しできていない: {found:?}"
    );

    // 注入 4: close 後のリフローで当て直さない
    let stale = dispatch.replace(
        "                        crate::worker_font::refit_worker_area(host, tab, anchor, &layout);",
        "",
    );
    assert!(stale != dispatch, "注入 4 の対象が見つからない");
    let found = all(&stale, &worker_font, &app);
    assert!(
        found.iter().any(|o| o.contains(DISPATCH)),
        "close 後に当て直さない形を名指しできていない: {found:?}"
    );

    // 注入 5: A/B の env を #1132 と同じにする（互いの回帰を隠す）
    let collide = worker_font.replace("TAKO_1439_LEGACY", "TAKO_1132_LEGACY");
    assert!(collide != worker_font, "注入 5 の対象が見つからない");
    let found = all(&dispatch, &collide, &app);
    assert!(
        found.iter().any(|o| o.contains(WORKER_FONT)),
        "A/B の env 衝突を名指しできていない: {found:?}"
    );

    // 注入 8: 対照の腕でも縮める（#1132 前を再現できなくなる = A/B が嘘になる）
    let armless = worker_font.replace(
        "        && PlacementArm::from_env() == PlacementArm::SameTab\n",
        "",
    );
    assert!(armless != worker_font, "注入 8 の対象が見つからない");
    let found = all(&dispatch, &armless, &app);
    assert!(
        found.iter().any(|o| o.contains(WORKER_FONT)),
        "対照の腕でも縮む形を名指しできていない: {found:?}"
    );

    // 注入 6: 手のズームを踏み潰す（provenance を見ない）
    let clobber = app.replace(
        "        let manual =\n            self.pane_font_sizes.contains_key(&pane) \
         && !self.auto_font_panes.contains(&pane);",
        "        let manual = false;",
    );
    assert!(clobber != app, "注入 6 の対象が見つからない");
    let found = all(&dispatch, &worker_font, &clobber);
    assert!(
        found.iter().any(|o| o.contains(APP)),
        "手のズームを踏み潰す形を名指しできていない: {found:?}"
    );

    // 注入 7: 項目 72b を「タブを見ない」形へ戻す（検出力が消える）
    let vacuous = app.replace(
        "                                placement == \"same_tab\" && out_tab == lt_tab,",
        "                                !placement.is_empty(),",
    );
    assert!(vacuous != app, "注入 7 の対象が見つからない");
    let found = all(&dispatch, &worker_font, &vacuous);
    assert!(
        found.iter().any(|o| o.contains(APP)),
        "項目 72b の検出力が消えた形を名指しできていない: {found:?}"
    );
}
