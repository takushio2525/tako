# tako — エージェント向けガイド

AI 駆動・エージェント集約監視に特化した OSS GUI ターミナル。
iTerm2 + Zed の思想で Zed 級に高速・軽量。macOS 先行、Windows 対応必須。Apache-2.0。

> このリポジトリの AI 向け規約はここに集約してある。詳細仕様は `.agent/` を参照。
> 人間向けの説明は `README.md` にある。

## 概要

- 目的: AI エージェント（Claude Code 等）+ 子エージェント + dev サーバーを「1 グループ = 1 タブ」で集約監視する
- 対象: AI エージェントで開発する開発者。**ただしゼロコンフィグで一般ユーザーが使えることが最優先の設計原則**
- 状況: **仕様策定フェーズ完了 → Phase 0（技術検証スパイク）待ち。コードは未着手**

## 技術スタック

| 領域 | 採用 | 補足 |
|---|---|---|
| 言語 | Rust | |
| UI | GPUI（Zed 製） | **pre-1.0・破壊的変更頻発・Windows 対応進行中**。リスクと対策は `.agent/architecture.md` |
| ターミナル | alacritty_terminal | |
| テスト / Lint | cargo test / fmt + clippy（-D warnings） | コード着手後に CI 化 |

## ディレクトリ規約

```
tako/
├── AGENTS.md / CLAUDE.md   ← AI 向け規約（このファイル）
├── .agent/                 ← AI 向け詳細仕様（下記参照）
├── README.md / LICENSE     ← 人間向け・Apache-2.0
└── (crates/ 等)            ← Phase 1 で tako-core / tako-control / tako-app / tako-cli を切る
```

- `.agent/` に置くもの: AI 向け仕様・作業文脈。置かないもの: 人間向け紹介文（README へ）
- コード着手前に `.agent/` の該当仕様を読み、仕様変更はコードと**同一コミット**で md に反映する

## 絶対ルール

- **cmux（GPL-3.0）のソースコードを読まない・参照しない・移植しない**。設計思想のみ参考可（`.agent/concept.md`）
- ペイン内容・送信テキスト・`TAKO_TOKEN` をログに出さない

## コマンド

コード未着手のため未定。Phase 1 で `dev` / `build` / `lint` / `test` をここに表で書く。

## AI 向け詳細仕様（必要なときだけ Read する）

- コンセプト・競合・Non-goals: `.agent/concept.md`
- 機能要件（FR / NFR）: `.agent/requirements.md`
- 技術設計・リスク・3 層制御プレーン: `.agent/architecture.md`
- 規約（命名・エラー・ログ）: `.agent/conventions.md`

### 作業履歴メモ（毎ターン参照・更新）

- 現在の作業状況（毎ターン上書き）: @.agent/activeContext.md
- 完了タスクの時系列（毎ターン追記）: @.agent/progress.md
- フェーズ計画・次の一手: @.agent/roadmap.md

セッション開始時に必ず読み、応答終了前に `activeContext` は最新状態で**上書き**、
作業が一段落していれば `progress` の末尾に**1〜3 行で追記**する。
スキップ可能なターン（単発質問への回答、タイポ修正のみ）では更新しない。
詳細ルールはグローバル CLAUDE.md の「プロジェクト作業履歴メモ」節を参照。

## コミット規約

グローバル CLAUDE.md（`~/.claude/CLAUDE.md`）の「Git コミット」節に従う。
push 運用: リポジトリ公開（Phase 7）までは main 直 push 可。公開後はブランチ + PR 経由に切り替える。
