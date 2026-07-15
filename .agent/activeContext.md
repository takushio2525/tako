# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-15・#262 setup UX 全面見直し）

**根本原因の隔離実測まで完了、実装着手前**:

- v0.5.3（`6a4e06e`）で実ユーザー設定を隔離コピーし、認証照会だけ実 CLI へ委譲
- claude=max / codex=free / agy=取得不能を検出。現行 setup は 1 回目・2 回目とも
  CLI 側だけで 5 問（スリープ、agent、Max 倍率、Google、profile 更新）
- GPT の検出値だけは採用。保存済み `selected_agent` / `provider_plans` は 2 回目も未使用
- 根因は config 読み込み順、全 provider 無条件巡回、設定済み項目の再質問、
  profile の無差分確認、CLI 後の agent 二重対話
- Issue #262 に着手コメントと実測・根本原因コメントを投稿済み
- 詳細: `.agent/investigations/issue-262-setup-ux.md`

## 次の一手

- 方針 A: 検出値の実効化と source 表示
- 方針 B: 前回設定引き継ぎと idempotent 化
- 方針 C: 最終確認 1 回と `tako setup --yes`
- 4 シナリオ、設定破損、検出値不一致、全品質ゲートを実測

## 現フェーズで Read すべき設計書

- Issue: #262（方針 A/B/C・受け入れ条件 6 項目）
- 調査: `.agent/investigations/issue-262-setup-ux.md`
- 要件: `.agent/requirements.md`（FR-2.14.7）
- 実装: `crates/tako-cli/src/setup.rs` / `crates/tako-control/src/setup.rs`
