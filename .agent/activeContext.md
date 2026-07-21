# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#453 再生ボタン無反応の根治 → 実機確認待ち）

**#453 Code Runner: 再生ボタン無反応バグ 2 根因を修正（fix/453-run-button-click）**

- 根因 1: persist 復元・ライブリロード経路で `preview_run_profiles` 未検出 →
  復元ペインはボタン淡色（on_click 無し）。`detect_preview_run_profiles` へ抽出し 3 経路で呼ぶ
- 根因 2: `spawn_command_pane` が複合シェルコードを SpawnCommand.program 1 語に詰め、
  login_shell_command のクォートで 1 コマンド名化 → Run ペインが 127 即死。/bin/sh -c 構造へ修正
- 隔離実測（TAKO_ISOLATED + 固定 data_dir + env -u ラッパー）: before profiles=None /
  Run 即死 → after profiles=Some(1) / ペイン生存 + 出力可視
- 品質ゲート全緑: cargo test 1195 / fmt / clippy(-D warnings)

## 次の一手

- `build-app.sh --install` → 実機確認:
  ① tako 再起動後の復元プレビューで再生ボタンが緑 + クリックで実行
  ② start-docs.command（日本語パス）で実行成功
  ③ ドロップダウン選択実行（2+ プロファイルファイル）
- #287 P1-2 UDS 化の実機確認も継続

## 現フェーズで Read すべき設計書

- Code Runner 設計: `.agent/plans/2026-07-code-runner.md`（§4 UI / §7 M4）
