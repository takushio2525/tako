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

## 2026-10-02（#1864: set -e と EXIT trap を併用するスクリプトが bash 3.2 で途中の死を exit 0 に化けさせるのを塞いだ）
- 条件は「set -e + EXIT trap + 展開エラー（set -u の未定義変数・`${x:?}`・不正な置換・readonly）」で、`/bin/sh` も同じ。番人の 1 実装 `scripts/lib/exit-guard.sh`（`tako_exit_trap` / `tako_exit 0`。印の無い 0 は 1）へ 11 本を寄せた（nightly-release.sh・release.sh --promote・promo 2・テスト 6・verify-setup-multiagent）。番犬 3 規則を `shell_scripts.rs` へ
- 実測: 注入 A/B は修正前 rc=0 → 修正後 rc=1（nightly は Test 18 で番人を素の trap に戻すと 0・本物で 1 + ログと通知）。nightly 139 / promote 125 / retry 55 緑

## 2026-10-08（#1877 S0: tako mod の設計と試作 — 実物の Claude Code 2.1.294 で mod を動かし通信路と導入方式を決めた）
- 設計書 `.agent/plans/2026-10-tako-mod.md`。隔離 GUI のペインで試作 mod を動かし `$.session.usage()`（初回応答まで tokens / rateLimits は欠ける・window と cost は claude が答える）・`turn.*`・権限 / 質問待ち（`classic.PermissionRequest`）・`session.append`・帯 / ペイン / ボタンからの `tako split` を実測
- 決定: mod → tako は `$.process.run` で tako CLI（8.5 ms。tako 再起動をまたぐ tmux worker でも CLI フォールバックで繋がるのはこれだけ。MCP は `--strict-mcp-config` で policy 拒否・HTTP 直は 1.5 ms だが再起動で URL / トークンが古びる）。導入は `<data_dir>` へ展開 + ペインの env `CLAUDE_CODE_PLUGIN_DIRS`（設定ファイルを書かない・設定 dir の数に依らない）。版の下限 2.1.294。スライス S1〜S6 + 調査を子 Issue へ

## 2026-10-08（#1681: LSP ホバー = 識別子にマウスを乗せると型・doc のカード・CLI / MCP）
- core `lsp::hover`（Hover の 3 形・16,000 字の上限・範囲・能力）→ manager の `hover`（マウスは補完と同じ取り消しの列 + `open: false` = 開いている文書に加わるだけ = 乗せただけでサーバを起こさない）→ dispatch 3 段（`show` でカード = `ControlHost::show_lsp_hover`）→ CLI `tako lsp hover` / MCP `tako_lsp` の `action=hover`（+290 B）→ GUI `lsp_hover_ui`（`render_block` 経由・1 フレーム目に測って語の行の上下へ・編集メニュー / パレットの口・右クリックメニューには載せない）
- 実測: e2e `issue1681_lsp_hover` 9 本・番犬の注入 8 通りを file:line で名指し・`scripts/test-lsp-hover-1681.sh` 23 PASS（visual-test `hover` 8 相 = 基準画像との差分は矩形の外 0 px・100 回で保持件数が増えない・A/B `TAKO_1681_LEGACY=1` で FAILED / `hover-real` で実の rust-analyzer の `String` の doc がカードに出る / CLI・MCP 19 項目）

## 2026-10-08（#1873: 閲覧中の ⌘F 検索を閉じたら描画へ戻し目次を作り直すようにした）
- 開閉を `open_preview_search_bar` / `close_preview_search_bar` の 1 実装へ寄せ（Escape・⌘F のトグル・CLI / MCP）、閲覧中に描画から開いた検索なら #1661 の `restore_rendered_preview` で戻してセッションも畳む（code へ落ちていたときだけ描き直す）。CLI `tako edit search --open|--close` / MCP `tako_preview_search` の `visible`（ツールは増やさない）。A/B `TAKO_1873_LEGACY=1`
- 実測: 修正前の main は Escape 後 mode=code・目次 0 件（visual 節）。`scripts/test-md-find-restore-1873.sh` 43 PASS 0 FAIL・番犬の注入 8 通りを file:line で名指し

## 2026-10-08（#1879: tako mod S1 = Claude Code の mod の同梱・展開・ペインへの注入と状態報告）
- mod（`crates/tako-core/claude-mod/`）を `<data_dir>/claude-mod/tako/` へ展開し、claude 2.1.294 以上のペインの env（`CLAUDE_CODE_PLUGIN_DIRS` / `TAKO_CLI`）で読ませる（設定ファイルは書かない・tmux は `-e` 固定）。mod は 1 秒 flush / 15 秒 heartbeat で `tako mod report` を叩き、GUI のメモリに 45 秒の鮮度で持つ。`tako mod [on|off]` / MCP `tako_mod`（report は載せない = FR-2.42.6）。組織アカウントで classic 系が mod へ届かないのを実測し、tool.call / tool.check で拾う形を足した（FR-2.42.7）
- 実測: `scripts/test-claude-mod-1879.sh`（隔離 GUI・実 claude）全段 41 PASS 0 FAIL・`claude plugin test` 11 本（注入 3 通りで fail）・番犬 3 本（注入 9 通り名指し）・カタログ +421 B

## 2026-10-08（#1867: ファイルツリーの複数選択・⌥⌘V（移動として貼る）・大きなコピーの進み具合と取り消し）
- 選択の正本 `tako_core::tree_select`（⌘ / ⇧クリック・範囲・配下の除去）+ 押下の捕捉フェーズで外した選択を退避。まとめた操作は dispatch `FileOpMany`（clipboard / trash / move を単数と同じ口で 1 件ずつ）、⌥⌘V（Win は Ctrl+Alt+V・AltGr の文字は奪わない）は `paste_move` = #1834 の移動、コピーは `file_copy::Progress` + 一覧 `jobs` で帯と `copy_progress` / `copy_cancel`（作りかけは戻す）。CLI / MCP `tako_file_op` の `paths` と同名の op（カタログ +396 B）
- 実測: `scripts/test-tree-multiselect-1867.sh` 57 PASS 0 FAIL（visual-test `tree-multiselect` 8 場面・A/B `TAKO_1867_LEGACY=1` で ① が FAILED・CLI と MCP 16 組が字面一致・CLI で始めたコピーを MCP で取り消す / 逆・権限エラー）・番犬 17 本（注入 8 通り file:line）

## 2026-10-08（#1869: 整形を離れた箇所ごとの差分へ一本化・補完をサーバの読み込み中も出す）
- 整形と補完の確定を `EditDelta::spans`（差分 1 件に離れた箇所を並べる）+ 書き換えの原始操作 `splice_text` の 1 本へ寄せた（`chained` は廃止）。真因の実測: rust-analyzer 1.95 は読み込みの前半に補完へ即 `null`、後半は答えずに待たせる。tako は打鍵の要求だけ待たずに 0 件で返していた → 打鍵も待って問い直し（次の打鍵・閉じるで抜ける）、GUI は「読み込み中」の 1 行、CLI / MCP は `waited_for_loading_ms` / `status: loading`、`tako lsp status` に `loading`（カタログ +117 B）
- 実測: 10 万行の整形 → undo の履歴 2,600,298 B・深さ 2・undo 2 回ともバイト一致（旧 `TAKO_1869_LEGACY=1` は 17,399,988 B・深さ 1）・visual-test `completion-loading` 緑（旧で FAILED）・e2e 6 本・番犬の注入 9 通りを file:line で名指し・実の rust-analyzer は読み込み中に GUI で打っても 2.2 秒後に一覧（旧は 25 秒出ない）・整形も緑（`scripts/test-lsp-followup-1869.sh` 37 PASS）

## 2026-10-09（#1890: visual-test 節 large-file-decor が debug で必ず落ちる = 塗りの戻りを回数の窓で待っていたのを、走っている間だけ待つ状態待ちにした）
- 真因: 読み取り表示 / 編集開始の全文の塗りを 3000 / 6000 回 × 10ms の窓で待ち、debug は 10 MB の 1 回の塗りが 418.8〜473.8 秒（窓は約 118 秒）。節が入った `deecfc9` の debug でも同じ箇所で落ちる = 実回帰ではない（release も CRLF で窓の 65%）。`wait_for_background_highlight`（`view_highlights_running` / `highlight_pending` の間だけ待ち、戻ったのに揃わなければ即偽）へ 2 節 4 か所を寄せた
- 実測: `scripts/test-highlight-wait-1890.sh`（遅れ 150 秒の注入で緑・`TAKO_1890_LEGACY=view|seed` で名指しの FAILED・`drop` は上限前に Settled）・debug 単独で緑（30 分）・番犬 4 本（注入 7 通りを file:line で名指し）

## 2026-10-09（#1880: tako mod S2 = mod の報告を ctx%・使用制限・ターン状態の一次ソースにした）
- `ctx_usage::resolve_full`（mod → 画面 → transcript）を 4 経路（self / worker_status / #749 tick / チャットヘッダ）が通り、引き当ては `claude_mod::lookup` の 1 本（落ちた理由は `ctx_mod_reason` / `mod_reason`）。使用制限はアカウント単位で束ね（resets_at → % の大きい方）、解除時刻だけ `LimitHint::from_mod`（停止の判定は画面のまま）。ターン状態は mod の turn が先・respond は画面。effort は `turn.step` から（組織アカウントでも欠けない）。A/B `TAKO_1877_S2_LEGACY=1`
- 実測: `scripts/test-mod-primary-1880.sh` fake 段・claude 段（statusLine なしの実 claude で 4 経路が mod / SIGSTOP で 47 秒後 mod_stale）・番犬 `issue1880_mod_primary_watchdog.rs`（注入 6 通り）・`claude plugin test` 12 本

## 2026-10-09（#1896: MCP ツールカタログの説明文を再び短くし、予算 200 KB に約 17 KB の余白を作った）
- 160 本中 38 本の description / 引数説明を #1540 / #1711 の方針で短縮（大きい順に profiles -1,464 / spawn -775 / remote_folder -643 / open_file -640）。構造（名前・型・enum・必須・既定値）は不変、残っていた要件番号 FR-3.18 も外した。外した原文は `.agent/mcp-catalog-notes.md` の #1896 節。並走 PR の 5 本（file_op / lsp / mod / self / worker_status）は触らない
- 実測: main（4bbbcc2）199,050 → 187,678 B（-11,372 B・残り 17,122 B。rebase 前に隔離 GUI + `tako mcp serve` の tools/list と `tako context-budget` の一致を確認）。description を落とした JSON が 160 本すべて一致

## 2026-10-09（#1895: ファイルツリーの ⇧↑ / ⇧↓・⌘⌫、1 つのファイルの途中での取り消し、まとめたコピーを 1 つのジョブへ、帯の残り時間）
- ⇧↑ / ⇧↓ = `tree_select::extend`（⇧クリックと同じ `apply`）・⌘⌫（Win は Delete）= 右クリックの「削除」と同じ `trash_tree_paths`（見出し・リモートは断る）。1 つのファイルは `fs_copy::copy_file_exclusive`（同じ APFS は clone・それ以外は fcopyfile / CopyFileExW の進み具合で 1 MiB ごとにバイトが進み途中で止めて作りかけを消す）。`tako file copy a b dst` / MCP `paths` の copy = `FileOpMany` の 1 ジョブ、`copy_progress` に `eta_secs`（2 秒・1% までは出さない）。カタログ +7 B
- 実測: 製品の経路で 256 MiB の同じボリューム 61.8 → 22.6 ms（`create_new` の後の `std::fs::copy` で clone が外れていたのを直した）・別ボリューム 180.5 / 181.4 ms で差なし。`scripts/test-tree-keyboard-copy-1895.sh` 39 PASS 0 FAIL（A/B `TAKO_1895_LEGACY=1` で ① が名指しで FAILED）・番犬 13 本（注入 11 通りを file:line で名指し）
