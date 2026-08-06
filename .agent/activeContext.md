# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-06、Issue #779 sleep guard の `ps` 起動削減）

- worktree `../tako-wt-779` / ブランチ `improve/779-sleep-guard-ps` / PR #783
- #772 の `ProcessSnapshot` を `agents` 側へ移して stale binary 検知と共有し、同じ tick で
  両方が必要とするプロセス一覧を 1 回の `tmux list-panes` + `ps` で賄う
- sleep guard の走査は初回・対象の backend / role / OSC 状態変化・モード有効化・60 秒経過時だけ。
  2 秒 tick ごとの assertion 適用は維持し、走査を省いた tick は直前の busy 集合を再利用する
- 走査対象はソートして HashMap 列挙順による偽の変化検知を防ぐ。走査と stale binary poll は
  どちらも background job 内で実行し、メインスレッドを外部コマンド待ちで止めない

## 実測証拠

- 隔離環境・worker role 6 ペイン・PATH shim の同条件で、アイドル時の `ps` 起動は
  before 34 回 / 約 75 秒 → after 3 回 / 約 72 秒（約 91% 減）
- 同じ計測の tako-app CPU は before 約 0.3% → after 約 0.6%。after は load average
  100 超の強い競合下での値だが、受け入れ目標の 10% を十分下回った
- 実経路で隔離アプリの worker に `sleep 30` を実行すると、その PID の
  `PreventUserIdleSystemSleep` assertion が保持され、終了後に解除された
- `TAKO_PERF_VERBOSE` の periodic 各区間は after の p50 / max が原則 0ms
- 2 秒周期の残りは main periodic と autorename poll。外部処理は状態変化・表示中・対象ありで
  条件化済みで、今回の実測から別 Issue が必要な重い常時処理は見つからなかった

## 検証状況

- `cargo test --workspace` / `cargo fmt --all --check` /
  `cargo clippy --workspace --all-targets -- -D warnings` は緑
- 最新 main（#781 の `8aeb939`）へ rebase 済み。競合は `activeContext` / `progress` の
  ドキュメント 2 件のみで、`main.rs` は自動マージ（#781 はペイン描画・本件は periodic の別領域）
- 隔離セルフテストは高負荷下で #771 型の無関係な GUI タイミング項目が 4 回フレークした
  （入力予測 zsh・#680・#181 が 2 回）。負荷の谷で完走結果を取り直すのが残作業

## 不変条件

- sleep guard の機能仕様、2 秒ごとの状態評価、assertion の取得・解放条件を変えない
- `busy_backend_sessions` は sleep guard 以外（GUI モード判定・close 確認）も読むので、
  モードが `while-agents-running` でなくても変化時 + 60 秒保険の走査は止めない
- 本番 GUI・本番 tmux socket `tako`・本番 data dir に触れない
- 検証は `TAKO_ISOLATED=1` + 専用 `TAKO_TMUX_SOCKET` + 専用 data / discovery dir
- System Events のキーストローク送出は禁止。隔離アプリは PID 指定で終了する
- 並走ビルドと同時に Cargo / app bundle ビルドを走らせない

## 次の手順

1. 負荷低下後に隔離セルフテストを再実行し `TAKO_APP_SELF_TEST_OK` を確認
2. force-with-lease で PR #783 を更新 → macOS CI 緑 → squash merge（`--delete-branch`）
3. Issue #779 に証拠をコメントしてクローズ → main 同期 → worktree 削除
4. 並走ビルドがないことを確認して `scripts/build-app.sh --install`。GUI 再起動は master 側

## 現フェーズで Read すべき設計書

- 性能・診断まわりを触るとき: `.agent/architecture.md`（periodic tick / perf.log 節）
- sleep guard の仕様確認: `.agent/requirements.md` FR-5.14
