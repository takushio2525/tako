# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#152 PDF 選択描画 / 標準言語色分け）

#152 を実装・ローカル検証済み。PR 作成・merge・`build-app.sh --install` が次工程。

- PDF canvas の static position が画像下端になる根因を `top_0 + left_0` で修正
- PDF 選択矩形を専用最前面 `paint_layer` へ分離
- syntect 入力の行末改行を維持し、読み取り / 編集の構文解決を共通化
- syntect 同梱標準言語セット全体 + TypeScript の JavaScript fallback
- `visual-test` feature で Metal scene を RGBA 読み戻しし、実ピクセル差分を検証

## 検証済み

- PDF 選択: 2,475px 変化
- C++: 読み取り 7,173px / 編集 7,277px 変化
- Python: 読み取り 7,089px / 編集 7,193px 変化
- workspace build / test（483 passed）/ fmt / clippy 全緑
- 通常隔離 selftest が `TAKO_APP_SELF_TEST_OK` まで完走

## 次の一手

- fix/152-preview-drawing を commit / push → PR（Closes #152）→ squash merge
- main 同期後に `scripts/build-app.sh --install`、インストール済みバイナリを確認
- ユーザー最終確認: 実 .app の実マウス PDF ドラッグと任意コードファイルの見た目

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-3.2〜FR-3.5
- 手動確認: `.agent/manual-checks.md`「PDF 選択描画・標準言語セット色分け」
