//! 本番コードだけの眺めを作る（番犬テストの共有部品）
//!
//! 走査型の番犬は「テストコードを検査対象から外す」ために、ソースを
//! `#[cfg(test)]` の位置で**切って**いた。この切り方はファイル中で
//! **最初に現れる `#[cfg(test)]` まで**しか残さないので、テスト用ヘルパの
//! `#[cfg(test)]` が 1 つ途中にあるだけで、それ以降の本番コードが番犬の
//! 視界から丸ごと消える（#1420）。#1403 の作業中に `remote.rs` で実測したときは、
//! 検査対象より後ろへ 4 行のヘルパを置いただけで走査範囲が 5,903 行 → 3,124 行
//! （全体の 70.7% → 37.1%）へ縮み、**番犬は 7 本とも緑のまま**だった。
//!
//! ここは**切らない**。テスト領域だけを空白へ潰して、残り全部を返す。
//!
//! - 途中にヘルパが 1 つあっても、消えるのは**そのヘルパだけ**
//! - テストモジュールが複数あっても、**あいだの本番コードは残る**
//! - **バイト長と行番号が保たれる**ので `file:line` の名指しがそのまま使える
//! - 潰す範囲は `code_view` で決めるが**中身は原文のまま**返すので、
//!   文字列リテラルを見る番犬（`body.contains("tako remote start")` など）が壊れない
//!
//! 「黙って縮んだ」の検出は [`production`] / [`production_with_floor`] が持つ
//! （本番コードが下限を割ったら、潰した先頭の領域を `file:line` で名指して落ちる）。
//! テスト側を見張る番犬のために裏返し（[`tests_only`]）も同じ 1 実装から出す。

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

// 位置決めだけに使う（コメント・文字列の中の `#[cfg(test)]` や `}` を数えないため）
#[path = "code_view.rs"]
mod code_view;

const ATTR: &str = "#[cfg(test)]";

/// 本番コードだけの眺めと、潰したテスト領域の内訳
pub struct Production {
    /// テスト領域を空白へ潰した本文（**原文とバイト長・行番号が同じ**）
    pub text: String,
    /// 潰した領域（`(開始行, 終了行)`。1 始まり・両端を含む）
    pub regions: Vec<(usize, usize)>,
    /// 潰したバイト数（改行を含む素の範囲長）
    pub removed_bytes: usize,
}

impl Production {
    /// 残った本番コードのバイト数
    pub fn kept_bytes(&self) -> usize {
        self.text.len().saturating_sub(self.removed_bytes)
    }

    /// 本番コードがファイル全体に占める割合（0.0〜1.0）
    pub fn coverage(&self) -> f64 {
        if self.text.is_empty() {
            return 1.0;
        }
        self.kept_bytes() as f64 / self.text.len() as f64
    }

    /// 潰した領域を人が読める形にする（落ちたときの手がかり）
    pub fn describe_regions(&self) -> String {
        if self.regions.is_empty() {
            return "なし".to_string();
        }
        self.regions
            .iter()
            .map(|(a, b)| format!("{a}〜{b} 行"))
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// 本番コードが全体に占める割合の既定の下限。
///
/// これを割るのは「テストが本番より厚い」ではなく「範囲取りが本番コードを
/// 飲み込んだ」ときなので落とす。下限を使っている番犬が読むファイルの現行最小は
/// `url_guard.rs` の 50.9% なので余裕がある。
///
/// ただしリポジトリ全体では `tmux_backend.rs` が 23.1%・`mcp/tests.rs` が 0.0%
/// （ファイルまるごとテスト）なので、**テストの厚いファイルを読む番犬は
/// [`production_with_floor`] で明示的に下げる**か、下限を使わず [`scan`] を直に呼ぶ
/// （`platform_parity` は 4 クレートの `src` を丸ごと走査するので後者）
pub const MIN_COVERAGE: f64 = 0.30;

/// テスト領域（`#[cfg(test)]` が付いた item）を空白へ潰した本文を返す。
///
/// 潰すのは属性行の行頭から item の終わり（対応する `}` か、宣言なら `;`）まで。
/// `#[cfg(test)] mod tests` も `#[cfg(test)] fn helper()` も
/// `#[cfg(test)] stmt();` も同じ扱いで、**どれが先に来ても残りの本番コードは消えない**
pub fn scan(src: &str) -> Production {
    let view = code_view::code_view(src);
    let ranges = test_ranges(&view);
    let mut out = src.as_bytes().to_vec();
    let mut removed = 0usize;
    for &(start, end) in &ranges {
        blank(&mut out, start, end);
        removed += end - start;
    }
    Production {
        text: to_text(out),
        regions: ranges
            .iter()
            .map(|&(s, e)| (line_at(&view, s), line_at(&view, e.saturating_sub(1))))
            .collect(),
        removed_bytes: removed,
    }
}

/// [`scan`] の裏返し: **テスト領域だけ**を残し、本番コードを空白へ潰した本文。
///
/// テスト側を見張る番犬（「テストが `Instant` を使っていない」等）が使う。
/// 行番号が保たれるので、こちらも `file:line` でそのまま名指しできる
pub fn tests_only(src: &str) -> String {
    let view = code_view::code_view(src);
    let mut out = src.as_bytes().to_vec();
    let mut cursor = 0usize;
    for (start, end) in test_ranges(&view) {
        blank(&mut out, cursor, start);
        cursor = end;
    }
    let len = out.len();
    blank(&mut out, cursor, len);
    to_text(out)
}

/// [`scan`] + 「黙って縮んでいない」ことの確認（下限は [`MIN_COVERAGE`]）
pub fn production(src: &str, rel: &str) -> String {
    production_with_floor(src, rel, MIN_COVERAGE)
}

/// [`scan`] + 下限を指定した確認。
///
/// 落ちるときは**潰した先頭の領域**を `rel:line` で名指す（どこから飲み込まれたかが
/// そのまま出る）。走査範囲が空になる形（目印の消失・ファイル改名）も同時に落とす
pub fn production_with_floor(src: &str, rel: &str, floor: f64) -> String {
    let scanned = scan(src);
    let head = scanned.regions.first().map(|(a, _)| *a).unwrap_or(1);
    assert!(
        scanned.kept_bytes() > 1000,
        "{rel}:{head} 走査範囲が空（本番コードが {} バイトしか残っていない）。\n\
         潰した領域: {}",
        scanned.kept_bytes(),
        scanned.describe_regions()
    );
    assert!(
        scanned.coverage() >= floor,
        "{rel}:{head} 走査範囲が全体の {:.1}% まで縮んだ（下限 {:.1}%）。\n\
         `#[cfg(test)]` の範囲取りが本番コードを飲み込んでいる（#1420）。\n\
         潰した領域: {}",
        scanned.coverage() * 100.0,
        floor * 100.0,
        scanned.describe_regions()
    );
    scanned.text
}

/// `#[cfg(test)]` が付いた item のバイト範囲（`code_view` 上で数える）
fn test_ranges(view: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    while let Some(hit) = view[cursor..].find(ATTR) {
        let at = cursor + hit;
        // 行頭に置かれた item 属性だけを見る（`#![cfg(test)]` や式中の綴りは対象外）
        let line_start = view[..at].rfind('\n').map(|n| n + 1).unwrap_or(0);
        if !view[line_start..at].trim().is_empty() {
            cursor = at + ATTR.len();
            continue;
        }
        let end = item_end(view, at + ATTR.len());
        ranges.push((line_start, end));
        cursor = end;
    }
    ranges
}

/// 属性の直後から item の終わりまで。対応する `}` か、宣言なら深さ 0 の `;`
fn item_end(view: &str, from: usize) -> usize {
    let bytes = view.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            // 釣り合わない `}`（切り出しに失敗）はそこで打ち切る
            b'}' if depth <= 1 => return i + 1,
            b'}' => depth -= 1,
            b';' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// `[start, end)` を空白で潰す（**改行は残す** = 行番号が保たれる）
fn blank(out: &mut [u8], start: usize, end: usize) {
    let end = end.min(out.len());
    for byte in &mut out[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// 位置決めに使う眺め（番犬が「コードとして残っているか」を見るときにも使う）
pub fn code_view_of(src: &str) -> String {
    code_view::code_view(src)
}

fn to_text(out: Vec<u8>) -> String {
    String::from_utf8(out).expect("空白で潰しても UTF-8 は壊れない")
}

/// バイト位置の行番号（1 始まり）
fn line_at(view: &str, offset: usize) -> usize {
    view[..offset.min(view.len())].matches('\n').count() + 1
}
