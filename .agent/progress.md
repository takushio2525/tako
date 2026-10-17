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

## 2026-10-17（#1838: setup の打ち切りテストで probe_timeouts が空の間欠 = 偽の子の初回 exec の検査が 30 秒の上限を食っていた）
- 偽の子を作った時点で空打ち・A/B `TAKO_1838_LEGACY=1` で修正前 10/10 落ち → 修正後 0/10・製品コード不変。PR #2218

## 2026-10-17（#2217: 右パネルの `git_feedback_text` の文字数の切り詰めを固定値テストで固定してから `truncate_chars(…, max + 1)` へ寄せる）
- 総当たり A/B 33,230 件の差は「行を落とさず 601 文字ちょうど」の 324 件だけ・行数の切り詰めは不変。PR #2219

## 2026-10-17（#2202: usage 分析 S3 = 観測した利用制限の % の増分を区間の使用量の比で段へ配る `tako_core::usage::share`・5h / 7d の % 列・tako の外・推定）
- 合成データでキーの誤差 1 pt 以内・実データで 7d の合計 = 観測・実経路 26 PASS・mod の無いログインは `available: false`・カタログ +32 B。PR #2221

## 2026-10-17（#1829: serde_json の `preserve_order` をワークスペースの依存で全メンバーへ有効にし、CLI の JSON の並びを単独ビルドと配布形で揃えた）
- tako-core の lib.rs 末尾にコンパイル時の番犬（`Map::shift_insert`）・`scripts/test-cli-json-key-order-1829.sh` 修正前 27/8 → 後 35/0・配布物の出力は不変。PR #2220

## 2026-10-17（#1825: 壊れた default.yaml では専用プロファイルを作らず、`projects add` は登録だけ成功させて warnings / persist.log に理由・中身は `.unreadable.bak` へ）
- 直したら `tako migrate run` で継承して作る。A/B `TAKO_1825_LEGACY=1`・結合テスト 5 本 + 番犬。`tako master` 側の残りは #2227。PR #2224

## 2026-10-17（#2222: 作りたての偽の実行ファイルを短い上限で待つテストを洗い出し、空打ちの共通の補助 `tako_core::fake_exec` / `scripts/lib/fake-exec.sh` へ通した）
- Rust 11 ファイル・CI のシェル 2 本・A/B `TAKO_2222_LEGACY=1`（#2218 の `TAKO_1838_LEGACY` を統合）・20 回 240/240 緑。残りは #2228。PR #2225

## 2026-10-17（#1824: recent / shortcuts の破損を `data_files` 経由で persist.log へ・layout の `.corrupt` rename を番号付きの退避 `stash_unrestorable` へ）
- A/B `TAKO_1824_LEGACY=1`・`scripts/test-corrupt-quarantine-1824.sh` 16 PASS・理由は `json_reason`（値を載せない）。残りは #2230 / #2231。PR #2226

## 2026-10-17（#2227: 壊れた default.yaml で `tako master` は組み込み既定で立てるが `警告:`・persist.log・system prompt で知らせ、`-<key>` は #1825 の理由と直し方で止まる）
- CLI と `master_launch::plan` を `load_profile` の 1 実装へ・A/B `TAKO_2227_LEGACY=1`・結合 9 本 + 番犬。残りの経路は #2233。PR #2229

## 2026-10-17（#2204: usage 分析 S5 = codex の rollout の token_count の累積の差分を索引へ・rate_limits を `limits.jsonl` へ `source: codex`・過去の窓まで按分）
- 実データで 19.41 億トークン完全一致・実経路 32 PASS・MATRIX の codex 2 マスは Degraded（単価表に無い）・auth.json は account_id だけを読む。PR #2234

## 2026-10-17（#2231: UTF-8 でない recent.json / shortcuts.json をバイトのまま番号付きの退避へ = `migration::quarantine_raw`。layout の rename 枝も同じ口へ）
- A/B `TAKO_2231_LEGACY=1`・`scripts/test-nonutf8-quarantine-2231.sh` 21 PASS・番犬。settings / run-configs / ui.json の同型は #2238。PR #2235

## 2026-10-17（#1868: テストの「存在しない ID」の決め打ち 33 か所を `tako_core::unused_id`（採番器から 1 つ取る）へ寄せ、番犬 `issue1868_stale_id_watchdog` で縛る）
- `u64::MAX` は採番器が巻き戻るので不採用・全体テスト並列 1 / 2 / 4 で緑。固定番号のレジストリの残りは #2239。PR #2236

## 2026-10-17（#1933 / #1969: 夜間の mod の検査の「取り出せない」の真因は `git archive | tar` で tar が終端で抜けた後の git の SIGPIPE。ファイル経由へ・記録が使えなくても結果の行と通知の本文を出す）
- 注入 `TAKO_1933_INJECT=archive-late-tail` で修正前は原報告と同じ 5 件 FAIL → 修正後 0・20 回連続 × 2 シェル緑。PR #2240

## 2026-10-17（#2230: persist.log と画面の読めない理由を serde の素の文言から「種類 + 行・列」へ = `load_issue::{json,yaml}_error_reason`・番犬 `issue2230_serde_reason_watchdog`）
- 型違いの目印の値で A/B `TAKO_2230_LEGACY=1`（settings / profiles / projects.yaml で値の件数 0）。PR #2237

## 2026-10-17（#1905: MCP カタログの折り返しで和文と英字が詰まる形・意味を並べた enum の漏れ（config_share の unlink ほか 5 件）を番犬に、説明文 72 本を圧縮して 203,726 → 194,512 B（残り 10,288 B））
- PR #2241

## 2026-10-17（#1970: 隔離 GUI の既定で本番の env を落とし・自動リネームを止め・claude の設定 dir を本物へ向けない + 番犬が検証スクリプトの持ち込みを file:line で名指す）
- 実経路 新 17/0・旧ヘルパ 3/14。番犬が見つけた持ち込み 5 本（verify-claude-mcp ほか）を修正。PR #2242

## 2026-10-17（#2239: 共有のテスト用レジストリへ固定のペイン番号で積むテスト 15 本と layout の 88888 を `unused_id` へ・番犬に `tabs` / `active_tab`）
- 注入で修正前 15 本中 12 本落ち → 修正後 0 本・製品コード不変。PR #2245

## 2026-10-17（#2243: `tako queue` の先頭が target dir 待ちの間、別の target dir の後ろが空き枠へ先に入る = 枠・メモリでは追い越さない・同じ dir は直列・1 件の待ちの追い越しは累計 同時数 × 2 まで）
- A/B `TAKO_2243_LEGACY=1`・`scripts/test-queue-head-of-line-2243.sh`・番犬 `issue2243_queue_head_of_line_watchdog`・待ちの理由を件ごとに（status `waiting[].reason`）。PR #2251

## 2026-10-17（#2238: UTF-8 でない settings.json / run-configs.json / ui.json も `LoadIssue::read_failed`（= `quarantine_raw`）でバイトのまま番号付きの退避へ。番犬の `SLOT_KNOWN` は空）
- A/B `TAKO_2238_LEGACY=1`・`scripts/test-nonutf8-quarantine-2238.sh`（修正前 11 NG → 修正後 46 PASS）・`tako migrate` は言語を `peek_lang_setting` で引く。PR #2253

## 2026-10-17（#2233: 黙った既定落としの残り = 引き継ぎの後任（専用プロファイルが無い / 読めないときも止めない）と `tako solo` は「立てて警告」、self / supervisor status / setup の表示は読むだけで `warnings`、PWA の「+ master」は起動結果に警告を表示）
- A/B `TAKO_2233_LEGACY=1`・結合 2 本 + 番犬の注入 8 → 23 種・e2e 115 passed・カタログ増 0 B。残りの経路は #2256。PR #2249

## 2026-10-17（#1891: check-health に tako mod の項目 = `claude_mod` 節（`tako mod --json` と同じキー・同じ値・同じ行の関数）・報告の無い理由をコマンド行と起動時の env で 1 つに決める `classify_no_report`）
- `tako mod` も背景の段で仕上げる（`OffloadJob::ModStatus`）・上限 8 ペイン・A/B `TAKO_1891_LEGACY=1`・実経路 48 PASS・カタログ +74 B。PR #2250
