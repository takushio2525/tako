//! `.app` の差し替えが「置き場のパスを空ける」形へ戻っていないことの番犬（#1042）
//!
//! Dock のピン留めは `.app` への file URL ブックマークで持たれ、CNID を優先して
//! 解決する。差し替えの途中で `/Applications/tako.app` が空くと、追跡している側は
//! 「アプリが退避先へ移動した」と読んで参照を書き直し、そのあと退避先を消されて
//! ピンが外れる（#1042 で機序を実測確定）。
//!
//! 正本は `tako_core::platform::bundle_install::replace_bundle_in_place`。
//! シェル側（`scripts/lib/bundle-install.sh`）はその写しなので、
//! **両方が同じ手順を踏んでいること**をここで機械検証する。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 行コメントを落とした本文（規約の説明文に当たって誤検知しないため）
fn without_comments(source: &str, marker: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with(marker))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn アプリ内更新の差し替えは境界を通る() {
    let body = without_comments(&read("crates/tako-app/src/update_checker.rs"), "//");
    assert!(
        body.contains("bundle_install::replace_bundle_in_place"),
        "update_via_zip は tako_core::platform::bundle_install を通すこと。\
         自前で「退避 → 新規コピー」を書くと Dock のピンが外れる（#1042）"
    );
    // 旧手順の痕跡（置き場を .bak へ退避してから設置する）が復活していないこと。
    // 更新の途中で落ちた旧世代の後始末として `remove_dir_all` するのは可
    assert!(
        !body.contains("rename(dest, backup)") && !body.contains("std::fs::rename(dest,"),
        "置き場を退避してから設置する形へ戻っている（#1042）"
    );
}

#[test]
fn buildappのinstallは置き場を消してからコピーしていない() {
    let body = without_comments(&read("scripts/build-app.sh"), "#");
    assert!(
        body.contains("install_bundle_in_place"),
        "build-app.sh --install は scripts/lib/bundle-install.sh を通すこと（#1042）"
    );
    for forbidden in [
        r#"rm -rf "$LS_CANONICAL_APP""#,
        r#"cp -R "$APP" "$LS_CANONICAL_APP""#,
    ] {
        assert!(
            !body.contains(forbidden),
            "build-app.sh に旧手順が残っている: {forbidden}（#1042）"
        );
    }
}

#[test]
fn シェル側の写しが正本と同じ手順を踏んでいる() {
    let sh = read("scripts/lib/bundle-install.sh");
    // 「隣へステージ → アトミックに入れ替え → 旧版を捨てる」の 3 点が揃っていること
    for needed in ["RENAME_SWAP", "renamex_np", ".tako-replace-", "ditto"] {
        assert!(
            sh.contains(needed),
            "scripts/lib/bundle-install.sh に {needed} が無い（正本と手順が食い違う。#1042）"
        );
    }
    // 置き場を消すのは swap が使えないときの落とし先だけ。その分岐は必ず警告を出す
    assert!(
        sh.contains("Dock のピン留めが外れることがあります"),
        "旧挙動へ落ちたことを伏せてはいけない（#1042）"
    );
    // 正本と同じ作業用ディレクトリ名を使っていること（残骸の掃除規則を揃えるため）
    let rs = read("crates/tako-core/src/platform/bundle_install.rs");
    assert!(
        rs.contains(".tako-replace-"),
        "正本の作業用ディレクトリ名が変わっている。写し側も直すこと（#1042）"
    );
}

#[test]
fn モックテストが同梱されている() {
    let path = repo_root().join("scripts/test-bundle-install.sh");
    assert!(
        path.exists(),
        "scripts/test-bundle-install.sh が無い（写しの検証手段が消えている。#1042）"
    );
    let body = read("scripts/test-bundle-install.sh");
    assert!(
        body.contains("検出力"),
        "旧手順なら不在を観測できることの対照が要る（#1042）"
    );
    // 置いてあるだけで回っていなければ回帰を捕まえられない
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("scripts/test-bundle-install.sh"),
        "モックテストが CI から外れている（#1042）"
    );
}
