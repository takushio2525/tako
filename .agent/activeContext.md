# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#181 スクロール体感問題の根治。#167 は main へマージ済み）

**#181（#159 スクロール改善が実機で体感できない）を根本修正**。根因は 3 つ + カクつき 1 つ:

1. ミラー経路の分岐が `backend_sessions` のみ → `tako tmux open` の TmuxOpen ビューペインが
   直接ペイン扱いに落ち、外側 alacritty は alt screen（履歴 0）でホイール・スクロールバー不発
2. persist ON ではビューペインの外側 PTY 自体も backend ラップされる（実測: ラッパー client_tty
   = backend pane_tty）→ backend 優先の実体解決だと外側（history 0）へ誤解決
3. persist 復元で戻ったビューペインは `tmux_view_panes` 未登録 + ネスト候補が既定サーバーのみで
   `--socket tako` のビュー先を辿れない → ネスト候補に backend socket を追加して tty 突き合わせ
4. カクつき = `OrchestratorWorkerStatus` dispatch が `claude agents --json`（550〜1100ms）を
   UI スレッド同期実行（perf.log 2h で 2000 件超、時刻がユーザー報告と一致）→
   IPC ループで snapshot（UI）/ compute（background executor）に分離

既知制約（仕様化）: alt screen TUI（claude Code / vim）内のスクロール粒度はアプリ依存で
スムーススクロール対象外。カスタム `-L` 外部サーバーのビューは復元後にネスト検出不能
（開き直せば回復）。いずれも manual-checks.md / requirements.md FR-2.5.13 に記載。

**main 取り込み済み（#167 = PR #184）**: マウスレポートは `send-keys -H` 直接注入 +
レート制限へ（wants_mouse=true 側の転送経路。#181 対象の mirror 経路とは独立）。
`history_state` の `HistoryState` 構造体化・`ScrollCtl` 新フィールドとの整合を
マージ後ビルド + セルフテストで確認する。

## 検証済み（#181。マージ前の fix/181 単体）

- 全 551 テスト / fmt / clippy(-D warnings) / 隔離セルフテスト完走（項目 73 = TmuxOpen ミラー
  e2e、74 = worker_status IPC 応答を新設）/ visual-test subline direct=22197 shifted=0（#176 一致）
- 隔離実機 e2e（TAKO_ISOLATED + 隔離 HOME、本番不接触）: バックエンド（history 275）/
  ビュー（276）/ 復元通常（275）/ 復元ビュー = backend socket 上（273）の各ミラー + スクロール
  バー描画をキャプチャ実証。worker_status 15 連打（各 174〜239ms 実負荷）中の scroll 応答
  24〜34ms 安定・隔離 perf.log 0 件
- 調査中の事故: CLI が TAKO_SOCKET 注入で本番へ誤接続（ビューペイン 1 個生成 → close 復旧済み）。
  以降の検証は `env -u TAKO_*` を徹底。Issue #181 コメントに記録済み

## 次の一手

- origin/main（#167）とのマージ整合を検証（ビルド + テスト + 隔離セルフテスト再実行）→
  PR #186 を squash merge --delete-branch → fetch + detach → `scripts/build-app.sh --install`
- ユーザー再確認: 本番の tako-view ペイン（405/400/420）でスクロール + カクつき解消の体感確認
- 明朝 5:00 の夜間ジョブ監視（v0.4.1 自動リリース見込み。#166）は継続

## 現フェーズで Read すべき設計書

- スクロールのミラー経路・実体解決（#181 で改稿）: `.agent/architecture.md`「スクロール制御」節
- マウスレポート転送（#167）: `.agent/architecture.md` 該当節
- スクロール要件・既知制約: `.agent/requirements.md` FR-2.5.13
- UI スレッドで外部プロセス禁止の教訓: `.agent/architecture.md`「UI スレッド同期処理」節
