# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-22）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#872（2 枚目のウィンドウでアプリが静かに終了する）を解消**（PR #895）。
  **Issue の前提は外れていた**: 2 枚目は元から作れていて（`gpui 枚数=2` / PTY / 描画まで実測）、
  死んでいたのは**項目 79（macOS 固有の Dock 復帰）が窓を 0 枚にした瞬間**。真因は GPUI の
  `QuitMode::Default` が非 macOS で「最後の窓が閉じたら終了」で、しかも `ExitProcess(0)` なので
  **診断が 1 行も残らない**。境界 **`platform::window_lifecycle`** に方針を置き、
  `QuitMode::Explicit` + `handle_window_close` の明示 quit に変えた。詳細は plan の「#872 の記録」
- **#766（器の中でシェル統合が届かない）も解消済み**（PR #891）。器の側では直らないので
  抽象境界 `tako_core::osc_sink`（側路）で同じ OSC をファイルで運び `osc_tap` へ通す
- **#884 / #877 / #875 / #873 / #881 / #870 も main へ入っている**
  （argv の引用・agents 走査・実行ペイン・方言判定の一本化・器へ渡す第 1 語・ホーム解決の一本化）

## 診断とテストの前提で 2 つ穴が空いていた（#872 で実測）

- **`TAKO_APP_SELF_TEST_OK` は「合格」を意味していなかった**。`on_app_quit` が無条件に
  印字していたので、**窓 0 枚の自動終了でも OK + 終了コード 0** が出る = 偽の緑。
  ラッチ（`SELF_TEST_AT_FINAL_STEP`）で最終項目到達だけに絞った（番犬つき）
- **`check` は成功時に黙る**。だから「最後に出たログの直後が犯人」は成り立たない。
  `cfg!` のスキップも 1 項目ずつにする（77 / 79 / 80 をまとめたせいで原因の項目を誤認した）
- 副産物: 項目 81 は #381 以降ずっと `setup_ok=false` で**素通り**していた（取り直しを前へ）

## 実機で env / 寿命を測るときの作法（#877 / #870 / #872 で踏んだ）

- **GUI 起動時の env を再現してから測る**。`SHELL`（#877）も `HOME`（#870）も SSH の
  Process スコープにしか無く、GUI 起動の tako には渡らない（`Remove-Item Env:…` を先に打つ）
- **entity の寿命はプラットフォームで違う**。窓 0 枚で最後の強参照（root view）が落ちるので
  **Windows では `TakoApp` ごと解放される**（macOS は残る = #381 の「同一 entity で開き直す」は
  macOS の retain に依存）。0 枚を跨ぐ検証はテスト側で entity を掴んで測る対象を固定する
- **session 0（SSH）から session 1 のウィンドウは列挙できない**。`schtasks /it` で
  プローブを投げる（道具は `C:\Users\shioz\dev\tako-evidence-872\`）
- **`Start-Process` で投げた長い処理は SSH セッションが切れると死ぬ**。
  `Invoke-CimMethod Win32_Process Create` か `schtasks` で投げる

## 次の一手

- **スライス 8（棚卸し）**: #865 の到達範囲表 + #875 / #877 / #884 / #872 の実測で
  `tako_run` / `tako_run_interactive` / `tako_show_command` / `tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` / `tako_window` を `Pending` から倒せる材料が揃った
  （作法 4 に従いマトリクスは #872 では触っていない）
- **同型の一族**（`$SHELL -l -c` の直書き）が残っている: `tako-core/src/lib.rs` /
  `tako-app/src/autorename.rs` / `tako-app/src/preview.rs` /
  `tako-control/src/config_share/env.rs` は **B21（`child_cmd::user_env_cli`）へ寄せられる形**。
  `setup_bootstrap.rs` の `login_shell_sees()` だけは**意図的な unix 専用**（#868 の申し送り）
- 実機セルフテストの停止位置は **項目 93（#694）**。#766 の射程外で **#889** が起票済み
  （`TAKO_ISOLATED=1` は persist OFF = 器なしのペインなので、前提は `$PROFILE` 配置と `cat` 決め打ち）
- **#896**（psmux の copy_mode ホイールテストが負荷で落ちる）/ **#893**（ホーム解決の残り 15 箇所）/
  **#885**（spawn が backend の `wrap_spawn` を通らない）は未着手

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#872 の記録」節に無音終了の機序・
  実機 before/after 表・偽の緑・作法 6 件。「#884 の記録」節に argv の対照実測。
  「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
