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

## 2026-09-27（#1758: CLI の出力をパイプで途中で閉じても panic しないようにし、gate の証拠の字下げを揃えた）
- std の `println!` が EPIPE で panic していた（実測: `agent-support --json | head -1` で panic 2 つ + 終了コード 101。Windows は `ERROR_NO_DATA`）。tako-cli の出力マクロ 4 種を `stdio.rs` の同名マクロへ差し替え、標準出力の切断は `resume_unwind` で静かに 0、標準エラーの切断は捨てて続行（失敗の 1 を保つ）。SIGPIPE を既定へ戻す案は Windows に効かず `remote serve` の daemon を殺すので不採用。`gate set / check / show` の証拠は全行へ同じ字下げ
- 実測: 統合テスト 4 本（読み手を先に閉じたパイプ / 111 KB の 1 行読み / `2>&1` 形で終了コード保持 / 宣言順の番犬）+ unit 2 本。注入 7 通りすべて FAILED → 戻して緑。`mcp serve` の終わり方 4 通りは修正前と字面一致・workspace 5971 passed 0 failed・clippy 3 宇宙 0

## 2026-09-27（#1811: テスト判定から外れたら安全側へ倒し、deps/ の外のテストバイナリが本番へ書かないようにした）
- `is_test_process()` が置き場（`deps/`）だけで決まり、`/tmp` へコピーしたテストバイナリが本番の `recent.json` 上書き・`cli-dir` を空に・実 agent CLI を実 HOME で起動した。製品の `main`（`tako-app` / `tako`）の 1 文目で `paths::mark_product_process()` を呼び、「`deps/` にある **または** 宣言が無い」をテストとする（規則は `judge_test_process` の 1 実装・倒した回は stderr へ 1 回知らせる）
- 実測: 一時 HOME を本番に見立てた再現で修正前 17 ファイル → 修正後 0（同じ名前 / 改名とも）・製品 `tako` は一時 HOME の本番相当を解決し知らせ無し・番犬 3 段（入口の静的検査 / deps 外の子 / 製品 CLI の実行時）へ注入 5 通りすべて FAILED → 戻して緑

## 2026-09-27（#1797: setup の依存段が tailscale を remote と同じ検出で探すようにした）
- 依存段（`setup_deps::resolve`）は tailscale を PATH だけで探し、PATH 外の App Store 版を「見つかりません」→ `--yes` で brew 版まで入れていた（#1038 の 2 系統同居）。正本 `tailscale::detect_tailscale`（Runnable / Unrunnable / Absent）を足して依存段・`find_tailscale`・remote setup [1/5] が読む。在るが動かない CLI は導入済みと読み入れ直させない。`run_tailscale_within` の自前の待ち（子の終了後に join）は `probe::output_with_timeout` へ寄せた
- 実測: `scripts/test-setup-tailscale-detect-1797.sh` 55 PASS（修正前の tako で 25 FAIL = PATH 外で brew 1 回 → 0 回 / 孫がパイプを握る tailscale で 60 秒の締め切り → 5 秒）・番犬 6 本へ注入 5 通りが file:line で FAILED → 戻して緑・setup 系の隔離 10 本全緑（1499 / 1505 は tailscale を `TAKO_TAILSCALE_BIN` で閉じ込めるよう直した）・workspace 5919 passed 0 failed・clippy 3 宇宙 0・check-windows 0

## 2026-09-27（#1783: TAKO_VD_NAME に物理画面の名前を渡しても tako-vd と見なさないようにし、面の指定の判定を面を起こす前へ移した）
- `ensure` の締めの先頭に「器が作った仮想ディスプレイか」（内蔵 = CGDisplayIsBuiltin / 器の一覧に名前が無い）を置き、物理画面なら終了コード 3 と理由 1 行で断る（修正前はスタブ実測で rc=0・その面の uuid を記録・内蔵なら器へ main を撃っていた）。ヘルパは 3 を使い方の誤り（2）として返し、`TAKO_DISPLAY` の判定を面を起こす前へ移した（uuid の記録との突き合わせだけは起こした後。FR-4.8.20〜22）
- 実測: 注入 9 通りすべて file:line 名指しで FAILED → 戻して緑・モック 154 PASS（/bin/bash 3.2）・本物の tako-vd で ensure rc=0（構成の前後差分なし）・書き方の誤った `TAKO_DISPLAY` は ensure を呼ばず 0.01 秒で rc=2

## 2026-09-27（#1763: 実行中のテーマの読み直しでも読めない色を persist.log へ残すようにした）
- 起動時だけが警告を残し、`reload_theme`（`tako theme` / MCP / 設定画面）とタブバーのトグルは `resolve_theme()` の警告を捨てていた。3 経路とも `load_theme_logged` → `settings::ThemeWarningLog` の 1 本へ寄せ、前回と同じ警告は出さず増えたぶんを起動時と同じ「無視」、消えたぶんを「解消」で出す
- 実測: `scripts/test-theme-reload-1763.sh` 修正前 16 PASS 10 FAIL（読み直し 0 行）→ 修正後 30 PASS 0 FAIL（CLI / MCP / toggle / 壊れた JSON）・注入 7 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-27（#1660: 10 万行 / 10 MB まで編集できるようにした）
- 上限の正本 `preview_limit`・超えたら理由と値を画面 / CLI / MCP へ。全文の塗りは background・行頭索引・描画の差し替えだけ更新
- 実測: 8.6 万行で編集開始 19 ms・1 打鍵 中央値 3.2〜3.7 ms。`test-large-file-edit-1660.sh` 緑
- #1800 / #1802 と合流: 閲覧中の ⌘F で CRLF の強調が消える退行を節 `large-file-decor` で再現 → 修正・番犬に規則 6

## 2026-09-27（#1819: 実行中に 2 回目以降に壊れた設定の中身も退避へ積んで残すようにした）
- `quarantine_unreadable` は `.unreadable.bak` が在ると何も写さず、settings.json の申告も 1 プロセス 1 回 = 実行中に 2 回目に壊れた中身は保存（load → 既定値 → save）で跡形もなく消えていた（隔離 GUI で実測。recent / shortcuts / layout（`.corrupt` の rename）も同型）。2 本目以降を `.unreadable.<n>.bak` へ積み（同じ中身は積まない・上限 10 本で 1 本目を残して 2 本目から押し出し = persist.log に記録）、申告は壊れた中身ごとに 1 回へ
- 実測: `scripts/test-unreadable-quarantine-1819.sh` before 21 PASS / 17 FAIL → after 38 PASS / 0 FAIL・注入 5 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 5995 passed 0 failed・clippy 3 宇宙 0

## 2026-09-27（#1730: 設定なしでプロジェクトの .venv / uv / poetry 等で走るようにした）
- `.py` が常に PATH の `python3` で走っていたので、S1 の検出（`runtime_env`）を `Run` / `RunResolve` へ配線した。`py` 行は `${python} ${fileBase}`（実行環境が無ければ今とバイト一致）、activation は境界 B1 の `compose_run_script` でコマンドの先頭へ埋める。Tier P は `probe::output_with_timeout` だけを通し、答えを覚えて `Run` は子プロセス無しで重ねる（`runtime_probe`）。`RunResolve` は `prepare_offload` へ載せ `refresh` を足した
- 実測: `scripts/test-runner-runtime-1730.sh` **24 PASS 0 FAIL**（venv / uv run / 壊れた venv / 空白と日本語 / 固まる Tier P の打ち切り / CLI と MCP の字面一致 / `TAKO_1730_LEGACY=1` で ① が FAILED）・Tier F 0.09〜0.35 ms・合流後の workspace 6070 passed 0 failed・MCP カタログ +434 B

## 2026-09-27（#1820: メニュー項目名から `/` を外して全項目を invoke できるようにし、tako theme の応答に warnings を載せた）
- 「ライト / ダークを切替」など 4 項目（日英）がパス区切りで割れて `tako menu invoke` で名指しできなかった（英語の `Light/Dark` は空白が無く偶然届いていた）。「・」へ言い換え、番犬 2 本（全ラベルの `/` を両 OS の文言まで file:line で名指し / 実メニューの全項目を日英でフルパス・項目名の両方から `resolve_menu_item` → `find_menu_action_in` へ通す）
- `tako theme` / MCP `tako_theme` の status / set / toggle へ `warnings`（persist.log と同じ帳簿 `ThemeWarningLog::current` から。0 件ならキーごと出さず従来とバイト一致）。カタログ +105 B（#1730 と合流後 194,174 / 204,800）
- 実測: `scripts/test-menu-theme-1820.sh` 修正後 33 PASS 0 FAIL / main 15 PASS 18 FAIL・注入 6 通りすべて FAILED → 戻して緑・clippy 3 宇宙 0

## 2026-09-27（#1659: 外部変更を検知した後の逃げ道（上書き / 読み直し / 差分）を足した）
- core `save_overwrite` / `reload_from_disk`（食い違った範囲だけ = undo で戻る）/ `disk_diff` → `PreviewSave{force}` / `PreviewRevert` / `PreviewDiff` → `tako edit save --force|reload|diff` → MCP `tako_preview_save` の `action`（+403 B）→ GUI の帯
- 競合中は自動保存を止め通知 1 回・未編集は追従・未保存のプレビューへ別ファイルは分割。実経路 45/45（main のバイナリは 30 FAIL）・注入 7 通り名指し FAILED

## 2026-09-27（#1769: LSP S1 の続き = 単独 CR・同じファイルの 2 ペイン目・サーバ解決のキャッシュ）
- LSP の行を仕様どおり単独 CR でも区切り、送る本文の単独 CR を LF に揃えた（実測: rust-analyzer / clangd の問い合わせは `\n` だけ・clangd の診断 / pyright / TS は仕様どおり）。同じファイルは 1 URI = 1 文書を持ち手で共有（didOpen / didClose は最初 / 最後だけ・版は単調）。解決はキャッシュし、restart・シェル統合の合図（cwd 変化 / コマンド終了）・パス消失で引き直す。探索は #1730 と同じ `exe::find_with_timeout`（上限つき）の 1 実装へ合流で寄せた
- 実測: 隔離 GUI の実経路 48 PASS 0 FAIL（servers 1 回目 1075 ms → 2 回目 33 ms・#1659 の追従 / 読み直しでも didChange が飛ぶ）・注入 15 通りすべて FAILED → 戻して緑。限界: rust-analyzer の flycheck 診断は単独 CR の後ろでずれる（rustc が `\n` だけで数える = 実測）

## 2026-09-27（#1778: split --command の保持を側路へ寄せ、プログラムが印字したマーカーで偽の確定をしないようにした）
- `split --command` の失敗時の保持が画面へ `__TAKO_EXIT=N` を出し、読む側は側路を持つ実行ペインでも画面を読んでいた（`__TAKO_EXIT=7` を印字して 0 で終わると 7 で確定・3000 行印字すると 141 で確定）。側路を `Pane::exit_file` へ移し、保持も実行ペインと同じ置き場・同じ伝える片（`posix_exit_report` / `powershell_exit_report`）へ寄せた。`run_pane_exit_code` は側路を持つペインで画面を読まない。`--wait` の打ち切りは exit 124、auto_close のペインログは `close:auto`
- 実測: `scripts/test-run-pane-followup-1778.sh` main 26 PASS 15 FAIL → 修正後 41 PASS 0 FAIL・注入 5 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 6117 passed 0 failed・clippy 3 宇宙 0・check-windows error 0・MCP カタログ +0 B
