# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-10・#120 worker の codex/agy 対応 merge 済み + #118 マージ待ち）

worker のエージェント CLI を claude / codex / agy から選択可能にする #120 が完了
（PR #122 squash merge `f8a8b3c` + build-app.sh --install 済み。反映は tako 再起動後）。

- `orchestrator::agent` 新設 + TUI 検出の和集合化（❯/›/>）+ Profile `worker_agent`/`worker_agents`
- spawn / run / profiles の agent 系を MCP・CLI に 1:1 公開（ツール数 53 のまま）
- codex / agy の status は画面推定（agents API 非対応）。agy は
  `profiles set --agent agy --agent-skip-permissions true` が実用上ほぼ必須
- 並行作業: #118 FDA ガイド（`fix/118-fda-guide` → PR → squash merge 待ち）、
  全体 / remote の監査は `reviews/2026-07-10_gpt5.6solレビュー.md` ほかに保存済み

## 残作業・既知の制約

- tako 再起動で新バイナリ（#120 入り 0.3.2）反映 → codex/agy worker の通常利用確認
- #118 のコミット・PR・マージ・build-app.sh --install・実機検証が残
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）は 6/12 以降停止中（無料枠逼迫）。品質保証はローカル全緑で代替

## 未着手タスク（優先順はユーザーと相談）

- [ ] **#115 GitLog / GitDiff dispatch の background 化**
- [ ] **#116 tako-coretest-* ソケット残骸の掃除・再発防止**
- [ ] **#112 セッション会話ログの管理と復元**
- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- FDA / setup 修正時: `crates/tako-control/src/fda.rs` + `crates/tako-cli/src/setup.rs`
- solo / オーケストレーター修正時: `.agent/orchestrator.md`
- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
