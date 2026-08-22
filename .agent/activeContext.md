# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-22）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#906（項目 101 = #749 の自動ハンドオフ）を解消**（PR は本文末尾）。**Issue の当たりは全部外れ**で、
  層は **psmux の `new-session` そのもの**（`アクセスが拒否されました。(os error 5)` / exit 5）。
  条件は **`-EncodedCommand` の base64 が `==` で終わること**で、**同一長で padding だけを
  変えると反転する**（448 / 544 / 576 の 3 点で A/B）。コマンドライン側は無関係。
  直し方は符号化の出口（`platform::shell::container_safe_script`）で二重パディングを作らない
  = 末尾へ空白 1 個。A/B は `TAKO_906_NO_PAD=1`
- **解消済み（詳細は plan の各記録節と progress.md）**: #907（器つきペインへの非 ASCII 送達 →
  打鍵ではなく器の注入口へ迂回）/ #903（項目 100 = 疑似 TUI をファイル駆動 + `-EncodedCommand`）/
  #866（psmux は `-t =name` を解決しない）/ #897（PTY へ書く Enter は CR）/ #889（項目 93）/
  #872（窓 0 枚の無音終了）/ #727 / #905（スリープ系の呼び名）/ #766 / #870 / #884 / #881 /
  #877 / #875 / #873
- **A/B の env（同一バイナリで旧挙動へ戻せる）**: `TAKO_906_NO_PAD` / `TAKO_907_NO_INJECT` /
  `TAKO_903_LEGACY` / `TAKO_866_KEEP_EXACT_TARGET`

## 実機セルフテストの到達範囲（#906 後の実測）

**到達範囲は項目 0〜115**（#906 で 101 が開き、102（#761 / #792）103（#772）105（#778）
106（#781）110（#803）111（#813）112（#815）114（#826）115（#830）が Windows で初めて緑）。
`TAKO_906_NO_PAD=1`（旧経路）に戻すと同じバイナリで項目 101 が FAILED になる。

次の壁は **#913**（項目 116 = #835 Finder の「このアプリケーションで開く」）。
`116: file URL が 4 本ともパスへ戻る (1)` で、**器とも符号化とも無関係の POSIX パス前提**:
`self_test::file_url` が `path.display()` をそのまま符号化するので Windows では
`file://C%3A%5CUsers%5C…` になり、`open_files::file_url_to_path` は `/` 始まりでないと
`None` を返す（4 本のうち通るのは `/` 始まりのダミー 1 本だけ = 観測値と一致）。

## 実機テストの読み方（要点。全文は plan の各記録節）

- **psmux の e2e / GUI セルフテストは `schtasks /it`（session 1）で回す**。SSH（session 0）で
  作った psmux の detached セッションは約 1 秒で自然死するので、測り方のせいで落ちる
- **実機テストのベースラインは 22 件**（失敗名まで照合する）。`schtasks /it` で回すと
  `psmux_backend` 16/0・`spawn_arg_quoting` 3/0・`shell_integration_powershell` 7/0・
  `encoding_conpty` 5/0 が全緑で、残る 22 件は #583 系の POSIX 前提テスト
- **孤児は run のたびに掃除する**（psmux 19 / pwsh 56 まで溜めると項目 20 / 24 の固定待ちが落ちた）。
  「tako-app が 1 つも居ない」を確かめてから `-L tako-iso-*` を**明示 pid で**落とす（`-L tako` は本番）
- **GPUI の DirectX アトラス panic**（項目 66 付近）は既知のフレーク。再実行で通る
- **GUI 起動時の env を再現してから測る**（`SHELL` / `HOME` は SSH セッションの Process
  スコープにしか無い）。長い処理は `schtasks` か `Invoke-CimMethod` で投げ、**ログの
  `EXITCODE=` 行で完了を待つ**（プロセス消失で待つと `cargo run` の起動待ちを完了と誤判定する）
- **測定側も UTF-8 にする**（`[Console]::OutputEncoding`。既定 cp932 だと `capture-pane -p` /
  `tako read` が測定側で化けて「送達が壊れた」と読み間違える）。ログは `-Encoding UTF8` で読む
- **`git stash` を A/B に使わない**（他 worker の古い stash を pop する）。`git checkout <sha> -- <path>`
- **fresh worktree は `web/tako-remote/dist/` を持たない**（`rust_embed` が埋め込むので即失敗）。
  実機の `npm ci` は lock 不整合で落ちるので既存 worktree からコピーする
- **`cp` が `-i` の別名かもしれない**: スクリプトでは `command cp -f` を使う

## 次の一手

- **#913（項目 116 = #835）**: 直せば 116 以降（#822 / #868 / #853 / #858 …）が開く。
  直し方は「Windows の入口に合わせて項目を gate する」か「`open_files` に Windows 形
  （`file:///C:/…`）を教える」の判断が要る
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

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#906 の記録」節に psmux の `==` 拒否の
  A/B 表と「内容依存と位置依存の切り分け」の作法。「#903 の記録」節に 4 つの機序と A/B 表・
  作法。「#897 の記録」節に 94 → 100 の A/B と実機テストの読み方・ベースライン内訳。
  「#889 の記録」節に 4 本の実機 A/B。「#872 の記録」節に無音終了の機序と偽の緑。
  「#866 の記録」節に psmux の `=` の機序と session 0 では測れない理由。
  「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
