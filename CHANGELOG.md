# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.2.2] - 2026-07-02

### Added

- Orchestrator profiles: `tako master -<name>` to launch with different configurations (model, effort, system prompt, project subset)
  オーケストレータープロファイル機能: `tako master -<名前>` で設定別のマスターを起動可能（モデル・effort・システムプロンプト・プロジェクトサブセット）
- Profile management: profiles stored in `~/Library/Application Support/tako/orchestrator/profiles/`
  プロファイル管理: `~/Library/Application Support/tako/orchestrator/profiles/` に YAML で保存
- `tako setup --check` now shows available profiles
  `tako setup --check` でプロファイル一覧を表示
- Default profile auto-created on first `tako master` run
  初回 `tako master` 実行時にデフォルトプロファイルを自動生成
- Backward compatible: `tako master dev` (old suffix form) still works
  後方互換: `tako master dev`（旧サフィックス形式）も引き続き動作

## [0.2.1] - 2026-07-02

### Added

- In-app auto-update notification: a status bar notification appears when a new stable release is available, with one-click update
  アプリ内自動更新通知機能を追加。新しい安定版がリリースされるとステータスバーに通知が表示され、ワンクリックで更新できます

## [0.2.0] - 2026-07-02

### Added

#### Interactive Setup / 対話式セットアップ

- `tako setup`: interactive setup command for Claude Code configuration (model selection, effort, CLAUDE.md backup)
  `tako setup`: Claude Code 設定の対話式セットアップコマンド（モデル選択・effort 設定・CLAUDE.md 自動バックアップ）
- `tako setup --reset`: reset and restart setup in one step
  `tako setup --reset`: リセット後にそのままセットアップを再開

#### Menu Bar & Window Management / メニューバー・ウィンドウ管理

- Menu bar: Open Directory, Open Repository, New Window; CLI `tako --dir` for launching with a specific directory
  メニューバー拡充: ディレクトリを開く・リポジトリを開く・新規ウィンドウ + CLI `tako --dir`

#### Media & File Preview / メディア・ファイルプレビュー

- mp4 preview: seek with arrow keys/click, keyboard shortcuts for playback control
  mp4 プレビュー: 矢印キー/クリックでシーク、キーボードショートカットで再生制御
- WebView pane: embedded Chrome-based web view within pane (headless mode, isolated profile)
  WebView ペイン: ペイン内の埋め込み Chrome ベース Web ビュー（headless モード、一時プロファイル）

#### Drag & Drop / ドラッグ＆ドロップ

- OS-level drag & drop: drop files/folders onto tako with context-aware behavior per drop target
  OS レベル D&D: ファイル/フォルダを tako にドロップ、ドロップ先に応じた挙動の出し分け

### Improved

#### Documentation Site / ドキュメントサイト

- Documentation site (tako-docs.pages.dev): Claude Design theme, improved content, sidebar widgets, mascot fix
  ドキュメントサイト（tako-docs.pages.dev）: Claude Design テーマ刷新・コンテンツ充実・サイドバーウィジェット・マスコット修正
- Distribution research and implementation draft (.pkg, Homebrew Cask)
  配布方法の調査結果と実装ドラフト（.pkg、Homebrew Cask）

#### Distribution / 配布

- Homebrew Cask support: `brew install --cask takushio2525/tako/tako`
  Homebrew Cask 対応: `brew install --cask takushio2525/tako/tako`

#### Project Infrastructure / プロジェクト基盤

- Issue templates for bug reports and feature requests
  Issue テンプレートの追加（バグ報告・機能リクエスト）
- .gitattributes for consistent line endings and binary detection
  .gitattributes 追加（改行コード統一・バイナリ判定）

### Fixed

- `orchestrator_spawn` CLI/MCP: pane/tab priority order was inverted
  `orchestrator_spawn` CLI/MCP: pane/tab 優先順位の逆転を修正
- `orchestrator_spawn`: pane/tab parameter now required to prevent ambiguous placement
  `orchestrator_spawn`: pane/tab 指定を必須化し配置の曖昧さを解消
- WebView Chrome launch: use temporary profile to avoid conflicts with existing Chrome sessions
  WebView Chrome 起動: 一時プロファイルで既存 Chrome との競合を回避
- `tako setup`: model suggestion based on user context + latest info; effort fixed to high; interactive mode
  `tako setup`: モデル提案をユーザー状況ベースに改善、effort を high 固定、対話モード修正

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
