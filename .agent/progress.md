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

## 2026-09-27（#1775: PWA e2e の証拠を spec ごとのサブ dir へ分け、CI だけ 2 workers にした）
- `TAKO_EVIDENCE_DIR` の平置きで `01-list.png` / `05-observe.png` / `06-forbidden.png` が spec をまたいで上書きし、80 回撮って 76 枚しか残らなかった。`evidencePath()` の 1 実装で `<dir>/<spec 名>/` を挟み 80 枚（outputDir 側は不変）。番犬 `issue1749_pwa_e2e_output_watchdog` に規則 5（サブ dir）・6（spec 内の同名）を足し、注入 3 通りが file:line 名指しで FAILED → 戻して緑
- CI の macOS（3 vCPU）は既定 50% で 1 worker = ステップ 129〜138 秒。同ランナーの実測で 2 workers 73〜87 秒・`--repeat-each=3` 327 項目 × 2 台 flaky 0 → ci.yml だけ `--workers=2`（3 は dev サーバーの取り分が無い）。README / commands.md の所要時間を実測値へ

## 2026-09-27（#1784: 隔離 GUI のヘルパが呼び出し側の偽の TAKO_ISOLATED を通さないようにした）
- `launch_isolated_gui` は既定 `${TAKO_ISOLATED:-1}` を `"$@"` の前に置いていたので、引数 / export の `0`・空・`false` がそのまま GUI へ届いていた（偽 GUI で実測）。GUI へは常に `TAKO_ISOLATED=1` を `"$@"` より後ろで渡し、偽（tako の `is_verification_gui` が偽と読む値）は面を起こす前に終了コード 2 + stderr 1 行で断る（#1760 と同じ「使い方の誤り」）
- 実測: 番犬 3 本追加（構造 / `/bin/bash` 3.2 の実走 / tako の真偽との突き合わせ）で注入 6 通りすべて file:line 名指しで FAILED → 戻して緑・tako-vd の実 GUI でも偽は起動せず 2、`on` は実プロセスの env が `TAKO_ISOLATED=1`

## 2026-09-27（#1653: 検索・置換で大文字小文字を区別し（既定）、単語単位のトグルを足した）
- `find_all` が常に小文字化し `replace_all("value"→"item")` が `Value::new()` を `item::new()` にしていた。`SearchOptions`（既定 = 区別する）を tako-core → dispatch（省略時は `SearchOptions::resolve` の 1 実装）→ CLI `-i` / `-w` → MCP `case_sensitive` / `whole_word` → 検索欄の SVG トグル 2 つへ 1:1。小文字写しは区別しない検索のときだけ作り本文が変わるまで使い回す（1 MB の 1 打鍵 4.449 → 0.509ms / 区別しない 1.251ms）
- 実測: `scripts/test-search-case-1653.sh` **25 PASS 0 FAIL**（tako-vd でトグルを実マウスで押して 3 → 5 → 4 → 2 → 3 件・置換で `Value` が残る + CLI / MCP の字面一致）・番犬 `issue1653_search_case_watchdog` + 単体へ注入 6 通りすべて file:line 名指しで FAILED → 戻して緑

## 2026-09-27（#1807: fd 継承の番犬が無関係な PR の CI で間欠的に落ちる件を、検査の片の偽陽性として直した）
- 真因は実装でもテストの並列性でもなく、検査の片 `fd_inherit::inherited_probe_script` の `{ : >&N; } 2>/dev/null`。bash（macOS の `/bin/sh`）は `>&N` の前に元の fd 2 / 1 を 10 以上の空き番号へ退避するので、閉じた 10 / 11 を「開いている」と読んでいた（CI で落ちた回は fd 10）。外部コマンドの fork 子で試す形（`/usr/bin/true 2>/dev/null >&N`）へ替え、daemon と PTY の両方の番犬が同時に直った
- 実測: 開く番号を 3〜20 で固定すると修正前は 10 / 11 だけ 100% FAILED（PTY 経路も同じ）→ 修正後 3 回ずつ全 ok。全 lib 20 回は修正前後とも失敗 0（手元では番号が 10 / 11 に当たらなかった）。seal を外す注入で両経路とも全番号で file:line 名指しの FAILED、旧い片へ戻す注入で片の単体テストが `[4, 10, 11, 12]` で FAILED → 戻して緑

## 2026-09-27（#1680: 定義ジャンプ（⌘クリックで定義先を新しいペインに）を足した）
- ⌘ホバーの下線（md リンクと同じ 1 実装）・⌘クリック・`tako lsp definition|declaration|type-definition|implementation` / MCP `tako_lsp` が同じ 3 段（UI で準備 → background で問い合わせ → UI で着地）を通る。offload に UI スレッドの続き（`OffloadOutcome::OnUi`）を足し、IPC ループの後処理を `after_dispatch` へ切り出した。着地は `open_file` 経由でジャンプ履歴へ積み、同じファイルは読み直さない。編集モードでなくても問い合わせのあいだだけ didOpen する
- 実サーバ: rust-analyzer は読み込み前に空で答えるので `experimental/serverStatus` を待って問い直す（tako の実ソースで初回 14 秒で `file_uri.rs:29` へ新ペイン・2 回目は使い回し）。clangd は `#include` → `foo.h` を新ペインで。実測: `scripts/test-lsp-goto-1680.sh` 38 PASS 0 FAIL（新ペイン / 使い回し / 同じペイン / #include / 戻る / 複数候補 / 3 状態 / CLI と MCP の字面一致 / UTF-16 / 未応答中も UI が止まらない）・e2e 11 本・番犬の注入 7 通り + 実ソースへの注入 A（新ペインを開かない）/ B（使い回さない）が dispatch.rs:762 / 761 を名指しで FAILED → 戻して緑
- #1791（S2 診断）の上へ合流: MCP `tako_lsp` は 1 本のまま action 5 つ（既定 diagnostics）・MATRIX の `tako_lsp` は 1 行（Windows の根拠に両方の e2e）・要件は FR-3.31 へ振り直し

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
