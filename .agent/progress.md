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

## 2026-10-31（#2365 の続き: dispatch の `op_err` が載せる中身のうち日本語のままだった git の書き込み系（tako-core `GitError` 16 種・型の口 `*_typed`）と GUI の Web ビューの navigate / eval・URL の検査（`HostRefusal` 9 種）・固定文 2 つを表示言語へ）
- 実経路 隔離 GUI へ CLI 12 + MCP 4 で英語表示の日本語入り 15 → 0・日本語はバイト一致・GUI 無しの実経路 14 / 15 → 0 / 15・カタログ +0 B。PR #2556

## 2026-10-31（#2555: LSP の e2e が `stop`（待たずに返る）の直後に置き場を消し、偽サーバの `exit` の書き込みで `tako-2035-*` / `tako-2048-*` が消え残っていた → テストの最後は `shutdown_all`・置き場は `ScratchDir`・番犬 `issue2555_lsp_stop_then_scope_end_watchdog`）
- 修正前 10 回で 2035 は 20 個・2048 は 2 個 → 修正後 0 個・SIGKILL の残りは test-residue が数える・注入で番犬が 4 行を名指す・既存の 257 個を片付け・カタログ +0 B。PR #2558

## 2026-10-31（#2557: setup bootstrap の未知のシェルへの断りが `export PATH="~/.local/bin:$PATH"` を 4 行重ねていた → tako-core `shell_profile::dirs_for_unknown_shell`（書く行と同じ `$HOME/…`・重複なし）でディレクトリだけ並べ、構文はそのシェルの書き方に任せる）
- CLI / MCP（日英）とも 4 行 → 1 行・既知のシェル 7 種の rc と応答はバイト一致・注入 3 通りで file:line を名指し・カタログ +0 B。PR #2563

## 2026-10-31（#2561: MCP 設定のテストの `mcp_test_dir` と #944 の番犬の `scratch` を `ScratchDir` へ寄せ、子をプロセスグループの頭で起こして刈り取る前に孫を止める・番犬 `issue2561_temp_dir_helper_watchdog`）
- 修正前は専用の TMPDIR で 3 回走らせると 7→14→21 個（944 は偽 agy の注入で 3→6→9 個）、修正後はどちらも 0→0→0 個。本物の `$TMPDIR` から 976 個を消した（失敗 0）・カタログ +0 B。PR #2562

## 2026-10-31（#2365 の続き: 委任台帳・引き継ぎの書き込み・supervisor・後任 master の起動コマンドを `ConfigError`、非同期 run・手順書を `Reason`、設定値の範囲・paste-guard を tako-core の `RangeRefusal` / `PolicyRefusal` へ。置き場 21 か所は #2497 で済みと確認）
- 隔離 GUI の A/B で英語表示の日本語入り 24 / 24 → 0 / 24・日本語は sha256 一致・注入 12 か所を名指し + E0004・カタログ +0 B。PR #2565

## 2026-10-31（#2566: IPC のテストの偽の受け口を `ScratchDir` の下へ・dispatch / orchestrator のテストの補助が設定の置き場の OnceLock を後始末なしで先に初期化するのを外す・#2561 の番犬を String / ソケット / OnceLock の初期化子へ広げる）
- 修正前は専用の TMPDIR で 3 回走らせると `tk2123*` 3→6→9・dispatch-config 1→2→3、修正後はどちらも 0→0→0。本物の `$TMPDIR` から 341 個を消した（失敗 0）・カタログ +0 B。PR #2568

## 2026-10-31（#2559: GUI をペインから再起動すると自動命名の `claude -p` がペインの識別と mod の注入を受け継ぎ、master の `mod_turn` / `session_id` / `rate_limits` を haiku の報告で上書きしていた → 起動形で `helper_env` を外す + CLI が祖先を添え GUI がペインの外の報告を `foreign` で捨てる（A/B `TAKO_2559_LEGACY=1`））
- 隔離 GUI + 偽 claude で修正前は上書き・`ended` で報告消失 → 修正後 0・番犬 `issue2559_helper_report_watchdog`・カタログ +0 B。PR #2567
- 本番確認で #2559 を閉じた

## 2026-10-31（#2569: 台帳 / projects.yaml / accounts.yaml の `mutate` が閉包の断りでも書き戻し 0 件のファイル作成・手書きの形の書き直しをしていた → `try_mutate_typed`（Err なら書かない）へ寄せ、番犬 `issue2569_refusal_writeback` が file:line で名指す）
- 実経路（CLI / MCP・日英）で修正前 NG 32 → 修正後 0・出力 128 ファイルがバイト一致・カタログ +0 B。PR #2572

## 2026-10-31（#2365 の続き: 対応マトリクス・移行・使用量・起動時ロードの予算の断りを `Reason` の 10 種、remote 系（会話の末尾・端末・daemon の admin API・スクロールバック・セットアップ）を新しい `RemoteRefusal`（31 種）へ。daemon の本文に `kind` を添えて呼び手の言語で組み直す）
- 実経路で英語表示の日本語入り dispatch 27 / 27・CLI 18 / 22・MCP 15 / 15 → 0・日本語は sha256 一致・注入 6 通りで名指し + E0004・カタログ +0 B。PR #2573

## 2026-10-31（#2564: 未知のシェルで tako setup が同じ断りで止まる `tako setup bootstrap path` へ誘導していた → 残りは `AgentPathByHand`（次の 1 行なし・理由に足すディレクトリの全文）、tako CLI の段と `--check` は一覧で閉じる、足し終えたら断らず already_on_path）
- 未知のシェル 12 本で誘導 0・既知のシェル 60 ファイルがバイト一致・注入 5 通りで file:line を名指し・カタログ +0 B。PR #2571

## 2026-10-31（#1632: remote のトークン・QR・状態ファイルの置き場を Windows でも本人 + SYSTEM だけの DACL（継承を切る）にする。境界 B32 `platform::private_file`・A/B `TAKO_1632_LEGACY=1`・unix は同じ呼び出し）
- CI の Windows ジョブで Win32 と icacls の読み戻し（修正後 2 者・`(I)` 無し / 旧アームは継承のまま）・番犬が no-op への戻しを file:line で名指す・カタログ +0 B。PR #2575

## 2026-10-31（#2570: ペインの中から起こし直した GUI がペインの env（識別・接続・claude の印・`GIT_EDITOR=true`・重なった `XDG_DATA_DIRS`）を新しいペインと子へ素通しにしていた → `tako_core::host_env::STRIP` を main の最初で外し unix は exec し直す（A/B `TAKO_2570_LEGACY=1`））
- 隔離 GUI の A/B でペインと子の env が Dock 起動相当と一致・`ps eww` も外れる・番犬 `issue2570_inherited_pane_env_watchdog`・カタログ +0 B。PR #2574
- 19:03 に `3aac66e` を install・GUI 再起動（master のペインから `open -a` = この再 exec の経路・復元失敗 0）の本番確認で #2570 を閉じた

## 2026-10-31（#2365 の続き: tmux のセッション操作（`TmuxRefusal` / `TmuxRunError`。口は名前のまま型を返す）・git show・edit type・LSP の初期化オプション・worker の解決とレジストリ・layout の config.yaml・remote agents の断りを表示言語へ）
- GUI 無しの実経路で英語の日本語入り 26 / 27 → 0（残り 1 は git の stderr）・隔離 GUI の CLI / MCP 18 / 18 → 0 / 18・日本語は sha256 一致・注入 6 か所を名指し + E0004・カタログ +0 B。PR #2579

## 2026-10-31（#2576: 未知のシェルで welcome の次の一手と check-health の cli_in_path が `tako setup bootstrap path` へ誘導していた → welcome は `NextStep::PathByHand`（段は path のまま command null・dirs と断りの全文）、check-health は `--check` と同じ判定 `tako_cli_dir_by_hand` + `tako_cli_in_path_issue`（日本語固定））
- 未知のシェル 4 ケース × 5 出力で誘導 0・既知のシェルと足し終えた後の 48 ファイルがバイト一致・注入 5 通りで file:line を名指し・カタログ +0 B。PR #2580

## 2026-10-31（#2581: control.json と instances/・IPC のトークン・usage の塩・shortcuts・スマホから受け取ったファイル・ペインの最後の出力の置き場を境界 `platform::private_file` へ寄せ、Windows も本人 + SYSTEM だけの DACL に。境界は当てられない置き場で呼び手を止めず・同じ DACL は当て直さない）
- CI の Windows で Win32 と icacls の読み戻し・隔離 GUI の A/B で unix の mode と CLI / MCP の接続が修正前と一致・番犬が境界の外の本人だけの mode を file:line で名指す・カタログ +0 B。PR #2584
