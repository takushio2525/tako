# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-06・docs サイト刷新 PR #73 レビュー待ち）

docs サイト（tako-docs.pages.dev で公開中）の内容刷新を PR #73 として提出。
ブランチ `docs/refresh-setup-releases`（3 コミット）。docs は公開配信中のため
main 直 push せず PR 経由。**マージは未実施（ユーザー判断待ち）**。

- セットアップページ全面刷新（tako setup 中心・初心者向け）
- CLI リファレンス詳細化 + shelve→background 等の実装乖離を修正
- MCP ツール一覧を実 51 ツールへ更新
- 新設: `/releases/`（リリースノート。CHANGELOG から起こす運用）、
  `/features/orchestration/`（オーケストレーション紹介・売り込みページ）
- ビルド緑 + dist 全内部リンク/アンカー機械チェック済み

## v0.2.8 の内容

| 種別 | Issue/PR | 概要 |
|---|---|---|
| Changed | #63 / PR#69 | リモート UI 再設計 v3（PC 非破壊 WS + リーダービュー） |
| Fixed | #64 / PR#70 | 半角文字消失の根治（whitespace_nowrap + グリフ隔離） |
| Fixed | #67 / PR#68 | migrate_legacy_default_profile の冪等性修正 |
| Fixed | #59 / PR#62 | 更新チェッカーの GitHub API レート制限誤報告修正 |

## リリース成果物

- GitHub Release: `v0.2.8`（tako-v0.2.8-macos-arm64.zip 添付）
- Homebrew tap: `takushio2525/homebrew-tako` cask 0.2.8 に更新済み
- 署名 DR: `identifier "dev.takushio.tako"`（固定、#54 で導入）
- CHANGELOG: [0.2.7] セクションを新設し #60 入れ忘れエントリを回収

## 未検証（スマホ実機テスト — #63 リーダービュー）

- [ ] タッチでの連続スクロール（上下）が滑らかに動作するか
- [ ] 下端追従: 新しい出力が来たとき自動スクロールするか
- [ ] 「↓最新へ」ボタン: 過去を見た後に押すと最下部に戻り追従再開するか
- [ ] ソフトキーボード入力: 文字入力 + Enter 送信が機能するか
- [ ] #64 PC 側確認: 日本語混在行で半角文字が消えないこと

## 残作業・既知の制約

- main.rs は 9,801 行。さらなる分割は別タスク
- MCP HTTP ポートのランダム問題は未解決（stdio ブリッジ経由なら影響なし）
- セルフテストの既知失敗は PDF（項目 70、CoreGraphics 環境依存）のみ
- CI（GitHub Actions）が 6/12 以降トリガーされていない — 要調査

## 未着手タスク（優先順はユーザーと相談）

- [ ] **Phase 5 続き**: FR-3.5 軽い編集
- [ ] **FR-2.19 localhost ポートパネル**
- [ ] **FR-2.18 未表示の子の自動サーフェス**
- [ ] **FR-2.14 MCP ゼロコンフィグオンボーディング**（配布前必須）

## 現フェーズで Read すべき設計書

- ターミナル描画修正時: `crates/tako-app/src/main.rs` の `chunk_line_chars` / `terminal_screen_lines` 周辺
- リモート API 修正時: `crates/tako-control/src/remote.rs` モジュールコメント
- リモート PWA 修正時: `web/tako-remote/src/pages/terminal.jsx` 冒頭コメント
- オーケストレーター修正時: `docs/orchestrator.md`
