//! **#812 の番犬**: ペイン枠線のインクを塗るのはルート側のオーバーレイ 1 枚だけ。
//!
//! ## なぜ止めるのか
//!
//! #803 でペインヘッダをルート側の兄弟へ持ち上げたとき、GPUI の `Style::paint` が
//! 「影 → 背景 → 子 → 枠線」の順で塗るせいで、ペイン枠の上 2 つの丸め角が
//! ヘッダの四角い背景で潰れた（実測: フォーカス枠の accent が角で 104px 消えた）。
//! 当座の直しは「ヘッダの外枠にも同じ枠線を描かせる」で重なり順は戻ったが、
//! **同じ枠線を本体とヘッダ外枠の 2 か所が塗る**状態になり、丸め角の AA 画素だけが
//! 二重に合成されて濃くなっていた（実フレーム全画素比較で 32 / 3,115,200）。
//!
//! #812 はインクをルート側の 1 枚（`pane_borders`）へ集約してこれを消した。
//! 本体とヘッダ外枠は**枠幅と角丸のクリップだけ**を持ち、色を持たない
//! （GPUI の `Style::is_border_visible()` は `border_color` が無ければ false なので
//! 枠線の quad が 1 枚も出ない）。
//!
//! ## 何を固定するか
//!
//! 実ピクセルは visual-test の `pane-border` 節が見る（`TAKO_VISUAL_ONLY=pane-border`）。
//! あれは GUI が要るので CI では回らない。ここが止めるのは**構造**で、1 行で戻せる形が
//! これだけある:
//!
//! 1. [`枠線のインクは本番の腕では1か所だけ`] — 本体かヘッダ外枠に無条件の
//!    `border_color` が戻る（= 二重塗りの再発。visual-test の A/B だけでは
//!    「両腕とも二重」になって検出できない形が実在した）
//! 2. [`枠線はヘッダより後に描く`] — 重なり順が戻ると #803 の「角が潰れる」が再発する
//! 3. [`枠線の色規則は1実装`] — `pane_border_color` を通らない直書きが混ざると
//!    枠の上下で色が食い違う
//! 4. [`abの逃げ道が在る`] — `TAKO_812_LEGACY` が消えると、回帰を隠していないことを
//!    同じバイナリで示せなくなる
//! 5. [`visual_testの節が両方の経路に載っている`] — 腕にしか無い節は「最終確認」で
//!    一度も走らない（#948）

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）。`main.rs` は `#[cfg(test)] mod …` が
// ファイルの途中に何本も在るので、「最初の `#[cfg(test)]` で切る」自前実装だと
// 製品コードの大半が黙って走査から落ちる
#[path = "common/production_range.rs"]
mod production_range;

const APP_REL: &str = "crates/tako-app/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）。
/// 範囲が想定より縮んだら [`production_range::production`] 自身が `file:line` で落ちる
fn production_source(src: &str) -> String {
    production_range::production(src, APP_REL)
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// コメントを除いた `(行番号, 行)` の一覧（行番号は 1 起点で原文と一致する）
fn code_lines(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .map(|(i, l)| (i + 1, l))
        .collect()
}

/// `needle` を含む行を `file:line` つきで拾う
fn hits(src: &str, needle: &str) -> Vec<(usize, String)> {
    code_lines(src)
        .into_iter()
        .filter(|(_, l)| l.contains(needle))
        .map(|(n, l)| (n, l.trim().to_string()))
        .collect()
}

// ------------------------------------------- 1. インクを塗るのは 1 か所だけ

/// 枠線の色（= インク）を渡している箇所が、**本番の腕では 1 か所**であること。
///
/// 旧挙動へ戻す 2 か所（本体・ヘッダ外枠）は `legacy_812()` ゲートの中にしか居ない。
/// ゲートが `true` や定数へ差し替わったら（= 二重塗りの再発）ここで落ちる
#[test]
fn 枠線のインクは本番の腕では1か所だけ() {
    let src = production_source(&read(APP_REL));
    let calls = hits(&src, "pane_border_color(");
    // 宣言 1 + オーバーレイ 1 + 旧挙動 2 = 4
    let uses: Vec<(usize, String)> = calls
        .iter()
        .filter(|(_, l)| !l.contains("fn pane_border_color("))
        .cloned()
        .collect();
    assert_eq!(
        uses.len(),
        3,
        "{APP_REL}: 枠線色の参照が {} 箇所（想定 3 = オーバーレイ 1 + 旧挙動 2）。\
         増減したら塗る場所が変わっている（#812。実測: {uses:?}）",
        uses.len()
    );
    // 旧挙動の 2 か所は `legacy_812()` のゲートの中（同じ行か直前の行に居る）
    let lines: Vec<&str> = src.lines().collect();
    let gated: Vec<usize> = uses
        .iter()
        .filter(|(n, _)| {
            (n.saturating_sub(3)..*n)
                .any(|i| lines.get(i).is_some_and(|l| l.contains("legacy_812()")))
        })
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(
        gated.len(),
        2,
        "{APP_REL}: `legacy_812()` ゲートの中に居る枠線色の参照が {} 箇所（想定 2）。\
         ゲートの外へ出た参照は**無条件の 2 か所塗り** = #812 の再発（実測: {uses:?} / \
         ゲート内: {gated:?}）",
        gated.len()
    );
    // 残る 1 か所がオーバーレイ
    let overlay: Vec<usize> = uses
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !gated.contains(n))
        .collect();
    assert_eq!(
        overlay.len(),
        1,
        "{APP_REL}: インクを塗る本番の腕が {} 箇所（想定 1）。#812 の集約が崩れている",
        overlay.len()
    );
    // そこが `pane_borders`（ルート側オーバーレイ）の中であること
    let decl = src
        .find("let pane_borders: Vec<_> =")
        .map(|i| src[..i].lines().count() + 1)
        .unwrap_or_else(|| {
            panic!("{APP_REL}: ルート側の枠線オーバーレイ `pane_borders` が消えている（#812）")
        });
    let at = overlay[0];
    assert!(
        at > decl && at < decl + 40,
        "{APP_REL}:{at}: インクを塗っているのが `pane_borders`（{APP_REL}:{decl}）の中ではない（#812）"
    );
}

// ------------------------------------------- 2. 重なり順（#803 を戻さない）

#[test]
fn 枠線はヘッダより後に描く() {
    let src = production_source(&read(APP_REL));
    let at = |needle: &str| -> usize {
        hits(&src, needle)
            .first()
            .map(|(n, _)| *n)
            .unwrap_or_else(|| panic!("{APP_REL}: ルートが {needle} を出していない（#803 / #812）"))
    };
    let bodies = at(".children(panes)");
    let headers = at(".children(pane_headers)");
    let borders = at(".children(pane_borders)");
    assert!(
        bodies < headers && headers < borders,
        "{APP_REL}: ルートの並びが本体({bodies}) → ヘッダ({headers}) → 枠線({borders}) では \
         ない。枠線がヘッダより先に塗られると上 2 つの丸め角がヘッダの四角い背景で潰れる \
         （#803 の実バグ。GPUI の paint 順は 影 → 背景 → 子 → 枠線）"
    );
}

// ------------------------------------------- 3. 色規則は 1 実装

#[test]
fn 枠線の色規則は1実装() {
    let src = production_source(&read(APP_REL));
    assert_eq!(
        hits(&src, "fn pane_border_color(").len(),
        1,
        "{APP_REL}: `pane_border_color` の実装が 1 本ではない（#803 / #812）"
    );
    // オーバーレイの中で色を直書きしていないこと（`pane_border_color` を通す）
    let start = src
        .find("let pane_borders: Vec<_> =")
        .expect("`pane_borders` が消えている（#812）");
    let head = src[start..].lines().take(40).collect::<Vec<_>>().join("\n");
    let at = src[..start].lines().count() + 1;
    assert!(
        head.contains(".border_color(self.pane_border_color("),
        "{APP_REL}:{at}: 枠線オーバーレイが `pane_border_color` を通っていない \
         = 色規則が 2 本に割れる（#812）"
    );
    // 丸めと枠幅も定数から引く（直値を散らすと角がずれる）
    for needle in [
        ".border(px(PANE_BORDER))",
        ".rounded(px(PANE_CORNER_RADIUS))",
    ] {
        assert!(
            head.contains(needle),
            "{APP_REL}:{at}: 枠線オーバーレイに {needle} が無い（#812）"
        );
    }
}

// ------------------------------------------- 4. A/B の逃げ道

#[test]
fn abの逃げ道が在る() {
    let src = production_source(&read(APP_REL));
    let decl = src
        .split_once("fn legacy_812() -> bool {")
        .map(|(head, rest)| (head.lines().count() + 1, rest))
        .unwrap_or_else(|| {
            panic!("{APP_REL}: #812 の A/B の逃げ道 `legacy_812` が無い（回帰を隠していないことを示せない）")
        });
    let (at, rest) = decl;
    let body: String = rest.lines().take(12).collect::<Vec<_>>().join("\n");
    assert!(
        body.contains("std::env::var(\"TAKO_812_LEGACY\")"),
        "{APP_REL}:{at}: `legacy_812` が `TAKO_812_LEGACY` を読んでいない（#812）"
    );
    // 節が実行中に腕を倒せること（同じ場面で撮り比べるため。プロセスを分けると
    // PTY 出力・時計・レイアウトの揺れが混ざって「角だけが違う」を言い切れない）
    assert!(
        src.contains("fn set_legacy_812(on: bool)"),
        "{APP_REL}: 実行中に腕を倒す口（`set_legacy_812`）が無い（#812 の A/B が \
         同じ場面で撮り比べられなくなる）"
    );
    // オーバーレイを止めるゲートは `legacy_812()` **だけ**。`if true` などへ差し替わると
    // 枠線が 1 本も出ないが、実ピクセルを見る節は GUI が要るので CI では回らない。
    // 「枠線が全部消えた」を CI で止められる唯一の場所がここ
    assert!(
        src.contains("let pane_borders: Vec<_> = if legacy_812() {"),
        "{APP_REL}: 枠線オーバーレイのゲートが `legacy_812()` ではない（#812）。\
         止めると枠線が 1 本も出ないが、実ピクセルの検査は CI では回らない"
    );
    // 表示中の全ペインに出すこと（`layout` を舐める）
    let start = src
        .find("let pane_borders: Vec<_> =")
        .expect("`pane_borders` が消えている（#812）");
    let head = src[start..].lines().take(24).collect::<Vec<_>>().join("\n");
    let at = src[..start].lines().count() + 1;
    assert!(
        head.contains("layout") && head.contains(".iter()"),
        "{APP_REL}:{at}: 枠線オーバーレイが `layout`（表示中の全ペイン）を舐めていない（#812）"
    );
}

// ------------------------------------------- 5. visual-test の節の配線（#948）

#[test]
fn visual_testの節が両方の経路に載っている() {
    let src = read(APP_REL);
    assert_eq!(
        hits(&src, "pane_border_visual(any, window, cx).await;").len(),
        2,
        "{APP_REL}: `pane_border_visual` の呼び出しが 2 か所（`TAKO_VISUAL_ONLY` の腕 + \
         全節実行の並び）ではない。腕にしか無い節は最終確認で一度も走らない（#948）"
    );
    assert!(
        src.contains("inject_section_failure(\"pane-border\")"),
        "{APP_REL}: `pane-border` 節に #948 の注入口が無い（実行経路に載っているかを \
         機械検証できない）"
    );
    assert!(
        src.contains("\"pane-border\" => {"),
        "{APP_REL}: `TAKO_VISUAL_ONLY=pane-border` の腕が無い（#812）"
    );
}
