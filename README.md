# 🐙 tako

**AI エージェント時代の、集約監視に特化した高速 GUI ターミナル**
**A fast GUI terminal built for the AI-agent era — monitor your whole agent fleet in one tab.**

> 🚧 仕様策定フェーズ。コードはまだありません。 / Spec phase — no code yet.

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
Phase 0（GPUI の Windows ビルド検証 + 最小ターミナル PoC）から開始します。

Specs live in [`.agent/`](.agent/) (concept / requirements / architecture / roadmap).
Development starts with Phase 0: a GPUI Windows build spike + minimal terminal rendering PoC.

## ライセンス / License

[Apache-2.0](LICENSE)
