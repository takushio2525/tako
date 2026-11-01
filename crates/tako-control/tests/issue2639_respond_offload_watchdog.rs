//! **#2639 の番犬**: `OrchestratorRespond`（選択肢ダイアログへの応答）を GUI のメインスレッドで待たない。
//!
//! ## なぜ縛るのか
//!
//! 応答の手順（`respond_via`）は番号キーの後に 400ms・Enter の後に 400ms の固定の待ち（`std::thread::sleep`）を
//! 挟んで画面を読み直す。UI スレッドの同期の dispatch で回すとその間 GUI が止まり、別の IPC も待たされる
//! （本番の perf.log の `dispatch:OrchestratorRespond` 8 回すべて 401〜414ms。段 1 では隔離 GUI の模擬 TUI で
//! 403〜408ms / Enter の要る形は 804〜805ms・その間の `tako list` が 308〜326 / 696〜766ms 待たされた）。画面のグリッドは PTY の I/O
//! スレッドが更新し、読み書きの手（`PaneAccess`）は `Send` なので、届き方の解決（host が要る）だけを UI スレッドで
//! 行い、手順は background で回せる（#813 の自動復帰が既に同じ形）。いまは IPC の受け口とチャット表示の承認カードが
//! `prepare_offload` → `OffloadJob::OrchestratorRespond` で background へ出す（#1555 / #2630 と同じ形）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `prepare_offload` に `Request::OrchestratorRespond` の腕があり、届き方の解決の 1 実装（`respond_access(`）で
//!    `OffloadJob::OrchestratorRespond(` を作る。A/B（`respond_offload_legacy()`）の門を持つ
//! 2. そのジョブは background から応答する: `run_reply` が `respond_via(` を錠（`RESPOND_SERIAL`）の内側で呼び
//!    （UI スレッドで 1 本ずつ走っていた順序を保つ）、`run_staged` は UI スレッドへの続きを作らない
//! 3. 同期の腕 `dispatch_orchestrator_respond` も届き方を `respond_access(` で解決する（準備と書き分けない）
//! 4. tako-app の中で `Request::OrchestratorRespond` を組むのはチャット表示の `fn chat_respond(` とセルフテストだけで、
//!    `chat_respond` は `prepare_offload(` を通す
//! 5. tako-app の中で応答の手順（`respond_via(` / `respond_to_choice_dialog(`）を直に呼ぶのは background で回す
//!    自動復帰（`limit_autoresume.rs`）だけ

use std::path::{Path, PathBuf};

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CHAT: &str = "crates/tako-app/src/chat_view.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const AUTORESUME: &str = "crates/tako-app/src/limit_autoresume.rs";

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

fn read_code(rel: &str) -> String {
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    code_of(&src, rel)
}

struct Sources {
    dispatch: String,
    chat: String,
    main: String,
    /// tako-app の残りのファイル（相対パス, 本番コード）。chat_view / main を除く
    others: Vec<(String, String)>,
}

fn sources() -> Sources {
    let dir = repo_root().join("crates/tako-app/src");
    let mut others = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("tako-app/src を読める") {
            let path = entry.expect("ディレクトリの項目").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(repo_root())
                .expect("リポジトリの中")
                .to_string_lossy()
                .replace('\\', "/");
            if rel == CHAT || rel == MAIN {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("読める");
            // 残りのファイルはテスト領域ごと見る（範囲取りの下限はテストの多い小さなファイルに当たるので
            // 使わない。テストの中で応答を組む・手順を呼ぶ形も UI スレッドの外とは限らないので名指してよい）
            others.push((rel, production_range::code_view::without_comments(&src)));
        }
    }
    others.sort();
    Sources {
        dispatch: read_code(DISPATCH),
        chat: read_code(CHAT),
        main: read_code(MAIN),
        others,
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

/// `sig` で始まる関数（か mod）の範囲。見つからなければ `None`
fn span(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// `body` の中の `needle` で始まる腕。腕は次の `stop_at` まで
fn arm_of<'a>(body: &'a str, needle: &str, stop_at: &str) -> Option<(usize, &'a str)> {
    let at = body.find(needle)?;
    let rest = &body[at + 1..];
    let stop = rest.find(stop_at).map_or(rest.len(), |i| i + 1);
    Some((at, &body[at..at + stop]))
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
            match arm_of(body, "Request::OrchestratorRespond {", "Request::") {
                None => out.push(format!(
                    "{DISPATCH}:{} `prepare_offload` に `Request::OrchestratorRespond` の腕が無い。応答の固定の待ち\
                     （400ms ずつ）を UI スレッドで払う（#2639）。`respond_access(` で `OffloadJob::OrchestratorRespond(` を作る",
                    line_at(&s.dispatch, start)
                )),
                Some((at, arm)) => {
                    let line = line_at(&s.dispatch, start + at);
                    if !arm.contains("respond_access(")
                        || !arm.contains("OffloadJob::OrchestratorRespond(")
                    {
                        out.push(format!(
                            "{DISPATCH}:{line} `Request::OrchestratorRespond` の腕が届き方の解決（`respond_access(`）で \
                             `OffloadJob::OrchestratorRespond(` を作っていない（#2639）"
                        ));
                    }
                    if !arm.contains("respond_offload_legacy()") {
                        out.push(format!(
                            "{DISPATCH}:{line} `Request::OrchestratorRespond` の腕に A/B の門（`respond_offload_legacy()`）が無い（#2639）"
                        ));
                    }
                }
            }
        }
    }

    // 2. background から応答する（錠の内側で・UI スレッドへ戻さない）
    match span(&s.dispatch, "fn run_reply(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_reply` が無い（走査の空振り）")),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            match arm_of(body, "OffloadJob::OrchestratorRespond(", "OffloadJob::") {
                None => out.push(format!(
                    "{DISPATCH}:{} `run_reply` が `OffloadJob::OrchestratorRespond(` を答えていない（background から応答する。#2639）",
                    line_at(&s.dispatch, start)
                )),
                Some((at, arm)) => {
                    let line = line_at(&s.dispatch, start + at);
                    if !arm.contains("respond_via(") {
                        out.push(format!(
                            "{DISPATCH}:{line} `OffloadJob::OrchestratorRespond` が同期の腕と同じ手順（`respond_via(`）を通っていない（#2639）"
                        ));
                    }
                    if !arm.contains("RESPOND_SERIAL") {
                        out.push(format!(
                            "{DISPATCH}:{line} `OffloadJob::OrchestratorRespond` が錠（`RESPOND_SERIAL`）を取らずに応答している。\
                             UI スレッドで 1 本ずつだった応答が重なり、同じダイアログへの 2 本のキーが混ざる（#2639）"
                        ));
                    }
                }
            }
        }
    }
    match span(&s.dispatch, "pub fn run_staged(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_staged` が無い（走査の空振り）")),
        Some((start, end)) => {
            for at in all_at(&s.dispatch[start..end], "OffloadJob::OrchestratorRespond") {
                out.push(format!(
                    "{DISPATCH}:{} `run_staged` が `OffloadJob::OrchestratorRespond` を UI スレッドへの続きへ回している\
                     （workspace に触らないので `run_reply` から応答する。#2639）",
                    line_at(&s.dispatch, start + at)
                ));
            }
        }
    }

    // 3. 同期の腕も届き方を 1 実装で解決する
    match span(&s.dispatch, "fn dispatch_orchestrator_respond(") {
        None => out.push(format!(
            "{DISPATCH}:1 `dispatch_orchestrator_respond` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &s.dispatch[start..end];
            let line = line_at(&s.dispatch, start);
            if !body.contains("respond_access(") {
                out.push(format!(
                    "{DISPATCH}:{line} 同期の腕が届き方を `respond_access(` で解決していない（準備と書き分けると、\
                     A/B と既定で届き方・断りが変わる。#2639）"
                ));
            }
            for at in all_at(body, "dialog_access(") {
                out.push(format!(
                    "{DISPATCH}:{} 同期の腕が届き方を自前で解決している（`respond_access` の 1 実装へ。#2639）",
                    line_at(&s.dispatch, start + at)
                ));
            }
        }
    }

    // 4. tako-app の中で OrchestratorRespond を組むのは chat_respond とセルフテストだけ
    let chat_fn = span(&s.chat, "fn chat_respond(");
    match chat_fn {
        Some((start, end)) => {
            if !s.chat[start..end].contains("prepare_offload(") {
                out.push(format!(
                    "{CHAT}:{} `chat_respond` が `prepare_offload(` を通していない（承認カードの応答で UI スレッドが \
                     400ms 止まる。#2639）",
                    line_at(&s.chat, start)
                ));
            }
        }
        None => out.push(format!(
            "{CHAT}:1 チャット表示の `fn chat_respond(` が無い（承認カードの応答を background へ出す口。#2639）"
        )),
    }
    let self_test = span(&s.main, "mod self_test {");
    for (src, rel, allowed) in [(&s.chat, CHAT, chat_fn), (&s.main, MAIN, self_test)] {
        for at in all_at(src, "Request::OrchestratorRespond") {
            if allowed.is_some_and(|(a, b)| (a..b).contains(&at)) {
                continue;
            }
            out.push(format!(
                "{rel}:{} `Request::OrchestratorRespond` を `chat_respond` / セルフテストの外で組んでいる。同期の dispatch は\
                 UI スレッドで応答の待ちを払う（#2639）。`prepare_offload` を通す",
                line_at(src, at)
            ));
        }
    }
    for (rel, src) in &s.others {
        for at in all_at(src, "Request::OrchestratorRespond") {
            out.push(format!(
                "{rel}:{} `Request::OrchestratorRespond` を組んでいる。同期の dispatch は UI スレッドで応答の待ちを払う\
                 （#2639）。`prepare_offload` を通す",
                line_at(src, at)
            ));
        }
    }

    // 5. 応答の手順を直に呼ぶのは background の自動復帰だけ
    let mut files: Vec<(&str, &str)> = vec![(CHAT, s.chat.as_str()), (MAIN, s.main.as_str())];
    files.extend(s.others.iter().map(|(r, c)| (r.as_str(), c.as_str())));
    for (rel, src) in files {
        if rel == AUTORESUME {
            continue;
        }
        for needle in ["respond_via(", "respond_to_choice_dialog("] {
            for at in all_at(src, needle) {
                out.push(format!(
                    "{rel}:{} `{needle}` を直に呼んでいる。応答の手順は 400ms ずつ待つので UI スレッドで回さない（#2639）。\
                     dispatch（`prepare_offload`）を通すか、background で回す",
                    line_at(src, at)
                ));
            }
        }
    }
    out
}

#[test]
fn issue2639_respondはメインスレッドで待たない() {
    let found = scan(&sources());
    assert!(
        found.is_empty(),
        "#2639 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn issue2639_走査が空振りしていない() {
    let s = sources();
    // 自動復帰の background の応答が見えている（5 の除外が実在の 1 件を外している）
    let autoresume = s
        .others
        .iter()
        .find(|(rel, _)| rel == AUTORESUME)
        .expect("limit_autoresume.rs が走査に入っている");
    assert!(
        autoresume.1.contains("respond_via("),
        "{AUTORESUME} の respond_via が見えない（走査の前提が変わった）"
    );
    assert_eq!(
        all_at(&s.chat, "Request::OrchestratorRespond").len(),
        1,
        "{CHAT} の `Request::OrchestratorRespond` が 1 件でない（走査の前提が変わった）"
    );
    let (a, b) = span(&s.main, "mod self_test {").expect("セルフテストの mod");
    assert!(
        all_at(&s.main, "Request::OrchestratorRespond")
            .iter()
            .any(|at| (a..b).contains(at)),
        "{MAIN} のセルフテストの `Request::OrchestratorRespond` が見えない"
    );
    assert!(
        s.others.len() > 20,
        "tako-app の他のファイルが走査に入っていない"
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
fn issue2639_注入_準備の腕を外すと名指す() {
    let s = sources();
    let base = &s.dispatch;
    let (start, _) = span(base, "pub fn prepare_offload(").unwrap();
    let dispatch = inject(
        base,
        "Request::OrchestratorRespond {\n            pane_id,\n            choice,\n            caller_role,\n        } if !respond_offload_legacy() =>",
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
fn issue2639_注入_ab門を外すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "        } if !respond_offload_legacy() => Some(respond_access(",
        "        } => Some(respond_access(",
        "A/B の門を外す",
    );
    let (start, _) = span(&dispatch, "pub fn prepare_offload(").unwrap();
    let line = line_at(
        &dispatch,
        start
            + dispatch[start..]
                .find("Request::OrchestratorRespond {")
                .unwrap(),
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), "A/B の門を外す");
}

#[test]
fn issue2639_注入_続きをuiスレッドへ戻すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "            other => OffloadOutcome::Reply(other.run_reply()),",
        "            OffloadJob::OrchestratorRespond(job) => OffloadOutcome::Reply(respond_via(job.access.as_ref(), job.pane_id, None, None)),\n            other => OffloadOutcome::Reply(other.run_reply()),",
        "run_staged で答える",
    );
    let line = line_of(
        &dispatch,
        "OffloadJob::OrchestratorRespond(job) => OffloadOutcome::Reply",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), "run_staged で答える");
}

#[test]
fn issue2639_注入_錠を外すと名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "                let _serial = RESPOND_SERIAL\n                    .lock()\n                    .unwrap_or_else(std::sync::PoisonError::into_inner);\n",
        "",
        "錠を外す",
    );
    let line = line_of(&dispatch, "OffloadJob::OrchestratorRespond(job) =>");
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(&found, &format!("{DISPATCH}:{line}"), "錠を外す");
}

#[test]
fn issue2639_注入_同期の腕が自前で届き方を解決すると名指す() {
    let s = sources();
    let dispatch = inject(
        &s.dispatch,
        "    let access = respond_access(host, pane_id)?;\n",
        "    let access = crate::reach::dialog_access(host, pane_id, None).map_err(|r| DispatchError::Operation(r.note()))?;\n",
        "同期の腕が自前で解決する",
    );
    let line = line_of(
        &dispatch,
        "crate::reach::dialog_access(host, pane_id, None)",
    );
    let found = scan(&Sources {
        dispatch,
        ..sources()
    });
    assert_named(
        &found,
        &format!("{DISPATCH}:{line}"),
        "同期の腕が自前で解決する",
    );
}

#[test]
fn issue2639_注入_承認カードを同期のdispatchへ戻すと名指す() {
    let s = sources();
    let chat = inject(
        &s.chat,
        "let job = match tako_control::prepare_offload(self, &request) {",
        "let job = match None::<Option<Result<tako_control::OffloadJob, tako_control::DispatchError>>>.flatten() {",
        "承認カードの同期の dispatch",
    );
    let line = line_of(&chat, "fn chat_respond(");
    let found = scan(&Sources { chat, ..sources() });
    assert_named(
        &found,
        &format!("{CHAT}:{line}"),
        "承認カードの同期の dispatch",
    );
}

#[test]
fn issue2639_注入_uiから応答の手順を直に呼ぶと名指す() {
    let s = sources();
    let chat = inject(
        &s.chat,
        "        if self.chat_respond_pending.contains(&pane_id) {",
        "        let _ = tako_control::dispatch::respond_to_choice_dialog(\"tako-x\", pane_id.as_u64(), Some(\"1\"), None);\n        if self.chat_respond_pending.contains(&pane_id) {",
        "UI から応答の手順を直に呼ぶ",
    );
    let line = line_of(&chat, "tako_control::dispatch::respond_to_choice_dialog(");
    let found = scan(&Sources { chat, ..sources() });
    assert_named(
        &found,
        &format!("{CHAT}:{line}"),
        "UI から応答の手順を直に呼ぶ",
    );
}

#[test]
fn issue2639_注入_別のファイルで応答を組むと名指す() {
    let s = sources();
    let mut others = s.others.clone();
    let (rel, src) = others
        .iter_mut()
        .find(|(rel, _)| rel.ends_with("settings_window.rs"))
        .expect("settings_window.rs が走査に入っている");
    src.push_str(
        "\nfn injected() { let _ = Request::OrchestratorRespond { pane_id: 1, choice: None, caller_role: None }; }\n",
    );
    let line = line_of(src, "Request::OrchestratorRespond");
    let want = format!("{rel}:{line}");
    let found = scan(&Sources {
        others,
        ..sources()
    });
    assert_named(&found, &want, "別のファイルで応答を組む");
}
