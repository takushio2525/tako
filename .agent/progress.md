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

## 2026-11-01（#2625: 器のペインを閉じるたびに器の tmux サーバー全体が約 600ms 止まっていた（tty の最後の close がサーバーの中で読まれない出力を抱えて待つ。tako のメインスレッドは止まっていない）→ 器のペインの端末は閉じるときに PTY のスレッドが attach クライアントへ SIGHUP を送り、子の側が全部閉じるまで親側を読み捨ててから閉じる・A/B `TAKO_2625_LEGACY=1`）
- 閉じた後の外からの `tmux ls` 593〜607ms → 6〜10ms・器が消えるまで 633〜654ms → 24〜50ms・fd / 子 / ゾンビ・終了と復元・直接ペインは同じ・番犬 4 件・カタログ +0 B。PR #2629
- 18:45 に `70faa47` を install・GUI 再起動（画面ロック中・remote daemon は replaced-generation）。本番の `tmux -L tako ls` が閉じた後 598ms → 49ms の本番確認で #2625 を閉じた

## 2026-11-01（#2624: 段 1 で setup（CLI / MCP）は serve 以外に tailscale を書き換えないと確認（up / login / set / funnel なし）→ 門の外の 3 経路（stop の後始末・対話 setup の brew 導入・QR のビューア）と Windows の本物の置き場を #2619 の門 `allows_op` へ通す（A/B `TAKO_2624_LEGACY=1`・③ の自動復帰は画面ロック中に確かめられず見送り））
- 偽の tailscale / brew / open の記録で修正前 6 / 1 / 1 回 → 修正後 0 / 0 / 0 回、opt-in・A/B・本番の経路は従来どおり、両ノードの serve status は前後一致・番犬が注入 9 通りを file:line で名指し・カタログ +0 B。PR #2632

## 2026-11-01（#1721: 狭いペインで claude が背景作業の申告を折り返し、mod なしの worker が `busy / agents-auto / idle_override_blocked: null` のままだった → 折り返しをつないで読み、入力欄の直前の申告に限る・読めず状態行が切れていれば `status_line_truncated`・A/B `TAKO_1721_LEGACY=1`）
- 実 GUI の 36 / 24 桁で修正前 busy → 修正後 idle（生成中は busy のまま）・mod の経路は修正前から idle・番犬が注入 4 通りを file:line で名指し・カタログ +0 B。PR #2635

## 2026-11-01（#2634: 器なしの `tako tmux open` のペインを閉じるたびに利用者の tmux サーバーが約 600ms 止まっていた → #2625 の印を器なしのビューにも立てる（`tmux_backend::hang_up_view_client_on_close`・A/B `TAKO_2634_LEGACY=1`）。GUI の終了は OS が親側を先に閉じるので止まらず変えない）
- 隔離 GUI で修正前 592〜606ms → 修正後 6ms（5/5）・legacy 592〜607ms・終了は器 3 枚で 6〜9ms・器は残り再起動で戻る・番犬が注入 6 通りを file:line で名指し・カタログ +0 B。PR #2636

## 2026-11-01（#2630: MCP `tako_remote_stop` / `tako_remote_status` と設定画面の停止・リモートタブの読み直しを `prepare_offload` → `OffloadJob::RemoteStop` / `RemoteStatus` で background へ（#1555 と同じ形・A/B `TAKO_2630_LEGACY=1`））
- 注入 3 秒で `tako list` の待ち 2.7 / 5.6 / 5.6 秒 → 17 / 17 / 22ms・perf.log の専有 0・応答は A/B と一致・カタログ +0 B。PR #2637

## 2026-11-01（#2633 (c): ログインシェルの問い合わせの打ち切りがグループへいきなり SIGKILL を送り、rc の `pyenv rehash` のロックを残していた → グループへ SIGTERM 1 回 → 猶予 1 秒 → 残りへ SIGKILL（刈り取り係・頭は判定まで刈らない）。A/B `TAKO_2633_LEGACY=1`・番犬 `issue2633_probe_group_term_watchdog`）
- 実経路で修正前 5/5 → 修正後 0/5、連鎖（後続の問い合わせの打ち切り）11/11 → 5/11。呼び手が返る時刻は同じ・カタログ +0 B。(a) は直さない（master が [提案] Issue へ）。PR #2642

## 2026-11-01（#1675: `tako context-budget fix` がアーカイブ済みのまま作業ログへ戻ったエントリを、#1645 の `revivals` の判定を共有して予算計算の前に落とし 1 回で収束するようにした。同じ見出しで本文の違うものは触らない・check は直るものを `fixable` / `revived_fixable` へ数えて `next_step` に fix）
- 修正前は予算内の二重で fix が「変更なし」のまま → 修正後 1 回目で 4→3 件・2 回目はバイト一致・二重でない 3 素材は修正前と出力・書き込みが一致（check --json に欄 1 つ）・カタログ +0 B。PR #2641

## 2026-11-01（#2639: `tako_orchestrator_respond` の 400ms の正体は `respond_via` の固定の sleep → 届き方の解決（`respond_access`）だけ UI スレッドで行い `OffloadJob::OrchestratorRespond` で background から応答（錠で 1 本ずつ・チャット表示の承認カードも同じ口・A/B `TAKO_2639_LEGACY=1`））
- 模擬 TUI で応答の間の `tako list` 308〜326 / 696〜766ms → 最長 17 / 20ms・perf.log の専有 0・応答と監査記録は A/B と一致・カタログ +0 B。PR #2643

## 2026-11-02（#2644: issue2576 の番犬が全体並列の負荷で落ちる間欠 → 作りたての偽シェルの初回 exec（macOS の検査・手元 130〜390ms）が確かめの上限 5 秒の内側で払われていた。偽シェルを作ったその場で 1 回起こして終わるまで待ってから測る（上限は延ばさない））
- 初回 exec を 6 秒にする注入で修正前は Issue と同じ `nu / Ja` の不明を含む FAILED → 修正後 8 / 8・同型の候補 4 本は同じ注入で全部 ok・カタログ +0 B。PR #2648

## 2026-11-02（#2365 の続き: 読めないプロファイルの理由（#2256 の系統）を表示言語へ。芯 `UnreadableProfile`・知らせ `ProfileFallback` 5 種・`ConfigError` 3 種、利用者へ返す口は表示言語・persist.log は日本語。check-health の profiles 行も型の口へ）
- 実経路（CLI + mcp_host の MCP）で英語表示の日本語入り 32/64 → 4/64（残りは projects add の枠）・日本語 56/64 sha256 一致（残り 8 は時刻と置き場だけ）・persist.log 一致・注入で名指し + E0004・カタログ +0 B。PR #2645

## 2026-11-02（#2228 の 3.: cargo が作る偽 LSP サーバ・debug の tako の初回 exec の検査（大きさに比例・361ms / 1,763ms）が LSP の e2e の 10 秒の窓と issue2367 の窓を食っていた → `fake_exec::warm_binary` で時計の前に空打ち・テスト専用の実行ファイルは `enter_test_binary`・A/B `TAKO_2228_LEGACY=1`）
- 注入 12 秒で修正前 FAILED 13 本 → 修正後 24 本 ok・issue2367 も FAILED → ok・番犬が逆戻り 2 種を file:line で名指し・カタログ +0 B。Issue の 1・2（tako-vd が要る）は残す。PR #2650

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
