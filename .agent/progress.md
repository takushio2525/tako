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

## 2026-10-18（#2279: test-queue-1992 の ⑪ は #2254 で見込みが過去の記録から引かれるようになったのにテストが既定 3.5 GB を前提にしていた（製品は FR-2.43.9 どおり）・⑫ の受付の競走（BA）も直した）
- 修正前 ⑪ 6 回中 5 回 NG → 修正後 5/5 PASS=50・製品コード不変。PR #2282

## 2026-10-18（#1643: `isolated_gui_bins [feature …]` が毎回 cargo に任せて今の木から建てる = visual-test 付きを素で上書きせず・rebase 後の古いバイナリで走らない。呼び手 87 本の回避策を外し `SKIP_BUILD` は廃止・番犬 issue1490 に規則 21〜27）
- 実物の cargo で ① 上書き ② 古いバイナリの A/B・注入で 6 規則 FAILED。PR #2283

## 2026-10-18（#1621: `tako setup --answers` の sleep_guard でスリープ防止の段が 2 度出ていた → 依存チェック段の 1 か所が `[input]` の 1 行で出し、書き込みは黙る。番犬 `issue1621_setup_sleep_guard_once_watchdog`）
- 実経路 `scripts/test-setup-sleep-guard-once-1621.sh` 33 PASS を CI の macOS へ。PR #2281

## 2026-10-18（#1705: 自動命名の `spawn_claude` が非ゼロ終了でも stdout の EOF を `READ_GRACE` まで待ち、漏れたパイプ（#1748）で `Failed` に化けて再試行を落としていた → 終了コードだけで `NonZero`）
- 孫に stdout を握らせる再現テストで修正前 FAILED → 修正後 ok・queue 経由 20/20・カタログ +0 B。PR #2287

## 2026-10-18（#2284: 蓋閉じ継続の `probe_owner` が終わったプロセスの起動時刻（Windows はオブジェクトが残る間は読める）を生存の証拠にし #2277 を素通りしていた → `process::live_start_time_unix`（終了コードは `exited` の 1 実装）を通す）
- 注入 `TAKO_2284_INJECT_LINGERING` で lid.rs:1068 を再現・番犬 `lid_guard_ownership`・カタログ +0 B。PR #2288

## 2026-10-18（#2285: verify-setup-multiagent.sh のスタブが #1959 の `claude --version` を対話起動と数えて落ちていた = 製品の回帰ではない。スタブが版に即答し argv を記録・検出力の自己検査・CI の macOS へ登録・番犬 `issue2285_setup_multiagent_ci_watchdog`）
- 製品の版の問い合わせを引数なしへ注入すると `claude argv=[]` で落ちる（検出力を保持）。初回 setup で mod が入らない件は #2292。PR #2290
