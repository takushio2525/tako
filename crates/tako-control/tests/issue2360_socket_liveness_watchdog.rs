//! **#2360 の番犬**: IPC の受け口が、一時的に繋がらないだけの生きた受け口を「死んだ残骸」と
//! 読んで奪わない。
//!
//! ## 何が起きていたか
//!
//! 起動時の生存確認（`preferred_socket_path`）は `exists() && UnixStream::connect(..).is_ok()`
//! の 1 回だけで、繋がらなければ `bind` が固定パスを unlink して張り直していた。生きた GUI の
//! 受け口が満杯（backlog あふれ = macOS は ECONNREFUSED）の瞬間に、同じ data dir の 2 つ目の
//! tako-app が多重起動のガード（#113 の control.json の pid）をすり抜けて起動すると
//! （control.json が無い・壊れている）、生きた受け口を奪った（隔離 GUI 2 本 + SIGSTOP と
//! backlog を埋める注入で実測。`scripts/test-ipc-liveness-2360.sh`）。
//!
//! ## 何を固定するか
//!
//! 1. [`生存確認のconnectは判定の1実装の中だけ`] — `ipc.rs` の本番コードで `UnixStream::connect`
//!    を呼ぶのは判定（`occupant_with`）と、判定の外で意味が違う 2 つ（自分の accept を起こす
//!    `wake_accept`・判定の直後に unlink 直前で取り直す `rebind`）だけ。connect 1 回で
//!    奪うかを決める形が戻ったら `file:line` で名指して落ちる
//! 2. [`奪うかを決める3か所が判定の1実装を呼ぶ`] — 起動時の置き場・明示パス（#1994）・
//!    #2002 の見張りが `occupant(` を通っている
//! 3. [`guiは固定パスの先客を第三のガードとして読む`] — IPC を立てた後に `fixed_path_holder` を
//!    読んでから `secondary` を確定している
//! 4. [`abの逃げ道は1つのenvに閉じている`] — `TAKO_2360_LEGACY` を読むのは tako-core の 1 か所

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる）
fn production(rel: &str) -> String {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::production(&src, rel)
}

/// `fn 名前(` の宣言行から関数名を取る
fn fn_name(line: &str) -> Option<&str> {
    let at = line.find("fn ")?;
    // `fn` の前は空白か可視性・修飾子だけ（`.fn_x(` のような呼び出しを拾わない）
    let head = line[..at].trim();
    let head_ok = head.is_empty()
        || head
            .split_whitespace()
            .all(|w| w.starts_with("pub") || matches!(w, "unsafe" | "const" | "async" | "extern"));
    if !head_ok {
        return None;
    }
    let rest = &line[at + 3..];
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    (end > 0).then(|| &rest[..end])
}

/// 各行を囲む関数の名前（直前に現れた `fn` 宣言。閉じ括弧は数えない近似 = この用途では十分）
fn enclosing_fns(text: &str) -> Vec<Option<String>> {
    let mut current: Option<String> = None;
    text.lines()
        .map(|line| {
            if let Some(name) = fn_name(line) {
                current = Some(name.to_string());
            }
            current.clone()
        })
        .collect()
}

/// `name` の関数本体（宣言行から、同じ字下げの `}` まで）
fn fn_body<'a>(text: &'a str, name: &str) -> Vec<(usize, &'a str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if fn_name(line) != Some(name) {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        for (j, body) in lines.iter().enumerate().skip(i) {
            out.push((j + 1, *body));
            let closing = body.len() - body.trim_start().len() == indent && body.trim() == "}";
            if j > i && closing {
                break;
            }
        }
    }
    out
}

const IPC: &str = "crates/tako-control/src/ipc.rs";

/// 判定の外で connect してよい関数と、その理由
const CONNECT_ALLOWED: &[(&str, &str)] = &[
    (
        "occupant_with",
        "判定の 1 実装（judge_occupant へ渡す connect）",
    ),
    (
        "wake_accept",
        "drop が自分の accept スレッドを起こす 1 本（#2136。生死の判定ではない）",
    ),
    (
        "rebind",
        "見張りの判定（occupant）が残骸と言った直後、unlink の直前に取り直す（#2002）",
    ),
];

#[test]
fn 生存確認のconnectは判定の1実装の中だけ() {
    let text = production(IPC);
    let owners = enclosing_fns(&text);
    let offenders: Vec<String> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("UnixStream::connect("))
        .filter(|(i, _)| {
            let owner = owners[*i].as_deref().unwrap_or("");
            !CONNECT_ALLOWED.iter().any(|(name, _)| *name == owner)
        })
        .map(|(i, l)| {
            format!(
                "{IPC}:{} （fn {}）`{}`",
                i + 1,
                owners[i].as_deref().unwrap_or("?"),
                l.trim()
            )
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "IPC の受け口が判定の 1 実装（unix_imp::occupant → tako_core::ipc_socket::judge_occupant）\
         を通らずに connect している。connect 1 回の結果で奪うかを決めると、一時的に満杯なだけの\
         生きた受け口を残骸と読んで奪う（#2360）。occupant(&path).holds() を使うこと:\n{}",
        offenders.join("\n")
    );
    // 許可した関数が実在する（改名で素通しにならない）
    for (name, why) in CONNECT_ALLOWED {
        assert!(
            !fn_body(&text, name).is_empty(),
            "{IPC} に fn {name} が無い（{why}）。改名したならこの表も直すこと"
        );
    }
}

#[test]
fn 奪うかを決める3か所が判定の1実装を呼ぶ() {
    let text = production(IPC);
    for name in ["preferred_socket_path", "start_at", "tick"] {
        let body = fn_body(&text, name);
        assert!(!body.is_empty(), "{IPC} に fn {name} が無い");
        let calls = body.iter().any(|(_, l)| l.contains("occupant("));
        let at = body.first().map(|(n, _)| *n).unwrap_or(0);
        assert!(
            calls,
            "{IPC}:{at} fn {name} がパスの先客の判定（occupant）を通っていない（#2360）"
        );
    }
    // 判定の本体は持ち主（接続情報）と A/B を渡している
    let body = fn_body(&text, "occupant_with");
    let joined: String = body.iter().map(|(_, l)| *l).collect::<Vec<_>>().join("\n");
    let at = body.first().map(|(n, _)| *n).unwrap_or(0);
    for needle in ["judge_occupant(", "live_owner_of(", "std::thread::sleep"] {
        assert!(
            joined.contains(needle),
            "{IPC}:{at} fn occupant_with が `{needle}` を渡していない（#2360）"
        );
    }
}

#[test]
fn guiは固定パスの先客を第三のガードとして読む() {
    let rel = "crates/tako-app/src/main.rs";
    let text = production(rel);
    let line_of = |needle: &str| text.lines().position(|l| l.contains(needle)).map(|i| i + 1);
    let start = line_of("IpcServer::start_with(")
        .unwrap_or_else(|| panic!("{rel} に `IpcServer::start_with(` が無い（IPC を立てる行）"));
    // 無いときは IPC を立てた行を名指す（第三のガードはその直後に置く）
    let holder = line_of("IpcServer::fixed_path_holder").unwrap_or_else(|| {
        panic!(
            "{rel}:{start} IPC を立てた後に固定パスの先客（IpcServer::fixed_path_holder）を読んでいない。\
             control.json が盲目なときに 2 つ目がセカンダリにならない（#2360 の第三のガード）"
        )
    });
    let decided = line_of("let secondary = secondary_reason.is_some();").unwrap_or_else(|| {
        panic!("{rel}:{holder} の後で secondary を確定していない（#2360 の第三のガード）")
    });
    assert!(
        start < holder && holder < decided,
        "{rel}:{holder} 固定パスの先客（fixed_path_holder）を IPC を立てた後・secondary を確定する前に\
         読んでいない（start_with={start} / fixed_path_holder={holder} / secondary={decided}）"
    );
}

#[test]
fn abの逃げ道は1つのenvに閉じている() {
    let readers: Vec<String> = production_range::product_sources()
        .iter()
        .flat_map(|rel| {
            let src = std::fs::read_to_string(workspace_root().join(rel))
                .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
            production_range::scan(&src)
                .text
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains("\"TAKO_2360_LEGACY\""))
                .map(|(i, _)| format!("{rel}:{}", i + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        readers.len() == 1 && readers[0].starts_with("crates/tako-core/src/ipc_socket.rs:"),
        "TAKO_2360_LEGACY を読むのは tako_core::ipc_socket::legacy_2360 だけにする\
         （起動時の判定・見張り・GUI の第三のガードの A/B がずれる）: {readers:?}"
    );
}
