//! **#2657 の番犬**: ペインの close（`Close` / たまり場の `BackgroundKill` / GUI の × / ⌘W / タブ close）が、
//! worker レジストリへの close の記録を GUI のメインスレッドで書かない。
//!
//! ## なぜ縛るのか
//!
//! 記録（`registry::mark_closed_by_origin`）は workers.yaml（本番で約 225 KB・200 件）を読んで YAML を解釈し、
//! 上限の無い flock（`workers.yaml.lock`）を取り、書き出して世代バックアップを回し、`sync_all`（macOS は
//! F_FULLFSYNC）まで行う。#2618 の段 1 の実測では、器の worker の close の UI スレッドの専有 18 / 37ms
//! （中央 / 最大）の 9 割以上がこの記録で、注入なしでも YAML の解釈 87.6ms・読み 30.5ms・fsync 21.3ms の裾が出て、
//! 別のプロセスが lock を 400ms 握る注入で `dispatch:Close が 350ms` になった。persist.log への「セッション kill」の
//! 追記も注入なしで 1 回 62.3ms だった。本番の perf.log の `dispatch:Close` 16 件（32〜263ms）はすべて
//! master による器の worker の close。
//!
//! いまは IPC の受け口が `prepare_offload` → 着地（`land_offload` の `close_place` = UI の段）→
//! background の記録の段（`OffloadJob::CloseRecord` → `CloseRecord::finish`）で、書き終わってから応答する。
//! GUI の close は `CloseMark::spawn`（背景のスレッド）で書き、終了の直前に `flush_close_records` が待つ。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `prepare_offload` に `Request::Close { .. } | Request::BackgroundKill { .. }` の腕があり、A/B の門
//!    （`close_record::legacy()`）を持って `OffloadJob::ClosePlace(` へ包む
//! 2. `run_staged` が `OffloadJob::ClosePlace` を UI の段へ戻し（`OffloadContinuation::ClosePlace`）、
//!    `OffloadJob::CloseRecord` を `.finish()` してから応答する
//! 3. `land_offload` の `OffloadContinuation::ClosePlace` の腕は `close_place` を呼んで `OffloadLanding::Settled` で
//!    `OffloadJob::CloseRecord` を出し、**記録を書かない**（`.finish()` / `.write()` を呼ばない）
//! 4. UI の段（`fn close_place(` / `fn close_pane_place(` / `fn background_kill_place(`）は記録を書かない
//! 5. dispatch.rs と tako-app は `mark_closed_by_origin` / `mark_closed_by_pane` を直に呼ばない
//!    （書くのは `close_record::CloseMark::write` の 1 実装）
//! 6. GUI の `fn mark_worker_closed(` は A/B の門の外で `.spawn()`（背景のスレッド）へ投げ、`on_app_quit` が
//!    `flush_close_records(` で書き終わりを待つ
//! 7. GUI の `fn drop_backend_session(` の「セッション kill」の persist.log への追記は kill のスレッドの中で書く
//!    （UI スレッドで書くのは A/B の門の中だけ）
//! 8. IPC の受信ループの #641 の「入力」の記録（`counts_as_input(`）は準備（`prepare_offload(app, &incoming.request)`）
//!    より前の 1 か所（close が準備 → 着地の経路へ移っても数え漏れない）

use std::path::{Path, PathBuf};

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const RECORD: &str = "crates/tako-control/src/close_record.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）
fn code_of(src: &str, rel: &str) -> String {
    production_range::code_view::without_comments(&production_range::production(src, rel))
}

struct Sources {
    dispatch: String,
    main: String,
    record: String,
}

fn sources() -> Sources {
    let read = |rel: &str| {
        let src = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
        code_of(&src, rel)
    };
    Sources {
        dispatch: read(DISPATCH),
        main: read(MAIN),
        record: read(RECORD),
    }
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at.min(src.len())].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> Option<usize> {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = code[open..].find('{').map(|i| open + i)?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `sig` で始まる関数（か impl）の範囲。見つからなければ `None`
fn span(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// `body` の中の `needle` で始まる腕。腕は `stops` のどれかが次に現れるところまで
fn arm_until<'a>(body: &'a str, needle: &str, stops: &[&str]) -> Option<(usize, &'a str)> {
    let at = body.find(needle)?;
    let rest = &body[at + needle.len()..];
    let stop = stops
        .iter()
        .filter_map(|s| rest.find(s))
        .min()
        .map_or(rest.len(), |i| i + needle.len());
    Some((at, &body[at..at + stop]))
}

/// 記録を書く呼び出し（UI スレッドで呼んではいけないもの）
const WRITES: [&str; 4] = [
    "mark_closed_by_origin(",
    "mark_closed_by_pane(",
    ".finish()",
    ".write()",
];

fn scan(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();

    // 1. prepare_offload の腕
    match span(&s.dispatch, "pub fn prepare_offload(") {
        None => out.push(format!(
            "{DISPATCH}:1 `prepare_offload` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            let needle = "Request::Close { .. } | Request::BackgroundKill { .. }";
            match arm_until(body, needle, &["Request::"]) {
                None => out.push(format!(
                    "{DISPATCH}:{} `prepare_offload` に `{needle}` の腕が無い。close の記録（workers.yaml の読み書き + \
                     fsync + 上限なしの flock）を UI スレッドで待つ（#2657）。`OffloadJob::ClosePlace(` へ包む",
                    line_at(&s.dispatch, start)
                )),
                Some((at, arm)) => {
                    let line = line_at(&s.dispatch, start + at);
                    if !arm.contains("OffloadJob::ClosePlace(") {
                        out.push(format!(
                            "{DISPATCH}:{line} close の準備の腕が `OffloadJob::ClosePlace(` へ包んでいない（#2657）"
                        ));
                    }
                    if !arm.contains("close_record::legacy()") {
                        out.push(format!(
                            "{DISPATCH}:{line} close の準備の腕に A/B の門（`close_record::legacy()`）が無い（#2657）"
                        ));
                    }
                }
            }
        }
    }

    // 2. run_staged: 準備の一巡は UI の段へ戻し、記録の段は書いてから応答する
    match span(&s.dispatch, "pub fn run_staged(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_staged` が無い（走査の空振り）")),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            let stops = ["OffloadJob::", "other =>"];
            match arm_until(body, "OffloadJob::ClosePlace(", &stops) {
                Some((at, arm)) if !arm.contains("OffloadContinuation::ClosePlace(") => {
                    out.push(format!(
                        "{DISPATCH}:{} `run_staged` の `OffloadJob::ClosePlace` が UI の段（`OffloadContinuation::ClosePlace`）\
                         へ戻していない（ペインを閉じるのは着地 = UI スレッド。#2657）",
                        line_at(&s.dispatch, start + at)
                    ));
                }
                Some(_) => {}
                None => out.push(format!(
                    "{DISPATCH}:{} `run_staged` が `OffloadJob::ClosePlace` を扱っていない（#2657）",
                    line_at(&s.dispatch, start)
                )),
            }
            match arm_until(body, "OffloadJob::CloseRecord(", &stops) {
                Some((at, arm)) if !arm.contains(".finish()") || !arm.contains("OffloadOutcome::Reply(") => {
                    out.push(format!(
                        "{DISPATCH}:{} `run_staged` の `OffloadJob::CloseRecord` が記録を書いて（`.finish()`）から応答\
                         （`OffloadOutcome::Reply`）していない（close の応答の後に workers を読めば closed。#2657）",
                        line_at(&s.dispatch, start + at)
                    ));
                }
                Some(_) => {}
                None => out.push(format!(
                    "{DISPATCH}:{} `run_staged` が `OffloadJob::CloseRecord` を扱っていない（#2657）",
                    line_at(&s.dispatch, start)
                )),
            }
        }
    }

    // 3. land_offload: UI の段は Settled で記録の段を出し、記録を書かない
    match span(&s.dispatch, "pub fn land_offload(") {
        None => out.push(format!(
            "{DISPATCH}:1 `land_offload` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            match arm_until(
                body,
                "OffloadContinuation::ClosePlace(",
                &["OffloadContinuation::", "other =>"],
            ) {
                None => out.push(format!(
                    "{DISPATCH}:{} `land_offload` に `OffloadContinuation::ClosePlace` の腕が無い（`finish_offload` へ落ちると\
                     UI スレッドで記録まで書く。#2657）",
                    line_at(&s.dispatch, start)
                )),
                Some((at, arm)) => {
                    let line = line_at(&s.dispatch, start + at);
                    if !arm.contains("close_place(")
                        || !arm.contains("OffloadLanding::Settled(")
                        || !arm.contains("OffloadJob::CloseRecord(")
                    {
                        out.push(format!(
                            "{DISPATCH}:{line} `land_offload` の close の腕が UI の段（`close_place(`）の後に記録の段を \
                             `OffloadLanding::Settled(` + `OffloadJob::CloseRecord(` で background へ出していない（#2657）"
                        ));
                    }
                    for w in WRITES {
                        for hit in all_at(arm, w) {
                            out.push(format!(
                                "{DISPATCH}:{} `land_offload` の close の腕（UI スレッド）が記録を書いている（`{w}`）。\
                                 書くのは background の記録の段（`OffloadJob::CloseRecord`。#2657）",
                                line_at(&s.dispatch, start + at + hit)
                            ));
                        }
                    }
                }
            }
        }
    }

    // 4. UI の段は記録を書かない
    for sig in [
        "fn close_place(",
        "fn close_pane_place(",
        "fn background_kill_place(",
    ] {
        match span(&s.dispatch, sig) {
            None => out.push(format!(
                "{DISPATCH}:1 close の UI の段 `{sig}` が無い（走査の空振り。#2657）"
            )),
            Some((start, end)) => {
                let body = &s.dispatch[start..end];
                for w in WRITES {
                    for hit in all_at(body, w) {
                        out.push(format!(
                            "{DISPATCH}:{} close の UI の段 `{sig}` が記録を書いている（`{w}`）。UI の段は記録（`CloseMark`）を\
                             返すだけで、書くのは記録の段（`CloseRecord::finish`。#2657）",
                            line_at(&s.dispatch, start + hit)
                        ));
                    }
                }
            }
        }
    }

    // 5. 記録を直に書くのは close_record の 1 実装だけ
    for (src, rel) in [(&s.dispatch, DISPATCH), (&s.main, MAIN)] {
        for needle in ["mark_closed_by_origin(", "mark_closed_by_pane("] {
            for at in all_at(src, needle) {
                out.push(format!(
                    "{rel}:{} `{needle}` を直に呼んでいる。close の記録は `close_record::CloseMark` を通す\
                     （IPC は background の記録の段・GUI は `CloseMark::spawn`。#2657）",
                    line_at(src, at)
                ));
            }
        }
    }
    match span(&s.record, "pub fn write(&self)") {
        Some((start, end)) if !s.record[start..end].contains("mark_closed_by_origin(") => {
            out.push(format!(
                "{RECORD}:{} `CloseMark::write` が `mark_closed_by_origin(` を呼んでいない（記録の 1 実装。#2657）",
                line_at(&s.record, start)
            ));
        }
        Some(_) => {}
        None => out.push(format!(
            "{RECORD}:1 `CloseMark::write` が無い（走査の空振り。#2657）"
        )),
    }
    match span(&s.record, "pub fn spawn(self)") {
        Some((start, end)) if !s.record[start..end].contains("thread::Builder::new()") => {
            out.push(format!(
                "{RECORD}:{} `CloseMark::spawn` が背景のスレッドを立てていない（GUI の close が UI スレッドで書く。#2657）",
                line_at(&s.record, start)
            ));
        }
        Some(_) => {}
        None => out.push(format!(
            "{RECORD}:1 `CloseMark::spawn` が無い（走査の空振り。#2657）"
        )),
    }

    // 6. GUI の close は背景のスレッドへ投げ、終了の直前に待つ
    match span(&s.main, "fn mark_worker_closed(") {
        None => out.push(format!(
            "{MAIN}:1 `fn mark_worker_closed(` が無い（走査の空振り。#2657）"
        )),
        Some((start, end)) => {
            let body = &s.main[start..end];
            let line = line_at(&s.main, start);
            if !body.contains(".spawn()") {
                out.push(format!(
                    "{MAIN}:{line} GUI の `mark_worker_closed` が記録を背景のスレッド（`CloseMark::spawn`）へ投げていない\
                     （× / ⌘W / タブ close が UI スレッドで workers.yaml を書く。#2657）"
                ));
            }
            if !body.contains("close_record::legacy()") {
                out.push(format!(
                    "{MAIN}:{line} GUI の `mark_worker_closed` に A/B の門（`close_record::legacy()`）が無い（#2657）"
                ));
            }
        }
    }
    match span(&s.main, "fn flush_close_records(") {
        Some((start, end)) if !s.main[start..end].contains("close_record::flush(") => {
            out.push(format!(
                "{MAIN}:{} `flush_close_records` が `close_record::flush(` で待っていない（#2657）",
                line_at(&s.main, start)
            ));
        }
        Some(_) => {}
        None => out.push(format!(
            "{MAIN}:1 `fn flush_close_records(` が無い（終了の直前に close の記録の書き終わりを待つ口。#2657）"
        )),
    }
    match span(&s.main, ".on_app_quit(") {
        Some((start, end)) if !s.main[start..end].contains("flush_close_records(") => {
            out.push(format!(
                "{MAIN}:{} `on_app_quit` が `flush_close_records(` を呼んでいない（最後に閉じた worker を active で残す。#2657）",
                line_at(&s.main, start)
            ));
        }
        Some(_) => {}
        None => out.push(format!("{MAIN}:1 `on_app_quit` が無い（走査の空振り）")),
    }

    // 7. 「セッション kill」の追記は kill のスレッドの中（UI スレッドは A/B の門の中だけ）
    match span(&s.main, "fn drop_backend_session(&mut self") {
        None => out.push(format!(
            "{MAIN}:1 `fn drop_backend_session(` が無い（走査の空振り。#2657）"
        )),
        Some((start, end)) => {
            let body = &s.main[start..end];
            match body.find("std::thread::spawn(") {
                None => out.push(format!(
                    "{MAIN}:{} `drop_backend_session` に kill のスレッドが無い（走査の空振り。#2657）",
                    line_at(&s.main, start)
                )),
                Some(thread_at) => {
                    let lines: Vec<&str> = s.main.lines().collect();
                    for hit in all_at(&body[..thread_at], "persist_diag(") {
                        let line = line_at(&s.main, start + hit);
                        let gated = lines[line.saturating_sub(4)..line]
                            .iter()
                            .any(|l| l.contains("close_record::legacy()"));
                        if !gated {
                            out.push(format!(
                                "{MAIN}:{line} `drop_backend_session` が kill のスレッドの外（UI スレッド）で persist.log へ\
                                 追記している。「セッション kill」の行は kill のスレッドの先頭で書く（#2657。UI スレッドで書くのは\
                                 A/B の門 `close_record::legacy()` の中だけ）"
                            ));
                        }
                    }
                    if !body[thread_at..].contains("persist_diag(") {
                        out.push(format!(
                            "{MAIN}:{} `drop_backend_session` の kill のスレッドが「セッション kill」の行を書いていない（#2657）",
                            line_at(&s.main, start + thread_at)
                        ));
                    }
                }
            }
        }
    }

    // 8. #641 の「入力」の記録は準備より前の 1 か所
    let prepare = s
        .main
        .find("tako_control::prepare_offload(app, &incoming.request)");
    let inputs = all_at(&s.main, "counts_as_input(");
    match prepare {
        None => out.push(format!(
            "{MAIN}:1 IPC の受信ループの `prepare_offload(app, &incoming.request)` が無い（走査の空振り）"
        )),
        Some(p) => {
            if inputs.is_empty() {
                out.push(format!(
                    "{MAIN}:{} IPC の受信ループが #641 の「入力」（`counts_as_input(`）を数えていない（#2657）",
                    line_at(&s.main, p)
                ));
            }
            for at in inputs {
                if at > p {
                    out.push(format!(
                        "{MAIN}:{} #641 の「入力」（`counts_as_input(`）を準備より後（同期の腕）で数えている。準備 → 着地へ\
                         移った close が数え漏れる（#2657）。準備より前の 1 か所で数える",
                        line_at(&s.main, at)
                    ));
                }
            }
        }
    }

    // 9. GUI が close を dispatch するのは統合 tmux ビューのゴミ箱（`pane_kill_confirmed`）だけで、
    //    それも準備 → 着地 → background（`dispatch_off_ui(`）を通す（セルフテストは除く）
    let self_test = span(&s.main, "mod self_test {");
    let kill = span(&s.main, "fn pane_kill_confirmed(");
    match kill {
        Some((start, end)) if !s.main[start..end].contains("dispatch_off_ui(") => {
            out.push(format!(
                "{MAIN}:{} 統合 tmux ビューのゴミ箱の kill が `dispatch_off_ui(` を通していない（同期の dispatch は close の\
                 記録を UI スレッドで書く。#2657）",
                line_at(&s.main, start)
            ));
        }
        Some(_) => {}
        None => out.push(format!(
            "{MAIN}:1 `fn pane_kill_confirmed(` が無い（走査の空振り。#2657）"
        )),
    }
    for at in all_at(&s.main, "Request::Close {") {
        let inside = |r: Option<(usize, usize)>| r.is_some_and(|(a, b)| (a..b).contains(&at));
        if inside(self_test) || inside(kill) {
            continue;
        }
        out.push(format!(
            "{MAIN}:{} GUI が `Request::Close` を組んでいる。close は IPC（`prepare_offload`）か `dispatch_off_ui(` を通す\
             （同期の dispatch は worker レジストリへの記録を UI スレッドで書く。#2657）",
            line_at(&s.main, at)
        ));
    }
    out
}

#[test]
fn issue2657_closeの記録をメインスレッドで書かない() {
    let found = scan(&sources());
    assert!(
        found.is_empty(),
        "#2657 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn issue2657_走査が空振りしていない() {
    let s = sources();
    // 記録の 1 実装が実際に記録を呼んでいる（0 件なら 5 の走査が無意味に緑になる）
    assert_eq!(
        all_at(&s.record, "mark_closed_by_origin(").len(),
        1,
        "{RECORD} の `mark_closed_by_origin(` が 1 件でない（走査の前提が変わった）"
    );
    assert!(
        s.main.contains("\"セッション kill: pane="),
        "{MAIN} の「セッション kill」の行が見えない"
    );
    assert_eq!(
        all_at(&s.main, "counts_as_input(").len(),
        1,
        "{MAIN} の `counts_as_input(` が 1 件でない（走査の前提が変わった）"
    );
}

/// 注入を当てる（`from` が無ければ注入そのものが空振り = 落とす）
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    assert!(base.contains(from), "注入「{what}」の当て先が無い: {from}");
    base.replacen(from, to, 1)
}

/// 注入した源の行番号（`needle` の最初の出現）
fn line_of(src: &str, needle: &str) -> usize {
    line_at(src, src.find(needle).expect("注入した箇所がある"))
}

fn assert_named(found: &[String], want: &str, what: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(want)),
        "注入「{what}」を名指しできていない（期待 {want}）: {found:#?}"
    );
}

#[test]
fn issue2657_注入_着地で記録を書くと名指す() {
    let s = sources();
    let what = "着地（UI スレッド）で記録まで書いて応答する";
    let dispatch = inject(
        &s.dispatch,
        "OffloadLanding::Settled(Box::new(OffloadJob::CloseRecord(Box::new(record))))",
        "OffloadLanding::Done(Ok(record.finish()))",
        what,
    );
    let line = line_of(&dispatch, "OffloadLanding::Done(Ok(record.finish()))");
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), what);
}

#[test]
fn issue2657_注入_準備の腕を外すと名指す() {
    let s = sources();
    let what = "close の準備の腕を外す（同期の腕 = UI スレッドで記録まで書く）";
    let base = &s.dispatch;
    let (start, _) = span(base, "pub fn prepare_offload(").unwrap();
    let dispatch = inject(
        base,
        "Request::Close { .. } | Request::BackgroundKill { .. }\n            if !crate::close_record::legacy() =>",
        "Request::RemoteAgents if false =>",
        what,
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{DISPATCH}:{}", line_at(base, start)),
        what,
    );
}

#[test]
fn issue2657_注入_準備のab門を外すと名指す() {
    let s = sources();
    let what = "close の準備の A/B の門を外す";
    let dispatch = inject(
        &s.dispatch,
        "            if !crate::close_record::legacy() =>\n        {\n            Some(Ok(OffloadJob::ClosePlace(",
        "            if true =>\n        {\n            Some(Ok(OffloadJob::ClosePlace(",
        what,
    );
    let line = line_of(
        &dispatch,
        "Request::Close { .. } | Request::BackgroundKill { .. }\n            if true",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), what);
}

#[test]
fn issue2657_注入_uiの段で記録を書くと名指す() {
    let s = sources();
    let what = "close の UI の段で workers.yaml を書く";
    let dispatch = inject(
        &s.dispatch,
        "    crate::handoff_guard::forget(target.as_u64());\n",
        "    crate::handoff_guard::forget(target.as_u64());\n    let _ = crate::orchestrator::registry::mark_closed_by_origin(target.as_u64(), close_origin_of(origin), None);\n",
        what,
    );
    let line = line_of(
        &dispatch,
        "let _ = crate::orchestrator::registry::mark_closed_by_origin(",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), what);
}

#[test]
fn issue2657_注入_記録の段が書かずに応答すると名指す() {
    let s = sources();
    let what = "記録の段が書かずに応答する";
    let dispatch = inject(
        &s.dispatch,
        "OffloadJob::CloseRecord(record) => OffloadOutcome::Reply(Ok(record.finish())),",
        "OffloadJob::CloseRecord(record) => OffloadOutcome::Reply(Ok(record.reply)),",
        what,
    );
    let line = line_of(
        &dispatch,
        "OffloadJob::CloseRecord(record) => OffloadOutcome::Reply(Ok(record.reply))",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), what);
}

#[test]
fn issue2657_注入_guiのcloseをuiスレッドで書くと名指す() {
    let s = sources();
    let what = "GUI の × / ⌘W / タブ close が UI スレッドで書く";
    let main = inject(
        &s.main,
        "            mark.spawn();\n",
        "            mark.write();\n",
        what,
    );
    let line = line_of(&main, "fn mark_worker_closed(");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);

    let what = "GUI が記録を直に呼ぶ";
    let main = inject(
        &s.main,
        "        let mark =\n            tako_control::close_record::CloseMark::new(",
        "        let _ = tako_control::orchestrator::registry::mark_closed_by_origin(pane_id.as_u64(), reason.origin(), None);\n        let mark =\n            tako_control::close_record::CloseMark::new(",
        what,
    );
    let line = line_of(
        &main,
        "let _ = tako_control::orchestrator::registry::mark_closed_by_origin(",
    );
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);
}

#[test]
fn issue2657_注入_終了で待たないと名指す() {
    let s = sources();
    let what = "終了の直前に close の記録を待たない";
    let main = inject(
        &s.main,
        "            this.flush_close_records(\"on_app_quit\");\n",
        "",
        what,
    );
    let line = line_of(&main, ".on_app_quit(");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);
}

#[test]
fn issue2657_注入_セッションkillの行をuiスレッドへ戻すと名指す() {
    let s = sources();
    let what = "「セッション kill」の行を UI スレッドで書く";
    let main = inject(
        &s.main,
        "            let socket = tako_core::tmux_backend::socket_name();\n            let logs = self.pane_logs.clone();\n",
        "            if let Some(line) = &kill_line {\n                persist_diag(line);\n            }\n            let socket = tako_core::tmux_backend::socket_name();\n            let logs = self.pane_logs.clone();\n",
        what,
    );
    let line = line_of(&main, "                persist_diag(line);\n");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);
}

#[test]
fn issue2657_注入_入力の記録を同期の腕へ戻すと名指す() {
    let s = sources();
    let what = "#641 の「入力」の記録を準備より後（同期の腕）へ戻す";
    let main = inject(
        &s.main,
        "                    let mut result = tako_control::dispatch(app, incoming.request, incoming.origin);\n",
        "                    if tako_control::restore_guard::counts_as_input(&incoming.request) {\n                        app.restore_guard.note_input();\n                    }\n                    let mut result = tako_control::dispatch(app, incoming.request, incoming.origin);\n",
        what,
    );
    let line = line_at(
        &main,
        main.rfind("tako_control::restore_guard::counts_as_input(&incoming.request)")
            .unwrap(),
    );
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);
}

#[test]
fn issue2657_注入_ゴミ箱のkillを同期のdispatchへ戻すと名指す() {
    let s = sources();
    let what = "統合 tmux ビューのゴミ箱の kill を同期の dispatch へ戻す";
    let main = inject(
        &s.main,
        "        self.dispatch_off_ui(\n            tako_control::protocol::Request::Close {",
        "        let _ = tako_control::dispatch(\n            self,\n            tako_control::protocol::Request::Close {",
        what,
    );
    let line = line_of(&main, "fn pane_kill_confirmed(");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);

    let what = "GUI のほかの口で Close を組む";
    let main = inject(
        &s.main,
        "    fn flush_close_records(&self, why: &str) {\n",
        "    fn flush_close_records(&self, why: &str) {\n        let _ = tako_control::protocol::Request::Close { pane: None, force: true, caller_role: None, caller_pane: None };\n",
        what,
    );
    let line = line_of(&main, "let _ = tako_control::protocol::Request::Close {");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), what);
}
