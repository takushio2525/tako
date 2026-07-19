# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-19・#372 修正完了）

**Issue #372: sleep-guard busy_agents 漏れ** — PR #389 squash merge 済み（`f652dc8`）

## 次の一手

- `build-app.sh --install` で .app 更新 → 本番で `tako sleep-guard status` の busy_agents 確認
- #364 の実 claude ペイン report e2e + codex fallback 実測（持ち越し）
- #287 の master レビュー・main マージ判断（renewal/remote-transport）
- v0.6.0 リリース判断

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
