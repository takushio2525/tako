# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-08-05・Windows 実機 — #728 完了）

**#728**（`tako sessions` の Windows 対応）。マトリクスの `Pending` は棚卸しで嘘と判明
（psmux 導入済みなら既に動いていた）。実装本体は**器なし構成**（psmux 未導入）で
セッションカタログが永久に空だった欠落の修正。

- 突き合わせキーを「器があればセッション名 / 無ければペイン ID」の二段構えへ（#592 と同型）。
  器なしの対応付けは `TerminalSession::child_pid` からの pid 祖先辿り。
  キーの解釈は `PendingSpawn::matches` に集約（`tmux_session` の直接比較は禁止）
- 器のペイン列挙は `SessionBackend::pane_pids_all()` 経由（`agents.rs` の `tmux_bin()`
  直叩きだと `TAKO_PSMUX_BIN` のみの構成で検出が全滅する）
- 復元は tako 自身のペイン生成 + `claude --resume` なので**器への送出は不要**

## 次の一手

1. **稼働中 GUI は旧バイナリ**なので #709 の `tako account` / #728 の器なし検出はまだ効かない。
   次のインストーラー更新で反映
2. **main とのアカウントモデル統合（マージ時の宿題）**: main には CLI
   `tako orchestrator accounts`（#548/#556）が別途あるので、両方の CLI パスを同じ dispatch へ
   向ける（ロジックは二重化しない）。master への account 適用も本ブランチ #653 と main #555 が
   独立実装で衝突する既知の債務
3. **#623 の実機確認（未決）**: 日本語入力の打鍵消失は直った確証が無い。症状が出たら
   `TAKO_IME_DIAG=1 TAKO_PERF_LOG=<path>` で採取し #623 のコメント §5 で切り分ける
4. **#640（OPEN）**: 器あり（psmux）の起動コマンド送達は今も取りこぼす。#728 の実測で
   `sessions resume` の行が欠落・重複するのを再現（baseline でも同一）

## Windows 実機で隔離検証するときの env 剥がし（#728 実測）

psmux ペインの中から検証を回すと、以下を剥がさない限り**製品の不具合に見える偽陰性**が出る。

- `PSMUX_SESSION` / `PSMUX_TARGET_SESSION` / `TMUX` / `TMUX_PANE` — 器が
  「nested with care」で作れず、PTY 死亡判定でアプリが即終了する（`cargo test --test
  psmux_backend` の 5 件もこれで落ちる）
- `CLAUDE_CODE_CHILD_SESSION` 等 — 子 claude の transcript 保存が切れ `resumable` が常に false
- `TAKO_ORCHESTRATOR_DIR` を空で隔離するときは **accounts.yaml をコピーする**
  （`claude agents --json` の走査先が既定 config dir だけになる）

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
