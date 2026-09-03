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
- **実ユーザー名・実ホームパス・実ホスト名を現行コードへ置かない（#927）**。public リポなので、
  実機の採取物（ペインの capture・PowerShell のプロンプト行・`HOME` の値・claude TUI の cwd 行）は
  貼る前にプレースホルダ（`testuser` / `winuser` / `山田` 等）へ置換する。番犬は
  `crates/tako-control/tests/no_personal_data.rs`、書き方は `.agent/conventions.md`

## 機能実装時の必須ルール（開発不変条件）

- **設計原則 5「AI フルコントロール」は不変条件**: すべての機能は追加した時点で MCP / CLI から
  操作可能でなければならない（UI でできることはすべて AI からもできる）。新機能の Definition of
  Done に「対応する MCP / CLI 操作の提供」を含め、例外は理由を `.agent/requirements.md` に明記する
- 新機能の操作ロジックは tako-core の操作 API として実装し、`tako-control::dispatch`
  （protocol + ControlHost）へ 1:1 で載せる（UI 層に閉じたロジックを作らない）。
  Phase 2 以降、CLI はこの経路で操作できる。MCP 公開（Phase 3）も同じ dispatch を呼ぶ
- **設定・データファイルのスキーマ変更は常に自動マイグレーション（#916）**: 永続ファイル
  （`settings.json` / `layout.json` / `projects.yaml` / `profiles/*.yaml` / `handoff/` 等）の
  **形式や置き場を変えるときは自動移行を同梱する**。ユーザーや master へ手動の移行作業を
  要求してはならない（「移行手順を提示する」も不可）。実装は `tako-core::migration` の機構へ
  `tako-control::migrations::SPECS` の `target_version` を上げて `Step` を足す形で載せる。
  発火は二段構え（`tako setup` 実行時 + GUI / master / CLI の実行時差分検出）で既に配線済み
  なので、登録するだけで両方から効く。安全要件（冪等 / 旧ファイルは `.pre-v<N>.bak` へ退避して
  消さない / 解釈できない内容は `.unreadable.bak` へ保全 / persist.log へ記録）は機構側の
  1 実装が担保する。**永続構造体のフィールドを増減・改名した PR は
  `migration_registry` テスト（指紋スナップショット）が落ちる**ので、
  「serde の default / alias で旧ファイルがそのまま読める」か「移行を足した」かを明示する
- **agent 系統ごとの能力差はマトリクスへ書く（#982）**: claude 以外で落ちる / まだ使えない機能を
  作った・見つけたら、判断を `if agent == claude` で散らさず
  **`tako-core::agent_support::MATRIX` の 1 マス**として宣言する（正本 1 箇所・根拠必須・日英の理由文）。
  能力を問う側は `agent_support::supports(agent, keys::…)` を通す。
  この宣言は診断・docs・将来の system prompt がすべて引くので、**過大にも過小にも申告しない**。
  agent 種別の enum は現状 5 つ並存している（統合しない理由と対応表は `.agent/agent-enums.md`）ので、
  **値を増減させた PR は `agent_parity` テストが落ちる** = マトリクスの列と docs も直すこと
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
| 自動セットアップ | `tako setup [--yes] [--answers <json|@file|->]`（質問ゼロ。`--review` だけ個別対話。MCP `tako_setup` と 1:1。#262）<br>**Claude CLI 未導入でもここから始められる（#868）**: 未導入のときだけ インストール → PATH 通し → ログイン の 3 段を案内し、導入済みなら**無言で素通り**する（従来の検出型と同じ体験）。インストール前に「何をどこに入れるか」（公式コマンド・取得元・置き場所・以後の更新）を必ず出してから確認を取り、`--yes` は表示だけして続行する。各段は冪等で、途中で失敗しても `tako setup` をやり直せば残りから再開する。PATH は**ログインシェルの profile**（zsh は `~/.zprofile`。`$SHELL -l -c` が `.zshrc` を読まないため）へマーカーブロック 1 個だけ足す。Homebrew は自動導入しない（インストーラが管理者パスワードを求めるため）。AI からの同等操作は `tako setup bootstrap [status|install|path|undo-path|handoff]` / MCP `tako_setup_bootstrap`（`status` は読み取り専用で `next_step` = install / path / auth / ready を返す。`install --dry-run` で実行せず計画だけ）。<br>**Windows も実行代行する（#1057）**: `install.ps1` を落として `-ExecutionPolicy Bypass -File` で走らせる（公式の `irm | iex` は文字列を食わせるので ExecutionPolicy の対象外だが、ファイル経路は既定 `RemoteSigned` に弾かれる = 実機実測）。取得物の見分け方も**署名**でプラットフォーム別（`install.sh` = shebang / `install.ps1` = `param(` 等の PowerShell の語。正本は `tako_core::platform::agent_install` の `InterpreterSpec` / `ScriptSignature`）。**PATH の通し方も OS で別**: unix はログインシェルの profile、Windows は**ユーザー環境変数 `Path`**（`HKCU\Environment`。境界 B23 = `tako_core::platform::user_path`。末尾へ追記・種別 `ExpandString` を保つ・`undo-path` で元のバイト列へ戻す）。**Windows の PATH 判定に `exe::find` を使わない**（PATH 外の導入先まで走査するので「PATH に在る」と誤答して段が飛ぶ）。<br>**自動導入が通らなければエージェントへ引き継ぐ（#1057）**: 導入済みの codex / agy が居れば `tako setup bootstrap handoff`（MCP は `action: "handoff"`。読み取り専用）が候補と指示文を返し、端末があれば確認のうえその CLI を起こす。1 つも居なければ従来の公式コマンド案内へ落ちる。**認証は代行させない**（ブラウザ操作が要るのでユーザーへ依頼させる）。A/B は `TAKO_1057_LEGACY=1` |
| **任意依存のその場導入（#88 / #1057）** | `tako setup deps [install] [--dep <名>] [--dry-run] [--json]`（引数なしで状態。MCP `tako_setup_deps` と 1:1）。対象は**永続化の器 / git / tailscale** で、**器の名前は OS で変わる**（macOS = `tmux` / Windows = `psmux`）ので名前を推測せず `status` の `bin` を使う。器は `exe::find` → `backend::binary()` の順で解決するので `TAKO_TMUX_BIN` 指定や既知の置き場も拾う。<br>**代行するのは実測済みの手段だけ**（macOS = `brew install`。Windows の winget は**案内のみ** = `can_run: false` / `skipped[].reason = not_delegable`）。導入済みは触らない（冪等）。手段が無い・代行できない・導入器が無いものは実行せず `skipped` に理由つきで載る。導入後は「引けるようになったか」を確かめてから成功と言う。<br>**質問するのは `--review` だけ**（標準 `tako setup` は質問ゼロ = #262。標準経路は状態 + `いま入れる: tako setup deps install` の 1 行だけ = #322 の最簡形）。#88 のその場導入は #262 の質問ゼロ化以降 `run_dependency_check(false)` 固定で**一度も通らない死んだコード**だったので、正本を `tako-control::setup_deps` へ集約して CLI / MCP / `--review` が同じ実装を通る形にした。A/B は `TAKO_1057_LEGACY=1` |
| **モデル一覧の実取得とピッカー（#1002）** | `tako setup models [--agent <claude\|codex\|agy>] [--json]`（GUI 不要のローカル処理。MCP `tako_setup_models` と 1:1）。<br>**モデルを勧める前にこれを引く**（学習時点の知識で存在しないモデル名を提案すると起動が失敗する）。取得手段は各 CLI の一覧コマンドで、argv の正本は `tako-control::agent_models::catalog_argv`: **codex = `codex debug models`**（`Render the raw model catalog as JSON`。`visibility=list` のものだけを `priority` 昇順で並べ、`supported_reasoning_levels` から**モデル別の effort 語彙**と `context_window` も取る。未認証でも既定カタログを返し、認証すると内容が変わる = ユーザー固有）/ **agy = `agy models`**（stdout に `id<TAB>表示名` の TSV。stderr の `Fetching available models...` は進捗）。**claude は一覧コマンドを持たない**（`claude models` は**プロンプトとして解釈される** = 実測）ので同梱のエイリアス（`opus` / `sonnet` / `fable`。`claude --model` の help が documented している語）+ `~/.claude/.claude.json` の非公式キャッシュを**加算のみ**で並べ、`failure.kind = no_list_command` で実取得でないことを明示する。<br>**失敗は 5 種に分類**（`cli_not_found` = 未導入で導入手順は #983 の `agent_cli` から引く / `not_authenticated` / `no_list_command` / `command_failed` / `parse_failed`）。どれも「理由 + 次の一手 + 生の詳細」を日英で返す。応答の `source` は `cli`（実取得）/ `builtin`（同梱リスト）/ `none`（並べられるものが無い）。<br>**対話ピッカーは `tako setup --review` だけ**（標準 `tako setup` の質問ゼロ = #262 を壊さない。標準経路は「いま何が選ばれているか + 選択肢の件数 + `tako setup models` / `tako setup --review`」の 2 行だけ出す）。**GUI は設定画面 → プロファイルの model 行**（「候補を取得」を**押したときだけ** background で取る = タブを開く動作は重くならない。`agy models` はネットワーク取得で実測数秒なので同期で呼ばない）。**1 番は常に「CLI の既定に任せる」**（tako はモデルを固定しない = #27 / #67）。未導入の系統も選択肢に並び、選ぶと導入案内が返る。<br>**反映は既存の 1 経路**（`tako orchestrator profiles set default --model <id> --effort <値>` / MCP `tako_orchestrator_profiles`。応答の `apply_command` が形を示す）で、書き込みツールは増やさない。書くのは model / effort と選択系統の `worker_agents` 項目だけ（ロック付き RMW = #169）なので既存プロファイルの他の設定は壊れない。**master が別系統のときは master 側へ書かない**（master=claude のプロファイルへ codex のモデル名を書くと起動が壊れる）。<br>**agy にも `--effort low\|medium\|high` が実在する**（実測 1.1.22: `agy models` の全モデルで不正値が `invalid --effort "bogus" (valid: low, medium, high)` として咎められる。表示名の `(High)` 等はモデル側の設定で別物。**未知のモデル名のときだけ** `--effort is not supported for model "…"` になるので読み違えないこと）ので worker 起動時に渡す。旧挙動は `TAKO_1002_LEGACY=1` |
| **MCP セットアップ（claude / codex / agy。#979）** | `tako setup-mcp`（引数なしで **claude + 導入済みの codex / agy へまとめて登録**。未導入は理由つき skip。`--agent <claude\|codex\|agy>` で 1 つに絞ると、その CLI が未導入・非対応スコープのときだけ分類済みエラーで止まる。`--project` は claude のみ = `<cwd>/.mcp.json`）。<br>書き先は claude = `~/.claude.json` / codex = `~/.codex/config.toml` の `[mcp_servers.tako]` / agy = `~/.gemini/config/mcp_config.json`。**codex / agy は各 CLI の `mcp add` に書かせる**（自分で TOML / JSON を書くと CLI 側の正規化と二重にずれる。実測: `codex mcp add` は書き戻しで env のキー順・`120` → `120.0`・`args = []` を正規化する）。冪等（再実行でバイト一致）。正本は `tako-control::agent_mcp`（argv・置き場・分類済みエラーの純粋関数）で、MCP `tako_setup_mcp` の `agent` と 1:1。<br>**成否を分けるのは env の引き継ぎ**（`tako mcp serve` は `TAKO_SOCKET` + `TAKO_TOKEN` が無いと **0 ツール**を返す = FR-2.3.2）。偽 MCP サーバーで実測した結果、**agy は親 env をそのまま渡す**ので登録だけでよく、**codex は既定で 1 つも渡さない**ので `env_vars` 許可リスト（`TAKO_SOCKET` / `TAKO_TOKEN` / `TAKO_PANE_ID` / `TAKO_ORCHESTRATOR_ROLE`）が要る。`env_vars` は**値ではなく名前**なのでペインごとに正しい値が届き、**トークンを設定ファイルへ残さない**。`codex mcp add` にこれを書くフラグは無く、しかも**再 add で消える**ので、順序は `codex mcp add` → `env_vars` を 1 行足す → `codex mcp list --json` で反映確認（欠けていたら `not_reflected`）。**登録があっても env 転送が欠けていれば「登録済み」と言わない**（0 ツールになるため付け替える）。<br>**副作用**: `codex mcp add` / `agy mcp add` は自分の正規化で設定ファイル全体を書き直す（実測: codex は env のキー順・`120` → `120.0`・`args = []` の消去、agy は空 `args` の消去。コメントとトップレベルの並びは保たれる） |
| `tako` CLI ビルド | `cargo build -p tako-cli`（バイナリは `target/debug/tako`） |
| .app バンドル生成（macOS） | `scripts/build-app.sh [--verify] [--install]`（`dist/tako.app`。tako CLI 同梱。**`--install` は配置後にビルド出力を消して Launch Services の登録も外す** = Finder の「このアプリケーションで開く」に tako が 2 つ並ばない。#837）<br>**配置は置き場のパスを一度も空けずに行う（#1042）**: Dock のピン留めは `.app` への file URL ブックマークで持たれ CNID を優先して解決するので、`rm -rf` → `cp -R` のように置き場が空く瞬間があると、追跡側が退避先へ張り付いたところでその実体を消されピンが外れる。差し替えの正本は `tako_core::platform::bundle_install::replace_bundle_in_place`（`Contents/` だけの `RENAME_SWAP` → バンドルごとの `RENAME_SWAP` → 退避 → 設置 の 3 段）。シェル側は写しの `scripts/lib/bundle-install.sh`、検証は `bash scripts/test-bundle-install.sh`、番犬は `crates/tako-control/tests/bundle_install_watchdog.rs`。A/B は `TAKO_1042_LEGACY=1` |
| リリース（**両 OS 同時が既定**。#594/#965） | `scripts/release.sh`（Cargo.toml バージョン自動読み取り + CHANGELOG.md 連携。`--publish` でタグ + GitHub Release 作成、`--draft` でドラフト。ノートは実アセットから生成 = ダウンロード表 + **動作要件** + OS 別手順 + Known limitations。`--notes-only` で生成物のドライラン、`--update-notes [tag]` でアセット後付け後のノート作り直し）。<br>**リリースは macOS / Windows の配布物が揃って初めて成立する（#965）**: タグ push が `.github/workflows/release-windows.yml` を起こして windows ランナーが installer exe / zip を同じ Release へ添付し、`release.sh` は**その添付を待ってから**ノートを作り直す。揃わなければ **exit 3**（= Release は作られたが片肺）で回収手順を出す。緊急の macOS 先行公開は `--no-wait-windows`。公開済みリリースの検査は `--check-assets [tag]`（揃っていなければ exit 1） |
| 夜間リリース（自動） | `scripts/nightly-release.sh`（launchd から毎日 5:00 実行。`--dry-run` で判定のみ、`--install-launchd` でジョブ登録。#166）。**両 OS 対応（#965）**: タグ push で Windows 配布物のワークフローが走り、`release.sh` の待ち合わせを通るので夜間も両 OS が揃う。片肺で終わったら通知 + ログに回収手順（Release 自体は成立しているのでロールバックはしない）。<br>**次回バージョンの予約（#1005）**: 既定は patch bump だが、節目の minor / major を夜間発火に乗せたいときは `scripts/nightly-release.sh --reserve 0.8.0`（確認は引数なしの `--reserve`、取消は `--unreserve`）。**予約は成立したリリース 1 回で消費**され、使えない値（semver 外 / 現行以下 / タグが既に在る）は**無視して patch bump へフォールバック**しつつ警告 + 通知を出す。リリースに至らなかった夜（変更ゼロ・dirty・ビルド失敗・dry-run）は**予約を保持**して次の夜へ持ち越す。正本は `scripts/lib/nightly-reserve.sh`、モックテストは `bash scripts/test-nightly-reserve.sh`（本番のタグ / Release / 予約に触らない） |
| **Windows 配布物生成（既定は CI。#587/#965）** | 通常はタグ push で `.github/workflows/release-windows.yml`（windows ランナー）が自動生成・自動添付するので**手で叩く必要はない**。実機で作るなら `pwsh -File installer/windows/build-installer.ps1 [-Version v0.7.0]`（`dist/windows/` に `tako-<tag>-windows-x86_64.exe`（インストーラー = 主形式）+ `tako-<tag>-windows-x86_64.zip`（ポータブル）。Inno Setup 6 の ISCC が要る。**アセット名の正は `tako-core::platform::release_assets`** で、PowerShell 側の写し `installer/windows/lib/release-assets.ps1` を経由して組む = リリース側と `tako update` の判定が食い違わない（#594/#595）） |
| **Windows リリース（CI が使えないときの実機経路。#587/#965）** | `pwsh -File installer/windows/release-windows.ps1`（前検査 → ビルド → 配布物検査まで。**既定は dry-run**、`-Upload` で GitHub Release へ添付、`-CreateRelease` で prerelease 新規作成。タグ省略時は Cargo.toml から `v<version>`）。配布物の検査は CI と同じ 1 実装（`installer/windows/lib/verify-assets.ps1`）を通るので、生成場所で基準が変わらない |
| Windows アプリアイコン再生成 | `pwsh -File installer/windows/make-icon.ps1`（A 案 PNG → `assets/icon/tako.ico`。System.Drawing だけで動く = Windows 専用。`.ico` はコミット済みなので通常は不要） |
| マスターオーケストレーター起動 | `tako master [-profile]`（master system prompt 付きでエージェント CLI を起動。プロファイルの `master_agent` で claude（既定）/ codex を選択。#127） |
| **SSH ペイン（ファイルメニュー「リモート接続…」/ ペインの右クリック / `tako open-in remote <host>`。#20 / #919 / #1006）** | ホストを選ぶと **いま開いているタブへ新しいペイン**を作って `ssh <host>` を実行する（#1006 で既定を新タブから変えた）。**接続に失敗してもペインは閉じない**（#919 で根治）: 接続前に「〜へ接続しています…」を出し、`ConnectTimeout` を 10 秒に切り、ssh 自身の失敗（exit 255）だけ**理由 + 次の一手**を出して入力待ちで止まる。`--remote-dir <path>` を付けると接続後にそのフォルダへ `cd` する（両方言で通る `cd "<path>"` を送達確認つき経路で打つ）。<br>**開き先は 3 通り（#1006）**: `--target split`（既定 = いまのタブへ新ペイン）/ `tab`（新しいタブ = #20 の従来動作）/ `pane`（**すでにあるペインをそのまま SSH にする**）。`pane` は**ペインの右クリック「このペインでリモート接続…」**の実体で、ペインもタブも増やさず **pane ID も変わらない**（素のシェルへ ssh の 1 行を #640 の送達確認つき経路で打つので、失敗すればそのペインのシェルのプロンプトへ戻る）。素のシェルでないペイン（全画面 TUI・実行中・AI エージェント・プレビュー）は**理由 + 次の一手つきで断る**（メニューにもその項目を出さない）。判定の正本は `tako_core::remote_open`（語彙 `split \| tab \| pane` と `can_ssh_pane` を CLI / MCP / GUI が共有）。**器（tmux）つきペインの外側 alt screen を判定へ渡さない**（tmux クライアント自身が alt screen なので全ペインが対象外になる。番犬 = `crates/tako-control/tests/remote_open_watchdog.rs`）。MCP `tako_open_remote` の `target` / `pane` / `tab` / `direction` と 1:1。<br>**接続待ちはペインヘッダに出る（#1010）**: 開始した瞬間から「〜へ接続中…」（回る弧つき）が出て、**失敗すると消えずに理由へ置き換わる**（クリックで閉じる）。3 経路とも同じ表示。AI は `tako list` / `tako read` の `ssh_connect`（`phase` = connecting / failed・`host`・`elapsed_secs`・`reason`）で同じ状態を読める |<br>**回線が切れても失わない・戻ったら自動で繋ぎ直す（#1040）**: 切断で**ペインもタブも閉じない**（ssh が終わったペインは理由を残して**ローカルのシェルへ戻る**。旧実装の「Enter でこのペインを閉じます」に従うとタブごと消えていた）。一度でも繋がったペインは**自動で再接続**する（バックオフ 2/5/10/20/30/30 秒・上限 6 回。打ち直しは 3 経路とも同じ 1 行を #640 の送達確認つき経路で送る）。**そのペインで打ち始めたら黙って降りる**。上限に達したら理由 + 次の一手（`ssh <host>` を実行）を出して静かに止まる。鍵・ホスト鍵・設定の失敗は繰り返さない。状態は `tako list` / `read` の `ssh_connect`（`phase` = connecting / reconnecting / failed / gave_up、`attempt` / `max_attempts` / `retry_in_secs` / `disconnected_secs` / `next_step`）で読める。A/B は `TAKO_1040_LEGACY=1`
| ソロエージェント起動（オーケストレーション無しの 1 対 1 対話） | `tako solo [-profile]`（solo system prompt 付きで起動。worker spawn 禁止・エコ運用 effort=high。master と同じプロファイル引数・`master_agent` 対応） |
| オーケストレーター master 自己情報 | `tako orchestrator self [--pane N]`（自 pane/tab/ctx%/handoff 状態 + 引き継ぎ閾値（`ctx_threshold` / `ctx_threshold_source` / `ctx_over_threshold` / `auto_handoff`）。#123/#193/#749） |
| オーケストレーター master 引き継ぎ（#193/#749/#915/#854/#917） | `tako orchestrator handoff [--pane N] [--tab T] [--projects a,b]`（**管轄プロジェクトの引き継ぎだけ**を読み、後任 master を同タブ・同 role・**同プロファイル / 同アカウント / 同モデル / 同 effort**で spawn。プロファイルは呼び出し元 env とペインの role ラベルの両方から解決するので、`TAKO_ORCHESTRATOR_ROLE` を失った master でも取り違えない = #854。後任は**退役 master のペインを分割**して作るので、旧ペイン close 後のレイアウトが交代前と一致する = #917）。**前任ペインはこの呼び出しでは閉じない**: 後任が「引き継ぎファイルと実態の突き合わせ → 前任の入力欄にユーザーの未送達指示が残っていないか確認 → `tako_close_pane`」の順で閉じる（後任の起動が失敗しても master を失わない）。応答の `previous_master_pane_id` が退役予定のペイン（null なら閉じるよう指示していない）。**各ファイルは 2 節に分ける**（#792。`## 知識（マシン非依存）` = 決定事項・方針・残タスクの意図 / `## 実行状態（このマシン限定）` = worker とその pane / tab・実行中のもの）。旧書式（節なし）もそのまま読め、後任プロンプトに「番号は実態で確認 + 次の更新で書き直せ」が付く。いまどちらかは `tako orchestrator self` の `handoff_format` / `project_handoffs[].format`、`handoff` 応答の `handoff_format`（`sectioned` / `legacy` / 新旧混在なら `mixed`）で分かる。置き場とプロジェクト単位化・自動移行は次の行を参照 |
| **master の自動ハンドオフ（#749）** | ctx% が閾値（既定 60。**50〜60 で設定可**）を超えると tako が master ペインへ `【tako 自動通知】` を送り、master が「handoff ファイル最新化 → `tako_orchestrator_handoff`」を**ユーザーの許可を待たずに**実行する。閾値の解決順は プロファイル → config.yaml → 既定 60（範囲外の明示指定はエラー、手書き設定は丸めて `warnings`）。設定は `tako orchestrator profiles set <名前> --ctx-threshold 55` / `--auto-handoff false`（GUI は設定画面 → プロファイル → 自動ハンドオフ）。送った記録は `<data_dir>/supervisor.log` の `action=ctx_handoff_nudge`。詳細は `.agent/orchestrator.md`「master の自動ハンドオフ」 |
| **引き継ぎファイルの管理（#915）** | `tako orchestrator handoffs list/show/write/migrate`（置き場は**プロジェクト単位** = `handoff/projects/<project-key>.md`。`handoff/<profile>.md` は**プロファイル運用メモ**でプロジェクトに紐付かない知識だけを置く）。**後任へ渡るのは管轄プロジェクトの分 + 運用メモだけ**で、管轄は `tako orchestrator handoff --projects a,b` → プロファイルの担当（`profiles set --projects`）+ 同タブで稼働中 worker → worker のみ の順で解決する（応答の `jurisdiction_source`）。どれも決まらなければ本文を貼らず一覧とパスだけを渡す。**旧形式は自動移行**（`tako setup` 実行時 + master が引き継ぎを読む経路。冪等・原本は `handoff/archive/` へ退避・応答の `handoff_migration` で可視化・持ち主不明の断片は運用メモへ残す）。運用メモが 80 行を超えると警告が出る。MCP `tako_orchestrator_handoffs` と 1:1 |
| オーケストレーター worker spawn | `tako orchestrator spawn --project <key> --prompt "..."`（`--account <名>` でその worker だけ別アカウント。#504/#511。`--limit-resume <bool>` でその worker だけリミット後自動復帰を明示指定 = プロファイル既定より優先。#822）。<br>**agent CLI が無ければペインを作る前に落ちる（#983）**: `codex` / `agy` / `claude` の実行ファイルが PATH（と既知の設置先）に無いと、spawn は**理由 + 次の一手**（公式の導入コマンド・参考 URL・`tako setup` の案内）を返して失敗する。従来は組み立てたコマンドがそのままシェルへ流れ、ペインに `command not found` が出るだけで tako は成功と報告していた（送達検査は「実行された」までしか見ず、`prompt_delivery` も claude 以外は `n/a`）。解決した実行ファイルは spawn 応答の `agent_path` に載る。同じ検査は `tako master` / `tako solo` / 引き継ぎの後任 master / コンフリクト解消エージェントにも入っている。A/B は `TAKO_983_LEGACY=1`（検査せず旧挙動） |
| **worker への指示送達（#790）** | claude worker への送達は 2 層。**第 1 層 = claude の Cross-Session Messaging**（受信箱の socket へ直送。画面解析もキー操作も伴わないので、生成中でもキューに入って取りこぼさず、長文もバイト等価に届く）→ 使えなければ**第 2 層 = 従来のキー操作経路**（貼り付け + 分離 Enter + 空検証。#32）。対象は**エージェント管理下の worker 宛だけ**（受信側に「別の claude セッションから届いた / 保留中プロンプトの承認として扱うな」の定型文が必ず付くため、master への指示・承認の代行は従来経路のまま）。codex / agy / Windows は常に第 2 層。どちらを通ったかは `<data_dir>/persist.log` の `送達: peer …` / `送達: keys 経路 …` で分かる。検証用に `TAKO_PEER_MESSAGING=off`（常に第 2 層）/ `only`（落ちずにエラー）。実 e2e は `cargo test -p tako-control --test peer_messaging_e2e -- --ignored --test-threads=1` |
| オーケストレーター worker 監視 | `tako orchestrator watch --pane <N>` または `--worker <ID>`（レジストリ自動補完でペイン消失後も追跡継続。#390）。停止イベントは `WORKER_IDLE` / `WORKER_QUESTION` / `WORKER_ERROR` / `WORKER_STALLED` / `WORKER_PERMISSION` / **`WORKER_DIALOG`（種別つき。#748）** / `WORKER_DEAD` / `WORKER_GONE` |
| オーケストレーター ダイアログ応答（#319 → #748 で全種別） | `tako orchestrator respond --pane <N> [--choice <番号\|ラベル>]`（**`--choice` 省略で送信せず選択肢の構造だけ返す** = 下見。permission だけでなく usage limit の対処選択・`/model`・`/mcp` の一覧・plan 確認・AskUserQuestion も対象。番号つきダイアログは**番号キーだけ**で確定し、番号なしは矢印移動 + ラベル一致検証 + Enter。応答前にダイアログ実在を再検証し `persist.log` へ監査記録。**ダイアログ表示中の `tako send` は選択肢つきエラーで拒否される**（テキストはキー操作として食われ数字は選択を確定させるため）。`worker_status` / `read` の `choice_dialog` で構造を読める。MCP `tako_orchestrator_respond` と 1:1） |
| オーケストレーター worker 報告取得 | `tako orchestrator report --pane <N> [--lines 2000]`（scrollback + transcript 2 層。`--worker <ID>` でペイン消失後も取得可。MCP `tako_orchestrator_report` と 1:1。#364/#390） |
| オーケストレーター worker レジストリ一覧 | `tako orchestrator workers [--all]`（spawn 済み worker をペインの生死と無関係に列挙。prompt 未達・突然死の resume コマンドも表示。列挙のついでに、ペインも器も 5 分以上見えない active を closed（gone）へ倒す。MCP `tako_orchestrator_workers` と 1:1。#390 / #658） |
| オーケストレーター プロジェクト管理 | `tako orchestrator projects list/add/remove` |
| オーケストレーター プロファイル管理（#721/#749） | `tako orchestrator profiles list/show/set/create/copy/delete`（`--solo` で `tako solo` の solo-profiles/ を対象。既定は master。`set --projects a,b` で担当プロジェクト割り当て。`set --ctx-threshold 50〜60` / `--auto-handoff <bool>` で自動ハンドオフ（#749）。`set --limit-resume <bool>` / `--clear-limit-resume` で spawn した worker のリミット後自動復帰の既定（#822）。`set --bypass-sandbox <bool>` で **codex のサンドボックス解除**（`--dangerously-bypass-approvals-and-sandbox`）の許可（既定 false = 外さない。master / solo と codex worker の両方に効く。#981）。`set --remote-control <bool>` で **Claude 公式 Remote Control への委譲**（既定 false = 繋がない。#1068。詳細は下の行）。`default` は削除不可。list / show / set は未登録 project・未登録アカウント・`[1m]` モデルを `warnings` で返す。**GUI は設定画面の「プロファイル」タブ**（Cmd+, → プロファイル）が同じ dispatch を通る。MCP `tako_orchestrator_profiles` と 1:1） |
| **スマホから会話を操作する（Claude 公式 Remote Control。#1068 / #1069 / #1077 / #1078）** | `tako orchestrator profiles set <名前> --remote-control true` で opt-in すると、そのプロファイルで起動する claude（master / solo / spawn した worker）が **claude.ai と Claude モバイルアプリから操作できる**ようになる（起動コマンドへ `--remote-control` が付く）。**既定は OFF**: 委譲した会話は Anthropic のサーバーにも transcript が保存され、認証は claude.ai アカウントへ移る（tako の機器ペアリングと role はその会話に効かない）ので、静かに外へ同期させない。<br>**不適格な環境ではフラグを付けず理由を出す**（付けると claude 自身が起動時に落ちてペインが即死する）。断るのは**ローカルの読み取りだけで確定する 4 種**（阻害 env = `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` / `DISABLE_TELEMETRY` / `DO_NOT_TRACK` / `DISABLE_GROWTHBOOK` / エンドポイント差し替え = `ANTHROPIC_BASE_URL` の host ≠ `api.anthropic.com` と Bedrock / Vertex / Foundry / 非サブスク認証 = `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` / 組織ポリシー = managed settings の `disableRemoteControl`）。**プラン・組織のエントイトルメント・ZDR はローカルから分からないので不適格と言わない**（フラグを付けて claude 自身に言わせる）。判定材料は「起動先が実際に見る env」なのでプロファイルの env 計画を反映した後で評価する。理由の出口は CLI の起動前 1 行 / spawn 応答の `warnings` / `profiles show` の `remote_control_blocked`。<br>**会話を開く URL は `tako sessions link [--pane N]`**（MCP は `tako_sessions` の `action: "link"`）。応答の `remote_link.state` は `connected`（`url` あり）/ `not_connected`（まだ繋いでいない）/ `ineligible: <理由>` / `unknown`（会話が特定できない）で、**繋がっていなければ URL を返さない**（捏造しない）。`account_label` はどの tako アカウント配下かで、**スマホが別アカウントでログインしていると一覧に出ない**ので切り分けに使う。同じ値が `GET /api/agents` / `GET /api/v2/panes` の `remote_link` と `tako remote agents` にも載る（4 経路が 1 実装）。<br>**claude 以外は対象外**（codex の `remote-control` は自前ホストの app-server 経路で別物、agy には手段が無い = `tako agent-support` の `remote_control` の宣言が正）。**`remoteControlAtStartup` は tako が書かない**（ユーザー設定なので案内だけ）。取得の正は transcript の `bridge_status` 行の `url`、無ければ `bridge-session` 行の `bridgeSessionId` を `cse_` → `session_` 変換して組む（**アカウント UUID は保持しない**）。A/B は `TAKO_1068_LEGACY=1`（フラグを一度も付けない）。<br>**スマホ（リモート PWA）側の見え方（#1077）**: ペインのカードに「Claude で開く」が出るのは `state == connected` のときだけで、**PWA は URL を組み立てない**（daemon が返したものをそのまま開く）。繋がっていないペインは 1 行にたたまれ、タップで**理由 + 次の一手 + PC 側で有効化するコマンド**が開く（`remote_link.reason` / `next_step` / `enable_command`。文言は Rust 側が表示言語で解決するので tako 本体の言語設定と一致する。**環境側の阻害では opt-in コマンドを出さない** = 触っても直らない。master / solo は設定ファイルの置き場が別なので `--solo` を出し分ける）。**opt-in を書き換えるボタンはスマホに作らない**。アカウント名も一覧に出る（別アカウントでログインしていると会話が出ないので切り分けに使う）。**自前チャットはフォールバックとして残す**（非適格環境・codex / agy worker・公式が使えないときの唯一の対話手段。既定 view は chat のまま）。<br>**スマホから master を立てる（#1078）**: 一覧ヘッダの「+ master」→ プロファイル選択 → **`POST /api/tabs`（`TabNew` を dispatch）→ `POST /api/tabs/:id/master`** の 2 経路で新しいタブに master が立つ（**role は Manage**。タブとプロセスを作るので close / resize より強い。observe / interact は 403 で、PWA は押す前に理由を出す）。**組み立て（プロファイル検証 / system prompt / 起動コマンド / role の語彙）は `orchestrator::master_launch` が正**で CLI の `tako master` と同じ順・同じ検証を通る（CLI 不在・未登録プロファイルは**ペインを作る前に** 400）。起動後は `remote_link.state` が `connected` になるまで PWA が `/api/v2/panes` を見て、繋がったら公式リンクへ送り出す。opt-in していないプロファイルは**待たずに**理由へ切り替える（応答の `remote_control.state` = enabled / off / ineligible）。選択肢は `GET /api/master/profiles`（Observe。`tako orchestrator profiles list` と同じ 1 実装）。**app が拒否した要求（存在しないフォルダ等）で IPC 接続を捨てない**（400 で返す。捨てると次の正当な操作が 503 になる = A/B で実測） |
| オーケストレーター アカウント管理（#504/#548） | `tako orchestrator accounts list/show/add/remove`（既定の資格情報を使うアカウントは `add <名前> --inherit`。既定パスの明示指定は警告。MCP `tako_orchestrator_accounts` と 1:1） |
| worker spawn のレイアウト設定 | `tako orchestrator layout [--policy master-reserved\|legacy] [--master-ratio 0.5] [--algorithm grid\|spiral]`（全省略で現在値表示。#165） |
| build | `cargo build --workspace` |
| lint | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` |
| test | `cargo test --workspace` |
| リリース経路のモックテスト（#256 / #965） | `bash scripts/test-release-retry.sh`（ダミー gh を PATH に挿して `release.sh` を実走させる。リトライ・冪等・**両 OS の待ち合わせと片肺リリースの検出**を検証。本番の Release / タグには触らない。**CI の macOS ジョブで毎 PR 走る**） |
| **スマホからの master 起動の実経路テスト（#1078）** | `bash scripts/test-remote-master-launch.sh`（偽 tailscale CLI + 隔離 state / data / orchestrator で**実 daemon + 実 tako-app** を走らせ、HTTP を実際に叩いて role の gate・タブ作成・master 起動・壊れた指定・監査ログを実測。claude は同梱スタブへ向くので**実エージェントは起動しない**。**本番の tailscale / remote デーモン / tako 設定には触らない**） |
| リモート serve 自己検査のモックテスト（#1049） | `bash scripts/test-serve-watch.sh`（偽 tailscale CLI + 隔離 state で実 daemon を走らせ、「消える → 検知 → 張り直す」「系統の入れ替わり」「上限で断念」「ユーザー設定は触らない」を実測。**本番の tailscale / serve 設定 / remote デーモンには触らない**） |
| ファイルツリーフォルダ操作 | `tako tree add <path>` / `tako tree remove <path>` / `tako tree list`（AI がプロジェクトフォルダを明示追加。#134） |
| **Finder の「このアプリケーションで開く」（#708 / #835）** | Finder でファイルを右クリック → tako を選ぶと**新しいタブ**で開く。**ファイル = そのファイルだけが載ったタブ 1 枚**（タブ名 = ファイル名・プレビューのみで PTY なし）、**フォルダ = そのフォルダでシェルを起動したタブ**、存在しないパスは読み飛ばし。**複数選択は 1 ファイル = 1 タブ**で最後に開いたものが前に出る。既存のタブ・ペインには触らない。AI からの同等操作は `tako open <file> --new-tab` / MCP `tako_open_file` の `new_tab`（`--right` 等の分割方向とは排他）と `tako tab new --cwd <dir>` / MCP `tako_create_tab` の `cwd`。**既定アプリは奪わない**（`LSHandlerRank` は全宣言 `Alternate` 固定 = 候補に出るだけ。番犬テストが `build-app.sh` を読んで機械検証）。`.rs` / `.toml` 等は macOS が UTI を持たないため候補には出ない（`open -a tako <file>` なら開ける） |
| **リモートからフォルダを開く（SSH 先のワークスペース化。#919 / #65 / #976 / #1041）** | **ペインで `ssh <host>` に入るだけで、そのホストのフォルダがツリーへ並ぶ**（#976。明示操作は要らない。初期パスは sftp の初期 cwd）。ルートは**ローカルフォルダと同じ形**（フォルダ名 + フォルダアイコン）で並び、SSH であることと相手は行末のバッジ（`SSH <host>`）が示す。**自動検知ぶんはローカルの後ろ**（#976）／**明示的に開いた分はツリーの先頭**（#1041。ローカルより前）。**切断してもフォルダは消さず**バッジが「切断」へ変わる（右クリックの「再読み込み」で復帰）。取り違えの危険がある形（`-p` / `-J` / `-o Hostname=` / `ssh host <コマンド>` / `-N` 等）は**見送って理由を残す**。鍵・agent で入れない相手はパスワードを聞かずに見送る（「リモート接続…」で一度ログインすれば ControlMaster を共有して次から開く）。**走査はアイドル時に走らない**（材料は OSC 133 の状態とペインの子 pid で、指紋が動いたときと 60 秒の保険だけ。採取は #772 / #779 の ProcessSnapshot へ相乗り）。切替は 設定画面 → リモート / `tako remote-folder auto [on\|off]` / MCP の `action: "auto"`（既定 ON。**無効化すると走査対象が空になる**）。Windows はプロセスのコマンド行を採れないので自動検知は働かない（明示経路だけ）。<br>明示的に開くときは ファイルメニュー「リモートからフォルダを開く…」/ ⌘K パレット → ホスト選択 → フォルダ選択 → **ファイルツリーの先頭に SSH 先の構造が並ぶ**（ファイルはプレビューで開いて**編集・保存できる** = #966）。右クリックから パスコピー / このフォルダで SSH ペイン / 再読み込み / 閉じる。<br>**開いたら同じタブへターミナルも用意される（#1041）**: そのホストへ SSH 済み + そのフォルダへ `cd` 済みの**新しいペイン**が立つ（VSCode Remote / Zed 相当）。**同じホストへ繋がった生きたペインがあれば増やさない**（判定材料は #1010 の接続状態 + #976 の `ssh` 検知。失敗して死んだペインは数えないので開き直せる）。既定 ON で、抑止は `--no-terminal` / MCP の `terminal: false`。**既存ペインの乗っ取りはしない**（打ちかけの行を見分けられないため。ペインの SSH 化は右クリック「このペインでリモート接続…」/ `--target pane` の明示操作 = #1006）。繋いだ / 繋がなかった（理由つき）はサイドバー上部の通知と `open` 応答の `terminal` に出る。**復元時には繋がない**。<br>AI からは `tako remote-folder open <host> [path] [--no-terminal]` / `close [host] [path] [--all] [--tab N]`（既定は全タブ横断）/ `list`（**ツリーに出ている並び**で返り、各行の `origin` = explicit / auto と `placement` = leading / trailing が付く。#1041）/ `ls <host> [path]`（**ツリーを開かずに覗ける** = リモート構造の把握用）/ `open-file <host> <path>` / `ssh-pane <host> [path]` / `pending` / `push [host] [path] [--force]` / `auto [on|off]`（自動追加の状態・切替。検知した接続の生死と見送った理由も返る）。MCP `tako_remote_folder` と 1:1。<br>**バックエンドはシステムの `ssh` / `sftp` + ControlMaster**（`~/.ssh/config`・鍵・agent・known_hosts・2FA をそのまま使う。russh / ssh2 は ControlMaster に相乗りできないので採っていない）。`sftp -b -` なので**相手のログインシェルに依存しない**（PowerShell の Windows 相手でも動く）。対話 SSH ペインも**同じ ControlPath** を通るので、パスワード認証しか無い相手は「リモート接続」で一度ログインすれば以後ツリーが追加認証なしで開く。<br>失敗は 14 種別へ分類し**理由 + 次の一手 + 生の詳細**を出す（ツリーは行として / 接続はサイドバー上部の通知。**失敗は自動で消さない**）。状態は `list` の `state` で読める（`loaded` / `loading` / `pending` / `sidebar_closed` = ツリーが閉じている / `not_displayed` = 裏タブ / `error: <理由>`）。開いたフォルダは layout.json へ永続化し、残っていれば起動時にツリーが開く。<br>**切断したフォルダは回線が戻ると自動で live へ戻る（#1040）**: 切断中はバックオフ（5/10/20/40/60/120 秒・上限 8 回）で繋ぎ直しを試し、戻ったらツリーを読み直して**保留中の書き戻し（#966 の `pending`）も自動で push** する（`force` はしないので競合は既存の分類のまま）。手動の「再読み込み」は従来どおり残る。キャッシュは `<data_dir>/remote-cache/`、接続は `<data_dir>/ssh/`（どちらも #513 では共有しない）。<br>**読み込み中はツリーの行に回る弧が出る（#1010）**: GUI の「開く」は SFTP の取得を**背景**へ出す（CLI / MCP は同期のまま = #966 の切り分け）。いま取得中のものは `list` の `loading_files` で読める。<br>並び規則の正本は `tako_core::sidebar::remote_root_order`（GUI と `list` が同じ関数を通る）。A/B は `TAKO_1041_LEGACY=1`（並びを全部ローカルの後ろへ + 自動接続を切る）／`TAKO_976_LEGACY=1`（#919 の全部先頭 hoist へ） |
| **スマホからファイルを見る・直す（#1079 / #1084 / #1085）** | PWA の `#/files` から **PC のファイルツリーに現に出ているルートの配下だけ**を一覧・プレビュー・端末へ保存でき、テキストならその場で直して**PC へ書き戻せる**。SSH 先（`remote-folder open` 済み）も同じ 1 本の一覧に並ぶ（並びは #1041 の規則そのまま）。**読み出しでも Interact 以上**（画面の閲覧と持ち出しは危険度が別物）。<br>**書くのは daemon ではなく PC 側の編集経路**（`PreviewEdit` → `PreviewApply` → `PreviewSave`）: #966 の保証（アトミック・内容での競合検知・mode 復元・退避）を二重に持たない。**競合検知は 2 段**で、①スマホが読んだ → 保存を送った の窓は**検証子**（読み出しの応答に載る `etag` をそのまま返してもらう。スマホ側で計算させない）②適用した → 書いた の窓は PC 側。**検証子が無い / 合わない保存は書かずに 409**。Mac 側に未保存の編集があれば 409 `busy_editing`（書きかけを踏み潰さない）、テキストとして編集できない種別は 400 `not_text`（判定は app の `preview.mode`）。<br>**SSH 先は daemon が proxy するだけ**（SFTP を張るのは PC = **スマホは SSH 鍵に触らない**）。**`force` はスマホから出さない**ので、押し出せなければ 502 + `kind: "remote_pending"` で退避され（`GET /api/files/pending`）、回線が戻ったら `POST /api/files/push` で送り直せる。相手へ届かず**再取得もできない**ときは、読んだときのペインが残っていればそこへ適用して退避へ回す（残っていなければ理由を返して断る = 写しへ書くだけの「保存できた気になる」を作らない）。<br>**設計原則 5 との関係**: これは既存機能のリモート転送路で、編集そのものは `tako edit` / MCP `tako_preview_edit` 系から完全に操作できる（入口がペアリング済みの機器であって AI ではないので新しい MCP / CLI ツールは増やさない = `/api/upload` と同じ扱い）。認可・脅威モデルの正は `.agent/requirements.md` FR-6.15〜6.17 と `.agent/threat-model-remote.md` |
| **リモートファイルの編集・保存（#966 / #65）** | リモートのファイルはプレビューで開いてそのまま編集でき、保存で **SFTP の書き戻し**が走る（Zed / VSCode Remote 相当）。**保存は「リモートへ書けるまで」**で、ローカルの写しへ書けただけでは成功と言わない。<br>**アトミック**: 同じディレクトリへ一時ファイルを `put` → `rename` で被せる（`posix-rename` は既存を上書きする = Linux / Windows 実機で実測）。`put` は元の mode を引き継がないので、POSIX として読める mode なら `chmod` で戻す（**実行権が落ちない**）。<br>**競合検知**: 開いた時点のリモートの内容を持ち、書く前に実体と突き合わせる（**サイズと mtime ではなく内容そのもの**。`ls -la` の日時は分の分解能しかない）。変わっていたら上書きせず `conflict` を返す = 次の一手は「読み直して編集をやり直す」か「`push --force` で上書き」。相手から消えていれば `not_found` で止まる。<br>**押し出せなかった保存は退避される**（切断中の保存が無言で消えない）: `tako remote-folder pending` で一覧、`push [--force]` で再試行。退避は `<data_dir>/remote-cache/pending/`。<br>**GUI の ⌘S はローカルへ同期・リモートへは背景**（1 バッチ 1〜2 秒の実測なので UI スレッドで待たない。ヘッダのチップに「リモートへ保存しています…」/「送れていません」が出る）。**CLI / MCP は同期**で、応答の `remote.state`（idle / uploading / saved / failed / pending = 前のセッションの退避が残っている）と `remote.pending_write` で結果が読める。<br>書けないファイル（mode のどこにも `w` が無い）は読み取り専用のまま。リモートの編集は**自動保存が既定 OFF**（1 保存 = SFTP 3 バッチ）。A/B は `TAKO_966_LEGACY=1`（段階 1 の読み取り専用へ戻す） |
| **リモート公開の自己検査と自動復旧（#1049）** | `tako remote status` / MCP `tako_remote_status` は `running`（プロセスの生死）だけでなく **`serve_ok`（tailnet から本当に見えるか）** を返す。daemon は「どの tailscaled（= どのノード）へ serve を張ったか」を固定し、30 秒ごとに**公開した URL のノードへ**問い合わせて消えていれば張り直す（上限 5 回。連続 10 回健全なら予算が戻る）。**奪い合いはしない**: tako 形状でも応答している相手なら譲り（`serve_state=taken_over`）、ユーザーが張った設定は触らない（`foreign`）。直せないときは `degraded`（理由 + 次の一手）を出し `warnings` 経由で GUI のリモートカードにも届く。**既定探索は相手が入れ替わる**（実測 #1049: GUI 版 Tailscale の LocalAPI 発見ファイル `/Library/Tailscale/ipnport` が作り直された瞬間に standalone → GUI 版へ切り替わり、**serve は消えていないのに `No serve config` に見えた**）ので、固定済みでも毎周期ノード名を照合する。後始末（daemon 終了 / `tako remote stop`）も同じ相手を通るので本物の設定を消し残さない。serve の増減は `<state_dir>/audit.log` へ **pid + 実行ファイルつき**で残る（`serve_set` / `serve_off` / `serve_reasserted` / `serve_reassert_gave_up` / `serve_taken_over` / `serve_node_switch` / `serve_node_missing`）。A/B は `TAKO_1049_LEGACY=1`（自己検査もノード固定もしない）、検証用の間隔上書きは `TAKO_1049_WATCH_SECS` |
| プレビュー目次操作 | `tako preview-outline [--pane N] [--item N]`（Markdown / PDF 目次の一覧・1 始まり項目ジャンプ。MCP `tako_preview_outline` と 1:1。#232） |
| プレビュー内リンク（#680 / #271） | `tako preview-link-list`（Markdown の `[text](url)` / PDF 注釈リンクを一覧。応答の `kind` が `markdown` / `pdf`）/ `tako preview-follow-link <index>`（URL は OS 既定ブラウザ。**http / https のみ**開き、`javascript:` / 相対パス / アンカーは拒否。PDF の内部リンクはページジャンプ）。GUI は ⌘+ホバーで下線 + ⌘+クリックで同じ経路。MCP `tako_preview_link_list` / `tako_preview_follow_link` と 1:1 |
| Markdown コードブロックのコピー（#680） | `tako preview-copy-code [index]`（装飾なしの全文をクリップボードへ。index は出現順 0 始まり・省略で先頭。GUI はブロック右上のコピーボタンと同一経路。MCP `tako_preview_copy_code` と 1:1） |
| プレビューライブリロード | `tako preview-reload [on\|off]`（引数なしで現在値。既定 ON・settings.json 永続化・MCP `tako_preview_reload` と 1:1。#233） |
| プレビュー画像キャッシュ | `tako preview-cache [max_mb]`（引数なしで上限・使用量・件数。既定 512MiB、256〜8192MiB、settings.json 永続化・MCP `tako_preview_cache` と 1:1。#258） |
| 設定の自動マイグレーション（#916） | `tako migrate [status\|run] [--schema <種別>]`（引数なしで全永続ファイルの形式を確認するだけ = 何も書き換えない。`run` で当てる）。**普段は呼ぶ必要がない**: `tako setup` 実行時と GUI / master / CLI の起動時に自動で当たる。旧内容は `.pre-v<N>.bak`、解釈できない内容は `.unreadable.bak` へ退避されるので消えない。冪等（何度流しても壊れない）。応答の `files[].state` が `unreadable` のものは「設定が壊れているので既定値で動いている」という意味（元の内容は退避先に残る）。`status` のときは書き換えていないので `backup_planned` / `quarantine_planned` というキー名になる。MCP `tako_migrate` と 1:1 |
| 受け入れゲート（#244 / #935） | `tako task gate set <task_id> --command "cmd" [--pr-merged N] [--custom "desc"]` / `tako task gate check <task_id>` / `tako task gate show <task_id>`（MCP `tako_task_gate` / `tako_task_gate_check` / `tako_task_gate_show` と 1:1）。<br>**コマンド型の述語はそのマシンの既定シェルの構文で書く（#935）**: 述語は「1 本の文字列」として抽象境界 B1（`tako_core::platform::shell::output_command`）へ渡り、**macOS = `sh -c` / Windows = PowerShell**（`-EncodedCommand`）で走る。方言は `tako_core::platform::shell::script_dialect()` が正で、`true` / `false` / `pwd` のような**POSIX 固有の語を Windows で使わない**（PowerShell に `true` / `false` は無く、`pwd` は表として整形されてパスが切られる = 実測）。#935 前は `sh -c` 決め打ちだったので Windows ではどの述語も「コマンド実行に失敗」で判定不能だった（登録と表示だけが動くので**ゲートが無いのではなく常に失敗する**形で現れる）。**終了コードは実際の値がそのまま `evidence` に載る**（`-EncodedCommand` は `$LASTEXITCODE` を素通ししないので明示 `exit` で返す）し、**非 ASCII の出力も化けない**（PowerShell 5.1 の既定コードページ = 日本語環境では CP932 を UTF-8 へ寄せる） |
| git ブランチ操作（#496） | `tako git checkout <branch>` / `branch <name> [--from <ref>] [--no-checkout]` / `merge <branch> [--no-ff]` / `abort` / `conflicts`（checkout・merge は既定で**実行せず**「何が起きるか」を出す。`--yes` で実行。MCP `tako_git_checkout` / `tako_git_branch_create` / `tako_git_merge` / `tako_git_merge_abort` / `tako_git_conflicts` と 1:1） |
| コンフリクト解消エージェント（#496） | `tako git resolve [--agent claude\|codex\|agy] [--tab N]`（同じタブにペインを立て、リポジトリ・未解決ファイル・マージ元/先を含む解消プロンプトを自動投入。文面は `<data_dir>/orchestrator/conflict-resolver.md` で差し替え可。MCP `tako_git_resolve_agent` と 1:1） |
| Web ビューペイン操作 | `tako web open <url>` / `list` / `show <id>` / `hide` / `close` / `nav <to>` / `eval <js>` / `eval-result <token>` / `read`（ネイティブ WKWebView ペイン。#155） |
| 複数ウィンドウ操作（ビューポート方式 + 共有タブバー。#339/#380） | `tako window list` / `new [--tab N]` / `close <W>` / `move-tab --tab N --window W` / `focus <W>`（タブバーは全ウィンドウ共通で全タブ表示、クリックで表示がそのウィンドウへ移る。MCP `tako_window` と 1:1） |
| **シェル統合（cwd 追従・コマンド状態。#525）** | `tako shell-integration [status\|install\|uninstall]`（引数なしで状態。unix は環境変数の注入だけで完結するので操作不要、**Windows は `install` で PowerShell 7 と 5.1 両方の `$PROFILE` へマーカー付きブロックを 1 個置く**（冪等。`uninstall` で元のバイト列へ完全復帰）。**配置できても効かない場合がある**: 器が psmux だと OSC を外へ通さない（実測）ので応答の `effective` が false になり `blocked_by_backend` に理由が入る = `TAKO_BACKEND=none` が要る。MCP `tako_shell_integration` と 1:1） |
| エージェント共通ルール同期 | `tako agents sync-rules` / `tako agents status`（正本から各エージェントのグローバル指示ファイルへマーカーブロック同期。#136） |
| AI 系設定のデバイス間共有（#513） | `tako config`（引数なしで状態と差分）/ `init [--path P] [--remote URL]` / `link <パス\|URL>` / `push [-m msg]` / `pull` / `list`（何を共有し何を共有しないかの分類表）。claude のグローバル指示（CLAUDE.md / snippets / commands / templates）+ tako の宣言的設定（profiles / projects / accounts / local-rules / settings）を git 1 本で mac ⇔ Windows 共有。秘匿情報とマシンローカル状態はホワイトリストで構造的に除外、未分類は共有しない。絶対パスはホーム部分が `~` に正規化される。MCP `tako_config_share` と 1:1 |
| レイアウト復旧（タブ・ペイン消失時。#177/#381/#770） | `tako recover`（バックアップ世代一覧）→ tako 終了 → `tako recover --apply <世代>`（1〜3 または `good` = 最後に復元へ成功した良品）→ tako 再起動。実体 tmux セッションの個別取り込みは `tako tmux open --socket tako --pane <N> <session>`。**世代は「何かを失う保存」の直前に作られる**: ペイン数の半減（#177）に加えて、**tmux セッションを持つペインが消える保存**（#770 のタブ close）。健全世代の押し出し防止で 10 分に 1 回まで |
| **何がいつ消えたかを調べる（#770）** | `<data_dir>/persist.log` に発生源つきで残る: `セッション kill: pane=… session=…（発生源 close:gui-tab）` / `タブ close: tab=… ペイン N / セッション kill M（発生源 …）`。復元・quit の記録と同じファイルなので「再起動で消えた」のか「明示 close で消えた」のかをここだけで切り分けられる（発生源は `close:gui-tab` = タブの × / `close:gui` = ペインの × / `close:kbd` = cmd+W / `close:dispatch(cli\|mcp, caller=…)` = CLI・MCP / `exit` = プロセス死。**quit と PTY 死亡ではセッションを kill しない**） |
| セッションカタログ（会話の発見・復元。#112 / #1069） | `tako sessions list [--role r] [--project p]` / `tako sessions show <id>` / `tako sessions resume <id>`（記録 cwd で `claude --resume` をペイン起動。claude のみ）/ `tako sessions link [--pane N \| --id <id>]`（Claude 公式 Remote Control の session URL。上の Remote Control 行を参照） |
| ペインの平文ログ（ペイン死亡後も出力を遡る。#112） | `tako logs list` / `tako logs show <pane> [--session <id>] [--lines N]` / `tako logs status` / `tako logs set --enabled --max-mb --total-max-mb` |
| スリープ防止 | `tako sleep-guard status` / `tako sleep-guard set --mode <off\|on\|while-agents-running> --power-condition <ac-only\|always>`（IOKit 電源アサーション。#173） |
| **会話を引き継いだセッション再起動（ペインの右クリック。#1067）** | `tako session-restart [--mode harness\|handoff] [--pane N]`（**引数なしで下見** = 何ができるか + できない理由だけを返し何も起こさない。MCP `tako_session_restart` と 1:1）。<br>**`--mode harness` = 会話をそのまま保って CLI プロセスだけ建て直す**: 旧プロセスへ SIGTERM → **落ちたのを確かめてから** `claude --resume <会話 ID>` を #640 の送達確認つき経路で投入する。アカウント（`CLAUDE_CONFIG_DIR`）・role・モデル・effort も元のまま復元される。claude CLI の自動更新後にプロセスが旧版のまま残っている（#498 の stale 警告）ときの解決手段。**会話 ID を解決できないときはプロセスに触らずに断る**（resume 先が分からないまま殺すと会話を失う）。解決は 生きた claude（agents 経由）→ セッションカタログ の順で、終了直後に claude 自身が画面へ出す `Resume this session with: claude --resume <id>`（実測 2.1.258）を拾えたらそちらを権威として差し替える。<br>**`--mode handoff` = 引き継ぎを書かせてセッション交代**（#749 の自動ハンドオフの手動版。**master ペインのみ**）: tako が引き継ぎファイルを読んで後任を立てるのではなく、**エージェント自身へ**「引き継ぎを書き直す → `tako_orchestrator_handoff` を呼ぶ」を依頼する（文面は #749 と同じ 1 実装で見出しだけ違う）。<br>**メニューに出るかは構造だけで決まる**（セッションの有無・role・agent 系統・handoff は master のみ）。**生成中 / キュー滞留 / 入力欄の下書き / 選択肢ダイアログ中は実行時に理由 + 次の一手つきで断る**（一時的な状態でメニューから消すと機能を見つけられなくなるため）。**生成中の判定に `is_busy` を使わない**（完了行 `✻ Brewed for 2s · done` を busy と読むのでアイドルなペインが永久に対象外になる = 実測）。<br>**#498 の張り直しボタン（`tako stale-binary restart`）も同一実装へ寄せた**（会話を保つ harness を優先し、会話 ID を解決できないときだけ handoff へ落ちる）。**claude 以外は対象外**（手段は上流に在るが tako 未配線 = `tako agent-support` の `session_restart_harness` / `_handoff`）。**Windows はハーネス更新だけ使えない**（プロセスの終了要求が未実装 = 境界 B5。引き継ぎ再起動は使える） |
| **リミット後の自動復帰（ペイン単位。#813）** | `tako limit-resume [on\|off] [--pane N] [--all]`（引数なしで現在値。既定 OFF・layout.json 永続化・**ペインの右クリック**からも切替）。有効なペインが 5h / 週次上限で止まると、リセット時刻（画面の `reset at …` 由来）+ 数分を過ぎたところで tako が再開させる。**上限対処ダイアログなら「解除まで待つ」相当をラベル一致で確定**（`Upgrade …` / `Continue with usage credits` のような課金・モデル変更は拒否リストで構造的に選ばない。安全な選択肢が無ければ何もしない）、**ダイアログが無ければ継続ナッジを送達**（#32/#790 の確認つき経路）。permission ダイアログ・API エラー・通常の idle・**人間の下書きが入力欄にある**ときは発動せず、画面が動いているあいだも触らない。試行は 1 回の上限あたり 3 回で打ち切り。記録は `<data_dir>/supervisor.log` の `action=limit_autoresume`。状態は `tako list` / `read` / `worker_status` にも載る。MCP `tako_limit_resume` と 1:1。**プロファイル既定（#822）**: `tako orchestrator profiles set <名前> --limit-resume true` にすると**そのプロファイルから spawn した worker が最初から有効**になる（解決順は spawn 引数 → プロファイル → 無効。`tako orchestrator spawn --limit-resume false` はプロファイル ON をその worker だけ打ち消す明示 OFF）。適用結果は spawn 応答 / `orchestrator workers` の `limit_resume` で読める。solo プロファイルは worker を spawn しないので効かず警告が出る。<br>**codex は解除時刻を構造化データから採る（#985）**: rollout の `rate_limits.<枠>.resets_at`（epoch 秒）を読むので、画面の文言や表示書式・タイムゾーンに依存しない。画面側も codex 0.150.1 の日付つき表記（`Try again at Aug 28th, 2026 4:24 AM.`）を読めるようにした（**#985 前はこの形が読めず、不明のときの猶予 900 秒で上限が解けるずっと前に撃ち始め、3 回で諦めていた** = 朝まで止まったまま）。codex の上限ダイアログは「待つ」出口を持たない（増枠申請・購入・獲得済みリセットの引き換えだけ）ので**何も選ばない**（`Yes, use reset` / `Request increase?` / `Get More AI Credits` は拒否リストで構造排除）。agy は前払いクレジット制で枠もリセット時刻も無いため対象外。A/B は `TAKO_985_LEGACY=1`。<br>**上限の文言は 1 箇所で持つ（#1093）**: 停止判定とステータスバーのメーターが同じ規則（`tako_core::limit_resume::is_limit_exhausted_line`）を通る。受けるのは claude の見出し**テンプレート** `You've hit your <限度の名前>limit…` で、限度の名前は `session limit`（5h）/ `weekly limit` / `Opus limit` / `Sonnet limit` / `Fable limit` / `usage credit limit` / 各種 spend limit / `usage limit` / `limit` の**すべてが `limit` で終わる**（claude 2.1.258 のバイナリ内テンプレートと `nF` 表で実測。版で名前が増えても規則で追従する）。**`limit reached, now using …` は除く**（自動モデル切替の告知で worker は止まらない）。**#1093 前は `hit your usage limit` 決め打ちだった**ので、組織クレジット上限の`You've hit your session limit · resets 7:50pm (Asia/Tokyo)` に 1 文字も当たらず、解除後 1 時間以上 worker が止まったまま `supervisor.log` に記録も残らなかった。**ステータスバーは上限中に `--` にしない**（フッターが `5h NN%` を出さなくなるので見出しから枠を読んで 100% を埋める。フッターに数値があればそちらが正・枠へ対応づけられない上限は `--` のまま）。A/B は `TAKO_1093_LEGACY=1`。<br>**受ける告知は claude 自身の分類に合わせる（#1096）**: 同バイナリは前置きを阻害（`dCt` / `pCt`）/ 警告（`fCt` = `You've used` / `You're close to`）/ 情報（`mCt` = `You're now using usage credits` 等）に分けている。tako が受けるのは**阻害のうち時間で解けるぶんだけ** = 動詞 2 種（`hit your` / `reached your`）+ `out of usage credits` + `org is out of usage`。**座席種別・組織の無効化・`group's usage limit is set to $0` は受けない**（時間で解けないので「解除まで待つ」が嘘になる）。**警告 / 情報も受けない**（まだ動けるペインへナッジを撃たない）。<br>**解除時刻は日付つきを絶対時刻として読む（#1096）**: claude は 24 時間より先だと日付を前置きする（`Sep 8, 3pm` / `Sep 8, 3:05pm` / 年が違えば `Sep 8, 2027, 3:05pm`。codex は `Aug 28th, 2026 4:24 AM`）ので、**週枠（最大 7 日先）は日付つきが通常形**。#1096 前は時刻だけを読んで「次に来る同じ時刻」へ丸め、解除の数日前から撃って 3 回で諦めていた（#985 のテストがこの丸めを 13 / 13 / 19 日早い期待値で固定していた）。日付なしの表記は従来どおり（24 時間上限も維持）で、**日付が書いてあるのに読めない形は時刻だけの解釈へ落とさず「不明」にする**。A/B は `TAKO_1096_LEGACY=1` |
| 入力予測（tako 内 zsh のゴースト予測。#600/#614） | `tako autosuggest [on\|off]`（引数なしで現在値。既定 ON・右矢印か Tab で確定・settings.json 永続化・稼働中ペインにも次のプロンプトから反映。MCP `tako_autosuggest` と 1:1。同梱 zsh-autosuggestions を ZDOTDIR 経路で tako 内の zsh にだけ読み込ませるので `~/.zshrc` と外の zsh は不変）<br>`tako autosuggest hint [on\|off]` = 確定キーの案内（ゴースト直後に薄く出るチュートリアル。既定 10 回で消える）／`tako autosuggest tab [on\|off]` = ゴースト表示中だけ Tab を確定にする（#614） |
| UI テーマ切替 | `tako theme [dark\|light\|toggle]`（引数なしで現在値。settings.json 永続化・GUI 即時反映。タブバー右のボタン / MCP `tako_theme` と 1:1。#217） |
| UI 表示モード切替（GUI ライク表示。#691/#694/#702/#715/#716/#720/#725/#737/#739） | `tako ui-mode [gui\|terminal\|toggle]`（引数なしで現在値。既定 terminal = 従来の表示。gui ではアイドルなシェルのペインが「AI チームに任せる / AI と 1 対 1 で話す / コマンド入力へ」の 3 ボタン + 下部に「初期設定をやり直す（`tako setup`）」の控えめなリンクになり、**claude 対話ペインは会話ビュー**になる。**起動カードの右端の ▾ でプロファイルを選べる**（#739。選択肢が 2 つ以上のときだけ出て、選ぶと `tako master -<名前>` が入る。カード本体は既定起動のまま = #322 の最簡形。各項目に担当プロジェクト / 起動フォルダ / モデルの手がかりが付く）。settings.json 永続化・全ウィンドウ即時反映。タブバーのテーマボタン左隣 / ⌘K パレット / MCP `tako_ui_mode` と 1:1）<br>**ペインを作った直後・エージェントを起動した直後は「準備中…」で覆う**（#720。direnv のロードログや起動途中の画面を見せない。上限つきなので確定しなければ通常表示へ落ちる。ヘッダの「ターミナルを表示」でいつでも中身へ抜けられる。`tako run` のようにコマンド付きで作るペインは覆わない）。`tako ui-mode` の応答 `pane_display` が**いま各ペインに何が出ているか**（`terminal` / `starter` / `chat` / `preparing`）を返すので、AI は画面の状態をそのまま確認できる<br>会話ビューの中身: モデル名・状態・コンテキスト残量バー（**80% 超で警告色 + 「/compact で会話を軽くする」の押せるヒント**。#739）・md 描画・ツール / 思考の折りたたみ・下端追従 + **user / assistant とも枠付きブロック**（#737。発話の境界とコピー対象が一目で分かる）+ **生成中は会話末尾の AI 側に作業中インジケータ**（#737。TUI のスピナー行 = 作業内容 + 経過時間 + 受信トークン数。終わると本文に置き換わる）+ **生成中に送った指示もすぐ自分の吹き出しで見える**（#737。transcript の `queue-operation` を読むので配送前から出て、配送後も二重化しない）+ **入力欄** + **スラッシュボタン 3 つ**（/compact・/clear（確認つき）・/help）+ **承認カード**（画面に permission ダイアログが実在するときだけ。押下は `tako orchestrator respond` と同経路）+ **コマンド提案カード（#666）のインライン表示**（md コードブロック風 = 背景パネル + 等幅 + コピー / 新規ペイン実行。ターミナル表示では従来の帯 #703）。画像添付・システム通知は正規化層で分類され生 XML は出ない（#715）<br>**入力欄は claude TUI の入力行のミラー（#718/#719）**: ローカル下書きを持たず打鍵は PTY へ素通しなので、Enter / Shift+Enter・IME・画像ペースト（⌘V = Ctrl+V 素通しで TUI が `[Image #N]` を挿入）・ゴースト提案が TUI と完全に一致し、表示モードを往復してもズレない。箱の高さは TUI の入力行数に追従し（1 行なら 1 行ぶん）8 行で頭打ち。**worker ペインにも入力欄が出る**（直接指示可）<br>**入力欄に見える文字列は常にミラー 1 本（#737）**: TUI が箱の中に自前の案内文（空欄時の `Try "…"` / キュー滞留時の `Press up to edit queued messages`）を描いているときは tako のプレースホルダを重ねない（重なって読めなくなっていた回帰の根治）。**IME の未確定文字列と変換候補ウィンドウは入力欄のキャレット位置**に出る（チャット表示はターミナルグリッドを描かないので、セル座標のアンカーは画面上のどこも指していなかった）<br>**会話本文はドラッグで選択でき ⌘C / ⌘A が効く（#725）**: 選択はプレビューと同じ実 shaping 座標系で、**複数の発話にまたがって**掃ける。発話の右にはコピーボタン（画面と同じプレーンテキストで全文。折りたたみ中でも全文）、md コードブロックには #680 と同じコピーボタンが出る<br>`tako ui-mode release [--pane N]` = そのペインだけターミナル表示へ（揮発。`restore` で戻す）。**表示レイヤだけの切替なので PTY・tmux セッション・実行中プロセスには影響しない** |
| チャット本文のコピー（#725） | `tako chat copy [--pane N] [--message N] [--code K] [--markdown] [--list]`（UI のコピーボタンと同じ経路。`--message` 省略で**最後の assistant 発話**、`--list` で添字・role・文字数・コードブロック数の下見、`--code` でその発話のコードブロックだけ、`--markdown` で md ソースをそのまま。既定は画面と同じプレーンテキスト。MCP `tako_chat_copy` と 1:1） |
| プラットフォーム対応マトリクス（#515 / #591） | `tako platform [--platform macos\|windows] [--status pending] [--known-limitations] [--json]`（この環境でどの機能が使える / 縮退 / 未実装かを表示。`--known-limitations` はリリースノート用の日英併記 markdown を出力（#594）。GUI 不要のローカル処理。MCP `tako_platform` と 1:1）。<br>**Windows の判定には実測根拠が必須（#591）**: `Feature::windows_evidence` に「実機セルフテストの項目 / 実機で緑のテスト名 / 実測の記録」のどれかを書く。書かずに `Supported` / `Degraded` / `Unsupported` へ倒すと **T7 が落ちる**（未実測なら `Pending` + `notes::WIN_UNVERIFIED` + 追跡 #937 のまま置く）。宣言は `PlatformFacts` 経由で master / solo / setup の system prompt へ流れる（#516）ので、**過大申告はエージェントを誤らせ、過小申告は使える機能を回避させる** |
| **codex の利用制限データ（#357 / #985）** | ステータスバーの codex 表示は **rollout の構造化データ**（`$CODEX_HOME/sessions/**/rollout-*.jsonl` の `token_count.rate_limits`）から採る。`primary` = 5h 枠 / `secondary` = 週枠で、`used_percent` は数値・`resets_at` は epoch 秒（有料プランで実測）。**#357 の画面スクレイピング（`primary NN%`）は codex 0.150.1 では成立しない**（実測: TUI のフッターはモデル名と cwd だけで、`5h limit: [██…] 90% left (resets 23:23)` は `/status` のモーダルの中にしか出ない）ので、構造化ソースが正・スクレイピングは旧版への後方互換。読むのは **background の 60 秒に 1 回**で、#772 / #779 の `ProcessSnapshot` へ相乗りするのでプロセス起動は増えない。AI からは `tako orchestrator worker-status` / MCP `tako_orchestrator_worker_status` の `rate_limits`（`limited` / `reset_at` つき）で読める |
| **agent 能力マトリクス（#982）** | `tako agent-support [--agent claude\|codex\|agy\|local] [--status supported\|degraded\|pending\|unsupported] [--json]`（**どのエージェント CLI でどこまで claude と同じことができるか**を表示。引数なしで全系統の表、`--agent` で理由と根拠つきの一覧。GUI 不要のローカル処理。MCP `tako_agent_support` と 1:1）。<br>**判定には根拠が必須（#982）**: `AgentEvidence` に「コード本文の引用 / 上流 CLI の仕様 / 実測の記録 / 緑のテスト名」のどれかを書く。書かずに claude 以外を `Supported` / `Degraded` / `Unsupported` へ倒すと **`t7_claude以外の判定には根拠が要る` が落ちる**（未調査なら `Pending` + 追跡 Issue のまま置く）。**「上流に手段が無い（`Unsupported`）」と 「まだ調べていない（`Pending`）」を混ぜない** = 未調査を `Unsupported` へ倒すと、宣言を読むエージェントが open な道を永久に避ける。docs は生成物（`node scripts/gen-agent-support-docs.mjs`。CI が `--check`）。<br>**claude が最弱になる能力は 1 マスで表せない**（`claudeは基準系なので全て対応済み` テストが claude = 全 Supported を強制する）。#1002 のモデル一覧はこれに当たったので、能力を「setup でモデルを選べるか」（3 系統とも Supported）へ切り出し、**取得手段の差は実行時の `agent_models` 側**（`source` = cli / builtin / none と `failure.kind`）で表した |
| Windows 対応状況ページの生成（#591） | `cargo build -p tako-cli && node scripts/gen-windows-support-docs.mjs`（`docs/src/content/docs/windows-support.md` は**生成物**。`--check` で同期検査し CI の macOS ジョブが実行する。カテゴリ未分類の機能があると生成が失敗するので、機能追加時の分類漏れもここで落ちる） |
| **Windows のウインドウを実測する（#1063）** | `pwsh -File scripts/windows/measure-window.ps1 -TakoPid <pid> [-Png out.png] [-Maximize] [-ClickX N -ClickY N]`（対話デスクトップ = session 1 で走らせる）。**素の `powershell.exe` は DPI 非認識**なので、自前スクリプトで測ると `GetWindowRect` / `SetCursorPos` は**物理 ÷ 表示倍率**なのに `CopyFromScreen` は**物理のまま**返り、健全なウインドウが「1.25 倍あふれている」ように見える（実測: 125% で 1938x1038 のウインドウが 1550x830 に見え、スクリーンショットは縮小ではなく**クロップ**になって右 1/5 と下 1/5 が消える。クリックも同じ倍率でずれる = #1063 の正体）。この道具は PerMonitorV2 を宣言し、**宣言が効かなければ実行を拒否する**（誤解を招く証拠を出さない）。tako 側の実測値は MCP `tako_check_health` の `dpi_awareness` / `dpi_awareness_expected` からも読める（起動時に期待から外れていれば persist.log にも 1 行残る）。マニフェストが落ちると Windows は無言で DPI 非認識になるので、番犬 `windows_dpi_manifest_watchdog`（macOS でも走る）+ セルフテスト項目 139（実機）で二段に見張っている |
| 右パネルのビュー切替 | `tako panel --show --view <fleet\|orch\|git>`（値は GUI のタブ表示名と同じ。orch = master + ワーカーツリーの俯瞰。`tmux` は fleet の旧称で後方互換のみ受理。#217/#553） |
| Code Runner でファイル実行（#453） | `tako run <file> [--profile <name>]`（ファイル内 `tako:run:` 宣言 or 拡張子既定で新ペイン分割実行。`--list` でプロファイル一覧、`--wait` で完了待ち。MCP `tako_run` / `tako_run_resolve` / `tako_run_defaults` と 1:1） |
| 拡張子既定コマンド設定 | `tako run-default [ext] [command]`（引数なし = 一覧。`--remove` で削除。MCP `tako_run_defaults` と 1:1） |
| AI コマンド提案カード（#666/#703） | `tako show-command <コマンド>...`（**ターミナル領域を縮めて作った専用帯**にコピー / 新規ペイン実行つきのカードを出す = 会話・入力欄・フッターと重ならない。`--label` で説明、`--pane` で対象、`--list` / `--copy` / `--run` / `--dismiss` でカード操作、`--card` / `--index` で対象指定）。**AI が会話本文に書いたコマンドは TUI の物理改行でコピーが壊れる**ため、実行を頼むコマンドはこれで提示する。カードは揮発（永続化しない）。MCP `tako_show_command` と 1:1 |
| 設定画面（#459/#721） | `tako settings [--tab <名>]`（Cmd+, / パレット / MCP `tako_settings`。独立ウィンドウで一般・外観・Code Runner・**プロファイル**・セットアップ・スリープ防止・リモート・高度の 8 タブ。`--tab profiles` で master / solo の起動プロファイルをフォーム編集 = `tako orchestrator profiles` と同じ dispatch） |
| 初回起動バナー（#549） | `tako welcome [show\|dismiss]`（引数なしで表示状態 + 案内コマンド。初回起動時だけ `tako setup` → `tako master` の導線をタブバー直下に出す。⌘K パレットにも同じ 3 項目が常設。MCP `tako_welcome` と 1:1） |
| アプリ内更新（#36/#403/#616/#690/#1042） | `tako update [status\|check\|apply\|apply-zip\|repair]` に加え、`tako update open` = 専用画面（設定画面と同じ独立ウィンドウ。現在 / 最新バージョン・チャンネル・配布系統・配布物・リリースノート・「今すぐ更新」+ 更新フロー全状態）、`tako update card [dismiss\|show]` = 上部通知カード（引数なしで状態。× で閉じるとそのバージョンは以後通知しない）。**下部ステータスバーには一切出さない**（#616）。リリースノートは **Markdown レンダリング**（見出し / 表 / リスト / コード / 引用。リンクは ⌘+クリックで既定ブラウザ = http / https のみ）で、描画はプレビューペインと同じ `md_view`（#690）。⌘K パレット「アップデートを開く」/ MCP `tako_update` と 1:1 |
| target 掃除 | `scripts/clean-target.sh`（dry-run。`--run` で実行。cargo clean + worktree prune） |

CI（`.github/workflows/ci.yml`）は macOS / Windows の両ランナーで build + test を回す。

## AI 向け詳細仕様（必要なときだけ Read する）

- コンセプト・競合・Non-goals: `.agent/concept.md`
- 機能要件（FR / NFR）: `.agent/requirements.md`
- 技術設計・リスク・3 層制御プレーン: `.agent/architecture.md`
- 規約（命名・エラー・ログ）: `.agent/conventions.md`
- agent 種別 enum の対応表（統合しない理由・寄せ先一覧）: `.agent/agent-enums.md`
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

### 両 OS 同時リリース（#965）

**リリース 1 回で macOS / Windows の配布物が揃うのが正常な状態**。片方だけ出ると、
欠けた OS の利用者には「更新が無い」ように見えたままバージョンだけが進む
（更新チェックは自 OS 向けアセットの有無で判定する = #595）。

- 生成場所: macOS = ローカル（`scripts/release.sh`）/ Windows = **CI の windows ランナー**
  （`.github/workflows/release-windows.yml`。タグ push で起動し、同じ Release へ添付する）。
  実機依存を避けるためこの順で、実機経路（`installer/windows/release-windows.ps1`）は
  CI が使えないときの代替として残す。配布物の検査は
  `installer/windows/lib/verify-assets.ps1` の 1 実装を両経路が共有する
- 待ち合わせ: `release.sh` は Windows の添付を待ってから、実アセットを読み直して
  ノートを作り直す（ダウンロード表 / 動作要件 / Windows 手順 / Known limitations が揃う）
- 片肺の検出: `release.sh` の終了コード **3**（= Release は作られたが揃っていない）。
  公開済みリリースは `scripts/release.sh --check-assets [tag]` でいつでも検査できる。
  判定の正は `tako-core::platform::release_assets`（`missing_platforms` / `is_complete`）で、
  シェル側の写しは同期テストが拘束する。モックテスト `scripts/test-release-retry.sh` は
  **CI の macOS ジョブで毎 PR 走る**（片肺の検出が壊れたらそこで落ちる）
- 動作要件の数値（macOS 11.0 / Windows 10.0.17763）も `release_assets` が正で、
  `tako.iss` の `MinVersion` と `build-app.sh` の `LSMinimumSystemVersion` との一致を
  テストが検証する（ノートの要件と配布物の実際の下限がズレない）

### 夜間リリース（自動。#166 / #1005）

- `scripts/nightly-release.sh` が launchd（`com.takushio.tako-nightly-release`、毎日 5:00）から
  実行され、前回タグ以降に main へ変更があった夜だけ自動リリースする
  （version bump → CHANGELOG 自動節 → コミット → annotated tag → release.sh でバイナリ付き
  GitHub Release）。クラウドルーチンでの夜間リリースはバイナリを作れず廃止した（経緯は #166）
- 自動スキップ条件: 変更なし / worktree dirty / 手動リリース進行中（Cargo.toml version ≠ 最新タグ）/
  プレリリース版 / 多重起動。ログは `~/.claude-orchestrator/logs/tako-nightly-release.log`
- ジョブ登録は `scripts/nightly-release.sh --install-launchd`（解除は `--uninstall-launchd`、
  確認は `launchctl list | grep tako-nightly`）。plist はリポに置かず実行時に生成する
- Homebrew cask 更新・リリースノートの日英併記は従来どおり手動で行う

#### 次回バージョンの予約（#1005）

**版数は既定で patch bump**。節目の minor / major を夜間発火に乗せたいときだけ予約する
（Cargo.toml を先に上げると「≠ 最新タグ = 手動リリース進行中」でスキップされるため、
版数の指定は**リポジトリの外**の状態ファイルで持つ）。

| 操作 | コマンド |
|---|---|
| 予約する | `scripts/nightly-release.sh --reserve 0.8.0` |
| 確認する | `scripts/nightly-release.sh --reserve`（引数なし） |
| 取消する | `scripts/nightly-release.sh --unreserve` |

- 正本は `scripts/lib/nightly-reserve.sh`（読み書き・検証・版種判定の 1 実装）。
  予約ファイルは `~/.claude-orchestrator/state/tako-nightly-next-version`
  （ログ / ロックと同じ置き場。**リポジトリの外**なので worktree を dirty にせず、
  ロールバックの `git reset --hard` でも消えず、誤コミットの余地も無い）
- **予約は成立したリリース 1 回で消費**される（タグを push した時点でクリア）。
  版種（patch / minor / major）は CHANGELOG の節・コミット件名・タグ注釈へ自動で載る
- **予約しても配布形態は変わらない**: 夜間リリースは常に**テスト版（prerelease）**として出る
  （#403）。節目の版を安定版として出したいときは、出たあとに
  `scripts/release.sh --promote v<tag>` で昇格させる
- 使えない予約値（semver 外 / プレリリース付き / 現行以下 / タグが既に在る）は
  **予約を無視して patch bump へフォールバック**し、警告ログ + 通知を出す
  （`--reserve` での指定時にも同じ検証で弾く）
- **リリースに至らなかった夜は予約を保持する**。「予約あり + 変更ゼロ」でも消費せず、
  次に変更が入った夜へ持ち越す（dirty / 手動リリース進行中 / プレリリース版 /
  ビルド失敗 / `--dry-run` も同じ）
- 検証は `bash scripts/test-nightly-reserve.sh`（一時ディレクトリに origin + 作業リポを
  作り、launchd と同じ `/bin/bash` で実走させる。release.sh はスタブ・HOME も隔離するので
  **本番のタグ / Release / 予約ファイル / launchd には触らない**）
- **launchd が実行するのは install_root 側のスクリプト**（既定 `~/dev/tako/scripts/nightly-release.sh`）。
  予約機構を直したときは、そのパスへ反映されているか（= main を pull 済みか）まで確認する
