# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-08-05・Windows 実機 — #729 完了 / #730 切り出し）

**#729（Shift+Enter で改行できない）を根治**。原因は送出側ではなく**運び手**で、
psmux が CSI u（`ESC[13;2u`）を内側アプリへ通していなかった（実測で確定。ConPTY は無罪）。

- 「CSI u を運べるか」を `BackendCapabilities::extended_keys` にし、運べない器に
  **包まれたペインだけ**レガシー形式へ落とす（修飾付き Enter → `ESC CR`、Shift+Tab → `ESC[Z`）。
  **判定はペイン単位**（persist OFF の直接ペインは CSI u のまま = 巻き添えにしない）
- 落とす表現は `keys::legacy_modified` の 1 箇所。GUI 経路（`keystroke_to_bytes` の
  `CsiUMode::Legacy`）と AI 経路（`encode_key`）が同じ関数を通る
- psmux は**配送が遅れる**（`ESC[Z` が 600ms 窓の外で届いた）。到達の判定は長めに待つこと。
  短い窓での「届かない」は誤断定になる
- 対応マトリクス `tako_send_keys` は Windows で Degraded（修飾の区別は失われる）

## 次の一手

1. **#730（IME 確定直後の Enter 連打が改行になる）**: #729 とは**別根因**。tako の連続
   `write` が 1 回の read にまとまり、内側 TUI の貼り付け判定で CR が改行に化ける
   （`RECV<e3-81-82-e3-81-84-e3-81-86-0d-0d-0d>` を実測。直接ペイン・psmux の両方で再現）。
   Enter の送達は #95 / #623 / #640 が積み上げた領域なので、触るなら実測つきで慎重に
2. **実機確認は次のインストーラー更新後**（稼働中 GUI は旧バイナリなので #729 の修正は未反映）
3. **#623 の実機確認（未決）**: 日本語入力の打鍵消失は直った確証が無い。症状が出たら
   `TAKO_IME_DIAG=1 TAKO_PERF_LOG=<path>` で採取し #623 のコメント §5 で切り分ける
4. **main とのアカウントモデル統合（#709 マージ時の宿題）**: main の CLI
   `tako orchestrator accounts`（#548/#556）と両方の CLI パスを同じ dispatch へ向ける。
   master への account 適用も本ブランチ #653 と main #555 が独立実装で衝突する既知の債務

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
