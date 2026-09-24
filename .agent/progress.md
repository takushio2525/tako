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

## 2026-09-23（#1505: setup --check と check-health の診断項目を 1 実装の正本から組むようにした）
- 同じ「この環境で tako は使えるか」を 2 実装が別々に答えていた（`--check` にシェル統合・tako CLI の PATH・更新・remote・IPC の行が無く、PATH は `check-health` だけが別口 = 棚卸し Z19）。`tako_control::diagnostics::collect()` を項目・判定・行の正本にし、`run_check` は 285 行 → 15 行（判断ゼロ）、`check_health` は応答へ `diagnostics` 節。認証とプランの問い合わせも `auth_state_for` の 1 回へ寄り、`--check` は 12.4 → 10.8 秒
- 重い正本（実測 10.8 秒）を UI スレッドで走らせないよう `Request::CheckHealth` を `prepare_offload` へ。IPC の項目は観測者で文面を変えない形に。**寄せた先で #1503 の打ち切りの知らせが落ちていた**（CI の macOS が実回帰で赤）ので `agent_probe::output_with_timeout` の 1 実装から出し直し、`diagnostics.probe_timeouts` で機械可読にもして番犬を足した
- 実測: `scripts/test-setup-check-single-source-1505.sh` **38 PASS 0 FAIL**（隔離 GUI で dispatch・`tako mcp serve` の MCP まで実経路照合）・注入 12 通りすべて file:line 名指しで FAILED → 戻して緑・既存の隔離 5 本（81/52/49/54/verify-multiagent）全緑・workspace 5249 passed 0 failed・clippy 3 宇宙 rc=0・check-windows rc=0・docs 32 ページ警告 0

## 2026-09-23（#1616: Windows の pid 正体確認を境界へ足した）
- `verify_pid_identity` は照合が丸ごと `#[cfg(unix)]` の中で Windows は末尾の `true` へ直行 = 「生きている pid はすべて tako の daemon」。#1596 で生存判定が境界へ寄り先頭の `if` を通り抜けたことで露出（#1599 が先に入ると誤 kill）
- 材料引き（`procinfo::observe_identity`）と突き合わせ（`judge_identity` = 3 値・`Unknown` は撃たない）を境界へ新設。`remote.rs` の腕は OS 分岐の無い `boundary_identity_confirmed` 1 本で、分岐は `#[cfg]` → `cfg!(unix)`（両腕を macOS でもコンパイル）
- 実測: 注入 4 通り（素通り復帰 / 腕だけ外す / 空回り / `Unknown` 反転）すべて file:line 名指しで FAILED → 戻して緑。unix の挙動は不変（`remote::tests` 112 本そのまま緑）

## 2026-09-23（#1655: Code Runner の組み込み既定を OS 別の 1 枚の表にした）
- 21 種のうち 10 種が Windows で不成立（`python3` / `cc` / `c++` / `rustc` + `./<出力>` / `bash` / `zsh`）・`runner.rs` に `cfg(windows)` 0 件だったので、表を `platform::runner_defaults::TABLE`（**41 拡張子 × 2 列**）へ移し、`Platform` 引数 + `cfg!` で macOS の単体から Windows 列を解決結果ごと固定した
- Windows 列は PowerShell 5.1 でも通る形（`&&` / `./` を使わず `; if ($?) { .\<名前>.exe }`）。`.ps1` / `.bat` / `.tsx` など 20 拡張子を追加し、意図して置かない 9 マスは理由（日英）を持って案内へ載る（置かない基準は「その OS に解釈系が無い / 決まらない」）
- 実測: 注入 A/B 3 通り（属性の `#[cfg(windows)]` / Windows 列の `python3` / `.command` の Windows 列を絶やす）が名指しで FAILED → 戻して緑。初版は `.command` を Windows で既定なしにして CI の Windows が赤（`Run` が Err = dispatch の実行テスト 3 件）→ 基準を「解釈系が無い / 決まらない」へ正した。GUI 実経路は画面スリープで未実施（#1160）

## 2026-09-23（#1651: undo を差分にし、連続タイプを 1 塊にまとめた）
- 編集のたびに `self.text.clone()` を積んでいた（release 実測: 1 MB へ 1000 打鍵で RSS 増分 **1022.0MB**・undo 1000 回）。履歴 1 件を `EditDelta`（範囲 + 置換前後 + 編集前後のカーソル・選択・**改行コード**）へ替え、**増分 1.1MB / undo 1 回**（塊を毎回切る最悪値でも 1.2MB / 1000 回）。上限は操作数 1000 と履歴 8MiB の先に効いたほう（全文差し替え・全置換は 1 操作で本文 2 本ぶん積むので操作数だけでは上限にならない）
- 本文を書き換える口を `apply_edit` 1 本へ寄せた（通らない書き換えは undo で戻らないので番犬が名指す）。**`set_cursor` は実際に動いたときだけ塊を切る**のが要点で、GUI は 1 打鍵ごとに画面の選択をバッファへ写す（同じ位置への `set_cursor` × 2）ため、素で切ると **GUI だけ 1 文字粒度**だった
- 実測: 注入 6 通りすべて file:line 名指しで FAILED → 戻して緑・隔離 GUI + 実 CLI で apply/replace → undo 2 回で元ファイルとバイト一致 → redo 2 回で復帰・ランダム編集列 200 手 × 4 シード（CRLF 含む）を undo で全部戻すと**元の本文とバイト一致**・workspace 5303 passed 0 failed・clippy 3 宇宙 0。`replace_all` がカーソルを多バイト文字の途中へ残す既存 panic も併せて直した
