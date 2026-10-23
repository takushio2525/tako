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

## 2026-10-23（#2344: 項目 113 の真因は製品 = tmux の器越しで 1 行ごとに来る CursorBlinkingChange が #816 のゲートを素通り → `redraw_hint` で CBC / MouseCursorDirty も描き直しの合図に・`tako redraw-limit` に `delivery`・項目 113 は 3 本の判定）
- 不可視ペインの往復 33.4 → 4.8 回/秒・見えているペインの再描画は修正前と一致・LEGACY は ③・nogate は ① で FAILED・カタログ +0 B。PR #2349

## 2026-10-23（#2304: #1924 の番犬の bash 3.2 波括弧の写しに `$'…'` の構文解析時の置き換え（`ParsedWord32`）と、包まずに置き換わった `$( … )` の中身の実行時の読み直し（3.2 だけが割る見逃しの向き）を入れた）
- Issue の 4 行で食い違い 2 → 0・表を広げて 5,730 / 11,260 行で 0・注入 4 通り・リポジトリの名指しは前後とも 0・カタログ +0 B。行継続の残りは #2352。PR #2351

## 2026-10-24（#1837: 項目 155 の panic は製品 = wry の親を初回 render で 1 度しか採らず、窓を閉じた後の Web ビューの生成が `mod.rs:392` で落ちる → render で採り直して付け替え・渡す前に窓を見て理由つきのエラー・`web list` の `attached`・巨大ファイル後の復帰は伸びる上限へ）
- 修正前 4 回中 1 回 panic → 修正後 5 回完走・A/B `TAKO_1837_LEGACY=web|wait`・実経路 10 PASS・カタログ +0 B。PR #2353

## 2026-10-24（#1762: `DispatchError` の表示を `text_in(Lang)` の日英の網羅 match から引く・CLI の直呼び 12 か所は `local_dispatch_err`・リモートの「見つからない」は `says_not_found`）
- 実経路 修正前 9 PASS 18 FAIL → 修正後 27 PASS・日本語表示 18 ファイルはバイト一致・カタログ +0 B。PR #2354

## 2026-10-24（#2002: IPC の受け口がパスから切り離されたら GUI が張り直す = accept を poll + 10 秒で `lstat` の実体を確かめる・奪われていたら奪わない・check-health が「GUI は動いているのに受け口が応答しない」を分ける）
- 段 1 で「同じパスへ別の受け口が作り直して閉じた」形を再現・実経路 PASS=26（張り直し 3.9 秒）・A/B は refused のまま・カタログ +0 B。申し送り #2359 / #2360 / #2361。PR #2356

## 2026-10-24（#2358: `PaneTreeError` / `WorkspaceError` とペイン未指定の断り（`NoTargetPaneAt::{Cli, Mcp}`）を #1762 と同じ形で表示言語に従わせた）
- 英語表示で日本語入りの出力 11 → 1 本・日本語は 38 本バイト一致・実経路 PASS=57・カタログ +0 B。残りのエラー型は #2365。PR #2362

## 2026-10-24（#1902: セルフテストの回数の窓 126 か所のうち background を待つ 16 か所を `wait_for_background_state` へ寄せ、番犬 `回数の窓で待っていない`（#2013 の番犬を統合・残り 110 は `KNOWN_COUNT_WINDOWS`）を置いた）
- 実経路 debug / release 各 32 PASS・隔離セルフテスト両方緑・旧の窓 + 遅れで名指しの FAILED・カタログ +0 B。Issue は開けたまま分類ごとに移す。PR #2363

## 2026-10-24（#2359: GUI の fd 上限を起動時に min(hard, 10240, kern.maxfilesperproc) へ上げ、ペインの PTY の子には起動時の値を戻す・check-health に `fd_limit` 節と 80% の warning）
- 本番 28 ペインで 121 / 256 本（4 本 / ペイン）→ 再起動後 10240・枯渇の注入で修正前 0/5 → 修正後 5/5 通る・カタログ +0 B。器の tmux の天井は #2367。PR #2364

## 2026-10-24（#2357: 実 tmux のテストの間欠の真因はテスト = 「読むと消す」`take_pane_died_dump` を書き出しの出現で叩き、書きかけを消していた → 器が消えたと `probe_session` で確かめてから 1 回読む・同型 4 か所も）
- 注入で main は CI と同文言で FAILED → 修正後 ok・30 回 × 並列 1 / 4 で全通過・カタログ +0 B。PR #2366

## 2026-10-24（#2360: 起動時の IPC ソケットの生存確認が connect 1 回で、control.json の無い 2 つ目が満杯の生きた受け口を奪っていた → `ipc_socket::judge_occupant` が持ち主の pid と繋ぎ直しまで見る・奪わなかった 2 つ目はセカンダリ・`ipc.fixed_path`）
- 実経路 修正前 27 PASS 22 FAIL → 修正後 49 PASS・A/B `TAKO_2360_LEGACY=1`・カタログ +0 B。net_stop の間欠は #2369。PR #2368

## 2026-10-24（#2355: 項目 154 の E4 の間欠の真因はテスト = 流していたシェルのループが ^C を取りこぼす（素の zsh で 450 回中 9 回）→ ループをサブシェルで包み、止まったことを状態で待つ）
- A/B `TAKO_2355_LEGACY=1` + 注入 `lostint` で旧は FAILED・新は通過・5 回完走・カタログ +0 B。漏れの判定の偽の緑は #2371。PR #2370
