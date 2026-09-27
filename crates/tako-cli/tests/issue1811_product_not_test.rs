//! #1811 の逆向き: **製品の `tako` はテストと誤判定されない**。
//!
//! #1811 で `is_test_process()` を「製品の入口（`mark_product_process`）を通っていなければ
//! テスト」へ倒した。製品の `main` が宣言を落とすと、製品が本番の data dir を読まず
//! 一時 dir へ倒れる（ユーザーには「tako が空になった」に見える）。
//! ビルドした本物の `tako` を**一時 HOME を本番に見立てて**起こし、
//! 解決した置き場が見立てた本番の下にあること・安全側へ倒した知らせが出ないことを見る。
//!
//! 叩くのは `tako migrate status`（GUI 不要・読むだけで何も書かない・置き場の
//! パスを JSON で出す）。宣言の置き場所そのものは
//! `crates/tako-control/tests/issue1811_product_entry_watchdog.rs` が静的に固定する。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 安全側へ倒したときに出る知らせの目印（`tako_core::paths::warn_no_product_entry`）
const FAILSAFE_MARKER: &str = "Issue #1811";

/// 一時 HOME で `tako migrate status` を走らせ、`(settings.json の置き場, stderr)` を返す
fn settings_path_under(home: &Path, tmp: &Path) -> (PathBuf, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_tako"))
        .args(["migrate", "status"])
        // 見立てた本番。unix / Windows の既定の解決元を全部ここへ向ける
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_DATA_HOME", home.join("xdg-data"))
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        // 誤判定されたときの倒れ先（`tako-test-data-<pid>`）を見立てた本番の外へ置く
        .env("TMPDIR", tmp)
        .env("TMP", tmp)
        .env("TEMP", tmp)
        // 明示の置き場・隔離の逃げ道・本番 GUI への接続は全部外す
        .env_remove("TAKO_DATA_DIR")
        .env_remove("TAKO_ISOLATED")
        .env_remove("TAKO_SELF_TEST")
        .env_remove("TAKO_VISUAL_TEST")
        .env_remove("TAKO_SOCKET")
        .env_remove("TAKO_PANE_ID")
        .env_remove("TAKO_TOKEN")
        .env_remove("TAKO_1811_LEGACY")
        .output()
        .expect("tako を起動できる");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "tako migrate status が失敗した\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("migrate status の出力が JSON でない: {e}\n{stdout}"));
    let path = json["files"]
        .as_array()
        .and_then(|files| {
            files
                .iter()
                .find(|f| f["schema"] == "settings")
                .and_then(|f| f["path"].as_str())
        })
        .unwrap_or_else(|| panic!("settings の置き場が出力に無い\n{stdout}"));
    (PathBuf::from(path), stderr)
}

#[test]
fn 製品のtakoは本番の置き場を解決しテストと誤判定されない() {
    let base = tako_core::test_residue::ScratchDir::new("1811-product");
    let home = base.path().join("home");
    let tmp = base.path().join("tmp");
    std::fs::create_dir_all(&home).expect("一時 HOME を作れる");
    std::fs::create_dir_all(&tmp).expect("一時 TMPDIR を作れる");

    let (settings, stderr) = settings_path_under(&home, &tmp);

    assert!(
        settings.starts_with(&home),
        "製品の tako が見立てた本番の置き場を解決していない = テストと誤判定された（#1811）\n\
         settings: {}\nstderr:\n{stderr}",
        settings.display()
    );
    assert!(
        !settings.starts_with(&tmp),
        "製品の tako の data dir が一時 dir へ倒れている（#1811）: {}",
        settings.display()
    );
    assert!(
        !stderr.contains(FAILSAFE_MARKER),
        "製品の tako が「製品の入口を通っていない」と名乗った（#1811）\nstderr:\n{stderr}"
    );
    // 読むだけのコマンドなので、見立てた本番には何も作らない
    let created: Vec<_> = walk(&home);
    assert!(
        created.is_empty(),
        "migrate status が見立てた本番へ書いた: {created:#?}"
    );
}

/// `root` 配下のファイル（ディレクトリは数えない）
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
