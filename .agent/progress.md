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

## 2026-10-28（#2455: PID ファイルが消えた remote daemon を `tako remote stop` で止める = 管理トークンの停止要求の応答から pid を得る・status に `pidless_listener`）
- 隔離の実 daemon で修正前は生き残る → 修正後は `via: shutdown` で停止・注入で 4 本が FAILED・カタログ +0 B。PR #2463

## 2026-10-28（#2448: テストが残す `sleep 600` は probe の「少し出してから固まる子」の孫 → `exec` にし、一巡の番犬がプロセスグループの残りを数える）
- PR #2465

## 2026-10-28（#2431: Windows の WebView2 のユーザーデータを `%LOCALAPPDATA%\tako\WebView2` へ（移行 `webview2_data`）・生成の失敗を無い / 古い / その他に分けて案内・check-health に版）
- PR #2464

## 2026-10-28（#2458: ipc の残骸のテストが全体実行で Live と読む = drop した受け口の fd を並走テストの子が握っていた → listen しないソケットで作る）
- 修正前は 19,998 回中 6 回通る → 0 回・注入で修正前 5/5 FAILED → 修正後 5/5 ok・カタログ +0 B。PR #2467

## 2026-10-28（#2454: Windows のペイン・言語サーバの子孫を Job Object で回収・ペインを閉じたときは GUI のアプリとその子孫を残す・境界 B30 `platform::job`）
- Windows CI の実プロセスで孫の停止と GUI の子孫の残りを確認・カタログ +0 B。実機と psmux は未（Refs）。PR #2461

## 2026-10-28（#2466: remote stop が起動 → 記録 5 秒超の本物の daemon を「pid 再利用」と断り state を消していた → 起動時刻を片側照合・断るときは state を残す）
- 隔離の実 daemon で修正前は取り残す → 修正後は停止・注入で 4 本が落ちる・カタログ +0 B。PR #2468

## 2026-10-28（#2424: 時間の番犬を束縛した関数の中だけで数える・#2413 の復元の時差のテストを gate の止めた時計で測る）
- 注入で修正前は番犬が補助関数を 3 か所名指し・時差テストは 5/5 FAILED → 修正後 ok・カタログ +0 B。PR #2469

## 2026-10-28（#2447: 再起動をまたぐ復元とカタログの再開にも spawn と同じ MCP の断片 = `worker_mcp_args` / `prepare_for_resume` の 1 実装）
- 隔離 GUI の実経路で修正前は復元の断片 0 個 → 修正後は spawn と一致・カタログ +0 B。突然死の復旧案内は #2470。PR #2471

## 2026-10-28（#2462: Windows の probe の打ち切り・`tako queue` の取り消し・AI 命名の上限・session-restart の孫を #2454 の Job の境界で回収 `KillScope`）
- Windows CI の実プロセスで孫が止まり（旧アームでは残る）、普通に終わった子の常駐は残ることを確認・カタログ +0 B。実機は未（Refs）。PR #2472

## 2026-10-29（#2473: 復元・sessions resume・会話を保った再起動の worker を spawn と同じ `build_worker_cmd_in` の 1 実装へ。落ちていた --permission-mode auto・--remote-control 等を戻した）
- 隔離 GUI + 偽 claude / codex で修正前 FAIL 6 → 修正後 PASS 26・番犬の注入 10 通り・カタログ +0 B。PR #2476

## 2026-10-28（#2446: 隔離セルフテストの項目 88 が本物の `tako setup` を完走させ項目 111 が落ちていた → `self_test::withdraw_launch` で取り下げ・番犬 `selftest_launch_withdraw_watchdog`）
- 修正前 `statusbar=Some(3)` → 修正後 `Some(100)`・カタログ +0 B。97(e) の本物の master の起動は残り。PR #2475

## 2026-10-28（#2415: dispatch.rs の実 PTY テスト 3 本の fixture を `$SHELL -l` から `/bin/sh -c "exec sleep 600"` へ・番犬が素の `SpawnOptions::default()` を名指し）
- 修正前 20/20 FAILED（ssh 探索の 5 秒に rc が alt screen へ入る形）→ 修正後 60/60 ok・カタログ +0 B。PR #2474

## 2026-10-29（#2479: test_write_isolation の `shell_probe` を尽きたら落ちる状態待ち `wait_for_shell` へ寄せ、実行は出力にだけ出るマーカーで判定・番犬 issue1308 を広げた）
- 修正前は毎回プロンプトの折り返しで 20 秒の窓を使い切って無言で通っていた → 修正後 0.4 秒・`stuck` / `deaf` は `TAKO_2479_WAIT` で落ちる・30 回 × 並列 1 / 4 全通過・カタログ +0 B。PR #2481

## 2026-10-29（#2477: 子の走査が 1 瞬間だけ居た子を拾うと 60 秒 busy が残り項目 694(e) が間欠で落ちる → busy の集合が変わった回を次の tick で確かめ直す `settled_against`・A/B `TAKO_2477_LEGACY=1`）
- 旧 + 注入で 694(e) が 14.5 秒で落ち、新 + 注入は 1.6 秒で通る・単体 30 回 × 並列 1 / 4 全通過・カタログ +0 B。PR #2483

## 2026-10-29（#2470: 突然死の復旧案内を `tako orchestrator resume <worker_id>` にし、spawn と同じ組み立て（`--settings`・許可モード・env・role）で戻す）
- 偽 claude の実経路で修正前は 4 つとも落ちる → 修正後は全部載る・無人の復旧は隣の CLI を絶対パスで打つ・カタログ +0 B。main の `supervisor_loop` が呼び手ゼロなのが判明（#665 にコメント）。PR #2482

## 2026-10-29（#885: tako-app のペイン spawn と器の名前の予約を backend の入口 `wrap_spawn_for_pane` / `reserve_for_pane` 経由へ・組み立てを `wrap_options_for` の 1 実装にして psmux も通す）
- 修正前後で器のペインの argv 6 種がバイト一致・不正な器の名前は器なしで起こして persist.log へ・カタログ +0 B。psmux の conf の差は #2484・Windows 実機は未（Refs）。PR #2485

## 2026-10-29（#2365 の続き: action で選ぶツールの断り（不明な action・値・要る引数）と設定の保存・置き場・`tako_theme` の残りを `Reason` へ。87 か所・16 種、dispatch の日本語の断り 243 → 157）
- 実経路で修正前 FAIL 68 / 28 → 修正後 0・日本語 68 + 77 本一致・注入 5 通りで名指し・カタログ +0 B。PR #2487

## 2026-10-30（#2388: セルフテストの FAILED の出口も `on_app_quit` と同じ後片付け `cleanup_process_scoped` を上限 5 秒で通す・設定置き場を器より先に消す・番犬 `selftest_exit_cleanup_watchdog`）
- 修正前の main で器・ソケット・設定置き場が残る → 修正後は残らない（実経路の A/B）・固まっても理由と rc=1 は保つ・カタログ +0 B。visual-test の出口は #2491。PR #2488

## 2026-10-30（#2486: layout.json で表のペインどうしが同じ器の名前を持つと 2 本目の `-D` が 1 本目を切り離して閉じていた → 先のペインが器を持ち 2 本目以降は新しい器で起こす `restore_vessels`・FR-5.22）
- 隔離 GUI で修正前 3→2 / 3→1 ペイン → 修正後 PASS 39・番犬の注入で main.rs を名指し・persist.log に 1 行・カタログ +0 B。PR #2489

## 2026-10-30（#2365 の続き: MCP の前段 33 か所と CLI の前段 27 か所の断りを `Reason`（+48 種）へ）
- 実経路 50 ケースで英語表示の日本語入り 50 → 0・日本語はバイト一致・カタログ +0 B。残りは dispatch の個別の文 約 157 か所ほか（Issue の最後のコメントの表）。PR #2492
