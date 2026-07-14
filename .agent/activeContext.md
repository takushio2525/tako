# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-14・#212 画面が重い・点滅・スクロールもっさり）

**#212 修正完了**（worktree tako-wt-212 / fix/212-perf-flicker。PR 準備中）:

- 犯人: sleep guard（#173、v0.5.0）の AC 判定 `pmset -g batt` が UI スレッドで
  2 秒毎に同期実行（アイドル 20〜30ms、CPU 飽和時に秒級 = perf.log の periodic_prep
  スパイク 0.8〜2.9s と一致）。外因（worker 4 体の cargo build 並走 = load avg 最大 161・
  swap 10.5/11GB・ディスク 99%）との複合で症状顕在化
- 修正: ① `on_ac_power` を IOKit FFI（`IOPSGetTimeRemainingEstimate`）へ置換
  ② periodic_prep にステップ別サブスパン追加 ③ perf.log 並行書き込みの行混線を単一 write 化
- 実測: 隔離アイドルで periodic_prep p50 17〜59ms / max 116ms → **p50 0ms / max 8ms**。
  FFI の AC 判定は pmset と一致（実機 AC 接続で -2.0 / true）

## 次の一手

- origin/main（#213〜#215 が進行）を rebase 取り込み → 全ゲート再実行 → push →
  PR（Closes #212）→ squash merge → 本体リポで `build-app.sh --install` →
  Issue に実測証拠つき完了コメント（症状解消の最終確認はユーザー体感・再発時 reopen）

## 現フェーズで Read すべき設計書

- ストール診断（perf_span / サブスパン / UI スレッド禁止事項）: `.agent/architecture.md` 診断節
