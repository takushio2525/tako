# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-12・#136 エージェント共通ルール同期）

#136 完了・merge 済み・install 済み。tako 再起動で新バイナリ（58 ツール）が反映される。

- `tako agents sync-rules`: 正本から各エージェント指示ファイルへマーカーブロック同期
- `tako agents status`: 同期状態確認
- MCP `tako_agents_sync_rules`（計 58 ツール）
- config.yaml `agents_sync` セクション（source_path + targets）
- `tako setup --check` に同期状態チェック追加

## 直近の観点

- tako 再起動後に CLI / MCP の実機確認が必要（`tako agents status` / MCP `tako_agents_sync_rules`）
- ユーザーの実際の dotfiles 正本パスを `config.yaml` に設定すれば即使用可能

## 次の一手

- tako 再起動後の最終実機確認（MCP 経由での同期実行）
- ユーザーの config.yaml に agents_sync セクションを設定（`tako setup` または手動）
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- オーケストレーター: `.agent/orchestrator.md`
- 要件: `.agent/requirements.md` FR-3.1 / FR-2.16
