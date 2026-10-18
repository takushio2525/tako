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

## 2026-10-18（#1842: 移動の実行を OS の上書きしない rename（`platform::fs_move::rename_noreplace` = renamex_np RENAME_EXCL / renameat2 RENAME_NOREPLACE / MoveFileExW フラグ 0）へ載せ替え、移したフォルダの recent.json の項目を移動先へ付け替える・A/B `TAKO_1842_LEGACY=1`・注入 `TAKO_1842_INJECT_RACE`）
- 単体の A/B で修正前は file_move.rs:714 で上書きを名指して落ちる・実経路 `scripts/test-move-noreplace-1842.sh` 31 PASS（CLI / MCP で隙間の同名を断る・recent の付け替え・LEGACY で上書き）・Windows CI は単体が根拠（実機は未実測）。PR #2311

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
