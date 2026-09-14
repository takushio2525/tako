# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-09-14）

- main = `cd6a3fa`（#1476 まで）。9/13〜9/14 の着地は 21 本（#812 / #1439 / #1441 / #1442 /
  #1445 / #1446 / #1447 / #1449 / #1450 B1〜B4 / #1451 / #1452 / #1453 / #1459 / #1466 /
  #1467 / #1472 A・B / #1473）。夜間リリース v0.8.13（9/14 05:00・prerelease）。1 件ずつは progress.md
- **/Applications は 9/14 08:12 ビルド（`a8238fe` 世代・v0.8.13）・GUI も 08:13 起動**。いまは
  main と同世代なので、次に製品挙動の PR が着地したら install + 再起動を通してから検収する
- **人待ちの正本は `tako todo`**（u-1〜u-19）。ここにも引き継ぎにも**列挙しない**（#1450 B4）
- **本番 remote は稼働中**（standalone tailscaled・`serve_ok=true`・`endpoint_kind=loopback-tcp`）。
  GUI 版 Tailscale と 2 系統同時なので `warnings` が 1 行出るのは既知（#1038）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーの
  メイン画面に窓を出さない）。起動前に `scripts/lib/virtual-display.sh ensure`、窓の位置・寸法は
  `TAKO_WINDOW_BOUNDS` / `tako window move|resize`（**AX 禁止** = #1442）。レシピは `.agent/conventions.md`
- **検収の status 読みは `/Applications/tako.app/Contents/MacOS/tako` で叩く**（PATH 先頭の
  `target` が stale だと新フィールドがキーごと無い = #432 と同じ罠）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）。**着地は 1 本ずつの直列**
  （`progress.md` が全 PR で衝突し CI 後に CONFLICTING へ落ちる = #1228 / #1352）。merge は
  `scripts/wait-pr-checks.sh` → `scripts/merge-pr.sh`（#1430 で worktree からも exit 0 になった）

## 直近の観点

- **#1450（ユーザー向けタスク）は B1〜B4 が全部着地**し、残りは実機確認だけ（`tako todo` の
  u-17 / u-19）。口は `tako todo` / 右パネル `tasks` ビュー / PWA `#/tasks` の 3 面あるが、
  中身はすべて B1 の `Request::UserTask` を素通しする 1 経路
- **MCP catalog の enum を正本からの生成へ寄せ始めた**（#1467）。移したのは実行時に読める
  10 種のうち 5 種で、残り（正本が非公開・列挙 API 無しで**縛れない 7 種**・**正本が無い 52 種**）は
  #1467 のコメントに一覧がある（[提案] 扱い・ユーザー判断待ち）
- **起動時ロードの予算（#1139）は `tako context-budget` が正**で CI の番犬が落とす。**リポ内は違反 0**。
  残る超過 3 件はすべて**リポ外の個人環境ファイル**（master system prompt の `append` =
  `local-rules.md` 11.9 KB → **分離はユーザー相談が必要** / `handoff/default.md` 374 行 /
  グローバル指示ファイル 26.6 KB）
- **Windows 実機**: 素のビルドのブロッカーだった #1133 は解消済み。`TAKO_APP_SELF_TEST_OK` までの
  完走は #1073（項目 105 / 143 の負荷依存）と #1278（ベースラインが 19 → 24 件）で追う。
  残りの実機依存は #1438（接続元検証の所有者ゲート）/ #1314（psmux の直書き）/ #971（serve が
  unix ソケット前提）。全体は #467 配下で、実測・A/B・引き継ぎ表は plan（下記）
- **リモート刷新（エピック #1059）は分割 A〜H が全部 main に着地**し、残りはユーザーの実機スマホ
  確認だけ（`tako todo` u-7）。Windows のファイル API（daemon → app の IPC が unix 実装のみ）は
  Windows 系の残バグとして別扱い

## 進行中（#1477・worker。merge は master の合図待ち）

- system prompt の取り分を **tako 18.5 KB / 追記 5.5 KB** へ分けた（base 実測 default 17,480 /
  codex 17,507 / fable 17,493 / takodev 18,450）。予算超過の `prompt_blocks.append` は
  `tako migrate run` が `<!-- tako:on-demand -->` を 1 行入れて分ける
- **この機械への反映はまだ**（`local-rules.md` 7,202 B のまま = master prompt 24,682 B で超過）。
  merge 後に `tako setup` か `tako migrate run` を 1 回打てば自動で整う

## 次の一手

- **自走で着手できる bug / 改善は 9/14 の棚卸しで出尽くした**。残る open は ①`tako todo` の
  ユーザー確認待ち ②Windows 実機依存（#1438 / #1314 / #1278 / #1073）③[提案] = ユーザー判断、
  のどれか。**次の着手先はユーザーへ相談してから決める**
- 相談リスト: #1405 / #1407・#1408（ファイルツリー）/ #1194・#1196・#1197（tmux 系）/ #1205 /
  #1228 + #1352（`progress.md` の衝突）/ #611 / #1291 / #1007 S1（LSP 基盤）/ #1374 / #1385 /
  #1316 の案 (a)（Issue は closed だが判断は保留）/ main の branch protection
- #1059 / #1081 は `tako todo`（u-7 / u-1）が済んだら close する
- #975 残: #987 / #988 / #990 / #991。関連 open は #985（codex / agy の limit 応答）
- #1001 残: C5 / C8〜C10（調査系）。C7 = #1426 まで着地済み
- 小口: `tasks_panel.rs:1548` の `caret_lines` を `TextField::caret()` へ（#1459 で範囲外扱い）

## 現フェーズで Read すべき設計書

- perf（#1001 の残り）を触る: `.agent/conventions.md`「効果を測る単体テストは実時間で比べない
  （#1167 / #1220）」+「番犬は 2 本立て」+「量を観る口の作り方」
- ファイルツリー（#1407 / #1408 / #1009）を触る: `.agent/conventions.md`「「ディレクトリか」の
  判定はリンクを辿る側に揃える（#1398）」+ `.agent/requirements.md` の FR-3 系（FR-3.1 / 3.12 / 3.24）
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
- Windows 実機: `.agent/plans/2026-08-windows-main-merge-wip.md`（ベースラインは 2026-09-02 に
  取り直した表・#1063 の DPI・#1091 / #935 / #936 / #1102 / #1127 の記録・`editbin` の迂回）
- SSH / リモート: `.agent/plans/2026-08-remote-folder.md`（#1040 / #1041 と A/B の env）+
  `.agent/plans/tako-remote-plan.md`（§10 = `remote serve` の不変条件）
- 動画・仮想ディスプレイ収録: `.agent/plans/2026-09-youtube-explainer.md`
- #1007 着手時 = `<data_dir>/orchestrator/research/2026-08-28-ide-editor-report.md`（リポ外）
- 対応マトリクスの判定を触るなら `crates/tako-core/src/platform/support.rs`
