# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-15・タブ退避 + tmux orphan 修正完了）

- **FR-2.15 タブ単位退避**: 最小化ボタン（ー）+ D&D 退避を実装
- **tmux D&D タブの orphan 化防止**: `drop_tmux_view_session()` 新設で全 close 経路を修正
- **tmux パネルに退避中セクション**: 退避ペインの状態表示 + 復帰ボタン
- **tako 終了 → `scripts/build-app.sh --install` → 再起動** をユーザーに依頼して実機確認
- 最終更新: 2026-06-15

## 残作業・既知の制約

- コンテキストメニューの位置がサイドバー基準でなくウィンドウ基準になる可能性
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知）
- git パネルのコミットグラフは現在テキストベース

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）
- **Edit ツールのフックが変更を巻き戻す**: Bash + python3 での一括パッチが安全
- **インライン編集 UI**: `handle_key` の冒頭で `inline_edit.is_some()` をチェック

## 現フェーズで Read すべき設計書

- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」
- 配布・オンボーディング着手時: `roadmap.md` Phase 7

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
