//! 番犬: **使い捨て dir を作る経路は、消す経路も持つ**（Issue #1296）
//!
//! #944 / #1253 は「検証プロセスの書き先を `<TMPDIR>/<名前>-<pid>` へ倒す」ところまでで
//! 止まっていた。macOS の `TMPDIR`（`/var/folders/…/T/`）は再起動でも消えないので、
//! `cargo test` 1 回につき 1 dir が残り続ける（実測 2026-09-11:
//! `tako-test-data-*` が 2,188 件 + `tako-agent-config-*` が 784 件）。
//!
//! 落とすのは 3 つの形:
//!
//! 1. `temp_dir().join(format!("…{}", std::process::id()))` で作っているのに
//!    [`tako_core::test_residue::arm_self_cleanup`] を呼んでいない関数
//!    （= 作る経路が消す経路を持たない = #1296 の再発）
//! 2. その接頭辞が `test_residue::KINDS` に載っていない
//!    （= 手動の掃除口 `tako test-residue` から見えない置き場が増える）
//! 3. `test_residue.rs` の中で、**判定を通さない `remove_dir_all`** が増えること
//!    （名前の一致だけで消すと、並行して走る別 worker の `cargo test` の
//!    置き場を巻き込む = #625 の事故クラス）

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// 走査するのは**置き場を決める側**（`paths.rs`）だけ。
///
/// 規約「テストの書き先は本番の外」（#944 / #1253）で、検証プロセスの
/// 置き場を決めるのはこの 1 ファイルと決まっている。テスト本体が作る
/// 使い捨ての作業ディレクトリ（`tako-git-496-…` 等）は**そのテストが自分で
/// 片付ける前提**の別物なので、ここでは見ない（見ると本題が埋もれる）
fn isolation_source() -> PathBuf {
    repo_root().join("crates/tako-core/src/paths.rs")
}

/// 使い捨て dir を作っている 1 か所
#[derive(Debug)]
struct Site {
    file: String,
    /// 囲んでいる関数の名前
    func: String,
    /// 作られる名前の接頭辞（`tako-test-data-`）
    prefix: String,
    /// 囲んでいる関数の本体
    body: String,
}

/// **自動の後始末を付けない**と決めた置き場（理由つき）。
/// 増やすときは「なぜプロセス終了時に消さなくてよいか」を書くこと
const EXEMPT: &[(&str, &str)] = &[(
    "verification_agent_home",
    "製品バイナリの検証起動（TAKO_ISOLATED / TAKO_SELF_TEST）も作るので、\
     終了時削除を入れると製品の挙動が変わる（#1253 の射程）。\
     手動の掃除口（tako test-residue）からは消せる = KINDS には載せてある",
)];

/// 「`temp_dir().join(format!("…-{}", std::process::id()))`」で
/// 使い捨て dir を作っている箇所を、囲んでいる関数ごと拾う
fn disposable_dir_sites() -> Vec<Site> {
    let path = isolation_source();
    let src = std::fs::read_to_string(&path).expect("paths.rs を読める");
    let file = path
        .strip_prefix(repo_root())
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut out = Vec::new();
    for (idx, _) in src.match_indices("std::env::temp_dir().join(format!(") {
        let tail = &src[idx..];
        let Some(end) = tail.find("));") else {
            continue;
        };
        let call = &tail[..end];
        if !call.contains("std::process::id()") {
            continue; // pid で分けていない一時ファイルは対象外
        }
        let Some(prefix) = call
            .split_once('"')
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(lit, _)| lit.replace("{}", ""))
        else {
            continue;
        };
        let (func, body) = enclosing_fn(&src, idx);
        out.push(Site {
            file: file.clone(),
            func,
            prefix,
            body,
        });
    }
    out
}

/// `at` を含む**トップレベル関数**の名前と本体（先頭が `fn ` / `pub fn ` の行から、
/// 桁 0 の `}` まで。`rustfmt` がこの形を保証する）
fn enclosing_fn(src: &str, at: usize) -> (String, String) {
    let head = src[..at]
        .rmatch_indices('\n')
        .map(|(i, _)| i + 1)
        .find(|&i| is_fn_head(&src[i..]))
        .unwrap_or(0);
    let name = src[head..]
        .split_once("fn ")
        .and_then(|(_, rest)| rest.split(['(', '<']).next())
        .unwrap_or("?")
        .trim()
        .to_string();
    let end = src[head..]
        .find("\n}\n")
        .map(|i| head + i)
        .unwrap_or(src.len());
    (name, src[head..end].to_string())
}

/// 桁 0 から始まる関数定義の頭か。`extern "C" fn` / `pub(crate) fn` を
/// 取りこぼすと**中の削除が別の関数のものに見える**（誤検知の原因）
fn is_fn_head(line: &str) -> bool {
    const HEADS: &[&str] = &[
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "const fn ",
        "unsafe fn ",
        "async fn ",
        "extern ",
        "pub extern ",
        "unsafe extern ",
    ];
    HEADS.iter().any(|h| line.starts_with(h)) && line.contains("fn ")
}

/// 1: 作る経路は消す経路を持つ
#[test]
fn 使い捨てdirを作る関数は終了時の後始末を武装する() {
    let sites = disposable_dir_sites();
    assert!(
        sites.len() >= 2,
        "走査が当たっていない（temp_dir + pid の生成箇所が {} 件）",
        sites.len()
    );
    let mut missing = Vec::new();
    for site in &sites {
        if EXEMPT.iter().any(|(func, _)| *func == site.func) {
            continue;
        }
        if !site.body.contains("test_residue::arm_self_cleanup") {
            missing.push(format!(
                "{}: fn {}（{}<pid>）が arm_self_cleanup を呼んでいない",
                site.file, site.func, site.prefix
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "使い捨て dir を作るのに終了時の後始末が無い（#1296 の再発）:\n{}\n\
         直し方: 作った直後に tako_core::test_residue::arm_self_cleanup(&dir) を呼ぶ。\
         意図して付けないなら EXEMPT へ理由つきで載せる",
        missing.join("\n")
    );
}

/// 2: どの置き場も手動の掃除口から見える
#[test]
fn 使い捨てdirの接頭辞はkindsに載っている() {
    let known: Vec<&str> = tako_core::test_residue::KINDS
        .iter()
        .map(|k| k.prefix)
        .collect();
    let mut unknown = Vec::new();
    for site in disposable_dir_sites() {
        if !known.contains(&site.prefix.as_str()) {
            unknown.push(format!(
                "{}: fn {} → {}<pid>",
                site.file, site.func, site.prefix
            ));
        }
    }
    assert!(
        unknown.is_empty(),
        "test_residue::KINDS に無い使い捨て dir がある（tako test-residue から掃けない）:\n{}\n\
         既知: {known:?}",
        unknown.join("\n")
    );
}

/// 3: 判定を通さない削除を増やさない
#[test]
fn 残骸の削除は判定を通した経路だけ() {
    let path = repo_root().join("crates/tako-core/src/test_residue.rs");
    let src = std::fs::read_to_string(&path).expect("test_residue.rs を読める");
    // 見るのは製品コードだけ（単体テストは自分で作った使い捨てを自分で片付ける）
    let src = src
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map(|(head, _)| head.to_string())
        .expect("test_residue.rs に単体テストのモジュールが無い");
    let mut bad = Vec::new();
    for (idx, _) in src.match_indices("remove_dir_all(") {
        let (func, _) = enclosing_fn(&src, idx);
        // 消してよいのは「自分の dir」と「再判定を通した残骸」の 2 経路だけ
        if !matches!(func.as_str(), "remove_own_dirs" | "remove_if_still_stale") {
            bad.push(func);
        }
    }
    assert!(
        bad.is_empty(),
        "判定を通さない削除がある（名前の一致だけで消すと生きている別 worker の \
         cargo test を巻き込む）: {bad:?}"
    );
    assert!(
        src.contains("fn remove_if_still_stale("),
        "消す直前の再判定（remove_if_still_stale）が消えている"
    );
}

/// 4: 起動時の掃除が作る経路から呼ばれている（案 2 が配線されたまま）
#[test]
fn テストのdata_dirを作る経路が起動時の掃除を呼ぶ() {
    let path = repo_root().join("crates/tako-core/src/paths.rs");
    let src = std::fs::read_to_string(&path).expect("paths.rs を読める");
    let idx = src
        .find("fn test_data_dir()")
        .expect("fn test_data_dir が無い");
    let (_, body) = enclosing_fn(&src, idx);
    assert!(
        body.contains("test_residue::sweep_stale_on_start"),
        "test_data_dir が起動時の掃除を呼んでいない（SIGKILL された回の残骸が永遠に残る）:\n{body}"
    );
}
