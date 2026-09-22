//! **#1557 / #1581 の番犬**: 「その pid は生きているか」の判定が
//! 境界 `tako_core::platform::process::pid_alive` の 1 実装を通る。
//!
//! ## なぜ止めるのか
//!
//! 生存判定は **Windows 実装のある境界**（`procinfo::snapshot` の在籍）が既にあるのに、
//! 呼び出し側が自前で `libc::kill(pid, 0)` を書くと `#[cfg(not(unix))]` の腕が要り、
//! そこは決まって「無条件 `false`」か「無条件 `true`」になる。どちらも
//! **Windows でだけ静かに全件を誤る**形で、単体テストは緑のまま通る:
//!
//! - `remote.rs` の `is_process_alive` は非 unix で無条件 `false` だった（#1557）。
//!   生きている daemon を全部「居ない」と読むので、`daemon_status` の `running` も
//!   pid 再利用時の誤 kill 防止も残骸 daemon の掃除判断も揃って誤る。
//!   `is_process_aliveは存在しないpidをfalseで返す` は**偽の緑**でもあった
//!   （何を渡しても `false` なので必ず通る）
//! - `ports::process_alive` は逆向きの罠で、非 unix で無条件 `true` を返す。
//!   あれは **tmux ソケットの回収**用に「何も回収しない側へ倒す」ための設計（#1253）なので、
//!   残骸の掃除（`test_residue`）の生存判定に流用すると Windows で 1 件も掃けなくなる
//!
//! ## 何を固定するか
//!
//! 規則そのもの（EPERM は「居る」・`pid_t` の範囲外は「居ない」）は
//! `tako_core::platform::process` の単体テストが持つ。ここが止めるのは**構造**で、
//! 1 行で戻せてしまう形が 4 通りある:
//!
//! 1. [`remoteの生存判定は境界へ委譲する`] — `is_process_alive` が自前実装へ戻る
//! 2. [`remoteにsignal0の直書きが無い`] — 別の場所へ `libc::kill(pid, 0)` が生える
//! 3. [`test_residueの生存判定は境界へ委譲する`] — `OwnerProbe` が判定を持ち直す
//! 4. [`test_residueはports_process_aliveを生存判定に使わない`] — 常に `true` の関数を掴む
//!
//! 実際の停止（`libc::kill(pid, SIGTERM)` / `SIGKILL`）は**別の話**なので当たらない。
//! 見るのは第 2 引数が `0` = シグナルを送らない生存確認の形だけ。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const RESIDUE: &str = "crates/tako-core/src/test_residue.rs";

/// 生存判定の寄せ先（境界）。綴りはこの 1 か所に持つ
const BOUNDARY: &str = "platform::process::pid_alive";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」を返す。
///
/// 2 段で潰す。どちらも**バイト長と行番号が保たれる**ので `file:line` の名指しがそのまま使える:
///
/// 1. `#[cfg(test)]` の付いた item（#1420 の作法。**切らずに**潰す）
/// 2. コメントと文字列リテラル（`code_view`）。**この番犬にはこれが要る**:
///    禁じている綴り（`libc::kill(pid, 0)` / `ports::process_alive`）は、
///    製品側の doc コメントに「こう書いてはいけない」として実在する
///
/// 下限は既定（`MIN_COVERAGE` = 30%）のまま。実測の本番コード比は
/// `remote.rs` 71.6% / `test_residue.rs` 64.0% で、どちらも余裕がある
fn code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    let production = production_range::production(&src, rel);
    production_range::code_view_of(&production)
}

/// 関数 1 本の本文（シグネチャ行から同インデントの `}` まで）。
/// 戻り値は `(シグネチャ行の行番号, 本文)`。渡すのは [`code`] が返す眺めなので、
/// コメントと文字列は既に空白へ潰れている
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

/// `libc::kill(…, 0)`（= シグナルを送らない生存確認）の行番号。
///
/// 停止のための `libc::kill(pid, libc::SIGTERM)` は**当たらない**。
/// 第 2 引数が素の `0` のものだけを数える
fn signal_zero_kills(source: &str) -> Vec<usize> {
    const HEAD: &str = "libc::kill(";
    let mut hits = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = source[cursor..].find(HEAD) {
        let at = cursor + at;
        let open = at + HEAD.len() - 1;
        cursor = at + HEAD.len();
        let Some(close) = matching_paren(source, open) else {
            continue;
        };
        let args = split_top(&source[open + 1..close]);
        if args.last().map(|a| a.trim()) == Some("0") {
            hits.push(source[..at].lines().count());
        }
    }
    hits
}

/// `open` の `(` に対応する `)`
fn matching_paren(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
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

/// 深さ 0 のカンマで割る
fn split_top(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts.into_iter().map(str::trim).collect()
}

// --------------------------------------------------------------- 検査本体
// 注入テストからも呼ぶので、読むソースは引数で受ける

fn check_remote_delegates(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\nfn is_process_alive(");
    assert!(
        body.contains(BOUNDARY),
        "{rel}:{line} `is_process_alive` が境界 `{BOUNDARY}` を通っていない（#1557）。\n\
         自前判定は `#[cfg(not(unix))]` の腕を要求し、そこは無条件 false へ倒れる\n\
         = Windows で生きている daemon を全部「居ない」と読む。本文:\n{body}"
    );
    assert!(
        !body.contains("libc::"),
        "{rel}:{line} `is_process_alive` に libc の直書きが戻っている（#1557）。\n\
         判定を 2 実装持たない。本文:\n{body}"
    );
    assert!(
        !body.contains("#[cfg("),
        "{rel}:{line} `is_process_alive` に OS 分岐が戻っている（#1557）。\n\
         プラットフォーム差は境界 `{BOUNDARY}` の中だけで持つ。本文:\n{body}"
    );
}

fn check_no_signal_zero(source: &str, rel: &str) {
    let hits = signal_zero_kills(source);
    assert!(
        hits.is_empty(),
        "{rel}:{} 生存確認の `libc::kill(…, 0)` が直書きされている（#1557）。\n\
         → `tako_core::{BOUNDARY}(pid)` を呼ぶこと（Windows 実装つき・\n\
         EPERM と pid_t 範囲外の扱いもそちらが持つ）",
        hits.iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(" / ")
    );
}

fn check_residue_delegates(source: &str, rel: &str) {
    let (aline, abody) = fn_body(source, rel, "\n    fn alive(&self, pid: u32) -> bool {");
    assert!(
        abody.contains(BOUNDARY),
        "{rel}:{aline} `OwnerProbe::alive` が境界 `{BOUNDARY}` を通っていない（#1581）。\n\
         Windows 側は同じ材料（procinfo のスナップショット）のキャッシュ版でよいが、\n\
         非 Windows は境界をそのまま呼ぶこと。本文:\n{abody}"
    );
    // 判定を `owner` の中へ書き戻すと、また `#[cfg]` ごとに 2 実装へ割れる
    let (oline, obody) = fn_body(
        source,
        rel,
        "\n    pub fn owner(&self, pid: u32) -> Owner {",
    );
    assert!(
        obody.contains("self.alive("),
        "{rel}:{oline} `OwnerProbe::owner` が生存判定を自前に持ち直している（#1581）。\n\
         判定は `OwnerProbe::alive` の 1 か所に置く。本文:\n{obody}"
    );
}

fn check_residue_not_ports(source: &str, rel: &str) {
    let Some(at) = source.find("ports::process_alive") else {
        return;
    };
    let line = source[..at].lines().count();
    panic!(
        "{rel}:{line} 生存判定に `ports::process_alive` を使っている（#1581）。\n\
         あれは tmux ソケットの回収用で、**非 unix では常に true**（#1253 で\n\
         「何も回収しない側へ倒す」ための設計）。残骸の掃除に流用すると\n\
         Windows で 1 件も掃けない。→ `tako_core::{BOUNDARY}(pid)` を使うこと"
    );
}

// --------------------------------------------------------------- テスト

#[test]
fn remoteの生存判定は境界へ委譲する() {
    check_remote_delegates(&code(REMOTE), REMOTE);
}

#[test]
fn remoteにsignal0の直書きが無い() {
    check_no_signal_zero(&code(REMOTE), REMOTE);
}

#[test]
fn test_residueの生存判定は境界へ委譲する() {
    check_residue_delegates(&code(RESIDUE), RESIDUE);
}

#[test]
fn test_residueはports_process_aliveを生存判定に使わない() {
    check_residue_not_ports(&code(RESIDUE), RESIDUE);
}

/// 番犬に検出力があること: 直前の 4 本が**壊れた形を実際に落とす**。
///
/// 製品ソースを書き換える代わりに、壊れた形の断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 壊れた形を実際に落とす() {
    let legacy_remote = "
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}
";
    assert!(
        std::panic::catch_unwind(|| check_remote_delegates(legacy_remote, "注入")).is_err(),
        "#1557 以前の自前実装を見逃した"
    );
    assert!(
        std::panic::catch_unwind(|| check_no_signal_zero(legacy_remote, "注入")).is_err(),
        "signal 0 の直書きを見逃した"
    );
    // 停止の kill は当たらない（当たると停止経路を書けなくなる）
    let terminate = "unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }";
    check_no_signal_zero(terminate, "注入");

    let legacy_residue = "
    pub fn owner(&self, pid: u32) -> Owner {
        #[cfg(windows)]
        let alive = self.live.contains(&pid);
        #[cfg(not(windows))]
        let alive = crate::ports::process_alive(pid);
        if !alive {
            return Owner::Dead;
        }
        Owner::Unknown
    }
";
    assert!(
        std::panic::catch_unwind(|| check_residue_delegates(legacy_residue, "注入")).is_err(),
        "判定を owner へ書き戻した形を見逃した"
    );
    assert!(
        std::panic::catch_unwind(|| check_residue_not_ports(legacy_residue, "注入")).is_err(),
        "常に true の `ports::process_alive` を見逃した"
    );
}
