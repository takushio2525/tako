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

## 2026-09-24（#1649: 編集カーソルを可視範囲へ追わせ、IME の下線も消えないようにした）
- `ListState` への `scroll_to(cursor)` が **0 件**で、打鍵 / 矢印 / ⌘F のヒット / undo / 貼り付けのどれでもカーソルが画面外のままだった。判断は `tako_core::editor_scroll`（`LineViewport` + `follow_cursor`・上下 3 行の余白・視野が狭いと詰める・端では可視化を優先）へ寄せ、UI は器の寸法を測って渡すだけにした。呼ぶのは `refresh_preview_from_editor`（編集の全経路の要）+ 検索ヒット + IME の変換開始の 3 か所、器は仮想リストと div スクロールの両方を**論理位置**で動かす
- 連鎖症状の IME 下線は `preview_pending_cursor_origin` が「可視化後にカーソル行が来る位置」を出して**本文の中にアンカーを残す**（行のレイアウトは paint でしか控えられないので、追従の直後 1 フレームは必ず無い）。効きは編集系の応答に乗る `viewport` で GUI の外から読める（位置自体は #1658 の `document.cursor`）
- 実測: **tako-vd 上の visual-test 節 `cursor-follow`（7 相）が新 = 全相 `ok=true` / `anchored=true`・旧（`TAKO_1649_LEGACY=1`）= 全相 `ok=false` / `anchored=false`**（視野 37 行 / 4,002 行 / PASS=10 FAIL=0）。番犬 6 本へ注入 9 通りすべて file:line 名指しで FAILED → 戻して緑・`editor_scroll` 単体 11 本 + `preview_render` 単体 2 本・workspace 0 failed・clippy 3 宇宙 0

## 2026-09-26（#1652: 修飾キー付きの打鍵が入口で全部捨てられていたのを直した）
- 入口の `if platform || control || alt { return false }` を外し、打鍵の意味を `platform::editor_keys` の 1 枚の表（両 OS の列）へ。`TextBuffer` に単語 / 行 / ページ / 文書端の移動と語・行単位の削除・smart Home・桁の記憶（desired column）を足し、`PreviewMove` / `PreviewDelete` + `tako edit move|delete` + MCP 2 本が打鍵と同じ口を通る
- 実測: `scripts/test-editor-keys-1652.sh` で tako-vd 上の実打鍵経路 9 相が緑・A/B `TAKO_1652_LEGACY=1` は ⌘↑ の相で FAILED・注入 7 通りがすべて file:line 名指しで FAILED → 戻して緑
## 2026-09-26（#1711: MCP ツールカタログの説明文を短くし、LSP 用に 17.6 KiB の余白を作った）
- `tools/list` が予算 204,800 B の残り 1.8 KB（202,956 B）だったので、154 本中 45 本の description / 引数説明から根拠・仕組み・経緯・重複（`clear_*` 13 本・「省略で現状維持」・schema と同じ値）を外し **186,820 B**。ツール名・順序・型・必須は機械照合で不変、外した原文は `.agent/mcp-catalog-notes.md`
- 実測: 隔離 GUI + `tako mcp serve` の tools/list 186,820 B（`tako context-budget` と一致・violations 0）・workspace 5397 passed 0 failed・clippy 3 宇宙 0。仮想ディスプレイが作れず検証窓がメイン画面に約 10 秒出た
