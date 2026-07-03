# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.2.6] - 2026-07-03

### Added

- Remote PWA overhaul — two-layer architecture (#42, #26): history layer (`GET /api/panes/:id/scrollback` + client-side rendering with free scroll and text selection) + live screen layer (REST polling → WebSocket push with viewport-linked auto-resize on connect and reset on disconnect). `<input>` → `<textarea>` for Shift+Enter multiline input (#26). Quick keys via tmux send-keys raw sequences + ctrl toggle mode. CLI `tako remote scrollback` / MCP `tako_remote_scrollback` (51 MCP tools total)
  リモート PWA 二層構成刷新（#42, #26）: 履歴レイヤー（`GET /api/panes/:id/scrollback` + クライアント側描画、自由スクロール・テキスト選択対応）+ ライブ画面レイヤー（REST ポーリング → WebSocket プッシュ、接続時ビューポート連動自動リサイズ + 切断時リセット）。`<input>` → `<textarea>` で Shift+Enter 改行対応（#26）。Quick keys を tmux send-keys 生キーシーケンス経由に変更 + ctrl トグルモード。CLI `tako remote scrollback` / MCP `tako_remote_scrollback`（MCP 計 51 ツール）
- Homebrew update failure recovery (#50): detects "broken-brew" state (app exists but cask ledger is missing after a failed `brew upgrade`), offers zip-based fallback update via status bar button, and adds `tako update repair` (re-register cask ledger) / `tako update apply-zip` (force update via GitHub Releases zip). README troubleshooting section updated
  brew 更新失敗の復旧導線（#50）: `brew upgrade` 失敗後の「.app 実体あり・cask 台帳なし」詰み状態を自動検知し、ステータスバーに zip 更新ボタンを表示。`tako update repair`（cask 台帳の再締結）/ `tako update apply-zip`（zip 強制更新）を追加。README トラブルシューティングに復旧手順を追記

### Changed

- Orchestrator master system prompt now includes 6 quality-ops principles derived from cross-PR review (#53): root-cause-first instructions, same-file serialization, DoD for untested areas, integration review layer, master-owned Closes decisions, and completion definition
  オーケストレーター master 共通 system prompt に品質運用原則 6 点を組み込み（#53）: 根因先行の指示、同一ファイル直列化、機械検証なし領域の DoD、統合レビュー層、master が持つ Closes 判断、完遂の定義

### Fixed

- TCC permission prompts ("access data from other apps") no longer reset across rebuilds and in-app updates (#54): the code signature's designated requirement is now pinned to the bundle identifier instead of the signing certificate. Previously the requirement changed whenever the signing identity changed (multiple Apple Development certificates in the keychain, certificate expiry, or ad-hoc fallback), making macOS treat each build as a different app and invalidate previously granted permissions. Note: updating from ≤0.2.5 requires re-granting once due to the requirement migration; granting Full Disk Access to tako.app suppresses the per-target dialogs entirely (see README troubleshooting)
  TCC の許可（「ほかのアプリからのデータへのアクセス」等）が再ビルド・アプリ内更新でリセットされる問題を修正（#54）: コード署名の designated requirement を署名証明書依存から bundle identifier 固定に変更。従来は署名 identity が変わるたび（キーチェーンに複数の Apple Development 証明書・証明書失効・ad-hoc への劣化）に requirement が変わり、macOS が別アプリと判定して付与済み許可を無効化していた。注意: 0.2.5 以前からの更新時は requirement 移行のため 1 回だけ再許可が必要。tako.app にフルディスクアクセスを付与すると対象別ダイアログ自体が出なくなる（README トラブルシューティング参照）
- Remote PWA: soft keyboard Enter now works on mobile (#41): removed empty-input guard in `send()` that blocked bare Enter (needed for Claude Code permission prompts), added `<form>` submit event capture as reliable mobile Enter path, and `enterkeyhint="send"` for soft keyboard send button
  リモート PWA: スマホのソフトキーボードから Enter が送信可能に（#41）: 空入力をブロックしていた `send()` のガードを除去（Claude Code の許可プロンプトに空 Enter で応答するケースに対応）、`<form>` submit イベントで確実にモバイル Enter を捕捉、`enterkeyhint="send"` でソフトキーボードに送信ボタンを表示
- Remote PWA: empty Enter regression from #45 restored + WebSocket zombie reconnection prevented (#51, #52): re-enabled empty-input send button that was disabled during the textarea migration; WS event handlers are now nullified before `close()` to prevent stale pane connections from triggering reconnection timers
  リモート PWA: #45 で落ちた空 Enter 送信経路を復旧 + WS ゾンビ再接続を防止（#51, #52）: textarea 移行で無効化されていた空入力送信ボタンを復活、`close()` 前に WS イベントハンドラを null 化して旧ペインの非同期 onclose による再接続タイマー設定を根治
- Full-width character click now resolves to the correct cell (#37): click coordinate calculation used font shaping advance instead of grid-based `cell_width × column` — unified to the grid coordinate system
  全角文字行のクリックが正しいセルに解決するように修正（#37）: クリック座標計算がフォント shaping の advance 値を使用しておりグリッド座標系（`cell_width × 列番号`）と不一致だった問題を統一
- `orchestrator watch` no longer false-fires WORKER_IDLE when session_id is omitted (#44): pane → backend session → pid ancestor traversal now auto-resolves the session, using `claude agents --json` status as primary signal (screen pattern matching is fallback only)
  `orchestrator watch` が session_id 未指定時に WORKER_IDLE を空振りする問題を根治（#44）: pane → バックエンドセッション → pid 祖先辿りで session を自動解決し、`claude agents --json` の status を一次シグナル化（画面パターン推定はフォールバック）
- Self-test no longer hangs under CPU contention (#39): terminal rendering changed from one-div-per-character to grouped runs of same-style half-width characters, reducing GPUI element count by 60–90%
  CPU 競合下でセルフテストがハングする問題を解消（#39）: ターミナル描画を「1 文字 = 1 div」から同スタイル連続半角文字のグループ化に変更、GPUI 描画要素数を 60〜90% 削減
- IME candidate window no longer appears at bottom-left when cursor is hidden (#29): added `ime_cursor` field to `Screen` that tracks cursor position even when `CursorShape::Hidden` (used by TUI apps like Claude Code). `bounds_for_range` now always returns a valid position
  カーソル非表示時に IME 変換候補ウィンドウが画面左下に出る問題を修正（#29）: `Screen` に `ime_cursor` フィールドを追加し、`CursorShape::Hidden`（Claude Code 等の TUI アプリが使用）でもカーソル位置を追跡。`bounds_for_range` が常に有効な位置を返すよう修正

## [0.2.5] - 2026-07-03

### Fixed

- Shift+Enter now inserts a newline in Claude Code on machines without tmux (#28): modified-key CSI u encoding is now enabled for all panes, not just tmux-backend panes. Homebrew cask installs (which don't depend on tmux) were silently sending bare `\r` instead of CSI u sequences, breaking multiline input in Claude Code
  tmux 未導入環境でも Claude Code の Shift+Enter 改行が効くように修正（#28）: 修飾付きキーの CSI u 送出を tmux バックエンドペイン限定から全ペインに拡大。Homebrew cask 配布先（tmux 非依存）では素の `\r` が送出され、Claude Code のマルチライン入力が動作しなかった
- Tab/pane layout persistence no longer requires tmux (#30): saving and restoring the layout was silently disabled on machines without tmux (e.g. Homebrew installs), losing all tabs across restarts. Without tmux, the layout is still saved and restored with fresh shells at the saved cwd; with tmux, full restore (running processes) works as before
  タブ / ペイン構成の永続化が tmux 必須でなくなった（#30）: tmux 未導入マシン（Homebrew 配布先等）では保存・復元が無音で無効化され、再起動で全タブが消えていた。tmux 不在でも構成は保存され、保存 cwd の新シェルで復元される。tmux があれば従来通り実行中プロセスごと完全復元
- PTY deaths (shell exit, tmux client kicked, backend tmux server killed) no longer kill backend sessions nor delete layout.json (#30): only user/AI-initiated closes (× button, cmd+W, CLI/MCP close) do. When every pane dies at once (e.g. the backend tmux server is killed externally), tako now keeps layout.json and restores the full tab structure on next launch
  PTY 死亡（シェル exit・tmux クライアント kick・バックエンドサーバー kill）ではバックエンドセッションの kill と layout.json の削除を行わなくなった（#30）: 削除はユーザー / AI の明示 close（× / cmd+W / CLI・MCP close）に限定。バックエンド tmux サーバーが外部から kill され全ペインが一斉終了しても layout.json は保持され、次回起動でタブ構成が復元される

### Added

- In-app update with auto-detection of install method (#36): automatically detects whether tako was installed via Homebrew Cask or GitHub Releases and runs the appropriate update command. Shows a confirmation dialog warning that running processes will be lost, then saves layout → applies update → auto-restarts. Also detects duplicate `tako` CLI binaries on PATH. CLI `tako update status/check/apply` + MCP `tako_update` (50 MCP tools total)
  アプリ内更新 + 配布系統自動判別（#36）: Homebrew Cask / GitHub Releases のどちらでインストールされたかを自動判別し、適切な更新コマンドを実行。更新前にプロセス消失を警告する確認ダイアログを表示し、レイアウト保存 → 更新適用 → 自動再起動。PATH 上の `tako` CLI 重複も検知。CLI `tako update status/check/apply` + MCP `tako_update`（MCP 計 50 ツール）
- Persistence diagnostics (#30): restore outcome/reason and explicit layout deletions are logged to `<data_dir>/persist.log` (rotated at 256KB); corrupted layout files are stashed as `layout.json.corrupt`; `tako persist` / MCP `tako_persist` now report `layout_path` / `layout_exists` / `last_restore` / `log_path`
  永続化の診断機能（#30）: 復元の成否・理由・layout.json の明示削除を `<data_dir>/persist.log` に記録（256KB でローテート）。破損した layout.json は `layout.json.corrupt` へ退避。`tako persist` / MCP `tako_persist` が `layout_path` / `layout_exists` / `last_restore` / `log_path` を返すようになった

## [0.2.4] - 2026-07-02

### Fixed

- **Hotfix (#27)**: default orchestrator profile no longer hardcodes `claude-opus-4-6[1m]`, which made `tako master` unusable on Pro plans (1M-context models are Max/API-only). New default is **no model specification** — the master launches with the claude CLI's default model. `[1m]` models now require explicit opt-in in the profile and print a warning at launch
  **緊急修正（#27）**: 既定プロファイルの `claude-opus-4-6[1m]` ハードコードを廃止（1M コンテキスト版は Max/API プラン限定のため、Pro プランで `tako master` が起動不能だった）。新しい既定は**モデル無指定** = claude CLI の既定モデルで起動。`[1m]` モデルはプロファイルへの明示 opt-in のみとなり、起動時に警告を表示
- Automatic migration (#27): a `default.yaml` still containing the old hardcoded `model: claude-opus-4-6[1m]` is detected at startup (`tako master` / `tako setup` / spawn) and the model line is removed with a backup (`default.yaml.backup-1m`). User-specified models other than the old default are respected
  自動マイグレーション（#27）: 旧既定値 `model: claude-opus-4-6[1m]` が残る `default.yaml` を起動時（`tako master` / `tako setup` / spawn）に検出し、バックアップ（`default.yaml.backup-1m`）を取って model 行を除去。旧既定値以外のユーザー指定モデルは尊重する

### Changed

- Config precedence clarified (#27): `profiles/*.yaml` is the single source of truth for master/worker launch settings. The unused `master_model` / `worker_model` / `effort` keys in `config.yaml` are removed (legacy keys are ignored); `config.yaml` now only holds setup state and `auto_close` / `auto_push`. The setup assistant now writes model settings to profiles and no longer recommends 1M-context models to Pro-plan users
  設定の優先順位を明文化（#27）: master/worker の起動設定の正は `profiles/*.yaml` に一本化。誰にも読まれていなかった `config.yaml` の `master_model` / `worker_model` / `effort` キーを廃止（旧キーが残っていても無視される）。`config.yaml` は setup 状態と `auto_close` / `auto_push` のみに。セットアップアシスタントはモデル設定を profiles に書き込み、Pro プランユーザーに 1M コンテキスト版を提案しないよう修正

### Added

- Profile management CLI/MCP (#27): `tako orchestrator profiles list/show/set` (`--model` / `--clear-model` / `--effort` etc.) + MCP `tako_orchestrator_profiles` (49 MCP tools total) — fix a broken profile without editing YAML by hand
  プロファイル管理 CLI/MCP（#27）: `tako orchestrator profiles list/show/set`（`--model` / `--clear-model` / `--effort` 等）+ MCP `tako_orchestrator_profiles`（MCP 計 49 ツール）— YAML 手編集なしでプロファイルを修復可能に
- Orchestrator profile extensions (#25): per-profile worker model policy (`inherit` / `fixed` / `delegate`), system prompt block control (`disable` / `override` / `prepend` / `append`), and session identity injection
  オーケストレータープロファイル拡張（#25）: プロファイル単位の子 worker モデル制御（`inherit` / `fixed` / `delegate`）、system prompt のブロック単位制御（`disable` / `override` / `prepend` / `append`）、セッション identity 注入
- Remote access (#23 Phase A): WebSocket screen push channel `GET /ws?pane=<id>` — server-side 250ms diff detection, ANSI-colored screen + cursor/size (HTTP polling remains as fallback)
  リモートアクセス（#23 フェーズ A）: WebSocket 画面プッシュ `GET /ws?pane=<id>` — サーバー側 250ms 差分検知、ANSI 色付き画面 + カーソル/サイズ（HTTP ポーリングはフォールバックとして維持）
- Remote screen API: `?ansi=1` (colored output for xterm.js), `?lines=N` (scrollback history), cursor position and pane size in response
  リモート画面取得 API: `?ansi=1`（xterm.js 用色付き出力）、`?lines=N`（スクロールバック履歴）、カーソル位置・ペインサイズを応答に追加
- Viewport-linked resize: `POST /api/panes/:id/resize` + CLI `tako tmux resize` + MCP `tako_tmux_resize`
  ビューポート連動リサイズ: `POST /api/panes/:id/resize` + CLI `tako tmux resize` + MCP `tako_tmux_resize`
- Agent list API: `GET /api/agents` (claude agents --json proxy with tmux pane mapping) + CLI `tako remote agents` + MCP `tako_remote_agents`
  エージェント一覧 API: `GET /api/agents`（claude agents --json プロキシ + tmux ペイン対応付け）+ CLI `tako remote agents` + MCP `tako_remote_agents`
- Conversation log API: `GET /api/sessions/:id/messages?tail=N` (normalized Claude Code transcript) + CLI `tako remote messages` + MCP `tako_remote_messages`
  会話ログ API: `GET /api/sessions/:id/messages?tail=N`（Claude Code transcript の正規化）+ CLI `tako remote messages` + MCP `tako_remote_messages`
- Pane close endpoint: `POST /api/panes/:id/close`
  ペインを閉じるエンドポイント: `POST /api/panes/:id/close`

### Changed

- Connect URL token moved to URL fragment (`/#/connect?token=...`) — no longer appears in server/tunnel access logs or Referer
  接続 URL のトークンを URL fragment 化（`/#/connect?token=...`）— サーバー/トンネルのアクセスログや Referer に残らない

### Fixed

- Shift+Enter now inserts a newline in Claude Code on machines without tmux (#28) — modified-key CSI u encoding is now enabled for all panes, not just tmux-backend panes; the setup assistant no longer claims to configure Claude Code keybindings
  tmux 未導入環境でも Claude Code の Shift+Enter 改行が効くように修正 (#28) — 修飾付きキーの CSI u 送出を tmux バックエンドペイン限定から全ペインに拡大。setup アシスタントが Claude Code 側キーバインドの設定を掲げる案内も廃止
- KV relay URL mismatch between daemon and PWA (unified to the live worker)
  デーモンと PWA で KV リレー URL が不一致だった問題を修正（稼働中の Worker に統一）
- Prompt delivery to claude TUI is now verified (#32): text is pasted via bracketed paste, the submitting Enter is sent as a separate delayed key event, and the input box is checked to be empty afterwards (with standalone Enter retries) — fixes multiline prompts stuck in the input box and intermittent Enter misses in `tako orchestrator spawn` / `tako send` / MCP `tako_send_input`
  claude TUI へのプロンプト送達を検証付きに（#32）: 本文は bracketed paste で貼り付け、送信の Enter は分離した単独キーとして遅延送信し、送信後に入力欄が空へ戻ったことを検証（残留時は Enter 単独再送）— `tako orchestrator spawn` / `tako send` / MCP `tako_send_input` のマルチライン残留・Enter 空振りを修正
- Trust dialog no longer consumes the spawn prompt (#32): the worker cwd is pre-trusted in `~/.claude.json` before launch, with on-screen dialog detection → auto-accept as fallback
  信頼確認ダイアログが spawn プロンプトを消費する問題を修正（#32）: 起動前に worker の cwd を `~/.claude.json` で事前信頼し、フォールバックとしてダイアログ検出 → 自動承諾も実装
- tmux session-targeted send/read fallback was broken on tmux 3.6 (`can't find pane: =<session>`) — target-pane commands now use the explicit `=<session>:` form
  tmux session 指定の send/read フォールバックが tmux 3.6 で壊れていた問題を修正（`can't find pane: =<session>`）— target-pane 系コマンドは `=<session>:` 形式に統一

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
