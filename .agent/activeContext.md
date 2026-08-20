# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#833 = セルフテストのクォート漏れ）

- ブランチ `fix/833-space-path`（worktree `~/dev/tako-wt-833`）
- 隔離なし（本番 data dir = `~/Library/Application Support/tako`）で走らせると
  #600 系の項目「検証用 zsh が起動する（入力予測）」が確定失敗する問題を根治

## 根因（実測で確定）

セルフテスト 41c / 41d が `format!("HOME={} ZDOTDIR={zdotdir} /bin/zsh", …)` で
コマンドを組んでおり、値がクォートされていない。data dir が既定の
`~/Library/Application Support/tako` だと `ZDOTDIR=…/Application` までが代入・
`Support/…` がコマンド名として割れ、zsh が起動しないまま項目が落ちる。

- **`TAKO_ISOLATED=1` の隔離起動は data dir が `/tmp` 配下（空白なし）**なので、
  隔離検証だけを回していると一度も踏まない = main 由来の確定失敗として残り続けた
  （#796 の「feature 付きビルドで #600 が落ちる」の一部はこれ）
- production 側（`orchestrator::agent::sh_quote` 経由の `export K=V;`）は
  クォート済みで、この穴はセルフテストだけにあった（ワークスペース全走査で確認）

## 直し方

`self_test::shell_env_command`（値を `tako_core::shell::quote_for_shell` へ通す）を
新設して 3 か所の呼び出しを置き換え。さらに **41c / 41d の隔離 HOME の
ディレクトリ名に意図的な空白を入れた**ので、`HOME=` / `PATH=` 側は
毎回の隔離セルフテストで踏む。番犬 `selftest_env_assignment_watchdog` が
`NAME={` の形をソース走査で落とす（見本行の逃げ道は `watchdog-allow`）。

## 次の一手

- 品質ゲート（fmt / clippy -D warnings / test 2070 passed / Windows クロスチェック
  エラー 0・警告 16）と空白パス data dir の隔離セルフテスト `TAKO_APP_SELF_TEST_OK`
  （全項目完走）は済み
- PR（`Closes #833`）→ macOS CI 緑 → squash merge。テスト系のみの変更なので install 不要

## 現フェーズで Read すべき設計書

- `.agent/conventions.md`「セルフテストの待ち条件の書き方」節 = ペインへ打つ
  コマンドの env 代入をクォートする規約（#833 で追記）の正
