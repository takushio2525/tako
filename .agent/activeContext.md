# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#315 修正完了）

**Issue #315: PDF プレビューのリンク ⌘クリック無反応を根治 — PR #323 squash merge 済み**

- canvas paint callback でページ画像 bounds を直接記録（テキストレイヤ不要）
- estimate_pdf_page_bounds（85 行の逆算ロジック）を廃止
- 全描画ページのリンクチェック + 全 PDF ペインのホバー検出
- ⌘ホバーでカーソル変化（PointingHand）+ リンク下線ハイライト

## 検証

- cargo fmt / clippy(-D warnings) / test 全緑（849 passed）
- PDF ヒットテスト単体テスト 2 本追加・通過
- 隔離セルフテスト完走（PDF 関連項目全通過）
- ユーザー目視確認待ち（Issue #315 に確認手順記載）

## 次の一手

- `build-app.sh --install` → tako 再起動 → ユーザー目視確認
- #315 クローズは master 判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
