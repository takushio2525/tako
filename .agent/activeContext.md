# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-13・v0.4.0 正規リリース + 夜間リリースのローカル化完了）

**v0.4.0 リリース済み**（tag `v0.4.0` = `98b17ea`、バイナリ付き GitHub Release、
Pages デプロイ、homebrew-tako cask 0.4.0 更新済み）。CHANGELOG は v0.3.2 以降の
未記載 13 件を回収済み。夜間リリースは launchd ローカルジョブへ移行（#166 / PR #170。
`com.takushio.tako-nightly-release` 毎日 5:00、クラウドルーチンは廃止）。

本日 main へマージ済み: #155（Web ビュー wry 化。PR #160 + #163）/ #103（Cmd-Q
グローバルアクション化。#162。Issue クローズ済み）/ #152 / #153 / #156 / #158。
`build-app.sh --install` は #162 込みの最新 main で実施済み（0.4.0。反映は tako 再起動後）。

- Web ビュー = wry `build_as_child`（WKWebView）。直接操作は OS 配送。
  ページ = `WebViewEntry`（ペイン独立）。ー = dock 退避（生存）、× = 破棄。
  ステータスバー 🌐 → dock パネル。layout.json 永続化（後方互換）。
  dispatch `Web` + CLI `tako web` + MCP `tako_web`（9 action、58 ツール不変）。
  タイトル/URL 追跡 = eval 2 秒ポーリング（ipc は data: URL 不達を実機確認）
- #103 = Quit を `cx.on_action` グローバル登録へ + 終了処理を `cx.on_app_quit` へ
  （Dock/OS 終了でも layout 保存。quitting ガードで #30/#113 維持）。
  根因はフォーカスパス依存: blur（focus=None）で dispatch path が root node のみになり
  キーバインド・メニュー両経路とも不発（Dock 終了だけ AppKit 経路で生存）

## 検証済み

- workspace build / test / fmt / clippy（-D warnings）全緑（#103 rebase 後 494 tests）
- セルフテスト完走（#155 項目 71 = webview e2e 8 操作を実 WKWebView で通過。
  #103 最終項目 = blur + cmd-q e2e: 旧構造 FAILED を実測 → 新構造 OK）
- #155 実機 e2e: セカンダリインスタンス + CLI で open → read（title=Example Domain）→
  list → close 成功、screencapture でネイティブ描画・🌐 バッジをピクセル確認
- #103 実機: osascript の実 Cmd-Q キーイベントで隔離インスタンス終了、
  インストール済み .app（md5 一致 + codesign 検証済み）でセルフテスト完走

## 次の一手

- tako 再起動後の GUI 確認（manual-checks.md）: 「Web ビューペイン」節（#155）、
  「#153 節」（cmd ホバー装飾）、「#152 節」（PDF 選択・色分け）+ Cmd-Q 経過観察（#103）
- 明朝 5:00 の夜間ジョブ初回実行を監視: main が v0.4.0 タグより先行しているため
  v0.4.1（#170 + ドキュメントのみの内容）が自動リリースされる見込み = 全経路の初通し検証
- Phase 5 の次候補は FR-2.19 localhost ポートパネル・FR-3.10 画像プレビュー等

## 現フェーズで Read すべき設計書

- Web ビュー実装詳細と z オーダー制約: `.agent/architecture.md`「Web ビューペイン」節
- 手動確認: `.agent/manual-checks.md`「Web ビューペイン」節
