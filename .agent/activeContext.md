# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-15・#258 アプリ全体メモリ監査）

**調査・修正フェーズ完了、長時間検証前**:

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
- `tako-core::ByteLru` で既定512MiB（設定256〜8192MiB）のデコード済み画像予算を実装。
  PDF は表示ページ前後だけを遅延 `gpui::Image` 化する
- LRU / 世代変更 / close 時は `Image::remove_asset` + `App::drop_image` を次render冒頭で実行し、
  CPU assetとGPU atlasを同時解放。動画の置換済みフレームもatlasから解放する
- ライブリロードは `(pane, path)` single-flight + 最新1件再実行。未回収run完了履歴は
  実行中を除外して最大256件、close時のpane link補助キャッシュも除去
- dispatch `PreviewCache` / CLI `tako preview-cache` / MCP `tako_preview_cache` を1:1追加。
  上限・使用bytes・entry数を返し、settings.jsonへ永続化する
- app 91件、CLI 25件、control 425件、core 276件の対象4クレートテストは全緑

## 次の一手

- 修正マイルストーンをコミット・push
- 隔離環境で PDFズーム + ページ移動 + ライブリロードの30分相当を実行し、
  RSS / physical footprint / cache stats / single-flight本数 / perf_spanを系列採取
- origin/main（#257）を取り込み、全品質ゲートと隔離セルフテスト後にPR・mergeする

## 現フェーズで Read すべき設計書

- Issue: #258（スコープ・容疑1〜7・受け入れ条件）
- 調査: `.agent/investigations/issue-258-memory-audit.md`
- 要件: `.agent/requirements.md`（FR-3.4 / FR-3.14 / FR-3.15、NFR-3 / NFR-8）
- 設計: `.agent/architecture.md`（プレビュー描画キャッシュ、backgroundロード、ストール診断）
