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

## 2026-09-11（#1261: 単体テストが実 agy / codex を起こして本番ホームへ書かないようにした）
- 起動元は空 HOME + 偽 CLI の shim で 2 本に特定（`setup_bootstrap::tests::状態は3系統ぶんまとめて返せる` = `agy models` / `codex login status` / `claude auth status --json`、`stale_binary::tests::test_check_stale_different_binary` = `claude --version`）。**tako が書かなくても実 CLI は起動しただけで自分のホームを作る**
- 問い合わせ起動を `tako_control::agent_probe::run` の 1 か所へ寄せた。判定は実行時（`paths::is_test_process`。#1253 の brew と同型）で、#586 のコンソール窓抑止もここが当てる
- 空 HOME の `cargo test --workspace` 実測: 36 → 1 ファイル（`.gemini` 32 / `.codex` 1 / `.claude` 2 が 0。残る `.zsh_history` は 0 バイトで修正前から同じ）。番犬 5 本が修正前ソースで FAILED

## 2026-09-11（#1265: osc7 系 3 テストの真因を測り直し、状態待ちへ寄せた）
- 見立て（固定 10 秒窓が短い）は**実測で否定**。修正前のまま CPU だけの負荷（load 11〜46 / PTY 99〜103・上限 511）で 150 回 = **0 FAILED**、OSC 7 の到達は 744〜1,171 ms（p50 858 ms・90 サンプル）で窓に 8 倍以上の余裕。元の 8/150 は PTY 404/511・tmux 135 本の枯渇（サーバーが `openpty` で止まる `sample`）の副作用だった。仮説②（`.zshenv` の競合）もテストの data dir が pid ごとに隔離（#944）されていて成立しない
- それでも固定窓は「窓が短い / 器が動いていない」を診断から消すので `wait_osc7_cwd` + `probe_osc7`（状態待ち + `state_wait_budget`・器のペインの `#{pane_dead}` / `#{pane_current_command}` / `#{pane_current_path}`・`ZDOTDIR` セッション/サーバー・`.zshenv` のバイト数）へ。**診断の採取にも期限**をつけた（器が応答しない場面でこそ要るのに素の `output()` では診断ごと固まる = #1271 の罠）
- A/B は `TAKO_1265_INJECT=late`（起動を 12 秒遅らせる）: 旧アーム（`TAKO_1265_LEGACY=1`）は **Issue と同じ画面**で 75/75 FAILED・新アーム 0/75。`nointegration` は両アーム FAILED（検出力）。番犬 `osc7_wait_watchdog` が固定回数窓の復活を落とす（修正前ソースで 9 件を名指し）

## 2026-09-11（#1277: codex / agy の「背景作業つき入力待ち」を MATRIX へ確定した）
- 隔離 tmux で実 CLI を起こして採取（codex-cli 0.154.0 / Antigravity CLI 1.2.0）。**両系統は一次シグナルが張り付かない**（背景作業が生きたまま rollout の `task_complete` / 実況 JSONL の終端が書かれ `read_turn_state` → idle）ので #1273 の覆す腕は要らず、MATRIX の `worker_idle_with_background` を 3 系統とも `Supported` へ
- 画面の申告は系統別に読めるようにした（codex = `1 background terminal running · /ps to view · /stop to close` / agy = フッターの `· 1 task(s) · /tasks`）。ただし**両系統の申告は生成中も同じ形で出る**ので `declaration_implies_turn_end` で claude 限定にし、番犬 3 本 + 単体 15 本 + A/B `TAKO_1277_LEGACY=1` で固定
- agy の実況行（`● [07:15:49] <コマンド> running`）は内訳にしない（コマンド全文が応答へ漏れる。#927）。docs 再生成で codex 34→35 / agy 23→24

## 2026-09-11（#1035: 番犬が gitignore 済みの未追跡ファイルで落ちないようにした）
- `no_personal_data` の走査対象を「public リポに出るファイル」= `git check-ignore` で**未追跡かつ ignore 済み**でないものへ限定。`.claude/settings.local.json` で手元が恒久的に赤い状態を解消（手元 2 failed → 6 passed）
- **索引を見る既定の `check-ignore` を使う**ので `git add -f` した tracked は ignore に一致しても走査に残る（`--no-index` 禁止 = `.gitignore` で隠す抜け道を塞ぐ）。`git` が無い / リポ外は全部走査（実測で旧挙動どおり 2 failed）
- 実測: tracked 注入 → ①②とも FAILED / 未追跡かつ非 ignore 注入 → ①②とも FAILED + 「CI は緑のまま」の案内行 / 未追跡かつ ignore 済み注入 → 緑。番犬 2 本追加

## 2026-09-11（#1293: 会話ログの番号つき箇条書きがダイアログの選択肢に混ざらないようにした）
- 真因は非対称。選択カーソルの探索は末尾 40 行に絞ってあるのに**収集（`numbered_rows`）だけが画面全体**だった。`numbered_block` で起点から上下へ「あいだがダイアログの一部だけ（`gap_line_kind`）」かつ「番号が 1 ずつ増える」あいだに限定し、経路 1〜4 すべてへ effect。`title` も同じ塊に限る（境界 = 罫線 + 0 桁の非空行 + 採らなかった番号つき行）
- A/B `TAKO_1293_LEGACY=1`: 単体 5 本が Issue と同じ `options=6`・`highlighted=Some(3)`・`header=["⏺ 直し方の候補は 3 つあります。"]` で FAILED → 修正後 3 択・header はダイアログの説明文のみ
- 実 tmux の模擬 TUI e2e（新規 2 本）: 下見が 3 択・`--choice 1` が `choice_text="Yes"` / `keys_sent=["1"]` / `resolved=true`。legacy は `choice_text="待ちを状態待ちへ寄せる"` = Issue の「監査ログが嘘になる」を再現

## 2026-09-11（#1282: `tako tmux cleanup --servers` を psmux の器でも使えるようにした）
- 器の列挙を 2 実装へ（unix = ソケットファイル走査 / Windows = 器のプロセスのコマンドライン `-L <名前>`。psmux は名前付きパイプで走査できるファイルが無く、実機に 24 個残っていても 0 件と答えていた）。コマンドラインは `NtQueryInformationProcess(ProcessCommandLineInformation)`・起動時刻は `GetProcessTimes` で、依存クレートは足していない
- 所有者は 2 段（ソケット名の pid = unix と同じ強さ / コマンドラインの隔離マーカー = 自分以外の生きた tako-app が居れば `peer_may_reattach` で見送る）。材料が無ければ従来どおり `owner_unknown`。回収は `taskkill /PID /T /F` の **pid 指定のみ**
- macOS: fmt / clippy / `test --workspace` 4074 件緑・クロスチェック エラー 0（警告は既存箇所のみ）。番犬 5 本（修正前ソースで 4 本 FAILED）+ 単体 11 本（応答の形は dispatch で固定）。FFI は CI の Windows ランナーで実行検査（既存の `cargo test --workspace` は #583 の失敗で tako-core まで届かないので blocking の別ステップを追加）。**Windows 実機実測は未取得**（機が offline。手順は plan の「#1282」節）

## 2026-09-11（#1294: peer 送達の「送ったかもしれない」を未達扱いにせず二重投函を止めた）
- `Stall::PeerSendStalled`（再送禁止の宣言）と registry の記録が逆を向いていた。送達の記録を 3 値（`tako_core::prompt_delivery::Confidence`）にし、`peer_send_stalled` / `peer_unconfirmed` は `Unverified`（#983 の `verify_then_resend`）へ。3 値目の宣言は `outcome_confidence` の表 1 本で、記録側と判定側が同じ表を引く（**既定は未達側**）
- A/B（`TAKO_1294_LEGACY=1`）: 旧アーム = `prompt_delivery=undelivered` / events `prompt_undelivered` / **自動再送 1 回**、新アーム = `unverified` / `prompt_delivery_unverified` / **0 回**。本当に未達（`paste_not_reflected`）は両アームとも 1 回で回帰なし
- 番犬 3 本（記録が bool を渡す形 / 判定が無条件 `OverdueSuspect` / 自動再送の引き金の一本化）が修正前ソースで file:line 名指し FAILED。単体 8 本・dispatch e2e 1 本

## 2026-09-11（#1300: trust_auto_accept_e2e の tmux 起動失敗を診断で割れる形にして真因を直した）
- 真因は見立て（#1265 と同じ PTY 枯渇）と別で**固定名の取り合い**。器のソケットもセッション名も定数なので、同じ機で `cargo test --workspace` が 2 本並ぶと片方が 0.03 秒で `duplicate session: tako1236new` に当たり、後始末の `kill-session -t <固定名>` は相手のセッションまで消す（失敗時の実測は PTY 103/511・ソケット 206 = 枯渇していない）
- 器の名前・起動・後始末・診断を `tests/common/tmux_e2e.rs` の 1 実装へ（pid つきソケット + 期限つき + 失敗時に stderr / そのソケットのセッション / PTY / ソケット / サーバー数 / load、**このプロセスの最後の 1 本**でだけ器を畳む）。tako-control の実 tmux e2e 7 本を寄せ、番犬 2 本が修正前ソースの 28 か所を名指し FAILED
- A/B: 2 本同時 × 110 ラウンドで旧（origin/main バイナリ）= 110/110 ラウンド FAILED（115 プロセス・全て duplicate）→ 新 = 0/110。診断の有無は `TAKO_1300_INJECT=duplicate` で同一バイナリ対比。「負荷で待ちが足りない」説は**否定**（固定の待ちのまま load 12.15 で 30 回 0 失敗。当初の 11/12 は自分の A/B ハーネスの `rm -rf` が走行中の作業 dir を消していた artifact で、並走 sweeper で同じ署名を再現）

## 2026-09-11（#758: AI 自動命名の claude 呼び出しから MCP サーバーを外した）
- `autorename` の起動を `spawn_claude` 1 本へ寄せ `--strict-mcp-config` を付ける。**再試行の引き金は非ゼロ終了だけ**で、フラグ無しの再試行が通ったときにだけ「使えない」と学習する（打ち切り・起動失敗では再試行しない）。A/B は `TAKO_758_LEGACY=1`
- macOS 実測（順序交互 10 ラウンド。順序固定の初回 15 ラウンドは 2 番目の腕が系統的に遅く測り直した）: 命名プロンプト p50 18.4→16.9s（対応差の中央値 -3.8s・9/10）/ 最小プロンプト 5.8→3.7s / **子孫プロセス 17→2**。隔離 GUI の 1 対 1 は 32.8s→16.6s。`CLAUDE_TIMEOUT` は縮めない（最大 43.6s）
- 単体 6 本（フォールバックを外すと 3 本 FAILED = 検出力）。上限テストが**上限が効いていない**のを発見（kill しても stdout の `join` は孫 = MCP サーバーが握る限り返らない。実測 上限 0.6s が 30.3s）→ 読み出しを `mpsc` の期限付き受信へ
