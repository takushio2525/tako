# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-14・#217 UI 大刷新: Claude Design カンプの忠実再現）

**#217 実装完了**（worktree tako-wt-217 / feat/217-ui-redesign。PR 準備中）:

- カンプ: `design/claude-design/tako-ui/project/tako Desktop 改善版.dc.html`（コミット済み）が正
- M1〜M7 全マイルストーン完了: テーマ基盤（ライト/ダーク + `tako theme` + MCP `tako_theme`、
  Catppuccin Mocha=カンプ実値 / Latte=ライト）→ ピル型タブバー（⌘K エントリ・ベル・テーマボタン、
  タイトルバー統合）→ ペインヘッダ（番号バッジ・workers ▾・↳ 親リンク・cwd チップ・再実行）→
  サイドバー（ブランチチップ・パスコピー・git サマリ）→ ステータスバー（breadcrumb・
  5h/週リミットメーター・ctx 改良）→ orch ビュー + トースト + ⌘K パレット → 絵文字全廃（grep 0 件）
- UI アイコンは `assets/icons/ui/*.svg`（カンプの SVG パスを忠実に写経、gpui::svg() マスク描画）
- origin/main rebase 済み（#220 sleep-guard 蓋閉じと status_bar でコンフリクト → 解決済み）

## 次の一手

- PR（Closes #217）→ squash merge → 本体リポで `build-app.sh --install` →
  Issue に実測証拠 + ユーザー目視チェックリスト

## 現フェーズで Read すべき設計書

- カンプの実値確認: `design/claude-design/tako-ui/project/tako Desktop 改善版.dc.html`
- テーマトークン対応: `crates/tako-core/src/theme.rs`（カンプ色 → トークンのコメント付き）
