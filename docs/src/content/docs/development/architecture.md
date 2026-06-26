---
title: アーキテクチャ
description: tako の内部構造 — crate 構成と 3 層制御プレーン
---

tako の技術スタックと内部構造の概要です。

## 技術スタック

| 領域 | 採用技術 |
|---|---|
| 言語 | Rust |
| UI フレームワーク | GPUI（Zed 製） |
| ターミナルエミュレーション | alacritty_terminal |
| ライセンス | GPL-3.0-or-later |

## Crate 構成

tako は 4 つの crate で構成されています。

```
crates/
├── tako-core/      ドメインモデル（GPUI 非依存）
├── tako-control/   制御プレーン（IPC + dispatch + MCP）
├── tako-app/       GUI アプリケーション（GPUI バイナリ）
└── tako-cli/       CLI + MCP stdio ブリッジ
```

### tako-core

GPUI に依存しない純粋なドメインモデル。

- **PaneTree**: タブ・ペインのツリー構造（分割・削除・フォーカス・リサイズ・均等化）。ユニットテスト 100 本以上
- **Workspace**: タブの管理・たまり場・レイアウト永続化
- **TerminalSession**: PTY 管理・tmux セッションとの対応
- **tmux**: tmux CLI ラッパー（セッション・window の列挙・操作）
- **git**: git CLI ラッパー（log・diff・status パーサ）
- **ports**: listen ポート検知（libproc + tty 突き合わせ）
- **osc_tap**: OSC 7/133 パーサ（cwd 通知・プロンプトマーク）

### tako-control

IPC サーバーと操作ディスパッチ。

- **dispatch**: 全操作の一元化ハブ。CLI・MCP・UI の全経路がここを通る
- **protocol**: リクエスト/レスポンスの型定義
- **mcp**: MCP エンジン（トランスポート非依存。Streamable HTTP + stdio ブリッジ）
- **ipc**: Unix domain socket + JSON-RPC + トークン認証

### tako-app

GPUI バイナリ。GPUI への依存はここに閉じる。

- **TakoApp**: メインアプリケーション構造体
- UI モジュール群: `tab_bar`, `status_bar`, `sidebar`, `right_panel`, `drawer`, `preview_render`, `keybindings`
- MCP サーバー・IPC サーバーの起動と統合
- セルフテスト機構

### tako-cli

`tako` コマンドの実装。

- サブコマンド群（`split`, `send`, `list`, `open`, `shelve`, `tmux` 等）
- `tako mcp serve`: MCP stdio ブリッジ（Claude Code への登録経路）
- `tako setup-mcp`: Claude Code への MCP 登録ヘルパー
- `tako master`: オーケストレーターのマスター起動

## 3 層制御プレーン

tako の AI 連携は 3 つのレイヤーで構成されています。

### Layer 1: 汎用 CLI

`tako` コマンドで画面操作を行う。シェルスクリプトやオーケストレーターから利用。

### Layer 2: 内蔵 MCP サーバー

AI エージェントが設定ゼロで直接操作する。Layer 1 と同じ dispatch を共有。

### Layer 3: パッシブ検知

ペイン内のプロセス活動を自動検知（ポート listen、コマンド状態、cwd 変化）し、提案や UI フィードバックに変換する。

## 設計原則

1. **ゼロコンフィグ**: 設定なしで AI 連携が動く
2. **Zed 級の速度**: GPU 描画によるネイティブ性能
3. **強制しない**: 勝手にペインを分割しない。提案は常にユーザーの承諾が必要
4. **クロスプラットフォーム**: macOS 先行だが Windows で動かない設計は避ける
5. **AI フルコントロール**: UI でできることはすべて AI からもできる
