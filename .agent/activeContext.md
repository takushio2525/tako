# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#435 i18n 実装完了 → 実機確認待ち。#287 実機確認も継続）

**#435 UI 日英 i18n: PR #454 merge 済み（`be0606d`）。実機確認まで Issue オープン**

- i18n: `tako-core::i18n` + `ui_text/` の `tr!(ja, en)` カタログ + 切替 3 経路
  （CLI `tako lang` / MCP `tako_lang` = 106 ツール / パレット「表示言語を切替」）。
  既定 OS ロケール解決・settings.json `language` 永続化。conventions.md に運用明文化
- #287 P1 cross-origin は merge 済み（実機確認待ち）。残 #287 所見は P1-2 identity spoof のみ

## 次の一手

- `build-app.sh --install` → 実機確認: ① #435 `tako lang en` → 主要 UI 英語 + パレット切替
  （証拠スクショ ~/Desktop/tako-435-evidence/）② remote 5 Issue + cross-origin（iPhone、
  evil origin fetch 403）
- #435 タスク 3（README 等の英語化）と #287 P1-2（Unix socket 化）は別タスクとして着手判断

## 現フェーズで Read すべき設計書

- リリースチャンネル仕様: `gh issue view 403 --comments`
- remote 計画: `.agent/plans/tako-remote-plan.md`
