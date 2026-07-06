---
title: 内蔵 MCP サーバー
description: tako 最大の差別化点 — 設定ゼロで AI がターミナルを操作
---

tako は **MCP（Model Context Protocol）サーバーを内蔵**しています。AI エージェントが設定ゼロでペインの分割・コマンド実行・ファイル表示を行えるのが、他のターミナルとの最大の違いです。

:::note[MCP とは？]
MCP は、AI エージェントが外部ツールを操作するための共通規格です。tako を MCP サーバーとして登録すると、Claude Code が tako の画面操作を「ツール」として直接呼び出せるようになります。
:::

## 仕組み

1. tako は起動時に MCP サーバー（Streamable HTTP）を内蔵で立ち上げる
2. 各ペインのシェルに環境変数（`TAKO_PANE_ID`、`TAKO_MCP_URL` 等）を注入
3. AI エージェントは環境変数から MCP サーバーを自動発見し、ツールとして利用

初回に `tako setup`（または `tako setup-mcp`）で stdio ブリッジを登録すれば、以降はどのプロジェクトでも設定不要です。

## 50+ の MCP ツール

tako は 50 以上の MCP ツールを公開しています。全リストは [MCP ツール一覧](/guides/mcp-tools/)にあります。主なカテゴリ:

### レイアウト操作
- `tako_split_pane` — ペイン分割（方向・比率・コマンド指定）
- `tako_close_pane` — ペイン削除（自己片付け対応）
- `tako_focus_pane` — フォーカス移動
- `tako_resize_pane` / `tako_equalize_layout` — サイズ調整・均等化
- `tako_list_panes` — タブ・ペイン構成の取得（JSON）

### テキスト操作
- `tako_send_input` — ペインへテキスト送信（送達確認付き）
- `tako_read_pane` — ペインの画面内容取得
- `tako_set_title` — タイトル設定

### ファイル・プレビュー
- `tako_open_file` — ファイルをプレビューペインで表示（コード / Markdown / 画像 / PDF / 動画）
- `tako_file_op` — ファイル操作（パスコピー・リネーム・作成・ゴミ箱等）

### タブ操作
- `tako_create_tab` / `tako_select_tab` / `tako_rename_tab` / `tako_move_pane_to_tab`

### バックグラウンド退避（たまり場）
- `tako_background_pane` — ペインを画面から退避（プロセスは維持）
- `tako_foreground_pane` — 退避ペインを復帰
- `tako_background_list` / `tako_background_kill` — 一覧・破棄

### tmux 管理
- `tako_tmux_list` / `tako_tmux_kill` / `tako_tmux_open` / `tako_tmux_cleanup` など

### git 連携
- `tako_git_log` / `tako_git_diff`

### オーケストレーター
- `tako_orchestrator_spawn` / `tako_orchestrator_run` — worker の起動・ワンショット実行
- `tako_orchestrator_worker_status` / `tako_orchestrator_projects` / `tako_orchestrator_profiles`

## 設計思想: AI フルコントロール

tako の設計原則は「**UI でできることはすべて AI からもできる**」です。新機能を追加するたびに対応する MCP ツールも同時に提供します。

AI エージェントは自分が何をしているかをユーザーに「見せる」ことも重視しています。MCP ツールの説明文には「レビューを求めるときは成果物をプレビューで見せろ」「方針相談は例を作って並べろ」といった行動規範が埋め込まれており、エージェントが自然に画面を活用するよう誘導しています。

## アクセス制御

- MCP サーバーは `localhost` にのみバインド（外部から接続不可）
- Bearer トークン認証
- Origin ヘッダー検証（不正な Web サイトからの呼び出し防止）
- tako の外で起動された Claude Code にはツールを公開しない（stdio ブリッジが 0 ツールで応答）
