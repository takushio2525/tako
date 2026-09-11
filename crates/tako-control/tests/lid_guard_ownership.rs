//! #1373 の番犬: 蓋閉じ継続の記録（`lid-guard.json`）が
//! 「**所有者つき・原子書き込み・fail-loud**」から外れていないことをソースで固定する。
//!
//! 振る舞いそのものは `platform::lid` の単体テストが見ている（macOS でも走る純粋判定）。
//! ここが見るのは**構造**で、次の 3 つが崩れたら file:line で名指しする:
//!
//! 1. 記録は `config_io::atomic_write` で書く（`fs::write` の truncate → write の窓を作らない）
//! 2. 記録を読めなかったら「記録なし」へ丸めず退避 + エラー（丸めると倒した 0 を
//!    元値として書き戻し、ユーザーの蓋設定が永久に失われる = #1373 の症状 2）
//! 3. 元値へ戻す（`restore`）のは、所有権を確かめてロックの下に居る経路だけ
//!    （他インスタンスの上書きを奪って戻すのが #1373 の症状 1 = macOS の #449 と同型）
//!
//! Windows 実機が無くても崩れを検出できるよう、**ソースの構造**で見る。

use std::path::{Path, PathBuf};

const SRC: &str = "crates/tako-control/src/platform/lid.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

fn source() -> String {
    let path = repo_root().join(SRC);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 製品コードの範囲（`#[cfg(test)] mod tests` より前）。テストの中身は検査の対象外
fn product(text: &str) -> &str {
    match text.find("#[cfg(test)]\nmod tests {") {
        Some(at) => &text[..at],
        None => text,
    }
}

/// `needle` を含む行を `SRC:行番号: 本文` の形で並べる（file:line で名指しするため）
fn hits(text: &str, needle: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(i, line)| format!("{SRC}:{}: {}", i + 1, line.trim()))
        .collect()
}

/// `fn <name>` の本体を（宣言行の行番号, 本文）で返す
fn body_of(text: &str, name: &str) -> (usize, String) {
    // 総称のある宣言（`fn with_record<T>(`）も拾う
    let at = [format!("fn {name}("), format!("fn {name}<")]
        .iter()
        .filter_map(|head| text.find(head.as_str()))
        .min()
        .unwrap_or_else(|| panic!("{SRC} に fn {name} が無い（改名したら番犬も直すこと）"));
    let line = text[..at].matches('\n').count() + 1;
    let open = at + text[at..].find('{').expect("本体の開き括弧");
    let mut depth = 0usize;
    let mut end = open;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    (line, text[open..=end].to_string())
}

/// `fn <name>` の本体が `needle` を通ること
fn assert_goes_through(text: &str, name: &str, needle: &str, why: &str) {
    let (line, body) = body_of(text, name);
    assert!(
        body.contains(needle),
        "{SRC}:{line}: fn {name} が `{needle}` を通っていない。{why}"
    );
}

/// 記録の書き込みは原子書き込み（tmp + fsync + rename）を通る。
///
/// 素の `fs::write` は truncate → write の 2 段階で、並行プロセスに空 / 部分ファイルが
/// 見える窓ができる（#169 の一段目）。`lid-guard.json` は `data_dir` に 1 つで
/// **複数の tako-app が共有する**ので、この窓は実際に踏まれる
#[test]
fn 記録の書き込みは原子書き込みを通る() {
    let text = source();
    let raw = hits(product(&text), "fs::write(");
    assert!(
        raw.is_empty(),
        "記録を素の fs::write で書いている（config_io::atomic_write を通すこと）:\n{}",
        raw.join("\n")
    );
    assert_goes_through(
        product(&text),
        "store_saved_at",
        "config_io::atomic_write(",
        "並行プロセスの読み手に空 / 部分ファイルを見せないため。",
    );
}

/// 読めない記録を「記録なし」へ丸めない（fail-loud）。
///
/// 丸めると #169 と同じ三段連鎖で、倒れたままの現在値（0）を元値として記録し直し、
/// ユーザーの「蓋を閉じたらスリープ」設定が永久に失われる
#[test]
fn 読めない記録は記録なしへ丸めない() {
    let text = source();
    let (line, body) = body_of(product(&text), "read_from");
    for rounding in [".ok()?", "unwrap_or_default()", "ok().flatten()"] {
        assert!(
            !body.contains(rounding),
            "{SRC}:{line}: fn read_from が `{rounding}` で読めない記録を丸めている\
             （`Err` を返して倒し直しを止めること。#1373 / conventions.md \
             「やってはいけない unwrap_or_default()」）"
        );
    }
    assert!(
        body.contains("quarantine_unreadable("),
        "{SRC}:{line}: fn read_from が解釈できない内容を退避していない\
         （`<name>.unreadable.bak` へ写す = #916 の作法）"
    );
}

/// 書く経路はプロセス間ロックの下でディスクを読み直す。
///
/// 写しは他プロセスの書き込みを知らないので、ロック無しの read-modify-write は
/// #169 と同じ形で互いの記録を潰し合う
#[test]
fn 書く経路はロックの下で読み直す() {
    let text = source();
    let text = product(&text);
    assert_goes_through(
        text,
        "with_record",
        "config_io::lock_exclusive(",
        "read-modify-write をプロセス間で直列化するため。",
    );
    assert_goes_through(
        text,
        "with_record",
        "read_from(",
        "ロックを取ったあとはディスクが正（写しは他プロセスの書き込みを知らない）。",
    );
    for entry in ["set_stay_awake", "clear_residual"] {
        assert_goes_through(
            text,
            entry,
            "with_record(",
            "記録を書き換えるならロックの下に居ること。",
        );
    }
}

/// 元値へ戻すのは、所有権を確かめた経路だけ。
///
/// #1373 の症状 1 は「`set_stay_awake(false, …)` が記録の出所を見ずに `restore()` した」こと。
/// busy な A が倒した上書きを idle な B が戻し、A は自分の写しを信じて倒し直さないので
/// **画面は「有効」のまま実機は眠る**（macOS 側で #449 として直した事故と同型）
#[test]
fn 解除は所有権を確かめてから行う() {
    let text = source();
    let text = product(&text);

    // 毎 tick の入口が実行するのは `decide` が返した操作だけ。
    // ここが記録の出所を見ずに `restore` を呼んでいたのが #1373 の症状 1
    let (line, body) = body_of(text, "set_stay_awake");
    for direct in ["restore(&", "restore(cur", "acquire(wanted"] {
        assert!(
            !body.contains(direct),
            "{SRC}:{line}: fn set_stay_awake が所有権の判定を通さずに `{direct}` を呼んでいる\
             （`decide` を通すこと。#1373）"
        );
    }
    assert_goes_through(
        text,
        "set_stay_awake",
        "decide(",
        "「誰の記録か」の判定を 1 か所（decide）に留めるため。",
    );
    assert_goes_through(
        text,
        "clear_residual",
        "may_touch(",
        "起動時の残留復元も、生きた所有者の記録は奪わない。",
    );

    // `decide` は「他人の記録」を必ず何もしない側へ倒す
    let (line, body) = body_of(text, "decide");
    assert!(
        body.contains("may_touch(claim)"),
        "{SRC}:{line}: fn decide が所有権（may_touch）を見ていない"
    );
}

/// 倒したときは**誰が倒したか**を記録へ書く。
/// 所有者が無ければ次の tick で別インスタンスが「自分の記録」と取り違える
#[test]
fn 倒すときは所有者を記録する() {
    let text = source();
    assert_goes_through(
        product(&text),
        "acquire",
        "owner: Some(",
        "所有者の無い記録は他インスタンスに戻されてしまう。",
    );
}

/// 所有者の判定は**注入できる `probe`** を通す。
///
/// OS 依存（pid の生死・起動時刻）を引数へ追い出しておかないと、Windows 実機の無い
/// CI で 4 通りの分岐を 1 つも固定できない
#[test]
fn 所有者の判定は注入できる形で書く() {
    let text = source();
    let (line, body) = body_of(product(&text), "claim_for");
    assert!(
        body.contains("probe(owner.pid)"),
        "{SRC}:{line}: fn claim_for が注入された probe を使っていない\
         （OS 依存を直に呼ぶと macOS の CI で分岐を固定できない）"
    );
    let text = product(&text);
    let direct = hits(text, "procinfo::start_time_unix(");
    assert!(
        direct.len() <= 2,
        "起動時刻を引く場所が増えている（`probe_owner` と `RecordOwner::current` の\
         2 か所に留めること）:\n{}",
        direct.join("\n")
    );
}
