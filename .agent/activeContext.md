# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#877（agents 走査の `$SHELL -l -c` 決め打ち）を解消**（PR #882）。抽象境界
  **B21（`platform::child_cmd`）** を新設し、「tako 自身がユーザーの環境で CLI を 1 回走らせる」形を
  そこへ寄せた。Windows は `platform::exe::find`（B16）で解決した実体を**直接起動**する
  （rc が無いので env 前置きが要らず `Command::env` / `env_remove` だけで確定）。
  **worker の状態が Windows でも agents 経由で取れる**ようになった
- **#875（実行ペインの `/bin/sh` 決め打ち）も解消**（PR #879）。#666 カード / #453 Code Runner /
  `tako show-command --run` が Windows で動く。セルフテストの停止位置は main と同じ項目 93（#766 待ち）
- **#873（方言判定の一本化）も merge 済み**（PR #878）。方言判定は
  `platform::shell_dialect::ShellDialect` の 1 本で、**enum の定義が 1 つだけであることを番犬テストが
  固定している**ので、方言が要る機能は新しい enum を作らず `from_program` を使い回すこと

## #877 で分かった実機の作法（次の worker が踏みやすい）

- **`SHELL` は Windows でも Process スコープに存在する**（SSH セッション由来。`User` / `Machine` は空）。
  GUI 起動の `tako.exe` には渡らないので、env 依存を測るときは `Remove-Item Env:SHELL` を先に打つ。
  打たないと**壊れている経路が半分動いて見える**（`-l -c "<前置き>; cmd"` の後半だけ走る）
- **claude は認証切れ（`Not logged in`）でも `agents --json` に載る**（`status` / `kind` / `sessionId` /
  `pid` が全部入る）= 認証が無い実機でもエージェント監視系を検証できる
- **器（psmux）越しのペイン対応付けは効く**。`psmux -u -L tako new-session` で作れば
  `tmux -L tako list-panes -a -F "#{session_name}…"` が**接頭辞なしの素の名前**で返る
  （`-L` を落として作ると名前空間が分かれて見えないだけ）
- 実機テストの突き合わせは**件数だけでなく失敗テスト名で**（同数のまま入れ替わる）。
  現ベースラインは **22 failed**（tako-control 15 / tako-core 7）

## 次の一手

- **スライス 8（棚卸し）**: #865 の到達範囲表 + #875 / #877 の実測で `tako_run` /
  `tako_run_interactive` / `tako_show_command` / `tako_orchestrator_watch` /
  `tako_orchestrator_worker_status` を `Pending` から倒せる材料が揃った
  （作法 4 に従いマトリクスは #875 / #877 では触っていない）
- **同型の一族**（`$SHELL -l -c` の直書き）が残っている: `tako-core/src/lib.rs` /
  `tako-app/src/autorename.rs` / `tako-app/src/preview.rs` /
  `tako-control/src/config_share/env.rs` / `tako-control/src/setup_bootstrap.rs`。
  **どれも B21（`child_cmd::user_env_cli`）へ寄せられる形**
- 項目 93 以降を Windows で通すには **#766**（psmux 越しに OSC が届かない）が要る
- **#881**（psmux の `cmd.exe /c` 包みが効かない = 空白入りプログラムパスの明示コマンドが即死）

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「agents 走査の Windows 対応（#877）」節に
  実機 A/B と作法。「#875 の記録」節に before/after 実測表。
  「8 の前提: セルフテストの方言対応（#865）」節に到達範囲・スキップ理由・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
