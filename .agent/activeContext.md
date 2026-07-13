# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#159 スクロール改善 + #165 spawn レイアウトが完了）

main 側の完了分: **v0.4.0 リリース済み**（tag `v0.4.0`、cask 0.4.0）、夜間パッチ
リリースは launchd ジョブへ移行（#166。毎日 5:00）、#169 projects.yaml 全消失の根治
（`config_io` 新設。**設定更新は load→save ではなく `mutate_config` 系を使うこと**）、
#159 スクロール大幅改善（ピクセル単位化 + tmux ペインのローカル履歴ミラー +
スクロールバー強化。PR #176 マージ済み）。

#165 実装完了（fix/165-spawn-layout-engine → PR #179）: worker spawn を
master-reserved（master の取り分維持 + 右側 worker 領域内 grid/spiral 配置）へ刷新。

- レイアウト計算 = `tako-core::spawn_layout`（型 + 領域構築純関数）+
  `PaneTree::spawn_worker` / `reflow_workers`。worker 領域 = spawned_by チェーンが
  anchor に到達するリーフのみのサブツリー（ユーザーペイン混在は領域外 = 不変）
- 設定 = config.yaml `spawn_layout`（policy / master_ratio / algorithm）。
  CLI `tako orchestrator layout` と MCP `tako_orchestrator_layout`（59 ツール）は
  `dispatch_orchestrator_layout` を共用（更新は #169 の mutate_config 経由）
- close リフロー = dispatch `Close` + tako-app `remove_pane_with` の両経路
- 検証済み: tako-core 単体 10 本 + セルフテスト項目 72 完走 + セカンダリ実機
  spawn ×4 → 十字四分割 → close リフローを screencapture ピクセル確認

## 検証時インシデント（解決済み・#178 起票）

TAKO_DISCOVERY_DIR 指定の dev 起動で多重起動ガードが無効化され、production の
tmux バックエンド 13 セッションを強奪（タブ 8 → 3。実プロセス損失ゼロ、
ユーザーが復旧済み）。根因 = プライマリ判定が discovery のみ依存 → #178。
**dev 併走検証は素の `cargo run -p tako-app`（セカンダリモード）で行うこと**

## 次の一手

- PR #179 の squash merge → `build-app.sh --install` → tako 再起動で実機反映
- tako 再起動後の GUI 確認（manual-checks.md）: 「ターミナルスクロールの大幅改善」節
  （#159）+「Web ビューペイン」節（#155）+ #153/#152 節 + Cmd-Q 経過観察（#103）
- 明朝 5:00 の夜間ジョブ初回実行を監視（v0.4.1 自動リリース見込み。#166）
- #178（多重起動ガードのプロセスベース判定併用）の着手判断
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- spawn レイアウトの設計: `.agent/architecture.md`「spawn レイアウトエンジン」節
- スクロールの要件（#159 で全面改稿）: `.agent/requirements.md` FR-2.5.13 +
  手動確認 `.agent/manual-checks.md`「ターミナルスクロールの大幅改善」
- 設定ファイル I/O の安全化（#169）: `.agent/architecture.md` 該当節
