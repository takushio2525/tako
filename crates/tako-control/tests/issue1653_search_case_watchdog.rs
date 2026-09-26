//! **#1653 の番犬**: 検索・置換が「常に小文字化する」形へ戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前）
//!
//! ```text
//! find_all("foo") on "Foo foo FOO"                 → 3 件（区別できない）
//! replace_all("value"→"item") on "let value = Value::new();"
//!                                                  → "let item = item::new();"（型名まで潰す）
//! 検索欄の 1 打鍵（release・1 MB）                  → 中央値 4.449ms（全文の小文字写しを 2 回）
//! ```
//!
//! `find_all` が唯一の入口で、中で必ず全文の小文字写し（`Lowered::build`）を作っていた。
//! 条件を渡す口が無いので、コード編集で `value` と `Value` を分けられず、全置換が
//! データを壊した。しかも GUI の検索欄は 1 打鍵で `find_all` と `find_next` を呼び、
//! 写しを 2 回作り直していた。
//!
//! ## ここで止める 5 つ
//!
//! 1. [`区別する検索は小文字写しを通らない`] — 常に小文字化へ戻る（**Issue の本体**）
//! 2. [`小文字写しを作るのはfoldedだけ`] — 呼ぶたびに全文を写し直す形が戻る
//! 3. [`本文が変わったら写しを捨てる`] — 古い写しで探して位置がずれる
//! 4. [`guiの全文検索は1箇所`] — 1 打鍵で全文を何度も探す形が戻る
//! 5. [`条件の既定はdispatchの1実装で決まる`] — CLI / MCP / GUI で既定が食い違う
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 4 通りの条件の件数・置換結果・写しを作った回数は `tako_core::text_edit` の単体テスト
//! （`_1653` で終わるもの）、省略時の既定と引き継ぎは `dispatch::tests::issue1653_*`、
//! MCP の引数は `mcp::tests::issue1653_*` が見る。ここは**配線が外れていないこと**だけを見る。
//! 肯定の存在確認はコメントを落とした眺めで見る（#1609）

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const TEXT_EDIT: &str = "crates/tako-core/src/text_edit.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const APP_SRC: &str = "crates/tako-app/src";
/// GUI で全文検索（`find_all`）を呼んでよい唯一の場所
const APP_SEARCH_OWNER: (&str, &str) = ("crates/tako-app/src/preview.rs", "refresh_search_hits");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象の 2 つの眺め（**同じバイト長**なので範囲を互いに使い回せる）。
///
/// - `code`: コメントも文字列も潰した眺め。波括弧の対応と関数の頭を数えるのに使う
/// - `view`: コメントだけ潰した眺め。**識別子とリテラルの中身**を見るのに使う（#1609）
struct Target {
    rel: String,
    code: String,
    view: String,
}

fn target(rel: &str) -> Target {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    // テスト領域だけを潰す（バイト長と行番号は保たれる）
    let prod = production_range::production(&src, rel);
    let target = Target {
        rel: rel.to_string(),
        code: code_view(&prod),
        view: without_comments_checked(&prod, rel),
    };
    assert_eq!(
        target.code.len(),
        target.view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    target
}

/// 関数 1 本（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    rel: String,
    name: String,
    line: usize,
    /// コメントだけ潰した眺めでの本体
    text: String,
    start: usize,
    end: usize,
}

impl Target {
    /// 関数の本体を名前で引く。修飾子（`pub(crate) fn` / `async fn`）に依らない（#1496）
    fn body(&self, name: &str) -> Option<Body> {
        self.bodies().into_iter().find(|b| b.name == name)
    }

    /// すべての関数の本体（入れ子の関数は外側と内側の両方に数える）
    fn bodies(&self) -> Vec<Body> {
        let mut out = Vec::new();
        let mut offset = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            let Some(name) = fn_head_name(line.trim_start()) else {
                continue;
            };
            // 宣言だけ（trait の既定なしメソッド）は本体を持たない
            let Some(open) = self.code[start..].find(['{', ';']).map(|i| i + start) else {
                continue;
            };
            if self.code.as_bytes()[open] == b';' {
                continue;
            }
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
            out.push(Body {
                rel: self.rel.clone(),
                name: name.to_string(),
                line: self.line_of(start),
                text: self.view[open..end].to_string(),
                start: open,
                end,
            });
        }
        out
    }

    /// 本体を引く（無ければ改名として落とす）
    fn require(&self, name: &str) -> Body {
        self.body(name).unwrap_or_else(|| {
            panic!(
                "{}:1 `fn {name}` が見つからない。\n\
                 改名したなら番犬の名前も直すこと（#1653 の配線が外れたまま緑になる）",
                self.rel
            )
        })
    }

    /// バイト位置を 1 起点の行番号へ
    fn line_of(&self, pos: usize) -> usize {
        self.code[..pos].lines().count().max(1) + usize::from(self.code[..pos].ends_with('\n'))
    }

    /// `needle` の出現位置（コメントを落とした眺めで）
    fn occurrences(&self, needle: &str) -> Vec<usize> {
        self.view.match_indices(needle).map(|(i, _)| i).collect()
    }

    /// 位置 `pos` を含む最も内側の関数の名前
    fn enclosing_fn(&self, pos: usize) -> Option<String> {
        self.bodies()
            .into_iter()
            .filter(|b| b.start <= pos && pos < b.end)
            .min_by_key(|b| b.end - b.start)
            .map(|b| b.name)
    }
}

impl Body {
    fn must_contain(&self, needle: &str, why: &str) {
        assert!(
            self.text.contains(needle),
            "{}:{} `fn {}` が `{needle}` を通っていない。\n{why}",
            self.rel,
            self.line,
            self.name
        );
    }

    /// 本体の中での位置（無ければ本体の行で名指しして落とす）
    fn position(&self, needle: &str, why: &str) -> usize {
        self.text.find(needle).unwrap_or_else(|| {
            panic!(
                "{}:{} `fn {}` に `{needle}` が無い。\n{why}",
                self.rel, self.line, self.name
            )
        })
    }
}

/// 1: 区別する検索（既定）は小文字写しを作らずに本文そのものを探す（Issue の本体）。
///
/// 「区別する」の分岐が小文字写し（`folded` / `lowercase_per_char`）より**前で**
/// 返っていること。分岐を消す・写しの後ろへ回すと、また常に小文字化する形になる
#[test]
fn 区別する検索は小文字写しを通らない() {
    let text_edit = target(TEXT_EDIT);
    let find_all = text_edit.require("find_all");
    let why = "区別する検索は本文そのものを探し、小文字写しを通らずに返すこと（#1653）。\n\
               常に小文字化すると `replace_all(\"value\"→\"item\")` が `Value::new()` まで潰す";
    let branch = find_all.position("if options.case_sensitive", why);
    let early_return = find_all.position("return hits", why);
    let folded = find_all.position("self.folded()", why);
    let lowered_query = find_all.position("lowercase_per_char(", why);
    assert!(
        branch < early_return && early_return < folded && early_return < lowered_query,
        "{TEXT_EDIT}:{} `fn find_all` の「区別する」分岐が小文字写しより後ろにある。\n{why}",
        text_edit.line_of(find_all.start + branch.min(folded).min(lowered_query))
    );
    // 既定そのものが「区別する」であること（MCP / CLI / GUI がこれを使う）
    let default = text_edit
        .occurrences("pub const DEFAULT: Self = Self {")
        .first()
        .copied()
        .unwrap_or_else(|| {
            panic!("{TEXT_EDIT}:1 `SearchOptions::DEFAULT` が無い（既定の正本を消した）")
        });
    let decl = &text_edit.view[default..];
    let decl = &decl[..decl.find("};").unwrap_or(decl.len())];
    assert!(
        decl.contains("case_sensitive: true") && decl.contains("whole_word: false"),
        "{TEXT_EDIT}:{} `SearchOptions::DEFAULT` が「区別する・単語単位なし」でない。\n{why}",
        text_edit.line_of(default)
    );
}

/// 2: 全文の小文字写しを作るのは `folded`（本文が変わるまで使い回す口）だけ
#[test]
fn 小文字写しを作るのはfoldedだけ() {
    let text_edit = target(TEXT_EDIT);
    let stray: Vec<String> = text_edit
        .occurrences("Lowered::build(")
        .into_iter()
        .filter(|&pos| text_edit.enclosing_fn(pos).as_deref() != Some("folded"))
        .map(|pos| {
            format!(
                "{TEXT_EDIT}:{} `Lowered::build` を `fn {}` が直接呼んでいる",
                text_edit.line_of(pos),
                text_edit.enclosing_fn(pos).unwrap_or_else(|| "?".into())
            )
        })
        .collect();
    assert!(
        stray.is_empty(),
        "全文の小文字写しを作る場所が `folded` の外にある:\n{}\n\
         写しは `folded` で作り、本文が変わるまで使い回すこと（#1653。\
         呼ぶたびに作ると検索欄の 1 打鍵ごとに 1 MB を写し直す）",
        stray.join("\n")
    );
    let folded = text_edit.require("folded");
    folded.must_contain(
        "get_or_init(",
        "写しは 1 度作ったら本文が変わるまで使い回すこと（#1653）",
    );
    folded.must_contain(
        "builds.fetch_add(",
        "写しを作った回数を数えること（単体テストが「1 打鍵ごとに作らない」を回数で固定する）",
    );
}

/// 3: 本文が変わる唯一の経路（版を進める口）で写しを捨てる
#[test]
fn 本文が変わったら写しを捨てる() {
    let text_edit = target(TEXT_EDIT);
    text_edit.require("bump_version").must_contain(
        "self.fold.lowered.take()",
        "本文が変わったら区別しない検索の小文字写しを捨てること（#1653）。\n\
         捨てないと古い本文の写しで探し、ヒットの位置がずれる（範囲外で panic もしうる）",
    );
    text_edit.require("release_search_cache").must_contain(
        "self.fold.lowered.take()",
        "検索欄を閉じたら写しを手放すこと（本文と同じ大きさがある。#1653）",
    );
}

/// 4: GUI の全文検索は `EditState::refresh_search_hits` の 1 箇所だけ。
///
/// 次へ / 前へ・件数・ハイライトはそこで数えたヒットを使い回す（修正前は 1 打鍵で
/// `find_all` と `find_next` が全文を 2 回探していた）
#[test]
fn guiの全文検索は1箇所() {
    let root = repo_root();
    let mut files = Vec::new();
    let mut stack = vec![root.join(APP_SRC)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                // 候補は字面で先に絞る（呼んでいないファイルは範囲取りの下限に当たる小さいものもある）
                && std::fs::read_to_string(&path).is_ok_and(|src| src.contains("find_all("))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut owner_seen = false;
    let mut stray = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let t = target(&rel);
        for pos in t.occurrences(".find_all(") {
            let owner = t.enclosing_fn(pos);
            if (rel.as_str(), owner.as_deref()) == (APP_SEARCH_OWNER.0, Some(APP_SEARCH_OWNER.1)) {
                owner_seen = true;
                continue;
            }
            stray.push(format!(
                "{rel}:{} `fn {}` が全文検索（find_all）を直接呼んでいる",
                t.line_of(pos),
                owner.unwrap_or_else(|| "?".into())
            ));
        }
    }
    assert!(
        stray.is_empty(),
        "GUI の全文検索が `{}::{}` の外にある:\n{}\n\
         検索欄の打鍵・トグル・次へ / 前へ・置換後の数え直しは `refresh_search_hits` を通すこと（#1653）",
        APP_SEARCH_OWNER.0,
        APP_SEARCH_OWNER.1,
        stray.join("\n")
    );
    assert!(
        owner_seen,
        "{}:1 `fn {}` が find_all を呼んでいない（改名したなら番犬も直す）",
        APP_SEARCH_OWNER.0, APP_SEARCH_OWNER.1
    );
}

/// 5: 省略時の条件は dispatch の 1 実装で決まる（CLI / MCP / GUI が同じ既定を使う）
#[test]
fn 条件の既定はdispatchの1実装で決まる() {
    let dispatch = target(DISPATCH);
    let arm = |from: &str, to: &str| -> (usize, String) {
        let start = dispatch
            .occurrences(from)
            .first()
            .copied()
            .unwrap_or_else(|| panic!("{DISPATCH}:1 `{from}` のアームが無い"));
        let len = dispatch.view[start..].find(to).unwrap_or_else(|| {
            panic!("{DISPATCH}:{} `{to}` が後ろに無い", dispatch.line_of(start))
        });
        (
            dispatch.line_of(start),
            dispatch.view[start..start + len].to_string(),
        )
    };
    let (line, search) = arm("Request::PreviewSearch {", "Request::PreviewReplace {");
    for needle in ["SearchOptions::resolve(", "with_search_options("] {
        assert!(
            search.contains(needle),
            "{DISPATCH}:{line} `PreviewSearch` のアームが `{needle}` を通っていない。\n\
             条件は tako-core の `SearchOptions::resolve` で決め（query を渡せば既定から、\
             省略すれば今の条件から）、使った条件を応答へ載せること（#1653）"
        );
    }
    let (line, replace) = arm("Request::PreviewReplace {", "Request::PreviewChangelog {");
    for needle in ["SearchOptions::DEFAULT.with(", "with_search_options("] {
        assert!(
            replace.contains(needle),
            "{DISPATCH}:{line} `PreviewReplace` のアームが `{needle}` を通っていない。\n\
             置換の省略は常に既定（区別する）で、画面の検索欄の条件を引き継がないこと（#1653）"
        );
    }
}
