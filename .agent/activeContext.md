# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-22・#466 リモートチャット凍結の根治 → 実機確認待ち）

**#466 リモート: チャットビュー凍結を根治（fix/466-remote-chat-view-stale）**

- 根因はフロントの切替ではなくサーバー側: `claude agents --json` の一時失敗・列挙漏れで
  live 解決が消えると、v2 panes の session_id が sessions カタログの stale 旧世代
  （同一 pane に 20 世代超堆積 + BTreeMap 辞書順先勝ち）へ化け、チャットが凍結
  transcript を読み続けていた（term は画面キャプチャなので正常 = 報告症状と一致）
- 修正: ① agents.rs に sticky live 解決（失敗・欠落時は直近成功値、ペイン消滅で破棄）
  ② sessions.rs`resolve_session_for_pane` を last_seen_at 最新優先に
- 隔離実測: 実 claude 2 世代 + fail 注入で before（アルファ凍結 + トグル消滅）/
  after（fail 中も現行会話が更新継続、切替 5 ラウンド全緑）を確認

## 次の一手

- PR squash merge → `build-app.sh --install` → 実機 iPhone でチャット⇔ターミナル切替の
  継続更新を確認（ユーザー）
- #287 P1-2 UDS 化の実機確認も継続

## 現フェーズで Read すべき設計書

- リモート系: `crates/tako-control/src/remote.rs` ヘッダコメント（二層認証・API 一覧）
