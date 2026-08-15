# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、#813 リミット後の自動復帰）

- ブランチ `feat/813-limit-autoresume`（worktree `~/dev/tako-wt-813`）。base は `af625b1`（v0.7.1）
- ペイン単位でオプトインすると、5h / 週次上限で止まったエージェントを
  リセット時刻後に tako が自動で再開させる（FR-2.27 新設）

## 層の分け方（この機能の読み方）

- `tako_core::limit_resume` — **純関数だけ**。発動判断（`decide`）・リセット時刻の
  パース（`parse_reset_at`。now とタイムゾーンは引数で注入）・安全な選択肢の選別
  （`safe_choice` = 許可リスト **かつ** 拒否リスト）
- `tako_control::limit_stop` — 既存の検知（#748 のダイアログ種別 / #157 の画面パターン）を
  束ねて `LimitStop` にするだけ。**新しい検知規則は足していない**
- `tako-app::limit_autoresume` — 2 秒 tick の駆動。有効ペインが 0 なら即 return

## 踏み抜きどころ（次に触る人へ）

- **リセット時刻はエピソード開始時に確定して途中で更新しない**。画面には古い上限
  メッセージが残るので、毎 tick 読み直すと復帰予定が後ろへずれ続ける
- 裏返しの既知の限界: 上限中に tako を再起動すると初観測が「今」になるため、
  すでに過ぎたリセット時刻は**翌日の同時刻**として解釈される（安全側だが復帰しない）
- ダイアログへの応答は `respond_to_choice_dialog`（dispatch から切り出したホスト非依存版）を
  **background** で呼ぶ。UI スレッドで呼ぶとキー送出のスリープでフレームが止まる
- `safe_limit_choice`（supervisor #401）も core の `safe_choice` へ寄せたので、
  拒否リストは自動復帰と supervisor の両方に効く

## 検証の状態

- 品質ゲート: fmt / clippy(-D warnings) / `cargo test --workspace` 全緑
- 隔離セルフテスト: 1 回目は**項目 99（#739 スターターのプロファイル ▾）**で FAILED。
  項目 111 より前で落ちるため未到達 → main 由来かの切り分け中
- 隔離 e2e（項目 111）: 正例 2 型（ダイアログ / idle）+ 負例 3 型（OFF / permission /
  api_error）+ 試行上限 + list・read の一致

## 次の一手

- セルフテスト完走の確認（#739 が main 由来なら Issue へ切り出す）
- visual-test 全節 → PR（`Closes #813`）→ macOS CI → squash merge → `build-app.sh --install`

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-2.27（安全条件と発動条件の正）
- 使い方: `.agent/orchestrator.md`「リミット後の自動復帰」
- 既存資産: #748（`tako_core::dialog` / `claude_tui::DialogKind`）・#157（`wait::detect_worker_error`）・
  #749（`drive_handoff_nudge` = 2 秒 tick の先例）・#401（`orchestrator::supervisor`）
