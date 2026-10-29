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

## 2026-10-28（#2445: wry 0.55.1 → 0.57.1・#724 の番犬を 0.57 の根拠へ・要素の全画面表示は切ったまま・背景の生成で前面に出ないことを番犬で固定）
- 隔離セルフテスト OK・実経路 PASS 16・カタログ +0 B。Windows 実機と全画面の出入りは未実測。PR #2451

## 2026-10-28（#641: 復元の直後に全ペインが PTY 死亡しても終了せず器なしのシェルで続ける `restore_guard`・連続 2 回で次の起動は復元を見送る）
- 隔離 GUI の実経路 PASS 43（修正前は終了 → 救済・見送り）・カタログ +0 B。Windows 実機は未確認。PR #2453

## 2026-10-28（#1510: 初回起動で副作用の小さい setup の段を自動で走らせ、「次にやること」をバナーに出す・導入とログインはボタン 1 つの同意・`tako welcome next|auto|hide`）
- 実経路 PASS 46（初回 / 未認証 / 完了 / オフライン / 途中で落とす）・カタログ +141 B。PR #2460

## 2026-10-28（#2455: PID ファイルが消えた remote daemon を `tako remote stop` で止める = 管理トークンの停止要求の応答から pid を得る・status に `pidless_listener`）
- 隔離の実 daemon で修正前は生き残る → 修正後は `via: shutdown` で停止・注入で 4 本が FAILED・カタログ +0 B。PR #2463

## 2026-10-28（#2448: テストが残す `sleep 600` は probe の「少し出してから固まる子」の孫 → `exec` にし、一巡の番犬がプロセスグループの残りを数える）
- PR #2465

## 2026-10-28（#2431: Windows の WebView2 のユーザーデータを `%LOCALAPPDATA%\tako\WebView2` へ（移行 `webview2_data`）・生成の失敗を無い / 古い / その他に分けて案内・check-health に版）
- PR #2464

## 2026-10-28（#2458: ipc の残骸のテストが全体実行で Live と読む = drop した受け口の fd を並走テストの子が握っていた → listen しないソケットで作る）
- 修正前は 19,998 回中 6 回通る → 0 回・注入で修正前 5/5 FAILED → 修正後 5/5 ok・カタログ +0 B。PR #2467

## 2026-10-28（#2454: Windows のペイン・言語サーバの子孫を Job Object で回収・ペインを閉じたときは GUI のアプリとその子孫を残す・境界 B30 `platform::job`）
- Windows CI の実プロセスで孫の停止と GUI の子孫の残りを確認・カタログ +0 B。実機と psmux は未（Refs）。PR #2461

## 2026-10-28（#2466: remote stop が起動 → 記録 5 秒超の本物の daemon を「pid 再利用」と断り state を消していた → 起動時刻を片側照合・断るときは state を残す）
- 隔離の実 daemon で修正前は取り残す → 修正後は停止・注入で 4 本が落ちる・カタログ +0 B。PR #2468

## 2026-10-28（#2424: 時間の番犬を束縛した関数の中だけで数える・#2413 の復元の時差のテストを gate の止めた時計で測る）
- 注入で修正前は番犬が補助関数を 3 か所名指し・時差テストは 5/5 FAILED → 修正後 ok・カタログ +0 B。PR #2469

## 2026-10-28（#2447: 再起動をまたぐ復元とカタログの再開にも spawn と同じ MCP の断片 = `worker_mcp_args` / `prepare_for_resume` の 1 実装）
- 隔離 GUI の実経路で修正前は復元の断片 0 個 → 修正後は spawn と一致・カタログ +0 B。突然死の復旧案内は #2470。PR #2471

## 2026-10-28（#2462: Windows の probe の打ち切り・`tako queue` の取り消し・AI 命名の上限・session-restart の孫を #2454 の Job の境界で回収 `KillScope`）
- Windows CI の実プロセスで孫が止まり（旧アームでは残る）、普通に終わった子の常駐は残ることを確認・カタログ +0 B。実機は未（Refs）。PR #2472

## 2026-10-29（#2473: 復元・sessions resume・会話を保った再起動の worker を spawn と同じ `build_worker_cmd_in` の 1 実装へ。落ちていた --permission-mode auto・--remote-control 等を戻した）
- 隔離 GUI + 偽 claude / codex で修正前 FAIL 6 → 修正後 PASS 26・番犬の注入 10 通り・カタログ +0 B。PR #2476

## 2026-10-28（#2446: 隔離セルフテストの項目 88 が本物の `tako setup` を完走させ項目 111 が落ちていた → `self_test::withdraw_launch` で取り下げ・番犬 `selftest_launch_withdraw_watchdog`）
- 修正前 `statusbar=Some(3)` → 修正後 `Some(100)`・カタログ +0 B。97(e) の本物の master の起動は残り。PR #2475

## 2026-10-28（#2415: dispatch.rs の実 PTY テスト 3 本の fixture を `$SHELL -l` から `/bin/sh -c "exec sleep 600"` へ・番犬が素の `SpawnOptions::default()` を名指し）
- 修正前 20/20 FAILED（ssh 探索の 5 秒に rc が alt screen へ入る形）→ 修正後 60/60 ok・カタログ +0 B。PR #2474
