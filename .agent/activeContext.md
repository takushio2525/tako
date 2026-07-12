# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・#146 + #147 cmd+クリックリンク）

#146（URL）・#147（パス）ともに完了・merge 済み・build-app.sh --install 済み。
tako 再起動で新バイナリが反映される。

- cmd+ホバーで URL / ファイルパスに下線表示、cmd+クリックで開く
- URL → デフォルトブラウザ、ディレクトリ → 右分割 cd、ファイル → 右分割プレビュー
- パス解決: cwd 相対 + ~ 展開 + 絶対パス（実在チェック付き）

## 直近の観点

- links.rs は GPUI 非依存（tako-core）で全テスト済み（URL 12 本 + パス 10 本）
- open_link() は将来の webview 差し替えポイント

## 次の一手

- tako 再起動で実機確認（cmd+クリックの動作）
- Phase 5 の次候補は FR-3.8 Web ビューまたは FR-2.19 localhost ポートパネル

## 現フェーズで Read すべき設計書

- 要件: `.agent/requirements.md` FR-3.1 / FR-3.8 / FR-2.19
