# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-15・#233 プレビューライブリロード）

**実装・全検証・最新 main 取り込み・全差分レビューまで完了。PR 作成待ち**:

- `notify` の OS ネイティブイベントで表示中ファイルの親ディレクトリだけを非再帰監視し、
  パス別 300ms デバウンスと世代照合を実装。callback は channel 送信だけで、アイドル時の
  UI ポーリングは追加していない
- コード / Markdown / 画像 / PDF の読み込み、syntect / pulldown-cmark、画像バイト取得、
  PDF ラスタライズを background で完了してから UI を差し替える。#231/#234 の
  `PreviewImageCache` と raster key を維持し、スクロール、mode、zoom / pan を保持する
- 編集モード中の外部変更は `SaveStatus::Conflict` で通知し、編集バッファを上書きしない。
  自保存由来のイベントは完成バイトとの比較で除外する。動画は再生位置保持のため対象外
- core `PreviewReloadState` → dispatch `PreviewReload` → CLI
  `tako preview-reload [on|off]` → MCP `tako_preview_reload`（全 80 ツール）を 1:1 実装。
  設定は既定 ON で `settings.json` に永続化する
- 連続 6 write（40ms 間隔）は 1 回へ集約され、最終 write から 427ms で反映。
  Markdown mode と scroll_y=-48.0 を維持。編集競合は 344ms、バッファ保持を確認
- 削除 / rename / 同一パス復帰、1MB 超の省略表示、PNG 更新と zoom / pan 保持、
  PDF 更新後の background 再ラスタライズを隔離セルフテストで確認
- `preview_watch_sync` のアイドル窓は p50 / p95 / p99 / max すべて 0ms、event / apply は 0 回。
  6 イベント時の `preview_watch_event` と 1 回の `preview_reload_apply` も全 percentile 0ms、
  16ms 以上の UI 専有ログは 0 件
- `origin/main`（#161 / #20、`30a56b5`）へ rebase 済み。workspace build / fmt --check /
  clippy -D warnings / test 全緑、隔離セルフテストは `TAKO_APP_SELF_TEST_OK`。変更 22 ファイルの
  diff を全行レビュー済み

## 次の一手

- ブランチを push し、`Closes #233` と perf_span 実測値を含む PR を作成する
- CI 成功後に squash merge + ブランチ削除し、Issue #233 へ実測証拠付き完了コメントを投稿する
- install は依頼どおり master 側で行うため、この worktree では実行しない

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md`（FR-3.5 / FR-3.15、NFR-8）
- 設計: `.agent/architecture.md`（background プレビュー、ライブリロード経路、ストール診断）
- 実機確認: `.agent/manual-checks.md`（プレビューライブリロード）
