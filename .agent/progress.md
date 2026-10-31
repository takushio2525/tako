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

## 2026-10-31（#2577: tcsh / csh のログインシェルへ `-l -c` を渡さず、argv[0] を `-<名>` にした `-c 'exec /bin/sh -c $argv[1]:q'` の形で起こす。表 `exe::login_invocation` の 1 実装へ寄せ、`ChildCmd::command` が argv[0] を当てる。A/B `TAKO_2577_LEGACY=1`）
- 砂場の check-health で tcsh / csh / 別名は修正前 claude 未検出・git 不明 → 修正後どちらも検出。zsh / bash / sh / fish / 空はバイト一致・番犬 `issue2577_csh_login_shell_watchdog`・カタログ +0 B。PR #2593

## 2026-10-31（#2365 の続き: GUI 側 = git パネルの 13 呼び出しを型の口へ・動画 `VideoRefusal`・Web ビューの親 `HostRefusal::WebViewParentDetached`・ファイルのクリップボード `ClipboardWriteError`・`.app` の差し替え `BundleError` / `SwapCause`・プレビューのリンク / アプリを選んで開く / 再起動は os_integration の型の口）
- 隔離 GUI の CLI / MCP で英語表示の日本語入り 8 / 8 → 0 / 8・日本語は sha256 一致・GUI 無しの実経路 4 / 4 → 0・注入 6 通りを名指し + E0004・カタログ +0 B。PR #2596

## 2026-11-01（#2594: SSH ペインが ssh の失敗・切断後に戻るローカルのシェルに統合が載らなかった → 戻る直前に素のペインと同じ統合（`shell_integration::relaunch`）を当て直す。A/B `TAKO_2594_LEGACY=1`）
- 隔離 GUI の A/B で戻りのシェルが `unknown → failed`・cwd 追従・HISTFILE が素のペインと同じ（器あり・切断・bash も）・番犬が呼び手を file:line で名指し・カタログ +0 B。PR #2599
- 02:53 に `3638362` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）。本番 GUI の SSH ペインの戻りのシェルが state=failed / cwd 追従 = 素のペインと一致した本番確認で #2594 を閉じた

## 2026-11-01（#2592: `SHELL` の名前が sh のとき setup が `~/.bash_profile` へ書き、`sh -l` は `~/.profile` だけを読むので段が path に残っていた → `ShellKind::Sh`（`~/.profile`）・undo は移す前の `.bash_profile` のブロックも外す・A/B `TAKO_2592_LEGACY=1`）
- 砂場の CLI で修正前は verified=false・next_step=path、修正後は verified=true・auth。既知のシェル 8 種は 15 ファイルずつバイト一致。注入 2 通りを番犬が file:line で名指し。カタログ +0 B。PR #2600

## 2026-11-01（#2598: `wait_isolated_gui` の素の `tako list` が呼び手の env（本番の `TAKO_SOCKET` / 既定の置き場）で本番 GUI へ届き、隔離 GUI が立つ前に返っていた → ヘルパの CLI を `iso_cli`（本番の env を落とし GUI と同じ発見の置き場）へ・置き場が空ならログの隣に決めて GUI にも渡す・番犬が `scripts/lib/*.sh` を「その行で隔離」の規則で読む（A/B `TAKO_2598_LEGACY=1`））
- 本番役の隔離 GUI + 中継で修正前は 7 通り中 6 通りで本番役へ届く（1 回の待ちで 1 件・launch を通さない待ちは 30 件）→ 修正後 0 件・代表 2 本の rc と PASS / FAIL は一致・カタログ +0 B。PR #2601
