# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-14、Issue #801 描画の残る固定費 = merge 済み・install 済み）

- PR #802 を squash merge（`a899f63`）。CI は macOS / Windows / Pages 全緑。Issue #801 クローズ済み
- #787 完了時点で残っていた**空画面でも毎フレーム掛かる 4.76M instructions** の内訳を
  段階的無効化ゲートで確定し、支配項（セル単位の変換）を削った
- 結果（grid-bench・300 フレーム・同一バイナリ A/B・`TAKO_801_NO_FAST_CELLS=1` が before・
  バナー無し 119x27）: 空画面 3.587M → **2.197M（−39%）** / 実務密度 −10% / 満杯 −2%。
  **目標の 1M 未満は未達**（理由は下記）

## 内訳の実測（空画面 119x21・バナーあり = #787 の 4.76M と同条件）

| 部位 | instr/frame |
|---|---|
| ウェルカムバナー #549（**初回起動時のみ表示**） | 1.17M |
| スナップショット + `plan_row` + グリッド element | 1.76M ← 今回削った |
| ペインヘッダ | 0.81M ← `cached` が入れ子にできず塞がっている |
| クローム 4 枚の cached 再利用 | 0.46M ← gpui 内部（`Scene::replay`）で削る余地なし |
| ルートの箱・ペイン枠・プローブ等 | 0.41M |
| gpui のフレーム下限（ルートが空 div） | 0.16M |

## 入った実装（`TAKO_801_NO_FAST_CELLS=1` で一括 off = 同一バイナリ A/B）

- `tako_core::screen::snapshot_opts`: 素の空白セル（半角スペース + フラグ無し + 既定色）は
  `grid` の初期値と同じ結果になるので解決も書き込みもしない。1 セルも書かれなかった行は
  `compose_line` を 1 本だけ組んで複製する
- `tako_app::terminal_grid::plan_row`: 描くものが無い行は `RowPlan::default()` を即返す。
  残る行も `Rgb -> Hsla` を**ラン単位**へ（旧: セルごと = 空画面でも毎フレーム 2,499 回）
- `render_pane` からタイトルバーを `render_pane_header` へ切り出し（持ち上げの準備。
  今回は**キャッシュしていない**）

## 見つけた制約（後続作業の前提。architecture.md / view_cache.rs に記載済み）

- **`AnyView::cached` は入れ子にできない**: GPUI はキャッシュビューを描き直すあいだ
  `window.refreshing = true` を立て、再利用条件に `!refreshing` が入っている。
  ペインヘッダを `PaneBody` の内側でキャッシュしても効果 0.046M（実装して実測）
- **`cached` は汚れていても得**: 中身が `layout_as_root` の確定サイズ別パスになるので、
  ルートの flexbox が部分木を測り直さない。「どうせ描き直すから素で出す」は **+0.86M** の逆効果
- → ヘッダ 0.81M を取るには**ペイン枠ごとルート側の兄弟へ持ち上げる**必要がある
  （丸め角のクリップを崩さないこと。visual-test の `terminal-grid` が見張る）

## 不変条件

- 空白セルの近道を**属性へ広げない**（下線・反転・DIM・明示背景色・全角スペーサーは
  空白でも見える / 列数がずれる）。判定は「フラグが 1 つも立っていない」が必要条件
- 既定色が OSC 4 / 11 で差し替えられている場合は近道を使わない（判定はループの外で 1 回）
- ⌘ホバーのリンク装飾は空白セルにも掛かるので、リンクのある行は早期打ち切りの対象外

## 次の一手（master 判断）

- 残件の Issue 化: ペインヘッダの持ち上げ（0.81M）/ ウェルカムバナー（1.17M・初回のみ）
- GUI 再起動で `/Applications/tako.app` の新バイナリを反映（install は済み）

## 現フェーズで Read すべき設計書

- 描画: `.agent/architecture.md`「ビュー単位の描画キャッシュ」「端末グリッドの専用 Element」
  「空白セルの近道」
- 実装: `crates/tako-core/src/screen.rs`（`is_plain_blank`）/
  `crates/tako-app/src/terminal_grid.rs`（`row_draws_nothing` / `RunStyle`）
