# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-03・Issue #42 完了 / Issue #26 完了）

Issue #42（リモートフロントエンド刷新 = #23 フェーズ B）を完了:
二層構成（履歴レイヤー + ライブ WS + 自動リサイズ）で PWA を刷新。
Issue #26（Shift+Enter 改行）も textarea 化で同時解決。PR #45 squash merge 済み。

## 実装した二層構成

- **履歴レイヤー**: `GET /api/panes/:id/scrollback` でプレーンテキスト取得 → クライアント側
  `<pre>` で描画。スマホ幅折り返し・自由スクロール・テキスト選択/コピー対応
- **ライブ画面レイヤー**: REST ポーリング → WS プッシュに移行。`GET /ws?pane=<id>&cols=N&rows=N`
  で接続時にペインを自動リサイズ、切断時にリセット

## サーバー側追加

- `GET /api/panes/:id/scrollback?lines=N` — スクロールバック API
- WS に `cols`/`rows` パラメータ（接続時自動リサイズ + 切断時リセット）
- `POST input` に `keys` フィールド（tmux send-keys 生キーシーケンス送信）
- `tmux_send_raw_keys()` 関数（`-l` なし = 特殊キー名解釈）
- CLI: `tako remote scrollback` / MCP: `tako_remote_scrollback`（計 51 ツール）

## 残作業・既知の制約

- **スマホ実機テスト未実施**: WS 接続・自動リサイズ・履歴レイヤー・Shift+Enter・Quick keys の
  実機確認が必要（PR の Test plan 参照）
- **リモート系 dispatch（RemoteAgents / RemoteMessages / RemoteScrollback / TmuxResize）は
  tako 本体の再起動後に MCP から有効**
- main.rs の残り 8,359 行にはまだ大きなブロックがある
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- **セルフテスト項目 46「全角行のクリックが正しいセルに解決」が決定的に失敗** = Issue #37

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント（API 仕様の正）
- オーケストレーター修正時: `docs/orchestrator.md`
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
