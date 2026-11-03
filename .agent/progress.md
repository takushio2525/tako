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

## 2026-11-03（#2045: 横断検索に置換。プレビュー → 1 行 / ファイル / 全部 → 取り消し。core の 1 実装・既定はプレビュー・検索後に変わったファイルは stale・開いているバッファは undo 1 回・開いていないファイルは退避してから書く）
- e2e PASS=23（stale・退避から戻す・CLI と MCP の一致・visual-test）・カタログ +467 B。PR #2683

## 2026-11-03（#2365 の続き: CLI の人向けの表示のうち orchestrator projects（list / add / remove / mcp）・guide の一覧・sessions resume の行・todo show の枠を `ReplyNote` の 25 種で表示言語へ。字下げは呼び手・`--json` は不変）
- 実経路の CLI 22 本 × 日英 × 修正前後で英語表示の日本語入り 16 → 0・日本語は全出力がバイト一致・終了コードと JSON の差 0・注入で名指し + E0004・カタログ +0 B。PR #2691

## 2026-11-03（#2685: `tako usage` の CLI が読むだけの経路で空の `tako-agent-config-<pid>` を残していた → `verification_agent_home()` を作らない読む口にし、置き場が先に要る書き手（シェル履歴）だけ作る口 `ensure_verification_agent_home()` を通す。番犬 `issue2685_agent_home_read_only_watchdog`）
- 隔離 CLI 6 回で空の置き場 修正前 6 → 修正後 0・事前信頼 / シェル履歴は従来どおり作る・注入 3 通りを file:line で名指し・カタログ +0 B。PR #2692

## 2026-11-03（#2690: test-wait-pr-checks.sh の t39merge の間欠（CI の bash は 3.2）は手元で再現せず真因は未確定 → assert が落ちたら腕の出力・終了コード・偽 gh の呼ばれ方を CI のログへ出す手掛かりと確認用の注入 `TAKO_2690_INJECT` を入れた）
- bash 3.2 / 5.3 とも PASS=273 FAIL=0（通った回は何も出さない）・注入 2 通りで CI の 1 回目と同じ 2 件が落ち、手掛かりで見分けられる。#2690 は再発待ちで開けてある。PR #2693

## 2026-11-03（#2491: visual-test の成功の出口 91 か所と知らない節の `exit(1)` が `on_app_quit` を通らず、設定置き場が（`TAKO_PERSIST=1` なら器とソケットも）残っていた → `self_test::visual_exit` で `cleanup_process_scoped` を通す・番犬の規則 5・A/B `TAKO_2491_LEGACY=1`）
- e2e 30/30（器ありの成功で残らず、旧は残る・失敗の出口は不変）・#2051 の e2e 14/14・カタログ +0 B。PR #2687
- 09:40 に `4c59881`（PR #2687 まで）を install・GUI 再起動

## 2026-11-03（#2365 の続き: CLI の人向けの表示のうち todo list / expand / collapse と sessions list / link の枠を `ReplyNote` の 13 種で表示言語へ。`sessions link` の表示は `print_sessions_link` へ切り出し・`--json` は不変）
- 実経路の CLI 25 本 × 日英 × 修正前後で英語表示の日本語入り 20 → 0 行・日本語は全出力がバイト一致・終了コードと JSON の差 0・注入で名指し + E0004・カタログ +0 B。PR #2696

## 2026-11-03（#2689: セルフテスト項目 41c-2 の ① ③ ④ が「Tab → 固定 800ms → Enter」の後、Tab の前からゴーストとして出ている文字列で判定していた（遅れた回は落ち、④ は偽 PASS）→ echo の出力の行を状態で待つ `tab_enter_41c`・注入 `TAKO_2689_INJECT=late|notab|notab4`・番犬 `fixed_wait_before_enter`）
- tako-vd の隔離 GUI で注入 late は旧 FAILED → 新は成功の出口まで・notab / notab4 は新で FAILED（旧の ④ は偽 PASS）・番犬は旧の 3 か所を file:line で名指す・カタログ +0 B。PR #2695

## 2026-11-03（#2694: `TAKO_ISOLATED=1` で `TAKO_DATA_DIR` を付けない CLI の手元の処理が本番の data dir を読み書きしていた → `paths::data_dir` / `local_data_dir` の 1 実装に門（`data_dir_refusal`）・CLI の手元のサブコマンド 34 本の入口で日英の断り（`local_data_gate`）・例外は `tako queue`。番犬 `issue2694_isolated_data_dir_watchdog`・A/B `TAKO_2694_LEGACY=1`）
- 砂場の HOME で修正前 16/16 が動き 2 種が既定の置き場へ書く → 修正後 16/16 断る・隔離 GUI のペインの CLI とセルフテストは通る・`verify-setup-multiagent.sh` に置き場を明示・カタログ +0 B。PR #2699

## 2026-11-03（#1996: `tako queue` に優先度 4 段（処理 > ペイン > 役割・待ち 15 分ごとに 1 段 high まで）・変更の口 `tako queue priority <受付番号 | --pane N> <段|default>` / MCP `tako_queue` の `priority`・終わりの見込み（端末でない cargo の N/M を wrapper が読んで素通しから外す + 過去の所要時間）・history.jsonl の `units` と移行の番地 `queue_history`（v1）。GUI のチップは #1995）
- e2e 32/32・A/B `TAKO_1996_LEGACY=1`・番犬 `issue1996_queue_priority_eta_watchdog`・カタログ +296 B。PR #2698

## 2026-11-03（#1902 の 2 回目: 回数の窓の既知リストのうち background 8 件を `wait_for_background_state` へ、画面の静止 4 件を新しい `wait_for_quiet_frame` へ寄せた。既知リスト 110 → 98）
- 隔離 GUI（tako-vd・debug）で 8 節が素 / 観測の遅れ（旧の窓 + 5 秒）で緑・旧の窓は 5 項目が名指しで FAILED・セルフテストは素 / 遅れで OK・旧の窓は 3 項目とも名指しで FAILED・カタログ +0 B。PR #2703

## 2026-11-03（#2697: Windows の取り消し・打ち切りの孫が Job の外に残る「起動直後の窓」を閉じた（境界 B30 の `Job::spawn_contained` / `KillScope::spawn` = `CREATE_SUSPENDED` で起こし Job へ入れてから `NtResumeProcess`。queue / probe / AI 命名 / 言語サーバ / `tako setup`））
- A/B `TAKO_2697_LEGACY=1`・番犬 `issue2697_spawn_window_watchdog` + Windows CI の注入テスト・カタログ +0 B。PR #2701

## 2026-11-03（#2365 の続き: CLI の人向けの表示のうち `tako sleep-guard`・`tako recover` 一式・`tako orchestrator watch` の説明文を `ReplyNote` 52 種 + `Reason` 10 種と中身の型の日英（`LayoutError` / `SkipReason` / `Untrusted` / `LidSkipReason` / `LidControlDone`）で表示言語へ。watch の `WORKER_*`・ラベル・現在の選択の印は master が読むので不変）
- 実経路の CLI 23 本 × 日英 × 修正前後で英語表示の人向けの行の日本語入り 77 → 0 行・日本語は全 72 ファイルがバイト一致・終了コードと JSON の差 0・偽 osascript で sudoers に触れない・注入で名指し + E0004・カタログ +0 B。PR #2702

## 2026-11-03（#2686: 検証起動の GUI が env へ自分で当てた置き場（10 個・付属ファイルつき）と pid 入りの IPC ソケットが成功・FAILED・SIGTERM のどの出口でも残っていた → `scoped_dirs::adopt` で覚え `cleanup_process_scoped` が消す（明示 = 残す・本番は不変・消した後の書き込みは捨てる・A/B `TAKO_2686_LEGACY=1`））
- 実 GUI 8 腕 PASS 27 / FAIL 0・番犬の注入 5 通りを名指し・カタログ +0 B。PR #2705

## 2026-11-03（#1995: 右パネルの resources ビューとステータスバーの区画。見ている間は CLI と同じ status を `dispatch_off_ui`（`OffloadJob::Queue` → `queue::answer`）、見ていない間は覚えだけを背景で読む・行でフォーカス・取り消しの確かめ・段のチップ（#1996 の持ち越し）・status の行に誰の欄・最小幅 288px）
- tako-vd の隔離 GUI（明示の調停役 + 本物の wrapper 3 本）で GUI / MCP / CLI の段と取り消しが同じ答え・区画の 3 色が画素一致・perf.log の専有 0（2 回）・カタログ +121 B。PR #2706

## 2026-11-03（#1997: spawn / run の応答に `machine` / `advice`（閾値は起動待ちと順番待ちの同じ `admit` に仮の 1 件 = `queue::headroom::probe`）・worker ごとの常駐メモリ（workers / fleet / `tako panel` が同じ走査の覚え）・guide と base prompt に 1 文・能力マトリクス `worker_heavy_queue_wait`）
- 実経路 PASS=32（境界 4198 / 4134 MB で助言と 2 本目の答えが一致・fleet / CLI / MCP / panel の常駐が一致）・一括復帰の直後 8 本同時は空き 8 GB で 1 本ずつ = 時差は要らない・カタログ +0 B。PR #2709

## 2026-11-03（#2365 の続き: CLI の `tako queue`（状況・cancel / start / priority / limits）と `tako usage` の人向けの表示を `ReplyNote` 72 種 + `Reason` 1 種で表示言語へ。調停役が組む文と usage の注記は応答の値のまま、英語の表示は応答の構造の欄から組む（注記は `usage::usage_notes` の 1 実装）・`--json` は不変）
- 実経路の CLI 25 ケース × 日英 × 修正前後で英語表示の日本語入り 154 → 0 行・日本語は 24/25 がバイト一致（残る 1 は見本の時刻だけ）・終了コードと JSON の差 0・実の調停役でも 0 行・注入 11 通りで名指し + E0004・カタログ +0 B。PR #2708

## 2026-11-03（#1902 の 3 回目: 回数の窓の既知リストのうち PTY の 25 件を新しい `wait_for_pty_state`（2 値・判定は待ちの戻り値）へ寄せ、#720 の観測の窓 1 件は計測へ。既知リスト 98 → 73）
- tako-vd（debug）で visual-test 4 節が素 / 遅れで緑・旧の窓は名指しで FAILED / SKIPPED、セルフテストは素 / 遅れで OK・旧の窓は 16/16 が名指しで FAILED、全節実行のスターター 1 か所は手前の別の節で落ちて未検証・カタログ +0 B。PR #2710

## 2026-11-04（#1999: スマホ（PWA）の `#/` に機のカード、`#/resources` に実行中と待ち。段の変更と取り消しは manage 以上・daemon の宣言表 `RESOURCE_ROUTES` の 4 本が `Request::Queue` を素通し）
- 実経路（隔離 GUI = 明示の調停役 + wrapper 3 本 + 実 daemon + chromium）PASS=56 FAIL=0・e2e 124 passed・カタログ +0 B。PR #2712

## 2026-11-04（#1899 の残り: 補完の「読み込み中」が上限に当たったら黙って消さずに理由の 1 行・キー / メニューのホバーの読み込み中を語の真下に・切ったカードの脚注に `tako lsp hover --full`・右クリックメニューの割り込みを数える `vt1899_menu` を待ちのある節へ。A/B `TAKO_1899_LEGACY=1`）
- tako-vd の実経路 PASS=19（上限 2 秒で 2.19 秒に理由の 1 行・矩形の外の差分 0 px・旧腕は黙って消える / 割り込みの注入 1 回は数えて緑・9 回は名指し）・整形の実経路 PASS=27（実 RA）・カタログ +0 B。PR #2713
- 02:22 に `64b9fbf`（PR #2713 まで）を install・GUI 再起動（復元失敗 0・remote daemon は replaced-generation）

## 2026-11-04（#2704: セルフテストの項目 60 / 122 の固定名の一時 dir（`tako-osc-e2e`・`tako-st915-a/b/x`）がどの出口でも残り並列で取り合っていた → pid 入りの名前で作り、`remove_on_fail` の登録簿を `scoped_dirs::claim` で終了の後片付けへ載せた（A/B `TAKO_2704_LEGACY=1`））
- 実 GUI 8 腕 PASS 25 / FAIL 0・番犬の注入 5 通りを名指し・カタログ +0 B。PR #2707
