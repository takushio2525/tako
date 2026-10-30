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

## 2026-10-30（#2365 の続き: GUI へ繋ぐ口（ipc.rs・時間切れ・起動失敗の包み）と置き場（ゲート・チェックポイント・人がやること・TaskStore・YAML の理由）の断りを日英へ（Reason 155 → 179））
- A/B: CLI / MCP 13 件・dispatch 30 件の英語表示の日本語入り → 0・日本語バイト一致・カタログ +0 B。PR #2497

## 2026-10-30（#2496: 項目 108 / 110 の `measure_output_redraw` も `open_redraw_windows` で両方の時計を空ける。いまの並びでは対象はフォーカス中で落ちない = 潜在）
- `unfocus` 注入で修正前は 108 / 110 とも FAILED → 修正後は通る・注入なし 4/4・番犬が `main.rs:<行>` を名指し・カタログ +0 B。PR #2500

## 2026-10-30（#2501: CI 待ちの既定を書いた md・コメント・--help の数字を `DEFAULT_TIMEOUT` と ci.yml の job の上限に突き合わせる番犬 `issue2501_wait_timeout_docs_watchdog`。docs だけの PR でも ubuntu で走る）
- 実ファイルへの注入 3 通りでファイルと行を名指しして落ち、戻すと緑・カタログ +0 B。PR #2502

## 2026-10-30（#2365 の続き: config_io の書き出し 6 文 → `WriteError`・remote.rs の AppIpcClient 9 行 → `Reason`・データディレクトリの断り 5 か所 → `DataDirUnresolved` / `IoCause`）
- 実経路 A/B で英語表示の日本語入り 置き場 5 → 0・remote daemon 4 → 0、日本語はバイト一致・カタログ +0 B。PR #2504

## 2026-10-30（#1614: cargo test 中の bash の syntax error は shell_dialect の意図的な負の対照（`bash -n`）だった → stderr を取り込んで `syntax error` を assert）
- 単体の stderr 2 行 → 0 行・注入 3 通りで FAILED・カタログ +0 B。PR #2503

## 2026-10-30（#1622: 環境診断の外部の問い合わせ（`claude mcp list` ほか）を導入状況と重ね、check-health 9.7〜12.0 → 5.8〜6.4 秒・setup --check 9.0〜9.9 → 5.4〜5.9 秒。出力は sha256 まで一致）
- 番犬は diagnostics.rs の待ち合いの単体テスト（直列へ戻す注入 4 通りで file:line を名指し）・実経路 22 / 22・カタログ +0 B。PR #2505

## 2026-10-30（#1593: CHANGELOG の `## [Unreleased]` 5 つを最上段の 1 つへ（埋もれた 42 項目は出荷版の節へ・0.7.9 の 8 項目は消費済み）・夜間の挿入を `scripts/lib/changelog.sh` へ（A/B `TAKO_1593_LEGACY=1`）・番犬 `issue1593_changelog_unreleased_watchdog`）
- 注入 2 通りで番犬が file:line で FAILED・test-release-retry 152 pass・カタログ +0 B。PR #2506

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
