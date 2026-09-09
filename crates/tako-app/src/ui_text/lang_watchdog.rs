//! 番犬: `ui_text` のテストが言語依存の文字列を**ロックの外で読み比べていない**（#1274）
//!
//! ## なぜ止めるのか
//!
//! 表示言語は `tako_core::i18n` の**プロセス全体の AtomicU8**。`cargo test` は同一
//! バイナリのテストを並列スレッドで走らせるので、言語依存の関数を 2 回呼んで結果を
//! 比べるテストは、**2 回の読み取りのあいだに別のテストが言語を切り替える**と
//! 別言語同士を比較して落ちる。
//!
//! 実測（#1274）: `ui_text::path_menu::tests::共通項目はファイルツリーと同一文言` が
//! `cargo test --workspace` でだけ低頻度に落ちた（単体では必ず通る / 直後に 3 回連続で
//! 緑 = 「回して緑」では判定できない）。`mod.rs` のコメントが言っていた
//! 「lang 依存の他テストは相対比較で書くこと」は、**比較の 2 点が同じ言語である
//! 保証にはならない**というのがこの穴だった。
//!
//! ## 何を違反とするか
//!
//! `crates/tako-app/src/ui_text/` 配下の `#[test] fn` の本体から、
//! `tests_support` のヘルパ呼び出し（`check_ja_en(…)` / `for_each_lang(…)` /
//! `with_lang(…)`）の**括弧の中身をまるごと取り除いた残り**に、言語依存の関数呼び出しが
//! 残っていたら落とす。本体の先頭で `lang_guard()` を束縛しているテストは、
//! ガードが本体の終わりまで生きるので対象外。
//!
//! 「言語依存の関数」は決め打ちではなく**ソースから閉包で求める**:
//! `tr!(` を含む関数を起点に、それを呼ぶ関数を不動点まで辿る（テストモジュール内の
//! 補助関数も同じ扱い。`mac_texts()` のような集約関数がロック外で呼ばれたら落ちる）。
//!
//! 併せて、`#[test]` が `i18n::set_lang` を直に叩く形も落とす
//! （切替は必ずロックの下 = `lang_guard()` かヘルパ経由にする）。
//!
//! ## 拾えないもの
//!
//! 呼び出し `名前(` の形だけを見るので、言語依存の関数を**値として渡してから
//! 別名で呼ぶ**形（`let f: fn() = 比較; f();`）は素通りする。#1274 の再現テストは
//! 意図的にその形（ロック外の比較）を取るため、番犬の対象外である
//! `crates/tako-app/src/ui_text_lang_race.rs` へ置いてある。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 言語を固定する `tests_support` のヘルパ（括弧の中身は「固定された 1 区間」）
const SCOPED_HELPERS: &[&str] = &["check_ja_en", "for_each_lang", "with_lang"];

/// 本体の終わりまで言語を固定する RAII ガード
const GUARD_HELPER: &str = "lang_guard";

fn ui_text_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui_text")
}

/// `src/ui_text/*.rs` を (パス, 中身) で集める
fn sources() -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(ui_text_dir())
        .expect("src/ui_text を読めない")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .map(|p| {
            let body = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}"));
            (p, body)
        })
        .collect();
    out.sort();
    assert!(out.len() >= 20, "ui_text のソースを拾えていない: {out:?}");
    out
}

/// コメントと文字列・文字リテラルの中身を空白へ潰した写しを返す（**バイト長は不変**）。
///
/// 括弧の対応付けと呼び出しの検出をこの写しの上で行うので、`tr!("… {} …")` の
/// 波括弧やコメント中の `fn` に引きずられない。`&'static` のライフタイムと
/// `'3'` の文字リテラルは前者を通常のコードとして扱って区別する
fn mask_non_code(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = vec![b' '; b.len()];
    let mut i = 0usize;
    // コードとして残す範囲を [start, i) 単位で書き戻す
    let keep = |out: &mut Vec<u8>, from: usize, to: usize| {
        out[from..to].copy_from_slice(&b[from..to]);
    };
    while i < b.len() {
        match b[i] {
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                let mut depth = 1usize;
                i += 2;
                while i < b.len() && depth > 0 {
                    if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'r' if raw_string_hashes(b, i).is_some() => {
                let hashes = raw_string_hashes(b, i).expect("直前に確認済み");
                i += 1 + hashes + 1; // r + # * n + "
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                while i < b.len() && !b[i..].starts_with(&close) {
                    i += 1;
                }
                i = (i + close.len()).min(b.len());
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
            }
            b'\'' if is_char_literal(src, i) => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
            }
            _ => {
                let start = i;
                i += 1;
                keep(&mut out, start, i);
            }
        }
    }
    String::from_utf8(out).expect("マスクは文字境界でのみ置換する")
}

/// `r"…"` / `r#"…"#` の開始なら `#` の個数を返す
fn raw_string_hashes(b: &[u8], i: usize) -> Option<usize> {
    if b[i] != b'r' {
        return None;
    }
    // 直前が識別子文字なら `r` は識別子の一部（`for` の r ではない。`char` 等）
    if i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    (j < b.len() && b[j] == b'"').then_some(j - i - 1)
}

/// `'a'` / `'\n'` の文字リテラルか（`&'static` のライフタイムと区別する）。
/// **1 文字ちょうどを挟んで閉じる形だけ**を文字リテラルとみなす
/// （`<'a, 'b>` のような並びを誤って literal と読まないため）
fn is_char_literal(src: &str, i: usize) -> bool {
    let rest = &src[i + 1..];
    if rest.starts_with('\\') {
        return true;
    }
    match rest.chars().next() {
        Some(c) => rest[c.len_utf8()..].starts_with('\''),
        None => false,
    }
}

/// `open` から対応する `close` までの範囲（`open` の位置を渡す。戻り値は閉じ括弧の次）
fn matching(masked: &[u8], open_at: usize, open: u8, close: u8) -> usize {
    // 開始が開き括弧でない（= すでに潰した領域を再訪した）ときは何もしない
    if masked.get(open_at) != Some(&open) {
        return open_at;
    }
    let mut depth = 0usize;
    let mut i = open_at;
    while i < masked.len() {
        if masked[i] == open {
            depth += 1;
        } else if masked[i] == close {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    masked.len()
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// 位置 `at` から識別子を読む
fn ident_at(masked: &[u8], at: usize) -> String {
    let mut end = at;
    while end < masked.len() && is_ident_byte(masked[end]) {
        end += 1;
    }
    String::from_utf8_lossy(&masked[at..end]).into_owned()
}

/// 関数定義（名前, 本体の範囲）を集める。`impl Fn()` のような大文字は拾わない
fn fn_defs(masked: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let b = masked.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = masked[i..].find("fn ") {
        let at = i + rel;
        i = at + 3;
        // 語境界（`asfn ` のような部分一致を弾く）
        if at > 0 && is_ident_byte(b[at - 1]) {
            continue;
        }
        let mut name_at = at + 3;
        while name_at < b.len() && b[name_at] == b' ' {
            name_at += 1;
        }
        let name = ident_at(b, name_at);
        if name.is_empty() {
            continue;
        }
        // 名前のあとに来る最初の `{`（括弧・角括弧の外）が本体の開始
        let mut j = name_at + name.len();
        let (mut paren, mut brack) = (0i32, 0i32);
        let mut body_at = None;
        while j < b.len() {
            match b[j] {
                b'(' => paren += 1,
                b')' => paren -= 1,
                b'[' => brack += 1,
                b']' => brack -= 1,
                b';' if paren == 0 && brack == 0 => break, // トレイト宣言など本体なし
                b'{' if paren == 0 && brack == 0 => {
                    body_at = Some(j);
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        let Some(body_at) = body_at else { continue };
        let end = matching(b, body_at, b'{', b'}');
        out.push((name, body_at..end));
    }
    out
}

/// `名前(` の呼び出しがあるか（`super::` 等の前置きは無視して末尾の名前で見る）
fn calls(masked: &str, name: &str) -> bool {
    call_positions(masked, name).next().is_some()
}

fn call_positions<'a>(masked: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
    let b = masked.as_bytes();
    masked.match_indices(name).filter_map(move |(at, _)| {
        // 直前が `.` ならメソッド呼び出し（`bar.find(…)`）。カタログにも
        // `menu::find()` のような一般名があるので、ここを見ないと総なめになる
        let before_ok = at == 0 || (!is_ident_byte(b[at - 1]) && b[at - 1] != b'.');
        let after = at + name.len();
        // 呼び出しは `名前(`。空白を挟む形はコード整形で出ないので見ない
        let after_ok = b.get(after) == Some(&b'(');
        (before_ok && after_ok).then_some(at)
    })
}

/// 言語依存の関数名の集合を、`tr!(` を起点に不動点まで広げて求める
fn lang_dependent_fns(files: &[(PathBuf, String)]) -> BTreeSet<String> {
    let bodies: Vec<(String, String)> = files
        .iter()
        .flat_map(|(_, src)| {
            let masked = mask_non_code(src);
            fn_defs(&masked)
                .into_iter()
                .map(|(name, range)| (name, masked[range].to_string()))
                .collect::<Vec<_>>()
        })
        .collect();

    let mut set: BTreeSet<String> = bodies
        .iter()
        .filter(|(_, body)| body.contains("tr!("))
        .map(|(name, _)| name.clone())
        .collect();
    assert!(
        set.len() >= 50,
        "tr! を含む関数を拾えていない（走査が壊れている）: {}",
        set.len()
    );
    loop {
        let mut grew = false;
        for (name, body) in &bodies {
            if set.contains(name) {
                continue;
            }
            if set.iter().any(|dep| dep != name && calls(body, dep)) {
                set.insert(name.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    set
}

/// `#[test] fn` の（名前, 本体マスク済み文字列, 本体の開始オフセット）
fn test_fns(masked: &str) -> Vec<(String, String, usize)> {
    let defs = fn_defs(masked);
    let b = masked.as_bytes();
    masked
        .match_indices("#[test]")
        .filter_map(|(at, _)| {
            // 直後に続く最初の関数定義がそのテスト
            let def = defs.iter().find(|(_, r)| r.start > at)?;
            // 属性とのあいだに別の `{` が挟まっていないこと（別モジュールへ跨がない）
            let between = &b[at + "#[test]".len()..def.1.start];
            (!between.contains(&b'}')).then(|| {
                (
                    def.0.clone(),
                    masked[def.1.clone()].to_string(),
                    def.1.start,
                )
            })
        })
        .collect()
}

/// ヘルパ呼び出しの括弧の中身を空白へ潰す（= 言語が固定された区間を取り除く）
fn strip_scoped_helpers(body: &str) -> String {
    let mut out = body.as_bytes().to_vec();
    for helper in SCOPED_HELPERS {
        let positions: Vec<usize> = call_positions(body, helper).collect();
        for at in positions {
            let open = at + helper.len();
            // すでに外側のヘルパで潰されていれば `matching` は何もせず戻る
            let end = matching(&out, open, b'(', b')');
            for c in out[open..end].iter_mut() {
                if *c != b'\n' {
                    *c = b' ';
                }
            }
        }
    }
    String::from_utf8(out).expect("空白置換は文字境界を壊さない")
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].matches('\n').count() + 1
}

/// 違反 1 件（報告用）
struct Violation {
    file: String,
    line: usize,
    test: String,
    detail: String,
}

/// 走査対象 `files`（パス, 中身）の違反を集める。
/// 対象を引数で受けるのは、番犬自身の A/B で「修正前の形」を流し込むため
fn violations(files: &[(PathBuf, String)]) -> Vec<Violation> {
    let lang_fns = lang_dependent_fns(files);
    let mut out = Vec::new();
    for (path, src) in files {
        let masked = mask_non_code(src);
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (test, body, body_at) in test_fns(&masked) {
            let line = line_of(src, body_at);
            // 本体の終わりまで言語を固定するガードを取っているテストは対象外
            if calls(&body, GUARD_HELPER) {
                continue;
            }
            let rest = strip_scoped_helpers(&body);
            let bare: Vec<&String> = lang_fns.iter().filter(|f| calls(&rest, f)).collect();
            if !bare.is_empty() {
                let names: Vec<&str> = bare.iter().map(|s| s.as_str()).collect();
                out.push(Violation {
                    file: file.clone(),
                    line,
                    test: test.clone(),
                    detail: format!(
                        "言語依存の関数 {names:?} をロックの外で呼んでいる（\
                         `tests_support::for_each_lang(|| …)` などで囲むか、\
                         本体の先頭で `tests_support::lang_guard()` を束縛する）"
                    ),
                });
            }
            if calls(&rest, "set_lang") {
                out.push(Violation {
                    file: file.clone(),
                    line,
                    test: test.clone(),
                    detail: "`set_lang` をロックの外で呼んでいる（言語の切替は \
                             `tests_support` のヘルパ経由にする）"
                        .to_string(),
                });
            }
        }
    }
    out
}

fn render(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("  {}:{} {} — {}", v.file, v.line, v.test, v.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ui_textのテストは言語を固定した区間の中で相対比較している() {
    let found = violations(&sources());
    assert!(
        found.is_empty(),
        "言語グローバルの競合で確率的に落ちるテストがある（#1274）:\n{}",
        render(&found)
    );
}

/// 番犬が**実際に落とせる**ことを、#1274 の修正前の形そのもので固定する。
/// 走査が壊れて何も検出しなくなったら（= 常に緑になったら）ここで気づく
#[test]
fn 番犬は修正前の形を名指しで落とす() {
    let mut files = sources();
    let before = r#"
pub fn open_default() -> &'static str {
    tr!("開く", "Open")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn 共通項目はファイルツリーと同一文言() {
        assert_eq!(open_default(), super::super::sidebar::menu_open_default());
    }
}
"#;
    files.push((PathBuf::from("修正前.rs"), before.to_string()));
    let found = violations(&files);
    let hit = found
        .iter()
        .find(|v| v.test == "共通項目はファイルツリーと同一文言")
        .unwrap_or_else(|| panic!("修正前の形を落とせていない:\n{}", render(&found)));
    assert!(
        hit.detail.contains("open_default"),
        "名指しになっていない: {}",
        hit.detail
    );

    // ヘルパで囲めば落ちないこと（= 誤検知しない側も固定する）
    let after = before.replace(
        "        assert_eq!(open_default(), super::super::sidebar::menu_open_default());",
        "        tests_support::for_each_lang(|| assert_eq!(open_default(), \
         super::super::sidebar::menu_open_default()));",
    );
    let mut fixed = sources();
    fixed.push((PathBuf::from("修正後.rs"), after));
    let rest = violations(&fixed);
    assert!(
        !rest
            .iter()
            .any(|v| v.test == "共通項目はファイルツリーと同一文言"),
        "ヘルパで囲んだ形を誤検知している:\n{}",
        render(&rest)
    );
}

/// `set_lang` を直に叩く形も落ちること
#[test]
fn 番犬はロック外のset_langを落とす() {
    let mut files = sources();
    files.push((
        PathBuf::from("直接切替.rs"),
        r#"
#[cfg(test)]
mod tests {
    #[test]
    fn 言語を勝手に変える() {
        tako_core::i18n::set_lang(tako_core::i18n::Lang::Ja);
    }
}
"#
        .to_string(),
    ));
    let found = violations(&files);
    assert!(
        found
            .iter()
            .any(|v| v.test == "言語を勝手に変える" && v.detail.contains("set_lang")),
        "ロック外の set_lang を落とせていない:\n{}",
        render(&found)
    );
}

/// マスク処理がコメント・文字列・ライフタイム・raw 文字列で壊れていないこと。
/// ここが壊れると番犬が黙って空振りする（= いちばん危ない壊れ方）
#[test]
fn マスクはコメントと文字列だけを潰す() {
    let src = r####"
// fn コメントの中の定義( は拾わない
/* fn ブロックコメント( */
fn 本物(x: &'static str) -> &'static str {
    let _ = "文字列の中の fn 偽物( と } は無視";
    let _ = r#"raw の中の fn 偽物2( と }"#;
    let _ = '}';
    x
}
"####;
    let masked = mask_non_code(src);
    assert_eq!(masked.len(), src.len(), "マスクでバイト長が変わっている");
    let names: Vec<String> = fn_defs(&masked).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["本物".to_string()], "拾った関数名が違う");
    assert!(!masked.contains("偽物"), "文字列の中身が残っている");
    assert!(!masked.contains("コメントの中"), "コメントが残っている");
    assert!(masked.contains("&'static str"), "ライフタイムを潰している");
}

/// メソッド呼び出しをカタログ関数と混同しないこと。
/// `menu::find()` のような一般名のカタログ関数があるので、ここを外すと
/// `bar.find(…)` を書いた全テストが違反になる（実際に踏んだ）
#[test]
fn メソッド呼び出しは関数呼び出しと区別する() {
    assert!(
        calls("let x = find();", "find"),
        "素の呼び出しを拾えていない"
    );
    assert!(
        calls("menu::find();", "find"),
        "パス付きの呼び出しを拾えていない"
    );
    assert!(!calls("bar.find(pat);", "find"), "メソッド呼び出しを拾った");
    assert!(!calls("refind();", "find"), "識別子の一部を拾った");
    assert!(
        !calls("let find = 1;", "find"),
        "呼び出しでない参照を拾った"
    );
}

/// 実ソースで閉包が効いていること（`tr!` を持たないが言語依存な関数を拾えている）
#[test]
fn 言語依存の閉包が委譲と集約を辿れている() {
    let set = lang_dependent_fns(&sources());
    for name in [
        // `tr!` を直接持つ
        "open_in_tako_file",
        // `sidebar::menu_open_default` への委譲（tr! を持たない）
        "open_default",
        // 分岐して文言関数を返す（tr! を持たない）
        "chip_label",
        // 文言関数の結果を String にして返す（tr! を持たない）
        "install_method_display",
    ] {
        assert!(set.contains(name), "{name} を言語依存として拾えていない");
    }
    for name in ["mask_non_code", "sources", "matching"] {
        assert!(!set.contains(name), "{name} を言語依存と誤認している");
    }
}
