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

## 2026-10-25（#2284 の再発: 真因はテスト = 看取った `Child` を drop してから読み、Windows が再利用した pid の別プロセスを `Alive` と読んだ → `process::ReapedChild` を持ったまま読む）
- 注入 `TAKO_2284_INJECT_REUSE` で修正前は CI と同じ `lid.rs:1071` で FAILED → 修正後 ok（macOS / Windows）・カタログ +0 B。PR #2385

## 2026-10-25（#2387: issue1977 の LSP の古い版の片付けのテストの `dead_pid()` を `reaped_child()` へ。test_data_residue の `reaped_pid` は対象外として conventions に例外を記録）
- 注入で修正前 `:548` FAILED → 修正後 ok・並列 24/24 × 2・カタログ +0 B。PR #2389

## 2026-10-25（#2350: 端末の問い合わせ（OSC 10/11/12/4・CSI ? 996 n・CSI 14 t）に答え、mode 2031 を購読した器（tmux）へテーマの切り替えを知らせる・OSC 52 の読み出しは断る・check-health の `terminal_queries`）
- 修正前は色と 14 t が無応答（器越しは OSC 4 の後 5.5 秒止まる）→ 修正後は両方で答えテーマにも追従・カタログ +63 B。PR #2382

## 2026-10-25（#2365 の 3 本目: 保存の断りに添える案内を `Reason` へ移し、プレビュー・編集・ファイル操作・ペイン / タブ操作の断り 34 種を足した（51 → 85 種））
- 実経路 修正前 PASS=157 FAIL=44 → 修正後 PASS=201・日本語表示 63 本バイト一致・注入 4 通り・カタログ +0 B。PR #2390

## 2026-10-25（#2393: 実 tmux のテストが `kill-server` の後にソケットファイルを残していた件を、失敗の経路も含めて `remove_socket_file` / `kill_server` する形へ。番犬 `テストが落とした器はソケットファイルまで消す`）
- 修正前 1 回 5 個 → 修正後 0（失敗の経路の注入も 0）・番犬は main の版で 10 か所を名指し・カタログ +0 B。PR #2396

## 2026-10-25（#2391: 走らせた人の env（TAKO_ISOLATED / TAKO_SELF_TEST / TAKO_DATA_DIR）で結果が変わるテスト 6 本を、env を組んだ子プロセスで見る形へ）
- tako-core 全テスト × env 5 通りの修正前の赤 → 修正後は全部緑・tako-control + tako-cli の 4 failed → 0・注入 3 通り・カタログ +0 B。PR #2397

## 2026-10-25（#2365 の続き: `tako mod ui` の断り（UiError 一式）を日英の網羅 match へ。setup / persist.log / migrate は日本語固定）
- A/B 25 件で日本語はバイト一致・英語表示の日本語 25 → 0・注入 5 通りで file:line を名指し・カタログ +0 B。PR #2394

## 2026-10-25（#2395: usage の索引のテストが A/B の env をプロセス全体に立て、並走する codex の区間按分のテストまで旧挙動にしていた → `walk_with(s5_legacy)` へ分け、テストは env を立てない）
- 順序を固定する注入で修正前は CI と同じ assert で FAILED → 修正後 ok・usage 50 回緑・カタログ +0 B。時限の 2 本は #2402。PR #2398

## 2026-10-25（#1899 の 1 点目: 全文を 1 つの TextEdit で返す整形を行ごとの置き換えへ割る = `text_diff::changed_regions` + `text_edit::line_split`。割って undo が軽くなるときだけ割る）
- 10 万行で undo 17.4 MB・深さ 1 → 2.6 MB・深さ 2・本文はバイト一致・A/B `TAKO_1899_LEGACY=1`・カタログ +0 B。CLI の実経路は未実測。PR #2401

## 2026-10-25（#2365 の続き: ファイルの移動・コピーの断りを理由ごと（`MoveRefusal` / `CopyRefusal` の `text_in` + dispatch の `Reason`）日英の網羅 match へ）
- 実経路 14 件で英語表示の日本語 14 → 0・日本語はバイト一致・注入 3 通りで file:line を名指し・カタログ +0 B。PR #2404

## 2026-10-25（#2402: usage::index の claude 系テストの固定日付が 2027-11 以降に月の掃除で消えて落ちる時限 → 実時間の 2 日前の正午からの相対へ・製品は無変更）
- `now_ms()` を 400 日ずらす注入で修正前 2 本 FAILED → 修正後は月末の境界も含め全通過・カタログ +0 B。PR #2405

## 2026-10-25（#2400: env を書く lib のテスト 62 本と結合テスト 5 本を `tako_core::run_alone!`（その 1 本だけの子）へ・env 専用ロック 5 つを撤去・番犬 `issue2400_test_env_write_watchdog`）
- 並走の注入で修正前 3/3 FAILED → 修正後 3/3 ok・番犬は注入で名指し・カタログ +0 B。PR #2409

## 2026-10-26（10/25 夜の PC 停止の調査: メモリ枯渇 → Jetsam → watchdog のカーネルパニック（3 回目）。主因は agent の常駐 = claude 1 本ごとに user scope の MCP 12〜15 プロセス）
- 記録は #1992 のコメント。ユーザー判断で #2412（worker は tako の MCP だけ）・#2413（逼迫中は起動を待たせる）を起票

## 2026-10-26（#1308 の再発: respond の実 PTY テストの fixture をログインシェル経由から `/bin/sh -c` の直接起動へ・失敗文の値を `TAKO_1308_WAIT` の 1 行へ）
- 注入 `stuck` / `deaf` で修正前は同じ行で FAILED → 修正後 ok・単独 20/20・カタログ +0 B。同じ形の残り 3 か所は #2415。PR #2410

## 2026-10-26（#2419: docs サイトの共用の consent.js を async で読み、最初の描画を止めない。製品は無変更）
- PR #2420

## 2026-10-26（#2399: 器を起こす検証スクリプト 32 本・47 か所の `kill-server` を `tmux_socket_kill`（`scripts/lib/tmux-socket.sh`）へ寄せ、ソケットファイルを残さない・番犬 `器を起こすスクリプトはソケットファイルまで消す`）
- 修正前 計 31 個 → 0・失敗経路の注入 22 本で 1〜4 → 0・番犬は修正前の版で 32 本を名指し・カタログ +0 B。PR #2408

## 2026-10-26（#2365 の続き: `tako mod ui` の成功の応答の枠・CLI の表示・`cli_request` の断り 9 種・排他の断り `config_io::LockError`・置き場が決まらない断りを日英の網羅 match へ）
- A/B 20 件で英語表示の日本語入り 20 → 0・日本語は 40 ファイルがバイト一致・注入 4 通り・カタログ +0 B。PR #2417

## 2026-10-26（#2412: claude の worker を tako の MCP だけで起動 = `--settings` の `deniedMcpServers` で user scope を止める・プロジェクト単位で足す `projects mcp`・projects.yaml v2 の移行で直近 30 日の実績を写す）
- 常駐 1,218 → 265 MB / 本・本番の GUI から起こした worker は 4 プロセス 695 MB・カタログ +321 B。codex は #2418。PR #2422

## 2026-10-26（#2413: メモリ逼迫中は agent の起動（spawn / run / 復元・sessions resume・git resolve）を `admit` と同じ純関数で待たせ、復元は master から 1 本ずつ時差・上書きは `tako queue start <ペイン>`）
- 単体で復元の間隔 150〜176 ms・立ち上がり中は最大 1 本・カタログ +394 B。10/27 の実 GUI の e2e は PASS 40 / FAIL 2（run_status の戻り・master が先）→ #2426。PR #2421

## 2026-10-26（#2365 の続き: `tako mod`（status / on / off / band / install / uninstall）の応答と CLI の表示を日英の網羅 match へ・置き場の断り `paths::NoDataDir`）
- A/B で英語表示の応答 14 → 0・CLI の表示 12 → 0・日本語はバイト一致・注入 4 通り・カタログ +0 B。PR #2423
