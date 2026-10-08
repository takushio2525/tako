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

## 2026-10-09（#748 / PR #754: 合成入力欄をダイアログと誤判定しない固定を今の main へ載せ直した）
- `dialog.rs` のテストを描く側と同じ組み立てへ（#719 / #718 = 罫線 16 桁・#737 = 20 桁・#1067 = 30 桁 + フッター）。キュー滞留ヒントは #737 ではなく #1067 の形
- 実測: 注入 A（罫線の棄却を外す）/ A+B（兄弟 1 つで並び）/ E（罫線の最小を 20 本）で形を名指しして FAILED（E は入力欄系でこれだけ）

## 2026-10-09（#1893: LSP ホバーの続き = 右クリックの項目・⇧⌘H・CLI / MCP の全文の口・読み込み中のマウス）
- 右クリックに「ホバー情報を表示」（`MenuItem::Hover`。移動 → ホバー → 整形・`hoverProvider` の申告で出し分け）・キー ⇧⌘H / Ctrl+Shift+H（⌘K ⌘I はパレットの発火が遅れるので不採用）を編集メニューと同じ `request_lsp_hover` へ。manager は全文を返し、カードは 16,000 字・CLI `--full` / `--limit N` = MCP `limit`（0 = 全文。カタログ +60 B）。マウスも #1869 の `wait_loaded` で待ち、語の真下に「読み込み中」→ カード。取り消しの番号は UI で先に取る（`reserve_hover`。背景で取ると取り消しを追い越して待ち続ける）
- 実測: `scripts/test-lsp-hover-1893.sh`（visual-test `hover-1893` / `hover-loading` / `hover-loading-real`・A/B `TAKO_1893_LEGACY=1` で名指しの FAILED・#1684 / #1681 の節の回帰・CLI / MCP 字面一致）・実の rust-analyzer は暖機なしで 0.08 秒で「読み込み中」→ 2.2 秒でカード（旧は 95 秒出ない）・e2e 7 本・番犬の注入 12 通りを file:line で名指し・右クリック前のマウスのホバーが明示のカードを消していたのを直した

## 2026-10-09（#1892: tako mod の validate / test を夜間リリースの前段で毎晩回し、落ちたら通知する）
- 本体 `scripts/check-claude-mod.sh`（origin/main の mod を一時 dir へ取り出し、使い捨ての設定 dir で `claude plugin validate --strict` / `test`。各段 60 秒の上限でプロセスグループごと打ち切り・`.catch` 抜けと 0 本も不合格・claude が無ければ未実測 = exit 3）。`nightly-release.sh` はロック直後に毎晩呼び、**結果でリリースを止めない**（ERROR + 既存の通知）。`~/.claude-orchestrator/state/tako-mod-check` に検査した claude の版と前回合格の版・mod の木を記録し「更新で壊れた / mod の変更で壊れた」を出し分ける
- 実測: `scripts/test-nightly-mod-check-1892.sh` 100 PASS（claude の無い PATH で 92 PASS + 未実測 1）・回帰の注入 6 通りを名指しで FAIL・実物の claude 2.1.294 で壊れた登録が exit 1・利用者の設定の mtime 一致・既存の nightly 139 / retry 55 / promote 125 緑

## 2026-10-09（#1881: tako mod S3 = Claude Code の画面のプロンプトの上に帯 1 行と /tako のサイドバーを出した）
- 判断は tako（`claude_mod::band_view` / `classify_worker` / `band_warnings` を `tako mod report` の応答の `tako.view` へ）、mod は `bodyColumns` に合わせて優先度の低い区切りから落とし `Text` 1 本（truncate-end）で描く。worker は右パネル orch と同じ `Workspace::workers_of` へ寄せた。トグルは `$.store`（`/tako band on|off`・ボタン・`tako mod band on|off` の中継。新しい方が勝つ）。A/B `TAKO_1877_S3_LEGACY=1`・検証用 `TAKO_1881_BAND_THRESHOLD`・MATRIX `claude_mod_band`・カタログ +74 B
- 実測: `scripts/test-claude-mod-band-1881.sh` 49 PASS 0 FAIL（実 claude 2.1.294・組織 / 個人の設定 dir）で 80 / 144 / 300 桁 × 21 行の帯が 1 行・ダイアログ 4 回の後に戻る・再起動後もトグル保持・A/B で描かない・`claude plugin test` 26 本（terminal / desktop）・番犬 `issue1881_claude_mod_band_watchdog.rs`（注入 12 通りを file:line で名指し）

## 2026-10-09（#1901: debug ビルドでも構文の塗りの依存 8 つだけ opt-level 3 にした）
- `.cargo/config.toml` に `[profile.dev.package.*]` を syntect / fancy-regex / regex-automata / regex-syntax / aho-corasick / memchr / bit-set / bit-vec へ（どれを外しても遅くなるのを 1 MB の TS で実測。自分のクレートは未最適化のまま。ルートの Cargo.toml だと rust-cache のキーに入らず CI が毎回下流を作り直した = PR の初回 macOS 39 分）。commands.md の build 行・#1890 の「debug は 427.6 秒」2 か所に後の値を添えた
- 実測（JOBS=2・前後交互に 2 回ずつ）: 10 MB の塗り 53.8 → 4.3 秒（12.4 倍）・visual-test `large-file-decor` の debug は 1 節 30 分 → 152 秒・差分ビルド tako-core 9.5 / 7.8 → 9.3 / 8.4 秒・クリーンは 8 つで +36 秒の CPU（壁時計は負荷のぶれ以下）・全体テスト 6515 passed（tako-app 単体 30 → 12 秒）
