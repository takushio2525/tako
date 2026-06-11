# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: Phase 1（macOS MVP）。前半（ワークスペース構成 / PaneTree / 最小ターミナル / CI）完了。次は後半
- ステータス: Phase 1 後半 未着手
- 最終更新: 2026-06-11

## 直近の観点・指摘

- **CI は macOS / Windows 両ランナーで緑**（Phase 0 残タスクの Windows スモーク完了。
  Spectre-mitigated libs は CI 内で VS インストーラー追加。実機検証は Phase 6 のまま）
- 設計原則 5「**AI フルコントロール**（UI でできる操作はすべて MCP/CLI から）」を要件化済み。
  PaneTree の操作 API（split/close/focus/resize_by/set_share/equalize/layout）は FR-2.5 と 1:1 対応前提
- tako-app は `TAKO_SELF_TEST=1 cargo run -p tako-app` で入力経路を機械検証できる
- gpui ハマりどころ（font-kit feature 必須等）は `poc/README.md` / `architecture.md` 参照（変わらず必読）

## 現フェーズで Read すべき設計書

- Phase 1 後半着手時: `.agent/roadmap.md`（Phase 1 残項目）、`crates/tako-core/src/pane_tree.rs`
  （ドメイン API。UI はこれを呼ぶだけにする）、`crates/tako-app/src/main.rs`（現状の最小 UI）

## 未解決・次の一手

- [ ] Phase 1 後半: 複数ペイン描画（PaneTree::layout → GPUI 要素ツリー変換）とタブバー UI
- [ ] Phase 1 後半: スクロールバック・コピペ・色/カーソル描画・PTY リサイズ（TerminalSession 拡張）
- [ ] Phase 5 送り: Web ビューペイン方式検証（FR-3.8）・注釈レイヤ（FR-2.6）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- PoC: `poc/`（Phase 0 検証コード。01: crates.io 版、02: git 版、03: 最小ターミナル）
