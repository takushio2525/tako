# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> **1 エントリは 1〜3 行**（何を / どこを / 結果）。詳細は git log・Issue・PR・`.agent/plans/` に委ねる。

このファイルは `AGENTS.md` から `@import` されるので**毎ターン全文が読み込まれる**。
予算（直近 5 作業日 / 20 エントリ / 12 KB）を超えたぶんは `progress-archive.md` へ
1 行で移る。移送は `tako context-budget fix` が行う（冪等・本文は改変しない・
全文は git 履歴に残る）。規約の全文は `AGENTS.md`「起動時ロードの予算」節。

## 追記フォーマット

```markdown

## YYYY-MM-DD（#Issue 一言）
- {何を / どこを / 結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-10-31（#2555: LSP の e2e が `stop`（待たずに返る）の直後に置き場を消し、偽サーバの `exit` の書き込みで `tako-2035-*` / `tako-2048-*` が消え残っていた → テストの最後は `shutdown_all`・置き場は `ScratchDir`・番犬 `issue2555_lsp_stop_then_scope_end_watchdog`）
- 修正前 10 回で 2035 は 20 個・2048 は 2 個 → 修正後 0 個・SIGKILL の残りは test-residue が数える・注入で番犬が 4 行を名指す・既存の 257 個を片付け・カタログ +0 B。PR #2558

## 2026-10-31（#2557: setup bootstrap の未知のシェルへの断りが `export PATH="~/.local/bin:$PATH"` を 4 行重ねていた → tako-core `shell_profile::dirs_for_unknown_shell`（書く行と同じ `$HOME/…`・重複なし）でディレクトリだけ並べ、構文はそのシェルの書き方に任せる）
- CLI / MCP（日英）とも 4 行 → 1 行・既知のシェル 7 種の rc と応答はバイト一致・注入 3 通りで file:line を名指し・カタログ +0 B。PR #2563

## 2026-10-31（#2561: MCP 設定のテストの `mcp_test_dir` と #944 の番犬の `scratch` を `ScratchDir` へ寄せ、子をプロセスグループの頭で起こして刈り取る前に孫を止める・番犬 `issue2561_temp_dir_helper_watchdog`）
- 修正前は専用の TMPDIR で 3 回走らせると 7→14→21 個（944 は偽 agy の注入で 3→6→9 個）、修正後はどちらも 0→0→0 個。本物の `$TMPDIR` から 976 個を消した（失敗 0）・カタログ +0 B。PR #2562

## 2026-10-31（#2365 の続き: 委任台帳・引き継ぎの書き込み・supervisor・後任 master の起動コマンドを `ConfigError`、非同期 run・手順書を `Reason`、設定値の範囲・paste-guard を tako-core の `RangeRefusal` / `PolicyRefusal` へ。置き場 21 か所は #2497 で済みと確認）
- 隔離 GUI の A/B で英語表示の日本語入り 24 / 24 → 0 / 24・日本語は sha256 一致・注入 12 か所を名指し + E0004・カタログ +0 B。PR #2565

## 2026-10-31（#2566: IPC のテストの偽の受け口を `ScratchDir` の下へ・dispatch / orchestrator のテストの補助が設定の置き場の OnceLock を後始末なしで先に初期化するのを外す・#2561 の番犬を String / ソケット / OnceLock の初期化子へ広げる）
- 修正前は専用の TMPDIR で 3 回走らせると `tk2123*` 3→6→9・dispatch-config 1→2→3、修正後はどちらも 0→0→0。本物の `$TMPDIR` から 341 個を消した（失敗 0）・カタログ +0 B。PR #2568

## 2026-10-31（#2559: GUI をペインから再起動すると自動命名の `claude -p` がペインの識別と mod の注入を受け継ぎ、master の `mod_turn` / `session_id` / `rate_limits` を haiku の報告で上書きしていた → 起動形で `helper_env` を外す + CLI が祖先を添え GUI がペインの外の報告を `foreign` で捨てる（A/B `TAKO_2559_LEGACY=1`））
- 隔離 GUI + 偽 claude で修正前は上書き・`ended` で報告消失 → 修正後 0・番犬 `issue2559_helper_report_watchdog`・カタログ +0 B。PR #2567
- 本番確認で #2559 を閉じた

## 2026-10-31（#2569: 台帳 / projects.yaml / accounts.yaml の `mutate` が閉包の断りでも書き戻し 0 件のファイル作成・手書きの形の書き直しをしていた → `try_mutate_typed`（Err なら書かない）へ寄せ、番犬 `issue2569_refusal_writeback` が file:line で名指す）
- 実経路（CLI / MCP・日英）で修正前 NG 32 → 修正後 0・出力 128 ファイルがバイト一致・カタログ +0 B。PR #2572

## 2026-10-31（#2365 の続き: 対応マトリクス・移行・使用量・起動時ロードの予算の断りを `Reason` の 10 種、remote 系（会話の末尾・端末・daemon の admin API・スクロールバック・セットアップ）を新しい `RemoteRefusal`（31 種）へ。daemon の本文に `kind` を添えて呼び手の言語で組み直す）
- 実経路で英語表示の日本語入り dispatch 27 / 27・CLI 18 / 22・MCP 15 / 15 → 0・日本語は sha256 一致・注入 6 通りで名指し + E0004・カタログ +0 B。PR #2573

## 2026-10-31（#2564: 未知のシェルで tako setup が同じ断りで止まる `tako setup bootstrap path` へ誘導していた → 残りは `AgentPathByHand`（次の 1 行なし・理由に足すディレクトリの全文）、tako CLI の段と `--check` は一覧で閉じる、足し終えたら断らず already_on_path）
- 未知のシェル 12 本で誘導 0・既知のシェル 60 ファイルがバイト一致・注入 5 通りで file:line を名指し・カタログ +0 B。PR #2571

## 2026-10-31（#1632: remote のトークン・QR・状態ファイルの置き場を Windows でも本人 + SYSTEM だけの DACL（継承を切る）にする。境界 B32 `platform::private_file`・A/B `TAKO_1632_LEGACY=1`・unix は同じ呼び出し）
- CI の Windows ジョブで Win32 と icacls の読み戻し（修正後 2 者・`(I)` 無し / 旧アームは継承のまま）・番犬が no-op への戻しを file:line で名指す・カタログ +0 B。PR #2575

## 2026-10-31（#2570: ペインの中から起こし直した GUI がペインの env（識別・接続・claude の印・`GIT_EDITOR=true`・重なった `XDG_DATA_DIRS`）を新しいペインと子へ素通しにしていた → `tako_core::host_env::STRIP` を main の最初で外し unix は exec し直す（A/B `TAKO_2570_LEGACY=1`））
- 隔離 GUI の A/B でペインと子の env が Dock 起動相当と一致・`ps eww` も外れる・番犬 `issue2570_inherited_pane_env_watchdog`・カタログ +0 B。PR #2574
- 19:03 に `3aac66e` を install・GUI 再起動（master のペインから `open -a` = この再 exec の経路・復元失敗 0）の本番確認で #2570 を閉じた

## 2026-10-31（#2365 の続き: tmux のセッション操作（`TmuxRefusal` / `TmuxRunError`。口は名前のまま型を返す）・git show・edit type・LSP の初期化オプション・worker の解決とレジストリ・layout の config.yaml・remote agents の断りを表示言語へ）
- GUI 無しの実経路で英語の日本語入り 26 / 27 → 0（残り 1 は git の stderr）・隔離 GUI の CLI / MCP 18 / 18 → 0 / 18・日本語は sha256 一致・注入 6 か所を名指し + E0004・カタログ +0 B。PR #2579

## 2026-10-31（#2576: 未知のシェルで welcome の次の一手と check-health の cli_in_path が `tako setup bootstrap path` へ誘導していた → welcome は `NextStep::PathByHand`（段は path のまま command null・dirs と断りの全文）、check-health は `--check` と同じ判定 `tako_cli_dir_by_hand` + `tako_cli_in_path_issue`（日本語固定））
- 未知のシェル 4 ケース × 5 出力で誘導 0・既知のシェルと足し終えた後の 48 ファイルがバイト一致・注入 5 通りで file:line を名指し・カタログ +0 B。PR #2580

## 2026-10-31（#2581: control.json と instances/・IPC のトークン・usage の塩・shortcuts・スマホから受け取ったファイル・ペインの最後の出力の置き場を境界 `platform::private_file` へ寄せ、Windows も本人 + SYSTEM だけの DACL に。境界は当てられない置き場で呼び手を止めず・同じ DACL は当て直さない）
- CI の Windows で Win32 と icacls の読み戻し・隔離 GUI の A/B で unix の mode と CLI / MCP の接続が修正前と一致・番犬が境界の外の本人だけの mode を file:line で名指す・カタログ +0 B。PR #2584

## 2026-10-31（#2578: claude 等が改行 + 字下げで折り返した URL / パスを cmd+クリック・ホバー・cmd+右クリック・`tako links` で 1 本にする。修正前の結合で決めてから新しい境目だけ確かめて重ねる・塊ごとの行の上限・`wrap` を応答へ・A/B `TAKO_2578_LEGACY=1`）
- 実経路で修正後 PASS 24 / 修正前 FAIL 20（直結・器 × 端末 / アプリ / 実物の claude）・注入 3 通りで番犬が file:line を名指し・カタログ +0 B。PR #2586
- Closes で閉じたが実画面の cmd+クリックが未実測なので master が開け直した（人の確認待ち = `tako todo`）

## 2026-10-31（#2582: ペインの中から CLI で起こした remote daemon がペインの env（識別・接続・claude の印・`GIT_EDITOR=true`）を持ったまま常駐していた → CLI の `main` が `tako remote serve` のときだけ `host_env::STRIP` で外し exec し直す（audit.log `daemon_startup_env`・A/B `TAKO_2582_LEGACY=1`））
- 隔離 GUI の A/B で CLI から起こした daemon の印 32 → 0・自動復帰の daemon は修正前と一致・番犬 `issue2582_daemon_inherited_pane_env_watchdog` + 実プロセスのテスト・カタログ +0 B。PR #2585
- 本番確認（ペインの CLI から remote stop → start・本番 daemon の env にペインの印 0・陽性対照 = 同じペインの claude にはある・audit.log に `daemon_startup_env`）で #2582 を閉じた

## 2026-10-31（#2365 の続き: `tako config` の断り 33 種を `ShareRefusal` / `GitError`（その場で型を返す）へ、CLI の手元 3 か所（watch --worker・lsp options --set・remote agents）を型の口へ。応答の push_error / files[].error も表示言語）
- 実経路 23 ケースで英語の日本語入り 23 → 0・CLI 27 / 28 → 0 / 28（日本語 28 / 28 sha256 一致）・隔離 GUI の MCP 19 / 19 → 0 / 19・注入 6 か所を名指し + E0004・カタログ +0 B。PR #2589

## 2026-10-31（#2583: setup の PATH の案内の残り = SHELL 未設定の問い合わせを setup の既定 zsh へ揃える（`exe::fallback_login_shell`・child_cmd も 1 実装）・handoff の PATH の段と check-health の `tako_cli_link` は未知のシェルで bootstrap path を示さない（A/B `TAKO_2583_LEGACY=1`））
- 砂場の CLI と隔離 GUI で修正前は path が残る / 誘導 1 → 修正後は auth・login へ進む / 誘導 0・既知のシェル 12 種はバイト一致・注入 7 通りで file:line を名指し・カタログ +0 B。PR #2590
- 23:36 に `3f031cc` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）

## 2026-10-31（#2588: 隔離 GUI の後片付けで `stop_isolated_gui` が GUI の pid だけを待って返し、SIGHUP を受けたペイン（SSH ペインの統合の無い zsh）の履歴保存と直後の `rm -rf "$TMP"` が競って rc=1 → GUI を落とす前にペインのプロセス（別の端末を持つ子孫）を控え、終わるまで待つ・居座りは TERM → KILL・番犬 `issue2588_stop_waits_pane_exit`）
- 本物の 1449 で修正前 + 注入 rc=1 → 修正後 + 同じ注入 rc=0・注入なし 5 / 5 rc=0（PASS 38 / FAIL 0・残骸 0）・逆戻りを番犬が file:line で名指し・カタログ +0 B。PR #2591
