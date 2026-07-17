# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-17・#307 完了）

**Issue #307: 左サイドバーのドラッグリサイズ — PR #316 squash merge 済み**

- 右端のリサイズハンドル（右パネルと同方式。ホバーで ↔ カーソル + ドラッグ追従）
- 最小 120px / 最大ウィンドウ幅 50% のクランプ
- settings.json に sidebar_width を永続化（既定 244px、後方互換）
- CLI `tako panel --sidebar-width` / MCP `sidebar_width` で操作可能
- ツール数 94 不変（既存ツールにパラメータ追加のみ）

## 検証

- cargo fmt / clippy(-D warnings lib) / test 全緑（settings 6/6 含む）
- ドラッグの実挙動はユーザー目視確認待ち（Issue #307 に目視チェックリスト記載）

## 次の一手

- #307 のクローズは master 判断（目視確認後）
- install はユーザー指示どおり master 側で行う

## 現フェーズで Read すべき設計書

- 特になし（直近タスク完了）
