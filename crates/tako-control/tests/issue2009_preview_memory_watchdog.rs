//! 見えていないプレビューの手放しの番犬（Issue #2009）
//!
//! # 何が壊れていたか
//!
//! 本番の GUI が 820 MB を抱えていた内訳の大半は、**裏のタブで開きっぱなしのプレビュー**だった
//! （隔離 GUI の実測: 描画メモリ IOAccelerator 199 MB のほぼ全部がプレビュー画像の GPU テクスチャ・
//! 60 ページの PDF 1 本で 201 MB）。デコード済みの画像はプロセス全体の LRU（既定 512 MB）が
//! 見えているかどうかに関係なく抱え、PDF は全ページの PNG を、動画は止めた後もフレームと
//! プレイヤーを持ち続けていた。
//!
//! # 検査は 7 本立て
//!
//! 1. [`定期更新から掃除を回す`] — 2 秒の定期更新が `sweep_preview_memory(` を呼ぶ
//! 2. [`見えているかは描画の経路と同じ判定`] — 掃除は `pane_visibility(` で決める
//!    （描いた時刻で決めると、キャッシュビューで描き直さない間の表示中のプレビューを手放す）
//! 3. [`手放しはGPUIからの解放経路を通す`] — `remove_preview_image_cache(` と
//!    `remove_video_frame_cache(` を通す（tako の Arc だけ落とすと GPU テクスチャが残る）
//! 4. [`手放したpdfのページを並びから外さない`] — PDF の並びは `page_in_layout(` で選ぶ
//!    （PNG が空かどうかで外すと、手放したページが消えてスクロール位置とページ番号がずれる）
//! 5. [`表に戻したら作り直す`] — 描画の冒頭で `restore_released_preview(` を呼び、
//!    再ラスタの判定が `has_released_pages(` を見る
//! 6. [`新しく入る前に実効上限を合わせる`] — デコード済みを LRU へ入れる前に
//!    `refresh_preview_budget(` を呼ぶ（古い小さな上限のまま入れると表示中の画像が追い出される）
//! 7. [`動画の開き直しは1本で裏ではフレームを写さない`] — 作り直しは `ensure_video_player(`
//!    （CLI / MCP の操作と同じ = 控えた位置で開く）を通り、ティッカーは `grab_frame_with(` で
//!    見えていない動画のフレームを写さない
//!
//! 各検査は**修正を外した形**を注入して file:line で名指しできることを
//! [`修正を外すと名指しできる`] で固定する。走査の区間が採れなければ落とす。

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const RENDER: &str = "crates/tako-app/src/preview_render.rs";

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

/// `head` で始まる関数の本文（1 始まりの行番号つき）。次の同じ字下げの関数の頭か
/// `#[cfg` の手前まで。採れなければ落とす（空振りで緑にしない）
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

fn find<'a>(body: &[(usize, &'a str)], needle: &str) -> Option<(usize, &'a str)> {
    body.iter().find(|(_, l)| l.contains(needle)).copied()
}

fn at(file: &str, (line_no, line): (usize, &str)) -> String {
    format!("{file}:{line_no}: {}", line.trim())
}

fn scan_periodic_sweep(main: &str) -> Vec<String> {
    let lines: Vec<(usize, &str)> = main.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
    let Some(span) = find(&lines, "\"periodic_prep:preview_memory\"") else {
        return vec![format!(
            "{MAIN}: 2 秒の定期更新に `periodic_prep:preview_memory` の区間が無い（#2009。\
             見えていないプレビューを手放す掃除が回らない）"
        )];
    };
    let called = lines
        .iter()
        .skip_while(|(n, _)| *n <= span.0)
        .take(3)
        .any(|(_, l)| l.contains("sweep_preview_memory("));
    if called {
        Vec::new()
    } else {
        vec![format!(
            "{} — 定期更新が `sweep_preview_memory` を呼んでいない（#2009）",
            at(MAIN, span)
        )]
    }
}

fn scan_visibility(render: &str) -> Vec<String> {
    let body = fn_body(RENDER, render, "pub(crate) fn sweep_preview_memory(");
    if find(&body, "pane_visibility(").is_some() {
        return Vec::new();
    }
    vec![format!(
        "{} — 掃除が見えているかを `pane_visibility` で決めていない（#2009。描いた時刻で\
         決めると、キャッシュビューで描き直さない間の表示中のプレビューを手放す）",
        at(RENDER, body[0])
    )]
}

fn scan_release_path(render: &str) -> Vec<String> {
    let body = fn_body(RENDER, render, "fn release_hidden_preview(");
    let mut out = Vec::new();
    for call in ["remove_preview_image_cache(", "remove_video_frame_cache("] {
        if find(&body, call).is_none() {
            out.push(format!(
                "{} — 手放しが `{call}` を通っていない（#2009。GPUI の asset と GPU テクスチャが残る）",
                at(RENDER, body[0])
            ));
        }
    }
    out
}

fn scan_pdf_layout(render: &str) -> Vec<String> {
    let body = fn_body(RENDER, render, "pub(crate) fn render_preview_pane(");
    let mut out = Vec::new();
    for hit in body
        .iter()
        .filter(|(_, l)| l.contains(".filter(|(_, png)| !png.is_empty())"))
    {
        out.push(format!(
            "{} — PDF の並びを PNG が空かどうかで選んでいる（#2009。手放したページが消え、\
             スクロール位置とページ番号がずれる。`page_in_layout` で選ぶ）",
            at(RENDER, *hit)
        ));
    }
    if find(&body, "page_in_layout(").is_none() {
        out.push(format!(
            "{} — PDF の並びが `page_in_layout` を通っていない（#2009）",
            at(RENDER, body[0])
        ));
    }
    out
}

fn scan_restore(render: &str) -> Vec<String> {
    let mut out = Vec::new();
    let body = fn_body(RENDER, render, "pub(crate) fn render_preview_pane(");
    match (
        find(&body, "restore_released_preview("),
        find(&body, "ensure_pdf_raster_quality("),
    ) {
        (Some(restore), Some(raster)) if restore.0 < raster.0 => {}
        (Some(restore), _) => out.push(format!(
            "{} — 作り直しが再ラスタの判定より後にある（#2009。表に戻した最初のフレームで\
             手放した印を外せない）",
            at(RENDER, restore)
        )),
        (None, _) => out.push(format!(
            "{} — 描画の冒頭で `restore_released_preview` を呼んでいない（#2009。表に戻しても\
             作り直しが数えられず、動画が開き直されない）",
            at(RENDER, body[0])
        )),
    }
    let raster = fn_body(RENDER, render, "fn ensure_pdf_raster_quality(");
    if find(&raster, "has_released_pages(").is_none() {
        out.push(format!(
            "{} — 再ラスタの判定が手放したページを見ていない（#2009。表に戻しても手放した\
             ページが空白のまま戻らない）",
            at(RENDER, raster[0])
        ));
    }
    out
}

fn scan_budget(render: &str) -> Vec<String> {
    let body = fn_body(RENDER, render, "fn ensure_preview_image_cache(");
    let Some(insert) = find(&body, "preview_image_lru.insert(") else {
        return vec![format!(
            "{} — デコード済みを LRU へ入れる行が無い（走査の前提が崩れた）",
            at(RENDER, body[0])
        )];
    };
    let refreshed = body
        .iter()
        .filter(|(n, _)| *n < insert.0)
        .any(|(_, l)| l.contains("refresh_preview_budget("));
    if refreshed {
        Vec::new()
    } else {
        vec![format!(
            "{} — 新しく入る前に実効上限を見えている量へ合わせていない（#2009。古い小さな上限の\
             まま入れると、表示中の画像が入った直後に追い出されて描けない）",
            at(RENDER, insert)
        )]
    }
}

fn scan_video(main: &str, render: &str) -> Vec<String> {
    let mut out = Vec::new();
    let restore = fn_body(RENDER, render, "fn restore_released_preview(");
    if find(&restore, "ensure_video_player(").is_none() {
        out.push(format!(
            "{} — 動画の開き直しが `ensure_video_player` を通っていない（#2009。CLI / MCP の\
             操作と表に戻す経路で控えた位置の扱いが割れる）",
            at(RENDER, restore[0])
        ));
    }
    for hit in restore
        .iter()
        .filter(|(_, l)| l.contains("VideoPlayer::open"))
    {
        out.push(format!(
            "{} — 作り直しがプレイヤーを直に開いている（#2009。`ensure_video_player` の 1 本へ寄せる）",
            at(RENDER, *hit)
        ));
    }
    let ensure = fn_body(MAIN, main, "fn ensure_video_player(");
    if find(&ensure, "video_resume").is_none() {
        out.push(format!(
            "{} — `ensure_video_player` が控えた再生状態を見ていない（#2009。手放した動画を\
             CLI / MCP から操作すると先頭から開き直す）",
            at(MAIN, ensure[0])
        ));
    }
    let ticker = fn_body(MAIN, main, "fn ensure_video_ticker(");
    if find(&ticker, "grab_frame_with(").is_none() {
        out.push(format!(
            "{} — ティッカーが見えていない動画のフレームも写している（#2009。裏で 30fps ×\
             1 フレームぶんの BGRA を作り続ける）",
            at(MAIN, ticker[0])
        ));
    }
    out
}

struct Sources {
    main: String,
    render: String,
}

impl Sources {
    fn load() -> Self {
        Self {
            main: read_code(MAIN),
            render: read_code(RENDER),
        }
    }

    fn scan(&self) -> Vec<String> {
        let mut out = scan_periodic_sweep(&self.main);
        out.extend(scan_visibility(&self.render));
        out.extend(scan_release_path(&self.render));
        out.extend(scan_pdf_layout(&self.render));
        out.extend(scan_restore(&self.render));
        out.extend(scan_budget(&self.render));
        out.extend(scan_video(&self.main, &self.render));
        out
    }
}

#[test]
fn 定期更新から掃除を回す() {
    let s = Sources::load();
    let hits = scan_periodic_sweep(&s.main);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 見えているかは描画の経路と同じ判定() {
    let s = Sources::load();
    let hits = scan_visibility(&s.render);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 手放しはgpuiからの解放経路を通す() {
    let s = Sources::load();
    let hits = scan_release_path(&s.render);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 手放したpdfのページを並びから外さない() {
    let s = Sources::load();
    let hits = scan_pdf_layout(&s.render);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 表に戻したら作り直す() {
    let s = Sources::load();
    let hits = scan_restore(&s.render);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 新しく入る前に実効上限を合わせる() {
    let s = Sources::load();
    let hits = scan_budget(&s.render);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn 動画の開き直しは1本で裏ではフレームを写さない() {
    let s = Sources::load();
    let hits = scan_video(&s.main, &s.render);
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

/// 修正を 1 つずつ外した形を注入して、それぞれが file:line で名指しされることを固定する
#[test]
fn 修正を外すと名指しできる() {
    let base = Sources::load();
    assert!(
        base.scan().is_empty(),
        "前提: 今の形は緑: {:?}",
        base.scan()
    );

    let inject = |edit: &dyn Fn(&mut Sources), expect: &str| {
        let mut s = Sources {
            main: base.main.clone(),
            render: base.render.clone(),
        };
        edit(&mut s);
        let hits = s.scan();
        assert!(
            hits.iter().any(|h| h.contains(expect)),
            "注入（{expect}）を名指しできない: {hits:?}"
        );
        // 名指しは file:line の形
        assert!(
            hits.iter()
                .filter(|h| h.contains(expect))
                .all(|h| h.starts_with("crates/") && h.contains(".rs")),
            "file:line で名指ししていない: {hits:?}"
        );
    };
    let replace_once = |src: &mut String, from: &str, to: &str| {
        assert!(src.contains(from), "注入の目印が無い: {from}");
        *src = src.replacen(from, to, 1);
    };

    // 1. 定期更新から掃除を外す
    inject(
        &|s| {
            replace_once(
                &mut s.main,
                "if app.sweep_preview_memory(std::time::Instant::now()) {",
                "if false {",
            )
        },
        "`sweep_preview_memory` を呼んでいない",
    );
    // 2. 見えているかを描画の経路で決めない
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                "            if self.pane_visibility(pane) != PaneVisibility::Hidden {\n                self.preview_hidden_since.remove(&pane);",
                "            if self.preview_last_drawn(pane) {\n                self.preview_hidden_since.remove(&pane);",
            )
        },
        "`pane_visibility` で決めていない",
    );
    // 3. GPUI からの解放を通さない
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                "        self.remove_preview_image_cache(pane);",
                "        self.preview_image_cache.remove(&pane);",
            )
        },
        "`remove_preview_image_cache(` を通っていない",
    );
    // 4. PDF の並びを PNG が空かどうかで選ぶ（修正前の形）
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                ".filter(|(i, _)| data.page_in_layout(*i))",
                ".filter(|(_, png)| !png.is_empty())",
            )
        },
        "PNG が空かどうかで選んでいる",
    );
    // 5a. 描画の冒頭で作り直さない
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                "        self.restore_released_preview(pane_id, cx);\n",
                "",
            )
        },
        "`restore_released_preview` を呼んでいない",
    );
    // 5b. 再ラスタの判定が手放したページを見ない（修正前の形）
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                "if data.raster_key == desired && !data.has_released_pages() {",
                "if data.raster_key == desired {",
            )
        },
        "手放したページを見ていない",
    );
    // 6. 入れる前に実効上限を合わせない
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                "                self.refresh_preview_budget();\n            }\n            if let Some(slot) = self",
                "            }\n            if let Some(slot) = self",
            )
        },
        "実効上限を見えている量へ合わせていない",
    );
    // 7a. 作り直しがプレイヤーを直に開く
    inject(
        &|s| {
            replace_once(
                &mut s.render,
                "        match self.ensure_video_player(pane) {",
                "        match video_player::VideoPlayer::open(&path).map(|_| ()) {",
            )
        },
        "`ensure_video_player` を通っていない",
    );
    // 7b. ティッカーが常にフレームを写す（修正前の形）
    inject(
        &|s| {
            replace_once(
                &mut s.main,
                "player.grab_frame_with(!hidden.contains(pane));",
                "player.grab_frame();",
            )
        },
        "見えていない動画のフレームも写している",
    );
}
