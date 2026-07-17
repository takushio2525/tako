# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#333 squash merge 済み）

**Issue #333: エラーレポートの自動送信基盤（テレメトリ）— PR #345 squash merge 済み**

- Worker デプロイ済み（https://tako-error-collector.takushio2525.workers.dev）
- 人工レポート → 到達確認済み
- Issue 証拠コメント済み。クローズは master 判断

## 次の一手

- `build-app.sh --install` → tako 再起動で新バイナリ反映
- Issue #333 のクローズ判断（master）

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
