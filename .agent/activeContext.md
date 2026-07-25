# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-25・#500 Part 5-7 → PR #506 レビュー待ち）

**ブランチ `feat/500-part5-7`（PR #506）**

- Part 5: Profile に cwd フィールド。master 起動時に指定 cwd へ cd（~ 展開 + 存在検証）
- Part 6: master 起動後に cwd + projects のフォルダをファイルツリーへ自動追加（IPC 経由）
- Part 7: projects 指定ありプロファイルで「専任マスター」。system prompt に Assigned Projects 注入
  + 担当外は説明して断る指示 + 未登録 key は起動時エラー

## 次の一手

- PR #506 をレビュー → squash merge（Closes #500 で #500 完結）→ install
- #500 の全 Part（1〜7）が完了

## 現フェーズで Read すべき設計書

- cwd / projects 注入: `generate_identity_section`（orchestrator/mod.rs）
- ファイルツリー追加: `orchestrator_master`（tako-cli/main.rs）の TreeFolder dispatch
