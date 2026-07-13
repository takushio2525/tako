# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#155 Web ビュー wry ネイティブ統合 完了）

#155 完了。PR #160（本体）+ #163（CLI 基準ペイン任意化）squash merge 済み、
`build-app.sh --install` 済み（0.4.0。反映は tako 再起動後）。
並行 worker の #152 / #153 / #156 / #158 とのマージ統合も済み。

- Web ビュー = wry `build_as_child`（WKWebView）。直接操作は OS 配送
- ページ = `WebViewEntry`（ペイン独立）。ー = dock 退避（生存）、× = 破棄
- ステータスバー 🌐 → dock パネル。layout.json 永続化（後方互換）
- dispatch `Web` + CLI `tako web` + MCP `tako_web`（9 action、58 ツール不変）
- タイトル/URL 追跡 = eval 2 秒ポーリング（ipc は data: URL 不達を実機確認）

## 検証済み

- workspace build / test（493）/ fmt / clippy（-D warnings）全緑
- セルフテスト完走（項目 71 = webview e2e 8 操作を実 WKWebView で通過）
- 実機 e2e: セカンダリインスタンス + CLI で open → read（title=Example Domain）→
  list → close 成功、screencapture でネイティブ描画・🌐 バッジをピクセル確認

## 次の一手

- tako 再起動後の GUI 確認（manual-checks.md）: 「Web ビューペイン」節（#155）、
  「#153 節」（cmd ホバー装飾）、「#152 節」（PDF ドラッグ選択・色分け）
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- 実装詳細と z オーダー制約: `.agent/architecture.md`「Web ビューペイン」節
- 手動確認: `.agent/manual-checks.md`「Web ビューペイン」節
