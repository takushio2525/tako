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

## 2026-11-02（#2646: orchestrator 系の dispatch（spawn の記録・台帳・状態照会の準備）が UI スレッドで YAML の読み書きと画面の採取を待っていた → 台帳は OffloadJob・状態照会は写し（UI）と解決 + 採取（background）・spawn は配置 / 記録 / 積み込みの 3 段（積むのは記録の後）。A/B `TAKO_2646_LEGACY=1`）
- 段 1: spawn 52ms のうち 50ms が配置の後の記録と I/O・台帳 14 / 27ms が YAML。修正後は注入 1.5 秒の間も `tako list` 15〜35ms・専有 0 件（A/B 1.4〜1.8 秒）・応答と記録は一致・カタログ +0 B。PR #2649

## 2026-11-02（#2365 の続き: 引き継ぎの後任の起動とスマホの「+ master」（`master_launch::plan`）を表示言語へ。後任の理由は `ProfileFallback::Successor`・起動フォルダの注意は `successor_cwd_warning_in`・`plan` は型の口 + `text_in(lang)`・返答の配送の枠は `Reason::UserTaskMasterLaunchFailed`。persist.log は日本語）
- GUI 無しのホストで英語表示の日本語入り 18/24 → 0/24・日本語 24/24 一致・persist.log 14/14 一致、実 daemon の HTTP で 6/6 → 0/6・日本語 6/6 一致・注入で名指し + E0004・カタログ +0 B。PR #2653
- 09:36 に `6613e2e` を install・GUI 再起動。本番の perf.log で spawn / 台帳 / 状態照会の専有が install 後 0 件の本番確認で #2646 を閉じた

## 2026-11-02（#2647: 2 秒 tick の 5 段（metrics / queued / handoff / limit / net）が UI スレッドでペインごとに端末をロックして画面を組み解析していた（24 本で p50 15.6ms・ロック 152 回 / tick）→ UI は `PaneAccess` を集めるだけ・背景で 1 ペイン 1 ロック + 解析 + 閾値の設定の読み直し、判定は同じ関数で UI（A/B `TAKO_2647_LEGACY=1`））
- 24 本で p50 209µs・UI のロック 0。注入 30ms で `tako list` p95 1,567 → 22ms。修正前と修正後で 4 事象の回数・文面・時機（差 ≤0.5s）が一致。番犬 `issue2647_periodic_prep_offload_watchdog`・カタログ +0 B。PR #2654

## 2026-11-02（#2365 の続き: remote daemon の HTTP API の本文の `error`（PWA と管理 API）を表示言語へ。handler 群と認証の層・本文の JSON・tmux / IPC の往復・アップロード・WebSocket の固定文を `RemoteRefusal` の 50 種 + `AppVia` + `PeerReject::describe_in` へ。英語表示の GUI で最後のペインの close が 502 で IPC を捨てていたのを `says_last_pane` で 409 に）
- 実 daemon への HTTP 51 ケースで英語表示の日本語入り 49 → 0・日本語 51/51 一致・ステータスの差は close の英語表示 502 → 409 だけ・注入 8 通りで名指し + E0004・カタログ +0 B。PR #2655
- 10:40 に `62dd0c8` を install・GUI 再起動（画面ロック中・復元失敗 0・remote daemon は replaced-generation）。11:03 に #2618 を閉じた（ペインログ側の専有は 11/01 18:45 の install 以後 0 件・残る `dispatch:Close` の専有の真因は worker レジストリへの close の記録 → #2657）

## 2026-11-02（#2365 の続き: remote daemon の他の受け口 4 つ（`remote_files` / `remote_tasks` / `remote_cards` / `remote_ssh`）の本文の `error` と注記（`can_ssh.note` / `poll`）を表示言語へ。`RemoteRefusal` に 35 種 + `AppVia` 2 種、前から日英の対を持っていた `Denial` / `ShortcutError` は `FileDenied` / `ShortcutRejected` が引き、本文の形は `kind_body` の 1 実装）
- 実 daemon への HTTP 56 ケース（GUI 無し 42 + 隔離 GUI 14）で英語表示の日本語入り 53 → 0・日本語 56/56 一致・ステータスの差 0・注入 9 通りで名指し + E0004・カタログ +0 B。PR #2658

## 2026-11-02（#2652: 夜間の footprint の 11/02 の起動 10.9 秒は 29 コミットの回帰ではなく計測の外れ値 → 起動の区間の混み具合を `startup_busy` / `startup_load1` に記録し、重なった夜は空くのを待って複製したバイナリで 1 回だけ測り直し、なお重なれば起動時間を比べず基準から外す・A/B `TAKO_2652_LEGACY=1`）
- PR #2656

## 2026-11-02（#2657: `dispatch:Close` が worker レジストリへの close の記録（YAML + fsync + 上限なしの flock）をメインスレッドで書いていた → UI の段 `close_place` と記録の段 `CloseRecord` に分け、IPC は background で書き終わってから応答・GUI の close は背景のスレッド・「セッション kill」の行は kill のスレッド・A/B `TAKO_2657_LEGACY=1`）
- 注入 1.5 秒で close の間の `tako list` 1516〜1540ms → 20〜29ms・`dispatch:Close` の専有 5 → 0 件・ロック 1.5 秒の間も 20〜25ms・応答と記録は A/B と一致（85/85）・カタログ +0 B。PR #2660
- 14:02 に install。以後の本番 perf.log の `専有: dispatch:Close` を数えて閉じる（#2657 は開けてある）

## 2026-11-02（#2365 の続き: 既存ペインの SSH 化の断り `PaneSshBlock` を表示言語へ（`text_in` の網羅 match・`message` は GUI の表示言語）。`list` / MCP / `/api/v2/panes` の `can_ssh.note` と open-in remote / `tako_open_remote` / PWA の 409 の本文。daemon の 409 の読みは `PaneSshBlock::says_blocked`（目印 `target=split`）の 1 実装へ）
- 隔離 GUI + 隔離 daemon の 29 ケースで英語表示の日本語入り 26 → 0・日本語は出力全体がバイト一致・ステータスの差 0（版ずれ 2 通りも 409）・注入 5 通りで名指し + E0004・カタログ +0 B。PR #2661

## 2026-11-02（#2365 の続き: 会話の再起動の断り `RestartBlock`（12 種 + mode の分岐）・建て直しの失敗 `RelaunchFailure`（5 種）・リモートフォルダの自動接続の注記 `AutoTerminalSkip`（2 種）を表示言語へ。`text_in` の網羅 match・`message` / `note` は GUI の表示言語・「見つからない」は英語にも ` not found`）
- 隔離 GUI の 89 ケース（偽 claude で建て直しの失敗・偽 ssh / sftp で remote-folder まで）で英語表示の日本語入り 89 → 0・日本語は出力全体がバイト一致・終了コード / isError / reason の差 0・注入で名指し + E0004 × 3・カタログ +0 B。PR #2667

## 2026-11-02（#2663: tako mod の報告・status・on / off・帯の切り替えを 準備（UI）→ 続き（background = 画面の採取・ui.json）→ 着地（UI = 置き場の写し・番号の守り）へ分けた。A/B `TAKO_2663_LEGACY=1`）
- 注入 50ms で perf.log の `dispatch:Mod` / `offload_prepare:Mod` の専有 修正前 9〜12 件 → 0 件・`tako lang` の最長 54〜133 → 14〜22ms・応答 / hub / ui.json は A/B と一致・カタログ +0 B。PR #2668
- 17:46 に install。以後の本番 perf.log の `専有: dispatch:Mod` / `offload_prepare:Mod` を数えて閉じる（#2663 は開けてある）

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
