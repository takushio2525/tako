# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#459 設定画面 M4〜M7 完了 → PR 作成待ち）

**#459 設定画面（Cmd+,）: M1〜M3 は PR #461 merge 済み。M4〜M7 を feature/459-settings-m4-m7 に実装完了**

- M4: Code Runner タブ（拡張子テーブル + user/builtin バッジ + リセット + 変数ヘルプ）
- M5: セットアップタブ（CLI 検出 / FDA / MCP 登録 / ルール同期 / tako setup 起動）
- M6: スリープ防止（ラジオ 3 セット）+ リモート（状態 + 開始/停止）+ 高度（JSON 表示 + 関連ファイル）
- M7: 1:1 監査完了（全項目が既存 CLI/MCP で操作可能。新設 dispatch 不要）
- テスト修正: テーマテスト回帰 / MCP ツール数 109→110 / clippy field_reassign
- 品質ゲート: cargo test 1193+ / fmt / clippy(-D warnings) 全緑

## 次の一手

- push → PR（`Closes #459`）→ squash merge → main 同期
- `build-app.sh --install` → 実機確認

## 現フェーズで Read すべき設計書

- 設定画面設計: `.agent/plans/2026-07-settings-ui.md`
