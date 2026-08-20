# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#467 Windows 統合）

- ブランチ `windows/467-main-merge-wip`（worktree `~/dev/tako-wt-467`）
- `windows/467-ipc-orchestration-local` へ `origin/main`（bb3033a）を合流させる作業。
  **中断して保全した状態**。統合ブランチ本体と PR #588 は無変更（a9eac6e）

## 状態

- 保全済み: Windows 検証機の未 push 作業 4 件を origin へ（`windows/656-md-preview-cherry-pick-wip` /
  `windows/724-port-crash` / `windows/525-shell-integration` / `windows/727-sleep-settings`）。
  **Windows 機からは push できない**（gh トークン無効 + GCM が SSH セッションで
  wincredman に触れない）ので、bundle を Mac へ運んで Mac の認証で push した
- マージ: 45 衝突ファイルのうち 41 を解決してコミット（ddf880e、2 親）。
  未解決は `tako-app/src/main.rs`(44) / `tako-control/src/dispatch.rs`(40) /
  `tako-cli/src/main.rs`(13) / `docs/.../keyboard-shortcuts.md`(7)
- Windows 実機: `cargo build --workspace` 成功（5m24s / error 0 / warning 17）。
  ただし `target/debug/tako.exe` を **08-05 から残っている tako.exe 2 プロセス**が
  ロックしているため `--target-dir C:\Users\shioz\target-467` で回した

## 判断が要るもの（ユーザー / master 待ち）

`.agent/plans/2026-08-windows-main-merge-wip.md` に全部書いた。要点だけ:

1. **#665（起動保証 + worker 常時監視）をどうするか** — win467 が main の復旧
   サブシステム 434 行を意図的に削除して別設計に置き換えている。union は成立しない
2. **#662（AskUserQuestion 対話）** — main の #748 と機能が重複している
3. **#709（`tako account`）** — main の `tako orchestrator accounts`（#504 / #548）と重複
4. **進め方そのもの** — main は `tako-app/src/main.rs` だけで +25,075 行先行。
   マージを続けるより **win467 の Windows 対応を main へスライス移植**する方が安い

## 次の一手

- 上記 1〜4 の方針決定 → 決まった方針で再開（スライス移植なら本 WIP は参照用）
- PR #588 の CI は **conflict が解けるまで起動できない**（GitHub が
  `refs/pull/588/merge` を作れないため `pull_request` ワークフローが走らない。
  実測: run 0 件 / `mergeable=CONFLICTING`）

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md` = 今回の判断ログ・規模実測・推奨
- `.agent/plans/2026-07-windows-port-architecture.md` = 抽象境界 B1〜B16 とパリティテストの正
