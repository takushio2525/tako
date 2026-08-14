# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、Issue #803 ペインヘッダをルート側へ持ち上げる = PR 提出）

- ブランチ `improve/803-header-lift`（worktree `~/dev/tako-wt-803`）。#796 / #658 merge 後の
  origin/main へ rebase 済み
- 目的: ペインのタイトルバーは **PTY の出力では変わらない**のに毎フレーム作り直していた。
  `AnyView::cached` は入れ子にできない（#801 の実測）ので、ペイン本体の内側に置いたままでは
  キャッシュが一度も当たらなかった → **本体の兄弟（どちらもルートの子）へ出した**

## 入った実装

- `view_cache::PaneHeader` 新設（本体 `PaneBody` と同列のキャッシュ単位）。ルートは
  ペインごとに「本体（矩形いっぱい）」+「ヘッダ（矩形の上端）」を並べる
- ヘッダの外側の div は**ペイン枠と同じ箱**（同じ矩形・同じ枠幅・同じ角丸・
  `overflow_hidden`）を taffy に組ませるためのもの = 位置を px で手計算しない。
  背景・影は持たないが、**枠線だけは同じ色で描かせる**（下記の罠）
- 本体は同じ高さ（`PANE_TITLE_BAR`）のスペーサーでヘッダの場所を空ける
  （`pane_text_areas` の会計 `stacked_top` は持ち上げの前後で不変）
- ヘッダを出すかは `lifted_header_panes`（= **本体が実際に場所を空けたか**）で決める
- `tick_pane_header_clocks`: `running · 4m12s` だけは時間で変わるので 1 秒に 1 回だけ
  Running のペインのヘッダを汚す
- grid-bench の DONE 行に `renders=(body +N header +N chrome +N)` を追加

## 実測（隔離 grid-bench・300 フレーム・main と改修版の交互 3 反復の中央値）

| 密度（119x27・バナー無し） | main | #803 | 差 |
|---|---|---|---|
| 空画面 | 2.192M instr/frame | **1.737M** | −0.455M（−21%） |
| 実務密度（915 セル） | 5.158M | **4.698M** | −0.460M（−9%） |
| 満杯（2,943 セル） | 8.435M | **7.977M** | −0.458M（−5%） |

ヘッダを丸ごと描かないゲートを当てた main が 1.517M = **ヘッダの総コスト 0.678M**、
うち **0.455M（67%）を回収**。残り 0.22M は `cached` の再利用そのもの（`reuse_prepaint` /
`reuse_paint` / `Scene::replay`）と外側 div で、tako 側から削る余地は薄い。
**Issue 本文の 0.81M は #801 の 119x21 + バナーありの構成の値**（本件の構成では 0.678M）。

## 踏み抜いた罠（同じ構造を触る人へ）

- GPUI の `Style::paint` は「影 → 背景 → 子 → **枠線**」の順。持ち上げる前は
  ペイン枠の丸め角がヘッダの上に来ていたので、兄弟にしただけだと上 2 つの角が
  ヘッダの四角い背景で潰れる（実測 104 画素の accent が消えた）。外側 div にも
  同じ矩形・同じ色の枠線を描かせて重なり順を戻した
- **`visual-test` のピクセル計測値だけでは足りない**（角の欠けは計測点の外だった）。
  `TAKO_VISUAL_DUMP_DIR` の実フレーム同士を全画素比較して初めて分かる

## 不変条件

- ヘッダの実体は `view_cache::PaneHeader` からしか呼ばない（`self.render_pane_header(` を
  main.rs へ書くと入れ子キャッシュに戻る = 番犬テストが落ちる）
- ヘッダの位置は taffy に解かせる（px の手計算を足さない）
- 表示種別ごとのヘッダ（Web ビュー / プレビュー / スターター / チャット / 準備中）は
  従来どおり各自の本体の中。持ち上げ対象はターミナル表示だけ

## 検証の環境メモ（次に同じ検証をする人向け）

- **visual-test の窓高で結果が変わる検査がある**: `terminal-grid` の
  「下端の部分行（extra_bottom）が隙間を埋める」は `テキスト領域の高さ mod セル高`
  依存で、既定 960x600 では **main でも落ちる**。`layout.json` に
  `window.height = 708` を書いた隔離起動で main / 改修版とも緑になる
- セルフテストの data dir を長いパス（scratchpad 直下）にすると IPC ソケットが
  `SUN_LEN` を超えて**起動直後に固まる**。`TAKO_ISOLATED=1` の既定（`$TMPDIR`）に任せる
- PDF / カーソルブロックの節は負荷で落ちる（main も同率）。3 連続緑は取れる

## 次の一手（master 判断）

- PR（`Closes #803`）→ CI 緑 → squash merge → install（他 worker と重ねない）
- 残り 0.22M を追うなら「枠線をルート側の 1 枚のオーバーレイに集約して本体からは外す」案が
  ある（角の AA の二重合成 32 画素も消える）が、枠線の責務が分かれるので別 Issue 推奨

## 現フェーズで Read すべき設計書

- キャッシュ構造: `.agent/architecture.md`「ビュー単位の描画キャッシュ」+
  「ペインヘッダの持ち上げ」/ `crates/tako-app/src/view_cache.rs`
- 会計: `.agent/architecture.md`「端末グリッドの専用 Element」/ `pane_text_area_rect`
