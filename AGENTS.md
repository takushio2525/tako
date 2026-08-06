# tako — エージェント向けガイド

AI 駆動・エージェント集約監視に特化した OSS GUI ターミナル。
iTerm2 + Zed の思想で Zed 級に高速・軽量。macOS 先行、Windows 対応必須。GPL-3.0-or-later。

> このリポジトリの AI 向け規約はここに集約してある。詳細仕様は `.agent/` を参照。
> 人間向けの説明は `README.md` にある。

## 概要

- 目的: AI エージェント（Claude Code 等）+ 子エージェント + dev サーバーを「1 グループ = 1 タブ」で集約監視する
- 対象: AI エージェントで開発する開発者。**ただしゼロコンフィグで一般ユーザーが使えることが最優先の設計原則**
- 状況: **Phase 1〜4 + 5.5 完了（macOS MVP / CLI / MCP / パッシブ検知 / tmux バックエンド永続化）。
  Phase 5（ワークスペース機能）はファイルツリーまで完了で中断中 → 次は FR-3.2 から再開**

## 技術スタック

| 領域 | 採用 | 補足 |
|---|---|---|
| 言語 | Rust | |
| UI | GPUI（Zed 製） | **pre-1.0・破壊的変更頻発・Windows 対応進行中**。リスクと対策は `.agent/architecture.md` |
| ターミナル | alacritty_terminal | |
| テスト / Lint | cargo test / fmt + clippy（-D warnings） | コード着手後に CI 化 |

## ディレクトリ規約

```
tako/
├── AGENTS.md / CLAUDE.md   ← AI 向け規約（このファイル）
├── .agent/                 ← AI 向け詳細仕様（下記参照）
├── README.md / LICENSE     ← 人間向け・GPL-3.0-or-later
├── crates/
│   ├── tako-core/          ← ドメインモデル（PaneTree / Workspace / TerminalSession、GPUI 非依存）
│   ├── tako-control/       ← 制御プレーン（IPC + dispatch + MCP 実装済み。検知は Phase 4）
│   ├── tako-app/           ← GPUI バイナリ（GPUI 依存はここだけ。IPC / MCP サーバー内蔵）
│   └── tako-cli/           ← Layer 1 CLI（`tako` コマンド）+ MCP stdio ブリッジ（`tako mcp serve`）
├── poc/                    ← Phase 0 の使い捨て検証コード（品質基準の対象外）
└── .github/workflows/      ← CI（macOS / Windows ビルド + テスト）
```

- `.agent/` に置くもの: AI 向け仕様・作業文脈。置かないもの: 人間向け紹介文（README へ）
- コード着手前に `.agent/` の該当仕様を読み、仕様変更はコードと**同一コミット**で md に反映する

## 絶対ルール

- **cmux（GPL-3.0）のソースコードを読まない・参照しない・移植しない**。設計思想のみ参考可（`.agent/concept.md`）
- ペイン内容・送信テキスト・`TAKO_TOKEN` を**診断ログ**（persist.log / perf.log / stderr）に出さない
  （ペインログ機能 FR-5.13 はユーザー管理のローカルデータとして明示の例外。`.agent/requirements.md`）

## 機能実装時の必須ルール（開発不変条件）

- **設計原則 5「AI フルコントロール」は不変条件**: すべての機能は追加した時点で MCP / CLI から
  操作可能でなければならない（UI でできることはすべて AI からもできる）。新機能の Definition of
  Done に「対応する MCP / CLI 操作の提供」を含め、例外は理由を `.agent/requirements.md` に明記する
- 新機能の操作ロジックは tako-core の操作 API として実装し、`tako-control::dispatch`
  （protocol + ControlHost）へ 1:1 で載せる（UI 層に閉じたロジックを作らない）。
  Phase 2 以降、CLI はこの経路で操作できる。MCP 公開（Phase 3）も同じ dispatch を呼ぶ
- **「最も簡単なコマンドを提案する」原則（#322）**: ユーザーへ提示するコマンドは常に最簡形
  （既定値で済む引数を付けない。`tako master -default` ではなく `tako master`）。機能追加は
  新しい `--オプション` ではなく既定動作を賢くする方向で設計する。CLI 出力・system prompt・
  docs のすべてに適用。詳細は `.agent/conventions.md`「コマンド案内の規約」

## コマンド

| 操作 | コマンド |
|---|---|
| dev（最小ターミナル起動） | `cargo run -p tako-app` |
| **実験・検証用の隔離起動（本番 GUI 稼働中は必須。#177）** | `TAKO_ISOLATED=1 cargo run -p tako-app`（discovery / persist / tmux socket を一括隔離。個別の `TAKO_DISCOVERY_DIR` だけの隔離は本番セッション強奪を起こすため**禁止**） |
| セルフテスト起動（入力経路 + CLI / MCP e2e の機械検証） | `TAKO_SELF_TEST=1 cargo run -p tako-app` |
| 実 claude の e2e（#28 の Shift+Enter = 45c / #716 のチャット送信 = 95c。要 claude CLI + 認証 + tmux） | `env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_SESSION_ID -u CLAUDECODE -u CLAUDE_CONFIG_DIR TAKO_SELF_TEST=1 TAKO_SELF_TEST_CLAUDE=1 cargo run -p tako-app`<br>**claude セッションの中から起動するときは `CLAUDE_CODE_*` を必ず外す**: ペイン内の claude が `CLAUDE_CODE_CHILD_SESSION` を継承すると **transcript 保存が無効化**され（画面に `Transcript saving is off` が出る）、transcript を読む機能（チャットビュー #702/#716・sessions #112・resume #652）の検証が全部空振りする |
| Claude Code 実機検証（MCP 設定ゼロ接続） | `scripts/verify-claude-mcp.sh`（要 claude CLI + 認証） |
| 自動セットアップ | `tako setup [--yes] [--answers <json|@file|->]`（質問ゼロ。`--review` だけ個別対話。MCP `tako_setup` と 1:1。#262） |
| MCP セットアップ | `tako setup-mcp`（`~/.claude/settings.json` に自動追加。`--project` でプロジェクト単位） |
| `tako` CLI ビルド | `cargo build -p tako-cli`（バイナリは `target/debug/tako`） |
| .app バンドル生成（macOS） | `scripts/build-app.sh [--verify] [--install]`（`dist/tako.app`。tako CLI 同梱） |
| リリース | `scripts/release.sh`（Cargo.toml バージョン自動読み取り + CHANGELOG.md 連携。`--publish` でタグ + GitHub Release 作成、`--draft` でドラフト。ノートは実アセットから生成 = ダウンロード表 + OS 別手順 + Known limitations。`--notes-only` で生成物のドライラン、`--update-notes [tag]` でアセット後付け後のノート作り直し。#594） |
| 夜間パッチリリース（自動） | `scripts/nightly-release.sh`（launchd から毎日 5:00 実行。`--dry-run` で判定のみ、`--install-launchd` でジョブ登録。#166） |
| マスターオーケストレーター起動 | `tako master [-profile]`（master system prompt 付きでエージェント CLI を起動。プロファイルの `master_agent` で claude（既定）/ codex を選択。#127） |
| ソロエージェント起動（オーケストレーション無しの 1 対 1 対話） | `tako solo [-profile]`（solo system prompt 付きで起動。worker spawn 禁止・エコ運用 effort=high。master と同じプロファイル引数・`master_agent` 対応） |
| オーケストレーター master 自己情報 | `tako orchestrator self [--pane N]`（自 pane/tab/ctx%/handoff 状態 + 引き継ぎ閾値（`ctx_threshold` / `ctx_threshold_source` / `ctx_over_threshold` / `auto_handoff`）。#123/#193/#749） |
| オーケストレーター master 引き継ぎ（#193/#749） | `tako orchestrator handoff [--pane N] [--tab T]`（handoff ファイルを読み後任 master を同タブ・同 role・同プロファイルで spawn）。**前任ペインはこの呼び出しでは閉じない**: 後任が「引き継ぎファイルと実態の突き合わせ → 前任の入力欄にユーザーの未送達指示が残っていないか確認 → `tako_close_pane`」の順で閉じる（後任の起動が失敗しても master を失わない）。応答の `previous_master_pane_id` が退役予定のペイン（null なら閉じるよう指示していない） |
| **master の自動ハンドオフ（#749）** | ctx% が閾値（既定 60。**50〜60 で設定可**）を超えると tako が master ペインへ `【tako 自動通知】` を送り、master が「handoff ファイル最新化 → `tako_orchestrator_handoff`」を**ユーザーの許可を待たずに**実行する。閾値の解決順は プロファイル → config.yaml → 既定 60（範囲外の明示指定はエラー、手書き設定は丸めて `warnings`）。設定は `tako orchestrator profiles set <名前> --ctx-threshold 55` / `--auto-handoff false`（GUI は設定画面 → プロファイル → 自動ハンドオフ）。送った記録は `<data_dir>/supervisor.log` の `action=ctx_handoff_nudge`。詳細は `.agent/orchestrator.md`「master の自動ハンドオフ」 |
| オーケストレーター worker spawn | `tako orchestrator spawn --project <key> --prompt "..."`（`--account <名>` でその worker だけ別アカウント。#504/#511） |
| オーケストレーター worker 監視 | `tako orchestrator watch --pane <N>` または `--worker <ID>`（レジストリ自動補完でペイン消失後も追跡継続。#390）。停止イベントは `WORKER_IDLE` / `WORKER_QUESTION` / `WORKER_ERROR` / `WORKER_STALLED` / `WORKER_PERMISSION` / **`WORKER_DIALOG`（種別つき。#748）** / `WORKER_DEAD` / `WORKER_GONE` |
| オーケストレーター ダイアログ応答（#319 → #748 で全種別） | `tako orchestrator respond --pane <N> [--choice <番号\|ラベル>]`（**`--choice` 省略で送信せず選択肢の構造だけ返す** = 下見。permission だけでなく usage limit の対処選択・`/model`・`/mcp` の一覧・plan 確認・AskUserQuestion も対象。番号つきダイアログは**番号キーだけ**で確定し、番号なしは矢印移動 + ラベル一致検証 + Enter。応答前にダイアログ実在を再検証し `persist.log` へ監査記録。**ダイアログ表示中の `tako send` は選択肢つきエラーで拒否される**（テキストはキー操作として食われ数字は選択を確定させるため）。`worker_status` / `read` の `choice_dialog` で構造を読める。MCP `tako_orchestrator_respond` と 1:1） |
| オーケストレーター worker 報告取得 | `tako orchestrator report --pane <N> [--lines 2000]`（scrollback + transcript 2 層。`--worker <ID>` でペイン消失後も取得可。MCP `tako_orchestrator_report` と 1:1。#364/#390） |
| オーケストレーター worker レジストリ一覧 | `tako orchestrator workers [--all]`（spawn 済み worker をペインの生死と無関係に列挙。prompt 未達・突然死の resume コマンドも表示。MCP `tako_orchestrator_workers` と 1:1。#390） |
| オーケストレーター プロジェクト管理 | `tako orchestrator projects list/add/remove` |
| オーケストレーター プロファイル管理（#721/#749） | `tako orchestrator profiles list/show/set/create/copy/delete`（`--solo` で `tako solo` の solo-profiles/ を対象。既定は master。`set --projects a,b` で担当プロジェクト割り当て。`set --ctx-threshold 50〜60` / `--auto-handoff <bool>` で自動ハンドオフ（#749）。`default` は削除不可。list / show / set は未登録 project・未登録アカウント・`[1m]` モデルを `warnings` で返す。**GUI は設定画面の「プロファイル」タブ**（Cmd+, → プロファイル）が同じ dispatch を通る。MCP `tako_orchestrator_profiles` と 1:1） |
| オーケストレーター アカウント管理（#504/#548） | `tako orchestrator accounts list/show/add/remove`（既定の資格情報を使うアカウントは `add <名前> --inherit`。既定パスの明示指定は警告。MCP `tako_orchestrator_accounts` と 1:1） |
| worker spawn のレイアウト設定 | `tako orchestrator layout [--policy master-reserved\|legacy] [--master-ratio 0.5] [--algorithm grid\|spiral]`（全省略で現在値表示。#165） |
| build | `cargo build --workspace` |
| lint | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` |
| test | `cargo test --workspace` |
| ファイルツリーフォルダ操作 | `tako tree add <path>` / `tako tree remove <path>` / `tako tree list`（AI がプロジェクトフォルダを明示追加。#134） |
| プレビュー目次操作 | `tako preview-outline [--pane N] [--item N]`（Markdown / PDF 目次の一覧・1 始まり項目ジャンプ。MCP `tako_preview_outline` と 1:1。#232） |
| プレビュー内リンク（#680 / #271） | `tako preview-link-list`（Markdown の `[text](url)` / PDF 注釈リンクを一覧。応答の `kind` が `markdown` / `pdf`）/ `tako preview-follow-link <index>`（URL は OS 既定ブラウザ。**http / https のみ**開き、`javascript:` / 相対パス / アンカーは拒否。PDF の内部リンクはページジャンプ）。GUI は ⌘+ホバーで下線 + ⌘+クリックで同じ経路。MCP `tako_preview_link_list` / `tako_preview_follow_link` と 1:1 |
| Markdown コードブロックのコピー（#680） | `tako preview-copy-code [index]`（装飾なしの全文をクリップボードへ。index は出現順 0 始まり・省略で先頭。GUI はブロック右上のコピーボタンと同一経路。MCP `tako_preview_copy_code` と 1:1） |
| プレビューライブリロード | `tako preview-reload [on\|off]`（引数なしで現在値。既定 ON・settings.json 永続化・MCP `tako_preview_reload` と 1:1。#233） |
| プレビュー画像キャッシュ | `tako preview-cache [max_mb]`（引数なしで上限・使用量・件数。既定 512MiB、256〜8192MiB、settings.json 永続化・MCP `tako_preview_cache` と 1:1。#258） |
| 受け入れゲート（#244） | `tako task gate set <task_id> --command "cmd" [--pr-merged N] [--custom "desc"]` / `tako task gate check <task_id>` / `tako task gate show <task_id>`（MCP `tako_task_gate` / `tako_task_gate_check` / `tako_task_gate_show` と 1:1） |
| git ブランチ操作（#496） | `tako git checkout <branch>` / `branch <name> [--from <ref>] [--no-checkout]` / `merge <branch> [--no-ff]` / `abort` / `conflicts`（checkout・merge は既定で**実行せず**「何が起きるか」を出す。`--yes` で実行。MCP `tako_git_checkout` / `tako_git_branch_create` / `tako_git_merge` / `tako_git_merge_abort` / `tako_git_conflicts` と 1:1） |
| コンフリクト解消エージェント（#496） | `tako git resolve [--agent claude\|codex\|agy] [--tab N]`（同じタブにペインを立て、リポジトリ・未解決ファイル・マージ元/先を含む解消プロンプトを自動投入。文面は `<data_dir>/orchestrator/conflict-resolver.md` で差し替え可。MCP `tako_git_resolve_agent` と 1:1） |
| Web ビューペイン操作 | `tako web open <url>` / `list` / `show <id>` / `hide` / `close` / `nav <to>` / `eval <js>` / `eval-result <token>` / `read`（ネイティブ WKWebView ペイン。#155） |
| 複数ウィンドウ操作（ビューポート方式 + 共有タブバー。#339/#380） | `tako window list` / `new [--tab N]` / `close <W>` / `move-tab --tab N --window W` / `focus <W>`（タブバーは全ウィンドウ共通で全タブ表示、クリックで表示がそのウィンドウへ移る。MCP `tako_window` と 1:1） |
| エージェント共通ルール同期 | `tako agents sync-rules` / `tako agents status`（正本から各エージェントのグローバル指示ファイルへマーカーブロック同期。#136） |
| AI 系設定のデバイス間共有（#513） | `tako config`（引数なしで状態と差分）/ `init [--path P] [--remote URL]` / `link <パス\|URL>` / `push [-m msg]` / `pull` / `list`（何を共有し何を共有しないかの分類表）。claude のグローバル指示（CLAUDE.md / snippets / commands / templates）+ tako の宣言的設定（profiles / projects / accounts / local-rules / settings）を git 1 本で mac ⇔ Windows 共有。秘匿情報とマシンローカル状態はホワイトリストで構造的に除外、未分類は共有しない。絶対パスはホーム部分が `~` に正規化される。MCP `tako_config_share` と 1:1 |
| レイアウト復旧（タブ・ペイン消失時。#177/#381/#770） | `tako recover`（バックアップ世代一覧）→ tako 終了 → `tako recover --apply <世代>`（1〜3 または `good` = 最後に復元へ成功した良品）→ tako 再起動。実体 tmux セッションの個別取り込みは `tako tmux open --socket tako --pane <N> <session>`。**世代は「何かを失う保存」の直前に作られる**: ペイン数の半減（#177）に加えて、**tmux セッションを持つペインが消える保存**（#770 のタブ close）。健全世代の押し出し防止で 10 分に 1 回まで |
| **何がいつ消えたかを調べる（#770）** | `<data_dir>/persist.log` に発生源つきで残る: `セッション kill: pane=… session=…（発生源 close:gui-tab）` / `タブ close: tab=… ペイン N / セッション kill M（発生源 …）`。復元・quit の記録と同じファイルなので「再起動で消えた」のか「明示 close で消えた」のかをここだけで切り分けられる（発生源は `close:gui-tab` = タブの × / `close:gui` = ペインの × / `close:kbd` = cmd+W / `close:dispatch(cli\|mcp, caller=…)` = CLI・MCP / `exit` = プロセス死。**quit と PTY 死亡ではセッションを kill しない**） |
| セッションカタログ（会話の発見・復元。#112） | `tako sessions list [--role r] [--project p]` / `tako sessions show <id>` / `tako sessions resume <id>`（記録 cwd で `claude --resume` をペイン起動。claude のみ） |
| ペインの平文ログ（ペイン死亡後も出力を遡る。#112） | `tako logs list` / `tako logs show <pane> [--session <id>] [--lines N]` / `tako logs status` / `tako logs set --enabled --max-mb --total-max-mb` |
| スリープ防止 | `tako sleep-guard status` / `tako sleep-guard set --mode <off\|on\|while-agents-running> --power-condition <ac-only\|always>`（IOKit 電源アサーション。#173） |
| 入力予測（tako 内 zsh のゴースト予測。#600/#614） | `tako autosuggest [on\|off]`（引数なしで現在値。既定 ON・右矢印か Tab で確定・settings.json 永続化・稼働中ペインにも次のプロンプトから反映。MCP `tako_autosuggest` と 1:1。同梱 zsh-autosuggestions を ZDOTDIR 経路で tako 内の zsh にだけ読み込ませるので `~/.zshrc` と外の zsh は不変）<br>`tako autosuggest hint [on\|off]` = 確定キーの案内（ゴースト直後に薄く出るチュートリアル。既定 10 回で消える）／`tako autosuggest tab [on\|off]` = ゴースト表示中だけ Tab を確定にする（#614） |
| UI テーマ切替 | `tako theme [dark\|light\|toggle]`（引数なしで現在値。settings.json 永続化・GUI 即時反映。タブバー右のボタン / MCP `tako_theme` と 1:1。#217） |
| UI 表示モード切替（GUI ライク表示。#691/#694/#702/#715/#716/#720/#725/#737/#739） | `tako ui-mode [gui\|terminal\|toggle]`（引数なしで現在値。既定 terminal = 従来の表示。gui ではアイドルなシェルのペインが「AI チームに任せる / AI と 1 対 1 で話す / コマンド入力へ」の 3 ボタン + 下部に「初期設定をやり直す（`tako setup`）」の控えめなリンクになり、**claude 対話ペインは会話ビュー**になる。**起動カードの右端の ▾ でプロファイルを選べる**（#739。選択肢が 2 つ以上のときだけ出て、選ぶと `tako master -<名前>` が入る。カード本体は既定起動のまま = #322 の最簡形。各項目に担当プロジェクト / 起動フォルダ / モデルの手がかりが付く）。settings.json 永続化・全ウィンドウ即時反映。タブバーのテーマボタン左隣 / ⌘K パレット / MCP `tako_ui_mode` と 1:1）<br>**ペインを作った直後・エージェントを起動した直後は「準備中…」で覆う**（#720。direnv のロードログや起動途中の画面を見せない。上限つきなので確定しなければ通常表示へ落ちる。ヘッダの「ターミナルを表示」でいつでも中身へ抜けられる。`tako run` のようにコマンド付きで作るペインは覆わない）。`tako ui-mode` の応答 `pane_display` が**いま各ペインに何が出ているか**（`terminal` / `starter` / `chat` / `preparing`）を返すので、AI は画面の状態をそのまま確認できる<br>会話ビューの中身: モデル名・状態・コンテキスト残量バー（**80% 超で警告色 + 「/compact で会話を軽くする」の押せるヒント**。#739）・md 描画・ツール / 思考の折りたたみ・下端追従 + **user / assistant とも枠付きブロック**（#737。発話の境界とコピー対象が一目で分かる）+ **生成中は会話末尾の AI 側に作業中インジケータ**（#737。TUI のスピナー行 = 作業内容 + 経過時間 + 受信トークン数。終わると本文に置き換わる）+ **生成中に送った指示もすぐ自分の吹き出しで見える**（#737。transcript の `queue-operation` を読むので配送前から出て、配送後も二重化しない）+ **入力欄** + **スラッシュボタン 3 つ**（/compact・/clear（確認つき）・/help）+ **承認カード**（画面に permission ダイアログが実在するときだけ。押下は `tako orchestrator respond` と同経路）+ **コマンド提案カード（#666）のインライン表示**（md コードブロック風 = 背景パネル + 等幅 + コピー / 新規ペイン実行。ターミナル表示では従来の帯 #703）。画像添付・システム通知は正規化層で分類され生 XML は出ない（#715）<br>**入力欄は claude TUI の入力行のミラー（#718/#719）**: ローカル下書きを持たず打鍵は PTY へ素通しなので、Enter / Shift+Enter・IME・画像ペースト（⌘V = Ctrl+V 素通しで TUI が `[Image #N]` を挿入）・ゴースト提案が TUI と完全に一致し、表示モードを往復してもズレない。箱の高さは TUI の入力行数に追従し（1 行なら 1 行ぶん）8 行で頭打ち。**worker ペインにも入力欄が出る**（直接指示可）<br>**入力欄に見える文字列は常にミラー 1 本（#737）**: TUI が箱の中に自前の案内文（空欄時の `Try "…"` / キュー滞留時の `Press up to edit queued messages`）を描いているときは tako のプレースホルダを重ねない（重なって読めなくなっていた回帰の根治）。**IME の未確定文字列と変換候補ウィンドウは入力欄のキャレット位置**に出る（チャット表示はターミナルグリッドを描かないので、セル座標のアンカーは画面上のどこも指していなかった）<br>**会話本文はドラッグで選択でき ⌘C / ⌘A が効く（#725）**: 選択はプレビューと同じ実 shaping 座標系で、**複数の発話にまたがって**掃ける。発話の右にはコピーボタン（画面と同じプレーンテキストで全文。折りたたみ中でも全文）、md コードブロックには #680 と同じコピーボタンが出る<br>`tako ui-mode release [--pane N]` = そのペインだけターミナル表示へ（揮発。`restore` で戻す）。**表示レイヤだけの切替なので PTY・tmux セッション・実行中プロセスには影響しない** |
| チャット本文のコピー（#725） | `tako chat copy [--pane N] [--message N] [--code K] [--markdown] [--list]`（UI のコピーボタンと同じ経路。`--message` 省略で**最後の assistant 発話**、`--list` で添字・role・文字数・コードブロック数の下見、`--code` でその発話のコードブロックだけ、`--markdown` で md ソースをそのまま。既定は画面と同じプレーンテキスト。MCP `tako_chat_copy` と 1:1） |
| プラットフォーム対応マトリクス（#515） | `tako platform [--platform macos\|windows] [--status pending] [--known-limitations] [--json]`（この環境でどの機能が使える / 縮退 / 未実装かを表示。`--known-limitations` はリリースノート用の日英併記 markdown を出力（#594）。GUI 不要のローカル処理。MCP `tako_platform` と 1:1） |
| 右パネルのビュー切替 | `tako panel --show --view <fleet\|orch\|git>`（値は GUI のタブ表示名と同じ。orch = master + ワーカーツリーの俯瞰。`tmux` は fleet の旧称で後方互換のみ受理。#217/#553） |
| Code Runner でファイル実行（#453） | `tako run <file> [--profile <name>]`（ファイル内 `tako:run:` 宣言 or 拡張子既定で新ペイン分割実行。`--list` でプロファイル一覧、`--wait` で完了待ち。MCP `tako_run` / `tako_run_resolve` / `tako_run_defaults` と 1:1） |
| 拡張子既定コマンド設定 | `tako run-default [ext] [command]`（引数なし = 一覧。`--remove` で削除。MCP `tako_run_defaults` と 1:1） |
| AI コマンド提案カード（#666/#703） | `tako show-command <コマンド>...`（**ターミナル領域を縮めて作った専用帯**にコピー / 新規ペイン実行つきのカードを出す = 会話・入力欄・フッターと重ならない。`--label` で説明、`--pane` で対象、`--list` / `--copy` / `--run` / `--dismiss` でカード操作、`--card` / `--index` で対象指定）。**AI が会話本文に書いたコマンドは TUI の物理改行でコピーが壊れる**ため、実行を頼むコマンドはこれで提示する。カードは揮発（永続化しない）。MCP `tako_show_command` と 1:1 |
| 設定画面（#459/#721） | `tako settings [--tab <名>]`（Cmd+, / パレット / MCP `tako_settings`。独立ウィンドウで一般・外観・Code Runner・**プロファイル**・セットアップ・スリープ防止・リモート・高度の 8 タブ。`--tab profiles` で master / solo の起動プロファイルをフォーム編集 = `tako orchestrator profiles` と同じ dispatch） |
| 初回起動バナー（#549） | `tako welcome [show\|dismiss]`（引数なしで表示状態 + 案内コマンド。初回起動時だけ `tako setup` → `tako master` の導線をタブバー直下に出す。⌘K パレットにも同じ 3 項目が常設。MCP `tako_welcome` と 1:1） |
| アプリ内更新（#36/#403/#616/#690） | `tako update [status\|check\|apply\|apply-zip\|repair]` に加え、`tako update open` = 専用画面（設定画面と同じ独立ウィンドウ。現在 / 最新バージョン・チャンネル・配布系統・配布物・リリースノート・「今すぐ更新」+ 更新フロー全状態）、`tako update card [dismiss\|show]` = 上部通知カード（引数なしで状態。× で閉じるとそのバージョンは以後通知しない）。**下部ステータスバーには一切出さない**（#616）。リリースノートは **Markdown レンダリング**（見出し / 表 / リスト / コード / 引用。リンクは ⌘+クリックで既定ブラウザ = http / https のみ）で、描画はプレビューペインと同じ `md_view`（#690）。⌘K パレット「アップデートを開く」/ MCP `tako_update` と 1:1 |
| target 掃除 | `scripts/clean-target.sh`（dry-run。`--run` で実行。cargo clean + worktree prune） |

CI（`.github/workflows/ci.yml`）は macOS / Windows の両ランナーで build + test を回す。

## AI 向け詳細仕様（必要なときだけ Read する）

- コンセプト・競合・Non-goals: `.agent/concept.md`
- 機能要件（FR / NFR）: `.agent/requirements.md`
- 技術設計・リスク・3 層制御プレーン: `.agent/architecture.md`
- 規約（命名・エラー・ログ）: `.agent/conventions.md`
- 手動確認チェックリスト（IME・.app 等、機械検証できない項目）: `.agent/manual-checks.md`
- オーケストレーター使い方ガイド: `.agent/orchestrator.md`

### 作業履歴メモ（毎ターン参照・更新）

- 現在の作業状況（毎ターン上書き）: @.agent/activeContext.md
- 完了タスクの時系列（毎ターン追記）: @.agent/progress.md
- フェーズ計画・次の一手: @.agent/roadmap.md

セッション開始時に必ず読み、応答終了前に `activeContext` は最新状態で**上書き**、
作業が一段落していれば `progress` の末尾に**1〜3 行で追記**する。
スキップ可能なターン（単発質問への回答、タイポ修正のみ）では更新しない。
詳細ルールはグローバル CLAUDE.md の「プロジェクト作業履歴メモ」節を参照。

## コミット規約

グローバル CLAUDE.md（`~/.claude/CLAUDE.md`）の「Git コミット」節に従う。
push 運用: リポジトリ公開（Phase 7）までは main 直 push 可。公開後はブランチ + PR 経由に切り替える。

## リリース運用

- 機能追加・バグ修正が一段落したら `CHANGELOG.md` に追記（日英併記、Keep a Changelog 形式）
- `Cargo.toml`（ワークスペースルート）の `[workspace.package] version` を bump
- `scripts/release.sh --publish` でタグ + GitHub Release 作成（CHANGELOG から自動抽出）
- リリースノートは日英併記

### 夜間パッチリリース（自動。#166）

- `scripts/nightly-release.sh` が launchd（`com.takushio.tako-nightly-release`、毎日 5:00）から
  実行され、前回タグ以降に main へ変更があった夜だけパッチバージョンを自動リリースする
  （patch bump → CHANGELOG 自動節 → コミット → annotated tag → release.sh でバイナリ付き
  GitHub Release）。クラウドルーチンでの夜間リリースはバイナリを作れず廃止した（経緯は #166）
- 自動スキップ条件: 変更なし / worktree dirty / 手動リリース進行中（Cargo.toml version ≠ 最新タグ）/
  多重起動。ログは `~/.claude-orchestrator/logs/tako-nightly-release.log`
- ジョブ登録は `scripts/nightly-release.sh --install-launchd`（解除は `--uninstall-launchd`、
  確認は `launchctl list | grep tako-nightly`）。plist はリポに置かず実行時に生成する
- minor / major リリース・Homebrew cask 更新・リリースノートの日英併記は従来どおり手動で行う
