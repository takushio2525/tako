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

1 行 1 コマンドの索引。**実測・罠・A/B の env・オプションの意味は `.agent/commands.md` の
同じ行**にあるので、そのコマンドを使う直前にそこを Read する（毎ターンは読まない）。

| 操作 | コマンド |
|---|---|
| dev（最小ターミナル起動） | `cargo run -p tako-app` |
| **実験・検証用の隔離起動（本番 GUI 稼働中は必須。#177）** | `TAKO_ISOLATED=1 cargo run -p tako-app` |
| **検証用 GUI の置き場（ユーザーの画面に窓を出さない。#1141）** | `scripts/lib/virtual-display.sh ensure` |
| **仮想ディスプレイの孤児の後片付け（#1150）** | `scripts/lib/virtual-display.sh cleanup-orphans` |
| **tmux サーバーの孤児の後片付け（隔離・テストの残骸。既定は dry-run。#1192 / #1282）** | `tako tmux cleanup --servers [--apply]` |
| セルフテスト起動（入力経路 + CLI / MCP e2e の機械検証） | `TAKO_SELF_TEST=1 cargo run -p tako-app` |
| 実 claude の e2e（#28 の Shift+Enter = 45c / #716 のチャット送信 = 95c。要 claude CLI + 認証 + tmux） | `env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_SESSION_ID -u CLAUDECODE -u CLAUDE_CONFIG_DIR TAKO_SELF_TEST=1 TAKO_SELF_TEST_CLAUDE=1 cargo run -p tako-app` |
| Claude Code 実機検証（MCP 設定ゼロ接続） | `scripts/verify-claude-mcp.sh` |
| 自動セットアップ | `tako setup [--yes] [--answers <json|@file|->]` |
| **エージェント CLI のゼロスタート導入（claude / codex / agy。#868 / #1057 / #989）** | `tako setup bootstrap [status\|status-all\|install\|path\|undo-path\|handoff] [--agent <claude\|codex\|agy>]` |
| **任意依存のその場導入（#88 / #1057）** | `tako setup deps [install] [--dep <名>] [--dry-run] [--json]` |
| **モデル一覧の実取得とピッカー（#1002）** | `tako setup models [--agent <claude|codex|agy>] [--json]` |
| **MCP セットアップ（claude / codex / agy。#979）** | `tako setup-mcp` |
| `tako` CLI ビルド | `cargo build -p tako-cli` |
| .app バンドル生成（macOS） | `scripts/build-app.sh [--verify] [--install]` |
| リリース（**両 OS 同時が既定**。#594/#965） | `scripts/release.sh` |
| 夜間リリース（自動） | `scripts/nightly-release.sh` |
| **Windows 配布物生成（既定は CI。#587/#965）** | `.github/workflows/release-windows.yml` |
| **Windows リリース（CI が使えないときの実機経路。#587/#965）** | `pwsh -File installer/windows/release-windows.ps1` |
| Windows アプリアイコン再生成 | `pwsh -File installer/windows/make-icon.ps1` |
| マスターオーケストレーター起動 | `tako master [-profile]` |
| **SSH ペイン（ファイルメニュー「リモート接続…」/ ペインの右クリック / `tako open-in remote <host>`。#20 / #919 / #1006）** | `ssh <host>` |
| ソロエージェント起動（オーケストレーション無しの 1 対 1 対話） | `tako solo [-profile]` |
| オーケストレーター master 自己情報 | `tako orchestrator self [--pane N]` |
| オーケストレーター master 引き継ぎ（#193/#749/#915/#854/#917） | `tako orchestrator handoff [--pane N] [--tab T] [--projects a,b]` |
| **master の自動ハンドオフ（#749）** | `【tako 自動通知】` |
| **引き継ぎファイルの管理（#915）** | `tako orchestrator handoffs list/show/write/migrate` |
| **master の手順書を引く（#1154）** | `tako orchestrator guide <topic>` |
| オーケストレーター worker spawn | `tako orchestrator spawn --project <key> --prompt "..."` |
| **worker への指示送達（#790）** | `<data_dir>/persist.log` |
| オーケストレーター worker 監視 | `tako orchestrator watch --pane <N>` |
| オーケストレーター ダイアログ応答（#319 → #748 で全種別） | `tako orchestrator respond --pane <N> [--choice <番号|ラベル>]` |
| オーケストレーター worker 報告取得 | `tako orchestrator report --pane <N> [--lines 2000]` |
| オーケストレーター worker レジストリ一覧 | `tako orchestrator workers [--all]` |
| オーケストレーター プロジェクト管理 | `tako orchestrator projects list/add/remove` |
| オーケストレーター プロファイル管理（#721/#749） | `tako orchestrator profiles list/show/set/create/copy/delete` |
| **スマホから会話を操作する（Claude 公式 Remote Control。#1068 / #1069 / #1077 / #1078）** | `tako orchestrator profiles set <名前> --remote-control true` |
| オーケストレーター アカウント管理（#504/#548） | `tako orchestrator accounts list/show/add/remove` |
| worker spawn のレイアウト設定 | `tako orchestrator layout [--policy master-reserved|legacy] [--master-ratio 0.5] [--algorithm grid|spiral]` |
| build | `cargo build --workspace` |
| lint | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` |
| test | `cargo test --workspace` |
| リリース経路のモックテスト（#256 / #965） | `bash scripts/test-release-retry.sh` |
| **スマホからの master 起動の実経路テスト（#1078）** | `bash scripts/test-remote-master-launch.sh` |
| リモート serve 自己検査のモックテスト（#1049） | `bash scripts/test-serve-watch.sh` |
| ファイルツリーフォルダ操作 | `tako tree add <path>` |
| **Finder の「このアプリケーションで開く」（#708 / #835）** | `tako open <file> --new-tab` |
| **ターミナル上のパスの cmd+右クリックメニュー（#1182）** | `tako file open-in-tako <path>` |
| **画面のどのパスがリンクになるかを読む（cmd+クリックの検証。#1283）** | `tako links [--pane N] [--text <画面テキスト>]` |
| **リモートからフォルダを開く（SSH 先のワークスペース化。#919 / #65 / #976 / #1041）** | `ssh <host>` |
| **スマホからファイルを見る・直す（#1079 / #1084 / #1085）** | `#/files` |
| **リモートファイルの編集・保存（#966 / #65）** | `put` |
| **リモート公開の自己検査と自動復旧（#1049）** | `tako remote status` |
| プレビュー目次操作 | `tako preview-outline [--pane N] [--item N]` |
| プレビュー内リンク（#680 / #271） | `tako preview-link-list` |
| Markdown コードブロックのコピー（#680） | `tako preview-copy-code [index]` |
| プレビューライブリロード | `tako preview-reload [on|off]` |
| プレビュー画像キャッシュ | `tako preview-cache [max_mb]` |
| 設定の自動マイグレーション（#916） | `tako migrate [status|run] [--schema <種別>]` |
| 受け入れゲート（#244 / #935） | `tako task gate set <task_id> --command "cmd" [--pr-merged N] [--custom "desc"]` |
| git ブランチ操作（#496） | `tako git checkout <branch>` |
| コンフリクト解消エージェント（#496） | `tako git resolve [--agent claude|codex|agy] [--tab N]` |
| Web ビューペイン操作 | `tako web open <url>` |
| 複数ウィンドウ操作（ビューポート方式 + 共有タブバー。#339/#380） | `tako window list` |
| **シェル統合（cwd 追従・コマンド状態。#525）** | `tako shell-integration [status|install|uninstall]` |
| エージェント共通ルール同期 | `tako agents sync-rules` |
| AI 系設定のデバイス間共有（#513） | `tako config` |
| **tmux の一覧・取り込み・window 切替（#1185 / #1190 / #1186）** | `tako tmux list` / `tako tmux open <session> [--window N]` / `tako tmux select-window N` |
| レイアウト復旧（タブ・ペイン消失時。#177/#381/#770） | `tako recover` |
| **何がいつ消えたかを調べる（#770）** | `<data_dir>/persist.log` |
| **再起動後にエージェントが戻らないとき（#1076 / #1238）** | `<data_dir>/persist.log` の「復元の内訳」 |
| セッションカタログ（会話の発見・復元。#112 / #1069） | `tako sessions list [--role r] [--project p]` |
| ペインの平文ログ（ペイン死亡後も出力を遡る。#112） | `tako logs list` |
| **スクロールバック保持上限（直接ペインの RAM。#818）** | `tako scrollback [lines]` |
| スリープ防止 | `tako sleep-guard status` |
| **会話を引き継いだセッション再起動（ペインの右クリック。#1067）** | `tako session-restart [--mode harness|handoff] [--pane N]` |
| **リミット後の自動復帰（ペイン単位。#813）** | `tako limit-resume [on|off] [--pane N] [--all]` |
| 入力予測（tako 内 zsh のゴースト予測。#600/#614） | `tako autosuggest [on|off]` |
| UI テーマ切替 | `tako theme [dark|light|toggle]` |
| UI 表示モード切替（GUI ライク表示。#691/#694/#702/#715/#716/#720/#725/#737/#739） | `tako ui-mode [gui|terminal|toggle]` |
| チャット本文のコピー（#725） | `tako chat copy [--pane N] [--message N] [--code K] [--markdown] [--list]` |
| プラットフォーム対応マトリクス（#515 / #591） | `tako platform [--platform macos|windows] [--status pending] [--known-limitations] [--json]` |
| **codex の利用制限データ（#357 / #985）** | `$CODEX_HOME/sessions/**/rollout-*.jsonl` |
| **agent 能力マトリクス（#982）** | `tako agent-support [--agent claude|codex|agy|local] [--status supported|degraded|pending|unsupported] [--json]` |
| Windows 対応状況ページの生成（#591） | `cargo build -p tako-cli && node scripts/gen-windows-support-docs.mjs` |
| **Windows のウインドウを実測する（#1063）** | `pwsh -File scripts/windows/measure-window.ps1 -TakoPid <pid> [-Png out.png] [-Maximize] [-ClickX N -ClickY N]` |
| 右パネルのビュー切替 | `tako panel --show --view <fleet|orch|git>` |
| Code Runner でファイル実行（#453） | `tako run <file> [--profile <name>]` |
| 拡張子既定コマンド設定 | `tako run-default [ext] [command]` |
| AI コマンド提案カード（#666/#703） | `tako show-command <コマンド>...` |
| 設定画面（#459/#721） | `tako settings [--tab <名>]` |
| 初回起動バナー（#549） | `tako welcome [show|dismiss]` |
| アプリ内更新（#36/#403/#616/#690/#1042） | `tako update [status|check|apply|apply-zip|repair]` |
| **テスト・検証プロセスの残骸の後片付け（一時 dir。既定は dry-run。#1296）** | `tako test-residue [--apply]` |
| target 掃除 | `scripts/clean-target.sh` |

CI（`.github/workflows/ci.yml`）は macOS / Windows の両ランナーで build + test を回す。

## AI 向け詳細仕様（必要なときだけ Read する）

- コマンドの詳細（実測・罠・A/B の env・オプションの意味）: `.agent/commands.md`
- コンセプト・競合・Non-goals: `.agent/concept.md`
- 機能要件（FR / NFR）: `.agent/requirements.md`
- 技術設計・リスク・3 層制御プレーン: `.agent/architecture.md`
- 規約（命名・エラー・ログ）: `.agent/conventions.md`
- agent 種別 enum の対応表（統合しない理由・寄せ先一覧）: `.agent/agent-enums.md`
- 手動確認チェックリスト（IME・.app 等、機械検証できない項目）: `.agent/manual-checks.md`
- オーケストレーター使い方ガイド: `.agent/orchestrator.md`
- 解説動画 / X 向けショートの作り方: `.agent/plans/2026-09-youtube-explainer.md` / `.agent/plans/2026-09-x-short.md`

### 作業履歴メモ（毎ターン参照・更新）

- 現在の作業状況（毎ターン上書き）: @.agent/activeContext.md
- 完了タスクの時系列（毎ターン追記）: @.agent/progress.md

セッション開始時に必ず読み、応答終了前に `activeContext` は最新状態で**上書き**、
作業が一段落していれば `progress` の末尾に**1〜3 行で追記**する。
スキップ可能なターン（単発質問への回答、タイポ修正のみ）では更新しない。

フェーズ計画・次の一手は `.agent/roadmap.md`、移送済みの古い履歴は
`.agent/progress-archive.md`（どちらも**毎ターンは読まない**。必要なときだけ Read する）。

### 起動時ロードの予算（Issue #1139）

**この節は生成物**（正本は `tako_core::context_budget` の予算表）。手で数値を書き換えない。
`tako context-budget` / MCP `tako_context_budget` が同じ表で判定し、CI の番犬
`crates/tako-control/tests/context_budget.rs` が落とす。

<!-- tako:context-budget-rule -->
### 絶対に読む範囲（`@import` してよいもの）

- **現在状態**（`activeContext.md` 型）: 80 行以内。「現在の対象 / 直近の観点 / 次の一手」だけを置く
- **作業ログ**（`progress.md` 型）: **直近 5 作業日 かつ 20 エントリ かつ 12 KB 以内**。
  1 エントリは「何を / どこを / 結果」の **3 行以内**にとどめ、詳細は git log・Issue・PR に委ねる
- タスクリスト 1 本（プロジェクトにあれば）

### アーカイブとする範囲

- 予算から外れたエントリは `progress-archive.md` へ **1 行**（`- YYYY-MM-DD #番号 一言`）で移す
- アーカイブは `@import` しない・普段は Read しない
- **90 日より古いアーカイブ行は消す**（git log・Issue・PR が正本なので情報は失われない）

### それ以外の上限

- エージェント規約（`AGENTS.md`）は **30 KB 以内**。長い注記・実測・罠は `.agent/` 配下の
  別ファイルへ出し、規約からは**バックティック参照**で案内する（`@import` にはしない）
- `@import` の合計は **40 KB 以内**
- 引き継ぎの運用メモは 80 行以内 / グローバル指示ファイルは 24 KB 以内
- master / solo の **system prompt は 24 KB 以内**。手順の詳細は
  `tako orchestrator guide <topic>` で必要なときだけ引く形にし、prompt には「いつ引くか」を残す

### 機械強制

- `tako context-budget` で状態を確認し、`tako context-budget fix` で作業ログの移送を自動で行う
  （**冪等・本文は改変しない・全文は git 履歴に残る**）
- 自動で直せないもの（規約の肥大・`@import` の増殖）は `proposals` として直し方が返る
- CI の番犬がこの予算を検査するので、超えたまま merge できない
<!-- /tako:context-budget-rule -->

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

### 夜間リリース（自動。#166 / #1005 / #1136）

- `scripts/nightly-release.sh` が launchd（`com.takushio.tako-nightly-release`、毎日 5:00）から
  実行され、前回タグ以降に main へ変更があった夜だけ自動リリースする
  （version bump → CHANGELOG 自動節 → コミット → annotated tag → release.sh でバイナリ付き
  GitHub Release）。クラウドルーチンでの夜間リリースはバイナリを作れず廃止した（経緯は #166）
- 自動スキップ条件: 変更なし / 共有ツリー dirty / 手動リリース進行中（Cargo.toml version ≠ 最新タグ）/
  プレリリース版 / 多重起動。ログは `~/.claude-orchestrator/logs/tako-nightly-release.log`
- **共有ツリー（install_root）の HEAD は動かさない（#1136）**: リリース作業は毎回
  `git worktree add --detach` した使い捨てのツリーで行い、成功・失敗・シグナルのどれでも
  `trap` で撤去する。ビルド中に origin/main が進んだら**何も作らず中止**する（旧版はここで
  push が拒否されて無言で死に、未 push のリリースコミットごと detached を残していた =
  同じ版が別 SHA で 2 本できる原因）。多重起動ロックは HOME 単位 + **リポジトリ単位**の 2 段
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
