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

## 2026-09-27（#1832 / #1814 / #1827: 偽の FAILED になるテスト 3 本と Windows の未使用警告を直した）
- tailscale のスタブは作った直後の起動が負荷で 1 秒を超えていた（54 回中 24 回）→ 終わるスタブは終わるまで待つ。`depsの外でも明示…` は子が出力ゼロで死ぬ形（原因は未確定）→ その回だけ上限 3 回で起こし直し終了状態を残す。`issue1724_…` は端末 ID の印の行だけ数える
- 実測: 注入 A/B（上限 1ms / 最初の子を SIGKILL）before 10/10 FAIL → after 0/10・data dir 使い回し 3 回緑・check-windows warning 31 → 28・workspace 6074 passed 0 failed

## 2026-09-28（#1772: ⌘+ / ⌘- / ⌘0 をコードプレビュー（エディタ）と md の本文に効かせた）
- ペインの文字サイズ（`pane_font_sizes`）は 13 → 16 に動くのに、本文はルートの `theme.font_size` を継承していた。本文の器 `preview-scroll` で `.text_size` + `.line_height(φ)` を継承側に指定し、md の基準・コピーボタン・行高の見積もり・仮想リストの `remeasure` を `preview_body_font_size` の 1 実装へ寄せた。#611 は行ピッチ（φ = 21px）を保ち、継承側で指定する半分だけ入れた
- 実測（tako-vd）: `scripts/test-editor-font-1772.sh` 新 = 11 相緑（行 21 → 26px・可視 30 → 24 行・帯 158×21 → 194×26px・md 22 → 27px・10 万行末尾）/ `TAKO_1772_LEGACY=1` = 名指し FAILED、CLI 30 → 24 → 30 行・MCP 24 行（main v0.8.23 は 30 → 30）・番犬 6 規則へ注入 8 通りすべて file:line で FAILED

## 2026-09-28（#1834: ファイルツリーの D&D でファイル・フォルダを別のフォルダへ移せるようにした）
- 判定・実行・付け替え先は `tako_core::file_move` の 1 実装 → dispatch `FileOp{op: move, dest}` → CLI `tako file move` / MCP `tako_file_op` の `op=move`（ツールは増やさない）。同名・自分の配下・別のボリューム（EXDEV）は理由つきで断る。開いているペインはパス・バッファ・LSP（didClose → didOpen）・監視ごと付け替わり #1659 の削除扱いにならない
- 仕上げで、大文字小文字を変えて名指すと付け替えと配下の判定が外れる穴（macOS の APFS / Windows。`from_real` の最後の成分が綴りのまま）を実測で再現して直した（リンク以外は移す元ごと canonicalize）。番犬「移動の実行はdispatchの1か所だけ」が Windows の区切り（`display()` の字面を `/` の定数と比較）で dispatch 自身を違反に数えていたのも直した
- 実測: `scripts/test-tree-move-1834.sh` 31 PASS 0 FAIL（実マウス 13 場面・CLI/MCP 字面一致 7 組・A/B `TAKO_1834_LEGACY=1` で FAILED）・workspace 6161 passed 0 failed・clippy 3 宇宙 0・check-windows error 0（足した行の警告 0）

## 2026-09-29（#1843: docs サイトの検索流入を増やす — 検索向けの title・構造化データ・フォントの非同期化・解説 2 本）
- 主要 17 ページに `seoTitle`（見出しは変えず `<title>` と og:title だけ）・description 10 本を検索意図へ。JSON-LD（トップ WebSite / 各ページ BreadcrumbList / 「よくある質問」節から FAQPage）を `docs/src/structuredData.ts` の 1 か所で組む。フォントの `@import` を head の preconnect + 非同期読み込みへ。解説「Claude Code を複数同時に動かす」「tmux で AI エージェントを動かす」とハブへの導線
- 実測: `docs/scripts/verify-seo.mjs`（新・CI の docs 節）33 ページ緑・注入 7 通りすべて名指しで FAILED・schema.org 語彙の検査 errors 0。Lighthouse の前後は PR 本文

## 2026-09-30（#1845: Windows の zip・インストーラーへライセンス 3 本を同梱し、両 OS の組み立てを番犬で固定した）
- `tako.iss` の `[Files]` と `build-installer.ps1` の zip へ `THIRD-PARTY-NOTICES.md` / `THIRD-PARTY-LICENSES.md` を足した（`LICENSE.txt` は従来どおり）。`verify-assets.ps1` が zip を展開して 3 本が元ファイルとバイト一致するかを見て、CI だけがインストーラーを無人インストールしてインストール先も見る。`release-windows.yml` はタグ以外の ref から dispatch するとドライラン（Release へ添付しない）
- 実測: 番犬 `license_bundle_watchdog.rs` 5 本緑・注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・検査関数を pwsh 7.6 で 5 通り（正常 / 欠け / 食い違い / 空 / CI の外）
