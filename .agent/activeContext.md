# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-09-22）

- main = `7bfeb6c`（#1524 / PR #1527 まで）。9/22 は **8 件着地** = #1499（未検出の依存を [y/N] で
  導入）/ #1502（外部ターミナルからも `tako`）/ #1516（`self --pane N` が名指しに従う）/
  #1518（bash 3.2 の空配列展開 41 箇所 + 番犬）/ #1501（未認証・未導入でも setup が全段完走）/
  #1509（remote setup の Tailscale 導入を deps へ）/ #1512（人に残す段を requirements へ）/
  #1524（標準 setup の [y/N] を 1 呼び出しへ）。1 件ずつは progress.md
- 版は **v0.8.17**（9/22 05:00 の夜間リリース = `a3bfe24`）。**9/22 の 8 件はまだ未リリース**
- **/Applications は 9/22 19:47 install・GUI も 19:47 起動**（#1524 着地 19:42 の直後。
  `check-health` に `tako_cli_path.link_state=correct` が載る = #1502 世代以降であることは実測済み。
  SHA は埋め込みが無いので厳密には測れない）
- **人待ちの正本は `tako todo`**。ここにも引き継ぎにも**列挙しない**（#1450 B4）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーの
  メイン画面に窓を出さない）。起動前に `scripts/lib/virtual-display.sh ensure`、窓の位置・寸法は
  `TAKO_WINDOW_BOUNDS` / `tako window move|resize`（**AX 禁止** = #1442）。検証スクリプトからは
  `. scripts/lib/isolated-gui.sh` の 1 実装で立てる（#1490）。レシピは `.agent/conventions.md`
- **検収の status 読みは `/Applications/tako.app/Contents/MacOS/tako` で叩く**（PATH 先頭の
  `target` が stale だと新フィールドがキーごと無い = #432 と同じ罠）
- **書いたシェルは macOS 同梱の bash 3.2 で通す**（空配列は `${arr[@]+"${arr[@]}"}`。番犬が
  リポジトリ全体の `.sh` を走査する = #1499 / #1518）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）。**着地は 1 本ずつの直列**
  （`progress.md` が全 PR で衝突し CI 後に CONFLICTING へ落ちる = #1228 / #1352）。merge は
  `scripts/wait-pr-checks.sh` → `scripts/merge-pr.sh`（#1430 で worktree からも exit 0 になった）

## 直近の観点

- **エピック #1500（setup ゼロタッチ化）が現在の主戦場**。9/22 の 8 件のうち 6 件が配下
  （#1499 / #1501 / #1502 / #1509 / #1512 / #1524。#1518 は #1499 の作業から出た派生で、
  #1516 は別系統）。残りのレーン分けは「次の一手」
- **setup の体験を組む層は `setup_deps::offer_and_install` の 1 実装へ寄った**（#1499 → #1509 →
  #1524）。依存導入の口を足すときはここへ載せる（呼び手で案内と [y/N] を組み直さない）
- **「入口が 2 種類の問いを同じ `pane` 欄へ畳む」型**は #1516 で `self` だけ直した
  （名指しなら呼び出し元の手掛かりを 1 つも載せない）。`adopt` / `guide` / `handoff` は同型のまま = #1520
- **#1485（remote daemon が再起動で戻らない）は着地・close 済み**。本番 remote は 9/21 12:51 に
  立てた `remote serve` が継続中
- **起動時ロードの予算（#1139）は `tako context-budget` が正**で CI の番犬が落とす。AGENTS.md が
  30657 / 30720 バイト（残り 63 バイト）まで膨れていたので、#1525 でリリース運用の詳細を
  `.agent/release.md` へ出した（**内容は 1 文字も変えず置き場だけ**）
- **Windows 実機**: 素のビルドのブロッカーだった #1133 は解消済み。`TAKO_APP_SELF_TEST_OK` までの
  完走は #1073（項目 105 / 143 の負荷依存）と #1278（ベースラインが 19 → 24 件）で追う。
  残りの実機依存は #1438 / #1314 / #971。全体は #467 配下で、実測・A/B・引き継ぎ表は plan（下記）

## 次の一手

- **レーン A（#1500 の自走できる残り。直列で 1 本ずつ）**: #1503（agent CLI の probe に
  タイムアウトが無く無言で固まる）→ #1504（Windows のシェル統合を setup の段へ）→
  #1505（`--check` の正本が check_health と 2 つある）→ #1506（`--review` が Enter だけで
  max-5x を max へ退行させる）→ #1507（末尾にリモート接続の 1 行）→ #1508（表示の手当て）
- **レーン C = #1510（ユーザー判断待ち）**: GUI 初回起動で副作用の無い setup 段を自動実行する件。
  判断してもらう点は「非 TTY の `curl | bash` を初回起動の自動実行に含めるか」
- **判断待ちなので着手しない**: #1511（sudo / 他アプリ起動を伴う自動化の可否）・#1515（default
  master の自動 adopt）・#1520（`--pane` の入口）・#1498（番犬の fn 追跡）
- 相談リスト: #1405 / #1407・#1408（ファイルツリー）/ #1194・#1196・#1197（tmux 系）/ #1205 /
  #1228 + #1352（`progress.md` の衝突）/ #611 / #1291 / #1007 S1（LSP 基盤）/ #1374 / #1385 /
  #1316 の案 (a) / main の branch protection
- #975 残: #987 / #988 / #990 / #991（関連 open は #985）。#1001 残: C5 / C8〜C10（調査系）
- #1059 / #1081 は `tako todo`（u-7 / u-1）が済んだら close する

## 現フェーズで Read すべき設計書

- setup の段を触る（#1503〜#1508）: `.agent/requirements.md` の **FR-2.41**（#1512 で入れた
  「意図して人に残す段」）+ `crates/tako-control/src/setup_deps.rs` の `offer_and_install` +
  `.agent/conventions.md`「コマンド案内の規約（Issue #322）」（提示は常に最簡形）
- md の置き場・予算を触る: `AGENTS.md`「起動時ロードの予算」節 + `.agent/conventions.md`
  「起動時ロードの予算（Issue #1139）」（数値の正本は `tako_core::context_budget`。手で書かない）
- リリースを打つ / 夜間リリースの機構を触る: `.agent/release.md`（#1525 で AGENTS.md から移した）
- シェルスクリプトを書く: `.agent/conventions.md`「シェルスクリプトは macOS 同梱の bash 3.2 で
  通す（Issue #1499 / #1518）」
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
- Windows 実機: `.agent/plans/2026-08-windows-main-merge-wip.md`
- SSH / リモート: `.agent/plans/2026-08-remote-folder.md` + `.agent/plans/tako-remote-plan.md`
