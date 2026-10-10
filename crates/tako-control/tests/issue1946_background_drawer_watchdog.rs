//! たまり場（退避ペインの一覧）の見え方と後始末の番犬（Issue #1946）
//!
//! # 何が壊れていたか
//!
//! 1. ドロワーが開くたびに退避ペインを**カード寸法へ resize** していた（40 列 × 8 行程度）。
//!    claude は狭くなった端末で下端だけを描き直すので、カードには入力欄と帯しか残らない
//!    （「中が全体として表示されていない」の真因）。描画も行 div の切り取りだった
//! 2. 並びは由来タブだけで、どの master の子か分からなかった
//! 3. 器（tmux セッション）の無い退避エントリ（幽霊）を見分ける手段が無く、閉じると
//!    レジストリへ「閉じた」と書いていた（本物が別 pane id で生きていることがある）
//! 4. 退避ペインの出力のたびにアプリ全体を 60fps で描き直していた（実測 43 回/秒・CPU 100%）
//!
//! # 検査は 5 本立て
//!
//! 1. [`たまり場は退避ペインをresizeしない`] — 新しい `render_drawer` に `.resize(` が無い
//!    （A/B の腕 `render_drawer_legacy1946` は対象外）
//! 2. [`たまり場はcoreのまとまりを通る`] — 見出しは `background_groups::groups(` から組む
//! 3. [`閉じる経路は閉じ方の判断を通る`] — GUI の `kill_shelved_pane` と dispatch の
//!    `BackgroundKill` が `close_plan(` を通し、器の kill とレジストリの記録をその答えで守る
//! 4. [`器の判定は1実装`] — GUI は `Vessel::classify(` を直に呼ばず dispatch の
//!    `background_vessel(` を通す
//! 5. [`サムネイルだけのペインは間引く`] — 出力の受け口が `PaneVisibility::Thumbnail` を
//!    間引いた再描画（`schedule_shelf_thumb_redraw(`）へ回す
//!
//! 各検査は**修正を外した形**を注入して file:line で名指しできることを
//! [`修正を外すと名指しできる`] で固定する。走査の区間が採れなければ落とす。

use std::path::{Path, PathBuf};

const DRAWER: &str = "crates/tako-app/src/drawer.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

#[path = "common/code_view.rs"]
mod code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// コメントを落とした本文（説明文に同じ綴りを書いても真にならない。#1609）。行番号は保たれる
fn read_code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    code_view::without_comments_checked(&text, rel)
}

/// `head` で始まる関数の本文（1 始まりの行番号つき）。次の同じ字下げの関数の頭
/// （判定は共有ヘルパ `fn_head_name` = #1496）か `#[cfg` の手前まで。
/// 採れなければ落とす（空振りで緑にしない）
fn fn_body<'a>(file: &str, source: &'a str, head: &str) -> Vec<(usize, &'a str)> {
    let numbered: Vec<(usize, &str)> = source
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .collect();
    let start = numbered
        .iter()
        .position(|(_, l)| l.contains(head))
        .unwrap_or_else(|| panic!("{file} に `{head}` が無い"));
    let indent = numbered[start].1.len() - numbered[start].1.trim_start().len();
    let end = numbered[start + 1..]
        .iter()
        .position(|(_, l)| {
            let t = l.trim_start();
            l.len() - t.len() == indent
                && (tako_core::source_scan::fn_head_name(l).is_some() || t.starts_with("#[cfg"))
        })
        .map(|i| i + start + 1)
        .unwrap_or(numbered.len());
    numbered[start..end].to_vec()
}

/// `Request::BackgroundKill` の腕（次の `Request::` の腕の手前まで）
fn kill_arm(source: &str) -> Vec<(usize, &str)> {
    let numbered: Vec<(usize, &str)> = source
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .collect();
    let start = numbered
        .iter()
        .position(|(_, l)| {
            l.trim_start()
                .starts_with("Request::BackgroundKill { pane } =>")
        })
        .unwrap_or_else(|| panic!("{DISPATCH} に BackgroundKill の腕が無い"));
    let end = numbered[start + 1..]
        .iter()
        .position(|(_, l)| l.trim_start().starts_with("Request::"))
        .map(|i| i + start + 1)
        .expect("BackgroundKill の次の腕");
    numbered[start..end].to_vec()
}

fn find<'a>(body: &[(usize, &'a str)], needle: &str) -> Option<(usize, &'a str)> {
    body.iter().find(|(_, l)| l.contains(needle)).copied()
}

fn at(file: &str, (line_no, line): (usize, &str)) -> String {
    format!("{file}:{line_no}: {}", line.trim())
}

fn scan_no_resize(drawer: &str) -> Vec<String> {
    let body = fn_body(DRAWER, drawer, "pub(crate) fn render_drawer(");
    body.iter()
        .filter(|(_, l)| l.contains(".resize("))
        .map(|&hit| {
            format!(
                "{} — たまり場が退避ペインを resize している（#1946。カード寸法へ縮めると \
                 claude は下端だけを描き直し、入力欄と帯しか残らない）",
                at(DRAWER, hit)
            )
        })
        .collect()
}

fn scan_core_groups(drawer: &str) -> Vec<String> {
    let body = fn_body(DRAWER, drawer, "pub(crate) fn render_drawer(");
    if find(&body, "background_groups::groups(").is_some() {
        return Vec::new();
    }
    vec![format!(
        "{} — たまり場の見出しが `background_groups::groups` を通っていない（#1946。\
         CLI / MCP と同じまとまりを画面が見なくなる）",
        at(DRAWER, body[0])
    )]
}

fn scan_close_plan(main: &str, dispatch: &str) -> Vec<String> {
    let mut out = Vec::new();
    let gui = fn_body(MAIN, main, "fn kill_shelved_pane(");
    if find(&gui, "close_plan(").is_none() {
        out.push(format!(
            "{} — GUI の退避 kill が閉じ方の判断（`close_plan`）を通っていない（#1946）",
            at(MAIN, gui[0])
        ));
    }
    for (call, guard) in [
        ("drop_backend_session(", "plan.kill_vessel"),
        ("mark_worker_closed(", "plan.record_worker_close"),
    ] {
        match find(&gui, call) {
            None => out.push(format!("{MAIN}: kill_shelved_pane に `{call}` が無い")),
            Some(hit) => {
                // 呼び出しの直前 3 行のどこかに守りの条件がある
                let before: Vec<&(usize, &str)> = gui.iter().filter(|(n, _)| *n < hit.0).collect();
                let guarded = before.iter().rev().take(3).any(|(_, l)| l.contains(guard));
                if !guarded {
                    out.push(format!(
                        "{} — `{call}` が `{guard}` で守られていない（#1946。器の無い幽霊を\
                         閉じたときに器を kill する / レジストリへ閉じたと書く）",
                        at(MAIN, hit)
                    ));
                }
            }
        }
    }
    let arm = kill_arm(dispatch);
    if find(&arm, "close_plan(").is_none() || find(&arm, "plan.record_worker_close").is_none() {
        out.push(format!(
            "{} — dispatch の BackgroundKill が閉じ方の判断を通っていない（#1946。\
             UI と AI で幽霊の閉じ方が割れる）",
            at(DISPATCH, arm[0])
        ));
    }
    out
}

fn scan_one_vessel(main: &str, drawer: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (file, src) in [(MAIN, main), (DRAWER, drawer)] {
        for (i, line) in src.lines().enumerate() {
            if line.contains("Vessel::classify(") {
                out.push(format!(
                    "{} — GUI が器を独自に判定している（#1946。dispatch の \
                     `background_vessel` を通す = 画面と CLI / MCP の判定を 1 つにする）",
                    at(file, (i + 1, line))
                ));
            }
        }
    }
    let gui = fn_body(MAIN, main, "fn kill_shelved_pane(");
    if find(&gui, "background_vessel(").is_none() {
        out.push(format!(
            "{} — GUI の退避 kill が `background_vessel` を通っていない（#1946）",
            at(MAIN, gui[0])
        ));
    }
    out
}

fn scan_thumbnail_throttle(main: &str) -> Vec<String> {
    let body = fn_body(MAIN, main, "fn on_term_event(");
    let Some(hit) = find(&body, "visibility == PaneVisibility::Thumbnail") else {
        return vec![format!(
            "{} — 出力の受け口がサムネイルだけのペインを区別していない（#1946。\
             退避 10 本以上の出力で 60fps の全体再描画に戻る）",
            at(MAIN, body[0])
        )];
    };
    let throttled = body
        .iter()
        .skip_while(|(n, _)| *n <= hit.0)
        .take(4)
        .any(|(_, l)| l.contains("schedule_shelf_thumb_redraw("));
    if throttled {
        Vec::new()
    } else {
        vec![format!(
            "{} — サムネイルだけのペインの出力を間引いた再描画へ回していない（#1946）",
            at(MAIN, hit)
        )]
    }
}

struct Sources {
    drawer: String,
    main: String,
    dispatch: String,
}

impl Sources {
    fn load() -> Self {
        Self {
            drawer: read_code(DRAWER),
            main: read_code(MAIN),
            dispatch: read_code(DISPATCH),
        }
    }

    fn scan(&self) -> Vec<String> {
        let mut out = scan_no_resize(&self.drawer);
        out.extend(scan_core_groups(&self.drawer));
        out.extend(scan_close_plan(&self.main, &self.dispatch));
        out.extend(scan_one_vessel(&self.main, &self.drawer));
        out.extend(scan_thumbnail_throttle(&self.main));
        out
    }
}

#[test]
fn たまり場は退避ペインをresizeしない() {
    let s = Sources::load();
    let hits = scan_no_resize(&s.drawer);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn たまり場はcoreのまとまりを通る() {
    let s = Sources::load();
    let hits = scan_core_groups(&s.drawer);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 閉じる経路は閉じ方の判断を通る() {
    let s = Sources::load();
    let hits = scan_close_plan(&s.main, &s.dispatch);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 器の判定は1実装() {
    let s = Sources::load();
    let hits = scan_one_vessel(&s.main, &s.drawer);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn サムネイルだけのペインは間引く() {
    let s = Sources::load();
    let hits = scan_thumbnail_throttle(&s.main);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

/// 修正を 1 つずつ外した形を注入して、それぞれが file:line で名指しされることを固定する
#[test]
fn 修正を外すと名指しできる() {
    let base = Sources::load();
    assert!(base.scan().is_empty(), "前提: 今の形は緑");

    let inject = |edit: &dyn Fn(&mut Sources), expect: &str| {
        let mut s = Sources {
            drawer: base.drawer.clone(),
            main: base.main.clone(),
            dispatch: base.dispatch.clone(),
        };
        edit(&mut s);
        let hits = s.scan();
        assert!(
            hits.iter().any(|h| h.contains(expect)),
            "注入（{expect}）を名指しできない: {hits:?}"
        );
    };
    let replace_once = |src: &mut String, from: &str, to: &str| {
        assert!(src.contains(from), "注入の目印が無い: {from}");
        *src = src.replacen(from, to, 1);
    };

    // 1. resize を戻す
    inject(
        &|s| {
            replace_once(
                &mut s.drawer,
                "let body_h = self.shelf_body_height();",
                "let body_h = self.shelf_body_height(); session.resize(cols, rows, cw, ch);",
            )
        },
        "resize している",
    );
    // 2. まとまりを core から取らない
    inject(
        &|s| {
            replace_once(
                &mut s.drawer,
                "tako_core::background_groups::groups(&self.workspace)",
                "Vec::<BackgroundGroup>::new()",
            )
        },
        "`background_groups::groups` を通っていない",
    );
    // 3. 器の kill の守りを外す
    inject(
        &|s| {
            replace_once(&mut s.main, "if plan.kill_vessel {", "{");
        },
        "`drop_backend_session(` が `plan.kill_vessel` で守られていない",
    );
    // 3'. dispatch のレジストリ記録の守りを外す
    inject(
        &|s| {
            replace_once(&mut s.dispatch, "if plan.record_worker_close {", "{");
        },
        "dispatch の BackgroundKill が閉じ方の判断を通っていない",
    );
    // 4. GUI が器を独自に判定する
    inject(
        &|s| {
            replace_once(
                &mut s.main,
                "let vessel = tako_control::dispatch::background_vessel(self, pane_id);",
                "let vessel = tako_core::background_groups::Vessel::classify(Default::default());",
            )
        },
        "GUI が器を独自に判定している",
    );
    // 5. サムネイルの間引きを外す
    inject(
        &|s| {
            replace_once(
                &mut s.main,
                "if !ui_outside_pane && visibility == PaneVisibility::Thumbnail {",
                "if false {",
            )
        },
        "サムネイルだけのペインを区別していない",
    );
}
