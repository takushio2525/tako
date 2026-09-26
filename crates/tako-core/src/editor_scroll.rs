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

/// 行の下端が器の下端に「収まっている」とみなす誤差（論理 px）。
///
/// 行の高さは 21.03px のような端数を持ち、何十行も積むと下端が 0.5px 未満だけ
/// はみ出す計算になることがある。画面上は収まっている行を数え落とさないための幅
const EDGE_EPSILON: f32 = 0.5;

/// 器の実測値から割り出した「行の単位で見た可視範囲」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineViewport {
    /// いちばん上に見えている行（0 始まり）
    pub first_visible: usize,
    /// 同時に見える行数（1 以上）。[`visible_rows`] で測る
    pub visible_lines: usize,
    /// 文書の全行数
    pub total_lines: usize,
}

/// 器に**はみ出さずに見える行数**を、実際に描いた行の実寸から数える（#1741）。
///
/// 可視行数の測り方はこの 1 実装だけにする。追従スクロールの余白（#1649）と
/// ページ移動の歩幅（#1652）が別の値を使うと、Page Down の着地が画面の最下段に
/// 貼り付いたり、追従が「見えている」と判断した行が画面の外にあったりする。
///
/// - `first_row_top`: 先頭可視行の上端（論理 px。器の上端から上余白ぶん下がった位置）
/// - `viewport_bottom`: 器の下端。下端がここまでに収まる行だけを数える
///   （端に半端に覗く行は数えない = 余白の取り方が「見えているつもりの行」に引きずられない）
/// - `row_heights`: 先頭可視行から順に、描いた行の実寸（`None` = まだ描いていない行）。
///   折り返して 2 行ぶんの高さになった行は、その高さのまま効く
/// - `line_height`: 実寸が尽きた先（文書末 / 未描画）を埋める 1 行の高さ
///
/// 実寸が尽きたら、残りの高さを `line_height` の行で埋めたと見なして足す。これで
/// 可視行数は文書の長さに依らない（1 行だけのファイルでも器の容量として数える）。
///
/// 旧実装は器の高さを `theme.line_height`（17px）で割っていたが、コード行は実測 21px で、
/// 先頭行は器の上端から上余白（14px）ぶん下に描かれる。可視行数を多く見積もるので
/// 追従の下の余白が画面の外へ出ていた（661px の器で実矩形 30 行に対し 37 行と数えた）。
///
/// **測れない値を 0 除算へ落とさない**。器がまだ 1 度も描かれていないフレームでは
/// 高さが 0 で、そのまま割ると可視行数が飽和し、どこへ飛んでも「見えている」に倒れて
/// 追従が空振りする。測れないときは 1 行として返す
pub fn visible_rows(
    first_row_top: f32,
    viewport_bottom: f32,
    row_heights: impl IntoIterator<Item = Option<f32>>,
    line_height: f32,
) -> usize {
    let positive = |v: f32| v.is_finite() && v > 0.0;
    if !first_row_top.is_finite() || !viewport_bottom.is_finite() || !positive(line_height) {
        return 1;
    }
    let bottom = viewport_bottom + EDGE_EPSILON;
    let mut y = first_row_top;
    let mut count = 0usize;
    for height in row_heights {
        let Some(height) = height.filter(|h| positive(*h)) else {
            break;
        };
        if y + height > bottom {
            return count.max(1);
        }
        y += height;
        count += 1;
    }
    let rest = bottom - y;
    if rest > 0.0 {
        count += (rest / line_height).floor() as usize;
    }
    count.max(1)
}

impl LineViewport {
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

    /// 高さが取れないフレームでも 0 除算へ落ちない（#1649 → #1741）
    #[test]
    fn 高さが取れないときは一行として扱う() {
        for (top, bottom, lh) in [
            (0.0_f32, 0.0_f32, 18.0_f32),
            (0.0, 600.0, 0.0),
            (0.0, f32::NAN, 18.0),
            (0.0, 600.0, f32::NAN),
            (f32::NAN, 600.0, 18.0),
            (0.0, -1.0, 18.0),
            (0.0, 600.0, -18.0),
        ] {
            let visible_lines = visible_rows(top, bottom, [], lh);
            assert_eq!(
                visible_lines, 1,
                "top={top} bottom={bottom} lh={lh} で可視行数が 1 でない"
            );
            // 「どこへ飛んでも見えている」に倒れない
            let v = view(0, visible_lines, 500);
            assert_eq!(follow_cursor(v, 100, FOLLOW_MARGIN), Some(100));
        }
        // 実寸の行が 0 や NaN でも、そこで数えるのをやめて見積もりへ倒す
        assert_eq!(visible_rows(0.0, 600.0, [Some(0.0)], 20.0), 30);
        assert_eq!(visible_rows(0.0, 600.0, [Some(f32::NAN)], 20.0), 30);
    }

    /// #1741 の実測値そのもの: 661px の器・上余白 14px・行 21px は 30 行。
    /// 旧実装（器の高さ − 上下の余白を 17px で割る）は 37 行と数えていた
    #[test]
    fn 上余白と描いた行の高さで数える() {
        let rows = std::iter::repeat_n(Some(21.0_f32), 400);
        assert_eq!(visible_rows(14.0, 661.0, rows, 21.0), 30);
        // 描いた行が無くても、同じ行の高さなら同じ数になる（見積もりが実寸と一致する）
        assert_eq!(visible_rows(14.0, 661.0, [], 21.0), 30);
        // 半端に覗く行は数えない（5 行目は下端 105px で、100px の器を 5px はみ出す）
        assert_eq!(visible_rows(0.0, 100.0, [Some(21.0); 10], 21.0), 4);
        // 端数のある行の高さを積んでも、画面上収まっている行は数え落とさない
        let odd = std::iter::repeat_n(Some(21.034_f32), 40);
        assert_eq!(
            visible_rows(14.0, 14.0 + 21.034 * 30.0 - 0.3, odd, 21.034),
            30
        );
    }

    /// 折り返しで 2〜3 行ぶんの高さになった行は、その高さのまま効く
    #[test]
    fn 折り返した行は実寸の高さで数える() {
        // 21px の行の中に 63px（3 行ぶん）の行が 2 本 → 30 行の器に 26 行
        let mut rows = vec![Some(21.0_f32); 40];
        rows[2] = Some(63.0);
        rows[7] = Some(63.0);
        assert_eq!(visible_rows(14.0, 661.0, rows, 21.0), 26);
        // 器の高さより高い 1 行（巨大な折り返し）でも 1 行として見える扱い
        assert_eq!(visible_rows(14.0, 661.0, [Some(2000.0)], 21.0), 1);
        // 追従はその可視行数で余白を取る
        let v = view(0, 26, 500);
        let first = follow_cursor(v, 200, FOLLOW_MARGIN).expect("画面外なので動く");
        let moved = LineViewport {
            first_visible: first,
            ..v
        };
        assert!(moved.contains(200));
        assert_eq!(moved.last_visible() - 200, FOLLOW_MARGIN);
    }

    /// 文書が器より短いときも、可視行数は器の容量（1 行のファイルでも同じ）
    #[test]
    fn 文書末より先は見積もりの行で埋める() {
        // 1 行だけのファイル
        assert_eq!(visible_rows(14.0, 661.0, [Some(21.0)], 21.0), 30);
        // 5 行のファイル（折り返し 1 本を含む）
        let short = [Some(21.0), Some(42.0), Some(21.0), Some(21.0), Some(21.0)];
        assert_eq!(visible_rows(14.0, 661.0, short, 21.0), 29);
        // まだ描いていない行（None）で止めて、残りを見積もりで埋める
        let partial = [Some(21.0), Some(21.0), None, Some(900.0)];
        assert_eq!(visible_rows(14.0, 661.0, partial, 21.0), 30);
    }

    /// 先頭可視行が上へ半分送られている（ホイールで offset が付いた）状態
    #[test]
    fn 先頭行が上へずれていてもその位置から数える() {
        // 先頭行の上端が器の上端より 10px 上 → 下へ 1 行ぶん余計に入る
        assert_eq!(visible_rows(-10.0, 661.0, [Some(21.0); 40], 21.0), 31);
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
