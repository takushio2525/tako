# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-02・Issue #23 フェーズ A 完了 / Issue #32 送達確認ループ完了）

スマホリモート接続改善（Issue #23）のフェーズ A（接続基盤・バックエンド API）を完了。
WS 画面プッシュ / ANSI screen / resize / 認証 fragment 化 / agents・messages 構造化 API /
不整合解消（リレー URL・close ハンドラ）。次はフェーズ B（フロントエンド刷新）を
別 worker が担当する予定。

並行 worker が Issue #32（spawn / send のプロンプト送達不安定）を修正済み:
`tako-control::claude_tui` 新設（TUI 状態検出・事前信頼・tmux 送達確認配送）+
PromptFlow 刷新（信頼ダイアログ承諾 → bracketed paste → 分離 Enter → 空検証 + 再送）。
仕様メモは `requirements.md` FR-2.2.2 実装メモと `orchestrator.md` spawn 節。
**プロンプト送達系の dispatch 変更も tako 本体の再起動後に有効**（下記と同様）。

## フェーズ B worker への引き継ぎ事項

- WS プロトコル: `GET /ws?pane=<id>` + `Sec-WebSocket-Protocol: tako-remote, token.<T>`。
  **プッシュ専用**（`{"type":"screen"|"keepalive"|"error"}` が届く）。操作系は REST を使う。
  仕様の正は `crates/tako-control/src/remote.rs` のモジュールコメント
- PWA の api.js に screen(ansi)/resize/agents/messages クライアント実装済み。UI 接続は未
- connect URL は `/#/connect?token=...`（fragment）。app.jsx のハッシュルーターがそのまま解釈
- terminal.jsx は暫定で ANSI ポーリング表示（WS 未使用）。フェーズ B で WS + fit → resize 連動へ

## 残作業・既知の制約

- **リモート系 dispatch（RemoteAgents / RemoteMessages / TmuxResize）は tako 本体の
  再起動後に MCP から有効**（実行中の旧バイナリは新 Request を知らない）
- `tako remote start` は **PATH の tako を子デーモンとして起動する**（resolve_tako_binary）。
  dev 検証では PATH の release 版が動く点に注意。ポート占有時のエラーは stderr 転記で
  原因が出るよう改善済み（2026-07-02。6/23 の orphan デーモンの 7749 占有が発端 → kill 済み）
- main.rs の残り 8,359 行にはまだ大きなブロックがある（render_pane 約450行、
  ControlHost 実装 966行、セルフテスト 2,800行 等）
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Issue #23 フェーズ B**: スマホ UI 刷新（WS 接続・xterm.js 色付き・エージェントビュー）
- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する
- **並行 worker と同一ワークツリーで作業する場合、未コミット変更が他者の commit/reset に
  巻き込まれる**（2026-07-02 に実際に発生）。編集 → 即コミット → 即 push を徹底する

## 現フェーズで Read すべき設計書

- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント（API 仕様の正）
- オーケストレーター修正時: `docs/orchestrator.md`
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
