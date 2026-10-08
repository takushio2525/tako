//! ファイルツリーの複数選択（FR-3.38 / Issue #1867）
//!
//! **何が選ばれているかを決める正本**。純粋関数だけで、ファイルシステムも画面も触らない。
//! GUI は押した行・押し方（修飾）・いま見えている行の並びを渡すだけで、選択の状態遷移と
//! ⇧クリックの範囲はここで決まる（Finder / VS Code のエクスプローラーと同じ規則）。
//!
//! - 素の押下 = その行だけ（[`ClickKind::Replace`]）
//! - ⌘クリック（Windows は Ctrl+クリック）= 足す / 外す（[`ClickKind::Toggle`]）
//! - ⇧クリック = 起点からその行まで（[`ClickKind::Range`]。それまでの選択は捨てる）
//! - ⌘⇧クリック = 起点からその行までを今の選択へ足す（[`ClickKind::RangeAdd`]）
//! - ⇧↑ / ⇧↓ = 最後に押した行の 1 行上 / 下を ⇧クリックしたのと同じ（[`extend`]。#1895。
//!   起点は動かないので、伸ばしてから逆へ打つと縮む = Finder / VS Code と同じ）
//!
//! まとめて扱う操作（コピー・切り取り・ごみ箱・移動）は、フォルダとその配下を同時に
//! 選んでいたら配下を外して親だけを扱う（[`distinct_roots`]。VS Code の `distinctParents`
//! と同じ）。配下まで重ねて渡すと、親を移した後に配下が「見つからない」で断られたり、
//! コピーで配下が 2 回写ったりする。

use std::path::{Path, PathBuf};

use crate::platform::support::Platform;

/// 行の押し方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKind {
    /// 素の押下: その行だけを選ぶ
    Replace,
    /// ⌘クリック（Windows は Ctrl+クリック）: 足す / 外す
    Toggle,
    /// ⇧クリック: 起点からその行までを選ぶ（それまでの選択は捨てる）
    Range,
    /// ⌘⇧クリック: 起点からその行までを今の選択へ足す
    RangeAdd,
}

impl ClickKind {
    /// 素の押下か（ファイルを開く・フォルダを開閉するのは素の押下だけ。修飾つきは選ぶだけ）
    pub fn is_plain(self) -> bool {
        self == Self::Replace
    }
}

/// 修飾から押し方を決める。主修飾は**リンクと同じ規則**（macOS = command のみ /
/// Windows = control のみ。もう片方が混ざったら主修飾と見なさない）。alt は見ない
pub fn click_kind(platform: Platform, platform_key: bool, control: bool, shift: bool) -> ClickKind {
    if multi_legacy() {
        return ClickKind::Replace;
    }
    let primary = match platform {
        Platform::MacOs => platform_key && !control,
        Platform::Windows => control && !platform_key,
    };
    match (primary, shift) {
        (false, false) => ClickKind::Replace,
        (true, false) => ClickKind::Toggle,
        (false, true) => ClickKind::Range,
        (true, true) => ClickKind::RangeAdd,
    }
}

/// 選んでいる行（空にはならない = 何も選んでいなければ `Option` の None）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// 選んでいる行（ツリーの並び順・重複なし）
    pub items: Vec<PathBuf>,
    /// ⇧クリックの起点（最後に素で / ⌘で押した行。外した行でもよい = Finder と同じ）
    pub anchor: PathBuf,
    /// 最後に押した行（貼り付けの宛先・右クリックメニューの基準）。必ず `items` に含まれる
    pub lead: PathBuf,
}

impl Selection {
    /// 1 行だけ
    pub fn single(row: &Path) -> Self {
        Self {
            items: vec![row.to_path_buf()],
            anchor: row.to_path_buf(),
            lead: row.to_path_buf(),
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.items.iter().any(|p| p == path)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// いま見えていない行（畳んだフォルダの中・消えた行）を外す。全部消えたら None。
    /// 見えない行を黙ってコピー・ごみ箱へ回さない（ユーザーは見ているものを扱っている）
    pub fn retain_visible(&self, order: &[PathBuf]) -> Option<Self> {
        let items: Vec<PathBuf> = self
            .items
            .iter()
            .filter(|p| order.contains(p))
            .cloned()
            .collect();
        let lead = if items.contains(&self.lead) {
            self.lead.clone()
        } else {
            items.last()?.clone()
        };
        Some(Self {
            items,
            anchor: self.anchor.clone(),
            lead,
        })
    }
}

/// 行を押した後の選択（`order` = いま見えている行の並び = ツリーの上から順）。
/// None = 何も選んでいない（⌘クリックで最後の 1 行を外した）
pub fn apply(
    prev: Option<&Selection>,
    row: &Path,
    kind: ClickKind,
    order: &[PathBuf],
) -> Option<Selection> {
    match (kind, prev) {
        (ClickKind::Replace, _) | (_, None) => Some(Selection::single(row)),
        (ClickKind::Toggle, Some(prev)) => {
            let mut items = prev.items.clone();
            if let Some(i) = items.iter().position(|p| p == row) {
                items.remove(i);
                if items.is_empty() {
                    return None;
                }
                let lead = if prev.lead == row {
                    items.last().cloned().unwrap_or_default()
                } else {
                    prev.lead.clone()
                };
                Some(sorted(
                    Selection {
                        items,
                        anchor: row.to_path_buf(),
                        lead,
                    },
                    order,
                ))
            } else {
                items.push(row.to_path_buf());
                Some(sorted(
                    Selection {
                        items,
                        anchor: row.to_path_buf(),
                        lead: row.to_path_buf(),
                    },
                    order,
                ))
            }
        }
        (ClickKind::Range | ClickKind::RangeAdd, Some(prev)) => {
            let Some(span) = range(order, &prev.anchor, row) else {
                // 起点が見えていない（畳んだ・消えた）: その行だけ（起点も付け替える）
                return Some(Selection::single(row));
            };
            let mut items = if kind == ClickKind::RangeAdd {
                prev.items.clone()
            } else {
                Vec::new()
            };
            for p in span {
                if !items.contains(&p) {
                    items.push(p);
                }
            }
            Some(sorted(
                Selection {
                    items,
                    // 起点は動かさない（続けて ⇧クリックすると同じ起点から引き直す）
                    anchor: prev.anchor.clone(),
                    lead: row.to_path_buf(),
                },
                order,
            ))
        }
    }
}

/// キーで範囲を伸ばす向き（⇧↑ / ⇧↓。#1895）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
}

/// ⇧↑ / ⇧↓ の後の選択（#1895）。最後に押した行（`lead`）の 1 行上 / 下を ⇧クリックしたのと同じ
/// = 起点（`anchor`）からその行までを選ぶ（起点は動かない）。
///
/// 先頭で ⇧↑・末尾で ⇧↓ は何も変えない（`prev` のまま）。最後に押した行が見えていなければ
/// （畳んだフォルダの中・消えた行）どこから伸ばすか決まらないので何も変えない。畳んだフォルダは
/// 1 行なので、範囲はその見出しだけを含む（中は見えていない = まとめた操作もフォルダごと）
pub fn extend(prev: &Selection, step: Step, order: &[PathBuf]) -> Selection {
    let Some(at) = order.iter().position(|p| *p == prev.lead) else {
        return prev.clone();
    };
    let next = match step {
        Step::Up => at.checked_sub(1),
        Step::Down => Some(at + 1).filter(|i| *i < order.len()),
    };
    let Some(next) = next else {
        return prev.clone();
    };
    apply(Some(prev), &order[next], ClickKind::Range, order).unwrap_or_else(|| prev.clone())
}

/// `from` から `to` までの行（両端を含む・上から順）。どちらかが見えていなければ None
pub fn range(order: &[PathBuf], from: &Path, to: &Path) -> Option<Vec<PathBuf>> {
    let a = order.iter().position(|p| p == from)?;
    let b = order.iter().position(|p| p == to)?;
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    Some(order[lo..=hi].to_vec())
}

/// ツリーの並び順へ並べ直す（見えていない行は後ろへ。順序は保つ）
fn sorted(mut sel: Selection, order: &[PathBuf]) -> Selection {
    sel.items
        .sort_by_key(|p| order.iter().position(|o| o == p).unwrap_or(usize::MAX));
    sel
}

/// まとめて扱うときの対象（重複と、ほかに選んだフォルダの配下を外す。順序は保つ）。
///
/// 返り値は（扱うもの, 外したもの）。外したものは親と一緒に扱われる
/// （コピー・移動・ごみ箱は親ごと動くので、配下を別に渡すと 2 回写る・「見つからない」になる）。
/// 比べるのは字面（成分単位の `starts_with` = `/a/b` と `/a/bc` を取り違えない）
pub fn distinct_roots(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut kept: Vec<PathBuf> = Vec::new();
    let mut skipped = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let duplicate = paths[..i].contains(p);
        let under_other = paths.iter().any(|q| q != p && p.starts_with(q));
        if duplicate || under_other {
            if !skipped.contains(p) && !kept.contains(p) {
                skipped.push(p.clone());
            }
        } else {
            kept.push(p.clone());
        }
    }
    (kept, skipped)
}

/// `TAKO_1867_LEGACY=1` で**複数選択・⌥⌘V・コピーの進み具合の帯を外す**（#1867 の A/B）。
/// ⌘クリック・⇧クリックは素の押下と同じ（1 行だけ選ぶ）になり、⌥⌘V はツリーへ向かない。
/// CLI / MCP の `paths`・`paste_move`・`copy_progress` / `copy_cancel` は同じ経路のまま
/// （画面の入口だけを外す）
pub fn multi_legacy() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1867_LEGACY").map(|v| v == "1") == Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// 見えている行の並び（/w の下に a, d, d/x, d/y, e）
    fn order() -> Vec<PathBuf> {
        ["/w", "/w/a", "/w/d", "/w/d/x", "/w/d/y", "/w/e"]
            .into_iter()
            .map(p)
            .collect()
    }

    fn items(sel: &Option<Selection>) -> Vec<String> {
        sel.as_ref()
            .map(|s| s.items.iter().map(|p| p.display().to_string()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn 押し方は主修飾とシフトで決まる() {
        use ClickKind::*;
        let mac = |p, c, s| click_kind(Platform::MacOs, p, c, s);
        let win = |p, c, s| click_kind(Platform::Windows, p, c, s);
        assert_eq!(mac(false, false, false), Replace);
        assert_eq!(mac(true, false, false), Toggle);
        assert_eq!(mac(false, false, true), Range);
        assert_eq!(mac(true, false, true), RangeAdd);
        // macOS の Ctrl は主修飾ではない（⌃クリックは右クリックの代わり）
        assert_eq!(mac(false, true, false), Replace);
        assert_eq!(mac(true, true, false), Replace);
        assert_eq!(win(false, true, false), Toggle);
        assert_eq!(win(false, true, true), RangeAdd);
        // Windows の Win キーは主修飾ではない
        assert_eq!(win(true, false, false), Replace);
        assert!(Replace.is_plain() && !Toggle.is_plain() && !Range.is_plain());
    }

    #[test]
    fn 素の押下はその行だけ() {
        let o = order();
        let sel = apply(None, &p("/w/a"), ClickKind::Replace, &o);
        assert_eq!(items(&sel), ["/w/a"]);
        let sel = apply(sel.as_ref(), &p("/w/e"), ClickKind::Replace, &o);
        assert_eq!(items(&sel), ["/w/e"]);
        let s = sel.unwrap();
        assert_eq!((s.anchor, s.lead), (p("/w/e"), p("/w/e")));
    }

    #[test]
    fn 主修飾クリックで足して外し最後の1行を外すと何も選ばない() {
        let o = order();
        let sel = apply(None, &p("/w/e"), ClickKind::Replace, &o);
        let sel = apply(sel.as_ref(), &p("/w/a"), ClickKind::Toggle, &o);
        // 並びはツリーの上から（押した順ではない）
        assert_eq!(items(&sel), ["/w/a", "/w/e"]);
        assert_eq!(sel.as_ref().unwrap().lead, p("/w/a"));
        let sel = apply(sel.as_ref(), &p("/w/d/x"), ClickKind::Toggle, &o);
        assert_eq!(items(&sel), ["/w/a", "/w/d/x", "/w/e"]);
        // 外す: lead を外したら残りの最後が lead
        let sel = apply(sel.as_ref(), &p("/w/d/x"), ClickKind::Toggle, &o);
        assert_eq!(items(&sel), ["/w/a", "/w/e"]);
        let s = sel.clone().unwrap();
        assert_eq!(s.lead, p("/w/e"));
        assert_eq!(
            s.anchor,
            p("/w/d/x"),
            "外した行も起点になる（Finder と同じ）"
        );
        let sel = apply(sel.as_ref(), &p("/w/a"), ClickKind::Toggle, &o);
        let sel = apply(sel.as_ref(), &p("/w/e"), ClickKind::Toggle, &o);
        assert_eq!(sel, None, "最後の 1 行を外したら何も選ばない");
        // 何も選んでいないときの ⌘クリックはその行を選ぶ
        let sel = apply(None, &p("/w/d"), ClickKind::Toggle, &o);
        assert_eq!(items(&sel), ["/w/d"]);
    }

    #[test]
    fn シフトクリックは起点からの範囲で起点は動かない() {
        let o = order();
        let sel = apply(None, &p("/w/a"), ClickKind::Replace, &o);
        let sel = apply(sel.as_ref(), &p("/w/d/y"), ClickKind::Range, &o);
        assert_eq!(items(&sel), ["/w/a", "/w/d", "/w/d/x", "/w/d/y"]);
        // 同じ起点から引き直す（上へ伸ばすと下は外れる）
        let sel = apply(sel.as_ref(), &p("/w"), ClickKind::Range, &o);
        assert_eq!(items(&sel), ["/w", "/w/a"]);
        let s = sel.clone().unwrap();
        assert_eq!((s.anchor, s.lead), (p("/w/a"), p("/w")));
        // 起点そのもの = 1 行
        let sel = apply(sel.as_ref(), &p("/w/a"), ClickKind::Range, &o);
        assert_eq!(items(&sel), ["/w/a"]);
    }

    #[test]
    fn シフトクリックはそれまでの選択を捨て主修飾シフトは足す() {
        let o = order();
        let sel = apply(None, &p("/w/e"), ClickKind::Replace, &o);
        let sel = apply(sel.as_ref(), &p("/w/a"), ClickKind::Toggle, &o);
        // 起点は /w/a。⇧ で /w/d まで = /w/e は捨てる
        let replaced = apply(sel.as_ref(), &p("/w/d"), ClickKind::Range, &o);
        assert_eq!(items(&replaced), ["/w/a", "/w/d"]);
        // ⌘⇧ は /w/e を残して足す
        let added = apply(sel.as_ref(), &p("/w/d"), ClickKind::RangeAdd, &o);
        assert_eq!(items(&added), ["/w/a", "/w/d", "/w/e"]);
    }

    #[test]
    fn 起点が見えていなければシフトクリックはその行だけ() {
        let o = order();
        let sel = apply(None, &p("/w/d/x"), ClickKind::Replace, &o);
        // /w/d を畳んだ = /w/d/x が見えない
        let folded: Vec<PathBuf> = ["/w", "/w/a", "/w/d", "/w/e"].into_iter().map(p).collect();
        let sel = apply(sel.as_ref(), &p("/w/e"), ClickKind::Range, &folded);
        assert_eq!(items(&sel), ["/w/e"]);
        assert_eq!(sel.unwrap().anchor, p("/w/e"));
        assert_eq!(range(&o, &p("/nope"), &p("/w/a")), None);
    }

    #[test]
    fn 見えていない行は扱う前に外す() {
        let o = order();
        let sel = apply(None, &p("/w/a"), ClickKind::Replace, &o);
        let sel = apply(sel.as_ref(), &p("/w/d/x"), ClickKind::Toggle, &o).unwrap();
        let folded: Vec<PathBuf> = ["/w", "/w/a", "/w/d", "/w/e"].into_iter().map(p).collect();
        let visible = sel.retain_visible(&folded).unwrap();
        assert_eq!(visible.items, vec![p("/w/a")]);
        assert_eq!(visible.lead, p("/w/a"), "lead が隠れたら残りの最後");
        assert_eq!(sel.retain_visible(&[p("/w")]), None);
    }

    #[test]
    fn シフト矢印は最後に押した行から1行ずつ伸びて逆へ打つと縮む() {
        let o = order();
        let sel = apply(None, &p("/w/a"), ClickKind::Replace, &o).unwrap();
        let sel = extend(&sel, Step::Down, &o);
        assert_eq!(sel.items, vec![p("/w/a"), p("/w/d")]);
        assert_eq!(
            (sel.anchor.clone(), sel.lead.clone()),
            (p("/w/a"), p("/w/d"))
        );
        let sel = extend(&sel, Step::Down, &o);
        assert_eq!(sel.items, vec![p("/w/a"), p("/w/d"), p("/w/d/x")]);
        // 逆へ打つと縮む（起点は動かない）
        let sel = extend(&sel, Step::Up, &o);
        assert_eq!(sel.items, vec![p("/w/a"), p("/w/d")]);
        let sel = extend(&sel, Step::Up, &o);
        assert_eq!(sel.items, vec![p("/w/a")]);
        // 起点を越えると反対側へ伸びる
        let sel = extend(&sel, Step::Up, &o);
        assert_eq!(sel.items, vec![p("/w"), p("/w/a")]);
        assert_eq!((sel.anchor.clone(), sel.lead.clone()), (p("/w/a"), p("/w")));
    }

    #[test]
    fn シフト矢印は先頭と末尾で止まり見えない行からは伸ばさない() {
        let o = order();
        let top = apply(None, &p("/w"), ClickKind::Replace, &o).unwrap();
        assert_eq!(
            extend(&top, Step::Up, &o),
            top,
            "先頭で上へ伸ばしても何も変えない"
        );
        let bottom = apply(None, &p("/w/e"), ClickKind::Replace, &o).unwrap();
        assert_eq!(
            extend(&bottom, Step::Down, &o),
            bottom,
            "末尾で下へ伸ばしても何も変えない"
        );
        // 最後に押した行が畳んだフォルダの中 = どこから伸ばすか決まらない
        let hidden = apply(None, &p("/w/d/x"), ClickKind::Replace, &o).unwrap();
        let folded: Vec<PathBuf> = ["/w", "/w/a", "/w/d", "/w/e"].into_iter().map(p).collect();
        assert_eq!(extend(&hidden, Step::Down, &folded), hidden);
        // 畳んだフォルダをまたぐ範囲は見出しの 1 行だけを含む（中は見えていない）
        let a = apply(None, &p("/w/a"), ClickKind::Replace, &folded).unwrap();
        let sel = extend(&extend(&a, Step::Down, &folded), Step::Down, &folded);
        assert_eq!(sel.items, vec![p("/w/a"), p("/w/d"), p("/w/e")]);
        // ⌘クリックで飛び飛びに選んでいても、⇧↓ は起点からの範囲へ置き換える（⇧クリックと同じ）
        let sel = apply(None, &p("/w/e"), ClickKind::Replace, &o);
        let sel = apply(sel.as_ref(), &p("/w/a"), ClickKind::Toggle, &o).unwrap();
        let sel = extend(&sel, Step::Down, &o);
        assert_eq!(sel.items, vec![p("/w/a"), p("/w/d")]);
    }

    #[test]
    fn フォルダと配下を同時に選んだら親だけを扱う() {
        let paths: Vec<PathBuf> = ["/w/d/x", "/w/d", "/w/a", "/w/dd", "/w/d/x/deep", "/w/a"]
            .into_iter()
            .map(p)
            .collect();
        let (kept, skipped) = distinct_roots(&paths);
        // `/w/dd` は `/w/d` の配下ではない（成分単位で比べる）
        assert_eq!(kept, vec![p("/w/d"), p("/w/a"), p("/w/dd")]);
        assert_eq!(skipped, vec![p("/w/d/x"), p("/w/d/x/deep")]);
        assert_eq!(distinct_roots(&[]), (Vec::new(), Vec::new()));
        // 重複は 1 つにする（外したものには載せない = 扱っている）
        let (kept, skipped) = distinct_roots(&[p("/w/a"), p("/w/a")]);
        assert_eq!((kept, skipped), (vec![p("/w/a")], Vec::new()));
    }
}
