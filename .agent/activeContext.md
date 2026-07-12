# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-12・#143 setup FDA 案内ステップ強化）

#143 完了・merge 済み・install 済み。tako 再起動で新バイナリが反映される。

- setup の依存チェック段階で FDA 案内を強化: TCC ダイアログが消える旨の説明、設定画面を開く対話、再起動案内
- changes.yaml rev 6 追加で既存ユーザーにも配信

## 直近の観点

- この環境は FDA 付与済みのため「✓ 付与済み」パスを実機確認。未付与パスはコードレビューとテストで担保
- 非対話（パイプ）環境でも壊れないことを確認

## 次の一手

- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル
- ユーザーの config.yaml に agents_sync セクションを設定（`tako setup` または手動）

## 現フェーズで Read すべき設計書

- オーケストレーター: `.agent/orchestrator.md`
- 要件: `.agent/requirements.md` FR-3.1 / FR-2.16
