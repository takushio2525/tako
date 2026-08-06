# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-06、Issue #778 prompt_undelivered 偽陽性）

- worktree `../tako-wt-778` / ブランチ `fix/778-prompt-delivery-false-positive`
- Issue の正本を確認し、調査結果と実装計画を Issue コメントに記録済み
- 根因を実コードで確認:
  - worker spawn と後続 send は dispatch 入口では別経路だが、同じ `PromptFlow` と
    pane 一致だけの `record_prompt_delivery` へ合流していた
  - 後続 send の失敗でも `prompt_delivery_failed_at` が立ち、失敗印を最優先する
    assessment により Delivered 済み worker が undelivered へ転落していた
- 修正:
  - `PromptDeliveryFlow`（SpawnPrompt / FollowUpSend）を追加
  - worker spawn 専用の `queue_spawn_prompt_flow` だけが初回送達状態を更新する
  - 通常 send / await_prompt / master ナッジ等の後続フローはレジストリ更新を no-op にする
  - #530 の spawn 初回ダイアログ失敗は従来どおり session 検出より優先する

## 実測・検証

- unit 2 本追加:
  - Delivered 済み + FollowUpSend 失敗 → failed_at なし / Delivered 維持
  - session 検出済み + SpawnPrompt の choice_dialog 失敗 → undelivered（#530 維持）
- 隔離セルフテスト項目 105:
  - 実 `Request::Send` → `queue_send_flow` → PromptFlow timeout → worker_status
  - `busy=true follow_up=true prompt_delivery=delivered failed_at=false undelivered_event=false`
  - 本番アプリ PID は前後不変、専用 data dir / discovery / tmux socket のみ使用
- 品質ゲート:
  - `cargo test --workspace` 全緑（1892 passed、11 ignored）
  - `cargo fmt --all --check` 緑
  - `cargo clippy --workspace --all-targets -- -D warnings` 緑
  - 隔離セルフテスト `TAKO_APP_SELF_TEST_OK`
  - 76d と 104 の描画依存検査はウィンドウ非前面で明示 skip（#778 項目は通過）

## 不変条件

- send_input の送達確認ループ自体と watch のイベント体系は変更しない
- #530 の「起動 ≠ 初回プロンプト到達」を維持する
- 本番 GUI・本番 tmux socket `tako`・本番 data dir に触れない
- System Events のキーストローク送出は禁止

## 次

- コミット → push → PR（Closes #778）→ macOS CI 緑確認 → squash merge
- Issue に実測証拠をコメントしてクローズ
- main 同期・worktree 削除後、他ビルド不在を確認して `scripts/build-app.sh --install`
