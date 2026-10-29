# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-10-29 夕）

- main = `6aa2045`（#2415 / PR #2474 まで）。10/27〜10/29 は 28 本着地（着地は 1 件ずつ progress.md）。/Applications は 10/29 夕に `6aa2045` を入れる
- **10/27 ユーザー依頼: Windows 版を日常で使えるようにする（エピック #467）**。実機なしで直せるものは着地済み（#724 Web ビュー・#2428 PATH・#2434 アプリ内更新・#2432 シェル統合・#2435 パスのリンク・#1599 / #2454 / #2462 プロセスの停止・#2431 WebView2・#641 起動ループ・#1510 初回起動・#2445 wry）。どれも Windows 実機（ssh の `win`・10/27 から未接続）での確認が残るので Refs で開けてある
- **10/25 夜に PC がメモリ枯渇で停止（3 回目）**。主因は agent の常駐 MCP。対策 #2412 / #2413 は着地（記録は #1992 のコメント）
- **usage 分析（エピック #2192・10/16 ユーザー依頼）**: 設計書 `.agent/plans/2026-10-usage-analytics.md`。S1（利用制限の
  観測 = `<data_dir>/usage/limits.jsonl`・`tako usage`）は 10/17 00:50 から本番で記録中。S2 #2201（索引）・S3 #2202（区間按分・% 列）・S4 #2203（画面 = 右パネルの usage ビュー）・S5 #2204（codex）は着地
- 版は **v0.8.39**（10/20 以降の夜間のテスト版）。安定版は `scripts/release.sh --promote <tag>` で昇格する
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

- **worker の MCP は既定で tako だけ（#2412）**。要る MCP は `tako orchestrator projects mcp <key> add <名前>` でプロジェクト単位に足す。codex は #2418 で実装中
- **メモリ逼迫（warn / critical）の間は spawn / run / 復元が起動待ち（#2413）**。応答の `launch_gate`・worker の状態 `launch_waiting`・今すぐ起こすのは `tako queue start <ペイン>`。A/B は `TAKO_2413_LEGACY=1`
- **#2367 以後、器のペインのシェルは GUI 自身の段 `__tako-container-exec` を通って起動する**（A/B は `TAKO_2367_LEGACY=1`。実行ファイルが置き換わった後は段を使わない）。器のサーバーの上限は check-health の `fd_limit.container`
- **変数を使う rm は `${var:?}` の形**（#2306）。番犬 `rmの引数で空になると広い場所を指す変数はvar_qで止める` が全 `.sh` を見る（例外は `# tako:rm-var-ok <理由>`）
- **THIRD-PARTY-LICENSES.md は CI の `licenses` ジョブが依存とのずれを落とす**（#1851）。依存を足し引きしたら `bash scripts/third-party-licenses.sh` で作り直してコミットする
- **MCP カタログは約 195.7 / 204.8 KB（残り約 9.1 KB・#2336 の +355 B と #2334 の +193 B の後）**。新しいツールは足さず
  既存ツールの引数・応答で表す。増えたバイト数を PR に書く（`mcp_catalog_snapshot` が落とす）
- **Code Runner の実行設定（エピック #1726）**: S3〜S7（#1731 / #1732 / #1733 / #1734 / #1735）まで着地。
  `run-configs.json` の args / env / cwd / before・実行環境・実行先が CLI / MCP と実行ボタン横のドロップダウンから効く
- **隔離 GUI は言語サーバを自動で起こさない**（#2048）。LSP の検証は `TAKO_LSP_AUTOSTART=1`（取得まで見るなら
  `TAKO_LSP_AUTO_FETCH=1` も）を付ける。visual-test の節の一時ファイルは `remove_on_fail` に登録する（#2051）
- **IDE の対応形式（エピック #1943）**: .ino の LSP（#1951 / #2113）・Code Runner（#1950）・
  PowerShell の構文（#1954）・基板とポートの選択（#1952）まで着地
- **tako mod（エピック #1877）**: #1964 の GUI モードの既定（GPUI のチャット / mod の描画）は
  ユーザーの判断待ち（`tako todo`）
- **PC リソース（#1992）・軽量化（#2011）**: `tako queue` は本番で使われている（#2243 で先頭詰まりを解消・
  #2254 で実行中の予約をピーク後に返し、見込みを過去の記録から引く）。夜間リリースの後の footprint 見張り（#2023）が走り始めた
- **夜間リリースは 2 台構成**（#2252）: リリースは旧 Mac、手元の Mac は `--role check`（mod の検査と footprint の見張りだけ）。
  旧 Mac の共有ツリーが古い main と行き来していた件は #2262、リリースする役の毎晩の追従は #2264 で着地
- **Windows（#467）**: CI は両 OS で緑が既定。実機依存の残りは #467 配下

## 次の一手

- 次の候補（実機なしで進められる）: #2470（突然死の復旧案内）/ #2477 / #2478（セルフテストの間欠）/ #2479（shell_probe の固定 20 秒）/ #2425（起動待ちの帯・UI）/ #885 / #937 / #2388 / #1899 の 2 点目・#1902 の続き。実機が要る: #2429 / #2430 / #2433 ほか #467 配下。#2392 は再発待ち（11/8 に閉じる）
- **夜間リリースは 10/21 から出ていない**（リリースする役の旧 Mac が offline）。人待ちは `tako todo`
- 相談リスト（[提案]・自動実装しない）: #2019 / #2022 / #2307 ほか。判断の正本は Issue と `tako todo`

## 現フェーズで Read すべき設計書

- usage 分析を触る（#2201〜#2206）: `.agent/plans/2026-10-usage-analytics.md`（§9 スライス・§6 プライバシー）+ FR-2.44

- Code Runner を触る（#1733 / #1734 / #1735）: `.agent/plans/2026-09-runner-settings.md`（§7 UI・
  §9 検出表・§10 スライス）+ `.agent/requirements.md` の FR-3.18
- IDE の対応形式を触る（#1952）: `.agent/plans/2026-10-language-coverage.md`
- LSP を触る: `.agent/plans/2026-09-lsp-s1.md`
- orchestrator の spawn / レイアウトを触る（#2133）: `.agent/orchestrator.md`
- md の置き場・予算を触る: `AGENTS.md`「起動時ロードの予算」節 + `.agent/conventions.md`
  「起動時ロードの予算（Issue #1139）」（数値の正本は `tako_core::context_budget`。手で書かない）
- リリースを打つ / 夜間リリースの機構を触る: `.agent/release.md`
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
