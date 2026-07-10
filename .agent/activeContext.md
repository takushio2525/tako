# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-10・#118 FDA ガイド実装完了）

macOS の TCC フォルダアクセス許可ダイアログが毎回表示される問題への対策として、
フルディスクアクセス（FDA）の検出・案内機能を実装。

- `tako-control::fda` モジュール新設（FDA 状態検出 + システム設定オープン）
- dispatch `Fda` + MCP `tako_fda`（計 53 ツール）+ CLI `tako fda status/open`
- `tako setup --check` に FDA チェックを追加（未付与時にシステム設定を開く提案）
- ブランチ `fix/118-fda-guide` → PR → squash merge 待ち

## 残作業・既知の制約

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
