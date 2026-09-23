//! **#1650 の番犬**: エディタが改行コードを壊す形へ戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前）
//!
//! ```text
//! HL3: cursor=4 text="abc\r!\ndef\r\n"      ← End が CR の後ろへ行き、そこで打った
//! E3:  cr=2 lf=3 saved="abc\r\nX\ndef\r\n"  ← Enter が常に `\n` を挿して混在改行
//! ```
//!
//! `line_end` が `\n` の位置を返していたので、CRLF ファイルの End は画面上の行末より
//! 右（CR の後ろ）へ止まった。`newline` は `self.insert("\n")` 固定だったので、
//! CRLF ファイルで 1 回 Enter を打つだけで改行が混ざった。正規化も保持もどこにも無かった。
//!
//! ## ここで止める 6 つ
//!
//! 1. [`enterはバッファの改行コードを挿す`] — `newline` が `\n` 直書きへ戻る（**Issue の本体**）
//! 2. [`挿入と置換は改行コードを揃える口を通る`] — 打鍵 / IME / 貼り付け / dispatch の
//!    どれか 1 経路だけが揃え忘れる形
//! 3. [`行末判定はcrを行内文字に数えない`] — End が CR の後ろへ戻る
//! 4. [`カーソルはcrとlfのあいだに入らない`] — 行末判定を直しても外から指されうる
//! 5. [`ファイルを開くとき改行コードを検出する`] — 検出が消えると全ファイルが既定の流儀になる
//! 6. [`新規ファイルの既定はプラットフォームを引数で受ける`] — `cfg!(windows)` で分けると
//!    macOS 上から Windows の腕を検査できず、**CI だけが落ちる**（この PR で実際に起きた）
//! 7. [`本番コードは改行リテラルを直接挿さない`] — 新しい挿入経路が `\n` を直書きする
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 往復保存の実挙動（CRLF / LF / 混在 / 末尾改行なし）は `tako_core::text_edit` の
//! 単体テスト。ここは**配線が外れていないこと**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments};

const REL: &str = "crates/tako-core/src/text_edit.rs";

/// 改行を差し込むメソッド（この行に改行リテラルが同居したら直書き）
const INSERTERS: [&str; 4] = ["insert_str(", ".insert(", "push_str(", ".push("];

/// 改行の綴り（エスケープのまま見る。`code_view` を通さない眺めで探す）
const NEWLINE_LITERALS: [&str; 3] = ["\"\\n\"", "'\\n'", "\"\\r\\n\""];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象の 2 つの眺め（**同じバイト長**なので範囲を互いに使い回せる）。
///
/// - `code`: コメントも文字列も潰した眺め。波括弧の対応を数えるのに使う
/// - `view`: コメントだけ潰した眺め。**リテラルの中身**を見るのに使う
struct Target {
    code: String,
    view: String,
}

fn target() -> Target {
    let path = repo_root().join(REL);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{REL} を読めない: {e}"));
    // テスト領域だけを潰す（バイト長と行番号は保たれる）
    let prod = production_range::production(&src, REL);
    let target = Target {
        code: code_view(&prod),
        view: without_comments(&prod),
    };
    assert_eq!(
        target.code.len(),
        target.view.len(),
        "{REL}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    target
}

/// 関数 1 本（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    name: String,
    line: usize,
    /// コメントだけ潰した眺めでの本体（リテラルの中身が残っている）
    text: String,
}

impl Target {
    /// 関数の本体を名前で引く。見つからなければ `None`（改名の検出は呼び出し側）
    fn body(&self, name: &str) -> Option<Body> {
        let mut offset = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if fn_head_name(line.trim_start()) != Some(name) {
                continue;
            }
            // 頭の行から最初の `{` を探し、対応する `}` まで数える
            // （眺めはコメントも文字列も潰してあるので、中の括弧を数えない）
            let open = self.code[start..].find('{')? + start;
            let mut depth = 0usize;
            let mut end = open;
            for (i, byte) in self.code[open..].bytes().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return Some(Body {
                name: name.to_string(),
                line: self.code[..start].lines().count() + 1,
                text: self.view[open..end].to_string(),
            });
        }
        None
    }

    /// 本体を引く（無ければ改名として落とす）
    fn require(&self, name: &str) -> Body {
        self.body(name).unwrap_or_else(|| {
            panic!(
                "{REL}:1 `fn {name}` が見つからない。\n\
                 改名したなら番犬の名前も直すこと（#1650 の配線が外れたまま緑になる）"
            )
        })
    }
}

impl Body {
    fn must_contain(&self, needle: &str, why: &str) {
        assert!(
            self.text.contains(needle),
            "{REL}:{} `fn {}` が `{needle}` を通っていない。\n{why}",
            self.line,
            self.name
        );
    }

    fn must_not_contain(&self, needle: &str, why: &str) {
        assert!(
            !self.text.contains(needle),
            "{REL}:{} `fn {}` に `{needle}` が在る。\n{why}",
            self.line,
            self.name
        );
    }
}

/// 1: Enter が挿す文字はファイルの改行コード（Issue の本体）
#[test]
fn enterはバッファの改行コードを挿す() {
    let body = target().require("newline");
    body.must_contain(
        "line_ending",
        "CRLF ファイルで Enter を 1 回打つだけで改行が混ざる（実測: CR 2 / LF 3）",
    );
    for literal in NEWLINE_LITERALS {
        body.must_not_contain(
            literal,
            "改行の綴りは `LineEnding::as_str` だけが持つ（#1650）",
        );
    }
}

/// 2: 本文が入ってくる口はすべて改行コードを揃える
#[test]
fn 挿入と置換は改行コードを揃える口を通る() {
    let target = target();
    for name in ["insert", "replace_range", "replace_all"] {
        target.require(name).must_contain(
            "normalize_line_endings",
            "打鍵・IME・貼り付け・dispatch（CLI / MCP）はすべてここを通るので、\n\
             1 経路でも揃え忘れると CRLF ファイルが混在改行になる（#1650）",
        );
    }
}

/// 3: 行末は CR の手前（画面上の行末）
#[test]
fn 行末判定はcrを行内文字に数えない() {
    let body = target().require("line_end_offset");
    body.must_contain(
        "'\\r'",
        "`\\n` の位置を返すと CRLF ファイルの End が CR の後ろへ止まり、\n\
         そこで打つと `abc\\r!\\n` になる（#1650 の実測 HL3）",
    );
}

/// 4: カーソルは CR と LF のあいだに入らない
#[test]
fn カーソルはcrとlfのあいだに入らない() {
    let body = target().require("snap_cursor");
    for needle in ["b'\\r'", "b'\\n'"] {
        body.must_contain(
            needle,
            "行末判定を直しても、マウスクリック・IME・dispatch は外から位置を指せる（#1650）",
        );
    }
    // 丸めを通さずに素の文字境界合わせへ戻ると、外からの指定が素通りする
    target().require("set_cursor").must_contain(
        "snap_cursor",
        "`set_cursor` が `snap_boundary` 直呼びへ戻ると CR の後ろを指せる（#1650）",
    );
}

/// 5: 開いたときに改行コードを読む
#[test]
fn ファイルを開くとき改行コードを検出する() {
    let target = target();
    for name in ["open", "from_text"] {
        target.require(name).must_contain(
            "LineEnding::detect",
            "検出が消えると、すべてのファイルがプラットフォーム既定の流儀で保存される（#1650）",
        );
    }
}

/// 5b: 新規ファイルの既定は `Platform` を引数で受ける純関数（macOS から Windows を測れる）
#[test]
fn 新規ファイルの既定はプラットフォームを引数で受ける() {
    let target = target();
    target.require("for_new_file").must_contain(
        "Platform::Windows",
        "改行を持たないファイルの既定は OS で変わる。`cfg!(windows)` で分けると\n\
         macOS 上から Windows の腕を検査できず、CI だけが落ちる（#1650 で実際に起きた）",
    );
    target.require("platform_default").must_contain(
        "for_new_file",
        "腕の対応表は `for_new_file` の 1 か所だけが持つ（#1650）",
    );
    // 判断が `cfg!` へ散ると引数で受ける意味が消える
    let mut offenders = Vec::new();
    for (i, line) in target.code.lines().enumerate() {
        if line.contains("cfg!(windows)") {
            offenders.push(format!("{REL}:{}", i + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "`cfg!(windows)` が在る:\n{}\n\n\
         プラットフォームの分岐は `Platform` を引数で受ける純関数にすること（#1650）",
        offenders.join("\n")
    );
}

/// 6: 新しい挿入経路が改行リテラルを直書きしない
#[test]
fn 本番コードは改行リテラルを直接挿さない() {
    let target = target();
    let mut offenders = Vec::new();
    for (i, line) in target.view.lines().enumerate() {
        // 潰した眺めのほうでメソッド呼び出しを見る（コメント中の例で誤検知しない）
        let code_line = target.code.lines().nth(i).unwrap_or("");
        if !INSERTERS.iter().any(|m| code_line.contains(m)) {
            continue;
        }
        if NEWLINE_LITERALS.iter().any(|l| line.contains(l)) {
            offenders.push(format!("{REL}:{} {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "改行リテラルを直接挿している:\n{}\n\n\
         挿す改行は `self.line_ending.as_str()`、入ってくる本文は\n\
         `normalize_line_endings` を通すこと（#1650）",
        offenders.join("\n")
    );
}

/// 空振り検査: 走査そのものが成立していること
#[test]
fn 番犬の走査が成立している() {
    let target = target();
    // 眺めが空になっていない（改名・移動で静かに緑になる形を落とす）
    assert!(
        target.code.contains("impl TextBuffer"),
        "{REL}:1 走査範囲に `impl TextBuffer` が無い"
    );
    // リテラルを残す眺めが本当に中身を持っている（`without_comments` の取り違え検出）
    let as_str = target.require("as_str");
    for literal in ["\"\\n\"", "\"\\r\\n\""] {
        assert!(
            as_str.text.contains(literal),
            "{REL}:{} リテラルを見る眺めが中身を落としている（`{literal}` が読めない）",
            as_str.line
        );
    }
    // 検査対象の関数が 1 本残らず引けること
    for name in [
        "newline",
        "insert",
        "replace_range",
        "replace_all",
        "line_end_offset",
        "snap_cursor",
        "set_cursor",
        "open",
        "from_text",
        "normalize_line_endings",
        "for_new_file",
        "platform_default",
    ] {
        assert!(
            target.body(name).is_some(),
            "{REL}:1 `fn {name}` を引けない（番犬が空振りする）"
        );
    }
}
