# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-07・#95 Enter 空振り修正）

worker ペインの claude TUI で Enter が空振りする問題（#95）を修正（fix/95-enter-delivery）。
調査で確定した事実: claude TUI は LF を「改行挿入」と解釈し送信にならない（B-2 の根因）/
`text:""+newline:true` は空 paste + 10 秒無駄待ち + 検証常時成功扱いで再送ゼロ（B-1 の根因）/
busy 中の claude は Enter を取りこぼすことがあり、CR 1 発の代行では残留が復旧しない
（実機 transcript の pane192 事例）。修正 = ①人間の Enter に送達検証 + 自動再送
（handle_key で baseline 記録 → PromptFlow enter_only）②Enter 単独送達フロー新設
（即 CR + 入力欄空検証 + 再送。dispatch / deliver_via_tmux 両経路）③直接 write の LF→CR 正規化。

- setup 関連の変更を入れたら `resources/setup/changes.yaml` に revision を 1 増やして追記する
  （運用ルール。記入方法はファイル冒頭コメント。連番・非空はテストで機械検証）
- 残 Issue: #84（MCP HTTP 直列処理）/ #85（タブ退避の CLI/MCP 対応）/ #86（ControlHost 分割）
- #91 対応済み（PR #99）: リモート接続の入口を tako-remote.pages.dev 固定 URL に一本化。
  Pages デプロイ手順 = `scripts/deploy-pages.sh`（release.sh --publish が自動実行。単体実行可）。
  次リリース（tag / Release / cask）は master 側で別途実施予定
- リモート接続バグ #89（cloudflared 不在時の無音 LAN フォールバック）: 警告の可視化は #91/PR #99
  で対応済み（tunnel_error を CLI 表示）。残り = lan_ip の en0 固定解消・cask への cloudflared
  依存追加など
- 公開監査は全条件クリア（判定 OK）。#79 は ignore 登録済み（PR #97）、#80 対応済み（PR #96）

## v0.3.0 の内容（2026-07-06 リリース）

| 種別 | Issue/PR | 概要 |
|---|---|---|
| Added | #88 / PR#92 | tako setup に依存ツールチェック段階（claude 必須 / tmux・cloudflared・git 任意） |
| Added | #94 / PR#98 | tako setup のアップデート追従（setup changelog + applied_revision） |
| Security | #80 / PR#96 | FileOp::Trash の argv 渡し化（AppleScript インジェクション排除） |
| Security | #78 / PR#93 | リレー登録の端末シークレット保護（first-write-wins） |
| Changed | #91 / PR#99 | リモート接続の入口を tako-remote.pages.dev 固定 URL に一本化 |
| Changed | #75 / PR#90 | ライセンス宣言を GPL-3.0-or-later に全マニフェスト統一 |
| Changed | #83 / PR#87 | 完了待ちポーリングを tako-control::orchestrator::wait に一本化 |
| Fixed | #82 / PR#87 | orchestrator_run の output 常時空を修正 |

## リリース成果物（v0.3.0）

- GitHub Release: `v0.3.0`（tako-v0.3.0-macos-arm64.zip 添付、annotated tag + --generate-notes）
- Homebrew tap: `takushio2525/homebrew-tako` cask 0.3.0 に更新済み
- Cloudflare Pages: tako-remote.pages.dev へ PWA デプロイ済み（release.sh --publish が自動実行）
- /Applications へ v0.3.0 配置済み（反映にはユーザーの tako 再起動が必要）
- 署名 DR: `identifier "dev.takushio.tako"`（固定、#54 で導入）

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
