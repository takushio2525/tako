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
  `integration_shell_command()` の 2 本。**到達範囲は項目 0〜93 になった**
- **#766 / #870 / #884 / #881 / #877 / #875 / #873 も main へ入っている**（器の中のシェル統合の
  側路・ホーム解決の一本化・空白入り cwd・器へ渡す第 1 語・agents 走査・実行ペイン・方言判定）

## 実機の作法（繰り返し踏んでいるもの）

- **GUI 起動時の env を再現してから測る**。`SHELL`（#877）も `HOME`（#870）も SSH セッションの
  Process スコープにしか無い。`Remove-Item Env:SHELL` を先に打たないと壊れた経路が動いて見える
- **A/B は「修正だけを戻す」形にする**。テストごと戻すと原因の層が切り分けられない
- **fresh worktree は `web/tako-remote/dist/` を持たない**（.gitignore 済み）。`rust_embed` が
  埋め込むので tako-control のコンパイルが即失敗する。`npm run build`（実機は npm ci が
  lock 不整合で落ちるので既存 worktree からコピー）してから cargo を回す
- **実機セルフテストは schtasks（session 1）で回し、完了は `EXITCODE=` 行で待つ**。
  プロセス消失で待つと `cargo run` の起動待ちを完了と誤判定する。ログは UTF-8 で読む
- **`git checkout -- <path>` は「その 1 ファイルの未コミット変更を全部捨てる」**。
  実験の巻き戻しに使うと自分の作業も消える（このセッションで 1 回踏んだ = python で再適用した）

## #889 で分かったこと（次の worker が踏みやすい）

- **`$PROFILE` に配置済みの実機では原因 2 が隠れる**。本番スクリプト
  （`%APPDATA%\tako\shell-integration\tako.ps1`）をリネームすると「未配置の実機」を再現できる
  （`$PROFILE` のブロックは `Test-Path` で守られている）= 隔離検証の変数として使える
- **PowerShell ペインの Enter は CR**。素の LF は PSReadLine が継続行（`>>`）にするので
  コマンドが確定しない。**セルフテスト項目 94 と実機テスト 1 件がこれで落ちている（#897）**
- **テストが製品の組み立てを決め打ちしていると、製品を直した瞬間に壊れる**。項目 93 (d) の
  期待値は `welcome::launch_command_line` から作る形にした（macOS は従来と同一文字列）

## 次の一手

- **#897（項目 94 = Enter の LF）**: 直せばセルフテストが 94 以降へ進み、実機テストの失敗も
  23 → 22 件へ戻る。直しは `\n` → `\r`（セルフテスト項目 94 と `psmux_backend.rs:930`）
- **スライス 8（棚卸し）**: #865 の到達範囲表 + #875 / #877 / #884 / #889 の実測で
  `tako_run` / `tako_run_interactive` / `tako_show_command` / `tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` / `tako_ui_mode` を `Pending` から倒せる材料が揃った
  （作法 4 に従いマトリクスは #889 でも触っていない）
- **製品側の起票**: **#898**（`dispatch::which` が POSIX 専用 = stale claude 検知が常に無効。
  境界 B16 `platform::exe::find` へ寄せる）/ **#899**（スターター・welcome のコマンド投入が
  LF + POSIX クォート）。どちらも #889 の検証中に実測で見つけたもの
- **同型の一族**（`$SHELL -l -c` の直書き）が残っている: `tako-core/src/lib.rs` /
  `tako-app/src/autorename.rs` / `tako-app/src/preview.rs` /
  `tako-control/src/config_share/env.rs` は **B21（`child_cmd::user_env_cli`）へ寄せられる形**。
  `setup_bootstrap.rs` の `login_shell_sees()` は**意図的な unix 専用**なので機械的に寄せない
- **#885**（tako-app の spawn が backend の `wrap_spawn` を通らない）/ **#893**（ホーム解決の
  残り 15 箇所）は恒久課題として未着手

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#889 の記録」節に 4 本の実機 A/B と
  ベースライン 23 件の内訳。「#884 の記録」「#881 の記録」「agents 走査の Windows 対応（#877）」
  「スライス 8 の前提: 器の中のシェル統合（#766）」「8 の前提」節に各実測と実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
