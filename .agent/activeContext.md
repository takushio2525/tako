# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#103 完了 / #155 マージ済み）

#103（Cmd-Q で終了しないことがある）の根本原因を GPUI ソースレベルで特定し修正済み。
並行 worker の #155（Web ビュー wry 化）/ #153 / #156 / #158 は main へマージ済み。

- #103 根因: Quit がルート div の on_action のみ = フォーカスパス依存。`window.focus == None`
  （blur。a11y ツール等で発生）だと dispatch path が root dispatch node のみになり、
  キーバインド・メニュー両経路とも不発（Dock 終了だけ AppKit 経路で生存 = 観測と整合）
- #103 修正: Quit を `cx.on_action` グローバル登録へ一本化 + layout 保存 / discovery cleanup を
  `cx.on_app_quit` へ移設（Dock 終了・OS 終了でも保存が走る）。全ペイン終了経路
  （quitting=true）は #30/#113 の削除 / 保持分岐を維持（ガード付き）

## 検証済み（#103）

- 再現: 旧構造 + blur + cmd-q ディスパッチ → 不発（セルフテスト FAILED / exit 1）
- 修正後: 同一テストが quit 経路で自然終了（OK マーカー / exit 0）
- 実 OS キーイベント（osascript Cmd-Q）で隔離インスタンス終了、exit による
  全ペイン終了経路も回帰なし
- workspace build / test（486 passed）/ fmt / clippy 全緑（#155 マージ後の再検証も緑）

## 次の一手

- #103 PR squash merge → `build-app.sh --install`（#155 Web ビュー / #156 セッション復元 /
  #158 パスリンクも同時に実機反映）
- tako 再起動後、通常利用で Cmd-Q の経過観察（間欠性だった旧症状の再発監視）+
  GUI 確認（manual-checks.md）: 「Web ビューペイン」節（#155）、「#153 節」
  （cmd ホバー装飾・実マウスクリック）
- Phase 5 の次候補は FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- Web ビュー実装詳細と z オーダー制約: `.agent/architecture.md`「Web ビューペイン」節
- 手動確認: `.agent/manual-checks.md`「Web ビューペイン」節
