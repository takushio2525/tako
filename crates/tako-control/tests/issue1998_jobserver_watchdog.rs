//! #1998 の番犬: cargo の共有 jobserver（札）を、全ペインの cargo を止めない形のまま配る
//!
//! # なぜ要るか
//!
//! 共有 jobserver の env はすべてのペインの cargo に効く。配り方を誤ると、利用者が `tako queue` を
//! 通さずに打った cargo まで巻き込んで壊す:
//!
//! - `MAKEFLAGS` で配ると、`-j` 無しの素の `make` まで並列で走り出し、並列に耐えない Makefile を壊す
//!   （設計書 §1.6 の実測）。cargo だけが読む `CARGO_MAKEFLAGS` で配る
//! - UI スレッド（`spawn_session`）が fifo を数えたり走査したりすると、#1968 / #1979 の「UI スレッドが
//!   重い I/O を待つ」型を持ち込む。UI は覚えを写すだけ
//! - 札を数え直す側が走査（約 20 ms）の間に番人のロックを持つと、ペインを起こす UI スレッドが待つ
//! - 終了で fifo を片付けないと、GUI の居ない間に器のペインで始めた cargo が札 0 枚の fifo を開いて
//!   直列になる
//!
//! # 何を縛るか
//!
//! 1. `spawn_session` は `jobserver::pane_env()` の覚えを写して env へ足す。`"MAKEFLAGS"` を直に足さない
//! 2. GUI の起動で番人を立て（`jobserver::start(`）、`cleanup_process_scoped`（成功と失敗の出口の
//!    1 実装）で `jobserver::shutdown()` を呼ぶ
//! 3. 番人の UI 向けの口（トップレベルの `pub fn pane_env`）は fifo を触らず走査しない
//! 4. 背景スレッドの 1 周（`fn tick_global`）は札を持ちうるプロセスを数える呼び出しをロックの塊の外に置く
//! 5. 配る env の名前は `CARGO_MAKEFLAGS`
//!
//! [`逆戻りを名指しできる`] で、修正前・誤りの形を注入した本文が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::without_comments;

const MAIN: &str = "crates/tako-app/src/main.rs";
const KEEPER: &str = "crates/tako-control/src/jobserver.rs";

/// UI 向けの口が呼んではいけないもの（右は理由）
const UI_FORBIDDEN: &[(&str, &str)] = &[
    ("jobserver_holders(", "走査を起こしうる（約 20 ms）"),
    ("refresh_processes(", "全プロセスの走査"),
    (".available(", "fifo を数える（FIONREAD）"),
    ("open_or_create(", "fifo を作る・開く"),
    (".reconcile(", "fifo を作る・開く・消す"),
    (".step(", "数え直し"),
    ("Command::new(", "子プロセスを起こす"),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read_raw(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} が読める: {e}", path.display()))
}

/// テスト領域とコメントを空白へ潰した本文（行番号は保たれる）
fn production(src: &str) -> String {
    without_comments(&production_range::scan(src).text)
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// 関数の窓（宣言行の 1-based 行番号と窓のバイト範囲）。`top_level` ならインデント無しの定義だけを見る。
/// 終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, name: &str, top_level: bool) -> Option<(usize, std::ops::Range<usize>)> {
    let mut offset = 0;
    let mut start: Option<(usize, usize, usize)> = None;
    for (i, line) in src.split_inclusive('\n').enumerate() {
        match start {
            None if tako_core::source_scan::fn_head_name(line) == Some(name)
                && (!top_level || !line.starts_with(char::is_whitespace)) =>
            {
                let indent = line.len() - line.trim_start().len();
                start = Some((i + 1, offset, indent));
            }
            Some((at, begin, indent)) => {
                let body = line.trim_end_matches(['\n', '\r']);
                if body.len() == indent + 1 && body.trim() == "}" {
                    return Some((at, begin..offset + line.len()));
                }
            }
            None => {}
        }
        offset += line.len();
    }
    None
}

fn missing(rel: &str, what: &str) -> String {
    format!("{rel} — {what} が見つからない（走査が空振り。名前を変えたらこの番犬も直す）")
}

/// 1 と 2（main.rs）
fn scan_main(rel: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    match fn_window(src, "spawn_session", false) {
        None => out.push(missing(rel, "`fn spawn_session`")),
        Some((head, range)) => {
            let window = &src[range.clone()];
            if !window.contains("jobserver::pane_env()") {
                out.push(format!(
                    "{rel}:{head} — `spawn_session` が `jobserver::pane_env()` を写していない \
                     （tako のペインの cargo に札が配られない）"
                ));
            }
            let mut from = 0;
            while let Some(found) = window[from..].find("\"MAKEFLAGS\"") {
                let at = range.start + from + found;
                out.push(format!(
                    "{rel}:{} — ペインの env へ `MAKEFLAGS` を足している（素の make まで並列になる。\
                     `CARGO_MAKEFLAGS` = `jobserver::pane_env()` だけで配る）",
                    line_of(src, at)
                ));
                from += found + 1;
            }
        }
    }
    if !src.contains("tako_control::jobserver::start(") {
        out.push(format!(
            "{rel} — GUI の起動で札の番人を立てていない（`tako_control::jobserver::start(`）"
        ));
    }
    match fn_window(src, "cleanup_process_scoped", true) {
        None => out.push(missing(rel, "`fn cleanup_process_scoped`")),
        Some((head, range)) => {
            if !src[range].contains("jobserver::shutdown()") {
                out.push(format!(
                    "{rel}:{head} — 終了の後片付けが `jobserver::shutdown()` を呼んでいない \
                     （GUI の居ない間の cargo が札 0 枚の fifo を開いて直列になる）"
                ));
            }
        }
    }
    out
}

/// 3 と 4（番人）
fn scan_keeper(rel: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    match fn_window(src, "pane_env", true) {
        None => out.push(missing(rel, "トップレベルの `pub fn pane_env`")),
        Some((_, range)) => {
            let window = &src[range.clone()];
            for (needle, why) in UI_FORBIDDEN {
                let mut from = 0;
                while let Some(found) = window[from..].find(needle) {
                    let at = range.start + from + found;
                    out.push(format!(
                        "{rel}:{} — UI スレッドから呼ぶ `pane_env` が `{needle}` を呼んでいる（{why}。\
                         覚えを写すだけ = #1968 / #1979）",
                        line_of(src, at)
                    ));
                    from += found + needle.len();
                }
            }
        }
    }
    match fn_window(src, "tick_global", true) {
        None => out.push(missing(rel, "`fn tick_global`")),
        Some((head, range)) => {
            let window = &src[range.clone()];
            let block = window.find("let need = {");
            let block_end = block.and_then(|b| window[b..].find("\n    };").map(|e| b + e));
            let count = window.find("jobserver_holders(");
            match (block_end, count) {
                (None, _) => out.push(format!(
                    "{rel}:{head} — `tick_global` のロックの塊（`let need = {{ … }};`）が見つからない"
                )),
                (_, None) => out.push(format!(
                    "{rel}:{head} — `tick_global` が札を持ちうるプロセスを数えていない（`jobserver_holders(`）"
                )),
                (Some(end), Some(c)) if c < end => out.push(format!(
                    "{rel}:{} — `jobserver_holders(` を番人のロックの中で呼んでいる（走査の間 UI スレッドの \
                     `pane_env` が待つ）",
                    line_of(src, range.start + c)
                )),
                _ => {}
            }
        }
    }
    out
}

/// 1〜4: 本番の本文に逆戻りが無い
#[test]
fn 札はcargoだけが読むenvで覚えから配りロックの外で数える() {
    let mut hits = scan_main(MAIN, &production(&read_raw(MAIN)));
    hits.extend(scan_keeper(KEEPER, &production(&read_raw(KEEPER))));
    assert!(
        hits.is_empty(),
        "共有 jobserver の配り方が #1998 の約束から外れている（全ペインの cargo に効くので、素の make を並列に \
         しない・UI スレッドで数えない・終了で片付ける）:\n  {}",
        hits.join("\n  ")
    );
}

/// 5: 配る env の名前
#[test]
fn 配るenvはcargo_makeflags() {
    assert_eq!(tako_core::jobserver::ENV, "CARGO_MAKEFLAGS");
    let v = tako_core::jobserver::makeflags(4, Path::new("/x/resources/jobserver.fifo"));
    assert!(!v.contains("MAKEFLAGS"), "{v}");
    assert!(tako_core::jobserver::refers_to_tako(&v));
}

/// 番犬そのものの検査: 誤りの形を注入した本文が file:line で名指しされる
#[test]
fn 逆戻りを名指しできる() {
    // spawn_session: 注入をやめる / MAKEFLAGS で配る
    let raw = read_raw(MAIN);
    let src = production(&raw);
    let fixed = "if let Some((key, value)) = tako_control::jobserver::pane_env() {";
    let at = src
        .find(fixed)
        .expect("注入の目印が本文に無い（番犬の注入を追従させること）");
    let head = {
        let (head, _) = fn_window(&src, "spawn_session", false).unwrap();
        format!("{MAIN}:{head} —")
    };
    let hits = scan_main(
        MAIN,
        &src.replacen(
            fixed,
            "if let Some((key, value)) = None::<(&str, String)> {",
            1,
        ),
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&head) && h.contains("写していない")),
        "注入をやめても {head} で名指しされない: {hits:?}"
    );
    let want = format!("{MAIN}:{} —", line_of(&src, at));
    let hits = scan_main(
        MAIN,
        &src.replacen(
            fixed,
            "if let Some((key, value)) = tako_control::jobserver::pane_env() { options.env.push((\"MAKEFLAGS\".into(), value.clone()));",
            1,
        ),
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("`MAKEFLAGS`")),
        "MAKEFLAGS で配っても {want} で名指しされない: {hits:?}"
    );
    assert!(
        raw.lines()
            .nth(line_of(&src, at) - 1)
            .is_some_and(|l| l.contains(fixed)),
        "名指しした行が原文の注入の行と違う"
    );
    // 終了の後片付けから外す
    let hits = scan_main(
        MAIN,
        &src.replacen(
            "tako_control::jobserver::shutdown()",
            "tako_control::jobserver::status_section()",
            1,
        ),
    );
    assert!(
        hits.iter()
            .any(|h| h.contains("`jobserver::shutdown()` を呼んでいない")),
        "後片付けから外しても名指しされない: {hits:?}"
    );

    // 番人: UI 向けの口で数える / ロックの中で数える
    let raw = read_raw(KEEPER);
    let src = production(&raw);
    let fixed = "lock().as_ref().and_then(|g| g.keeper.pane_env())";
    let at = src
        .find(fixed)
        .expect("注入の目印が本文に無い（番犬の注入を追従させること）");
    let want = format!("{KEEPER}:{} —", line_of(&src, at));
    let hits = scan_keeper(
        KEEPER,
        &src.replacen(
            fixed,
            "{ let _ = crate::resources::jobserver_holders(std::path::Path::new(\"x\"), false); lock().as_ref().and_then(|g| g.keeper.pane_env()) }",
            1,
        ),
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("`jobserver_holders(`")),
        "UI 向けの口で数えても {want} で名指しされない: {hits:?}"
    );
    let fixed =
        "        reload_settings_if_changed(global);\n        let step = global.keeper.step();";
    let at = src
        .find(fixed)
        .expect("注入の目印が本文に無い（番犬の注入を追従させること）");
    let want = format!("{KEEPER}:{} —", line_of(&src, at));
    let hits = scan_keeper(
        KEEPER,
        &src.replacen(
            fixed,
            "        let _ = crate::resources::jobserver_holders(std::path::Path::new(\"x\"), false);\n        let step = global.keeper.step();",
            1,
        ),
    );
    assert!(
        hits.iter()
            .any(|h| h.starts_with(&want) && h.contains("ロックの中")),
        "ロックの中で数えても {want} で名指しされない: {hits:?}"
    );
}
