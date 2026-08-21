# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#875（実行ペインの `/bin/sh` 決め打ち）を解消**（PR #879）。#666 カード / #453 Code Runner /
  `tako show-command --run` が Windows で動くようになり、**セルフテスト項目 91 の SKIP が消えて
  実行検査が緑**。セルフテストの停止位置は main と同じ項目 93（#766 待ち）で不変
- **#873（方言判定の一本化）も merge 済み**（PR #878）。`LaunchSyntax` は廃止され、方言判定は
  `platform::shell_dialect::ShellDialect` の 1 本。**enum の定義が 1 つだけであることを番犬テストが
  固定している**ので、方言が要る機能は新しい enum を作らず `from_program` を使い回すこと
  （#875 もそうしている）。知らないシェルを POSIX へ倒すかは呼び出し側の判断

## #875 で入れたもの

- `platform::shell::run_pane_command(command, marker_prefix)`（境界 B1）= 実行ペインの起動コマンド。
  POSIX は従来の直書きとバイト一致、Windows は PowerShell へ `-EncodedCommand`（base64 / UTF-16LE）
- `platform::shell::declared_shell_command(shell, command)` = `tako:shell` 宣言の包み方。
  判定は同じ `from_program` 1 本で、知らないシェルは POSIX 形のまま
- マーカーの正は `dispatch::EXIT_MARKER_PREFIX` の 1 個（組み立て側と `find_exit_marker` が共有）
- セルフテスト項目 91(d) の「PTY が立たないときだけ実行検査を外す」緩和を撤去（SKIP ではなく FAILED へ）

## 次の一手

- **スライス 8（棚卸し）**: #865 の到達範囲表 + #875 の実測で `tako_run` / `tako_run_interactive` /
  `tako_show_command` も `Pending` から倒せる材料が揃った（作法 4 に従いマトリクスは #875 では触っていない）
- 項目 93 以降を Windows で通すには **#766**（psmux 越しに OSC が届かない）が要る
- **#881**（psmux の `cmd.exe /c` 包みが効かない = 空白入りプログラムパスの明示コマンドが即死）。
  8.3 短縮名が通ることまで実測済み。`tako split -- "C:\Program Files\x\y.exe"` に波及する

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「#875 の記録」節に before/after 実測表。
  「8 の前提: セルフテストの方言対応（#865）」節に到達範囲・スキップ理由・実機レシピ）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
