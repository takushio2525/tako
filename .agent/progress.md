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

## 2026-09-30（#1709: tako アプリのデータの扱いのページを最新の main で確かめ直し、窓口をメールと Issue の 2 本立てにして公開した）
- PR #1714 を #1844 の上へ rebase（フッターは作者のサイト → tako のデータの扱い → 共通ポリシー → Cookie 設定）。事実の記述を現行コードと 1 件ずつ突き合わせ、食い違い 4 件（更新確認の時機・自動リネームの発火条件・設定の共有の始め方・導入の条件）を直し、抜けていた事実 5 件を足した
- 窓口: 非公開は contact@takushio2525.com、不具合は GitHub Issue（telemetry.md の削除依頼も同じ）。docs 検査 6 本 rc=0・PC 幅 / スマホ幅の横はみ出し 0 px

## 2026-09-30（#1849: SECURITY.md を置いて脆弱性の非公開の報告先を案内した）
- リポジトリ直下に日英併記の `SECURITY.md`（第一の窓口 = GitHub の Private vulnerability reporting、第二 = メール。対象の版 = 最新の安定版とテスト版・対象範囲・書いてほしいこと・受領の目安 7 日・公開の流れ）。README と privacy.md のお問い合わせ節に導線 1 行ずつ
- 窓口の書き分けは PR #1714 のお問い合わせ節と同じ（非公開 = メール / 不具合・要望 = 公開の Issue）。GitHub の Markdown API で描画して見出し 6・リンク切れ 0 を確認

## 2026-09-30（#1848: macOS の配布物からビルド機のホームパスと署名者の個人名を消した）
- build-app.sh の中だけでホームを `~` へ付け替える（rustc は `--remap-path-prefix` を `CARGO_ENCODED_RUSTFLAGS` で・metallib は PATH 先頭の `scripts/lib/xcrun-remap/xcrun`・専用の `target/release-dist`）。セルフテストの `env!("CARGO_MANIFEST_DIR")` 2 か所は実行時に辿る形へ。署名は既定 ad-hoc。検査 `check_bundle_privacy` を build-app.sh の署名後と release.sh の zip 直前の 2 か所から
- 実測: HOME を含む strings 行 v0.8.24 = 1,064 / 395 → 0 / 0。release.sh は v0.8.24 で rc=1（zip 0 本）・修正後で rc=0。Gatekeeper（quarantine 付き zip）と TCC の要件つき全 19 行の判定が v0.8.24 と一致。`test-bundle-privacy-1848.sh` 33 PASS

## 2026-09-30（#1855: docs サイトの GA4 を同意まで読み込まない新しい形（basic 型）へ移した）
- `docs/astro.config.mjs` の head から gtag.js と inline の config（保険の `consent default` を含む）を消し、ハブの consent.js 1 本に `data-ga-id` を付けた。gtag.js は consent.js が地域と同意を見て差し込む（Refs takushio2525/takushio2525.com#36）。`.agent/conventions.md` の同意の節も同じ形へ
- 実測（ヘッドレス Chromium・国判定だけ差し替え・送信は 204 で打ち切り）: EEA 未選択で Google への要求 0・同意で gtag.js 1 / page_view 1・日本は 1 / 1。旧版の consent.js のままだと全場面 0 なので、merge はエッジの入れ替わりの後

## 2026-09-30（#1857: tmux の attach クライアントだけが外から終わってもペインを閉じず再 attach するようにした）
- Exited で器へ生死を聞き（`list-clients`・上限 3 秒・background）、生きていて他のクライアントが居なければ attach 専用（`attach-session`）で同じペインを張り替える。60 秒に 5 回で止まり、閉じずにチップ + 画面 2 行。persist.log に 1 行・状態は list / read の `backend_reattach`・手動は `tako persist reattach` / MCP `tako_persist` の `reattach`（カタログ +204 B）
- 実測: `scripts/test-reattach-1857.sh` 30 PASS 0 FAIL（v0.8.25 は ① で FAILED = ペインが閉じる）・番犬の注入 5 通りすべて main.rs:行 で FAILED・A/B `TAKO_1857_LEGACY=1`

## 2026-10-02（#1853 / #1592: 安定版への昇格で Homebrew cask と docs の「最新の安定版」も動かすようにした）
- `release.sh --promote` が夜間の素のタグ（prerelease を外して Latest）も受け付け、続けて tap の cask を公開アセットの sha256 で更新（PR → merge → tap の main を読み直し）、releases.md を `check-releases-page.mjs --set-stable` で書き換えた PR を `merge-pr.sh` で merge。後続の失敗は exit 4・打ち直しは同じコマンド。bash 3.2 の set -u + EXIT trap が exit 0 に化ける穴も塞いだ
- 実測: `scripts/test-release-promote-1853.sh` 125 PASS（注入 9 通りすべて FAILED）・既存の retry 55 / nightly 129 緑・docs の build + 検査 6 本 rc=0。releases.md の安定版は v0.8.26 へ
