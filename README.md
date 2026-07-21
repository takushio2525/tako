# 🐙 tako

**AI エージェント時代の、集約監視に特化した高速 GUI ターミナル**
**A fast GUI terminal built for the AI-agent era — monitor your whole agent fleet in one tab.**

> 🚧 開発中（macOS で動作、Windows はビルドのみ CI 検証）。 / In development — runs on macOS; Windows is build-verified in CI.

## なぜ tako？ / Why tako?

Claude Code のような AI エージェントを使う開発では、1 つの作業が「エージェント本体 + 子エージェント + dev サーバー + ログ」に分裂し、既存ターミナルではタブやウィンドウに散らばってしまいます。tako は **「1 グループ = 1 タブ」** で、エージェントが起動した子プロセスのペインを同じタブ内に自動で生やし、全体をひと目で監視できるようにします。

Working with AI agents like Claude Code, a single task naturally splits into the agent itself, sub-agents, dev servers, and logs — scattered across tabs and windows in existing terminals. tako keeps **one group in one tab**: panes for agent-spawned processes appear automatically right next to their parent, so you can watch the whole fleet at a glance.

## 特徴 / Features

- **エージェント集約監視 / Agent fleet monitoring** — 3 層の検知・制御（汎用 CLI、**設定ゼロで使える内蔵 MCP サーバー**、opt-in のパッシブ検知）/ Three integration layers: a generic CLI, a **built-in zero-config MCP server**, and opt-in passive detection
- **Zed 級の速度 / Zed-class speed** — Rust + GPUI + alacritty_terminal によるネイティブ GPU 描画 / Native GPU rendering, no Electron
- **軽量ワークスペース / Lightweight workspace** — cwd 連動ファイルツリー、自動更新されるコード / Markdown / 画像 / PDF プレビュー、git graph / cwd-aware file tree, live code / Markdown / image / PDF previews, git graph
- **クロスプラットフォーム / Cross-platform** — macOS 先行、Windows 対応必須 / macOS first, Windows is a hard requirement

## ステータス / Status

仕様は [`.agent/`](.agent/) にあります（concept / requirements / architecture / roadmap）。
ターミナル基盤（タブ・分割・IME）、`tako` CLI、内蔵 MCP サーバー（Claude Code 連携）まで動作します。

Specs live in [`.agent/`](.agent/) (concept / requirements / architecture / roadmap).
The terminal core (tabs, splits, IME), the `tako` CLI, and the built-in MCP server (Claude Code integration) are working.

## ダウンロード / Download

[GitHub Releases](https://github.com/takushio2525/tako/releases) から最新の zip をダウンロードできます。
Pre-built macOS binaries are available on the [Releases](https://github.com/takushio2525/tako/releases) page.

### インストール手順 / Installation

1. `tako-vX.X.X-macos-arm64.zip` をダウンロード / Download the zip
2. ダブルクリックで展開 / Extract by double-clicking
3. `tako.app` を `/Applications` へドラッグ / Drag `tako.app` into `/Applications`
4. 初回起動時に Gatekeeper の警告が表示される（Developer ID 署名がないため） / macOS Gatekeeper will warn on first launch (not notarized yet):
   - `tako.app` をダブルクリックして警告が出たら一旦キャンセル / Double-click, then cancel the warning
   - **システム設定 → プライバシーとセキュリティ** を開く / Open **System Settings → Privacy & Security**
   - 下部に「"tako"は開発元を確認できないため〜」と表示されるので **「このまま開く」** をクリック / Click **"Open Anyway"** next to the tako warning
   - もう一度 `tako.app` を起動すると「開く」ボタンが表示される / Launch again and click **"Open"**

## ソースからビルド / Build from Source

macOS で `tako.app` を生成して `/Applications` へ配置するには:

```sh
# dist/tako.app を生成（--verify でバンドル版のセルフテストも実行）
scripts/build-app.sh --verify

# /Applications へ配置（手動なら dist/tako.app をコピーでも同じ）
scripts/build-app.sh --install
```

アイコンの再描画には `rsvg-convert`（`brew install librsvg`）を使います。
無い場合は同梱の PNG から自動でフォールバックします。

開発時はバンドル不要で `cargo run -p tako-app` がそのまま使えます。

To build `tako.app` on macOS, run `scripts/build-app.sh --verify` (creates `dist/tako.app` and
runs the bundled self-test), then `scripts/build-app.sh --install` to copy it into `/Applications`.
Icon rendering uses `rsvg-convert` (`brew install librsvg`) with a PNG fallback.
For development, plain `cargo run -p tako-app` works without bundling.

### Claude Code 連携 / Claude Code integration

tako 内で Claude Code からペイン操作（分割・送信・読み取り等）を使うには、初回 1 回だけ
MCP サーバーの接続設定が必要です（以後はどのプロジェクトでも設定ゼロ）。

**方法 1: コマンド一発（推奨） / One command (recommended)**

```sh
tako setup-mcp
```

`~/.claude/settings.json` に tako MCP サーバーの設定を自動追加します。
プロジェクト単位で設定したい場合は `tako setup-mcp --project`（カレントディレクトリの `.claude/settings.json` に追加）。
tako アプリが起動中なら、Claude Code に「tako の MCP を設定して」と頼んでも設定できます（MCP ツール `tako_setup_mcp`）。

This adds the tako MCP server config to `~/.claude/settings.json` automatically.
Use `--project` to write to the current directory's `.claude/settings.json` instead.
If the tako app is running, you can also ask Claude Code "set up tako MCP" (via the `tako_setup_mcp` tool).

**方法 2: 手動設定 / Manual setup**

`~/.claude/settings.json`（または `.claude/settings.json`）に以下を追加:

```json
{
  "mcpServers": {
    "tako": {
      "command": "/Applications/tako.app/Contents/MacOS/tako",
      "args": ["mcp", "serve"]
    }
  }
}
```

`command` のパスは tako CLI のインストール場所に合わせてください（`which tako` で確認可）。

**方法 3: claude コマンド / Using claude CLI**

```sh
claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve
```

Register the bundled stdio bridge with any of the methods above; after that, pane-control tools are available with zero per-project setup. Outside tako the bridge exposes 0 tools and stays out of the way.

## リモートアクセス / Remote access

`tako remote start` はスマホのブラウザから tako のペインを操作するための HTTP API サーバーを起動します。**この機能は既定で無効**で、明示的に起動したときだけ動きます。transport は **Tailscale Serve + Unix domain socket**: サーバーは UDS（0600）のみで listen し TCP ポートは一切開きません。[Tailscale](https://tailscale.com/) があなたの tailnet（プライベートネットワーク）内限定の恒久固定 URL `https://<ホスト名>.<tailnet>.ts.net` で公開します。通信は WireGuard で**エンドツーエンド暗号化**され、URL は public internet には存在しません。Mac とスマホの両方に Tailscale アプリを入れ、同一アカウントでログインしている必要があります。Tailscale が未セットアップの場合、`tako remote start` は不足項目を列挙して起動を拒否します。

**セキュリティ上の注意（使う前に必ず読んでください）:**

- **これは正規の遠隔操作ツールです。** 接続すると、リモートのブラウザから**あなたのターミナルへ任意のキー入力・コマンドを送信できます**（＝実質的にシェルへのフルアクセス）。自分の端末を自分で操作する目的でのみ使ってください。他人の端末に無断で導入・接続する用途のものではありません。
- **接続 URL・QR コードに含まれるトークンは秘密情報です。** URL の `#token=...` 部分は端末を操作できる鍵そのものです。SNS・スクリーンショット・画面共有で他人に見せないでください。トークンはサーバー起動のたびに新しく生成され、停止で無効になります。
- **到達できるのは同じ tailnet の端末だけです。** それでも tailnet 内の全端末が信頼できるとは限らない場合（共有 tailnet 等）は、この機能を使わないでください。Tailscale アカウント自体の保護（2 要素認証等）も重要です。

---

`tako remote start` launches an HTTP API server that lets you drive tako's panes from a phone browser. **It is disabled by default** and only runs when you explicitly start it. Treat it as a legitimate remote-control tool: once connected, the remote browser can send **arbitrary keystrokes and commands to your terminal** (effectively full shell access). Transport is **Tailscale Serve + Unix domain socket**: the daemon listens only on a UDS (0600) and opens no TCP ports. It is published exclusively inside your tailnet at a permanent URL (`https://<host>.<tailnet>.ts.net`), end-to-end encrypted via WireGuard and invisible to the public internet. Both your Mac and phone need the Tailscale app signed into the same account; if Tailscale is not set up, `tako remote start` lists what is missing and refuses to start.

## トラブルシューティング / Troubleshooting

### brew upgrade が失敗して更新できなくなった場合

Homebrew の Swift toolchain（`copy-xattrs.swift`）が CommandLineTools/SDK のバージョン不整合でビルド失敗し、`brew upgrade --cask tako` が中断されると、cask 台帳から tako が消えているのに `/Applications/tako.app` の実体は残る「詰み状態」が発生することがあります。

この状態では `brew install --cask tako` も「It seems there is already an App at '/Applications/tako.app'」で失敗します。

**復旧方法（いずれか）:**

```sh
# 方法 1: tako CLI で修復（推奨。tako が起動している場合）
tako update repair

# 方法 2: brew で台帳を再締結
brew install --cask takushio2525/tako/tako --force

# 方法 3: brew を諦めて zip で手動更新
tako update apply-zip
```

`tako update status` で現在の配布系統を確認できます。`install_method` が `broken-brew` と表示される場合、上記の復旧が必要です。

**根本原因の解消:** Homebrew の Swift toolchain エラーが根本原因の場合、以下で Xcode CommandLineTools を再インストールすると brew 側の問題も解消します:

```sh
sudo rm -rf /Library/Developer/CommandLineTools
xcode-select --install
```

### 「ほかのアプリからのデータへのアクセス」ダイアログが繰り返し出る場合

macOS 26 (Tahoe) 以降では、tako 内で動く AI エージェント（Claude Code 等）のサンドボックス化されたコマンドが iCloud Drive・Google Drive・他アプリのデータ領域に触れるたびに、macOS が**対象ごとに個別の**許可ダイアログを tako.app 名義で表示します（tako 自身がこれらの領域を読むわけではありません）。対象（クラウドドライブのアカウントやアプリ）の数だけダイアログが出るため、頻発する場合は以下で恒久解消できます:

**システム設定 → プライバシーとセキュリティ → フルディスクアクセス → tako を ON**

フルディスクアクセスは個別許可の上位互換のため、以後このダイアログは表示されません。

v0.2.6 以降は署名の designated requirement が identifier 固定になり、付与した許可（フルディスクアクセス・個別許可とも）が再ビルド・アプリ内更新をまたいで保持されます。**v0.2.5 以前からの更新直後は署名要件の移行のため 1 回だけ再許可が必要です。**

### タブ・ペインが大量に消えてしまった場合

タブやターミナルペインが突然大量に消えても、**実体のプロセスはバックエンド tmux セッションの中で生き続けていることがほとんど**です（AI エージェントは会話の文脈ごと生存しています）。以下の順で復旧してください:

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

## ライセンス / License

[GPL-3.0-or-later](LICENSE) — 依存クレート（zlog / ztracing、Zed リポ由来）が GPL-3.0 のため。
