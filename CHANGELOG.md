# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] - 2026-06-26

### Added

#### Terminal Core / ターミナル基盤

- macOS terminal with tabs, pane split/resize/focus, 256-color/truecolor, copy-on-select, bracket paste, IME inline composition, .app bundle with code signing
  macOS ターミナル: タブ・ペイン分割/リサイズ/フォーカス・256色/truecolor・copy-on-select・ブラケットペースト・IMEインライン変換・コード署名付き.appバンドル

#### CLI & MCP / CLI・MCPサーバー

- `tako` CLI with subcommands: split, send, focus, list, read, close, title, resize, equalize, tab operations
  `tako` CLI サブコマンド: split/send/focus/list/read/close/title/resize/equalize/tab操作
- Built-in MCP server (stdio bridge + Streamable HTTP) with zero-config Claude Code connection (`tako setup-mcp`)
  内蔵MCPサーバー（stdioブリッジ + Streamable HTTP）、Claude Codeゼロ設定接続（`tako setup-mcp`）

#### Passive Detection / パッシブ検知

- Shell integration via OSC 7/133 (zsh/bash/fish auto-injection, cwd/state/exit_code tracking)
  シェル統合（OSC 7/133、zsh/bash/fish自動注入、cwd/状態/終了コード追跡）
- Listen port detection (macOS libproc) with inline suggestion chips
  listenポート検知（macOS libproc）+ インライン提案チップ
- AI auto-rename for tabs and panes (Claude Haiku + heuristic fallback)
  タブ・ペインのAI自動リネーム（Claude Haiku + ヒューリスティックフォールバック）
- tmux session visibility panel with tab-grouped display
  tmuxセッション可視化パネル（タブ別グルーピング表示）

#### Workspace / ワークスペース

- File tree sidebar with multi-root workspace display (per-tab cwd aggregation)
  ファイルツリーサイドバー（タブごとのcwd集約マルチルート表示）
- Code preview with syntax highlighting (syntect) and line numbers
  シンタックスハイライト付きコードプレビュー（syntect）+ 行番号
- Markdown preview with rendered/code toggle
  Markdownプレビュー（レンダリング/コード切替）
- Context menu (path copy, Finder reveal, cd, rename, new file/folder, trash) with inline editing
  コンテキストメニュー（パスコピー/Finder表示/cd/リネーム/新規ファイル・フォルダ/ゴミ箱）+ インライン編集
- Drag & drop path insertion from file tree to terminal pane
  ファイルツリーからターミナルペインへのD&Dパス挿入

#### Session Persistence / セッション永続化

- tmux backend: full session restore on restart (running processes + screen content)
  tmuxバックエンド永続化: 再起動時のセッション完全復元（実行中プロセス + 画面内容）
- Graceful fallback to direct spawn when tmux is unavailable; `tako persist` toggle
  tmux未使用環境では直接spawnへ劣化、`tako persist` でトグル可能

#### Shelving / たまり場

- Pane and tab shelving: hide from view while keeping processes alive
  ペイン/タブ退避: プロセスを維持したまま表示から除外
- Drawer UI with live terminal preview cards, horizontal scroll, and drag-and-drop restore
  ライブプレビューカード付きドロワーUI（横スクロール + D&D復帰）

#### Panel & UI / パネル・UI

- Status bar (Zed/VSCode style) with file tree, tmux, and git sidebar toggles
  ステータスバー（Zed/VSCode風）+ ファイルツリー/tmux/gitサイドバートグル
- Integrated tmux view with status badges, orphan detection, and one-click cleanup
  統合tmuxビュー（状態バッジ、orphan検出、ワンクリッククリーンアップ）
- Tab tree: hover preview, pin-to-float, collapse/expand
  タブツリー: ホバープレビュー・ピン留めフロート・折りたたみ
- git graph + diff viewer (sidebar accordion: branch/changes/commits/diff)
  git graph + diffビューア（サイドバーアコーディオン: ブランチ/変更/コミット/diff）

#### Orchestrator / オーケストレーター

- Built-in orchestrator: `tako master` for multi-agent coordination with worker spawn/watch/status
  内蔵オーケストレーター: `tako master` でマルチエージェント連携（worker spawn/watch/status）
- Project management via `tako orchestrator projects`
  `tako orchestrator projects` によるプロジェクト管理

#### Remote Access / リモートアクセス

- HTTP API + PWA for remote terminal access with cloudflared tunnel integration
  HTTP API + PWAリモートターミナル（cloudflaredトンネル統合）
- Daemon mode with QR code display (`tako remote start/stop/status`)
  QRコード表示付きデーモンモード（`tako remote start/stop/status`）

#### Reliability & Performance / 信頼性・パフォーマンス

- MCP/IPC restart resilience (fixed socket path + persistent token)
  MCP/IPC再起動耐性（固定ソケットパス + 永続トークン）
- Full-width character rendering fix, half-width character disappearance fix
  全角文字幅の根本修正、半角文字消失バグ根治
- Spawn reliability: TAKO_PANE_ID stale issue root cause fix
  spawn信頼性: TAKO_PANE_ID stale問題の根治
- UI rendering optimization (16ms debounce, event-driven file tree sync, cached style runs and rows)
  UI描画最適化（16msデバウンス、イベント駆動ファイルツリー同期、スタイルラン/行キャッシュ）
- main.rs modular decomposition (13,736 → 8,359 lines, 39% reduction)
  main.rsモジュール分割（13,736 → 8,359行、39%削減）

#### Distribution / 配布

- GitHub Releases distribution with `scripts/release.sh` (auto version + CHANGELOG extraction)
  GitHub Releases配布（`scripts/release.sh`、バージョン自動読み取り + CHANGELOG連携）
- Version management via `[workspace.package]` in Cargo.toml
  Cargo.toml `[workspace.package]` によるバージョン一元管理
