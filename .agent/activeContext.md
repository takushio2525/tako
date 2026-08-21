# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21）

- **#868（tako setup のゼロスタート対応）を PR #871 で提出**。claude 未導入の環境から
  `tako setup` 一発で インストール → PATH 通し → 認証誘導 → 対話起動まで通る
- 並行して **#467 Windows 移植**がスライス 7b まで main へ入っている（残りはスライス 8 = 棚卸し）

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

## 次の一手

- PR #871 の CI 緑を確認 → squash merge → #868 クローズ → #525 へ境界の申し送り
- `build-app.sh --install` は**未実施**（本番 GUI 稼働中のため master へ申し送り）
- #868 の残り: 実ブラウザでのログイン完走は未実測（実アカウントに触れるため手前まで）。
  Windows の実行代行は #525

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義。B17 を足した）
- `.agent/plans/2026-08-windows-main-merge-wip.md`（スライス 8 の申し送り）
