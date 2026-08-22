# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-22）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#907（器つきペインへの非 ASCII 送達）を解消**（PR は本文末尾）。層は **psmux の client の
  打鍵経路**で確定（器なしはバイト等価 / 器ありだけ cp932 に無い文字が落ちる。カタカナ・漢字は
  通るので「日本語が壊れる」は半分外れ）。器の注入口（`send-keys -l`）は UTF-8 をそのまま運ぶので、
  **非 ASCII のときだけ打鍵ではなく注入口へ迂回**する（`keystrokes_ascii_only` 能力 +
  `SessionBackend::inject_text` + 純粋関数 `needs_text_injection`）。A/B は `TAKO_907_NO_INJECT=1`
- **#903（セルフテスト項目 100 = #737 チャット入力欄）を解消**（PR #908）。**Issue の仮説
  （準備待ちの不足）は外れ**、実測で機序が 4 つ出た: ①状態切替の Ctrl+C で**器（psmux）の
  client が終了**しペインごと死ぬ ②(g) の楽観 echo は**外側 PTY の alt screen** 条件なので
  器は外せない（器なしで alt screen へ入ると内側扱いで表示が Chat → Terminal へ落ちる）
  ③**器越しの打鍵から非 ASCII が落ちる**（`─` / `❯` が消える。出力経路は無傷と対照実験で確認）
  ④器は**内側コマンドを自分で単語分割する**ので引用符入りの `-Command '<片>'` は即死（#875 の 3 層問題）。
  直し方は疑似 TUI を**ペイン自身のコマンド + ファイル駆動**（`ShellDialect::repaint_file_loop`）に
  し、シェル片を **`-EncodedCommand`**（base64 / UTF-16LE）で渡す
- **#866（psmux で `tako tmux kill` が効かない）を解消**（PR #902）。psmux は `-t =name` を
  **解決せず「消えるまで 5 秒待つ」だけ**（素の名前が完全一致）。`=` の組み立てを
  `tako_core::tmux`（`TmuxTargetSyntax` / `exact_target`）1 本へ寄せ、直書き 33 箇所を通した。
  **項目 48 は Windows でも回る**
- **#897（項目 94）/ #889（項目 93）/ #872（窓 0 枚の無音終了）/ #727（設定画面のスリープ防止
  タブ）も解消済み**（PR #901 / #900 / #895 / #904）。境界は `self_test::pty_line` /
  `ShellDialect::echo_stdin_command()` / `platform::window_lifecycle` / `platform::lid`
- **#766 / #870 / #884 / #881 / #877 / #875 / #873 も main へ入っている**（器の中のシェル統合の
  側路・ホーム解決の一本化・空白入り cwd・器へ渡す第 1 語・agents 走査・実行ペイン・方言判定）

## 実機セルフテストの到達範囲（#903 後の実測）

**項目 100（#737 チャット入力欄）が Windows で初めて緑**。4 状態すべて `tries=1` で
`ready=true`（`outer_alt=Some(true)` / `inner_alt=false` / `backend=Some(...)` = 器つきのまま）。
3 回連続で再現し、`TAKO_903_LEGACY=1`（旧経路）に戻すと同じバイナリで FAILED になる。

次の壁は **#906**（項目 101 = #749 の自動ハンドオフ）。`TAKO_SELF_TEST_749_SPAWN` が
出ない = **spawn は成功しているのに fixture プロセスが終了している**
（`seen=None session=false size=None backend=None tail=""`）。`paint_and_hold` +
`Start-Sleep 3600` のはずが居なくなるので、`-EncodedCommand` 経由の
`Clear-Host` / `Write-Host` / `Start-Sleep` のどれかが器の中で落ちている疑い。

## 実機テストの読み方（#897 / #903 で実測。ここを間違えると判定が壊れる）

- **psmux の e2e / GUI セルフテストは `schtasks /it`（session 1）で回す**。SSH（session 0）で
  作った psmux の detached セッションは約 1 秒で自然死するので、測り方のせいで落ちる
- **ベースラインは 22 件**（#903 で再確認。失敗名も一致）。`schtasks /it` で回すと
  `psmux_backend` 16/0・`spawn_arg_quoting` 3/0・`shell_integration_powershell` 7/0・
  `encoding_conpty` 5/0 が全緑で、残る 22 件は #583 系の POSIX 前提テスト
- **孤児は run のたびに掃除する**（#903 で踏んだ）。隔離セルフテストは psmux サーバー
  6 個前後 + pwsh を残す。psmux 19 / pwsh 56 まで溜めた状態で走らせたら**項目 20 / 24 の
  固定待ちが落ちた**（掃除後は同じ HEAD で通った）。掃除は「tako-app が 1 つも居ない」を
  確かめてから `-L tako-iso-*` を明示 pid で落とす（`-L tako` は本番）
- **GPUI の DirectX アトラスで落ちることがある**（項目 66 付近で `directx_atlas.rs:255` の
  unwrap panic → `STATUS_STACK_BUFFER_OVERRUN`）。#903 では 3 回踏んだ。再実行で通る

## 実機の作法（繰り返し踏んでいるもの）

- **GUI 起動時の env を再現してから測る**。`SHELL`（#877）も `HOME`（#870）も SSH セッションの
  Process スコープにしか無い（`Remove-Item Env:…` を先に打つ）
- **`Start-Process` で投げた長い処理は SSH が切れると死ぬ**。`schtasks`（GUI は `/it` で session 1）
  か `Invoke-CimMethod Win32_Process Create` で投げ、**完了はログの `EXITCODE=` 行で待つ**
  （プロセス消失で待つと `cargo run` の起動待ちを完了と誤判定する）。ログは UTF-8 で読む
  （書き込み中は排他で `[System.IO.File]::ReadAllText` が失敗する。`Get-Content -Tail` は通る）
- **fresh worktree は `web/tako-remote/dist/` を持たない**（.gitignore 済み）。`rust_embed` が
  埋め込むので tako-control のコンパイルが即失敗する。実機の `npm ci` は lock 不整合で落ちるので
  既存 worktree からコピーしてから cargo を回す
- **`git checkout -- <path>` はそのファイルの未コミット変更を全部捨てる**。実験の巻き戻しに
  使うと自分の作業ごと消える（#889 で 1 回踏んだ）
- **`git stash` を A/B に使わない**（#903 で踏んだ）。変更が無いと no-op なのに `git stash pop` が
  他 worker の古い stash を pop してコンフリクトを作る。ファイルは
  `git checkout <sha> -- <path>` で差し替える
- **子プロセスの stdout を測るときは `[Console]::OutputEncoding` を UTF-8 にする**（#907）。
  既定は ANSI（cp932）なので `capture-pane -p` / `tako read` が測定側で化け、
  「送達が壊れた」と読み間違える
- **`tako persist off` は器つきの既存ペインを失う**（#907）。器あり / 器なしを比べるなら
  インスタンスを 2 本立てる（`TAKO_BACKEND=none` で 1 本）
- **`cp` が `-i` の別名かもしれない**: スクリプトでは `command cp -f` を使う（上書き確認で 10 分ハングした）
- **tmux ターゲットの `=` を直書きしない**（#866）。`tako_core::tmux::exact_target` /
  `session_pane_target` が `-V` の申告から決める（番犬が直書きを落とす。A/B は
  `TAKO_866_KEEP_EXACT_TARGET=1`）

## 次の一手

- **#906（項目 101 = #749）**: 直せば 101 以降（#761 / #772 / #781 / #789 / #803 / #813 /
  #815 / #816 / #826 / #830 / #835 / #822 / #868）が開く
- **スライス 8（棚卸し）**: #865 の到達範囲表 + #872 / #875 / #877 / #884 / #889 / #897 / #903 の
  実測で `tako_run` / `tako_run_interactive` / `tako_show_command` / `tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` / `tako_window` / `tako_ui_mode` を `Pending` から倒せる材料が
  揃った。**#866 で `tako_tmux_list` / `tako_tmux_kill` も製品経路で通した**（`tako_tmux_resize` は
  psmux が `-x` を反映しないので Pending 継続 / `tako_tmux_open` は attach 前提で Pending 継続）。
  作法 4 に従いマトリクスは #897 / #866 / #903 でも触っていない
- **製品側の起票（未着手）**: **#898**（`dispatch::which` が POSIX 専用 = stale claude 検知が
  常に無効。境界 B16 `platform::exe::find` へ寄せる）/ **#899**（スターター・welcome の
  コマンド投入が LF + POSIX クォート = GUI モードのカードが機能しない）
- **同型の一族**（`$SHELL -l -c` の直書き）: `tako-core/src/lib.rs` / `tako-app/src/autorename.rs` /
  `tako-app/src/preview.rs` / `tako-control/src/config_share/env.rs` は
  **B21（`child_cmd::user_env_cli`）へ寄せられる形**。`setup_bootstrap.rs` の
  `login_shell_sees()` だけは**意図的な unix 専用**（#868 の申し送り）
- **#893**（ホーム解決の残り 15 箇所）/ **#885**（spawn が backend の `wrap_spawn` を通らない）は未着手

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#903 の記録」節に 4 つの機序と A/B 表・
  作法。「#897 の記録」節に 94 → 100 の A/B と実機テストの読み方・ベースライン内訳。
  「#889 の記録」節に 4 本の実機 A/B。「#872 の記録」節に無音終了の機序と偽の緑。
  「#866 の記録」節に psmux の `=` の機序と session 0 では測れない理由。
  「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
