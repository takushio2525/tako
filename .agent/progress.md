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

## 2026-11-01（#2602: bash のログインは `.bash_profile` → `.bash_login` → `.profile` の最初の 1 つだけを読むのに、setup が `.profile` / `.bash_login` だけの人に `.bash_profile` を作り、undo 後も 0 バイトで残して覆い隠していた → 書き先を `bash -l` が読む 1 つに（`ShellKind::login_profile_in`）・undo で 0 バイトになったファイルは消す・A/B `TAKO_2602_LEGACY=1`）
- 本物の `bash -l` で修正前は `.profile` の変数が unset → 修正後は読む。9 シェル × 5 通りの 45 ケースのうち変わったのは理由つきの 26（bash の書き先 2・作ったファイルの削除 24）で、残り 19 はバイト一致。注入 2 通りを番犬が file:line で名指し。カタログ +0 B。PR #2605

## 2026-11-01（#2365 の続き: dispatch の注記 32 種と fda の description を新しい `ReplyNote` へ、CLI の手元の断り 17 種を `Reason`・突然死からの復旧 3 種を `SessionRefusal` へ、同じ出力の行 17 種も `ReplyNote` へ）
- CLI 23 ケースで英語の日本語入り 23 → 0（日本語 20 / 23 sha256 一致・残り 3 は移行の知らせの言語の修正）・隔離 GUI 13 / 13 → 0 / 13（日本語 13 / 13 一致）・注入で名指し + E0004・カタログ +0 B。PR #2606

## 2026-11-01（#2597: tcsh / csh の利用者の明示コマンドのペインが `-l` で消える / 器経由は `Unmatched '''.` で止まる → ログインの層は表の csh 系の行を `exec -a` の段越しに・器の 1 行は csh の引用（A/B `TAKO_2597_LEGACY=1`））
- 隔離 GUI で修正前 9 本とも不成立（器なしの tcsh / csh 6 本 = `Unknown option`・器ありの tcsh 3 本 = `Unmatched`） → 修正後 tcsh / csh × 器なし / 器ありの 12 本が起きる・csh 系でない 6 種の argv は修正前と一致・カタログ +0 B。PR #2607

## 2026-11-01（#2603: 名前が dash / ksh のシェルは種別の分からないシェルの断りで手作業だった → 本物の dash と ksh93u+ が `-l -c`・argv[0]・対話ログインで `.profile` を読むと確かめて `ShellKind::Sh` へ寄せる（`SH_PROFILE_SHELLS`）・確かめていない mksh ほかは寄せない・A/B `TAKO_2603_LEGACY=1`）
- 砂場の CLI で修正前は断り（rc=1）・段は path → 修正後は `.profile` に書いて auth へ進み、本物の `dash -l` / `ksh -l` から claude を引ける。既知のシェル 8 種 × 3 通りの 24 ケースはバイト一致。注入 2 通りを番犬が file:line で名指し。カタログ +0 B。PR #2609

## 2026-11-01（#2608: GUI の自動復帰が daemon を入れ替える途中の remote status が、新版の待ち受けを PID ファイルの無い待ち受け（#2455）と読んで `tako remote stop` を案内していた → `spawn_daemon` が抜けるまで `tako-remote.starting` を置き、status は `starting`（replacing / starting）を返して stop を案内しない（書き手の死・pid の再利用・90 秒で失効・A/B `TAKO_2608_LEGACY=1`））
- 隔離 GUI の実経路で修正前 FAIL 6（区間の 35 読みが全部 stop の案内）→ 修正後 PASS 27・MCP も同じ・注入で番犬が file:line を名指し・カタログ +0 B。PR #2611

## 2026-11-01（#2365 の続き: check-health 一式と `tako setup --check` を表示言語へ。文は新しい `HealthText`（147 種）、共有の文は各モジュールの `_in(lang)`。`setup --check` は「setup は日本語」の枠から外した）
- 英語表示の日本語入り: CLI 経路 12/12 → 0/12・隔離 GUI の CLI / MCP 3/3 → 0/3（問題 0 件でも同じ）。日本語は全件一致（意図した差は旧文の空白 1 か所）。注入 7 通りを名指し / E0004・カタログ +0 B。PR #2612
- 09:09 に `31b5683` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）。再起動直後 90 秒の status 180 回で stop の案内 0（区間は starting.state=replacing）の本番確認で #2608 を閉じた

## 2026-11-01（#2610: save_layout が layout.json の書き出しとペインログの Mutex 待ちでメインスレッドを止めていた（本番最大 99 秒）→ 専用の書き手スレッド 1 本（最新 1 件の合流・tmp + rename）+ `try_lock`、終了と起動し直しの前は書き終わりを待つ・A/B `TAKO_2610_LEGACY=1`）
- 注入 5 秒で IPC 5,023ms → 18ms / ペインログ 4,840ms → 22ms・SIGKILL / 終了 / 順序 / 書けない dir を実経路で確認・番犬が逆戻り 4 種を file:line で名指し・カタログ +0 B。PR #2614

## 2026-11-01（#2613: 起動・入れ替えの途中（#2608 の印が効いている間）の `tako remote start` が旧版を止めて daemon を 2 本にしていた / 印を消していた → `spawn_daemon` が状態を読む前に印を見て `RemoteRefusal::DaemonStarting` で断る・stop / `--force` も止めても戻るので `DaemonStartingStop` で断る（A/B `TAKO_2613_LEGACY=1`））
- 実経路で修正前 FAIL 28（daemon 2 本・区間の残り 26 回が stop の案内）→ 修正後 PASS 59 / 0・番犬が注入 7 通りを file:line で名指し・カタログ +0 B。PR #2616

## 2026-11-01（#2365 の続き: `tako master` / `tako solo` の起動の流れを表示言語へ。言語を移行より前に決め（日本語表示に英語の移行の知らせが混ざっていた）、断りは `Reason` 3 種と型の口、行は `ReplyNote` 26 種。host.rs の既定 26 種を `HostRefusal::Default*` へ・mcp_host が表示言語と切り替えに従う）
- GUI 無しの CLI 19/23 → 0/23（日本語 14/23 一致・残り 9 は移行の知らせの修正）・隔離 GUI 10/10 → 0/10（日本語 10/10 一致）・mcp_host 12/12 → 0/12・注入 6 か所を名指し + E0004・カタログ +0 B。PR #2617
- 11:29 に `00a3b4b` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）。再起動直後の入れ替えの区間に打った remote start / stop が両方断られ、daemon は 1 本のまま新版へ移った本番確認で #2613 を閉じた
