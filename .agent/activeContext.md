# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-08-01・Windows 実機 — #709 完了）

**#709 マージ済み**（PR #712 → `47a524f`）。claude ログインアカウントの切替を
`tako account`（list / add / remove / show / login / use）へ 1 本化した。

- MCP は新ツールを増やさず `tako_orchestrator_accounts` に action を足して 1:1（129 ツールのまま）
- 前提として **main のアカウントモデル #512 / #543 を先に移植**した（`config_dir: Option<String>`
  + `inherit` + `AccountConfigDir` + `EnvPlan`）。claude は `CLAUDE_CONFIG_DIR` が**設定されて
  いるだけで**資格情報エントリを分けるので、「既定の資格情報を使う」は既定パスの明示ではなく
  **未設定**でしか表現できない。`inherit` は起動コマンドで明示 unset を出す
- ログイン状態は 3 値（`missing` / `logged_out` / `logged_in`）+ 壊れたエントリの `invalid`
- GUI は ⌘K パレットの「claude アカウントを切り替え」（default プロファイルの master へ割り当て）

## 次の一手

1. **稼働中 GUI は旧バイナリ**なので `tako account` はまだ使えない。次のインストーラー更新で反映
2. **main とのアカウントモデル統合（マージ時の宿題）**: main には CLI
   `tako orchestrator accounts`（#548/#556）が別途あるので、両方の CLI パスを同じ dispatch へ
   向ける（ロジックは二重化しない）。master への account 適用も本ブランチ #653 と main #555 が
   独立実装で衝突する既知の債務
3. **#623 の実機確認（未決）**: 日本語入力の打鍵消失は直った確証が無い。症状が出たら
   `TAKO_IME_DIAG=1 TAKO_PERF_LOG=<path>` で採取し #623 のコメント §5 で切り分ける
4. **main 改善の取り込み（PR #743 で md プレビュー 3 件を移植済み）**: 残りの仕分けは
   #588 のコメントが正。次に取るのは #546（アプリ内テキスト入力の貼り付け。Windows は
   `ctrl-v` を `is_paste_keystroke` へ足す小改変が要る）→ #608（support テストのフレーク）
   → #684（縦積み UI 表示中の PTY 行数超過。たまり場ドロワーで本ブランチでも踏む）。
   **`pending_clipboard` は #666 の土台から切り出した最小移植**なので、main 統合時に一本化する

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
