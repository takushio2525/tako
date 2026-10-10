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

## 2026-10-09（#1960: tako mod S7-2 = 画面の UI 設定 ui.json を CLI / MCP / setup の同じ口で選んで変える。既定ボタン /compact）
- 正本 `tako_core::claude_mod_ui`（語彙・既定・検証・寛容な読み込み・一時ファイル → rename）を CLI `tako mod ui`（ローカル処理）/ MCP `tako_mod` の `action=ui` / setup（`--answers` の `mod_ui`・`--review` の最後の 3 択 + ボタン）が通る。帯のトグルの正本を `$.store` から ui.json の `band.hidden` へ（応答の `band_request` をここから作り、報告の新しい `toggled_at` は取り込む）・`tako.view.ui`・#916 の `SchemaId::ClaudeModUi`・MATRIX `claude_mod_ui`・カタログ +1,506 B
- 実測: core 16 本（ランダムな操作列 1,000 通りで常に形を満たす）・dispatch 3 本・番犬 `issue1960_mod_ui_watchdog`（3 つの口で ui.json が字面で一致・注入 10 通りを file:line で名指し）・`scripts/test-mod-ui-1960.sh` 67 PASS 0 FAIL（/bin/bash・隔離 GUI 越しの MCP / report / band を含む）・`scripts/test-mod-ui-llm.sh` の採点器 oracle 20/20・null 1/20（実モデルは API キーが無く未実測）

## 2026-10-10（#1959: tako mod S7-1 = setup が Claude Code の設定 dir ごとに skills/tako へ管理印つきの写しを入れる）
- 判断と書き込みは `tako_core::claude_mod_install`（印 `.tako-managed`・衝突の読み取り・`skills/.tako.tmp-*` からの差し替え・検証プロセスは一時 dir の外へ書かない番人）、置く先の解決と JSON / setup の文面は `tako_control::claude_mod_install`。発火は setup の段・GUI 起動時・mod の報告の `config_dir` の差分検出、口は `tako mod install|uninstall [--dry-run]`（MCP `tako_mod`・カタログ +217 B）。衝突 / `/plugin` で止めた設定 dir のペインへは env も注入せず mod も休眠（`dormant`）。A/B `TAKO_1959_LEGACY=1`・MATRIX `claude_mod_install`・SPECS `claude_mod_mark`
- 実測: `scripts/test-mod-skills-1959.sh` CLI 62 PASS・GUI 13 PASS（実 claude 2.1.294・一時 HOME。plugin list に tako@skills-dir・env 注入と同時でも hooks module は tako@inline の 1 つ・本物の ~/.claude* は前後で同じ）・`claude plugin test` 30 本・番犬（注入 15 通りを file:line で名指し）

## 2026-10-10（#1979: 本番 GUI が `ps` の poll で 10 分固まった件 = UI スレッドから届く子プロセスの待ちに上限・`tako list` の採り直しを background へ・フォーカスの無いペインの再描画に上限）
- 真因（stripped の sample を同じ日の main のシンボル付き release と命令列で突き合わせ）: IPC → `prepare_offload` → `collect_worker_status_ctx` → `has_running_children` → `process_parent_map` → `ps` の `output()` → `poll(-1)`（= #1976 が外した経路）。型を塞いだ: 親子表は libproc（`PROC_PIDT_SHORTBSDINFO` + `KERN_PROCARGS2`）・tmux / `claude agents` は `probe::command_output_with_timeout`・打ち切り後は同期に wait しない・`OffloadJob::List`。再描画は `tako redraw-limit` / MCP `tako_scrollback` の `unfocused_fps`（既定 30・フォーカス中 60）。A/B `TAKO_1979_LEGACY=1`・番犬 `issue1979_*`・カタログ +330 B
- 実測: `scripts/test-ui-thread-wait-1979.sh` 修正後 15 PASS / 修正前は①②で UI が固まる（別の要求が 8 秒で返らない・メインスレッドに本番と同じ関数の並び）・GUI 無しの注入（修正後 0.03 / 2.4 秒・修正前 10 秒で返らず）・再描画の要求 55〜72 → 29.5 回/秒。CPU は蓋閉じでフレームが組まれず（`body_renders` 0）差が出ない = 蓋を開けた機では未測

## 2026-10-10（#1958: pane を省いた `tako_show_command` / `sessions link` を呼び出し元ペインで埋める）
- 真因: MCP の変換（`mcp/request.rs`）と CLI が pane 省略時に呼び出し元（`TAKO_PANE_ID` / `X-Tako-Pane`）で埋めず、show は「対象ペインが未指定」・link はフォーカスペインの会話を返していた。show は `target_pane`、card 指定は埋めない。番犬 `issue1958_caller_pane_watchdog` がカタログで約束する 58 本 × 全 action を公開の入口で検査。`tako_open_remote` は説明を挙動（アクティブタブ）へ寄せ、「混ぜない」側として縛る
- 実測: `scripts/test-show-command-caller-1958.sh`（隔離 GUI・呼び出し元とフォーカスを分ける・HTTP / stdio / CLI + visual `show-command-caller` の実ピクセル）新 22 PASS / 旧 9 PASS 13 FAIL・注入 7 通りで単体と番犬が名指し

## 2026-10-09（#1944: 未導入の言語サーバを開いた時点で data dir へ取って起こし、エディタに状態を出した）
- 検出表の各行に `fetch`（pyright 1.1.414・ts-ls 5.1.3 + typescript 5.9.3 = npm の tarball、rust-analyzer 2026-09-21 = GitHub の単体、Node.js 24.21.0 は足りる node が PATH に無いときだけ。clangd は取らない）を配布元の公開ハッシュで検証して `<data_dir>/lsp-servers` へ。解決は env → PATH（`probe_args` で動くか確かめる）→ 置き場 → 取得。状態「取得中」・タイトルの 1 行と失敗の帯（入れる / もう一度取得）・`tako lsp install`（GUI 無しでも）・シェル統合の合図で引き直し（#1823 の 2）。A/B `TAKO_1944_LEGACY=1`・カタログ +235 B
- 実測: まっさら（一時 HOME・最小 PATH）で .py を開くだけで Node.js 52.9 MB + pyright 4.2 MB を取り 11.6 秒で読み込み済み・診断 2 件・補完 34 件・常駐 155 MB（ts-ls 4.9 MB / 4.2 秒・RA 13.9 MB / 21 秒）。e2e 10 本・番犬（注入 10 通り）・`scripts/test-lsp-fetch-1944.sh` 25 PASS
