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

## 2026-10-15（#2119: 夜間の PATH の先頭の古い tako に負けず、run_heavy が順番待ちを知る tako を探して並ぶ）
- PATH の tako を上から全部 → .app の同梱 CLI を `--max-wait` で確かめ、無い / 応答しない（10 秒）は直に走る。A/B `TAKO_2119_LEGACY=1`。モック `scripts/test-nightly-queue-path-2119.sh` を CI の macOS へ。PR #2121

## 2026-10-16（#2123: CLI の IPC の 1 往復に応答の上限 = 返事をしない GUI で tako queue・夜間リリースが固まらない）
- `protocol::reply_timeout`（順番待ち 30 秒・心拍と done 5 秒・外の仕事を待つ要求は上限なし・他は 300 秒）+ unix の締め切りつきソケット / Windows のスレッド。`tako queue` は時間切れで直に走る。A/B `TAKO_2123_LEGACY=1`。PR #2125
- 実測 `scripts/test-ipc-timeout-2123.sh` 40 OK（修正前 45 秒で打ち切り / 修正後 30 秒で直に走る・72 秒の待ちは切れない・途中の SIGSTOP は 67 秒で走る）

## 2026-10-16（#2120: master のタブ名を役割名の手動名で固定せず、管轄プロジェクトの自動名で付ける・adopt でプロジェクト名・旧版の「master」手動名は復元時に解除）
- 真因は起動 3 経路（CLI / スマホ / 人タスク）の手動名 + adopt がタブ名を変えない。アプリの自動命名は master のタブを命名しない。`scripts/test-master-tab-title-2120.sh` 39 PASS（A/B `TAKO_2120_LEGACY=1`）・visual-test `master-tab-title`・カタログ増 0 B。PR #2124。solo の同型は #2127

## 2026-10-16（#1731: Code Runner の実行設定を `<data_dir>/run-configs.json` に覚える = `tako run-config` / MCP `tako_run_defaults` の path 側）
- 保存した環境が解けなければ自動へ落とさず `Run` はエラー（`--reset runtime` を案内）・短縮が 2 候補なら選ばない。カタログ +1,012 B・A/B `TAKO_1731_LEGACY=1`。PR #2122
- 実測 `scripts/test-run-config-1731.sh` 37 PASS（env2 で走る / 消すと止まる / reset で .venv / CLI と MCP 一致）

## 2026-10-16（#2113: スケッチ内の `.h` / `.cpp` を arduino-language-server へ・本体の `.ino` を tako が隠れて先に didOpen）
- 表の `FolderClaim` + 開いた文書は開いたときの器で引く `LspManager::resolve` へ呼び手を寄せた（番犬 `issue2113_lsp_resolve_watchdog`）。A/B `TAKO_2113_LEGACY=1`。PR #2126
- 実測 `scripts/test-lsp-sketch-headers-2113.sh` 22 OK（新は嘘のエラー 0・ホバー / 補完あり、旧と main は 2 件・基板なしは unconfigured・スケッチ外は新旧同じ）

## 2026-10-16（#2127: solo のタブ名を「solo」の手動名で固定せず、作業フォルダのリポジトリ名の自動名で付ける）
- HOME では触らない・`--tab` はそのフォルダで開く・旧版の手動名は復元時に解除（role から組み立てた旧名と完全一致のときだけ）。PR #2130
- 実測 `scripts/test-solo-tab-title-2127.sh` 35 PASS（A/B `TAKO_2120_LEGACY=1`）・visual-test `master-tab-title` に solo の判定・カタログ増 0 B

## 2026-10-16（#2129: ipc の fd 漏れテストを子プロセス（同じテストバイナリの `--exact`）で数え、並列テストの fd に引きずられないようにした）
- 往復はサーバーが閉じる EOF を見届けてから戻す = 許容 +6・6 秒の最小値をやめて許容 0。注入 `TAKO_2129_INJECT_FD_NOISE` / `_LEAK`・A/B `TAKO_2129_LEGACY=1`。PR #2131
- 実測: 雑音 7 本で旧 5 → 12 FAILED / 新 5 → 5 ok、漏れ 1 本は旧が見逃し新は 5 → 9 で落ちる。残りの後始末は #2136

## 2026-10-16（#1732: 実行設定に引数・環境変数・作業ディレクトリ・実行前コマンド = `tako run-config --args/--env/--unset-env/--cwd/--before`）
- env はコマンド文字列へ埋める（器の中へ届く）・before は群の門（本体が複合文でも止まる）・cwd を移すと `${fileBase}` は絶対パス。カタログ +391 B（残り約 2.5 KB）。PR #2132
- A/B `TAKO_1732_LEGACY=1`・番犬 `issue1732_run_config_log_watchdog`・実経路 `scripts/test-run-config-1732.sh` 37 PASS

## 2026-10-16（#1954: two-face に無い PowerShell（.ps1 / .psm1 / .psd1）の構文を足した）
- ビルド時に YAML を構文定義へ読み、起動時は末尾へ継ぎ足す（既存 213 構文は 1 バイトも変えない・器 +15.2 KB・起動時 6〜7 ms）。yaml-rust はビルド依存だけ。.astro は ST4 の継承で読めず見送り。PR #2135
- 比較は language-coverage §6.2・実測 `scripts/test-extra-syntax-1954.sh` 34 PASS（旧は Plain Text）。#1955（Finder の候補）は既定を動かさない方法が無く採らない

## 2026-10-16（#2133: master の引き継ぎの後に spawn した worker が master の領域を割らない = 前任を閉じたら worker の `spawned_by` を後任へ付け替え）
- 持ち主の判定に後任 master を含め、付け替えは「閉じたのが master・後任がちょうど 1 枚」のときだけ。A/B `TAKO_2133_LEGACY=1`（旧は master 33 → 8 桁 / 新は 33 桁のまま）。PR #2137
- 実測 `scripts/test-spawn-master-region-2133.sh` 29 PASS・番犬 `issue2133_handoff_spawn_owner_watchdog`・カタログ増 0 B

## 2026-10-16（#2136: fd 漏れテストを理由にした回避策 7 箇所の理由文を実態へ直し（外したもの 0）、IpcServer の drop で受け口を閉じる）
- 自分へ 1 本繋いで accept を起こし上限 2 秒で listener を手放す。A/B `TAKO_2136_LEGACY=1`（子プロセスの fd 旧 5 → 10 / 新 4 → 4）。PR #2140

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
