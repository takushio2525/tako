# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-06・レビュー起点の修正: #82/#83 完了、残 Issue #84〜#86）

コードベース全体レビュー（`reviews/2026-07-06_構造・MCPリファクタ提案.md`、提案 17 件）を実施し、
高優先度 #82〜#86 を起票。うち #82（orchestrator run の output 常時空バグ）+
#83（完了待ちポーリングの MCP/CLI 二重実装）を PR #87 で修正・squash merge 済み
（`tako-control::orchestrator::wait` 新設、テスト 9 本追加）。`build-app.sh --install` で実機反映済み。

- 残 Issue: #84（MCP HTTP 直列処理）/ #85（タブ退避の CLI/MCP 対応）/ #86（ControlHost 分割）
- 公開監査（別セッション）: #75〜#81 起票済み、#76/#77 は削除対応済み
- docs 刷新（PR #73/#74）はマージ済み・自動デプロイ済み
- リモート接続バグ調査（#89 起票済み・修正未着手）: `tako remote start` が cloudflared 未導入時に
  無警告で LAN 限定 URL（プライベート IP 直）を出す。spawn_daemon が成功パスで stderr を読まず
  フォールバック警告が消える（remote.rs:167/331）。**修正はリレー worker 並行作業 + #78 認証方針の決定後**
- #88 対応済み（PR #92）: `tako setup` に依存ツールチェック段階（claude 必須 / tmux・cloudflared・git 任意、
  brew でその場インストール可）を追加。#89 のセットアップ側の入口はこれでカバー

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
