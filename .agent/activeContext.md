# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#159 ターミナルスクロール大幅改善）

#159 実装完了（fix/159-smooth-scroll worktree）。①ピクセル単位スムーススクロール
（表示位置 = display_offset - fract の行小数分解 + サブライン描画）②操作感改善
（1 行未満切り捨て廃止 → Pixels デルタを行小数のまま反映、慣性は OS momentum）
③スクロールバー強化（ホバー維持 + サム強調 + トラック）④バックエンド(tmux)ペインを
copy-mode 駆動 → **ローカル履歴ミラー方式**へ刷新（tako-core::scroll_mirror 新設）。

- Zed 比較所見: Zed ターミナルは行単位のまま。ピクセルオフセットは Zed エディタの
  scroll_position (f64 行小数) 方式で、それをターミナルの下端アンカーに翻案した
- 外側 alacritty に tmux 履歴は積もらない（実測 outer_history=0）→ ミラー方式が必然
- 既知制約: ミラー表示中（バックエンド過去閲覧中）の選択・cmd+クリックは無効

## 検証済み

- 実ピクセル実証: visual-test「半行戻しでほぼ一致」direct=22197 / shifted=0
- 隔離セルフテスト全項目パス（44b サブライン / 61b-61e ミラー・CLI 経路）
- workspace build / test（500 テスト）/ fmt / clippy（-D warnings）全緑

## 次の一手

- PR（Closes #159）→ squash merge → fetch + detach → build-app.sh --install
- tako 再起動後、`.agent/manual-checks.md`「ターミナルスクロールの大幅改善」節の
  操作感チェックリスト（トラックパッド慣性・バー操作・TUI 整合）を人手確認

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-2.5.13（#159 で全面改稿）
- 手動確認: `.agent/manual-checks.md`「ターミナルスクロールの大幅改善」
