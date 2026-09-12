//! **#1401 の番犬**: `tako remote stop` の **2 本の kill 経路の両方**が
//! PID の正体確認を通る。
//!
//! ## なぜ止めるのか
//!
//! 停止には ①PID ファイルの PID を撃つ正規経路 ②PID ファイルが消えていて
//! `/api/health` が答えた PID を撃つ stale 経路 がある。#329 で①に
//! 「`tako remote serve` だと確認できなければ撃たない」fail-safe を入れたが、
//! **②は素通り**だった。記録済みポートを別プロセスが再利用していれば、
//! `tako remote stop` が無関係なプロセスへ SIGTERM → 5 秒後 SIGKILL を撃つ。
//!
//! 実測（修正前の実 CLI・隔離 state）: 偽 health に使い捨て `/bin/sleep` の PID を
//! 載せると `{"stopped": true, "stale_pid": …}` を返して sleep が消えた。health が
//! **停止操作をしているプロセス自身**の PID を返す形では、`cargo test` の
//! テストバイナリが `signal: 15` で落ちた（#329 の flaky と同じ壊れ方）。
//!
//! ## 何を固定するか
//!
//! 規則そのもの（何を通し何を弾くか）は `remote.rs` の単体テストが持つ
//! （`daemon_stop_implはstale経路でも正体不明のpidをkillしない` /
//! `ps出力が空なら確認できない扱いになる` ほか）。ここが止めるのは**構造**で、
//! 1 行で戻せてしまう形が 5 通りある:
//!
//! 1. [`kill_stale_daemonはシグナルより先に正体確認する`] — 確認が消える / 順序が入れ替わる
//! 2. [`kill_stale_daemonは結果型で拒否を返す`] — 戻り値を捨てる形へ戻る
//! 3. [`stale経路は停止結果を伝播する`] — 呼び出し側が `?` を外して握り潰す
//! 4. [`ps出力の判定は1実装で空を弾く`] — 空出力の素通り（#1401 の補足）が戻る
//! 5. [`確認できないときstateを消さない`] — 拒否の前に state を掃除してしまう
//!
//! 併せて、中止の文言が 1 実装から組まれていること
//! （[`中止の文言は1実装から組む`]）も見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コード（`#[cfg(test)]` の付いた item を空白へ潰した眺め）を返す。
///
/// **切らない**理由は #1420: 旧実装は「最初の `#[cfg(test)]` まで」で切っていたので、
/// `remote.rs` の途中にテスト用ヘルパを 1 つ置いただけで走査範囲が 3,124 行へ縮み、
/// 検査対象を見失ったまま**緑のまま通った**（実測）。潰す形なら消えるのはヘルパだけで、
/// 範囲が想定より縮んだら [`production_range::production`] 自身が `file:line` で落ちる
fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// 関数 1 本の本文（シグネチャ行から同インデントの `}` まで）。
/// 戻り値は `(シグネチャ行の行番号, コメントを落とした本文)`。
/// コメントを落とすのは、説明文中の `libc::kill` で落ちないようにするため
fn fn_body(source: &str, rel: &str, signature: &str) -> (usize, String) {
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
    let body = source[start..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    (head_line, body)
}

/// 検査本体（注入テストからも呼ぶので、読むソースを引数で受ける）。
/// `rel` / `line` は落ちたときの file:line 名指しに使う
fn check_kill_stale_daemon(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\nfn kill_stale_daemon(");
    let verify = body.find("verify_pid_identity").unwrap_or_else(|| {
        panic!(
            "{rel}:{line} `kill_stale_daemon` が正体確認を通っていない（#1401）。\n\
             health が答えた PID を素で撃つと、ポートを再利用した無関係プロセスを殺す"
        )
    });
    let refusal = body.find("return Err(").unwrap_or_else(|| {
        panic!("{rel}:{line} 確認できなかったときに撃たずに返す経路が無い（#1401）")
    });
    for signal in [
        "libc::kill",
        "platform::process::terminate",
        "SIGTERM",
        "SIGKILL",
    ] {
        if let Some(at) = body.find(signal) {
            assert!(
                verify < at && refusal < at,
                "{rel}:{line} `kill_stale_daemon` の {signal} が正体確認より前にある（#1401）。\n\
                 確認 = {verify} / 拒否 = {refusal} / {signal} = {at}（本文内の byte 位置）"
            );
        }
    }
    // 拒否して返る前に state を消さない: 応答している相手が本物の daemon かもしれない
    // うちに記録を消すと、生きている daemon を到達不能にする
    assert!(
        !body[..refusal].contains("cleanup_state_files"),
        "{rel}:{line} 正体を確認できないまま state を掃除している（#1401 / #445）"
    );
}

/// 停止経路が `kill_stale_daemon` の結果を捨てていないこと
fn check_call_site(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\nfn daemon_stop_impl(");
    let at = body
        .find("kill_stale_daemon(")
        .unwrap_or_else(|| panic!("{rel}:{line} stale 経路の停止呼び出しが見つからない"));
    let tail = &body[at..];
    let call_end = tail.find('\n').unwrap_or(tail.len());
    let call = &tail[..call_end];
    assert!(
        call.contains(")?;") || call.contains(")?"),
        "{rel}:{line} stale 経路が停止の結果を握り潰している（#1401）: {call}\n\
         撃たずに中止した理由は呼び出し側へ返さないと、`{{\"stopped\": true}}` の嘘になる"
    );
}

#[test]
fn kill_stale_daemonはシグナルより先に正体確認する() {
    check_kill_stale_daemon(&production(REMOTE), REMOTE);
}

#[test]
fn 確認できないときstateを消さない() {
    // check_kill_stale_daemon の中で見る（順序と掃除は同じ本文を読むため 1 実装）
    check_kill_stale_daemon(&production(REMOTE), REMOTE);
}

#[test]
fn kill_stale_daemonは結果型で拒否を返す() {
    let src = production(REMOTE);
    let at = src
        .find("\nfn kill_stale_daemon(")
        .expect("kill_stale_daemon が見つからない");
    let line = src[..at].lines().count() + 2;
    let sig_end = src[at..].find('{').map(|i| at + i).expect("本文の開き括弧");
    let sig = &src[at..sig_end];
    assert!(
        sig.contains("-> Result<"),
        "{REMOTE}:{line} `kill_stale_daemon` が結果を返さない形に戻っている（#1401）: {}",
        sig.trim()
    );
}

#[test]
fn stale経路は停止結果を伝播する() {
    check_call_site(&production(REMOTE), REMOTE);
}

#[test]
fn ps出力の判定は1実装で空を弾く() {
    let src = production(REMOTE);
    let (vline, vbody) = fn_body(&src, REMOTE, "\nfn verify_pid_identity(");
    assert!(
        vbody.contains("ps_args_is_tako_remote_serve"),
        "{REMOTE}:{vline} `verify_pid_identity` が ps 出力の判定を再実装している（#1401）"
    );
    assert!(
        !vbody.contains("!cmd.is_empty() && !is_tako_remote"),
        "{REMOTE}:{vline} 空の ps 出力を素通りさせる旧判定に戻っている（#1401）"
    );
    let (pline, pbody) = fn_body(&src, REMOTE, "\nfn ps_args_is_tako_remote_serve(");
    assert!(
        pbody.contains("!cmd.is_empty()"),
        "{REMOTE}:{pline} `ps_args_is_tako_remote_serve` が空出力を弾いていない（#1401）"
    );
    for token in ["\"tako\"", "\"remote\"", "\"serve\""] {
        assert!(
            pbody.contains(token),
            "{REMOTE}:{pline} 判定から {token} が消えている（#1401）"
        );
    }
}

#[test]
fn 中止の文言は1実装から組む() {
    let src = production(REMOTE);
    let (sline, sbody) = fn_body(&src, REMOTE, "\nfn stale_stop_refusal(");
    assert!(
        sbody.contains("pid_identity_refusal"),
        "{REMOTE}:{sline} stale 経路の中止文言が共通の前置きを通っていない（#1401 / #329）"
    );
    for step in ["tako remote start", "tasklist", "ps -p"] {
        assert!(
            sbody.contains(step),
            "{REMOTE}:{sline} 中止の説明から手順 `{step}` が消えている（#1401 の受け入れ条件 1）"
        );
    }
    let (dline, dbody) = fn_body(&src, REMOTE, "\nfn daemon_stop_impl(");
    assert!(
        dbody.contains("pid_identity_refusal"),
        "{REMOTE}:{dline} 正規経路の中止文言が共通の前置きを通っていない（#329）"
    );
}

/// 走査範囲が本番コード全体を覆っていること（#1420）。
///
/// 旧実装は「最初の `#[cfg(test)]` まで」で切っていたので、`remote.rs` の途中に
/// テスト用ヘルパを 1 つ置くと検査対象を見失った。**しかも落ち方が
/// 「`fn kill_stale_daemon(` が見つからない（改名したら番犬も直す）」**なので、
/// 読んだ人は改名を疑ってヘルパの側を避けてしまう（#1403 で実際にそうなった）。
/// 検査対象の直前へヘルパを注入して、6 本が同じ結論へ届くことを固定する
#[test]
fn 途中のテスト用ヘルパで走査範囲が消えない() {
    let src = std::fs::read_to_string(repo_root().join(REMOTE)).expect("remote.rs を読む");
    let at = src
        .find("\nfn kill_stale_daemon(")
        .expect("注入点（検査対象の直前）が見つからない");
    let injected = format!(
        "{}\n#[cfg(test)]\nfn 注入したテスト用ヘルパ() -> u32 {{\n    0\n}}\n{}",
        &src[..at],
        &src[at..]
    );

    // 新実装: ヘルパだけが消え、検査は全部届く
    let view = production_range::production(&injected, REMOTE);
    check_kill_stale_daemon(&view, REMOTE);
    check_call_site(&view, REMOTE);
    assert!(
        !view.contains("注入したテスト用ヘルパ"),
        "テスト用ヘルパが走査範囲に残っている"
    );

    // 旧アーム: 同じ入力で見失うこと（測れないなら この回帰テストに意味が無い）
    // 目印は切り出しと**同じ行**に置く（#1420 の番犬は行単位で見る）
    let cut = injected.find("\n#[cfg(test)]"); // #1420-legacy-arm
    let legacy_cut = cut.expect("旧実装の切れ目");
    assert!(
        !injected[..legacy_cut].contains("\nfn kill_stale_daemon("),
        "旧実装が見失う形になっていない = A/B が成立していない"
    );
}

/// 検査に検出力があること自体を固定する（#1401 で実際に在った形を注入する）
#[test]
fn 検査は修正前の形を検出する() {
    // ① 正体確認なしで撃つ（修正前の kill_stale_daemon そのもの）
    let legacy = "\nfn kill_stale_daemon(pid: u32) {\n\
        \x20   unsafe {\n\
        \x20       libc::kill(pid as libc::pid_t, libc::SIGTERM);\n\
        \x20   }\n\
        \x20   cleanup_state_files();\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_kill_stale_daemon(legacy, "注入"));
    assert!(caught.is_err(), "① 正体確認なしの kill を検出できていない");

    // ② 確認より前に撃つ（順序の入れ替え）
    let reordered = "\nfn kill_stale_daemon(pid: u32) -> Result<(), String> {\n\
        \x20   unsafe {\n\
        \x20       libc::kill(pid as libc::pid_t, libc::SIGTERM);\n\
        \x20   }\n\
        \x20   if !verify_pid_identity(&info) {\n\
        \x20       return Err(stale_stop_refusal(pid));\n\
        \x20   }\n\
        \x20   Ok(())\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_kill_stale_daemon(reordered, "注入"));
    assert!(caught.is_err(), "② 確認より前の kill を検出できていない");

    // ③ 拒否の前に state を掃除する
    let cleans = "\nfn kill_stale_daemon(pid: u32) -> Result<(), String> {\n\
        \x20   cleanup_state_files();\n\
        \x20   if !verify_pid_identity(&info) {\n\
        \x20       return Err(stale_stop_refusal(pid));\n\
        \x20   }\n\
        \x20   Ok(())\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_kill_stale_daemon(cleans, "注入"));
    assert!(caught.is_err(), "③ 拒否前の state 掃除を検出できていない");

    // ④ 呼び出し側が結果を握り潰す（修正前の呼び出し）
    let swallowed = "\nfn daemon_stop_impl(force: bool) -> Result<Value, String> {\n\
        \x20   kill_stale_daemon(pid as u32);\n\
        \x20   Ok(json!({}))\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_call_site(swallowed, "注入"));
    assert!(caught.is_err(), "④ 握り潰しを検出できていない");

    // ⑤ 現行の形は通る（検査が常に落ちる無意味な番犬になっていないこと）
    let ok = "\nfn kill_stale_daemon(pid: u32) -> Result<(), String> {\n\
        \x20   if !verify_pid_identity(&info) {\n\
        \x20       return Err(stale_stop_refusal(pid));\n\
        \x20   }\n\
        \x20   unsafe {\n\
        \x20       libc::kill(pid as libc::pid_t, libc::SIGTERM);\n\
        \x20   }\n\
        \x20   cleanup_state_files();\n\
        \x20   Ok(())\n\
        }\n";
    check_kill_stale_daemon(ok, "注入");
}
