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

## 2026-10-30（#2490: 隔離セルフテストの項目 107 が #1979 の上限の時計を巻き戻さず間欠的に落ちる → 両方の時計を `open_redraw_windows` で空けてから流す・予約が流れる判定を追加・番犬 `selftest_redraw_window_watchdog`）
- 注入 clock / piggyback で旧は FAILED（osc req=0）→ 新は通る・カタログ +0 B。PR #2493

## 2026-10-30（#1704: wait-pr-checks.sh / merge-pr.sh の gh の接続エラーを `lib/gh-retry.sh` で 1, 2, 4, 8 秒あけて 5 回まで試し、届かないままなら到達性の終了コード 5・認証切れ等は従来どおり 3）
- モックテスト 修正前 FAIL 50 → 修正後 PASS 273・カタログ +0 B。PR #2495

## 2026-10-30（#2498: CI 待ちの既定の上限の記述（commands.md・`DEFAULT_TIMEOUT` のコメント）を 6600 秒・macOS の job 90 分へ・`release.sh --promote` の docs の PR で merge-pr.sh の 5 を「回線が戻ってから」と出し分け）
- 昇格のモック Test 4c / 4d を追加（修正前は 3 件 FAIL → 修正後 142 pass）・カタログ +0 B。PR #2499

## 2026-10-30（#2365 の続き: GUI へ繋ぐ口（ipc.rs・時間切れ・起動失敗の包み）と置き場（ゲート・チェックポイント・人がやること・TaskStore・YAML の理由）の断りを日英へ（Reason 155 → 179））
- A/B: CLI / MCP 13 件・dispatch 30 件の英語表示の日本語入り → 0・日本語バイト一致・カタログ +0 B。PR #2497

## 2026-10-30（#2496: 項目 108 / 110 の `measure_output_redraw` も `open_redraw_windows` で両方の時計を空ける。いまの並びでは対象はフォーカス中で落ちない = 潜在）
- `unfocus` 注入で修正前は 108 / 110 とも FAILED → 修正後は通る・注入なし 4/4・番犬が `main.rs:<行>` を名指し・カタログ +0 B。PR #2500

## 2026-10-30（#2501: CI 待ちの既定を書いた md・コメント・--help の数字を `DEFAULT_TIMEOUT` と ci.yml の job の上限に突き合わせる番犬 `issue2501_wait_timeout_docs_watchdog`。docs だけの PR でも ubuntu で走る）
- 実ファイルへの注入 3 通りでファイルと行を名指しして落ち、戻すと緑・カタログ +0 B。PR #2502

## 2026-10-30（#2365 の続き: config_io の書き出し 6 文 → `WriteError`・remote.rs の AppIpcClient 9 行 → `Reason`・データディレクトリの断り 5 か所 → `DataDirUnresolved` / `IoCause`）
- 実経路 A/B で英語表示の日本語入り 置き場 5 → 0・remote daemon 4 → 0、日本語はバイト一致・カタログ +0 B。PR #2504

## 2026-10-30（#1614: cargo test 中の bash の syntax error は shell_dialect の意図的な負の対照（`bash -n`）だった → stderr を取り込んで `syntax error` を assert）
- 単体の stderr 2 行 → 0 行・注入 3 通りで FAILED・カタログ +0 B。PR #2503

## 2026-10-30（#1622: 環境診断の外部の問い合わせ（`claude mcp list` ほか）を導入状況と重ね、check-health 9.7〜12.0 → 5.8〜6.4 秒・setup --check 9.0〜9.9 → 5.4〜5.9 秒。出力は sha256 まで一致）
- 番犬は diagnostics.rs の待ち合いの単体テスト（直列へ戻す注入 4 通りで file:line を名指し）・実経路 22 / 22・カタログ +0 B。PR #2505
