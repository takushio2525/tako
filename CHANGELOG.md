# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added / 機能追加

- 委任台帳: spawn/run 時に task_type × model × 結果を自動蓄積 + 検収記録 CLI + ユーザーフィードバック反映 + 判断基準の二層化 (#292)
  Delegation ledger: auto-records task_type × model × outcome on spawn/run, CLI/MCP for acceptance recording (record/amend), judgment criteria two-layer injection (built-in defaults + local overrides), survey frequency control (#292)

### Fixed / 修正

- MCP 登録パスが消失しても検知・自己修復されない問題を修正: 安定パス優先解決 + ヘルスチェック + master 起動時警告 (#299)
  Fixed MCP registration pointing to a dead binary path going undetected: stable path resolution (/Applications priority), health check on existing registrations, and master startup warning (#299)
- master がタスク受付時に登録プロジェクトの照合を最優先で行うよう順序制約を追加（プロジェクト名の誤認防止）(#263)
  Added project resolution gate (Step 0) to master task intake: registered projects are matched before general exploration or browser access (#263)
- nightly-release が launchd 環境（npm 不在の PATH）で GitHub Release 未作成のまま停止する問題を修正 (#297)
  Fixed nightly-release stopping without creating GitHub Release when npm is not in PATH (launchd environment) (#297)

## [0.5.5] - 2026-07-17

Nightly patch release (automated). Changes since v0.5.4:
夜間パッチリリース（自動）。v0.5.4 以降の変更:

- [機能追加] setup 完了後にエージェント起動ランチャーを追加 (#295) (#296)
- [改善] remote の dispatch 統合 + WS broadcaster 化 + API v2 (#281) (#294)
- [修正] remote daemon の封じ込め修正 (#280) (#290)
- [修正] self/spawn の caller 解決に pid 祖先辿りを一次化 (#288) (#291)
- [ドキュメント] Tailscale Serve PoC 実測レポート (#279)
- [ドキュメント] tako remote 全面刷新計画（Tailscale一本化 + UI刷新）に改訂

## [0.5.3] - 2026-07-15

Nightly patch release (automated). Changes since v0.5.2:
夜間パッチリリース（自動）。v0.5.2 以降の変更:

- [ドキュメント] Issue 258の完了状態を記録 (#258) (#261)
- [修正] アプリ全体のメモリ肥大を抑制 (#258) (#260)
- [修正] PDF プレビューの周期的暗転を根治: イベントフィルタ強化 + ファイルスタンプ比較 + ダブルバッファ化 (#257) (#259)

## [0.5.2] - 2026-07-15

Nightly patch release (automated). Changes since v0.5.1:
夜間パッチリリース（自動）。v0.5.1 以降の変更:

- [修正] MCP 全ツールの未知パラメータを検出してエラーにする (#227) (#255)
- [改善] tmux タブのリファクタ: 表示情報の充実・復帰ボタン・orphan 判定改良 (#183) (#254)
- [機能追加] プレビューペインのバックグラウンド退避対応 (#230) (#253)
- [機能追加] 受け入れゲートの状態機械: タスクに機械検証可能な述語を持たせる (#244) (#252)
- [機能追加] プレビュー目次ナビゲーションを実装 (#232) (#251)
- [機能追加] worker 異常イベントの種別拡張: question / model_switched / context_high (#243) (#250)
- [機能追加] worker タスクのチェックポイント永続化と resume 操作 (#242) (#249)
- [ドキュメント] #233 の完了状態を記録 (#248)
- [機能追加] プレビューのライブリロードを実装 (#233) (#247)
- [機能追加] メニューバー拡充: Open Directory/Repository/Remote/Recent + CLI/MCP 1:1 (#20) (#246)
- [ドキュメント] LangGraph 概念の tako 翻訳: オーケストレーション設計メモ (#161) (#245)
- [ドキュメント] PDF品質改善とズームの完了状態を記録 (#231) (#234) (#241)
- PDFプレビュー品質改善とPDF・画像ズーム対応 (#231 / #234) (#240)
- [修正] nightly-release.sh: worktree からの launchd 登録で本体リポへ正規化 (#205) (#239)
- [修正] AI のコマンド操作でフォーカスを奪わないよう統一 (#211) (#238)
- [改善] 狭ペインのヘッダを「...」メニューに集約: 最小化/クローズを選択式に (#229) (#237)
- [機能追加] 外部ファイル D&D の挙動をドロップ先で出し分け (#21) (#236)
- [機能追加] setup をマルチエージェント対応 (#226) (#235)
- [修正] worker の idle/busy 検知精度を改善: 偽 IDLE 根治 + 停滞検知 + 折りたたみ対策 (#224) (#228)
- [改善] 小ペインでの UI 見切れ解消 + プレビューヘッダ刷新 + 右クリックメニュー (#185) (#225)
- [機能追加] ペインのタブ横断 D&D: タブバーへドロップで新タブ化・既存タブへドロップで合流 (#209) (#223)
- [改善] タブバーのオーバーフロー対応: タブ幅自動縮小 + 横スクロール + 自動スクロールイン (#208) (#222)
- [ドキュメント] progress.md に #210/#217 のエントリを追記 + .vite/ を gitignore へ
- [スタイル] UI 大刷新: Claude Design カンプの忠実再現 + 絵文字全廃 + 新規コントロール (#217) (#221)
- [機能追加] sleep-guard の蓋閉じ（lid-close）対応 (#218) (#220)
- [修正] UI スレッドの pmset 同期実行を IOKit FFI へ置換し画面の重さ・点滅を根治 (#212) (#216)
- [修正] 復元後の master role 消失と同一プロファイル複数 master の self/spawn 誤認を根治 (#210) (#215)
- [機能追加] ステータスバーの 🌐 ボタンから Web ビューペインを開く (#207) (#214)
- [改善] mp4 プレビューの操作性改善: ホバー時刻・音量・ループ (#22) (#213)

## [0.5.1] - 2026-07-14

Nightly patch release (automated). Changes since v0.5.0:
夜間パッチリリース（自動）。v0.5.0 以降の変更:

- [改善] プレビュー検索の polish: ヒットハイライト描画・フィールドクリックフォーカス・IME 未確定表示 (#200) (#206)
- [リファクタ] ControlHost trait を 8 つの責務別サブトレイトへ分割 (#86) (#204)
- [改善] MCP HTTP サーバーをリクエスト毎スレッド化し並行処理を可能にする (#84) (#203)
- [機能追加] タブ単位の退避を CLI / MCP から操作可能にする (#85) (#202)
- [機能追加] orchestrator_run の非同期化 (#121) (#201)
- [修正] 検索バーの GUI 直接テキスト入力を実装 (#195)
- [機能追加] master 自己特定 + ctx 監視 + handoff コマンド (#123, #193) (#198)
- [機能追加] プレビュー編集の強化: 自動保存・undo/redo・検索/置換 (#195) (#197)
- [機能追加] セッション会話ログの管理と復元: カタログ + ペイン平文ログ (#112) (#196)
- [ドキュメント] CHANGELOG: #165 レイアウトエンジンを [0.4.0] から [0.5.0] へ移動 (#165)

## [Unreleased]

### Added

- Displayed code, Markdown, image, and PDF previews now live-reload after external file changes (#233). Native OS file events watch only the non-recursive parent directories of displayed files; 300ms debouncing coalesces rapid AI writes, and all rereading, syntect / pulldown-cmark work, image loading, and PDF rasterization run in the background before an atomic UI swap. Scroll position, code/Markdown mode, image/PDF zoom and pan are preserved. Editing buffers are never overwritten; external changes surface through the existing conflict state. The setting defaults on, persists in `settings.json`, and is available 1:1 through dispatch `PreviewReload`, `tako preview-reload [on|off]`, and MCP `tako_preview_reload` (80 tools total). Video remains excluded so playback state is not reset
  表示中のコード・Markdown・画像・PDF が外部ファイル変更後にライブリロードされるようにした（#233）。OS ネイティブイベントで表示ファイルの親ディレクトリだけを非再帰監視し、300ms デバウンスで AI の連続 write をまとめる。再読み込み、syntect / pulldown-cmark、画像読み込み、PDF ラスタライズはすべて background で完成させてから UI を差し替える。スクロール位置、code/Markdown モード、画像/PDF の倍率とパンは保持する。編集バッファは上書きせず、既存の競合状態で通知。設定は既定 ON・`settings.json` 永続化で、dispatch `PreviewReload`、`tako preview-reload [on|off]`、MCP `tako_preview_reload`（全 80 ツール）に 1:1 公開。動画は再生状態をリセットしないため対象外

- PDF and image previews now support content zoom without resizing the pane (#234): pinch, Cmd+/Cmd-, modified scroll, header SVG controls, and a compact percentage indicator cover 25–400%; two-finger scrolling pans the enlarged content and Cmd+0 / clicking the percentage returns to fit width. PDF selection bounds follow the actual zoomed and panned page image. The same state is available 1:1 through dispatch `PreviewView`, `tako preview --zoom 150 --page 3`, and MCP `tako_preview_view` (75 tools total), including page selection, pan deltas, reset, and state reads
  PDF・画像プレビューを、ペイン寸法を変えずにコンテンツズーム可能にした（#234）: ピンチ、⌘+/⌘-、修飾キー付きスクロール、ヘッダの SVG 操作と小型倍率表示で 25〜400% に対応。拡大中は 2 本指スクロールでパンし、⌘0 / 倍率クリックで幅フィットへ戻る。PDF の選択矩形はズーム・パン済みの実ページ画像へ追従する。同じ状態を dispatch `PreviewView`、`tako preview --zoom 150 --page 3`、MCP `tako_preview_view`（全 75 ツール）へ 1:1 公開し、ページ指定・パン差分・リセット・状態取得に対応

- `tako setup` now supports claude / codex / agy end to end (#226): it detects every installed CLI, auto-selects a single candidate or presents an authenticated multi-choice list, reads available auth/plan signals without logging credentials, asks only for unavailable Claude / GPT / Google plan details, and generates a plan-sized `profiles/default.yaml` recommendation (CLI-default models, scaled effort, multi-agent delegate policy). Existing system-prompt and project customizations are preserved. A scratch-HOME/PATH verifier covers the single-Claude and multi-CLI flows
  `tako setup` を claude / codex / agy に全面対応（#226）: インストール済み CLI の全検出、単一時の自動選択、複数時の認証状態つき選択、認証情報を出力しないプラン自動検出、取得不能な Claude / GPT / Google プランだけの質問、プラン規模別 `profiles/default.yaml` 推奨（CLI 既定モデル・effort・複数エージェント delegate）を追加。既存 profile の system prompt / projects カスタマイズは保持する。スクラッチ HOME / PATH の検証スクリプトで Claude 単独・複数 CLI の両フローを実測

### Changed

- `tako setup` now completes the standard authenticated single-CLI flow with zero questions (#262): values resolve in detected → previous → default order with source labels, repeated runs are idempotent, detection wins over stale previous plans with an explicit notice, and a final summary replaces repeated confirmations and the unconditional setup-agent dialog. `--yes` is stdin-independent; `--answers <json|@file|->` supplies agent, plans, instructions, profile, projects, orchestrator behavior, and sleep guard non-interactively. The same payload is available through dispatch `SetupRun` and MCP `tako_setup`, enabling an AI to translate Japanese preferences into a complete setup. `--review` retains the explicit conversational review path
  `tako setup` の認証済み単一 CLI 標準フローを質問ゼロへ刷新（#262）。値を detected → previous → default の順に source ラベルつきで解決し、再実行を冪等化。前回プランと再検出値が違えば通知して検出値を優先し、確認連打と setup agent の無条件起動を最終サマリへ置換した。`--yes` は標準入力非依存、`--answers <json|@file|->` は agent・plan・指示・profile・projects・orchestrator 挙動・sleep guard を非対話指定できる。同じペイロードを dispatch `SetupRun` / MCP `tako_setup` に公開し、AI が日本語の希望を完全な setup へ変換可能にした。明示的な対話見直しは `--review` で維持

### Fixed

- Preview image memory is now bounded by a configurable byte-budgeted LRU (#258). PDF pages are decoded only around the visible page, and eviction explicitly removes both GPUI CPU assets and GPU atlas textures; replaced video frames are also dropped from the atlas. Live reload rasterization is single-flight with one latest retry, completed async run history is capped at 256, and `tako preview-cache [max_mb]` / MCP `tako_preview_cache` expose the 512MiB default budget and live usage
  プレビュー画像メモリを設定可能なバイト予算つき LRU で上限化（#258）。PDF は表示ページ近傍だけをデコードし、退避時は GPUI の CPU asset と GPU atlas texture の両方を明示解放し、置換済み動画フレームも atlas から除去する。ライブリロードのラスタライズは single-flight + 最新 1 件再実行、非同期 run 完了履歴は 256 件上限とし、`tako preview-cache [max_mb]` / MCP `tako_preview_cache` で既定 512MiB の予算と利用状況を公開

- PDF text selection no longer turns into a whole-document selection when dragging from line gaps or page margins (#231). PDF pages are also re-rasterized in the background at the quantized device scale × zoom × viewport width, making Retina and zoomed text sharp while preserving the path-and-raster-keyed `PreviewImageCache`
  PDF の行間・ページ余白からドラッグしたとき全文選択になる不具合を修正（#231）。PDF ページは device scale × zoom × 表示幅を量子化した解像度で background 再ラスタライズし、path + raster key の `PreviewImageCache` を維持したまま Retina・ズーム表示の文字を鮮明化

- UI thread no longer blocks on a `pmset` subprocess every 2 seconds (#212): the sleep-guard AC-power check (introduced in v0.5.0 via #173) ran `pmset -g batt` synchronously on the UI thread from the 2-second periodic tick — 20–30ms per call even when idle, stretching to multi-second stalls under CPU saturation (e.g. 4 parallel `cargo build` workers), which surfaced as a sluggish screen, flickering terminal text, and janky scrolling. Replaced with an IOKit FFI call (`IOPSGetTimeRemainingEstimate`, microseconds). Measured: isolated idle instance `periodic_prep` p50 17–59ms / max 116ms → p50 0ms / max 8ms. Also: per-step sub-spans (`periodic_prep:*`) added to perf diagnostics for future attribution, and perf.log lines no longer interleave when written from multiple threads
  UI スレッドが 2 秒毎に `pmset` サブプロセスでブロックされる問題を修正（#212）: sleep guard の AC 電源判定（v0.5.0 の #173 で導入）が 2 秒毎の定期 tick から `pmset -g batt` を UI スレッドで同期実行しており、アイドルでも 1 回 20〜30ms、CPU 飽和時（cargo build 4 並走等）は秒級まで伸びて「画面が重い・ターミナル表記の点滅・スクロールもっさり」として顕在化していた。IOKit FFI（`IOPSGetTimeRemainingEstimate`、マイクロ秒）へ置換。実測: 隔離アイドル環境の `periodic_prep` p50 17〜59ms / max 116ms → p50 0ms / max 8ms。あわせて perf 診断にステップ別サブスパン（`periodic_prep:*`）を追加し、perf.log の複数スレッド並行書き込みによる行混線も修正

## [0.5.0] - 2026-07-14

### Added

- Background persistence now automatically recovers orphan tmux sessions on startup (#191): when tako restarts after a crash or `kill -9` where layout.json couldn't be saved in time, surviving `tako-*` sessions are auto-discovered and placed into a "Recovery" tab — no manual `tako recover` or `tako tmux open` needed. Recovered sessions join the protected set so orphan cleanup won't kill them. Secondary mode, persist OFF, and tmux-absent environments are unaffected
  バックグラウンド永続化が起動時に orphan tmux セッションを自動復帰するようになった（#191）: クラッシュや kill -9 で layout.json の保存が間に合わなかった場合、生存している `tako-*` セッションを自動発見し「復帰」タブにまとめて配置する。手動の `tako recover` / `tako tmux open` が不要に。復帰セッションは保護リストに入るため orphan cleanup で kill されない。セカンダリモード・persist OFF・tmux 不在では影響なし

- Sleep prevention via IOKit power assertions (#173): new `tako sleep-guard status/set` + MCP `tako_sleep_guard` (61 tools) prevent idle sleep while agents are running. Three modes: `off` / `on` (always awake) / `while-agents-running` (automatic), with power condition `ac-only` / `always`. Settings persist to config.yaml. Status bar shows a ☕ badge while the assertion is held. App Nap is also disabled. `tako setup` gains a sleep-prevention level chooser (setup changelog rev 7)
  IOKit 電源アサーションによるスリープ防止機能（#173）: `tako sleep-guard status/set` + MCP `tako_sleep_guard`（計 61 ツール）でエージェント実行中のアイドルスリープを防止する。3 モード: `off` / `on`（常時）/ `while-agents-running`（自動）、電源条件 `ac-only` / `always`。設定は config.yaml に永続化。アサーション保持中はステータスバーに ☕ バッジを表示。App Nap も無効化。`tako setup` にスリープ防止レベルの選択を追加（setup changelog rev 7）

- Close confirmation dialog for tabs and panes (#172): clicking the × button now shows a summary of what will be lost (pane count, running processes, active workers, tmux sessions) and asks for confirmation. cmd+click bypasses the dialog for power users. Enter confirms, Esc/background-click cancels. The setting persists in config.yaml (`confirm_close`, default true). `tako confirm-close` (CLI) + MCP `tako_confirm_close` (60 tools)
  タブ・ペインの × ボタンに確認ダイアログを追加（#172）: × クリックで失われるもの（ペイン数・実行中プロセス・稼働中 worker・tmux セッション）を要約表示し確認を求める。cmd+クリックでダイアログをスキップして即クローズ（パワーユーザー動線）。Enter=確定 / Esc・背景クリック=キャンセル。config.yaml に永続化（`confirm_close`、既定 true）。CLI `tako confirm-close` + MCP `tako_confirm_close`（計 60 ツール）

- Major terminal scrolling overhaul — pixel-based rendering, local history mirror, enhanced scrollbar (#159): scrolling is now sub-line smooth (Zed editor's line-fraction approach adapted for bottom-anchored terminals) instead of discrete line jumps. Trackpad inertia uses macOS momentum events natively. For tmux-backed panes, the old copy-mode approach (which ate keystrokes and stuttered on each round-trip) is replaced with a local history mirror (`scroll_mirror`): history is captured from tmux in 500-line ANSI chunks and rendered entirely locally — no more key swallowing or latency. The scrollbar gains hover persistence, thumb thickening, track visibility on hover, and continuous (sub-line) thumb positioning with drag follow. All scroll operations go through the same path for CLI/MCP (`backend_scroll_view`) as the UI (development invariant)
  ターミナルスクロールの大幅改善 — ピクセル単位描画・ローカル履歴ミラー・スクロールバー強化（#159）: 1 行単位の離散ジャンプからサブライン単位のスムーススクロールへ全面刷新（Zed エディタの行小数方式をターミナルの下端アンカーに翻案）。トラックパッドの慣性スクロールは macOS の momentum イベントでネイティブに動作。tmux バックエンドペインでは旧 copy-mode 方式（キー飲み込み・往復レイテンシによるカクつき）を廃止し、ローカル履歴ミラー（`scroll_mirror`）へ置換: 500 行チャンクの ANSI キャプチャを完全ローカルで描画し、キー消失と遅延を構造的に解消。スクロールバーはホバー維持・サム太化・トラック表示・サブライン連続位置と追従ドラッグを追加。CLI / MCP の Scroll 操作も UI と同一経路（`backend_scroll_view`、開発不変条件）

- Nightly patch releases now run locally via launchd (#166): `scripts/nightly-release.sh` replaces the failed cloud routine — runs daily at 5:00 via launchd, auto-bumps patch version + generates CHANGELOG section + builds + tags + publishes a GitHub Release with the macOS binary when main has changes since the last tag. Safety: no-op on clean main / dirty worktree / manual release in progress / concurrent run. `--dry-run` for testing, `--install-launchd` / `--uninstall-launchd` for job management
  夜間パッチリリースを launchd ローカルジョブ化（#166）: 失敗し続けていたクラウドルーチンを `scripts/nightly-release.sh` に置換。launchd で毎日 5:00 に実行し、前回タグ以降に main へ変更があれば patch bump → CHANGELOG 自動節 → ビルド → バイナリ付き GitHub Release を自動作成する。安全装置: 変更なし / dirty worktree / 手動リリース進行中 / 多重起動でスキップ。`--dry-run` でテスト、`--install-launchd` / `--uninstall-launchd` でジョブ管理

- Worker spawn layout engine (#165): spawning workers no longer squeezes every pane into ever-thinner columns. With the new default `master-reserved` policy, the spawning pane (master) keeps its share of the screen (default 50%, configurable 0.1–0.9) and workers tile inside a dedicated worker area on its right: `grid` (1 worker = full area → 2 = stacked → 3–4 = quadrant cross → more columns as needed, default) or `spiral` (alternating half-splits, golden-ratio style). The worker area is recognized via each pane's `spawned_by` chain, so panes the user opened manually are never rearranged — when a worker closes (MCP/CLI close, UI ×, or process exit), only the worker area reflows and master/user panes keep their exact rectangles. Configure via config.yaml `spawn_layout`, `tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]`, or the new MCP tool `tako_orchestrator_layout` (59 tools total); `legacy` restores the old right-split behavior. Master/solo system prompts now instruct agents to prioritize the readability of the master pane and user-opened panes when rearranging layouts
  worker spawn のレイアウトエンジンを新設（#165）: spawn のたびに全ペインが横へ等分圧縮される問題を解消。新既定の `master-reserved` ポリシーでは spawn 元（master）が画面の取り分（既定 50%、0.1〜0.9 で設定可）を維持し、worker は右側の worker 領域内に配置される: `grid`（1 体=全面 → 2 体=上下 → 3〜4 体=十字四分割 → 以降は列を追加。既定）/ `spiral`（縦横交互の半分割、黄金比風）。worker 領域は各ペインの `spawned_by` チェーンで認識するため、ユーザーが手動で開いたペインが再配置されることはない — worker の close（MCP/CLI・UI の ×・プロセス exit）時も領域内だけがリフローされ、master とユーザー由来ペインの矩形は不変。設定は config.yaml の `spawn_layout`、`tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]`、新 MCP ツール `tako_orchestrator_layout`（計 59 ツール）から。`legacy` で従来の右等分割へ戻せる。master / solo の system prompt に「レイアウト操作時は master とユーザー由来ペインの可読性を最優先する」行動規範を追記

- `tako orchestrator watch` now emits a `WORKER_ERROR: tako:<pane> (<kind>)` event when a worker stalls on a known error pattern instead of reporting a misleading `WORKER_IDLE` (#157). Detected kinds (all patterns taken from captured real screens): `api_error` (claude "API Error: Connection closed mid-response" etc. — a resume nudge usually recovers it), `usage_limit` (claude / codex usage-limit stop — wait for the reset time), and `limit_dialog` (codex's rate-limit model-switch dialog — answer the dialog). Extra `detail:` / `action:` lines follow the event line so the master can make a first-level decision without reading the pane. `tako_orchestrator_worker_status` (MCP) and `tako orchestrator status` (CLI) return the same classification 1:1 as `status: "error"` plus an `error` object (`kind` / `detail` / `recommended_action`: resume / wait_reset / respond_dialog), and `tako_orchestrator_run` returns `status: "worker_error"` with the same `error` object while skipping auto-close so the worker's context stays recoverable. Guards against false positives: no detection while busy (auto-retry "Retrying…" screens stay busy), "limit reached, now using …" auto-model-switch notices are ignored, api_error detection is limited to the bottom 15 lines so stale scrollback errors after a recovery don't re-fire, and normal WORKER_IDLE / WORKER_GONE behavior is unchanged
  `tako orchestrator watch` が、worker が既知のエラーパターンで停止したとき紛らわしい `WORKER_IDLE` ではなく `WORKER_ERROR: tako:<pane> (<種別>)` イベントを出力するようになった（#157）。検知種別（パターンはすべて実採取画面由来）: `api_error`（claude の「API Error: Connection closed mid-response」等 — 続行指示で復帰できることが多い）、`usage_limit`（claude / codex の usage limit 到達停止 — 解除時刻まで待つ）、`limit_dialog`（codex のレートリミット・モデル切替ダイアログ — ダイアログに応答）。イベント行に続けて `detail:` / `action:` 行が付き、master がペインを読まずに一次判断できる。MCP `tako_orchestrator_worker_status` / CLI `tako orchestrator status` は同じ判別を `status: "error"` + `error` オブジェクト（`kind` / `detail` / `recommended_action`: resume / wait_reset / respond_dialog）として 1:1 で返し、`tako_orchestrator_run` は `status: "worker_error"` + 同 `error` オブジェクトを返して auto_close をスキップする（worker の文脈を復帰可能なまま残す）。誤検知ガード: busy 中は判定しない（自動リトライ「Retrying…」画面は busy のまま）、「limit reached, now using …」の自動モデル切替告知は無視、api_error は末尾 15 行限定（復帰後にスクロールバックへ残った古いエラーで再発火しない）、既存の WORKER_IDLE / WORKER_GONE の挙動は不変

### Changed

- Test tmux sockets are now cleaned up reliably (#116): `TmuxTestGuard` replaces scattered per-file cleanup structs, `kill_server` deletes socket files, and `cleanup_stale_sockets` auto-collects leftovers from aborted test runs (previously accumulated 4,500+ zombie sockets)
  テスト用 tmux ソケットの掃除を信頼性改善（#116）: 散在していたファイル単位の `Cleanup` を共通の `TmuxTestGuard` に統一し、`kill_server` でソケットファイルを削除、`cleanup_stale_sockets` で中断テストの残骸を自動回収する（従来は 4,500 件以上のゾンビソケットが蓄積）

### Fixed

- Scrolling now works correctly on reattached and tmux-view panes, and worker-status polling no longer blocks the UI (#181): three root causes kept #159's pixel scrolling from working on real restored sessions: (1) the mirror-scroll path only checked `backend_sessions`, missing TmuxOpen view panes (which fell through to the direct-pane path where alt-screen history is 0); (2) with persist ON, the view pane's outer PTY is itself backend-wrapped, and backend-first resolution picked the outer wrapper (history 0) instead of the view target; (3) after persist restore, view panes weren't registered in `tmux_view_panes` and nest-detection only searched the default tmux server, missing `--socket tako` targets. Additionally, `OrchestratorWorkerStatus` dispatch ran `claude agents --json` (550–1100ms, Node startup) synchronously on the UI thread — 2000+ stalls in 2h20m of perf.log, matching user-reported jank timing. Fixes: unified `mirror_scroll_pane` (backend ∪ view), view-target-first resolution, backend socket added to nest candidates, and worker_status split into snapshot (UI thread) / compute (background)
  再アタッチ・ビューペインでスクロールが効かず UI がカクつく問題を修正（#181）: #159 のピクセルスクロールが実機の復元セッションで効かない根因 3 件を修正: (1) ミラースクロール判定が `backend_sessions` のみで TmuxOpen ビューペインが直接ペイン扱い（alt screen = 履歴 0 で不発）、(2) persist ON では外側 PTY 自体が backend ラップされ backend 優先解決で外側（history 0）へ誤解決、(3) 復元後は `tmux_view_panes` 未登録 + ネスト候補が既定サーバーのみで `--socket tako` のビュー先が辿れない。加えて `OrchestratorWorkerStatus` dispatch が `claude agents --json`（550〜1100ms、Node 起動）を UI スレッドで同期実行（perf.log 2 時間 20 分で 2000 件超、ユーザー報告時刻と一致）。修正: `mirror_scroll_pane`（backend ∪ view）統一、ビュー先優先の実体解決、backend socket をネスト候補に追加、worker_status を snapshot（UI）/ compute（background）に分離

- AI-pinned file tree folders no longer duplicate or show stale entries (#171): `canonicalize()` is now used consistently across add/remove/list and `sync_filetree_roots`, preventing duplicates caused by symlinks (e.g. `/tmp` vs `/private/tmp`) or cwd overlap. Dead-folder pruning (`prune_dead_folders`) runs on sync, list, and layout restore, automatically removing entries whose paths no longer exist on disk
  ファイルツリーの AI 追加フォルダの重複・残骸を修正（#171）: add / remove / list と `sync_filetree_roots` の全経路で `canonicalize()` による正規パス比較に統一し、symlink 経由（`/tmp` vs `/private/tmp` 等）や cwd との重複表示を解消。`prune_dead_folders` を sync・list・layout 復元の 3 経路で実行し、実体が消えたエントリを自動除去する

- Fixed app-wide intermittent freezes and sluggish PDF viewing / prompt typing (#168, #115). perf.log analysis of 3.3 hours of real usage identified three culprits, all confirmed by measurement: (1) the `OrchestratorWorkerStatus` dispatch ran `claude agents --json` (a login shell + Node startup, 500ms–1s per call) plus tmux/ps subprocesses **on the UI thread** — 4124 calls averaging 687ms (max 6.2s, 47 minutes of cumulative UI blocking; every recorded 0.5s+ UI stall co-occurred with it); (2) PDF previews rebuilt `gpui::Image` from all page PNGs **every frame** (full byte-hash per image), degrading frame construction to p50 96ms on a 71-page PDF (normally 2ms); (3) opening a PDF rasterized every page synchronously on the UI thread (1354ms block). Fixes: subprocess-bearing read-only dispatches (worker status / git log / git diff) now collect their context on the UI thread in microseconds and run in the background for both CLI and MCP (`TAKO_OFFLOAD=0` restores the old path; `claude agents --json` also gains a 2s TTL cache with lock serialization), preview images are cached as `Arc<gpui::Image>` per pane and reused while the path is unchanged (PDF viewing: p50 96ms → 1–3ms/frame), and PDF/video loading moved to the background behind a "loading…" placeholder (`tako open` PDF response: 1354ms → 48ms). Measured effect on UI responsiveness: a concurrent `tako list` during worker_status dropped from 159–204ms to 4–5ms. Adds a permanent main-thread stall watchdog (`diag::perf_span`): 32ms+ UI-thread occupations are logged with the culprit's tag, 2s+ hangs are reported mid-flight, `TAKO_PERF_VERBOSE=1` emits per-tag latency distributions every 10s, and `TAKO_PERF_LOG` redirects the log for isolated measurements
  アプリ全体の間欠フリーズと PDF 閲覧・プロンプト入力のモサモサを修正（#168、#115）。実運用 3.3 時間分の perf.log 分析で 3 犯を計測特定: (1) `OrchestratorWorkerStatus` dispatch が `claude agents --json`（ログインシェル + Node 起動 = 1 回 500ms〜1s）+ tmux / ps サブプロセスを **UI スレッドで同期実行** — 4124 回・平均 687ms（最大 6.2s、UI ブロック累計 47 分。記録された 0.5s+ の UI ストールは全件これと共起）、(2) PDF プレビューが**毎フレーム**全ページ PNG から `gpui::Image` を再構築（画像ごとに全バイトハッシュ）し、71 ページ PDF でフレーム構築が p50 96ms に劣化（通常 2ms）、(3) PDF を開く瞬間に全ページラスタライズを UI スレッドで同期実行（1354ms ブロック）。修正: サブプロセスを伴う read-only dispatch（worker status / git log / git diff)は UI スレッドでは µs オーダーの文脈収集だけ行い CLI / MCP 両経路で background 実行（`TAKO_OFFLOAD=0` で旧経路。`claude agents --json` には TTL 2 秒キャッシュ + ロック直列化も追加）、プレビュー画像はペインごとに `Arc<gpui::Image>` でキャッシュし path 不変の間は再利用（PDF 表示中: p50 96ms → 1〜3ms/フレーム）、PDF / 動画のロードは「読み込み中…」プレースホルダの背後で background 化（`tako open` の PDF 応答: 1354ms → 48ms）。UI 応答性の実測効果: worker_status 実行中の並行 `tako list` が 159〜204ms → 4〜5ms。恒久のメインスレッド・ストール診断（`diag::perf_span`）を同梱: 32ms 超の UI スレッド専有を犯人タグ付きで記録、2 秒超のハングは実行中に中間報告、`TAKO_PERF_VERBOSE=1` で 10 秒ごとにタグ別レイテンシ分布、`TAKO_PERF_LOG` で隔離実測用にログ先を変更できる

- Fixed mouse escape-sequence fragments (e.g. `4;45;18M` / `<64;12;17M`) leaking into TUI input fields as plain text (#167). When scrolling a mouse-reporting TUI (claude etc.), tako forwards SGR wheel reports; if the byte stream stalls mid-sequence for more than tmux's escape-time (10ms) — which inertial-scroll floods and UI-thread stalls can cause — tmux commits the lone ESC as a key and forwards the remainder as literal text into the inner app's input field (reproduced against real claude in an isolated tmux). Two-layer fix: backend-pane wheel reports no longer travel through the outer client PTY at all — they are injected directly into the tmux server via `send-keys -H` (structured socket data, immune to splitting/escape-time), with SGR/X10 chosen by the inner app's `#{mouse_sgr_flag}`; and all wheel forwarding is token-bucket rate-limited (150 events/s, burst 8) so in-flight bytes stay far below the PTY buffer during stalls. Excess wheel events are dropped, which is harmless for relative scrolling
  マウスエスケープシーケンスの断片（`4;45;18M` / `<64;12;17M` 等）が TUI の入力欄にテキストとして混入するバグを修正（#167）。マウスレポート要求 TUI（claude 等）のスクロールで tako は SGR ホイールレポートを転送するが、バイト列がシーケンス途中で tmux の escape-time（10ms）を超えて停滞する（慣性スクロールの洪水や UI スレッドのストールで起きる）と、tmux が ESC を単独キーとして確定し残りを平文として内側アプリの入力欄へ流していた（隔離 tmux + 実 claude で再現）。二層で修正: バックエンドペインのホイールレポートは外側クライアント PTY を一切通らず `send-keys -H` で tmux サーバーへ直接注入（ソケット越しの構造化データのため分割・escape-time と無縁。SGR / X10 は内側の `#{mouse_sgr_flag}` で出し分け）+ 全ホイール転送にトークンバケットのレート制限（150 イベント/秒・バースト 8）を導入し、停滞時の飛行中バイト量を PTY バッファより十分小さく保つ。超過ホイールイベントは破棄する（相対スクロールのため無害）

- Fixed a critical bug where all terminal panes could vanish from the UI while their processes kept running in backend tmux sessions (#177). A dev/test instance launched with only `TAKO_DISCOVERY_DIR` isolated would pass the multi-instance guard (which only checked discovery's control.json), restore the production layout.json as primary, and its `new-session -A -D` re-attach would steal every tmux client from the live GUI — killing its PTYs in one sweep, after which the periodic save overwrote the healthy layout with the degraded remnant. Three layers of defense were added: a **restore-takeover guard** that scans `tmux list-clients` before restoring and demotes the new instance to secondary mode when any target session still has a client owned by a live tako-app (works regardless of env-var isolation combinations); a **degraded-save guard** that rotates layout.json into generation backups (`.bak.1`–`.bak.3`) before any save that would drop the pane count below half (with a 10-minute rotation guard so cascading shrinks can't push the healthy generation out); and a **one-shot isolation mode** `TAKO_ISOLATED=1` that isolates discovery, persistence, and the tmux socket together so experimental launches can't half-isolate. persist.log lines now include the writing pid for post-incident analysis
  UI から全ターミナルペインが消失する（実体プロセスはバックエンド tmux セッションで生存）重大バグを修正（#177）。`TAKO_DISCOVERY_DIR` だけを隔離した dev / 検証インスタンスが多重起動ガード（discovery の control.json しか見ない）を素通りしてプライマリ判定になり、本番 layout.json を復元 → 再 attach の `new-session -A -D` が稼働中 GUI の tmux クライアントを全部強奪 → PTY 一斉死亡 → 定期保存が縮退レイアウトで健全な layout.json を上書きしていた。三層の防御を追加: **復元強奪ガード**（復元前に `tmux list-clients` を走査し、対象セッションに生きた tako-app 配下のクライアントが居ればセカンダリモードへ降格。環境変数の隔離組合せに依存しない）、**縮退保存ガード**（ペイン数が半分未満に減る保存の前に layout.json を世代バックアップ `.bak.1`〜`.bak.3` へ退避。連鎖縮退で健全世代が押し出されないよう 10 分の回転ガード付き）、**一括隔離モード** `TAKO_ISOLATED=1`（discovery / persist / tmux socket をまとめて隔離し、実験起動の片脚隔離を構造的に排除）。persist.log の各行に書き込み元 pid を付与し、事後調査を容易にした

- Added `tako recover` for restoring the layout from generation backups after a mass pane loss (#177): bare `tako recover` lists layout.json and its backups (tabs / panes / age), `tako recover --apply <generation>` restores one (stashing the current file as `layout.json.pre-recover`), refusing while a tako instance is running (`--force` to override for unrelated data dirs). Recovery steps are documented in the README troubleshooting section
  ペイン大量消失後にレイアウトを世代バックアップから戻す `tako recover` を新設（#177）: 引数なしで layout.json とバックアップの一覧（タブ / ペイン数 / 更新時刻）、`tako recover --apply <世代>` で復元（現行は `layout.json.pre-recover` へ退避）。tako 稼働中は拒否する（別データディレクトリの tako なら `--force` で上書き可）。復旧手順は README のトラブルシューティングに記載

- Fixed a data-loss bug where a concurrent `projects add` could wipe the entire orchestrator projects.yaml (58 entries → only the added one) (#169). Root cause was a three-part chain: the old save used `std::fs::write` (truncate → write, exposing an empty/partial file to concurrent readers), serde_yaml successfully parses empty/partial content as "0 projects" instead of erroring, and read-modify-write had no cross-process serialization. All config-file writes (projects.yaml, profiles/*.yaml, config.yaml) now go through a new `config_io` layer: atomic writes (tmp + fsync + rename), an exclusive `<path>.lock` file lock serializing read-modify-write across processes, fail-loud behavior that refuses to overwrite an unparseable file (including the profiles-set path that silently reset corrupt profiles to defaults), and automatic rotated backups (`.bak.1`–`.bak.3`) before every content change
  並行 `projects add` で orchestrator の projects.yaml が全消失する（58 件 → add した 1 件だけになる）データ消失バグを修正（#169）。根本原因は三段連鎖: 旧 save が `std::fs::write`（truncate → write の 2 段階で並行プロセスに空 / 部分ファイルが見える）、serde_yaml が空 / 部分内容をエラーにせず「0 件」として成功パース、read-modify-write のプロセス間直列化なし。設定ファイル（projects.yaml / profiles/*.yaml / config.yaml）の書き込みを新設の `config_io` 層へ集約: アトミック書き込み（tmp + fsync + rename）、`<path>.lock` の排他ロックによるプロセス間 read-modify-write 直列化、パース不能ファイルを絶対に上書きしない fail-loud 化（破損プロファイルを黙って default に戻していた profiles set 経路も修正）、変更のたびの自動世代バックアップ（`.bak.1`〜`.bak.3`）

## [0.4.0] - 2026-07-13

### Added

- Web view panes are now real native browsers (#155): the CDP-mirror proof of concept (headless Chrome + screenshot polling + click relay) has been replaced with wry's `build_as_child` integration — macOS WKWebView rendered as a true child view of the GPUI window. Clicking, scrolling, typing, and IME input are delivered natively by the OS with zero relay latency. Pages live independently of panes: the pane titlebar gains back / forward / reload buttons plus a minimize button that parks the page in a new web dock (status-bar 🌐 button) with its SPA state, login, and scroll position intact, and a close button that destroys it. Open pages persist in layout.json and reopen by URL after a restart. Everything is exposed 1:1 for AI/CLI via `Request::Web`, `tako web open|list|show|hide|close|nav|eval|eval-result|read`, and the MCP tool `tako_web` (9 actions; in-page interaction uses two-phase JS evaluation: `eval` → token → `eval_result`). Port-detection chips now open their preview in a web view pane next to the detected pane (falling back to the external browser). Replaces `tako_chrome_open` / `tako chrome`
  Web ビューペインが本物のネイティブブラウザになった（#155）: CDP ミラー方式の PoC（ヘッドレス Chrome + スクショポーリング + クリック中継）を wry の `build_as_child` 統合へ全面置換 — macOS の WKWebView を GPUI ウィンドウの真の子ビューとして表示する。クリック・スクロール・文字入力・IME は OS がネイティブ配送し、中継遅延ゼロ。ページはペインから独立して生存: タイトルバーに 戻る / 進む / 再読み込み ボタンと、ページを Web dock（ステータスバーの 🌐 ボタン）へ SPA 状態・ログイン・スクロール位置ごと退避する最小化ボタン、破棄する × を追加。開いたページは layout.json に永続化され、再起動後に URL で開き直される。全操作を `Request::Web` / `tako web open|list|show|hide|close|nav|eval|eval-result|read` / MCP ツール `tako_web`（9 action。ページ内操作は eval → token → eval_result の 2 段階 JS 評価）で AI / CLI に 1:1 公開。ポート検知チップの承諾は検知元ペインの隣に Web ビューペインを開くようになった（開けない場合は外部ブラウザへフォールバック）。`tako_chrome_open` / `tako chrome` は置き換えで廃止

- Editable code previews (#126): text/code files can now enter an in-place edit mode with UTF-8-safe typing, deletion, newlines, cursor movement, selection replacement, paste, dirty indication, and Cmd+S saving. Save refuses read-only files and detects external changes made after editing began instead of overwriting them. The same workflow is available through `tako edit start|status|apply|save|stop` and MCP (`tako_preview_edit`, `tako_preview_apply`, `tako_preview_save`); `tako list` exposes `preview.editing` / `preview.dirty`. Non-text and truncated previews remain read-only for safety
  コードプレビューのその場編集を追加（#126）: テキスト／コードファイルで編集モードへ切り替え、UTF-8 安全な文字入力・削除・改行・カーソル移動・選択置換・貼り付け・dirty 表示・⌘S 保存が可能になった。読み取り専用ファイルは拒否し、編集開始後に外部変更された場合も上書きせず競合を通知する。同じ一連の操作を `tako edit start|status|apply|save|stop` と MCP（`tako_preview_edit` / `tako_preview_apply` / `tako_preview_save`）へ公開し、`tako list` の `preview.editing` / `preview.dirty` で状態を取得できる。非テキストと末尾省略プレビューは安全のため読み取り専用のまま

- New `tako solo [-profile]` command for a 1:1 conversation mode without orchestration (#111): launches claude in a new tab with a solo-specific system prompt that **forbids orchestration** (`tako_orchestrator_spawn` / sub-agents / the Workflow tool) — the solo session does the work directly (read, edit, test, commit) instead of delegating to workers. Designed for economical use on plans like Claude Pro: default `effort=high` (below master's `max`), and recent activity is not preloaded at startup (checked via `git log` on demand). Shares the master `projects.yaml` and `build_master_claude_cmd`, so you can talk in terms of project names ("fix the README in demo") without `cd`. Uses the same profile-argument pattern as master (`-<name>` = profile, bare word = backward-compatible suffix); role and `TAKO_ORCHESTRATOR_ROLE` are `solo` / `solo:<suffix>`, distinct from master's `orchestrator-master`. Solo profiles live in `solo-profiles/`
  オーケストレーション無しの 1 対 1 対話モード `tako solo [-profile]` を新設（#111）: solo 専用の system prompt を付けて新タブで claude を起動する。プロンプトで**オーケストレーションを禁止**し（`tako_orchestrator_spawn` / sub-agent / Workflow ツール）、worker へ委任せず solo セッション自身がファイル編集・テスト・コミットを直接行う。Claude Pro 等のプランでの省トークン運用を想定し、既定 `effort=high`（master の `max` より低い）、「最近やってること」は起動時にロードせず必要時に `git log` で参照する。master と `projects.yaml` / `build_master_claude_cmd` を共有するため、`cd` せずプロジェクト名で（「demo の README 直して」）話せる。プロファイル引数は master と同一パターン（`-<名前>` = プロファイル、裸の語 = 後方互換サフィックス）。role と `TAKO_ORCHESTRATOR_ROLE` は `solo` / `solo:<suffix>`（master の `orchestrator-master` と区別）。solo プロファイルは `solo-profiles/` に置く

- Orchestrator workers can now run on codex and agy in addition to claude (#120): profiles gain `worker_agent` plus per-agent `worker_agents` settings (model, effort mapping, skip_permissions, extra args), and spawn / run / profiles expose the agent choice 1:1 via MCP (`agent` parameter) and CLI (`--agent`, `--worker-agent`, `--agent-*`). TUI handling (input-line, trust-dialog and busy detection) was extended to the union of all three agents based on captured real screens, busy/idle is screen-estimated for agents without OSC signals, and agy's always-on "(Thinking)" footer no longer reads as forever-busy
  オーケストレーションの worker が claude に加えて codex / agy で起動できるようになった（#120）: プロファイルに `worker_agent` とエージェント別 `worker_agents` 設定（model・effort 写像・skip_permissions・追加引数）を追加し、spawn / run / profiles の agent 指定を MCP（`agent` パラメータ）と CLI（`--agent` / `--worker-agent` / `--agent-*`）へ 1:1 公開。TUI 対応（入力欄・信頼ダイアログ・busy 検出）を実採取画面に基づく 3 種の和集合へ拡張し、OSC シグナルの無いエージェントは画面推定で busy/idle を判定する。agy の常時フッター「(Thinking)」が永遠 busy と誤判定される問題も修正済み

- The orchestrator master itself can now be codex (#127): profiles gain `master_agent` (claude / codex), honored by both `tako master` and `tako solo`. For codex, the system prompt is injected via developer instructions and the tako MCP server is wired in with temporary `-c mcp_servers.tako.*` config (TAKO_* env passthrough). A guard keeps a non-claude master's model / effort from propagating to claude workers, and agy as master is rejected with an explicit error. CLI `--master-agent` / MCP `master_agent` expose the setting 1:1
  オーケストレーションの master 自体を codex にできるようになった（#127）: プロファイルに `master_agent`（claude / codex）を追加し、`tako master` / `tako solo` の両方が対応。codex は developer instructions で system prompt を注入し、tako MCP サーバーは `-c mcp_servers.tako.*` の一時設定（TAKO_* 環境変数の引き継ぎ）で配線する。master≠claude のとき model / effort を claude worker へ継承しない波及ガード付き。agy の master 指定は明示エラー。CLI `--master-agent` / MCP `master_agent` で 1:1 公開

- PDF previews now support text selection and clipboard copy (#124): a PDFKit-extracted text layer feeds the same drag-selection / ⌘C / highlight path used by code and Markdown previews. PDFs without a text layer degrade gracefully to view-only
  PDF プレビューでテキスト選択とクリップボードコピーが可能になった（#124）: PDFKit で抽出したテキストレイヤを Code / Markdown プレビューと同じドラッグ選択・⌘C・ハイライト描画パスへ統合。テキストレイヤの無い PDF は従来どおり表示のみ

- Terminal text now supports cmd+click links (#146, #147, #153): URLs (including ones wrapped across lines) open in the default browser, file paths open a preview pane split to the right, and directories split-and-cd. Path resolution tries cwd-relative / ~-expanded / absolute candidates with an existence check and strips `:line:col` suffixes; while cmd is held, link text is underlined and highlighted. #153 fixed five root causes that made path links unreliable (link hover hitting the wrong pane, an empty pane on directory click, unknown cwd in TUI panes without OSC 7, detection skipped entirely when cwd was unknown, and an infinite loop in link scanning)
  ターミナル文字列の cmd+クリックリンクに対応（#146, #147, #153）: URL（行折り返しをまたぐものも連結検出）はデフォルトブラウザで開き、ファイルパスは右分割のプレビューで開き、ディレクトリは右分割 + cd する。パス解決は cwd 相対 / ~ 展開 / 絶対パスの 3 戦略 + 実在チェックで、`:行:列` サフィックスも除去。cmd 押下中はリンク文字列だけに下線 + ハイライトを表示する。#153 でパスリンクを不安定にしていた根本原因 5 件（ホバーの別ペイン誤ヒット・ディレクトリクリック時の空ペイン・OSC 7 なし TUI での cwd 不明・cwd 不明時の検出スキップ・リンク走査の無限ループ）を修正

- AI can pin project folders into the file tree (#134): `tako tree add/remove/list` + MCP `tako_tree_folder` (57 tools) manage per-tab pinned folders that persist in layout.json and merge with the cwd-derived workspace roots
  ファイルツリーへの AI からのフォルダ追加・削除（#134）: `tako tree add/remove/list` + MCP `tako_tree_folder`（計 57 ツール）で、タブ単位のピン留めフォルダを管理する。layout.json に永続化され、cwd 由来のワークスペースルートと合流表示される

- Common agent rules can be synced from one source of truth (#136): `tako agents sync-rules` / `tako agents status` + MCP `tako_agents_sync_rules` (58 tools) embed a source file into each agent's global instruction file (claude / codex / agy) between marker blocks — everything outside the block stays untouched, with automatic backups. Also available as a new `tako setup` item, with sync status shown in `tako setup --check`
  エージェント共通ルールの同期機能（#136）: `tako agents sync-rules` / `tako agents status` + MCP `tako_agents_sync_rules`（計 58 ツール）が、正本ファイルの内容を各エージェント（claude / codex / agy）のグローバル指示ファイルへマーカーブロックで埋め込む。ブロック外は不変・バックアップ付き。`tako setup` の新項目としても提供され、同期状態は `tako setup --check` に表示される

- Full Disk Access guidance (#118): new `tako fda status/open` + MCP `tako_fda` (53 tools) detect whether tako has Full Disk Access and open the exact System Settings pane to grant it; `tako setup --check` includes the same check. This targets macOS TCC folder-access dialogs reappearing on every access
  フルディスクアクセス（FDA）ガイド機能（#118）: `tako fda status/open` + MCP `tako_fda`（計 53 ツール）が FDA の付与状態を検出し、付与用のシステム設定画面を直接開く。`tako setup --check` にも同じチェックを追加。macOS TCC のフォルダアクセス許可ダイアログが毎回出る問題への対策

### Changed

- codex / agy workers now skip approval prompts by default (#132): spawned codex / agy workers run with permissions bypassed unless opted out, and a codex master launches with `--dangerously-bypass-approvals-and-sandbox` — verified against a real codex to be the only mode that also bypasses MCP tool approvals (`-a never` does not). `tako orchestrator profiles set` gains `--worker-model-policy`, and `scripts/clean-target.sh` was added to prune build artifacts
  codex / agy worker の承認を既定でスキップ（#132）: spawn される codex / agy worker は opt-out しない限り承認バイパスで起動し、codex master は `--dangerously-bypass-approvals-and-sandbox` を使う（実 codex での検証により、MCP ツール承認までバイパスするのはこのモードのみ。`-a never` では不十分）。`tako orchestrator profiles set` に `--worker-model-policy` を追加し、target 掃除の `scripts/clean-target.sh` を新設

- Master / solo system prompts now pin project folders proactively (#141): folders for projects mentioned in conversation are added to the file tree before the user has to ask
  master / solo のデフォルト system prompt がプロジェクトフォルダを積極的にピン留めするようになった（#141）: 会話に上がったプロジェクト・関連フォルダを、ユーザーに聞かれる前にファイルツリーへ追加する行動規範を強化

- `tako setup` now walks through Full Disk Access (#143): the setup flow names missing FDA as the cause of repeated TCC folder dialogs, offers to open System Settings on the spot, notes that an app restart is required after granting, and shows a checkmark when already granted. Delivered to existing users as setup changelog rev 6
  `tako setup` の FDA 案内を強化（#143）: TCC ダイアログ頻発の原因が FDA 未付与であることを明示し、その場でシステム設定を開く対話・付与後のアプリ再起動案内・付与済みなら「✓ 済み」表示を追加。setup changelog rev 6 として既存ユーザーにも配信

### Fixed

- Claude Code conversations now resume after a full PC restart (#139): tako periodically associates running Claude session IDs with their tmux-backed panes and stores them in `layout.json`. On restore, an existing backend session is still reattached unchanged; only when that backend disappeared (as happens on reboot) does tako validate the local transcript and run `claude --resume <session-id>` in the recreated pane. Explicitly exited or unidentifiable sessions are not guessed, and the behavior remains controlled by the existing `tako persist` / `tako_persist` setting
  PC 再起動後も Claude Code の会話を復旧（#139）: 実行中 Claude の session ID を tmux backend ペインへ定期的に対応付け、`layout.json` に保存する。復元時、backend session が生存していれば従来どおりそのまま再 attach し、PC 再起動のように backend 自体が消失した場合だけローカル transcript を検証して、再作成したペインで `claude --resume <session-id>` を実行する。明示終了済み・特定不能なセッションを推測で戻すことはなく、既存の `tako persist` / `tako_persist` 設定で制御される

- PDF drag selection is visible again (#152): the PDF text canvas is now pinned to the page image's top-left instead of inheriting a static position below the image, and selection rectangles are composited in a dedicated topmost GPUI layer. Syntax highlighting now preserves line endings required by syntect's parser and uses one path/filename/shebang resolver for both read and edit modes across the bundled standard language set (including C++ and Python), with JavaScript fallback for TypeScript files
  PDF のドラッグ選択ハイライトを再修正（#152）: PDF テキスト canvas を画像直後の static position ではなくページ画像左上へ固定し、選択矩形を GPUI の専用最前面 layer で合成する。シンタックスハイライトは syntect パーサが必要とする行末改行を保持し、読み取り／編集の両モードを同一のパス・特殊ファイル名・shebang 解決器へ統一した。C++／Python を含む同梱標準言語セット全体を対象とし、TypeScript は JavaScript 文法へ安全にフォールバックする

- Preview selection now follows the actual GPUI-shaped text coordinates instead of terminal-cell estimates (#145), including Markdown font sizes, mixed Japanese/ASCII text, tabs, and vertical scrolling. PDF selection uses PDFKit line/character rectangles transformed onto the rendered page, and editable previews keep syntax colors while composing selection/caret highlights. Preview swaps invalidate stale coordinate caches, and self-tests synchronize on real CLI/paint completion instead of fixed delays
  プレビュー選択の座標ずれを修正（#145）: ターミナル固定セル換算をやめ、GPUI が実際に shaping した座標から Markdown の文字サイズ・日本語／半角混在・タブ・縦スクロール後の byte 位置を逆算する。PDF は PDFKit の行／文字矩形を表示ページへ変換して選択し、編集モードでも構文色と選択／キャレットを合成する。ファイル差し替え時は旧座標キャッシュを破棄し、セルフテストは固定待ちではなく実 CLI／paint 完了へ同期する

- Starting a second tako instance no longer destroys panes (#113): the root cause was a three-step chain — the late instance's restore (`new-session -A -D`) hijacked the primary's tmux clients, the resulting cascade of Exited states overwrote layout.json mid-shutdown, and the next startup's orphan cleanup killed live worker sessions that had leaked out of the protected set. A multi-instance guard now starts late instances in a secondary mode (no restore, no layout.json writes, no tmux backend, no socket takeover; `TAKO_FORCE_PRIMARY=1` overrides), startup orphan cleanup skips sessions active within the last hour, and pane-exit / quit handling is idempotent against double-fired exit events. A UI-stall watchdog and dispatch timing now log to perf.log (256KB rotation, event names only), and tmux window capture for hover previews moved off the UI thread
  2 個目の tako 起動でペインが消える問題を根治（#113）: 根因は三段連鎖 — 後発インスタンスの復元（`new-session -A -D`）がプライマリの tmux クライアントを強奪 → Exited 連鎖の途中状態が layout.json を上書き → 次回起動の orphan cleanup が保護から漏れた実行中セッションを kill。多重インスタンスガード（後発はセカンダリモードで起動: 復元しない・layout.json に書かない・tmux バックエンドに乗らない・ソケットを乗っ取らない。`TAKO_FORCE_PRIMARY=1` で上書き可）+ 起動時 cleanup の 1 時間アクティビティ猶予 + 終了イベント二重発火の冪等化で対策。UI ストールウォッチドッグと dispatch 処理時間の perf.log 記録（256KB ローテート・種別名のみ）、ホバープレビュー用 tmux window キャプチャの background 化も同梱

- `tako remote start` no longer fails when a stale daemon holds the port (#129): the port is probed before spawning, a stale tako remote daemon is reclaimed automatically (SIGTERM → poll → SIGKILL), and an unrelated process occupying the port produces a clear error including its PID. `daemon_stop` now polls for actual process exit (up to 5s, then SIGKILL) instead of a fixed 500ms wait, and recovers stale daemons even when the PID file is gone
  stale デーモンのポート占有で `tako remote start` が失敗する問題を修正（#129）: 起動前にポート占有を検知し、stale な tako remote デーモンなら SIGTERM → ポーリング → SIGKILL で自動回収して再起動する。無関係プロセスが占有中なら PID 入りのエラーで案内。`daemon_stop` も固定 500ms 待ちをやめ実際のプロセス終了をポーリングし（最大 5 秒、超過時 SIGKILL）、PID ファイル消失時もポート占有者から stale デーモンを回収する

- Cmd-Q now always quits (#103): Quit was registered only on the root div's `on_action`, making it focus-path dependent — with no focused element (blur, e.g. caused by accessibility tools), both the keybinding and the menu item silently did nothing, and only quitting from the Dock worked. Quit is now a global `cx.on_action` registration, and shutdown work (layout save, discovery cleanup) moved to `cx.on_app_quit` so it also runs on Dock- or OS-initiated quits. The all-panes-exited path keeps its layout delete / keep semantics
  Cmd-Q で終了しないことがある問題を根治（#103）: Quit がルート div の `on_action` のみに登録されフォーカスパス依存だったため、フォーカス無し（blur。a11y ツール等で発生）ではキーバインド・メニューの両経路が無音で不発になり、Dock からの終了だけが効いていた。Quit を `cx.on_action` のグローバル登録へ一本化し、終了処理（layout 保存・discovery cleanup）を `cx.on_app_quit` へ移設（Dock・OS 起因の終了でも保存が走る）。全ペイン終了経路の layout 削除 / 保持の分岐は不変

## [0.3.2] - 2026-07-07

### Fixed

- Multi-master orchestrator spawn no longer sends workers to the wrong tab (#109): when multiple masters run in parallel (`tako master -fable`, `tako master -aram`, etc.) and the caller's `TAKO_PANE_ID` is stale, the spawn fallback now uses `TAKO_ORCHESTRATOR_ROLE` to identify the correct master instead of blindly picking the first one found. The role is propagated through the MCP session (`caller_role` field) from the stdio bridge / HTTP transport to the dispatch layer
  複数 master 並行時に `tako_orchestrator_spawn` の worker が意図しないタブに出る問題を修正（#109）: 呼び出し元の `TAKO_PANE_ID` が stale な場合のフォールバックで、`TAKO_ORCHESTRATOR_ROLE` 環境変数を使って正しい master を特定する。role は MCP セッション（`caller_role` フィールド）を通じて stdio ブリッジ / HTTP トランスポートから dispatch 層まで伝搬する

## [0.3.1] - 2026-07-07

### Security

- `tako remote` is now secure-by-default (#104): the remote server hosts only over an encrypted cloudflared tunnel and **refuses to start** if a tunnel cannot be established (the old silent plaintext-LAN fallback is gone). Plain HTTP LAN mode is available only via the explicit, opt-in `--no-tunnel` replacement `--insecure` (off by default). Token comparison is now constant-time (HTTP Bearer + WebSocket), token/QR state files are written `0o600`, and `remote status` masks the token by default — both the standalone `token` field and the `token=` query embedded in `connect_url` / `fallback_url` — revealing raw values only with `--show-token` (CLI) / `show_token=true` (MCP). The public relay worker gained per-source-IP rate limiting (register 60/min, resolve 240/min). README / docs document that remote access is a legitimate remote-control tool granting arbitrary command execution and is not end-to-end encrypted
  `tako remote` を secure-by-default 化（#104）: リモートサーバーは暗号化された cloudflared トンネル経由でのみホストし、トンネルを張れない場合は**起動を拒否**する（従来の無音の平文 LAN フォールバックを廃止）。平文 HTTP の LAN モードは明示 opt-in の `--insecure`（既定 off）でのみ有効。トークン比較を定数時間化（HTTP Bearer + WebSocket）、token / QR の state ファイルを `0o600` で書き出し、`remote status` は既定でトークンをマスクする — 単体の `token` フィールドに加え `connect_url` / `fallback_url` に埋め込まれた `token=` クエリも伏せ、生値は `--show-token`（CLI）/ `show_token=true`（MCP）でのみ表示。公共リレー worker に送信元 IP 単位のレートリミット（register 60/分・resolve 240/分）を追加。README / docs に、リモートアクセスが任意コマンド実行を許す正規の遠隔操作ツールであり E2E 暗号化ではない旨を明記

### Added

- New CLAUDE.md section template `06-completion-verification` distributed by `tako setup` (#100): defines a completion-verification quality gate — build / lint / tests green, exercise the change end-to-end ("a passing build is not evidence it works"), probe edge cases, re-read the full diff — and an evidence-based report format with an explicit "not verified" section. Registered as setup changelog rev 5 (guided), so existing users are offered the addition interactively on their next `tako setup` without overwriting customizations
  `tako setup` が配布する CLAUDE.md セクションテンプレートに `06-completion-verification`（完了検証）を新設（#100）: 完了報告前の品質ゲート（ビルド・リント・テスト緑 / 変更を実際に動かして観察 =「ビルドが通った ≠ 動く」/ エッジケース確認 / diff の読み直し）と、証拠つき + 「未検証」明示の報告様式を定義。setup changelog の rev 5（guided）として登録し、既存ユーザーは次回の `tako setup` で対話的に追記を提案される（カスタマイズは上書きしない）

### Changed

- Orchestration quality pipeline standardized in the default master system prompt (#100): new `task-intake` block (enumerate the requests in each user message, assign one worker per deliverable with a closed list of merge exceptions, decide parallel vs sequential, post the plan and spawn in the same turn), new `worker-prompt-template` block (a mandatory fill-in template — Task / Background / Scope / Constraints / acceptance criteria / verification steps / git flow / evidence-based report format — with root-cause-first and requirement-bound rules), and new `acceptance` block (inspect worker reports against evidence and diff spot-checks before reporting to the user; send back with a concrete defect list, rethink after 2 failed rounds). Existing block names are unchanged, so `prompt_blocks` customizations keep working. Monitoring / lifecycle blocks absorbed field lessons (idle notifications can misfire — confirm via `tako_read_pane`; never respawn a merely-thinking worker; commit per milestone on long tasks)
  master 用デフォルトシステムプロンプトにオーケストレーション品質パイプラインを標準化（#100）: `task-intake` ブロック新設（依頼を列挙し 1 worker = 1 成果物で割り当て・統合の例外は閉じたリスト・並列/直列判定・分担計画の提示と同ターン spawn）、`worker-prompt-template` ブロック新設（Task / Background / Scope / Constraints / 受け入れ条件 / 検証手順 / Git / 証拠つき報告様式を必須とする穴埋め式の型。根因先行・要件密着タスクの転記ルール込み）、`acceptance` ブロック新設（worker の完了報告を証拠と diff スポットチェックで検査してからユーザーに報告。差し戻しは具体的な欠陥リストで行い、2 回失敗したら方針を再考）。既存ブロック名は不変のため `prompt_blocks` によるカスタマイズはそのまま動く。monitoring / lifecycle ブロックにも運用知見を反映（idle 通知の空振りは `tako_read_pane` で確認・thinking 中の worker を respawn しない・長尺タスクはマイルストーンごとにコミット）

### Fixed

- Enter no longer goes missing in claude TUI worker panes (#95): three delivery paths are hardened. (1) A human Enter pressed on a claude TUI pane is now verified — tako snapshots the input line (`❯ …`) before writing `\r`, and if the same text is still sitting there afterwards (claude occasionally drops Enter while busy), the Enter is automatically re-sent (up to 4 times). (2) `tako_send_input` with empty `text` + `newline: true` becomes a proper "Enter only" delivery: it no longer waits out a pointless 10-second reflection timeout, and its verification actually checks that the input line emptied (previously the empty-prompt check always passed, so it never retried — one silently dropped CR meant permanent stuck text). (3) LF characters written directly to a pane (`text: "\n"`, etc.) are normalized to CR — claude TUI interprets LF as "insert newline", never "submit", so raw-LF sends could clear-looking-but-unsent input. The same Enter-only delivery (send → verify emptied → resend) also applies to the tmux fallback path
  claude TUI の worker ペインで Enter が空振りする問題を修正（#95）: 送達 3 経路を強化。(1) claude TUI ペインへの人間の Enter を検証つきに — `\r` 書き込み前に入力欄（`❯ …`）の内容を控え、書き込み後も同じテキストが残っていれば（busy 中の claude は Enter を取りこぼすことがある）Enter を自動再送する（最大 4 回）。(2) `tako_send_input` の空 `text` + `newline: true` を正式な「Enter 単独送達」に — 無意味な 10 秒の反映待ちタイムアウトを廃止し、検証も「入力欄が空へ戻ったか」を実際に確認する（従来は空プロンプトの検証が常に成功扱いで再送ゼロのため、CR 1 発の取りこぼし = 恒久残留だった）。(3) ペインへ直接書く経路の LF を CR へ正規化 — claude TUI は LF を「改行挿入」と解釈し決して送信しないため、生 LF 送信は「消えたように見えて未送信」になっていた。tmux フォールバック経路にも同じ Enter 単独送達（送信 → 空検証 → 再送）を適用

## [0.3.0] - 2026-07-06

### Added

- `tako setup` now starts with a dependency check stage (#88): claude (required) and tmux / cloudflared / git (optional) are detected with a one-line purpose note each, and missing tools can be installed on the spot via Homebrew (per-tool y/N prompt). cloudflared joined the list following #89 (tunnel-less silent LAN fallback). The same list is shown by `tako setup --check`, and the docs dependency table is kept in sync
  `tako setup` の冒頭に依存ツールチェック段階を追加（#88）: claude（必須）と tmux / cloudflared / git（任意）を用途の一言説明付きで検出し、不足分は Homebrew でその場インストールできる（ツールごとに y/N 確認）。cloudflared は #89（トンネル不成立時の無音 LAN フォールバック）を受けて対象化。同じ一覧は `tako setup --check` にも表示され、docs の依存表も同期
- `tako setup` now tracks setup-related changes across updates (#94): the binary embeds a machine-readable setup changelog (`resources/setup/changes.yaml`, revision-numbered), and the revision applied at the last setup is recorded in `config.yaml` (`setup.applied_revision`). Re-running `tako setup` after an update lists what changed since, writes a `pending-changes.md` brief into the setup directory, and the setup agent follows up in conversation — `auto` entries (new checks, template updates) are applied by the re-run itself and only announced, while `guided` entries (anything touching user-owned files such as a custom `master-system.md`) are confirmed interactively and never overwrite customizations silently. Inspect anytime with `tako setup --changes [--json]` (CLI) or `tako_setup_changes` (MCP, 52 tools total); `tako setup --check` also reports the follow-up status
  `tako setup` にアップデート追従機能を追加（#94）: バイナリに機械可読の setup changelog（`resources/setup/changes.yaml`、リビジョン番号付き）を同梱し、最後に setup したときの適用リビジョンを `config.yaml`（`setup.applied_revision`）に記録。アップデート後に `tako setup` を再実行すると前回以降の変更が一覧表示され、setup ディレクトリに書き出される `pending-changes.md` をもとに setup エージェントが対話で追従する。`auto` の変更（チェック項目追加・テンプレート更新等）は再実行自体が適用を兼ねて通知のみ、`guided` の変更（カスタム `master-system.md` などユーザー所有ファイルに関わるもの）は対話で確認し、カスタマイズを黙って上書きしない。`tako setup --changes [--json]`（CLI）/ `tako_setup_changes`（MCP、計 52 ツール）でいつでも確認でき、`tako setup --check` にも追従状況が表示される

### Security

- `FileOp::Trash` (macOS) now passes the path to `osascript` as an argument instead of concatenating it into the AppleScript source (#80): the Finder delete script uses `on run argv` and reads the path from `item 1 of argv`, so filenames containing `"`, `\`, or newlines can no longer break out of the string literal and inject AppleScript. This removes the reliance on escape correctness (and the prior control-character reject guard is no longer needed). A deterministic test proves an injection payload passed via argv is treated as data (no side effect), and an `#[ignore]` e2e trashes a file whose name contains quotes/backslash/newline
  `FileOp::Trash`（macOS）がパスを AppleScript ソースへ文字列連結せず `osascript` の引数として渡すよう変更（#80）: Finder の削除スクリプトを `on run argv` 化し `item 1 of argv` からパスを読むことで、`"` `\` 改行を含むファイル名が文字列リテラルを抜け出して AppleScript を注入する余地を構造的に排除。エスケープの正しさへの依存（および従来の制御文字拒否ガード）が不要になった。argv 経由のインジェクション payload がデータとして扱われる（副作用なし）ことを決定的テストで実証し、引用符・バックスラッシュ・改行を含むファイル名の削除を `#[ignore]` の e2e で用意
- Relay registration is now protected by a per-machine secret (#78): `tako remote start` auto-generates `<data_dir>/relay_secret` (hex 64, mode 0600) and sends it to `/api/register`; the relay worker stores only its SHA-256 hash and rejects overwrites with a mismatched secret (first-write-wins — legacy secret-less registration is still accepted for unclaimed machine IDs, so old clients keep working). The default relay is now documented as a best-effort public instance that stores only machineId → tunnel URL (no terminal content, no tokens) and can be replaced via `TAKO_RELAY_URL`; self-hosting steps live in `web/tako-remote-worker/README.md`, and the worker gained an offline test suite (`npm test`). Deployed to the production relay on 2026-07-06 (version `5acac8f5`); overwrite protection was verified live against the running instance (mismatched-secret and secret-less overwrites both return 403, resolve keeps the original tunnel URL)
  リレー登録を端末ごとのシークレットで保護（#78）: `tako remote start` が `<data_dir>/relay_secret`（hex 64・0600）を自動生成して `/api/register` に送り、リレー worker は SHA-256 ハッシュのみ保存して secret 不一致の上書きを 403 で拒否（first-write-wins。secret 無しの旧クライアントは未保護 ID に限り従来どおり登録可能で互換維持）。デフォルトリレーは「machineId → tunnel URL のみを保存するベストエフォート公共インスタンス（画面内容・トークンは通らない）」として文書化し、`TAKO_RELAY_URL` で自前リレーへ差し替え可能に。セルフホスト手順は `web/tako-remote-worker/README.md`、worker にオフラインテスト（`npm test`）を追加。2026-07-06 に本番リレーへデプロイ済み（version `5acac8f5`）。稼働中インスタンスに対して上書き保護を実地検証（別 secret・secret 無しの上書きはいずれも 403、resolve は元の tunnel URL を維持）

### Changed

- Remote connection entry point unified to the fixed Pages URL (#91): with the tunnel up and relay registration succeeded, the connect link / QR now always points to `https://tako-remote.pages.dev/#/connect?machine=<id>&...` — the PWA is served by Cloudflare Pages and resolves the machine's current tunnel URL via the KV relay, so bookmarks survive tunnel restarts and the random trycloudflare URL is never shown. The tunnel-direct URL is still printed as a spare link (relay-outage backup), and the daemon-embedded PWA remains the LAN-only fallback. `tako remote status` reconstructs the same link (tunnel state is persisted to `<state_dir>/tako-remote.tunnel`), `tako remote start` now warns visibly when the tunnel could not be established and the URL is LAN-only (#89 visibility), the PWA skips the pointless self-health probe when served from pages.dev and records the daemon version from `/api/health` for compatibility warnings, and `scripts/release.sh --publish` deploys the PWA to Pages via the new `scripts/deploy-pages.sh`
  リモート接続の入口を Pages 固定 URL に一本化（#91）: トンネル確立 + リレー登録成功時の接続リンク / QR は常に `https://tako-remote.pages.dev/#/connect?machine=<id>&...` を指すようになった。PWA は Cloudflare Pages が配信し、KV リレーで各マシンの現在のトンネル URL を解決するため、トンネルが再起動してもブックマークは不変で、trycloudflare のランダム URL はユーザーに見えない。トンネル直 URL は予備リンクとして併記（リレー障害時の受け皿）し、LAN-only 用にデーモン内蔵 PWA も維持。`tako remote status` も同じリンクを再構成（トンネル状態を `<state_dir>/tako-remote.tunnel` に永続化）、トンネルが張れず LAN 限定 URL になった場合は `tako remote start` が明示的に警告（#89 の可視化）、pages.dev 配信時の PWA は自分への無駄な health 試行をスキップし、`/api/health` のデーモンバージョンを互換警告用に記録。`scripts/release.sh --publish` は新設の `scripts/deploy-pages.sh` で PWA を Pages へデプロイする
- License declarations unified to GPL-3.0-or-later across all manifests (#75): added the `license` field to the three `poc/` crates and the three `docs/` / `web/` package.json files. The license itself is unchanged — the repository has declared GPL-3.0-or-later throughout (LICENSE / Cargo.toml / README); this completes manifest-level consistency for the public release
  ライセンス宣言を全マニフェストで GPL-3.0-or-later に統一（#75）: `poc/` クレート 3 つと `docs/` / `web/` の package.json 3 つに license フィールドを追加。ライセンス自体は変更なし（LICENSE / Cargo.toml / README は従来から GPL-3.0-or-later を宣言）。公開に向けたマニフェスト単位の一貫性を仕上げた
- Orchestrator completion-wait polling unified into tako-control (#83): the polling state machine duplicated across MCP `tako_orchestrator_run` and CLI `tako orchestrator run` / `watch` (~300 lines) is now a single engine (`orchestrator::wait`). The tmux-liveness guard against false "gone" during tako restarts — previously CLI-only — now also applies to the MCP path, so `tako_orchestrator_run` no longer misreports `error` while tako restarts
  オーケストレーターの完了待ちポーリングを tako-control へ一本化（#83）: MCP `tako_orchestrator_run` と CLI `tako orchestrator run` / `watch` に重複していたポーリング状態機械（約 300 行）を単一エンジン（`orchestrator::wait`）に統合。CLI のみにあった tmux 生存確認による gone 誤検知防止（tako 再起動中の対策）が MCP 経路にも効くようになり、再起動中の `tako_orchestrator_run` が `error` を誤報告しなくなった

### Fixed

- `tako_orchestrator_run` / `tako orchestrator run` no longer return an empty `output` (#82): the result-read step referenced a nonexistent `content` field of the Read response (actual field: `text`), so the worker's final output was always empty — with `auto_close` defaulting to true, the pane was closed before the master could re-read it. A regression test now asserts the output round-trip
  `tako_orchestrator_run` / `tako orchestrator run` の `output` が常に空になる問題を修正（#82): 出力取得ステップが Read 応答に存在しない `content` フィールド（実際は `text`）を参照していたため worker の成果が常に空だった。`auto_close` 既定 true のため master が読み直す前にペインも閉じられていた。出力の往復を検証する回帰テストを追加

## [0.2.8] - 2026-07-05

### Changed

- Remote UI redesign v3 — PC-safe read-only WebSocket + continuous-scroll reader view (#63): WebSocket auto-resize of cols/rows is completely removed — `/ws?pane=<id>` is now read-only and never affects the PC pane size. Protocol changed to push `init` (history + current screen with ANSI, cursor) on connect, then `update` diffs every 250ms. xterm.js is replaced with a self-contained reader view: one continuous scroll with bottom-following (scroll up to browse history, scroll to bottom to re-follow), line-wrapping for mobile readability, and a custom ANSI SGR parser (`web/tako-remote/src/ansi.js`) — zero added dependencies. Font size A−/A+, pane switching via swipe + header ‹ ›
  リモート UI を再設計 v3 — PC 非破壊の読み取り専用 WebSocket + 連続スクロールリーダービュー（#63）: WS の cols/rows 自動リサイズを全廃し、`/ws?pane=<id>` は読み取り専用で PC のペインサイズに一切影響しなくなった。プロトコルを接続時 `init`（履歴 + 現画面、ANSI 付き + カーソル）→ 250ms 差分 `update` のプッシュ方式に刷新。xterm.js を廃止し、折り返しリーダービュー（1 本の連続スクロール、下端追従・上スクロールで過去閲覧・下端復帰で追従再開）+ 自前 ANSI SGR パーサ（`web/tako-remote/src/ansi.js`）で再実装。依存追加ゼロ。フォント A−/A+、スワイプ + ヘッダー ‹ › でペイン切替

### Fixed

- Half-width characters no longer vanish sporadically in mixed Japanese/ASCII lines (#64): grouped half-width runs (#39) rendered text inside a grid-width div, and GPUI treated that width as a wrap width — a hairline (f32 ULP) overshoot of the shaped width made GPUI wrap the tail word/character onto an invisible second line inside the `overflow_hidden` row (e.g. "ターミナルUI" → "ターミナルU", "Fable 5 + max" → "Fable 5 + "). Rows now set `whitespace_nowrap` (structurally disables wrapping), and glyphs whose advance differs from the cell width (fallback-font symbols like ⏺ ⎿) are excluded from grouping into their own cell-width div so misalignment cannot accumulate. The #39 hang fix (element count reduction) is preserved: ASCII runs stay grouped
  日本語混在行で半角文字が確率的に消える問題を根治（#64）: 半角グループ化描画（#39）はグリッド幅の div 内にテキストを置くが、GPUI はその幅を折り返し幅として扱うため、シェイプ幅がヘアライン（f32 ULP）でも超えると末尾の単語/文字が折り返されて行 div の `overflow_hidden` 外へ消えていた（例:「ターミナルUI」→「ターミナルU」、「Fable 5 + max」→「Fable 5 + 」）。行 div に `whitespace_nowrap` を指定して折り返しを構造的に禁止し、advance がセル幅と一致しないグリフ（⏺ ⎿ 等のフォールバックフォント記号）はグループから除外してセル幅固定の個別 div に隔離、ずれの累積も遮断。#39 のハング解消効果（描画要素数削減）は維持（ASCII 連続はグループ化のまま）
- `migrate_legacy_default_profile` no longer strips user-configured model on every master launch (#67): when the backup file (`default.yaml.backup-1m`) already exists, the migration is considered done and skipped. Previously, each `tako master` / `tako setup` / spawn run re-triggered the migration, removing any model that had been set via `tako orchestrator profiles set --model`
  `migrate_legacy_default_profile` が master 起動のたびにユーザー設定の model を消す問題を修正（#67）: backup ファイル（`default.yaml.backup-1m`）が存在する場合はマイグレーション済みと判断してスキップするようにした。従来は `tako master` / `tako setup` / spawn の実行ごとにマイグレーションが再発火し、`tako orchestrator profiles set --model` で設定した model が消えていた
- Update checker no longer misreports GitHub API rate limits as "no update available" (#59): switched from GitHub API to web redirect-based version detection (not subject to API rate limits), introduced `CheckError` type to distinguish errors from genuine "no update" state, added silent retry on failure (waits until rate-limit reset for 403, 1 hour for others), and surfaced error details in CLI/MCP JSON and status bar
  更新チェッカーが GitHub API レート制限を「更新なし」と誤報告する問題を修正（#59）: GitHub API から Web リダイレクト方式（API レート制限の対象外）に移行し、`CheckError` 型でエラーと「更新なし」を区別、自動チェック失敗時の静かなリトライ（レート制限は reset 時刻まで、他は 1 時間後）を追加、CLI/MCP の JSON とステータスバーにエラー詳細を表示

## [0.2.7] - 2026-07-03

### Fixed

- Release build now includes PWA rebuild (#60): `build-app.sh` runs `npm ci && npm run build` for `web/tako-remote` before `cargo build`, ensuring `rust_embed` always bundles the latest PWA dist. `release.sh` verifies that the bundled JS contains source-derived markers (e.g. history UI strings) to prevent stale dist from shipping again. Without npm, a warning is shown if an existing dist is available; otherwise the build errors
  リリースビルドに PWA ビルド工程を組み込み（#60）: `build-app.sh` が `cargo build` の前に `web/tako-remote` の `npm ci && npm run build` を実行し、`rust_embed` に常に最新の PWA dist を埋め込む。`release.sh` は同梱 JS にソース由来マーカー（履歴 UI 文字列等）が含まれることを機械検証し、stale な dist の再発を防止。npm 不在時は既存 dist があれば警告スキップ、なければエラー終了

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
