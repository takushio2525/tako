# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-08-05・Windows 実機 — #525 シェル統合）

**#525 の本体（PowerShell の OSC 7 / 133 シェル統合）を実装**。ペインの cwd 追従と
コマンド状態（待機中 / 実行中 / 失敗 + 終了コード）が Windows でも出るようになった。

- 届け方が macOS と違う。PowerShell に `ZDOTDIR` 相当が無いので、`tako setup` が
  `$PROFILE`（pwsh 7 と 5.1 の両方）へ**マーカーで囲んだ ASCII のブロック**を書く。
  冪等・解除でバイト一致復元・既存の符号（CP932 等）を壊さない
- `133;C`（実行開始）のフックは **`PSConsoleHostReadLine` のラップ**。
  `PreCommandLookupAction` は PSReadLine が prompt 直後に
  `PSConsoleHostReadLine` / `Set-StrictMode` を引くので誤爆する（実測）
- **psmux（器）の中では効かない** — psmux は `allow-passthrough on` を受理するのに
  OSC を素通ししない（実測。平文だけ届く）。`BackendCapabilities::osc_passthrough` を
  新設して器に尋ねる形にし、setup が理由つきで表示する。追跡は **#766**

## 次の一手

1. **#766**（psmux の器で OSC が届かない）。候補は ①psmux に passthrough を実装
   ②器へ `#{pane_current_path}` をポーリング ③器を替える。②はコスト実測が要る
2. **稼働中 GUI は旧バイナリ**なので反映は次のインストーラー更新から
3. **main とのアカウントモデル統合（#709 の宿題）**: main の CLI
   `tako orchestrator accounts`（#548/#556）と本ブランチの `tako account` を
   同じ dispatch へ向ける（ロジックは二重化しない）
4. **#623 の実機確認（未決）**: 日本語入力の打鍵消失は直った確証が無い。症状が出たら
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
