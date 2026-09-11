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

## 2026-09-11（#1208: remote_link_live の実時間比較は #1220 で修正済みと確認し、再現の作法を規約へ）
- Issue は #1220（`3a0ea26`）の重複で、現行 main のテストは既に `scan_counters` の量比較。番犬 `test_timing_watchdog` は `crates/*/tests` 全体を見ており（`tako-core` への注入も名指し）、旧版を戻すと `remote_link_live.rs:247: assert(… warm <= cold …)` で FAILED
- 症状解消を実測: 新実装は負荷下 330 回 + 同時 16 本 × 10 ラウンドで **0 FAILED**。旧実装は `yes` 負荷では 0/170 だが**バイナリ多重同時起動**で 3/160 反転（1 件は Issue と同じ `初回 24.5ms / 2 回目 38.8ms`）
- 検出力の差が決定的: リンク memo を殺すと旧は 2/10 しか落ちない（8 回見逃し）・所在 memo は 0/10。新は両方 10/10。コード修正は不要で #1208 は close、再現の作法だけ `.agent/conventions.md` へ残した

## 2026-09-11（#1300: trust_auto_accept_e2e の tmux 起動失敗を診断で割れる形にして真因を直した）
- 真因は見立て（#1265 と同じ PTY 枯渇）と別で**固定名の取り合い**。器のソケットもセッション名も定数なので、同じ機で `cargo test --workspace` が 2 本並ぶと片方が 0.03 秒で `duplicate session: tako1236new` に当たり、後始末の `kill-session -t <固定名>` は相手のセッションまで消す（失敗時の実測は PTY 103/511・ソケット 206 = 枯渇していない）
- 器の名前・起動・後始末・診断を `tests/common/tmux_e2e.rs` の 1 実装へ（pid つきソケット + 期限つき + 失敗時に stderr / そのソケットのセッション / PTY / ソケット / サーバー数 / load、**このプロセスの最後の 1 本**でだけ器を畳む）。tako-control の実 tmux e2e 7 本を寄せ、番犬 2 本が修正前ソースの 28 か所を名指し FAILED
- A/B: 2 本同時 × 110 ラウンドで旧（origin/main バイナリ）= 110/110 ラウンド FAILED（115 プロセス・全て duplicate）→ 新 = 0/110。診断の有無は `TAKO_1300_INJECT=duplicate` で同一バイナリ対比。「負荷で待ちが足りない」説は**否定**（固定の待ちのまま load 12.15 で 30 回 0 失敗。当初の 11/12 は自分の A/B ハーネスの `rm -rf` が走行中の作業 dir を消していた artifact で、並走 sweeper で同じ署名を再現）

## 2026-09-11（#962: ゾンビ pid テストの「2 秒以内に返る」を機構の観測値へ替えた）
- 真因は見立てどおり**アサートの取り方**（予算 2 秒に対して落ちる側のタイムアウト経路が 5 秒 = 桁が開いておらず、正常でも観測 1 回ぶんの `/bin/ps` が詰まると 10.07 秒まで伸びる）。`TerminationWait`（`via` / `polls`）+ `last_termination_wait()` を開け `via == Some(Zombie)` で固定。待ち本体は観測を差し替えられる `wait_for_termination_with`（`kill_stale_daemon` も同じ 1 実装へ）
- A/B `TAKO_962_LEGACY=1` + `TAKO_962_INJECT_DELAY_MS=1500`（`cfg(test)` 限定）: 旧 12/12 FAILED（`実際: 3.03s`）→ 新 **60/60 PASSED**。検出力は `TAKO_962_INJECT=blind_zombie` で `TerminationWait { via: None, polls: 48 }` を名指し FAILED。エッジ 4 形の戻り値は修正前と同一
- 番犬の走査を `crates/*/src/` へ拡大（#962 の現場は `src` の `mod tests` = 元から不可視）+ 絶対予算は根拠つき許可リスト制。修正前ソースで `remote.rs:6994` を名指し FAILED
## 2026-09-11（#946: ペインの TERM 注入を「最後に無条件上書き」へ寄せ、1b の失敗理由を名指しできるようにした）
- Issue の見立て（親の TERM がペインへ素通し）は**実測で否定**。修正前バイナリ（`f1af0cc`）を tako のペインの中（親 `TERM=tmux-256color`）から起こしても項目 1b は **3/3 通過**（`ok=true waited=0.1s`）。alacritty は `Options.env` を親 env の上に当てる（`tty/unix.rs:235`）ので注入は元から勝っていた。報告時点（`e703e40`）の 3/3 失敗は**同じ項目を落とす #1165 の固定 6.4 秒窓**で、9/8（`4d84695`）に解消済み
- 残っていた穴は「注入が既定でしかない」こと（`env.extend(options.env)` が後 = 呼び出し側の env 1 つで無言で消える）。`finalize_pane_env` で TERM / COLORTERM を**最後に上書き**へ寄せ、番犬 `端末申告の注入はoptions_envより後にある` が修正前ソースで FAILED。項目 1b の失敗時だけ `TAKO_SELF_TEST_946: cause=injected|inherited|unexpected|no-echo` を出す（SKIP にはしない）
- 実測: 修正後 × 継承あり / `-u TERM` とも `TAKO_APP_SELF_TEST_OK`（316s / 294s）。注入口 `TAKO_946_INJECT=inherit` では 1b が `cause=inherited`（観測 `tmux-256color,truecolor`）で FAILED。`cargo test -p tako-core --test pane_term_env` が親 TERM 6 系統 + `options.env` の `TERM=dumb` でも `xterm-256color` を実シェルで固定

## 2026-09-11（#1309: 新規 worktree で PWA の dist が無くてもビルドが通るようにした）
- 真因は #574 の手当てが CI にしか無かったこと。`crates/tako-control/build.rs` を新設し、`dist/index.html` が無ければ npm でビルドする（**既にある dist は触らない**。rust_embed の埋め込み元へ `rerun-if-changed` も張った）。npm 無し / npm 失敗は**1 行目に手順が出る**エラーで止める（空埋め込みで通す案は不採用 = 製品バイナリに PWA が入らない事故の余地を作る）
- PWA ビルドの正本を `scripts/build-pwa.sh` へ 1 本化（`build-app.sh` / `check-windows.sh` が呼ぶ。既定は毎回作り直す = #60、`--if-missing` は dist があれば何もしない）
- 実測: クリーン worktree で修正前 = `PwaAssets::get` 未定義 4 件で失敗 → 修正後は成功（npm ci + build が自動で走り dist 生成）。npm を PATH から外すと `scripts/build-pwa.sh` の 1 行案内で停止。no-op 再ビルド 3 回 = before 0.17/0.15/0.15s → after 0.16/0.17/0.17s。release rlib に dist のハッシュ付きアセット名を確認。テスト 10 本（偽 npm + 一時 dir、実 npm は起こさない）

## 2026-09-11（#1296: テストの pid ごとの data dir が消えずに溜まるのを直した）
- #944 の隔離は「本番の外へ倒す」までで消す仕掛けが無く、再起動でも消えない macOS の `TMPDIR` に積もっていた（実測 2,188 件 + `tako-agent-config-*` 784 件 = `du` で 115 MB）。`tako_core::test_residue` に後始末を 1 実装し、作る経路（`paths::test_data_dir`）が `arm_self_cleanup`（`libc::atexit`）+ `sweep_stale_on_start`（SIGKILL 分を次回起動で回収）を持つ形へ
- 消すのは**自分の pid か pid が生きていないもの**だけ。pid 再利用は「dir の作成時刻より後に始まったプロセス」として見分けて見送る（実測 19 件）。消す直前に生死と作成時刻を取り直して、列挙後に作り直された置き場を巻き込まない
- 既存残骸の口は `tako test-residue`（既定 dry-run・`--apply` で実削除・MCP `tako_test_residue`）。番犬 2 本（修正前ソースで 2 件 FAILED）+ 子プロセス実測 2 本 + 単体 12 本。A/B は `TAKO_1296_LEGACY=1`
