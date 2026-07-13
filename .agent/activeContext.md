# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#155 Web ビュー wry ネイティブ統合）

FR-3.8 Web ビューペインを CDP ミラー PoC から wry 0.55（WKWebView の
`build_as_child`）へ全面刷新。ブランチ `fix/155-webview-wry`（worktree
tako-wt-webview）で実装完了、PR 作成 → merge → install の最終段階。

- ページ = `WebViewEntry`（ペインから独立）。ー = dock 退避（ページ生存）、× = 破棄
- ステータスバー 🌐 → dock パネル（flex 内 = webview と重ならない）
- dispatch `Web` + CLI `tako web` + MCP `tako_web`（9 action、計 58 ツール不変）
- タイトル/URL 追跡は eval 2 秒ポーリングが正（ipc は data: URL で不達を実機確認）
- 永続化: PaneLayout.webview + LayoutFile.webview_dock（後方互換）

## 検証済み

- workspace build / test（487）/ fmt / clippy（-D warnings）全緑
- セルフテスト完走（`TAKO_APP_SELF_TEST_OK`。項目 71 = webview e2e 8 操作を
  実 WKWebView で通過。失敗時診断コード入り）

## 次の一手

- PR squash merge → `build-app.sh --install` → 隔離インスタンス + screencapture で
  ピクセル実証 → Issue #155 完了コメント
- ユーザー再起動後: `.agent/manual-checks.md`「Web ビューペイン」節の実機確認

## 現フェーズで Read すべき設計書

- 実装詳細と z オーダー制約: `.agent/architecture.md`「Web ビューペイン」節
- 手動確認: `.agent/manual-checks.md`「Web ビューペイン」節
