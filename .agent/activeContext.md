# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: Phase 0（技術検証スパイク）完了。スタック成立を確認済み。次は Phase 1（macOS MVP）
- ステータス: Phase 1 未着手
- 最終更新: 2026-06-11

## 直近の観点・指摘

- **スタック確定**: Rust + GPUI（zed リポ git rev 固定）+ alacritty_terminal 0.26（PTY も同クレートの tty モジュール）+ GPUI executor（tokio 不要）
- git 版 gpui のハマりどころ（font-kit feature 必須等）は `poc/README.md` と `architecture.md` に記録済み。Phase 1 実装前に必読
- Windows は調査ベースで成立見込み高と判断（実ビルド未実施）。Phase 1 の CI 整備が実質最初の Windows 検証になる
- ゼロコンフィグ最優先 / cmux のコードは読まない / **zed リポの gpui 以外（GPL 系）のコードも読まない**

## 現フェーズで Read すべき設計書

- Phase 1 着手時: `.agent/roadmap.md`（Phase 1 チェックリスト）、`.agent/architecture.md`（レイヤ構成・Phase 0 検証結果）、`poc/README.md`（ハマりどころ）と `poc/03-term-poc/src/main.rs`（動く参考実装）を Read してから着手

## 未解決・次の一手

- [ ] Phase 1: Cargo ワークスペース構成（tako-core / tako-control / tako-app / tako-cli）確定
- [ ] Phase 1: タブ・ペイン分割の PaneTree ドメインモデル（GPUI 非依存）
- [ ] Phase 1 早期: GitHub Actions に macOS / windows ビルドを組み込む（Phase 0 残タスクの Windows スモーク）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- PoC: `poc/`（01: crates.io 版ウィンドウ、02: git 版ウィンドウ、03: 最小ターミナル）
