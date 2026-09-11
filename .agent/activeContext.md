# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-09-12）

- main = `7b71dc6`。9/11〜9/12 の 2 日で 73 本が着地（無言の失敗を通知欄へ = #1399 / #1417 /
  ssh config の `Match` 混入 = #1400 / remote の stale stop = #1401 と HTTP 並列化 = #1403 /
  ツリーの切り捨てとリンク = #1402 / #1398 / 待ちの移送 = #1375 / 番犬の走査範囲 = #1420）。
  1 件ずつは progress.md
- **/Applications は 9/12 09:59 ビルド（`1f4eb3f` 世代・v0.8.11）・GUI も 09:59 起動**。それ以降の
  main は **#1425（C6 の `save_layout` = 製品挙動）**なので、次の製品挙動 PR とまとめて install + 再起動する
- **ユーザー目視待ち**: ツリー git 色（#1009）/ Finder D&D の実マウス（#1043）/ Dock ピン留めは
  次回の更新で確認（#1042）/ #1059 のスマホ 4 項目 / #1081 v3 の試聴（音声は数値のみ検査）
- **本番 remote は稼働中**（standalone tailscaled・`serve_ok=true`・`endpoint_kind=loopback-tcp`）。
  GUI 版 Tailscale と 2 系統同時なので `warnings` が 1 行出るのは既知（#1038）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーの
  メイン画面に窓を出さない）。起動前に `scripts/lib/virtual-display.sh ensure`、
  レシピは `.agent/conventions.md`（`-u TERM -u COLORTERM` は不要になった = #946）
- **検収の status 読みは `/Applications/tako.app/Contents/MacOS/tako` で叩く**（PATH 先頭の
  `~/dev/tako/target` が stale だと新フィールドがキーごと無い = #432 と同じ罠）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）。**着地は 1 本ずつの直列**
  （`progress.md` が全 PR で衝突し CI 後に CONFLICTING へ落ちる = #1228 / #1352）

## 直近の観点

- **「無言で失敗する UI」を通知欄 + persist.log へ寄せる系統が進行中**。#1399（ツリー 13 か所）と
  #1417（右パネル / プレビュー 3 件）が着地し、出し口は **`notify_ui_failure(area, ..)` の 1 実装**
  （画面の別は `sidebar::NoticeArea` で区別）。残りが #1422（tmux 復元 + `eprintln!` 5 か所）→
  #1432（UI の `eprintln!` 10 件）。番犬の走査範囲は #1420 で `production_range` へ寄せ済み
- **セルフテストの「固定予算 → 状態待ち」移送は完了**。#1375 で `KNOWN_FIXED_CLI_WAITS` が空になり、
  高負荷フレークの open だった #1167 / #995 / #771 / #1122 / #1114 / #1124 は**全部 close 済み**。
  以後は新規に固定予算を書かない（作法は下記の設計書）
- **起動時ロードの予算（#1139）は `tako context-budget` が正**で CI の番犬が落とす。**リポ内は違反 0**。
  残る超過 3 件はすべて**リポ外の個人環境ファイル**（master system prompt の `append` =
  `local-rules.md` 11.7 KB → **分離はユーザー相談が必要** / `handoff/default.md` 374 行 /
  `~/.claude/CLAUDE.md` 26.6 KB）
- **Windows 実機**: 素のビルドのブロッカーだった #1133（項目 80 のスタックオーバーフロー）は**解消**
  （受け入れ条件 2/2）。`TAKO_APP_SELF_TEST_OK` までの完走は #1073（項目 105 / 143 の負荷依存）と
  #1278（ベースラインが 19 → 24 件に増えていた）で追う。#971 は実装済み・実機実測待ち。
  Windows 全体は #467 配下。実測・A/B・引き継ぎ表は plan（下記）
- **リモート刷新（エピック #1059）は分割 A〜H が全部 main に着地**。残りはユーザーの実機スマホ
  確認だけ（①「Claude で開く」→ アプリ ②「+ master」でタブ + 起動 ③ファイルの閲覧・編集・保存
  ④SSH の切り替え / 新規接続）。Windows のファイル API（daemon → app の IPC が unix 実装のみ）は
  Windows 系の残バグとして別扱い

## 次の一手

- **PR 作成済み・着地待ちの直列**: #1422（PR #1431）→ #1430（PR #1433）。`progress.md` で必ず
  CONFLICTING になるので、合図が来たら rebase → `scripts/wait-pr-checks.sh` →
  `scripts/merge-pr.sh`（#1430 の通り worktree からは merge 成功でも exit 1 = 成否は `gh` で見る）
- **作業中**: #1426（#1001 C7: `sync_offscreen_pane_sizes` を key + 間引きへ）/
  #1404（`refresh_targets` の `dedup()` が効かず毎回 2 回 `read_dir` する）
- **キュー**: #1432（UI の `eprintln!` 10 件 = #1422 の `KNOWN_EPRINTLN` の残り）
- #1059 / #1081 はユーザー確認が済んだら close する
- **ファイルツリー系の棚卸しが溜まっている**: #1404 / #1407（MCP / CLI から操作できない =
  設計原則 5 の判断）/ #1408（テストの穴 4 件）/ #1009（git 色）/ #1010（SSH の進行状況）
- #975 残: #987 / #988 / #990 / #991（#986 / #989 / #992 は着地）。関連 open は #985（limit 応答）
- #1001 残: C7（#1426・作業中）・C5 / C8〜C10（調査系）。**C2 / C3 = #1301 / C4 = #1012 / C6 = #1425**
- #1007 IDE 化: S0 の一部（#1016）済み → S1（LSP 基盤）

## 現フェーズで Read すべき設計書

- perf（#1426 C7）を触る: `.agent/conventions.md`「効果を測る単体テストは実時間で
  比べない（#1167 / #1220）」+「番犬は 2 本立て」+「量を観る口の作り方」。計測の正本は
  `<data_dir>/orchestrator/research/2026-08-28-perf-profile-report.md`（リポ外）
- ファイルツリー（#1404 / #1407 / #1408）を触る: `.agent/conventions.md`「「ディレクトリか」の
  判定はリンクを辿る側に揃える（#1398）」+ `.agent/requirements.md` の FR-3 系（FR-3.1 / 3.12 / 3.24）
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
- Windows 実機: `.agent/plans/2026-08-windows-main-merge-wip.md`（ベースラインは 2026-09-02 に
  取り直した表・#1063 の DPI・#1091 / #935 / #936 / #1102 / #1127 の記録・`editbin` の迂回）
- SSH / リモート: `.agent/plans/2026-08-remote-folder.md`（#1040 / #1041 と A/B の env）+
  `.agent/plans/tako-remote-plan.md`（§10 = `remote serve` の不変条件）+
  `research/2026-09-01-remote-renewal-claude-official.md`（#1059）
- 動画・仮想ディスプレイ収録: `.agent/plans/2026-09-youtube-explainer.md`
- #1007 着手時 = `<data_dir>/orchestrator/research/2026-08-28-ide-editor-report.md`（リポ外）
- 対応マトリクスの判定を触るなら `crates/tako-core/src/platform/support.rs`
