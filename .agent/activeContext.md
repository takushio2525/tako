# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-15・#262 setup UX 全面見直し）

**要件 A〜E の実装と検証を完了**:

- 検出値 → 前回値 → 安全な既定値を source ラベルつきで自動解決
- 認証済み CLI 1 つの標準ケース、2 回目、`--yes` を質問・入力 0 回へ変更
- 2 回目の `config.yaml` は実変更なしなら byte-for-byte 不変
- 検出値と前回値の競合は両方を通知して検出値を優先
- 破損 config は書き込まず停止、未認証はログイン手順つきで停止
- `--answers <json|@file|->` で全項目を非対話指定
- dispatch `SetupRun` / MCP `tako_setup` / CLI を 1:1 接続
- AI に日本語で希望を伝えて setup を代行する導線を docs に記載
- 個別対話は明示的な `tako setup --review` に分離
- setup revision 9、workspace v0.5.4、CHANGELOG を同期

## 検証

- 実 Claude Max 認証を読み取り専用で参照し、スクラッチ HOME へのみ書き込んで
  `[detected]`・入力 0・質問 0・完走を確認
- 初回 / 2 回目 / `--yes` / 未認証、プラン不明、検出競合、破損 config、
  全 answers、複数 CLI を隔離 E2E で確認
- workspace build / fmt / clippy / test、docs build は全緑
- 詳細: `.agent/investigations/issue-262-setup-ux.md`

## 次の一手

- #262 のコード・検証側に残作業なし。delivery 状態は Issue / PR を正とする
- install はユーザー指示どおり master 側で行う

## 現フェーズで Read すべき設計書

- Issue: #262（方針 A〜E・受け入れ条件 6 項目）
- 調査と実測: `.agent/investigations/issue-262-setup-ux.md`
- 要件: `.agent/requirements.md`（FR-2.14.7〜FR-2.14.9）
- 実装: `crates/tako-cli/src/setup.rs` / `crates/tako-control/src/setup.rs`
