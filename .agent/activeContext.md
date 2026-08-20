# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#467 Windows 対応 = スライス移植フェーズへ移行）

- master 裁定でマージは打ち切り、**`origin/main` を base にしたスライス移植**が正式方針
- 判断ログ・スライスの順序と依存関係: `.agent/plans/2026-08-windows-main-merge-wip.md`
- `windows/467-main-merge-wip`（`837684b`）は**判断ログとして残置**。マージは再開しない。
  PR #588 もマージしない（統合ブランチ `a9eac6e` は無変更）

## 決まったこと

- **#665**（起動保証 + 常時監視）= reopen して追跡継続。main の復旧サブシステム
  （#401 / #748 / #813）の上に作り直す。win467 版をそのまま持ってくるのは禁止
- **#662**（AskUserQuestion 対話）= main の #748 で代替済み。移植しない（close 維持）
- **#709**（`tako account`）= main の `tako orchestrator accounts`（#504 / #548）で
  代替済み。移植しない（close 維持）
- Windows 検証機の stale `tako.exe` 2 プロセスは kill 済み。**既定 target が使える**
  （kill 後 `cargo build --workspace` = 19.15s / exit 0 を実測）

## 次の一手

**スライス移植 第 1 弾 = `platform/` 境界（基盤）**。他の全スライスがここを呼ぶので最初にやる。
`origin/main` から `git worktree add` して、`origin/windows/467-ipc-orchestration-local` の
`platform/*` を持ち込み、main の現行 API に合わせて直す。詳細は plan ドキュメントの
「スライス移植の推奨順序と依存関係」節。

## 引き継ぎ時の注意（実測済み）

- **Windows 機からは push できない**（`gh` トークン無効 + GCM が SSH セッションで
  wincredman に触れない）。成果物は Mac 側で commit / push する
- Windows の `[Console]::OutputEncoding` は shift_jis。SSH 経由で git の UTF-8 出力を
  読むと文字化けするが**表示だけの問題**
- `#583` の Windows 既知失敗は 2026-08-21 時点で「12 件解消 / 6 件継続 / 新規 11 件 /
  psmux e2e 8 件（psmux 未導入の環境要因）」
- **`windows/467-ipc-orchestration-local`（PR #588 の head）のツリーには
  `docs/.../telemetry.md:83` に実メールアドレスが残っている**。保全 4 ブランチと
  WIP からは除去済みだが、このブランチは裁定の対象外だったので手つかず

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md` = 判断ログ + スライスの順序・依存関係
- `.agent/plans/2026-07-windows-port-architecture.md` = 抽象境界 B1〜B16 とパリティテストの正
- `.agent/windows-setup.md` = Windows 実機のビルド前提（psmux は §3.5）
