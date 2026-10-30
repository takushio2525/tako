# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-10-30 夕方）

- main = `fe20cc0`（#2365 / PR #2530 まで）。10/30 は作業ログの docs PR を除いて 27 本着地（夕方の 6 本 = #2519 / #2518 / #2520 / #2527 / #2525 / #2365。着地は 1 件ずつ progress.md）。/Applications は 10/30 15:24 に `543a3cc`（#2522 / #2523 入り）を install・GUI 再起動済み。その後の着地はテストとスクリプトだけなので install 不要
- **#2513 / #2518 / #2519 は本番確認の上で閉じた**。#2518 以後、install 後は GUI を再起動するだけで remote daemon も新版へ入れ替わる（手で stop / start しない）
- **対応中**: #2365 の日英化は `host.*`（GUI 側の口）が PR #2530 で着地・次のスライス（sessions / resume_launch / pane_log・sleep_guard ほか・scripts の日本語だけの判定 3 か所）が対応中 / #2531（`TAKO_ISOLATED=1` の CLI / daemon が既定の置き場の発見ファイルを読んで本番 GUI へ縮退する）
- **10/27 ユーザー依頼: Windows 版を日常で使えるようにする（エピック #467）**。実機なしで直せるものは着地済み（#724 Web ビュー・#2428 PATH・#2434 アプリ内更新・#2432 シェル統合・#2435 パスのリンク・#1599 / #2454 / #2462 プロセスの停止・#2431 WebView2・#641 起動ループ・#1510 初回起動・#2445 wry・#885 spawn の backend 経由）。どれも Windows 実機（ssh の `win`・10/27 から未接続）での確認が残るので Refs で開けてある。CI は両 OS で緑が既定
- **10/25 夜に PC がメモリ枯渇で停止（3 回目）**。主因は agent の常駐 MCP。対策 #2412 / #2418 / #2413 は着地（記録は #1992 のコメント）
- **usage 分析（エピック #2192）**: S1〜S5 は着地（S1 の `<data_dir>/usage/limits.jsonl` は 10/17 から本番で記録中）。設計書 `.agent/plans/2026-10-usage-analytics.md`
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

- **worker の MCP は既定で tako だけ（#2412・codex は #2418）**。要る MCP は `tako orchestrator projects mcp <key> add <名前>` でプロジェクト単位に足す
- **メモリ逼迫（warn / critical）の間は spawn / run / 復元が起動待ち（#2413）**。応答の `launch_gate`・worker の状態 `launch_waiting`・今すぐ起こすのは `tako queue start <ペイン>`。A/B は `TAKO_2413_LEGACY=1`
- **worker の起動は spawn / 復元 / sessions resume / 突然死の復旧のすべてが `build_worker_cmd_in` の 1 実装**（#2473 / #2470）。
  突然死の復旧案内は `tako orchestrator resume <worker_id>`。#2470 の受け入れで main の `supervisor_loop` が今も呼び手ゼロと分かった（#665 にコメント済み）
- **#2367 以後、器のペインのシェルは GUI 自身の段 `__tako-container-exec` を通って起動する**（A/B は `TAKO_2367_LEGACY=1`。実行ファイルが置き換わった後は段を使わない）。器のサーバーの上限は check-health の `fd_limit.container`
- **tako-app のペイン spawn は backend の入口 `wrap_spawn_for_pane` / `reserve_for_pane` を通る**（#885）。psmux の conf の差は #2484（要実機）。
  同じ器の名前が layout.json で重なったら先のペインが器を持ち、2 本目以降は新しい器で起こす（#2486・FR-5.22）
- **長生きするプロセス（GUI・remote daemon）は外部コマンドを素の名前で起こし続けない**（#2513 / #2519）。PATH で見つけたものは `exe::find_in_process_path` で絶対パスに持つ（起こすたびに引き直す口は `exe::resolve_for_spawn`。install の差し替え後は PATH の空振りが AppleSystemPolicy の拒否になる）。規約は `.agent/conventions.md` の同名の節
- **`TAKO_ISOLATED=1` は CLI の GUI の発見を隔離しない**（#2527 で確定）。CLI と daemon は `TAKO_SOCKET` が無いと `<TAKO_DISCOVERY_DIR か data dir>/control.json` を読み、既定は本番の置き場。検証スクリプトは `TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR` を一時へ向ける（番犬 `verification_isolation_watchdog`）。製品側の対応は #2531（対応中）
- **テストが env を書き換えるなら `run_alone!` の子の中で**（#2520 / #2525）。目印つきで手で戻す形は番犬 `issue2400_test_env_write_watchdog` が file:line で落とす。既知の残りは issue2092 の 1 対だけ
- **英語表示の断りは日英へ寄せている途中（#2365）**。dispatch 本体は setup 系を除き済み（`Reason`・orchestrator の設定は `ConfigError` の型の口 `*_typed`・GUI 側の口は `HostRefusal`）。残り（setup 系・sessions ほか）は Issue の最後のコメントの表
- **LSP の e2e で取ってきたサーバは `FetchConfig::plain_shell`（rc を読まない `/bin/sh -c`）で起こす**（#2509 / #2514。開発者のログインシェルの rc が stdout を握って落ちる）
- **CLI のテストのビルドは `--pane` 省略で手元の `TAKO_PANE_ID` を読まない**（#1716・番犬 `issue1716_caller_pane_env_watchdog`）。**CHANGELOG の `## [Unreleased]` は最上段の 1 つだけ**（#1593・番犬 `issue1593_changelog_unreleased_watchdog`）
- **変数を使う rm は `${var:?}` の形**（#2306）。番犬 `rmの引数で空になると広い場所を指す変数はvar_qで止める` が全 `.sh` を見る（例外は `# tako:rm-var-ok <理由>`）
- **THIRD-PARTY-LICENSES.md は CI の `licenses` ジョブが依存とのずれを落とす**（#1851）。依存を足し引きしたら `bash scripts/third-party-licenses.sh` で作り直してコミットする
- **MCP カタログは約 196.9 / 204.8 KB（残り約 7.9 KB・10/30 午後の `tako context-budget` の実測 = 162 本）**。新しいツールは足さず
  既存ツールの引数・応答で表す。増えたバイト数を PR に書く（`mcp_catalog_snapshot` が落とす）
- **セルフテストは FAILED の出口も成功と同じ後片付けを通る**（#2388）。visual-test の成功時の出口が通らない疑いは #2491
- **セルフテストで出力の再描画を測る項目は `open_redraw_windows` で #1979 の上限の時計を両方空けてから流す**（#2490 / #2496。番犬 `selftest_redraw_window_watchdog`）
- **gh が GitHub API に届かないとき `wait-pr-checks.sh` / `merge-pr.sh` は 5 回まで試し、届かないままなら終了コード 5**（CI の結論ではない。赤の 3 とは別 = #1704 / #2498）。
  CI 待ちの既定（6600 秒・job の上限 90 分）を書いた docs は番犬 `issue2501_wait_timeout_docs_watchdog` が実値と突き合わせる（#2501）
- **`check-health` / `setup --check` は外部の問い合わせを導入状況と重ねて約 6 秒**（#1622。以前は約 10 秒）
- **隔離 GUI は言語サーバを自動で起こさない**（#2048）。LSP の検証は `TAKO_LSP_AUTOSTART=1`（取得まで見るなら
  `TAKO_LSP_AUTO_FETCH=1` も）を付ける。visual-test の節の一時ファイルは `remove_on_fail` に登録する（#2051）
- **Code Runner の実行設定（エピック #1726）**・**IDE の対応形式（エピック #1943）**・**tako mod（エピック #1877）**は着地済みのスライスまで CLI / MCP から効く
- **PC リソース（#1992）・軽量化（#2011）**: `tako queue` は本番で稼働（#2243 / #2254）。夜間リリースの後の footprint 見張り（#2023）が走っている
- **夜間リリースは 2 台構成**（#2252）: リリースは旧 Mac、手元の Mac は `--role check`（mod の検査と footprint の見張りだけ）

## 次の一手

- 蓋が開いて tako-vd が描けるとき: #2478（項目 128 の間欠）/ #2425（起動待ちの帯・UI）/
  #1902（visual-test の回数の窓）/ #1899 の 2 点目 / #2491 / #2228。実機が要る: #937・#2484 ほか #467 配下（#2429 / #2430 / #2433）。#2392 は再発待ち（11/8 に閉じる）
- **夜間リリースは 10/21 から出ていない**（リリースする役の旧 Mac が offline）。人待ちは `tako todo`
- 相談リスト（[提案]・自動実装しない）: #2019 / #2022 / #2307 ほか。判断の正本は Issue と `tako todo`

## 現フェーズで Read すべき設計書

- usage 分析を触る（#2201〜#2206）: `.agent/plans/2026-10-usage-analytics.md`（§9 スライス・§6 プライバシー）+ FR-2.44
- Code Runner を触る（#1733 / #1734 / #1735）: `.agent/plans/2026-09-runner-settings.md`（§7 UI・
  §9 検出表・§10 スライス）+ `.agent/requirements.md` の FR-3.18
- IDE の対応形式を触る（#1952）: `.agent/plans/2026-10-language-coverage.md`
- LSP を触る: `.agent/plans/2026-09-lsp-s1.md`
- orchestrator の spawn / レイアウト / worker の起動の組み立てを触る（#2133 / #2473）: `.agent/orchestrator.md`
- md の置き場・予算を触る: `AGENTS.md`「起動時ロードの予算」節 + `.agent/conventions.md`
  「起動時ロードの予算（Issue #1139）」（数値の正本は `tako_core::context_budget`。手で書かない）
- リリースを打つ / 夜間リリースの機構を触る: `.agent/release.md`
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
