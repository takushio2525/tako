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

## 2026-10-16（#1952: `.ino` の基板とポートをドロップダウン・`tako run-config --board/--port`・MCP で選ぶ = `board attach` が sketch.yaml へ書く・コア無しは案内だけ）
- 実経路 `scripts/test-run-board-1952.sh` 28 PASS（3 経路とも arduino-cli の直書きとバイト一致・`core install` 0 回）・カタログ +282 B。PR #2144

## 2026-10-16（#2025: 上端ドラッグのオートスクロールが送れる履歴なし / 最上端で選択を (0, 0) へ縮めていた → `drag_scroll_step` で送れないときは端を動かさない）
- A/B `TAKO_2025_LEGACY=1`・番犬 `issue2025_tick_が判定を通す_watchdog`・実クリック `scripts/test-drag-autoscroll-2025.sh`。下端の対称は #2153。PR #2147

## 2026-10-16（#2152: ファイルツリーで `.env.local` / `.env.production` 等の `.env.*` を `.env` と同じ設定アイコンに）
- 特殊ファイル名の表に 1 行・最後の拡張子が表にある `.env.ts` 等と `.envrc` は従来どおり。PR #2156

## 2026-10-16（#2150: 変更履歴の行の diff の展開（クリック / `--expand`）を `OffloadJob::PreviewChangelogDiff` へ・#2014 と同じ上限）
- A/B `TAKO_2150_LEGACY=1`・番犬 14 節（注入 12 通り）・`scripts/test-changelog-expand-2150.sh` cli 16 / visual 2 PASS・カタログ増 0 B。PR #2160

## 2026-10-16（#2153: 最下部ではドラッグ選択のオートスクロールで選択の端を行末へ寄せない = #2025 の下端版）
- `drag_scroll_step` に下端の Hold・A/B `TAKO_2153_LEGACY=1`・番犬 `issue2153_tick_が下端で寄せない_watchdog`。実画面は蓋閉じで未実測。PR #2158

## 2026-10-16（#2157: ネット復帰の再開が、エラー直後の通知行が挟まった停止と、同じ形の止まり直しを取りこぼさない = ユーザー依頼）
- 締めの行より下の `⏺` を読み飛ばし、会話が進んだら・届いたら鍵を捨てる。A/B `TAKO_2157_LEGACY=1`・`scripts/test-net-resume-2157.sh` 38 PASS。PR #2165
- 10/16 12:40 の 22 分の DNS 断で、止まった全ペインに自動で続きが届いた（手で再開 0 本）

## 2026-10-16（#1734: Code Runner の実行先（reuse / new）を実行設定に覚える = `tako run-config --target`・MCP `tako_run_defaults` の `target`）
- 一回限りの `new_pane` > 保存値 > 既定・serde default で移行なし。カタログ +159 B・`scripts/test-run-target-1734.sh` 32 PASS。PR #2162

## 2026-10-16（#2163 / #2161 / #2164: CI の間欠 3 つ = LSP 取り消しの数え・ipc の fd テストの EINVAL・git 履歴の date_relative）
- いずれも注入で原因を確定してテストだけ直した（`wait_received` で待つ・上限を shutdown の前へ・コミットを過去の固定時刻に）。PR #2167 / #2168 / #2170

## 2026-10-16（#2051: md の表の `|` を `\|` で揃えて番犬で縛る + visual-test の `fail()` の経路でも節の一時ファイルを消す）
- 番犬 `issue2051_md_table_columns_watchdog`・`remove_on_fail` の登録 93 か所・A/B `TAKO_2051_LEGACY=1`。PR #2154

## 2026-10-16（#2159: tako mod の会話のカードに [ run ] / [ copy ] = クリック 1 回で実行（ユーザー依頼）・二重押しは止める）
- 手順 新 1 クリック / 旧 3 打（A/B `TAKO_2159_LEGACY=1`）・`scripts/test-mod-card-click-2159.sh` 42 PASS・カタログ増 0 B。PR #2166

## 2026-10-16（#2151: git_bin() / tmux_bin() の初回の解決に全体 5 秒の締め切り = 戻らないログインシェルで止まらない）
- `tako_core::bin_resolve`・打ち切ったら素の名前へ・理由を persist.log へ。A/B `TAKO_2151_LEGACY=1`・修正後 2.05 秒 / 修正前は戻らない。PR #2171

## 2026-10-16（#2169: CLI の IPC で上限の入れ直しが EINVAL のとき、届いた応答を待たない読みで読む）
- 未着なら待たずに元の誤り。修正前は 4 本が os error 22・番犬 `issue2169_ipc_einval_watchdog`。PR #2172

## 2026-10-16（#2048: 隔離 GUI では言語サーバを自動で起こさず・取らない = `TAKO_LSP_AUTOSTART=1` / `TAKO_LSP_AUTO_FETCH=1` で従来どおり）
- 呼び手 22 本へ env・A/B `TAKO_2048_LEGACY=1`・`scripts/test-lsp-isolated-2048.sh` 26 PASS。既存の `test-lsp-1678.sh` ③ の失敗は #2174。PR #2173

## 2026-10-16（#2174: test-lsp-1678.sh の ③ で servers の `cached`（呼んだ順の印）の値だけを比べず、キーと型・他の全フィールドは比べる）
- 修正前 32 PASS 1 FAIL → 修正後 33 PASS・id / installed の取り違えの注入は ③ で落ちる。PR #2176

## 2026-10-16（#1858: recover --apply がバックアップ後のタブを合流・復元時に器の無い master / worker のペインだけを刈り込む）
- 判断の正本 `tako_control::layout_reconcile`・A/B `TAKO_1858_LEGACY=1`・実経路 `scripts/test-recover-merge-prune-1858.sh`。PR #2178

## 2026-10-16（#2042: 番犬 issue1984 の 15 節 = UI スレッドの入口から呼び出しを辿り、別名の関数で包んだ子プロセスの口も名指し）
- 共有部品 `tests/common/call_graph.rs`・製品コード不変・誤検知 0・注入 (G1)〜(G5)。見つかった返答配送の master 起動は #2182。PR #2179

## 2026-10-16（#2035: rust-analyzer へ `cargo.targetDir = true` を既定で渡し、利用者の cargo build が target のロック待ちにならない）
- 上書きは `tako lsp options` / MCP `tako_lsp_server` action=options・実測 `scripts/test-lsp-target-dir-2035.sh` 22 PASS・A/B `TAKO_2035_LEGACY=1`・カタログ +241 B。PR #2177

## 2026-10-16（#2180: bin_resolve の「戻らないログインシェル」テストの間欠 = 真因は macOS の新しい実行ファイルの初回 exec の検査（約 0.3 秒・システム全体で直列））
- pid の確認を締め切りの判定から分け、偽の実行ファイルを空打ち・注入 `TAKO_2180_INJECT_SHELL_DELAY_MS`・修正後 25/25 緑。同型の結合テストは #2185。PR #2183

## 2026-10-16（#2036: `tako panel --help` の `--view` の説明を `PanelViewWire::summary` から生成し、md の候補を 7 値へ揃えて番犬 `issue2036_panel_view_docs_watchdog` で縛る）
- FR 番号の重複は前方一致による誤検出だった・カタログ増 0 B。PR #2184

## 2026-10-16（#2043: 番犬の固定バイト幅の切り出し 12 か所を `tests/common/byte_window.rs` へ寄せ、`issue2043_byte_window_watchdog` で直書きを縛る）
- 修正前 panic / 修正後 緑の A/B 3 本・製品ファイルの `#[cfg(test)]` に残る同型は #1790 へ。PR #2186
