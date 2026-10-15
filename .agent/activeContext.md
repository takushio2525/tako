# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-10-16）

- main = `2386abd`（#1954 / PR #2135 まで）。10/16 未明の着地 = #1731 / #2113 / #2127 / #2129 /
  #1732 / #1954（1 件ずつは progress.md）。#1955 は調査の結果「既定を動かさずに Finder の候補を
  広げる方法は無い」で Info.plist を変えない（結論は Issue のコメント）
- 版は **v0.8.34**（夜間のテスト版）。安定版は `scripts/release.sh --promote <tag>` で昇格する
- **人待ちの正本は `tako todo`**。ここにも引き継ぎにも**列挙しない**（#1450 B4）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーの
  メイン画面に窓を出さない）。検証スクリプトからは `. scripts/lib/isolated-gui.sh` の 1 実装で
  立てる（#1490）。窓の位置・寸法は `TAKO_WINDOW_BOUNDS` / `tako window move|resize`（**AX 禁止** = #1442）
- **ビルドした CLI は `env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 TAKO_DATA_DIR=<短い一時 dir>`
  で叩く**（作業シェルは本番 GUI の socket を継承している）。status 読みは
  `/Applications/tako.app/Contents/MacOS/tako`（PATH 先頭の `target` が stale なことがある）
- **重い処理（cargo build / test / clippy・build-app.sh）は `tako queue <コマンド>` を通す**（#1994。
  同時数・空きメモリ・メモリ圧で割り当てる）。負荷を生成して再現しない（注入で再現する）
- **書いたシェルは macOS 同梱の bash 3.2 で通す**（空配列は `${arr[@]+"${arr[@]}"}`。番犬が
  リポジトリ全体の `.sh` を走査する = #1499 / #1518）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）。**PR では `progress.md` /
  `activeContext.md` を触らない**（並列の PR が毎回そこで衝突するため。PR 本文の末尾に
  「progress 用」1〜3 行を書き、master がまとめて 1 本の PR にする）。merge は
  `scripts/wait-pr-checks.sh` → `scripts/merge-pr.sh`（docs / .agent だけの PR は Rust の CI が skip = #2084）

## 直近の観点

- **MCP カタログは約 202.3 / 204.8 KB（残り約 2.5 KB）**。新しいツールは足さず既存ツールの引数・
  応答で表す。増えたバイト数を PR に書く（`mcp_catalog_snapshot` が落とす）
- **Code Runner の実行設定（エピック #1726）**: S3 #1731・S4 #1732 着地。`run-configs.json` の
  args / env / cwd / before と実行環境・プロファイルの選択が CLI / MCP から効く。UI（S5 #1733）は実装中
- **IDE の対応形式（エピック #1943）**: .ino の LSP（#1951 / #2113）・Code Runner（#1950）・
  PowerShell の構文（#1954）まで着地。残りは S5 #1952（.ino の基板とポートを実行設定で選ぶ）
- **tako mod（エピック #1877）**: #1964 の GUI モードの既定（GPUI のチャット / mod の描画）は
  ユーザーの判断待ち（`tako todo`）
- **PC リソース（#1992）・軽量化（#2011）**: `tako queue` は本番で使われている。夜間リリースの
  後の footprint 見張り（#2023）が走り始めた
- **Windows（#467）**: CI は両 OS で緑が既定。実機依存の残りは #467 配下

## 次の一手

- 実装中: #1733（実行ボタンのドロップダウン UI）/ #1735（実行環境の表に Node と Rust）/
  #2133（master の引き継ぎ後の spawn が master 側の領域を割る）/ #1955 の仕上げ（設計書と
  `tako platform` への Windows 未実機の申告）
- 続き: #1734（実行先の記憶）・#1952（.ino の基板とポート）は #1733 の後（同じ実行設定の UI）。
  #2136（#2129 の後始末・小）
- 相談リスト（[提案]・自動実装しない）: #2019 / #2022 ほか。判断の正本は Issue と `tako todo`

## 現フェーズで Read すべき設計書

- Code Runner を触る（#1733 / #1734 / #1735）: `.agent/plans/2026-09-runner-settings.md`（§7 UI・
  §9 検出表・§10 スライス）+ `.agent/requirements.md` の FR-3.18
- IDE の対応形式を触る（#1952）: `.agent/plans/2026-10-language-coverage.md`
- LSP を触る: `.agent/plans/2026-09-lsp-s1.md`
- orchestrator の spawn / レイアウトを触る（#2133）: `.agent/orchestrator.md`
- md の置き場・予算を触る: `AGENTS.md`「起動時ロードの予算」節 + `.agent/conventions.md`
  「起動時ロードの予算（Issue #1139）」（数値の正本は `tako_core::context_budget`。手で書かない）
- リリースを打つ / 夜間リリースの機構を触る: `.agent/release.md`
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
