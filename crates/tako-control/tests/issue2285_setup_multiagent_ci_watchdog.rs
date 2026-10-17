//! **#2285 の番犬**: `scripts/verify-setup-multiagent.sh`（#262 の setup UX の実経路テスト）が
//! CI から外れない。
//!
//! このスクリプトは CI に載っていなかったので、#1959（PR #1980）で標準 setup が
//! `claude --version` を呼ぶようになってから、スタブが版の問い合わせを対話起動と数えて
//! 落ちたまま気付かれなかった（#1621 の worker が手で回して見つけた）。
//! 落ち方の中身（質問の数・対話起動・スタブの検出力の自己検査）はスクリプト側が見る

use std::path::{Path, PathBuf};

const SCRIPT: &str = "scripts/verify-setup-multiagent.sh";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 実経路テストが在り、CI に載っている
#[test]
fn 実経路テストが登録されている() {
    assert!(
        repo_root().join(SCRIPT).is_file(),
        "{SCRIPT} が無い（#262 の setup UX の実測がここに居る）"
    );
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml を読める");
    assert!(
        ci.contains(SCRIPT),
        ".github/workflows/ci.yml に {SCRIPT} の登録が無い（#2285）"
    );
}
