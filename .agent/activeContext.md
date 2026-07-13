# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#167 マウスエスケープ断片の入力欄混入を根治）

**#167（SGR マウスレポート断片 `4;45;18M` / `<64;12;17M` が claude 入力欄に平文混入）を根本修正**。
隔離 tmux + 実 claude で機序を実測確定: 外側クライアント PTY へ書いた SGR シーケンスが
途中で 10ms 以上途切れる（慣性スクロール洪水の部分 write + UI/イベントループ停滞）と、
tmux（escape-time 10ms）が ESC を単独キー確定し残りを平文として内側へ転送する。
仮説①（モードレース）②（単純分割 write）は tmux が正しく再構成することを確認し棄却。

対策（二層）:
- **バックエンドペインのホイールレポートは外側 PTY を通さない**: `scroll_mirror::send_wheel`
  （`tmux send-keys -H` 直接注入）+ UI 層 `pump_wheel`（in-flight 1 本直列化）。
  SGR / X10 は `#{mouse_sgr_flag}`（`HistoryState`）で出し分け
- **全ホイール転送にレート制限**（`terminal.rs`）: トークンバケット 150 イベント/秒・
  バースト 8（macOS PTY バッファ 1024B に対する安全マージン）

詳細は `.agent/architecture.md`「マウスレポート転送（#167）」節。

## 検証済み（#167）

- workspace build / test（551 passed）/ fmt / clippy(-D warnings) 全緑 + 隔離セルフテスト完走
- e2e 新設 2 本: 洪水 2100 イベントで断片ゼロ + レート制限の存在 / send-keys 注入が生で届く
- 実 claude before/after（隔離 tmux、本番不接触）: before = PTY 書き込み洪水で入力欄に断片
  大量混入を再現 / after = 新経路で idle 1500 + busy 588 イベント断片ゼロ
- 教訓: capture-pane の断片判定は `-J`（折り返し結合）必須（80 桁折り返しの誤検出を踏んだ）

## 次の一手

- fix/167 の PR を squash merge → `build-app.sh --install` → tako 再起動で実機反映
- **並行 worker #181**（#159 スクロールが再アタッチペインで体感できない）が同じスクロール
  制御群を変更中（tako-wt-181）。`history_state` のシグネチャ変更・`ScrollCtl` 新フィールド・
  wants_mouse=true 経路の send-keys 化を Issue #181 コメントで共有済み。rebase 側が対応する
- fix/177 の残タスク: 明朝 5:00 の夜間ジョブ初回実行を監視（v0.4.1 自動リリース見込み。#166）
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- マウスレポート転送の設計（#167）: `.agent/architecture.md` 該当節
- 多重インスタンスの資源保護（#177）: `.agent/architecture.md` 該当節
- スクロールの要件（#159 で全面改稿 + #167 で転送経路更新）: `.agent/requirements.md` FR-2.5.13
- spawn レイアウトの設計（#165）: `.agent/architecture.md`「spawn レイアウトエンジン」節
