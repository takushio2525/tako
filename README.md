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
- **軽量ワークスペース / Lightweight workspace** — cwd 連動ファイルツリー、コード / Markdown プレビュー、git graph / cwd-aware file tree, code & Markdown preview, git graph
- **クロスプラットフォーム / Cross-platform** — macOS 先行、Windows 対応必須 / macOS first, Windows is a hard requirement

## ステータス / Status

仕様は [`.agent/`](.agent/) にあります（concept / requirements / architecture / roadmap）。
ターミナル基盤（タブ・分割・IME）、`tako` CLI、内蔵 MCP サーバー（Claude Code 連携）まで動作します。

Specs live in [`.agent/`](.agent/) (concept / requirements / architecture / roadmap).
The terminal core (tabs, splits, IME), the `tako` CLI, and the built-in MCP server (Claude Code integration) are working.

## ビルドとインストール / Build & Install

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
MCP の stdio ブリッジを登録します（以後はどのプロジェクトでも設定ゼロ）:

```sh
claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve
```

Register the bundled stdio bridge once (`claude mcp add --scope user tako -- /Applications/tako.app/Contents/MacOS/tako mcp serve`); after that, pane-control tools are available with zero per-project setup. Outside tako the bridge exposes 0 tools and stays out of the way.

## ライセンス / License

[Apache-2.0](LICENSE)
