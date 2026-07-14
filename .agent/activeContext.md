# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-14・#231 / #234 PDF 品質改善 + PDF・画像ズーム）

**実装・検証・squash merge・Issue 報告まで完了**（PR #240 / merge `22f1d23`）:

- #231: PDF 行間・余白のヒットテストを `None` にし、ドラッグ全文選択を防止。UTF-8 座標テスト追加
- #231: device scale × zoom × 表示幅の `PdfRasterKey`、background 再ラスタライズ、
  path + raster key の `PreviewImageCache` を実装。Retina 2x 全幅で 1224×1584 → 1920×2485 を実測
- #234: PDF・画像を 25〜400% ズーム、2 軸パン、現在ページ維持リセット、倍率 SVG 操作、
  ピンチ / ⌘+/⌘- / ⌘0 / 修飾スクロールへ対応。PDF ページ高さを明示してページ移動を修正
- core `PreviewViewState` → dispatch `PreviewView` → CLI `tako preview` → MCP
  `tako_preview_view`（75 ツール）を 1:1 実装。3 ページ目 + 150% の変換テストあり
- 隔離実機: PDF 100/150%、2 ページ目、パン、リセット、画像 100/200%、行間ドラッグ、
  150% 選択ハイライトを確認。render p50 1ms / p99 2ms / max 4ms
- GPUI `PlatformInput::Pinch` の Started → Moved → Ended を実 scroll bounds へ送り、
  1.500 → 1.650 → 1.485 の増減を隔離 E2E で確認。keyboard modality 直後も
  capture + ペイン bounds 判定で取りこぼさず、他ペインへ誤配信しない
- `origin/main`（#21 / #229、`6af2d47`）を競合なしで rebase。統合後も workspace
  build / fmt --check / clippy -D warnings / test 全緑
  （app 83・CLI 22・control 362・core 249 passed）。隔離セルフテストは
  `TAKO_APP_SELF_TEST_OK`、PDF 150% は raster key 150 + hit `(0, 0)`、pinch 増減を実描画で確認
- PR #240 は Cloudflare Pages 成功後に squash merge、作業ブランチ削除済み。#231 / #234 は closed、
  実測値と目視チェックリストを各 Issue へコメント済み

## 次の一手

- install は依頼どおり master 側で行うため、この worktree では実行しない
- 安全に切り出した before / after PNG は目視確認済みだが、GitHub Web が未ログインかつ
  CLI の Issue API はバイナリ添付非対応のため未添付。検証画像を git へ入れない規約を優先

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md`（FR-3.4 / FR-3.10 / FR-3.14、NFR-8）
- 設計: `.agent/architecture.md`（PDF 選択・ラスタライズ・ズーム、ストール診断）
- 実機確認: `.agent/manual-checks.md`（PDF 選択描画・ズーム）
