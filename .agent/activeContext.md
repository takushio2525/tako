# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-06、Issue #781 IME 位置・選択座標のズレ再発）

- worktree `../tako-wt-781` / ブランチ `fix/781-ime-selection-offset`
- **疑われていた #725 / #737（チャット表示）の回帰ではなかった**。稼働は `ui_mode: terminal` で、
  ターミナル表示の座標系の問題。#737 マージ以降の diff に IME / 選択の座標変更は無い
- 根因: **stale claude バナー（#498）の 28px がテキスト領域の会計から漏れていた**。
  バナーはペインヘッダとターミナル領域のあいだに積まれる「流れの中」の要素なのに、
  `pane_text_areas`（= PTY 行数 / マウス座標変換 / IME アンカーの共通の正）を作る算術が
  差し引いていなかった。#684 が正にしたのは「ペインを並べるコンテナ」で、ペイン内部は対象外だった
- タイミングの実測: `~/.local/bin/claude` の symlink が 13:40 に 2.1.220 → 2.1.223 へ更新
  → ユーザー報告 13:43:42（3 分後）。claude が自己更新すると全 master / worker ペインで
  一斉にバナーが出るので、「また発生してる」の周期性は claude の更新周期と一致する

## 実測・検証

- セルフテスト項目 106 を新設（実描画のテキスト領域を prepaint で採取して算術と突き合わせる）
  - 修正前: `top 77.0 -> 105.0 gap 0.0 -> 28.0` → FAILED（exit 1）
  - 修正後: `top 77.0 -> 105.0 gap 0.0 -> 0.0` → `TAKO_APP_SELF_TEST_OK`（exit 0）
  - バナーの押し下げ量が両方で +28px = 検査は空振りしていない
- 単体 7 本（`pane_text_area_tests`）。会計を外すと番犬が FAILED になることを実測
- 番犬 `ターミナルペインの直接の子は想定どおり`（直接の子の数を固定 = 流れの中に足したら落ちる）
- 実行時の自己申告: `render()` 冒頭で算術と実描画を突き合わせ、1px 以上なら perf.log へ 1 回だけ
- `cargo test --workspace` 1897 件緑 / fmt --check 緑
- 76d / 104 のマーカー検査はウィンドウ非前面のため既知の SKIPPED（素の main でも同じ）

## 修正の要点

1. `STALE_BANNER_HEIGHT` を描画側の `.h()` と会計側で共有
2. 矩形は `pane_text_area_rect(content, unit_rect, stacked_top, band, scale_factor)` の 1 か所で作る
3. `PaneTextAreaProbe`（ペイン単位）で実矩形を採取。**正としては使わない**
   （PTY resize と結ぶと 1 フレーム遅れが行数の振動を生む）= 観測と自己申告のみ

## 不変条件

- 本番 GUI・本番 tmux socket `tako`・本番 data dir に触れない（検証は TAKO_ISOLATED=1 のみ）
- System Events のキーストローク送出は禁止。本番 pid は検証の前後で不変（53327）
- 採取プローブは描画中に entity を触らない（`Cell` への書き込みだけ。#684 と同じ理由）

## 次

- コミット → push → PR（Closes #781）→ macOS CI 緑 → squash merge
- Issue へ実測証拠をコメント。**実 IME の見た目は未検証**（この機に日本語入力ソースが無い）
  なのでクローズはユーザー実機確認後
- マージ後 `scripts/build-app.sh --install`（他 worker のビルドと同時実行しない）

## 持ち越し

- #782（UI ストールそのもの）はこの Issue の対象外。主因でないことは隔離実測で確認済み
