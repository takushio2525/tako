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

## 2026-10-26（#1308 の再発: respond の実 PTY テストの fixture をログインシェル経由から `/bin/sh -c` の直接起動へ・失敗文の値を `TAKO_1308_WAIT` の 1 行へ）
- 注入 `stuck` / `deaf` で修正前は同じ行で FAILED → 修正後 ok・単独 20/20・カタログ +0 B。同じ形の残り 3 か所は #2415。PR #2410

## 2026-10-26（#2419: docs サイトの共用の consent.js を async で読み、最初の描画を止めない。製品は無変更）
- PR #2420

## 2026-10-26（#2399: 器を起こす検証スクリプト 32 本・47 か所の `kill-server` を `tmux_socket_kill`（`scripts/lib/tmux-socket.sh`）へ寄せ、ソケットファイルを残さない・番犬 `器を起こすスクリプトはソケットファイルまで消す`）
- 修正前 計 31 個 → 0・失敗経路の注入 22 本で 1〜4 → 0・番犬は修正前の版で 32 本を名指し・カタログ +0 B。PR #2408

## 2026-10-26（#2365 の続き: `tako mod ui` の成功の応答の枠・CLI の表示・`cli_request` の断り 9 種・排他の断り `config_io::LockError`・置き場が決まらない断りを日英の網羅 match へ）
- A/B 20 件で英語表示の日本語入り 20 → 0・日本語は 40 ファイルがバイト一致・注入 4 通り・カタログ +0 B。PR #2417

## 2026-10-26（#2412: claude の worker を tako の MCP だけで起動 = `--settings` の `deniedMcpServers` で user scope を止める・プロジェクト単位で足す `projects mcp`・projects.yaml v2 の移行で直近 30 日の実績を写す）
- 常駐 1,218 → 265 MB / 本・本番の GUI から起こした worker は 4 プロセス 695 MB・カタログ +321 B。codex は #2418。PR #2422

## 2026-10-26（#2413: メモリ逼迫中は agent の起動（spawn / run / 復元・sessions resume・git resolve）を `admit` と同じ純関数で待たせ、復元は master から 1 本ずつ時差・上書きは `tako queue start <ペイン>`）
- 単体で復元の間隔 150〜176 ms・立ち上がり中は最大 1 本・カタログ +394 B。10/27 の実 GUI の e2e は PASS 40 / FAIL 2（run_status の戻り・master が先）→ #2426。PR #2421

## 2026-10-26（#2365 の続き: `tako mod`（status / on / off / band / install / uninstall）の応答と CLI の表示を日英の網羅 match へ・置き場の断り `paths::NoDataDir`）
- A/B で英語表示の応答 14 → 0・CLI の表示 12 → 0・日本語はバイト一致・注入 4 通り・カタログ +0 B。PR #2423

## 2026-10-27（#2416: mod の報告のテストの ui.json の置き場が同じ TAKO_DATA_DIR で前の回の残りを読んでいた → MockHost ごとの `ScratchDir` へ）
- 修正前は同じ dir の 2 回目で 1960 が必ず FAILED → 修正後は連続 3 回・並列 2〜4 プロセスで全部緑・カタログ +0 B。PR #2436

## 2026-10-28（#2411: issue2367 の旧挙動の腕の間欠の真因はテスト = 理由を tmux の代替画面から読んでいた → stderr から採り、終了と fd の空きを状態待ちへ）
- 注入 `late-read` で旧 0/5 → 新 5/5・30 回 × 並列 1 / 4 で全通過・カタログ +0 B。PR #2442

## 2026-10-28（#2406: ネスト tmux 越しのホイールの e2e の間欠 = 起動中にマウス要求が外れる窓へ 1 発だけのホイールが落ちていた → 器のペイン側の要求で待ち、遡るまで打ち直す）
- 注入 `TAKO_2406_INJECT=gap` で修正前 FAILED → 修正後 ok・30 回 × 並列 1 / 4 で全通過・カタログ +0 B。PR #2439

## 2026-10-28（#2428: Windows の新しいペインがレジストリの PATH を読み直す・setup の「次の 1 手」に「新しいタブを開いて」を添える）
- check-windows / clippy 3 種 / test が緑・Windows 専用の実 ConPTY テストで A/B・カタログ +0 B。Windows 実機は未検証（Refs）。PR #2440

## 2026-10-28（#2418: codex の worker も tako の MCP とプロジェクトで足した MCP だけで起動 = `-c mcp_servers.<名>.enabled=false`・`projects mcp` に `codex_plan`）
- マトリクスの codex 列は supported。PR #2437

## 2026-10-28（#724: Windows の Web ビューの生成を App の借用の外で 1 本ずつ・Windows では ipc を付けない・生成中に落ちた Web ビューを次の起動で保留）
- macOS 実経路 sync 7/7・async 24/24・番犬は注入 2 通りで名指し・カタログ +0 B。Windows 実機は未確認（Refs）。PR #2444

## 2026-10-28（#2247: 「Finder で表示」を tako 自身の NSWorkspace で Finder を前面へ出す 1 実装にし、プレビュー・設定画面も dispatch へ寄せた）
- A/B `TAKO_2247_LEGACY=1`・実経路 18 PASS・カタログ +0 B。本物の Finder は install 後に確認（`tako todo` u-86）。PR #2441

## 2026-10-28（#2426: #2413 の e2e の 2 件 = 起動待ちから起こした run の run_status が `launch_waiting` のまま残る・e2e が master の役割を付けていなかった）
- e2e PASS 46 / FAIL 0（修正前 40 / 2）・単体 3 本は修正前 FAILED → 修正後 ok・カタログ +0 B。PR #2438

## 2026-10-28（#1599: Windows の `platform::process::terminate` を実装・remote の停止 2 経路を `stop_verified_daemon` へ寄せ、止められなければ Err）
- 注入で実プロセス 5 本 / 番犬 3 通りが名指しで FAILED・実 daemon で 3 経路を確認・カタログ +0 B。stale 経路は #2455。PR #2449

## 2026-10-28（CI の macOS ジョブの上限を 60 → 90 分へ・CI 待ちの既定 4800 → 6600 秒）
- PR の CI が重なると 60 分で打ち切られていた（10/27 に 2 本）。PR #2450

## 2026-10-28（#2434: Windows のアプリ内更新 = インストーラーの検査（取得元・大きさ・sha256・PE）→ 無言の起動 → tako が抜けてから入れ替え → 起動し直し）
- カタログ +194 B。実機の通しは未実測（Refs）。PR #2443

## 2026-10-28（#2432: Windows のシェル統合を `$PROFILE` に書かず、ペインの PowerShell を `-NoExit -EncodedCommand` で起こして読ませる。旧ブロックは移行 `powershell_profile` が外す）
- macOS の pwsh 実物で確認・Windows CI で Restricted 相当の e2e・カタログ +74 B。実機と psmux は未検証（Refs）。PR #2457

## 2026-10-28（#2435: Windows の `\` の形のパス（`C:\…`・UNC・`src\main.rs:10:5`）をリンクの候補にする・ignore 5 本を外す）
- 単体 68 本 + dispatch 2 本・CLI 実経路 8/8（macOS）・注入で 2 本が落ちる・カタログ +0 B。実機の Ctrl+クリックは未（Refs）。PR #2456
