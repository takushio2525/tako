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

## 2026-10-30（#2365 の続き: dispatch の個別の断り（選択肢ダイアログへの応答・orchestrator・設定の保存・Web・メニュー・値の検証）82 か所を `Reason` へ（182 → 244 種）。CLI の handoffs / shortcuts の直呼びに表示言語の初期化）
- 実経路 修正前 英語表示の日本語入り 49 / 49 → 0・CLI 7 / 7 → 0・日本語は全件バイト一致・注入 4 通りで名指し・カタログ +0 B。PR #2508

## 2026-10-30（#1716: tako-cli のテストのビルドは `caller_pane` を env を読まない `test_caller` へ向け、`--pane` 省略の偽の緑を塞ぐ・迂回と結合テストの env の外し忘れは番犬 `issue1716_caller_pane_env_watchdog` が file:line で落とす）
- 修正前は env ありで通り env なしで落ちる → 修正後は両方で同じ断り・注入 4 種を名指し・カタログ +0 B。PR #2510

## 2026-10-30（#2509: issue1944_lsp_fetch の取ってきた 4 件が全体実行でだけ `starting` で落ちる = ログインシェルの rc の `pyenv rehash` がロック待ちで stdout を握り、10 秒の kill 後も EOF が来ない → `FetchConfig::plain_shell` で rc を読まない `/bin/sh -c`・上限は本番の既定）
- 注入（rc が stdout を握る偽の `$SHELL`）で旧は実物と同じ 4 件・同じ JSON で落ち新は 10 passed・全体実行 3 回で 1944 の FAILED 0・カタログ +0 B。PR #2511

## 2026-10-30（#1715: context-budget の起動前 1 行を予算超過と作業ログの復活・重複で言い分ける・応答の `startup_line` を CLI / MCP / master の 1 実装に）
- 修正前は復活だけで「予算超過: 1 件」・両方で復活が 1 行から消えていた → 修正後は 4 通り × 日英で言い分け・カタログ +0 B。PR #2512

## 2026-10-30（#2365 の続き: dispatch 本体に残っていた断り 59 種（LSP・検索・変更履歴・リモートフォルダ・カード・Code Runner・ファイルツリー・supervisor・ledger・セッション再起動ほか）を `Reason` へ・番犬の本体の範囲が途中の `#[cfg(test)] fn` で切れていたのを直した）
- 実経路 43 ケースで英語表示の日本語入り 43 → 0・日本語はバイト一致・カタログ +0 B。残りは setup 系 17 行と他モジュールの文。PR #2515

## 2026-10-30（#2513: install 後の `.tako-replace-<pid>` の ASP 拒否 22 本 / 31 秒の真因は「GUI 配下の tako CLI」ではなく、install をまたいだ remote daemon の serve 監視が `tailscale` を素の名前で起こす PATH の空振り（親の消えた実行ファイルで検査される）→ 検出の正本が絶対パスで持つ `exe::find_in_process_path`・A/B `TAKO_2513_LEGACY=1`）
- 実 daemon + 本物の install_bundle_in_place の A/B で差し替え後の拒否 42 → 0・注入で番犬が tailscale.rs:160 を名指し・カタログ +0 B。本番確認待ちで Issue は開けたまま。PR #2516

## 2026-10-30（#2514: issue1977 / issue1956 の e2e も取得つきの manager を helper 1 か所で作り `plain_shell = /bin/sh`・本番の既定の上限を渡す。番犬が helper の外の `with_fetch` も名指す）
- 注入（rc が stdout を握る偽の `$SHELL`）で 1977 は旧 FAILED → 新 8 passed。1956 は起動に届かない（旧新とも ok）ので helper 経由の一時テストで旧 FAILED → 新 ok・カタログ +0 B。PR #2517

## 2026-10-30（#2365 の続き: dispatch が素通ししていた orchestrator の設定ファイル（projects / accounts / profiles）とエージェント名の検証の断りを `ConfigError`（32 種）へ・型の口 `*_typed`）
- 実経路 36 ケースで英語表示の日本語入り 36 → 0・CLI 11 件で 10 → 0・日本語はバイト一致・カタログ +0 B。PR #2521

## 2026-10-30（#2519: GUI が周期的に起こす git / tmux（bin_resolve の PATH の段）・codex / agy の lsof・sudo / pmset を PATH で引いた絶対パスで起こす・`exe::resolve_for_spawn` / `bin_resolve::Held`・A/B `TAKO_2519_LEGACY=1`）
- 本物の contents-swap の再現で差し替え後 10 秒の拒否 204 → 0・番犬 `issue2519_gui_spawn_path_watchdog` が注入を file:line で名指し・カタログ +0 B。PR #2522

## 2026-10-30（#2518: install をまたいで旧版のまま動く remote daemon を PID ファイル 4 行目の実行ファイルの識別（3 行の旧形式は ctime と起動時刻）で見分け、GUI 起動時の自動復帰と `tako remote start` が desired を保ったまま入れ替える・status に `serve_generation`・A/B `TAKO_2518_LEGACY=1`）
- 本物の daemon + 本物の contents-swap の A/B で PASS 41・注入で番犬が remote.rs:3467 / remote_autostart.rs:218 を名指し・カタログ +0 B。PR #2523
- 15:24 の install・GUI 再起動の本番確認で #2513 / #2518 / #2519 を閉じた

## 2026-10-30（#2520: issue1977 の旧腕 3 本の `TAKO_1977_LEGACY` を `run_alone!` の子に閉じ、#2400 の番犬が目印つきで手で戻す対を `file:line` で名指す・既存 12 対は `KNOWN_RESTORE_PAIRS`・残りは #2525）
- 注入で修正前 1 → 3 件の連鎖 → 修正後は注入した 1 件だけ（12 回）・カタログ +0 B。PR #2526

## 2026-10-30（#2527: test-serve-watch.sh が既定の置き場の control.json から本番 GUI へ届いていた（remote start の QR の open_file と daemon の list）→ 発見の置き場と data dir を一時へ・同じ型 2 本も・番犬 `verification_isolation_watchdog` が tako CLI の行で発見の置き場を拘束・例外は `# tako:prod-ok`）
- 本番役の隔離 GUI + 中継で修正前 接続 16 件・ペイン 1→2 → 修正後 0 件・1→1・カタログ +0 B。PR #2528

## 2026-10-30（#2525: 目印つきで env を書き換えて手で戻す残り 5 ファイル・11 対を `run_alone!` の子へ寄せ、#2400 の番犬の `KNOWN_RESTORE_PAIRS` を issue2092 の 1 対だけに）
- 注入で修正前は 1893 / 2034 / 1909 が後のテストへ連鎖（1 → 4 / 3 / 3 件）→ 修正後は注入した 1 本だけ・旧形への注入で番犬が 11 対を file:line で名指す・カタログ +0 B。PR #2529

## 2026-10-30（#2365 の続き: GUI 側の口（ControlHost の実装）が組む断り 48 種を `HostRefusal` へ。dispatch は変えず GUI が表示言語で返す）
- 隔離 GUI の A/B で英語表示の日本語入り 31 / 35 → 0 / 35・日本語は 35 件バイト一致・注入 3 通りで名指し・カタログ +0 B。PR #2530

## 2026-10-30（#2531: `TAKO_ISOLATED=1` の CLI / MCP ブリッジ / remote daemon は置き場（`TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR`）の指定が無ければ既定（本番）の置き場の発見ファイルを読まず日英で断る（正本 `discovery::read_candidates`・順番待ちだけ例外・A/B `TAKO_2531_LEGACY=1`））
- 本番役の隔離 GUI + 中継で修正前 接続 各 1・remote start 2 件・ペイン 1→2 → 修正後 0 件・1→1・番犬 `issue2531_isolated_discovery`・カタログ +0 B。PR #2535
- 本番確認で #2531 を閉じた

## 2026-10-30（#2365 の続き: sessions / resume_launch / pane_log の断り 11 か所を `SessionRefusal`・fda / sleep_guard / shell_integration の 4 か所を `OsRefusal` + tako-core の `SiError` へ。scripts の日本語だけの判定 3 か所も日英に）
- 実経路の英語表示で日本語入り 21 → 0（隔離 GUI 16 → 0）・日本語はバイト一致・カタログ +0 B。蓋閉じの解除・Battery・os_integration・lid.rs は次のスライス。PR #2534

## 2026-10-30（#2519 開け直し: Dock 起動の GUI は remote daemon の稼働中、2 秒ごとの `refresh_remote_state` が `daemon_status` を 2 回呼び、`tailscale::pick_candidate` が PATH に無い素の `tailscale` を 4 エントリ空振りしていた → unix では引けない素の名前を起こさない `try_unresolved_bare`・A/B `TAKO_2519_LEGACY=1`）
- 隔離 GUI + 本物の contents-swap で差し替え後 60 秒の拒否 240 → 0（本番 3.9 件/秒と一致）・番犬が注入を tailscale.rs:190 / remote.rs:2521 で名指し・カタログ +0 B。PR #2536
- 本番確認で #2519 を閉じた（tailscale を 1 tick に 6 本起こす件は #2539）

## 2026-10-30（#2533: 17:37 の remote daemon の入れ替えの要求元は特定できず（desired の書き直し = 人向けの stop → start の対・自動 3 経路と GUI の dispatch は perf.log と記録で外れた）→ 頼む側の `daemon_stop_requested` / `daemon_start_requested` と daemon 側の `requester`（接続元を OS から引く・経路の名乗り・祖先 6 段の名前）を audit.log へ）
- 実 daemon の A/B で修正前は要求元 0 行 → 修正後は CLI の pid・ペイン・祖先で名指せる・番犬 `issue2533_remote_request_origin_watchdog`・カタログ +0 B。Issue は Refs のまま。PR #2540

## 2026-10-30（#2365 の続き: 更新の口（`UpdateRefusal` / `UpdateNote` / `CheckError` の中身 / `InstallerError`）と tako-core の 5 系統（`PlanRefusal` / `ViewRefusal` / `OutlineRefusal` / `Truncation::edit_refusal_in` / `ClickRefusal`）・変更履歴の打ち切り（`HistoryTimedOut::text_in`）を表示言語へ。scripts の 2150 / 1676 の判定は日英）
- 実経路 23 ケースで英語表示の日本語入り 23 → 0・日本語は 23 / 23 バイト一致・注入 4 通りで名指し + E0004・カタログ +0 B。PR #2541

## 2026-10-30（#2538: install の前から待っていた `tako queue` の包みが待ち明けに子を素の名前で起こし、PATH の空振りぶん拒否（本番 41 × 2 を 2 回。段 1 で install の手順ではないと確定）→ 受付の前に `exe::pin_before_wait` で引いた絶対パスで起こす・argv[0] は打った形のまま・A/B `TAKO_2519_LEGACY=1`）
- 隔離の調停役 + 本物の contents-swap で待ち明けの拒否 10 → 0（2 回）・見え方 / 終了コード / シグナル / 127 は sha 一致・注入 3 通りで番犬が queue_wrapper.rs:577 / 378 を名指し・カタログ +0 B。本番確認待ちで Issue は開けたまま。PR #2542
