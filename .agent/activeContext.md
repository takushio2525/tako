# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-11・#124 PDF テキスト選択 merge 済み）

#124 PDF プレビューのテキスト選択・クリップボードコピーが完了
（PR #125 squash merge `ba0bc7a` + build-app.sh --install 済み）。

- PDFKit FFI でテキストレイヤ抽出 → 既存 preview_line_bounds/texts に統合
- ドラッグ選択・⌘C コピー・ハイライト描画が Code/Markdown と同じパスで動作
- テキストなし PDF でもクラッシュしない防御処理 + テスト 2 本追加
- GUI でのテキスト選択操作は実機マウスドラッグが必要（未検証、要ユーザー確認）

## 残作業・既知の制約

- GUI テキスト選択の実機検証が未了（ユーザーによるマウスドラッグ操作が必要）
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

- PDF プレビュー修正時: `crates/tako-app/src/preview.rs` の `pdf_render` モジュール + `preview_render.rs` の PDF body 生成
- solo / オーケストレーター修正時: `.agent/orchestrator.md`
- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
