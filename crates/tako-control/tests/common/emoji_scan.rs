//! 「本番コードの文字列リテラルに絵文字が無いか」の走査（番犬テストの共有部品）
//!
//! tako は UI にも CLI 出力にも絵文字を置かない（ユーザーの確定方針。#217 / #1536 / #1578）。
//! これを見張る番犬が 2 本ある。
//!
//! - [`issue1536_no_emoji_ui_watchdog`] — `crates/tako-app/src`（GUI が描く文字列）
//! - [`issue1578_no_emoji_cli_watchdog`] — `crates/tako-cli/src` / `crates/tako-control/src`
//!   （CLI が標準出力・標準エラーへ出す文字列）
//!
//! 走査そのものを番犬ごとに書き直すと、**片方だけが `\u{XXXX}` 表記を取りこぼす**
//! ような食い違いが起きる（`code_view` / `production_range` を 1 実装にしたのと同じ理由）。
//! ここに 1 本だけ置き、番犬側は「どこを見るか」と「何を許すか」だけを持つ。
//!
//! # 何を見るか
//!
//! **本番コードの文字列 / 文字リテラルの中身だけ**。
//! [`production_range::scan`] でテスト領域を空白へ潰してから
//! [`code_view::literals_only`] を掛ける。
//!
//! - **コメントは対象外**。画面にも端末にも出ないし、規約や理由の説明文に禁止文字
//!   そのものを書けなくなると「何を禁じているか」をソースに残せない
//! - **`#[cfg(test)]` の中も対象外**。範囲取りは `production_range::scan` の 1 実装を
//!   通す（`src.find("#[cfg(test)]")` で切ると途中のヘルパ以降が視界から消える = #1420）
//! - 生の文字だけでなく **`\u{XXXX}` の書き方も復号して見る**（生の文字を避けて
//!   書けば通る、という抜け道を残さない）
//! - 「絵文字か」の判定は [`tako_core::emoji::is_emoji`] の **1 実装**を通す

// 取り込む番犬ごとに使う関数が違う（片方しか使わないファイルがある）
#![allow(dead_code)]

// 取り込む側から**二重に宣言させない**ため公開する（`clippy::duplicate_mod`）
#[path = "production_range.rs"]
pub mod production_range;

use production_range::code_view;
use std::path::{Path, PathBuf};

/// リポジトリルート（`crates/tako-control` の 2 つ上）
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// 走査対象（`roots` 以下の全 `.rs`）を `(リポジトリ相対パス, 本文)` で返す。
/// パス区切りは `/` へ正規化するので、許可リストの綴りが OS で変わらない
pub fn sources_under(roots: &[&str]) -> Vec<(String, String)> {
    let root = repo_root();
    let mut out = Vec::new();
    for rel_root in roots {
        let mut stack = vec![root.join(rel_root)];
        while let Some(dir) = stack.pop() {
            let entries =
                std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?} を読めない: {e}"));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path
                        .strip_prefix(&root)
                        .expect("リポジトリ内")
                        .to_string_lossy()
                        .replace('\\', "/");
                    let src = std::fs::read_to_string(&path).expect("UTF-8 のソース");
                    out.push((rel, src));
                }
            }
        }
    }
    out.sort();
    out
}

/// 見つけた 1 件（`file:line` で名指しするための最小の情報）
#[derive(Debug, Clone)]
pub struct Hit {
    /// リポジトリルートからの相対パス
    pub rel: String,
    /// 1 始まりの行番号
    pub line: usize,
    /// 見つけた文字
    pub ch: char,
    /// `\u{XXXX}` の書き方で見つけたか（報告に出す）
    pub escaped: bool,
}

impl Hit {
    pub fn describe(&self) -> String {
        let how = if self.escaped {
            r"（\u{} 表記）"
        } else {
            ""
        };
        format!(
            "{}:{}: {:?} (U+{:04X}){}",
            self.rel, self.line, self.ch, self.ch as u32, how
        )
    }
}

/// `\u{XXXX}` を復号して返す（`}` が無い・16 進でない書き方は無視する）
pub fn unicode_escapes(line: &str) -> Vec<char> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 < b.len() {
        if b[i] == b'\\' && b[i + 1] == b'u' && b[i + 2] == b'{' {
            let start = i + 3;
            let mut j = start;
            // 16 進の続きは ASCII なので、多バイト文字の途中で切れることはない
            while j < b.len() && b[j] != b'}' {
                j += 1;
            }
            if j > start && j < b.len() {
                if let Ok(cp) = u32::from_str_radix(&line[start..j], 16) {
                    if let Some(c) = char::from_u32(cp) {
                        out.push(c);
                    }
                }
            }
            i = j.max(start);
            continue;
        }
        i += 1;
    }
    out
}

/// 1 ファイルぶんの違反（**本番コードの文字列リテラルの中身だけ**を見る）
pub fn hits_in(rel: &str, src: &str) -> Vec<Hit> {
    // 下限は掛けない: `main.rs` のようにテストの厚いファイルまで舐めるため
    // （黙って縮んでいないことは各番犬の `走査範囲が黙って消えていない` が見る）
    let production = production_range::scan(src).text;
    let view = code_view::literals_only(&production);
    let mut out = Vec::new();
    for (idx, line) in view.lines().enumerate() {
        let no = idx + 1;
        for ch in line.chars() {
            if tako_core::emoji::is_emoji(ch) {
                out.push(Hit {
                    rel: rel.to_string(),
                    line: no,
                    ch,
                    escaped: false,
                });
            }
        }
        for ch in unicode_escapes(line) {
            if tako_core::emoji::is_emoji(ch) {
                out.push(Hit {
                    rel: rel.to_string(),
                    line: no,
                    ch,
                    escaped: true,
                });
            }
        }
    }
    out
}

/// 絵文字を残してよい場所。**`why` が空の行は無効**（各番犬の検査が落とす）
pub struct Allow {
    /// リポジトリルートからの相対パス
    pub rel: &'static str,
    /// 許す文字
    pub ch: char,
    /// そのファイルにその文字が出る件数（増減したら落とす = 見直しを強制する）
    pub count: usize,
    /// なぜ「tako が出す文字列の絵文字」ではないのか
    pub why: &'static str,
}

/// 許可リストへ突き合わせた結果の問題点を組み立てる。
///
/// - 許可に無い違反 → `advice` を添えて 1 件ずつ名指し
/// - 許可はあるが件数が違う → 増減を名指し（**同じ文字が 1 個増えても落ちる**）
pub fn problems(hits: &[Hit], allow: &[Allow], advice: &str) -> Vec<String> {
    let mut unexpected: Vec<&Hit> = Vec::new();
    let mut counted: Vec<(&Allow, Vec<&Hit>)> = allow.iter().map(|a| (a, Vec::new())).collect();
    for hit in hits {
        match counted
            .iter_mut()
            .find(|(a, _)| a.rel == hit.rel && a.ch == hit.ch)
        {
            Some((_, got)) => got.push(hit),
            None => unexpected.push(hit),
        }
    }

    let mut out: Vec<String> = Vec::new();
    for hit in &unexpected {
        out.push(format!("{}  ← {advice}", hit.describe()));
    }
    for (a, got) in &counted {
        if got.len() != a.count {
            let where_ = got
                .iter()
                .map(|h| h.describe())
                .collect::<Vec<_>>()
                .join("\n    ");
            out.push(format!(
                "{} の {:?} が {} 件（許可リストは {} 件）。\
                 増えたなら出力へ混ざっていないか、減ったなら許可の理由がまだ要るかを見直す\n    {}",
                a.rel,
                a.ch,
                got.len(),
                a.count,
                where_
            ));
        }
    }
    out
}

/// 許可リストが「理由つき・件数つき」であることを検査する（両番犬の共通条件）
pub fn assert_allow_is_justified(allow: &[Allow]) {
    for a in allow {
        assert!(
            !a.why.trim().is_empty(),
            "{} の {:?} に理由が無い（理由なしの許可は認めない）",
            a.rel,
            a.ch
        );
        assert!(a.count > 0, "{} の {:?} の件数が 0", a.rel, a.ch);
    }
}
