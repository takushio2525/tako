# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-25・orchestrator spawn tab パラメータ追加完了）

`tako_orchestrator_spawn` に `tab` パラメータを追加。指定タブのフォーカスペインを
分割元にする。複数マスター運用時に子 worker の出力先タブを明示指定可能に。
MCP / CLI / dispatch / protocol の 4 ファイル修正。全テスト緑、.app 配置済み。

## 残作業・既知の制約

- main.rs の残り 8,359 行にはまだ大きなブロックがある（render_pane 約450行、
  ControlHost 実装 966行、セルフテスト 2,800行 等）
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- scroll テスト「履歴ゼロではcopy_modeに入らない」がフレーキー（環境依存のタイミング問題）

## 未着手タスク（優先順はユーザーと相談）

- [x] ~~**orchestrator spawn のタブ配置問題**~~: `tab` パラメータ追加で解決済み（`dc4b65c`）
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
