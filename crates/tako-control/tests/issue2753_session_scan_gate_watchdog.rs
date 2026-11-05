//! **#2753 の番犬**: GUI の会話の走査の門は `sessions::scan_plan`（純関数）だけが決め、persist OFF でも
//! セッションカタログを書く。persist OFF の腕は子プロセスを起こさず、復元の種にも触らない。
//!
//! ## なぜ縛るのか
//!
//! 修正前の門は走査ループの中の `let should = app.tmux_persist && …` で、persist が OFF の間
//! （`TAKO_ISOLATED=1` の既定 = `TAKO_PERSIST=0`・設定で永続化を切った利用者）はカタログを一度も書かなかった。
//! 隔離 GUI で起こした master の会話は `tako sessions show` から永久に「カタログに見つからない」になり、
//! 外部の GUI フロントエンドがチャットに会話を描けなかった。門をループへ書き戻すと同じ壊れ方が黙って戻る。
//!
//! 逆向きの罠もある: persist OFF の腕で `claude agents --json` を起こすと、既定の設定 dir は必ず起こされ
//! （#1011）、claude 本体は起動のたびに `~/.claude.json` を書き戻す（#944）。persist OFF の隔離 GUI
//! （検証で日々立つ）から利用者の設定へ書く経路ができる。だから persist OFF の腕は mod の報告だけで書く。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `sessions.rs` に門の純関数 `pub fn scan_plan(` がある
//! 2. tako-app の走査ループは `sessions::scan_plan(` に `persist: app.tmux_persist` と A/B の門
//!    `legacy_2753()` を渡し、`plan.catalog` / `plan.processes` で分ける。門を `app.tmux_persist` の
//!    真偽式でループへ書き戻した形（`let should = app.tmux_persist` 等）は名指す
//! 3. persist OFF の腕（`if !plan.processes {`）は子プロセスの走査・復元の種に触らず、
//!    `record_detected_sessions(` だけで書く
//! 4. `record_detected_sessions` はカタログ（`sync_session_catalog(`）と worker レジストリ
//!    （`record_session_detected`）の両方を書く（persist ON の腕と同じ 1 実装）
//! 5. mod の報告の受け口（`accept_report`）は受け付けた報告（`Accepted::Stored`）の会話 ID で
//!    `note_mod_session(` を呼ぶ（案 B = 反映の遅れを 30 秒から次の確認へ縮める）

use std::path::{Path, PathBuf};

const SESSIONS: &str = "crates/tako-control/src/sessions.rs";
const CLAUDE_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

// 固定のバイト幅の窓は 1 実装（#2043）。端を外側へ文字境界まで広げるので日本語で落ちない
#[path = "common/byte_window.rs"]
mod byte_window;

fn read_code(rel: &str) -> String {
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production(&src, rel))
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

/// `sig` で始まるブロックの範囲（最初の 1 つ）
fn span(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

/// persist OFF の腕で呼んではいけないもの（子プロセスの走査・復元の種）
const FORBIDDEN_WITHOUT_PERSIST: &[(&str, &str)] = &[
    (
        "list_agents_for_scan_scoped(",
        "`claude agents --json` を起こす（既定の設定 dir は必ず起こされ、`~/.claude.json` を書き戻す = #944）",
    ),
    (
        "any_target_has_running_children(",
        "前段ガード（プロセス表の走査）",
    ),
    ("ProcessSnapshot::capture(", "プロセス表の採取"),
    ("detect_resume_ids(", "codex / agy の会話 ID の採取（lsof）"),
    (
        "apply_claude_resume_sessions(",
        "復元の種（ペイン → 会話の写し = layout.json と limit-resume の材料）",
    ),
    ("apply_agent_resume_sessions(", "codex / agy の復元の種"),
    ("adopt_limit_resume_default(", "limit-resume の既定の適用"),
    ("save_layout(", "layout.json の書き出し"),
];

/// 門を走査ループへ書き戻した形（修正前の `let should = app.tmux_persist && …`）
const LEGACY_GATE_SHAPES: &[&str] = &[
    "let should = app.tmux_persist",
    "app.tmux_persist\n                            && !app.secondary",
    "app.tmux_persist && !app.secondary",
];

fn scan(sessions: &str, claude_mod: &str, main: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 1. 門の純関数
    if !sessions.contains("pub fn scan_plan(") {
        out.push(format!(
            "{SESSIONS}:1 門の純関数 `pub fn scan_plan(` が無い（#2753。走査ループの門の正本）"
        ));
    }

    // 2. 走査ループは門の純関数だけを見る
    match main.find("sessions::scan_plan(") {
        None => out.push(format!(
            "{MAIN}:1 会話の走査ループが `sessions::scan_plan(` を通っていない。門をループへ書くと persist OFF で\
             カタログが育たない形へ黙って戻る（#2753）"
        )),
        Some(at) => {
            let line = line_at(main, at);
            let call = &main[at..block_end(main, at).map_or(main.len(), |e| e + 1)];
            let tail = byte_window::after(main, at, 600);
            if !call.contains("persist: app.tmux_persist") {
                out.push(format!(
                    "{MAIN}:{line} `scan_plan` へ `persist: app.tmux_persist` を渡していない（#2753）"
                ));
            }
            if !tail.contains("legacy_2753()") {
                out.push(format!(
                    "{MAIN}:{line} `scan_plan` へ A/B の門（`legacy_2753()`）を渡していない（#2753）"
                ));
            }
        }
    }
    for shape in LEGACY_GATE_SHAPES {
        for (at, _) in main.match_indices(shape) {
            out.push(format!(
                "{MAIN}:{} 会話の走査の門を `app.tmux_persist` の真偽式で書いている（`{}`）。persist OFF で\
                 カタログが一度も書かれない修正前の形（#2753）。門は `sessions::scan_plan` だけが決める",
                line_at(main, at),
                shape.replace('\n', " ").split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
    }

    // 3. persist OFF の腕
    match span(main, "if !plan.processes {") {
        None => out.push(format!(
            "{MAIN}:1 persist OFF の腕（`if !plan.processes {{`）が無い（#2753。mod の報告だけで書く腕）"
        )),
        Some((start, end)) => {
            let body = &main[start..end];
            let line = line_at(main, start);
            for (needle, why) in FORBIDDEN_WITHOUT_PERSIST {
                if let Some(i) = body.find(needle) {
                    out.push(format!(
                        "{MAIN}:{} persist OFF の腕（{line} 行目から）が `{needle}` を呼んでいる = {why}。\
                         persist OFF では子プロセスを起こさず復元の種にも触らない（#2753）",
                        line_at(main, start + i)
                    ));
                }
            }
            if !body.contains("record_detected_sessions(") {
                out.push(format!(
                    "{MAIN}:{line} persist OFF の腕が `record_detected_sessions(` で書いていない（カタログと worker\
                     レジストリへ mod の報告の会話を載せる。#2753）"
                ));
            }
            if !body.contains("continue") {
                out.push(format!(
                    "{MAIN}:{line} persist OFF の腕が `continue` で抜けていない（後ろの子プロセスの走査へ落ちる。#2753）"
                ));
            }
        }
    }

    // 4. 書き手の 1 実装
    match span(main, "fn record_detected_sessions(") {
        None => out.push(format!(
            "{MAIN}:1 `fn record_detected_sessions(` が無い（persist ON / OFF の腕が同じ 1 実装で書く。#2753）"
        )),
        Some((start, end)) => {
            let body = &main[start..end];
            let line = line_at(main, start);
            for needle in [
                "sync_session_catalog(",
                "record_session_detected(",
                "record_session_detected_by_pane(",
            ] {
                if !body.contains(needle) {
                    out.push(format!(
                        "{MAIN}:{line} `record_detected_sessions` が `{needle}` を呼んでいない（#2753 / #390）"
                    ));
                }
            }
        }
    }
    let writers = main.matches("record_detected_sessions(").count();
    if writers < 3 {
        out.push(format!(
            "{MAIN}:1 `record_detected_sessions(` の呼び手が {} 本（定義 + persist ON の腕 + persist OFF の腕 = 3 以上のはず。#2753）",
            writers.saturating_sub(1)
        ));
    }

    // 5. mod の報告の受け口
    match span(claude_mod, "pub fn accept_report(") {
        None => out.push(format!(
            "{CLAUDE_MOD}:1 `accept_report` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &claude_mod[start..end];
            let line = line_at(claude_mod, start);
            match body.find("note_mod_session(") {
                None => out.push(format!(
                    "{CLAUDE_MOD}:{line} `accept_report` が `note_mod_session(` を呼んでいない。新しい会話は走査の 30 秒を\
                     待たないとカタログに載らない（#2753 の案 B）"
                )),
                Some(i) => {
                    let accept = body.find("hub.accept(");
                    if accept.is_none_or(|a| a > i) {
                        out.push(format!(
                            "{CLAUDE_MOD}:{} `note_mod_session(` が `hub.accept(` より前にある（受け付けた報告だけで頼む。#2753）",
                            line_at(claude_mod, start + i)
                        ));
                    }
                    let near = byte_window::before(body, i, 200);
                    if !near.contains("Accepted::Stored") {
                        out.push(format!(
                            "{CLAUDE_MOD}:{} `note_mod_session(` が受け付けた報告（`Accepted::Stored`）に限っていない\
                             （ended / dormant の報告で走査を頼まない。#2753）",
                            line_at(claude_mod, start + i)
                        ));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn issue2753_会話の走査の門はscan_planだけが決めpersist_offの腕は子プロセスを起こさない() {
    let sessions = read_code(SESSIONS);
    let claude_mod = read_code(CLAUDE_MOD);
    let main = read_code(MAIN);
    let found = scan(&sessions, &claude_mod, &main);
    assert!(found.is_empty(), "#2753 の番犬:\n{}", found.join("\n"));
}

#[test]
fn issue2753_番犬は修正前の門とpersist_offの腕の子プロセスを名指す() {
    // 検出力の確かめ: 実物を写して壊した形を食わせる（実ファイルは書き換えない）
    let sessions = read_code(SESSIONS);
    let claude_mod = read_code(CLAUDE_MOD);
    let main = read_code(MAIN);

    // 修正前の門（走査ループへ書き戻す）
    let legacy = main.replacen(
        "sessions::scan_plan(",
        "let should = app.tmux_persist && !app.secondary; sessions::scan_plan_gone(",
        1,
    );
    let found = scan(&sessions, &claude_mod, &legacy);
    assert!(
        found
            .iter()
            .any(|f| f.contains("`sessions::scan_plan(` を通っていない")),
        "{found:#?}"
    );
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(MAIN) && f.contains("`app.tmux_persist` の真偽式")),
        "{found:#?}"
    );

    // persist OFF の腕で `claude agents --json` を起こす
    let arm = main.find("if !plan.processes {").expect("persist OFF の腕");
    let mut leaky = main.clone();
    leaky.insert_str(
        arm + "if !plan.processes {".len(),
        " let _ = tako_control::agents::list_agents_for_scan_scoped(&[], &[]); app.save_layout(); ",
    );
    let found = scan(&sessions, &claude_mod, &leaky);
    let line = main[..arm].matches('\n').count() + 1;
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(&format!("{MAIN}:{line} "))
                && f.contains("list_agents_for_scan_scoped(")),
        "{found:#?}"
    );
    assert!(
        found.iter().any(|f| f.contains("`save_layout(`")),
        "{found:#?}"
    );

    // 案 B を外す
    let unhooked = claude_mod.replacen("note_mod_session(", "note_mod_session_gone(", 1);
    let found = scan(&sessions, &unhooked, &main);
    assert!(
        found
            .iter()
            .any(|f| f.starts_with(CLAUDE_MOD) && f.contains("`note_mod_session(` を呼んでいない")),
        "{found:#?}"
    );
}
