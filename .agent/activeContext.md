# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-25・spawn 信頼性 + セッション追跡の改善完了）

オーケストレーター spawn の信頼性改善 4 項目を実装完了:
1. 複数 master 時の role 検索を suffix マッチ対応（呼び出し元の `:tako` 等を優先）
2. worker ペインに `spawned_by` フィールド追加（spawn 元を記録・list/レスポンスに公開）
3. `worker_status` の `pane_exists` で shelved ペインも走査（退避時の誤 gone 防止）
4. dead code `find_session_id` の除去

## 残作業・既知の制約

- main.rs の残り 8,359 行にはまだ大きなブロックがある（render_pane 約450行、
  ControlHost 実装 966行、セルフテスト 2,800行 等）
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- scroll テスト「履歴ゼロではcopy_modeに入らない」がフレーキー（環境依存のタイミング問題）

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
