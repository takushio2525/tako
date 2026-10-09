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

## 2026-10-09（#1903: tako mod S2 の続き = ステータスバーの 5h / 7d も mod から・observed_at を値の変化時だけ・MCP 説明文・検査スクリプトの 1 実装化）
- mod の `stampLimits` が窓ごとに値が変わったときだけ `observed_at` を打つ（`$.state` に置きホットリロードをまたぐ）→ 束ね方を最新の観測へ（A/B `TAKO_1903_LEGACY=1` = #1880 の順）。ステータスバーは `status_bar_limits`（フォーカス順で最初に引けたペインのアカウント → 無ければ画面。取得元は `tako limit-service --refresh` の `claude.source`）。MCP 説明文 +527 B。`check-claude-mod.sh` に肯定形の文言の自己検査、1879 の段 0 はそれを呼ぶだけ
- 実測: `scripts/test-mod-limits-1903.sh`（visual-test `mod-limits` で 42 / 18 の帯・旧は ① FAILED / fake 17 PASS = 46 秒で画面へ戻る・放置 80% と動いている 20% で 20）・`claude plugin test` 28 本（旧の打ち方の注入で #1903 の 1 本が名指しで落ちる）・番犬の注入 12 通りを file:line で名指し

## 2026-10-09（#1924: `"$( … "{…,…}" … )"` を bash 3.2 が波括弧展開して語を割る形を直し、番犬を足した）
- 真因: 3.2 の `brace_gobbler` は `"` の中の `$(` を知らず `"` の偶奇だけで読む。`test-tree-keyboard-copy-1895.sh` の 2 箇所（`mcp_call "{\"op\":…,…}"` を `mcp_text "$( … )"` の引数に入れた形）を jq で組む形へ。番犬 `shell_scripts.rs` は全 `.sh` を語へ分け 3.2 の写しで読む（実の 3.2 / 5 の出力の差と 48,000 通り突き合わせ、食い違い 3 件はどれも検査の外の事情）
- 実測: `/bin/bash scripts/test-tree-keyboard-copy-1895.sh` 修正前 PASS=32 FAIL=7 → 後 39 / 0（bash 5 も 39 / 0。隔離 GUI・tako-vd）・棚卸し 118 本で 2 箇所 → 0・注入 6 通りを file:line で名指し。`local` / 配列 / `[[ ]]` / case 等の表は `.agent/conventions.md` の #1924 節

## 2026-10-09（#1909: 補完の打鍵と説明の補いの取り消しの番号も UI スレッドで先に取る = ホバーの #1893 と同じ口へ）
- `reserve_completion` / `reserve_resolve` → 要求の `ticket`。manager の背景は補完・説明・ホバーとも `enter_lane` の 1 口から列へ入り、`supersede(` は UI の口と `enter_lane` だけ（番犬が他を名指し）。残りは `tako lsp status` / MCP の `inflight`。A/B `TAKO_1909_LEGACY=1`・注入 `TAKO_1909_INJECT_HOLD`（背景の走り出しを合図まで止める）
- 実測: visual-test `completion-cancel` で旧い形は閉じた後の背景がサーバへ届き `inflight=1` / `pending_requests=1`（③ で名指しの FAILED）、直した形は 0 / 0・問い合わせ 0。e2e 6 本（背景で取り直す注入で 5 本が落ちる）・番犬の注入 11 通りを file:line で名指し・`scripts/test-lsp-completion-cancel-1909.sh` 4 PASS 0 FAIL（2 回）

## 2026-10-09（#1917: TS の構文の塗りを release の 1 MB で 5.9 → 1.8 秒にした = 正規表現へ「当たらない行で VM を起こさない」等価な前置き）
- 真因: two-face の TS は先読み・後読みだらけで、fancy-regex は行の全バイト位置で VM を回す（正規表現 365 本に均等に散る・1 行 1 回ほぼ当たらない）。`syntax_prefilter.rs` が TS / TSX の 425 本を `\G(?=(?s:.)*?(?:必要条件))(?s:.)*?\K(?:元)` へ書き換え（構文セットの直列化を 2 構文だけ解いて詰め直す・往復検査・プロセスで 1 回 34 ms を起動時に別スレッドで）。A/B `TAKO_1917_LEGACY=1`
- 実測（release・交互 3 回）: 1 MB の TS 5.90 → 1.73〜1.83 秒・TSX 6.1 → 1.7 秒・TS / Rust 4.6 → 1.4 倍・Rust は不変・40 行の冷えた 1 回目だけ 82 → 100 ms。塗りの全記録が実在の TS 12 本で 1 行残らず一致。単体 9 本（バックトラック回数の番犬）・番犬 4 本

## 2026-10-09（#1913: 合成入力欄の組み立てを tako-core の `synthetic_input` へ寄せ、描く側 4 か所と #754 のテストが同じ形を使う）
- 描く側（visual-test の #719 / #718・セルフテストの #737 / #1067）は `chat_g3_command` / `autogrow_command` / `gui_input_paint` / `restart_tui_paint`、dialog のテストは同じ `*_lines` を呼ぶ（描くバイトは必ず行から作る）。寄せる前の原文を切り出した比較で 13 状態がバイト一致
- 実測: #754 の注入 A / A+B / E で同じ形を名指しして FAILED・番犬 `issue1913_synthetic_input_watchdog` 4 本（全戻し・1 か所戻し・テストへの手書き 1 行を file:line で名指し）

## 2026-10-09（#1940: spawn の起動コマンドが 2〜3 行のペインで化ける・届く前に delivered と言う・起動失敗が黙る を直した）
- 真因（実測）: worker ペインが 2〜3 行だと zsh は入力行を `<…` / `>....` に畳み全文一致が成立しない → `shell_send` が書き直し 10 回（45〜47 秒）の末に**消していない行へ**本文 + Enter を書き足し `autoexport` + 正しい引数で 2 回目の起動。会話の検出（起動直後）を到達とみなして delivered → 諦めた後に undelivered。claude は 2 行で空白・器の tmux も同寸法（capture でも読めない）
- 修正: 寸法で全文を出せないなら書き直さず Enter 1 回 + OSC 133;C で実行確認・書き切り前に必ず Ctrl+C・新しいペインは最初のプロンプトの印を待つ・起動フック中の先行入力を区別 / delivered は送達フローの確認だけ / 起動直後の終了を `agent_exited` で即決着し `workers` の `launch=failed` / 8 行未満で入力欄が無ければ peer だけ（`pane_too_short`）。A/B `TAKO_1940_LEGACY=1`・カタログ +280 B
- 実測: `scripts/test-spawn-launch-1940.sh`（隔離 GUI・86×2 行・direnv 3 秒）旧 = `autoexport` + 2 回起動 / 新 = 1 回・正しい引数、失敗は 7 秒で agent_exited（12 PASS）。e2e 8 条件・番犬 4 本（注入 10 通りを file:line で名指し）

## 2026-10-09（#1926: #173 の disable_app_nap は /proc 前提で一度も効いていなかった = 消して、利用者が待つ読み込みだけ App Nap を止める）
- 交互 2 周の実測で (2) を選択: 寿命の間止めるとアイドル 341〜365 → 856〜1,154 µW・裏の出力処理 2〜5 倍の電力、得をするのは待たれる重い処理だけ。エージェント稼働中は #173 のアサーションで App Nap の対象外（優先度 28）。`UserWork::begin_load` を PDF のラスタライズ（開く / ズーム）・Markdown の組み立て / 描き直しへ（A/B `TAKO_1926_LEGACY=1`）
- 実測: PDF 117 ページ 7.2〜8.0 → 3.0 秒・ズーム 49.9 → 20.8 秒。`scripts/test-preview-load-app-nap-1926.sh` 7 PASS（旧の腕で ①② が名指しで FAILED）・番犬 `issue1926_app_nap_watchdog` 4 本（注入 12 通りを file:line で名指し）

## 2026-10-09（#1968: master の監視が UI スレッドで子プロセスを待つ・tako の子プロセスが本体の 4〜6 倍の CPU・target を Spotlight が索引）
- 真因（本番の `sample` / perf.log / `proc_pid_rusage` + 同じ構成の隔離 GUI のシンボル付き A/B）: Report が丸ごと同期・status の準備部が `has_running_children`（tmux + ps）・照会ごとに ps 2〜3 本とレジストリ 200 KB の解釈・PATH の痩せた `.app` で 2 秒ごとにログインシェル・UI ストールの誤分類。`OffloadJob::Report`・`probe_running_children`・`agents::with_shared_scan`・レジストリの中身一致の使い回し・`which_claude` の覚え・`recent_spans_within`。A/B `TAKO_1968_LEGACY=1`・番犬 `issue1968_ui_thread_subprocess_watchdog`。`scripts/spotlight-noindex.sh`（target → `target.noindex` のリンク）
- 実測（1 時間・左右同時）: 本体 CPU 6.69 → 3.88%・子プロセス 39.2 → 18.9%・ログインシェル 23 → 0 本/分・UI ストール 5 → 0・UI 専有 計 438 秒 → 0.25 秒・`list` p95 106 → 55ms・メモリは両方増えない。本番の「554 MB」は描画面の計上の出入り（151 MB）が主。描画ありの CPU 14% は出力の描画（37 fps）で差なし

## 2026-10-09（#1949: `.ino`（Arduino のスケッチ）を C++ の構文で塗る・```ino / ```arduino・ツリーのアイコン）
- `preview.rs` の `extension_alias` / `fence_alias`（純関数。#1948 で `file_type` へ移る）+ `file_icons.rs`。A/B `TAKO_1949_LEGACY=1`・visual-test `ino-highlight`（`scripts/test-ino-highlight-1949.sh` が新旧を別の dir へ書き出す）
- 実測: 9 色・同じ中身の `.cpp` と span が完全一致・開いたまま 31.7 秒で構文セットを手放す（猶予 30 秒）・閉じて 1.8 秒・ヒープ `.ino` +30.35 / 解放 −28.58 / `.cpp` +28.90 MB（構文は増えない）・旧は ① で名指しの FAILED

## 2026-10-10（#1930: LSP の e2e の答えの順序を合図で決め、読み込み中に返った古い空を製品側で 1 回だけ問い直すようにした）
- 製品: 定義ジャンプ・補完・ホバーの空の答えの後は `wait_after_empty` → `EmptyAnswer` の 1 実装（読み込み中に送った要求の空は、見る前に済んでいても 1 回だけ問い直す・状態を送らないサーバは不変・A/B `TAKO_1930_LEGACY=1`）。テスト: 偽サーバの `delay_ms` を `hold_until`（`AnswerGate`）へ・1684 は `stop_and_settle` の後に数える・visual-test / スクリプトは `LOADING_UNTIL`・番犬 #1922 に規則 4 つ
- 実測: CI の間欠 2 つを注入で再現（1684:262 = ログ 20ms 遅れで 9/10・1869:197 = 空の直後に読み込みを終えると 10/10）。修正後は 3 本 × 注入 3 種で 0/10・旧挙動は 10/10 で名指し、7 か所は扉を先に開ける注入と本来の回帰 8 種で名指し、visual-test 7 段・スクリプト 3 本（bash 3.2）緑

## 2026-10-09（#1576: 退避（たまり場・退避タブ）のペインを再起動で退避のまま同じ器へ繋ぎ直す）
- 真因（修正前バイナリで実測確定）: 復元ループが退避を `let-else` で素通りさせ器を `backend_sessions` へ登録しない → orphan 自動復帰が「復帰」タブへ別 pane id で拾い、退避エントリは端末の無い幽霊（次の保存で器 null）。退避も表と同じ枝で起こし、判断（起こす / 残す / 外す = 器も手掛かりも無い・同じ器の重複）は `shelved_restore::plan`。内訳に「戻し方」。A/B `TAKO_1576_LEGACY=1`
- 実測: `scripts/test-shelved-restore-1576.sh` 修正前 22 NG → 58 PASS 0 FAIL（orphan 0・同じ pane id / role / タイトル / limit_resume・器の pid 不変・表に出すと目印が見える・A/B で「復帰」タブ再現）・#1554 の実経路 42 PASS・番犬 6 本（修正前の main.rs で 6 本とも file:line を名指し）
## 2026-10-09（#1945: ステータスバーのワンボタンで全エージェントのリミット後の自動復帰を一括 ON / OFF・以後に立つペインも従う・退避中も対象）
- 正本 `tako_core::limit_resume_all`（エージェント = role か会話の検出・3 状態・一括・既定の採用・「決定済み」の印）を dispatch `LimitResume` の `all` + `enabled` が通り、ボタン / `tako limit-resume on|off --all` / MCP が 1 実装。既定は settings.json `limit_resume_all`（serde default）。退避中も `--pane N`・駆動の対象。`worker_status` が退避した worker を false と読んでいたのは読み出しが表のタブしか見ていなかったため（値は消えていない）。A/B `TAKO_1945_LEGACY=1`・カタログ +403 B
- 実測: `scripts/test-limit-resume-all-1945.sh` 24 PASS（旧 19 NG。再起動後も保持・手起動の codex を約 5 秒で検出して ON）・visual-test `limit-resume-all` を実マウスで 8 段（旧は ① で FAILED）・番犬 `issue1945_limit_resume_all_watchdog`（注入で `dispatch.rs:15587` を名指し）
