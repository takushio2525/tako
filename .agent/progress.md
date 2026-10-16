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

## 2026-10-16（#2180: bin_resolve の「戻らないログインシェル」テストの間欠 = 真因は macOS の新しい実行ファイルの初回 exec の検査（約 0.3 秒・システム全体で直列））
- pid の確認を締め切りの判定から分け、偽の実行ファイルを空打ち・注入 `TAKO_2180_INJECT_SHELL_DELAY_MS`・修正後 25/25 緑。同型の結合テストは #2185。PR #2183

## 2026-10-16（#2036: `tako panel --help` の `--view` の説明を `PanelViewWire::summary` から生成し、md の候補を 7 値へ揃えて番犬 `issue2036_panel_view_docs_watchdog` で縛る）
- FR 番号の重複は前方一致による誤検出だった・カタログ増 0 B。PR #2184

## 2026-10-16（#2043: 番犬の固定バイト幅の切り出し 12 か所を `tests/common/byte_window.rs` へ寄せ、`issue2043_byte_window_watchdog` で直書きを縛る）
- 修正前 panic / 修正後 緑の A/B 3 本・製品ファイルの `#[cfg(test)]` に残る同型は #1790 へ。PR #2186

## 2026-10-16（#2181: ネット復帰の自動再開が、低いペインで締めの行より上へ押し出された接続エラーの停止を拾う）
- 見えている形が「締めの行 + 通知だけ」のときだけ画面の外の直近 200 行を足して同じ判定を通す。A/B `TAKO_2181_LEGACY=1`・`scripts/test-net-resume-2181.sh` 54 PASS。PR #2187

## 2026-10-16（#2003: concept.md の Non-goals から LSP 統合を外し、「コードエディタの位置づけ」節に 9/24 の方針変更・やり方・未着手を書いた）
- README / docs に同じ主張は無し。PR #2189

## 2026-10-16（#2182: 返答の配送で起票 master を起こす組み立て（ログインシェルの所在確認）を `OffloadJob::UserTaskLaunch` で UI の外へ・右パネルも同じ）
- 戻らないログインシェルで最中の IPC 新 45 ms / 旧 14,957 ms・`scripts/test-respond-master-launch-2182.sh` 13 PASS・A/B `TAKO_2182_LEGACY=1`。PR #2190

## 2026-10-16（#2185: issue2151_git_bin_oncelock の結合テストも初回 exec の検査の列で落ちる間欠を #2183 と同じ形で止めた）
- 8 本同時で修正前 24 本中 9 本落ち → 修正後 0 本・製品コード不変。PR #2191

## 2026-10-16（#1938: セルフテスト項目 154 が止まる真因は、右クリックで立った行の選択が入力欄を閉じても残り、次の Enter がツリーへ吸われていたこと）
- `close_inline_edit` で選択も畳む（製品 3 行）・セルフテストが `TAKO_APP_SELF_TEST_OK` まで完走・A/B `TAKO_1938_LEGACY=1`。PR #2194

## 2026-10-16（#2192 S0: usage 分析の設計書 `.agent/plans/2026-10-usage-analytics.md` = 区間按分・API 換算の重み・ログイン単位・`tako_sessions` の action・増分索引）
- 利用制限の % の履歴がどこにも無いことを実測で確認 → S1 を最優先に。台帳の主 Issue の不具合（#2199）を発見。PR #2195

## 2026-10-16（#2196: scroll_mirror のホイール直接注入 e2e の間欠 = 内側の stty 前に注入して tty と perl の echo で 2 回見えていた）
- アンカーを器のペインの `history_state` へ・列を個数まで比較・A/B `TAKO_2196_LEGACY=1` で CI と同一の画面。PR #2197

## 2026-10-16（#2193: ports の libproc と ps の照合テストの間欠の真因は、ps が改行などを `\012` へ表記し直すこと（時刻差ではない））
- libproc → ps → libproc で挟んで全件一致・表記し直しは手前の ASCII だけ比べる・テストだけの変更。PR #2198

## 2026-10-16（#2199: 委任台帳の `issue` とカタログの `issues` の先頭を、ラベルに書いた主の Issue にした）
- `extract_issues` を書いた順へ・A/B `TAKO_2199_LEGACY=1`・`scripts/test-ledger-primary-issue-2199.sh` 26 PASS・フィールド不変。PR #2207

## 2026-10-16（#2200: usage 分析 S1 = tako mod の報告から利用制限の観測を `usage/limits.jsonl` へ・ログインの鍵は accountUuid の塩付きハッシュ・`tako usage`）
- `scripts/test-usage-limits-2200.sh` 31 PASS・A/B `TAKO_2192_S1_LEGACY=1`・カタログ +91 B。10/17 00:50 に本番へ入れて記録開始を確認。PR #2208

## 2026-10-17（#1790 項目 1: 製品ファイルの理由の無い範囲添字 100 件を仕分け、番犬 `issue1790_byte_index_repo_watchdog` を `crates/*/src/` 全体へ）
- 判定は `range_index::sites` へ移して #2043 と共有。本番の経路で多バイトで落ちるものは 0 件・偽 LSP の `%xx` の panic を A/B 付きで修正。PR #2210

## 2026-10-17（#2209: ipc の fd 数えテストが 120 秒で落ちる真因は XNU の half-close の起こしの取りこぼし = 46.7 万往復に 1 回）
- EOF は 500 ms で区切り、来なければ完全に閉じて fd の数で見届ける。A/B `TAKO_2209_LEGACY=1`・製品コード不変。PR #2212

## 2026-10-17（#2201: usage 分析 S2 = 会話記録の使用量の索引 `<data_dir>/usage/` と `tako usage --by/--window`・MCP `tako_sessions` usage の by / window / account）
- 実データで参照集計とトークン完全一致（37 日）・初回 4.4 GB を 4.1 秒 / RSS 72 MB・カタログ +514 B。PR #2214

## 2026-10-17（#1790 項目 2: `truncate_path_middle` の末尾省略・`trim_detail`・`prompt_head` を固定値テストで固定してから `truncate_chars` へ寄せる）
- 総当たり A/B の差は「畳んだ後が max + 1 文字ちょうど」の 44 件だけ（同じ幅で原文を返す）・注入で固定値テスト 5 本が落ちる。PR #2215

## 2026-10-17（#2213: #2161 の番犬も XNU の half-close の起こしの取りこぼしで 120 秒落ちうる → FIN 待ちを 500 ms で区切り、新しい接続で往復をやり直す）
- A/B `TAKO_2213_LEGACY=1`（120 秒で落ちる）・製品コード不変。PR #2216

## 2026-10-17（#1838: setup の打ち切りテストで probe_timeouts が空の間欠 = 偽の子の初回 exec の検査が 30 秒の上限を食っていた）
- 偽の子を作った時点で空打ち・A/B `TAKO_1838_LEGACY=1` で修正前 10/10 落ち → 修正後 0/10・製品コード不変。PR #2218

## 2026-10-17（#2217: 右パネルの `git_feedback_text` の文字数の切り詰めを固定値テストで固定してから `truncate_chars(…, max + 1)` へ寄せる）
- 総当たり A/B 33,230 件の差は「行を落とさず 601 文字ちょうど」の 324 件だけ・行数の切り詰めは不変。PR #2219
