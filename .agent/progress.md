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

## 2026-10-18（#1931 / #2301: 番犬の bash 3.2 波括弧展開の写しを実の /bin/bash と突き合わせる常設テスト（483 行・約 1.5 秒・macOS は 3.2 が無ければ落とす）と、それで見つかった braces.c との 2 規則の食い違い（閉じの `}` は区切りの後だけ・展開できない対で語の残りを止める）の修正）
- 修正前 13 箇所で赤 → 0・注入 A/B（規則を 1 つずつ戻す）で 17 / 7 箇所を名指して落ちる・全 .sh の名指しは前後とも 0 行。表を広げて見つけた `$'…'` の置き換えの食い違いは #2304

## 2026-10-18（#2313: `measure-unfocused-redraw-1979.sh` が面の眠りを `status` の rc の成功を前提に読み、#1979 から常に「描画可能」と出ていた → `vd_drawable_word` が 1 行目の文言で 描画可能 / 眠っていた / 確かめられない を返す。同型は 1 か所だけ）
- モック `scripts/test-measure-vd-sleep-2313.sh` 22 PASS（3.2 / 5.3。修正前の形を注入すると 11 FAIL）を CI の macOS へ。カタログ +0 B

## 2026-10-18（#2312: LSP e2e のアイドルの検査が didOpen への診断の到着前に受信数の基準を取っていた → `diagnostics_count == 1` を待ってから取り、偽サーバの `--publish-delay-ms 50` を常に渡す。製品は不変）
- 修正前 + 注入 0/10（CI と同じ :468 の 2/1）→ 修正後 10/10・30 回 × 並列 1 / 4 で 30/30・カタログ +0 B

## 2026-10-18（#2302: 狭いペインの AskUserQuestion でラベルと説明の境目を表示幅で読む = `dialog::label_ends`（段落の切れ目・説明つきの並びの判定）。読めない選択肢はラベルの 1 行目だけ + `label_truncated`、respond のラベル指定は拒否・番号は効く）
- 実物の claude 2.1.301 の fixture 9 本・注入 7 通り・器越しの e2e 新旧（旧は説明の文言で選択肢 3 を確定）・A/B `TAKO_2302_LEGACY=1`・カタログ +36 B

## 2026-10-18（#1830: `tako theme colors` の読めない色を `source: "ignored"` + `warnings` で返す（`ColorSource::of` / `active_theme_overrides` の 1 本・ライトに dark の上書きの出どころを付けていた食い違いも）・パレットのテーマ / 表示言語をメニューの表記へ）
- 実経路 `scripts/test-theme-colors-ignored-1830.sh` 修正前 14 PASS 12 FAIL → 修正後 26 PASS・カタログ +47 B

## 2026-10-18（#2306: 変数を使う rm の引数で空になると行き先が広がる形（`"$d/x"`・`"/tmp/$n"`・`"$p"*`）を番犬 `rmの引数で空になると広い場所を指す変数はvar_qで止める` が名指し、67 本・117 か所 + 偽 CLI 4 行 + release.sh 4 か所を `${var:?}` へ（例外 `# tako:rm-var-ok`））
- 空にする注入で旧 `rm -rf /home /bin` 相当 → 新は exit で止まる・main に当てると 115 件（rebase 後の main の新規 2 件も直した）・test-release-retry 131 PASS（3.2 / 5.3）・カタログ +0 B

## 2026-10-18（#2305: Windows のリリース検査 `verify-assets.ps1` の同梱ライセンス表の写しを消し、`Get-TakoLicenseBundle` が `scripts/lib/license-bundle.txt` を読む（無い・空・崩れは表の file:line を名指し）。番犬は「表を読んでいること」の静的検査 + pwsh での実行検査へ）
- 修正前は表に 1 行足す / 消す / 空でも 4 通りとも通った → 修正後はすべて名指しで落ちる・表に 1 行足す注入で番犬は組み立て 3 か所を名指し・カタログ +0 B。PR #2325

## 2026-10-20（#2321: `tako theme colors --target` / MCP `target` を dispatch が読み捨てていた → 指定のテーマの色・出どころ・warnings を返す（`resolve_theme_named` / `theme_overrides_named` の 1 本・知らない名前はエラー・省略時はバイト一致））
- 実経路 `scripts/test-theme-colors-target-2321.sh` 修正前 12 PASS 20 FAIL → 修正後 32 PASS・注入 4 通り・カタログ +45 B。書き込み側の同型は #2331。PR #2328

## 2026-10-20（#2320: 段落の切れ目が並びのどこにも出ない AskUserQuestion で説明がラベルへ丸ごと混ざっていた → 自由入力の `Type something.` がある並びは説明を必ず持つと読み、従来の結合が続きを飲む選択肢はラベルの 1 行目だけ + `label_truncated`（続き 1 行なら説明）= `dialog::carries_descriptions`）
- 実物の claude 2.1.301 の 5 種 × 24 幅の採取・fixture 7 本・注入 4 通り・器越しの e2e 新旧・A/B `TAKO_2320_LEGACY=1`・カタログ +0 B。残りは #2324。PR #2327

## 2026-10-20（#1816: tailscale の検出の続き = `check_status` の tailscale 項目を ok / unrunnable / missing / unknown の 4 通りに・setup は依存段の検出を末尾まで使い回す・依存段は在るが動かない CLI を `[警告]` に・正本は最後にログインシェルの PATH を上限つきで引く）
- `--version` が固まる CLI で setup 33.0 → 10.7 秒（検出 3 → 1 回）・ログインシェルにだけ在る CLI で brew 1 → 0 回。実経路 46 PASS（修正前 25 FAIL）・カタログ +0 B。PR #2323

## 2026-10-23（#2331: `tako theme` の色の書き込み 3 操作（set-color / reset-color / reset-colors）が知らない `--target` を GUI のいまのモードの表へ落としていた → colors と同じ `theme_target_name` で断る・表の選び方は `apply_color_write` の 1 本）
- 実経路 `scripts/test-theme-write-target-2331.sh` 修正前 14 PASS 35 FAIL → 修正後 49 PASS・省略と知っている名前は修正前とバイト一致・注入 3 通り・カタログ +0 B。PR #2333

## 2026-10-23（#2203: usage 分析 S4 = 右パネルの usage ビュー。ステータスバーの 5h / 7d から開き、プロジェクト → Issue → 会話と掘る。読みは CLI と同じ `Request::Sessions` を `dispatch_off_ui` で・書式は `usage::display` を共有・`--issue` / `project:` / `profile:` / `none` / `--show`）
- 実経路 `scripts/test-usage-panel-2203.sh` 22 PASS・CLI との照合 13 行一致・3 万会話で UI 占有 15 ms（A/B 旧 546 ms）・カタログ +355 B。PR #2336

## 2026-10-23（#2329: リポジトリの .ps1 を 5.1 に対して (A) BOM で読める / (B) 1 行目の #Requires -Version 7.0 で止まる のどちらかへ = installer 5 本は (B)・measure-window.ps1 は (A)・tako.ps1 は例外表。番犬 `powershell51_watchdog` と checker `scripts/windows/check-powershell51.ps1`、CI の windows ジョブに 5.1 の実物の段）
- 修正前 main は 5 本が broken（verify-assets.ps1 は CP1252 22 件 / CP932 27 件）→ 0 本・カタログ +0 B。PR #2335

## 2026-10-23（#2330: 狭いペインで本文の先頭と `❯` 行が入力欄から流れ、届いた送達を gave_up にしていた → 諦める前に transcript で受領を確かめ `received`。master 同士の close は受領済みの後任・タブのそのプロファイルの最後の master を force でも断る（別タブの後任が前任を閉じる close は通す）・guide handoff に確かめ方）
- 実経路 `scripts/test-handoff-close-2330.sh` 修正前 gave_up + 相互 close で master 0 人 → 修正後 PASS=24・注入 A/B で番犬が file:line を名指し・カタログ +193 B。PR #2334

## 2026-10-23（#2322: パレットの「表示モードを切り替え（かんたん / ターミナル）」→「表示モードを切替（かんたん・ターミナル）」・「新しいタブ」→「新規タブ」・tako-core の案内文の項目名も揃え、#1830 の番犬を全固定コマンドの分類（対 13 / 例外 10）へ広げた）
- 修正前 4 本赤 → 修正後 72 passed・注入で palette.rs:33/34・GuiModeToggle.astro:68/86 を名指し・カタログ +0 B。PR #2337

## 2026-10-23（#2324: 狭いペインの全角ラベルの折り返しを `dialog::seam` が画面の証拠で決め、決まらなければ確実な先頭 + `label_truncated`。長い語の規則は `long_word_evidence` のときだけ説明と読む）
- 実物の claude 2.1.301 の 8 種 × 24 幅（978 選択肢）で食い違い申告なし 92 → 0・注入 9 通り・A/B `TAKO_2324_LEGACY=1`・カタログ +0 B。PR #2339

## 2026-10-23（#2013: セルフテスト項目 93 (c)（#694 の判定表）を固定 4 秒窓から `wait_for_app_state` + `state_wait_budget` へ・番犬 `表示種別の判定を固定窓で待っていない`。#2012 は今の main で未再現のため閉じた）
- 旧 + 注入 late で FAILED → 新 + late で通過・never で FAILED・カタログ +0 B。PR #2342

## 2026-10-23（#2340: パレットの固定コマンドに検索だけに効く別名 = 外れた旧表記の全文（`ui_text::palette::cmd_aliases`）・絞り込みは `filter_palette_items` の 1 本・番犬は根拠の表 `PALETTE_OLD_LABELS`・visual-test `palette-alias`）
- 別名を外す注入で 20 語が引けず `palette.rs:82` を名指し・カタログ +0 B。PR #2345

## 2026-10-23（#2343: 24 桁で 3 行に折れた確定キーの案内を `dialog::WrappedHint` が `<キー> to <動詞>` の文の形で繋いで読み、背の高い AskUserQuestion を検知する）
- 全 fixture 214 画面 + 変種 2140 画面で変わったのは未検知 → 検知だけ（悪化 0）・注入 4 通り・A/B `TAKO_2343_LEGACY=1`・カタログ +0 B。PR #2346

## 2026-10-23（#2341: 表示メニューの「パネル」に tasks / usage のビューを足し、`app_menu_tests::PANEL_VIEW_ENTRIES` でビューの値ごとのメニューとパレットの入口か置かない理由を表にした）
- 実経路 `scripts/test-menu-panel-views-2341.sh` 修正前 9 PASS 16 FAIL → 修正後 25 PASS・注入で番犬が file:line を名指し・カタログ +0 B。PR #2347
