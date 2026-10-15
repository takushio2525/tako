//! プロファイルの追記（`prompt_blocks.append`）を常時部と on-demand 部に分ける（Issue #1477）
//!
//! master / solo の system prompt は**長寿命セッションが起動した瞬間に払う固定費**で、
//! #1154 で tako 自身が配る手順は `tako orchestrator guide <topic>` へ出した。
//! 残っていたのが**利用者が書いた追記**（個人環境のルール）で、これは tako が中身を
//! 削るわけにいかない。そこで「常に要る規則」と「必要になったら読む詳細」を
//! **1 行のマーカーで区切り**、prompt には前半だけを載せる。
//!
//! # 不変条件
//!
//! - **1 文字も消さない**: 区切りを入れる操作は [`insert_marker`] の**1 行挿入だけ**で、
//!   [`strip_marker`] を通せば元のファイルへ戻る（`strip_marker(new) == old`）。
//!   読む側も [`split`] の `always` + マーカー行 + `on_demand` が原文になる
//! - **索引はファイルへ書かない**: prompt に載せる索引は [`index_text`] が
//!   on-demand 部の見出しから**その場で組み立てる**。正本は利用者のファイル 1 本なので、
//!   あとから見出しを直しても索引が腐らない
//! - **表記ゆれを吸収する**: 利用者が手で書くマーカーなので、`ondemand` 綴り・大文字・
//!   前後の空白は [`is_marker`] の 1 実装で受ける

use crate::platform::support::Note;

/// 常時部と on-demand 部の区切り（**正本**。CLI / 移行 / prompt 生成がここを引く）
pub const MARKER: &str = "<!-- tako:on-demand -->";

/// 索引に添える引き方（prompt は英語なので英語で書く。見出しは利用者の言語のまま）
const FETCH_HINT: &str = "on-demand rules (fetch with tako_orchestrator_guide({ topic: \"local-rules\" }) when you need one)";

/// 区切りの手前（常時部）と後ろ（on-demand 部）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split<'a> {
    /// prompt に載る側
    pub always: &'a str,
    /// `tako orchestrator guide local-rules` で引く側（区切りが無ければ空）
    pub on_demand: &'a str,
    /// 区切りがあったか
    pub marked: bool,
}

/// その行が区切りマーカーか。`<!-- tako:on-demand -->` が正で、
/// `ondemand` 綴り・大文字・前後の空白も受ける（手で書くものなので）
pub fn is_marker(line: &str) -> bool {
    let Some(inner) = line
        .trim()
        .strip_prefix("<!--")
        .and_then(|s| s.strip_suffix("-->"))
    else {
        return false;
    };
    matches!(
        inner.trim().to_ascii_lowercase().as_str(),
        "tako:on-demand" | "tako:ondemand"
    )
}

/// **最初の**マーカーで分ける（2 本目以降は on-demand 部の中身として残す）
pub fn split(text: &str) -> Split<'_> {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if is_marker(line) {
            return Split {
                always: &text[..offset],
                on_demand: &text[offset + line.len()..],
                marked: true,
            };
        }
        offset += line.len();
    }
    Split {
        always: text,
        on_demand: "",
        marked: false,
    }
}

/// マーカー行を**すべて**取り除く（`strip_marker(insert_marker(t)) == t` の検査用）
pub fn strip_marker(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|l| !is_marker(l))
        .collect()
}

/// 見出し行の深さ（`## x` なら 2）。見出しでなければ `None`
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    (1..=6)
        .contains(&hashes)
        .then_some(hashes)
        .filter(|n| line.chars().nth(*n).is_some_and(|c| c == ' ' || c == '\t'))
}

/// 見出しの文言（`## 言語` → `言語`）。索引に使う
pub fn headings(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| heading_level(l).is_some())
        .map(|l| l.trim_start_matches('#').trim())
        .filter(|l| !l.is_empty())
        .collect()
}

/// prompt へ載せる索引（**生成物**。ファイルには書かない）。
/// on-demand 部が空なら空文字（索引も出さない）
pub fn index_text(on_demand: &str) -> String {
    if on_demand.trim().is_empty() {
        return String::new();
    }
    let heads = headings(on_demand);
    if heads.is_empty() {
        return format!("<{FETCH_HINT}>\n");
    }
    format!("<{FETCH_HINT}: {}>\n", heads.join(" / "))
}

/// 追記が prompt に払う量 = `常時部 + 生成される索引`（**取り分と比べる値の正本**）。
/// 区切りを入れる位置（#1477）も、区切りを上げる案（#1981）も、この 1 本で測る
pub fn always_cost(always: &str, on_demand: &str) -> usize {
    always.len() + index_text(on_demand).len()
}

/// 見出し行の行頭バイトオフセット・深さ・行番号（1 始まり）を、`text` の先頭から
/// `end` バイトの手前まで集める。
///
/// **コードフェンス（```` ``` ```` / `~~~`）の中の `#` 行は見出しにしない**（#1981）。
/// シェルの例の `# コメント` を見出しと取り違えると、それが最も浅いレベルになって
/// 区切りの候補を独占し、コードブロックの途中で切ってしまう
fn heading_lines(text: &str, end: usize) -> Vec<(usize, usize, usize)> {
    let mut heads = Vec::new();
    let mut fence: Option<char> = None;
    let mut offset = 0usize;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        if offset >= end {
            break;
        }
        let trimmed = line.trim_start();
        let opener = ['`', '~']
            .into_iter()
            .find(|c| trimmed.starts_with(&c.to_string().repeat(3)));
        match (fence, opener) {
            (None, Some(c)) => fence = Some(c),
            (Some(f), Some(c)) if f == c => fence = None,
            (None, None) => {
                if let Some(level) = heading_level(line) {
                    heads.push((offset, level, i + 1));
                }
            }
            _ => {}
        }
        offset += line.len();
    }
    heads
}

/// 区切りの候補 = 先頭の見出し（題名）を除き、残りの**最も浅いレベル**の見出しの
/// 行頭（バイトオフセットと行番号）。節の途中（深い見出し）では切らない
/// （親の見出しと本文が別れると意味が壊れる）
fn cut_candidates(heads: &[(usize, usize, usize)]) -> Vec<(usize, usize)> {
    let Some(rest) = heads.get(1..) else {
        return Vec::new();
    };
    let Some(min_level) = rest.iter().map(|(_, l, _)| *l).min() else {
        return Vec::new();
    };
    rest.iter()
        .filter(|(_, l, _)| *l == min_level)
        .map(|(off, _, line)| (*off, *line))
        .collect()
}

/// 区切りを入れる位置（バイトオフセット）を決める。
///
/// 見出し境界でしか切らない。ファイル順に常時部へ詰め、
/// **`常時部 + 生成される索引` が `budget` に収まる最後の境界**を選ぶ。
/// 切れないもの（見出しが足りない / 先頭の節だけで既に超過）は `None` = 触らない
/// （機械的に切ると意味が壊れるので `context_budget` の提案へ回す）
pub fn plan_marker_offset(text: &str, budget: usize) -> Option<usize> {
    if split(text).marked {
        return None;
    }
    cut_candidates(&heading_lines(text, text.len()))
        .into_iter()
        .map(|(off, _)| off)
        .rfind(|off| always_cost(&text[..*off], &text[*off..]) <= budget)
}

/// 区切りのある追記の常時部が取り分を超えたときの、区切りを上げる案（#1981）。
///
/// #1477 の自動分割は「区切りがまだ無いファイル」へ 1 回入れるだけなので、その後に
/// 利用者が常時部へ規則を書き足して取り分を超えても誰も直さない。どこまでを常時に
/// 残すかは利用者の判断なので**自動では動かさず**、ここで案を作り、明示の承認
/// （`tako migrate run`）でだけ [`move_marker`] が当てる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerMove {
    /// 何も出さない = 区切りが無い（#1477 の自動分割の受け持ち）か、取り分に収まっている
    Fits,
    /// 見出し境界へ上げれば取り分に収まる
    Move(MovePlan),
    /// 上げても収まらない（常時部に題名以外の見出しが無い / 先頭の節だけで超過）。
    /// 区切りは動かさない（機械的に切ると意味が壊れる）
    CannotFit {
        /// 今の `常時部 + 索引`
        cost: usize,
        budget: usize,
    },
}

/// 区切りをどこへ上げるか（行番号はすべて**動かす前の**ファイルの 1 始まり）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePlan {
    /// 今の `常時部 + 索引`
    pub cost: usize,
    pub budget: usize,
    /// 動かしたあとの `常時部 + 索引`
    pub new_cost: usize,
    /// 今の区切りの行
    pub marker_line: usize,
    /// 区切りをこの見出しの前へ置く（その行）
    pub heading_line: usize,
    /// その見出し行（`## 詳細` のように `#` ごと）
    pub heading: String,
    /// on-demand 部へ移る見出し（`#` を除いた文言。ファイル順）
    pub moved: Vec<String>,
    /// 見出し行の行頭バイトオフセット（[`move_marker`] が使う）
    offset: usize,
}

impl MovePlan {
    /// 取り分の超過バイト数
    pub fn over_bytes(&self) -> usize {
        self.cost.saturating_sub(self.budget)
    }
}

/// 区切りを上げる案を作る（書き換えない）。
///
/// 候補は**今の常時部の中の**見出し境界（題名を除く最も浅いレベル。#1477 と同じ規則）で、
/// 取り分に収まる境界のうち**区切りに最も近いもの**（= 常時部に最も多く残るもの）を選ぶ。
/// 区切りを下げる案は出さない（取り分に収まっていれば何も言わない）
pub fn plan_marker_move(text: &str, budget: usize) -> MarkerMove {
    let s = split(text);
    if !s.marked {
        return MarkerMove::Fits;
    }
    let cost = always_cost(s.always, s.on_demand);
    if cost <= budget {
        return MarkerMove::Fits;
    }
    let marker_at = s.always.len();
    let heads = heading_lines(text, marker_at);
    for (off, line) in cut_candidates(&heads).into_iter().rev() {
        let on_demand = format!("{}{}", &text[off..marker_at], s.on_demand);
        let new_cost = always_cost(&text[..off], &on_demand);
        if new_cost > budget {
            continue;
        }
        let line_at = |o: usize| text[o..].lines().next().unwrap_or_default().trim_end();
        let level = heading_level(line_at(off));
        return MarkerMove::Move(MovePlan {
            cost,
            budget,
            new_cost,
            marker_line: s.always.split_inclusive('\n').count() + 1,
            heading_line: line,
            heading: line_at(off).to_string(),
            // 移る**節**（区切りと同じ深さの見出し）だけを並べる。小見出しまで並べると
            // 案の 1 文が読めなくなる
            moved: heads
                .iter()
                .filter(|(o, l, _)| *o >= off && Some(*l) == level)
                .map(|(o, _, _)| line_at(*o).trim_start_matches('#').trim().to_string())
                .collect(),
            offset: off,
        });
    }
    MarkerMove::CannotFit { cost, budget }
}

/// 案のとおりに区切りを上げた本文（`None` = 案が無い = 触らない）。
///
/// やるのは**区切りの行を抜いて見出しの前へ 1 行入れ直すことだけ**なので、
/// 区切りの行を除いた本文は前後でバイト単位で一致する
/// （`strip_marker(new) == strip_marker(old)`）。表記ゆれの区切り
/// （`<!-- TAKO:ONDEMAND -->` 等）は正本の [`MARKER`] で入れ直す。
/// 2 本目以降の区切りは on-demand 部の中身なので触らない
pub fn move_marker(text: &str, budget: usize) -> Option<String> {
    let MarkerMove::Move(plan) = plan_marker_move(text, budget) else {
        return None;
    };
    let s = split(text);
    let marker_at = s.always.len();
    let marker_end = text.len() - s.on_demand.len();
    Some(format!(
        "{}{MARKER}\n{}{}",
        &text[..plan.offset],
        &text[plan.offset..marker_at],
        &text[marker_end..]
    ))
}

/// 区切りを 1 行だけ入れた本文（`None` = 切れないので触らない）。
/// **挿入するのはマーカー行 1 本だけ**なので、[`strip_marker`] で元へ戻る
pub fn insert_marker(text: &str, budget: usize) -> Option<String> {
    let offset = plan_marker_offset(text, budget)?;
    Some(format!("{}{MARKER}\n{}", &text[..offset], &text[offset..]))
}

/// 分割しても常時部が予算を超えるときの直し方（**具体値つき**）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitAdvice {
    /// 超過バイト数
    pub over_bytes: usize,
    /// 常時部から on-demand 部へ動かす行数の目安（常時部の実際の平均行長から出す）
    pub move_lines: usize,
    pub ja: String,
    pub en: String,
}

/// 「常時部を N バイト（M 行）短くするか on-demand へ動かす」を組み立てる。
///
/// 行数は**その常時部の実測の平均行長**から出す（一般論の定数ではなく、
/// 目の前のファイルで何行動かせば足りるかを答える）。
///
/// `marked` が false = まだ区切りが無い追記なので、**人に手を動かさせる前に
/// 自動移行を案内する**（#916 の原則。手で切るのは最終手段）。
/// `marked` が true でも、手で動かさせる前に `tako migrate` の案（#1981）を案内する
pub fn split_advice(over_bytes: usize, always: &str, marked: bool) -> SplitAdvice {
    let lines = always.lines().count().max(1);
    let avg = (always.len() / lines).max(1);
    let move_lines = over_bytes.div_ceil(avg).max(1);
    let (ja, en) = if marked {
        (
            format!(
                "常時部が {over_bytes} バイト超過している。区切り `{MARKER}` を見出し境界へ上げる案を `tako migrate` が出す（承認すると `tako migrate run` で動く。上げた節は on-demand 部 = `tako orchestrator guide local-rules` で引け、内容は 1 文字も消えない）。案が出ない（上げても収まらない）ときは区切りを {move_lines} 行ぶん上へ自分で動かすか、常時部を {over_bytes} バイト短くする"
            ),
            format!(
                "The always-on part is {over_bytes} bytes over. `tako migrate` proposes moving the `{MARKER}` divider up to a heading boundary (approve it with `tako migrate run`; the moved sections stay in the on-demand part, fetched with `tako orchestrator guide local-rules`, and nothing is deleted). If no proposal appears (moving up cannot make it fit), move the divider up about {move_lines} lines yourself or shorten the always-on part by {over_bytes} bytes"
            ),
        )
    } else {
        (
            format!(
                "追記に区切りがまだ無い。`tako migrate run`（`tako setup` と起動時にも自動で走る）が見出し境界へ `{MARKER}` を 1 行入れ、その行より後ろは `tako orchestrator guide local-rules` で引く形になる（内容は 1 文字も消えない）。切れない場合は自分で `{MARKER}` を {move_lines} 行ぶん手前へ置く"
            ),
            format!(
                "This append has no divider yet. `tako migrate run` (it also runs from `tako setup` and at startup) inserts one `{MARKER}` line at a heading boundary, and everything after it is then fetched with `tako orchestrator guide local-rules` (nothing is deleted). If it cannot cut, place `{MARKER}` yourself about {move_lines} lines earlier"
            ),
        )
    };
    SplitAdvice {
        over_bytes,
        move_lines,
        ja,
        en,
    }
}

/// 区切りが無い追記に対する案内（移行が切れなかったとき）
pub const CANNOT_SPLIT: Note = Note::new(
    "追記が予算を超えているが、見出し境界で切れなかった（見出しが無いか、先頭の節だけで超過している）。`<!-- tako:on-demand -->` を自分で入れると、その行より後ろは常時読み込まれなくなる",
    "The append is over budget but could not be cut at a heading boundary (no headings, or the first section alone is already over). Insert `<!-- tako:on-demand -->` yourself and everything after that line stops being loaded every turn",
);

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# 題名\n\n前書き\n\n## A\n\naaa\n\n## B\n\nbbb\n\n## C\n\nccc\n";

    #[test]
    fn マーカーの表記ゆれを吸収する() {
        assert!(is_marker("<!-- tako:on-demand -->"));
        assert!(is_marker("  <!--tako:on-demand-->  "));
        assert!(is_marker("<!-- TAKO:ONDEMAND -->"));
        assert!(!is_marker("<!-- tako:context-budget-rule -->"));
        assert!(!is_marker("## tako:on-demand"));
    }

    #[test]
    fn 区切りの前後に分かれる() {
        let text = "常時\n<!-- tako:on-demand -->\n詳細\n";
        let s = split(text);
        assert_eq!(s.always, "常時\n");
        assert_eq!(s.on_demand, "詳細\n");
        assert!(s.marked);
        // 区切りが無ければ全部が常時部
        let s2 = split("常時だけ\n");
        assert_eq!(s2.always, "常時だけ\n");
        assert_eq!(s2.on_demand, "");
        assert!(!s2.marked);
    }

    #[test]
    fn 二本目のマーカーはon_demand部の中身として残る() {
        let text = "a\n<!-- tako:on-demand -->\nb\n<!-- tako:on-demand -->\nc\n";
        let s = split(text);
        assert_eq!(s.always, "a\n");
        assert_eq!(s.on_demand, "b\n<!-- tako:on-demand -->\nc\n");
    }

    /// SAMPLE を切ったときの `常時部 + 索引` の実寸（境界ごと）。
    /// 索引の文言が変わっても意図（どの境界が選ばれるか）が読めるよう実測で持つ
    const FITS_FIRST_CUT: usize = 131;
    const FITS_LAST_CUT: usize = 145;

    #[test]
    fn 挿入は1行だけで元へ戻せる() {
        let out = insert_marker(SAMPLE, FITS_LAST_CUT).expect("切れる");
        assert_eq!(
            strip_marker(&out),
            SAMPLE,
            "マーカー行以外は 1 文字も変えない"
        );
        assert_eq!(out.lines().count(), SAMPLE.lines().count() + 1);
        // 連結すると原文（マーカー行を除く）に戻る
        let s = split(&out);
        assert_eq!(format!("{}{}", s.always, s.on_demand), SAMPLE);
    }

    #[test]
    fn 予算に収まる最後の境界で切る() {
        // 予算を大きく取れば後ろの境界（常時部が長い）を選ぶ
        let big = insert_marker(SAMPLE, FITS_LAST_CUT).expect("切れる");
        assert!(big.contains("ccc"), "{big}");
        let idx = big.find(MARKER).unwrap();
        assert!(big[..idx].contains("## B"), "最後の境界で切る: {big}");
        // 予算が 1 バイト足りなければ 1 つ前の境界へ下がる
        let mid = insert_marker(SAMPLE, FITS_LAST_CUT - 1).unwrap();
        let idx = mid.find(MARKER).unwrap();
        assert!(
            mid[..idx].contains("## A") && !mid[..idx].contains("## B"),
            "{mid}"
        );
        // 予算が小さければ先頭の境界（題名と前書きだけが常時部）
        let small = insert_marker(SAMPLE, FITS_FIRST_CUT).unwrap();
        let idx = small.find(MARKER).unwrap();
        assert!(!small[..idx].contains("## A"), "先頭の境界で切る: {small}");
        // 先頭の境界すら収まらなければ切らない
        assert!(insert_marker(SAMPLE, FITS_FIRST_CUT - 1).is_none());
    }

    #[test]
    fn 切れないものは触らない() {
        // 見出しが題名だけ
        assert!(insert_marker("# 題名\n本文\n", 10).is_none());
        // 見出しが 1 つも無い
        assert!(insert_marker("ただの本文\n", 1).is_none());
        // 先頭の節だけで予算超過（索引を足すと収まらない）
        assert!(insert_marker(SAMPLE, 1).is_none());
        // 既に区切りがある
        assert!(insert_marker("a\n<!-- tako:on-demand -->\n## B\n## C\n", 5).is_none());
    }

    #[test]
    fn 索引は見出しから組み立てる() {
        let marked = insert_marker(SAMPLE, FITS_FIRST_CUT).unwrap();
        let s = split(&marked);
        let idx = index_text(s.on_demand);
        assert!(idx.contains("local-rules"), "{idx}");
        assert!(idx.contains("A / B / C"), "{idx}");
        assert!(idx.ends_with('\n'));
        // 見出しが無い on-demand 部でも引き方だけは出す
        assert!(index_text("本文だけ\n").contains("local-rules"));
        // 空なら索引も出さない
        assert_eq!(index_text("  \n"), "");
    }

    #[test]
    fn 助言は実測の平均行長から行数を出す() {
        // 1 行 11 バイト（"0123456789\n"）× 10 行
        let always = "0123456789\n".repeat(10);
        let a = split_advice(30, &always, true);
        assert_eq!(a.over_bytes, 30);
        assert_eq!(a.move_lines, 3, "30 バイト / 平均 11 バイト = 3 行");
        assert!(a.ja.contains("3 行"), "{}", a.ja);
        assert!(a.en.contains("3 lines"), "{}", a.en);
        // 区切りがまだ無いときは**手で切らせる前に**自動移行を案内する
        let none = split_advice(30, &always, false);
        assert!(none.ja.contains("tako migrate run"), "{}", none.ja);
        assert!(none.en.contains("tako migrate run"), "{}", none.en);
        // 空でも 0 除算しない
        assert_eq!(split_advice(1, "", true).move_lines, 1);
    }

    // ─── #1981: 区切りを上げる案 ─────────────────────────────────────

    /// 区切りの手前に 3 節（A / B / C）、後ろに 1 節（D）
    const MARKED: &str = "# 題名\n\n前書き\n\n## A\n\naaa\n\n## B\n\nbbb\n\n## C\n\nccc\n<!-- tako:on-demand -->\n## D\n\nddd\n";

    fn cost_of(text: &str) -> usize {
        let s = split(text);
        always_cost(s.always, s.on_demand)
    }

    fn plan(text: &str, budget: usize) -> MovePlan {
        match plan_marker_move(text, budget) {
            MarkerMove::Move(p) => p,
            other => panic!("案が出ない: {other:?}"),
        }
    }

    #[test]
    fn 取り分を超えたら区切りに最も近い見出しの前へ上げる案を出す() {
        let cost = cost_of(MARKED);
        // 1 バイト超過 = 1 節（C）を下ろせば足りる
        let p = plan(MARKED, cost - 1);
        assert_eq!(p.heading, "## C");
        assert_eq!(p.heading_line, 13);
        assert_eq!(p.marker_line, 16);
        assert_eq!(p.over_bytes(), 1);
        assert_eq!(p.moved, vec!["C"]);
        assert!(p.new_cost <= p.budget, "{p:?}");
        assert_eq!(cost_of(&move_marker(MARKED, cost - 1).unwrap()), p.new_cost);
        // C を下ろしても収まらなければ 1 つ手前（B）へ
        let p2 = plan(MARKED, p.new_cost - 1);
        assert_eq!(p2.heading, "## B");
        assert_eq!(p2.moved, vec!["B", "C"]);
    }

    #[test]
    fn 上げても区切りの行を除いた本文はバイト単位で一致する() {
        let budget = cost_of(MARKED) - 1;
        let moved = move_marker(MARKED, budget).expect("上げられる");
        assert_eq!(strip_marker(&moved), strip_marker(MARKED));
        assert_eq!(moved.len(), MARKED.len(), "区切りの行が 1 本のまま");
        assert_eq!(moved.matches(MARKER).count(), 1);
        let s = split(&moved);
        assert!(s.always.ends_with("bbb\n\n"), "{moved}");
        assert!(s.on_demand.starts_with("## C\n"), "{moved}");
        // 当てたあとは取り分に収まる = もう案は出ない（2 回目は no-op）
        assert_eq!(plan_marker_move(&moved, budget), MarkerMove::Fits);
        assert_eq!(move_marker(&moved, budget), None);
    }

    #[test]
    fn 取り分内か区切りが無ければ何も出さない() {
        assert_eq!(
            plan_marker_move(MARKED, cost_of(MARKED)),
            MarkerMove::Fits,
            "ちょうど取り分なら超過ではない"
        );
        // 区切りが無いものは #1477 の自動分割の受け持ち（ここでは何も言わない）
        assert_eq!(plan_marker_move(SAMPLE, 1), MarkerMove::Fits);
        assert_eq!(move_marker(SAMPLE, 1), None);
    }

    #[test]
    fn 上げても収まらなければ動かさない() {
        // 常時部に見出しが 1 つも無い
        let flat = format!("{}{MARKER}\n## D\nddd\n", "本文\n".repeat(50));
        assert!(matches!(
            plan_marker_move(&flat, 10),
            MarkerMove::CannotFit { .. }
        ));
        assert_eq!(move_marker(&flat, 10), None);
        // 題名と先頭の節だけで取り分を超える（どの境界へ上げても収まらない）
        let fat = format!(
            "# 題名\n{}\n## A\na\n{MARKER}\n## D\nddd\n",
            "x".repeat(500)
        );
        match plan_marker_move(&fat, 100) {
            MarkerMove::CannotFit { cost, budget } => {
                assert_eq!(cost, cost_of(&fat));
                assert_eq!(budget, 100);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(move_marker(&fat, 100), None);
        // 区切りが先頭 = 常時部が空。索引だけなら収まる（何も出さない）
        let top = format!("{MARKER}\n## A\naaa\n## B\nbbb\n");
        assert_eq!(plan_marker_move(&top, 1000), MarkerMove::Fits);
        // 索引だけで超えても、上げる先が無いので動かさない
        assert!(matches!(
            plan_marker_move(&top, 1),
            MarkerMove::CannotFit { .. }
        ));
    }

    #[test]
    fn コードフェンスの中のシャープ行は見出しにしない() {
        let text = "# 題名\n\n## A\n\n```bash\n# コメント\necho\n```\n\n## B\n\nbbb\n<!-- tako:on-demand -->\n## C\n";
        let p = plan(text, cost_of(text) - 1);
        assert_eq!(
            p.heading, "## B",
            "フェンス内の `# コメント` で切った: {p:?}"
        );
        // #1477 の自動分割も同じ走査を通る（フェンスの中では切らない）
        let unmarked = strip_marker(text);
        let b = unmarked.find("## B").unwrap();
        let budget = always_cost(&unmarked[..b], &unmarked[b..]);
        let off = plan_marker_offset(&unmarked, budget);
        assert_eq!(off, Some(b), "フェンス内の `# コメント` で切った");
    }

    #[test]
    fn 表記ゆれの区切りは正本で入れ直し_2本目の区切りは残す() {
        let text = "# 題名\n\n## A\n\naaa\n\n## B\n\nbbb\n<!-- TAKO:ONDEMAND -->\n## C\n\nccc\n<!-- tako:on-demand -->\n## D\n";
        let moved = move_marker(text, cost_of(text) - 1).expect("上げられる");
        assert!(!moved.contains("TAKO:ONDEMAND"), "{moved}");
        assert_eq!(
            moved.matches(MARKER).count(),
            2,
            "2 本目は on-demand 部の中身"
        );
        assert_eq!(strip_marker(&moved), strip_marker(text));
    }

    #[test]
    fn 末尾の改行が無い区切りでも本文は変わらない() {
        // on-demand 部が空 = 索引が無い。上げると索引（約 100 バイト）が増えるので、
        // それより大きい節を下ろす形にする
        let text = format!(
            "# 題名\n\n## A\n\naaa\n\n## B\n\n{}\n{MARKER}",
            "b".repeat(300)
        );
        let text = text.as_str();
        let moved = move_marker(text, cost_of(text) - 1).expect("上げられる");
        assert_eq!(strip_marker(&moved), strip_marker(text));
        assert!(split(&moved).on_demand.starts_with("## B"), "{moved}");
    }

    #[test]
    fn 移る節は区切りと同じ深さの見出しだけを並べる() {
        let text = "# 題名\n\n## A\n\naaa\n\n## B\n\n### B-1\n\nb\n\n## C\n\nccc\n<!-- tako:on-demand -->\n## D\n";
        let c = plan(text, cost_of(text) - 1);
        let p = plan(text, c.new_cost - 1);
        assert_eq!(p.heading, "## B");
        assert_eq!(p.moved, vec!["B", "C"], "小見出し B-1 は並べない");
    }
}
