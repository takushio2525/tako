# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-18・オーケストレーター機能 完了）

tako にオーケストレーター機能を完全内蔵。外部スクリプト依存ゼロで `tako master` で
マスターエージェントを起動し、MCP / CLI から子 worker の spawn・監視・管理ができる。

- **orchestrator モジュール**: `tako-control/src/orchestrator/`（projects.yaml パース、
  設定管理、デフォルト system prompt 埋め込み、claude agents --json 連携）
- **protocol**: `OrchestratorProjects` / `OrchestratorSpawn` / `OrchestratorWorkerStatus` の 3 Request
- **dispatch**: 3 操作のハンドラ（projects CRUD / worker split+起動 / status 確認）
- **MCP**: `tako_orchestrator_projects` / `tako_orchestrator_spawn` / `tako_orchestrator_worker_status`（計 40 ツール）
- **CLI**: `tako master [suffix]` / `tako orchestrator watch` / `projects` / `spawn` / `status`
- **ドキュメント**: `docs/orchestrator.md`（セットアップ・CLI リファレンス）
- **検証**: build / clippy(-D warnings) / fmt / test 全緑。セルフテスト期待値 40 に更新
- 最終更新: 2026-06-18

## 残作業・既知の制約

- spawn の prompt 送信は claude 起動後に send_input で行う設計（dispatch 内で待機しない）。
  呼び出し側が send + sleep で claude 起動を待つ必要がある
- `tako orchestrator watch` は 5 秒ポーリング。session_id ありなら連続 2 回 idle で確定、
  なしなら grep フォールバック（連続 6 回）
- system prompt はバイナリ埋め込み + カスタムファイル優先。カスタムファイルが無い場合は
  config dir に `_default_system_prompt.md` として書き出す

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する

## 現フェーズで Read すべき設計書

- オーケストレーター修正時: `docs/orchestrator.md`
- タブツリー/プレビュー/ピン再修正時: `requirements.md` FR-2.15 / FR-2.16
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
