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

## 2026-09-11（#638: 状態ファイルの tmp 名を書き込み 1 回ごとに分けた）
- 真因は #625 と同型。tmp 名が pid 止まりなので A の rename 後に B が**本番ファイルになった同じ inode** へ書き込む（実測の途中状態 `len=8194 head="S\ns\nLLLLLLLL" tail=…lll` = 短い本文が長い本文の先頭を潰した形）+ B の rename は ENOENT で失敗
- `shell_integration::write_state_file` を `tmux_backend::write_conf_in` と同じ作法へ（pid + `AtomicU64` の seq・rename 失敗時は tmp を掃除）。tmp 名は純粋関数 `state_tmp_name` に出して両アームを単体で固定
- A/B `TAKO_638_LEGACY=1`: 8 スレッド × 300 書き込みの再現テストが旧アーム **50/50 FAILED** → 新アーム **0/100**（CPU 負荷 load 92〜113 下でも 0/50）
## 2026-09-11（#1208: remote_link_live の実時間比較は #1220 で修正済みと確認し、再現の作法を規約へ）
- Issue は #1220（`3a0ea26`）の重複で、現行 main のテストは既に `scan_counters` の量比較。番犬 `test_timing_watchdog` は `crates/*/tests` 全体を見ており（`tako-core` への注入も名指し）、旧版を戻すと `remote_link_live.rs:247: assert(… warm <= cold …)` で FAILED
- 症状解消を実測: 新実装は負荷下 330 回 + 同時 16 本 × 10 ラウンドで **0 FAILED**。旧実装は `yes` 負荷では 0/170 だが**バイナリ多重同時起動**で 3/160 反転（1 件は Issue と同じ `初回 24.5ms / 2 回目 38.8ms`）
- 検出力の差が決定的: リンク memo を殺すと旧は 2/10 しか落ちない（8 回見逃し）・所在 memo は 0/10。新は両方 10/10。コード修正は不要で #1208 は close、再現の作法だけ `.agent/conventions.md` へ残した

## 2026-09-11（#1300: trust_auto_accept_e2e の tmux 起動失敗を診断で割れる形にして真因を直した）
- 真因は見立て（#1265 と同じ PTY 枯渇）と別で**固定名の取り合い**。器のソケットもセッション名も定数なので、同じ機で `cargo test --workspace` が 2 本並ぶと片方が 0.03 秒で `duplicate session: tako1236new` に当たり、後始末の `kill-session -t <固定名>` は相手のセッションまで消す（失敗時の実測は PTY 103/511・ソケット 206 = 枯渇していない）
- 器の名前・起動・後始末・診断を `tests/common/tmux_e2e.rs` の 1 実装へ（pid つきソケット + 期限つき + 失敗時に stderr / そのソケットのセッション / PTY / ソケット / サーバー数 / load、**このプロセスの最後の 1 本**でだけ器を畳む）。tako-control の実 tmux e2e 7 本を寄せ、番犬 2 本が修正前ソースの 28 か所を名指し FAILED
- A/B: 2 本同時 × 110 ラウンドで旧（origin/main バイナリ）= 110/110 ラウンド FAILED（115 プロセス・全て duplicate）→ 新 = 0/110。診断の有無は `TAKO_1300_INJECT=duplicate` で同一バイナリ対比。「負荷で待ちが足りない」説は**否定**（固定の待ちのまま load 12.15 で 30 回 0 失敗。当初の 11/12 は自分の A/B ハーネスの `rm -rf` が走行中の作業 dir を消していた artifact で、並走 sweeper で同じ署名を再現）

## 2026-09-11（#962: ゾンビ pid テストの「2 秒以内に返る」を機構の観測値へ替えた）
- 真因は見立てどおり**アサートの取り方**。予算 2 秒に対して落ちる側（`daemon_stop_impl` の タイムアウト経路）が 5 秒で桁が開いておらず、正常でも観測 1 回ぶんの `/bin/ps`（fork+exec）が詰まれば所要が 10.07 秒まで伸びた。`TerminationWait`（`via` / `polls`）+ `last_termination_wait()` を開け、`via == Some(Zombie)` で固定。待ち本体は観測を差し替えられる `wait_for_termination_with` にして回数と経路を実時間なしで単体固定（`kill_stale_daemon` の待ちも同じ 1 実装へ寄せた）
- A/B `TAKO_962_LEGACY=1` + `TAKO_962_INJECT_DELAY_MS=1500`（どちらも `cfg(test)` 限定）: 旧アーム 12/12 FAILED（`実際: 3.03s` = Issue と同じ形）・新アーム **60/60 PASSED**（load 4〜13）。検出力は `TAKO_962_INJECT=blind_zombie`（#619 前の誤判定）で新アサートが `TerminationWait { via: None, polls: 48 }` を名指し FAILED。素の fork 圧では旧アームも 12/12 通る = 棚卸しの実測どおり注入が要る
- 番犬は走査を `crates/*/src/` へ広げ（#962 の現場は `src` の `mod tests` で**最初から見えていなかった**）、絶対予算は根拠つき許可リスト制に。修正前ソースで `remote.rs:6994（daemon_stop_implはゾンビpidを終了済みとして扱う）` を名指し FAILED。src 走査で露出した既存の誤検知（同名の `let waited = ….elapsed()` を 3 つと数えて `main.rs:70010` を拾う）も畳んだ

## 2026-09-11（#1296: テストの pid ごとの data dir が消えずに溜まるのを直した）
- #944 の隔離は「本番の外へ倒す」までで消す仕掛けが無く、再起動でも消えない macOS の `TMPDIR` に積もっていた（実測 2,188 件 + `tako-agent-config-*` 784 件 = `du` で 115 MB）。`tako_core::test_residue` に後始末を 1 実装し、作る経路（`paths::test_data_dir`）が `arm_self_cleanup`（`libc::atexit`）+ `sweep_stale_on_start`（SIGKILL 分を次回起動で回収）を持つ形へ
- 消すのは**自分の pid か pid が生きていないもの**だけ。pid 再利用は「dir の作成時刻より後に始まったプロセス」として見分けて見送る（実測 19 件）。消す直前に生死と作成時刻を取り直して、列挙後に作り直された置き場を巻き込まない
- 既存残骸の口は `tako test-residue`（既定 dry-run・`--apply` で実削除・MCP `tako_test_residue`）。番犬 2 本（修正前ソースで 2 件 FAILED）+ 子プロセス実測 2 本 + 単体 12 本。A/B は `TAKO_1296_LEGACY=1`
