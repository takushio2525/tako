# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#467 Windows 移植はスライス 7b まで完了）

- **スライス 7b は PR #869 で main へ**。#640（器が起動直後の入力を落として起動コマンドが
  全損する）を送達確認ステートマシンで根治。実機で 旧 0/4 → 新 4/4 到達、製品経路の
  spawn も 5/5 全文到達・5/5 実行
- **残りはスライス 8（対応マトリクスの棚卸し）だけ**。1〜7b のすべてに依存するので最後

## 実装（完了）

- `tako_core::shell_send` = 純粋ステートマシン（PTY もタイマーも持たないので欠落を
  テストで再現できる）。`WaitReady → WaitEcho → WaitSubmitted` を回し、**Enter は本文が
  画面に全文見えているときしか撃たない**。経過時間では打ち切らず「進みが止まったとき」だけ
  壊れたと判断する（ただし画面が動き続ける環境用に絶対上限も持つ）
- 4 経路（spawn / handoff / sessions resume / git resolve）が `queue_command_flow` を通る。
  駆動は tako-app の 500ms tick（`drive_command_flows`）。段階遷移は `TAKO_FLOW_DIAG=1` で
  `flow_log` へ（本文・画面内容は出さない）
- 起動コマンドが未達のペインではプロンプト送達フローを開始しない（素のシェルへプロンプト
  本文を貼り付けるのを防ぐ）

## 次の一手

- **スライス 8（対応マトリクスの棚卸し）**。7b までの実測を Supported / Degraded の判断材料に
- 7b が残した宿題: **#867**（届いた起動コマンドが PowerShell 構文でない = `VAR=value cmd` の
  env 前置き。#640 の 4 経路と master / solo が該当。#865 の `shell_dialect` は
  セルフテスト用の境界なので製品側は対象外）
- スライス 9 が残した宿題: #724 症状②（WebView2 の借用 panic）/ #727（設定画面のスリープ系）

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（スライス 8 節 + 引き継ぎ・作法 12 項目。
  7b 完了記録に「#640 より後の main 変更との衝突」3 件の実例）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界の定義）
