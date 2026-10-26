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
//! 並列に走る他のテストへ漏れるのでやらない。起こし直しは lib のテストと共有する
//! `tako_core::test_child::rerun_alone` の 1 実装（#2400。子の中なら本文で env を書いても漏れない）

/// 子なら `false`（= 呼び出し側はそのまま本文へ進む）。親なら `keys` を外した子で `test` を
/// 走らせ、通ったことを確かめて `true`（= 呼び出し側は本文を走らせずに戻る）
pub fn rerun_without(test: &str, keys: &[&str]) -> bool {
    tako_core::test_child::rerun_alone(test, keys)
}
