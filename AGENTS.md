# tako — エージェント向けガイド

AI 駆動・エージェント集約監視に特化した OSS GUI ターミナル。
iTerm2 + Zed の思想で Zed 級に高速・軽量。macOS 先行、Windows 対応必須。GPL-3.0-or-later。

> このリポジトリの AI 向け規約はここに集約してある。詳細仕様は `.agent/` を参照。
> 人間向けの説明は `README.md` にある。

## 概要

- 目的: AI エージェント（Claude Code 等）+ 子エージェント + dev サーバーを「1 グループ = 1 タブ」で集約監視する
- 対象: AI エージェントで開発する開発者。**ただしゼロコンフィグで一般ユーザーが使えることが最優先の設計原則**
- 状況: **Phase 1〜4 + 5.5 完了（macOS MVP / CLI / MCP / パッシブ検知 / tmux バックエンド永続化）。
  Phase 5（ワークスペース機能）はファイルツリーまで完了で中断中 → 次は FR-3.2 から再開**

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
├── README.md / LICENSE     ← 人間向け・GPL-3.0-or-later
├── crates/
│   ├── tako-core/          ← ドメインモデル（PaneTree / Workspace / TerminalSession、GPUI 非依存）
│   ├── tako-control/       ← 制御プレーン（IPC + dispatch + MCP 実装済み。検知は Phase 4）
│   ├── tako-app/           ← GPUI バイナリ（GPUI 依存はここだけ。IPC / MCP サーバー内蔵）
│   └── tako-cli/           ← Layer 1 CLI（`tako` コマンド）+ MCP stdio ブリッジ（`tako mcp serve`）
├── poc/                    ← Phase 0 の使い捨て検証コード（品質基準の対象外）
└── .github/workflows/      ← CI（macOS / Windows ビルド + テスト）
```

- `.agent/` に置くもの: AI 向け仕様・作業文脈。置かないもの: 人間向け紹介文（README へ）
- コード着手前に `.agent/` の該当仕様を読み、仕様変更はコードと**同一コミット**で md に反映する

## 絶対ルール

- **cmux（GPL-3.0）のソースコードを読まない・参照しない・移植しない**。設計思想のみ参考可（`.agent/concept.md`）
- ペイン内容・送信テキスト・`TAKO_TOKEN` をログに出さない

## 機能実装時の必須ルール（開発不変条件）

- **設計原則 5「AI フルコントロール」は不変条件**: すべての機能は追加した時点で MCP / CLI から
  操作可能でなければならない（UI でできることはすべて AI からもできる）。新機能の Definition of
  Done に「対応する MCP / CLI 操作の提供」を含め、例外は理由を `.agent/requirements.md` に明記する
- 新機能の操作ロジックは tako-core の操作 API として実装し、`tako-control::dispatch`
  （protocol + ControlHost）へ 1:1 で載せる（UI 層に閉じたロジックを作らない）。
  Phase 2 以降、CLI はこの経路で操作できる。MCP 公開（Phase 3）も同じ dispatch を呼ぶ

## コマンド

| 操作 | コマンド |
|---|---|
| dev（最小ターミナル起動） | `cargo run -p tako-app` |
| セルフテスト起動（入力経路 + CLI / MCP e2e の機械検証） | `TAKO_SELF_TEST=1 cargo run -p tako-app` |
| Claude Code 実機検証（MCP 設定ゼロ接続） | `scripts/verify-claude-mcp.sh`（要 claude CLI + 認証） |
| MCP セットアップ | `tako setup-mcp`（`~/.claude/settings.json` に自動追加。`--project` でプロジェクト単位） |
| `tako` CLI ビルド | `cargo build -p tako-cli`（バイナリは `target/debug/tako`） |
| .app バンドル生成（macOS） | `scripts/build-app.sh [--verify] [--install]`（`dist/tako.app`。tako CLI 同梱） |
| リリース | `scripts/release.sh`（Cargo.toml バージョン自動読み取り + CHANGELOG.md 連携。`--publish` でタグ + GitHub Release 作成、`--draft` でドラフト） |
| マスターオーケストレーター起動 | `tako master [suffix]`（claude を master system prompt 付きで起動） |
| オーケストレーター worker spawn | `tako orchestrator spawn --project <key> --prompt "..."` |
| オーケストレーター worker 監視 | `tako orchestrator watch --pane <N> --session-id <S>` |
| オーケストレーター プロジェクト管理 | `tako orchestrator projects list/add/remove` |
| build | `cargo build --workspace` |
| lint | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` |
| test | `cargo test --workspace` |

CI（`.github/workflows/ci.yml`）は macOS / Windows の両ランナーで build + test を回す。

## AI 向け詳細仕様（必要なときだけ Read する）

- コンセプト・競合・Non-goals: `.agent/concept.md`
- 機能要件（FR / NFR）: `.agent/requirements.md`
- 技術設計・リスク・3 層制御プレーン: `.agent/architecture.md`
- 規約（命名・エラー・ログ）: `.agent/conventions.md`
- 手動確認チェックリスト（IME・.app 等、機械検証できない項目）: `.agent/manual-checks.md`
- オーケストレーター使い方ガイド: `.agent/orchestrator.md`

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

## リリース運用

- 機能追加・バグ修正が一段落したら `CHANGELOG.md` に追記（日英併記、Keep a Changelog 形式）
- `Cargo.toml`（ワークスペースルート）の `[workspace.package] version` を bump
- `scripts/release.sh --publish` でタグ + GitHub Release 作成（CHANGELOG から自動抽出）
- リリースノートは日英併記
