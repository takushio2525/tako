# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-12・#134 ファイルツリーフォルダ操作）

#134 完了・merge 済み・install 済み。tako 再起動で新バイナリ（57 ツール）が反映される。

- `tako tree add/remove/list`: AI がプロジェクトフォルダをファイルツリーに明示追加
- MCP `tako_tree_folder`（計 57 ツール）
- タブ単位スコープ・layout.json 永続化（後方互換）
- master/solo system prompt にフォルダ追加ガイドを追記

## 直近の観点

- tako 再起動後に CLI / MCP の実機 e2e 確認が必要（`tako tree add /path` → list → remove）
- dispatch テスト 5 本で add→list→remove・エラー経路は全て機械検証済み

## 次の一手

- tako 再起動後の最終実機確認（`tako tree add` / MCP `tako_tree_folder`）
- FR-3.5 の実機常用フィードバック反映
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- オーケストレーター: `.agent/orchestrator.md`
- 要件: `.agent/requirements.md` FR-3.1 / FR-2.16
