# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-14、Issue #796 隔離セルフテストの main 由来フレークの根治）

- ブランチ `fix/796-selftest-stability`（worktree `~/dev/tako-wt-796`）
- 目的: worker の Definition of Done「隔離セルフテスト完走」を信頼できる状態へ戻す
  （同じソースなら同じ結果になる = main 由来の失敗と PR 由来の失敗を切り分けなくて済む）

## 確定した根因 3 つ（すべて実測で切り分け済み）

1. **`AnyView::cached`（#786）と「汚さずに draw」の組み合わせ** ← PDF アウトライン（#232）の正体。
   製品経路（IPC / MCP の dispatch ループ）は dispatch 直後に `cx.notify()` してから次フレームを
   描くが、セルフテストは直接 `dispatch` して `draw()` するだけだったので子ビューが描き直されず、
   スクロールの幾何がキャッシュのまま = ジャンプが効いていないように見えていた。
   2 秒ポーリングの notify がたまたま挟まった回だけ通っていた。
   実測: `children=2 max_offset_y=199` なのに `offset_y` が 4 秒 80 フレームで 0 のまま →
   `notify → draw` の順に直すと同一ビルドで `pdf_jump_ok=true delta=91.0`
2. **偽の待ち条件**（#601）: A / B 両フェーズのプロンプトが同じ `ST601>` で、B の起動待ちが
   A の残り表示へ即マッチ → `clear` と `tako` が起動前の外側シェルへ流れていた。
   リトライでは直らない型（`ST601A>` / `ST601B>` に分離）
3. **「出るもの」を固定時間で待っていた**（26 組）: `--features visual-test` は
   `gpui_platform/test-support` → **`gpui/leak-detection`** を有効にし、entity ハンドルの
   生成・複製・破棄を毎回ロック + HashMap で記録するので数割遅い = feature の有無だけで落ちていた

## 入った実装

- `wait_for_focused_text` / `wait_for_focused_text_timed`（状態到達まで待ち、上限で偽 +
  `TAKO_SELF_TEST_WAIT_TIMEOUT` に待った時間・画面末尾・実行環境）へ 26 組を移行
- `absent_after_anchor`（否定検査は「先に必ず出るもの」を待ってから見る = 偽 PASS を防ぐ）
- `notify_and_draw`（幾何を読む前は必ず汚してから 1 フレーム = 製品経路と同じ順序）
- `PdfScrollProbe`（children / max_offset まで出す）+ 落ちた条件の内訳
- `TAKO_APP_SELF_TEST_ENV`（profile / feature / leak-detection / load / 経過）を開始時と失敗時に出力。
  load は `tako_control::diag::load_average`（新設。書式は純粋関数で単体テスト）
- #732 は「分割直後のペインが素のアイドルになる」前提の成立を待ってから検査
- 番犬テスト `selftest_wait_watchdog`（固定待ち + 肯定 `focused_contains` をソース検査で禁止）
- 規約: `.agent/conventions.md`「セルフテストの待ち条件の書き方」

## 開発環境の注意（このマシン固有・報告済み）

- **Metal Toolchain が入っていない**（macOS 26.4 / Xcode 26.2 で別ダウンロード扱い）。
  `xcrun -sdk macosx metal` が失敗するので **gpui_macos の build script が走る構成では
  ビルドできない**（debug 全般 / feature を変えた release）。
  復旧は `xcodebuild -downloadComponent MetalToolchain`。
  今回は「シェーダは gpui の rev 固定で不変」なので、既存 `shaders.metallib` を再利用する
  ローカル shim（リポジトリ外）で回避して検証した

## 追加で判明した環境要因（重要）

- **ウィンドウが他アプリに完全に隠れると GPUI が描画を止め、新規ペインのシェルが
  1 行も出力しない**（項目 76d / 104 が既にスキップしている条件）。これが #666 と
  項目 63 の失敗の正体だった。判定 signal は `pane_text_areas`（1 度でも描画された
  ペインだけが載る）で、未描画なら**明示スキップ**（落とさない・黙って通さない）

## 次の一手（master 判断）

- #732 はクローズ提案のコメント済み。#771 は 101c 本体だけ残す方針をコメント済み
- セルフテストは feature なしのビルドで回す運用が正（visual-test は
  `TAKO_VISUAL_TEST=1 TAKO_VISUAL_ONLY=<節>` 側）。ただし本 PR 以降は**両構成で完走する**

## 現フェーズで Read すべき設計書

- セルフテストの待ち条件: `.agent/conventions.md`「セルフテストの待ち条件の書き方」
- 実装: `crates/tako-app/src/main.rs`（`mod self_test` の `wait_for_focused_text` /
  `notify_and_draw` / `selftest_wait_watchdog`）
