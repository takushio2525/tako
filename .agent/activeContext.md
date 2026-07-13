# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#165 spawn レイアウトエンジン完了）

#165 実装完了（fix/165-spawn-layout-engine → PR 予定）: worker spawn を
master-reserved（master の取り分維持 + 右側 worker 領域内 grid/spiral 配置）へ刷新。

- レイアウト計算 = `tako-core::spawn_layout`（型 + 領域構築純関数）+
  `PaneTree::spawn_worker` / `reflow_workers`。worker 領域 = spawned_by チェーンが
  anchor に到達するリーフのみのサブツリー（ユーザーペイン混在は領域外 = 不変）
- 設定 = config.yaml `spawn_layout`（policy / master_ratio / algorithm）。
  CLI `tako orchestrator layout` と MCP `tako_orchestrator_layout`（59 ツール）は
  `dispatch_orchestrator_layout` を共用（host 非依存・二重実装なし）
- close リフロー = dispatch `Close` + tako-app `remove_pane_with` の両経路
- 検証済み: tako-core 単体 10 本 + セルフテスト項目 72 完走 + セカンダリ実機
  spawn ×4 → 十字四分割 → close リフローを screencapture ピクセル確認

## 検証時インシデント（解決済み・#178 起票）

TAKO_DISCOVERY_DIR 指定の dev 起動で多重起動ガードが無効化され、production の
tmux バックエンド 13 セッションを強奪（タブ 8 → 3。実プロセス損失ゼロ、
ユーザーが復旧済み）。根因 = プライマリ判定が discovery のみ依存 → #178。
**dev 併走検証は素の `cargo run -p tako-app`（セカンダリモード）で行うこと**

## 次の一手

- PR 作成 → squash merge → `build-app.sh --install` → tako 再起動で実機反映
- #178（多重起動ガードのプロセスベース判定併用)の着手判断
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- spawn レイアウトの設計: `.agent/architecture.md`「spawn レイアウトエンジン」節
- 要件: `.agent/requirements.md` FR-2.20
