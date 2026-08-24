# tako

**AI エージェント時代の、集約監視に特化した高速 GUI ターミナル**
**A fast GUI terminal built for the AI-agent era — monitor your whole agent fleet in one tab.**

開発中です。macOS で動作し、Windows は移植を進めています（[対応状況](https://tako-docs.pages.dev/windows-support/)）。
In development. Runs on macOS; the Windows port is in progress.

**ドキュメント / Documentation — [tako-docs.pages.dev](https://tako-docs.pages.dev/)**
[セットアップ](https://tako-docs.pages.dev/getting-started/) ・
[クイックスタート](https://tako-docs.pages.dev/getting-started/quickstart/) ・
[CLI リファレンス](https://tako-docs.pages.dev/guides/cli-reference/) ・
[MCP ツール一覧](https://tako-docs.pages.dev/guides/mcp-tools/) ・
[オーケストレーション](https://tako-docs.pages.dev/features/orchestration/)

## なぜ tako？ / Why tako?

Claude Code のような AI エージェントを使う開発では、1 つの作業が「エージェント本体 + 子エージェント + dev サーバー + ログ」に分裂し、既存ターミナルではタブやウィンドウに散らばってしまいます。tako は **「1 グループ = 1 タブ」** で、エージェントが起動した子プロセスのペインを同じタブ内に自動で生やし、全体をひと目で監視できるようにします。

Working with AI agents like Claude Code, a single task naturally splits into the agent itself, sub-agents, dev servers, and logs — scattered across tabs and windows in existing terminals. tako keeps **one group in one tab**: panes for agent-spawned processes appear automatically right next to their parent, so you can watch the whole fleet at a glance.

## 特徴 / Features

- **エージェント集約監視 / Agent fleet monitoring** — 3 層の検知・制御（汎用 CLI、**設定ゼロで使える内蔵 MCP サーバー**、opt-in のパッシブ検知）/ Three integration layers: a generic CLI, a **built-in zero-config MCP server**, and opt-in passive detection
- **Zed 級の速度 / Zed-class speed** — Rust + GPUI + alacritty_terminal によるネイティブ GPU 描画 / Native GPU rendering, no Electron
- **軽量ワークスペース / Lightweight workspace** — cwd 連動ファイルツリー、自動更新されるコード / Markdown / 画像 / PDF プレビュー、git graph / cwd-aware file tree, live code / Markdown / image / PDF previews, git graph
- **セッション永続化 / Persistent sessions** — tmux をバックエンドに使い、tako を再起動しても実行中プロセスと画面が復元される / With tmux as the backend, running processes and screen contents survive a restart
- **クロスプラットフォーム / Cross-platform** — macOS 先行、Windows 対応必須 / macOS first, Windows is a hard requirement

## インストール / Install

配布しているビルド済みバイナリは **Apple Silicon（macOS 11 以降）** 向けです。
The prebuilt binaries target **Apple Silicon (macOS 11+)**.

### Homebrew（推奨） / Homebrew (recommended)

```sh
brew install --cask takushio2525/tako/tako
```

更新は `brew upgrade --cask takushio2525/tako/tako`、またはアプリ内の更新通知から行えます。
tako CLI も同時に PATH へ入るため、`tako` コマンドがそのまま使えます。

Update with `brew upgrade --cask takushio2525/tako/tako`, or from the in-app update notification.
The cask also links the `tako` CLI into your PATH.

### zip を手動で / Manual zip

[GitHub Releases](https://github.com/takushio2525/tako/releases) から `tako-vX.X.X-macos-arm64.zip` を取得します。
Grab `tako-vX.X.X-macos-arm64.zip` from the [Releases](https://github.com/takushio2525/tako/releases) page.

1. zip をダブルクリックで展開し、`tako.app` を `/Applications` へドラッグ / Extract and drag `tako.app` into `/Applications`
2. 未署名のため初回起動時に Gatekeeper の警告が出ます / macOS Gatekeeper warns on first launch (not notarized yet):
   - `tako.app` をダブルクリックして警告が出たら一旦キャンセル / Double-click, then cancel the warning
   - **システム設定 → プライバシーとセキュリティ** を開く / Open **System Settings → Privacy & Security**
   - 下部の「"tako"は開発元を確認できないため〜」の隣の **「このまま開く」** をクリック / Click **"Open Anyway"** next to the tako warning
   - もう一度 `tako.app` を起動すると「開く」ボタンが表示される / Launch again and click **"Open"**

## 使い始める / Getting started

素のターミナルとして使うなら、起動すればそのまま使えます。AI 連携を使う場合は tako 内のターミナルで次の 2 つを実行します。
As a plain terminal, just launch it. To use the AI integration, run these two commands inside tako:

```sh
tako setup     # 初回のみ。claude / codex / agy を検出して設定を整える
tako master    # 司令塔の AI（マスター）を今いるペインで起動する
```

あとは日本語で頼むだけです。マスターが作業役の AI（worker）を隣のペインに立ち上げ、指示を渡し、完了を見届けて報告します。

```
「~/Documents/webapp にあるリポジトリを管理対象に追加して」
「webapp の README の誤字を直しておいて」
```

オーケストレーションを使わず 1 対 1 で相談したいときは `tako solo`。専用タブで動かしたいときは `tako master --tab` です。
初回起動時はタブバー下のバナー（および Cmd+K のコマンドパレット）から同じ操作ができます。
詳しい流れは[クイックスタート](https://tako-docs.pages.dev/getting-started/quickstart/)、設定項目は[セットアップガイド](https://tako-docs.pages.dev/getting-started/)にあります。

`tako setup` detects your installed and authenticated agent CLIs (claude / codex / agy) and fills in the rest with previous or safe default values — with a single authenticated CLI it asks nothing. `tako master` then starts the orchestrator in the current pane, and you talk to it in plain language; it spawns workers next to itself and reports back. Use `tako solo` for one-on-one work without orchestration, and `tako master --tab` for a dedicated tab.

## Claude Code 連携 / Claude Code integration

tako 内の Claude Code からペイン操作（分割・送信・読み取り等）を使うには、初回 1 回だけ MCP サーバーの接続設定が必要です（以後はどのプロジェクトでも設定ゼロ）。

```sh
tako setup-mcp
```

Claude Code のユーザー設定に tako MCP サーバーを自動登録します（内部で `claude mcp add --scope user` を呼び出します）。
プロジェクト単位にしたい場合は `tako setup-mcp --project`（カレントディレクトリの `.mcp.json` に追加）。
tako アプリが起動中なら、Claude Code に「tako の MCP を設定して」と頼んでも設定できます（MCP ツール `tako_setup_mcp`）。
旧バージョンが `~/.claude/settings.json` に書いた無効な設定は自動で掃除されます。

This registers the tako MCP server in Claude Code's user config (internally `claude mcp add --scope user`); `--project` writes to the current directory's `.mcp.json` instead. If tako is running you can also ask Claude Code to "set up tako MCP" (the `tako_setup_mcp` tool). Outside tako the bridge exposes 0 tools and stays out of the way.

<details>
<summary>手動で設定する場合 / Setting it up manually</summary>

claude CLI から登録する場合:

```sh
claude mcp add --scope user --transport stdio tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve
```

`command` のパスは tako CLI のインストール場所に合わせてください（`which tako` で確認できます）。

設定ファイルを直接書く場合は `~/.claude.json` の `mcpServers` に以下を追加します（既存のキーを壊さないよう注意）。プロジェクト単位ならプロジェクトルートの `.mcp.json` に同じ構造を書きます。

```json
{
  "mcpServers": {
    "tako": {
      "type": "stdio",
      "command": "/Applications/tako.app/Contents/MacOS/tako",
      "args": ["mcp", "serve"],
      "env": {}
    }
  }
}
```

</details>

## リモートアクセス / Remote access

`tako remote start` は、外出先のスマホのブラウザから tako のペインを見て操作するための API サーバーを起動します。**既定で無効**で、明示的に起動したときだけ動きます。セットアップは `tako remote setup` の対話ウィザードが案内します。

通信は [Tailscale](https://tailscale.com/) の `serve` が HTTPS → Unix domain socket をプロキシする構成で、daemon は TCP ポートを一切開きません。URL は tailnet 内にのみ存在し、WireGuard でエンドツーエンド暗号化されます。認証は二層で、層①が `tailscale whois` による tailnet ノードの検証、層②が機器ペアリング（初回接続時に Mac 画面の承認ダイアログを通すまで画面データを受け取れない）です。仕組みの詳細は[リモートアクセスのドキュメント](https://tako-docs.pages.dev/features/remote/)にあります。

**使う前に必ず読んでください / Read before use:**

- **これは正規の遠隔操作ツールです。** 接続すると、リモートのブラウザから**あなたのターミナルへ任意のキー入力・コマンドを送信できます**（＝実質的にシェルへのフルアクセス）。自分の端末を自分で操作する目的でのみ使ってください。他人の端末に無断で導入・接続する用途のものではありません。
- **接続 URL を共有しないでください。** URL 自体にトークンは含まれませんが、tailnet 内の端末からはアクセス可能です。SNS やスクリーンショットで公開しないでください。
- **到達できるのは同じ tailnet の端末だけです。** それでも tailnet 内の全端末を信頼できない場合（共有 tailnet 等）は、この機能を使わないでください。Tailscale アカウント自体の保護（2 要素認証・tailnet lock）も重要です。

`tako remote start` launches an API server that lets you drive tako's panes from a phone browser. **It is disabled by default.** Treat it as a legitimate remote-control tool: once connected, the remote browser can send arbitrary keystrokes and commands to your terminal — effectively full shell access. Use it only to control your own machine, never share the connection URL, and do not enable it if you cannot trust every device on your tailnet.

## ソースからビルド / Build from source

開発中は `cargo run -p tako-app` がそのまま使えます（バンドル不要）。
For development, plain `cargo run -p tako-app` works without bundling.

`tako.app` を生成して `/Applications` へ配置する場合:

```sh
# dist/tako.app を生成（--verify でバンドル版のセルフテストも実行）
scripts/build-app.sh --verify

# /Applications へ配置（配置後、ビルド出力の dist/tako.app は片付けられる）
scripts/build-app.sh --install
```

同じ `.app` が 2 つディスク上にあると macOS の Launch Services が両方を登録し、Finder の「このアプリケーションで開く」に tako が 2 つ並びます。`--install` は配置後にビルド出力を消して登録も外すので、候補は `/Applications` の 1 つだけになります。

アイコンの再描画には `rsvg-convert`（`brew install librsvg`）を使います。無い場合は同梱の PNG から自動でフォールバックします。

`scripts/build-app.sh --verify` creates `dist/tako.app` and runs the bundled self-test; `--install` copies it into `/Applications`, then removes the build copy and unregisters it from Launch Services so Finder's "Open With" lists tako only once. Icon rendering uses `rsvg-convert` (`brew install librsvg`) with a PNG fallback.

### 開発コマンド / Development commands

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
```

AI エージェント向けの規約は [AGENTS.md](AGENTS.md)、詳細仕様は [`.agent/`](.agent/) にあります。
Conventions for AI agents live in [AGENTS.md](AGENTS.md); detailed specs are in [`.agent/`](.agent/).

## トラブルシューティング / Troubleshooting

<details>
<summary>brew upgrade が失敗して更新できなくなった場合</summary>

Homebrew の Swift toolchain（`copy-xattrs.swift`）が CommandLineTools/SDK のバージョン不整合でビルド失敗し、`brew upgrade --cask tako` が中断されると、cask 台帳から tako が消えているのに `/Applications/tako.app` の実体は残る「詰み状態」が発生することがあります。この状態では `brew install --cask tako` も「It seems there is already an App at '/Applications/tako.app'」で失敗します。

復旧方法（いずれか）:

```sh
# 方法 1: tako CLI で修復（推奨。tako が起動している場合）
tako update repair

# 方法 2: brew で台帳を再締結
brew install --cask takushio2525/tako/tako --force

# 方法 3: brew を諦めて zip で手動更新
tako update apply-zip
```

`tako update status` で現在の配布系統を確認できます。`install_method` が `broken-brew` と表示される場合、上記の復旧が必要です。

根本原因の解消: Homebrew の Swift toolchain エラーが根本原因の場合、以下で Xcode CommandLineTools を再インストールすると brew 側の問題も解消します。

```sh
sudo rm -rf /Library/Developer/CommandLineTools
xcode-select --install
```

</details>

<details>
<summary>「ほかのアプリからのデータへのアクセス」ダイアログが繰り返し出る場合</summary>

macOS 26 (Tahoe) 以降では、tako 内で動く AI エージェント（Claude Code 等）のサンドボックス化されたコマンドが iCloud Drive・Google Drive・他アプリのデータ領域に触れるたびに、macOS が**対象ごとに個別の**許可ダイアログを tako.app 名義で表示します（tako 自身がこれらの領域を読むわけではありません）。対象の数だけダイアログが出るため、頻発する場合は以下で恒久解消できます。

**システム設定 → プライバシーとセキュリティ → フルディスクアクセス → tako を ON**

フルディスクアクセスは個別許可の上位互換のため、以後このダイアログは表示されません。

v0.2.6 以降は署名の designated requirement が identifier 固定になり、付与した許可（フルディスクアクセス・個別許可とも）が再ビルド・アプリ内更新をまたいで保持されます。**v0.2.5 以前からの更新直後は署名要件の移行のため 1 回だけ再許可が必要です。**

</details>

<details>
<summary>タブ・ペインが大量に消えてしまった場合</summary>

タブやターミナルペインが突然大量に消えても、**実体のプロセスはバックエンド tmux セッションの中で生き続けていることがほとんど**です（AI エージェントは会話の文脈ごと生存しています）。以下の順で復旧してください。

1. **レイアウトのバックアップから戻す（推奨）**: ペイン数が大きく減る保存の直前には、レイアウトが自動で世代バックアップされています。

   ```sh
   tako recover                 # バックアップ世代の一覧（タブ数 / ペイン数 / 更新時刻）
   # tako を終了（Cmd-Q）してから:
   tako recover --apply 1       # 直前の世代を復元
   # tako を再起動 → 実行中プロセスごと画面に戻ります
   ```

2. **個別にセッションを取り込む**: バックアップが無い・一部だけ戻したい場合は、生きているセッションを直接タブへ取り込めます。

   ```sh
   tako tmux list               # バックエンドセッションと cwd の一覧
   tako tab new                 # 受け皿のタブを作る（出力の pane ID を控える）
   tako tmux open --socket tako --pane <ペインID> <セッション名>
   ```

</details>

## ライセンス / License

[GPL-3.0-or-later](LICENSE) — 依存クレート（zlog / ztracing、Zed リポ由来）が GPL-3.0 のため。

同梱している第三者成果物（zsh-autosuggestions ほか）の告知は
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) にあります。
Notices for bundled third-party works are in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
