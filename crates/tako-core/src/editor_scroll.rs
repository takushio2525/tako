//! editor_scroll — 編集カーソルの追従スクロールの算術（Issue #1649）
//!
//! 「いま何行目から何行目までが見えているか」と「カーソル行を見せるには
//! 先頭可視行をどこへ動かすか」だけを持つ。GPUI の器（`ListState` /
//! `ScrollHandle`）は**寸法を測って渡すだけ**で、判断はこの 1 実装が行う。
//!
//! UI 層に算術を置かない理由は 2 つある。① 器が 2 種類あるから（#821 の
//! 仮想リストと `TAKO_821_NO_VIRTUAL_LIST=1` の div スクロール）。片方にだけ
//! 書くともう片方が追従しない。② 実ピクセルを持たないテストで
//! 「カーソル行が可視範囲に入ったか」を固定できるから（実時間・実ピクセルの
//! assert を置かずにエッジケースを網羅できる）。
//!
//! 旧実装には `scroll_to(cursor)` の呼び出しが 1 件も無く、打鍵・矢印・
//! 検索ヒット・undo / redo のどれでもカーソルが画面外へ出たままだった。

/// 追従スクロールで確保する上下の余白（行）。
///
/// 0 にするとカーソルが器の端に貼り付き、次に打つ 1 文字の行き先が見えない
/// （改行の直後がいちばん分かりやすい）。3 行は Zed / VSCode の既定と同じ水準
pub const FOLLOW_MARGIN: usize = 3;

/// 器の実測値から割り出した「行の単位で見た可視範囲」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineViewport {
    /// いちばん上に見えている行（0 始まり）
    pub first_visible: usize,
    /// 同時に見える行数（1 以上）
    pub visible_lines: usize,
    /// 文書の全行数
    pub total_lines: usize,
}

impl LineViewport {
    /// 器の高さと 1 行の高さ（どちらも論理 px）から作る。
    ///
    /// **高さが取れない場合を 0 除算へ落とさない**のが要点。器がまだ 1 度も
    /// 描かれていないフレームでは高さが 0 で、そのまま割ると可視行数が
    /// 無限（`as usize` は飽和するので `usize::MAX`）になり、どこへ飛んでも
    /// 「見えている」に倒れて追従が空振りする
    pub fn from_pixels(
        first_visible: usize,
        total_lines: usize,
        viewport_height: f32,
        line_height: f32,
    ) -> Self {
        let measurable = viewport_height.is_finite()
            && viewport_height > 0.0
            && line_height.is_finite()
            && line_height > 0.0;
        let visible_lines = if measurable {
            // 端に半端に覗く行は数えない（切り捨て）。余白の取り方が
            // 「見えているつもりの行」に引きずられないようにする
            ((viewport_height / line_height).floor() as usize).max(1)
        } else {
            1
        };
        Self {
            first_visible,
            visible_lines,
            total_lines,
        }
    }

    /// いちばん下に見えている行（0 始まり）
    pub fn last_visible(&self) -> usize {
        self.first_visible
            .saturating_add(self.visible_lines.max(1) - 1)
    }

    /// その行が（余白を問わず）見えているか
    pub fn contains(&self, line: usize) -> bool {
        line >= self.first_visible && line <= self.last_visible()
    }

    /// 先頭可視行として取り得る最大値（末尾より先へは送らない）
    pub fn max_first_visible(&self) -> usize {
        self.total_lines.saturating_sub(self.visible_lines.max(1))
    }
}

/// 視野の高さに収まるまで詰めた余白。
///
/// 3 行しか見えない窓で上下 3 行の余白は成立しない（カーソルの居場所が無くなり、
/// 上へ寄せる条件と下へ寄せる条件が同時に成り立って毎回スクロールし続ける）。
/// 上下に `m` 行 + カーソル行の `2m + 1` 行が収まる `m` までに丸める
pub fn effective_margin(visible_lines: usize, margin: usize) -> usize {
    margin.min(visible_lines.max(1).saturating_sub(1) / 2)
}

/// カーソル行を余白込みで可視範囲へ入れるための新しい先頭可視行。
///
/// **動かす必要が無ければ `None`**（ユーザーが見ている位置を理由なく動かさない）。
/// 文書の端では余白が足りなくても寄せきる（1 行目にカーソルがあるとき、
/// その上に 3 行の余白は作れない）
pub fn follow_cursor(view: LineViewport, cursor_line: usize, margin: usize) -> Option<usize> {
    if view.total_lines == 0 {
        return None;
    }
    let visible = view.visible_lines.max(1);
    let cursor = cursor_line.min(view.total_lines - 1);
    let margin = effective_margin(visible, margin);
    let max_first = view.max_first_visible();
    // 器が末尾より先を指していることがある（ライブリロード・undo で行数が減った直後）。
    // その状態を起点に判断すると「動かす必要が無い」へ倒れるので先に詰める
    let first = view.first_visible.min(max_first);
    let last = first + visible - 1;
    let want = if cursor < first + margin {
        cursor.saturating_sub(margin)
    } else if cursor + margin > last {
        (cursor + margin + 1).saturating_sub(visible)
    } else {
        first
    };
    let want = want.min(max_first);
    (want != view.first_visible).then_some(want)
}

/// 追従スクロールを効かせたあと、カーソル行が器の上端から何行目に来るか。
///
/// IME の未確定下線のアンカーに使う（#1649 の連鎖症状）。行の `TextLayout` が
/// 控えられるのは **paint** なので、追従スクロールを要求した直後の 1 フレームは
/// カーソル行のレイアウトがまだ無い。そこで諦めるとアンカーが消え、下線ごと
/// 消えてしまうので、可視化後に来る位置を先に出しておく
pub fn cursor_row_in_viewport(view: LineViewport, cursor_line: usize, margin: usize) -> usize {
    let visible = view.visible_lines.max(1);
    let first = follow_cursor(view, cursor_line, margin).unwrap_or(view.first_visible);
    cursor_line.saturating_sub(first).min(visible - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(first: usize, visible: usize, total: usize) -> LineViewport {
        LineViewport {
            first_visible: first,
            visible_lines: visible,
            total_lines: total,
        }
    }

    /// 追従したあとの可視範囲（呼び手が `scroll_to` へ渡す値を当てたもの）
    fn after(v: LineViewport, cursor: usize) -> LineViewport {
        let first = follow_cursor(v, cursor, FOLLOW_MARGIN).unwrap_or(v.first_visible);
        LineViewport {
            first_visible: first,
            ..v
        }
    }

    #[test]
    fn 余白の内側にいるカーソルは器を動かさない() {
        // 20 行見えていて 3 行の余白 → 3..=16 行目なら動かさない
        for cursor in 3..=16 {
            assert_eq!(
                follow_cursor(view(0, 20, 500), cursor, FOLLOW_MARGIN),
                None,
                "{cursor} 行目で器が動いた"
            );
        }
    }

    #[test]
    fn 下へ出たカーソルは余白ぶん先まで送る() {
        // 0..=19 が見えている状態で 50 行目へ飛んだ
        let v = view(0, 20, 500);
        assert_eq!(follow_cursor(v, 50, FOLLOW_MARGIN), Some(34));
        let a = after(v, 50);
        assert!(a.contains(50));
        assert_eq!(a.last_visible() - 50, FOLLOW_MARGIN, "下の余白が 3 行");
    }

    #[test]
    fn 上へ出たカーソルは余白ぶん手前から見せる() {
        let v = view(20, 20, 500);
        assert_eq!(follow_cursor(v, 5, FOLLOW_MARGIN), Some(2));
        let a = after(v, 5);
        assert!(a.contains(5));
        assert_eq!(5 - a.first_visible, FOLLOW_MARGIN, "上の余白が 3 行");
    }

    /// 端では余白が足りなくても寄せきる（余白の確保より可視化が先）
    #[test]
    fn 文書の端では余白より可視化を優先する() {
        // 先頭: 上に 3 行は作れない
        let v = view(40, 20, 500);
        assert_eq!(follow_cursor(v, 0, FOLLOW_MARGIN), Some(0));
        assert!(after(v, 0).contains(0));
        // 末尾: 下に 3 行は作れない。末尾より先へも送らない
        let v = view(0, 20, 500);
        assert_eq!(follow_cursor(v, 499, FOLLOW_MARGIN), Some(480));
        let a = after(v, 499);
        assert!(a.contains(499));
        assert_eq!(a.max_first_visible(), 480);
        // 末尾が既に見えているならそれ以上動かさない
        assert_eq!(follow_cursor(view(480, 20, 500), 499, FOLLOW_MARGIN), None);
    }

    #[test]
    fn 全部が収まる文書では一度も動かさない() {
        for cursor in 0..10 {
            assert_eq!(follow_cursor(view(0, 20, 10), cursor, FOLLOW_MARGIN), None);
        }
    }

    /// エッジ: 1 行のファイル / 空ファイル
    #[test]
    fn 一行のファイルと空ファイルで動かない() {
        assert_eq!(follow_cursor(view(0, 20, 1), 0, FOLLOW_MARGIN), None);
        assert_eq!(follow_cursor(view(0, 20, 0), 0, FOLLOW_MARGIN), None);
        // 行数を越えたカーソル（外から来た値）は末尾へ丸める
        assert_eq!(follow_cursor(view(0, 20, 1), 999, FOLLOW_MARGIN), None);
    }

    /// エッジ: ウインドウの高さが 3 行以下。余白を詰めても振動しない
    #[test]
    fn 視野が狭いと余白を詰めて振動しない() {
        assert_eq!(effective_margin(1, FOLLOW_MARGIN), 0);
        assert_eq!(effective_margin(2, FOLLOW_MARGIN), 0);
        assert_eq!(effective_margin(3, FOLLOW_MARGIN), 1);
        assert_eq!(effective_margin(7, FOLLOW_MARGIN), 3);
        for visible in 1..=7 {
            let v = view(0, visible, 500);
            let first = follow_cursor(v, 100, FOLLOW_MARGIN).expect("画面外なので動く");
            let moved = LineViewport {
                first_visible: first,
                ..v
            };
            assert!(moved.contains(100), "視野 {visible} 行でカーソルが見えない");
            // 追従したあとは「もう動かす必要が無い」に収束する（毎フレーム動き続けない）
            assert_eq!(
                follow_cursor(moved, 100, FOLLOW_MARGIN),
                None,
                "視野 {visible} 行で振動する"
            );
        }
    }

    /// エッジ: 行数が減った直後、器が末尾より先を指している
    #[test]
    fn 器が末尾より先を指していたら詰める() {
        // 500 行の位置を見ていたのに文書が 40 行へ縮んだ
        assert_eq!(
            follow_cursor(view(500, 20, 40), 39, FOLLOW_MARGIN),
            Some(20)
        );
        assert_eq!(follow_cursor(view(500, 20, 10), 0, FOLLOW_MARGIN), Some(0));
    }

    /// 高さが取れないフレームでも 0 除算へ落ちない
    #[test]
    fn 高さが取れないときは一行として扱う() {
        for (h, lh) in [
            (0.0_f32, 18.0_f32),
            (600.0, 0.0),
            (f32::NAN, 18.0),
            (600.0, f32::NAN),
            (-1.0, 18.0),
        ] {
            let v = LineViewport::from_pixels(0, 500, h, lh);
            assert_eq!(v.visible_lines, 1, "h={h} lh={lh} で可視行数が 1 でない");
            // 「どこへ飛んでも見えている」に倒れない
            assert_eq!(follow_cursor(v, 100, FOLLOW_MARGIN), Some(100));
        }
        // 実測値が入れば切り捨てで数える（半端に覗く行は数えない）
        assert_eq!(
            LineViewport::from_pixels(0, 500, 600.0, 18.0).visible_lines,
            33
        );
    }

    /// 折り返しで 1 行が 2 行分の高さになった器（高さを実測して渡す前提）
    #[test]
    fn 折り返しで行の高さが増えても可視化する() {
        // 器 600px・折り返しで 1 行 36px → 16 行しか見えない
        let v = LineViewport::from_pixels(0, 500, 600.0, 36.0);
        assert_eq!(v.visible_lines, 16);
        let first = follow_cursor(v, 200, FOLLOW_MARGIN).expect("画面外なので動く");
        let moved = LineViewport {
            first_visible: first,
            ..v
        };
        assert!(moved.contains(200));
        assert_eq!(moved.last_visible() - 200, FOLLOW_MARGIN);
    }

    #[test]
    fn ime用のカーソル行位置は可視化後の距離を返す() {
        // 画面外へ飛んだ直後でも、可視化後に来る位置（下から 3 行目）を返す
        let v = view(0, 20, 500);
        assert_eq!(cursor_row_in_viewport(v, 50, FOLLOW_MARGIN), 16);
        // 既に見えているならその場の距離
        assert_eq!(
            cursor_row_in_viewport(view(40, 20, 500), 45, FOLLOW_MARGIN),
            5
        );
        // 器の外へは出さない（視野 1 行なら 0 行目）
        assert_eq!(
            cursor_row_in_viewport(view(0, 1, 500), 300, FOLLOW_MARGIN),
            0
        );
    }
}
