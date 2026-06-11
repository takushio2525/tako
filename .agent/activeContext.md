# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: Phase 1（macOS MVP）実装完了。次は Phase 2（Layer 1 — CLI と環境変数注入）
- ステータス: Phase 2 未着手。Phase 1 の常用フィードバック収集中
- 最終更新: 2026-06-11

## 直近の観点・指摘

- **設計原則 5「AI フルコントロール」は開発不変条件に昇格**（AGENTS.md 必須ルール参照）。
  新機能 = tako-core の操作 API + 将来の MCP/CLI 公開前提の構造、を徹底する
- **描画色・フォントは tako-core の Theme を一枚通す**（FR-4）。UI への直書き禁止
- tako-app のキーバインドは iTerm2 踏襲（cmd+D 分割 / cmd+W close / cmd+alt+矢印フォーカス /
  ctrl+cmd+矢印リサイズ / cmd+T・cmd+数字タブ / cmd+C/V）。cmd+W は「ペイン → タブ → アプリ」の順
- `TAKO_SELF_TEST=1 cargo run -p tako-app` で 13 項目を機械検証できる（入力 / 分割 / フォーカス /
  リサイズ / クローズ / タブ / ANSI 色 / スクロールバック / ペースト / 選択コピー / PTY リサイズ）
- スクリーン抽出は `tako-core::screen::snapshot`（Term 直接受けの純関数、PTY なしでテスト可能）
- gpui ハマりどころ（font-kit feature 必須等）は `poc/README.md` / `architecture.md` 参照（変わらず必読）

## 現フェーズで Read すべき設計書

- Phase 2 着手時: `.agent/architecture.md`「制御プレーン」節と `requirements.md` FR-2.1〜2.2 を
  Read してから実装。IPC の操作セマンティクスは `crates/tako-core/src/pane_tree.rs` の API と 1:1

## 未解決・次の一手

- [ ] Phase 2: `TAKO_PANE_ID` 等の環境変数注入（TerminalSession::spawn 拡張）→ IPC サーバー → `tako` CLI
- [ ] Phase 1 残骨格: ドラッグでのペイン境界リサイズ・IME 変換中表示は未実装（常用しながら判断）
- [ ] Phase 5 送り: Web ビューペイン（FR-3.8）・注釈レイヤ（FR-2.6）・diff ビューア（FR-3.9）・
      提示系（FR-2.7）・フィードバック（FR-2.8）・cmd+K（FR-2.9）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
- PoC: `poc/`（Phase 0 検証コード。01: crates.io 版、02: git 版、03: 最小ターミナル）
