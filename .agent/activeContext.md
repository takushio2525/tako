# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-27 夜・Windows 実機開発 — テスター配布直前）

**PR #588**（`windows/467-ipc-orchestration-local`、34 コミット）が open。Windows 実機での
1 日集中開発の成果一式。マージ → 初回 Windows リリース（インストーラー配布）目前。

入っているもの（詳細は各 Issue の実測証拠コメント）:
- 修正: Alt+meta エンコード #575 / タブバーボタンクリック #576 / IME 半行ずれ #582 /
  ショートカット 45 本 #585 / 起動時コンソール窓 #586 / 更新通知の 404 死 #528 /
  worker 状態検知の全滅 #592 / **setup のコマンド検出全滅 #525**（`$SHELL` 直呼びで
  claude / git が導入済みでも exit 1 だった。抽象境界 B16 `platform::exe` で根治。
  MCP 自動登録も同根で成立 → `tako_setup_mcp` は supported へ）
- 機能: ウィンドウコントロール + Snap Layouts #584 / 永続化 M1+M2（**psmux 採用**、
  自作 winmux は中止）#518 #519 / Inno インストーラー + ローカルリリース #587 /
  アプリアイコン埋め込み #587 / doc の Windows 導線 + 対応状況ページ #528 #591 /
  対応マトリクス棚卸し（supported 1→89）#591 / CRT 静的リンク
- リリースは **GitHub Actions 不使用**（ユーザー決定）。`installer/windows/release-windows.ps1`
  でこの Windows 機からローカル実行

## 次の一手

1. PR #588 マージ（マージで doc サイトが Cloudflare Pages へ自動デプロイ）
2. `release-windows.ps1 -Upload` で v0.5.12 Release に Windows アセット 2 点添付
3. ユーザーがインストーラーで乗り換え（インストーラーテスト兼用）→ 実機確認
   （IME 位置 / Alt+V / ウィンドウコントロール / Ctrl+Shift+T / コンソール非表示 / アイコン / psmux 復元）
4. **#623 の実機確認（未決）**: 日本語入力の打鍵消失は**まだ直った確証が無い**。
   潜在欠陥（描画のたびに IME を強制確定しうる経路）は塞いだが実測では一度も発火せず、
   合成入力でも再現できなかった。症状が出たら
   `TAKO_IME_DIAG=1 TAKO_PERF_LOG=<path>` で採取し #623 のコメント §5 で切り分ける

## mac 側 master への引き継ぎ（マージ後に要対応）

- **macOS ビルド + 目視確認**: Windows 側は cfg 構造保証のみで macOS 未コンパイル。
  特に #584（TRAFFIC_LIGHTS_SPACER の cfg 化）と #582（変換中も
  `invalidate_character_coordinates` が走る = mac でも挙動変化・改善方向）の目視を
- **tmux 経路の実機再確認**: M1 の配線変更（#519）は argv スナップショットで不変保証済みだが、
  再 attach と orphan 掃除の実測は mac でやること
- doc の getting-started は Tabs（mac/win 切替）化 + `windows-support.md` 新設（生成は
  `scripts/gen-windows-support-docs.mjs`、support.rs 変更時に再生成）
- **#525 で mac にも出る挙動差 3 点**（いずれも改善方向・要目視）: ①CLI 入口で表示言語を
  解決するようにしたので、日本語設定なら全サブコマンドの縮退理由が日本語になる
  ②`setup` の `completed_at` がローカル時刻 → UTC（`…Z`）表記へ ③MCP 自動登録の失敗が
  setup 全体を中断しなくなった（手動手順を出して続行）
- 夜間リリース（launchd）と Windows ローカルリリースの棲み分け: アセット名で OS 分離
  （`tako-setup-{tag}-x64.exe` / `tako-{tag}-windows-x64.zip`）。チャンネル軸とは直交

## 環境メモ（この Windows 機）

- 稼働中 GUI は**旧バイナリ**（今日の修正が未反映）。乗り換えはインストーラー経由で行う
- worker 検証の罠: ペイン注入の `TAKO_SOCKET`/`TAKO_TOKEN`/`TAKO_PANE_ID` を剥がさないと
  本番インスタンスへ誤接続する（今日 3 回発生）。隔離は `TAKO_ISOLATED=1` + env 剥がし + 接続先 assert
- psmux 検証資材: `~/dev/psmux-eval/`（REPORT.md + 生ログ + 検証済みバイナリ）
- **隔離の穴 2 つ（#525 で実測）**: ①`tako lang` 等の CLI は IPC で稼働中の本番へ届くので、
  `HOME`/`APPDATA` を隔離しても本番 `settings.json` が変わる ②`claude.exe` は
  `HOME`/`USERPROFILE` の上書きを無視するので `claude mcp add` 経路は隔離できない
  （`tako setup` 自身の設定生成は APPDATA 隔離が効く）

## 現フェーズで Read すべき設計書

- 永続化を触る: `.agent/plans/2026-07-windows-persistence-backend.md`（§11 psmux 採用）+
  `crates/tako-core/src/backend/psmux.rs` 冒頭 doc
- 配布・リリースを触る: `installer/windows/release-windows.ps1` ヘッダ + AGENTS.md リリース節
- 新機能を足す: `platform/support.rs` の MATRIX に分類必須（パリティテスト T1/T3 が落ちる）+
  `scripts/gen-windows-support-docs.mjs` で doc 再生成
