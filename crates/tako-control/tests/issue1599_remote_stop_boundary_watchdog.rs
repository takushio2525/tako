//! **#1599 の番犬**: `tako remote stop` が撃つのは境界
//! `platform::process::terminate` の 1 実装だけで、止められなければ `Err` を返す。
//!
//! ## なぜ止めるのか
//!
//! 停止には ①PID ファイルの PID を撃つ正規経路 ②PID ファイルが消えていて、記録された待ち受けが
//! 答えた PID を撃つ stale 経路 がある（#2455 からは管理トークンの停止要求を受け付けた daemon が
//! 答えた PID）。#1599 以前の ② は撃つ部分が `#[cfg(unix)]` の
//! `libc::kill(pid, SIGTERM / SIGKILL)` で、その後ろの state の掃除と `Ok(())` は無条件だった。
//! Windows は**何も撃たずに** state を消して `{"stopped": true}` を返し、daemon は動いたまま
//! tako から止められない孤児になった（嘘の成功）。
//!
//! 境界（`tako_control::platform::process::terminate`）には Windows の腕
//! （`OpenProcess` + `TerminateProcess`）があるので、`remote.rs` が自前で `libc::kill` を書くと、
//! それは決まって `#[cfg(unix)]` の中に入り、Windows で黙って何もしない形が戻る。
//!
//! ## 何を固定するか
//!
//! 規則そのもの（段の順序・止まらなければ Err・撃てない相手は止めたことにしない）は
//! `remote.rs` の単体テスト（`停止の芯は…` / `撃てない相手は…` ほか）と、境界の実プロセス
//! テスト（`platform::process::tests`。CI の Windows ジョブが名指しで回す）が持つ。
//! ここが止めるのは**構造**で、1 行で戻せてしまう形が 4 通りある:
//!
//! 1. [`remoteにlibc_killの直書きが無い`] — `libc::kill` / `libc::killpg` がどこかへ生える
//! 2. [`停止の芯は境界の終了要求を通る`] — 本番の手立てが境界を通らなくなる
//! 3. [`停止の経路にcfg属性を書かない`] — 撃つ部分が `#[cfg(unix)]` の中へ戻る
//! 4. [`停止の2経路は芯の結果を伝播する`] — `stop_verified_daemon` の失敗を握り潰して
//!    `stopped: true` を返す
//!
//! 正体確認が撃つより先にあること（#1401）は `issue1401_stale_stop_identity_watchdog` が持つ。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";

/// 撃つ操作の寄せ先（境界）。綴りはこの 1 か所に持つ
const BOUNDARY: &str = "crate::platform::process::terminate(";

/// 停止の芯（正規経路と stale 経路の 1 実装）
const CORE: &str = "stop_verified_daemon(";

/// 停止の経路を成す関数（ここに `#[cfg]` を書かない）
const STOP_PATH: [&str; 4] = [
    "\nfn daemon_stop_impl(",
    "\nfn kill_stale_daemon(",
    "\nfn stop_verified_daemon(",
    "\nfn request_then_wait(",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」（`#[cfg(test)]` の item・コメント・文字列を潰す）。
///
/// どちらも**バイト長と行番号が保たれる**ので `file:line` の名指しがそのまま使える。
/// コメントを潰すのは、製品側の doc コメントに「`libc::kill` を書いていた」という経緯が
/// 実在するため
fn code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    let production = production_range::production(&src, rel);
    production_range::code_view_of(&production)
}

/// 関数 1 本の本文（シグネチャ行から同インデントの `}` まで）。
/// 戻り値は `(シグネチャ行の行番号, 本文の先頭の byte 位置, 本文)`
fn fn_body<'a>(source: &'a str, rel: &str, signature: &str) -> (usize, usize, &'a str) {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{rel} に `{signature}` が見つからない（改名したら番犬も直す）"));
    let head_line = source[..start].lines().count() + 1;
    let sig_line = signature.trim_start_matches('\n');
    let indent = " ".repeat(sig_line.len() - sig_line.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    (head_line, start, &source[start..end])
}

/// `offset` の行番号（1 始まり）
fn line_of(source: &str, offset: usize) -> usize {
    source[..offset].matches('\n').count() + 1
}

// --------------------------------------------------------------- 検査本体
// 注入テストからも呼ぶので、読むソースは引数で受ける

fn check_no_libc_kill(source: &str, rel: &str) {
    let hits: Vec<usize> = source
        .match_indices("libc::kill")
        .map(|(at, _)| line_of(source, at))
        .collect();
    assert!(
        hits.is_empty(),
        "{rel}:{} `libc::kill` が直書きされている（#1599）。\n\
         → 停止は `{BOUNDARY}pid, force)` を通すこと（Windows の腕 = TerminateProcess つき・\n\
         0 / 範囲外の pid を送る前に断るのもそちら）。自前で書くと `#[cfg(unix)]` の中に入り、\n\
         Windows は何も撃たずに `stopped: true` を返す（#1599 の嘘の成功）。\n\
         生存確認なら `tako_core::platform::process::pid_alive`（#1557）",
        hits.iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(" / ")
    );
}

fn check_core_uses_boundary(source: &str, rel: &str) {
    let (_, istart, imp) = fn_body(source, rel, "\nimpl StopMeans for LiveStopMeans {");
    let (_, tstart, terminate) = fn_body(imp, rel, "\n    fn terminate(");
    assert!(
        terminate.contains(BOUNDARY),
        "{rel}:{} 本番の停止の手立て（`LiveStopMeans::terminate`）が境界 `{BOUNDARY}…)` を\n\
         通っていない（#1599）。本文:\n{terminate}",
        // 本文は改行から始まるので、その次の行がシグネチャ
        line_of(source, istart + tstart) + 1
    );
    let (cline, _, core) = fn_body(source, rel, &format!("\nfn {CORE}"));
    assert!(
        !core.contains("libc::"),
        "{rel}:{cline} 停止の芯に libc の直書きがある（#1599）。撃つのは手立て（境界）だけ"
    );
}

fn check_no_cfg_in_stop_path(source: &str, rel: &str) {
    for signature in STOP_PATH {
        let (_, start, body) = fn_body(source, rel, signature);
        if let Some(at) = body.find("#[cfg") {
            panic!(
                "{rel}:{} `{}` に `#[cfg]` がある（#1599）。\n\
                 停止の経路を OS で割ると、片方の OS で撃つ部分だけが消えて後ろの掃除と成功が\n\
                 無条件に残る（#1599 以前の stale 経路の形）。OS の差は境界の中だけで持つ",
                line_of(source, start + at),
                signature.trim().trim_end_matches('('),
            );
        }
    }
}

/// `body` の中で `CORE` を呼ぶ文が `?` で結果を伝播していること。戻り値は呼び出しの byte 位置
fn core_call_propagates(body: &str, rel: &str, line: usize, name: &str) -> usize {
    let at = body.find(CORE).unwrap_or_else(|| {
        panic!(
            "{rel}:{line} `{name}` が停止の芯 `{CORE}…)` を通っていない（#1599）。\n\
             段（畳ませる → 終了要求 → 強制）と「止まらなければ Err」は芯の 1 実装に置く"
        )
    });
    let stmt_end = body[at..].find(';').map(|i| at + i).unwrap_or(body.len());
    let stmt = &body[at..stmt_end];
    assert!(
        stmt.trim_end().ends_with(")?"),
        "{rel}:{line} `{name}` が停止の芯の失敗を握り潰している（#1599）: `{}`\n\
         止められなかったのに state を消して `{{\"stopped\": true}}` を返す嘘になる",
        stmt.trim()
    );
    at
}

fn check_paths_propagate(source: &str, rel: &str) {
    let (sline, _, stale) = fn_body(source, rel, "\nfn kill_stale_daemon(");
    let call = core_call_propagates(stale, rel, sline, "kill_stale_daemon");
    let cleanup = stale.find("cleanup_state_files").unwrap_or(stale.len());
    assert!(
        call < cleanup,
        "{rel}:{sline} stale 経路が停止より先に state を掃除している（#1599）"
    );

    let (dline, _, normal) = fn_body(source, rel, "\nfn daemon_stop_impl(");
    // 正規経路の呼び出しは stale 分岐（`kill_stale_daemon(`）の後ろにある最後の 1 つ
    let call = normal.rfind(CORE).unwrap_or_else(|| {
        panic!("{rel}:{dline} `daemon_stop_impl` が停止の芯 `{CORE}…)` を通っていない（#1599）")
    });
    core_call_propagates(&normal[call..], rel, dline, "daemon_stop_impl");
    let ok = normal.rfind("Ok(json!").unwrap_or(0);
    assert!(
        call < ok,
        "{rel}:{dline} 正規経路が停止の芯より先に成功を返している（#1599）"
    );
}

// --------------------------------------------------------------- テスト

#[test]
fn remoteにlibc_killの直書きが無い() {
    check_no_libc_kill(&code(REMOTE), REMOTE);
}

#[test]
fn 停止の芯は境界の終了要求を通る() {
    check_core_uses_boundary(&code(REMOTE), REMOTE);
}

#[test]
fn 停止の経路にcfg属性を書かない() {
    check_no_cfg_in_stop_path(&code(REMOTE), REMOTE);
}

#[test]
fn 停止の2経路は芯の結果を伝播する() {
    check_paths_propagate(&code(REMOTE), REMOTE);
}

/// 番犬に検出力があること: 直前の 4 本が**壊れた形を実際に落とし、行を名指しする**。
///
/// 製品ソースを書き換える代わりに、壊れた形の断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 壊れた形を実際に落とす() {
    // #1599 以前の stale 経路そのもの（撃つ部分だけが #[cfg(unix)] の中）
    let legacy = "
fn daemon_stop_impl(force: bool) -> Result<Value, String> {
    kill_stale_daemon(pid)?;
    let by = stop_verified_daemon(pid_num, mode, &LiveStopMeans)?;
    Ok(json!({}))
}

fn kill_stale_daemon(pid: u32) -> Result<(), String> {
    if !verify_pid_identity(&info) {
        return Err(stale_stop_refusal(pid));
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    cleanup_state_files();
    Ok(())
}

fn stop_verified_daemon(pid: u32) -> Result<StoppedBy, String> {
    Ok(StoppedBy::Terminate)
}

fn request_then_wait(pid: u32) -> Result<bool, String> {
    Ok(true)
}
";
    let named = |f: &dyn Fn()| -> String {
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .expect_err("壊れた形を見逃した");
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    };
    // 1. 直書きの libc::kill を行で名指しする（断片の 15 行目）
    let msg = named(&|| check_no_libc_kill(legacy, "注入"));
    assert!(msg.starts_with("注入:15 "), "行を名指ししていない: {msg}");
    // 3. 撃つ部分の #[cfg(unix)] を名指しする（断片の 12 行目）
    let msg = named(&|| check_no_cfg_in_stop_path(legacy, "注入"));
    assert!(msg.starts_with("注入:12 "), "行を名指ししていない: {msg}");
    // 4. stale 経路が芯を通らない
    let msg = named(&|| check_paths_propagate(legacy, "注入"));
    assert!(msg.contains("kill_stale_daemon"), "{msg}");

    // 4'. 芯を通っても結果を捨てる（`let _ =`）形
    let swallowed = "
fn kill_stale_daemon(pid: u32) -> Result<StoppedBy, String> {
    let _ = stop_verified_daemon(pid, StopMode::Escalate, &LiveStopMeans);
    cleanup_state_files();
    Ok(StoppedBy::Terminate)
}
fn daemon_stop_impl(force: bool) -> Result<Value, String> {
    let by = stop_verified_daemon(pid_num, mode, &LiveStopMeans)?;
    Ok(json!({}))
}
";
    let msg = named(&|| check_paths_propagate(swallowed, "注入"));
    assert!(msg.contains("握り潰して"), "{msg}");

    // 2. 本番の手立てが境界を通らない（自前の kill へ戻した）形
    let bypass = "
impl StopMeans for LiveStopMeans {
    fn terminate(&self, pid: u32, force: bool) -> Result<(), String> {
        unsafe { libc::kill(pid as i32, 15) };
        Ok(())
    }
}

fn stop_verified_daemon(pid: u32) -> Result<StoppedBy, String> {
    Ok(StoppedBy::Terminate)
}
";
    let msg = named(&|| check_core_uses_boundary(bypass, "注入"));
    assert!(msg.contains("LiveStopMeans::terminate"), "{msg}");

    // 正しい形は通る（番犬が何でも落とす偽の赤ではない）
    let good = "
fn daemon_stop_impl(force: bool) -> Result<Value, String> {
    let by = kill_stale_daemon(pid)?;
    let by = stop_verified_daemon(pid_num, mode, &LiveStopMeans)?;
    Ok(json!({}))
}

fn kill_stale_daemon(pid: u32) -> Result<StoppedBy, String> {
    let by = stop_verified_daemon(pid, StopMode::Escalate, &LiveStopMeans)?;
    cleanup_state_files();
    Ok(by)
}

impl StopMeans for LiveStopMeans {
    fn terminate(&self, pid: u32, force: bool) -> Result<(), String> {
        crate::platform::process::terminate(pid, force)
    }
}

fn stop_verified_daemon(pid: u32) -> Result<StoppedBy, String> {
    Ok(StoppedBy::Terminate)
}

fn request_then_wait(pid: u32) -> Result<bool, String> {
    Ok(true)
}
";
    check_no_libc_kill(good, "注入");
    check_core_uses_boundary(good, "注入");
    check_no_cfg_in_stop_path(good, "注入");
    check_paths_propagate(good, "注入");
}
