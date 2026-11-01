//! **#1555 の番犬**: `RemoteStart`（daemon の起動）を GUI のメインスレッドで待たない。
//!
//! ## なぜ縛るのか
//!
//! `remote::spawn_daemon` は子（`tako remote serve`）の起動情報を最長 30 秒待つ。しかも子は起動の途中で
//! GUI の IPC へ probe（`AppConnection::new` → `List`）を送る。GUI のメインスレッドで待つと、GUI は子の
//! 起動情報を・子は GUI の答えを待ち合い、子の読み取りの上限（10 秒）が切れるまで GUI が丸ごと止まる
//! （本番の perf.log の `dispatch:RemoteStart` 10,499〜10,690ms。MCP `tako_remote_start` の IPC）。
//! いまは IPC の受け口と設定画面の「開始」が `prepare_offload` → `OffloadJob::RemoteStart` で background へ
//! 出し、GUI の自動復帰と remote パネルのボタンは `background_executor()` の中で起こす。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `prepare_offload` に `Request::RemoteStart` の腕があり、host の仕事（`remote_start_job()`）を
//!    `OffloadJob::RemoteStart(` へ包む。A/B（`start_offload_legacy()`）の門を持つ
//! 2. その仕事は background から応答する: `run_reply` が `OffloadJob::RemoteStart(` を答え、`run_staged` は
//!    UI スレッドへの続き（`OffloadOutcome::OnUi`）を作らない
//! 3. GUI の host（`impl RemoteHost for TakoApp`）が `remote_start_job` で仕事を渡す（渡さないと準備が
//!    `None` を返し、同期の腕 = UI スレッドで待つ）
//! 4. tako-app の中で `Request::RemoteStart` を組むのは設定画面の `fn remote_start_off_ui(` だけで、そこは
//!    `prepare_offload(` を通す（`run_with_message(Request::RemoteStart` の同期の dispatch へ戻さない）
//! 5. tako-app の中で `remote::spawn_daemon` を呼ぶのは `fn remote_start_from_dispatch(`（同期の腕と仕事の
//!    1 実装）か、`background_executor()` の中だけ（自動復帰・remote パネルのボタン）

use std::path::{Path, PathBuf};

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const SETTINGS: &str = "crates/tako-app/src/settings_window.rs";
const PANEL: &str = "crates/tako-app/src/remote_panel.rs";

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
    settings: String,
    panel: String,
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
        settings: read(SETTINGS),
        panel: read(PANEL),
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

fn scan(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();

    // 1. prepare_offload の腕
    match span(&s.dispatch, "pub fn prepare_offload(") {
        None => out.push(format!(
            "{DISPATCH}:1 `prepare_offload` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            let arm = body.find("Request::RemoteStart {}").map(|at| {
                // 腕は次の `Request::` まで
                let rest = &body[at + 1..];
                let stop = rest.find("Request::").map_or(rest.len(), |i| i + 1);
                (at, &body[at..at + stop])
            });
            match arm {
                None => out.push(format!(
                    "{DISPATCH}:{} `prepare_offload` に `Request::RemoteStart` の腕が無い。daemon の起動を UI スレッドで\
                     待つ（子の probe と 10 秒待ち合う = #1555）。`host.remote_start_job()` を `OffloadJob::RemoteStart(` へ",
                    line_at(&s.dispatch, start)
                )),
                Some((at, arm)) => {
                    let line = line_at(&s.dispatch, start + at);
                    if !arm.contains("remote_start_job()") || !arm.contains("OffloadJob::RemoteStart(")
                    {
                        out.push(format!(
                            "{DISPATCH}:{line} `Request::RemoteStart` の腕が host の仕事（`remote_start_job()`）を \
                             `OffloadJob::RemoteStart(` へ包んでいない（#1555）"
                        ));
                    }
                    if !arm.contains("start_offload_legacy()") {
                        out.push(format!(
                            "{DISPATCH}:{line} `Request::RemoteStart` の腕に A/B の門（`start_offload_legacy()`）が無い（#1555）"
                        ));
                    }
                }
            }
        }
    }

    // 2. background から応答する（UI スレッドへ戻さない）
    match span(&s.dispatch, "fn run_reply(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_reply` が無い（走査の空振り）")),
        Some((start, end)) => {
            if !s.dispatch[start..end].contains("OffloadJob::RemoteStart(") {
                out.push(format!(
                    "{DISPATCH}:{} `run_reply` が `OffloadJob::RemoteStart(` を答えていない（background から応答する。#1555）",
                    line_at(&s.dispatch, start)
                ));
            }
        }
    }
    match span(&s.dispatch, "pub fn run_staged(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_staged` が無い（走査の空振り）")),
        Some((start, end)) => {
            for at in all_at(&s.dispatch[start..end], "OffloadJob::RemoteStart") {
                out.push(format!(
                    "{DISPATCH}:{} `run_staged` が `OffloadJob::RemoteStart` を UI スレッドへの続きへ回している\
                     （workspace に触らないので `run_reply` から応答する。#1555）",
                    line_at(&s.dispatch, start + at)
                ));
            }
        }
    }

    // 3. GUI の host が仕事を渡す
    match span(&s.main, "impl RemoteHost for TakoApp {") {
        None => out.push(format!(
            "{MAIN}:1 `impl RemoteHost for TakoApp` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &s.main[start..end];
            match span(body, "fn remote_start_job(") {
                None => out.push(format!(
                    "{MAIN}:{} GUI の host が `remote_start_job` を渡していない（既定の `None` = 準備が同期の腕へ\
                     倒れ、UI スレッドで daemon を待つ。#1555）",
                    line_at(&s.main, start)
                )),
                Some((f_start, f_end)) => {
                    let f = &body[f_start..f_end];
                    if !f.contains("Some(") || f.contains("None") {
                        out.push(format!(
                            "{MAIN}:{} GUI の `remote_start_job` が仕事を渡していない（`Some(Box::new(…))` で返す。#1555）",
                            line_at(&s.main, start + f_start)
                        ));
                    }
                }
            }
        }
    }

    // 4. tako-app の中で RemoteStart を組むのは設定画面の off-UI の口だけ
    let off_ui = span(&s.settings, "fn remote_start_off_ui(");
    if let Some((start, end)) = off_ui {
        if !s.settings[start..end].contains("prepare_offload(") {
            out.push(format!(
                "{SETTINGS}:{} `remote_start_off_ui` が `prepare_offload(` を通していない（UI スレッドで同期に dispatch する。#1555）",
                line_at(&s.settings, start)
            ));
        }
    } else {
        out.push(format!(
            "{SETTINGS}:1 設定画面の `fn remote_start_off_ui(` が無い（「開始」を background へ出す口。#1555）"
        ));
    }
    for (src, rel, allowed) in [
        (&s.settings, SETTINGS, off_ui),
        (&s.main, MAIN, None),
        (&s.panel, PANEL, None),
    ] {
        for at in all_at(src, "Request::RemoteStart") {
            if allowed.is_some_and(|(a, b)| (a..b).contains(&at)) {
                continue;
            }
            out.push(format!(
                "{rel}:{} `Request::RemoteStart` を `remote_start_off_ui` の外で組んでいる。同期の dispatch は UI スレッドで\
                 daemon を待つ（#1555）。設定画面は `remote_start_off_ui`、他は IPC（`prepare_offload`）を通す",
                line_at(src, at)
            ));
        }
    }

    // 5. spawn_daemon を呼ぶのは 1 実装か background の中だけ
    let helper = span(&s.main, "fn remote_start_from_dispatch(");
    if helper.is_none() {
        out.push(format!(
            "{MAIN}:1 `fn remote_start_from_dispatch(` が無い（同期の腕と仕事の 1 実装。#1555）"
        ));
    }
    for (src, rel, allowed) in [
        (&s.main, MAIN, helper),
        (&s.settings, SETTINGS, None),
        (&s.panel, PANEL, None),
    ] {
        let lines: Vec<&str> = src.lines().collect();
        for at in all_at(src, "remote::spawn_daemon") {
            if allowed.is_some_and(|(a, b)| (a..b).contains(&at)) {
                continue;
            }
            let line = line_at(src, at);
            let window = &lines[line.saturating_sub(9)..line];
            if window.iter().any(|l| l.contains(".background_executor()")) {
                continue;
            }
            out.push(format!(
                "{rel}:{line} `remote::spawn_daemon` を background の外（UI スレッド）で呼んでいる。daemon の起動を\
                 UI スレッドで待つと子の probe と 10 秒待ち合う（#1555）。`background_executor()` の中で起こす"
            ));
        }
    }
    out
}

#[test]
fn issue1555_remote_startはメインスレッドで待たない() {
    let found = scan(&sources());
    assert!(
        found.is_empty(),
        "#1555 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn issue1555_走査が空振りしていない() {
    let s = sources();
    // 呼び手の走査が実際に呼び出しを見ている（0 件なら全部が無意味に緑になる）
    let spawns = |src: &str| all_at(src, "remote::spawn_daemon").len();
    assert!(spawns(&s.main) >= 1, "{MAIN} の spawn_daemon が見えない");
    assert!(
        spawns(&s.panel) >= 2,
        "{PANEL} の spawn_daemon が 2 件見えない（自動復帰・ボタン）"
    );
    assert_eq!(
        all_at(&s.settings, "Request::RemoteStart").len(),
        1,
        "{SETTINGS} の `Request::RemoteStart` が 1 件でない（走査の前提が変わった）"
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
fn issue1555_注入_準備の腕を外すと名指す() {
    let s = sources();
    let base = &s.dispatch;
    let (start, _) = span(base, "pub fn prepare_offload(").unwrap();
    let dispatch = inject(
        base,
        "Request::RemoteStart {} if !crate::remote::start_offload_legacy() =>",
        "Request::RemoteAgents if false =>",
        "準備の腕を外す",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{DISPATCH}:{}", line_at(base, start)),
        "準備の腕を外す",
    );
}

#[test]
fn issue1555_注入_ab門を外すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "Request::RemoteStart {} if !crate::remote::start_offload_legacy() =>",
        "Request::RemoteStart {} =>",
        "A/B の門を外す",
    );
    let line = line_of(&dispatch, "Request::RemoteStart {} => host");
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), "A/B の門を外す");
}

#[test]
fn issue1555_注入_続きをuiスレッドへ戻すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "            other => OffloadOutcome::Reply(other.run_reply()),",
        "            OffloadJob::RemoteStart(job) => OffloadOutcome::Reply(job().map_err(DispatchError::Operation)),\n            other => OffloadOutcome::Reply(other.run_reply()),",
        "run_staged で答える",
    );
    let line = line_of(
        &dispatch,
        "OffloadJob::RemoteStart(job) => OffloadOutcome::Reply",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), "run_staged で答える");
}

#[test]
fn issue1555_注入_guiが仕事を渡さないと名指す() {
    let s = sources();
    let main = inject(
        &s.main,
        "Some(Box::new(remote_start_from_dispatch))",
        "None",
        "仕事を渡さない",
    );
    let line = line_of(&main, "fn remote_start_job(");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), "仕事を渡さない");
}

#[test]
fn issue1555_注入_設定画面を同期のdispatchへ戻すと名指す() {
    let s = sources();
    let settings = inject(
        &s.settings,
        "cx.listener(|this, _, _, cx| this.remote_start_off_ui(cx)),",
        "cx.listener(|this, _, _, cx| { this.run_with_message(Request::RemoteStart {}, txt::msg_remote_started().to_string(), cx); }),",
        "設定画面の同期の dispatch",
    );
    let line = line_of(&settings, "this.run_with_message(Request::RemoteStart {}");
    let found = scan(&Sources {
        settings,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{SETTINGS}:{line}"),
        "設定画面の同期の dispatch",
    );
}

#[test]
fn issue1555_注入_自動復帰をuiスレッドで起こすと名指す() {
    let s = sources();
    let panel = inject(
        &s.panel,
        "        self.remote.autostart_ran = true;\n",
        "        self.remote.autostart_ran = true;\n        let _ = tako_control::remote::with_origin(tako_control::remote::RequestOrigin::Autostart, tako_control::remote::spawn_daemon);\n",
        "自動復帰を UI スレッドで起こす",
    );
    let line = line_of(
        &panel,
        "RequestOrigin::Autostart, tako_control::remote::spawn_daemon);",
    );
    let found = scan(&Sources { panel, ..sources() });
    assert_named(
        &found,
        &format!("{PANEL}:{line}"),
        "自動復帰を UI スレッドで起こす",
    );
}
