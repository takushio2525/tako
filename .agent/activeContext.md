# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-06-16・tmux window タブツリー統合 完了）

バックエンドセッション内の tmux window をタブツリーに表示し、既存のプレビュー/ピン留め
機能を適用する実装が完了。子 worker が `tmux new-window` で作った window が tako のサイドバー
tmux ビューに見えるようになった。

- **tmux.rs**: `select_window()` + `capture_pane_text()` を追加（window 切替 + プレビュー用テキスト取得）
- **protocol.rs**: `Request::TmuxSelectWindow { pane, window }` を追加
- **dispatch.rs**: ハンドラ実装（pane → backend session 解決 → tmux select-window 実行）。
  `ControlHost::backend_windows()` を追加し list 応答に `backend_windows` を含める
- **main.rs**: `PreviewTarget::TmuxWindow(PaneId, u32)` 追加。`backend_windows` / `window_captures`
  フィールドで 2 秒ポーリングから window 追跡 + 非アクティブ window のテキストキャプチャ。
  render_tmux_view でペイン行の下に非アクティブ window の子行を表示（ホバープレビュー +
  クリックで切替 + 📌 ピン留め）
- **CLI**: `tako tmux select-window <window> [--pane <id>]`
- **MCP**: `tako_tmux_select_window` ツール（計 34 ツール）
- **検証**: build / clippy(-D warnings) / fmt / test 全緑。セルフテスト期待値 34 に更新
- 最終更新: 2026-06-16

## 残作業・既知の制約

- window キャプチャはプレーンテキスト（ANSI 色なし）。端末スタイル付きプレビューは将来課題
- `sync_backend_windows` は tmux ポーリング内で capture-pane を呼ぶ（非アクティブ window 数 × 1 コマンド）。
  大量 window でのパフォーマンスは実測で判断
- PDF プレビューのセルフテストが Core Graphics 環境依存で失敗（既知・本変更と無関係）
- ピンの永続化（再起動またぎ）は未実装＝意図的スコープ

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）
- [ ] **FR-2.17 ネスト tmux の検出・診断・ワンタップ適用**（Phase 7）

## 直近の観点・指摘（実装時に踏みやすい点）

- **CI（GitHub Actions）はリポ設定で意図的に無効化中**（2026-06-12〜）。コミット前は必ず
  `cargo fmt --all --check`（exit code）を確認する
- **ライブプレビューは追加実装不要**: `on_term_event` が全ペインの出力で `cx.notify()` を
  呼ぶので、`terminal_screen_lines` ベースのプレビューは再描画で勝手にライブ化する
- **TmuxWindow プレビューはキャプチャテキストベース**: `terminal_screen_lines` は使えない
  （バックグラウンド window は in-memory 端末にない）。`capture_pane_text` で取得した
  プレーンテキストを StyledText で描画

## 現フェーズで Read すべき設計書

- タブツリー/プレビュー/ピン再修正時: `requirements.md` FR-2.15 / FR-2.16（特に 13〜16）
- FR-3.5 軽い編集着手時: `architecture.md`「コンセプト②の実現」

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 主な変更: `crates/tako-core/src/tmux.rs`（select_window / capture_pane_text）/
  `crates/tako-control/src/{protocol,dispatch,mcp}.rs`（TmuxSelectWindow）/
  `crates/tako-app/src/main.rs`（PreviewTarget::TmuxWindow / backend_windows / window_captures /
  render_tmux_view の window 子行）/ `crates/tako-cli/src/main.rs`（tako tmux select-window）
