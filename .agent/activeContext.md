# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#168 パフォーマンス改善: メインスレッド非ブロック化）

**#168（アプリ全体の間欠フリーズ・PDF/入力モサモサ）+ #115（GitLog の UI 同期実行）を根本修正**。
perf.log 実測（本番 3.3h 分）で 3 犯を確定し、いずれも before/after 実測済み:

1. **OrchestratorWorkerStatus dispatch**（4124 回 × avg687ms の UI 専有。UI ストール全件と共起）
   → `dispatch::prepare_offload` / `OffloadJob` で UI は文脈収集のみ・実行は background
   （IPC/MCP 両経路。`TAKO_OFFLOAD=0` で旧経路）。`claude agents --json` に TTL 2s キャッシュ。
   実測: 並行 list 159〜204ms → 4〜5ms、専有記録 5/5 回 → 0 件。
   **#181（PR #186）が同じ問題を worker_status_snapshot/compute で先行修正しており、
   マージ時に OffloadJob 機構へ一本化した**（GitLog/GitDiff + TAKO_OFFLOAD + キャッシュを包含。
   #181 のテストは検証内容を維持して新 API へ移植: unit 4 本 + セルフテスト 74）
2. **PDF 表示中の毎フレーム Image 再構築**（71p PDF で render p50 96ms）
   → `preview_render::PreviewImageCache`（Arc<gpui::Image> を path 不変の間再利用）。
   実測: p50 96ms → 1〜3ms（初回構築 110ms × 1 回のみ）
3. **PDF/動画ロードの UI 同期実行**（open 1354ms ブロック）
   → Loading プレースホルダ + `pending_preview_loads` → `spawn_preview_load` で background 化。
   実測: open 応答 1354ms → 48ms

恒久診断: `diag::perf_span`（32ms 超をタグ付き記録・2s 超ハングの中間報告・
`TAKO_PERF_VERBOSE=1` で 10s ごとタグ別分布・`TAKO_PERF_LOG` でログ先差し替え）。
白判定: save_layout/flock（p50 0〜7ms）・リンク走査（cmd 押下中のみ）・通常 render（2ms）。

## 検証済み（#168）

- workspace build / test / fmt / clippy(-D warnings) 全緑 + 隔離セルフテスト完走
  （PDF 3 項目 66/66b-2/70 は background 読み込みの完了待ちポーリングへ更新）
- 隔離 A/B 計測（TAKO_ISOLATED=1 + TAKO_PERF_LOG 分離 + env -u TAKO_* で本番不接触）
- GitLog offload: 200 commits 応答 104ms・メインスレッド専有記録なし（#115 受け入れ条件）
- origin/main（#181 = PR #186）との rebase 整合: worker_status 分離実装を OffloadJob へ
  一本化し、rebase 後に build / test / セルフテスト再実行

## 次の一手

- PR #187 squash merge → fetch + detach → `build-app.sh --install` → tako 再起動 →
  ユーザー体感の再確認依頼（PDF 閲覧・プロンプト入力・master 稼働中の全体カクつき）
- 明朝 5:00 の夜間ジョブ初回実行を監視（v0.4.1 自動リリース見込み。#166）
- 将来の最適化候補（スコープ外）: PDF 初回キャッシュ構築 110ms の background 化、
  #84（MCP HTTP 直列）は offload で dispatch 詰まり解消後に実害を再計測してから

## 現フェーズで Read すべき設計書

- メインスレッド非ブロック化の設計（#168）: `.agent/architecture.md` 該当節 + NFR-8
- スクロールのミラー経路・実体解決（#181）: `.agent/architecture.md`「スクロール制御」節
- 多重インスタンスの資源保護（#177）: `.agent/architecture.md` 該当節
