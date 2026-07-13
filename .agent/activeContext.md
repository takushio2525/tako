# Active Context

> このファイルは AI が**毎ターン上書き更新**する現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。ここには履歴を残さない。
> セッション開始時に AGENTS.md の直後に必ず読む。

## 現在の対象（2026-07-14・#112 セッション会話ログの管理と復元）

**#112 を二本立てで実装完了**（worktree feat/112-session-log。PR 準備中）:

1. **A: セッションカタログ**（FR-5.12・`tako-control::sessions` 新設）
   - 会話本文は保存せず claude transcript への参照 + メタデータを
     `<data_dir>/sessions.yaml`（config_io 保護・`TAKO_SESSIONS_FILE` で隔離）へ索引化
   - spawn 時に tmux セッション名キーの pending 記録（プロンプト由来 Issue 番号含む）→
     GUI の 5 秒スキャン（`claude agents --json` × pid 祖先照合）で session_id 検出時に昇格
   - `tako sessions list/show/resume` + MCP `tako_sessions`。resume = 記録 cwd で分割 +
     `claude --resume` 注入（#30 の復元と同方式）。codex / agy は resume 非対応を明示エラー
2. **B: ペイン平文ログ**（FR-5.13・`tako-core::pane_log` 新設）
   - 確定行のみ保存: 直接ペイン = alacritty history 増分、バックエンド = tmux
     `#{history_size}` 増分 + `capture-pane -p`（ANSI 除去・再 attach 重複なし）
   - alt screen 中は history が増えない = TUI 描画スパム構造排除（マーカーのみ。実測 93B）
   - close/exit/quit で可視画面フラッシュ。5MB/ペイン `.1` ローテ + 200MB 全体上限 +
     tick 400 行上限。`tako logs list/show/status/set` + MCP `tako_logs`（計 63 ツール）

副産物の修正: spawn 応答 `tmux_session` が常に null だった問題
（`ControlHost::reserve_backend_session` で dispatch 時に採番確定）/
`TAKO_DATA_DIR` 上書き新設（TAKO_ISOLATED が一括設定。#177 の
「TAKO_ISOLATED + TAKO_PERSIST=1 で本番 layout 復元」の穴を閉塞）

## 検証済み（#112）

- workspace build / test（591+）/ fmt / clippy(-D warnings) 全緑 + セルフテスト完走（63 ツール）
- 隔離 e2e（TAKO_DATA_DIR + 隔離ソケット + 実 claude）: spawn → pending 記録 → 昇格 →
  ペイン/タブ/tmux 全滅 + アプリ再起動 → `tako sessions resume` → 会話文脈維持
  （合言葉を記憶から再出力）まで通し実証
- ペインログ: kill 後の読み出し（可視画面 + クローズマーカー）/ 洪水 30000 行 → 26KB +
  省略マーカー / claude TUI 数分稼働 → 93 バイト
- 検証時の注意: 隔離 tako から spawn する claude は `CLAUDECODE` 等の環境変数を
  継承すると `claude agents --json` に載らない（ネスト claude 扱い）。e2e ハーネスでは
  `env -u CLAUDECODE -u CLAUDE_CODE_*` が必須

## 次の一手

- origin/main（v0.5.0 + #157 + #191）を rebase 取り込み → コンフリクト解消 →
  全ゲート再実行 → push → PR（Closes #112）→ squash merge → `build-app.sh --install`
- マージ後: Issue #112 に実測証拠つき完了コメント

## 現フェーズで Read すべき設計書

- セッションカタログ / ペインログの仕様: `.agent/requirements.md` FR-5.12 / FR-5.13
- 多重インスタンス・隔離検証の注意（#177 + TAKO_DATA_DIR）: `.agent/architecture.md` 該当節
