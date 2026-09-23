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

## 2026-09-24（#1007: LSP 統合のスライスを起票し、S1 の実装設計書を出した）
- 調査レポート §10 の分割を現状へ更新して 19 件起票（S0-b #1676 / S0-d #1677 / **S1 #1678** / S2〜S13 #1679〜#1690 / SE-3〜SE-6 #1691〜#1694）。S0-a は #1648 で済・S0-c は #1653・SE-1 は #1652・SE-2 は #1654 を参照に載せ、依存グラフと着手順を #1007 へロールアップ
- `.agent/plans/2026-09-lsp-s1.md`（395 行）: モジュール配置（GPUI 依存は tako-app だけ）/ スレッド + futures channel（tokio も GPUI executor も使わない。`ipc.rs` の前例）/ 検出表 = 行追加だけで言語が増える形 / 版は #1658 のものを使う / UTF-16 変換の置き場 / 偽サーバ 2 段のテスト戦略 / MCP・CLI の口 / 分割不可の理由 / 判断待ち 4 点
- 前払い: #1648 着地済み・#1651（PR #1671）と #1658 の着地待ちが S1 の前提

## 2026-09-23（#1597: 在籍の列挙に失敗した回を「プロセス不在」と読まないようにした）
- Windows の生死判定は Toolhelp の在籍で決まるのに、**列挙に失敗した回の空 `Vec`** をそのまま読んでいた（全 pid が不在に見え、1 回の失敗で `sweep_in` が並行して走る別 worker の test dir まで消す = #625 の事故クラス）。境界へ `procinfo::snapshot_checked`（失敗 = `None`・**0 件も失敗として畳む**）+ `snapshot_supported` を足し、`pid_alive` の Windows 腕は「居る」側・`OwnerProbe` は `Roster` の 3 値で `Owner::Unknown` = 見送りへ。macOS は `Roster::PerPid` で 1 マスも変わらない
- **先に事故を実測してから直した**: 注入 `TAKO_1597_ROSTER=empty`（#1597 以前の読み方）で**生きている子の data dir が実際に消える**（`test_data_residue.rs:422` の A/B assert を反転して実測）。受け入れは 3 本立て（`fail` = 0 件 / 注入なし = 死んだ残骸だけ消える対照 / `empty` = 消える）を実プロセスで常設
- 番犬 `issue1597_snapshot_failure_watchdog`（構造 5 + 規則 1）。注入 8 通りすべて FAILED（7 通りは file:line 名指し）→ 戻して緑・workspace 5218 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-24（#1658: 行・桁で指す範囲編集 API と文書の版を足した）
- 編集の口が全文置換 1 つだけで、5,000 行の 1 行を直すのに本文を丸ごと IPC で送っていた（実測 275,000 → 61 バイト = 4,508 分の 1）。`PreviewEditRange` / `PreviewCursor` を tako-core 操作 API → dispatch → CLI `tako edit replace-range` / `cursor` → MCP `tako_preview_edit_range` / `tako_preview_cursor` へ 1:1。座標は行 1 始まり / 桁 0 始まりの行内 UTF-8 バイトで、範囲外・文字の途中・CR と LF のあいだは**丸めずに拒否**する
- 応答へ `document`（`version` / `line_count` / `cursor` / `selection` / `undo_depth` / `undo_history_bytes`）を載せ、編集系すべてを `dispatch::preview_edit_reply` の 1 実装から組む。版は本文が変わるたびに進み（undo / redo でも戻らず進む）、`expected_version` で楽観ロックできる = LSP（#1007 S1）の `didChange` の前払い
- 実測: 隔離 GUI の実経路 30 項目すべて OK（`scripts/test-edit-range-1658.sh`）・番犬 7 規則へ注入 10 通りすべて名指しで FAILED → 戻して緑・workspace 5360 passed 0 failed・clippy 3 宇宙 0・カタログ 202,204 / 204,800 バイト（予算内）

## 2026-09-24（#1676: 開いた直後にその行へ着地できるようにした）
- `OpenFile` に `line` / `column`（1 始まり）を足し、CLI `tako open --line L [--column C]` と MCP `tako_open_file`（**ツールは増やさない**）を同じ dispatch へ 1:1 で載せた。md の写像は「`line` を渡された時点で code へ倒す」（レンダリング表示は 1 item = 1 ブロックで原文の行が残らない）と決めて FR-3.27 へ明記。行を持たない種別は**開く前**にエラー、超過は末尾行へ丸めて `clamped` で知らせる
- 着地は `ListState` が描画時に作られるので「次の描画で 1 度だけ飛ぶ」予約（`preview_pending_reveal` → `consume_pending_reveal`）。省略時は wire に現れない（`skip_serializing_if`）ので JSON は引数が生える前とバイト一致
- 実測: セルフテスト項目 153（5,000 行 / 42・1・超過・4990）が `TAKO_APP_SELF_TEST_OK` 完走・A/B `TAKO_1676_LEGACY=1` で 153a が FAILED（応答は同じ値なので画面まで見ないと差が出ない）・`scripts/test-open-line-1676.sh` **26 PASS 0 FAIL**（CLI と MCP の応答が字面一致）・注入 3 通り（写像 / 丸め / skip_serializing_if）すべて FAILED → 戻して緑・workspace 5345 passed 0 failed・clippy 3 宇宙 0

## 2026-09-23（#1645: rebase で復活した作業ログのエントリを番犬で止めた）
- `context-budget fix` の移送（古いエントリを**消す**）と main 側の移送が rebase で噛み合っても git は衝突を報告せず auto-merge するので、archive 済みのエントリが `progress.md` へ黙って戻る（9/23 だけで 4 回・毎回 worker の目視で発見。予算を超えなければ既存の番犬では落ちない）。判定を `tako_core::context_budget::revivals` の 1 実装として足し、`tako context-budget` / MCP の violations と新設の番犬が同じものを通る
- 鍵は `LogEntry::archive_line()` = `fix` がアーカイブへ書くのと同じ 1 行。(日付, Issue 番号) の組は**実データで誤検出する**（アーカイブに `2026-09-14 #1450` が 3 件・`2026-07-05 #63` が 2 件）ので採らなかった
- 実測: 注入 3 通り（復活 / 重複 / 見出しの書式を壊す空振り検査）すべて file:line 名指しで FAILED → 戻して緑。**この PR 自身の rebase でも 2 件の復活を file:line で捕らえた**

## 2026-09-26（#1709: 配布物へライセンス本文と第三者の著作権表示を同梱した）
- 法務監査（tako-legal）の即時修正分。`cargo about` で配布対象 547 クレートの本文・著作権表示を `THIRD-PARTY-LICENSES.md` へ生成（`about.toml` / `about.hbs`）、NOTICES に Zed のファイルアイコン（GPL + Lucide の ISC）と PWA の preact / marked / DOMPurify / Geist を追記、`build-app.sh` が 3 ファイルを `.app` の Resources へ署名前に置く
- `cargo deny check licenses` 0 errors・docs build / og:verify / verify-links 緑・telemetry.md の食い違い 3 点を訂正。`.app` の実ビルドはマシン負荷のため未実施（同梱は script を読んで判定）。Windows の同梱とアプリ内表示は提案止まり

## 2026-09-25（#1708: ドキュメントサイトの配信設定を点検した）
- `docs/public/_headers` を新設（HSTS・frame-ancestors / X-Frame-Options・CSP の基本指令・Permissions-Policy）。CSP はスクリプトを縛らない（Pagefind の wasm と GA・同意バナーの送信先を漏れなく許可しないと検索・計測が黙って止まるため）。旧ドメイン転送はスキーム・ポートを固定し FQDN の末尾ドットも拾う形に
- 再発防止に `docs/scripts/test-middleware.mjs`（25 ケース）と `verify-headers.mjs`（dist / 実 URL）を CI の docs 節へ。修正前の版へ当てると前者 5 件・後者 36 件で FAILED → 修正後は緑。wrangler pages dev + Playwright で検索・GA・同意の読み込みが壊れず、別オリジンの iframe だけ拒否されることを確認
- 依存は `npm audit fix` の範囲（12 → 6 件）。残りは astro 7 / starlight 0.42 / sharp 0.35 のメジャー更新が要る（静的出力で外部から届く経路は無い）

## 2026-09-23（#1579: UI の印をグリフから描画プリミティブへ）
- `×` U+00D7 を 8 件（drawer 2 / right_panel 4 / preview_render 1 / main 1）→ `svg().path(ui_icon::CLOSE)`、`⎇ tmux` → 語だけの `tmux`（U+2387 は多くのフォントに無く豆腐）、`● LIVE` 2 件 → `div().rounded_full()` の丸 + `LIVE`（`live_badge` の 1 実装）。新設 SVG はゼロ（CLOSE 流用 + 図形）
- 判定を `tako_core::emoji::is_icon_glyph`（表は明示。`▾`/`▸` は診断メッセージで実使用があるのでブロックで採らない）、番犬 `issue1579_ui_glyph_icon_watchdog` は**画面へ出る 2 経路だけ**（子要素シンク `CHILD_SINKS` = `.child(` / `.children(` の文字列 / `ui_text` カタログ）を見るので**許可リスト 0 件**。実ファイル注入 10 通りすべて file:line 名指しで FAILED → 戻して緑
- 実測: `bash scripts/test-glyph-icon-1579.sh` 5 PASS 0 FAIL（tako-vd 上・`live_dot` 82 色 / `close_icon` 9 色）・`ui_asset!("close")` を外す A/B で `close_icon` が **9 → 1 色**（`live_dot` は 82 色のまま）・workspace 5397 passed 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1697: 名前が読めない瞬間に検証用 GUI がユーザーの画面へ落ちないようにした）
- `ensure` が面の uuid を記録し、名前が読めない面（`name=?`）はその uuid で当てる。`TAKO_DISPLAY` 明示を見失った検証用 GUI は開かずに終了 4（`isolated-gui.sh` は配線済みの機でだけ明示）
- 実測: A/B `TAKO_1697_LEGACY=1` と注入 5 通りが file:line 名指しで FAILED → 戻して緑・tako-vd 実経路 18 PASS・workspace 全 ok

## 2026-09-24（#1628 / #1707: 子の終了時に PTY を読み切るようにし、待ちを観測事象で段に切った）
- 待ちの段分け（`ARGC=` を起点に `PROBE-DONE` を待つ / 上限は `state_wait_budget` 由来 / 失敗メッセージに経過・段・子の生死・混み具合）を入れたら、**Issue の前提が誤りだと分かった**: 起動は 0.2 秒で終わっていて、遅いのではなく**子の終了後に末尾が読めていない**（画面は `ARGC=1` だけ）
- 真因は製品側。自前の `PtyLoop` は子の終了で `break 'event_loop` するが、upstream alacritty がそこに持つ `drain_on_exit` の段が **#817 の移植で落ちていた**（`..Options::default()` の既定 `false` を「未使用」と読んだ）。以後 PTY を読む者はいないので末尾は永久に失われる。**知らせる前に読み切る**段を入れた（読み手は両 OS とも非ブロッキング。1 回の `pty_read` は 64 KiB で切り上げるので繰り返しが要る）。契約「終了を知った時点で出力は全部画面にある」は `architecture.md` の `pty_loop` 節へ明記（真因は #1707 として独立起票）
- 網は 1 度作り直した: 大量出力は**背圧**で子が tako を追い越せず再現しない。真の機構は mio の 1 束での順序競合なので、実測の形（短い出力 → 即終了）を 30 回繰り返す形へ。**修正後は毎回緑**だが逆向き（外すと落ちる）は順序次第で保証できないため、旧経路 A/B は `#[ignore]` で CI から外し Windows 実機の手順として残した。CI の Windows は `pty_exit_drain` 1 passed / 1 ignored・`spawn_arg_quoting` 0.33 秒（main と同値）
