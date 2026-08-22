# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-22）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#897（セルフテスト項目 94 が Windows で必ず止まる）を解消**（PR #901）。原因はテスト側で、
  **PTY へ書く Enter が LF だった**こと。端末の Enter は CR で、PSReadLine は素の LF を
  継続行（`>>`）の開始と解釈するのでコマンドが確定しない。POSIX は tty の ICANON + ICRNL が
  どちらも改行へ倒すので **CR に寄せれば両方通る**（方言差ではないので `ShellDialect` ではなく
  `self_test::pty_line` に置いた）。**到達範囲は項目 0〜99**（次の壁は 100 = #903）
- **#866（psmux で `tako tmux kill` が効かない）を解消**（PR #902）。psmux は `-t =name` を
  **解決せず「消えるまで 5 秒待つ」だけ**（素の名前が完全一致）。`=` の組み立てを
  `tako_core::tmux`（`TmuxTargetSyntax` / `exact_target`）1 本へ寄せ、直書き 33 箇所を通した。
  **項目 48 は Windows でも回る**（隣の `tako-test2` が残る対照つき）
- **#889（項目 93）/ #872（窓 0 枚の無音終了）も解消済み**（PR #900 / #895）。
  境界は `ShellDialect::echo_stdin_command()` / `integration_shell_command()` /
  `platform::window_lifecycle`
- **#766 / #870 / #884 / #881 / #877 / #875 / #873 も main へ入っている**（器の中のシェル統合の
  側路・ホーム解決の一本化・空白入り cwd・器へ渡す第 1 語・agents 走査・実行ペイン・方言判定）

## 実機セルフテストの到達範囲（#897 後の実測）

**項目 94（#702 alt screen）/ 95（#716）/ 96（#721）/ 97（#720 準備中）/ 98（#725 チャット
選択・コピー）/ 99（#739 起動カードのプロファイル）が Windows で初めて緑**になった。
次の壁は **#903**（項目 100 = #737 チャット入力欄）。診断が `got=None display=Chat tail=""`
= **画面に空でない行が 1 本も無い**（プロンプトすら出ていない）ので、シェルの準備を待たずに
送って起動途中の PTY が打鍵を落としている（#640 と同型）で確定に近い。#897 の LF ではない。

## 実機テストの読み方（#897 で実測。ここを間違えると判定が壊れる）

- **psmux の e2e は `schtasks /it`（session 1）で回す**。**SSH（session 0）で作った psmux の
  detached セッションは約 1 秒で自然死する**（#866 worker の実測）ので、
  `Invoke-CimMethod Win32_Process Create` で `cargo test` を投げると**測り方のせいで**落ちる。
  #897 でこれを踏み、単独走行でも **main = 10 件失敗 / branch = 7 件失敗**（16 本中）と
  main のほうが悪い結果になった。兄弟セッションの並行ビルドは増幅要因
- **`schtasks /it` で回すと psmux_backend が 16 / 0 で全緑**（23.59 秒。session 0 では
  91〜175 秒かけて 8〜10 件失敗）。**ワークスペース全体の失敗はちょうど 22 件で名前も一致**。
  つまり **ベースラインは 23 件ではなく 22 件**で、#889 が足した 23 件目と **#896 のフレークは
  どちらも session 0 で測っていた副作用**だった
- **隔離セルフテストと psmux e2e は孤児を残す**（psmux サーバーはプロセス名 `tmux.exe`）。
  `-L tako-iso-<pid>` / `-L tako-884test-<pid>` が自分の残骸で `-L tako` は本番。
  溜まると psmux e2e の失敗が増えるので run のたびに**明示 pid** で落とす

## 実機の作法（繰り返し踏んでいるもの）

- **GUI 起動時の env を再現してから測る**。`SHELL`（#877）も `HOME`（#870）も SSH セッションの
  Process スコープにしか無い（`Remove-Item Env:…` を先に打つ）
- **`Start-Process` で投げた長い処理は SSH が切れると死ぬ**。`schtasks`（GUI は `/it` で session 1）
  か `Invoke-CimMethod Win32_Process Create` で投げ、**完了はログの `EXITCODE=` 行で待つ**
  （プロセス消失で待つと `cargo run` の起動待ちを完了と誤判定する）。ログは UTF-8 で読む
- **GPUI の DirectX アトラスで落ちることがある**（#897 の run 1 で実測: 項目 66 付近で
  `directx_atlas.rs:255` の `unwrap` panic → `STATUS_STACK_BUFFER_OVERRUN`）。再実行で通った
- **fresh worktree は `web/tako-remote/dist/` を持たない**（.gitignore 済み）。`rust_embed` が
  埋め込むので tako-control のコンパイルが即失敗する。実機の `npm ci` は lock 不整合で落ちるので
  既存 worktree からコピーしてから cargo を回す
- **`git checkout -- <path>` はそのファイルの未コミット変更を全部捨てる**。実験の巻き戻しに
  使うと自分の作業ごと消える（#889 で 1 回踏んだ）
- **tmux ターゲットの `=` を直書きしない**（#866）。`tako_core::tmux::exact_target` /
  `session_pane_target` が `-V` の申告から決める（番犬が直書きを落とす。A/B は
  `TAKO_866_KEEP_EXACT_TARGET=1`）

## 次の一手

- **#903（項目 100 = #737）**: 直せば 100 以降（#748 / #749 / #761 / #772 / #781 / #789 /
  #803 / #813 / #815 / #816 / #826 / #830 / #835 / #822 / #868）が開く。
  直し方の案は Issue に書いた（準備待ちを入れる / リトライで送り直す。#796 の作法）
- **スライス 8（棚卸し）**: #865 の到達範囲表 + #872 / #875 / #877 / #884 / #889 / #897 の実測で
  `tako_run` / `tako_run_interactive` / `tako_show_command` / `tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` / `tako_window` / `tako_ui_mode` を `Pending` から倒せる材料が
  揃った。**#866 で `tako_tmux_list` / `tako_tmux_kill` も製品経路で通した**（`tako_tmux_resize` は
  psmux が `-x` を反映しないので Pending 継続 / `tako_tmux_open` は attach 前提で Pending 継続）。
  作法 4 に従いマトリクスは #897 / #866 でも触っていない
- **製品側の起票（未着手）**: **#898**（`dispatch::which` が POSIX 専用 = stale claude 検知が
  常に無効。境界 B16 `platform::exe::find` へ寄せる）/ **#899**（スターター・welcome の
  コマンド投入が LF + POSIX クォート = GUI モードのカードが機能しない）
- **同型の一族**（`$SHELL -l -c` の直書き）: `tako-core/src/lib.rs` / `tako-app/src/autorename.rs` /
  `tako-app/src/preview.rs` / `tako-control/src/config_share/env.rs` は
  **B21（`child_cmd::user_env_cli`）へ寄せられる形**。`setup_bootstrap.rs` の
  `login_shell_sees()` だけは**意図的な unix 専用**（#868 の申し送り）
- **#893**（ホーム解決の残り 15 箇所）/ **#885**（spawn が backend の `wrap_spawn` を通らない）は未着手

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#897 の記録」節に 94 → 100 の A/B と
  実機テストの読み方。「#889 の記録」節に 4 本の実機 A/B とベースライン内訳。「#872 の記録」節に
  無音終了の機序と偽の緑。「#866 の記録」節に psmux の `=` の機序と session 0 では
  測れない理由。「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
