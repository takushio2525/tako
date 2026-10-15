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

## 2026-10-15（#2050: 鍵らしき文字の確認を mod の送信時・スマホの入力・tako send へ広げ、許可リスト・規則ごとの無効化・警告だけの段を足した）
- mod は `prompt.submit` → `paste-guard check --pane`・選択ダイアログ、スマホは 409 → 確認シート、`tako send` / `tako_send_input` は止めずに警告。許可リストは指紋だけ。tako が流した値はペインごとに 10 分確認済み（PR #2105）
- 実経路 `scripts/test-paste-guard-2050.sh` 58 PASS（実物の claude・実 daemon・A/B）+ visual ⑫・カタログ +680 B

## 2026-10-15（#2098: tmux_backend の conf 差し替えテストを 700 ms の実時間の窓から読み取り・書き換え各 200 回の回数区切りへ・issue1857 の子の終わりの待ちを 10 → 60 秒）
- 届かないときはクライアントの状態を出す。注入: 読み取り 30 ms 遅延で旧 3/3「検出力が無い」・新 3/3 ok、非アトミック化で新 5/5「不完全な conf」、kill 12 秒遅延で旧 FAILED / 新 ok。Windows CI 新 38/38 ok（PR #2100）

## 2026-10-15（#1686: LSP の名前の変更 = 既定 dry-run・段階 1 は頼んだペインのバッファだけ・右クリックの欄）
- `tako_core::lsp::rename`（WorkspaceEdit の changes / documentChanges）+ prepareRename → rename + CLI `tako lsp rename [--yes]` / MCP `tako_lsp` の action=rename（+428 B）+ 右クリックの「名前の変更…」。A/B `TAKO_1686_LEGACY=1`（PR #2099）
- 実測: `scripts/test-lsp-rename-1686.sh`（偽サーバ + 実の rust-analyzer）で dry-run 後の sha256 一致・--yes で 2 箇所・undo 1 回で差分 0・別ファイルは not-applied・`1abc` は refused、visual-test 新 OK / 旧 ① FAILED

## 2026-10-15（#2092: Windows のペインは文字コード固定の決着まで入力を預かる = `encoding_pin::PinGate`・上限 直接 500 ms / 器 1500 ms）
- `check-health` の `encoding_pin` 節と persist.log へ諦めを記録。器の内側の固定が本番経路で呼ばれていなかったのを `spawn_in` で配線（PR #2103）
- Windows CI 実測（各 25 回）: 直接は固定 5〜16 ms / プロンプト 26〜46 ms、器は内側 154〜180 ms / プロンプト 226〜310 ms。注入の A/B で旧は化け・新は `日本語テスト`（直接・器とも 5 巡）

## 2026-10-15（#2104: 対応状況の 2 ページ（windows-support / agent-support）の md をコミットせず、件数を持たない写し `docs/src/data/*.json` から docs のビルドが組む）
- 写しは 1 行 = 1 フィールドで並列の PR が別々の行を直しても衝突しない（`docs/scripts/support-pages.mjs`・astro の integration）。番犬 `scripts/test-support-snapshot-merge-2104.sh`（CI の docs ジョブ）。PR #2108
- 実演: 実物の support.rs を直した 2 本は修正前が md で衝突・修正後は衝突なしで `--check` 緑。dist は前後で `diff -r` の差分 0

## 2026-10-15（#1964: tako mod でチャット風の描画 = 吹き出し・コードの Copy（クリック）と帯の `o: copy last code`（キー）・ツールの 1 行の要約）
- ターミナル表示の既定は Copy だけ。GUI モードは `chat.place mod` でそのペインを本物の画面のまま見せる（既定 = GPUI のチャットはユーザーの判断待ち = tako todo）。GPUI のチャットは報告の `chat_seq` で読み直す（#1883 の内容）。PR #2111
- A/B `TAKO_1964_LEGACY=1`・番犬 `issue1964_*`（注入 15 通り）・`scripts/test-mod-chat-1964.sh` 全段緑・カタログ増 0 B

## 2026-10-15（#1951: `.ino` の言語サーバ = arduino-language-server の行・伴走（arduino-cli / clangd）と空の設定ファイルを表の `Arg` で・基板が未選択なら unconfigured で起こさず選ぶコマンドを案内）
- 実測 `scripts/test-lsp-arduino-1951.sh` 23 OK（IDE 同梱物で診断・ホバー・補完・帯の画像・選べば合図で起きる・ALS なし・A/B）。常駐 ALS + clangd 72〜77 MB。PR #2109
- `.h` / `.cpp` を ALS へ回すと先に `.ino` を開いた時だけ正しい = #2113 に分けた

## 2026-10-15（#2106: CI の macOS で実 tmux のテストが 1 本も走っていなかった = ランナーの image に tmux が無く全部 skip のまま ok）
- tmux の導入段（11 秒）+ `tako_core::tmux_test_gate`（`TAKO_TEST_REQUIRE_TMUX=1` で skip を panic・台帳から件数を出す）+ 番犬 `issue2106_*`。CI で 52 本が実 tmux で走る。PR #2110
- 罠: 件数の `sort -u` が en_US.UTF-8 の照合で日本語のテスト名を潰した（`LC_ALL=C`）。issue1857 の負荷依存のフレークは #2114

## 2026-10-15（#2023: 夜間リリースの後に footprint を測って基準と比べ、悪化を tako todo へ = 同じ悪化は重ねない・蓋閉じは未実測・終了コードは変えない）
- `scripts/nightly-footprint-watch.sh` + `scripts/lib/footprint-watch.py`。実機で注入 `TAKO_PERSIST=0` → 起票（+116%）・翌晩は重ねない・tako-vd 不可は未実測。計測の段 163〜165 秒。モック 130 PASS。PR #2116
- 出力中の footprint は同じコミットでも ±12% 揺れる（閾値 +20%）= 誤検出が出たら閾値を見直す

## 2026-10-15（#2112: ネット復帰で接続エラーで止まった全セッションを即時に再開 = 既定 ON・`tako limit-resume --network` / MCP `tako_limit_resume` の `network` / 設定画面「一般」）
- 入力欄を錨にした画面の判定 + 送ってよい停止があるあいだだけの到達確認 + 送達確認つきの経路（#813 と同じ層）。カタログ +332 B。PR #2118
- 実測 `scripts/test-net-resume-2112.sh` 43 PASS（復帰 → 3 本へ送達・送ってはいけない 5 種へ 0・OFF の A/B・止まっていないあいだの到達確認 0 回・visual-test `net-resume`）

## 2026-10-15（#2114: issue1857 のフレークの真因 = PTY への書き込みが EIO（子の側が全部閉じた）で `pty_loop` が ChildExit を送らずに畳まれていた）
- unix の EIO は相手側が閉じたとして入力を捨て fd を監視から外し、子の終了の合図を待つ。A/B `TAKO_2114_LEGACY=1`・注入 `TAKO_2114_INJECT_CHILD_EVENT_DELAY_MS`・番犬 `issue2114_*`。PR #2117

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
