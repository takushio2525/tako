# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-22）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#889（セルフテスト項目 93 が Windows で必ず止まる）を解消**（PR #900）。原因はどちらも
  **テスト側**: ①`cat` の argv 直書き（Windows の `cat` は `Get-Content` のエイリアスで実体が無く、
  Windows は argv を包まず `CreateProcess` するのでペインが即死）②素のシェルペインが実機の
  `$PROFILE` 配置に依存（`TAKO_ISOLATED` の data_dir 隔離で `status()` の見る script と
  `$PROFILE` の指す本番パスが別物になる）。境界は `ShellDialect::echo_stdin_command()` と
  `integration_shell_command()` の 2 本。**到達範囲は項目 0〜93**（次の壁は 94 = #897）
- **#872（2 枚目のウィンドウでアプリが静かに終了する）も解消済み**（PR #895）。真因は GPUI の
  `QuitMode::Default` が非 macOS で「最後の窓が閉じたら終了」+ `ExitProcess(0)` で診断が残らない
  こと。境界 **`platform::window_lifecycle`** に寿命の方針を置いた
- **#766 / #870 / #884 / #881 / #877 / #875 / #873 も main へ入っている**（器の中のシェル統合の
  側路・ホーム解決の一本化・空白入り cwd・器へ渡す第 1 語・agents 走査・実行ペイン・方言判定）

## 診断とテストの前提で空いていた穴（#872 / #889 で実測）

- **`TAKO_APP_SELF_TEST_OK` は「合格」を意味していなかった**（#872 で是正）。`on_app_quit` が
  無条件に印字していたので窓 0 枚の自動終了でも OK + 終了コード 0 が出ていた
- **`check` は成功時に黙る**ので「最後に出たログの直後が犯人」は成り立たない。
  `cfg!` のスキップも 1 項目ずつにする（77 / 79 / 80 をまとめて原因の項目を誤認した）
- **消えたペインでも判定が通ることがある**（#889）。`pane_display_for` は不明を Terminal へ
  倒すので、`cat` が起動できずペインが即死しても「実行中は据え置き」は緑になっていた。
  **ペインを作る検証は「作れたか」も見る**
- **テストが製品の組み立てを決め打ちしていると、製品を直した瞬間に壊れる**（#889）。項目 93 (d) の
  期待値は `welcome::launch_command_line` から作る形にした（macOS は従来と同一文字列）

## 実機の作法（繰り返し踏んでいるもの）

- **GUI 起動時の env を再現してから測る**。`SHELL`（#877）も `HOME`（#870）も SSH セッションの
  Process スコープにしか無い（`Remove-Item Env:…` を先に打つ）
- **`Start-Process` で投げた長い処理は SSH が切れると死ぬ**。`schtasks`（GUI は `/it` で session 1）
  か `Invoke-CimMethod Win32_Process Create` で投げ、**完了はログの `EXITCODE=` 行で待つ**
  （プロセス消失で待つと `cargo run` の起動待ちを完了と誤判定する）。ログは UTF-8 で読む
- **fresh worktree は `web/tako-remote/dist/` を持たない**（.gitignore 済み）。`rust_embed` が
  埋め込むので tako-control のコンパイルが即失敗する。実機の `npm ci` は lock 不整合で落ちるので
  既存 worktree からコピーしてから cargo を回す
- **`$PROFILE` 配置済みの実機ではシェル統合系の前提崩れが隠れる**（#889）。本番スクリプト
  （`%APPDATA%\tako\shell-integration\tako.ps1`）をリネームすると「未配置の実機」を再現できる
- **`git checkout -- <path>` はそのファイルの未コミット変更を全部捨てる**。実験の巻き戻しに
  使うと自分の作業ごと消える（#889 で 1 回踏んだ）
- **PowerShell ペインの Enter は CR**。素の LF は PSReadLine が継続行（`>>`）にするので
  コマンドが確定しない（#889 で実測 → #897）

## 次の一手

- **#897（Enter が LF）**: セルフテスト項目 94 と `psmux_backend.rs:930` の両方がこれで落ちる。
  直せば 94 以降（チャット / 設定画面 / limit-resume）が開き、実機テストの失敗も 23 → 22 件へ戻る
  （#896 の「負荷で落ちる」も同じテストで、原因は LF = #897 のコメント参照）
- **スライス 8（棚卸し）**: #865 の到達範囲表 + #872 / #875 / #877 / #884 / #889 の実測で
  `tako_run` / `tako_run_interactive` / `tako_show_command` / `tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` / `tako_window` / `tako_ui_mode` を `Pending` から倒せる材料が
  揃った（作法 4 に従いマトリクスは #889 でも触っていない）
- **製品側の起票（#889 の検証中に実測）**: **#898**（`dispatch::which` が POSIX 専用 =
  stale claude 検知が常に無効。境界 B16 `platform::exe::find` へ寄せる）/ **#899**（スターター・
  welcome のコマンド投入が LF + POSIX クォート = GUI モードのカードが機能しない）
- **同型の一族**（`$SHELL -l -c` の直書き）: `tako-core/src/lib.rs` / `tako-app/src/autorename.rs` /
  `tako-app/src/preview.rs` / `tako-control/src/config_share/env.rs` は
  **B21（`child_cmd::user_env_cli`）へ寄せられる形**。`setup_bootstrap.rs` の
  `login_shell_sees()` だけは**意図的な unix 専用**（#868 の申し送り）
- **#893**（ホーム解決の残り 15 箇所）/ **#885**（spawn が backend の `wrap_spawn` を通らない）は未着手

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#889 の記録」節に 4 本の実機 A/B と
  ベースライン 23 件の内訳。「#872 の記録」節に無音終了の機序と偽の緑。「#884 の記録」節に
  argv の対照実測。「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
