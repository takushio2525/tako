# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、#821 = コードプレビューの行数比例リーク）

- ブランチ `fix/821-preview-leak`（worktree `~/dev/tako-wt-821`、base = `5e48470`）
- #814（Phase 1 実測監査）の子。#815 の A/B 中に見つかった「閉じても戻らない
  約 41 KB/行」を、**コード本文の仮想化**（`gpui::list` で可視行だけ描く）で根治した

## 根因（推測ではなく allocation プロファイルで確定）

- コードプレビューは**ファイル全行ぶんの element を毎フレーム**作っていた
  （3,884 行 = 1 フレーム約 2 万個）
- `heap`（MallocStackLogging + シンボル付き release）の live 先頭が
  `TaffyLayoutEngine::request_measured_layout` の **7,883 ブロック ≒ 2 × 行数**
- gpui は測定クロージャへ `TextLayout`（整形済み）をキャプチャさせ、それが taffy の
  `node_context_data` に入る。**taffy 0.10.1 の `TaffyTree::clear()` はこれを消さない**
  ため、「今までで一番大きかったフレーム」ぶんが永久に残る
- 残り 65 MB 級は element アリーナとフレーム用 Vec の**高水位**。どちらも
  close 時の解放では戻らない（閉じたあと 300 フレーム描いても 1 バイトも減らない）

## 効果（隔離・同一バイナリ A/B。`TAKO_821_NO_VIRTUAL_LIST=1` が旧挙動）

| 3,884 行 .rs | before | after |
|---|---|---|
| 開いた | 124.03 MB | **14.85 MB** |
| **閉じた（1 往復）** | 121.71（残留 110.1） | **13.80（残留 2.2）** |
| 閉じた（3 往復） | 158.89（残留 147.3） | 14.18（残留 2.6） |
| 定常フレーム | 0.94〜1.00 ms | **0.12〜0.13 ms** |

1 万行では footprint **210 MB → 46 MB**。

## 同梱で直した別バグ

CLI / MCP の close（`detach_session`）が GUI の close と別々にフィールドを列挙していて、
プレビューの行テキスト・行レイアウトを落としていなかった（1 開閉あたり約 0.8 MB 残留）。
`drop_preview_pane_state` へ集約 + 番犬テスト `preview_cleanup_watchdog` で拘束。

## 踏み抜いた罠（次に触る人へ）

- **`pkill -x tako-app` は本番 GUI にも当たる**。実際に落として復旧した（layout.json と
  tmux は無事で 9 タブ 21 ペイン復元）。隔離インスタンスは**明示 pid でのみ**落とす
- GPUI は macOS で**遮蔽されると display link を止める**ので、裏で起動した隔離
  インスタンスは 1 フレームも描かない。#821 は描画でしか再現しないため、
  計測は `TAKO_VISUAL_ONLY=preview-leak`（`Window::draw` を自分で回す）で行う
- list の item は `w_full` が要る / 未 prepaint の `TextLayout` を触ると panic /
  余白はリスト側に置く（詳細は architecture.md）

## 次の一手

- PR（`Closes #821` / `Refs #814`）→ macOS CI 緑 → squash merge → install
  （`build-app.sh` は #817 worker と重ねない）
- 上流へ報告する価値がある: taffy `TaffyTree::clear()` が `node_context_data` を
  消さない（1 行の修正。tako は gpui を rev 固定で参照しているだけなので自前では直せない）

## 現フェーズで Read すべき設計書

- `.agent/architecture.md`「コードプレビューの仮想化（#821）」= 機序・実測・踏み抜きどころ
- `crates/tako-app/src/preview_render.rs` の `render_preview_code_line` /
  `preview_code_list_state` / `drop_preview_pane_state` / `preview_viewport_bounds`
