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

## 2026-10-17（#2255: check-health の認証確認を上限つき（既定 10 秒・3 系統を並べて）にし、打ち切りは「認証: 不明（N 秒で応答なし）」）
- `setup_bootstrap::status_all_within` / `agent_probe::output_within`・A/B `TAKO_2255_LEGACY=1`・番犬 `issue2255_diagnostics_auth_probe_watchdog`（call_graph で collect から辿る）・実経路 41 PASS。残りの上限なし 3 か所は #1533 へ。PR #2259

## 2026-10-17（#2254: tako queue の実行中の予約をピーク後に返し、見込みを過去の記録 `resources/history.jsonl` から引く。諦めた件に押さえていた処理と量を出す）
- A/B `TAKO_2254_LEGACY=1`・`scripts/test-queue-reservation-2254.sh` PASS=14・番犬 `issue2254_queue_reservation_watchdog`・カタログ +0 B。PR #2258

## 2026-10-17（#2256: 黙った既定落としの残り = worker の spawn・解消エージェントは「立てて警告」（`load_caller_profile`・退避つき）、adopt の起動時プロファイル・guide・自動復帰の既定は読むだけで理由（`read_for_judgment`）。`guide local-rules` は「無い」ではなく「読めない」を返す）
- A/B `TAKO_2256_LEGACY=1`・結合 2 本 + 番犬の注入 23 → 40 種・カタログ増 0 B。PR #2260

## 2026-10-17（#2252: 夜間リリースに「検査だけ」の役 = `scripts/nightly-release.sh --role check`。2 台構成でリリースしない PC は mod の検査と footprint の見張りだけ・既定は全部）
- モック 75 PASS・旧版に当てると 31 FAIL。手元の Mac に `--role check` を設定済み。旧 Mac で検査が回っていない件は #2262。PR #2261

## 2026-10-17（#2248: 子のテストバイナリの目印を `split_once` で拾い子を `--test-threads=1` で起こす = issue2035 / issue2048・番犬 `issue2248_child_marker_watchdog`。registry のテストは共有 workers.yaml の自分のエントリだけを `dead_since` を外して比べる）
- `RUST_TEST_THREADS=1` で修正前 2 本落ち → 修正後 0・並列の書き手の注入でファイル全体の比較は落ちる・全体テスト並列 1 / 2 / 4 で緑。PR #2263

## 2026-10-17（#2262: 夜間が戻した共有ツリーの main を origin/main へ fast-forward で追従 = `follow_origin_main`。独自のコミット / untracked で ff できないときは触らず WARN と通知・A/B `TAKO_2262_LEGACY=1`）
- モック `scripts/test-nightly-restore-main-2262.sh` 79 PASS（修正前は 3 晩が 新 / 旧 / 新 で 22 FAIL）。戻した夜以外の追従は #2264。PR #2265

## 2026-10-17（#1532 / #1533: ログインシェルへの問い合わせを `platform::exe::ask_login_shell` の 1 実装（上限 5 秒・`TAKO_LOGIN_SHELL_TIMEOUT_SECS`・孫ごと畳む）へ寄せ、打ち切りを「確認できません」として setup / check-health / 依存の検出まで運ぶ）
- 番犬 `issue1533_login_shell_timeout_watchdog`・A/B `TAKO_1532_LEGACY=1`・実経路 62 PASS・関係スクリプト 17 本緑・カタログ増 0 B。提案は #2269。PR #2266

## 2026-10-17（#2264: リリースする役の夜間は共有ツリーの main を毎晩 origin/main へ fast-forward で追従 = `follow_main_nightly`。検査だけの役は動かさない・独自のコミット / untracked は触らず WARN と通知・A/B `TAKO_2264_LEGACY=1`）
- モック `scripts/test-nightly-follow-main-2264.sh` 83 PASS（修正前は 2 晩とも古い版で 18 FAIL）。PR #2268

## 2026-10-17（#1710: 自動復帰が前日の告知をいまから解き直していた = 告知を最初に見た時刻を layout.json の `limit_seen` に持ってそこから解き、追跡の再開始の時点で解けていたら「その時刻 + 2 分」で再開・後から届いた mod / codex の時刻も採る）
- A/B `TAKO_1710_LEGACY=1`・番犬 `issue1710_limit_reset_detected_base`・実経路 `scripts/test-limit-resume-restart-1710.sh` 22 PASS。PR #2270

## 2026-10-17（#1666: テストの固定名の一時 dir 22 か所と名前を引数で受ける `write_test_pdf`・autorename の `temp_dir_for` を `ScratchDir` へ。番犬 `issue1666_fixed_temp_dir_watchdog` が固定名と引数そのままの join を file:line で名指す）
- 修正前は 2 プロセス同時で 5 回とも片方が落ち → 修正後 10/10 緑・残骸 0。PR #2271

## 2026-10-17（#1633: pipefail の下の `echo/printf | grep -q` を 31 本・76 箇所で here-string / 配列の比較へ・番犬 `pipefailの下でechoとprintfの出力をgrep_qへパイプしない`（例外 `# tako:pipefail-ok`））
- 4 MB の注入で旧 100/100 偽 → 新 0/100（3.2 / 5）。残り（echo/printf 以外の左辺・`| head`）は #2272。PR #2273

## 2026-10-17（#1701 / #1674: CLI / MCP の検索で飛んだあと選択の写しもヒットへ揃える = `preview::search_jump` の 1 実装・番犬 `issue1701_search_selection_watchdog`。全置換のあとのカーソルは最後の置換の末尾・undo で置換前へ = FR-3.5）
- A/B `TAKO_1701_LEGACY=1` / `TAKO_1674_LEGACY=1`・単体 7 本・GUI の実経路は蓋閉じで未実測・カタログ +0 B。PR #2276

## 2026-10-17（#2272: pipefail の下の `X | grep -q`（左辺を問わず 62 箇所）と終了コードを見る `X | head`（20 箇所）+ eval 文字列 2 行をパイプ無しへ・番犬 `pipefailの下で早く抜ける読み手へパイプしない`）
- A/B 248 KB の左辺で旧 0/20・新 20/20（3.2 / 5.3）・例外宣言 0。既存の落ち test-queue-1992 ⑪ は #2279。PR #2278

## 2026-10-18（#2275: Windows の `pid_alive` が終了状態を見ず在籍表だけで読み、`wait()` 直後の終わったプロセスを「居る」と答えていた → 先に `GetExitCodeProcess` を問う `alive_unless_exited`・テストは `wait_gone` で待つ（同型 3 か所も））
- 注入 `TAKO_2275_INJECT_ALIVE_MS`・Windows CI 緑・カタログ +0 B。PR #2277
