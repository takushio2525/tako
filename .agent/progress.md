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

## 2026-10-16（#1733: 実行ボタン横のドロップダウンで実行環境・引数・環境変数等を見て変える = Code Runner S5）
- `RunResolve` の答えを background で取って描くだけ・書き込みは `RunnerDefaults`・出典タグで file ↔ project・候補は 5 件で畳む。`preview_run_selected` 廃止・カタログ増 0 B。PR #2141
- A/B `TAKO_1733_LEGACY=1`（旧は一覧が `python3 a.py` で Run は .venv）・番犬 `issue1733_run_menu_watchdog`・`scripts/test-run-dropdown-1733.sh`

## 2026-10-16（#1735: 実行環境の表に Node（nvm / fnm・`.nvmrc` の前方一致・`node_modules/.bin` を PATH の先頭）と Rust（rust-toolchain）の行）
- Rust は一覧に出し、明示に選んだときだけ `RUSTUP_TOOLCHAIN`（自動では rustup にファイルを読ませる）。Tier P は並列にしない。PR #2139
- 実測 `scripts/test-runner-runtime-1735.sh` 35 PASS（Rust 自動 = `<unset>` / 明示 = 届く・旧は node が候補に出ない）・カタログ増 0 B

## 2026-10-16（#1955: Finder の「このアプリケーションで開く」を動的 UTI の形式へ広げない）
- 7 案を本物の Launch Services で実測し、候補に出る形はどれも他に宣言するアプリが無い Mac で既定を動かす（後始末後は全拡張子の既定が一致）。`tako_open_file` を Windows degraded（エクスプローラー未登録 = #2134）と申告・設計書 §6.3・FR-3.22。PR #2142

## 2026-10-16（#1981: local-rules の常時部が取り分を超えたら `tako migrate` に区切りを上げる案を出し、明示の `tako migrate run` でだけ動かす）
- 自動の発火点（setup・起動時）は動かさない・本文はバイト一致・`.pre-v2.bak`。本番の写しで 474 B 超過 → 5,031 B（当てるかはユーザー判断 = tako todo）。PR #2143
- 実経路 `scripts/test-local-rules-marker-move-1981.sh` 29 PASS（旧 16 FAIL）・番犬 `issue1981_marker_move_watchdog`・カタログ +89 B

## 2026-10-16（#2047: CI 待ちの上限を 4800 秒へ伸ばし、上限に達したら最後に 2 回まで見て緑 / 赤を確定する）
- `TAKO_WAIT_PR_CHECKS_TIMEOUT` で変更可。モック 197 PASS（修正前に当てると Test 33〜35 の 34 件が落ちる）・A/B `TAKO_2047_LEGACY=1`。PR #2145

## 2026-10-16（#2015: ファイルツリーで表に無い拡張子なしの dotfile（`.npmrc` 等）を設定アイコンに = 常に偽だった判定を直した）
- `.env.local` は従来どおり拡張子で引く（`.env` との食い違いは #2152）。PR #2149

## 2026-10-16（#2014: プレビューの変更履歴を開く切替の git（rev-parse / log --follow）を background の `OffloadJob::PreviewChangelog` へ・上限 30 秒）
- 実経路 `scripts/test-changelog-offload-2014.sh` 19 PASS（最中の list は修正後 0.03 秒 / 修正前 8 秒で打ち切り）・A/B `TAKO_2014_LEGACY=1`・カタログ増 0 B。PR #2148。diff の展開は #2150

## 2026-10-16（#1952: `.ino` の基板とポートをドロップダウン・`tako run-config --board/--port`・MCP で選ぶ = `board attach` が sketch.yaml へ書く・コア無しは案内だけ）
- 実経路 `scripts/test-run-board-1952.sh` 28 PASS（3 経路とも arduino-cli の直書きとバイト一致・`core install` 0 回）・カタログ +282 B。PR #2144

## 2026-10-16（#2025: 上端ドラッグのオートスクロールが送れる履歴なし / 最上端で選択を (0, 0) へ縮めていた → `drag_scroll_step` で送れないときは端を動かさない）
- A/B `TAKO_2025_LEGACY=1`・番犬 `issue2025_tick_が判定を通す_watchdog`・実クリック `scripts/test-drag-autoscroll-2025.sh`。下端の対称は #2153。PR #2147

## 2026-10-16（#2152: ファイルツリーで `.env.local` / `.env.production` 等の `.env.*` を `.env` と同じ設定アイコンに）
- 特殊ファイル名の表に 1 行・最後の拡張子が表にある `.env.ts` 等と `.envrc` は従来どおり。PR #2156

## 2026-10-16（#2150: 変更履歴の行の diff の展開（クリック / `--expand`）を `OffloadJob::PreviewChangelogDiff` へ・#2014 と同じ上限）
- A/B `TAKO_2150_LEGACY=1`・番犬 14 節（注入 12 通り）・`scripts/test-changelog-expand-2150.sh` cli 16 / visual 2 PASS・カタログ増 0 B。PR #2160

## 2026-10-16（#2153: 最下部ではドラッグ選択のオートスクロールで選択の端を行末へ寄せない = #2025 の下端版）
- `drag_scroll_step` に下端の Hold・A/B `TAKO_2153_LEGACY=1`・番犬 `issue2153_tick_が下端で寄せない_watchdog`。実画面は蓋閉じで未実測。PR #2158

## 2026-10-16（#2157: ネット復帰の再開が、エラー直後の通知行が挟まった停止と、同じ形の止まり直しを取りこぼさない = ユーザー依頼）
- 締めの行より下の `⏺` を読み飛ばし、会話が進んだら・届いたら鍵を捨てる。A/B `TAKO_2157_LEGACY=1`・`scripts/test-net-resume-2157.sh` 38 PASS。PR #2165
- 10/16 12:40 の 22 分の DNS 断で、止まった全ペインに自動で続きが届いた（手で再開 0 本）

## 2026-10-16（#1734: Code Runner の実行先（reuse / new）を実行設定に覚える = `tako run-config --target`・MCP `tako_run_defaults` の `target`）
- 一回限りの `new_pane` > 保存値 > 既定・serde default で移行なし。カタログ +159 B・`scripts/test-run-target-1734.sh` 32 PASS。PR #2162

## 2026-10-16（#2163 / #2161 / #2164: CI の間欠 3 つ = LSP 取り消しの数え・ipc の fd テストの EINVAL・git 履歴の date_relative）
- いずれも注入で原因を確定してテストだけ直した（`wait_received` で待つ・上限を shutdown の前へ・コミットを過去の固定時刻に）。PR #2167 / #2168 / #2170

## 2026-10-16（#2051: md の表の `|` を `\|` で揃えて番犬で縛る + visual-test の `fail()` の経路でも節の一時ファイルを消す）
- 番犬 `issue2051_md_table_columns_watchdog`・`remove_on_fail` の登録 93 か所・A/B `TAKO_2051_LEGACY=1`。PR #2154

## 2026-10-16（#2159: tako mod の会話のカードに [ run ] / [ copy ] = クリック 1 回で実行（ユーザー依頼）・二重押しは止める）
- 手順 新 1 クリック / 旧 3 打（A/B `TAKO_2159_LEGACY=1`）・`scripts/test-mod-card-click-2159.sh` 42 PASS・カタログ増 0 B。PR #2166

## 2026-10-16（#2151: git_bin() / tmux_bin() の初回の解決に全体 5 秒の締め切り = 戻らないログインシェルで止まらない）
- `tako_core::bin_resolve`・打ち切ったら素の名前へ・理由を persist.log へ。A/B `TAKO_2151_LEGACY=1`・修正後 2.05 秒 / 修正前は戻らない。PR #2171

## 2026-10-16（#2169: CLI の IPC で上限の入れ直しが EINVAL のとき、届いた応答を待たない読みで読む）
- 未着なら待たずに元の誤り。修正前は 4 本が os error 22・番犬 `issue2169_ipc_einval_watchdog`。PR #2172

## 2026-10-16（#2048: 隔離 GUI では言語サーバを自動で起こさず・取らない = `TAKO_LSP_AUTOSTART=1` / `TAKO_LSP_AUTO_FETCH=1` で従来どおり）
- 呼び手 22 本へ env・A/B `TAKO_2048_LEGACY=1`・`scripts/test-lsp-isolated-2048.sh` 26 PASS。既存の `test-lsp-1678.sh` ③ の失敗は #2174。PR #2173
