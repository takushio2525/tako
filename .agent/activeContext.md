# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-12・#132 codex/agy 承認既定スキップ）

#132 完了・merge 済み・install 済み。tako 再起動で新バイナリが反映される。

- codex / agy worker は既定で承認スキップ（プロファイル未設定でも）
- codex master/solo は `--dangerously-bypass-approvals-and-sandbox` で MCP ツール承認もバイパス
- `profiles set --worker-model-policy` フラグを CLI / MCP / IPC に追加
- `scripts/clean-target.sh` 追加

## 直近の観点

- 実機 AC 1〜3 は tako 再起動後に最終確認が必要（worker spawn / master MCP 呼び出し）
- codex の `-a never` は MCP ツール承認をバイパスしない（実測で判明し修正済み）
- `skip_permissions: false` の明示で承認を戻せる安全弁あり

## 次の一手

- tako 再起動後の最終実機確認（codex/agy worker spawn、codex master MCP）
- FR-3.5 の実機常用フィードバック反映
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- オーケストレーター: `.agent/orchestrator.md`
- 要件: `.agent/requirements.md` FR-2.16 / FR-3.5
