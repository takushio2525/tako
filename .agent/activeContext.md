# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-14・#226 setup のマルチエージェント対応）

**実装・検証完了、PR マージ待ち**（worktree tako-wt-226 / feat/226-setup-multiagent）:

- claude / codex / agy のインストール・認証・取得可能なプランを検出し、単一 CLI は自動選択、
  複数 CLI は対話選択する setup フローを実装
- 自動取得できない Claude / GPT / Google プランは対話フォールバックし、規模に応じて
  master_agent / worker_agents / effort / worker_model_policy を推奨生成
- setup-context、`--check` / `--changes`、system prompt、changes revision 8、ドキュメントを同期
- Issue #226 に実地調査結果をコメント済み。認証情報は読み取りのみで、保存・ログ出力なし
- 隔離 HOME / PATH で「claude のみ」「3 CLI から codex 選択」を実測。build / fmt / clippy /
  workspace test / docs build / setup 検証スクリプトは全緑

## 次の一手

- コミット → origin/main rebase → push → PR（Closes #226）→ CI 確認 → squash merge
- Issue #226 に実測証拠付き完了コメント。アプリ install は master 側で実施

## 現フェーズで Read すべき設計書

- setup 要件: `.agent/requirements.md`（FR-2.14）
- setup 実装: `crates/tako-cli/src/setup.rs`、`crates/tako-control/src/setup.rs`
- setup 追従資材: `resources/setup/changes.yaml`、`resources/setup/system-prompt.md`
