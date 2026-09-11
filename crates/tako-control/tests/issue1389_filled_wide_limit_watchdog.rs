//! 番犬: `visible_lines_filled` の「行末が全角は取りこぼす」限界が書かれたままである（#1389）
//!
//! ## なぜ止めるのか
//!
//! `TerminalSession::visible_lines_filled`（#651）は「行が右端まで埋まっているか」を
//! alacritty の `Row::line_length()` で**列**で測る。doc は「文字数で代用すると全角が
//! 1 つあるだけで取りこぼす」と正しく書いているので、**全角も列なら見られる**と読める。
//! しかし `line_length()` は末尾から `cell.c != ' '` のセルを探す実装で、全角の後続セル
//! （`WIDE_CHAR_SPACER`）は `c == ' '` なので**空きと数える**。
//!
//! 実測（10 桁の実 PTY・2026-09-12）:
//!
//! ```text
//! A 右端が全角・以降の出力なし  abcdefghあ  filled=false line_length=9  WIDE_CHAR_SPACER
//! B 右端が全角・続きあり        abcdefghあ  filled=true  line_length=10 WRAPLINE | WIDE_CHAR_SPACER
//! E CUP で描き直し・右端が全角  abcdefghあ  filled=false line_length=9  WIDE_CHAR_SPACER
//! ```
//!
//! 現行の消費者（`dispatch::find_exit_marker`）は全 ASCII のマーカーの断片を持つ行しか
//! 見ないので**ユーザーに見える壊れ方は無い**。止めたいのは**次にこの API を使う人が
//! 右端の全角を検査しない**ことで、兄弟実装 `links::combined_screen_text` の同じ穴だけが
//! `.agent/conventions.md` の #1283 節に記録されている状態（= 2 つの物差しが同じ穴を
//! 持つことが読み取れない）へ戻らないよう、3 つを揃って要求する。
//!
//! ## 3 本立て（どれか 1 つでは穴が残る）
//!
//! 1. [`docに行末全角の限界が書かれている`] — doc から限界が消えていないこと
//! 2. [`conventions_mdの1283節に同じ穴が並べてある`] — 片方だけの記録に戻っていないこと
//! 3. [`限界を固定するテストが実ptyで機構を見ている`] — 記述が実物と食い違わないこと
//!    （doc / md は文章なので、機械検査が無いと実装が変わったときに黙って嘘になる）
//!
//! 判定を直した（= A / E が真になる）ときは 3 つ揃って直す。この番犬は
//! 「限界が在ること」ではなく「**限界の扱いが 1 か所に偏っていないこと**」を守る

use std::path::{Path, PathBuf};

/// 限界を書く場所（doc）
const TERMINAL_RS: &str = "crates/tako-core/src/terminal.rs";
/// 限界を並べる場所（規約）
const CONVENTIONS_MD: &str = ".agent/conventions.md";
/// doc を持つ関数の signature
const SIGNATURE: &str = "    pub fn visible_lines_filled(&self) -> Vec<(String, bool)> {";
/// 限界を固定する単体テスト
const LIMIT_TEST: &str = "    fn visible_lines_filled_は行末の全角を取りこぼす() {";
/// 兄弟実装の同じ穴を並べてある節（`## ` 見出しの一部一致で探す）
const SECTION_1283: &str = "画面テキストの切り出しは「区切りを増やす」より「削って実在で決める」";

/// doc に必ず在る needle（機構 / 条件 / 結果 / 出所）
const DOC_NEEDLES: [&str; 4] = ["WIDE_CHAR_SPACER", "行末が全角", "filled=false", "#1389"];
/// 規約の #1283 節に必ず在る needle（2 つの物差しが同じ穴を持つことが読み取れる形）
const MD_NEEDLES: [&str; 3] = ["visible_lines_filled", "WIDE_CHAR_SPACER", "#1389"];
/// 限界を固定するテストに必ず在る needle（実 PTY で機構まで見ている形）
const TEST_NEEDLES: [&str; 4] = [
    "TerminalSession::spawn(",
    "row.line_length()",
    "Flags::WIDE_CHAR_SPACER",
    "CUP で描き直し",
];

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

/// 切り出した塊（`file:line` で名指しできるよう開始行を持つ）
struct Block {
    /// 1-indexed の開始行
    line: usize,
    text: String,
}

/// `signature` の**直前に連なる doc コメント**（`///` の塊）を切り出す。
///
/// 塊の切れ目で止めるので、手前の別の関数の doc は混ざらない
fn doc_block(src: &str, signature: &str) -> Option<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines.iter().position(|l| *l == signature)?;
    let mut start = at;
    while start > 0 && lines[start - 1].trim_start().starts_with("///") {
        start -= 1;
    }
    (start < at).then(|| Block {
        line: start + 1,
        text: lines[start..at].join("\n"),
    })
}

/// `## ` 見出しから次の `## ` までを 1 節として切り出す
fn md_section(src: &str, header_part: &str) -> Option<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("## ") && l.contains(header_part))?;
    let end = lines[at + 1..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map(|i| at + 1 + i)
        .unwrap_or(lines.len());
    Some(Block {
        line: at + 1,
        text: lines[at..end].join("\n"),
    })
}

/// インデント 4 の `fn` 宣言行から、インデント 4 の閉じ括弧までを本体として切り出す
/// （テストモジュールの中の関数は必ずこの形。`rustfmt` が保証する）
fn fn_block(src: &str, signature: &str) -> Option<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines.iter().position(|l| *l == signature)?;
    let mut end = at + 1;
    while end < lines.len() && lines[end] != "    }" {
        end += 1;
    }
    Some(Block {
        line: at + 1,
        text: lines[at..=end.min(lines.len() - 1)].join("\n"),
    })
}

/// 塊から欠けている needle を `file:line` 付きで並べる（空なら合格）
fn missing(rel: &str, block: &Block, needles: &[&str], what: &str) -> Vec<String> {
    needles
        .iter()
        .filter(|n| !block.text.contains(**n))
        .map(|n| format!("{rel}:{} {what}に `{n}` が無い", block.line))
        .collect()
}

#[test]
fn docに行末全角の限界が書かれている() {
    let src = read(TERMINAL_RS);
    let block = doc_block(&src, SIGNATURE).unwrap_or_else(|| {
        panic!(
            "{TERMINAL_RS}: visible_lines_filled の doc が切り出せていない（改名したら番犬も直す）"
        )
    });
    let gaps = missing(TERMINAL_RS, &block, &DOC_NEEDLES, "doc の「既知の限界」");
    assert!(
        gaps.is_empty(),
        "`line_length()` は全角の後続セル（WIDE_CHAR_SPACER）を空きと数えるので、\n\
         行末が全角の行は右端まで埋まっていても filled=false になる（#1389 の実測 A / E）。\n\
         doc がこれを書いていないと「全角も列なら見られる」と読めてしまう。\n\
         判定を直したなら doc / {CONVENTIONS_MD} / 単体テストを同じコミットで直すこと:\n  {}",
        gaps.join("\n  ")
    );
}

#[test]
fn conventions_mdの1283節に同じ穴が並べてある() {
    let src = read(CONVENTIONS_MD);
    let block = md_section(&src, SECTION_1283).unwrap_or_else(|| {
        panic!("{CONVENTIONS_MD}: #1283 の節が見つからない（改題したら番犬も直す）")
    });
    let gaps = missing(CONVENTIONS_MD, &block, &MD_NEEDLES, "#1283 節");
    assert!(
        gaps.is_empty(),
        "`links::combined_screen_text` と `TerminalSession::visible_lines_filled` は\n\
         **同じ穴（右端の全角を「埋まっていない」と読む）** を持つ。片方だけ記録すると、\n\
         折り返しの結合を新しく書く人がもう片方で踏む（#1389）:\n  {}",
        gaps.join("\n  ")
    );
}

#[test]
fn 限界を固定するテストが実ptyで機構を見ている() {
    let src = read(TERMINAL_RS);
    let block = fn_block(&src, LIMIT_TEST).unwrap_or_else(|| {
        panic!("{TERMINAL_RS}: 限界を固定するテスト（{LIMIT_TEST:?}）が無い = doc が実物と食い違っても誰も落ちない")
    });
    let gaps = missing(TERMINAL_RS, &block, &TEST_NEEDLES, "限界の固定テスト");
    assert!(
        gaps.is_empty(),
        "限界の固定は**実 PTY で機構まで**見る（`line_length()` の値と右端セルの\n\
         `WIDE_CHAR_SPACER`）。画面テキストだけを見る形へ痩せると、alacritty の\n\
         仕様が変わったときに doc が黙って嘘になる:\n  {}",
        gaps.join("\n  ")
    );
}

/// 記述を消したときに `file:line` で名指しできること（検出力の確認）
#[test]
fn i1389_注入_記述を消すと名指しで落とせる() {
    // doc: 限界の段落だけを削った形
    let stripped = format!(
        "    /// 表示行と「その行が右端まで埋まっているか」を返す（#651）。\n{SIGNATURE}\n"
    );
    let block = doc_block(&stripped, SIGNATURE).expect("doc を切り出せる");
    let gaps = missing(TERMINAL_RS, &block, &DOC_NEEDLES, "doc の「既知の限界」");
    assert_eq!(
        gaps.len(),
        DOC_NEEDLES.len(),
        "注入を拾えていない: {gaps:?}"
    );
    assert!(
        gaps[0].starts_with(&format!("{TERMINAL_RS}:1 ")),
        "行番号を名指しできていない: {gaps:?}"
    );

    // 規約: 節は在るが `visible_lines_filled` に触れていない形
    let md = format!(
        "## {SECTION_1283}（Issue #1283）\n- 既知の限界: 右端 1 列前の全角文字\n\n## 次の節\n"
    );
    let block = md_section(&md, SECTION_1283).expect("節を切り出せる");
    let gaps = missing(CONVENTIONS_MD, &block, &MD_NEEDLES, "#1283 節");
    assert_eq!(gaps.len(), MD_NEEDLES.len(), "注入を拾えていない: {gaps:?}");
    assert!(
        gaps.iter()
            .all(|g| g.starts_with(&format!("{CONVENTIONS_MD}:1 "))),
        "行番号を名指しできていない: {gaps:?}"
    );

    // テスト: 画面テキストだけを見る形へ痩せた（機構を見ていない）
    let thin = format!("{LIMIT_TEST}\n        assert_eq!(got[0].1, false);\n    }}\n");
    let block = fn_block(&thin, LIMIT_TEST).expect("本体を切り出せる");
    let gaps = missing(TERMINAL_RS, &block, &TEST_NEEDLES, "限界の固定テスト");
    assert_eq!(
        gaps.len(),
        TEST_NEEDLES.len(),
        "注入を拾えていない: {gaps:?}"
    );

    // 合格の形は 1 件も挙がらない（誤検知の確認）
    let ok = format!(
        "    /// 行末が全角のときは filled=false（WIDE_CHAR_SPACER を空きと数える。#1389）\n{SIGNATURE}\n"
    );
    let block = doc_block(&ok, SIGNATURE).expect("doc を切り出せる");
    assert!(
        missing(TERMINAL_RS, &block, &DOC_NEEDLES, "doc").is_empty(),
        "合格の形を落としている"
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象を見つけている() {
    let terminal = read(TERMINAL_RS);
    let doc = doc_block(&terminal, SIGNATURE).expect("doc を切り出せる");
    assert!(
        doc.text.lines().count() >= 10,
        "doc が {} 行しか切り出せていない = 切り出しが壊れている",
        doc.text.lines().count()
    );
    let test = fn_block(&terminal, LIMIT_TEST).expect("限界の固定テストを切り出せる");
    assert!(
        test.text.contains("printf 'abcdefghあ'"),
        "限界の固定テストが #1389 の実測 A の形（行末が全角）を張っていない:\n{}",
        test.text
    );
    let section = md_section(&read(CONVENTIONS_MD), SECTION_1283).expect("#1283 節を切り出せる");
    assert!(
        section.text.contains("combined_screen_text"),
        "#1283 節に兄弟実装の記録が無い = 節を取り違えている"
    );
}
