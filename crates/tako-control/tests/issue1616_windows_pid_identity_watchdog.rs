//! **#1616 の番犬**: `verify_pid_identity` の**非 unix 側**が素通り（`true` へ直行）へ
//! 戻っていない。
//!
//! ## なぜ止めるのか
//!
//! 正体確認の中身はまるごと `#[cfg(unix)]` の中にあり、Windows は末尾の `true` へ
//! 直行していた = **「生きている pid はすべて tako の daemon」**と答えていた。
//! #329 / #1401 が入れた「pid を再利用した無関係なプロセスを撃たない」fail-safe が
//! Windows には存在しなかった。
//!
//! 事故になっていなかったのは**もう 1 つの穴がフタになっていた**からにすぎない:
//! `is_process_alive` が非 unix で無条件 `false`（#1557）だったので、先頭の `if` で
//! 必ず `false` を返していた。#1596 で生存判定を境界へ寄せたら素通りが露出し、
//! 停止の実体（`platform::process::terminate`）が Windows 実装される #1599 が
//! 先に入れば `tako remote stop` が無関係なプロセスを `TerminateProcess` する。
//!
//! ## 何を固定するか
//!
//! 規則そのもの（何を通し何を弾くか）は単体テストが持つ
//! （`tako_core::platform::procinfo` の `judge_identity` 系 6 本 /
//! `remote.rs` の `boundary_identity_confirmed…` 3 本）。ここが止めるのは**構造**で、
//! 1 行で戻せてしまう形が 4 通りある:
//!
//! 1. [`非unix側が境界の腕を通る`] — 腕の呼び出しが消えて `true` へ直行する
//! 2. [`正体確認にcfg属性を書かない`] — `#[cfg(unix)]` / `#[cfg(not(unix))]` へ戻る
//!    （腕が片方しかコンパイルされない = Windows 側の綴りが黙って腐る）
//! 3. [`境界の腕が判定を自前に持たない`] — `boundary_identity_confirmed` が
//!    `observe_identity` / `judge_identity` を通らなくなる・無条件 `true` を返す
//! 4. [`材料が無いときは撃たない`] — `IdentityVerdict::Unknown` を「確認済み」へ
//!    倒す（`confirmed()` の `matches!` に 1 語足すだけで戻る）

use std::path::{Path, PathBuf};

use tako_core::platform::procinfo::{
    judge_identity, ExpectedProcess, IdentityVerdict, ObservedProcess,
};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";

/// 非 unix 側の腕（呼び出し側）。綴りはこの 1 か所に持つ
const ARM: &str = "boundary_identity_confirmed";
/// 腕が通る境界。材料を引く側と突き合わせる側
const OBSERVE: &str = "observe_identity";
const JUDGE: &str = "judge_identity";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」（`#[cfg(test)]` の item とコメント・文字列を
/// 空白へ潰す。バイト長と行番号は保たれるので `file:line` の名指しがそのまま使える）。
///
/// コメントを潰すのはこの番犬に要る: 禁じている形（`#[cfg(not(unix))]` の素通り）は
/// 製品側の doc コメントに「こう書いてはいけなかった」として実在する
fn code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::code_view_of(&production_range::production(&src, rel))
}

/// 関数 1 本の本文（シグネチャ行から同インデントの `}` まで）。
/// 戻り値は `(シグネチャ行の行番号, 本文)`
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
    (head_line, source[start..end].to_string())
}

// --------------------------------------------------------------- 検査本体
// 注入テストからも呼ぶので、読むソースは引数で受ける

fn check_non_unix_arm(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\nfn verify_pid_identity(");
    let arm = body.find(ARM).unwrap_or_else(|| {
        panic!(
            "{rel}:{line} `verify_pid_identity` の非 unix 側が正体確認を通っていない（#1616）。\n\
             `{ARM}` を呼ばずに末尾の `true` へ落ちると、Windows では\n\
             **生きている pid をすべて tako の daemon と答える**（#1599 が入ると誤 kill）。本文:\n{body}"
        )
    });
    // 素通りの印: 材料を捨てるだけの腕（`let _ = info;`）が戻っていないこと
    assert!(
        !body.contains("let _ = info"),
        "{rel}:{line} 正体確認を捨てる腕（`let _ = info;`）が戻っている（#1616）。本文:\n{body}"
    );
    // 腕の呼び出しは `true` へ落ちる前に居ること（順序が入れ替わると意味が無い）
    let tail = body.rfind("\n    true").unwrap_or(body.len());
    assert!(
        arm < tail,
        "{rel}:{line} `{ARM}` が末尾の `true` より後ろにある（#1616）: 腕 = {arm} / true = {tail}"
    );
}

fn check_no_cfg_attribute(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\nfn verify_pid_identity(");
    assert!(
        !body.contains("#[cfg("),
        "{rel}:{line} 正体確認に `#[cfg]` の腕が戻っている（#1616）。\n\
         分岐は `cfg!(unix)` で書き、**両方の腕をどちらの OS でもコンパイルする**\n\
         （`#[cfg]` だと Windows 側の綴りが macOS のビルドで腐る）。本文:\n{body}"
    );
    assert!(
        body.contains("cfg!(unix)"),
        "{rel}:{line} OS ごとの腕の分岐（`cfg!(unix)`）が消えている（#1616）。本文:\n{body}"
    );
}

fn check_arm_delegates(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, &format!("\nfn {ARM}("));
    for needle in [OBSERVE, JUDGE] {
        assert!(
            body.contains(needle),
            "{rel}:{line} `{ARM}` が境界の `{needle}` を通っていない（#1616）。\n\
             材料の採り方（コマンドライン / 実行ファイル / 起動時刻）と突き合わせの規則は\n\
             `tako_core::platform::procinfo` の 1 実装に置く。本文:\n{body}"
        );
    }
    assert!(
        body.contains("confirmed()"),
        "{rel}:{line} `{ARM}` が結論を `confirmed()` で読んでいない（#1616）。\n\
         `Unknown`（確認できない）を撃ってよい側へ倒さないため。本文:\n{body}"
    );
    assert!(
        !body.contains("#[cfg("),
        "{rel}:{line} `{ARM}` に OS 分岐が戻っている（#1616）。\n\
         材料の可否は境界が答えるので、この関数は macOS でも動く形に保つ。本文:\n{body}"
    );
}

// --------------------------------------------------------------- テスト

#[test]
fn 非unix側が境界の腕を通る() {
    check_non_unix_arm(&code(REMOTE), REMOTE);
}

#[test]
fn 正体確認にcfg属性を書かない() {
    check_no_cfg_attribute(&code(REMOTE), REMOTE);
}

#[test]
fn 境界の腕が判定を自前に持たない() {
    check_arm_delegates(&code(REMOTE), REMOTE);
}

/// 材料が 1 つも引けなければ「確認できない」= 撃たない（#1616 の fail-safe 既定）。
///
/// 規則の全文は `procinfo` の単体テストが持つが、**この向きだけは 1 語で反転できる**
/// （`confirmed()` の `matches!` に `| Self::Unknown` を足す）ので実関数で固定する
#[test]
fn 材料が無いときは撃たない() {
    let expected = ExpectedProcess {
        exe: Some("/usr/local/bin/tako"),
        start_time_unix: Some(1_700_000_000),
    };
    let verdict = judge_identity(&ObservedProcess::default(), expected, |_| true);
    assert_eq!(
        verdict,
        IdentityVerdict::Unknown,
        "材料が引けないのは Confirmed ではない"
    );
    assert!(
        !verdict.confirmed(),
        "Unknown で撃ってはいけない（#1616 / #329 / #1401 の fail-safe 既定）"
    );
    assert!(
        !IdentityVerdict::Mismatch.confirmed(),
        "Mismatch でも撃ってはいけない"
    );
}

/// 番犬に検出力があること: 直前の 3 本が**#1616 以前の形を実際に落とす**。
///
/// 製品ソースを書き換える代わりに、素通りの断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 素通りの形を実際に落とす() {
    // ① #1616 以前そのもの（正体確認が `#[cfg(unix)]` の中だけにあり、非 unix は
    //    材料を捨てて末尾の `true` へ直行する）
    let legacy = "
fn verify_pid_identity(info: &PidInfo) -> bool {
    if !is_process_alive(info.pid) {
        return false;
    }
    #[cfg(unix)]
    {
        if !command_line_is_tako_remote_serve(&ps_args(info.pid)) {
            return false;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = info;
    }
    true
}
";
    assert!(
        std::panic::catch_unwind(|| check_non_unix_arm(legacy, "注入")).is_err(),
        "① 非 unix 側の素通りを見逃した"
    );
    assert!(
        std::panic::catch_unwind(|| check_no_cfg_attribute(legacy, "注入")).is_err(),
        "① `#[cfg]` の腕を見逃した"
    );

    // ② 腕はあるが `#[cfg(windows)]` で撒いた形（macOS のビルドで綴りが腐る）
    let cfg_arm = "
fn verify_pid_identity(info: &PidInfo) -> bool {
    #[cfg(windows)]
    {
        if !boundary_identity_confirmed(info) {
            return false;
        }
    }
    true
}
";
    assert!(
        std::panic::catch_unwind(|| check_no_cfg_attribute(cfg_arm, "注入")).is_err(),
        "② `#[cfg(windows)]` で撒いた形を見逃した"
    );

    // ③ 腕が境界を通らず自前判定へ戻る
    let own_rule = "
fn boundary_identity_confirmed(info: &PidInfo) -> bool {
    #[cfg(windows)]
    {
        return tako_core::platform::procinfo::snapshot()
            .iter()
            .any(|p| p.pid == info.pid);
    }
    true
}
";
    assert!(
        std::panic::catch_unwind(|| check_arm_delegates(own_rule, "注入")).is_err(),
        "③ 自前判定へ戻った腕を見逃した"
    );

    // ④ 現行の形は通る（常に落ちる無意味な番犬になっていないこと）
    let ok = "
fn verify_pid_identity(info: &PidInfo) -> bool {
    if !is_process_alive(info.pid) {
        return false;
    }
    if cfg!(unix) {
        if !command_line_is_tako_remote_serve(&ps_args(info.pid)) {
            return false;
        }
    } else if !boundary_identity_confirmed(info) {
        return false;
    }
    true
}

fn boundary_identity_confirmed(info: &PidInfo) -> bool {
    judge_identity(
        &observe_identity(info.pid),
        ExpectedProcess {
            exe: info.exe.as_deref(),
            start_time_unix: info.start_time,
        },
        command_line_is_tako_remote_serve,
    )
    .confirmed()
}
";
    check_non_unix_arm(ok, "注入");
    check_no_cfg_attribute(ok, "注入");
    check_arm_delegates(ok, "注入");
}
