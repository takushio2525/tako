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

## 2026-11-02（#2365 の続き: orchestrator の設定の理由（作らなかった理由 `ProfileGen::Skipped` = `ConfigError` 2 種・adopt の断り `Reason` 2 種）と他モジュールが組む注記（adopt の警告・`profile_warnings`・`window_warning`・会話の再開と worker の MCP の `warnings` = `ReplyNote` 18 種）を型で返して表示言語へ。persist.log と移行の記録は日本語）
- GUI 無しのホスト + CLI の 27 ケース（spawn / adopt / projects add / profiles / projects mcp / orchestrator resume / sessions resume）で英語表示の日本語入り 27 → 0・日本語は全出力と persist.log がバイト一致・終了コード / isError の差 0・注入で名指し + E0004 × 3・カタログ +0 B。PR #2670

## 2026-11-02（#2665: UI ストールの分類の窓を「期限 → 記録」へ。期限より前に終わった区間を専有と名指さない・半分未満は内訳と残り・型 `StallCause` + 日英 `text_in`・A/B `TAKO_2665_LEGACY=1`・注入 `TAKO_2665_INJECT_FILE`・番犬 `issue2665_stall_window_watchdog`）
- 隔離 GUI で ① 中 300 → 外 900 の記録 27 件で専有 0（A/B 32 件で 10）・② 1500ms 6/6 専有・③ 3000ms 中間報告 5/5・④ 内訳 10/10（A/B は外だけ 10/10）・カタログ +0 B。PR #2669

## 2026-11-02（#2659: Windows の CI の `shell_integration_powershell` の間欠は同じプロセスのテストが 1 つの `tako.ps1` を書き直す共有違反（PSReadline は主因でない）→ テストごとの置き場・製品は共有違反だけ 0.5 秒まで読み直し読めなければ警告・同じ中身は書き直さない（A/B `TAKO_2659_LEGACY=1`））
- Windows の CI で `shell_integration_powershell` 22 passed（注入 4 本が実シェルで通る）・修正前の 2 ファイルへ戻すと番犬 `issue2659_pwsh_script_share_watchdog` 5/5 FAILED・カタログ +0 B。PR #2671

## 2026-11-02（#2666 段 1: 画面構成の変更（macOS の画面再構成）の begin / end を境界 B33 `platform::display_reconfig` で記録し、UI ストールの分類が遅延の間に窓「begin → end + 2 秒」が重なれば「画面構成の変更の処理中（OS / GPUI 側）」と名指す・計測区間の専有でも併記・A/B `TAKO_2666_LEGACY=1`）
- 隔離 GUI の通知の合成で 24/24（A/B は「計測区間の外」）・番犬 `issue2666_display_reconfig_stall_watchdog`・カタログ +0 B。本番未確認（install 後に perf.log の再構成の行で）。PR #2672

## 2026-11-02（#2365 の続き: orchestrator の応答の warnings の残り 3 つ = 引き継ぎの自動移行（`ReplyNote` 4 種 + 退避の `ConfigError` 3 種）・モデルの食い違い（`ReplyNote` 2 種。tako-core の日本語の口は外した）・spawn の事前信頼の失敗の `detail`（新しい `TrustWriteError` 12 種）を型で返して表示言語へ。persist.log・`tako migrate` の記録・stderr の診断は日本語）
- GUI 無しのホスト + CLI の 17 ケースで英語表示の日本語入り 17 → 0・日本語は全出力と persist.log がバイト一致・終了コード / isError の差 0・注入で名指し + E0004 × 3・カタログ +0 B。PR #2674

## 2026-11-02（#2537: test-lsp-menu-1684.sh の ① が 12 のうち 6 で落ちていた = #1685 / #1686 で `MenuItem::ALL` が増えたのに visual-test が偽サーバへ参照・階層・名前の変更の規則を渡していなかった（製品の出し分けは仕様どおり）→ 規則の env の組 `FAKE_FULL_MENU_RULE_ENVS` を節で渡し、`ALL` と偽サーバの申告のずれを CI の番犬で縛る）
- tako-vd でスクリプト 3 回とも PASS 25 / FAIL 0（修正前の ① は 4/4 NG）・注入 2 種で番犬が落ちる・カタログ +0 B。実の rust-analyzer の節 ④ は #2673 へ切り出した。PR #2675
- 23:31 に `b305af4` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）

## 2026-11-02（#2676: preview_changelog の開く切替のテストが実ファイルの履歴を `date_relative` ごと比べ分の境目で落ちていた → 過去の固定時刻のコミットを持つ使い捨てのリポジトリへ（`date_relative` は比較に残す・A/B `TAKO_2676_LEGACY=1`））
- 注入（`TAKO_GIT_BIN` の包みで git の「いま」を呼ぶたびに進める）で修正前 FAILED → 修正後 ok（20/20・残骸 0）。同じ型の残りは grep で 0 件・カタログ +0 B。PR #2678

## 2026-11-02（#2365 の続き: 引き継ぎの応答のほかの文 = 移行の 1 行サマリ（`ReplyNote::HandoffMigrated`）・運用メモの肥大（`ProfileMemoOversized`。前から英語だったので出力は不変・tako-core の文は外した）と `workers` の `resend_command`（`ResendCommand`。プレースホルダだけ `<同じ依頼文>` / `<same prompt>`）を型で返して表示言語へ）
- GUI 無しのホスト + CLI の 18 ケースで英語表示の日本語入り 4 → 0・日本語は全 56 出力と persist.log がバイト一致・終了コードの差 0・注入で名指し + E0004・カタログ +0 B。PR #2679

## 2026-11-03（#2673: rust-analyzer が読み込み直しの最中の要求に返す `-32801 ContentModified` を最終の失敗にしていた → 定義・参照・ホバー・名前の変更・整形は済むのを待って問い直し、補完は未完の 0 件。A/B `TAKO_2673_LEGACY=1`・番犬 `issue2673_content_modified_watchdog`）
- 実の rust-analyzer 1.95.0 で test-lsp-menu-1684.sh 3/3 PASS 27 / FAIL 0（④ は 0.4〜0.7 秒で着地・旧腕は 90 秒着地せず）・偽サーバの e2e 11 件で旧腕の server-error → 修正後は着地・カタログ +0 B。PR #2680

## 2026-11-03（#2425: 起動待ちのペインに理由・順番・「今すぐ起こす」（= `tako queue start`）の帯を出し、待っている間と起動コマンドを打ち終えるまで人の打鍵・貼り付け・IME の確定をペインへ流さない（回数を帯に出す）・`tako list` の `launch_wait_band`・A/B `TAKO_2425_LEGACY=1`）
- tako-vd の visual-test で新: 入力 5 回を止めて起動コマンド 1 回・混ざり 0 / 旧: `xyzPASTE2425あecho …` で起動失敗。実 spawn + CLI / MCP も 19/19・カタログ +0 B。PR #2681

## 2026-11-03（#2365 の続き: 返答の配送の理由 4 つ（`Reason::UserTask*`。記録へ書いた時点の表示言語）と移行の記録の理由（`FileOutcome` の理由を日英の対 `OutcomeReason` に。`PlanError` / `FileFailure` / `MigrateReason` / `SetupRelocateError` で組む）を表示言語へ。persist.log と `tako setup` の行は日本語）
- GUI 無しのホスト + CLI / MCP + 実 daemon の HTTP の 16 回で英語表示の日本語入り 10 → 0・日本語は全 54 出力と persist.log がバイト一致・終了コード / ステータスの差 0・注入で名指し + E0004・カタログ +0 B。PR #2682

## 2026-11-03（#2684: セルフテスト項目 56 が #2657 の非同期の close を同じ update で確かめて必ず落ちていた（製品の回帰ではない）→ 閉じるのを `wait_for_app_state` で待ち、check を 2 つに割る・注入 `TAKO_2684_INJECT=noclose`・番犬 `selftest_offui_wait_watchdog`）
- tako-vd の隔離 GUI でセルフテストが成功の出口まで（修正前は 4/4 FAILED）・注入 noclose は上限まで待って FAILED・番犬は修正前の形を file:line で名指す・カタログ +0 B。PR #2688

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
