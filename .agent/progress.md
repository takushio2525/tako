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

## 2026-09-23（#763: リンクを開く修飾キーをプラットフォームごとに 1 箇所で決めた）
- リンク経路 13 サイト（ターミナルのクリック / cmd+右クリック #1182 / ホバー 6 / md・PDF のクリック / リリースノート）が `Modifiers::platform` を直読みし、Windows は Win+クリック要求だった。判定を `tako_core::platform::keys::link_modifier_active(platform, platform_key, control)`（macOS = command のみ / Windows = control のみ。`platform || control` を素で足すと macOS の Ctrl+クリック = 右クリック相当と衝突）へ寄せ、GPUI 側は `keybindings::link_modifier_active` / `link_modifiers` / `non_link_modifiers` の 3 本だけが `Modifiers` を触る形にした。表記も同じ表を見る `keys::link_click` で MCP カタログ・CLI ヘルプ・docs が実行 OS に追従する
- 実測: 番犬 `issue763_link_modifier_watchdog`（4 規則）へ注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・隔離 GUI（tako-vd）のセルフテストが `TAKO_APP_SELF_TEST_OK` 完走（310 秒 / 240 診断行）で `TAKO_SELF_TEST_763: file_click=true dir_click=true wrong_modifier_opened=false`・workspace 5204 passed 0 failed・clippy 3 宇宙 0・check-windows error 0
- 次: Windows 実機での Ctrl+クリック実測は #467 の実機レーンで（offline のため未検証）

## 2026-09-23（#1540: MCP ツールカタログの説明文を圧縮し、Issue 番号を落とした）
- AI が引けない Issue 番号を description / inputSchema から**195 箇所すべて**落とし（根拠は `// 出自: #…` のソースコメントへ 83 ツールぶん移送）、`tako_orchestrator_worker_status` を 3,587 → 1,566 字へ。残り 76% は識別子なので地の文は 368 字（これ以上は応答キー・enum 値を捨てることになる）。ダイアログ構造は `tako_orchestrator_respond`、`prompt_delivery_failure` の値は `tako_orchestrator_workers`、`delivery` の項目は `tako_read_pane` を正本に寄せた
- `next_step` / `degraded` を返す 9 ツールへ読み方を明記し、`tako_setup` の `orchestrator` / `sleep_guard`（カタログ唯一の description 欠落）を補い、導線の無かった 6 ツール（`select_tab` / `recent` / `git_push` / `git_pull` / `preview_undo` / `redo`）へ前提ツールを足した。規則は `.agent/conventions.md` の新節、上限は #1539 の 210 KB → **200 KB** へ締め直し
- 実測: カタログ 201,535 → 198,830 バイト・52,028 → 51,076 トークン（tiktoken `o200k_base`）・Issue 番号 195 → 0。番犬 `mcp説明文にissue番号を書かない` へ注入 3 通りすべてツール名指しで FAILED → 戻して緑。識別子は**カタログ全体で消失 0**（機械照合）・workspace 5172 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1578: CLI 出力から絵文字を消し、is_emoji の番犬を CLI へ広げた）
- 本番リテラルの実測は 18 件（Issue の `ℹ`5 / `⚠`1 に加え `✓`4 / `✗`3 / `❯`6）。14 件を文字ラベルへ置換し、**語彙は発明せず `setup.rs` から引いた**（同じ文言を setup.rs は既に `[OK] …` で出していて main.rs だけが取り残されていた）。同じ列の `─` / `△` も揃えないと混在列になるのでその 2 つの match だけ寄せた
- 走査は `tests/common/emoji_scan.rs` の 1 実装へ寄せ #1536 の番犬も載せ替えた。残る 4 件（`mcp/catalog.rs` の `❯`）は AI だけが読むツールカタログなので理由つき ALLOW。規約は `.agent/conventions.md`「絵文字を出さない」節
- 実測: 隔離 CLI の before/after 4 経路・origin/main の形へ戻す注入で 14 件すべて file:line 名指しで FAILED → 戻して緑（#1536 は注入中も緑）・workspace 5178 passed 0 failed・clippy 3 宇宙 0

## 2026-09-23（#1627: Instant の巻き戻しをやめ、起動直後の panic を止めた）
- `PaneMapping::new()` の `Instant::now() - Duration::from_secs(999)` は**ブートから 999 秒未満で panic**（`Instant` の起点はブート）。本番の呼び手は `remote serve` の起動と `backend_session_of_pane` で、CI Windows では uptime が閾値を跨ぐかだけで結果が反転していた（746 秒 = 3 件 FAILED / 1012 秒 = ok = 速い CI ほど落ちる）
- 初期値は `Option<Instant>` の `None`（= 期限切れ。時刻を捏造しない）へ。「N 前に起きたことにする」用途は `tako_core::monotonic::rewound`（飽和する 1 実装）へ寄せ、**実は 9 箇所**あった直書き（Issue の 6 箇所は行単位 grep の見落ち = 改行に割れた 3 件）を全部通した
- 実測: 番犬 4 本（改行をまたぐ走査・寄せ先の飽和・空振り検査）+ 実ファイル注入 3 通りすべて file:line 名指しで FAILED → 戻して緑。`is_none_or` → `is_some_and` の 1 語反転も新しい単体テストが落とす

## 2026-09-23（#1569: probe_path が Windows で repo_rel を空にしないようにした）
- `config_share::env::probe_path` が `std::fs::canonicalize`（Windows は verbatim `\\?\C:\…`）の戻りを `git rev-parse --show-toplevel` の戻りへ `Path::strip_prefix` していた。成分単位の比較で `Prefix(VerbatimDisk)` と `Prefix(Disk)` は別物なので必ず `Err` → `unwrap_or_default()` が `repo_rel` を黙って空文字にし、`tako config` の外部管理検出（#513）で `same_place` が常に false 側へ倒れていた。解決は境界（B26）へ（`resolved` は git の cwd として**子プロセスへ渡る** = #970 そのもの）
- 表記の食い違いは新設の `tako_core::platform::path::relative_under` が吸収する（verbatim を**無条件で**落とす / `/` と `\` の両方で成分を割る / ドライブ文字だけ大小無視 / 配下でなければ `None`）。剥がす条件を付けないのは戻りが相対表記で Win32 へ渡らないため。`cfg` 無しなので macOS から Windows 形を検査できる。`platform_parity` の許可リストは 2 → 1 件（残る 1 件 = `same_dir` は両辺が同じ関数なので「比較キー専用」の例外が成り立つ）
- 実測: 純粋関数テスト 4 本（`\\?\C:\repo\home\.claude` + `C:/repo` → `home/.claude`）・注入 5 通りすべて FAILED → 戻して緑・CI の Windows 実機で該当テストが `ok`（`#[cfg_attr(windows, ignore)]` を除去。5127 passed 0 failed）・workspace 5177 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1571: Windows の master system prompt を予算内へ戻した）
- `{{platform_notes}}` が縮退理由を**全文**並べており Windows で `platform` 片 4110 B・tako が作る側が取り分（18944 B）を 1.7〜2.7 KB 超過（CI 実測 20708 / 21420 / 21639 = そのぶん利用者の追記の取り分が削られる）。#1154 / #1477 の作法で prompt には件数と引き方だけを残し、全文は動的 topic `platform` へ。短縮形・手順書・A/B は `PlatformFacts`（`notes_section_in` / `full_section_in`）の 1 実装を共有
- 番犬が**実機でしか測れない**のが元凶なので `Platform` を prompt 組み立てまで引数で通し（`system_prompt_pieces_on` ほか）、`prompt_budget_1477` は macOS / Windows 両方の形を測る。#1278 の `#[cfg_attr(windows, ignore)]` は外した。行き先が変わった既存番犬 2 件も追従（`platform_parity` の単一ソース検査 / MCP カタログ snapshot）
- 実測: platform 片 4110 → 412 B・base 20709→17118 / 21421→17830 / 21640→18049（同一バイナリで旧テンプレート + A/B から main を再現）・A/B `TAKO_1571_LEGACY=1` で番犬 4 件 FAILED → 戻して緑・CI の Windows 実機で `prompt_budget_1477` が ok・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1609: 番犬が自分の doc コメントで緑になる型を棚卸しした）
- 肯定の存在確認を**全文**へ `contains` する番犬は、実体が消えてもコメントの綴りで緑のまま（#1536 が踏み #1578 が発見した型）。`crates/*/tests/**` 180 本を 2 段の検出器で棚卸しし、該当 **27 本**を `code_view` の 3 つ目の眺め `without_comments_checked`（コメントだけ潰す / 文字列は囲みごと残す / バイト長と行番号を保つ / 空振りはその場で名指して落ちる）へ寄せた。**不在検査は全文のまま**
- 実バグ 1 件: #1308 の「A/B 入口が在る」はドライバ本体の**アーム目印コメント**だけで満たされていた（実体の `env::var` は兄弟関数 `legacy_1308`）。分岐の呼び出しと env の読みへ分けた
- 実測: 偽の緑の A/B **28 ケースすべて before(origin/main)=緑 / after=FAILED**・生読みサイト 50 → 18 組（走査先のコード部分に無い needle は 7 → 0）・新設番犬 `issue1609_comment_view_watchdog` 7 本・workspace 0 failed・clippy 3 宇宙 0

## 2026-09-23（#1648: 編集中の再ハイライトを差分化した）
- 1 打鍵ごとに全文を syntect へ通していた `apply_editor_text` を、行の切れ目の状態を 8 行ごとに持ち回る差分へ。全文経路と差分経路は同じ `step` を通すので塗り分けは食い違わない
- 実測（release / 1 打鍵）: 4,666 行 344.6→**0.56ms**（611x・再ハイライト 8 行）・5,000 行 355.4→0.47ms（760x）。間隔 8 行は時間とメモリ（1 地点 925 バイト）の釣り合いで選んだ
- 注入 6 通りすべて file:line 名指しで FAILED → 戻して緑。Markdown ⇄ Code の切替で表示だけ外から差し替わる経路は `highlight_stamp` の照合で塞いだ

## 2026-09-23（#1650: エディタが CRLF ファイルを壊さないようにした）
- `line_end` が `\n` の位置を返し `newline` が `"\n"` 固定だったので、CRLF ファイルは End で CR の後ろへ止まり（実測 `cursor=4` / `"abc\r!\n"`）Enter 1 回で混在改行（CR 2 / LF 3）になっていた。`TextBuffer` に `LineEnding` を持たせ `\r\n` を 1 つの行区切りとして扱う（行末は CR の手前 / カーソルは CR と LF のあいだに入らない / BS・Delete は 2 バイトまとめて / 移動はまたぐ）
- **既存行の改行は 1 バイトも書き換えない**（混在は多数派へ寄せず保持）。新しい改行だけが多数派に揃い、揃える口は `normalize_line_endings` の 1 実装。改行 0 のファイルだけ `for_new_file(Platform)` = OS の流儀（純関数なので macOS から両腕を固定できる。初回 CI は Windows だけ赤で、既存テスト 2 件の `\n` 直書き期待値がずれていた）。表示側の行頭オフセットも `line_start_offsets` へ
- 実測: 4 通り（CRLF / LF / 混在 / 末尾改行なし）の往復保存が修正前 FAILED → 修正後 緑・番犬 `issue1650_line_ending_watchdog` 8 本へ注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・Windows の腕を強制した全数走 5260 passed・workspace 5261 passed 0 failed・clippy 3 宇宙 0
