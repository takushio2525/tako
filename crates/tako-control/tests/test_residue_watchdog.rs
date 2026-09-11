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

/// 走査するのは**置き場を決める側**だけ。
///
/// 規約「テストの書き先は本番の外」（#944 / #1253）で、検証プロセスの
/// 置き場を決めるのはこれらのファイルと決まっている。テスト本体が作る
/// 使い捨ての作業ディレクトリは**そのテストが自分で片付ける前提**の別物で、
/// そちらは [`tako_core::test_residue::ScratchDir`]（#1312）と
/// 下の `使い捨ての器はスコープで消える` が受け持つ
fn isolation_sources() -> Vec<PathBuf> {
    [
        "crates/tako-core/src/paths.rs",
        "crates/tako-control/src/orchestrator/mod.rs",
        "crates/tako-control/src/orchestrator/supervisor.rs",
    ]
    .iter()
    .map(|rel| repo_root().join(rel))
    .collect()
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
/// 増やすときは「なぜプロセス終了時に消さなくてよいか」を書くこと。
///
/// #1312 で唯一の免除（`verification_agent_home`）が外れた。製品バイナリの
/// 検証起動（`TAKO_ISOLATED` / `TAKO_SELF_TEST`）も同じ dir を作るという事情は
/// 変わっていないが、**武装を `is_test_process()` で絞る**ことで
/// 製品の挙動を変えずに `cargo test` の残骸だけを消せるようになったため
const EXEMPT: &[(&str, &str)] = &[];

/// 「`temp_dir().join(format!("…-{}", std::process::id()))`」で
/// 使い捨て dir を作っている箇所を、囲んでいる関数ごと拾う
fn disposable_dir_sites() -> Vec<Site> {
    let mut out = Vec::new();
    for path in isolation_sources() {
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読める（{e}）", path.display()));
        let file = path
            .strip_prefix(repo_root())
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        // 見るのは**製品コード側**（置き場を決める関数）だけ。単体テストの中で
        // テスト本体が作る使い捨ては別の話で、そちらは ScratchDir（#1312）と
        // 下の `テストを一巡しても…`（tako-core 側の実測）が受け持つ
        let head = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .map(|(head, _)| head.to_string())
            .unwrap_or(src);
        collect_sites(&head, &file, &mut out);
    }
    out
}

/// 1 ファイルぶんの走査
fn collect_sites(src: &str, file: &str, out: &mut Vec<Site>) {
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
        let (func, body) = enclosing_fn(src, idx);
        out.push(Site {
            file: file.to_string(),
            func,
            prefix,
            body,
        });
    }
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
        sites.len() >= 4,
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
        // 消してよいのは「自分の dir」「再判定を通した残骸」「使い捨ての器」の 3 経路。
        // `remove_scratch` は一時ディレクトリ配下であることを確かめてから消す（#1312）
        if !matches!(
            func.as_str(),
            "remove_own_dirs" | "remove_if_still_stale" | "remove_scratch"
        ) {
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

// ----------------------------------- テスト本体の使い捨て dir（Issue #1312）

fn residue_source() -> String {
    let path = repo_root().join("crates/tako-core/src/test_residue.rs");
    std::fs::read_to_string(&path).expect("test_residue.rs を読める")
}

/// 5: 使い捨ての器（`ScratchDir`）が「作る側が消す」形を保っている
#[test]
fn 使い捨ての器はスコープで消える() {
    let src = residue_source();

    // 親は #1296 の 2 段構えに乗る（終了時の atexit + 次回起動時の pid 回収）
    let idx = src
        .find("pub fn scratch_root()")
        .expect("fn scratch_root が無い");
    let (_, body) = enclosing_fn(&src, idx);
    assert!(
        body.contains("test_residue::arm_self_cleanup"),
        "scratch_root が終了時の後始末を武装していない:\n{body}"
    );
    assert!(
        body.contains("test_residue::sweep_stale_on_start"),
        "scratch_root が起動時の掃除を呼んでいない（SIGKILL された回が永遠に残る）:\n{body}"
    );

    // 個々の dir はスコープを抜けた時点で消える
    assert!(
        src.contains("impl Drop for ScratchDir"),
        "ScratchDir が Drop を持たない = スコープで消えない（#1312 の再発）"
    );
    let idx = src
        .find("impl Drop for ScratchDir")
        .expect("impl Drop for ScratchDir");
    let drop_body = &src[idx..idx + src[idx..].find("\n}\n").expect("impl の終わり")];
    assert!(
        drop_body.contains("remove_scratch("),
        "ScratchDir の Drop が削除の 1 経路（remove_scratch）を通っていない:\n{drop_body}"
    );

    // 削除の唯一の経路には「一時ディレクトリ配下か」の安全弁がある
    let idx = src
        .find("fn remove_scratch(")
        .expect("fn remove_scratch が無い");
    let (_, body) = enclosing_fn(&src, idx);
    assert!(
        body.contains("starts_with(std::env::temp_dir())"),
        "remove_scratch に一時ディレクトリ配下の確認が無い（実環境を消す事故の入口）:\n{body}"
    );
}

/// 6: 使い捨ての親も手動の掃除口から見える（`tako test-residue` の種別に載る）
#[test]
fn 使い捨ての親の接頭辞もkindsに載っている() {
    let known: Vec<&str> = tako_core::test_residue::KINDS
        .iter()
        .map(|k| k.prefix)
        .collect();
    assert!(
        known.contains(&"tako-test-scratch-"),
        "ScratchDir の親が KINDS に無い（tako test-residue から掃けない）: {known:?}"
    );
    let src = residue_source();
    let idx = src.find("pub fn scratch_root()").expect("fn scratch_root");
    let (_, body) = enclosing_fn(&src, idx);
    assert!(
        body.contains("\"tako-test-scratch-{}\""),
        "scratch_root が作る名前と KINDS の接頭辞がズレている:\n{body}"
    );
}

/// 7: 種別を足したら**案内も**そろえる（掃除の口が 1 つに揃っていることの担保）
#[test]
fn 種別はcliとmcpの案内に載っている() {
    let docs = [
        "crates/tako-cli/src/main.rs",
        "crates/tako-control/src/mcp/catalog.rs",
        "crates/tako-control/src/protocol.rs",
    ];
    let mut missing = Vec::new();
    for rel in docs {
        let src = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("{rel} を読める（{e}）"));
        for kind in tako_core::test_residue::KINDS {
            if !src.contains(kind.prefix) {
                missing.push(format!("{rel}: {}<pid> の案内が無い", kind.prefix));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "test-residue の種別を足したのに案内が追いついていない:\n{}\n\
         直し方: 各ファイルの tako test-residue の説明へ 1 種類ぶん足す",
        missing.join("\n")
    );
}

/// 8: 製品の検証起動（`TAKO_ISOLATED` / `TAKO_SELF_TEST`）が作る置き場の後始末は
///    **テストプロセスのときだけ**（#1312 で EXEMPT を外した代わりの拘束）。
///    この門番が外れると、セルフテストが終わった瞬間に検証用の設定が消えて
///    製品の挙動が変わる（#1253 が隔離先をここへ置いた前提が崩れる）
#[test]
fn 検証起動の置き場の後始末はテストプロセス限定() {
    let path = repo_root().join("crates/tako-core/src/paths.rs");
    let src = std::fs::read_to_string(&path).expect("paths.rs を読める");
    let idx = src
        .find("pub fn verification_agent_home()")
        .expect("fn verification_agent_home が無い");
    let (_, body) = enclosing_fn(&src, idx);
    assert!(
        body.contains("test_residue::arm_self_cleanup"),
        "検証起動の置き場が終了時の後始末を持たない:\n{body}"
    );
    assert!(
        body.contains("if is_test_process() {"),
        "後始末が is_test_process() で絞られていない（製品バイナリの検証起動でも \
         消えるようになる = #1253 の前提が崩れる）:\n{body}"
    );
}
