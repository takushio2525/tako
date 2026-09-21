# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-09-21）

- main = `b887cb0`（v0.8.16 夜間リリース。製品コードは `e6c402c` = #1481 まで）。9/18 の着地は
  #1483（docs ドメイン移行 #1482）/ #1484（#1481 Web ビュー後の打鍵）。1 件ずつは progress.md
- **/Applications は 9/18 13:58 ビルド（`e6c402c` 世代）・GUI は 9/21 10:46 に Mac 再起動で起動**
  （persist.log「Claude resume 13 = 全部役割つき」）。main と同世代なので install 待ちなし
- **人待ちの正本は `tako todo`**（u-1〜u-23）。ここにも引き継ぎにも**列挙しない**（#1450 B4）
- **本番 remote は Mac 再起動で落ちたままだった**（= #1485）。9/21 11:00 に手で立て直し
  `serve_ok=true`・`endpoint_kind=loopback-tcp`。GUI 版 Tailscale と 2 系統同時なので `warnings` が
  1 行出るのは既知（#1038）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーの
  メイン画面に窓を出さない）。起動前に `scripts/lib/virtual-display.sh ensure`、窓の位置・寸法は
  `TAKO_WINDOW_BOUNDS` / `tako window move|resize`（**AX 禁止** = #1442）。レシピは `.agent/conventions.md`
- **検収の status 読みは `/Applications/tako.app/Contents/MacOS/tako` で叩く**（PATH 先頭の
  `target` が stale だと新フィールドがキーごと無い = #432 と同じ罠）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）。**着地は 1 本ずつの直列**
  （`progress.md` が全 PR で衝突し CI 後に CONFLICTING へ落ちる = #1228 / #1352）。merge は
  `scripts/wait-pr-checks.sh` → `scripts/merge-pr.sh`（#1430 で worktree からも exit 0 になった）

## 直近の観点

- **#1485（remote daemon が再起動で戻らない）に着手**: `tako remote start` が「起動していた」を
  `<data_dir>/remote/` に永続し、GUI 起動時（persist 復元のあと）に `spawn_daemon()` で戻す。
  tailscaled が遅れて上がるのでバックオフ・**無言にしない**（persist.log + リモートチップ +
  `remote status` の `desired` / `last_autostart`）。#1446 と同型（プロセスの寿命しか持たない記憶を永続へ）
- **#1481 は着地済み**（真因 = `focus_parent()` が hide 分岐にしか無かった。webview.rs の 1 実装へ）。
  実マウス確認は `tako todo` u-23（前提「修正済みバイナリで起動」は 9/21 に満たした）
- **MCP catalog の enum を正本からの生成へ寄せ始めた**（#1467）。移したのは実行時に読める
  10 種のうち 5 種で、残り（縛れない 7 種・正本が無い 52 種）は #1467 のコメントに一覧（[提案] 扱い）
- **起動時ロードの予算（#1139）は `tako context-budget` が正**で CI の番犬が落とす。**リポ内は違反 0**。
  リポ外の超過は `handoff/default.md` 374 行のみ（default master の管轄）。takodev は
  #1477 の `<!-- tako:on-demand -->` 分割が本番に適用済みで違反 0
- **Windows 実機**: 素のビルドのブロッカーだった #1133 は解消済み。`TAKO_APP_SELF_TEST_OK` までの
  完走は #1073（項目 105 / 143 の負荷依存）と #1278（ベースラインが 19 → 24 件）で追う。
  残りの実機依存は #1438 / #1314 / #971。全体は #467 配下で、実測・A/B・引き継ぎ表は plan（下記）
- **リモート刷新（エピック #1059）は分割 A〜H が全部 main に着地**し、残りはユーザーの実機スマホ
  確認だけ（`tako todo` u-7）

## 次の一手

- #1485 の PR 着地 → install → GUI 再起動 → 「Mac 再起動で remote が戻るか」を `tako todo` へ
- **自走で着手できる bug / 改善は #1485 以外は出尽くした**。残る open は ①`tako todo` の
  ユーザー確認待ち ②Windows 実機依存（#1438 / #1314 / #1278 / #1073）③[提案] = ユーザー判断
- 相談リスト: #1405 / #1407・#1408（ファイルツリー）/ #1194・#1196・#1197（tmux 系）/ #1205 /
  #1228 + #1352（`progress.md` の衝突）/ #611 / #1291 / #1007 S1（LSP 基盤）/ #1374 / #1385 /
  #1316 の案 (a) / main の branch protection
- #1059 / #1081 は `tako todo`（u-7 / u-1）が済んだら close する
- #975 残: #987 / #988 / #990 / #991。関連 open は #985（codex / agy の limit 応答）
- #1001 残: C5 / C8〜C10（調査系）。C7 = #1426 まで着地済み
- 小口: `tasks_panel.rs` の `caret_lines` を `TextField::caret()` へ（#1459 で範囲外扱い）

## 現フェーズで Read すべき設計書

- remote daemon（#1485）を触る: `.agent/plans/tako-remote-plan.md` §10（`remote serve` の不変条件）+
  `.agent/requirements.md` FR-6.13（#1049 の自己検査）+ `crates/tako-control/src/remote.rs` 冒頭の pid 管理
- perf（#1001 の残り）を触る: `.agent/conventions.md`「効果を測る単体テストは実時間で比べない
  （#1167 / #1220）」+「番犬は 2 本立て」+「量を観る口の作り方」
- ファイルツリー（#1407 / #1408 / #1009）を触る: `.agent/conventions.md`「「ディレクトリか」の
  判定はリンクを辿る側に揃える（#1398）」+ `.agent/requirements.md` の FR-3 系
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
- Windows 実機: `.agent/plans/2026-08-windows-main-merge-wip.md`
- SSH / リモート: `.agent/plans/2026-08-remote-folder.md` + `.agent/plans/tako-remote-plan.md`
- 動画・仮想ディスプレイ収録: `.agent/plans/2026-09-youtube-explainer.md`
- 対応マトリクスの判定を触るなら `crates/tako-core/src/platform/support.rs`
