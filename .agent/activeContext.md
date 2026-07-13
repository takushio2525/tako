# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#155 Web ビュー wry ネイティブ統合）

FR-3.8 Web ビューペインを CDP ミラー PoC から wry 0.55（WKWebView の
`build_as_child`）へ全面刷新。ブランチ `fix/155-webview-wry`（worktree
tako-wt-webview）。並行 worker の #152 / #153 / #156 / #158 は main へ
マージ済みで、origin/main を取り込み統合済み。

- ページ = `WebViewEntry`（ペインから独立）。ー = dock 退避（ページ生存）、× = 破棄
- ステータスバー 🌐 → dock パネル（flex 内 = webview と重ならない）
- dispatch `Web` + CLI `tako web` + MCP `tako_web`（9 action、計 58 ツール不変）
- タイトル/URL 追跡は eval 2 秒ポーリングが正（ipc は data: URL で不達を実機確認）
- 永続化: PaneLayout.webview + LayoutFile.webview_dock（後方互換）

## 検証済み

- workspace build / test（487）/ fmt / clippy（-D warnings）全緑（マージ前。
  マージ後の再検証は実施中）
- セルフテスト完走（`TAKO_APP_SELF_TEST_OK`。項目 71 = webview e2e 8 操作を
  実 WKWebView で通過。失敗時診断コード入り）

## 次の一手

- マージ後の全緑再確認 → PR #160 squash merge → `build-app.sh --install` →
  隔離インスタンス + screencapture でピクセル実証 → Issue #155 完了コメント
- ユーザー再起動後の GUI 確認（manual-checks.md）: 「Web ビューペイン」節（#155）、
  「#153 節」（cmd ホバー装飾・実マウスクリック）、「#152 節」（PDF ドラッグ選択・色分け）

## 現フェーズで Read すべき設計書

- 実装詳細と z オーダー制約: `.agent/architecture.md`「Web ビューペイン」節
- 手動確認: `.agent/manual-checks.md`「Web ビューペイン」節
