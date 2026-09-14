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
    // 行頭のバイトオフセットと見出しの深さ
    let mut heads: Vec<(usize, usize)> = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if let Some(level) = heading_level(line) {
            heads.push((offset, level));
        }
        offset += line.len();
    }
    // 先頭の見出し（題名）では切らない。残りの最も浅いレベルだけを境界にする
    let rest = heads.get(1..)?;
    let min_level = rest.iter().map(|(_, l)| *l).min()?;
    let cuts: Vec<usize> = rest
        .iter()
        .filter(|(_, l)| *l == min_level)
        .map(|(off, _)| *off)
        .collect();
    cuts.into_iter()
        .rfind(|off| text[..*off].len() + index_text(&text[*off..]).len() <= budget)
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
/// 自動移行を案内する**（#916 の原則。手で切るのは最終手段）
pub fn split_advice(over_bytes: usize, always: &str, marked: bool) -> SplitAdvice {
    let lines = always.lines().count().max(1);
    let avg = (always.len() / lines).max(1);
    let move_lines = over_bytes.div_ceil(avg).max(1);
    let (ja, en) = if marked {
        (
            format!(
                "常時部が {over_bytes} バイト超過している。区切り `{MARKER}` を {move_lines} 行ぶん上へ動かして詳細を on-demand 部（`tako orchestrator guide local-rules` で引ける）へ移すか、常時部を {over_bytes} バイト短くする"
            ),
            format!(
                "The always-on part is {over_bytes} bytes over. Move the `{MARKER}` divider up by about {move_lines} lines so the detail sits in the on-demand part (fetched with `tako orchestrator guide local-rules`), or shorten the always-on part by {over_bytes} bytes"
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
}
