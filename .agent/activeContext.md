# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-11、Issue #787 端末グリッドの専用 Element 化 = 実装完了・PR 待ち）

- worktree `~/dev/tako-wt-787` / ブランチ `improve/787-grid-element`
- ペイン本体の端末グリッドを「行 div + チャンク div のスタック」から
  **1 個の `Element`（`crates/tako-app/src/terminal_grid.rs`）**へ置き換えた。
  セルの原点を `col * cell_width` で直接決め、背景は `paint_quad`、
  グリフは `shape_line(force_width) + ShapedLine::paint`、下線・取り消し線は自前で置く
- 同時に #797（SGR 4 と ⌘ホバーの下線が 1 px も出ない）と #798（全角の長い連なりで
  最大 1 セル左へ詰まる）が構造的に解消。visual-test の主張を「直った側」へ更新した
- 行 div の `terminal_screen_lines` は**残してある**（チャット入力欄のミラー #719 /
  たまり場サムネイル / タブツリーのホバープレビュー = 行を他要素へ埋め込む用途）

## 設計判断（force_width と全角スペーサー）

- `shape_line` の `force_width = cell_width` はグリフ位置をセル境界へスナップするので、
  advance がセル幅と合わないグリフ（`⏺`・絵文字）の**後続が自動でグリッドへ戻る**。
  #64 対策の「個別 div へ隔離して overflow_hidden で切る」は不要になった
- ただし `force_width` は「グリフ 1 個 = 1 セル」を仮定するため、**全角の 2 セル目に
  スペースを 1 個差し込む**（`shape_segments`）。これでグリフ数と列数が 1:1 に戻る
- **行高はセル高を渡す**。旧実装は `StyledText` 経由で環境既定行高（13×1.618≒21px）
  基準にベースラインを置いていたため字が 2px 下へずれ、ディセンダが切れていた。
  **この 2px はユーザーに見える変化**（PR に明記済み）

## 検証状況（すべて隔離・本番 pid 1099 は全計測の前後で生存）

- 品質ゲート: fmt / clippy(-D warnings、feature なし + visual-test 付き) / test 1935 件 全緑 /
  Windows クロスチェック エラー 0・警告 16 = main と同数
- visual-test `terminal-grid` 節 3 連続 OK（検査 22 行）。全節は 6 回中 4 回完走、
  落ちた 2 回は **PDF 文字矩形の paint**（`wait_for_preview_maps` の実時間待ち）。
  **素の main（a071852）でも 3 回中 2 回同じ項目で落ちる**ことを実測 = main 由来（#796）
- 隔離セルフテスト **完走**（`TAKO_APP_SELF_TEST_OK` / exit 0）。SKIPPED は 76d / 104 の
  2 件のみ = #786 と同じ既知の環境要因（ウィンドウ非前面）
- 実 claude（`TAKO_VISUAL_CLAUDE=1`）: 13 行 523 セルで missing 0 / drift 左右とも 0
- 性能（同一バイナリ A/B・`TAKO_787_NO_GRID_ELEMENT=1` が before）:
  満杯 15.59M → 8.68M / 実務密度 15.68M → 6.42M instr/frame。**ライブ画面の CPU% は
  ディスプレイが消えていて測れず**、`Window::draw` を自前で回す `grid-bench` で代替

## 不変条件

- `pane_text_areas` の算術が正（PTY 行数・マウス座標・IME の共通の正）。element の
  矩形はその原点と一致させる（visual-test の `drift_gap <= 0.5` が見張り）
- サブラインスクロールは「行スタック全体を fract 行ぶん上へ + extra_bottom で下端を埋める」
- 空白セルはグリフを描かないが、**背景・下線・カーソルは別の層が描く**。
  「空白だから何もしない」をグリフ以外へ広げない

## 次の手順

1. PR（`Closes #787` / `Closes #797` / `Closes #798`）→ macOS CI 全ジョブ緑 → squash merge
2. `scripts/build-app.sh --install`（GUI 再起動は master 側）
3. 残る 1 フレーム 4.76M の固定費は**グリッドではない**（空画面でも掛かる）= #786 の残り。
   別 Issue にするかは master 判断

## 現フェーズで Read すべき設計書

- 描画: `.agent/architecture.md`「端末グリッドの専用 Element」「ビュー単位の描画キャッシュ」
- 実装: `crates/tako-app/src/terminal_grid.rs`（モジュール冒頭に方式と踏み抜きどころ）
