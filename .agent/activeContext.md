# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-12・#141 ファイルツリー追加プロンプト強化）

#141 完了・merge 済み・install 済み。tako 再起動で新バイナリ（58 ツール）が反映される。

- master / solo 両方のデフォルト system prompt に「Keep the file tree current」行動規範を追加
- 会話に上がったプロジェクト・関連フォルダを聞かれる前に追加する積極的な規範

## 直近の観点

- tako 再起動後に master / solo のプロンプトに規範が反映されていることを確認可能
- #136 エージェント共通ルール同期も install 済み（前セッション）

## 次の一手

- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル
- ユーザーの config.yaml に agents_sync セクションを設定（`tako setup` または手動）

## 現フェーズで Read すべき設計書

- オーケストレーター: `.agent/orchestrator.md`
- 要件: `.agent/requirements.md` FR-3.1 / FR-2.16
