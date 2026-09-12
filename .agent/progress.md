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

## 2026-09-12（#1420: 番犬の走査範囲が途中の `#[cfg(test)]` で切れるのを直した）
- 範囲取りを「最初の `#[cfg(test)]` で切る」から「**テスト領域だけ空白へ潰す**」1 実装（`common/production_range.rs`）へ。バイト長と行番号が保たれるので `file:line` がずれず、途中のテスト用ヘルパでも本番コードが消えない。下限（既定 30%）を割ったら潰した先頭の領域を名指して落ちる
- 実測 A/B: `remote.rs` の検査対象の**後ろ**へ 4 行のヘルパを注入すると legacy は 5,903 → 3,124 行（70.7% → 37.1%）へ縮んで **7 本とも緑のまま**・**手前**へ注入すると「改名したら番犬も直す」で 6 本 FAILED（誤診）。新は両方 8 本緑・本番 70.7% を維持。縮める注入では `remote.rs:1505 走査範囲が全体の 18.3% まで縮んだ（下限 30.0%）` で FAILED
- 棚卸しで 8 本を寄せた（`platform_parity` は 4 クレートの src を丸ごと走査していて `orchestrator/mod.rs` を 1.5%・`mcp/mod.rs` を 10.5% しか見ていなかった）。番犬 `issue1420_production_range_watchdog` 12 本 + 実ファイル 254 本の不変条件。寄せられない 4 件（製品コード内の番犬）は `KNOWN_COARSE` へ件数つきで宣言。**テストのみ = install 不要**

## 2026-09-12（#1402: ツリーが上限超過ぶんを黙って捨てるのを直した）
- `read_dir_sorted` の戻り値を `DirListing`（entries + 切り詰め + 読み取り失敗）へ広げ、切り詰めと「読めない」を #1398 の器（`RowNote` / `render_note_row`）の行として出した。判断は CLI の `tree git-status` と同じ 1 実装 `tako_core::sidebar::Truncation`（`total > limit` の手書きを両側から排除）
- A/B（`TAKO_1402_LEGACY=1`）: legacy = 560 件で **行 501 / note 0**（Issue の実測そのもの）・権限なしが空と同じ → 新 = 行 502 で `Truncated{shown:500,total:560}`・読めないは Error 行。番犬 3 本（注入 6 通りで file:line 名指し）+ 単体 8 本
- 次: 上限そのもの（500）の見直しは別 Issue（暴走防止として妥当なことは perf 実測済み）

## 2026-09-12（#1425: save_layout の変化検出を JSON 直列化の前へ出した）
- 判定材料を「組んでから比べる」から「組む前の 128bit キー」へ（`layout::change_key` + 借用版 `PaneMetaRef`）。UI 側の付帯情報は `LayoutExtras` に 1 度だけ組んでキーと穴埋めが同じ値を読む。`collapsed` は HashSet 由来なので昇順に整える。載せ忘れは 3 段で止める（網羅的分解でコンパイル / `CHANGE_KEY_FIELDS` × #916 指紋の番犬 / 連続 30 スキップで必ず突き合わせる保険）
- 実測 A/B（23 ペイン・同一手順の隔離 GUI）: 70 秒アイドルで新 = 37 呼び出し中 **35 スキップ・capture 2 回**、legacy（`TAKO_1425_LEGACY=1`）= **capture 63 回**。`written` は両腕とも 26 で同一（保存結果は不変）。確保は 22 ペインで **319 回 / 29893 バイト → 0 回 / 0 バイト**（0.349ms → 0.065ms）
- 番犬 6 本 + 確保計測 1 本（注入 6 通りで file:line 名指し FAILED）。操作 13 種で保存を実測・再起動復元 23 ペイン成功。内訳は `tako persist` の `save_layout` で読める。**install 要**

## 2026-09-12（docs: activeContext を実態へ同期）
- 「現在の対象」が 9/8 / main `4d84695` で止まり、close 済みの Issue 12 件（#1167 / #995 / #771 / #1122 / #1114 / #1124 / #1013 / #1015 / #1022 / #1030 / #1033 / #1034 / #1035）を open として案内していたのを、gh の state と git log で全行照合して書き直した
- 直した主な食い違い: install 世代（9/7 19:52 / v0.8.7 → **9/12 09:59 / `1f4eb3f`**）・「#1167 着手中」（close 済み）・Windows のブロッカー（#1133 → #1073 / #1278）・予算の残超過（4 件 → **3 件・全部リポ外**）。無言の失敗系統（#1399 / #1417 → #1422 / #1432）とファイルツリー系の棚卸しを追加し、直列着地の理由（#1228 / #1352）を明記
- 79 行（予算 80）・`tako context-budget check` で activeContext の違反 0・docs のみなので **install 不要**

## 2026-09-12（#1422: 右パネルの tmux 復元と UI の eprintln! 5 か所を通知欄 / 診断へ寄せた）
- #1417 の番犬は窓の中に `Err(` が**在るだけ**で「扱った」と数えるので、`right_panel.rs:2022` は結果を `if opened.is_ok()` でしか見ていないのに**無関係な** `if let Err(e) = attach_pending_sessions(..)` で緑だった（実測で確認）。判定を**結果の束縛名の追跡**へ寄せ、`eprintln!` の検査を `sidebar.rs` 限定から全 UI モジュール（テストモジュールは除外・スコープ外は `KNOWN_EPRINTLN` の件数で段階導入）へ広げた
- 6 か所を振り分け: ユーザー操作 5 件（tmux 復元 / 復元ペインの PTY 起動 / バックグラウンド復帰 / コードのコピー / Code Runner）は共有の通知欄 + persist.log、背景処理 1 件（PDF 再ラスタライズ）は `log_ui_failure` で診断だけ。A/B の軸を画面（`NoticeArea`）から Issue（`NoticeArm`）へ分離（同じ画面に #1417 と #1422 の通知が同居し、画面で env を選ぶと互いの回帰を隠すため）
- 隔離 GUI（tako-vd）項目 84d の A/B: 新 = `restore=Some("セッションの復元 に失敗しました…")` / `unshelve=Some(…)` / `copy=Some(…)` / `bg_silent=true bg_lines=3` / `ok_silent=true` で完走（FAILED 0）、legacy（`TAKO_1422_LEGACY=1`）は全部 `None` で FAILED かつ #1417 の診断行は `legacy=false` のまま（A/B が互いを隠さない）。注入 10 通りが file:line 名指し FAILED・workspace 4538 passed 0 failed

## 2026-09-12（#1430: merge-pr.sh の終了コードを実測で言い切り、ローカル head も自分で消した）
- 実測で Issue の推測を否定: 現行スクリプトは worktree 事故でも**既に exit 0**（#1347 で解消済み）。実在した誤読の元は「`failed to run git: fatal:` + `警告: merge は済んだが gh が 1 で終わった`」の 2 行と、**すでに MERGED の PR への再実行が 1** を返すこと（使い捨て private リポ + 実 gh 2.88.1 で PR 7 本を実 merge して確定）
- gh の非ゼロを「merge の失敗ではない」注記へ、最終行を `merge 成立: PR #N は MERGED（url）/ 終了コード 0` へ、MERGED の再実行を冪等 0 へ（CLOSED は 1）。ローカル head は `git branch -D`、作業ツリーが握るときだけ外し方を名指しして残す（`worktree_holding_branch` / `delete_local_head_branch` の 1 実装）
- モック Test 24〜29（ローカルブランチと作業ツリーだけ実 git。25b = 本体の作業ツリーには「畳め」と言わない）で 137 PASS 0 FAIL。注入 4 通り（後始末を外す / 再実行を refuse へ戻す / 旧警告文へ戻す / 作業ツリー検出を殺す）すべて FAILED。A/B = `TAKO_1430_LEGACY=1`。**install 不要**（scripts + docs のみ）

## 2026-09-12（#1404: ツリーのスキャン対象の重複と、消えたルート配下の読み続けを直した）
- 組み立てを「`roots` の順 → `expanded` の未出（名前順）」の 1 実装へ（`Vec::dedup` は隣接しか落とさず `expanded` は `HashSet` = 順序が任意なので、全ルートが 2 回ずつ `read_dir` されていた）。外れたルートは `forget_under` で**配下ごと**忘れる（生きているルートの下は巻き込まない = 入れ子のルート）。事実と違う注釈も実態へ
- 隔離 GUI（tako-vd）の項目 135 を「ユニーク化 + 展開ディレクトリの存在」へ書き換え: 新 = 完走 / `TAKO_1404_LEGACY=1` = `targets=5 uniq=3` で FAILED / 展開 0 件の注入 = `targets=2 uniq=2 extra=[]` で FAILED（旧 assert `targets.len() > git_roots.len()` は重複だけで常に真 = 検出力ゼロだった）
- 単体 5 本（legacy で 4 本 FAILED・`rows` 不変の 1 本は両腕で緑）+ 番犬 3 本（注入 6 通りで file:line 名指し）。workspace 4579 passed 0 failed・clippy 両宇宙 0・check-windows error 0。**install 要**

## 2026-09-12（#1426: 裏タブの寸法合わせを毎フレームから key + 間引きへ寄せた）
- `sync_offscreen_pane_sizes` は render から毎フレーム通るのに、当て直す中身（`offscreen_areas`）が既に「key + 2 秒」で回っていたので**材料が同じあいだは同じ答えを出し直していた**。当て直す側も同じ単位へ寄せ、キーは `OffscreenAreaKey`（1 実装 `offscreen_area_key` に集約）+ 既定セル寸法（#647 の再発防止）+ 表示中ペイン数 + ペイン単位ズームの指紋。間隔は `OFFSCREEN_REFRESH_INTERVAL` の 1 定数を両者が見る
- 実測（隔離 GUI の grid-bench・22 ペイン / 表示 4・3000 フレーム）: 596〜776 ns/frame（`render` の 5.88〜6.28%・走査 82 比較 + 18 ペイン当て直し）→ **36〜38 ns/frame**（0.38〜0.41%・走査 0 / 当て直し 0）。同一バイナリの `TAKO_1426_LEGACY=1` は 645〜690 ns で旧挙動を再現。#932 の flicker ラウンドは `late_resize=false` で緑、既存 A/B（`TAKO_932_NO_OFFSCREEN_GEOMETRY=1`）では `late_resize=true` で落ちる = 検出力あり
- 単体 7 本 + 番犬 `issue1426_offscreen_sync_watchdog` 8 本（注入 8 通りで `main.rs:16126` / `:16158` / `:16238` / `:16245` / `:1330` / `:1344` を file:line 名指し）。範囲取りは #1420 の `production_range` の 1 実装を通す。workspace 4551 passed 0 failed・clippy 両宇宙 0・check-windows error 0・隔離セルフテスト完走。**install 要**
