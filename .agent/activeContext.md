# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-23・MCP/IPC 再起動耐性の強化 完了）

tako の MCP サーバーとセッション管理の再起動耐性を改善。
⌘Q → 再起動で MCP クライアントから全操作不能になる問題を 3 点の変更で解消:

1. **IPC ソケットの固定パス化**: `/tmp/tako-{PID}-{SEQ}.sock` → `<data_dir>/tako.sock`
2. **認証トークンの永続化**: `<data_dir>/token` に保存し再起動をまたいで再利用
3. **discovery cleanup の条件化**: persist ON 時の ⌘Q で接続情報を保持

tmux セッション内の既存クライアント（古い TAKO_SOCKET/TAKO_TOKEN 環境変数を持つプロセス）が
再起動後もそのまま再接続できるようになった。

## 残作業・既知の制約

- MCP HTTP サーバー（Streamable HTTP）のポートはまだランダム。ただし Claude Code は
  stdio ブリッジ（`tako mcp serve`）経由なので影響なし。直接 HTTP で接続する
  クライアント向けにはポート固定化が今後の改善点
- セルフテスト（`TAKO_SELF_TEST=1`）では従来通り一時パス・一時トークンを使用（隔離）
- 多重起動時は 2 番目以降のインスタンスがフォールバックで一時パスを使用

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する
- main.rs に未コミットの UI 変更（BoxShadow、padding 等）と terminal.rs の agent metrics 改善が残っている

## 現フェーズで Read すべき設計書

- オーケストレーター修正時: `docs/orchestrator.md`
- タブツリー/プレビュー/ピン再修正時: `requirements.md` FR-2.15 / FR-2.16
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
