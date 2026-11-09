//! 番犬: `tako_send_input` の `keys` / `attachments`（#2743）の実 PTY テストが Windows の ConPTY でも
//! 走る形のまま残っている（Issue #2824）
//!
//! # なぜ要るか
//!
//! #2743 の実 PTY テストは `cfg(unix)` だけで、Windows（ConPTY）で子に届くバイト列は一度も
//! 確かめていなかった。#2824 で記録係（`i2824_recorder_command`）に Windows の腕を足し、同じテストを
//! 両 OS で回すようにした。ここへ `#[cfg(unix)]` を戻しても macOS の CI は緑のままなので、
//! 「Windows で確かめている」という対応表の根拠（`tako_send_input.keys` / `.attachments` の行）が
//! 黙って嘘になる。だから形をここで縛る。
//!
//! # 縛ること
//!
//! - [`移植した実ptyのテストはwindowsでも走る`] — [`PORTED`] の各テストの直前の属性に、Windows を
//!   外す `cfg`・`ignore` が無い
//! - [`記録係は両osの腕を持つ`] — `i2824_recorder_command` に unix と windows の腕が在る
//! - [`対応表の根拠は移植したテストを名指す`] — 対応表の引数の行が在り、根拠が [`PORTED`] の
//!   テストを名指している（テストを改名したのに根拠が古い名前のまま、を落とす）
//! - [`逆戻りを名指しできる`] — 現行ソースへ `#[cfg(unix)]` を戻した注入が `file:line` で名指される

use std::path::{Path, PathBuf};

use tako_core::platform::support::{Platform, MATRIX};

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

/// Windows の ConPTY で走るべき実 PTY のテスト（記録係の子が受け取った入力を見る。最後の 1 本以外は両 OS）
const PORTED: &[&str] = &[
    "issue2743_ダイアログ中のkeysは断り選択を確定させない",
    "issue2743_keysは名前の順に届き知らない名前は何も送らない",
    "issue729_keysの修飾付きenterは器がcsi_uを運べないペインへesc_crで届く",
    "issue2743_添付は置き場へ写し本文にパスを足して送る",
    "issue2743_ダイアログ中の添付は写す前に断る",
    "issue2824_添付は日本語と空白を含むパスでも写り本文にそのパスが入る",
    "issue2824_keysの語彙は名前ごとに同じバイト列で子に届く",
    // Windows だけのテスト（`#[cfg(windows)]` は外す属性ではない）
    "issue2824_conptyのキーイベントは修飾付きenterを落としescと次のキーをaltへ畳む",
];

/// Windows を外す属性（テストの直前の属性の行にこれが在れば違反）
const EXCLUDING: &[&str] = &[
    "#[cfg(unix)]",
    "#[cfg(not(windows))]",
    "#[cfg(target_os = \"macos\")]",
    "#[cfg(target_family = \"unix\")]",
    "#[ignore",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// `name` のテスト関数の直前の属性の行のうち、Windows を外すものを `file:line` で返す。
/// 関数が見つからなければその旨を返す（改名・削除の取りこぼしを緑にしない）
fn excluded(src: &str, name: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let head = format!("fn {name}()");
    let Some(at) = lines.iter().position(|l| l.trim_start().starts_with(&head)) else {
        return vec![format!(
            "{DISPATCH}: テスト {name} が見つからない（改名・削除？）"
        )];
    };
    let mut out = Vec::new();
    let mut i = at;
    while i > 0 {
        i -= 1;
        let line = lines[i].trim();
        if !line.starts_with("#[") {
            break;
        }
        if EXCLUDING.iter().any(|bad| line.starts_with(bad)) {
            out.push(format!("{DISPATCH}:{} {name}: {line}", i + 1));
        }
    }
    out
}

#[test]
fn 移植した実ptyのテストはwindowsでも走る() {
    let src = read(DISPATCH);
    let bad: Vec<String> = PORTED.iter().flat_map(|n| excluded(&src, n)).collect();
    assert!(
        bad.is_empty(),
        "#2743 の実 PTY テストが Windows の ConPTY で走らない形へ戻っている（#2824）:\n  {}\n\
         → 記録係 i2824_recorder_command の Windows の腕で回る。ConPTY で形が変わるなら cfg で\n\
         期待値を分けて名指し、テストごと外さないこと",
        bad.join("\n  ")
    );
}

#[test]
fn 記録係は両osの腕を持つ() {
    let src = read(DISPATCH);
    for arm in ["#[cfg(unix)]", "#[cfg(windows)]"] {
        let pattern = format!("{arm}\n    fn i2824_recorder_command(");
        assert!(
            src.contains(&pattern),
            "記録係 i2824_recorder_command の {arm} の腕が無い（#2824）"
        );
    }
    assert!(
        src.contains("fn i2824_conpty_recorder_child()"),
        "Windows の記録係の本体（ConPTY の子）が無い（#2824）"
    );
}

#[test]
fn 対応表の根拠は移植したテストを名指す() {
    for key in ["tako_send_input.keys", "tako_send_input.attachments"] {
        let row = MATRIX
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("対応表に {key} の行が無い（#2824）"));
        let citation = row.windows_evidence.citation().unwrap_or_default();
        let cited: Vec<&&str> = PORTED.iter().filter(|n| citation.contains(**n)).collect();
        assert!(
            !cited.is_empty(),
            "{key} の Windows の根拠が移植した実 PTY のテストを名指していない（#2824）: {citation}"
        );
        assert!(row.on(Platform::Windows).is_usable(), "{key}");
    }
}

#[test]
fn 逆戻りを名指しできる() {
    let src = read(DISPATCH);
    let name = PORTED[0];
    let head = format!("    #[test]\n    fn {name}()");
    assert!(src.contains(&head), "注入の足場が見つからない: {name}");
    let injected = src.replacen(&head, &format!("    #[cfg(unix)]\n{head}"), 1);
    let bad = excluded(&injected, name);
    assert!(
        bad.len() == 1 && bad[0].contains(name) && bad[0].contains("#[cfg(unix)]"),
        "注入した #[cfg(unix)] を名指せない: {bad:?}"
    );
    // 改名（関数が見つからない）も名指す
    let renamed = src.replacen(&format!("fn {name}()"), "fn 改名した名前()", 1);
    assert!(excluded(&renamed, name)[0].contains("見つからない"));
}
