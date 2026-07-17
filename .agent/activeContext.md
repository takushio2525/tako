# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#312 修正完了）

**Issue #312: macOS ウインドウ操作の不備 2 件を修正 — PR #318 squash merge 済み**

- タブバー空き領域ドラッグでウインドウ移動（`start_window_move`）
- ダブルクリックでズーム（`titlebar_double_click`）
- 赤ボタン close 前に layout 保存（`on_window_should_close`）
- Dock クリックで保存済みレイアウトからウインドウ復帰（`on_reopen`）

## 検証

- cargo fmt / clippy(-D warnings) / test 全緑（476 passed）
- 隔離セルフテスト完走（`TAKO_APP_SELF_TEST_OK`）
- ユーザー目視確認待ち（Issue #312 に目視チェックリスト記載）

## 次の一手

- #312 のクローズはユーザー目視確認後
- install はユーザー指示で `build-app.sh --install`

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
