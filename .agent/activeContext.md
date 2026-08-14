# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-15、Issue #658 worker レジストリの残留と GC 不全 = PR 提出）

> 直前に #790（worker 送達の 2 層化）が main へ入り install 済み（`f57e661`）。
> GUI 再起動が要るのは #790 と本件の両方（バイナリを差し替えたら 1 回で足りる）。

- **前提のズレ（重要）**: #658 は 2026-07-31 に「クローズ済み」だが、PR #701 の base は
  **`windows/467-ipc-orchestration-local`** であって main ではない。main には修正が
  1 行も無い（`dead_since` が存在しない）。本番 workers.yaml も 51/53/54/184 が
  `active` のまま・`dead_since` 未刻印で、症状は継続していた
- 対処は**再実装ではなく `ef89ca3` の main への移植**（設計は Windows 実機で検証済み。
  別実装を作ると windows/467 の将来マージで衝突が悪化する）

## 入った実装（移植 + main 適応）

- `registry.rs`: `DEAD_CONFIRM_SECS`（300 秒）/ `dead_since` フィールド / `liveness()` /
  `plan_sweep()` → `SweepPlan{mark,close,revive}` / `sweep_dead_at()`。
  **一覧の pane_alive・tmux_alive と GC が同じ `liveness()` を通る**
- GUI 経路の close（× / cmd+W / タブ close）をレジストリへ記録。main は
  `CloseReason::Explicit(CloseOrigin)`（#566）なので `reason.is_explicit()` へ適応
- セルフテストの隔離対象を `self_test_isolation_defaults()` に集約
  （`TAKO_WORKERS_FILE` / 新設 `TAKO_ORCHESTRATOR_DIR`）+ **セルフテスト項目 0**
- 仕様は `.agent/requirements.md` **FR-2.26** に新設（#390 は FR が無かった）

## 不変条件

- **消してよいのは「ペインも器も見えない状態が 300 秒続いた active」だけ**。
  1 回の観測では倒さない（`dead_since` を刻んで待つ）。生存の再観測で刻印は消える
- GC は status を倒すだけで**エントリを削除しない**（削除は別機構 = `MAX_WORKERS` 200 の
  保持上限で、古い closed から削る）。closed でも resume_command / report は引ける
- 同一性は「ペイン番号 + 器」。番号は再利用されるので追跡キーがあれば器の一致まで見る
- セカンダリインスタンスは GC を回さない（他人の worker を殺さない）
- `CloseReason::Exited`（PTY 死亡）では倒さない（#390 の追跡意図）
- **GC を回すのは「実ペインを持っているプライマリ GUI」だけ**。ペインを持たない
  インスタンスから叩くと、生きている worker が「見えない」= 死亡扱いになる
  （隔離検証で実際に起きた。本番の掃除は本番 GUI から叩くこと）

## 環境メモ（他 worker も踏む）

- **tako ペインの中から CLI を叩くと `TAKO_SOCKET` / `TAKO_TOKEN` が本番 GUI を指す**。
  `TAKO_DATA_DIR` / `TAKO_DISCOVERY_DIR` を隔離しても env が先に効くので本番へ届く。
  隔離検証は `env -u TAKO_SOCKET -u TAKO_TOKEN` を必ず付ける（今回 1 回踏んだ。
  本番 GUI が旧バイナリ = sweep 非搭載だったため実害ゼロ）
- 新しい worktree は `web/tako-remote/dist/` が無いと rust_embed でビルドできない

## 次の一手（master 判断）

- PR（`Closes #658`）→ CI 緑 → squash merge → install
- **本番の掃除は install + GUI 再起動のあと**（GC は GUI プロセス側で走るため、
  旧バイナリのままでは `workers` を叩いても倒れない）。手順は PR / 報告に記載
- windows/467 側は同じ変更を持っているので、将来のマージで registry.rs は衝突する
  （解決は「main 側 = 移植済み」を採る。#665 の `launch` 系は Windows 側にのみある）

## 現フェーズで Read すべき設計書

- GC の仕様: `.agent/requirements.md` FR-2.26 / `.agent/orchestrator.md`「workers」節
- 実装: `crates/tako-control/src/orchestrator/registry.rs`（`plan_sweep` / `liveness`）、
  `crates/tako-control/src/dispatch.rs`（`finish_workers_list` の sweep 分岐）
