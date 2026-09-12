//! **#1426 の番犬**: 裏タブの寸法合わせを毎フレームやり直さない。
//!
//! ## なぜ止めるのか
//!
//! `TakoApp::sync_offscreen_pane_sizes` は `render` から**毎フレーム**通る。
//! 中身は「裏に居るペインを、表に出たときの寸法へ合わせる」処理で、当て直せる中身は
//! `offscreen_areas`（= `refresh_offscreen_pane_areas` が「key + 2 秒」で作る）が
//! 持っている。したがって**材料が同じあいだは毎フレームまったく同じ答えを出し直して
//! いた**。実測（隔離 GUI の grid-bench・22 ペイン / 表示 4・3000 フレーム）:
//!
//! | | 1 フレームあたり | `TakoApp::render` に占める割合 |
//! |---|---|---|
//! | 修正前 | 596〜776 ns（Vec 1 確保 + 82 比較 + 18 ペインの当て直し） | 5.88〜6.24% |
//! | 修正後 | 下記テストが構造で固定する | — |
//!
//! ## 何を固定するか
//!
//! 速さそのものは実測（Issue のコメント）に残す。ここが止めるのは**構造**で、
//! 1 行で戻せてしまう形がこれだけある:
//!
//! 1. [`間引きを通してから走査する`] — 間引きが外れる / 走査の**後ろ**へ移る
//!    （後ろだと毎フレームの Vec 確保と O(n×m) 比較が残る = 直っていない）
//! 2. [`間引きのキーに5つの材料が入っている`] — とくに**既定セル寸法**。
//!    抜けるとフォントを変えても裏タブが最大 2 秒そのまま = #647 の再発
//! 3. [`abの逃げ道が在る`] — `TAKO_1426_LEGACY` が消えると回帰を隠していないことを
//!    同じバイナリで示せなくなる
//! 4. [`間引きの判定はlegacyを見る`] — 逃げ道が判定へ配線されていない形を落とす
//! 5. [`領域キーは1実装`] — `refresh_offscreen_pane_areas` が自前でタプルを組み直すと、
//!    片方だけ材料が増えて「割り出したのに当て直さないフレーム」が生まれる
//! 6. [`間隔は1つの定数`] — どちらかが `Duration::from_secs(2)` の直書きへ戻ると、
//!    当て直す側だけ短くして「古い領域を撃ち直すだけ」になる
//! 7. [`ズームの指紋は掛けて混ぜている`] — `id ^ 値` の線形な畳み込みへ戻すと、
//!    2 ペインで値を入れ替えたときに同じ指紋になる（実装中に単体テストが実際に落ちた）
//! 8. [`セル寸法の単体テストが在る`] — #647 の再発防止テストが消えていない

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）。`main.rs` は `#[cfg(test)] mod …` が
// ファイルの途中に何本も在るので、「最初の `#[cfg(test)]` で切る」自前実装だと
// 製品コードの大半が黙って走査から落ちる
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const APP_REL: &str = "crates/tako-app/src/main.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）。
/// 範囲が想定より縮んだら [`production_range::production`] 自身が `file:line` で落ちる
fn production_source(src: &str) -> String {
    production_range::production(src, APP_REL)
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {signature:?} の本体を閉じる `}}` が見つからない"));
    let body: Vec<(usize, String)> = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    // 下限は「範囲取りそのものが壊れた」を落とすためだけの保険。低めに置く
    // （`pane_font_fingerprint` のように 4 行の関数が実在し、その中身の検査は
    // 個別のアサートが持つ。下限を高くすると回帰の理由が下限の話にすり替わる）
    assert!(
        body.len() >= 3,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    (body, start + 1)
}

const SYNC_FN: &str = "fn sync_offscreen_pane_sizes(";
const REFRESH_FN: &str = "fn refresh_offscreen_pane_areas(";

/// 本体の中で `needle` が最初に出る行（1 起点）
fn find(body: &[(usize, String)], needle: &str) -> Option<usize> {
    body.iter()
        .find(|(_, l)| l.contains(needle))
        .map(|(n, _)| *n)
}

// ------------------------------------------------- 1. 間引きが走査より手前に在る

#[test]
fn 間引きを通してから走査する() {
    let src = production_source(&read(APP_REL));
    let (body, at) = fn_body(APP_REL, &src, SYNC_FN);
    let skip = find(&body, "offscreen_sync_can_skip(").unwrap_or_else(|| {
        panic!(
            "{APP_REL}:{at}: {SYNC_FN} が間引き（offscreen_sync_can_skip）を通っていない \
             = render から毎フレーム走査へ戻っている（#1426）"
        )
    });
    let scan = find(&body, ".terminals").unwrap_or_else(|| {
        panic!("{APP_REL}:{at}: 裏ペインの走査（self.terminals …）が見つからない")
    });
    assert!(
        skip < scan,
        "{APP_REL}:{scan}: 走査が間引き（{APP_REL}:{skip}）より手前に在る \
         = 毎フレームの Vec 確保と O(ペイン×表示ペイン) 比較が残ったまま（#1426）"
    );
    let refresh = find(&body, "self.refresh_offscreen_pane_areas(").unwrap_or_else(|| {
        panic!("{APP_REL}:{at}: 領域の割り出し（refresh_offscreen_pane_areas）の呼び出しが無い")
    });
    assert!(
        skip < refresh,
        "{APP_REL}:{refresh}: 領域の割り出しが間引きより手前に在る（#1426）"
    );
}

// ------------------------------------------------- 2. キーの材料

#[test]
fn 間引きのキーに5つの材料が入っている() {
    let src = production_source(&read(APP_REL));
    let (body, at) = fn_body(APP_REL, &src, SYNC_FN);
    let joined: String = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // (材料, 抜けたときに何が壊れるか)
    let want: &[(&str, &str)] = &[
        (
            "self.offscreen_area_key(",
            "ウィンドウ寸法・タブ数・端末数・バナー・カード・拡大率の変化を見落とす（#932）",
        ),
        (
            "f32::from(default_cell.width).to_bits()",
            "フォントサイズを変えても裏タブが最大 2 秒そのまま（#647 の再発）",
        ),
        (
            "f32::from(default_cell.height).to_bits()",
            "行高が変わっても裏タブの行数が古いまま（#647 の再発）",
        ),
        (
            "self.pane_text_areas.len()",
            "タブを切り替えて裏に回る集合が変わったのに当て直さない",
        ),
        (
            "pane_font_fingerprint(&self.pane_font_sizes)",
            "ペイン単位ズームを裏タブへ届けない",
        ),
    ];
    let missing: Vec<String> = want
        .iter()
        .filter(|(needle, _)| !joined.contains(needle))
        .map(|(needle, why)| format!("{APP_REL}:{at}: 間引きのキーに `{needle}` が無い（{why}）"))
        .collect();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

// ------------------------------------------------- 3/4. A/B の逃げ道

#[test]
fn abの逃げ道が在る() {
    let src = production_source(&read(APP_REL));
    assert!(
        src.contains("TAKO_1426_LEGACY"),
        "{APP_REL}: A/B の逃げ道 `TAKO_1426_LEGACY` が消えている \
         （間引きが #932 / #647 の回帰を隠していないことを同じバイナリで示せない）"
    );
    let (body, at) = fn_body(APP_REL, &src, SYNC_FN);
    assert!(
        find(&body, "offscreen_sync_legacy()").is_some(),
        "{APP_REL}:{at}: 逃げ道が {SYNC_FN} へ配線されていない"
    );
}

#[test]
fn 間引きの判定はlegacyを見る() {
    let src = production_source(&read(APP_REL));
    let (body, at) = fn_body(APP_REL, &src, "fn offscreen_sync_can_skip(");
    let joined: String = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("!legacy"),
        "{APP_REL}:{at}: 判定が legacy を見ていない（逃げ道が飾りになっている）"
    );
    assert!(
        joined.contains("last == Some(key)"),
        "{APP_REL}:{at}: 材料の一致を見ていない（時間だけで飛ばすと #932 が戻る）"
    );
}

// ------------------------------------------------- 5. 領域キーは 1 実装

#[test]
fn 領域キーは1実装() {
    let src = production_source(&read(APP_REL));
    let (body, at) = fn_body(APP_REL, &src, REFRESH_FN);
    let joined: String = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("self.offscreen_area_key("),
        "{APP_REL}:{at}: {REFRESH_FN} が領域キーを自前で組み直している \
         = 当て直す側（#1426）と材料がずれる"
    );
    assert!(
        !joined.contains("self.stale_binary_banners.len()"),
        "{APP_REL}:{at}: 領域キーの材料が 2 実装に増えている（`offscreen_area_key` へ寄せる）"
    );
}

// ------------------------------------------------- 6. 間隔は 1 つの定数

#[test]
fn 間隔は1つの定数() {
    let src = production_source(&read(APP_REL));
    assert!(
        src.contains("const OFFSCREEN_REFRESH_INTERVAL: Duration"),
        "{APP_REL}: 間隔の定数 OFFSCREEN_REFRESH_INTERVAL が消えている"
    );
    for signature in [SYNC_FN, REFRESH_FN, "fn offscreen_sync_can_skip("] {
        let (body, at) = fn_body(APP_REL, &src, signature);
        let offenders: Vec<String> = body
            .iter()
            .filter(|(_, l)| l.contains("Duration::from_secs(") || l.contains("from_millis("))
            .map(|(n, l)| format!("{APP_REL}:{n}: {} ← 直書きの間隔", l.trim()))
            .collect();
        assert!(
            offenders.is_empty(),
            "{signature}（{APP_REL}:{at}）が間隔を直書きしている。\
             片方だけ短くすると「割り出し済みの古い領域を撃ち直すだけ」になる:\n{}",
            offenders.join("\n")
        );
    }
}

// ------------------------------------------------- 7. 指紋の混ぜ方

#[test]
fn ズームの指紋は掛けて混ぜている() {
    let src = production_source(&read(APP_REL));
    let (body, at) = fn_body(APP_REL, &src, "fn pane_font_fingerprint(");
    let joined: String = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.matches("wrapping_mul(").count() >= 2,
        "{APP_REL}:{at}: 指紋が線形な畳み込み（`id ^ 値`）へ戻っている。\
         2 ペインで値を入れ替えると同じ指紋になり、ズームの変化を取りこぼす"
    );
}

// ------------------------------------------------- 8. #647 の単体テスト

#[test]
fn セル寸法の単体テストが在る() {
    let src = read(APP_REL);
    assert!(
        src.contains("mod offscreen_sync_tests"),
        "{APP_REL}: #1426 の単体テストモジュールが消えている"
    );
    assert!(
        src.contains("fn セル寸法が変われば材料が変わる()"),
        "{APP_REL}: セル寸法をキーへ入れ忘れる回帰（= #647 の再発）を見る単体テストが消えている"
    );
    assert!(
        src.contains("fn 間隔を過ぎたら材料が同じでも回す()"),
        "{APP_REL}: 保険（材料に現れない変化）を見る単体テストが消えている"
    );
}
