//! ジャンプ履歴（戻る / 進む）の純粋なスタック（FR-3.29 / Issue #1677 / #1007 S0-d）
//!
//! 行を指定して開く（`tako open --line` = #1676）・定義ジャンプ（#1680 = S3）で
//! 「飛んだ」あとに、元の場所へ戻るための履歴。`tako_recent`（`recent.json`）は
//! 「最近開いたもの」の一覧で、戻る用途ではない。
//!
//! GPUI にも I/O にも依存しない。ペインが生きているか・ファイルがあるかは呼び手が
//! [`Liveness`] で答え、このモジュールは**その答えに対する方針**だけを持つ
//! （方針を単体テストで固定できるようにするため）。
//!
//! ## 形（ブラウザの戻る / 進むと同じ）
//!
//! - 項目の列と現在位置 1 つ。飛ぶたびに**飛ぶ前にいた場所**と**着地点**を積む
//!   （[`JumpHistory::record_jump`]）。飛ぶ前の場所を積まないと、最初の 1 回目の
//!   ジャンプから戻れない（着地点しか無い）
//! - 戻ってから別の場所へ飛んだら、進む側は捨てる
//! - **連続する同じ場所**（同じファイルの同じ行）は 1 つに畳む。桁とペインは新しいほうで
//!   上書きする（同じ行の別の桁へ飛び直しても、戻る先が 1 つ増えるだけで役に立たない）
//! - 上限（[`DEFAULT_CAPACITY`]）を超えたら古い側から落とす
//!
//! ## 閉じたペイン・消えたファイル（決定）
//!
//! - ペインを閉じても項目は消さない。履歴の単位は「ファイルのどの行か」で、ペインは
//!   そのとき表示していた場所に過ぎない。戻る / 進むで閉じたペインの項目に当たったら
//!   **開き直す**（[`Target::reopen`]）。読み飛ばすと、tako が同じプレビューを差し替えて
//!   使い回す以上「戻る先が黙って消える」ことになり、#1677 が直したい迷子そのものになる
//! - ファイルが消えていた項目は開き直せないので、**読み飛ばして履歴から捨てる**
//!   （[`Navigation::dropped`]）。残すと、戻るを押すたびに同じ失敗を繰り返す

use crate::pane::PaneId;
use std::collections::VecDeque;

/// 履歴の上限（項目数）。vim の jumplist と同じ 100
pub const DEFAULT_CAPACITY: usize = 100;

/// 履歴の 1 項目 = ファイルのどこを見ていたか
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpLocation {
    /// そのとき表示していたペイン（閉じられていても項目は残る）
    pub pane: PaneId,
    /// ファイルの絶対パス（開いたときに正規化済みのもの）
    pub path: String,
    /// 行（1 始まり）。`None` = 行を持たない表示（Markdown のレンダリング表示・
    /// 画像・PDF 等）で、戻るときは行を指定せずに開く（表示種別を code へ倒さない）
    pub line: Option<usize>,
    /// 桁（1 始まりの文字単位）
    pub column: Option<usize>,
}

impl JumpLocation {
    /// 畳むときの「同じ場所」: 同じファイルの同じ行（桁・ペインは見ない）
    pub fn same_place(&self, other: &Self) -> bool {
        self.path == other.path && self.line == other.line
    }
}

/// 呼び手が答える、項目の今の状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// ペインが生きていてファイルもある → そのペインへ出す
    Shown,
    /// ペインは閉じられたがファイルはある → 開き直す
    PaneClosed,
    /// ファイルが無い → 読み飛ばして捨てる
    FileMissing,
}

/// 戻る / 進むで移った先
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub location: JumpLocation,
    /// 項目のペインが閉じられているので、開き直す必要がある
    pub reopen: bool,
}

/// 戻る / 進むの結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Navigation {
    /// 移った先。`None` = もう戻れない / 進めない（位置は変えない）
    pub target: Option<Target>,
    /// 途中で捨てた項目（ファイルが消えていた）。移れなかったときも報告する
    pub dropped: Vec<JumpLocation>,
}

/// 戻る向き / 進む向き
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Back,
    Forward,
}

/// ジャンプ履歴。`entries` が空でなければ `cursor < entries.len()`
#[derive(Debug, Clone)]
pub struct JumpHistory {
    entries: VecDeque<JumpLocation>,
    cursor: usize,
    capacity: usize,
}

impl Default for JumpHistory {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl JumpHistory {
    /// 上限 `capacity` の空の履歴（0 は 1 として扱う = 現在位置だけは持てる）
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            cursor: 0,
            capacity: capacity.max(1),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 古い順の全項目
    pub fn entries(&self) -> impl Iterator<Item = &JumpLocation> {
        self.entries.iter()
    }

    /// 現在位置（古い側から 0 始まり）。空なら `None`
    pub fn cursor(&self) -> Option<usize> {
        (!self.entries.is_empty()).then_some(self.cursor)
    }

    /// 現在位置の項目
    pub fn current(&self) -> Option<&JumpLocation> {
        self.entries.get(self.cursor)
    }

    /// 古い側に項目があるか（ファイルが消えていれば実際には移れないことがある）
    pub fn can_back(&self) -> bool {
        !self.entries.is_empty() && self.cursor > 0
    }

    /// 新しい側に項目があるか
    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    /// 1 回のジャンプを積む。`from` = 飛ぶ前にいた場所（分からなければ `None`）、
    /// `to` = 着地点。現在位置は `to` になる
    pub fn record_jump(&mut self, from: Option<JumpLocation>, to: JumpLocation) {
        if let Some(from) = from {
            self.push(from);
        }
        self.push(to);
    }

    /// 現在位置の後ろへ 1 つ積む（進む側は捨てる・連続する同じ場所は畳む・上限で古い側を落とす）
    fn push(&mut self, location: JumpLocation) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.cursor + 1);
        }
        match self.entries.back_mut() {
            Some(last) if last.same_place(&location) => *last = location,
            _ => self.entries.push_back(location),
        }
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        self.cursor = self.entries.len() - 1;
    }

    /// 戻る
    pub fn back(&mut self, judge: impl FnMut(&JumpLocation) -> Liveness) -> Navigation {
        self.navigate(Direction::Back, judge)
    }

    /// 進む
    pub fn forward(&mut self, judge: impl FnMut(&JumpLocation) -> Liveness) -> Navigation {
        self.navigate(Direction::Forward, judge)
    }

    /// 1 歩移る。ファイルが消えた項目は捨てて次を見る。空・端では何もしない（panic しない）
    pub fn navigate(
        &mut self,
        direction: Direction,
        mut judge: impl FnMut(&JumpLocation) -> Liveness,
    ) -> Navigation {
        let mut dropped = Vec::new();
        loop {
            let candidate = match direction {
                Direction::Back if self.can_back() => self.cursor - 1,
                Direction::Forward if self.can_forward() => self.cursor + 1,
                _ => {
                    return Navigation {
                        target: None,
                        dropped,
                    }
                }
            };
            let reopen = match judge(&self.entries[candidate]) {
                Liveness::FileMissing => {
                    if let Some(gone) = self.entries.remove(candidate) {
                        dropped.push(gone);
                    }
                    // 古い側を 1 つ抜いたので、現在位置の添字も 1 つ手前へずれる
                    if direction == Direction::Back {
                        self.cursor -= 1;
                    }
                    continue;
                }
                Liveness::PaneClosed => true,
                Liveness::Shown => false,
            };
            self.cursor = candidate;
            return Navigation {
                target: Some(Target {
                    location: self.entries[candidate].clone(),
                    reopen,
                }),
                dropped,
            };
        }
    }

    /// 閉じたペイン `old` の項目を、開き直した先 `new` へ付け替える。付け替えた件数を返す。
    ///
    /// 同じペインの別の項目へ戻ったとき、もう一度ペインを増やさず `new` を使い回すため
    pub fn retarget_pane(&mut self, old: PaneId, new: PaneId) -> usize {
        let mut count = 0;
        for entry in self.entries.iter_mut().filter(|e| e.pane == old) {
            entry.pane = new;
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pane: u64, path: &str, line: usize) -> JumpLocation {
        JumpLocation {
            pane: PaneId::from_raw(pane),
            path: path.to_string(),
            line: Some(line),
            column: None,
        }
    }

    fn shown(_: &JumpLocation) -> Liveness {
        Liveness::Shown
    }

    /// 移った先を「パス:行」で表す（`None` = 移れなかった）
    fn step(nav: Navigation) -> Option<String> {
        nav.target.map(|t| {
            format!(
                "{}:{}",
                t.location.path,
                t.location.line.map_or("-".to_string(), |l| l.to_string())
            )
        })
    }

    /// 受け入れ条件 1: 「A へ飛ぶ → B へ飛ぶ → back → back → forward」の位置列が固定値と一致する。
    ///
    /// O（最初にいた場所）から A へ飛び、A から B へ飛ぶ。`record_jump` は飛ぶ前の場所も
    /// 積むので、2 回目の back で O まで戻れる
    #[test]
    fn aへ飛びbへ飛んで戻る戻る進むの位置列() {
        let o = at(1, "/src/main.rs", 120);
        let a = at(1, "/src/a.rs", 10);
        let b = at(1, "/src/b.rs", 50);
        let mut h = JumpHistory::default();
        h.record_jump(Some(o.clone()), a.clone());
        h.record_jump(Some(a.clone()), b.clone());
        let seq = vec![
            step(h.back(shown)),
            step(h.back(shown)),
            step(h.forward(shown)),
        ];
        assert_eq!(
            seq,
            vec![
                Some("/src/a.rs:10".to_string()),
                Some("/src/main.rs:120".to_string()),
                Some("/src/a.rs:10".to_string()),
            ]
        );
        // 2 回目の record で from = A は直前の着地点 A と同じ場所なので畳まれ、3 項目になる
        assert_eq!(h.len(), 3);
        assert_eq!(h.cursor(), Some(1));
    }

    /// 飛ぶ前の場所が分からない（`from = None`）と、最初の着地点より前へは戻れない
    #[test]
    fn 飛ぶ前の場所が無ければ最初の着地点で止まる() {
        let mut h = JumpHistory::default();
        h.record_jump(None, at(1, "/a.rs", 10));
        h.record_jump(None, at(1, "/b.rs", 50));
        assert_eq!(step(h.back(shown)), Some("/a.rs:10".into()));
        assert_eq!(step(h.back(shown)), None, "先頭より前は無い");
        assert_eq!(h.cursor(), Some(0), "移れなければ位置は変えない");
        assert_eq!(step(h.forward(shown)), Some("/b.rs:50".into()));
        assert_eq!(step(h.forward(shown)), None, "末尾より後ろは無い");
    }

    /// 受け入れ条件 2（境界 1）: 上限 N を超えると古い側から落ちる
    #[test]
    fn 上限を超えると古い側から落ちる() {
        let mut h = JumpHistory::new(3);
        for line in 1..=5 {
            h.record_jump(None, at(1, "/a.rs", line * 10));
        }
        let kept: Vec<Option<usize>> = h.entries().map(|e| e.line).collect();
        assert_eq!(kept, vec![Some(30), Some(40), Some(50)]);
        assert_eq!(h.cursor(), Some(2));
        assert_eq!(step(h.back(shown)), Some("/a.rs:40".into()));
        assert_eq!(step(h.back(shown)), Some("/a.rs:30".into()));
        assert_eq!(step(h.back(shown)), None, "落ちた 10 / 20 へは戻れない");
    }

    /// 上限ちょうどでは落とさない（N 個目までは保持する）
    #[test]
    fn 上限ちょうどでは落とさない() {
        let mut h = JumpHistory::new(3);
        for line in 1..=3 {
            h.record_jump(None, at(1, "/a.rs", line));
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.entries().next().and_then(|e| e.line), Some(1));
    }

    /// 受け入れ条件 2（境界 2）: 空の履歴で back / forward を呼んでも panic しない
    #[test]
    fn 空の履歴で戻る進むを呼んでもpanicしない() {
        let mut h = JumpHistory::default();
        assert_eq!(h.back(shown), Navigation::default());
        assert_eq!(h.forward(shown), Navigation::default());
        assert_eq!(h.cursor(), None);
        assert!(h.current().is_none());
        assert!(!h.can_back() && !h.can_forward());
        // 上限 0 を渡されても 1 として扱い、積める
        let mut zero = JumpHistory::new(0);
        zero.record_jump(Some(at(1, "/a.rs", 1)), at(1, "/b.rs", 2));
        assert_eq!(zero.len(), 1);
        assert_eq!(step(zero.back(shown)), None);
    }

    /// 連続する同じ場所（同じファイルの同じ行）は 1 つに畳み、桁とペインは新しいほうで上書きする
    #[test]
    fn 連続する同じ場所は畳む() {
        let mut h = JumpHistory::default();
        h.record_jump(None, at(1, "/a.rs", 42));
        let mut again = at(7, "/a.rs", 42);
        again.column = Some(5);
        h.record_jump(None, again.clone());
        assert_eq!(h.len(), 1, "同じ行へ 2 回飛んでも 1 項目");
        assert_eq!(h.current(), Some(&again), "ペインと桁は新しいほう");
        // 同じファイルでも行が違えば別の場所（同じファイル内の定義ジャンプを畳まない）
        h.record_jump(None, at(7, "/a.rs", 200));
        assert_eq!(h.len(), 2);
        // 連続していなければ畳まない（A → B → A は 3 項目）
        h.record_jump(None, at(7, "/a.rs", 42));
        assert_eq!(h.len(), 3);
    }

    /// 戻ってから別の場所へ飛んだら、進む側は捨てる（ブラウザと同じ）
    #[test]
    fn 戻ってから飛ぶと進む側を捨てる() {
        let mut h = JumpHistory::default();
        h.record_jump(Some(at(1, "/o.rs", 1)), at(1, "/a.rs", 10));
        h.record_jump(Some(at(1, "/a.rs", 10)), at(1, "/b.rs", 20));
        assert_eq!(step(h.back(shown)), Some("/a.rs:10".into()));
        h.record_jump(Some(at(1, "/a.rs", 10)), at(1, "/c.rs", 30));
        let paths: Vec<&str> = h.entries().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/o.rs", "/a.rs", "/c.rs"]);
        assert!(!h.can_forward(), "b へはもう進めない");
    }

    /// 受け入れ条件 3: 閉じたペインを指す項目へ戻ると **開き直す**（読み飛ばさない）。
    /// 位置はその項目へ移り、`reopen = true` が立つ
    #[test]
    fn 閉じたペインの項目へ戻ると開き直す() {
        let mut h = JumpHistory::default();
        // ペイン 1 で O → A、ペイン 2 で A → B と飛んだあと、ペイン 1 を閉じた
        h.record_jump(Some(at(1, "/o.rs", 5)), at(1, "/a.rs", 10));
        h.record_jump(Some(at(1, "/a.rs", 10)), at(2, "/b.rs", 20));
        let closed = PaneId::from_raw(1);
        let judge = |loc: &JumpLocation| {
            if loc.pane == closed {
                Liveness::PaneClosed
            } else {
                Liveness::Shown
            }
        };
        let nav = h.back(judge);
        assert_eq!(
            nav,
            Navigation {
                target: Some(Target {
                    location: at(1, "/a.rs", 10),
                    reopen: true,
                }),
                dropped: vec![],
            }
        );
        assert_eq!(
            h.cursor(),
            Some(1),
            "閉じたペインの項目も読み飛ばさず位置を移す"
        );
        assert_eq!(h.len(), 3, "閉じたペインの項目は消さない");
        // 開き直した先（ペイン 9）へ、閉じたペインの項目をまとめて付け替える
        assert_eq!(h.retarget_pane(closed, PaneId::from_raw(9)), 2);
        let panes: Vec<u64> = h.entries().map(|e| e.pane.as_u64()).collect();
        assert_eq!(panes, vec![9, 9, 2]);
        // 付け替え後は生きているペインとして扱われる（もう開き直さない）
        let nav = h.back(judge);
        assert_eq!(
            nav.target.map(|t| (t.location.path, t.reopen)),
            Some(("/o.rs".into(), false))
        );
    }

    /// ファイルが消えた項目は読み飛ばして履歴から捨て、その先へ移る。捨てたものは報告する
    #[test]
    fn 消えたファイルの項目は読み飛ばして捨てる() {
        let mut h = JumpHistory::default();
        h.record_jump(None, at(1, "/a.rs", 1));
        h.record_jump(None, at(1, "/gone.rs", 2));
        h.record_jump(None, at(1, "/c.rs", 3));
        let missing = |loc: &JumpLocation| {
            if loc.path == "/gone.rs" {
                Liveness::FileMissing
            } else {
                Liveness::Shown
            }
        };
        let nav = h.back(missing);
        assert_eq!(step(nav.clone()), Some("/a.rs:1".into()));
        assert_eq!(nav.dropped, vec![at(1, "/gone.rs", 2)]);
        let paths: Vec<&str> = h.entries().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/a.rs", "/c.rs"]);
        assert_eq!(h.cursor(), Some(0));
        // 進む側でも同じ（捨てたので c へ直接進める）
        assert_eq!(step(h.forward(missing)), Some("/c.rs:3".into()));
    }

    /// 進む側で消えたファイルに当たったとき、現在位置の添字はずれない
    #[test]
    fn 進む側で消えたファイルを捨てても位置はずれない() {
        let mut h = JumpHistory::default();
        h.record_jump(None, at(1, "/a.rs", 1));
        h.record_jump(None, at(1, "/gone.rs", 2));
        h.record_jump(None, at(1, "/c.rs", 3));
        h.back(shown);
        h.back(shown);
        assert_eq!(h.cursor(), Some(0));
        let nav = h.forward(|loc| {
            if loc.path == "/gone.rs" {
                Liveness::FileMissing
            } else {
                Liveness::Shown
            }
        });
        assert_eq!(step(nav), Some("/c.rs:3".into()));
        assert_eq!(h.cursor(), Some(1));
        assert_eq!(h.current().map(|e| e.path.as_str()), Some("/c.rs"));
    }

    /// 全部消えていたら移らずに捨てた分だけ報告する（空になっても panic しない）
    #[test]
    fn 戻る先が全部消えていたら移らず報告だけする() {
        let mut h = JumpHistory::default();
        h.record_jump(Some(at(1, "/gone1.rs", 1)), at(1, "/here.rs", 2));
        let nav = h.back(|loc| {
            if loc.path.starts_with("/gone") {
                Liveness::FileMissing
            } else {
                Liveness::Shown
            }
        });
        assert_eq!(nav.target, None);
        assert_eq!(nav.dropped.len(), 1);
        assert_eq!(h.len(), 1);
        assert_eq!(h.cursor(), Some(0));
        assert_eq!(h.current().map(|e| e.path.as_str()), Some("/here.rs"));
    }

    /// 行を持たない項目（Markdown のレンダリング表示等）も積め、同じファイル同士は畳む
    #[test]
    fn 行を持たない項目も積める() {
        let mut h = JumpHistory::default();
        let readme = JumpLocation {
            pane: PaneId::from_raw(1),
            path: "/README.md".into(),
            line: None,
            column: None,
        };
        h.record_jump(Some(readme.clone()), at(1, "/src/lib.rs", 3));
        assert_eq!(step(h.back(shown)), Some("/README.md:-".into()));
        h.record_jump(Some(readme.clone()), at(1, "/src/lib.rs", 9));
        assert_eq!(h.len(), 2, "README（行なし）は同じ場所として畳まれる");
    }
}
