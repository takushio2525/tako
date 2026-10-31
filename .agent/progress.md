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

## 2026-10-31（#2365 の続き: dispatch のファイル操作 4 種と蓋閉じ継続の解除・Battery の断りを中身（`OsIntegrationError` 26 種・`LidError` 13 種・tako-core の `JsonReason` / `UrlBlocked::text_in`）ごと表示言語へ。Windows の文も cfg の外の表）
- 実経路テストで英語表示の日本語入り 6 / 7 → 0 / 7・隔離 GUI の CLI / MCP 11 / 12 → 0 / 12・日本語は sha256 一致・注入 5 通りで名指し・カタログ +0 B。PR #2545

## 2026-10-31（#2539: GUI の 2 秒ごとの remote の状態の更新が daemon の生死の判定で `daemon_status` を 2 回引き、tailscale を 2 秒ごとに 6 本起こしていた → `remote::daemon_running`（PID ファイルだけ）/ `poll_admin_state`・A/B `TAKO_2539_LEGACY=1`）
- 隔離 GUI + 偽 tailscale の 60 秒で GUI 由来 173.8 → 0 本/分・子の CPU 366 → 7 ms/分・status の sha と検知の遅れは不変・番犬 `issue2539_remote_poll_tailscale_watchdog`・カタログ +0 B。PR #2543
- 本番確認で #2539 を閉じた（再起動後の本番 GUI の子に tailscale 0 本・陽性対照つき）

## 2026-10-31（#1833: 読み直しを undo で戻した直後の自動保存が外部変更を上書きするのを止める（`EditKind::Reload` / `TextBuffer::reload_undone`・A/B `TAKO_1833_LEGACY=1`）・リモートの競合を帯と `conflict`（`remote: true`）・`notices` に載せる・CLI `tako edit save --force` に別名 `--overwrite`）
- 実経路 38 / 38（修正前は 1A / 1B とも外部変更が消え、リモートは conflict 無し）・番犬の注入 4 通りで file:line を名指し・カタログ +0 B。PR #2546

## 2026-10-31（#2365 の続き: dispatch の setup 系の断り（MCP 登録・子の tako setup・answers・config.yaml / changes.yaml・共通ルール同期・依存の導入・導入の系統名とホーム）を `SetupRefusal`（40 種）と `AgentMcpError::text_in` へ。CLI の setup-mcp の直呼びに表示言語の初期化）
- 実経路（隔離 GUI へ MCP 18 + CLI 3）で英語表示の日本語入り 20 / 21 → 0 / 21・日本語は 21 件 sha256 一致・注入 4 通りで file:line を名指し + E0004・カタログ +0 B。PR #2547

## 2026-10-31（#1998: tako のペインへ共有 jobserver（`CARGO_MAKEFLAGS` の fifo）を注入し包まない cargo も札で絞る・器へは -e で固定・macOS の data dir の空白は一時 dir へ逃がす・受け継いだ jobserver があると cargo は `CARGO_BUILD_JOBS` を見ないと実測で確認（設計書 §9.7））
- 実経路で 3 本並べて N + 3 = 7 に対し 7（A/B 11）・再起動をまたいで空きの最大 = N・kill で失った 4 枚を数え直しで戻した・カタログ +98 B。PR #2548
- 本番確認済み（再起動後に作ったペインに `CARGO_MAKEFLAGS`・`tako queue` に「cargo の札: 空き 18 / 18」）

## 2026-10-31（#2365 の続き: bootstrap の install / path / undo-path の断りを、中身（インストーラの取得と実行・tako-core の `PathBlockError` / `LinkError` / `UserPathError`）ごと `SetupRefusal`（40 → 57 種）へ）
- 隔離 GUI の A/B で英語表示の日本語入り 16 / 16 → 0 / 16・日本語は sha256 一致・実経路テストは 19 / 19 → 0 / 19・注入 5 通りで名指し + E0004・カタログ +0 B。PR #2549

## 2026-10-31（#1935: 構文の塗りの前置き（#1917）を 21 構文へ広げ、書き換えを build.rs へ移し、構文ごとの損益分岐以下は元の正規表現で塗る（Svelte / Vue は TS だけ前置き付き）。A/B `TAKO_1935_LEGACY=1`）
- 1 MB の冷えた 1 回目 JS 3.75 → 1.88 秒・C# 3.16 → 1.03 秒ほか 19 言語で 22〜67% 短縮・境目 +1 行は同じか速い・全記録 48 件一致・常駐 +0.96 MB・バイナリ +231 KB・カタログ +0 B。PR #2552

## 2026-10-31（#2551: issue2035 の子プロセス（本番の解決 = `$SHELL -l -c`）が開発者の rc の `pyenv rehash` のロック待ちで 1 回目の initialize の 15 秒を使い切る → 子へ `SHELL=/bin/sh` と一時の HOME・番犬 `issue2551_lsp_child_shell_watchdog`）
- 修正前は rehash のロック待ちの注入で同じ文言で FAILED → 修正後は同じ注入で ok・注入なし 10 回連続 ok・番犬が逆戻り 4 通りを file:line で名指し・カタログ +0 B。PR #2553

## 2026-10-31（#2550: setup bootstrap の PATH 通しが種別の分からない `$SHELL`（nu・tcsh 等）を zsh と見なして `.zprofile` へ書いていた → `shell_kind_for` で空・未設定のときだけ OS の既定、未知は既存の断りで止める）
- 砂場の CLI で修正前 rc=0・`.zprofile` を書く → 修正後 rc=1・書かない（4 シェル）・既知 7 種はファイルと応答がバイト一致・注入で単体と実経路が file:line で落ちる・カタログ +0 B。PR #2554
- 09:07 に `5d3f73d` を install・GUI 再起動の本番確認で #2538 も閉じた（install をまたいだ `tako queue` の包みの待ち明けで拒否 0）。断りの案内文の誤りは #2557
