//! tmux の attach クライアントが外から終わってもペインを閉じない、の番犬（Issue #1857）
//!
//! # 何が壊れていたか
//!
//! tmux バックエンドのペインで tako が子として見ているのは **attach クライアント**で、
//! その終わり（`SessionNotice::Exited`）を「セッションが終わった」と区別せずに閉じていた。
//! 2026-09-30、BSD pkill の引数順の罠で attach クライアントだけが 24 本 SIGTERM され、
//! tmux サーバー・24 セッション・中の claude は生きていたのにペイン 24 枚とタブが消えた。
//! persist.log にも原因が残らなかった。
//!
//! # 検査（どれか 1 つが外れると症状が戻る）
//!
//! 1. [`exitedの腕は器の生死を聞いてから閉じる`] — `on_term_event` の Exited が
//!    `begin_backend_reattach` を通らずに閉じる形（= 再 attach の分岐を外した形）を名指す
//! 2. [`配送は端末の番号を照合する`] — 再 attach で端末を張り替えた後、古い PTY の Exit が
//!    新しい端末を閉じないよう、配送タスクは `on_term_event_from` を通す
//! 3. [`再attachはattach専用のクライアントで立てる`] — `new-session -A` で繋ぎ直すと、
//!    その間にセッションが終わっていたとき黙って新しいシェルが生える
//! 4. [`判断はcoreの1実装でpersistlogへ残す`] — 判断の写しを app に持たない・記録を落とさない
//! 5. [`待っている間に閉じたペインを蘇らせない`] / [`閉じたペインの記録を捨てる`]
//! 6. [`状態はlistとreadに載りmcpとcliは同じ要求へ振り分ける`] — 開発不変条件（AI から観測・操作できる）
//!
//! 検出力は [`注入した回帰をfile_lineで名指す`] が、実ソースへ回帰を注入して固定する
//! （走査が空振りして緑になる番犬にしない）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;
#[path = "common/production_range.rs"]
mod production_range;

const APP: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// 走査する 2 つの眺め（バイト位置は原文と同じ = 行番号がそのまま使える）。
///
/// - `code`: コメントと文字列を潰した眺め（波括弧の対応を文字列の `{name}` に惑わされずに取る）
/// - `text`: コメントだけを潰した眺め（**肯定の存在確認**はこちら。doc コメントに書いた
///   同じ綴りで緑にならない = #1609）
struct Views {
    rel: &'static str,
    code: String,
    text: String,
}

impl Views {
    fn of(rel: &'static str, src: &str) -> Self {
        let production = production_range::scan(src).text;
        Self {
            rel,
            code: code_view::code_view(&production),
            text: code_view::without_comments_checked(&production, rel),
        }
    }

    /// `head`（`fn 名前(` 等）で始まる関数の本体（`{` から対応する `}` まで）と先頭位置。
    /// **見つからなければ Err**（走査範囲が空の番犬にしない）
    fn body(&self, head: &str) -> Result<(usize, &str), String> {
        let start = self.code.find(head).ok_or_else(|| {
            format!(
                "{}: {head:?} が見つからない（走査範囲を作れない）",
                self.rel
            )
        })?;
        let open = start
            + self.code[start..]
                .find('{')
                .ok_or_else(|| format!("{}: {head:?} の本体が無い", self.rel))?;
        let mut depth = 0usize;
        for (i, b) in self.code.as_bytes()[open..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok((open, &self.text[open..=open + i]));
                    }
                }
                _ => {}
            }
        }
        Err(format!("{}: {head:?} の本体が閉じていない", self.rel))
    }

    fn at(&self, base: usize, offset: usize) -> String {
        format!("{}:{}", self.rel, line_of(&self.text, base + offset))
    }
}

// ---- 検査本体（ソース文字列を受け取る = 注入した回帰にも同じ検査を当てられる） ----

/// 1. Exited の最初の腕は `begin_backend_reattach` を条件に持ち、閉じる腕はその後ろ
fn check_exited_arm(app: &Views) -> Result<(), String> {
    let (base, body) = app.body("fn on_term_event(")?;
    let arm = "Some(SessionNotice::Exited)";
    let first = body
        .find(arm)
        .ok_or_else(|| format!("{}: on_term_event に Exited の腕が無い", app.at(base, 0)))?;
    let guard_end = first + body[first..].find("=>").unwrap_or(0);
    let guard = &body[first..guard_end];
    if !guard.contains("self.begin_backend_reattach(pane_id") {
        return Err(format!(
            "{} Exited の腕が器の生死を聞かずに閉じている（#1857: attach クライアントだけが終わった\
             ペインまで閉じる。`if self.begin_backend_reattach(pane_id, cx)` の腕を先に置くこと）",
            app.at(base, first)
        ));
    }
    let close = body
        .find("self.remove_pane_with(pane_id, CloseReason::Exited")
        .ok_or_else(|| format!("{} Exited で閉じる腕が消えている", app.at(base, 0)))?;
    if close < first {
        return Err(format!(
            "{} 器の生死を聞く前に閉じている（#1857）",
            app.at(base, close)
        ));
    }
    Ok(())
}

/// 2. 配送タスク（spawn_session の受信ループと batch_term_events）は番号を照合する入口を通す
fn check_delivery(app: &Views) -> Result<(), String> {
    let mut problems = Vec::new();
    for head in ["fn spawn_session(", "async fn batch_term_events("] {
        let (base, body) = app.body(head)?;
        let mut from = 0;
        let mut guarded = 0;
        while let Some(i) = body[from..].find("on_term_event") {
            let at = from + i;
            let rest = &body[at..];
            if rest.starts_with("on_term_event_from(") {
                guarded += 1;
            } else if rest.starts_with("on_term_event(") {
                problems.push(format!(
                    "{} 配送タスクが端末の番号を照合せずに on_term_event を呼んでいる（#1857: \
                     再 attach で張り替えた後、古い PTY の Exit が新しい端末を閉じる。\
                     `on_term_event_from(source, …)` を通すこと）",
                    app.at(base, at)
                ));
            }
            from = at + "on_term_event".len();
        }
        if guarded == 0 {
            problems.push(format!(
                "{} {head} が on_term_event_from を呼んでいない（配送の入口が消えた）",
                app.at(base, 0)
            ));
        }
    }
    let (base, body) = app.body("fn on_term_event_from(")?;
    if !body.contains("s.serial() != source.serial") {
        problems.push(format!(
            "{} on_term_event_from が端末の番号を照合していない",
            app.at(base, 0)
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// 3. spawn_session は再 attach の印があれば attach 専用のクライアントで立てる（wrap より先に見る）
fn check_attach_only(app: &Views) -> Result<(), String> {
    let (base, body) = app.body("fn spawn_session(")?;
    let flag = body
        .find("self.reattach_spawn.remove(&pane_id)")
        .ok_or_else(|| {
            format!(
                "{} spawn_session が再 attach の印（reattach_spawn）を見ていない（#1857）",
                app.at(base, 0)
            )
        })?;
    let attach_only = body
        .find("tako_core::tmux_backend::reattach_options(")
        .ok_or_else(|| {
            format!(
                "{} 再 attach が attach 専用のクライアント（reattach_options）を使っていない（#1857: \
                 new-session -A だとセッションが終わっていたとき黙って新しいシェルが生える）",
                app.at(base, flag)
            )
        })?;
    let wrap = body
        .find("tako_core::tmux_backend::wrap_options(")
        .ok_or_else(|| {
            format!(
                "{} 新しいペインの wrap_options が消えている",
                app.at(base, 0)
            )
        })?;
    if !(flag < attach_only && attach_only < wrap) {
        return Err(format!(
            "{} 再 attach の分岐が new-session -A の分岐より後ろにある（印が立っていても \
             wrap_options が先に効く）",
            app.at(base, attach_only)
        ));
    }
    Ok(())
}

/// 4. 判断は core（decide / log_line）、問い合わせは background、記録は persist.log
fn check_settle(app: &Views) -> Result<(), String> {
    let mut problems = Vec::new();
    let (base, begin) = app.body("fn begin_backend_reattach(")?;
    for (needle, why) in [
        (
            "tako_core::backend_reattach::probe_session(",
            "器への問い合わせ（上限つきの 1 実装）を通していない",
        ),
        (
            "background_executor()",
            "器への問い合わせをメインスレッドで待っている",
        ),
        (
            ".begin_exit(serial",
            "同じ端末の Exit / ChildExit を 1 回に畳んでいない",
        ),
        (
            "tako_core::backend::Choice::Tmux",
            "tmux 以外の器（psmux）まで経路を変えている",
        ),
        (
            "backend_reattach_legacy()",
            "A/B の旧挙動（TAKO_1857_LEGACY）の入口が無い",
        ),
    ] {
        if !begin.contains(needle) {
            problems.push(format!("{} begin_backend_reattach: {why}", app.at(base, 0)));
        }
    }
    let (base, settle) = app.body("fn settle_backend_exit(")?;
    for (needle, why) in [
        (
            ".decide(&probe",
            "判断を core の ReattachState::decide へ寄せていない",
        ),
        (
            "tako_core::backend_reattach::log_line(",
            "persist.log の 1 行を core の log_line から組んでいない",
        ),
        ("persist_diag(&line)", "判断を persist.log へ残していない"),
        (
            "s.serial() == serial",
            "問い合わせの間に張り替え / 閉じたペインへ答えを当てている",
        ),
    ] {
        if !settle.contains(needle) {
            problems.push(format!("{} settle_backend_exit: {why}", app.at(base, 0)));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// 5a. 待ちが明けた再 attach は、端末が同じで状態が waiting のときだけ撃つ
fn check_no_resurrect(app: &Views) -> Result<(), String> {
    let (base, body) = app.body("fn run_backend_reattach(")?;
    let guard = body.find("s.serial() == serial");
    let spawn = body.find("self.spawn_backend_reattach(pane_id");
    match (guard, spawn) {
        (Some(g), Some(s)) if g < s && body.contains("Status::Waiting") => Ok(()),
        (_, Some(s)) => Err(format!(
            "{} 待ちが明けた再 attach が、閉じた / 張り替えたペインを確かめずに撃っている（#1857: \
             再 attach 待ちの間に閉じたペインが蘇る）",
            app.at(base, s)
        )),
        _ => Err(format!(
            "{} run_backend_reattach が再 attach を撃っていない",
            app.at(base, 0)
        )),
    }
}

/// 5b. ペインを閉じたら再 attach の記録を捨てる（明示 close / PTY 死亡の両経路）
fn check_drop(app: &Views) -> Result<(), String> {
    let mut problems = Vec::new();
    for head in ["fn drop_backend_session_with(", "fn drop_backend_session("] {
        let (base, body) = app.body(head)?;
        if !body.contains("self.drop_backend_reattach(pane_id)") {
            problems.push(format!(
                "{} {head} が再 attach の記録（drop_backend_reattach）を捨てていない",
                app.at(base, 0)
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

// ---- テスト ----

fn app_views() -> Views {
    Views::of(APP, &read(APP))
}

#[test]
fn exitedの腕は器の生死を聞いてから閉じる() {
    check_exited_arm(&app_views()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 配送は端末の番号を照合する() {
    check_delivery(&app_views()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 再attachはattach専用のクライアントで立てる() {
    check_attach_only(&app_views()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 判断はcoreの1実装でpersistlogへ残す() {
    check_settle(&app_views()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 待っている間に閉じたペインを蘇らせない() {
    check_no_resurrect(&app_views()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 閉じたペインの記録を捨てる() {
    check_drop(&app_views()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn 状態はlistとreadに載りmcpとcliは同じ要求へ振り分ける() {
    let dispatch = Views::of(DISPATCH, &read(DISPATCH));
    let hits = dispatch
        .text
        .matches("\"backend_reattach\": host.backend_reattach_state(")
        .count();
    assert!(
        hits >= 2,
        "{DISPATCH}: list と read の応答に backend_reattach が載っていない（{hits} か所。\
         開発不変条件: 再接続の状態を CLI / MCP から読めること）"
    );
    assert!(
        dispatch
            .text
            .contains("Request::BackendReattach { pane } => host"),
        "{DISPATCH}: BackendReattach を host の 1 実装へ渡していない"
    );
    let mcp = Views::of(MCP_REQUEST, &read(MCP_REQUEST));
    let persist = mcp
        .text
        .find("\"tako_persist\" => match u64_arg(args, \"reattach\")")
        .unwrap_or_else(|| {
            panic!(
                "{MCP_REQUEST}: tako_persist が reattach を受けていない\
                 （MCP のツールは増やさず既存の引数で表す）"
            )
        });
    let window = &mcp.text[persist..(persist + 400).min(mcp.text.len())];
    assert!(
        window.contains("Request::BackendReattach { pane }"),
        "{}:{} tako_persist の reattach が BackendReattach へ振り分けられていない",
        MCP_REQUEST,
        line_of(&mcp.text, persist)
    );
    let cli = Views::of(CLI, &read(CLI));
    let at = cli
        .text
        .find("Command::Persist(args) if args.state.as_deref() == Some(\"reattach\")")
        .unwrap_or_else(|| {
            panic!("{CLI}: tako persist reattach が無い（CLI から再 attach できない）")
        });
    let window = &cli.text[at..(at + 400).min(cli.text.len())];
    assert!(
        window.contains("Request::BackendReattach"),
        "{}:{} tako persist reattach が BackendReattach へ振り分けられていない",
        CLI,
        line_of(&cli.text, at)
    );
}

/// 検出力: 実ソースへ回帰を注入すると、各検査が `main.rs:行` を名指して落ちる
#[test]
fn 注入した回帰をfile_lineで名指す() {
    let src = read(APP);
    let cases: [(&str, &str, &str, fn(&Views) -> Result<(), String>); 5] = [
        (
            "再 attach の分岐を外す",
            "Some(SessionNotice::Exited) if self.begin_backend_reattach(pane_id, cx) =>",
            "Some(SessionNotice::Exited) if false =>",
            check_exited_arm,
        ),
        (
            "配送の照合を外す",
            "for event in events {\n                app.on_term_event_from(source, event, cx);",
            "for event in events {\n                app.on_term_event(source.pane, event, cx);",
            check_delivery,
        ),
        (
            "再 attach を new-session -A へ戻す",
            "options = tako_core::tmux_backend::reattach_options(",
            "options = tako_core::tmux_backend::wrap_options(",
            check_attach_only,
        ),
        (
            "persist.log への記録を落とす",
            "persist_diag(&line);\n        }\n        match verdict {",
            "let _ = &line;\n        }\n        match verdict {",
            check_settle,
        ),
        (
            "閉じたペインの確認を外す",
            ".is_some_and(|s| s.serial() == serial)\n            && self\n                .backend_reattach",
            ".is_some()\n            && self\n                .backend_reattach",
            check_no_resurrect,
        ),
    ];
    for (name, from, to, check) in cases {
        assert_eq!(
            src.matches(from).count(),
            1,
            "注入先 {from:?} が {APP} にちょうど 1 か所ない（空振り / 別の箇所へ当たると検出力を測れない）: {name}"
        );
        // 注入前は通る
        check(&Views::of(APP, &src)).unwrap_or_else(|e| panic!("注入前に落ちた（{name}）: {e}"));
        let injected = src.replacen(from, to, 1);
        let err = check(&Views::of(APP, &injected))
            .err()
            .unwrap_or_else(|| panic!("注入しても落ちない = 検出力が無い: {name}"));
        let named = err.split_whitespace().any(|w| {
            w.strip_prefix(&format!("{APP}:"))
                .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
        });
        assert!(named, "file:line で名指していない（{name}）: {err}");
    }
}
