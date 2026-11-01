# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md`、フェーズ計画は `roadmap.md` を見ること。
> 予算は **80 行以内**（`tako context-budget`）。過去ターンの実装詳細はここに書かない。

## 現在の対象（2026-11-01 夜）

- main = `1a894ef`（#1721 / PR #2635 まで）。前回のまとめ（PR #2620）の後に 9 本（PR #2621 / #2622 / #2623 / #2626 / #2628 / #2631 / #2629 / #2632 / #2635）着地（1 件ずつ progress.md）。/Applications は 11/01 18:45 に `70faa47`（PR #2629 まで）を install・GUI 再起動済み（画面ロック中・remote daemon は replaced-generation）。**未 install の製品の修正: PR #2632（#2624 = 隔離中の remote）・#2635（#1721 = 狭いペインの idle 判定）**
- **本番確認の上で閉じた**: #2613（再起動直後の入れ替えの区間に打った remote start / stop が両方断られ、daemon は 1 本のまま新版へ）・#2625（本番の `tmux -L tako ls` が閉じた後 598ms → 49ms）。**本番確認待ちで開けてある**: #2610（save_layout の専有・10:52 の install 以後 0 件）・#2618（ペインログの専有）・#1555（RemoteStart の専有・1 日 1 回程度だったので数日見る）。#2365 は Refs のまま。#2533 は記録の改善（PR #2540）で Refs のまま = 次に起きたら audit.log の `daemon_stop_requested` / `requester` で要求元を名指す。#2518 以後、install 後は GUI を再起動するだけで remote daemon も新版へ入れ替わる（手で stop / start しない）
- **対応中**: #2634（`tako tmux open` のペインと GUI の終了の瞬間の tmux の止まり）/ #2630（remote の stop / status の UI スレッドの待ち）。起票済み・未着手: #2633（製品の GUI の終了の HUP の重なり・ログインシェルの問い合わせの打ち切りが rc の後始末を断つ疑い。#2634 の後）
- **10/27 ユーザー依頼: Windows 版を日常で使えるようにする（エピック #467）**。実機なしで直せるものは着地済み（#724 Web ビュー・#2428 PATH・#2434 アプリ内更新・#2432 シェル統合・#2435 パスのリンク・#1599 / #2454 / #2462 プロセスの停止・#2431 WebView2・#641 起動ループ・#1510 初回起動・#2445 wry・#885 spawn の backend 経由）。どれも Windows 実機（ssh の `win`・10/27 から未接続）での確認が残るので Refs で開けてある。CI は両 OS で緑が既定
- **usage 分析（エピック #2192）**: S1〜S5 は着地（S1 の `<data_dir>/usage/limits.jsonl` は 10/17 から本番で記録中）。設計書 `.agent/plans/2026-10-usage-analytics.md`
- 版は **v0.8.39**（10/20 以降の夜間のテスト版）。安定版は `scripts/release.sh --promote <tag>` で昇格する
- **人待ちの正本は `tako todo`**。ここにも引き継ぎにも**列挙しない**（#1450 B4）
- **検証用 GUI は常設の仮想ディスプレイ `tako-vd` へ出す**（#1141 / #1150 / #1160。ユーザーのメイン画面に窓を出さない）。検証スクリプトからは `. scripts/lib/isolated-gui.sh` の 1 実装で立てる（#1490）。窓の位置・寸法は `TAKO_WINDOW_BOUNDS` / `tako window move|resize`（**AX 禁止** = #1442）。後片付けの `stop_isolated_gui` はペインのプロセスの終了まで待って返すので、呼び手は直後に一時 dir を消してよい（待ちや rm の打ち直しを自前で足さない = #2588・番犬 `issue2588_stop_waits_pane_exit`）。ペインの zsh は既定で砂場の ZDOTDIR = 利用者の rc を読まない（#2627。読ませるのは `TAKO_ISO_USER_RC=1` + `# tako:user-rc-ok <理由>`・A/B `TAKO_2627_LEGACY=1`・理由の無い opt-in は番犬 `verification_isolation_watchdog`）
- **ビルドした CLI は `env -u TAKO_SOCKET -u TAKO_PANE_ID TAKO_ISOLATED=1 TAKO_DATA_DIR=<短い一時 dir>` で叩く**（作業シェルは本番 GUI の socket を継承している）。status 読みは `/Applications/tako.app/Contents/MacOS/tako`（PATH 先頭の `target` が stale なことがある）
- **重い処理（cargo build / test / clippy・build-app.sh）は `tako queue <コマンド>` を通す**（#1994。同時数・空きメモリ・メモリ圧で割り当てる）。負荷を生成して再現しない（注入で再現する）
- **書いたシェルは macOS 同梱の bash 3.2 で通す**（空配列は `${arr[@]+"${arr[@]}"}`。番犬がリポジトリ全体の `.sh` を走査する = #1499 / #1518）
- 作業は**専用 worktree**・main 直 push はしない（docs も PR 経由）。**PR では `progress.md` / `activeContext.md` を触らない**（並列の PR が毎回そこで衝突するため。PR 本文の末尾に「progress 用」1〜3 行を書き、master がまとめて 1 本の PR にする）。
  merge は `scripts/wait-pr-checks.sh` → `scripts/merge-pr.sh`（docs / .agent だけの PR は Rust の CI が skip = #2084）

## 直近の観点

- **worker の MCP は既定で tako だけ（#2412・codex は #2418）**。要る MCP は `tako orchestrator projects mcp <key> add <名前>` でプロジェクト単位に足す
- **メモリ逼迫（warn / critical）の間は spawn / run / 復元が起動待ち（#2413）**。応答の `launch_gate`・worker の状態 `launch_waiting`・今すぐ起こすのは `tako queue start <ペイン>`。A/B は `TAKO_2413_LEGACY=1`
- **worker の起動は spawn / 復元 / sessions resume / 突然死の復旧のすべてが `build_worker_cmd_in` の 1 実装**（#2473 / #2470）。
  突然死の復旧案内は `tako orchestrator resume <worker_id>`。#2470 の受け入れで main の `supervisor_loop` が今も呼び手ゼロと分かった（#665 にコメント済み）
- **#2367 以後、器のペインのシェルは GUI 自身の段 `__tako-container-exec` を通って起動する**（A/B は `TAKO_2367_LEGACY=1`。実行ファイルが置き換わった後は段を使わない）。器のサーバーの上限は check-health の `fd_limit.container`。器のペインを閉じるときは PTY のスレッドが attach クライアントへ SIGHUP を送り、子の側が全部閉じるまで親側を読み捨ててから閉じる（#2625。閉じるたびに器の tmux サーバー全体が約 600ms 止まっていた・A/B `TAKO_2625_LEGACY=1`・番犬 `issue2625_backend_client_hangup_watchdog`）
- **tako-app のペイン spawn は backend の入口 `wrap_spawn_for_pane` / `reserve_for_pane` を通る**（#885）。psmux の conf の差は #2484（要実機）。
  同じ器の名前が layout.json で重なったら先のペインが器を持ち、2 本目以降は新しい器で起こす（#2486・FR-5.22）
- **長生きするプロセス（GUI・remote daemon・待ちの長い queue の包み）は外部コマンドを素の名前で起こし続けない**（#2513 / #2519 / #2538）。PATH で見つけたものは `exe::find_in_process_path` で絶対パスに持つ（起こすたびに引き直す口は `exe::resolve_for_spawn`・待ってから起こす口は `exe::pin_before_wait`。install の差し替え後は PATH の空振りが AppleSystemPolicy の拒否になる）。規約は `.agent/conventions.md` の同名の節。GUI の 2 秒ごとの daemon の生死は PID ファイルだけで見る（`remote::daemon_running`・#2539）
- **#2531 以後、`TAKO_ISOLATED=1` で置き場（`TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR`）の指定が無い CLI / MCP ブリッジ / daemon は本番へ繋がず日英で断る**（`tako queue` だけ例外 = 本番の順番待ちへ並ぶ。正本 `discovery::read_candidates`・A/B `TAKO_2531_LEGACY=1`・本番確認済み）。検証スクリプトの側は番犬 `verification_isolation_watchdog` が発見の置き場を拘束する（#2527）
- **`scripts/lib/*.sh` のヘルパは呼び手の env で走るので、中で tako CLI を起こすときは `iso_cli` を通す**（#2598。本番の env を落とし GUI と同じ発見の置き場を付ける・置き場が空なら `launch_isolated_gui` がログの隣に決めて `$ISOLATED_GUI_DISCOVERY_DIR` に残す・A/B `TAKO_2598_LEGACY=1`・番犬 `verification_isolation_watchdog` がその行を file:line で名指す・例外は `# tako:prod-ok <理由>`）
- **テストが env を書き換えるなら `run_alone!` の子の中で**（#2520 / #2525）。目印つきで手で戻す形は番犬 `issue2400_test_env_write_watchdog` が file:line で落とす。既知の残りは issue2092 の 1 対だけ。**テストの使い捨ての置き場は `ScratchDir` で作る**（#2555 / #2561 / #2566。素の `PathBuf` / `String` を返す補助は番犬 `issue2561_temp_dir_helper_watchdog` が落とす）
- **英語表示の断りは日英へ寄せている途中（#2365）**。dispatch 本体は済み（`Reason`・orchestrator の設定は `ConfigError` の型の口 `*_typed`・GUI 側の口は `HostRefusal`・会話系は `SessionRefusal`・OS の設定は `OsRefusal` / `SiError`・更新の口は `UpdateRefusal`・setup 系と bootstrap は `SetupRefusal`・OS のシェル連携と蓋閉じ・Battery は `OsIntegrationError` / `LidError`・remote 系は `RemoteRefusal`・tmux は `TmuxRefusal`・設定共有（`tako config`）は `ShareRefusal`（33 種・#2589）・tako-core は `PlanRefusal` / `GitError` / `RangeRefusal` / `PolicyRefusal` ほか）。CLI の手元の断り（#2589 / #2606）と GUI 側（#2596: git パネルの型の口・動画 `VideoRefusal`・`ClipboardWriteError`・`.app` の差し替え `BundleError` / `SwapCause`・`HostRefusal::WebViewParentDetached`）も済み。dispatch の注記と CLI の同じ出力の行は `ReplyNote`（#2606）、check-health 一式と `tako setup --check` は `HealthText`（#2612。プロファイルの行は件数・先頭 3 個・他 N 個、全件は `--json` の `items[].names` = #1623・A/B `TAKO_1623_LEGACY=1`）、CLI 側の表示言語は `cli_lang()` で引く。`tako master` / `tako solo` の起動の流れ（言語は移行より前に決める）と host.rs の既定の断り `HostRefusal::Default*` も済み（#2617）。残りは Issue の最後のコメントの表 + 11/01 のコメント
- **本人だけが読むべきファイル（トークン・QR・control.json ほか）は境界 B32 `platform::private_file` を通す**（#1632 / #2581。Windows は本人 + SYSTEM だけの DACL・unix は同じ呼び出し）。境界の外で本人だけの mode を当てると番犬が file:line で名指す
- **台帳 / projects.yaml / accounts.yaml の変更は `try_mutate_typed`（Err なら書かない）を通す**（#2569。断りで書き戻す形は番犬 `issue2569_refusal_writeback` が名指す）
- **GUI（#2570）と remote daemon（#2582）は起動の最初にペインの env（`tako_core::host_env::STRIP`）を外し、unix は exec し直す**（A/B `TAKO_2570_LEGACY=1` / `TAKO_2582_LEGACY=1`）。daemon は CLI の `main` の門 `remote::is_serve_invocation` を通った起動だけ外すので、起こす引数は `remote::SERVE_ARGS` を使い直書きしない（番犬 `issue2582_daemon_inherited_pane_env_watchdog`）
- **remote daemon の起動・入れ替えの途中は `<state_dir>/tako-remote.starting` の印が立つ**（#2608。`spawn_daemon` が抜けるまで置き、status / MCP は `starting`（replacing / starting）を返して `tako remote stop` を案内しない・書き手の死 / pid の再利用 / 90 秒で失効・A/B `TAKO_2608_LEGACY=1`・番犬 `issue2608_remote_status_starting_watchdog`）。印が効いている間の `tako remote start` / `stop`（`--force` も）は `RemoteRefusal::DaemonStarting` / `DaemonStartingStop` で断り、2 本目の daemon を起こさない（#2613・A/B `TAKO_2613_LEGACY=1`・番犬 `issue2613_remote_start_while_starting_watchdog`）。MCP `tako_remote_start` と設定画面の「開始」は daemon の起動を UI スレッドで待たず `OffloadJob::RemoteStart` で background から応答（#1555・A/B `TAKO_1555_LEGACY=1`・番犬 `issue1555_remote_start_offload_watchdog`）。stop / status の UI スレッドの待ちの残りは #2630
- **隔離中（`TAKO_ISOLATED`）の remote は機械全体へ触らない**（#2619 / #2624。start の serve・stop の後始末・対話 setup の brew 導入・QR のビューアを門 `remote_isolated_serve::check` / `allows_op` の 1 実装で断る。通す理由は操作ごと（serve は検証モード・偽 tailscale・opt-in `TAKO_REMOTE_ISOLATED_SERVE=1`、導入とビューアは opt-in だけ）・本物の tailscale の判定は Windows の置き場も見る・A/B `TAKO_2619_LEGACY=1` / `TAKO_2624_LEGACY=1`・番犬 `issue2619_isolated_remote_serve_watchdog`）
- **layout.json の書き出しは専用の書き手スレッド 1 本（`layout_writer`・最新 1 件の合流・tmp + rename）、ペインログの Mutex はメインスレッドで `try_lock`**（#2610。終了と起動し直しの前は書き終わりを待つ・A/B `TAKO_2610_LEGACY=1`・番犬 `issue2610_save_layout_offload_watchdog`）。**ペインログのファイルへの操作も専用の書き手スレッド 1 本（積んだ順の FIFO）で、Mutex の内側はメモリだけ**（#2618。終了は書き終わりを待つ・A/B `TAKO_2618_LEGACY=1`・番犬 `issue2618_pane_log_io_offload_watchdog`）
- **mod なしの worker の idle 判定は、狭いペインで折り返した背景作業の申告をつないで読む**（#1721。入力欄の直前の申告に限る・読めず状態行が切れていれば `status_line_truncated`・A/B `TAKO_1721_LEGACY=1`・番犬 `issue1721_narrow_pane_idle_watchdog`）
- **折り返しをまたぐ URL / パスのリンクは `links::detect_all` の 1 実装**（#2578。修正前の結合で決めてから新しい境目だけ確かめて重ねる・GUI の cmd+クリック / ホバー / cmd+右クリックと `tako links` が同じ答え・応答の `wrap`・A/B `TAKO_2578_LEGACY=1`・番犬 `issue2578_wrapped_link_watchdog`）
- **`SHELL` が空・未設定のときのログインシェルの既定は `exe::fallback_login_shell()`**（#2583。macOS は `/bin/zsh` = setup の書き先の既定と同じ・他の unix は `/bin/sh`）。**setup の PATH はそのシェルのログインが実際に読む 1 つへ書く**: 名前が sh / dash / ksh なら `ShellKind::Sh` = `~/.profile`（#2592 / #2603。`SH_PROFILE_SHELLS`・確かめていない mksh ほかは寄せない）、bash は `.bash_profile` → `.bash_login` → `.profile` の最初に在る 1 つ（#2602・`ShellKind::login_profile_in`）。undo で 0 バイトになったファイルは消し、移す前の `.bash_profile` のブロックも外す。A/B `TAKO_2592_LEGACY=1` / `TAKO_2602_LEGACY=1` / `TAKO_2603_LEGACY=1`・番犬 `issue2592_sh_profile_watchdog` / `issue2602_bash_profile_shadow_watchdog` / `issue2603_dash_ksh_profile_watchdog`
- **ログインシェルへの問い合わせ（`exe::ask_login_shell`・`child_cmd::user_env_cli`）の起こし方は表 `exe::login_invocation` の 1 実装**（#2577。tcsh / csh は `-l` を唯一の引数としてしか受けないので argv[0] を `-<名>` にして `-c 'exec /bin/sh -c $argv[1]:q'`・他は `-l -c`・A/B `TAKO_2577_LEGACY=1`・番犬 `issue2577_csh_login_shell_watchdog`）。ペインの明示コマンド（`platform::login_shell_command`）も csh 系は同じ表の行を argv[0] の段（`exec -a` を持つ zsh → bash → どちらも無ければ argv[0] を当てない = 非ログイン）越しに起こし、器の内側の 1 行は csh の引用で組む（#2597・A/B `TAKO_2597_LEGACY=1`・番犬 `issue2597_csh_pane_command_watchdog`）
- **SSH ペインが ssh の失敗・切断後に戻るローカルのシェルにも素のペインと同じ統合を当て直す**（#2594。`remote_fs::ssh_pane_script` へ `shell_integration::relaunch()` を渡す・素のペインの env の組は `pane_shell_env()` の 1 本・A/B `TAKO_2594_LEGACY=1`・番犬 `issue2594_ssh_fallback_shell` が呼び手を名指す）
- **CLI のテストのビルドは `--pane` 省略で手元の `TAKO_PANE_ID` を読まない**（#1716・番犬 `issue1716_caller_pane_env_watchdog`）。**CHANGELOG の `## [Unreleased]` は最上段の 1 つだけ**（#1593・番犬 `issue1593_changelog_unreleased_watchdog`）。**変数を使う rm は `${var:?}` の形**（#2306）。番犬 `rmの引数で空になると広い場所を指す変数はvar_qで止める` が全 `.sh` を見る（例外は `# tako:rm-var-ok <理由>`）
- **THIRD-PARTY-LICENSES.md は CI の `licenses` ジョブが依存とのずれを落とす**（#1851）。依存を足し引きしたら `bash scripts/third-party-licenses.sh` で作り直してコミットする
- **MCP カタログは約 197.0 / 204.8 KB（残り約 7.8 KB・#2548 で +98 B・10/31 の `tako context-budget` の実測 = 162 本）**。新しいツールは足さず既存ツールの引数・応答で表す。増えたバイト数を PR に書く（`mcp_catalog_snapshot` が落とす）
- **セルフテストは FAILED の出口も成功と同じ後片付けを通る**（#2388。visual-test の成功時の出口が通らない疑いは #2491）。項目 14 の cargo はリポジトリの根（実行ファイルの位置 → `CARGO_MANIFEST_DIR` → cwd）で回し、見つからなければ日英の案内で止まる（#1556・A/B `TAKO_1556_LEGACY=1`・番犬 `issue1556_self_test_repo_root_watchdog`）。**出力の再描画を測る項目は `open_redraw_windows` で #1979 の上限の時計を両方空けてから流す**（#2490 / #2496。番犬 `selftest_redraw_window_watchdog`）
- **gh が GitHub API に届かないとき `wait-pr-checks.sh` / `merge-pr.sh` は 5 回まで試し、届かないままなら終了コード 5**（CI の結論ではない。赤の 3 とは別 = #1704 / #2498）。
  CI 待ちの既定（6600 秒・job の上限 90 分）を書いた docs は番犬 `issue2501_wait_timeout_docs_watchdog` が実値と突き合わせる（#2501）
- **隔離 GUI は言語サーバを自動で起こさない**（#2048）。LSP の検証は `TAKO_LSP_AUTOSTART=1`（取得まで見るなら `TAKO_LSP_AUTO_FETCH=1` も）を付け、e2e で取ってきたサーバは `FetchConfig::plain_shell`（rc を読まない `/bin/sh -c`）で起こす（#2509 / #2514。開発者のログインシェルの rc が stdout を握って落ちる）。visual-test の節の一時ファイルは `remove_on_fail` に登録する（#2051）
- **#2546 以後、読み直しを undo した間は自動保存が止まる**（`EditKind::Reload`・A/B `TAKO_1833_LEGACY=1`）。CLI の `tako edit save --force` に MCP と同じ綴りの別名 `--overwrite`
- **#1935 以後、構文の塗りの前置きは build.rs がビルド時に書き換える**（21 構文・小さい入力は元の正規表現・A/B `TAKO_1935_LEGACY=1`）。Code Runner（#1726）・IDE の対応形式（#1943）・tako mod（#1877）は着地済みのスライスまで CLI / MCP から効く
- **PC リソース（#1992）・軽量化（#2011）**: `tako queue` は本番で稼働（#2243 / #2254）。夜間リリースの後の footprint 見張り（#2023）が走っている。10/25 のメモリ枯渇（主因は agent の常駐 MCP）の対策 #2412 / #2418 / #2413 は着地（記録は #1992 のコメント）
- **#1998 以後、新しく作るペインの cargo は共有 jobserver の札 18 枚（論理 CPU 数）を全体で分け合う**。札を受け継いだ cargo は `CARGO_BUILD_JOBS` を見ない（設計書 §9.7 で訂正済み）。再起動前からある器のペインには入らない。設定 `resources.jobserver`・A/B `TAKO_1992_NO_JOBSERVER=1`・状況は `tako queue`
- **夜間リリースは 2 台構成**（#2252）: リリースは旧 Mac、手元の Mac は `--role check`（mod の検査と footprint の見張りだけ）

## 次の一手

- 蓋が開いて tako-vd が描けるとき: #2537（LSP の右クリックメニューの項目が 12 のうち 6）/ #2478（項目 128 の間欠）/ #2425（起動待ちの帯・UI）/
  #1902（visual-test の回数の窓）/ #1899 の 2 点目 / #2491 / #2228 / #2045（横断置換）。実機が要る: #937・#2484 ほか #467 配下（#2429 / #2430 / #2433）。#2392 は再発待ち（11/8 に閉じる）・**本番確認待ち**: #2610 / #2618 / #1555（本番 perf.log で save_layout・ペインログ・RemoteStart の専有が出ないことを見て閉じる。#1555 は数日見る）。対応中 #2634 / #2630 → #2633
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
