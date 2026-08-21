# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21）

- **#467 Windows 移植はスライス 1〜7c / 9 が main へ入り、残りはスライス 8（棚卸し）だけ**
- **#865（セルフテストの方言対応）を merge**。Windows 実機のセルフテストが
  **項目 0〜92 まで到達**（修正前は項目 1b で FAILED = カバレッジ 0）。
  到達範囲表がそのまま棚卸しの材料になる
- #867（起動コマンドの env 前置き）も merge 済み。**判定が 2 本（`ShellDialect` /
  `LaunchSyntax`）並んでいるので #873 で一本化する**（#867 の worker が担当）

## #865 で入れたもの

- `platform::shell_dialect`（境界）= ペインへ**打ち込む文字列**の方言差。方言は OS ではなく
  `default_shell()` が選んだプログラムから引く純粋関数なので、macOS から PowerShell 側の
  生成結果を全部テストできる。`cmd.exe` / fish は `None`（呼び出し側が対象外を明示）
- セルフテストは「機能が無い項目」を能力で明示スキップ（`pdf::capabilities().text_layer` /
  `shell_integration::status().effective()` / 本物の tmux か / `MAIN_SEPARATOR`）。
  **直れば自動で検証が復活する**形
- 起票: #866（psmux の `=name`）/ #870（links の HOME 決め打ち）/ #872（2 枚目の
  ウィンドウで静かに終了）/ #875（実行ペインの `/bin/sh` 決め打ち）。#724 へ panic 位置を追記

## 次の一手

- **スライス 8（棚卸し）**: #865 の到達範囲表を使って `tako_theme` / `tako_open_file` /
  `tako_preview_view` 等を `Pending` から実測に合わせて倒す（セルフテストが実機で通している）。
  `tako_orchestrator_watch` を倒す前に `claude agents --json` の `$SHELL -l -c` 経路
  （Windows で必ず失敗）を見る必要がある
- 項目 93 以降を Windows で通すには **#766**（psmux 越しに OSC が届かない）が要る
- #873（方言判定の一本化）は #867 の worker が担当

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（「8 の前提: セルフテストの方言対応（#865）」節に
  到達範囲・スキップ理由・実機レシピ。7c 節に #867 の記録）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
