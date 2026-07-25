# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-25・#500 Part 1-4 + #504 → PR #505 レビュー待ち）

**ブランチ `feat/500-profile-env`（2 コミット・PR #505）**

- #500 Part 1-4: Profile に汎用 env マップ追加。master/worker 全経路にenv注入、
  direnv 対策（export 後勝ち）、内部変数拒否、値マスク、projects 強制、CLI/MCP 1:1
- #504: accounts.yaml レジストリ（CRUD + 116 ツール）、spawn の account パラメータ、
  プロファイルの master_account/worker_account、model/effort 解決順

## 次の一手

- PR #505 をレビュー → squash merge → `scripts/build-app.sh --install`
- 隔離環境での実測（env 付きプロファイル + アカウント切替の e2e）
- Part 5-7（cwd / ファイルツリー / 専任マスター）は別タスク

## 現フェーズで Read すべき設計書

- env 注入: `crates/tako-control/src/orchestrator/mod.rs` の `Profile::validate_env` / `resolved_env_with_account`
- accounts: `AccountsConfig` / `ResolvedAccount`（同ファイル）
- spawn 経路: `dispatch_orchestrator_spawn`（dispatch.rs）の account 解決フロー
