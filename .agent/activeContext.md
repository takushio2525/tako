# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-24）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#898（コマンド解決の `which` 決め打ち）を解消**。`which` は Windows に無いので
  解決が例外なく `None` = tako.exe が PATH 上に居るのに「無い」ように見えていた。境界 B16
  （`platform::exe::find`）へ寄せ、`resolve_tako_binary` の「隣」も `EXE_SUFFIX` で組む形へ。
  **Issue の一覧より 2 箇所多かった**（`stale_binary` の複製 / 設定画面のエージェント検出）。
  番犬は `which` / `where` の直起動をソース走査で禁止（許可リストは空）
- **#920（項目 119 = #868 install_plan）を解消 → Windows のセルフテストが完走**
  （`TAKO_APP_SELF_TEST_OK` / exit 0 / FAILED 0・skip 19 は全部理由つきの既知）。
  原因はテスト側の unix リテラル（`claude.ai/install.sh` / `.local/bin/claude`）で、
  期待値を `agent_install::current_recipe` から作る形へ。**同型のリテラルが単体テストにも
  あり**（`導入計画は何をどこに入れるかを必ず含む` = 実機ベースライン 22 件の 1 つ）
  そちらも直したので**ベースラインは 22 → 21**。A/B は `TAKO_920_LEGACY=1`
- **#913（項目 116 = #835）を解消**。両側の POSIX 前提（テスト側が `file://C%3A%5C…` を作り、
  製品側は `/` 始まりでないと弾く）。**同じ RFC 8089 の規則が `osc_tap` に既にあった**ので
  `tako_core::file_uri` へ一本化。A/B は `TAKO_913_LEGACY=1`
- **#927（public リポの現行コードに残っていた実ユーザー名・実ホームパス）を除去**。
  4 ファイル（`claude_tui.rs` / `shell_profile.rs` / plan / progress）をプレースホルダへ置換し、
  番犬 `crates/tako-control/tests/no_personal_data.rs` を新設（ホームパス形の許可リスト = CI で効く +
  環境から作る当該マシンの識別子 = 手元で効く の 2 本立て）。**実機の採取物を貼る前に置換する**
  のが規約（`.agent/conventions.md` / AGENTS.md 絶対ルール）
- **解消済み（詳細は plan の各記録節と progress.md）**: #927 / #907（器つきペインへの非 ASCII 送達 →
  打鍵ではなく器の注入口へ迂回）/ #903（項目 100 = 疑似 TUI をファイル駆動 + `-EncodedCommand`）/
  #866（psmux は `-t =name` を解決しない）/ #897（PTY へ書く Enter は CR）/ #889（項目 93）/
  #872（窓 0 枚の無音終了）/ #727 / #905（スリープ系の呼び名）/ #766 / #870 / #884 / #881 /
  #877 / #875 / #873
- **A/B の env（同一バイナリで旧挙動へ戻せる）**: `TAKO_920_LEGACY` / `TAKO_913_LEGACY` / `TAKO_906_NO_PAD` /
  `TAKO_907_NO_INJECT` / `TAKO_903_LEGACY` / `TAKO_866_KEEP_EXACT_TARGET`

## 実機セルフテストの到達範囲（#920 後の実測）

**完走した**（`TAKO_APP_SELF_TEST_OK` / exit 0 / FAILED 0）。#865 で項目 1b が落ちて
カバレッジ 0 だった状態から、#866 / #870 / #872 / #875 / #877 / #881 / #884 / #889 /
#897 / #903 / #906 / #913 / #920 を積んで全項目に到達した。

skip は 19 件で全部理由つきの既知（psmux が本物の tmux でない系 #519 / PDF の
text_layer 不在 #693 / WebView2 の panic #724 / macOS 固有の項目 79 #872 /
POSIX 専用の道具 = nc・ジョブ制御・`/dev/fd`・ECHOCTL #729 / links の POSIX 前提 #522 /
蓋閉じで未描画になる項目）。**直れば自動で検証が復活する形**なので、
残りはスキップ理由の Issue を潰すぶんだけ。

**実機テストのベースラインは 22 → 21**（#920 で
`setup_bootstrap::tests::導入計画は何をどこに入れるかを必ず含む` を直した）。

## 実機テストの読み方（要点。全文は plan の各記録節）

- **psmux の e2e / GUI セルフテストは `schtasks /it`（session 1）で回す**。SSH（session 0）で
  作った psmux の detached セッションは約 1 秒で自然死するので、測り方のせいで落ちる
- **実機テストのベースラインは 22 件**（#920 で 21 へ減ったが、#919 由来の
  `remote_fs_e2e::解決できないホストは接続前に分類される` が加わって 22。**#930** で起票済み。
  失敗名まで照合する）。`schtasks /it` で回すと
  `psmux_backend` 16/0・`spawn_arg_quoting` 3/0・`shell_integration_powershell` 7/0・
  `encoding_conpty` 5/0 が全緑で、残る 21 件は #583 系の POSIX 前提テスト（+ #930 の 1 件）
- **孤児は run のたびに掃除する**（psmux 19 / pwsh 56 まで溜めると項目 20 / 24 の固定待ちが落ちた）。
  「tako-app が 1 つも居ない」を確かめてから `-L tako-iso-*` を**明示 pid で**落とす（`-L tako` は本番）
- **GPUI の DirectX アトラス panic**（項目 66 付近）は既知のフレーク。再実行で通る
- **GUI 起動時の env を再現してから測る**（`SHELL` / `HOME` は SSH セッションの Process
  スコープにしか無い）。長い処理は `schtasks` か `Invoke-CimMethod` で投げ、**ログの
  `EXITCODE=` 行で完了を待つ**（プロセス消失で待つと `cargo run` の起動待ちを完了と誤判定する）
- **測定側も UTF-8 にする**（`[Console]::OutputEncoding`。既定 cp932 だと `capture-pane -p` /
  `tako read` が測定側で化けて「送達が壊れた」と読み間違える）。ログは `-Encoding UTF8` で読む
- **`git stash` を A/B に使わない**（他 worker の古い stash を pop する）。`git checkout <sha> -- <path>`。
  ただし**未コミットのまま `git checkout HEAD -- <path>` は自分の変更を全部捨てる**
  （main との比較の前に必ずコミットする。2026-08-24 に踏んで再適用した）
- **fresh worktree は `web/tako-remote/dist/` を持たない**（`rust_embed` が埋め込むので即失敗）。
  実機の `npm ci` は lock 不整合で落ちるので既存 worktree からコピーする
- **`cp` が `-i` の別名かもしれない**: スクリプトでは `command cp -f` を使う

## 次の一手

- **スライス 8（棚卸し）が本命になった**: セルフテストが完走したので「Windows で何が
  動いて何が動かないか」は skip 19 件 + 実機テスト 21 件がそのまま実測一覧になる
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

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#920 の記録」節に完走までの経緯と
  skip 19 件の内訳。「#913 の記録」節に file URI の
  ドライブレター規則の一本化と「規則が 2 か所にあり片方だけ POSIX 専用」の型。
  「#906 の記録」節に psmux の `==` 拒否の
  A/B 表と「内容依存と位置依存の切り分け」の作法。「#903 の記録」節に 4 つの機序と A/B 表・
  作法。「#897 の記録」節に 94 → 100 の A/B と実機テストの読み方・ベースライン内訳。
  「#889 の記録」節に 4 本の実機 A/B。「#872 の記録」節に無音終了の機序と偽の緑。
  「#866 の記録」節に psmux の `=` の機序と session 0 では測れない理由。
  「8 の前提」節に到達範囲・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
