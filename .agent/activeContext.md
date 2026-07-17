# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#313 修正完了）

**Issue #313: git タブがファイルツリーの表示リポジトリに追随しない問題の根治 — PR #331 squash merge 済み**

- 根因: git タブ・サイドバー git サマリが `active_tab_cwd()`（フォーカスペインの cwd のみ）を参照。ファイルツリーは全ペイン cwd + pinned フォルダを集約するが git タブはこのソースを見ていなかった
- 修正: `git_cwd_for_tab()` を新設。フォーカスペイン → 他ペイン → pinned → background ペインのフォールバック検索。`.git` 親方向走査の軽量チェック `has_git_ancestor()` はプロセス spawn なし

## 検証

- cargo build / fmt / clippy(-D warnings) / test 全緑（282 passed、新規テスト 4 本含む）
- 隔離セルフテスト 2 回とも exit 0
- 実機確認待ち（build-app.sh --install → tako 再起動）

## 次の一手

- `build-app.sh --install` → tako 再起動 → ユーザー実機確認
- #313 クローズは master 判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
