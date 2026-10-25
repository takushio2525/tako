//! 走らせた人の env から切り離した子でテストを 1 本走らせ直す（Issue #2391。テスト専用）
//!
//! ## なぜ要るか（worker の検証 env で回して確かめた）
//!
//! worker は CLI の検証で `TAKO_ISOLATED=1 TAKO_DATA_DIR=<短い一時 dir>` を付けたシェルから
//! `cargo test` も回す。実経路のテストがその env をそのまま読むと、CI（env なし）では緑なのに
//! 手元でだけ落ちる。usage の番犬 3 本は `TAKO_DATA_DIR=/tmp/…` の下で
//! 「テストの data dir が一時ディレクトリの外」で必ず落ち（macOS の `temp_dir()` は
//! `/var/folders/…`）、`$TMPDIR` の下を渡すと走らせた人の data dir へ書いていた
//!
//! ## 作り（#2048 の `probe` と同じ形）
//!
//! 親は `keys` を外した子で自分を `--exact` で起こし直し、子が**その 1 本を通した**ことだけを見る
//! （名前の打ち間違いで 0 本のまま緑にしない）。プロセス全体の env を `set_var` で揃えると、
//! 並列に走る他のテストへ漏れるのでやらない

/// 子だけが立てる目印
const CHILD_ENV: &str = "TAKO_2391_DETACHED_CHILD";

/// 子なら `false`（= 呼び出し側はそのまま本文へ進む）。親なら `keys` を外した子で `test` を
/// 走らせ、通ったことを確かめて `true`（= 呼び出し側は本文を走らせずに戻る）
pub fn rerun_without(test: &str, keys: &[&str]) -> bool {
    if std::env::var_os(CHILD_ENV).is_some() {
        return false;
    }
    let exe = std::env::current_exe().expect("テストバイナリのパス");
    let mut cmd = std::process::Command::new(exe);
    // `--test-threads=1`: 子の出力の形を親の env（`RUST_TEST_THREADS`）に依らず 1 つに決める（#2248）
    cmd.args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(CHILD_ENV, "1")
        .stdin(std::process::Stdio::null());
    for key in keys {
        cmd.env_remove(key);
    }
    let out = cmd.output().expect("子のテストを起こせる");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "子（{test}）が失敗した（{}）:\n{text}",
        out.status
    );
    assert!(
        text.contains("test result: ok. 1 passed"),
        "子が {test} を走らせていない（改名漏れ？）:\n{text}"
    );
    true
}
