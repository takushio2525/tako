//! **#2610 の番犬**: `save_layout` は layout.json の書き出しをメインスレッドで待たない。
//!
//! ## なぜ縛るのか
//!
//! `save_layout`（tako-app）は 2 秒 tick と dispatch ごとに GUI のメインスレッドで走る。
//! 以前は書き出し（直前の版の読み直し・世代バックアップ・整形・write・rename）をそこで同期し、
//! さらにペインログの Mutex（背景の取り込みがファイルへの追記の間ずっと握る）を待っていたので、
//! ディスクが詰まると GUI が丸ごと固まった（本番の perf.log で最大 98,982ms）。
//! いまは書き出しを専用の書き手（`tako_control::layout_writer::LayoutWriter`）へ渡し、
//! ペインログの取り込み位置は `try_lock` で読む（握られていれば前回の値で比べ、次の呼び出しで追いつく）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `save_layout` の本体で `layout::save(` を呼ぶのは A/B（`legacy_2610()`）の枝の中だけ。
//!    既定の道は `self.layout_writer.post(` で渡す
//! 2. tako-app の中で layout.json へ直に書く・消す（`layout::save(` / `layout::remove()`）のは
//!    上の A/B の枝と、消すの A/B の枝（`fn remove_layout_file`）だけ
//!    （ほかから直に書くと、書き手に渡した古い版が後から上書きする = 順序が崩れる）
//! 3. `save_layout` の本体はペインログの Mutex を待たない（`pane_logs_lock()` を呼ばず `try_lock`。
//!    待ってでも読む `refresh_pane_log_history()` は A/B の枝の中だけ）
//! 4. 終了（`on_app_quit`）は書き手を待ってから抜ける: `flush_layout_writer(` が
//!    `if !this.quitting { … }` の外（最後のタブの明示クローズの削除も待つ）・
//!    `cleanup_process_scoped(` より前（検証起動の置き場を消した後で書き戻さない）。
//!    最後の保存の前に `refresh_pane_log_history()` で取り込み位置を最新にする
//! 5. 新しいプロセスへの引き渡し（更新の起動し直し 2 か所）は `refresh_pane_log_history()` →
//!    `save_layout()` → `flush_layout_writer(` → `restart_app(` の順（新しいプロセスが古い layout.json を読まない）

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const UPDATE_WINDOW: &str = "crates/tako-app/src/update_window.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// ソースを読み、行コメント（`//` から行末）を同じバイト数の空白へ置き換えて返す
/// （コメントアウトした呼び出しを「ある」と数えない。バイト位置・行番号はずれない）
fn read(rel: &str) -> String {
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    strip_line_comments(&src)
}

fn strip_line_comments(src: &str) -> String {
    let mut bytes = src.as_bytes().to_vec();
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            // 文字リテラルの `'"'` は文字列の始まりではない
            b'\''
                if !in_str
                    && bytes.get(i + 1) == Some(&b'"')
                    && bytes.get(i + 2) == Some(&b'\'') =>
            {
                i += 2
            }
            b'"' => in_str = !in_str,
            b'\n' => in_str = false,
            b'/' if !in_str && bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    bytes[i] = b' ';
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    // 置き換えたのは行末までの全バイト（多バイト文字は丸ごと空白になる）なので UTF-8 のまま
    String::from_utf8(bytes).expect("コメントを空白にしても UTF-8")
}

#[test]
fn issue2610_番犬はコメントアウトした呼び出しを数えない() {
    let src = "a();\n// this.flush_layout_writer(\"x\");\nlet u = \"https://x\"; b(); // 説明\n";
    let code = strip_line_comments(src);
    assert_eq!(code.len(), src.len(), "バイト位置がずれた");
    assert!(!code.contains("flush_layout_writer"));
    assert!(
        code.contains("\"https://x\"; b();"),
        "文字列の中の // を消した"
    );
    assert!(!code.contains("説明"));
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列・コメントの中の括弧は数えない粗い版。
/// 対象の関数はその形を持たない）
fn block_end(src: &str, open: usize) -> usize {
    let bytes = src.as_bytes();
    let start = src[open..].find('{').map(|i| open + i).expect("`{` がある");
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("対応する `}}` が無い");
}

/// `sig` で始まる関数の本体の範囲（シグネチャの位置〜閉じ括弧）
fn fn_span(src: &str, sig: &str, rel: &str) -> (usize, usize) {
    let at = src.find(sig).unwrap_or_else(|| {
        panic!("{rel} に `{sig}` が無い（構造が変わったらこの番犬も見直すこと）")
    });
    (at, block_end(src, at))
}

/// 本体の中の A/B の枝（`if legacy_2610() { … }`）の範囲をすべて返す
fn legacy_blocks(body: &str) -> Vec<std::ops::Range<usize>> {
    all_at(body, "if legacy_2610() {")
        .into_iter()
        .map(|at| at..block_end(body, at))
        .collect()
}

/// `needle` の出現位置をすべて返す
fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// 1: 既定の道は書き手へ渡す。直に書くのは A/B の枝だけ
#[test]
fn issue2610_save_layoutは書き出しを書き手へ渡す() {
    let src = read(MAIN);
    let (start, end) = fn_span(&src, "    fn save_layout(&mut self) {", MAIN);
    let body = &src[start..end];
    assert!(
        body.contains("self.layout_writer.post("),
        "{MAIN}:{} の `save_layout` が書き手へ版を渡していない（#2610 が戻っている）",
        line_at(&src, start)
    );
    let legacy = legacy_blocks(body);
    assert!(
        !legacy.is_empty(),
        "{MAIN}:{} の `save_layout` に A/B の枝 `if legacy_2610() {{` が無い",
        line_at(&src, start)
    );
    for at in all_at(body, "layout::save(") {
        assert!(
            legacy.iter().any(|r| r.contains(&at)),
            "{MAIN}:{} の `save_layout` が A/B の枝の外で layout.json を同期で書いている\
             （メインスレッドが I/O を待つ = #2610 の 99 秒の凍結が戻る）。書き出しは `self.layout_writer.post(` へ",
            line_at(&src, start + at)
        );
    }
}

/// 2: tako-app の中で layout.json を直に書く・消すのは A/B の枝だけ
#[test]
fn issue2610_layout_jsonへ直に書くのは書き手とab枝だけ() {
    let src_dir = repo_root().join("crates/tako-app/src");
    let mut allowed: Vec<(String, std::ops::Range<usize>)> = Vec::new();
    let main = read(MAIN);
    for sig in [
        "    fn save_layout(&mut self) {",
        "    fn remove_layout_file(&mut self) {",
    ] {
        let (start, end) = fn_span(&main, sig, MAIN);
        let body = &main[start..end];
        for r in legacy_blocks(body) {
            allowed.push((MAIN.to_string(), start + r.start..start + r.end));
        }
    }
    let mut offenders = Vec::new();
    let mut stack = vec![src_dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("src を読む").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(repo_root())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let text = strip_line_comments(&std::fs::read_to_string(&path).unwrap());
            // テストの中（番犬が綴りを並べる箇所）は数えない
            let code_end = text.find("\n#[cfg(test)]").unwrap_or(text.len());
            for needle in [
                "tako_control::layout::save(",
                "tako_control::layout::remove()",
            ] {
                for at in all_at(&text[..code_end], needle) {
                    let ok = allowed
                        .iter()
                        .any(|(file, range)| *file == rel && range.contains(&at));
                    if !ok {
                        offenders.push(format!("{rel}:{} `{needle}`", line_at(&text, at)));
                    }
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "layout.json を書き手を通さずに書く・消す呼び出しがある（書き手に渡した古い版が後から上書きする = \
         順序が崩れる・メインスレッドが I/O を待つ。#2610）。`self.layout_writer.post(` / \
         `self.remove_layout_file()` を使う:\n{}",
        offenders.join("\n")
    );
}

/// 3: ペインログの Mutex を待たない（背景の取り込みがファイルへの追記の間ずっと握る）
#[test]
fn issue2610_save_layoutはペインログのmutexを待たない() {
    let src = read(MAIN);
    let (start, end) = fn_span(&src, "    fn save_layout(&mut self) {", MAIN);
    let body = &src[start..end];
    let legacy = legacy_blocks(body);
    for needle in ["pane_logs_lock()", "refresh_pane_log_history()"] {
        for at in all_at(body, needle) {
            assert!(
                legacy.iter().any(|r| r.contains(&at)),
                "{MAIN}:{} の `save_layout` がペインログの Mutex を待っている（`{needle}`。背景の取り込みは \
                 追記の I/O の間ずっと握るので、ディスクが詰まると GUI が固まる = #2610）。\
                 `try_lock` で読み、握られていれば前回の値を使う",
                line_at(&src, start + at)
            );
        }
    }
    assert!(
        body.contains(".try_lock()"),
        "{MAIN}:{} の `save_layout` がペインログの取り込み位置を `try_lock` で読んでいない",
        line_at(&src, start)
    );
}

/// 4: 終了は書き手を待ってから抜ける
#[test]
fn issue2610_終了は書き手を待ってから後片付けへ進む() {
    let src = read(MAIN);
    let quit = src
        .find("cx.on_app_quit(")
        .expect("on_app_quit の登録が見つからない（構造が変わったらこの番犬も見直すこと）");
    let quit_end = quit
        + src[quit..]
            .find("\n        .detach();")
            .expect("on_app_quit クロージャの終端");
    let body = &src[quit..quit_end];
    let flush = body.find("this.flush_layout_writer(").unwrap_or_else(|| {
        panic!(
            "{MAIN}:{} の終了処理が layout の書き手を待っていない（GPUI の終了の猶予 200ms では \
             背景の書き出しが終わらず、最後の変更が残らない = #2610）",
            line_at(&src, quit)
        )
    });
    let quitting = body
        .find("if !this.quitting {")
        .expect("終了処理の `if !this.quitting {` が見つからない");
    let quitting_end = block_end(body, quitting);
    assert!(
        flush > quitting_end,
        "{MAIN}:{} の `flush_layout_writer(` が `if !this.quitting {{ … }}` の中にある\
         （最後のタブの明示クローズは quitting で来るので、渡した削除を待たずに抜ける）",
        line_at(&src, quit + flush)
    );
    let cleanup = body
        .find("cleanup_process_scoped(")
        .expect("終了処理の `cleanup_process_scoped(` が見つからない");
    assert!(
        flush < cleanup,
        "{MAIN}:{} の `flush_layout_writer(` が `cleanup_process_scoped(` より後ろにある\
         （検証起動の置き場を消した後で書き手が書き戻す）",
        line_at(&src, quit + flush)
    );
    let save = body
        .find("this.save_layout();")
        .expect("終了処理の `this.save_layout();` が見つからない");
    let refresh = body.find("this.refresh_pane_log_history();");
    assert!(
        refresh.is_some_and(|r| r < save),
        "{MAIN}:{} の終了処理の最後の保存の前に `refresh_pane_log_history()` が無い\
         （`save_layout` は Mutex を待たないので、取り込み位置が古いまま残り再起動後に二重に取り込む）",
        line_at(&src, quit + save)
    );
}

/// 5: 新しいプロセスへの引き渡しの前は書き手を待つ
#[test]
fn issue2610_起動し直しの前は書き手を待つ() {
    for (rel, sig) in [
        (MAIN, "    fn hand_over_to_installer("),
        (UPDATE_WINDOW, "    fn finish_update("),
    ] {
        let src = read(rel);
        let (start, end) = fn_span(&src, sig, rel);
        let body = &src[start..end];
        let find = |needle: &str| {
            body.find(needle).unwrap_or_else(|| {
                panic!(
                    "{rel}:{} に `{needle}` が無い（起動し直しの前は取り込み位置を最新にして保存し、\
                     背景の書き手を待つ = 新しいプロセスが書き終わる前の layout.json を読まない。#2610）",
                    line_at(&src, start)
                )
            })
        };
        let refresh = find("self.refresh_pane_log_history();");
        let save = find("self.save_layout();");
        let flush = find("self.flush_layout_writer(");
        let restart = find("restart_app(");
        assert!(
            refresh < save && save < flush && flush < restart,
            "{rel}:{} の順序が `refresh_pane_log_history()` → `save_layout()` → \
             `flush_layout_writer(` → `restart_app(` になっていない",
            line_at(&src, start + flush)
        );
    }
}
