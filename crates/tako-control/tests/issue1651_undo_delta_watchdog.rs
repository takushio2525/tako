//! **#1651 の番犬**: undo 履歴が「全文のスナップショット」へ戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前）
//!
//! ```text
//! mode=natural text=1048576 bytes  RSS(before)=6.1 MB
//! mode=natural 打鍵=1000  RSS(after)=1028.1 MB  増分=1022.0 MB  undo 回数=1000
//! ```
//!
//! `push_undo` が編集のたびに `self.text.clone()` を積んでいたので、1 MB のファイルへ
//! 1000 打鍵すると履歴だけで 1 GB 積もった（上限が「操作数 1000」だけで、
//! **バイト数の予算が無かった**）。しかも粒度が 1 文字なので `hello` を戻すのに 5 回。
//!
//! ## ここで止める 5 つ
//!
//! 1. [`履歴は全文の写しを持たない`] — スナップショットへ戻る（**Issue の本体**）
//! 2. [`本文を書き換える口は1本`] — 履歴に載らない書き換え経路が増える
//! 3. [`連続した編集をまとめる口を通る`] — まとめが外れて 1 文字粒度へ戻る
//! 4. [`塊を閉じる口が配線されている`] — カーソル移動・保存で塊が切れなくなる
//! 5. [`上限はバイト数でも効く`] — 予算が操作数だけへ戻る
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! まとめの粒度・差分の往復（ランダム編集列を undo で全部戻すとバイト一致）・
//! 予算の実効は `tako_core::text_edit` の単体テスト。ここは**配線が外れていないこと**
//! だけを見る。**肯定の存在確認はコメントを落とした眺めで**見る（#1609）

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const REL: &str = "crates/tako-core/src/text_edit.rs";

/// 本文を直接書き換えるメソッド（#1651）。
///
/// これを呼んでよいのは `apply_edit`（差分を積む口）と `undo` / `redo`（差分を戻す口）だけ
const MUTATORS: [&str; 5] = [
    "self.text.replace_range(",
    "self.text.insert_str(",
    "self.text.drain(",
    "self.text.push_str(",
    "self.text = ",
];

/// 本文を直接書き換えてよい関数
const MUTATOR_OWNERS: [&str; 3] = ["apply_edit", "undo", "redo"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象の 2 つの眺め（**同じバイト長**なので範囲を互いに使い回せる）。
///
/// - `code`: コメントも文字列も潰した眺め。波括弧の対応を数えるのに使う
/// - `view`: コメントだけ潰した眺め。**識別子とリテラルの中身**を見るのに使う（#1609）
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
        view: without_comments_checked(&prod, REL),
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
    /// コメントだけ潰した眺めでの本体
    text: String,
    /// 本体が始まるバイト位置（眺めは 2 つで共通）
    start: usize,
    end: usize,
}

impl Target {
    /// 関数の本体を名前で引く。修飾子（`pub(crate) fn` / `async fn`）に依らない（#1496）
    fn body(&self, name: &str) -> Option<Body> {
        let mut offset = 0;
        for line in self.code.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            if fn_head_name(line.trim_start()) != Some(name) {
                continue;
            }
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
                start: open,
                end,
            });
        }
        None
    }

    /// 本体を引く（無ければ改名として落とす）
    fn require(&self, name: &str) -> Body {
        self.body(name).unwrap_or_else(|| {
            panic!(
                "{REL}:1 `fn {name}` が見つからない。\n\
                 改名したなら番犬の名前も直すこと（#1651 の配線が外れたまま緑になる）"
            )
        })
    }

    /// バイト位置を 1 起点の行番号へ
    fn line_of(&self, pos: usize) -> usize {
        self.code[..pos].lines().count().max(1)
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

/// 1: 履歴の 1 件は差分（範囲 + 置換前後）で、全文の写しを持たない（Issue の本体）
#[test]
fn 履歴は全文の写しを持たない() {
    let target = target();
    // 差分の型が「置換前 / 置換後」を持っている
    let delta = target
        .view
        .find("struct EditDelta")
        .map(|pos| {
            let rest = &target.view[pos..];
            let end = rest.find("\n}").map(|i| i + 2).unwrap_or(rest.len());
            (target.line_of(pos), rest[..end].to_string())
        })
        .unwrap_or_else(|| {
            panic!(
                "{REL}:1 `struct EditDelta` が無い。\n\
                 undo 履歴の 1 件は差分（範囲 + 置換前後の文字列）で持つこと（#1651）"
            )
        });
    for field in ["start:", "before:", "after:"] {
        assert!(
            delta.1.contains(field),
            "{REL}:{} `struct EditDelta` に `{field}` が無い。\n\
             差分は「どこを・何から・何へ」の 3 つで表す（#1651）",
            delta.0
        );
    }
    assert!(
        !delta.1.contains("text:"),
        "{REL}:{} `struct EditDelta` が `text:` を持っている。\n\
         全文のスナップショットへ戻っている（1 MB のファイルで 1 打鍵 1 MB = #1651 の症状）",
        delta.0
    );

    // 本文を丸ごと写す綴りが本番コードのどこにも無い
    let mut offenders = Vec::new();
    for (i, line) in target.view.lines().enumerate() {
        for needle in ["self.text.clone()", "self.text.to_string()"] {
            if line.contains(needle) {
                offenders.push(format!("{REL}:{} {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "本文を丸ごと写している:\n{}\n\n\
         履歴へ積むのは置き換える範囲だけ（`self.text[range]`）にすること（#1651）",
        offenders.join("\n")
    );

    // 戻す側も差分を当てる（`self.text = snap.text` へ戻っていない）
    for name in ["undo", "redo"] {
        let body = target.require(name);
        body.must_contain(
            "replace_range",
            "戻すのは差分の当て直し。全文の代入へ戻ると履歴が全文を持つことになる（#1651）",
        );
        for field in ["before", "after"] {
            body.must_contain(
                field,
                "差分の `before` / `after` を使って往復すること（#1651）",
            );
        }
    }
}

/// 2: 本文を書き換える口は 1 本（履歴に載らない編集経路を作らない）
#[test]
fn 本文を書き換える口は1本() {
    let target = target();
    // 書き換えてよい関数の範囲を先に集める
    let allowed: Vec<(usize, usize)> = MUTATOR_OWNERS
        .iter()
        .map(|name| {
            let body = target.require(name);
            (body.start, body.end)
        })
        .collect();
    let mut offenders = Vec::new();
    let mut offset = 0;
    for line in target.code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        // メソッド呼び出しの検出は潰した眺めで（コメント中の例で誤検知しない）
        if !MUTATORS.iter().any(|m| line.contains(m)) {
            continue;
        }
        if allowed.iter().any(|(s, e)| start >= *s && start < *e) {
            continue;
        }
        offenders.push(format!(
            "{REL}:{} {}",
            target.line_of(start) + 1,
            line.trim()
        ));
    }
    assert!(
        offenders.is_empty(),
        "履歴を通さずに本文を書き換えている:\n{}\n\n\
         本文を触るのは `apply_edit`（差分を積む）と `undo` / `redo`（差分を戻す）だけ。\n\
         ここを外れた書き換えは undo で戻らない（#1651）",
        offenders.join("\n")
    );
    // 積む口が本当に差分を作っていること
    let apply = target.require("apply_edit");
    apply.must_contain(
        "EditDelta",
        "`apply_edit` が差分を組み立てていない（#1651）",
    );
    apply.must_contain(
        "self.record(",
        "組み立てた差分を履歴へ積んでいない（#1651）",
    );
}

/// 3: 連続した編集をまとめる（1 文字粒度へ戻らない）
#[test]
fn 連続した編集をまとめる口を通る() {
    let target = target();
    target.require("record").must_contain(
        "merge_into_last",
        "まとめが外れると `hello` を戻すのに 5 回 undo することになる（#1651）",
    );
    let merge = target.require("merge_into_last");
    // まとまる条件の 4 つが実際に見られている
    for (needle, why) in [
        ("group_open", "カーソル移動・保存・undo を挟んだら足さない"),
        ("kind", "種別が変われば別の塊（挿入と削除が混ざらない）"),
        ("COALESCE_WINDOW_MS", "前回から時間が空けば別の塊"),
        ("contains_newline", "改行をまたぐ塊は作らない"),
    ] {
        merge.must_contain(needle, why);
    }
    // 時計は注入できる 1 実装を通る（実時間の assert をテストへ持ち込まない）
    target.require("now_millis").must_contain(
        "manual_millis",
        "まとめの時間判定は注入できる時計で測ること\n\
         （実時間で比べるテストは負荷で反転する = `.agent/conventions.md`）",
    );
    // `Instant` の巻き戻しへ戻らない（#1627）
    target.require("process_millis").must_not_contain(
        "- Duration",
        "`Instant::now() - Duration` はブートより前へ巻き戻すと panic する（#1627）",
    );
}

/// 4: 塊を閉じる口が配線されている（切れる条件が消えない）
#[test]
fn 塊を閉じる口が配線されている() {
    let target = target();
    for (name, why) in [
        ("set_cursor", "カーソルを動かしたら打鍵の連なりは切れる"),
        ("select_all", "全選択も位置の移動なので塊を切る"),
        ("save", "保存した姿まで戻れる位置を履歴に残す"),
        ("undo", "undo した直後の編集を前の塊へ足すと戻しすぎる"),
        ("redo", "redo した直後も同じ"),
    ] {
        target.require(name).must_contain("seal_undo_group", why);
    }
}

/// 5: 上限はバイト数でも効く（画像には予算があるのにテキストだけ無予算だった）
#[test]
fn 上限はバイト数でも効く() {
    let target = target();
    target.require("record").must_contain(
        "enforce_budget",
        "予算の適用が外れると履歴が無制限に積もる（#1651 の実測は 1 GB）",
    );
    let budget = target.require("enforce_budget");
    budget.must_contain("UNDO_LIMIT", "操作数の上限（#195）");
    budget.must_contain(
        "UNDO_BYTE_LIMIT",
        "バイト数の上限（#1651）。1 操作で本文 2 本ぶんを積む編集があるので、\n\
         操作数だけでは上限にならない",
    );
    target.require("undo_history_bytes").must_contain(
        "redo_stack",
        "予算は undo 側と redo 側の合計で数える（undo すると差分は redo 側へ移るだけ）",
    );
}

/// 空振り検査: 走査そのものが成立していること
#[test]
fn 番犬の走査が成立している() {
    let target = target();
    assert!(
        target.code.contains("impl TextBuffer"),
        "{REL}:1 走査範囲に `impl TextBuffer` が無い"
    );
    // 検査対象の関数が 1 本残らず引けること
    for name in [
        "apply_edit",
        "record",
        "merge_into_last",
        "enforce_budget",
        "undo_history_bytes",
        "undo_depth",
        "seal_undo_group",
        "now_millis",
        "process_millis",
        "undo",
        "redo",
        "set_cursor",
        "select_all",
        "save",
    ] {
        assert!(
            target.body(name).is_some(),
            "{REL}:1 `fn {name}` を引けない（番犬が空振りする）"
        );
    }
    // 本文を書き換える綴りが実在すること（`MUTATORS` の綴りが腐ると 0 件で緑になる）。
    // 差分へ寄せた今、実際に使う書き換えは `replace_range` だけ（積む 1 + 戻す 2 = 3 箇所）で、
    // 残りの綴りは**戻ってきたら落とすため**に並べてある
    let writes = target.code.matches(MUTATORS[0]).count();
    assert!(
        writes >= 3,
        "{REL}:1 `{}` が {writes} 箇所しか無い（`apply_edit` / `undo` / `redo` の 3 箇所が要る）。\n\
         書き換えの綴りを変えたなら `MUTATORS` も直すこと（番犬が空振りする）",
        MUTATORS[0]
    );
    assert!(
        MUTATORS.iter().all(|m| m.starts_with("self.text")),
        "`MUTATORS` は `self.text` への書き換えを並べる（走査の前提）"
    );
    // 予算の定数が実在すること
    for needle in [
        "const UNDO_LIMIT",
        "const UNDO_BYTE_LIMIT",
        "const COALESCE_WINDOW_MS",
    ] {
        assert!(
            target.view.contains(needle),
            "{REL}:1 `{needle}` が無い（上限・まとめの幅は定数で宣言する）"
        );
    }
}
