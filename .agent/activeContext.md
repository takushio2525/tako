# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-18・#339 実装完了）

**Issue #339: 複数ウィンドウ対応（ビューポート方式）**

- コミット `52bb49d` on main、PR #367 squash merge 済み（CI は disabled_manually のため作成直後マージ）
- Issue に実測証拠コメント済み（auto-close。実機確認待ちの項目もコメントに記載）
- worktree `~/dev/tako-wt-339` は除去済み

## 次の一手

- `build-app.sh --install` で .app 更新 → **実機目視: New Window（⌘⇧N）でタブ・状態が同期された
  追加ウィンドウが開くこと**（元 FAIL 報告の再確認）/ 最後の 1 枚の赤ボタン close → Dock 復帰（#312）
- #364 の実 claude ペイン report e2e + codex fallback 実測（前タスクからの持ち越し）
- #287 の master レビュー・main マージ判断（renewal/remote-transport）
- v0.6.0 リリース判断（#339 + #364 同梱）

## 既知の妥協点（#339。将来改善候補）

- サイドバー / 右パネル / ドロワーは全ウィンドウに描画されるが内容はアクティブウィンドウ基準
- 非アクティブウィンドウのタブバー横スクロール位置は共有（tab_scroll_handle 単一）
- Web ビューを含むタブを別ウィンドウへ移すと wry の親はプライマリウィンドウのまま（未検証）

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
