# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-10-31 夜）

- main = `037c907`（#2581 / PR #2584 まで）。10/31 は作業ログの docs PR を除いて 24 本着地（朝までの 9 本 = #2545〜#2554・後半の 15 本 = #2556〜#2584。着地は 1 件ずつ progress.md）。/Applications は 10/31 19:03 に `3aac66e`（PR #2574 まで）を install・GUI 再起動済み（master のペインから `open -a` = #2570 の再 exec の経路で正常起動・復元失敗 0・remote daemon は replaced-generation）。#2579 / #2580 / #2584 は未 install
- **#2559 / #2570 は本番確認の上で閉じた**（#2559 = 自動命名の `claude -p` の mod 報告が master の状態を上書き / #2570 = ペインから起こし直した GUI がペインの env を持ち込む）。#2533 は記録の改善（PR #2540）で Refs のまま = 次に起きたら audit.log の `daemon_stop_requested` / `requester` で要求元を名指す。#2518 以後、install 後は GUI を再起動するだけで remote daemon も新版へ入れ替わる（手で stop / start しない）
- **対応中**: #2582（remote daemon がペインの env を受け継ぐ・PR #2585）/ #2578（ユーザー依頼: cmd+クリックの URL / パスがペインの幅の折り返しで切れる・PR #2586）
- **10/27 ユーザー依頼: Windows 版を日常で使えるようにする（エピック #467）**。実機なしで直せるものは着地済み（#724 Web ビュー・#2428 PATH・#2434 アプリ内更新・#2432 シェル統合・#2435 パスのリンク・#1599 / #2454 / #2462 プロセスの停止・#2431 WebView2・#641 起動ループ・#1510 初回起動・#2445 wry・#885 spawn の backend 経由）。どれも Windows 実機（ssh の `win`・10/27 から未接続）での確認が残るので Refs で開けてある。CI は両 OS で緑が既定
- **usage 分析（エピック #2192）**: S1〜S5 は着地（S1 の `<data_dir>/usage/limits.jsonl` は 10/17 から本番で記録中）。設計書 `.agent/plans/2026-10-usage-analytics.md`
- 版は **v0.8.39**（10/20 以降の夜間のテスト版）。安定版は `scripts/release.sh --promote <tag>` で昇格する
- **人待ちの正本は `tako todo`**。ここにも引き継ぎにも**列挙しない**（#1450 B4）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーのメイン画面に窓を出さない）。検証スクリプトからは `. scripts/lib/isolated-gui.sh` の 1 実装で立てる（#1490）。窓の位置・寸法は `TAKO_WINDOW_BOUNDS` / `tako window move|resize`（**AX 禁止** = #1442）
- **ビルドした CLI は `env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 TAKO_DATA_DIR=<短い一時 dir>` で叩く**（作業シェルは本番 GUI の socket を継承している）。status 読みは `/Applications/tako.app/Contents/MacOS/tako`（PATH 先頭の `target` が stale なことがある）
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
- **長生きするプロセス（GUI・remote daemon・待ちの長い queue の包み）は外部コマンドを素の名前で起こし続けない**（#2513 / #2519 / #2538）。PATH で見つけたものは `exe::find_in_process_path` で絶対パスに持つ（起こすたびに引き直す口は `exe::resolve_for_spawn`・待ってから起こす口は `exe::pin_before_wait`。install の差し替え後は PATH の空振りが AppleSystemPolicy の拒否になる）。規約は `.agent/conventions.md` の同名の節。GUI の 2 秒ごとの daemon の生死は PID ファイルだけで見る（`remote::daemon_running`・#2539）
- **#2531 以後、`TAKO_ISOLATED=1` で置き場（`TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR`）の指定が無い CLI / MCP ブリッジ / daemon は本番へ繋がず日英で断る**（`tako queue` だけ例外 = 本番の順番待ちへ並ぶ。正本 `discovery::read_candidates`・A/B `TAKO_2531_LEGACY=1`・本番確認済み）。検証スクリプトの側は番犬 `verification_isolation_watchdog` が発見の置き場を拘束する（#2527）
- **テストが env を書き換えるなら `run_alone!` の子の中で**（#2520 / #2525）。目印つきで手で戻す形は番犬 `issue2400_test_env_write_watchdog` が file:line で落とす。既知の残りは issue2092 の 1 対だけ。**テストの使い捨ての置き場は `ScratchDir` で作る**（#2555 / #2561 / #2566。素の `PathBuf` / `String` を返す補助は番犬 `issue2561_temp_dir_helper_watchdog` が落とす）
- **英語表示の断りは日英へ寄せている途中（#2365）**。dispatch 本体は済み（`Reason`・orchestrator の設定は `ConfigError` の型の口 `*_typed`・GUI 側の口は `HostRefusal`・会話系は `SessionRefusal`・OS の設定は `OsRefusal` / `SiError`・更新の口は `UpdateRefusal`・setup 系と bootstrap は `SetupRefusal`・OS のシェル連携と蓋閉じ・Battery は `OsIntegrationError` / `LidError`・remote 系は `RemoteRefusal`・tmux は `TmuxRefusal`・tako-core は `PlanRefusal` / `GitError` / `RangeRefusal` / `PolicyRefusal` ほか）。残りは Issue の最後のコメントの表（次は config_share = `tako config`）
- **本人だけが読むべきファイル（トークン・QR・control.json ほか）は境界 B32 `platform::private_file` を通す**（#1632 / #2581。Windows は本人 + SYSTEM だけの DACL・unix は同じ呼び出し）。境界の外で本人だけの mode を当てると番犬が file:line で名指す
- **台帳 / projects.yaml / accounts.yaml の変更は `try_mutate_typed`（Err なら書かない）を通す**（#2569。断りで書き戻す形は番犬 `issue2569_refusal_writeback` が名指す）
- **#2570 以後、GUI は main の最初でペインの env（`tako_core::host_env::STRIP`）を外し、unix は exec し直す**（A/B `TAKO_2570_LEGACY=1`）。remote daemon の同じ件は #2582（対応中）
- **LSP の e2e で取ってきたサーバは `FetchConfig::plain_shell`（rc を読まない `/bin/sh -c`）で起こす**（#2509 / #2514。開発者のログインシェルの rc が stdout を握って落ちる）
- **CLI のテストのビルドは `--pane` 省略で手元の `TAKO_PANE_ID` を読まない**（#1716・番犬 `issue1716_caller_pane_env_watchdog`）。**CHANGELOG の `## [Unreleased]` は最上段の 1 つだけ**（#1593・番犬 `issue1593_changelog_unreleased_watchdog`）
- **変数を使う rm は `${var:?}` の形**（#2306）。番犬 `rmの引数で空になると広い場所を指す変数はvar_qで止める` が全 `.sh` を見る（例外は `# tako:rm-var-ok <理由>`）
- **THIRD-PARTY-LICENSES.md は CI の `licenses` ジョブが依存とのずれを落とす**（#1851）。依存を足し引きしたら `bash scripts/third-party-licenses.sh` で作り直してコミットする
- **MCP カタログは約 197.0 / 204.8 KB（残り約 7.8 KB・#2548 で +98 B・10/31 の `tako context-budget` の実測 = 162 本）**。新しいツールは足さず
  既存ツールの引数・応答で表す。増えたバイト数を PR に書く（`mcp_catalog_snapshot` が落とす）
- **セルフテストは FAILED の出口も成功と同じ後片付けを通る**（#2388。visual-test の成功時の出口が通らない疑いは #2491）。**出力の再描画を測る項目は `open_redraw_windows` で #1979 の上限の時計を両方空けてから流す**（#2490 / #2496。番犬 `selftest_redraw_window_watchdog`）
- **gh が GitHub API に届かないとき `wait-pr-checks.sh` / `merge-pr.sh` は 5 回まで試し、届かないままなら終了コード 5**（CI の結論ではない。赤の 3 とは別 = #1704 / #2498）。
  CI 待ちの既定（6600 秒・job の上限 90 分）を書いた docs は番犬 `issue2501_wait_timeout_docs_watchdog` が実値と突き合わせる（#2501）
- **隔離 GUI は言語サーバを自動で起こさない**（#2048）。LSP の検証は `TAKO_LSP_AUTOSTART=1`（取得まで見るなら
  `TAKO_LSP_AUTO_FETCH=1` も）を付ける。visual-test の節の一時ファイルは `remove_on_fail` に登録する（#2051）
- **#2546 以後、読み直しを undo した間は自動保存が止まる**（`EditKind::Reload`・A/B `TAKO_1833_LEGACY=1`）。CLI の `tako edit save --force` に MCP と同じ綴りの別名 `--overwrite`
- **#1935 以後、構文の塗りの前置きは build.rs がビルド時に書き換える**（21 構文・小さい入力は元の正規表現・A/B `TAKO_1935_LEGACY=1`）。Code Runner（#1726）・IDE の対応形式（#1943）・tako mod（#1877）は着地済みのスライスまで CLI / MCP から効く
- **PC リソース（#1992）・軽量化（#2011）**: `tako queue` は本番で稼働（#2243 / #2254）。夜間リリースの後の footprint 見張り（#2023）が走っている。10/25 のメモリ枯渇（主因は agent の常駐 MCP）の対策 #2412 / #2418 / #2413 は着地（記録は #1992 のコメント）
- **#1998 以後、新しく作るペインの cargo は共有 jobserver の札 18 枚（論理 CPU 数）を全体で分け合う**。札を受け継いだ cargo は `CARGO_BUILD_JOBS` を見ない（設計書 §9.7 で訂正済み）。再起動前からある器のペインには入らない。設定 `resources.jobserver`・A/B `TAKO_1992_NO_JOBSERVER=1`・状況は `tako queue`
- **夜間リリースは 2 台構成**（#2252）: リリースは旧 Mac、手元の Mac は `--role check`（mod の検査と footprint の見張りだけ）

## 次の一手

- 蓋が開いて tako-vd が描けるとき: #2537（LSP の右クリックメニューの項目が 12 のうち 6）/ #2478（項目 128 の間欠）/ #2425（起動待ちの帯・UI）/
  #1902（visual-test の回数の窓）/ #1899 の 2 点目 / #2491 / #2228 / #2045（横断置換）。実機が要る: #937・#2484 ほか #467 配下（#2429 / #2430 / #2433）。#2392 は再発待ち（11/8 に閉じる）
- 次の候補: #2577（tcsh の `-l -c`）/ #2583（setup の PATH の案内の残り 3 件）/ #2365 の次のスライス（config_share = `tako config`）
- **夜間リリースは 10/21 から出ていない**（リリースする役の旧 Mac が offline）。人待ちは `tako todo`
- 相談リスト（[提案]・自動実装しない）: #2019 / #2022 / #2307 ほか。判断の正本は Issue と `tako todo`

## 現フェーズで Read すべき設計書

- usage 分析を触る（#2201〜#2206）: `.agent/plans/2026-10-usage-analytics.md`（§9 スライス・§6 プライバシー）+ FR-2.44
- Code Runner を触る（#1733 / #1734 / #1735）: `.agent/plans/2026-09-runner-settings.md`（§7 UI・
  §9 検出表・§10 スライス）+ `.agent/requirements.md` の FR-3.18
- IDE の対応形式・構文の塗りを触る（#1952 / #1935）: `.agent/plans/2026-10-language-coverage.md`
- PC リソース・`tako queue`・jobserver を触る（#1992 / #1998）: `.agent/plans/2026-10-resource-manager.md`（§9.7）+ FR-2.43
- LSP を触る: `.agent/plans/2026-09-lsp-s1.md`
- orchestrator の spawn / レイアウト / worker の起動の組み立てを触る（#2133 / #2473）: `.agent/orchestrator.md`
- md の置き場・予算を触る: `AGENTS.md`「起動時ロードの予算」節 + `.agent/conventions.md`
  「起動時ロードの予算（Issue #1139）」（数値の正本は `tako_core::context_budget`。手で書かない）
- リリースを打つ / 夜間リリースの機構を触る: `.agent/release.md`
- セルフテストの待ち条件・番犬を触る: `.agent/conventions.md`「セルフテストの待ち条件の書き方」節
