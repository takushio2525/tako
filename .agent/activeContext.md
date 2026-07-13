# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#157 watch 異常検知イベント完了）

**#157（orchestrator watch の WORKER_ERROR イベント）を実装・マージ済み**（PR #190 = `9847ee5`）。
worker のエラー停止を watch が検知し `WORKER_ERROR: tako:<pane> (<種別>)` を出力、
worker_status / run にも `status: "error"` + `error: {kind, detail, recommended_action}` を 1:1 公開:

- 種別（実採取画面由来）: `api_error`→resume / `usage_limit`→wait_reset / `limit_dialog`→respond_dialog
- 検知は `orchestrator::wait::detect_worker_error`（busy/idle ヒューリスティックと同居）。
  dispatch `finish_worker_status` が idle 確定後に細分類、watch は dispatch 判定を優先しつつ
  旧 tako-app 相手でも画面から自力検知（フォールバック）
- 誤検知ガード: busy 中は判定しない（`Retrying…` は busy 継続）/「limit reached, now using」
  自動切替は除外 / api_error は末尾 15 行限定（復帰後のスクロールバック残留対策）
- run はエラー停止時 `worker_error` + auto_close スキップ（復帰余地を残す）
- master default prompt に WORKER_ERROR リカバリ手順（respawn 禁止・resume 優先）を追記

## 検証済み（#157）

- build / test（581 passed。unit 10 本追加）/ fmt / clippy(-D warnings) 全緑
- 隔離 e2e（TAKO_ISOLATED=1 + 隔離 discovery + env -u TAKO_*）: WORKER_ERROR 実測
  （api_error、35 秒で確定）/ 正常 idle は WORKER_IDLE のまま / エラー画面中の close は
  WORKER_GONE 優先 / MCP 直叩きと CLI の応答一致 / codex limit 画面で usage_limit 優先
- origin/main（#116 / #173）rebase 取り込み後に全検証を再実行

## 次の一手

- tako 再起動で新バイナリ反映（`build-app.sh --install` 済み）→ 実運用で WORKER_ERROR の
  発火を観察（次の API エラー多発時に master が自動判別できるか）
- 明朝 5:00 の夜間ジョブで自動パッチリリース見込み（#166。CHANGELOG Unreleased に #157 記載済み）
- 将来の拡張候補（スコープ外）: エラーパターンの外部設定化・claude の新 limit UI 文言の追随

## 現フェーズで Read すべき設計書

- watch / worker_status の判定構造: `crates/tako-control/src/orchestrator/wait.rs` 冒頭コメント
- オーケストレーターの使い方・イベント一覧: `.agent/orchestrator.md`「tako orchestrator watch」節
