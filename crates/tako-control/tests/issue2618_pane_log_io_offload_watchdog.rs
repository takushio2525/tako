//! **#2618 の番犬**: ペインログのファイルへの操作をマネージャーの Mutex の内側・メインスレッドで行わない。
//!
//! ## なぜ縛るのか
//!
//! ペインログのマネージャー（`tako_core::pane_log::PaneLogManager`）は GUI の 1 つの Mutex 越しに、
//! メインスレッド（直接ペインの取り込み・閉じたときのフラッシュ・`tako logs status` ほか）と背景
//! （器のペインの取り込み・器を kill する直前のフラッシュ・全体上限の強制）の両方から使われる。
//! 以前は追記・ローテーション・全体上限の削除を Mutex を握ったまま呼び手のスレッドで行っていたので、
//! 追記に 5 秒の遅れを注入すると `periodic_prep:pane_log が 5002ms`・閉じる IPC が 15,031ms、
//! 背景が握る間に Mutex を取る口が `dispatch:Logs が 9939ms` で GUI を止めた。
//! いまはマネージャーはメモリの操作だけを行い、ファイルへの操作は専用の書き手
//! （`tako_core::pane_log_writer::PaneLogWriter`）の列へ積む。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `impl PaneLogManager` と `append_lines_to` の本体はファイルに触らない（`std::fs::` /
//!    `OpenOptions` / `write_all` / `rotate(` / `enforce_total_cap_at(` を呼ばない）
//! 2. pane_log.rs の中でローテーション・全体上限の削除を呼ぶのは定義だけ（呼ぶのは書き手の `WriteOp::run`）
//! 3. GUI のマネージャーは書き手を差し込んで作る（`.with_writer(`）。同期の書き手
//!    （`PaneLogWriter::inline()`）は A/B（`issue2618_legacy()`）の枝の中だけ
//! 4. 終了（`on_app_quit`）は書き手を待ってから抜ける: `flush_pane_log_writer(` が条件の枝の外
//!    （`flush_layout_writer(` と同じ深さ）・直接ペインのクローズフラッシュより後・`cleanup_process_scoped(` より前
//! 5. 全体上限の受け口（`queue_total_cap()`）はマネージャーの Mutex を放してから待つ
//!    （同期の `enforce_total_cap()` へ戻さない）

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const CORE_LOG: &str = "crates/tako-core/src/pane_log.rs";
const CORE_WRITER: &str = "crates/tako-core/src/pane_log_writer.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）。下限つき = 黙って縮んだら落ちる
fn read(rel: &str) -> String {
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production(&src, rel))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置。文字列の中の括弧は数えない
fn block_end(src: &str, open: usize) -> usize {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = code[open..]
        .find('{')
        .map(|i| open + i)
        .expect("`{` がある");
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

/// `sig` で始まるブロックの範囲（シグネチャの位置〜閉じ括弧）
fn span_of(src: &str, sig: &str, rel: &str) -> (usize, usize) {
    let at = src.find(sig).unwrap_or_else(|| {
        panic!("{rel} に `{sig}` が無い（構造が変わったらこの番犬も見直すこと）")
    });
    (at, block_end(src, at))
}

/// `src` の先頭から `at` までの括弧の深さ（文字列の中は数えない）
fn depth_at(src: &str, at: usize) -> i64 {
    let code = production_range::code_view_of(src);
    code.as_bytes()[..at].iter().fold(0i64, |d, b| match b {
        b'{' => d + 1,
        b'}' => d - 1,
        _ => d,
    })
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// 1: マネージャーの本体（= Mutex の内側で走るもの）はファイルに触らない
#[test]
fn issue2618_ペインログのマネージャーはmutexの内側でファイルに触らない() {
    let src = read(CORE_LOG);
    let mut offenders = Vec::new();
    for sig in ["impl PaneLogManager {", "fn append_lines_to("] {
        let (start, end) = span_of(&src, sig, CORE_LOG);
        let body = &src[start..end];
        for needle in [
            "std::fs::",
            "OpenOptions",
            ".write_all(",
            "rotate(&",
            "enforce_total_cap_at(",
            ".is_file()",
            "create_dir_all(",
        ] {
            for at in all_at(body, needle) {
                offenders.push(format!(
                    "{CORE_LOG}:{} `{needle}`（`{sig}` の中）",
                    line_at(&src, start + at)
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ペインログのマネージャーが Mutex の内側でファイルに触っている（ディスクが詰まると、メインスレッドの \
         取り込み・閉じたときのフラッシュと Mutex を待つ口がその長さだけ GUI を止める = #2618）。\
         ファイルへの操作は `PaneLogWriter::submit(WriteOp::…)` で書き手の列へ積む:\n{}",
        offenders.join("\n")
    );
}

/// 2: ローテーション・全体上限の削除を呼ぶのは書き手だけ
#[test]
fn issue2618_ローテーションと全体上限の削除を呼ぶのは書き手だけ() {
    let log = read(CORE_LOG);
    let mut offenders = Vec::new();
    for (needle, def) in [
        ("rotate(", "pub(crate) fn rotate("),
        ("enforce_total_cap_at(", "pub fn enforce_total_cap_at("),
    ] {
        let def_at = log.find(def).unwrap_or_else(|| {
            panic!("{CORE_LOG} に `{def}` が無い（構造が変わったらこの番犬も見直すこと）")
        });
        for at in all_at(&log, needle) {
            // 定義そのもの・`WriteOp::Rotate(` などの綴りの一部は数えない
            let is_def = at >= def_at && at < def_at + def.len();
            let prev = log[..at].chars().next_back();
            let part_of_name = prev.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !is_def && !part_of_name {
                offenders.push(format!("{CORE_LOG}:{} `{needle}`", line_at(&log, at)));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "pane_log.rs の中でローテーション・全体上限の削除を直に呼んでいる（呼び手のスレッドで I/O する = \
         Mutex の内側なら GUI を止める・書き手の列の外なら追記との順序が崩れる。#2618）。\
         `WriteOp::Rotate` / `queue_total_cap()` で書き手へ渡す:\n{}",
        offenders.join("\n")
    );
    let writer = read(CORE_WRITER);
    let (start, end) = span_of(&writer, "    pub(crate) fn run(&self) {", CORE_WRITER);
    let run = &writer[start..end];
    for needle in [
        "crate::pane_log::rotate(",
        "crate::pane_log::enforce_total_cap_at(",
        "append_file(",
    ] {
        assert!(
            run.contains(needle),
            "{CORE_WRITER}:{} の `WriteOp::run` が `{needle}` を呼んでいない（書き手が本物の I/O をしていない）",
            line_at(&writer, start)
        );
    }
}

/// 3: GUI のマネージャーは背景の書き手で書く（同期の書き手は A/B の枝だけ）
#[test]
fn issue2618_guiのペインログは背景の書き手で書く() {
    let src = read(MAIN);
    let new_at = src
        .find("tako_core::pane_log::PaneLogManager::new(")
        .expect("GUI のペインログのマネージャーの生成が見つからない（構造が変わったらこの番犬も見直すこと）");
    // 生成の式の終わり = `pane_logs: Arc::new(Mutex::new( … ))` の次のフィールド
    let stmt_end = new_at
        + src[new_at..]
            .find("pane_log_pace:")
            .expect("マネージャーの生成の後ろのフィールド `pane_log_pace:`");
    assert!(
        src[new_at..stmt_end].contains(".with_writer("),
        "{MAIN}:{} の GUI のペインログのマネージャーが書き手を差し込まずに作られている（既定の同期の書き手 = \
         Mutex の内側・メインスレッドで I/O する = #2618 が戻る）。`.with_writer(pane_log_writer.clone())`",
        line_at(&src, new_at)
    );
    let gate = src
        .find("if tako_core::pane_log::issue2618_legacy() {")
        .unwrap_or_else(|| {
            panic!("{MAIN} に A/B の枝 `if tako_core::pane_log::issue2618_legacy() {{` が無い")
        });
    let gate_end = block_end(&src, gate);
    for at in all_at(&src, "PaneLogWriter::inline()") {
        assert!(
            at > gate && at < gate_end,
            "{MAIN}:{} が A/B の枝の外で同期の書き手を使っている（メインスレッドで I/O する = #2618 が戻る）",
            line_at(&src, at)
        );
    }
    let after = &src[gate_end + 1..];
    let else_ok = after.trim_start().starts_with("else {") && {
        let else_at = gate_end + 1 + after.find("else {").expect("else の枝");
        src[else_at..block_end(&src, else_at)].contains("PaneLogWriter::spawn(")
    };
    assert!(
        else_ok,
        "{MAIN}:{} の既定の道（`else {{ … }}`）が背景の書き手（`PaneLogWriter::spawn(`）を起こしていない",
        line_at(&src, gate)
    );
}

/// 4: 終了は書き手を待ってから抜ける
#[test]
fn issue2618_終了はペインログの書き手を待ってから後片付けへ進む() {
    let src = read(MAIN);
    let quit = src
        .find("cx.on_app_quit(")
        .expect("on_app_quit の登録が見つからない（構造が変わったらこの番犬も見直すこと）");
    let quit_end = quit
        + src[quit..]
            .find("\n        .detach();")
            .expect("on_app_quit クロージャの終端");
    let body = &src[quit..quit_end];
    let flush = body.find("this.flush_pane_log_writer(").unwrap_or_else(|| {
        panic!(
            "{MAIN}:{} の終了処理がペインログの書き手を待っていない（背景の書き手へ積んだ直接ペインの \
             最後の行とクローズマーカーを書き終える前にプロセスが終わる = #2618）",
            line_at(&src, quit)
        )
    });
    let layout_flush = body
        .find("this.flush_layout_writer(")
        .expect("終了処理の `flush_layout_writer(` が見つからない");
    assert_eq!(
        depth_at(body, flush),
        depth_at(body, layout_flush),
        "{MAIN}:{} の `flush_pane_log_writer(` が条件の枝の中にある（`flush_layout_writer(` と同じ深さに置く。\
         quitting でも・どの条件でも待つ = 最後のタブの明示クローズのフラッシュもここで待つ）",
        line_at(&src, quit + flush)
    );
    let direct_close = body
        .find("CloseOrigin::AppQuit")
        .expect("終了処理の直接ペインのクローズフラッシュが見つからない");
    let cleanup = body
        .find("cleanup_process_scoped(")
        .expect("終了処理の `cleanup_process_scoped(` が見つからない");
    assert!(
        direct_close < flush && flush < cleanup,
        "{MAIN}:{} の `flush_pane_log_writer(` が直接ペインのクローズフラッシュの後・`cleanup_process_scoped(` の前に無い\
         （積む前に待つと最後の行を待たない・後片付けの後だと検証起動の置き場を消した後で書き戻す）",
        line_at(&src, quit + flush)
    );
}

/// 5: 全体上限の受け口はマネージャーの Mutex を放してから待つ
#[test]
fn issue2618_全体上限の受け口はmutexを放してから待つ() {
    let src = read(MAIN);
    let sync_cap: Vec<String> = all_at(&src, ".enforce_total_cap()")
        .into_iter()
        .map(|at| format!("{MAIN}:{}", line_at(&src, at)))
        .collect();
    assert!(
        sync_cap.is_empty(),
        "同期の全体上限（`.enforce_total_cap()`）を呼んでいる（Mutex を握ったまま削除の I/O をする = #2618）。\
         `queue_total_cap()` を積み、Mutex を放してから `.recv()` で待つ:\n{}",
        sync_cap.join("\n")
    );
    let queued = all_at(&src, ".queue_total_cap()");
    assert!(
        !queued.is_empty(),
        "{MAIN} に全体上限の強制（`.queue_total_cap()`）が無い（構造が変わったらこの番犬も見直すこと）"
    );
    for at in queued {
        let rest = &src[at..];
        let recv = rest
            .find(".recv()")
            .unwrap_or_else(|| panic!("{MAIN}:{} の受け口を待っていない", line_at(&src, at)));
        assert!(
            rest[..recv].contains('}'),
            "{MAIN}:{} が Mutex のガードを持ったまま全体上限の受け口を待っている（書き手が前の追記を書き終えるまで \
             Mutex を握る = メインスレッドの取り込み・閉じたときのフラッシュを止める。#2618）。\
             ガードのブロックを閉じてから `.recv()` する",
            line_at(&src, at)
        );
    }
}
