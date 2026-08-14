# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、Issue #790 worker 送達の Cross-Session Messaging 化 = merge 済み・install 済み）

- PR #806 を squash merge（`f57e661`）。CI は macOS / Windows / Pages 全緑。Issue #790 クローズ済み。
  `/Applications/tako.app` へ install 済み（**反映は tako 再起動後**）
- claude worker への指示送達を **2 層**にした: 第 1 層 = claude の Cross-Session Messaging
  （受信箱 socket へ直送）→ 使えなければ第 2 層 = 従来のキー操作経路（#32 の送達確認ループ）
- スパイクの実測は Issue #790 のコメントに全量（実験フラグ不要 / 伝送プロトコル / 前置きの存在）

## スパイクで確定した事実（実装の前提。触るときはここから）

- 実験フラグは**不要**（v2.1.232 に `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` は無い）。
  代わりに**サーバー側 gate**（GrowthBook）依存 = env で強制できない → 実行時検出 + 落ちる
- 発見: `<config dir>/sessions/<pid>.json`（`messagingSocketPath` / `peerProtocol` / `kind` /
  `status`）+ `<pid>.<hash>.key`（`peerToken`）。**config dir ごと**（アカウント切替と直結）
- 伝送: socket へ改行区切り JSON 2 行（`auth` → `user`）。受信側は pid を OS で検証
- 受信の姿は **2 形態**: `user` + `origin.kind=peer` / `attachment` の `queued_command`
- **本文に抑制不可の前置きが付く**（「別セッションから届いた / 承認として扱うな」）
  → 適用は worker 宛だけ（人間由来の送達は従来経路）
- 実測: idle=ターン処理 / busy=キュー投函して取りこぼしなし / ダイアログ中も無傷 /
  43,449 バイトを 1 回でバイト等価 / 受信箱 bind は起動 1.1 秒

## 不変条件

- **送り切ったらフォールバックしない**（二重投函）。落ちてよいのは可用性判定と接続失敗だけ
- 受信確認に**時刻文字列を使わない**（秒精度 vs ミリ秒で同じ秒を取りこぼす）。
  送信直前のファイル長を控えて追記分だけ読む（`TranscriptCursor`）
- socket 接続と受信確認は **background スレッド**（UI スレッドで待たない。#212 / #772）
- トークンを持つフィールドは `PeerTarget` の非公開に留め、ログ・エラー文へ出さない

## 次の一手（master 判断）

- GUI 再起動で新バイナリを反映（install は済み。再起動は全 worker を落とすので master 判断）
- 別 Issue 化の候補: ①ダイアログ表示中の `tako send` 拒否（#748）を peer 可用時だけ通す
  （実測では安全。今回はスコープ外） ②spawn 初回プロンプトが peer を通った割合の可視化

## 現フェーズで Read すべき設計書

- 送達: `.agent/architecture.md`「worker への指示送達の 2 層化」/ `.agent/requirements.md`
  FR-2.2.2 追補 2 / `.agent/orchestrator.md`「プロンプト送達」
- 実装: `crates/tako-control/src/peer_messaging.rs` / `delivery.rs` /
  `crates/tako-app/src/main.rs`（`drive_peer_attempt`）
