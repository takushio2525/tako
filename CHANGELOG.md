# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Worker spawn layout engine (#165): spawning workers no longer squeezes every pane into ever-thinner columns. With the new default `master-reserved` policy, the spawning pane (master) keeps its share of the screen (default 50%, configurable 0.1–0.9) and workers tile inside a dedicated worker area on its right: `grid` (1 worker = full area → 2 = stacked → 3–4 = quadrant cross → more columns as needed, default) or `spiral` (alternating half-splits, golden-ratio style). The worker area is recognized via each pane's `spawned_by` chain, so panes the user opened manually are never rearranged — when a worker closes (MCP/CLI close, UI ×, or process exit), only the worker area reflows and master/user panes keep their exact rectangles. Configure via config.yaml `spawn_layout`, `tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]`, or the new MCP tool `tako_orchestrator_layout` (59 tools total); `legacy` restores the old right-split behavior. Master/solo system prompts now instruct agents to prioritize the readability of the master pane and user-opened panes when rearranging layouts
  worker spawn のレイアウトエンジンを新設（#165）: spawn のたびに全ペインが横へ等分圧縮される問題を解消。新既定の `master-reserved` ポリシーでは spawn 元（master）が画面の取り分（既定 50%、0.1〜0.9 で設定可）を維持し、worker は右側の worker 領域内に配置される: `grid`（1 体=全面 → 2 体=上下 → 3〜4 体=十字四分割 → 以降は列を追加。既定）/ `spiral`（縦横交互の半分割、黄金比風）。worker 領域は各ペインの `spawned_by` チェーンで認識するため、ユーザーが手動で開いたペインが再配置されることはない — worker の close（MCP/CLI・UI の ×・プロセス exit）時も領域内だけがリフローされ、master とユーザー由来ペインの矩形は不変。設定は config.yaml の `spawn_layout`、`tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]`、新 MCP ツール `tako_orchestrator_layout`（計 59 ツール）から。`legacy` で従来の右等分割へ戻せる。master / solo の system prompt に「レイアウト操作時は master とユーザー由来ペインの可読性を最優先する」行動規範を追記

- Web view panes are now real native browsers (#155): the CDP-mirror proof of concept (headless Chrome + screenshot polling + click relay) has been replaced with wry's `build_as_child` integration — macOS WKWebView rendered as a true child view of the GPUI window. Clicking, scrolling, typing, and IME input are delivered natively by the OS with zero relay latency. Pages live independently of panes: the pane titlebar gains back / forward / reload buttons plus a minimize button that parks the page in a new web dock (status-bar 🌐 button) with its SPA state, login, and scroll position intact, and a close button that destroys it. Open pages persist in layout.json and reopen by URL after a restart. Everything is exposed 1:1 for AI/CLI via `Request::Web`, `tako web open|list|show|hide|close|nav|eval|eval-result|read`, and the MCP tool `tako_web` (9 actions; in-page interaction uses two-phase JS evaluation: `eval` → token → `eval_result`). Port-detection chips now open their preview in a web view pane next to the detected pane (falling back to the external browser). Replaces `tako_chrome_open` / `tako chrome`
  Web ビューペインが本物のネイティブブラウザになった（#155）: CDP ミラー方式の PoC（ヘッドレス Chrome + スクショポーリング + クリック中継）を wry の `build_as_child` 統合へ全面置換 — macOS の WKWebView を GPUI ウィンドウの真の子ビューとして表示する。クリック・スクロール・文字入力・IME は OS がネイティブ配送し、中継遅延ゼロ。ページはペインから独立して生存: タイトルバーに 戻る / 進む / 再読み込み ボタンと、ページを Web dock（ステータスバーの 🌐 ボタン）へ SPA 状態・ログイン・スクロール位置ごと退避する最小化ボタン、破棄する × を追加。開いたページは layout.json に永続化され、再起動後に URL で開き直される。全操作を `Request::Web` / `tako web open|list|show|hide|close|nav|eval|eval-result|read` / MCP ツール `tako_web`（9 action。ページ内操作は eval → token → eval_result の 2 段階 JS 評価）で AI / CLI に 1:1 公開。ポート検知チップの承諾は検知元ペインの隣に Web ビューペインを開くようになった（開けない場合は外部ブラウザへフォールバック）。`tako_chrome_open` / `tako chrome` は置き換えで廃止

- Editable code previews (#126): text/code files can now enter an in-place edit mode with UTF-8-safe typing, deletion, newlines, cursor movement, selection replacement, paste, dirty indication, and Cmd+S saving. Save refuses read-only files and detects external changes made after editing began instead of overwriting them. The same workflow is available through `tako edit start|status|apply|save|stop` and MCP (`tako_preview_edit`, `tako_preview_apply`, `tako_preview_save`); `tako list` exposes `preview.editing` / `preview.dirty`. Non-text and truncated previews remain read-only for safety
  コードプレビューのその場編集を追加（#126）: テキスト／コードファイルで編集モードへ切り替え、UTF-8 安全な文字入力・削除・改行・カーソル移動・選択置換・貼り付け・dirty 表示・⌘S 保存が可能になった。読み取り専用ファイルは拒否し、編集開始後に外部変更された場合も上書きせず競合を通知する。同じ一連の操作を `tako edit start|status|apply|save|stop` と MCP（`tako_preview_edit` / `tako_preview_apply` / `tako_preview_save`）へ公開し、`tako list` の `preview.editing` / `preview.dirty` で状態を取得できる。非テキストと末尾省略プレビューは安全のため読み取り専用のまま

- New `tako solo [-profile]` command for a 1:1 conversation mode without orchestration (#111): launches claude in a new tab with a solo-specific system prompt that **forbids orchestration** (`tako_orchestrator_spawn` / sub-agents / the Workflow tool) — the solo session does the work directly (read, edit, test, commit) instead of delegating to workers. Designed for economical use on plans like Claude Pro: default `effort=high` (below master's `max`), and recent activity is not preloaded at startup (checked via `git log` on demand). Shares the master `projects.yaml` and `build_master_claude_cmd`, so you can talk in terms of project names ("fix the README in demo") without `cd`. Uses the same profile-argument pattern as master (`-<name>` = profile, bare word = backward-compatible suffix); role and `TAKO_ORCHESTRATOR_ROLE` are `solo` / `solo:<suffix>`, distinct from master's `orchestrator-master`. Solo profiles live in `solo-profiles/`
  オーケストレーション無しの 1 対 1 対話モード `tako solo [-profile]` を新設（#111）: solo 専用の system prompt を付けて新タブで claude を起動する。プロンプトで**オーケストレーションを禁止**し（`tako_orchestrator_spawn` / sub-agent / Workflow ツール）、worker へ委任せず solo セッション自身がファイル編集・テスト・コミットを直接行う。Claude Pro 等のプランでの省トークン運用を想定し、既定 `effort=high`（master の `max` より低い）、「最近やってること」は起動時にロードせず必要時に `git log` で参照する。master と `projects.yaml` / `build_master_claude_cmd` を共有するため、`cd` せずプロジェクト名で（「demo の README 直して」）話せる。プロファイル引数は master と同一パターン（`-<名前>` = プロファイル、裸の語 = 後方互換サフィックス）。role と `TAKO_ORCHESTRATOR_ROLE` は `solo` / `solo:<suffix>`（master の `orchestrator-master` と区別）。solo プロファイルは `solo-profiles/` に置く

### Fixed

- Claude Code conversations now resume after a full PC restart (#139): tako periodically associates running Claude session IDs with their tmux-backed panes and stores them in `layout.json`. On restore, an existing backend session is still reattached unchanged; only when that backend disappeared (as happens on reboot) does tako validate the local transcript and run `claude --resume <session-id>` in the recreated pane. Explicitly exited or unidentifiable sessions are not guessed, and the behavior remains controlled by the existing `tako persist` / `tako_persist` setting
  PC 再起動後も Claude Code の会話を復旧（#139）: 実行中 Claude の session ID を tmux backend ペインへ定期的に対応付け、`layout.json` に保存する。復元時、backend session が生存していれば従来どおりそのまま再 attach し、PC 再起動のように backend 自体が消失した場合だけローカル transcript を検証して、再作成したペインで `claude --resume <session-id>` を実行する。明示終了済み・特定不能なセッションを推測で戻すことはなく、既存の `tako persist` / `tako_persist` 設定で制御される

- PDF drag selection is visible again (#152): the PDF text canvas is now pinned to the page image's top-left instead of inheriting a static position below the image, and selection rectangles are composited in a dedicated topmost GPUI layer. Syntax highlighting now preserves line endings required by syntect's parser and uses one path/filename/shebang resolver for both read and edit modes across the bundled standard language set (including C++ and Python), with JavaScript fallback for TypeScript files
  PDF のドラッグ選択ハイライトを再修正（#152）: PDF テキスト canvas を画像直後の static position ではなくページ画像左上へ固定し、選択矩形を GPUI の専用最前面 layer で合成する。シンタックスハイライトは syntect パーサが必要とする行末改行を保持し、読み取り／編集の両モードを同一のパス・特殊ファイル名・shebang 解決器へ統一した。C++／Python を含む同梱標準言語セット全体を対象とし、TypeScript は JavaScript 文法へ安全にフォールバックする

- Preview selection now follows the actual GPUI-shaped text coordinates instead of terminal-cell estimates (#145), including Markdown font sizes, mixed Japanese/ASCII text, tabs, and vertical scrolling. PDF selection uses PDFKit line/character rectangles transformed onto the rendered page, and editable previews keep syntax colors while composing selection/caret highlights. Preview swaps invalidate stale coordinate caches, and self-tests synchronize on real CLI/paint completion instead of fixed delays
  プレビュー選択の座標ずれを修正（#145）: ターミナル固定セル換算をやめ、GPUI が実際に shaping した座標から Markdown の文字サイズ・日本語／半角混在・タブ・縦スクロール後の byte 位置を逆算する。PDF は PDFKit の行／文字矩形を表示ページへ変換して選択し、編集モードでも構文色と選択／キャレットを合成する。ファイル差し替え時は旧座標キャッシュを破棄し、セルフテストは固定待ちではなく実 CLI／paint 完了へ同期する

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
