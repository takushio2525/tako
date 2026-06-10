# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象

- 何を / どこを: 仕様策定フェーズ完了。次は Phase 0（GPUI Windows ビルド検証スパイク + 最小ターミナル描画 PoC）
- ステータス: Phase 0 未着手
- 最終更新: 2026-06-11

## 直近の観点・指摘

- ゼロコンフィグ（一般ユーザーが設定なしで使える）が最優先の設計原則
- cmux（GPL-3.0）のコードは絶対に読まない。設計思想のみ参考
- GPUI は pre-1.0。依存は ui/ レイヤに閉じ込め、core/control は GPUI 非依存に保つ
- 仕様の未決事項リストあり（progress.md の 2026-06-11 参照。MCP トランスポート確定は Phase 3 等、各フェーズで判断）

## 現フェーズで Read すべき設計書

- Phase 0 着手時: `.agent/roadmap.md`（Phase 0 のチェックリスト）と `.agent/architecture.md`（技術スタック・リスク）を Read してから着手

## 未解決・次の一手

- [ ] Phase 0: GPUI 単体アプリの macOS / Windows ビルド検証
- [ ] Phase 0: alacritty_terminal + PTY + GPUI の最小描画 PoC（macOS → Windows）

## 関連ファイル / リンク

- リポジトリ: https://github.com/takushio2525/tako（private）
- 仕様一式: `.agent/concept.md` / `requirements.md` / `architecture.md` / `roadmap.md`
