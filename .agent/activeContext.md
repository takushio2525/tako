# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-11、Issue #787 の前提整備 = 実装完了・PR 待ち）

- worktree `~/dev/tako-wt-787pre` / ブランチ `test/787-visual-net`
- #787（端末グリッドを div スタックから専用 Element へ置き換え）は #64 / #159 /
  #497・#781 / #725・#145 / #153 の実装に触るので、**置き換える前に今の見た目を
  ピクセルで固定する**のが本タスク。Element 化本体は後続 worker が担当（ここではやらない）
- 追加したのは visual-test の `terminal-grid` 節 1 本（6 検査）+ 共通部品 6 個。
  `TAKO_VISUAL_ONLY=terminal-grid` で単独実行でき、全節通しの先頭でも走る
  （ターミナルが素の状態 = 1 ペイン・テーマ既定のうちに撮るため）

## 6 検査の中身（すべて実ピクセル or 実レイアウト矩形）

1. **日本語混在行**（#64）: fixture 12 行を PTY へ流し、**非空セルが全部塗られているか**を
   セル単位で見る。`⏺ Fable 5 + max` / `ターミナルUI` / 絵文字混在 / 行末まで届く半角行
2. **ピクセルスクロール**（#159）: インク縦プロファイルの**位相**が半セル
   （17 device px）ちょうどずれる + 上端が繰り上がる + 下端の extra_bottom が隙間を埋める
3. **選択ハイライト**（#725/#145）: 合成 `PlatformInput` のドラッグで行をまたいで選択 →
   選択色の塗り + `pbpaste` 一致（copy-on-select）
4. **色とスタイル**: `ScreenLine::runs` が解決した色をそのままピクセルと突き合わせる
   （truecolor fg / truecolor bg / 256 色 / bold / dim / 反転）。期待色をテストへ焼かない
5. **IME アンカー**（#781/#497）: `pane_text_area_drift` = 0 + カーソルブロックの実塗り位置と
   算術の一致（0.07px）+ `ime_overlay_anchor` がその位置を指す
6. **カーソル描画**: ブロック / バー × フォーカス有無の 4 通りでセルが塗られ、
   DECTCEM で隠すと 0（対照）

## この整備で見つけた既知の癖（Issue 化済み。Element 化で直る見込み）

- **#797**: SGR 4 の下線が**1 px も描かれない**。GPUI は下線を行ボックス下端
  （ベースライン + descent×0.618）へ置くので、チャンク div の `overflow_hidden`
  （#64 対策で外せない）が丸ごと切る。節では「モデルは underline と解決する」
  「次の行へはみ出さない」だけを主張し、ピクセルの主張は #797 を直す側で入れる
- **#798**: 全角が長く連なる行で描画位置がグリッドより最大 1 セル左へ詰まる
  （div 幅のデバイスピクセル丸めが 55 個ぶん累積）。**半角行は drift 0**。
  節では VC 行で「1 セル以内」と塗られたセル数で固定してある

## 不変条件

- **描画本体は 1 行も変えない**。追加は全部 `#[cfg(feature = "visual-test")]`。
  feature 無しビルドの**グローバルシンボル 135,988 件と `__text` 49,141,068 バイトが
  main と完全一致**することで担保（SHA は行番号 DWARF が動くぶんだけ違う）
- 検証は `TAKO_ISOLATED=1` + 注入済み `TAKO_*` を `env -u` で外す
  （ランナーは scratchpad の `787/run-visual.sh`）。本番 GUI の pid は毎回不変を確認
- ダンプ（`TAKO_VISUAL_DUMP_DIR`）はホスト名・ユーザー名が写るので**リポジトリへ入れない**

## 次の手順

1. PR（`Refs #787`）→ macOS CI 全ジョブ緑 → squash merge → worktree 片付け（install 不要）
2. #787 本体（Element 化）の worker は、着手前に `TAKO_VISUAL_ONLY=terminal-grid` で
   before を採り、置き換え後に同じ数値と突き合わせる。#797 / #798 が直ったら
   その節の主張を「ピクセルが出る」「drift 0」へ**意図的に**上げる

## 現フェーズで Read すべき設計書

- 端末グリッドの描画: `crates/tako-app/src/main.rs` の `terminal_screen_lines` /
  `chunk_line_chars` / `pane_text_area_rect`、`.agent/architecture.md`
- visual-test の作法: `main.rs` の `self_test::run_visual` と `capture_frame` 周辺の共通部品
