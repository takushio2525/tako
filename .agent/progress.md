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

## 2026-10-18（#1668: Code Runner の変数展開を実行ペインの方言で引用 = `ShellDialect::quote_fragment`（PowerShell は特別な文字の前にだけバッククオート。単引用符は `-o 'x'.exe` がメンバー参照になるので不採用・about_Parsing））
- pwsh 7 の構文木で Windows の展開 8,352 件: 修正前 3,239 件食い違い → 0 件・macOS は `tako:shell: pwsh` 以外不変・番犬 `issue1668_runner_quote_dialect_watchdog`・カタログ -18 B。残りの経路は #2286。PR #2289

## 2026-10-18（#1638: worker の宣言 / 実際のモデルを分けて持つ = transcript の最初の応答の model を `model_actual` に・食い違いは workers / worker_status の warnings と persist.log・ledger stats は実際のモデルで・codex / agy は matrix で degraded）
- A/B `TAKO_1638_LEGACY=1`・実経路 17 PASS・カタログ +479 B。PR #2291

## 2026-10-18（#2292: 新品の HOME の 1 回目の setup で tako mod が入らず、2 回目に入っても「変更なし」と出ていた → mod の段を `~/.claude` を作る指示ファイルの段の後ろへ移し、書いた・外した dir を最終サマリへ積む）
- 実経路 `scripts/test-setup-mod-first-run-2292.sh` 40 PASS（修正前 10 FAIL）を CI へ・番犬 `issue2292_setup_mod_first_run_watchdog`。PR #2294

## 2026-10-18（#1805: CLI の経路で走るライブラリ側の出力もパイプの切断で panic させない = 書き込み口を `tako_core::stdio` の 1 実装へ寄せ、tako-core / tako-control / tako-cli lib の根でも差し替え・規則は `tako` / `tako-mcp` の入口だけが採る（GUI は std と同じ）。`mcp serve` はクライアントが stdout を閉じたら静かに 0）
- 修正前 gate check 101・mcp serve の gate check 101 / 切断 1 → 修正後 0。番犬 `ライブラリ側でstdの出力マクロを名指さない`・宣言順を 4 つの根へ。カタログ +0 B

## 2026-10-18（#2286: Code Runner の変数展開以外の引用 3 経路 = `tako:shell` の外側・プロジェクト既定・宣言の `tako:cwd` を実行ペインの方言の 1 実装へ。番犬 `issue2286_runner_outer_quote_dialect_watchdog`）
- 全件 1590 行の前後突き合わせで動いていた行が壊れた行 0・macOS で変わった 170 行はすべて修正前に壊れていた行・カタログ +0 B。PR #2297

## 2026-10-18（#1847: 狭いペインの ENOTFOUND の停止を status が api_error と読む = `detect_worker_error` の段 3' が `net_stop::detect` を通す・背の高い AskUserQuestion を経路 5 `numbered_above_scan` で拾い worker_status はダイアログを画面全体で判定）
- 実物の claude 2.1.280 / 2.1.301 の 32 / 60 桁の fixture・A/B `TAKO_1847_LEGACY=1`・隔離 GUI の e2e 新旧 11 PASS・カタログ +0 B

## 2026-10-18（#1851: THIRD-PARTY-LICENSES.md の作り直しを `scripts/third-party-licenses.sh` の 1 本へ（cargo-about 0.9.2 を `--frozen`・CRLF を LF へ）し、CI の `licenses` ジョブ（ubuntu）が `--check` で依存とのずれを落とす。main で抜けていた LSP の依存 4 クレートを作り直しで足した）
- A/B: 依存を足す → 名指しで赤 → 作り直して緑・消す → 余分を名指し。モック 35 PASS・配線 95 PASS。PR #2300

## 2026-10-18（#1850: `release.sh --check-assets` が公開済み Release の zip（両 OS）を取得し、同梱表 `scripts/lib/license-bundle.txt`（番犬と共有の 1 か所）の全部が入りタグのファイルとバイト一致するかを見る）
- 実物 v0.8.25 は Windows の zip の 2 本の欠けで exit 1・v0.8.37 は exit 0・モック 131 PASS（3.2 / 5.3。main の release.sh では 41 FAIL）・番犬に直書きの検出。PR #2303

## 2026-10-18（#1817: `virtual-display.sh ensure` が器に作らせる・繋がせるのは `tako-vd` の完全一致だけ = `vd_create_verdict` を器に触る前に。`bounds` / `status` / `move-window` も `vd_virtual_verdict` を通し物理画面に座標・使用可を返さない（3・器を起こさない））
- 修正前は存在しない名前・大文字違い・蓋閉じの内蔵の名前で作成 + 接続して rc=0 → 修正後 rc=3・作成 0。注入 7 通りで番犬が file:line を名指し・モック 243 PASS（3.2 / 5.3）

## 2026-10-18（#2299: Code Runner の宣言 `tako:cwd` の値全体を囲む二重引用符も `strip_quotes` で外し、中の逃がしを実行ペインの方言の規則で戻す = `posix_double_quoted` / `ps_double_quoted`。番犬 `issue2286_runner_outer_quote_dialect_watchdog` に規則と行）
- 1590 行の表で変わったのは `"${fileDir}"` の 40 行だけ（すべて 壊れ → 直る）・隔離 GUI の実物で修正前は `cwd が存在しない`・`/bin/sh` と PowerShell の実物と一致・カタログ +0 B。PR #2310
