# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21）

- **#868（tako setup のゼロスタート対応）を PR #871 で提出**。claude 未導入の環境から
  `tako setup` 一発で インストール → PATH 通し → 認証誘導 → 対話起動まで通る
- 並行して **#467 Windows 移植**がスライス 7c まで進んでいる（残りはスライス 8 = 棚卸し）。
  **#867（起動コマンドの env 前置きをシェル方言へ）は PR #874**: これで Windows 実機で
  `orchestrator spawn` から **claude が実際に起動してプロンプトに応答する**ところまで通った
  （`$env:TAKO_ORCHESTRATOR_ROLE='...'; claude --effort max` を生成。実プロセスの環境に
  role が届いていることを PEB 読み出しで確認）

## #868 で入れたもの

- 境界 **B17**（`platform::agent_install`）= 公式インストール手順を `Platform` 引数の
  純粋関数で持つ。macOS 上から Windows 向けの内容も検証できる
- `shell_profile` = PATH ブロックの冪等な読み書き。**書き先はログインシェルの profile**
  （zsh = `.zprofile`）。`$SHELL -l -c` が `.zshrc` を読まないため、公式 docs の案内
  （`.zshrc`）に従うと tako が自分で入れた CLI を見つけられない（実測で確定）
- `text_block` = マーカーブロックの規則（区切り改行 1 個・元バイト列への完全復帰）を集約。
  `shell_integration` の PowerShell 実装もここへ委譲
- `setup_bootstrap`（tako-control）= いまどの段か（install / path / auth / ready）を判定して実行
- 1:1 公開: `tako setup bootstrap` / MCP `tako_setup_bootstrap`（137 ツール）

## #867 で入れたもの（スライス 7c）

- `tako-control::launch_cmd` = 起動コマンドの env 前置きとクォートを構文別に組み立てる。
  `LaunchSyntax::for_program` は純粋関数なので macOS から PowerShell 側を全分岐テストできる
- **5 フローが 3 関数に集約**されていた（`build_worker_cmd` / `build_master_cmd` /
  `resume_env_prefix_for`）。各関数に構文を明示する `*_in` 版がある
- macOS は 1 バイトも不変。そのためクォートは `quote`（必要なときだけ）と
  `quote_always`（常に）の 2 系統

## 次の一手

- PR #871 の CI 緑を確認 → squash merge → #868 クローズ → #525 へ境界の申し送り
- **#873**（`LaunchSyntax` と `platform::shell_dialect` の方言判定の一本化。#865 merge 後）
- **#875**（`spawn_command_pane` の `/bin/sh -c` 決め打ちで Windows では PTY が立たない =
  #666 のカード実行と #453 の Code Runner が死んでいる）
- `build-app.sh --install` は**未実施**（本番 GUI 稼働中のため master へ申し送り）
- #868 の残り: 実ブラウザでのログイン完走は未実測（実アカウントに触れるため手前まで）。
  Windows の実行代行は #525

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義。B17 を足した）
- `.agent/plans/2026-08-windows-main-merge-wip.md`（スライス 8 の申し送り）
