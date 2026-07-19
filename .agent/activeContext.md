# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-19・#371 完了）

**Issue #371: タブ D&D 挿入位置インジケータ** — PR #373 squash merge 済み（`55f9c31`）

## 次の一手

- #368 / #369 の着手判断（#368 が効果最大: アイドルの見えない消費 4% 相当）
- 残骸プロセスの掃除判断（ユーザー）: 5.8 日常駐の headless Chrome（/tmp/tako-chrome-cdp、
  #155 廃止 PoC の残骸）/ tako-wt-285 の vite / 検証 tmux サーバ残骸 25 個 → 詳細は #340 コメント
- `build-app.sh --install` で .app 更新 → 実機目視: New Window（⌘⇧N）の状態同期（#339 持ち越し）
  + #340 修正の本番 perf.log で sleep_guard 専有が消えることの確認
  + **#371 のタブ D&D インジケータの本番目視確認**
- #364 の実 claude ペイン report e2e + codex fallback 実測（持ち越し）
- #287 の master レビュー・main マージ判断（renewal/remote-transport）
- v0.6.0 リリース判断（#339 + #364 + #340 + #371 修正同梱）

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
