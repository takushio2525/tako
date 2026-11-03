//! **#2686 の番犬**: 検証起動の GUI が env へ自分で当てた置き場と pid 入りのソケットファイルは、
//! 終了の後片付けの 1 実装で片付ける。明示した置き場は消さない。本番の GUI は変えない。
//!
//! ## なぜ縛るのか
//!
//! 隔離起動（`TAKO_ISOLATED=1`）とセルフテストの GUI は、利用者が置き場を指定しなかった env に自分の
//! pid 入りの一時の置き場（`tako-iso-data-<pid>`・`tako-iso-pane-logs-<pid>`・セッションカタログ・
//! セルフテストの workers / orchestrator ほか 10 個）を当てる。これと本体の受け口の pid 入りのソケット
//! ファイル（`tako-<pid>-0.sock`）が、成功・セルフテストの FAILED・SIGTERM のどの出口でも残り、
//! 検証のたびに一時 dir へ積もっていた（#2686 で実測。ソケットは GPUI が TakoApp を drop せずに終わるので
//! drop の片付けが走らない）。片付けは `tako_core::scoped_dirs` が覚えた置き場だけを消す
//! （明示 = 残す・暗黙 = 片付ける）。消した後に書き手が置き場を作り直さないよう、退役した置き場への
//! 書き込みは捨てる。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. **片付けを外さない**: `cleanup_process_scoped`（成功・SIGTERM・FAILED・visual-test の出口の 1 実装）が
//!    `scoped_dirs::remove_adopted(` と `ipc::remove_scoped_socket_files(` を呼ぶ。片付けの口
//!    `remove_adopted(` を呼ぶのはそこだけ
//! 2. **明示した置き場を消さない**:
//!    - `cleanup_process_scoped` の中で直に消すのは検証用の設定置き場（`verification_agent_home`）だけ
//!      （data dir・ペインログの置き場を直に消す形を名指す）
//!    - 当てる口 `scoped_dirs::adopt` は env が在れば当てずに戻り（覚える `push` はその後）、消す口
//!      `remove_adopted` は `removable(` で名前が自分の pid か・env がまだその値かを確かめてから消す
//! 3. **自分で決めた置き場は当てる口を通す**: tako-app の起動で `tako-iso-…` / `tako-st-…` の置き場を
//!    `set_var(` で直に当てない（当てても覚えないので片付けから漏れる）
//! 4. **消した置き場を作り直さない**: 診断ログの書き手 `append_log` は `create_dir_all(` の前に、
//!    セッションカタログの終了時の書き出し `flush_last_seen` は書く前に `scoped_dirs::is_retired(` を見る
//! 5. **本番の GUI は変えない**: 覚える・消す・退役を見る口と、ソケットの名簿へ載せる口は検証プロセスの
//!    門（`is_verification_process()`）の内側

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const SCOPED: &str = "crates/tako-core/src/scoped_dirs.rs";
const DIAG: &str = "crates/tako-control/src/diag.rs";
const SESSIONS: &str = "crates/tako-control/src/sessions.rs";
const IPC: &str = "crates/tako-control/src/ipc.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す = 行番号はずれない）
fn read(rel: &str) -> String {
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    let production = if rel == MAIN {
        production_range::production(&src, rel)
    } else {
        production_range::scan(&src).text
    };
    production_range::code_view::without_comments(&production)
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

/// `sig` で始まる関数の本体の範囲（シグネチャの位置〜閉じ括弧）
fn fn_span(src: &str, sig: &str, rel: &str) -> (usize, usize) {
    let at = src.find(sig).unwrap_or_else(|| {
        panic!("{rel} に `{sig}` が無い（構造が変わったらこの番犬も見直すこと）")
    });
    (at, block_end(src, at))
}

/// `needle` の出現位置をすべて返す
fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// `at` を含む文（`at` から `;` まで）
fn statement_at(src: &str, at: usize) -> &str {
    let end = src[at..].find(';').map_or(src.len(), |i| at + i);
    &src[at..end]
}

// ------------------------------------------------------------------ 1. 片付けを外さない

#[test]
fn issue2686_終了の後片付けは自分で決めた置き場とソケットを片付ける() {
    let src = read(MAIN);
    let (start, end) = fn_span(&src, "fn cleanup_process_scoped(", MAIN);
    let body = &src[start..end];
    for (call, what) in [
        (
            "tako_core::scoped_dirs::remove_adopted(",
            "GUI が env へ自分で当てた置き場",
        ),
        (
            "tako_control::ipc::remove_scoped_socket_files(",
            "本体の受け口の pid 入りのソケットファイル",
        ),
    ] {
        assert!(
            body.contains(call),
            "{MAIN}:{} の `cleanup_process_scoped` が `{call}` を呼んでいない = {what}が成功・FAILED・\
             SIGTERM のどの出口でも残る（#2686 の片付けが外れている）",
            line_at(&src, start)
        );
    }
}

#[test]
fn issue2686_置き場を消す口を呼ぶのは終了の後片付けの_1_実装だけ() {
    let mut out = Vec::new();
    for rel in production_range::product_sources() {
        if rel == SCOPED {
            continue;
        }
        let src = read(&rel);
        let allowed = (rel == MAIN).then(|| fn_span(&src, "fn cleanup_process_scoped(", MAIN));
        for at in all_at(&src, "remove_adopted(") {
            if allowed.is_some_and(|(s, e)| (s..e).contains(&at)) {
                continue;
            }
            out.push(format!(
                "{rel}:{} が `remove_adopted(` を呼んでいる（片付けは `cleanup_process_scoped` の 1 実装へ。\
                 出口ごとに別の片付けを書かない）",
                line_at(&src, at)
            ));
        }
    }
    assert!(out.is_empty(), "#2686:\n{}", out.join("\n"));
}

// ------------------------------------------------------------------ 2. 明示した置き場を消さない

#[test]
fn issue2686_終了の後片付けが直に消すのは検証用の設定置き場だけ() {
    let src = read(MAIN);
    let (start, end) = fn_span(&src, "fn cleanup_process_scoped(", MAIN);
    let body = &src[start..end];
    let mut out = Vec::new();
    for needle in ["remove_dir_all(", "remove_file("] {
        for at in all_at(body, needle) {
            // 許すのは `let dir = tako_core::paths::verification_agent_home();` の直後の 1 つだけ
            let before = &body[..at];
            let prev_stmt = before
                .rfind("let dir = ")
                .map(|i| statement_at(before, i))
                .unwrap_or("");
            let gap = before.rfind("let dir = ").map_or("", |i| &before[i..]);
            if prev_stmt.contains("verification_agent_home()") && gap.matches(';').count() == 1 {
                continue;
            }
            out.push(format!(
                "{MAIN}:{} の `cleanup_process_scoped` が `{needle}` で直に消している（{}）。\
                 片付けるのは `scoped_dirs::remove_adopted` が覚えた置き場だけ = 利用者が明示した \
                 `TAKO_DATA_DIR` / ペインログの置き場は消さない",
                line_at(&src, start + at),
                statement_at(body, at).trim()
            ));
        }
    }
    assert!(out.is_empty(), "#2686:\n{}", out.join("\n"));
}

#[test]
fn issue2686_当てる口は明示した値を覚えず消す口は持ち主を確かめる() {
    let src = read(SCOPED);
    let mut out = Vec::new();

    let (start, end) = fn_span(&src, "pub fn adopt(", SCOPED);
    let body = &src[start..end];
    let guard = body.find("if std::env::var_os(key).is_some() {");
    let guard_returns =
        guard.is_some_and(|g| body[g..block_end(body, g)].contains("return false;"));
    let push = body.find(".push(");
    if !guard_returns || push.is_none_or(|p| guard.is_none_or(|g| p < g)) {
        out.push(format!(
            "{SCOPED}:{} の `adopt` が、env が在るとき（利用者が明示した置き場）に当てずに戻る前に覚えている\
             （`if std::env::var_os(key).is_some() {{ return false; }}` → `.push(` の順でない）= 明示した置き場を\
             片付けが消す",
            line_at(&src, start)
        ));
    }

    let (start, end) = fn_span(&src, "pub fn remove_adopted(", SCOPED);
    let body = &src[start..end];
    let check = body.find("if !removable(");
    for needle in ["remove_path(", "remove_dir_all(", "remove_file("] {
        for at in all_at(body, needle) {
            if check.is_none_or(|c| at < c) {
                out.push(format!(
                    "{SCOPED}:{} の `remove_adopted` が持ち主の確認（`if !removable(` = 名前が自分の pid か・\
                     env がまだその値か）より前に消している",
                    line_at(&src, start + at)
                ));
            }
        }
    }
    let (start, end) = fn_span(&src, "pub fn removable(", SCOPED);
    let body = &src[start..end];
    for (needle, what) in [
        ("owned_name(", "名前が自分の pid か"),
        ("env_now == Some(", "env がまだ当てた値か"),
    ] {
        if !body.contains(needle) {
            out.push(format!(
                "{SCOPED}:{} の `removable` が {what} を確かめていない（`{needle}` が無い）",
                line_at(&src, start)
            ));
        }
    }
    assert!(out.is_empty(), "#2686:\n{}", out.join("\n"));
}

// ------------------------------------------------------------------ 3. 当てる口を通す

#[test]
fn issue2686_自分で決めた置き場は当てる口で当てる() {
    let src = read(MAIN);
    let mut out = Vec::new();
    for at in all_at(&src, "set_var(") {
        let stmt = statement_at(&src, at);
        if stmt.contains("\"tako-iso-") || stmt.contains("\"tako-st-") {
            out.push(format!(
                "{MAIN}:{} が GUI の置き場を `set_var(` で直に当てている（{}）。\
                 `tako_core::scoped_dirs::adopt` を通さないと覚えないので、終了の後片付けから漏れる",
                line_at(&src, at),
                stmt.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
    }
    // セルフテストの置き場の一覧は loop で当てる（値は `self_test_isolation_defaults` が持つ）
    let at = src
        .find("in self_test_isolation_defaults(")
        .unwrap_or_else(|| panic!("{MAIN} に `self_test_isolation_defaults` の loop が無い"));
    let body = &src[at..block_end(&src, at)];
    if !body.contains("tako_core::scoped_dirs::adopt(") || body.contains("set_var(") {
        out.push(format!(
            "{MAIN}:{} の `self_test_isolation_defaults` の loop が `scoped_dirs::adopt` で当てていない",
            line_at(&src, at)
        ));
    }
    assert!(out.is_empty(), "#2686:\n{}", out.join("\n"));
}

// ------------------------------------------------------------------ 4. 消した置き場を作り直さない

#[test]
fn issue2686_退役した置き場へは書かずに捨てる() {
    let mut out = Vec::new();
    for (rel, sig, before, what) in [
        (
            DIAG,
            "fn append_log(",
            "create_dir_all(",
            "診断ログ（persist.log / perf.log / panic.log）",
        ),
        (
            SESSIONS,
            "pub fn flush_last_seen(",
            "flush_last_seen_at(",
            "セッションカタログの終了時の書き出し",
        ),
    ] {
        let src = read(rel);
        let (start, end) = fn_span(&src, sig, rel);
        let body = &src[start..end];
        let write = body.find(before).unwrap_or_else(|| {
            panic!("{rel} の `{sig}` に `{before}` が無い（構造が変わったら見直す）")
        });
        let guard = body.find("tako_core::scoped_dirs::is_retired(");
        if guard.is_none_or(|g| g > write) {
            out.push(format!(
                "{rel}:{} の {what} が `{before}` の前に `scoped_dirs::is_retired(` を見ていない = 終了の後片付けが\
                 消した検証起動の置き場を、片付けの後の書き込みが作り直す",
                line_at(&src, start + write)
            ));
        }
    }
    assert!(out.is_empty(), "#2686:\n{}", out.join("\n"));
}

// ------------------------------------------------------------------ 5. 本番の GUI は変えない

#[test]
fn issue2686_覚える消す退役を見る口は検証プロセスの門の内側() {
    let mut out = Vec::new();
    let src = read(SCOPED);
    let (start, end) = fn_span(&src, "fn active(", SCOPED);
    if !src[start..end].contains("crate::paths::is_verification_process()") {
        out.push(format!(
            "{SCOPED}:{} の `active` が検証プロセスの門（`is_verification_process()`）を見ていない = 本番の GUI でも\
             置き場を覚えて消す",
            line_at(&src, start)
        ));
    }
    for sig in [
        "pub fn adopt(",
        "pub fn remove_adopted(",
        "pub fn left_adopted(",
        "pub fn is_retired(",
    ] {
        let (start, end) = fn_span(&src, sig, SCOPED);
        if !src[start..end].contains("active()") {
            out.push(format!(
                "{SCOPED}:{} の `{}` が `active()`（検証プロセスの門）を通っていない",
                line_at(&src, start),
                sig.trim_end_matches('(')
            ));
        }
    }
    let src = read(IPC);
    let (start, end) = fn_span(&src, "pub(super) fn remember_scoped(", IPC);
    if !src[start..end].contains("tako_core::paths::is_verification_process()") {
        out.push(format!(
            "{IPC}:{} の `remember_scoped` が検証プロセスの門を見ていない = 本番の `<data_dir>/tako.sock` の扱いが変わる",
            line_at(&src, start)
        ));
    }
    assert!(out.is_empty(), "#2686:\n{}", out.join("\n"));
}
