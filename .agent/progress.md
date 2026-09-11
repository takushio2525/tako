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

## 2026-09-11（#1292: send_input の queued 応答が前回の決着済み送達の顛末を名乗らないようにした）
- 真因は「積む前の状態」を未決着かどうかで分けていなかったこと。`prompt_delivery_states` はペインを閉じるまで消えないので、素通しすると 1 時間前の `gave_up` や 10 秒前の `delivered` を新しい送達が名乗る（後者は master が届いたと判断して**監視をやめる**）
- 判定は `tako_core::prompt_delivery::pending_predecessor`（`State::is_pending` = Queued / Waiting だけ）の 1 実装へ。絞り込みは `queued_json` の 1 箇所なので send フロー / Enter 単独 / tmux フォールバックが全部通る。MCP の説明文も一致させた
- 隔離 GUI + 模擬 TUI の実測: 1 通目を flow_timeout（120s）させた後の 2 通目が legacy `gave_up … elapsed=120s` → 修正後 `queued elapsed=0s`。番犬 3 本が修正前ソースを file:line 名指し FAILED

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

## 2026-09-11（#758: AI 自動命名の claude 呼び出しから MCP サーバーを外した）
- `autorename` の起動を `spawn_claude` 1 本へ寄せ `--strict-mcp-config` を付ける。**再試行の引き金は非ゼロ終了だけ**で、フラグ無しの再試行が通ったときにだけ「使えない」と学習する（打ち切り・起動失敗では再試行しない）。A/B は `TAKO_758_LEGACY=1`
- macOS 実測（順序交互 10 ラウンド。順序固定の初回 15 ラウンドは 2 番目の腕が系統的に遅く測り直した）: 命名プロンプト p50 18.4→16.9s（対応差の中央値 -3.8s・9/10）/ 最小プロンプト 5.8→3.7s / **子孫プロセス 17→2**。隔離 GUI の 1 対 1 は 32.8s→16.6s。`CLAUDE_TIMEOUT` は縮めない（最大 43.6s）
- 単体 6 本（フォールバックを外すと 3 本 FAILED = 検出力）。上限テストが**上限が効いていない**のを発見（kill しても stdout の `join` は孫 = MCP サーバーが握る限り返らない。実測 上限 0.6s が 30.3s）→ 読み出しを `mpsc` の期限付き受信へ

## 2026-09-11（#1309: 新規 worktree で PWA の dist が無くてもビルドが通るようにした）
- 真因は #574 の手当てが CI にしか無かったこと。`crates/tako-control/build.rs` を新設し、`dist/index.html` が無ければ npm でビルドする（**既にある dist は触らない**。rust_embed の埋め込み元へ `rerun-if-changed` も張った）。npm 無し / npm 失敗は**1 行目に手順が出る**エラーで止める（空埋め込みで通す案は不採用 = 製品バイナリに PWA が入らない事故の余地を作る）
- PWA ビルドの正本を `scripts/build-pwa.sh` へ 1 本化（`build-app.sh` / `check-windows.sh` が呼ぶ。既定は毎回作り直す = #60、`--if-missing` は dist があれば何もしない）
- 実測: クリーン worktree で修正前 = `PwaAssets::get` 未定義 4 件で失敗 → 修正後は成功（npm ci + build が自動で走り dist 生成）。npm を PATH から外すと `scripts/build-pwa.sh` の 1 行案内で停止。no-op 再ビルド 3 回 = before 0.17/0.15/0.15s → after 0.16/0.17/0.17s。release rlib に dist のハッシュ付きアセット名を確認。テスト 10 本（偽 npm + 一時 dir、実 npm は起こさない）

## 2026-09-11（#1296: テストの pid ごとの data dir が消えずに溜まるのを直した）
- #944 の隔離は「本番の外へ倒す」までで消す仕掛けが無く、再起動でも消えない macOS の `TMPDIR` に積もっていた（実測 2,188 件 + `tako-agent-config-*` 784 件 = `du` で 115 MB）。`tako_core::test_residue` に後始末を 1 実装し、作る経路（`paths::test_data_dir`）が `arm_self_cleanup`（`libc::atexit`）+ `sweep_stale_on_start`（SIGKILL 分を次回起動で回収）を持つ形へ
- 消すのは**自分の pid か pid が生きていないもの**だけ。pid 再利用は「dir の作成時刻より後に始まったプロセス」として見分けて見送る（実測 19 件）。消す直前に生死と作成時刻を取り直して、列挙後に作り直された置き場を巻き込まない
- 既存残骸の口は `tako test-residue`（既定 dry-run・`--apply` で実削除・MCP `tako_test_residue`）。番犬 2 本（修正前ソースで 2 件 FAILED）+ 子プロセス実測 2 本 + 単体 12 本。A/B は `TAKO_1296_LEGACY=1`
