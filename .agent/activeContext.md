# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-07・v0.3.2 リリース済み + リレー worker デプロイ済み）

**v0.3.2 リリース完了**。#109（複数 master 並行時に spawn worker が意図しないタブに出る）を修正。
MCP セッションに `caller_role`（`TAKO_ORCHESTRATOR_ROLE` 環境変数由来）を追加し、
`caller_pane` が stale で `resolve_pane` に失敗した場合でも role suffix から正しい master を
特定するフォールバックを実装。回帰テスト 3 本追加。

- v0.3.2 tag + GitHub Release + Pages デプロイ + build-app.sh --install 済み
- リレー worker レートリミットを本番反映済み（`npm run deploy`、正常系 register/resolve 確認済み）
- **tako 本体の再起動が必要**（稼働中プロセスは旧バイナリのまま）

---

## 残作業・既知の制約

- main.rs は 9,800 行前後。さらなる分割は別タスク
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）が 6/12 以降トリガーされていない — Actions 無料枠逼迫で停止中
- cask 0.3.2 更新は未実施（`homebrew-tako` 側）
- KV の eventual consistency でレートリミットのバースト耐性は緩い（1 分窓では効く）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント
- オーケストレーター修正時: `.agent/orchestrator.md`（品質パイプラインの表）+
  `crates/tako-control/src/orchestrator/default_system_prompt.md`
