# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-14・#231 / #234 PDF 品質改善 + PDF・画像ズーム）

**3 マイルストーン実装済み、全体検証・PR 前**（worktree `tako-wt-231` / `fix/231-234-pdf-quality-zoom`）:

- #231: PDF 行間・余白のヒットテストを `None` にし、ドラッグ全文選択を防止。UTF-8 座標テスト追加
- #231: device scale × zoom × 表示幅の `PdfRasterKey`、background 再ラスタライズ、
  path + raster key の `PreviewImageCache` を実装。Retina 2x 全幅で 1224×1584 → 1920×2485 を実測
- #234: PDF・画像を 25〜400% ズーム、2 軸パン、現在ページ維持リセット、倍率 SVG 操作、
  ピンチ / ⌘+/⌘- / ⌘0 / 修飾スクロールへ対応。PDF ページ高さを明示してページ移動を修正
- core `PreviewViewState` → dispatch `PreviewView` → CLI `tako preview` → MCP
  `tako_preview_view`（75 ツール）を 1:1 実装。3 ページ目 + 150% の変換テストあり
- 隔離実機: PDF 100/150%、2 ページ目、パン、リセット、画像 100/200%、行間ドラッグ、
  150% 選択ハイライトを確認。render p50 1ms / p99 2ms / max 4ms

## 次の一手

- #234 マイルストーンコミット
- workspace build / fmt / clippy / test + `TAKO_ISOLATED=1` セルフテスト
- origin/main 取り込み、push、PR（Closes #231 / #234）、CI 後 squash merge
- 両 Issue に実測値・安全に切り出したスクショ・目視チェックリストをコメント

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md`（FR-3.4 / FR-3.10 / FR-3.14、NFR-8）
- 設計: `.agent/architecture.md`（PDF 選択・ラスタライズ・ズーム、ストール診断）
- 実機確認: `.agent/manual-checks.md`（PDF 選択描画・ズーム）
