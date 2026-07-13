# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#153 ターミナルリンク実機修正）

#153 完了（codex → Fable 引き継ぎ）。パスリンク cmd+クリック不動作の根本原因 5 件
（ペイン判定誤ヒット / ディレクトリ空ペイン / TUI cwd 不明 / cwd=None 検出スキップ /
走査無限ループ）+ cmd 押下中の下線・背景ハイライト・即時装飾更新を実装。

- 起動時 working directory をセッション初期 cwd に保持（OSC 7 で上書き）
- リンク装飾は `link_byte_range_in_chunk` でリンク文字列だけに限定
- 選択ドラッグは `cell_at_clamped` 分離でペイン外でも伸びる旧挙動を維持（引き継ぎ検証で追加）

## 検証済み

- 隔離セルフテスト（TAKO_DISCOVERY_DIR + TAKO_PERSIST=0 + TAKO_TMUX_SOCKET）完走
  = 69c 全 7 判定（3 形式検出 / OSC 7 cwd 一致 / ファイル→プレビュー / ディレクトリ→PTY 分割)パス
- workspace build / test / fmt / clippy（-D warnings）全緑

## 次の一手

- tako 再起動後、`.agent/manual-checks.md` #153 節（cmd ホバー装飾・実マウスクリック）を GUI で確認
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-3.2〜FR-3.5
- 手動確認: `.agent/manual-checks.md`「ターミナルリンクの cmd+クリック・cmd ホバー装飾」
