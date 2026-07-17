# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#338 実装完了）

**Issue #338: プレビューペインにチェンジログビュー切替を実装 — PR #348 作成済み**

- 「履歴」トグルボタンでコードプレビュー ⇔ git 履歴ベースのチェンジログビューを切替
- チェンジログビュー: ファイル単位のコミット一覧 + クリックで diff 展開/折りたたみ
- CLI `tako preview-changelog` + MCP `tako_preview_changelog`（計 95 ツール、1:1）
- git 管理外ファイルでは安全に「git 管理外のファイルです」を表示

## 検証

- cargo build / fmt / clippy(-D warnings) / test 全緑（606 passed、新規テスト 4 本含む）
- 隔離セルフテスト: ランダム 1 件（タイミング依存・本タスク外）。ツール数 95 は通過
- 実機確認待ち（build-app.sh --install → tako 再起動）

## 次の一手

- `build-app.sh --install` → tako 再起動 → ユーザー実機確認
- #338 クローズは master 判断
- worktree `~/dev/tako-wt-338` の削除は merge 後

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
