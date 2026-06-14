# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-14・パフォーマンスバグ修正 2 回目）

- **tmux ポーリングの非同期化**: 2 秒ポーリングの `refresh_tmux_data` が 6 回の同期
  tmux サブプロセス実行（計 25〜50ms）で UI スレッドをブロックしていた問題を修正。
  コンテキスト収集のみ UI スレッド（< 0.1ms）、tmux コマンドは background executor に移行。
  `TmuxOpen` の存在確認も `list_sessions`（3 コマンド）→ `has_session`（1 コマンド）に軽量化
- cargo test 88 pass・clippy / fmt 緑・`.app` 生成済み（`dist/tako.app`）
- **tako 終了 → `scripts/build-app.sh --install` → 再起動** をユーザーに依頼して実機確認
- 最終更新: 2026-06-14

## 残作業・既知の制約

- コンテキストメニューの位置がサイドバー基準でなくウィンドウ基準になる可能性
  （GPUI の `position` がウィンドウ座標のため。実機で確認してから調整）
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知。今回無関係）

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.6 git graph / FR-3.5 軽い編集 / FR-3.9 diff ビューア
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）
- [ ] **FR-2.15 ターミナルのたまり場**（UI の見せ方をユーザーと相談してから着手）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）
- **tmux ポーリングの非同期化パターン**: UI スレッドで `collect_tmux_context()` →
  background で `fetch_tmux_sessions()` → UI スレッドで `self.tmux_sessions` 適用。
  dispatch 層（CLI/MCP 用）はそのまま同期で残す
- **Edit ツールのフックが変更を巻き戻す**: Bash + python3 での一括パッチが安全
- **インライン編集 UI**: `handle_key` の冒頭で `inline_edit.is_some()` をチェック

## 現フェーズで Read すべき設計書

- FR-3.6 git graph 着手時: `architecture.md`「コンセプト②の実現」
- 配布・オンボーディング着手時: `roadmap.md` Phase 7

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
