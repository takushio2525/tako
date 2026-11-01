//! **#2630 の番犬**: `RemoteStop`（daemon の停止）と `RemoteStatus`（daemon の状態）を GUI のメインスレッドで待たない。
//!
//! ## なぜ縛るのか
//!
//! `remote::daemon_stop` は daemon が畳み終わる（tailscale の serve の解除を含む）のを最長数秒待ち、止まった後に
//! 状態（`daemon_status`）を読み直す。`remote::daemon_status` は daemon の稼働中、2 系統の同居の調査で tailscale の
//! CLI を最大 3 本起こす（1 本の上限 10 秒）。どちらも UI スレッドの同期の dispatch で待つと、その間 GUI が止まり
//! 別の IPC も待たされる（本番の perf.log の `dispatch:RemoteStop` 247〜349ms。段 1 では偽の tailscale の遅延注入 3 秒で
//! `dispatch:RemoteStop` 3,173〜3,220ms・`dispatch:RemoteStatus` 6,106〜6,119ms・設定画面のリモートタブの読み直しも
//! `dispatch:RemoteStatus` 6,082〜6,106ms で、その間の `tako list` が 2.7〜5.6 秒待たされた）。
//! いまは IPC の受け口・設定画面の「停止」・リモートタブの読み直しが `prepare_offload` →
//! `OffloadJob::RemoteStop` / `OffloadJob::RemoteStatus` で background へ出す（#1555 の `RemoteStart` と同じ形）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `prepare_offload` に `Request::RemoteStop` / `Request::RemoteStatus` の腕があり、host の仕事
//!    （`remote_stop_job()` / `remote_status_job()`）を `OffloadJob::RemoteStop {` / `OffloadJob::RemoteStatus(` へ包む。
//!    A/B（`stop_status_offload_legacy()`）の門を持つ
//! 2. その仕事は background から応答する: `run_reply` が両方を答え（`force` の停止は同期の腕と同じく
//!    `daemon_force_stop()`）、`run_staged` は UI スレッドへの続き（`OffloadOutcome::OnUi`）を作らない
//! 3. GUI の host（`impl RemoteHost for TakoApp`）が `remote_stop_job` / `remote_status_job` で仕事を渡す
//!    （渡さないと準備が `None` を返し、同期の腕 = UI スレッドで待つ）
//! 4. tako-app の中で `Request::RemoteStop` を組むのは設定画面の `fn remote_stop_off_ui(` だけ、
//!    `Request::RemoteStatus` を組むのは `fn refresh_remote_status(` だけで、どちらも `prepare_offload(` を通す
//!    （`run_with_message(Request::RemoteStop` / `self.query(Request::RemoteStatus` の同期の dispatch へ戻さない）
//! 5. tako-app の中で `remote::daemon_stop` / `remote::daemon_force_stop` を呼ぶのは `fn remote_stop_from_dispatch(`
//!    （同期の腕と仕事の 1 実装）か `background_executor()` の中（remote パネルのボタン）だけ。
//!    `remote::daemon_status` は GUI の host（同期の腕と仕事）・`background_executor()` の中・セルフテストだけ

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

/// `sig` で始まる関数（か impl・mod）の範囲。見つからなければ `None`
fn span(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// `body` の中の `needle`（`Request::…`）で始まる腕。腕は次の `Request::` まで
fn arm_of<'a>(body: &'a str, needle: &str) -> Option<(usize, &'a str)> {
    let at = body.find(needle)?;
    let rest = &body[at + 1..];
    let stop = rest.find("Request::").map_or(rest.len(), |i| i + 1);
    Some((at, &body[at..at + stop]))
}

/// `body` の中の `needle`（`OffloadJob::…`）で始まる腕。腕は次の `OffloadJob::` まで
fn job_arm_of<'a>(body: &'a str, needle: &str) -> Option<(usize, &'a str)> {
    let at = body.find(needle)?;
    let rest = &body[at + 1..];
    let stop = rest.find("OffloadJob::").map_or(rest.len(), |i| i + 1);
    Some((at, &body[at..at + stop]))
}

/// 位置 `at` が `background_executor()` の中か（呼び出しの前 9 行に `.background_executor()` がある）
fn in_background(src: &str, at: usize) -> bool {
    let lines: Vec<&str> = src.lines().collect();
    let line = line_at(src, at);
    lines[line.saturating_sub(9)..line]
        .iter()
        .any(|l| l.contains(".background_executor()"))
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
            for (needle, getter, job, what) in [
                (
                    "Request::RemoteStop {",
                    "remote_stop_job()",
                    "OffloadJob::RemoteStop {",
                    "daemon の停止（畳み終わりの待ち + 止まった後の状態の読み直し）",
                ),
                (
                    "Request::RemoteStatus",
                    "remote_status_job()",
                    "OffloadJob::RemoteStatus(",
                    "daemon の状態（tailscale の CLI を最大 3 本）",
                ),
            ] {
                match arm_of(body, needle) {
                    None => out.push(format!(
                        "{DISPATCH}:{} `prepare_offload` に `{needle}` の腕が無い。{what}を UI スレッドで待つ（#2630）。\
                         `host.{getter}` を `{job}` へ",
                        line_at(&s.dispatch, start)
                    )),
                    Some((at, arm)) => {
                        let line = line_at(&s.dispatch, start + at);
                        if !arm.contains(getter) || !arm.contains(job) {
                            out.push(format!(
                                "{DISPATCH}:{line} `{needle}` の腕が host の仕事（`{getter}`）を `{job}` へ包んでいない（#2630）"
                            ));
                        }
                        if !arm.contains("stop_status_offload_legacy()") {
                            out.push(format!(
                                "{DISPATCH}:{line} `{needle}` の腕に A/B の門（`stop_status_offload_legacy()`）が無い（#2630）"
                            ));
                        }
                    }
                }
            }
        }
    }

    // 2. background から応答する（UI スレッドへ戻さない）
    match span(&s.dispatch, "fn run_reply(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_reply` が無い（走査の空振り）")),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            for needle in ["OffloadJob::RemoteStop {", "OffloadJob::RemoteStatus("] {
                if !body.contains(needle) {
                    out.push(format!(
                        "{DISPATCH}:{} `run_reply` が `{needle}` を答えていない（background から応答する。#2630）",
                        line_at(&s.dispatch, start)
                    ));
                }
            }
            if let Some((at, arm)) = job_arm_of(body, "OffloadJob::RemoteStop {") {
                if !arm.contains("daemon_force_stop()") {
                    out.push(format!(
                        "{DISPATCH}:{} `run_reply` の `OffloadJob::RemoteStop` が `force` を同期の腕と同じ \
                         `daemon_force_stop()` で止めていない（応答の中身が変わる。#2630）",
                        line_at(&s.dispatch, start + at)
                    ));
                }
            }
        }
    }
    match span(&s.dispatch, "pub fn run_staged(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_staged` が無い（走査の空振り）")),
        Some((start, end)) => {
            for needle in ["OffloadJob::RemoteStop", "OffloadJob::RemoteStatus"] {
                for at in all_at(&s.dispatch[start..end], needle) {
                    out.push(format!(
                        "{DISPATCH}:{} `run_staged` が `{needle}` を UI スレッドへの続きへ回している\
                         （workspace に触らないので `run_reply` から応答する。#2630）",
                        line_at(&s.dispatch, start + at)
                    ));
                }
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
            for sig in ["fn remote_stop_job(", "fn remote_status_job("] {
                match span(body, sig) {
                    None => out.push(format!(
                        "{MAIN}:{} GUI の host が `{sig}` を渡していない（既定の `None` = 準備が同期の腕へ\
                         倒れ、UI スレッドで待つ。#2630）",
                        line_at(&s.main, start)
                    )),
                    Some((f_start, f_end)) => {
                        let f = &body[f_start..f_end];
                        if !f.contains("Some(") || f.contains("None") {
                            out.push(format!(
                                "{MAIN}:{} GUI の `{sig}` が仕事を渡していない（`Some(Box::new(…))` で返す。#2630）",
                                line_at(&s.main, start + f_start)
                            ));
                        }
                    }
                }
            }
        }
    }

    // 4. tako-app の中で RemoteStop / RemoteStatus を組むのは設定画面の off-UI の口だけ
    for (needle, sig) in [
        ("Request::RemoteStop", "fn remote_stop_off_ui("),
        ("Request::RemoteStatus", "fn refresh_remote_status("),
    ] {
        let off_ui = span(&s.settings, sig);
        match off_ui {
            Some((start, end)) => {
                if !s.settings[start..end].contains("prepare_offload(") {
                    out.push(format!(
                        "{SETTINGS}:{} `{sig}` が `prepare_offload(` を通していない（UI スレッドで同期に dispatch する。#2630）",
                        line_at(&s.settings, start)
                    ));
                }
            }
            None => out.push(format!(
                "{SETTINGS}:1 設定画面の `{sig}` が無い（`{needle}` を background へ出す口。#2630）"
            )),
        }
        for (src, rel, allowed) in [
            (&s.settings, SETTINGS, off_ui),
            (&s.main, MAIN, None),
            (&s.panel, PANEL, None),
        ] {
            for at in all_at(src, needle) {
                if allowed.is_some_and(|(a, b)| (a..b).contains(&at)) {
                    continue;
                }
                out.push(format!(
                    "{rel}:{} `{needle}` を `{sig}` の外で組んでいる。同期の dispatch は UI スレッドで待つ（#2630）。\
                     設定画面は `{sig}`、他は IPC（`prepare_offload`）を通す",
                    line_at(src, at)
                ));
            }
        }
    }

    // 5. daemon の停止・状態を呼ぶのは 1 実装か background の中だけ
    let stop_helper = span(&s.main, "fn remote_stop_from_dispatch(");
    if stop_helper.is_none() {
        out.push(format!(
            "{MAIN}:1 `fn remote_stop_from_dispatch(` が無い（同期の腕と仕事の 1 実装。#2630）"
        ));
    }
    let host_impl = span(&s.main, "impl RemoteHost for TakoApp {");
    let self_test = span(&s.main, "mod self_test {");
    for (needle, main_allowed) in [
        ("remote::daemon_stop", vec![stop_helper]),
        ("remote::daemon_force_stop", vec![stop_helper]),
        ("remote::daemon_status", vec![host_impl, self_test]),
    ] {
        for (src, rel, allowed) in [
            (&s.main, MAIN, main_allowed.clone()),
            (&s.settings, SETTINGS, Vec::new()),
            (&s.panel, PANEL, Vec::new()),
        ] {
            for at in all_at(src, needle) {
                if allowed
                    .iter()
                    .flatten()
                    .any(|(a, b)| (*a..*b).contains(&at))
                    || in_background(src, at)
                {
                    continue;
                }
                out.push(format!(
                    "{rel}:{} `{needle}` を background の外（UI スレッド）で呼んでいる。daemon の停止・状態は\
                     tailscale の CLI と daemon の終わりを待つ（#2630）。`background_executor()` の中で呼ぶか、\
                     dispatch（`prepare_offload`）を通す",
                    line_at(src, at)
                ));
            }
        }
    }
    out
}

#[test]
fn issue2630_remote_stopとstatusはメインスレッドで待たない() {
    let found = scan(&sources());
    assert!(
        found.is_empty(),
        "#2630 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn issue2630_走査が空振りしていない() {
    let s = sources();
    // 呼び手の走査が実際に呼び出しを見ている（0 件なら全部が無意味に緑になる）
    assert!(
        !all_at(&s.main, "remote::daemon_stop").is_empty(),
        "{MAIN} の daemon_stop が見えない"
    );
    assert!(
        all_at(&s.main, "remote::daemon_status").len() >= 2,
        "{MAIN} の daemon_status が 2 件見えない（同期の腕・仕事）"
    );
    assert!(
        !all_at(&s.panel, "remote::daemon_stop").is_empty(),
        "{PANEL} の daemon_stop が見えない（停止ボタン）"
    );
    for needle in ["Request::RemoteStop", "Request::RemoteStatus"] {
        assert_eq!(
            all_at(&s.settings, needle).len(),
            1,
            "{SETTINGS} の `{needle}` が 1 件でない（走査の前提が変わった）"
        );
    }
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
fn issue2630_注入_停止の準備の腕を外すと名指す() {
    let s = sources();
    let base = &s.dispatch;
    let (start, _) = span(base, "pub fn prepare_offload(").unwrap();
    let dispatch = inject(
        base,
        "Request::RemoteStop { force } if !crate::remote::stop_status_offload_legacy() =>",
        "Request::RemoteAgents if false =>",
        "停止の準備の腕を外す",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{DISPATCH}:{}", line_at(base, start)),
        "停止の準備の腕を外す",
    );
}

#[test]
fn issue2630_注入_状態のab門を外すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "Request::RemoteStatus if !crate::remote::stop_status_offload_legacy() =>",
        "Request::RemoteStatus =>",
        "状態の A/B の門を外す",
    );
    let line = line_of(&dispatch, "Request::RemoteStatus => host");
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{DISPATCH}:{line}"),
        "状態の A/B の門を外す",
    );
}

#[test]
fn issue2630_注入_続きをuiスレッドへ戻すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "            other => OffloadOutcome::Reply(other.run_reply()),",
        "            OffloadJob::RemoteStatus(job) => OffloadOutcome::Reply(Ok(job())),\n            other => OffloadOutcome::Reply(other.run_reply()),",
        "run_staged で答える",
    );
    let line = line_of(
        &dispatch,
        "OffloadJob::RemoteStatus(job) => OffloadOutcome::Reply",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), "run_staged で答える");
}

#[test]
fn issue2630_注入_forceをhostの停止へ回すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "crate::remote::daemon_force_stop()\n            } else {",
        "job()\n            } else {",
        "force を host の停止へ回す",
    );
    let line = line_of(&dispatch, "OffloadJob::RemoteStop { force, job } =>");
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{DISPATCH}:{line}"),
        "force を host の停止へ回す",
    );
}

#[test]
fn issue2630_注入_guiが仕事を渡さないと名指す() {
    let s = sources();
    let main = inject(
        &s.main,
        "Some(Box::new(remote_stop_from_dispatch))",
        "None",
        "停止の仕事を渡さない",
    );
    let line = line_of(&main, "fn remote_stop_job(");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), "停止の仕事を渡さない");

    let main = inject(
        &s.main,
        "Some(Box::new(tako_control::remote::daemon_status))",
        "None",
        "状態の仕事を渡さない",
    );
    let line = line_of(&main, "fn remote_status_job(");
    let found = scan(&Sources { main, ..sources() });
    assert_named(&found, &format!("{MAIN}:{line}"), "状態の仕事を渡さない");
}

#[test]
fn issue2630_注入_設定画面の停止を同期のdispatchへ戻すと名指す() {
    let s = sources();
    let settings = inject(
        &s.settings,
        "cx.listener(|this, _, _, cx| this.remote_stop_off_ui(cx)),",
        "cx.listener(|this, _, _, cx| { this.run_with_message(Request::RemoteStop { force: false }, txt::msg_remote_stopped().to_string(), cx); }),",
        "設定画面の停止の同期の dispatch",
    );
    let line = line_of(&settings, "this.run_with_message(Request::RemoteStop {");
    let found = scan(&Sources {
        settings,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{SETTINGS}:{line}"),
        "設定画面の停止の同期の dispatch",
    );
}

#[test]
fn issue2630_注入_設定画面の読み直しを同期の照会へ戻すと名指す() {
    let s = sources();
    let settings = inject(
        &s.settings,
        "                self.refresh_remote_status(cx);\n",
        "                self.status.remote = self.query(Request::RemoteStatus, cx);\n",
        "設定画面の読み直しの同期の照会",
    );
    let line = line_of(&settings, "self.query(Request::RemoteStatus, cx)");
    let found = scan(&Sources {
        settings,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{SETTINGS}:{line}"),
        "設定画面の読み直しの同期の照会",
    );
}

#[test]
fn issue2630_注入_設定画面でdaemonの状態を直に読むと名指す() {
    let s = sources();
    let settings = inject(
        &s.settings,
        "                self.refresh_remote_status(cx);\n",
        "                self.status.remote = Some(tako_control::remote::daemon_status());\n",
        "設定画面で daemon の状態を直に読む",
    );
    let line = line_of(&settings, "tako_control::remote::daemon_status()");
    let found = scan(&Sources {
        settings,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{SETTINGS}:{line}"),
        "設定画面で daemon の状態を直に読む",
    );
}
