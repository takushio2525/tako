# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-15・#258 アプリ全体メモリ監査）

**調査フェーズ完了、修正着手前**:

- v0.5.2 / `8d80be3` の隔離自前ビルドで Issue 記載の容疑1〜7を調査
- 6ページ PDF の倍率世代で physical footprint が 0.48GB → 2.63GBへ増加。
  内訳は `MALLOC_LARGE` 1.30GB + `IOAccelerator` 1.08GB、`leaks` は48 bytesだけ
- 主因は tako のペイン別 map ではなく、旧 `gpui::Image` に `remove_asset` を呼ばず、
  GPUI の全体 asset cache / sprite atlas がデコード済み CPU + GPU 画像を保持する経路
- 71ページ・同じ6倍率へ線形換算すると CPU + GPU 約27.35GiB。4096px幅は
  1世代約11.49GiBで、60GB報告を説明可能
- ライブリロード400ms間隔8回で全ページラスタライズが7本並行し、RSS最大808,656KiB、
  CPU最大274.7%。世代不一致結果は最終的に解放されるが、single-flightが必要
- BG退避とcloseで footprint不変。ターミナル10,000行上限、sessions 500件、ログ5/200MB、
  worker events一時生成はGB級原因でないことを確認
- 詳細: `.agent/investigations/issue-258-memory-audit.md`

## 次の一手

- 原因の定量結果を Issue #258へ報告して調査マイルストーンをコミット
- 512MiB既定の設定可能な画像バイト予算 + LRU、可視近傍ページだけのデコード、
  GPUI asset / atlasの明示eviction、ライブリロードsingle-flightを実装
- 長時間相当RSS系列、perf_span、全品質ゲート、隔離セルフテストを通す

## 現フェーズで Read すべき設計書

- Issue: #258（スコープ・容疑1〜7・受け入れ条件）
- 調査: `.agent/investigations/issue-258-memory-audit.md`
- 要件: `.agent/requirements.md`（FR-3.4 / FR-3.14 / FR-3.15、NFR-3 / NFR-8）
- 設計: `.agent/architecture.md`（プレビュー描画キャッシュ、backgroundロード、ストール診断）
