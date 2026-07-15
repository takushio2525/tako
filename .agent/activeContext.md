# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-15・#258 アプリ全体メモリ監査）

**調査・修正・検証完了、PR前**:

- v0.5.2 / `8d80be3` の隔離実測で、GPUI asset cache / sprite atlas に残るPDFの
  デコード済みCPU + GPU画像を主因と定量特定。71ページ・同6倍率は約27.35GiB相当
- 既定512MiB（設定256〜8192MiB）のバイト予算付きLRU、表示近傍3ページの遅延デコード、
  `Image::remove_asset` + `App::drop_image`、動画旧frame解放を実装
- ライブリロードを `(pane, path)` single-flight + 最新1件へ直列化。未回収run完了履歴は
  最大256件、close時にペイン補助キャッシュを除去
- dispatch `PreviewCache` / CLI `tako preview-cache` / MCP `tako_preview_cache` を1:1追加し、
  上限・使用bytes・entry数を返してsettings.jsonへ永続化
- #257 のファイルスタンプ比較・ダブルバッファ化をorigin/mainから取り込み済み
- 30分相当のPDFズーム・移動・120回変更でfootprint peakはcycle 10〜30の795MBで不変。
  終了時RSS 84,816KiB、close後68,672KiB / LRU 0 bytesへ回収
- #257統合後も追加21サイクルでfootprint peak 812MB横ばい、RSS傾き
  -4,266.1KiB / cycle。`render` p95 / p99最大6ms、最大15ms
- build / fmt / clippy / workspace test全緑。隔離セルフテストは終了コード0、
  `TAKO_APP_SELF_TEST_OK`
- 詳細: `.agent/investigations/issue-258-memory-audit.md`、
  `.agent/investigations/issue-258-memory-validation.md`

## 次の一手

- 検証マイルストーンをコミット・push
- PR（Closes #258）を作成し、CI緑後にsquash merge・ブランチ削除
- Issue #258へ実測証拠付き完了コメントを投稿

## 現フェーズで Read すべき設計書

- Issue: #258（スコープ・容疑1〜7・受け入れ条件）
- 調査: `.agent/investigations/issue-258-memory-audit.md`
- 要件: `.agent/requirements.md`（FR-3.4 / FR-3.14 / FR-3.15 / FR-3.17、NFR-3 / NFR-8）
- 設計: `.agent/architecture.md`（プレビュー描画キャッシュ、backgroundロード、ストール診断）
